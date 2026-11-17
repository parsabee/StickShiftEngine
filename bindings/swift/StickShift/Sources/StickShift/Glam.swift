// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import CStickShift
import simd

// glam's vector, quaternion and matrix operations, computed by glam itself
// inside the engine, so a Swift game gets the bits a Rust game gets. Use them
// wherever a result must match the Rust engine (a port, a replay, a trace):
// `simd_dot`, `simd_length`, `simd_normalize`, `simd_act` and the quaternion
// and matrix `*` sum in other orders and fuse multiplies with adds.

/// `a · b`, as glam's `Vec3::dot`: `(a.x * b.x + a.y * b.y) + a.z * b.z`, with
/// no fused multiply-add.
public func glamDot(_ a: SIMD3<Float>, _ b: SIMD3<Float>) -> Float {
    stickshift_vec3_dot(StickShiftVec3(a), StickShiftVec3(b))
}

/// `a × b` (right-handed), as glam's `Vec3::cross`.
public func glamCross(_ a: SIMD3<Float>, _ b: SIMD3<Float>) -> SIMD3<Float> {
    SIMD3(stickshift_vec3_cross(StickShiftVec3(a), StickShiftVec3(b)))
}

/// The length of `v`, as glam's `Vec3::length`: the square root of `v · v`.
public func glamLength(_ v: SIMD3<Float>) -> Float {
    stickshift_vec3_length(StickShiftVec3(v))
}

/// `v` times `1 / length(v)`, or zero when that reciprocal is not finite and
/// positive (a zero, tiny, huge or non-finite `v`), as glam's
/// `Vec3::normalize_or_zero`.
public func glamNormalizeOrZero(_ v: SIMD3<Float>) -> SIMD3<Float> {
    SIMD3(stickshift_vec3_normalize_or_zero(StickShiftVec3(v)))
}

/// `a * (1 - t) + b * t`, each operation rounded on its own, as glam's
/// `Vec3::lerp`. `t` is not clamped. (Not `a + (b - a) * t`, whose last bit
/// differs on many inputs.)
public func glamLerp(_ a: SIMD3<Float>, _ b: SIMD3<Float>, _ t: Float) -> SIMD3<Float> {
    SIMD3(stickshift_vec3_lerp(StickShiftVec3(a), StickShiftVec3(b), t))
}

extension simd_quatf {
    /// A rotation of `angle` radians about +X, as glam's `Quat::from_rotation_x`.
    public init(glamRotationX angle: Float) {
        self.init(stickshift_quat_from_rotation_x(angle))
    }

    /// A rotation of `angle` radians about +Y, as glam's `Quat::from_rotation_y`.
    public init(glamRotationY angle: Float) {
        self.init(stickshift_quat_from_rotation_y(angle))
    }

    /// A rotation of `angle` radians about +Z, as glam's `Quat::from_rotation_z`.
    public init(glamRotationZ angle: Float) {
        self.init(stickshift_quat_from_rotation_z(angle))
    }

    /// `v` rotated by this quaternion, as glam's `Quat * Vec3`. The quaternion
    /// should be a unit one; it is not normalised.
    public func glamRotate(_ v: SIMD3<Float>) -> SIMD3<Float> {
        SIMD3(stickshift_quat_mul_vec3(StickShiftQuat(self), StickShiftVec3(v)))
    }

    /// The Hamilton product `self × other`, as glam's `Quat * Quat`: the
    /// rotation that applies `other` first, then this one. Neither is normalised.
    public func glamMultiply(_ other: simd_quatf) -> simd_quatf {
        simd_quatf(stickshift_quat_mul(StickShiftQuat(self), StickShiftQuat(other)))
    }

    init(_ q: StickShiftQuat) {
        self.init(ix: q.x, iy: q.y, iz: q.z, r: q.w)
    }
}

extension simd_float4x4 {
    /// The point `p` transformed by this matrix, translation included and with
    /// no perspective divide, as glam's `Mat4::transform_point3`.
    public func glamTransformPoint(_ p: SIMD3<Float>) -> SIMD3<Float> {
        SIMD3(stickshift_mat4_transform_point3(StickShiftMat4(self), StickShiftVec3(p)))
    }

    /// The direction `v` transformed by this matrix, without translation, as
    /// glam's `Mat4::transform_vector3`.
    public func glamTransformVector(_ v: SIMD3<Float>) -> SIMD3<Float> {
        SIMD3(stickshift_mat4_transform_vector3(StickShiftMat4(self), StickShiftVec3(v)))
    }

    /// Translation × rotation × scale, as the engine draws a transform
    /// component: `stickshift_math::Transform::to_matrix`.
    init(transform t: StickShiftTransform) {
        self.init(stickshift_transform_to_mat4(t))
    }
}

extension StickShiftQuat {
    init(_ q: simd_quatf) {
        self.init(x: q.imag.x, y: q.imag.y, z: q.imag.z, w: q.real)
    }
}

extension StickShiftTransform {
    /// A transform, field by field: `simd_quatf` and `SIMD3<Float>` are laid out
    /// differently from the C types, so the fields are copied, not the bytes.
    init(position: SIMD3<Float>, rotation: simd_quatf, scale: SIMD3<Float>) {
        self.init(position: StickShiftVec3(position), rotation: StickShiftQuat(rotation), scale: StickShiftVec3(scale))
    }
}

/// Rust's `f32` constants and the `f32` operations whose Swift counterparts
/// differ, so game code computes what the Rust engine computes.
public enum RustF32 {
    /// `std::f32::consts::PI` (bits `0x40490FDB`). Swift's `Float.pi` is one
    /// bit lower (`0x40490FDA`): it rounds toward zero, Rust to nearest.
    public static let pi = Float(bitPattern: 0x4049_0FDB)
    /// `std::f32::consts::FRAC_PI_2` (bits `0x3FC90FDB`).
    public static let fracPi2 = Float(bitPattern: 0x3FC9_0FDB)
    /// `f32::EPSILON`: the gap between 1 and the next `Float`.
    public static let epsilon: Float = .ulpOfOne
    /// `f32::MIN`: the most negative finite value (not the smallest positive one).
    public static let lowest: Float = -.greatestFiniteMagnitude

    /// `PI / 180`, folded to one `f32` as Rust folds it.
    private static let radiansPerDegree: Float = pi / 180

    /// `f32::to_radians`: `degrees * (PI / 180)`.
    public static func toRadians(_ degrees: Float) -> Float {
        degrees * radiansPerDegree
    }

    /// `f32::max`: a NaN operand is ignored, and `+0` is larger than `-0`.
    public static func max(_ a: Float, _ b: Float) -> Float {
        if a.isNaN { return b }
        if b.isNaN { return a }
        if a > b { return a }
        if b > a { return b }
        // Equal: only the sign of a zero can differ.
        return a.sign == .minus ? b : a
    }

    /// `f32::min`: a NaN operand is ignored, and `-0` is smaller than `+0`.
    public static func min(_ a: Float, _ b: Float) -> Float {
        if a.isNaN { return b }
        if b.isNaN { return a }
        if a < b { return a }
        if b < a { return b }
        return a.sign == .minus ? a : b
    }

    /// `f32::clamp`: `lo` if `x` is below it, else `hi` if above it. A NaN
    /// stays NaN.
    public static func clamp(_ x: Float, _ lo: Float, _ hi: Float) -> Float {
        var x = x
        if x < lo { x = lo }
        if x > hi { x = hi }
        return x
    }

    /// `f32::rem_euclid`: the remainder of `a / b` that is never negative
    /// (`a % b`, plus `|b|` if that is negative).
    public static func remEuclid(_ a: Float, _ b: Float) -> Float {
        let r = a.truncatingRemainder(dividingBy: b)
        return r < 0 ? r + abs(b) : r
    }

    /// Rust's `x as u64`: toward zero, saturating at both ends; NaN is 0.
    /// (Swift's `UInt64(x)` traps out of range.)
    public static func toU64(_ x: Float) -> UInt64 {
        if x.isNaN || x <= 0 { return 0 }
        if x >= 18_446_744_073_709_551_616 { return .max }
        return UInt64(x)
    }

    /// Rust's `x as i64` for an `f64`: toward zero, saturating at both ends;
    /// NaN is 0.
    public static func toI64(_ x: Double) -> Int64 {
        if x.isNaN { return 0 }
        if x <= -9_223_372_036_854_775_808 { return .min }
        if x >= 9_223_372_036_854_775_808 { return .max }
        return Int64(x)
    }

    /// Rust's `x as u32` for an `f64`: toward zero, saturating at both ends;
    /// NaN is 0.
    public static func toU32(_ x: Double) -> UInt32 {
        if x.isNaN || x <= 0 { return 0 }
        if x >= 4_294_967_296 { return .max }
        return UInt32(x)
    }

    /// `Duration::as_secs_f32` for `nanoseconds`: the whole seconds as an
    /// `f32`, plus the remaining nanoseconds as an `f32` divided by 1e9. (For
    /// the fixed step, that is `0x3C888888`, one bit below
    /// ``Engine/fixedDeltaTime``.)
    public static func seconds(nanoseconds: UInt64) -> Float {
        let whole = Float(nanoseconds / 1_000_000_000)
        let fraction = Float(UInt32(nanoseconds % 1_000_000_000))
        return whole + fraction / 1_000_000_000
    }
}
