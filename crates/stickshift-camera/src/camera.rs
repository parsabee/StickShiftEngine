// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! A target-based perspective [`Camera`] producing view / projection matrices.
//!
//! This is the migrated core of the prototype camera, trimmed to what the
//! engine's render seam actually consumes: a look-at view
//! matrix, a right-handed perspective projection, their product, the aspect
//! ratio and field of view, position/target setters, and the forward/right
//! basis vectors. The
//! prototype's free-fly `move_*` / orbit `rotate_*` controllers are
//! intentionally not migrated here — the runner needs
//! a chase camera (see [`crate::chase`]), and those controllers migrate lazily if
//! a consumer needs them.

use stickshift_math::glam::{Mat4, Vec3};

/// A perspective camera positioned at `position` and looking toward `target`.
///
/// The camera uses a right-handed "look-at" system with `up` as the world up
/// axis and a right-handed perspective projection. It exposes the view /
/// projection / view-projection matrices the render seam carries, plus the
/// forward / right basis vectors.
///
/// # Degenerate poses
///
/// The basis and the view matrix stay finite in every pose, so a degenerate one
/// can't turn the frame into NaN:
///
/// - With `position == target` there is no view direction; the camera looks
///   along `-Z` (the engine's travel direction, as
///   [`ChaseController`](crate::ChaseController) assumes for a zero facing).
/// - With `up` zero, or parallel to the view direction (looking straight up or
///   down), the view uses the world axis least aligned with the view instead,
///   preferring `+Y`, then `-Z`, then `+X`.
///
/// Every other pose produces exactly the matrices and vectors it always did.
///
/// # Example
///
/// ```rust
/// use stickshift_camera::Camera;
/// use stickshift_math::glam::Vec3;
///
/// let mut camera = Camera::new(16.0 / 9.0);
/// camera.set_position(Vec3::new(0.0, 5.0, 10.0));
/// camera.set_target(Vec3::ZERO);
/// let view_projection = camera.view_projection_matrix();
/// assert!(view_projection.is_finite());
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    position: Vec3,
    target: Vec3,
    up: Vec3,
    fov_y: f32,
    aspect_ratio: f32,
    near: f32,
    far: f32,
}

impl Camera {
    /// Create a camera with the given viewport aspect ratio and sensible
    /// defaults: 45° vertical FOV, a `0.1..100.0` clip range, `+Y` up, and an
    /// elevated, pulled-back placement looking at the origin.
    ///
    /// # Arguments
    ///
    /// * `aspect_ratio` - viewport width / height (e.g. `16.0 / 9.0`).
    ///
    /// # Example
    ///
    /// ```rust
    /// use stickshift_camera::Camera;
    ///
    /// let camera = Camera::new(16.0 / 9.0);
    /// ```
    #[must_use]
    pub fn new(aspect_ratio: f32) -> Self {
        Self {
            position: Vec3::new(0.0, 5.0, 8.0),
            target: Vec3::ZERO,
            up: Vec3::Y,
            fov_y: 45.0_f32.to_radians(),
            aspect_ratio,
            near: 0.1,
            far: 100.0,
        }
    }

    /// The right-handed view matrix (world → view space). Finite in every pose
    /// (see [Degenerate poses](Self#degenerate-poses)).
    #[must_use]
    pub fn view_matrix(&self) -> Mat4 {
        match self.basis() {
            Basis::Regular => Mat4::look_at_rh(self.position, self.target, self.up),
            Basis::Fallback { forward, up } => Mat4::look_to_rh(self.position, forward, up),
        }
    }

    /// The right-handed perspective projection matrix (view → clip space).
    #[must_use]
    pub fn projection_matrix(&self) -> Mat4 {
        Mat4::perspective_rh(self.fov_y, self.aspect_ratio, self.near, self.far)
    }

    /// The combined view-projection matrix (world → clip space).
    ///
    /// Equivalent to `projection_matrix() * view_matrix()`.
    #[must_use]
    pub fn view_projection_matrix(&self) -> Mat4 {
        self.projection_matrix() * self.view_matrix()
    }

    /// Update the viewport aspect ratio (width / height); call this on resize.
    pub fn set_aspect_ratio(&mut self, aspect_ratio: f32) {
        self.aspect_ratio = aspect_ratio;
    }

    /// The viewport aspect ratio (width / height) the projection uses: the value
    /// given to [`new`](Self::new) or the last
    /// [`set_aspect_ratio`](Self::set_aspect_ratio), exactly as stored.
    #[must_use]
    pub fn aspect_ratio(&self) -> f32 {
        self.aspect_ratio
    }

    /// Set the vertical field of view, in **radians**. The default is 45°
    /// (`45.0_f32.to_radians()`).
    ///
    /// Stored as given, like [`set_aspect_ratio`](Self::set_aspect_ratio): a value
    /// that is not finite, or not strictly between 0 and π, gives a non-finite or
    /// degenerate [`projection_matrix`](Self::projection_matrix).
    ///
    /// # Example
    ///
    /// ```rust
    /// use stickshift_camera::Camera;
    ///
    /// let mut camera = Camera::new(16.0 / 9.0);
    /// assert_eq!(camera.fov_y(), 45.0_f32.to_radians());
    /// camera.set_fov_y(60.0_f32.to_radians());
    /// assert_eq!(camera.fov_y(), 60.0_f32.to_radians());
    /// ```
    pub fn set_fov_y(&mut self, fov_y: f32) {
        self.fov_y = fov_y;
    }

    /// The vertical field of view, in radians: 45° until
    /// [`set_fov_y`](Self::set_fov_y), exactly as stored.
    #[must_use]
    pub fn fov_y(&self) -> f32 {
        self.fov_y
    }

    /// Set the near and far clip distances (view-space depth, world units).
    ///
    /// The defaults (`0.1..100.0`) suit a small scene; a game that draws further
    /// out pushes `far` beyond its farthest visible geometry, and may raise `near`
    /// in proportion to keep the depth buffer's precision (which scales with
    /// `near`) where it was. Both must be finite and positive with
    /// `near < far`; an invalid range is ignored and the camera keeps its
    /// previous planes, so the projection never goes non-finite.
    ///
    /// # Example
    ///
    /// ```rust
    /// use stickshift_camera::Camera;
    ///
    /// let mut camera = Camera::new(16.0 / 9.0);
    /// camera.set_clip_planes(0.3, 300.0);
    /// assert_eq!(camera.clip_planes(), (0.3, 300.0));
    /// ```
    pub fn set_clip_planes(&mut self, near: f32, far: f32) {
        if near.is_finite() && far.is_finite() && near > 0.0 && far > near {
            self.near = near;
            self.far = far;
        }
    }

    /// The current `(near, far)` clip distances.
    #[must_use]
    pub fn clip_planes(&self) -> (f32, f32) {
        (self.near, self.far)
    }

    /// Set the camera's world position.
    pub fn set_position(&mut self, position: Vec3) {
        self.position = position;
    }

    /// Set the point the camera looks at.
    pub fn set_target(&mut self, target: Vec3) {
        self.target = target;
    }

    /// Set the world up axis used to orient the view.
    pub fn set_up(&mut self, up: Vec3) {
        self.up = up;
    }

    /// The camera's current world position.
    #[must_use]
    pub fn position(&self) -> Vec3 {
        self.position
    }

    /// The point the camera is currently looking at.
    #[must_use]
    pub fn target(&self) -> Vec3 {
        self.target
    }

    /// The world up axis.
    #[must_use]
    pub fn up(&self) -> Vec3 {
        self.up
    }

    /// The normalized forward direction (position → target), or `-Z` when the
    /// position is the target.
    #[must_use]
    pub fn forward(&self) -> Vec3 {
        (self.target - self.position)
            .try_normalize()
            .unwrap_or(Vec3::NEG_Z)
    }

    /// The normalized right direction (perpendicular to forward and up). Finite
    /// in every pose (see [Degenerate poses](Self#degenerate-poses)).
    #[must_use]
    pub fn right(&self) -> Vec3 {
        let forward = self.forward();
        forward
            .cross(self.up)
            .try_normalize()
            .unwrap_or_else(|| forward.cross(fallback_up(forward)).normalize())
    }

    /// Whether the pose has a usable view direction and up axis, and the
    /// replacements to use if not.
    fn basis(&self) -> Basis {
        let direction = self.target - self.position;
        match direction.try_normalize() {
            Some(forward) if forward.cross(self.up).try_normalize().is_some() => Basis::Regular,
            forward => {
                let forward = forward.unwrap_or(Vec3::NEG_Z);
                let up = match forward.cross(self.up).try_normalize() {
                    Some(_) => self.up,
                    None => fallback_up(forward),
                };
                Basis::Fallback { forward, up }
            }
        }
    }
}

/// How [`Camera::view_matrix`] orients the view.
enum Basis {
    /// `position`, `target` and `up` define it, as they always have.
    Regular,
    /// A degenerate pose: look along `forward` with `up`.
    Fallback { forward: Vec3, up: Vec3 },
}

/// The world axis least aligned with unit `forward`: `+Y`, then `-Z`, then
/// `+X` on a tie. Never parallel to `forward`, so it is always a usable up.
fn fallback_up(forward: Vec3) -> Vec3 {
    [Vec3::Y, Vec3::NEG_Z, Vec3::X]
        .into_iter()
        .min_by(|a, b| a.dot(forward).abs().total_cmp(&b.dot(forward).abs()))
        .unwrap_or(Vec3::Y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_projection_is_finite_and_not_identity() {
        let cam = Camera::new(16.0 / 9.0);
        assert_ne!(cam.view_projection_matrix(), Mat4::IDENTITY);
        assert!(cam.view_projection_matrix().is_finite());
    }

    #[test]
    fn view_projection_is_projection_times_view() {
        let cam = Camera::new(4.0 / 3.0);
        let expected = cam.projection_matrix() * cam.view_matrix();
        assert_eq!(cam.view_projection_matrix(), expected);
    }

    #[test]
    fn setters_update_state() {
        let mut cam = Camera::new(1.0);
        cam.set_position(Vec3::new(1.0, 2.0, 3.0));
        cam.set_target(Vec3::new(4.0, 5.0, 6.0));
        assert_eq!(cam.position(), Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(cam.target(), Vec3::new(4.0, 5.0, 6.0));
    }

    #[test]
    fn forward_and_right_are_unit_and_orthogonal() {
        let mut cam = Camera::new(1.0);
        cam.set_position(Vec3::new(0.0, 0.0, 5.0));
        cam.set_target(Vec3::ZERO);
        assert!((cam.forward().length() - 1.0).abs() < 1e-5);
        assert!((cam.right().length() - 1.0).abs() < 1e-5);
        assert!(cam.forward().dot(cam.right()).abs() < 1e-4);
    }

    #[test]
    fn forward_points_from_position_to_target() {
        let mut cam = Camera::new(1.0);
        cam.set_position(Vec3::new(0.0, 0.0, 10.0));
        cam.set_target(Vec3::ZERO);
        // Looking down -Z.
        assert!((cam.forward() - Vec3::new(0.0, 0.0, -1.0)).length() < 1e-5);
    }

    #[test]
    fn a_regular_pose_keeps_its_exact_matrices() {
        let mut cam = Camera::new(16.0 / 9.0);
        cam.set_position(Vec3::new(1.5, 4.0, 9.0));
        cam.set_target(Vec3::new(0.2, 1.0, -3.0));
        let up = Vec3::new(0.0, 2.0, 0.1);
        cam.set_up(up);
        // Bit for bit what it always was, so nothing rendered changes.
        assert_eq!(
            cam.view_matrix(),
            Mat4::look_at_rh(cam.position(), cam.target(), up)
        );
        assert_eq!(cam.forward(), (cam.target() - cam.position()).normalize());
        assert_eq!(cam.right(), cam.forward().cross(up).normalize());
    }

    /// Every degenerate pose still yields a finite, orthonormal basis and a
    /// finite view-projection.
    #[test]
    fn degenerate_poses_stay_finite() {
        let finite = |cam: &Camera, what: &str| {
            assert!(
                cam.view_projection_matrix().is_finite(),
                "{what}: view-projection"
            );
            let (forward, right) = (cam.forward(), cam.right());
            assert!(
                (forward.length() - 1.0).abs() < 1e-5,
                "{what}: forward {forward}"
            );
            assert!((right.length() - 1.0).abs() < 1e-5, "{what}: right {right}");
            assert!(forward.dot(right).abs() < 1e-5, "{what}: orthogonal");
        };

        // Position on the target: no view direction.
        let mut cam = Camera::new(1.0);
        cam.set_position(Vec3::new(3.0, 1.0, 2.0));
        cam.set_target(Vec3::new(3.0, 1.0, 2.0));
        finite(&cam, "position == target");
        assert_eq!(cam.forward(), Vec3::NEG_Z);

        // Looking straight down and straight up the up axis.
        for target_y in [-10.0, 10.0] {
            let mut cam = Camera::new(1.0);
            cam.set_position(Vec3::ZERO);
            cam.set_target(Vec3::new(0.0, target_y, 0.0));
            finite(&cam, "parallel to up");
        }

        // A zero up axis.
        let mut cam = Camera::new(1.0);
        cam.set_up(Vec3::ZERO);
        finite(&cam, "zero up");
    }

    #[test]
    fn an_invalid_clip_range_is_ignored() {
        let mut cam = Camera::new(1.0);
        cam.set_clip_planes(0.3, 300.0);
        for (near, far) in [
            (0.0, 10.0),
            (-1.0, 10.0),
            (5.0, 5.0),
            (5.0, 1.0),
            (f32::NAN, 10.0),
            (0.1, f32::INFINITY),
        ] {
            cam.set_clip_planes(near, far);
            assert_eq!(cam.clip_planes(), (0.3, 300.0), "{near}..{far} ignored");
        }
        assert!(cam.projection_matrix().is_finite());
    }

    #[test]
    fn aspect_ratio_change_alters_projection() {
        let mut cam = Camera::new(1.0);
        let before = cam.projection_matrix();
        cam.set_aspect_ratio(2.0);
        assert_ne!(before, cam.projection_matrix());
    }

    #[test]
    fn known_projection_entries() {
        // Right-handed perspective: m[0][0] = f / aspect, m[1][1] = f, where
        // f = 1 / tan(fov_y / 2). Check the aspect scaling is applied to X.
        let cam = Camera::new(2.0);
        let proj = cam.projection_matrix();
        let cols = proj.to_cols_array_2d();
        let f = 1.0 / (45.0_f32.to_radians() / 2.0).tan();
        assert!((cols[0][0] - f / 2.0).abs() < 1e-4);
        assert!((cols[1][1] - f).abs() < 1e-4);
    }

    #[test]
    fn the_getters_report_the_stored_fov_and_aspect() {
        let mut cam = Camera::new(4.0 / 3.0);
        assert_eq!(cam.aspect_ratio().to_bits(), (4.0_f32 / 3.0).to_bits());
        assert_eq!(cam.fov_y().to_bits(), 45.0_f32.to_radians().to_bits());

        // Plain stores: what goes in comes back bit for bit, even a value the
        // projection cannot use.
        for aspect in [1600.0 / 1200.0, 640.0, -0.0, f32::NAN] {
            cam.set_aspect_ratio(aspect);
            assert_eq!(cam.aspect_ratio().to_bits(), aspect.to_bits());
        }
        for fov in [0.3, 1.2, f32::INFINITY] {
            cam.set_fov_y(fov);
            assert_eq!(cam.fov_y().to_bits(), fov.to_bits());
        }
    }

    #[test]
    fn the_projection_uses_the_fov_it_was_given() {
        let mut cam = Camera::new(16.0 / 9.0);
        let default = cam.projection_matrix();
        cam.set_fov_y(cam.fov_y());
        assert_eq!(
            cam.projection_matrix(),
            default,
            "the same fov, the same bits"
        );

        let fov = 70.0_f32.to_radians();
        cam.set_fov_y(fov);
        let (near, far) = cam.clip_planes();
        assert_eq!(
            cam.projection_matrix(),
            Mat4::perspective_rh(fov, 16.0 / 9.0, near, far)
        );
        assert_ne!(cam.projection_matrix(), default);
    }
}
