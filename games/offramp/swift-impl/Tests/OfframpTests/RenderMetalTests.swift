// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Foundation
import ImageIO
import StickShift
import Testing
import UniformTypeIdentifiers
import simd

@testable import OfframpGame

/// The render pass on a real GPU: the recorder's frame 150, rebuilt from the Rust
/// dumps, drawn into an offscreen Metal context.
///
/// The comparison with the Rust demo's own `frame_0000.png`, pixel for pixel
/// (same GPU, same shader build), runs only when `OFFRAMP_RUST_FRAMES` names a
/// directory written by
/// `cargo run --release -p playable-demo --example record_demo -- <dir>`, and is
/// reported as skipped otherwise; `scripts/offramp-parity.sh` compares all 80
/// frames either way. `OFFRAMP_SAVE_FRAMES` names a directory to write the Swift
/// frame to, for a look.
///
/// Without a Metal device the tests skip, unless `STICKSHIFT_REQUIRE_GPU=1`.
@MainActor
@Suite("render on Metal")
struct RenderMetalTests {
    private static let width = 1280
    private static let height = 720

    /// A Metal context with the demo's resources and font, or `nil` without a
    /// device.
    private func metalRenderer() throws -> (Context, RenderResources, HudFont)? {
        guard let context = try metalContext(width: Self.width, height: Self.height) else { return nil }
        return (context, try loadResources(context), try loadHudFont(context))
    }

    /// Record one frame with `record`, read it back, and return it as RGBA8.
    private func pixels(_ context: Context, _ record: (Frame) throws -> Void) throws -> [UInt8] {
        try framePixels(context, record)
    }

    /// The RGB of pixel (x, y).
    private func rgb(_ image: [UInt8], _ x: Int, _ y: Int) -> SIMD3<UInt8> {
        let i = (y * Self.width + x) * 4
        return SIMD3(image[i], image[i + 1], image[i + 2])
    }

    /// Frame 150 rebuilt from the Rust dumps: its sun and camera set on
    /// `context`, and the draw that records its scene and HUD.
    private func frame150(
        _ context: Context,
        _ resources: RenderResources,
        _ font: HudFont
    ) throws -> (frame: RecorderFrame, draw: (Frame) throws -> Void) {
        let frame = try recorderFrame150()
        try context.setSunSky(DEMO_SUN)
        try context.setCamera(frame.camera)
        let list = frame.drawList
        return (frame, { f in
            try recordScene(list, resources: resources, frame: f)
            try drawHud(frame.hud, font: font, frame: f)
        })
    }

    @Test func theRecordersFrame150RendersDeterministicallyAndPlausibly() throws {
        guard let (context, resources, font) = try metalRenderer() else { return }
        let (frame, draw) = try frame150(context, resources, font)
        let image = try pixels(context, draw)
        #expect(context.stats.drawsSkipped == 0 && context.stats.framesSkipped == 0)
        #expect(image.count == Self.width * Self.height * 4)

        if let dir = ProcessInfo.processInfo.environment["OFFRAMP_SAVE_FRAMES"] {
            try writePNG(image, width: Self.width, height: Self.height, to: URL(fileURLWithPath: dir).appendingPathComponent("offramp_frame150.png"))
        }

        // The same frame again is the same pixels: the sticky camera and sun are
        // applied again when the frame opens.
        #expect(try pixels(context, draw) == image, "rendering is deterministic")
        // A camera set while the frame is open applies to that frame.
        try context.setCamera(Camera(viewProjection: matrix_identity_float4x4, position: .zero))
        let moved = try pixels(context) { f in
            try f.setCamera(frame.camera)
            try draw(f)
        }
        #expect(moved == image, "the camera pushed during the frame is the one it is seen from")

        // A plausible frame: sky at the top, the road below, the score's glyphs
        // bright in the top-left, and no wash (the fade is long gone).
        let sky = rgb(image, Self.width / 2, 5)
        #expect(sky.z > sky.x, "the sky is bluer than it is red: \(sky)")
        let road = rgb(image, Self.width / 2, Self.height - 20)
        #expect(road != SIMD3(0, 0, 0), "the road is drawn, not washed out")
        var brightInCorner = 0
        for y in 28..<60 {
            for x in 20..<170 where rgb(image, x, y).min() > 200 {
                brightInCorner += 1
            }
        }
        #expect(brightInCorner > 100, "\"SCORE 35\" is drawn in the top-left corner")
    }

    @Test(.enabled(if: rustFramesDirectory != nil, "set OFFRAMP_RUST_FRAMES to a record_demo directory"))
    func theRecordersFrame150IsTheRustFramePixelForPixel() throws {
        guard let (context, resources, font) = try metalRenderer() else { return }
        let image = try pixels(context, try frame150(context, resources, font).draw)
        let dir = try #require(rustFramesDirectory)
        let rust = try pngPixels(URL(fileURLWithPath: dir).appendingPathComponent("frame_0000.png"))
        try #require(rust.width == Self.width && rust.height == Self.height)
        var differing = 0
        var largest = 0
        var box = (minX: Int.max, minY: Int.max, maxX: Int.min, maxY: Int.min)
        for y in 0..<Self.height {
            for x in 0..<Self.width {
                let i = (y * Self.width + x) * 4
                let d = (0..<3).map { abs(Int(image[i + $0]) - Int(rust.rgba[i + $0])) }.max()!
                if d > 0 {
                    differing += 1
                    largest = max(largest, d)
                    box = (min(box.minX, x), min(box.minY, y), max(box.maxX, x), max(box.maxY, y))
                }
            }
        }
        #expect(
            differing == 0,
            "\(differing) pixels differ from the Rust frame (largest channel difference \(largest), within \(box))"
        )
    }

    @Test func aReadyFrameIsBlackAndTextWhateverTheScene() throws {
        // Spec 03 §9.4: the title screen's wash is opaque, and source-over with
        // alpha 1 gives exactly the source, so a Ready frame is the same over any
        // scene: black, and the banner.
        guard let (context, resources, font) = try metalRenderer() else { return }
        let frame = try recorderFrame150()
        try context.setSunSky(DEMO_SUN)
        try context.setCamera(frame.camera)
        let list = frame.drawList
        let ready = HudState(state: .ready)
        let overScene = try pixels(context) { f in
            try recordScene(list, resources: resources, frame: f)
            try drawHud(ready, font: font, frame: f)
        }
        let overNothing = try pixels(context) { f in try drawHud(ready, font: font, frame: f) }
        #expect(overScene == overNothing, "the scene does not show through the title screen")
        #expect(context.stats.drawsSkipped == 0 && context.stats.framesSkipped == 0)

        // Black away from the banner, the banner's glyphs bright.
        for (x, y) in [(0, 0), (Self.width - 1, 0), (0, Self.height - 1), (Self.width - 1, Self.height - 1), (640, 100), (640, 650)] {
            #expect(rgb(overScene, x, y) == SIMD3(0, 0, 0), "(\(x), \(y)) is black")
        }
        var bright = 0
        for y in 300..<460 {
            for x in 320..<960 where rgb(overScene, x, y).min() > 200 {
                bright += 1
            }
        }
        #expect(bright > 1000, "the title and prompt are drawn")
    }

    /// Write RGBA8 pixels as a PNG.
    private func writePNG(_ rgba: [UInt8], width: Int, height: Int, to url: URL) throws {
        let provider = try #require(CGDataProvider(data: Data(rgba) as CFData))
        let image = try #require(
            CGImage(
                width: width, height: height, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: width * 4,
                space: CGColorSpace(name: CGColorSpace.sRGB)!,
                bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.last.rawValue),
                provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent
            )
        )
        let destination = try #require(CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil))
        CGImageDestinationAddImage(destination, image, nil)
        try #require(CGImageDestinationFinalize(destination), "wrote \(url.path)")
    }
}
