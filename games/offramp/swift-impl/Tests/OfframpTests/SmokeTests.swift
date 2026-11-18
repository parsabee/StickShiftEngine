// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import StickShift
import Testing

@testable import OfframpGame

/// `main.rs`'s tests of the smoke oracle, on the `runSmoke` the executable runs.
@MainActor
@Suite("smoke")
struct SmokeTests {
    @Test("smoke_runs_without_panicking") func smokeRunsWithoutPanicking() throws {
        // The oracle completes the fixed frame count without a GPU.
        var lines: [String] = []
        try runSmoke(frames: SMOKE_FRAMES) { lines.append($0) }

        // Golden: the Rust demo's `--smoke` stdout, byte for byte: the game's own
        // report from frame 99's update, then the contract line.
        try #require(lines == ["GAME OVER — score 23 — best 23. Press Space to replay.", "smoke: 120 frames OK"])
        #expect(Array(lines[0].utf8)[10...12] == [0xE2, 0x80, 0x94], "an em dash, U+2014")
    }

    @Test("smoke_prints_the_expected_contract_line") func smokePrintsTheExpectedContractLine() {
        // Guards the exact `--smoke` stdout contract.
        #expect(smokeReport(SMOKE_FRAMES) == "smoke: 120 frames OK")
    }

    @Test func theSmokeRunTapsSpaceForTheFirstFrameOnly() throws {
        let game = Hooks()
        try driveSmoke(Engine.headless(game), frames: 5)
        #expect(game.updates == 5 && game.renders == 5)
        #expect(game.spaceDown == [true, false, false, false, false], "held for frame 0, then released")
    }

    @Test func aFrameWhoseRenderThrowsFailsTheSmokeRun() throws {
        // Where Rust's oracle panics on a failing draw, the Swift one must not
        // print its OK line: the engine keeps a render error and goes on, so the
        // driver checks every tick.
        let game = Hooks()
        game.throwsFromRender = 3
        let engine = try Engine.headless(game)
        let error = try #require(throws: SmokeFailure.self) { try driveSmoke(engine, frames: 10) }
        #expect(error.description.hasPrefix("smoke frame 3: not one update and one rendered, submitted frame"))
        #expect(error.description.contains("render error RenderFailed()"))
        #expect(engine.state.frames == 4, "it stops at the first failed frame")
    }

    @Test func aRunOfTheWrongLengthFailsTheSmokeRun() throws {
        // The game asks to exit on its third update: the run ends early.
        let game = Hooks()
        game.exitsOnUpdate = 3
        let error = try #require(throws: SmokeFailure.self) { try driveSmoke(Engine.headless(game), frames: 10) }
        #expect(error.description == "driver ran the wrong frame count")
        #expect(game.updates == 3)
    }
}

/// A game that counts its hooks, records whether Space is down in each update,
/// and fails on demand.
@MainActor
private final class Hooks: Game {
    var updates = 0
    var renders = 0
    var spaceDown: [Bool] = []
    /// The 0-based render that throws, and every one after it.
    var throwsFromRender: Int?
    /// The update (counting from 1) that asks to exit.
    var exitsOnUpdate: Int?

    struct RenderFailed: Error {}

    func initialize(_ ctx: EngineContext) throws {}

    func update(_ ctx: EngineContext, dt: Float) {
        updates += 1
        spaceDown.append(ctx.input.isKeyDown(.space))
        if updates == exitsOnUpdate {
            ctx.requestExit()
        }
    }

    func render(_ ctx: EngineContext, into frame: Frame) throws {
        defer { renders += 1 }
        if let first = throwsFromRender, renders >= first {
            throw RenderFailed()
        }
    }
}
