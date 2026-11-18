// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Foundation
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

    /// A deterministic stream of floats in `-8..<8`, with a spread of
    /// exponents, so the products round in many different ways.
    private struct Floats {
        var state: UInt64 = 0x2545_F491_4F6C_DD1D

        mutating func next() -> Float {
            state = state &* 6_364_136_223_846_793_005 &+ 1_442_695_040_888_963_407
            let unit = Float(state >> 40) / Float(1 << 24)
            let exponent = Float(Int(state >> 20 & 7) - 3)
            return (unit * 2 - 1) * exp2(exponent)
        }

        mutating func matrix() -> simd_float4x4 {
            simd_float4x4((0..<4).map { _ in SIMD4((0..<4).map { _ in next() }) })
        }
    }

    /// The engine's (glam's) product: each element a left-to-right sum of
    /// rounded products, never fused.
    private func engineProduct(_ a: simd_float4x4, _ b: simd_float4x4) -> simd_float4x4 {
        var out = simd_float4x4()
        for column in 0..<4 {
            for row in 0..<4 {
                var sum = a[0][row] * b[column][0]
                sum = sum + a[1][row] * b[column][1]
                sum = sum + a[2][row] * b[column][2]
                sum = sum + a[3][row] * b[column][3]
                out[column][row] = sum
            }
        }
        return out
    }

    @Test func stickShiftMathMultipliesAsTheEngineDoes() {
        var floats = Floats()
        for _ in 0..<2_000 {
            let (a, b) = (floats.matrix(), floats.matrix())
            #expect(bits(StickShiftMath.multiply(a, b)) == bits(engineProduct(a, b)))
        }
        // `a × b` applies `b` first.
        let move = simd_float4x4(translation: [1, 0, 0])
        let double = simd_float4x4(translation: .zero, scale: [2, 2, 2])
        #expect(StickShiftMath.multiply(double, move).columns.3 == [2, 0, 0, 1])
    }

    @Test func stickShiftMathRotatesAsTheEngineDoes() {
        // Rust's `std::f32::consts::PI`, one bit above Swift's `Float.pi`.
        let pi = Float(bitPattern: 0x4049_0FDB)
        for angle in [pi, pi / 2, -pi / 2, 0.3, -2.5] {
            // glam: `(sin, cos)` of the half angle, and the axis times the sine.
            let (s, c) = (sin(angle * 0.5), cos(angle * 0.5))
            let q = StickShiftMath.rotation(axis: [0, 1, 0], angle: angle)
            let expected: [UInt32] = [0 * s, s, 0 * s, c].map(\.bitPattern)
            #expect([q.imag.x, q.imag.y, q.imag.z, q.real].map(\.bitPattern) == expected)
            // The axis is normalised first.
            let long = StickShiftMath.rotation(axis: [0, 2, 0], angle: angle)
            #expect(long.vector == q.vector)
        }
        // A negative half-angle sine keeps the zero components' sign.
        #expect(StickShiftMath.rotation(axis: [0, 1, 0], angle: -1).imag.x.bitPattern == 0x8000_0000)
        #expect(StickShiftMath.rotation(axis: .zero, angle: 1).vector == [0, 0, 0, 1])
    }

    @Test func aCameraIsMultipliedAsTheEngineMultiplies() {
        let camera = Camera.perspective(
            eye: [1.75, 2.4, 7.2], target: [1.75, 1, -4], fovY: 0.785_398_2, aspect: 4.0 / 3.0, near: 0.3, far: 300
        )
        let expected = StickShiftMath.multiply(
            .perspective(fovY: 0.785_398_2, aspect: 4.0 / 3.0, near: 0.3, far: 300),
            .lookAt(eye: [1.75, 2.4, 7.2], target: [1.75, 1, -4])
        )
        #expect(bits(camera.viewProjection) == bits(expected))
    }
}
