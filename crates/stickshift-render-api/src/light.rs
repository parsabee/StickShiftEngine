// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! **Local lights**: point and spot lights a frame adds next to its one sun.
//!
//! A [`Light`] is plain data, added to the open frame with
//! [`FrameRecorder::add_light`](crate::FrameRecorder::add_light). Unlike the sun
//! and the camera, lights are **not sticky**: each frame starts with none, and
//! every light added before [`submit`](crate::FrameRecorder::submit) lights every
//! draw in that frame, whatever order the calls came in. A game with headlights,
//! streetlights and lit windows re-adds them every frame from its own state.
//!
//! # Falloff
//!
//! A light's brightness falls off with the **inverse square** of the distance,
//! and is faded smoothly to exactly zero at its [`range`](Light::range), so a
//! light never reaches past it and the renderer can skip it there:
//!
//! ```text
//! falloff(d) = (1 - (d / range)^4)^2 / max(d, LIGHT_MIN_DISTANCE)^2
//! ```
//!
//! [`intensity`](Light::intensity) is therefore roughly the light's brightness
//! at one world unit, on the same scale as the sun's
//! [`sun_intensity`](crate::SunSky::sun_intensity): a streetlight 8 units above
//! the road needs an intensity near 50 to light it about as brightly as a sun of
//! `0.8`. [`Light::falloff`] computes the curve, so a game can ask how lit a
//! point is without a GPU.
//!
//! # Spot cones
//!
//! A spot light shines along its `direction` (the way the light travels, like
//! [`SunSky::direction`](crate::SunSky::direction)). Its angles are **half
//! angles** in degrees, measured from that axis: inside `inner_angle_deg` the
//! light is at full strength, past `outer_angle_deg` it is gone, and in between
//! it fades smoothly ([`Light::cone`]).
//!
//! # Budget
//!
//! A frame holds up to [`MAX_LIGHTS`] lights; a backend ignores the rest. The
//! Metal backend also has a per-frame budget for how many screen regions the
//! lights cover in total, documented on its light-binning module; a frame that
//! exceeds it drops whole lights that no longer fit (never parts of one), so
//! lights added earlier keep priority.

use stickshift_math::glam::Vec3;

/// The most lights one frame can hold. A backend ignores lights added past it.
pub const MAX_LIGHTS: usize = 256;

/// The distance below which [`Light::falloff`] stops rising: a surface closer
/// than this to a light is lit as if it were this far away, so the inverse
/// square never divides by zero.
pub const LIGHT_MIN_DISTANCE: f32 = 0.1;

/// The widest spot cone, as a half angle in degrees: a hemisphere.
pub const MAX_SPOT_ANGLE_DEG: f32 = 90.0;

/// What shape of light a [`Light`] is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LightKind {
    /// Shines equally in every direction from its position.
    Point,
    /// Shines in a cone from its position.
    Spot {
        /// The direction the light travels, along the cone's axis. Need not be
        /// unit length, but must not be zero.
        direction: Vec3,
        /// Half angle, in degrees, inside which the light is at full strength.
        inner_angle_deg: f32,
        /// Half angle, in degrees, past which the light is gone. At least
        /// `inner_angle_deg` and at most [`MAX_SPOT_ANGLE_DEG`].
        outer_angle_deg: f32,
    },
}

/// A point or spot light. See the [module docs](self) for the falloff, the cone
/// and the per-frame budget.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Light {
    /// Point or spot, and for a spot its cone.
    pub kind: LightKind,
    /// World-space position.
    pub position: Vec3,
    /// Linear RGB colour.
    pub color: [f32; 3],
    /// Brightness, roughly at one world unit away (see [`falloff`](Self::falloff)).
    /// Zero is allowed and lights nothing.
    pub intensity: f32,
    /// Distance at which the light has faded to exactly zero. Must be positive.
    /// Keep it as short as the light needs: the renderer shades a light only
    /// where it reaches, so a shorter range is cheaper.
    pub range: f32,
}

impl Light {
    /// A point light.
    #[must_use]
    pub fn point(position: Vec3, color: [f32; 3], intensity: f32, range: f32) -> Self {
        Self {
            kind: LightKind::Point,
            position,
            color,
            intensity,
            range,
        }
    }

    /// A spot light at `position` shining along `direction`, at full strength
    /// inside `inner_angle_deg` of the axis and gone past `outer_angle_deg`.
    #[must_use]
    pub fn spot(
        position: Vec3,
        direction: Vec3,
        color: [f32; 3],
        intensity: f32,
        range: f32,
        inner_angle_deg: f32,
        outer_angle_deg: f32,
    ) -> Self {
        Self {
            kind: LightKind::Spot {
                direction,
                inner_angle_deg,
                outer_angle_deg,
            },
            position,
            color,
            intensity,
            range,
        }
    }

    /// Whether a backend can draw this light: every number finite, a positive
    /// range, a non-negative intensity and colour, and for a spot a non-zero
    /// direction with `0 <= inner <= outer <= 90` degrees. A backend ignores a
    /// light that is not valid.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        let finite = self.position.is_finite()
            && self.color.iter().all(|c| c.is_finite() && *c >= 0.0)
            && self.intensity.is_finite()
            && self.intensity >= 0.0
            && self.range.is_finite()
            && self.range > 0.0;
        finite
            && match self.kind {
                LightKind::Point => true,
                LightKind::Spot {
                    direction,
                    inner_angle_deg,
                    outer_angle_deg,
                } => {
                    direction.is_finite()
                        && direction.length_squared() > 0.0
                        && inner_angle_deg.is_finite()
                        && outer_angle_deg.is_finite()
                        && (0.0..=outer_angle_deg).contains(&inner_angle_deg)
                        && outer_angle_deg <= MAX_SPOT_ANGLE_DEG
                }
            }
    }

    /// How much of the light reaches `distance` away, before the cone: the
    /// inverse square, faded to zero at [`range`](Self::range). Zero at and past
    /// the range.
    #[must_use]
    pub fn falloff(&self, distance: f32) -> f32 {
        let ratio = distance / self.range;
        if ratio >= 1.0 {
            return 0.0;
        }
        let window = 1.0 - ratio.powi(4);
        let near = distance.max(LIGHT_MIN_DISTANCE);
        window * window / (near * near)
    }

    /// How much of a spot light's cone reaches `point`: `1` inside the inner
    /// angle, `0` past the outer one, a smooth fade between. Always `1` for a
    /// point light, and for a point at the light's own position.
    #[must_use]
    pub fn cone(&self, point: Vec3) -> f32 {
        let LightKind::Spot {
            direction,
            inner_angle_deg,
            outer_angle_deg,
        } = self.kind
        else {
            return 1.0;
        };
        let to_point = point - self.position;
        if to_point.length_squared() == 0.0 {
            return 1.0;
        }
        let (cos_inner, cos_outer) = cone_cosines(inner_angle_deg, outer_angle_deg);
        let cos = to_point.normalize().dot(direction.normalize());
        smoothstep(cos_outer, cos_inner, cos)
    }

    /// The light's colour times its intensity, times [`falloff`](Self::falloff)
    /// and [`cone`](Self::cone) at `point`: the light arriving there before the
    /// surface's angle and colour are taken into account. What a game can test
    /// "is this point lit?" against.
    #[must_use]
    pub fn radiance_at(&self, point: Vec3) -> [f32; 3] {
        let scale = self.intensity * self.falloff(point.distance(self.position)) * self.cone(point);
        self.color.map(|c| c * scale)
    }
}

/// The cosines of a spot cone's inner and outer half angles, as a backend
/// shades with them: the inner cosine is kept strictly above the outer one, so
/// a hard-edged cone (`inner == outer`) still has a well-defined (very short)
/// fade instead of a division by zero.
#[must_use]
pub fn cone_cosines(inner_angle_deg: f32, outer_angle_deg: f32) -> (f32, f32) {
    let cos_outer = outer_angle_deg.to_radians().cos();
    let cos_inner = inner_angle_deg.to_radians().cos().max(cos_outer + 1e-4);
    (cos_inner, cos_outer)
}

/// Hermite smoothstep, as Metal's `smoothstep`: `0` at or below `edge0`, `1` at
/// or above `edge1`. `edge0 < edge1`.
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn street_light() -> Light {
        Light::point(Vec3::new(0.0, 8.0, 0.0), [1.0, 0.9, 0.7], 50.0, 20.0)
    }

    fn headlight() -> Light {
        Light::spot(
            Vec3::ZERO,
            Vec3::NEG_Z,
            [1.0, 1.0, 1.0],
            100.0,
            30.0,
            15.0,
            25.0,
        )
    }

    #[test]
    fn falloff_is_inverse_square_near_the_light_and_zero_at_the_range() {
        let light = street_light();
        // Close in, the window is ~1, so the falloff is ~1/d².
        assert!((light.falloff(1.0) - 1.0).abs() < 1e-4);
        assert!((light.falloff(2.0) - 0.25).abs() < 1e-3);
        // At the range and past it: nothing.
        assert_eq!(light.falloff(20.0), 0.0);
        assert_eq!(light.falloff(1000.0), 0.0);
        // Never infinite at the light itself.
        let at_source = light.falloff(0.0);
        let cap = 1.0 / (LIGHT_MIN_DISTANCE * LIGHT_MIN_DISTANCE);
        assert!(at_source.is_finite() && at_source <= cap);
        assert!(at_source >= light.falloff(LIGHT_MIN_DISTANCE));
    }

    #[test]
    fn falloff_only_decreases_with_distance() {
        let light = street_light();
        let mut previous = f32::INFINITY;
        for step in 0..=400 {
            let d = step as f32 * 0.05;
            let f = light.falloff(d);
            assert!(f <= previous, "falloff rose at {d}: {f} > {previous}");
            previous = f;
        }
    }

    #[test]
    fn a_spot_is_full_inside_its_inner_cone_and_gone_outside_its_outer_cone() {
        let light = headlight();
        let along = |deg: f32| {
            let r = deg.to_radians();
            Vec3::new(r.sin(), 0.0, -r.cos()) * 10.0
        };
        assert_eq!(light.cone(along(0.0)), 1.0);
        assert_eq!(light.cone(along(14.0)), 1.0);
        let between = light.cone(along(20.0));
        assert!(between > 0.0 && between < 1.0, "{between}");
        assert_eq!(light.cone(along(26.0)), 0.0);
        // Behind the light.
        assert_eq!(light.cone(Vec3::new(0.0, 0.0, 10.0)), 0.0);
        // A point light has no cone.
        assert_eq!(street_light().cone(Vec3::new(5.0, -3.0, 2.0)), 1.0);
    }

    #[test]
    fn a_hard_edged_cone_still_fades_without_dividing_by_zero() {
        let (inner, outer) = cone_cosines(20.0, 20.0);
        assert!(inner > outer);
        let mut light = headlight();
        light.kind = LightKind::Spot {
            direction: Vec3::NEG_Z,
            inner_angle_deg: 20.0,
            outer_angle_deg: 20.0,
        };
        assert_eq!(light.cone(Vec3::NEG_Z), 1.0);
        assert!(light.cone(Vec3::new(1.0, 0.0, -1.0)).is_finite());
    }

    #[test]
    fn radiance_combines_colour_intensity_falloff_and_cone() {
        let light = street_light();
        let below = light.radiance_at(Vec3::ZERO);
        let expected = 50.0 * light.falloff(8.0);
        assert!((below[0] - expected).abs() < 1e-5);
        assert!((below[2] - 0.7 * expected).abs() < 1e-5);
        assert_eq!(light.radiance_at(Vec3::new(0.0, 8.0, 30.0)), [0.0; 3]);
        assert_eq!(headlight().radiance_at(Vec3::new(0.0, 0.0, 5.0)), [0.0; 3]);
    }

    #[test]
    fn validation_rejects_what_a_shader_cannot_draw() {
        assert!(street_light().is_valid());
        assert!(headlight().is_valid());
        let mut zero_intensity = street_light();
        zero_intensity.intensity = 0.0;
        assert!(zero_intensity.is_valid());

        let broken = |change: fn(&mut Light)| {
            let mut light = headlight();
            change(&mut light);
            assert!(!light.is_valid(), "{light:?} should be invalid");
        };
        broken(|l| l.range = 0.0);
        broken(|l| l.range = -1.0);
        broken(|l| l.range = f32::INFINITY);
        broken(|l| l.intensity = -1.0);
        broken(|l| l.intensity = f32::NAN);
        broken(|l| l.color = [1.0, -0.5, 1.0]);
        broken(|l| l.position = Vec3::new(f32::NAN, 0.0, 0.0));
        broken(|l| {
            l.kind = LightKind::Spot {
                direction: Vec3::ZERO,
                inner_angle_deg: 10.0,
                outer_angle_deg: 20.0,
            }
        });
        broken(|l| {
            l.kind = LightKind::Spot {
                direction: Vec3::NEG_Z,
                inner_angle_deg: 30.0,
                outer_angle_deg: 20.0,
            }
        });
        broken(|l| {
            l.kind = LightKind::Spot {
                direction: Vec3::NEG_Z,
                inner_angle_deg: 10.0,
                outer_angle_deg: 95.0,
            }
        });
        broken(|l| {
            l.kind = LightKind::Spot {
                direction: Vec3::NEG_Z,
                inner_angle_deg: -1.0,
                outer_angle_deg: 20.0,
            }
        });
    }
}
