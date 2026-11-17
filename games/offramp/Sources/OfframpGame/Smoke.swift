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

/// A smoke run that did not do what Rust's does, where the Rust oracle panics:
/// a frame that did not run one update and render a submitted frame without an
/// error, a skipped draw or frame, or the wrong frame count.
public struct SmokeFailure: Error, CustomStringConvertible {
    public let description: String
}

/// Drive `frames` headless frames of the game, Space tapped on the first, then
/// print `smokeReport(frames)`, as `main.rs:161-185` does on Rust's `Headless`:
/// the engine over a null context of 1280 × 720 at scale 1, silent, one fixed
/// step per frame. The game's own messages and the report all go to `output`
/// (stdout by default), in order.
///
/// - Throws: What the game's `initialize` throws (a missing asset), and
///   ``SmokeFailure`` for a run that went wrong (see ``driveSmoke(_:frames:)``).
@MainActor
public func runSmoke(frames: Int, output: @escaping (String) -> Void = { print($0) }) throws {
    let game = CarRunner()
    game.output = output
    try driveSmoke(Engine.headless(game), frames: frames)
    output(smokeReport(frames))
}

/// The smoke run's frames on `engine`, a tick at a time: Space pressed for the
/// first frame, then released. The demo opens frozen on its title screen, so the
/// tap is what starts the run (before `initialize`, as `main.rs:172` presses it);
/// otherwise the run would be `frames` frames of a still title.
///
/// Every tick must run one update and render a frame that is submitted, with no
/// render error, and the run must skip no draw and no frame and end after
/// exactly `frames` frames. Where Rust's oracle would panic (a draw that fails,
/// the wrong frame count), this throws ``SmokeFailure`` naming the frame, so the
/// smoke run never prints its report over a failed frame.
@MainActor
func driveSmoke(_ engine: Engine, frames: Int) throws {
    engine.input.press(.space)
    try engine.initialize()
    for frame in 0..<max(0, frames) {
        if engine.state.exitRequested { break }
        let tick = engine.advance()
        if tick.isPoisoned {
            throw SmokeFailure(
                description: "smoke frame \(frame): a panic (a bug in Stick Shift Engine) poisoned the engine"
            )
        }
        guard tick.steps == 1, tick.rendered, tick.submitted, engine.lastRenderError == nil else {
            let error = engine.lastRenderError.map { "\($0)" } ?? "none"
            throw SmokeFailure(
                description: "smoke frame \(frame): not one update and one rendered, submitted frame "
                    + "(\(tick), render error \(error))"
            )
        }
        if frame == 0 {
            engine.input.release(.space)
        }
    }
    guard engine.state.frames == UInt64(max(0, frames)) else {
        throw SmokeFailure(description: "driver ran the wrong frame count")
    }
    let stats = engine.context.stats
    guard stats.drawsSkipped == 0, stats.framesSkipped == 0 else {
        throw SmokeFailure(description: "the smoke run skipped draws or frames (\(stats))")
    }
}
