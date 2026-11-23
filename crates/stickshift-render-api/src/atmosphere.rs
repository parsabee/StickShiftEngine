// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! The **atmosphere**: what a frame's air and sky hold besides the sun and the
//! sky gradient ([`SunSky`](crate::SunSky)). How far one sees through the air,
//! a rainbow, a bolt of lightning.
//!
//! A game pushes one through
//! [`FrameRecorder::set_atmosphere`](crate::FrameRecorder::set_atmosphere).
//! Like the sun, it is **physical parameters only**. The engine knows no
//! weather: "rain clearing at four in the afternoon" is a game's policy,
//! expressed as a shorter visibility, then a rainbow's place and strength. A
//! storm's flash over the whole scene is the game's too: it brightens the sky
//! and the sky fill through the [`SunSky`](crate::SunSky). The bolt here is
//! only the streak of light in the sky.
//!
//! Angles follow the [sun's convention](crate::sun#elevation-and-azimuth-convention):
//! elevation up from the horizon, azimuth from `-Z` (north) turning toward
//! `+X` (east), both in degrees.
//!
//! The [default](Atmosphere::default) is clear air with nothing in the sky,
//! and the frame a backend draws under it is exactly the frame it drew before
//! atmospheres existed.

use stickshift_math::glam::Vec3;

/// How far from its centre a rainbow's primary bow stands, in degrees: its red
/// outer edge. Rain drops send the sun's light back at about 42°.
pub const RAINBOW_RADIUS_DEG: f32 = 42.0;

/// How far from its centre a rainbow's faint secondary bow stands, in degrees:
/// its red inner edge. The secondary bow's colours run the other way.
pub const RAINBOW_SECONDARY_RADIUS_DEG: f32 = 51.0;

/// What a frame's air and sky hold besides the sun and the sky gradient. Plain
/// data; see the [module docs](self).
///
/// Values out of range are brought into range by the backend (see
/// [`is_valid`](Self::is_valid)).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Atmosphere {
    /// How far one sees through the air, as a share of the draw distance:
    /// `1` (the default) the distance the
    /// [render settings](crate::RenderSettings::draw_distance_scale) give, less
    /// to draw the distance fog in (mist, rain, a storm). The fog's start and
    /// its opaque depth both scale by it. Never more than `1`: past the draw
    /// distance the streamed world ends.
    pub visibility: f32,
    /// A rainbow in the sky, if one shows.
    pub rainbow: Option<Rainbow>,
    /// A bolt of lightning in the sky, if one is striking.
    pub bolt: Option<Bolt>,
}

impl Default for Atmosphere {
    /// Clear air, as far as the draw distance, with nothing in the sky.
    fn default() -> Self {
        Self {
            visibility: 1.0,
            rainbow: None,
            bolt: None,
        }
    }
}

/// A rainbow: two bows of colour around a point in the sky, the primary
/// [`RAINBOW_RADIUS_DEG`] out from it (red outside, violet inside) and a faint
/// secondary [`RAINBOW_SECONDARY_RADIUS_DEG`] out (its colours reversed), the
/// sky inside the primary a little brighter. Only what is above the horizon
/// shows. In nature the centre is the point straight away from the sun, as far
/// below the horizon as the sun is above it, so a rainbow stands opposite a
/// low sun; a game may set it anywhere.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rainbow {
    /// The centre's height above the horizon, in degrees: below it (negative)
    /// for a bow that stands on the horizon.
    pub elevation_deg: f32,
    /// The centre's bearing, in degrees from north toward east.
    pub azimuth_deg: f32,
    /// How strongly it shows, `0` (not at all) to `1` (as vivid as a rainbow
    /// gets). Fading it in and out is the caller's.
    pub strength: f32,
}

impl Rainbow {
    /// Unit vector from the scene toward the rainbow's centre, in the
    /// [sun's convention](crate::sun#elevation-and-azimuth-convention).
    #[must_use]
    pub fn toward_centre(&self) -> Vec3 {
        toward(self.elevation_deg, self.azimuth_deg)
    }
}

/// A bolt of lightning: a jagged streak of light from the horizon up into the
/// sky, with a branch, drawn behind everything as the sky is, so whatever
/// stands in front of it hides it. Its shape is drawn from its
/// [`seed`](Self::seed); its [`light`](Self::light), flickered by the caller,
/// is all that changes as it strikes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bolt {
    /// Its bearing, in degrees from north toward east: where along the
    /// horizon it comes down.
    pub azimuth_deg: f32,
    /// How high above the horizon its top is, in degrees, `1` to `60`.
    pub height_deg: f32,
    /// Which bolt: its jagged path and its branch are drawn from this, so the
    /// same seed draws the same bolt.
    pub seed: u32,
    /// Its linear RGB light. Like an emissive colour it may exceed `1`, and
    /// should: a bolt several times white clips to white through the tonemap
    /// and glows. `[0, 0, 0]` draws nothing.
    pub light: [f32; 3],
}

impl Bolt {
    /// Unit vector along the horizon toward the bolt's bearing.
    #[must_use]
    pub fn toward(&self) -> Vec3 {
        toward(0.0, self.azimuth_deg)
    }
}

/// Unit vector toward `elevation_deg` above the horizon at `azimuth_deg` from
/// north toward east: `(sin az · cos el, sin el, -cos az · cos el)`.
fn toward(elevation_deg: f32, azimuth_deg: f32) -> Vec3 {
    let (el, az) = (elevation_deg.to_radians(), azimuth_deg.to_radians());
    Vec3::new(az.sin() * el.cos(), el.sin(), -az.cos() * el.cos())
}

impl Atmosphere {
    /// Whether every value is finite and in range: a visibility in
    /// `0.05..=1`, a rainbow's strength in `0..=1`, a bolt's height in
    /// `1..=60` and its light no less than zero.
    ///
    /// A backend brings an invalid atmosphere into range
    /// ([`sanitized`](Self::sanitized)) rather than refusing it; the
    /// [`NullRenderer`](crate::NullRenderer) also records the call as a
    /// protocol violation, so tests catch it.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self == &self.sanitized()
    }

    /// The same atmosphere brought into range: every value clamped as
    /// [`is_valid`](Self::is_valid) describes, and a non-finite value replaced
    /// by the [default](Self::default)'s (no rainbow or bolt, for one of
    /// theirs).
    #[must_use]
    pub fn sanitized(&self) -> Self {
        let num =
            |x: f32, or: f32, lo: f32, hi: f32| if x.is_finite() { x.clamp(lo, hi) } else { or };
        Self {
            visibility: num(self.visibility, 1.0, 0.05, 1.0),
            rainbow: self.rainbow.and_then(|r| {
                (r.elevation_deg.is_finite() && r.azimuth_deg.is_finite()).then(|| Rainbow {
                    strength: num(r.strength, 0.0, 0.0, 1.0),
                    ..r
                })
            }),
            bolt: self.bolt.and_then(|b| {
                b.azimuth_deg.is_finite().then(|| Bolt {
                    height_deg: num(b.height_deg, 1.0, 1.0, 60.0),
                    light: std::array::from_fn(|i| num(b.light[i], 0.0, 0.0, f32::MAX)),
                    ..b
                })
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_clear_air_with_nothing_in_the_sky() {
        let a = Atmosphere::default();
        assert_eq!(a.visibility, 1.0);
        assert_eq!(a.rainbow, None);
        assert_eq!(a.bolt, None);
        assert!(a.is_valid());
    }

    #[test]
    fn directions_follow_the_suns_convention() {
        let close = |a: Vec3, b: Vec3| (a - b).length() < 1e-5;
        let rainbow = |elevation_deg, azimuth_deg| Rainbow {
            elevation_deg,
            azimuth_deg,
            strength: 1.0,
        };
        assert!(
            close(rainbow(0.0, 0.0).toward_centre(), Vec3::NEG_Z),
            "north"
        );
        assert!(close(rainbow(0.0, 90.0).toward_centre(), Vec3::X), "east");
        assert!(
            close(rainbow(-90.0, 0.0).toward_centre(), Vec3::NEG_Y),
            "straight down"
        );
        let bolt = Bolt {
            azimuth_deg: 180.0,
            height_deg: 20.0,
            seed: 0,
            light: [1.0; 3],
        };
        assert!(close(bolt.toward(), Vec3::Z), "south, along the horizon");
    }

    #[test]
    fn out_of_range_values_are_brought_into_range() {
        let wild = Atmosphere {
            visibility: 3.0,
            rainbow: Some(Rainbow {
                elevation_deg: -20.0,
                azimuth_deg: 10.0,
                strength: 2.0,
            }),
            bolt: Some(Bolt {
                azimuth_deg: 10.0,
                height_deg: 200.0,
                seed: 7,
                light: [-1.0, f32::NAN, 4.0],
            }),
        };
        assert!(!wild.is_valid());
        let tame = wild.sanitized();
        assert!(tame.is_valid());
        assert_eq!(tame.visibility, 1.0, "never past the draw distance");
        assert_eq!(tame.rainbow.map(|r| r.strength), Some(1.0));
        let bolt = tame.bolt.expect("kept");
        assert_eq!((bolt.height_deg, bolt.light), (60.0, [0.0, 0.0, 4.0]));
        assert_eq!(
            Atmosphere {
                visibility: 0.0,
                ..wild
            }
            .sanitized()
            .visibility,
            0.05
        );
        let lost = Atmosphere {
            rainbow: Some(Rainbow {
                elevation_deg: f32::NAN,
                azimuth_deg: 0.0,
                strength: 1.0,
            }),
            ..Atmosphere::default()
        };
        assert_eq!(lost.sanitized().rainbow, None, "nowhere to put it");
    }
}
