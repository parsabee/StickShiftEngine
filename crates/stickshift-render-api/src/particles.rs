// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! **Particles**: emitters a game creates, places and fires, whose particles the
//! backend simulates and draws.
//!
//! A game never touches a particle. It creates an emitter with
//! [`EmitterParams`] (how its particles leave it, move, and look over their
//! lives), places it each frame where the effect is ([`ParticleSystem::place_emitter`]:
//! a position, a rotation that turns its [`direction`](EmitterParams::direction),
//! and the velocity it moves at), and either fires bursts
//! ([`emit`](ParticleSystem::emit)) or lets it emit continuously at its
//! [`rate`](EmitterParams::rate). The backend spawns, moves and draws the
//! particles; on the Metal backend that all happens on the GPU, so the CPU cost is
//! per emitter and per burst, not per particle.
//!
//! # Time
//!
//! Particles move on simulated time, not on frames:
//! [`advance_particles`](ParticleSystem::advance_particles) moves every particle
//! on by the time that passed. The engine loop calls it once per frame with the
//! fixed steps that frame ran, so a game never does, and particles keep moving
//! while a game holds its own state still (a game-over screen, say). A
//! continuously emitting emitter spreads the particles it owes over that time and
//! along the path it was placed along since the last advance, so a fast-moving
//! emitter leaves a trail, not clumps.
//!
//! # Motion
//!
//! Each particle leaves the emitter within [`radius`](EmitterParams::radius) of
//! it, along its direction within [`spread_deg`](EmitterParams::spread_deg), at a
//! speed in [`speed`](EmitterParams::speed), plus
//! [`inherit`](EmitterParams::inherit) of the emitter's velocity. From then on it
//! falls under [`gravity`](EmitterParams::gravity), slows with
//! [`drag`](EmitterParams::drag), and bounces off a [`Floor`] if the emitter has
//! one, until its lifetime (in [`lifetime`](EmitterParams::lifetime)) runs out.
//! The random draws are hashed from a sequence the backend keeps, so the same
//! calls give the same particles.
//!
//! # Looks
//!
//! A particle is drawn as a soft-edged quad facing the camera, its
//! [`size`](EmitterParams::size) and [`color`](EmitterParams::color) blending from
//! their start to their end values over its life. With a
//! [`stretch`](EmitterParams::stretch) it is drawn as a streak along its motion
//! instead, as long as the distance it covers in that time: sparks, rain. Particles
//! are depth-tested against the scene (hidden behind what is in front of them)
//! but never hide anything, and the scene's fog fades them with distance like
//! everything else. [`ParticleBlend::Additive`] particles add light (sparks, fire,
//! glints) and need no ordering; [`ParticleBlend::Alpha`] particles cover what is
//! behind them (smoke, dust) and are drawn in the order they were spawned, not
//! sorted by depth.
//!
//! # Budget
//!
//! A backend holds up to [`MAX_PARTICLES`] live particles; past that, new ones
//! replace the oldest. It holds up to [`MAX_EMITTERS`] emitters, counting
//! destroyed emitters whose particles are still alive.

use stickshift_math::glam::Vec3;
use stickshift_math::Transform;

use crate::handles::EmitterHandle;

/// The most particles a backend keeps alive at once. Past it, new particles
/// replace the oldest.
pub const MAX_PARTICLES: usize = 65_536;

/// The most emitters a backend holds at once, counting destroyed emitters whose
/// particles are still alive.
pub const MAX_EMITTERS: usize = 256;

/// How a particle's colour combines with what is behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ParticleBlend {
    /// It adds its light: sparks, fire, glints. Order does not matter.
    #[default]
    Additive,
    /// It covers what is behind it by its alpha: smoke, dust. Particles draw in
    /// the order they were spawned, not sorted by depth.
    Alpha,
}

/// A horizontal surface particles bounce off.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Floor {
    /// Its world `Y`.
    pub height: f32,
    /// How much of a particle's speed into the floor bounces back, `0`
    /// (it stops) to `1` (all of it).
    pub bounce: f32,
    /// How much of a particle's speed along the floor each bounce takes away,
    /// `0` to `1`.
    pub friction: f32,
}

/// What an emitter's particles are like: how they leave it, how they move, and
/// how they look. Plain data, set with
/// [`ParticleSystem::create_emitter`] and [`ParticleSystem::set_emitter`]; see
/// the [module docs](self).
///
/// Values out of range are brought into range by the backend (see
/// [`is_valid`](Self::is_valid)).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EmitterParams {
    /// Particles emitted each second, continuously, for as long as the rate is
    /// set. `0` emits only on [`emit`](ParticleSystem::emit).
    pub rate: f32,
    /// The shortest and longest a particle lives, in seconds. Each particle's is
    /// drawn evenly between them.
    pub lifetime: [f32; 2],
    /// The slowest and fastest a particle leaves at, in world units per second.
    pub speed: [f32; 2],
    /// The way particles leave, in the emitter's own frame (turned by its
    /// placement's rotation). Need not be unit length; zero is taken as `+Y`.
    pub direction: Vec3,
    /// How far from [`direction`](Self::direction) a particle may leave, in
    /// degrees: `0` all along it, `180` every way.
    pub spread_deg: f32,
    /// How far from the emitter a particle may start, in world units: a ball
    /// around its position.
    pub radius: f32,
    /// The share of the emitter's velocity a particle leaves with, on top of its
    /// own: `1` for sparks thrown off a moving car, `0` for particles left
    /// hanging where they were made.
    pub inherit: f32,
    /// Acceleration on every particle, in world units per second².
    pub gravity: Vec3,
    /// How fast a particle slows, per second: its velocity falls by `e^-drag`
    /// each second (before gravity's pull).
    pub drag: f32,
    /// A surface the particles bounce off, if any.
    pub floor: Option<Floor>,
    /// A particle's size across, in world units, at the start and the end of its
    /// life.
    pub size: [f32; 2],
    /// A particle's linear RGBA colour at the start and the end of its life. The
    /// colour may exceed `1` to glow brighter than white before the frame's
    /// tonemap; alpha scales it ([`Additive`](ParticleBlend::Additive)) or covers
    /// with it ([`Alpha`](ParticleBlend::Alpha)).
    pub color: [[f32; 4]; 2],
    /// Seconds of motion a particle's streak covers: drawn as a streak along its
    /// motion, the distance it moves in this long behind it. `0` draws a quad
    /// facing the camera.
    pub stretch: f32,
    /// How a particle combines with what is behind it.
    pub blend: ParticleBlend,
}

impl Default for EmitterParams {
    /// A bursts-only emitter of small white additive points that drift up and
    /// fade out over a second.
    fn default() -> Self {
        Self {
            rate: 0.0,
            lifetime: [1.0, 1.0],
            speed: [1.0, 1.0],
            direction: Vec3::Y,
            spread_deg: 15.0,
            radius: 0.0,
            inherit: 1.0,
            gravity: Vec3::ZERO,
            drag: 0.0,
            floor: None,
            size: [0.1, 0.1],
            color: [[1.0, 1.0, 1.0, 1.0], [1.0, 1.0, 1.0, 0.0]],
            stretch: 0.0,
            blend: ParticleBlend::Additive,
        }
    }
}

impl EmitterParams {
    /// Whether every value is finite and in range: a rate, drag, radius,
    /// inherit, stretch, sizes and colours no less than zero; lifetimes over zero
    /// and speeds no less than zero, each pair in order; a spread in `0..=180`;
    /// a floor's bounce and friction in `0..=1`.
    ///
    /// A backend brings an invalid emitter into range ([`sanitized`](Self::sanitized))
    /// rather than refusing it; the
    /// [`NullRenderer`](crate::NullRenderer) also records the call as a
    /// protocol violation, so tests catch it.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self == &self.sanitized()
    }

    /// The same emitter brought into range: every value clamped as
    /// [`is_valid`](Self::is_valid) describes, a non-finite value replaced by
    /// the [default](Self::default)'s, and each pair put in order.
    #[must_use]
    pub fn sanitized(&self) -> Self {
        let fallback = Self::default();
        let num =
            |x: f32, or: f32, lo: f32, hi: f32| if x.is_finite() { x.clamp(lo, hi) } else { or };
        let pair = |p: [f32; 2], or: [f32; 2], lo: f32| {
            let a = num(p[0], or[0], lo, f32::MAX);
            let b = num(p[1], or[1], lo, f32::MAX);
            [a.min(b), a.max(b)]
        };
        let vec = |v: Vec3, or: Vec3| if v.is_finite() { v } else { or };
        let color =
            |c: [f32; 4], or: [f32; 4]| std::array::from_fn(|i| num(c[i], or[i], 0.0, f32::MAX));
        Self {
            rate: num(self.rate, fallback.rate, 0.0, f32::MAX),
            lifetime: pair(self.lifetime, fallback.lifetime, f32::MIN_POSITIVE),
            speed: pair(self.speed, fallback.speed, 0.0),
            direction: vec(self.direction, fallback.direction),
            spread_deg: num(self.spread_deg, fallback.spread_deg, 0.0, 180.0),
            radius: num(self.radius, fallback.radius, 0.0, f32::MAX),
            inherit: num(self.inherit, fallback.inherit, 0.0, f32::MAX),
            gravity: vec(self.gravity, fallback.gravity),
            drag: num(self.drag, fallback.drag, 0.0, f32::MAX),
            floor: self.floor.map(|f| Floor {
                height: num(f.height, 0.0, f32::MIN, f32::MAX),
                bounce: num(f.bounce, 0.0, 0.0, 1.0),
                friction: num(f.friction, 0.0, 0.0, 1.0),
            }),
            size: [
                num(self.size[0], fallback.size[0], 0.0, f32::MAX),
                num(self.size[1], fallback.size[1], 0.0, f32::MAX),
            ],
            color: [
                color(self.color[0], fallback.color[0]),
                color(self.color[1], fallback.color[1]),
            ],
            stretch: num(self.stretch, fallback.stretch, 0.0, f32::MAX),
            blend: self.blend,
        }
    }
}

/// Particle effects: create emitters, place them, fire them. See the
/// [module docs](self) for how particles move and look.
///
/// Unlike [`FrameRecorder`](crate::FrameRecorder) calls, every call here may be
/// made at any time, with or without an open frame: a game typically fires a
/// burst from its fixed update, where a crash happens, and the particles appear
/// in the next frame drawn. The particles themselves are drawn with every
/// frame's scene, after its geometry and before its 2D overlay, without the
/// game recording anything.
///
/// # Handles
///
/// A destroyed emitter's handle, or one never created, is ignored by every call
/// (the [`NullRenderer`](crate::NullRenderer) records it as a protocol
/// violation). Past [`MAX_EMITTERS`], `create_emitter` still returns a handle,
/// which emits nothing.
pub trait ParticleSystem {
    /// Create an emitter with `params`. It is at the world origin, unturned and
    /// still until [`place_emitter`](Self::place_emitter) puts it somewhere, and
    /// starts emitting at once if `params` has a [`rate`](EmitterParams::rate).
    fn create_emitter(&mut self, params: &EmitterParams) -> EmitterHandle;

    /// Replace `emitter`'s params. Its live particles take the new looks (size,
    /// colour, stretch, blend) and motion (gravity, drag, floor) from here on;
    /// how they left it stays as it was. Setting only a new
    /// [`rate`](EmitterParams::rate) is how a game turns a continuous effect on
    /// and off.
    fn set_emitter(&mut self, emitter: EmitterHandle, params: &EmitterParams);

    /// Destroy `emitter`. It stops emitting at once; the particles it already
    /// made live out their lives.
    fn destroy_emitter(&mut self, emitter: EmitterHandle);

    /// Put `emitter` at `transform`'s position, turned by its rotation (its
    /// scale is ignored), moving at `velocity` in world units per second. Bursts
    /// fired from here on leave from here; continuous emission is spread along
    /// the way from where it was at the last
    /// [`advance_particles`](Self::advance_particles) to here.
    fn place_emitter(&mut self, emitter: EmitterHandle, transform: &Transform, velocity: Vec3);

    /// Fire a burst of `count` particles from `emitter`, from where it is placed
    /// now. They appear with the next advance.
    fn emit(&mut self, emitter: EmitterHandle, count: u32);

    /// Move every particle on by `dt` seconds, spawning what continuous emitters
    /// owe over that time. The engine loop calls this once per frame with the
    /// simulated time the frame's fixed steps covered; a game driving its own
    /// loop calls it the same way.
    fn advance_particles(&mut self, dt: f32);

    /// Move every live particle, and every emitter's last placement, by
    /// `offset`: what a floating-origin rebase does to the rest of the world
    /// (see `stickshift-scene`), so particles stay where they were relative to
    /// it. Emitters' floors and gravity are not moved.
    fn shift_particles(&mut self, offset: Vec3);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_emitter_is_valid() {
        assert!(EmitterParams::default().is_valid());
    }

    #[test]
    fn out_of_range_values_are_brought_into_range() {
        let wild = EmitterParams {
            rate: -3.0,
            lifetime: [2.0, f32::NAN],
            speed: [5.0, 1.0],
            spread_deg: 270.0,
            radius: -1.0,
            drag: f32::INFINITY,
            floor: Some(Floor {
                height: -0.3,
                bounce: 1.5,
                friction: -0.2,
            }),
            size: [-0.1, 0.2],
            color: [[2.0, -1.0, 0.5, 1.0], [0.0; 4]],
            direction: Vec3::new(f32::NAN, 0.0, 0.0),
            ..EmitterParams::default()
        };
        assert!(!wild.is_valid());
        let fixed = wild.sanitized();
        assert!(fixed.is_valid(), "{fixed:?}");
        assert_eq!(fixed.rate, 0.0);
        assert_eq!(
            fixed.lifetime,
            [1.0, 2.0],
            "NaN falls back, then the pair is ordered"
        );
        assert_eq!(fixed.speed, [1.0, 5.0]);
        assert_eq!(fixed.spread_deg, 180.0);
        assert_eq!(fixed.radius, 0.0);
        assert_eq!(
            fixed.drag, 0.0,
            "a non-finite drag falls back to the default's"
        );
        let floor = fixed.floor.expect("kept");
        assert_eq!(
            (floor.height, floor.bounce, floor.friction),
            (-0.3, 1.0, 0.0)
        );
        assert_eq!(fixed.size, [0.0, 0.2]);
        assert_eq!(
            fixed.color[0],
            [2.0, 0.0, 0.5, 1.0],
            "brighter than white stays"
        );
        assert_eq!(fixed.direction, Vec3::Y);
    }

    #[test]
    fn a_lifetime_is_never_zero() {
        let fixed = EmitterParams {
            lifetime: [0.0, 0.0],
            ..EmitterParams::default()
        }
        .sanitized();
        assert!(fixed.lifetime[0] > 0.0 && fixed.lifetime[1] > 0.0);
    }
}
