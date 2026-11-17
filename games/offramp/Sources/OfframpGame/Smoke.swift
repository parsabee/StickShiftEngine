// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

// The headless smoke oracle, a port of `run_smoke` in
// `games/playable-demo/src/main.rs` (spec 02 §5): boot the game headlessly, tap
// Space on the first frame, drive the rest with no input (the car runs straight
// down the centre lane, a fully determined run), and report.
//
// Its stdout is the Rust demo's, byte for byte: the game's own GAME OVER line
// (printed during frame 99's update), then the contract line.

import StickShift

/// The frames the smoke oracle drives.
public let SMOKE_FRAMES = 120

/// The line the smoke oracle prints on success.
public func smokeReport(_ frames: Int) -> String {
    "smoke: \(frames) frames OK"
}

/// Drive `frames` headless frames of the game, Space tapped on the first, then
/// print `smokeReport(frames)`, as `main.rs:161-185` does on Rust's `Headless`:
/// the engine over a null context of 1280 × 720 at scale 1, silent, one fixed
/// step per frame. The game's own messages and the report all go to `output`
/// (stdout by default), in order.
///
/// A frame count other than `frames` traps with Rust's panic message.
///
/// - Throws: What the game's `initialize` throws (a missing asset).
@MainActor
public func runSmoke(frames: Int, output: @escaping (String) -> Void = { print($0) }) throws {
    let game = CarRunner()
    game.output = output
    let engine = try Engine.headless(game)

    // The demo opens frozen on its title screen, so tap the start key on the
    // first frame (before `run`'s init, as `main.rs:172` presses it); otherwise
    // the run would be 120 frames of a still title.
    engine.input.press(.space)
    try engine.runHeadless(frames: 1)
    engine.input.release(.space)
    try engine.runHeadless(frames: frames - 1)

    guard engine.state.frames == UInt64(frames) else {
        fatalError("driver ran the wrong frame count")
    }
    output(smokeReport(frames))
}
