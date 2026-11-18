// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Foundation
import StickShift
import Testing
import simd

@testable import OfframpGame

/// The game against the Rust demo, frame by frame.
///
/// Two independent sources are compared:
/// - `trace_*.jsonl`, printed by `games/offramp/rust-impl/examples/trace.rs` from the
///   Rust demo: the Swift trace must be the same bytes, line for line, the game's
///   own messages included.
/// - The probe's traces (`smoke_trace.txt`, `record_trace.txt`,
///   `long_run_events.txt`) and dumps, printed by `offramp-goldens`
///   (`games/offramp/rust-impl/goldens`) from the same Rust modules: the real
///   `CarRunner` must reproduce every frame's state,
///   player, speed, scores, fade, camera bits, counts and world digest.
///
/// Set `OFFRAMP_TRACE_OUT=<dir>` to write the Swift traces there (the directory
/// is created): `swift_smoke.jsonl` and `swift_autopilot.jsonl`, every line, for
/// a diff against `trace.rs`'s output, and `swift_long_summary.jsonl`, the long
/// run's summary. A whole long run is diffed with `offramp --trace long`, as
/// `scripts/offramp-parity.sh` does.
@MainActor
@Suite("trace parity")
struct TraceTests {
    /// Run `mode`'s trace, collecting its lines; `inspect` sees every frame.
    private func swiftTrace(
        _ mode: TraceMode,
        summary: Bool = false,
        inspect: ((TraceStep) throws -> Void)? = nil
    ) throws -> [String] {
        var lines: [String] = []
        try runTrace(mode, summary: summary, emit: { lines.append($0) }, inspect: inspect)
        if let dir = ProcessInfo.processInfo.environment["OFFRAMP_TRACE_OUT"] {
            let directory = URL(fileURLWithPath: dir)
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            let name = "swift_\(mode.rawValue)\(summary ? "_summary" : "").jsonl"
            let text = lines.map { $0 + "\n" }.joined()
            try text.write(to: directory.appendingPathComponent(name), atomically: true, encoding: .utf8)
        }
        return lines
    }

    /// The Swift lines equal fixture `name`'s, line for line.
    private func expectSameLines(_ swift: [String], _ name: String) throws {
        let rust = try exactFixtureLines(name)
        #expect(swift.count == rust.count, "\(swift.count) lines, trace.rs printed \(rust.count)")
        if let i = firstDifference(swift, rust) {
            Issue.record("line \(i + 1) differs from trace.rs:\n  swift: \(swift[i])\n  rust:  \(rust[i])")
        }
    }

    /// The real game's frame `step` reproduces the probe's `probe` frame.
    @discardableResult
    private func expectProbe(_ probe: TraceFrame, _ step: TraceStep) -> Bool {
        let r = step.record
        let tap = step.tap.map(traceName) ?? "-"
        let same =
            probe.frame == step.frame && probe.tap == tap && probe.state == "\(r.state)"
            && bits(probe.player) == bits(r.player) && bits(probe.speed) == bits(r.speed)
            && probe.score == Int(r.score) && probe.best == Int(r.best) && bits(probe.fade) == bits(r.fade)
            && bits(probe.cameraPosition) == bits(r.cameraPosition) && bits(probe.cameraTarget) == bits(r.cameraTarget)
            && probe.streamed == r.streamed && probe.entities == r.entities && probe.obstacles == r.obstacles
            && probe.digest == r.digest
        if !same {
            Issue.record(
                """
                frame \(step.frame) is not the Rust frame:
                  swift: tap \(tap) \(r.state) player \(bits(r.player)) speed \(bits(r.speed)) score \(r.score) \
                best \(r.best) fade \(bits(r.fade)) cam \(bits(r.cameraPosition)) \(bits(r.cameraTarget)) \
                counts \(r.streamed)/\(r.entities)/\(r.obstacles) digest \(String(r.digest, radix: 16))
                  rust:  tap \(probe.tap) \(probe.state) player \(bits(probe.player)) speed \(bits(probe.speed)) \
                score \(probe.score) best \(probe.best) fade \(bits(probe.fade)) cam \(bits(probe.cameraPosition)) \
                \(bits(probe.cameraTarget)) counts \(probe.streamed)/\(probe.entities)/\(probe.obstacles) \
                digest \(String(probe.digest, radix: 16))
                """
            )
        }
        return same
    }

    // MARK: - Against trace.rs

    @Test func theSmokeTraceIsTheRustTrace() throws {
        var firstGameOver: Int?
        let lines = try swiftTrace(.smoke) { step in
            if firstGameOver == nil && step.game.state == .gameOver {
                firstGameOver = step.frame
            }
        }
        try expectSameLines(lines, "trace_smoke.jsonl")

        // The centre runner crashes on the update of frame 98 (the 99th), with
        // score 23, and the game's report comes just before that frame's line.
        #expect(firstGameOver == 98)
        let report = try #require(lines.firstIndex { !$0.hasPrefix("{") })
        #expect(lines[report] == "GAME OVER — score 23 — best 23. Press Space to replay.")
        #expect(lines[report + 1].hasPrefix("{\"frame\":98,"))
        #expect(lines[report + 1].contains("\"state\":\"GameOver\""))
        #expect(lines[report + 1].contains("\"score\":23,\"best\":23"))
        #expect(lines.count == SMOKE_FRAMES + 1, "120 frames and the one report")
    }

    @Test func theAutopilotTraceIsTheRustTrace() throws {
        let probe = try trace("record_trace.txt")
        #expect(probe.count == RecordDemo.frames)
        let probeLines = try fixtureLines("record_trace.txt").filter { $0.hasPrefix("f=") }
        let projections = try viewProjections()
        let frame150 = try worldDump("record_frame150_world.txt")
        var taps: [Int: Key] = [:]
        var matched = 0
        let lines = try swiftTrace(.autopilot) { step in
            if let tap = step.tap {
                taps[step.frame] = tap
            }
            if expectProbe(probe[step.frame], step) {
                matched += 1
            }
            // The autopilot's own speed estimate.
            let autopilotSpeed = try scalar("autopilot.speed=", in: probeLines[step.frame])
            #expect(bits(step.record.autopilotSpeed ?? .nan) == bits(autopilotSpeed), "frame \(step.frame)")
            // The view-projection the frame was drawn with.
            if let vp = projections[step.frame] {
                #expect(bits(step.engine.camera.viewProjection) == vp, "frame \(step.frame)'s view-projection")
            }
            // The whole world at frame 150, ids and swap-remove order included.
            if step.frame == 150 {
                let actual = dump(step.engine.world, player: step.game.player)
                #expect(actual.count == frame150.count)
                if let i = firstDifference(actual, frame150) {
                    Issue.record("frame 150 entity #\(i): \(actual[i]) is not the Rust \(frame150[i])")
                }
            }
        }
        try expectSameLines(lines, "trace_autopilot.jsonl")
        #expect(matched == RecordDemo.frames)
        // record_demo's golden taps: Space to start, then three lane changes, and
        // no crash.
        #expect(taps == [0: .space, 55: .right, 154: .left, 342: .right])
        #expect(!lines.contains { $0.contains("GameOver") }, "the recorder's run never crashes")
    }

    @Test(.timeLimit(.minutes(10))) func theLongRunIsTheRustLongRun() throws {
        // The probe's sampled frames, with their REBASE and TRANSITION notes.
        let events = try fixtureLines("long_run_events.txt")
        let probe = try events.filter { $0.hasPrefix("f=") }.map(TraceFrame.init)
        let byFrame = Dictionary(uniqueKeysWithValues: probe.map { ($0.frame, $0) })
        var rebases: [Int] = []
        var transitions: [String] = []
        var previous = GameState.ready
        var maxEntities = 0
        var checked = 0
        let lines = try swiftTrace(.long, summary: true) { step in
            if step.record.rebased {
                rebases.append(step.frame)
            }
            if step.game.state != previous {
                transitions.append("\(step.frame) \(previous)->\(step.game.state)")
            }
            previous = step.game.state
            maxEntities = max(maxEntities, step.record.entities)
            if let p = byFrame[step.frame] {
                expectProbe(p, step)
                checked += 1
            }
        }
        try expectSameLines(lines, "trace_long_summary.jsonl")

        #expect(checked == probe.count)
        // Four rebases, the first on frame 2705 at score 1000.
        #expect(rebases == [2705, 5404, 8066, 10239])
        #expect(byFrame[2705]?.score == 1000)
        let notes = try events.compactMap { line -> String? in
            guard let r = line.range(of: " TRANSITION ") else { return nil }
            return "\(try integer("f=", in: line)) \(line[r.upperBound...])"
        }
        #expect(transitions == notes, "the crashes and replays fall on the probe's frames")
        let last = try #require(events.last)
        #expect(last == "max entities=\(maxEntities) loop_starts=1 one_shots=3")
    }

    // MARK: - Against the probe's smoke run

    @Test func theSmokeRunMatchesTheProbeOnEveryFrame() throws {
        // 900 frames in the centre lane, Space at frame 0 and on every GameOver
        // frame divisible by 60: crashes, frozen frames and replays.
        let probe = try trace("smoke_trace.txt")
        #expect(probe.count == 900)
        let game = CarRunner()
        let h = try Headless(game)
        try h.run(frames: 0)
        var held: Key?
        var matched = 0
        for frame in 0..<probe.count {
            if let key = held {
                h.input.release(key)
                held = nil
            }
            let tap: Key? = frame == 0 || (game.state == .gameOver && frame % 60 == 0) ? .space : nil
            if let key = tap {
                h.input.press(key)
                held = key
            }
            try h.run(frames: 1)
            let step = TraceStep(frame: frame, tap: tap, game: game, engine: h.engine, record: snapshot(frame, game, h))
            guard expectProbe(probe[frame], step) else { break }
            matched += 1
        }
        #expect(matched == 900)
        #expect(h.isClean, "\(h.lostFrames.prefix(5))")
        let audio = try #require(try fixtureLines("smoke_trace.txt").last)
        let stats = h.audio.stats
        #expect(audio == "audio: loop_starts=\(stats.loopStarts) one_shots=\(stats.oneShotsPlayed)")
    }

    /// The probe-comparable part of a frame's state.
    private func snapshot(_ frame: Int, _ game: CarRunner, _ h: Headless) -> TraceRecord {
        TraceRecord(
            frame: frame, tap: nil, state: game.state, player: game.playerPosition(), speed: game.currentSpeed(),
            score: game.score(), best: game.bestScore(), fade: game.fadeAlpha(), cameraPosition: h.camera.position,
            cameraTarget: h.camera.target, viewProjection: 0, streamed: h.scene.streamedCount,
            entities: h.world.count, obstacles: h.world.query(Obstacle.self).count, rebased: false, loopStarts: 0,
            oneShots: 0, draws: 0, quads: 0, hud: 0, autopilotSpeed: nil, digest: worldDigest(h.world, player: game.player)
        )
    }

    /// `record_view_projection.txt`: frame number to the sixteen bits.
    private func viewProjections() throws -> [Int: [UInt32]] {
        var out: [Int: [UInt32]] = [:]
        var current: Int?
        for line in try fixtureLines("record_view_projection.txt") {
            if line.hasPrefix("frame ") {
                current = try integer("frame ", in: line)
                out[current!] = []
            } else if let c = current {
                out[c]! += bitPatterns(in: Substring(line))
            }
        }
        return out
    }

    // MARK: - The trace's own form

    @Test func aTraceLineHasTheRustForm() {
        let record = TraceRecord(
            frame: 7, tap: .left, state: .gameOver, player: SIMD3(0, 0.1, -0.0), speed: 14, score: 3, best: 9,
            fade: 0, cameraPosition: SIMD3(1, 2, 3), cameraTarget: SIMD3(-1, -2, -3), viewProjection: 0xAB,
            streamed: 170, entities: 171, obstacles: 15, rebased: true, loopStarts: 1, oneShots: 2, draws: 563,
            quads: 7, hud: 0x1234_5678_9ABC_DEF0, autopilotSpeed: 45, digest: 0xEC93_C951_1A2E_730C
        )
        #expect(
            record.line
                == """
                {"frame":7,"tap":"Left","state":"GameOver","player":["00000000","3dcccccd","80000000"],\
                "speed":"41600000","score":3,"best":9,"fade":"00000000","camera":{"position":["3f800000",\
                "40000000","40400000"],"target":["bf800000","c0000000","c0400000"]},"vp":"00000000000000ab",\
                "streamed":170,"entities":171,"obstacles":15,"rebased":true,"loop_starts":1,"one_shots":2,\
                "draws":563,"hud":{"quads":7,"fnv":"123456789abcdef0"},"autopilot_speed":"42340000",\
                "digest":"ec93c9511a2e730c"}
                """
        )
        var smoke = record
        smoke.tap = nil
        smoke.autopilotSpeed = nil
        #expect(smoke.line.contains("\"tap\":null"))
        #expect(!smoke.line.contains("autopilot_speed"))
        #expect(TraceMode.allCases.map(\.frames) == [120, 390, 12_000])
    }

    @Test func theHashesAreFNV1aOverLittleEndianBits() throws {
        // An empty input hashes to the offset basis.
        #expect(traceHash([]) == 0xCBF2_9CE4_8422_2325)
        var h = FNV1a64()
        h.eat(UInt8(0))
        for v: Float in [0, 0, 1280, 720, 0, 0, 1, 1, 0, 0, 0, 1] {
            h.eat(v)
        }
        // The wash, as the engine records it.
        let context = try nullContext()
        let wash = try recordedFrame(context) { try $0.draw(Quad(rect: SIMD4(0, 0, 1280, 720), color: SIMD4(0, 0, 0, 1))) }
        #expect(wash.quads.count == 1)
        #expect(traceHash(wash.quads) == h.value)
        var m = FNV1a64()
        for c in 0..<4 {
            for r in 0..<4 {
                m.eat(matrix_identity_float4x4[c][r])
            }
        }
        #expect(traceHash(matrix_identity_float4x4) == m.value)
    }
}

/// Every line of fixture `name`, empty ones kept, without the final newline's
/// empty tail.
func exactFixtureLines(_ name: String) throws -> [String] {
    var lines = try fixture(name).split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
    if lines.last == "" {
        lines.removeLast()
    }
    return lines
}
