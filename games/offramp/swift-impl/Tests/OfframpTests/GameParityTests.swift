// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Foundation
import StickShift
import Testing
import simd

@testable import OfframpGame

/// The real game against values the Rust probe captured that the trace cannot
/// show (spec 02 §7.3): the world `initialize` builds, dumped entity by entity
/// with its hecs ids, and the private state (lane, travel, distance, run clock,
/// fade) of every frame of the smoke run. And the two quirks of the Rust loop the
/// game keeps (contract decision 6), which now come from the engine itself.
@MainActor
@Suite("game parity")
struct GameParityTests {
    @Test func theInitWorldIsTheRustWorld() throws {
        let game = CarRunner()
        let h = try headlessRun(game, frames: 0)
        let world = h.world
        #expect(h.scene.streamedCount == 170 && world.count == 171 && world.query(Obstacle.self).count == 15)
        #expect(worldDigest(world, player: game.player) == 0xEC93_C951_1A2E_730C)
        #expect(game.player == Entity(bits: 1 << 32 | 0), "the player is the first entity: id 0, generation 1")

        // Every entity, in hecs query order, with its id.
        let expected = try worldDump("init_world.txt")
        let actual = dump(world, player: game.player)
        #expect(actual.count == expected.count)
        if let i = firstDifference(actual, expected) {
            Issue.record("entity #\(i): \(actual[i]) is not the Rust \(expected[i])")
        }

        // The header lines: the streaming configuration, the camera, the audio.
        let lines = try fixtureLines("init_world.txt")
        let config = try #require(lines.first { $0.hasPrefix("after init_once") })
        #expect(bits(h.scene.config.axis) == (try tupleBits("axis=", in: config)))
        #expect(bits(h.scene.config.spawnInterval) == (try scalar("spawn_interval=", in: config)).bitPattern)
        #expect(bits(h.scene.config.spawnAhead) == (try scalar("spawn_ahead=", in: config)).bitPattern)
        #expect(bits(h.scene.config.despawnBehind) == (try scalar("despawn_behind=", in: config)).bitPattern)
        #expect(bits(h.scene.config.rebaseThreshold) == (try scalar("rebase_threshold=", in: config)).bitPattern)
        let camera = try #require(lines.first { $0.hasPrefix("camera ") })
        #expect(bits(h.camera.position) == bits(try vector("pos=", in: camera)))
        #expect(bits(h.camera.target) == bits(try vector("target=", in: camera)))
        #expect(bits(h.camera.up) == bits(try vector("up=", in: camera)))
        #expect(camera.hasSuffix("clip=(0.3, 300.0)") && h.camera.clipPlanes == (0.3, 300))
        #expect(lines.contains("audio: sounds=2 master=Volume(-3.0) looping=None"))
        let audio = h.audio.stats
        #expect(audio.soundCount == 2 && audio.masterVolume == Volume(decibels: -3) && audio.looping == nil)

        // What the loop pushed after `initialize`: the default render settings.
        #expect(h.context.settings == GraphicsSettings.default.renderSettings)
        #expect(h.isClean)
    }

    // MARK: - The demo's quirks the loop keeps (contract decision 6)

    @Test func theGamesFirstPassStreamsItsFullReachWhateverTheRestoredDrawDistance() throws {
        // Restored Near settings apply before `initialize` but scale the reach
        // only after it, so its stream pass fills the full 180; the scene then
        // runs at 90 and spawns nothing new until the car passes the old frontier.
        let game = CarRunner()
        let h = try Headless(game)
        h.engine.graphics.request(GraphicsSettings(drawDistance: .near))
        try h.run(frames: 0)
        #expect(h.engine.graphics.current.drawDistance == .near, "the restored settings are in effect")
        #expect(h.scene.streamedCount == 170, "slots 0...30, as at the full reach")
        #expect(h.engine.graphics.baseSpawnAhead == SPAWN_AHEAD)
        #expect(h.scene.config.spawnAhead == 90.0)
        try h.run(frames: 1)
        #expect(h.scene.streamedCount == 170, "nothing streams inside the old frontier")
    }

    @Test func aReplayFrameDrawsTheCarWhereItsLastPlayingStepLeftIt() throws {
        // The replay frame runs the GameOver branch, which resets the lane but
        // writes no transform: only a Playing step does. So the car is drawn from
        // its stored transform, which still holds the crash position, while the
        // game's position (the streaming focus) has moved on.
        let (game, h) = try driveToGameOver()
        // The centre runner crashed in the start lane; stand in for a crash in
        // the left lane, so the stored transform and the reset lane differ.
        let player = try #require(game.player)
        h.world.modify(Transform.self, of: player) { $0.position.x = CarRunner.laneX(0) }
        let stored = try #require(h.world.get(Transform.self, of: player))
        h.input.press(.space)
        try h.run(frames: 1)
        #expect(game.state == .playing && game.lane == START_LANE)
        let drawn = try #require(h.world.get(Transform.self, of: player))
        #expect(bits(drawn.position) == bits(stored.position), "the replay frame wrote no transform")
        #expect(bits(drawn.position.x) != bits(game.playerPosition().x))
        // The next (Playing) step writes it.
        try h.run(frames: 1)
        let moved = try #require(h.world.get(Transform.self, of: player))
        #expect(bits(moved.position) == bits(game.playerPosition()))
    }

    @Test func theHeadlessCameraIsFourByThreeOnASixteenByNineSurface() throws {
        let h = try Headless(CarRunner())
        #expect(h.context.size == (1280, 720))
        #expect(bits(h.camera.aspectRatio) == 0x3FAA_AAAB, "4/3, as Rust's DEFAULT_ASPECT")
        #expect(bits(RecordDemo.aspectRatio) == 0x3FE3_8E39, "the recorder's 1280/720")
    }

    @Test func theSmokeRunsPrivateStateIsRustsOnEveryFrame() throws {
        // `smoke_state.json`: the probe's accessors read the fields `game.rs` keeps
        // private, for the 120 frames of `--smoke` (numbered from 1).
        let frames = try smokeStates()
        #expect(frames.count == SMOKE_FRAMES)
        let game = CarRunner()
        let h = try Headless(game)
        var matched = 0
        for (i, rust) in frames.enumerated() {
            if i == 0 {
                h.input.press(.space)
            } else if i == 1 {
                h.input.release(.space)
            }
            try h.run(frames: 1)
            let swift = SmokeState(
                frame: h.framesRun,
                state: "\(game.state)",
                lane: game.lane,
                travel: bits(game.travel),
                distance: bits(game.distance),
                runTime: bits(game.runTime),
                fadeRemaining: bits(game.fadeRemaining),
                score: Int(game.score()),
                entities: h.world.count,
                streamed: h.scene.streamedCount,
                obstacles: h.world.query(Obstacle.self).count,
                cameraPosition: bits(h.camera.position),
                cameraTarget: bits(h.camera.target)
            )
            guard swift == rust else {
                Issue.record("frame \(rust.frame):\n  swift: \(swift)\n  rust:  \(rust)")
                break
            }
            matched += 1
        }
        #expect(matched == SMOKE_FRAMES)
    }

    /// One frame of `smoke_state.json`, floats as bits.
    private struct SmokeState: Equatable {
        let frame: Int
        let state: String
        let lane: Int
        let travel: UInt32
        let distance: UInt32
        let runTime: UInt32
        let fadeRemaining: UInt32
        let score: Int
        let entities: Int
        let streamed: Int
        let obstacles: Int
        let cameraPosition: [UInt32]
        let cameraTarget: [UInt32]
    }

    /// The frames of `smoke_state.json`, one object per line. The probe printed
    /// each float in its shortest round-trip form, so parsing the text as `Float`
    /// gives back the exact value (going through `Double` could round twice).
    private func smokeStates() throws -> [SmokeState] {
        try fixtureLines("smoke_state.json").filter { $0.contains("\"frame\"") }.map { line in
            func raw(_ key: String) throws -> Substring {
                try after("\"\(key)\": ", in: line).prefix { $0 != "," && $0 != "}" }
            }
            func float(_ key: String) throws -> UInt32 {
                let text = try raw(key)
                guard let value = Float(text) else { throw FixtureError(description: "\(key) \(text) in: \(line)") }
                return value.bitPattern
            }
            func int(_ key: String) throws -> Int {
                let text = try raw(key)
                guard let value = Int(text) else { throw FixtureError(description: "\(key) \(text) in: \(line)") }
                return value
            }
            func floats(_ key: String) throws -> [UInt32] {
                let list = try after("\"\(key)\": [", in: line).prefix { $0 != "]" }
                return try list.split(separator: ",").map { item in
                    let text = item.trimmingCharacters(in: .whitespaces)
                    guard let value = Float(text) else { throw FixtureError(description: "\(key) \(text) in: \(line)") }
                    return value.bitPattern
                }
            }
            func hexBits(_ key: String) throws -> UInt32 {
                let text = try raw(key).trimmingCharacters(in: CharacterSet(charactersIn: "\"")).dropFirst(2)
                guard let value = UInt32(text, radix: 16) else { throw FixtureError(description: "\(key) in: \(line)") }
                return value
            }
            let travel = try hexBits("travel_bits")
            #expect(travel == (try float("travel")), "the probe's travel and its bits agree")
            return SmokeState(
                frame: try int("frame"),
                state: String(try raw("state").trimmingCharacters(in: CharacterSet(charactersIn: "\""))),
                lane: try int("lane"),
                travel: travel,
                distance: try hexBits("distance_bits"),
                runTime: try float("run_time"),
                fadeRemaining: try float("fade_remaining"),
                score: try int("score"),
                entities: try int("entities"),
                streamed: try int("streamed"),
                obstacles: try int("obstacles"),
                cameraPosition: try floats("camera_position"),
                cameraTarget: try floats("camera_target")
            )
        }
    }
}
