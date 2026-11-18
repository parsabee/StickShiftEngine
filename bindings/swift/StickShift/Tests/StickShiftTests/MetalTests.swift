// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Testing
import simd

@testable import StickShift

/// On the Metal backend; each test skips without a device (and fails instead
/// with `STICKSHIFT_REQUIRE_GPU=1`).
@MainActor
@Suite("Metal")
struct MetalTests {
    /// Render the triangle into a 64 × 48 frame and return its BGRA pixels.
    private func render(
        _ context: Context,
        sun: SunSky = .default,
        lights: [Light] = [],
        emissive: SIMD3<Float> = .zero
    ) async throws -> [UInt8] {
        let (vertices, indices) = triangle()
        let mesh = try context.makeMesh(layout: .positionNormalColor, vertices: vertices, indices: indices)
        var readback: PixelReadback?
        try context.frame { frame in
            try frame.setCamera(camera(aspect: 64.0 / 48.0))
            try frame.setSunSky(sun)
            for light in lights {
                try frame.addLight(light)
            }
            try frame.draw(mesh, material: Material(emissive: emissive))
            readback = try frame.readPixels()
        }
        return try await readback!.pixels()
    }

    @Test func anOffscreenFrameRendersAndReadsBack() async throws {
        guard let context = try metalContext() else { return }
        let pixels = try await render(context)
        #expect(pixels.count == 64 * 48 * 4)
        #expect(Set(pixels).count > 8, "the frame has content")
        // The triangle's centre is red-dominated (BGRA).
        let centre = (24 * 64 + 32) * 4
        #expect(pixels[centre + 2] > pixels[centre])
    }

    @Test func lightsAndEmissionLightTheNight() async throws {
        guard let context = try metalContext() else { return }
        var night = SunSky.default
        night.sunIntensity = 0
        night.skyFill = 0.02
        let dark = try await render(context, sun: night)
        let lit = try await render(context, sun: night, lights: [.point(at: [0, 0, 0.6], intensity: 2, range: 3)])
        let glowing = try await render(context, sun: night, emissive: [0.6, 0.3, 0.1])
        let centre = (24 * 64 + 32) * 4
        #expect(lit[centre + 2] > dark[centre + 2])
        #expect(glowing[centre + 2] > dark[centre + 2])
    }

    @Test func aReadbackArrivesThroughFlushToo() throws {
        guard let context = try metalContext() else { return }
        var readback: PixelReadback?
        try context.frame { readback = try $0.readPixels() }
        try context.flush()
        #expect(readback?.state == .complete)
        #expect(readback?.bytes?.count == 64 * 48 * 4)
    }

    /// Record a frame from the triangle test's camera with `record`, and return
    /// its BGRA pixels.
    private func pixels(_ context: Context, _ record: (Frame) throws -> Void) async throws -> [UInt8] {
        var readback: PixelReadback?
        try context.frame { frame in
            try frame.setCamera(camera(aspect: 64.0 / 48.0))
            try record(frame)
            readback = try frame.readPixels()
        }
        return try await readback!.pixels()
    }

    @Test func aModelDrawsAtWorldTimesLocal() async throws {
        guard let context = try metalContext() else { return }
        let cube = try await context.loadModel(contentsOf: texturedCubeFixture)
        // Values whose products are exact in any grouping, so the matrices
        // `world × local` and `world × (local × node)` are the same bits.
        let world = simd_float4x4(translation: [0.25, 0, -1])
        let local = simd_float4x4(translation: .zero, scale: [0.5, 0.5, 0.5])
        let red: SIMD4<Float> = [1, 0.25, 0.25, 1]

        let sky = try await pixels(context) { _ in }
        let placed = try await pixels(context) { try $0.draw(cube, world: world, local: local) }
        let byOneMatrix = try await pixels(context) { try $0.draw(cube, transform: StickShiftMath.multiply(world, local)) }
        let swapped = try await pixels(context) { try $0.draw(cube, transform: StickShiftMath.multiply(local, world)) }
        #expect(placed != sky)
        #expect(placed == byOneMatrix, "world applies after local")
        #expect(placed != swapped)

        let tinted = try await pixels(context) { try $0.draw(cube, world: world, local: local, tint: red) }
        let tintedByOneMatrix = try await pixels(context) {
            try $0.draw(cube, transform: StickShiftMath.multiply(world, local), tint: red)
        }
        #expect(tinted == tintedByOneMatrix)
        #expect(tinted != placed)

        // The cube's one part is textured.
        let textured = try await pixels(context) { try $0.draw(cube, world: world, local: local, parts: .textured) }
        let untextured = try await pixels(context) { try $0.draw(cube, world: world, local: local, parts: .untextured) }
        #expect(textured == placed)
        #expect(untextured == sky)
    }
}
