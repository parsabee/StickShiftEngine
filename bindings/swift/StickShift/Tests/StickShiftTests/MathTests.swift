// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import StickShift
import Testing
import simd

@Suite("simd interop")
struct MathTests {
    @Test func translationRotationScaleComposes() {
        let rotation = simd_quatf(angle: .pi / 2, axis: [0, 1, 0])
        let m = simd_float4x4(translation: [1, 2, 3], rotation: rotation, scale: [2, 2, 2])
        #expect(m.columns.3 == [1, 2, 3, 1])
        // +X, scaled by 2 and turned a quarter about +Y, points down -Z.
        let x = m * SIMD4<Float>(1, 0, 0, 0)
        #expect(simd_distance(SIMD3(x.x, x.y, x.z), [0, 0, -2]) < 1e-5)
    }

    @Test func aCameraLooksAtItsTarget() {
        let camera = Camera.perspective(eye: [3, 2, 5], target: [0, 1, 0], fovY: 1, aspect: 16.0 / 9.0)
        let clip = camera.viewProjection * SIMD4<Float>(0, 1, 0, 1)
        // The target lands in the middle of the screen, in front of the camera,
        // inside Metal's 0...1 depth range.
        #expect(abs(clip.x / clip.w) < 1e-5)
        #expect(abs(clip.y / clip.w) < 1e-5)
        #expect(clip.w > 0)
        #expect((0...1).contains(clip.z / clip.w))
        #expect(camera.position == [3, 2, 5])
    }
}
