// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Foundation
import OfframpGame
import StickShift

// `offramp --record <dir>`: a port of `games/offramp/rust-impl/examples/record_demo.rs`.
// The game runs on an engine offscreen at 1280×720 on Metal, steered by the
// autopilot (`RecordDemo.record`), and every third frame after the 150-frame
// warm-up is
// written as `frame_0000.png`, `frame_0001.png`, ..., the names and cadence
// `record_demo` uses, so `scripts/offramp-parity.sh` can compare the two runs
// frame by frame.

/// A directory the recorder could not make.
struct RecorderError: Error, CustomStringConvertible {
    let description: String
}

/// The name of written frame `number`: `frame_0042.png`.
func frameName(_ number: Int) -> String {
    let digits = String(number)
    return "frame_" + String(repeating: "0", count: max(0, 4 - digits.count)) + digits + ".png"
}

/// Record `frames` frames into `directory` (created if needed), then print what
/// was written, as `record_demo` does.
@MainActor
func record(to directory: String, frames: Int) throws {
    let out = URL(fileURLWithPath: directory)
    do {
        try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
    } catch {
        throw RecorderError(description: "cannot create the output directory \(directory): \(error.localizedDescription)")
    }
    let context = try Context(offscreenWidth: RecordDemo.width, height: RecordDemo.height)
    let recording = try RecordDemo.record(context: context, frames: frames) { number, rgba in
        try writePNG(rgba, width: RecordDemo.width, height: RecordDemo.height, to: out.appendingPathComponent(frameName(number)))
    }
    print("wrote \(recording.written) frames (\(recording.laneChanges) lane changes) to \(directory)")
}
