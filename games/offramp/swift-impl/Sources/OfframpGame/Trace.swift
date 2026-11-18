// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import StickShift
import simd

// The parity trace: one JSON line of game state per frame, which
// `games/offramp/rust-impl/examples/trace.rs` prints from the Rust demo in exactly
// the same form, so the two can be compared byte for byte (`offramp --trace`).
//
// Each run is the engine over a null context (1280 × 720, scale 1, silent),
// driven one fixed step per frame, as `trace.rs` drives the Rust loop:
// - `smoke`: the `--smoke` run, 120 frames, Space on the first, the 4:3 headless
//   camera.
// - `autopilot`: the recorder's run, 390 frames steered by the `Autopilot` at the
//   recorder's 1280/720 aspect.
// - `long`: 12 000 frames of the same autopilot that also taps Space whenever the
//   game is in GameOver: crashes, replays and four rebases.
//
// A frame's line holds what the Rust demo's crate-visible API shows: the state,
// the player's position, the speed, the scores and the fade; the camera's eye and
// target, and a hash of its view-projection; the streaming and entity counts;
// whether the frame rebased; the audio counters; the 3D draws and a hash of the
// HUD's quads the frame recorded; the autopilot's speed estimate; and the world
// digest. Floats are their bit patterns in hex, so nothing depends on how either
// language prints a float. The game's own messages ("GAME OVER ...", "replay
// ...") are printed where they happen, between the lines, as both programs print
// them to the same stdout.
//
// With `summary` only some lines are printed (every 600th frame, a frame whose
// state changed or that rebased, and the last), then one summary line with the
// FNV-1a 64 of every frame's line: a whole long run compared in a few lines.

/// Which run a trace drives.
public enum TraceMode: String, CaseIterable, Sendable {
    /// The `--smoke` run.
    case smoke
    /// The recorder's autopilot run.
    case autopilot
    /// A long autopilot run that replays after every crash.
    case long

    /// How many frames it drives.
    public var frames: Int {
        switch self {
        case .smoke: SMOKE_FRAMES
        case .autopilot: RecordDemo.frames
        case .long: 12_000
        }
    }
}

/// How the trace names a key (Rust's `{:?}` of `Key`).
func traceName(_ key: Key) -> String {
    switch key {
    case .w: "W"
    case .a: "A"
    case .s: "S"
    case .d: "D"
    case .q: "Q"
    case .e: "E"
    case .left: "Left"
    case .right: "Right"
    case .up: "Up"
    case .down: "Down"
    case .space: "Space"
    case .escape: "Escape"
    }
}

/// `value` as `digits` lowercase hex digits, zero-padded.
private func hex<T: BinaryInteger>(_ value: T, digits: Int) -> String {
    let s = String(value, radix: 16)
    return String(repeating: "0", count: Swift.max(0, digits - s.count)) + s
}

/// A float as its quoted bit pattern: `"3dcccccd"`.
private func json(_ value: Float) -> String {
    "\"" + hex(value.bitPattern, digits: 8) + "\""
}

/// A vector as its three quoted bit patterns.
private func json(_ v: SIMD3<Float>) -> String {
    "[" + json(v.x) + "," + json(v.y) + "," + json(v.z) + "]"
}

/// A 64-bit hash, quoted.
private func json(hash: UInt64) -> String {
    "\"" + hex(hash, digits: 16) + "\""
}

/// FNV-1a 64 of a matrix's sixteen elements, column by column, each its bits
/// little-endian.
func traceHash(_ m: simd_float4x4) -> UInt64 {
    var h = FNV1a64()
    for c in 0..<4 {
        for r in 0..<4 {
            h.eat(m[c][r])
        }
    }
    return h.value
}

/// FNV-1a 64 of overlay quads, in record order: per quad a fill byte (solid 0,
/// textured 1, SDF 2; the texture is not hashed, as Rust's handles and the C
/// API's ids differ), then the bits of its rect, uv and colour, little-endian.
func traceHash(_ quads: [FrameRecord.RecordedQuad]) -> UInt64 {
    var h = FNV1a64()
    for q in quads {
        switch q.fill {
        case .solid: h.eat(UInt8(0))
        case .texture: h.eat(UInt8(1))
        case .sdf: h.eat(UInt8(2))
        case let .other(code): h.eat(UInt8(truncatingIfNeeded: code))
        }
        for v in [q.rect, q.uv, q.color] {
            for i in 0..<4 {
                h.eat(v[i])
            }
        }
    }
    return h.value
}

/// What one frame's trace line reports.
struct TraceRecord {
    var frame: Int
    var tap: Key?
    var state: GameState
    var player: SIMD3<Float>
    var speed: Float
    var score: UInt64
    var best: UInt64
    var fade: Float
    var cameraPosition: SIMD3<Float>
    var cameraTarget: SIMD3<Float>
    var viewProjection: UInt64
    var streamed: Int
    var entities: Int
    var obstacles: Int
    var rebased: Bool
    var loopStarts: Int
    var oneShots: Int
    var draws: UInt64
    var quads: Int
    var hud: UInt64
    /// The autopilot's speed estimate; `nil` in the smoke run.
    var autopilotSpeed: Float?
    var digest: UInt64

    /// The JSON line, keys in a fixed order, no spaces.
    var line: String {
        var s = "{\"frame\":\(frame)"
        s += ",\"tap\":" + (tap.map { "\"" + traceName($0) + "\"" } ?? "null")
        s += ",\"state\":\"\(state)\""
        s += ",\"player\":" + json(player)
        s += ",\"speed\":" + json(speed)
        s += ",\"score\":\(score),\"best\":\(best)"
        s += ",\"fade\":" + json(fade)
        s += ",\"camera\":{\"position\":" + json(cameraPosition) + ",\"target\":" + json(cameraTarget) + "}"
        s += ",\"vp\":" + json(hash: viewProjection)
        s += ",\"streamed\":\(streamed),\"entities\":\(entities),\"obstacles\":\(obstacles)"
        s += ",\"rebased\":\(rebased)"
        s += ",\"loop_starts\":\(loopStarts),\"one_shots\":\(oneShots)"
        s += ",\"draws\":\(draws)"
        s += ",\"hud\":{\"quads\":\(quads),\"fnv\":" + json(hash: hud) + "}"
        if let autopilotSpeed {
            s += ",\"autopilot_speed\":" + json(autopilotSpeed)
        }
        s += ",\"digest\":" + json(hash: digest) + "}"
        return s
    }
}

/// Drive the trace run `mode` and hand each line to `emit`, the game's own
/// messages included where they happen (see the top of this file). With `summary`,
/// only the sampled lines and a closing summary line.
///
/// - Throws: What the game's `initialize` throws (a missing asset).
@MainActor
public func runTrace(_ mode: TraceMode, summary: Bool = false, emit: @escaping (String) -> Void) throws {
    try runTrace(mode, summary: summary, emit: emit, inspect: nil)
}

/// One frame of a trace run, for tests to inspect after the frame was driven.
struct TraceStep {
    /// The frame's number, from 0.
    let frame: Int
    /// The key tapped for it.
    let tap: Key?
    /// The game.
    let game: CarRunner
    /// The engine running it.
    let engine: Engine
    /// What its line reports.
    let record: TraceRecord
}

/// `runTrace`, calling `inspect` after every frame.
@MainActor
func runTrace(
    _ mode: TraceMode,
    summary: Bool,
    emit: @escaping (String) -> Void,
    inspect: ((TraceStep) throws -> Void)?
) throws {
    let game = CarRunner()
    game.output = emit
    // `Loop::new()` and a `NullRenderer` (1280 × 720 at scale 1): Rust's
    // `Headless`, which `Engine.headless` builds.
    let engine = try Engine.headless(game)
    let context = engine.context
    if mode != .smoke {
        engine.camera.aspectRatio = RecordDemo.aspectRatio
    }
    try engine.initialize()

    var autopilot = Autopilot(start: game.playerPosition())
    var held: Key?
    var all = FNV1a64()
    var rebases = 0
    var maxEntities = 0
    for frame in 0..<mode.frames {
        let tap: Key?
        if mode == .smoke {
            // `run_smoke`: Space held for the first frame, then nothing.
            if let key = held {
                engine.input.release(key)
                held = nil
            }
            tap = frame == 0 ? .space : nil
            if let key = tap {
                engine.input.press(key)
                held = key
            }
        } else {
            tap = autopilot.beforeFrame(
                frame,
                engine: engine,
                player: game.playerPosition(),
                gameOver: game.state == .gameOver,
                replayOnGameOver: mode == .long
            )
        }

        let stateBefore = game.state
        let zBefore = game.playerPosition().z
        let drawsBefore = context.stats.drawsSubmitted
        let tick = engine.advance()
        precondition(
            tick.steps == 1 && tick.rendered && tick.submitted && engine.lastRenderError == nil,
            "trace frame \(frame): one update and one rendered, submitted frame "
                + "(\(tick), render error \(String(describing: engine.lastRenderError)))"
        )
        let player = game.playerPosition()
        if mode != .smoke {
            autopilot.afterFrame(playerZ: player.z)
        }
        // A rebase moves the car back by a whole threshold; nothing else moves it
        // toward +z.
        let rebased = player.z - zBefore > 500
        let world = engine.world
        let camera = engine.camera
        let audio = engine.audio.stats
        let quads = context.lastFrame.quads
        let entities = world.count
        let record = TraceRecord(
            frame: frame,
            tap: tap,
            state: game.state,
            player: player,
            speed: game.currentSpeed(),
            score: game.score(),
            best: game.bestScore(),
            fade: game.fadeAlpha(),
            cameraPosition: camera.position,
            cameraTarget: camera.target,
            viewProjection: traceHash(camera.viewProjection),
            streamed: engine.scene.streamedCount,
            entities: entities,
            obstacles: world.query(Obstacle.self).count,
            rebased: rebased,
            loopStarts: audio.loopStarts,
            oneShots: audio.oneShotsPlayed,
            draws: context.stats.drawsSubmitted - drawsBefore,
            quads: quads.count,
            hud: traceHash(quads),
            autopilotSpeed: mode == .smoke ? nil : autopilot.speed,
            digest: worldDigest(world, player: game.player)
        )
        try inspect?(TraceStep(frame: frame, tap: tap, game: game, engine: engine, record: record))
        let line = record.line
        all.eat(line.utf8)
        all.eat(UInt8(ascii: "\n"))
        if rebased {
            rebases += 1
        }
        maxEntities = Swift.max(maxEntities, entities)
        if !summary || frame % 600 == 0 || game.state != stateBefore || rebased || frame == mode.frames - 1 {
            emit(line)
        }
    }
    if summary {
        let audio = engine.audio.stats
        var s = "{\"summary\":{\"mode\":\"\(mode.rawValue)\",\"frames\":\(mode.frames)"
        s += ",\"fnv\":" + json(hash: all.value)
        s += ",\"rebases\":\(rebases),\"max_entities\":\(maxEntities)"
        s += ",\"loop_starts\":\(audio.loopStarts),\"one_shots\":\(audio.oneShotsPlayed)}}"
        emit(s)
    }
}
