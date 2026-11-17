// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Foundation
import StickShift
import Testing

@testable import OfframpGame

// The recorder (`RecordDemo.record`, what `offramp --record` runs): the
// cadence, and its first frame against the frame rebuilt from the Rust dumps.
// Whether that frame is the Rust recorder's `frame_0000.png` pixel for pixel is
// checked by `scripts/offramp-parity.sh`, which compares all 80 frames, and here
// only when `OFFRAMP_RUST_FRAMES` names the Rust recorder's directory: the
// tests that need it are reported as skipped otherwise.

@MainActor
@Suite("recorder")
struct RecorderTests {
    /// Frame 150 as `RenderMetalTests` draws it: the Rust dump's world, camera and
    /// HUD on a fresh Metal context with no engine, as RGBA8.
    private func rebuiltFrame150() throws -> [UInt8]? {
        guard let context = try metalContext(width: RecordDemo.width, height: RecordDemo.height) else { return nil }
        let resources = try loadResources(context)
        let font = try loadHudFont(context)
        let frame = try recorderFrame150()
        try context.setSunSky(DEMO_SUN)
        try context.setCamera(frame.camera)
        let list = frame.drawList
        return try framePixels(context) { f in
            try recordScene(list, resources: resources, frame: f)
            try drawHud(frame.hud, font: font, frame: f)
        }
    }

    @Test func theRecorderWritesEveryThirdFrameAfterTheWarmUp() throws {
        guard let context = try metalContext(width: RecordDemo.width, height: RecordDemo.height) else { return }
        var numbers: [Int] = []
        var frames: [[UInt8]] = []
        let recording = try RecordDemo.record(context: context, frames: 2) { number, rgba in
            numbers.append(number)
            frames.append(rgba)
        }
        // 150 warm-up frames, then frames 150 and 153 written and 154 and 155
        // simulated; the autopilot's tap on frame 154 (Left) is the one lane
        // change in that window.
        #expect(recording == RecordDemo.Recording(simulated: 156, written: 2, laneChanges: 1))
        #expect(numbers == [0, 1])
        #expect(frames.allSatisfy { $0.count == RecordDemo.width * RecordDemo.height * 4 })
        // Every frame was rendered, none skipped, nothing refused.
        #expect(context.stats.framesSubmitted == 156)
        #expect(context.stats.framesSkipped == 0)
        #expect(context.stats.drawsSkipped == 0)
        #expect(frames[0] != frames[1], "the run moves on between written frames")

        // An opaque frame: what a PNG keeps exactly.
        #expect(stride(from: 3, to: frames[0].count, by: 4).allSatisfy { frames[0][$0] == 255 })

        // The recorder's frame 150, driven through the whole loop, is the frame
        // rebuilt from the Rust dumps.
        let rebuilt = try #require(try rebuiltFrame150())
        #expect(frames[0] == rebuilt, "the recorder's first frame is frame 150 as Rust drew it")

        // As a PNG, and back.
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("offramp-recorder-\(UUID().uuidString).png")
        defer { try? FileManager.default.removeItem(at: url) }
        try writePNG(frames[0], width: RecordDemo.width, height: RecordDemo.height, to: url)
        let back = try readPNG(url)
        #expect(back.width == RecordDemo.width && back.height == RecordDemo.height)
        #expect(back.rgba == frames[0], "the PNG holds the frame's samples exactly")
    }

    @Test(.enabled(if: rustFramesDirectory != nil, "set OFFRAMP_RUST_FRAMES to a record_demo directory"))
    func theFirstTwoFramesAreTheRustRecordersPixelForPixel() throws {
        guard let context = try metalContext(width: RecordDemo.width, height: RecordDemo.height) else { return }
        var frames: [[UInt8]] = []
        _ = try RecordDemo.record(context: context, frames: 2) { _, rgba in frames.append(rgba) }
        let dir = try #require(rustFramesDirectory)
        #expect(frames.count == 2)
        for (i, frame) in frames.enumerated() {
            let rust = try readPNG(URL(fileURLWithPath: dir).appendingPathComponent(String(format: "frame_%04d.png", i)))
            #expect(rust.rgba == frame, "frame_\(i) is the Rust recorder's")
        }
    }

    @Test func aPNGKeepsItsSamples() throws {
        // Every value in every channel, alpha opaque.
        let width = 64
        let height = 48
        var rgba = [UInt8](repeating: 255, count: width * height * 4)
        for i in 0..<(width * height) {
            rgba[i * 4] = UInt8(truncatingIfNeeded: i)
            rgba[i * 4 + 1] = UInt8(truncatingIfNeeded: i * 7 + 3)
            rgba[i * 4 + 2] = UInt8(truncatingIfNeeded: i / 3)
        }
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("offramp-png-\(UUID().uuidString).png")
        defer { try? FileManager.default.removeItem(at: url) }
        try writePNG(rgba, width: width, height: height, to: url)
        let back = try readPNG(url)
        #expect(back.width == width && back.height == height)
        #expect(back.rgba == rgba)
        #expect(throws: PNGError.self) { try writePNG([0, 0, 0], width: 1, height: 1, to: url) }
        #expect(throws: PNGError.self) { try readPNG(URL(fileURLWithPath: "/nonexistent.png")) }
    }
}
