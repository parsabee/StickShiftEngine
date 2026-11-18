// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! Math helpers: pure, thread-safe, never fail. Right-handed, Y up, Metal clip
//! depth `0..1`, column-major matrices; the same conventions as `stickshift-camera`.
//!
//! The `vec3`, `quat`, `mat4_transform` and `transform` helpers are glam's own
//! operations, called on this library's target: a host that does its game math
//! through them gets the bits a Rust game gets, including glam's SIMD paths
//! (NEON on aarch64), which a host's own `simd` code need not match.

use stickshift_math::glam::{Mat4, Quat, Vec3};
use stickshift_math::Transform;

use crate::types::{
    StickShiftColor, StickShiftMat4, StickShiftQuat, StickShiftTransform, StickShiftVec3,
};

impl From<StickShiftVec3> for Vec3 {
    fn from(v: StickShiftVec3) -> Self {
        Vec3::new(v.x, v.y, v.z)
    }
}

impl From<Vec3> for StickShiftVec3 {
    fn from(v: Vec3) -> Self {
        Self {
            x: v.x,
            y: v.y,
            z: v.z,
        }
    }
}

impl From<StickShiftQuat> for Quat {
    fn from(q: StickShiftQuat) -> Self {
        Quat::from_xyzw(q.x, q.y, q.z, q.w)
    }
}

impl From<Quat> for StickShiftQuat {
    fn from(q: Quat) -> Self {
        Self {
            x: q.x,
            y: q.y,
            z: q.z,
            w: q.w,
        }
    }
}

impl From<StickShiftMat4> for Mat4 {
    fn from(m: StickShiftMat4) -> Self {
        Mat4::from_cols_array(&m.m)
    }
}

impl From<Mat4> for StickShiftMat4 {
    fn from(m: Mat4) -> Self {
        Self {
            m: m.to_cols_array(),
        }
    }
}

impl From<StickShiftTransform> for Transform {
    fn from(t: StickShiftTransform) -> Self {
        Transform {
            position: t.position.into(),
            rotation: t.rotation.into(),
            scale: t.scale.into(),
        }
    }
}

impl From<Transform> for StickShiftTransform {
    fn from(t: Transform) -> Self {
        Self {
            position: t.position.into(),
            rotation: t.rotation.into(),
            scale: t.scale.into(),
        }
    }
}

impl StickShiftColor {
    pub(crate) fn to_array(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }
}

/// The identity matrix.
#[no_mangle]
pub extern "C" fn stickshift_mat4_identity() -> StickShiftMat4 {
    Mat4::IDENTITY.into()
}

/// `a × b`: the transform that applies `b` first, then `a`.
#[no_mangle]
pub extern "C" fn stickshift_mat4_mul(a: StickShiftMat4, b: StickShiftMat4) -> StickShiftMat4 {
    (Mat4::from(a) * Mat4::from(b)).into()
}

/// Translation × rotation × scale.
#[no_mangle]
pub extern "C" fn stickshift_mat4_from_trs(
    translation: StickShiftVec3,
    rotation: StickShiftQuat,
    scale: StickShiftVec3,
) -> StickShiftMat4 {
    Mat4::from_scale_rotation_translation(scale.into(), rotation.into(), translation.into()).into()
}

/// A right-handed view matrix looking from `eye` toward `target`.
#[no_mangle]
pub extern "C" fn stickshift_mat4_look_at(
    eye: StickShiftVec3,
    target: StickShiftVec3,
    up: StickShiftVec3,
) -> StickShiftMat4 {
    Mat4::look_at_rh(eye.into(), target.into(), up.into()).into()
}

/// A right-handed perspective projection with Metal's `0..1` clip depth: the
/// projection `stickshift-camera` uses. `fov_y` is in radians.
#[no_mangle]
pub extern "C" fn stickshift_mat4_perspective(
    fov_y: f32,
    aspect: f32,
    z_near: f32,
    z_far: f32,
) -> StickShiftMat4 {
    Mat4::perspective_rh(fov_y, aspect, z_near, z_far).into()
}

/// A rotation of `angle` radians about `axis` (normalised here; a zero axis
/// gives the identity).
#[no_mangle]
pub extern "C" fn stickshift_quat_from_axis_angle(
    axis: StickShiftVec3,
    angle: f32,
) -> StickShiftQuat {
    match Vec3::from(axis).try_normalize() {
        Some(axis) => Quat::from_axis_angle(axis, angle).into(),
        None => Quat::IDENTITY.into(),
    }
}

/// `a · b`: glam's `Vec3::dot`, `(a.x * b.x + a.y * b.y) + a.z * b.z` with
/// no fused multiply-add.
#[no_mangle]
pub extern "C" fn stickshift_vec3_dot(a: StickShiftVec3, b: StickShiftVec3) -> f32 {
    Vec3::from(a).dot(b.into())
}

/// `a × b` (right-handed): glam's `Vec3::cross`.
#[no_mangle]
pub extern "C" fn stickshift_vec3_cross(a: StickShiftVec3, b: StickShiftVec3) -> StickShiftVec3 {
    Vec3::from(a).cross(b.into()).into()
}

/// The length of `v`: glam's `Vec3::length`, the square root of `v · v`.
#[no_mangle]
pub extern "C" fn stickshift_vec3_length(v: StickShiftVec3) -> f32 {
    Vec3::from(v).length()
}

/// `v` times `1 / length(v)`, or `{0, 0, 0}` when that reciprocal is not a
/// finite positive number (a zero, NaN or infinite `v`, or one so small or
/// so large that its length underflows or overflows): glam's
/// `Vec3::normalize_or_zero`.
#[no_mangle]
pub extern "C" fn stickshift_vec3_normalize_or_zero(v: StickShiftVec3) -> StickShiftVec3 {
    Vec3::from(v).normalize_or_zero().into()
}

/// `a * (1 - t) + b * t`, each multiply and add rounded on its own (no fused
/// multiply-add): glam's `Vec3::lerp`. `t` is not clamped. This is not
/// `a + (b - a) * t`, whose last bit differs on many inputs.
#[no_mangle]
pub extern "C" fn stickshift_vec3_lerp(
    a: StickShiftVec3,
    b: StickShiftVec3,
    t: f32,
) -> StickShiftVec3 {
    Vec3::from(a).lerp(b.into(), t).into()
}

/// A rotation of `angle` radians about +X: glam's `Quat::from_rotation_x`.
#[no_mangle]
pub extern "C" fn stickshift_quat_from_rotation_x(angle: f32) -> StickShiftQuat {
    Quat::from_rotation_x(angle).into()
}

/// A rotation of `angle` radians about +Y: glam's `Quat::from_rotation_y`.
#[no_mangle]
pub extern "C" fn stickshift_quat_from_rotation_y(angle: f32) -> StickShiftQuat {
    Quat::from_rotation_y(angle).into()
}

/// A rotation of `angle` radians about +Z: glam's `Quat::from_rotation_z`.
#[no_mangle]
pub extern "C" fn stickshift_quat_from_rotation_z(angle: f32) -> StickShiftQuat {
    Quat::from_rotation_z(angle).into()
}

/// The Hamilton product `a × b`: the rotation that applies `b` first, then
/// `a`. glam's `Quat * Quat`; neither input is normalised.
#[no_mangle]
pub extern "C" fn stickshift_quat_mul(a: StickShiftQuat, b: StickShiftQuat) -> StickShiftQuat {
    (Quat::from(a) * Quat::from(b)).into()
}

/// `v` rotated by `q`: glam's `Quat * Vec3`. `q` should be a unit quaternion;
/// it is not normalised here.
#[no_mangle]
pub extern "C" fn stickshift_quat_mul_vec3(q: StickShiftQuat, v: StickShiftVec3) -> StickShiftVec3 {
    (Quat::from(q) * Vec3::from(v)).into()
}

/// The point `p` transformed by `m`, translation included and with no
/// perspective divide: glam's `Mat4::transform_point3`.
#[no_mangle]
pub extern "C" fn stickshift_mat4_transform_point3(
    m: StickShiftMat4,
    p: StickShiftVec3,
) -> StickShiftVec3 {
    Mat4::from(m).transform_point3(p.into()).into()
}

/// The direction `v` transformed by `m`, without translation: glam's
/// `Mat4::transform_vector3`.
#[no_mangle]
pub extern "C" fn stickshift_mat4_transform_vector3(
    m: StickShiftMat4,
    v: StickShiftVec3,
) -> StickShiftVec3 {
    Mat4::from(m).transform_vector3(v.into()).into()
}

/// The matrix of `t`, translation × rotation × scale:
/// `stickshift_math::Transform::to_matrix`, the matrix the engine draws a
/// `TransformComponent` with. The same bits as `stickshift_mat4_from_trs` with
/// `t`'s fields.
#[no_mangle]
pub extern "C" fn stickshift_transform_to_mat4(t: StickShiftTransform) -> StickShiftMat4 {
    Transform::from(t).to_matrix().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f32, y: f32, z: f32) -> StickShiftVec3 {
        StickShiftVec3 { x, y, z }
    }

    #[test]
    fn matrices_are_column_major_and_match_glam() {
        let t = stickshift_mat4_from_trs(v(1.0, 2.0, 3.0), Quat::IDENTITY.into(), v(1.0, 1.0, 1.0));
        // The translation is the fourth column: m[12..15].
        assert_eq!(&t.m[12..15], &[1.0, 2.0, 3.0]);
        let product = stickshift_mat4_mul(t, stickshift_mat4_identity());
        assert_eq!(product, t);
    }

    #[test]
    fn the_projection_is_the_cameras() {
        // stickshift-camera's defaults: 45° vertical FOV, near 0.1, far 100.
        let camera = stickshift_camera::Camera::new(1.5);
        let ours = Mat4::from(stickshift_mat4_perspective(
            45f32.to_radians(),
            1.5,
            0.1,
            100.0,
        ));
        assert_eq!(ours, camera.projection_matrix());
    }

    #[test]
    fn a_zero_axis_rotation_is_the_identity() {
        assert_eq!(
            stickshift_quat_from_axis_angle(v(0.0, 0.0, 0.0), 1.0),
            Quat::IDENTITY.into()
        );
        let q = Quat::from(stickshift_quat_from_axis_angle(v(0.0, 2.0, 0.0), 1.0));
        assert!((q.length() - 1.0).abs() < 1e-6, "the axis was normalised");
    }

    #[test]
    fn look_at_puts_the_target_down_negative_z() {
        let view = Mat4::from(stickshift_mat4_look_at(
            v(0.0, 0.0, 5.0),
            v(0.0, 0.0, 0.0),
            v(0.0, 1.0, 0.0),
        ));
        let target_in_view = view.transform_point3(Vec3::ZERO);
        assert!((target_in_view - Vec3::new(0.0, 0.0, -5.0)).length() < 1e-5);
    }

    // ---- Bit equality with glam -------------------------------------------
    //
    // Each helper must return glam's own bits, so a host doing its game math
    // through them matches a Rust game exactly. Inputs go through `black_box`
    // so neither side is folded at compile time. A NaN result only has to be
    // a NaN on both sides: its payload is not part of the contract.

    use std::f32::consts::{FRAC_PI_2, PI};
    use std::fmt::Display;
    use std::hint::black_box;

    fn same(a: f32, b: f32) -> bool {
        a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
    }

    fn check_f32(ours: f32, glam: f32, what: &dyn Display) {
        assert!(
            same(ours, glam),
            "{what}: {ours:?} ({:#010x}), glam {glam:?} ({:#010x})",
            ours.to_bits(),
            glam.to_bits()
        );
    }

    fn check_lanes(ours: &[f32], glam: &[f32], what: &dyn Display) {
        assert_eq!(ours.len(), glam.len());
        for (lane, (&o, &g)) in ours.iter().zip(glam).enumerate() {
            check_f32(o, g, &format_args!("{what}, lane {lane}"));
        }
    }

    fn check_vec3(ours: StickShiftVec3, glam: Vec3, what: &dyn Display) {
        check_lanes(&[ours.x, ours.y, ours.z], &glam.to_array(), what);
    }

    fn check_quat(ours: StickShiftQuat, glam: Quat, what: &dyn Display) {
        check_lanes(&[ours.x, ours.y, ours.z, ours.w], &glam.to_array(), what);
    }

    fn check_mat4(ours: StickShiftMat4, glam: Mat4, what: &dyn Display) {
        check_lanes(&ours.m, &glam.to_cols_array(), what);
    }

    /// Every helper on one set of inputs, against glam.
    fn check_all(a: Vec3, b: Vec3, t: f32, q: Quat, r: Quat, m: Mat4, what: &dyn Display) {
        let (ca, cb, cq, cr, cm) = (
            black_box(StickShiftVec3::from(a)),
            black_box(StickShiftVec3::from(b)),
            black_box(StickShiftQuat::from(q)),
            black_box(StickShiftQuat::from(r)),
            black_box(StickShiftMat4::from(m)),
        );
        let (a, b, t, q, r, m) = (
            black_box(a),
            black_box(b),
            black_box(t),
            black_box(q),
            black_box(r),
            black_box(m),
        );
        check_f32(
            stickshift_vec3_dot(ca, cb),
            a.dot(b),
            &format_args!("{what}: dot"),
        );
        check_vec3(
            stickshift_vec3_cross(ca, cb),
            a.cross(b),
            &format_args!("{what}: cross"),
        );
        check_f32(
            stickshift_vec3_length(ca),
            a.length(),
            &format_args!("{what}: length"),
        );
        check_vec3(
            stickshift_vec3_normalize_or_zero(ca),
            a.normalize_or_zero(),
            &format_args!("{what}: normalize_or_zero"),
        );
        check_vec3(
            stickshift_vec3_lerp(ca, cb, t),
            a.lerp(b, t),
            &format_args!("{what}: lerp"),
        );
        check_quat(
            stickshift_quat_from_rotation_x(t),
            Quat::from_rotation_x(t),
            &format_args!("{what}: from_rotation_x"),
        );
        check_quat(
            stickshift_quat_from_rotation_y(t),
            Quat::from_rotation_y(t),
            &format_args!("{what}: from_rotation_y"),
        );
        check_quat(
            stickshift_quat_from_rotation_z(t),
            Quat::from_rotation_z(t),
            &format_args!("{what}: from_rotation_z"),
        );
        check_quat(
            stickshift_quat_mul(cq, cr),
            q * r,
            &format_args!("{what}: quat_mul"),
        );
        check_vec3(
            stickshift_quat_mul_vec3(cq, ca),
            q * a,
            &format_args!("{what}: quat_mul_vec3"),
        );
        check_vec3(
            stickshift_mat4_transform_point3(cm, ca),
            m.transform_point3(a),
            &format_args!("{what}: transform_point3"),
        );
        check_vec3(
            stickshift_mat4_transform_vector3(cm, ca),
            m.transform_vector3(a),
            &format_args!("{what}: transform_vector3"),
        );
        let transform = Transform {
            position: a,
            rotation: q,
            scale: b,
        };
        check_mat4(
            stickshift_transform_to_mat4(black_box(transform.into())),
            transform.to_matrix(),
            &format_args!("{what}: transform_to_mat4"),
        );
    }

    /// SplitMix64: a fixed seed, so a failure reproduces.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^ (z >> 31)
        }

        /// Uniform in `-scale..scale`.
        fn uniform(&mut self, scale: f32) -> f32 {
            let unit = (self.next() >> 40) as f32 / (1u64 << 24) as f32;
            (unit * 2.0 - 1.0) * scale
        }

        /// Mostly ordinary values at several scales, plus edge cases and any
        /// bit pattern at all (NaNs and infinities included).
        fn f32(&mut self) -> f32 {
            const EDGES: [f32; 18] = [
                0.0,
                -0.0,
                1.0,
                -1.0,
                0.5,
                f32::EPSILON,
                f32::MIN_POSITIVE,
                1e-45,
                1e-20,
                1e19,
                f32::MAX,
                f32::MIN,
                f32::INFINITY,
                f32::NEG_INFINITY,
                f32::NAN,
                FRAC_PI_2,
                PI,
                -PI,
            ];
            match self.next() % 10 {
                0 => EDGES[(self.next() % EDGES.len() as u64) as usize],
                1 => f32::from_bits(self.next() as u32),
                2 => self.uniform(1e-3),
                3 | 4 => self.uniform(1.0),
                5..=7 => self.uniform(1000.0),
                _ => self.uniform(1e6),
            }
        }

        fn vec3(&mut self) -> Vec3 {
            Vec3::new(self.f32(), self.f32(), self.f32())
        }

        /// A unit quaternion half the time, any four values otherwise.
        fn quat(&mut self) -> Quat {
            if self.next() & 1 == 0 {
                let axis = Vec3::new(self.uniform(1.0), self.uniform(1.0), self.uniform(1.0));
                let axis = axis.try_normalize().unwrap_or(Vec3::Y);
                Quat::from_axis_angle(axis, self.uniform(2.0 * PI))
            } else {
                Quat::from_xyzw(self.f32(), self.f32(), self.f32(), self.f32())
            }
        }

        /// A rigid-plus-scale matrix half the time, sixteen values otherwise.
        fn mat4(&mut self) -> Mat4 {
            if self.next() & 1 == 0 {
                let q = self.quat();
                Mat4::from_scale_rotation_translation(self.vec3(), q, self.vec3())
            } else {
                Mat4::from_cols_array(&std::array::from_fn(|_| self.f32()))
            }
        }
    }

    #[test]
    fn the_helpers_are_glam_bit_for_bit_on_random_inputs() {
        let mut rng = Rng(0x5717_c45f_0e4c_0de5);
        for i in 0..20_000 {
            let (a, b, t) = (rng.vec3(), rng.vec3(), rng.f32());
            let (q, r, m) = (rng.quat(), rng.quat(), rng.mat4());
            check_all(a, b, t, q, r, m, &format_args!("random case {i}"));
        }
    }

    #[test]
    fn the_transform_matrix_is_from_trs() {
        let mut rng = Rng(7);
        for i in 0..1_000 {
            let (p, q, s) = (rng.vec3(), rng.quat(), rng.vec3());
            let ours = stickshift_transform_to_mat4(StickShiftTransform {
                position: p.into(),
                rotation: q.into(),
                scale: s.into(),
            });
            let trs = stickshift_mat4_from_trs(p.into(), q.into(), s.into());
            check_lanes(&ours.m, &trs.m, &format_args!("case {i}"));
        }
        let t = Transform::from_position(Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(
            Transform::from(StickShiftTransform::from(t)).position,
            t.position
        );
    }

    #[test]
    fn normalize_or_zero_gives_zero_for_what_cannot_be_normalised() {
        for v in [
            Vec3::ZERO,
            Vec3::new(1e-45, 0.0, 0.0),
            Vec3::new(f32::MAX, f32::MAX, 0.0),
            Vec3::new(f32::INFINITY, 0.0, 0.0),
            Vec3::new(f32::NAN, 1.0, 0.0),
        ] {
            assert_eq!(
                stickshift_vec3_normalize_or_zero(v.into()),
                Vec3::ZERO.into(),
                "{v}"
            );
        }
        assert_eq!(
            stickshift_vec3_normalize_or_zero(v(0.0, 0.0, -3.0)),
            v(0.0, 0.0, -1.0)
        );
    }

    #[test]
    fn lerp_is_a_times_one_minus_t_plus_b_times_t() {
        // At t = 1 this form gives b exactly; `a + (b - a) * t` gives 0 here,
        // because 1 - 1e8 rounds to -1e8 in f32.
        assert_eq!(
            stickshift_vec3_lerp(v(1e8, 1e8, 1e8), v(1.0, 1.0, 1.0), 1.0),
            v(1.0, 1.0, 1.0)
        );
        // An ordinary input where the two forms differ in the last bit.
        let (a, b, t) = (-7.31f32, 6.95f32, 0.76f32);
        let ours = stickshift_vec3_lerp(v(a, a, a), v(b, b, b), t);
        let documented = black_box(a) * (1.0 - black_box(t)) + black_box(b) * black_box(t);
        let other_form = black_box(a) + (black_box(b) - black_box(a)) * black_box(t);
        assert_ne!(documented.to_bits(), other_form.to_bits());
        assert_eq!(ours, v(documented, documented, documented));
    }

    // The demo's inputs (games/playable-demo/src/config.rs).
    const CAR_LEN: f32 = 2.85;
    const CAR_BOTTOM: f32 = -0.4;
    const CAR_YAW: f32 = PI;
    const BUILDING_FOOTPRINT: f32 = 8.0;
    /// `ScrollingConfig::default().axis`, the direction the demo's `dot`s use.
    const AXIS: Vec3 = Vec3::new(0.0, 0.0, -1.0);

    /// The demo's own models: the car, traffic and building prefabs, the road
    /// tile and the skyline (games/playable-demo/assets).
    fn demo_models() -> Vec<(String, stickshift_assets::SceneAsset)> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../games/playable-demo/assets");
        let mut paths: Vec<_> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
            .map(|entry| entry.expect("a directory entry").path())
            .filter(|p| matches!(p.extension().and_then(|e| e.to_str()), Some("glb" | "gltf")))
            .collect();
        paths.sort();
        let models: Vec<_> = paths
            .iter()
            .map(|p| {
                let scene = stickshift_assets::import_gltf(p)
                    .unwrap_or_else(|e| panic!("{}: {e}", p.display()));
                (p.file_name().unwrap().to_string_lossy().into_owned(), scene)
            })
            .collect();
        assert!(
            models.len() >= 12,
            "found only {} demo models in {}",
            models.len(),
            dir.display()
        );
        models
    }

    /// The demo's fit math (`fit_transform` and `building_fit`,
    /// games/playable-demo/src/assets.rs:84-143), done once with glam and
    /// once through the helpers, step by step: the baked vertices, the
    /// pivots, the car's yaw, the fitted transforms, their matrices, and each
    /// node's local matrix.
    #[test]
    fn the_helpers_are_glam_bit_for_bit_on_the_demo_fit_math() {
        let yaw = black_box(CAR_YAW);
        let rotation = Quat::from_rotation_y(yaw);
        let c_rotation = stickshift_quat_from_rotation_y(yaw);
        check_quat(c_rotation, rotation, &"the car yaw");

        let facing = Quat::from_rotation_y(black_box(-FRAC_PI_2));
        let mut last = Vec3::ZERO;
        let mut vertices = 0usize;
        for (name, scene) in demo_models() {
            let mut min = Vec3::splat(f32::INFINITY);
            let mut max = Vec3::splat(f32::NEG_INFINITY);
            for (n, node) in scene.mesh_nodes() {
                let mesh = &scene.meshes[node.mesh.expect("mesh node has a mesh")];
                let world = black_box(StickShiftMat4::from(node.world));
                for (i, p) in mesh.positions.iter().enumerate() {
                    let p = Vec3::from_array(*p);
                    let w = node.world.transform_point3(p);
                    let what = format_args!("{name} node {n} vertex {i}");
                    check_vec3(stickshift_mat4_transform_point3(world, p.into()), w, &what);
                    check_vec3(
                        stickshift_mat4_transform_vector3(world, p.into()),
                        node.world.transform_vector3(p),
                        &what,
                    );
                    check_f32(
                        stickshift_vec3_dot(w.into(), AXIS.into()),
                        w.dot(AXIS),
                        &what,
                    );
                    // And every other helper, on this vertex and the last.
                    check_all(w, last, 0.25, rotation, facing, node.world, &what);
                    last = w;
                    min = min.min(w);
                    max = max.max(w);
                    vertices += 1;
                }
            }

            let size = max - min;
            let horizontal = size.x.max(size.z).max(f32::EPSILON);
            let pivot = Vec3::new((min.x + max.x) * 0.5, min.y, (min.z + max.z) * 0.5);

            // fit_transform: scale, yaw about the pivot, lift to CAR_BOTTOM.
            let scale = CAR_LEN / horizontal;
            let scaled = pivot * scale;
            let rotated = rotation * scaled;
            let c_rotated = stickshift_quat_mul_vec3(c_rotation, scaled.into());
            check_vec3(c_rotated, rotated, &format_args!("{name}: the car pivot"));
            let car = Transform {
                position: -rotated + Vec3::new(0.0, CAR_BOTTOM, 0.0),
                rotation,
                scale: Vec3::splat(scale),
            };

            // building_fit: scale, pivot to the origin, no rotation.
            let scale = BUILDING_FOOTPRINT / horizontal;
            let building = Transform {
                position: -(pivot * scale),
                rotation: Quat::IDENTITY,
                scale: Vec3::splat(scale),
            };

            for (fit, label) in [(car, "car fit"), (building, "building fit")] {
                let matrix = stickshift_transform_to_mat4(black_box(fit.into()));
                check_mat4(matrix, fit.to_matrix(), &format_args!("{name}: {label}"));
                // load_model's per-node local matrix: fit × node (assets.rs:61).
                for (n, node) in scene.mesh_nodes() {
                    check_mat4(
                        stickshift_mat4_mul(matrix, node.world.into()),
                        fit.to_matrix() * node.world,
                        &format_args!("{name}: {label} × node {n}"),
                    );
                }
            }
        }
        assert!(
            vertices > 1_000,
            "only {vertices} vertices: the models did not load"
        );
    }

    /// The facing quaternions the demo spawns its buildings with
    /// (`Quat::from_rotation_y(-sign * FRAC_PI_2)`, game.rs:417), and the
    /// rotations they apply to the travel axis and to each other.
    #[test]
    fn the_helpers_are_glam_bit_for_bit_on_the_demo_facing_angles() {
        let angles = [-FRAC_PI_2, FRAC_PI_2, CAR_YAW, -CAR_YAW, 0.0, -0.0];
        for &sign in &[1.0f32, -1.0] {
            let angle = black_box(-sign * FRAC_PI_2);
            assert!(angles.iter().any(|a| a.to_bits() == angle.to_bits()));
        }
        let probes = [AXIS, Vec3::X, Vec3::Y, Vec3::new(3.25, -0.5, -120.0)];
        for &a in &angles {
            let a = black_box(a);
            let q = Quat::from_rotation_y(a);
            let c = stickshift_quat_from_rotation_y(a);
            check_quat(c, q, &format_args!("from_rotation_y({a})"));
            check_quat(
                stickshift_quat_from_rotation_x(a),
                Quat::from_rotation_x(a),
                &a,
            );
            check_quat(
                stickshift_quat_from_rotation_z(a),
                Quat::from_rotation_z(a),
                &a,
            );
            for &p in &probes {
                let what = format_args!("from_rotation_y({a}) * {p}");
                check_vec3(
                    stickshift_quat_mul_vec3(c, black_box(p).into()),
                    q * p,
                    &what,
                );
            }
            for &b in &angles {
                let r = Quat::from_rotation_y(b);
                check_quat(
                    stickshift_quat_mul(c, black_box(r.into())),
                    q * r,
                    &format_args!("from_rotation_y({a}) * from_rotation_y({b})"),
                );
            }
        }
    }
}
