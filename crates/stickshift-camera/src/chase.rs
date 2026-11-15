// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! [`ChaseController`] — a follow camera that trails a moving target.
//!
//! Given a target world position and a facing direction, the controller places
//! the camera **behind and above** the target and points it at the target (with a
//! small look-ahead height so the framing sits a touch above the ground). The
//! placement is a pure function of a few offsets, so it is fully deterministic and
//! engine-generic — it names no game type.
//!
//! # Placement math
//!
//! For a target at `T` moving toward unit `forward`, with world up `up`,
//! back-distance `distance`, and height `height`:
//!
//! ```text
//! camera_position = T - forward * distance + up * height
//! camera_target   = T + up * look_at_height
//! ```
//!
//! The camera therefore sits `distance` behind the target along its facing and
//! `height` above it, looking slightly up at the target.
//!
//! # Smoothing
//!
//! With a **half-life** in seconds, each [`follow`](ChaseController::follow)
//! closes part of the gap between the camera's current pose and the desired
//! pose: half of it for every half-life of `dt` that passes. The fraction moved
//! in one call is
//!
//! ```text
//! t = 1 - 2^(-dt / half_life)
//! ```
//!
//! so the ease depends only on elapsed time, not on how often `follow` is
//! called: two calls of `dt / 2` land where one call of `dt` does, and the
//! camera trails identically at 30, 60 or 120 Hz. A half-life of `0.0` (the
//! default) disables smoothing and the camera snaps to the desired pose; small
//! half-lives approach that snap continuously.

use stickshift_math::glam::Vec3;

use crate::camera::Camera;

/// A follow camera that trails a target from behind and above.
///
/// Configure the back-distance, height, and (optional) smoothing, then call
/// [`follow`](Self::follow) each frame with the target's position and facing to
/// drive a [`Camera`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChaseController {
    /// Distance the camera trails behind the target, along the target's facing.
    distance: f32,
    /// Height the camera sits above the target, along the world up axis.
    height: f32,
    /// Height above the target the camera aims at (keeps framing slightly high).
    look_at_height: f32,
    /// World up axis used for the height offset and the look-at up vector.
    up: Vec3,
    /// Seconds for the camera to close half the gap to its desired pose
    /// (`0.0` snaps; larger trails more).
    half_life: f32,
}

impl ChaseController {
    /// A chase controller trailing `distance` behind and `height` above the
    /// target, with no smoothing (the camera snaps to the desired pose).
    ///
    /// Uses `+Y` as the world up axis and aims at the target itself
    /// (`look_at_height == 0`). Use the builder-style setters to adjust.
    ///
    /// # Example
    ///
    /// ```rust
    /// use stickshift_camera::ChaseController;
    ///
    /// let chase = ChaseController::new(8.0, 4.0);
    /// ```
    #[must_use]
    pub fn new(distance: f32, height: f32) -> Self {
        Self {
            distance,
            height,
            look_at_height: 0.0,
            up: Vec3::Y,
            half_life: 0.0,
        }
    }

    /// Set the world up axis (default `+Y`). It is normalized; a zero or
    /// non-finite axis, which has no direction, keeps `+Y`. Returns `self` for
    /// chaining.
    #[must_use]
    pub fn with_up(mut self, up: Vec3) -> Self {
        self.up = up.try_normalize().unwrap_or(Vec3::Y);
        self
    }

    /// Set the height above the target the camera aims at (default `0.0`).
    /// Returns `self` for chaining.
    #[must_use]
    pub fn with_look_at_height(mut self, look_at_height: f32) -> Self {
        self.look_at_height = look_at_height;
        self
    }

    /// Set the smoothing half-life in seconds (default `0.0`, i.e. snap).
    ///
    /// The camera closes half of the gap to its desired pose every
    /// `half_life` seconds, whatever the frame rate (see the
    /// [module docs](self#smoothing)), so larger values trail the target more
    /// loosely. A negative or NaN value is treated as `0.0`. Returns `self` for
    /// chaining.
    #[must_use]
    pub fn with_half_life(mut self, half_life: f32) -> Self {
        self.half_life = half_life.max(0.0);
        self
    }

    /// The fraction of the gap to the desired pose that one
    /// [`follow`](Self::follow) of `dt` seconds closes, in `0.0..=1.0`.
    #[must_use]
    pub fn follow_fraction(&self, dt: f32) -> f32 {
        if self.half_life <= 0.0 {
            return 1.0;
        }
        let dt = dt.max(0.0);
        (1.0 - (-dt / self.half_life).exp2()).clamp(0.0, 1.0)
    }

    /// The desired camera position for a target at `target` facing `forward`.
    ///
    /// `camera_position = target - forward_norm * distance + up * height`.
    #[must_use]
    pub fn desired_position(&self, target: Vec3, forward: Vec3) -> Vec3 {
        let forward = normalize_or(forward, -Vec3::Z);
        target - forward * self.distance + self.up * self.height
    }

    /// The desired look-at point for a target at `target`
    /// (`target + up * look_at_height`).
    #[must_use]
    pub fn desired_target(&self, target: Vec3) -> Vec3 {
        target + self.up * self.look_at_height
    }

    /// Place `camera` to follow a target at `target` facing `forward`, `dt`
    /// seconds after the previous call.
    ///
    /// Computes the desired behind-and-above pose, then (with a half-life) eases
    /// the camera's current position/target toward it by
    /// [`follow_fraction`](Self::follow_fraction)`(dt)`, or (with none) snaps to
    /// it. Pass the time step the call covers: the fixed `dt` when following from
    /// a game's `update`, or the time since the last frame when following from
    /// `render`. `forward` need not be normalized; a zero `forward` falls back to
    /// `-Z`.
    ///
    /// # Example
    ///
    /// ```rust
    /// use stickshift_camera::{Camera, ChaseController};
    /// use stickshift_math::glam::Vec3;
    ///
    /// let mut camera = Camera::new(16.0 / 9.0);
    /// let chase = ChaseController::new(8.0, 4.0);
    /// // Target at the origin moving toward -Z, one 60 Hz step later.
    /// chase.follow(&mut camera, Vec3::ZERO, Vec3::new(0.0, 0.0, -1.0), 1.0 / 60.0);
    /// // Camera sits behind (+Z) and above (+Y) the target.
    /// assert!(camera.position().z > 0.0);
    /// assert!(camera.position().y > 0.0);
    /// ```
    pub fn follow(&self, camera: &mut Camera, target: Vec3, forward: Vec3, dt: f32) {
        let desired_pos = self.desired_position(target, forward);
        let desired_target = self.desired_target(target);

        let t = self.follow_fraction(dt);
        let (new_pos, new_target) = if t >= 1.0 {
            (desired_pos, desired_target)
        } else {
            (
                camera.position().lerp(desired_pos, t),
                camera.target().lerp(desired_target, t),
            )
        };

        camera.set_position(new_pos);
        camera.set_target(new_target);
        camera.set_up(self.up);
    }
}

/// Normalize `v`, or return `fallback` if `v` is (near) zero-length.
fn normalize_or(v: Vec3, fallback: Vec3) -> Vec3 {
    let len = v.length();
    if len > 1e-6 {
        v / len
    } else {
        fallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One 60 Hz step.
    const STEP: f32 = 1.0 / 60.0;

    #[test]
    fn places_camera_behind_and_above_target() {
        let chase = ChaseController::new(8.0, 4.0);
        // Target moving toward -Z.
        let pos = chase.desired_position(Vec3::ZERO, Vec3::new(0.0, 0.0, -1.0));
        // Behind (+Z by distance) and above (+Y by height).
        assert!((pos - Vec3::new(0.0, 4.0, 8.0)).length() < 1e-5);
    }

    #[test]
    fn follow_snaps_without_smoothing() {
        let mut cam = Camera::new(1.0);
        let chase = ChaseController::new(10.0, 5.0);
        chase.follow(
            &mut cam,
            Vec3::new(0.0, 0.0, -20.0),
            Vec3::new(0.0, 0.0, -1.0),
            STEP,
        );
        assert!((cam.position() - Vec3::new(0.0, 5.0, -10.0)).length() < 1e-5);
        assert!((cam.target() - Vec3::new(0.0, 0.0, -20.0)).length() < 1e-5);
    }

    #[test]
    fn follow_tracks_target_movement() {
        let mut cam = Camera::new(1.0);
        let chase = ChaseController::new(6.0, 3.0);
        chase.follow(&mut cam, Vec3::ZERO, Vec3::new(0.0, 0.0, -1.0), STEP);
        let first = cam.position();
        chase.follow(
            &mut cam,
            Vec3::new(0.0, 0.0, -10.0),
            Vec3::new(0.0, 0.0, -1.0),
            STEP,
        );
        // The camera followed the target along -Z.
        assert!(cam.position().z < first.z);
    }

    #[test]
    fn smoothing_eases_toward_desired_pose() {
        let mut cam = Camera::new(1.0);
        cam.set_position(Vec3::ZERO);
        cam.set_target(Vec3::ZERO);
        let chase = ChaseController::new(10.0, 5.0).with_half_life(STEP);
        let desired = chase.desired_position(Vec3::ZERO, Vec3::new(0.0, 0.0, -1.0));
        chase.follow(&mut cam, Vec3::ZERO, Vec3::new(0.0, 0.0, -1.0), STEP);
        // One half-life closed half the gap, not all of it.
        let halfway = Vec3::ZERO.lerp(desired, 0.5);
        assert!((cam.position() - halfway).length() < 1e-5);
        // Repeated follows converge on the desired pose.
        for _ in 0..40 {
            chase.follow(&mut cam, Vec3::ZERO, Vec3::new(0.0, 0.0, -1.0), STEP);
        }
        assert!((cam.position() - desired).length() < 1e-3);
    }

    /// Where the camera is after following a still target for `seconds`, in
    /// calls of `dt`.
    fn eased_for(chase: &ChaseController, seconds: f32, dt: f32) -> Vec3 {
        let mut cam = Camera::new(1.0);
        cam.set_position(Vec3::ZERO);
        cam.set_target(Vec3::ZERO);
        let calls = (seconds / dt).round() as u32;
        for _ in 0..calls {
            chase.follow(&mut cam, Vec3::ZERO, Vec3::new(0.0, 0.0, -1.0), dt);
        }
        cam.position()
    }

    #[test]
    fn smoothing_does_not_depend_on_the_frame_rate() {
        let chase = ChaseController::new(10.0, 5.0).with_half_life(0.1);
        let at_30 = eased_for(&chase, 0.2, 1.0 / 30.0);
        let at_60 = eased_for(&chase, 0.2, 1.0 / 60.0);
        let at_120 = eased_for(&chase, 0.2, 1.0 / 120.0);
        assert!((at_30 - at_60).length() < 1e-4, "{at_30} vs {at_60}");
        assert!((at_60 - at_120).length() < 1e-4, "{at_60} vs {at_120}");
        // Two half-lives close three quarters of the gap.
        let desired = chase.desired_position(Vec3::ZERO, Vec3::new(0.0, 0.0, -1.0));
        let expected = desired * 0.75;
        assert!((at_60 - expected).length() < 1e-4, "{at_60} vs {expected}");
    }

    #[test]
    fn a_tiny_half_life_is_almost_a_snap() {
        // No jump between "off" and "on": the ease shrinks to a snap smoothly.
        let snap = ChaseController::new(10.0, 5.0);
        let tiny = ChaseController::new(10.0, 5.0).with_half_life(1e-4);
        assert_eq!(snap.follow_fraction(STEP), 1.0);
        assert!(tiny.follow_fraction(STEP) > 0.999);
        // No time passing moves nothing; a negative half-life snaps.
        let eased = ChaseController::new(10.0, 5.0).with_half_life(0.1);
        assert_eq!(eased.follow_fraction(0.0), 0.0);
        assert_eq!(eased.follow_fraction(-1.0), 0.0);
        assert_eq!(
            ChaseController::new(1.0, 1.0)
                .with_half_life(-1.0)
                .follow_fraction(STEP),
            1.0
        );
    }

    #[test]
    fn a_zero_up_axis_keeps_plus_y() {
        let chase = ChaseController::new(4.0, 2.0).with_up(Vec3::ZERO);
        let mut cam = Camera::new(1.0);
        chase.follow(&mut cam, Vec3::ZERO, Vec3::NEG_Z, STEP);
        assert_eq!(cam.up(), Vec3::Y);
        assert!(cam.view_projection_matrix().is_finite());
        assert_eq!(
            ChaseController::new(4.0, 2.0)
                .with_up(Vec3::new(0.0, 0.0, 3.0))
                .desired_position(Vec3::ZERO, Vec3::X),
            Vec3::new(-4.0, 0.0, 2.0),
            "a valid axis is still normalized"
        );
    }

    #[test]
    fn zero_forward_falls_back_to_minus_z() {
        let chase = ChaseController::new(4.0, 0.0);
        let pos = chase.desired_position(Vec3::ZERO, Vec3::ZERO);
        // Fallback forward -Z ⇒ camera behind at +Z.
        assert!((pos - Vec3::new(0.0, 0.0, 4.0)).length() < 1e-5);
    }

    #[test]
    fn look_at_height_raises_the_target() {
        let chase = ChaseController::new(4.0, 2.0).with_look_at_height(1.0);
        let t = chase.desired_target(Vec3::new(1.0, 0.0, -3.0));
        assert!((t - Vec3::new(1.0, 1.0, -3.0)).length() < 1e-5);
    }
}
