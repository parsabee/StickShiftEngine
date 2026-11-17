// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import CStickShift
import Foundation
@testable import StickShift
import Testing
import simd

/// A deterministic stream of floats in `-8..<8`, with a spread of exponents.
private struct Floats {
    var state: UInt64 = 0x9E37_79B9_7F4A_7C15

    mutating func next() -> Float {
        state = state &* 6_364_136_223_846_793_005 &+ 1_442_695_040_888_963_407
        let unit = Float(state >> 40) / Float(1 << 24)
        let exponent = Float(Int(state >> 20 & 7) - 3)
        return (unit * 2 - 1) * exp2(exponent)
    }

    mutating func vector() -> SIMD3<Float> {
        SIMD3(next(), next(), next())
    }
}

private func bits(_ v: SIMD3<Float>) -> [UInt32] {
    [v.x.bitPattern, v.y.bitPattern, v.z.bitPattern]
}

private func bits(_ q: simd_quatf) -> [UInt32] {
    [q.imag.x.bitPattern, q.imag.y.bitPattern, q.imag.z.bitPattern, q.real.bitPattern]
}

@Suite("glam's operations and Rust's f32 rules")
struct GlamTests {
    @Test func vectorOperationsRoundEachStepAsGlamDoes() {
        var floats = Floats()
        for _ in 0..<500 {
            let a = floats.vector()
            let b = floats.vector()
            let t = floats.next()
            // glam's order, one rounded operation at a time (Swift never fuses).
            let dot = (a.x * b.x + a.y * b.y) + a.z * b.z
            #expect(glamDot(a, b).bitPattern == dot.bitPattern)
            let cross = SIMD3(a.y * b.z - b.y * a.z, a.z * b.x - b.z * a.x, a.x * b.y - b.x * a.y)
            #expect(bits(glamCross(a, b)) == bits(cross))
            #expect(glamLength(a).bitPattern == glamDot(a, a).squareRoot().bitPattern)
            let s = 1 - t
            #expect(bits(glamLerp(a, b, t)) == bits(SIMD3(a.x * s, a.y * s, a.z * s) + SIMD3(b.x * t, b.y * t, b.z * t)))
            #expect(bits(glamNormalizeOrZero(a)) == bits(a * (1 / glamLength(a))))
        }
        #expect(glamLength([3, 0, 4]) == 5)
        #expect(glamNormalizeOrZero(.zero) == .zero)
        #expect(glamNormalizeOrZero([1e-30, 0, 0]) == .zero, "a length that underflows")
        #expect(glamNormalizeOrZero([.nan, 0, 0]) == .zero)
    }

    @Test func quaternionsKeepTheirFieldOrder() {
        // glam's `Quat * Vec3` (its aarch64 NEON path), written out: proves the
        // x, y, z, w order crosses intact.
        func rotate(_ q: simd_quatf, _ v: SIMD3<Float>) -> SIMD3<Float> {
            let (qx, qy, qz, w) = (q.imag.x, q.imag.y, q.imag.z, q.real)
            let k1 = (w * w) - ((qx * qx + qy * qy) + qz * qz)
            let d = ((v.x * qx + v.y * qy) + v.z * qz) * 2
            let k3 = w * 2
            let c = SIMD3(qy * v.z - qz * v.y, qz * v.x - qx * v.z, qx * v.y - qy * v.x)
            return SIMD3(
                (v.x * k1 + qx * d) + c.x * k3,
                (v.y * k1 + qy * d) + c.y * k3,
                (v.z * k1 + qz * d) + c.z * k3
            )
        }
        var floats = Floats()
        for _ in 0..<500 {
            let q = simd_quatf(glamRotationY: floats.next())
            let v = floats.vector()
            #expect(bits(q.glamRotate(v)) == bits(rotate(q, v)))
        }

        // A quarter turn about +Y takes +X to -Z.
        let quarter = simd_quatf(glamRotationY: RustF32.fracPi2)
        #expect(simd_distance(quarter.glamRotate([1, 0, 0]), [0, 0, -1]) < 1e-6)
        // The untouched lanes are +0, even for a negative angle (an axis-angle
        // product would make them -0).
        let back = simd_quatf(glamRotationY: -1)
        #expect(back.imag.x.bitPattern == 0 && back.imag.z.bitPattern == 0)
        #expect(simd_quatf(glamRotationX: 1).imag.y.bitPattern == 0)
        #expect(simd_quatf(glamRotationX: 1).imag.x > 0)
        #expect(simd_quatf(glamRotationZ: 1).imag.z > 0)

        // Two quarter turns make a half turn: +X to -X.
        let half = quarter.glamMultiply(quarter)
        #expect(simd_distance(half.glamRotate([1, 0, 0]), [-1, 0, 0]) < 1e-6)
        // b first, then a.
        let x = simd_quatf(glamRotationX: RustF32.fracPi2)
        #expect(simd_distance(quarter.glamMultiply(x).glamRotate([0, 1, 0]), quarter.glamRotate(x.glamRotate([0, 1, 0]))) < 1e-6)
    }

    @Test func matricesTransformPointsAndVectors() {
        let m = simd_float4x4(translation: [1, 2, 3], rotation: simd_quatf(glamRotationY: RustF32.fracPi2), scale: [2, 2, 2])
        #expect(simd_distance(m.glamTransformPoint([1, 0, 0]), [1, 2, 1]) < 1e-5)
        #expect(simd_distance(m.glamTransformVector([1, 0, 0]), [0, 0, -2]) < 1e-5)

        // The engine's transform-component matrix is the same TRS product.
        var floats = Floats()
        for _ in 0..<100 {
            let position = floats.vector()
            let rotation = simd_quatf(glamRotationY: floats.next())
            let scale = floats.vector()
            let trs = simd_float4x4(translation: position, rotation: rotation, scale: scale)
            let component = simd_float4x4(transform: StickShiftTransform(position: position, rotation: rotation, scale: scale))
            #expect(bits(component) == bits(trs))
        }
    }

    @Test func rustConstantsHaveRustsBits() {
        #expect(RustF32.pi.bitPattern == 0x4049_0FDB)
        #expect(Float.pi.bitPattern == 0x4049_0FDA, "why RustF32.pi exists")
        #expect(RustF32.fracPi2.bitPattern == 0x3FC9_0FDB)
        #expect(RustF32.epsilon.bitPattern == 0x3400_0000)
        #expect(RustF32.lowest == -Float.greatestFiniteMagnitude)
        #expect(abs(RustF32.toRadians(180) - RustF32.pi) <= RustF32.pi.ulp)
        #expect(RustF32.seconds(nanoseconds: Engine.fixedStepNanoseconds).bitPattern == 0x3C88_8888)
        #expect(RustF32.seconds(nanoseconds: 2_500_000_000) == 2.5)
    }

    @Test func rustComparisonsIgnoreNaNAndOrderZeros() {
        #expect(RustF32.max(.nan, 1) == 1)
        #expect(RustF32.max(1, .nan) == 1)
        #expect(RustF32.max(-0.0, 0.0).sign == .plus)
        #expect(RustF32.max(0.0, -0.0).sign == .plus)
        #expect(RustF32.min(-0.0, 0.0).sign == .minus)
        #expect(RustF32.min(0.0, -0.0).sign == .minus)
        #expect(RustF32.min(.nan, -3) == -3)
        #expect(RustF32.clamp(5, 0, 1) == 1)
        #expect(RustF32.clamp(-5, 0, 1) == 0)
        #expect(RustF32.clamp(.nan, 0, 1).isNaN)
        #expect(RustF32.remEuclid(-1, 3) == 2)
        #expect(RustF32.remEuclid(7, -3) == 1)
        #expect(RustF32.remEuclid(-7, -3) == 2)
    }

    @Test func rustCastsSaturate() {
        #expect(RustF32.toU64(-1) == 0)
        #expect(RustF32.toU64(.nan) == 0)
        #expect(RustF32.toU64(.infinity) == .max)
        #expect(RustF32.toU64(3.9) == 3)
        #expect(RustF32.toI64(-.infinity) == .min)
        #expect(RustF32.toI64(.infinity) == .max)
        #expect(RustF32.toI64(-3.9) == -3)
        #expect(RustF32.toI64(.nan) == 0)
        #expect(RustF32.toU32(5e9) == .max)
        #expect(RustF32.toU32(-2) == 0)
        #expect(RustF32.toU32(41.5) == 41)
    }
}
