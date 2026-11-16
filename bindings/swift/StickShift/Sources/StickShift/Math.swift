// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import CStickShift
import simd

// The engine's conventions, the same as the C API's: right-handed, +Y up, a camera
// looks down -Z, matrices are column-major (the same memory layout as
// `simd_float4x4`), clip-space depth is Metal's 0...1, and angles are radians.

extension simd_float4x4 {
    /// The matrix a `StickShiftMat4` holds (both are column-major).
    init(_ m: StickShiftMat4) {
        self = unsafeBitCast(m, to: simd_float4x4.self)
    }

    /// A right-handed perspective projection with depth `0...1`: the projection
    /// the engine's own camera uses.
    ///
    /// - Parameters:
    ///   - fovY: The vertical field of view, in radians.
    ///   - aspect: Width over height.
    ///   - near: The near plane's distance, `> 0`.
    ///   - far: The far plane's distance, `> near`.
    public static func perspective(fovY: Float, aspect: Float, near: Float, far: Float)
        -> simd_float4x4
    {
        simd_float4x4(stickshift_mat4_perspective(fovY, aspect, near, far))
    }

    /// A right-handed view matrix for a camera at `eye` looking at `target`.
    public static func lookAt(eye: SIMD3<Float>, target: SIMD3<Float>, up: SIMD3<Float> = [0, 1, 0])
        -> simd_float4x4
    {
        simd_float4x4(stickshift_mat4_look_at(StickShiftVec3(eye), StickShiftVec3(target), StickShiftVec3(up)))
    }

    /// Translation × rotation × scale: a model-to-world matrix.
    public init(
        translation: SIMD3<Float>,
        rotation: simd_quatf = simd_quatf(ix: 0, iy: 0, iz: 0, r: 1),
        scale: SIMD3<Float> = .one
    ) {
        let q = StickShiftQuat(x: rotation.imag.x, y: rotation.imag.y, z: rotation.imag.z, w: rotation.real)
        self.init(stickshift_mat4_from_trs(StickShiftVec3(translation), q, StickShiftVec3(scale)))
    }
}

extension StickShiftMat4 {
    init(_ m: simd_float4x4) {
        self = unsafeBitCast(m, to: StickShiftMat4.self)
    }
}

extension StickShiftVec3 {
    init(_ v: SIMD3<Float>) {
        self.init(x: v.x, y: v.y, z: v.z)
    }
}

extension SIMD3<Float> {
    init(_ v: StickShiftVec3) {
        self.init(v.x, v.y, v.z)
    }

    /// The vector as a C `float[3]`.
    var tuple: (Float, Float, Float) {
        (x, y, z)
    }

    init(_ t: (Float, Float, Float)) {
        self.init(t.0, t.1, t.2)
    }
}

extension StickShiftColor {
    init(_ c: SIMD4<Float>) {
        self.init(r: c.x, g: c.y, b: c.z, a: c.w)
    }
}
