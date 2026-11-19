// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! **Particles**, the CPU half: what to spawn, and where the live ones are.
//!
//! The particles themselves live only on the GPU, in one ring buffer of
//! [`MAX_PARTICLES`] slots. Every frame's command buffer runs two compute
//! kernels before the scene pass: `particle_update` moves the particles already
//! there on by the time that passed, and `particle_spawn` writes the new ones into
//! the next slots of the ring. The scene pass then draws the live slots as
//! instanced quads (see `particle_vertex_main` in the shader). The CPU never
//! reads a particle back.
//!
//! This module is everything the CPU decides, kept free of Metal so it can be
//! tested without a device:
//!
//! - **The emitter table.** Up to [`MAX_EMITTERS`] slots, each holding an
//!   emitter's params (as the shaders read them, [`EmitterUniforms`]) and its
//!   placement. A handle carries its slot and a generation, so a handle to a
//!   destroyed emitter never reaches the slot's next tenant. A destroyed emitter's
//!   slot is kept until its last particle has died, since the shaders read its
//!   params every frame they draw it.
//! - **Spawning.** A burst ([`emit`](ParticlePlanner::emit)) becomes one
//!   [`SpawnBatch`] at the emitter's placement. A continuous rate owes `rate * dt`
//!   particles each [`advance`](ParticlePlanner::advance), carrying the fraction
//!   to the next one so a low rate still emits on average exactly its rate; they
//!   are spread over the advance's time and along the emitter's path from its last
//!   placement, so a fast emitter leaves a trail. Each batch carries the time into
//!   the frame its particles are born, and the spawn kernel moves each one on by
//!   the rest of the frame.
//! - **The live window.** Particles are written into the ring in order, so the
//!   live ones are always the last few written. The planner keeps, per batch,
//!   the latest any of its particles can still be alive; the window runs from the
//!   oldest batch still within it to the newest slot written. The kernels and the
//!   draw cover only that window, so their cost follows the particles alive, not
//!   the ring's size. More than [`MAX_PARTICLES`] alive overwrites the oldest.
//!
//! Everything here is plain arithmetic on the values the game passed in, so the
//! same calls give the same batches and seeds.

use std::collections::VecDeque;

use stickshift_math::glam::{Quat, Vec3};
use stickshift_render_api::{
    EmitterHandle, EmitterParams, ParticleBlend, MAX_EMITTERS, MAX_PARTICLES,
};

/// The most spawn batches one frame holds: bursts fired plus emitters emitting
/// continuously, between two submits. Bursts past it are dropped (and counted).
pub const MAX_SPAWN_BATCHES: usize = 4096;

/// An emitter's params as the shaders read them: one 128-byte entry of the
/// emitter table. Mirrors `EmitterParams` in the shader.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct EmitterUniforms {
    /// Linear RGBA at the start of a particle's life.
    pub color_start: [f32; 4],
    /// Linear RGBA at the end of it.
    pub color_end: [f32; 4],
    /// `xyz` gravity, `w` drag.
    pub gravity_drag: [f32; 4],
    /// Size at the start and the end of life, stretch seconds, and the blend
    /// (`0` additive, `1` alpha).
    pub look: [f32; 4],
    /// The floor's height, bounce and friction, and `1` if there is one.
    pub floor: [f32; 4],
    /// `xyz` the unit direction particles leave along (emitter frame), `w` the
    /// cosine of the spread.
    pub direction_spread: [f32; 4],
    /// Shortest and longest lifetime, slowest and fastest speed.
    pub lifetime_speed: [f32; 4],
    /// Spawn radius, share of the emitter's velocity inherited, unused, unused.
    pub radius_inherit: [f32; 4],
}

/// Byte size of one [`EmitterUniforms`]; the shader's `EmitterParams` must match.
pub const EMITTER_UNIFORMS_SIZE: usize = std::mem::size_of::<EmitterUniforms>();

impl EmitterUniforms {
    /// `params`, already [sanitized](EmitterParams::sanitized), as the shaders
    /// read them.
    #[must_use]
    pub fn from_params(params: &EmitterParams) -> Self {
        let direction = params.direction.try_normalize().unwrap_or(Vec3::Y);
        let floor = params
            .floor
            .map_or([0.0; 4], |f| [f.height, f.bounce, f.friction, 1.0]);
        Self {
            color_start: params.color[0],
            color_end: params.color[1],
            gravity_drag: params.gravity.extend(params.drag).to_array(),
            look: [
                params.size[0],
                params.size[1],
                params.stretch,
                match params.blend {
                    ParticleBlend::Additive => 0.0,
                    ParticleBlend::Alpha => 1.0,
                },
            ],
            floor,
            direction_spread: direction
                .extend(params.spread_deg.to_radians().cos())
                .to_array(),
            lifetime_speed: [
                params.lifetime[0],
                params.lifetime[1],
                params.speed[0],
                params.speed[1],
            ],
            radius_inherit: [params.radius, params.inherit, 0.0, 0.0],
        }
    }
}

/// One run of particles to spawn: a burst, or what a continuous emitter owed
/// over one advance. Mirrors `SpawnBatch` in the shader, 80 bytes.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SpawnBatch {
    /// `xyz` where the emitter was at the start of the span, `w` the seconds
    /// into the frame the span starts.
    pub from: [f32; 4],
    /// `xyz` where it was at the end, `w` the span's length in seconds (`0` for
    /// a burst: every particle born at once).
    pub to: [f32; 4],
    /// The emitter's rotation, as a quaternion `xyzw`.
    pub rotation: [f32; 4],
    /// `xyz` the emitter's velocity, `w` unused.
    pub velocity: [f32; 4],
    /// The emitter's slot, the batch's first particle among the frame's spawns,
    /// how many particles, and the batch's random seed.
    pub meta: [u32; 4],
}

/// Byte size of one [`SpawnBatch`]; the shader's must match.
pub const SPAWN_BATCH_SIZE: usize = std::mem::size_of::<SpawnBatch>();

/// The per-frame values the two compute kernels read. Mirrors `ParticleStep` in
/// the shader.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ParticleStep {
    /// `xyz` the shift every particle already there takes, `w` the seconds the
    /// frame moves them on.
    pub shift_dt: [f32; 4],
    /// The particles already there to move: first slot, count; then the ring's
    /// size, and how many particles the frame spawns.
    pub window: [u32; 4],
    /// The first slot the spawns go into, and how many batches there are.
    pub spawn: [u32; 4],
}

/// The per-frame values the particle draw reads. Mirrors `ParticleView` in the
/// shader.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ParticleView {
    /// World to clip.
    pub view_projection: [[f32; 4]; 4],
    /// `xyz` the camera's position, `w` which blend this draw covers (`0`
    /// additive, `1` alpha).
    pub camera: [f32; 4],
    /// The live slots to draw: first slot, count; then the ring's size.
    pub window: [u32; 4],
}

/// The live slots of the ring: the first, and how many.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Window {
    /// The first live slot.
    pub first: u32,
    /// How many slots, from `first` and wrapping around the ring.
    pub count: u32,
}

/// What one frame asks of the GPU, taken by [`ParticlePlanner::take_frame`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParticleFrame {
    /// The emitter table, up to the highest slot in use.
    pub emitters: Vec<EmitterUniforms>,
    /// The particles already on the GPU to move on, before this frame's spawns.
    pub update: Window,
    /// The seconds to move them on (`0`: nothing moves).
    pub dt: f32,
    /// The shift they take.
    pub shift: Vec3,
    /// The frame's spawn batches, and the first slot their particles go into.
    pub spawns: Vec<SpawnBatch>,
    /// The ring slot the spawns start at.
    pub spawn_first: u32,
    /// How many particles the batches spawn.
    pub spawn_count: u32,
    /// The live slots to draw after the kernels.
    pub draw: Window,
    /// Whether any emitter in the table draws additively, and with alpha.
    pub blends: (bool, bool),
}

/// A placed emitter.
#[derive(Debug, Clone, Copy)]
struct Emitter {
    params: EmitterParams,
    position: Vec3,
    rotation: Quat,
    velocity: Vec3,
    /// Where it was at the last advance: the start of its next trail.
    last: Option<Vec3>,
    /// The fraction of a particle its rate owes.
    owed: f32,
}

/// One slot of the emitter table.
#[derive(Debug, Clone, Copy)]
enum Slot {
    /// Unused.
    Free,
    /// A live emitter.
    Live(Emitter),
    /// A destroyed emitter whose particles may still be alive until `until`.
    Retiring(EmitterParams),
}

/// Where a batch's particles start in the ring, and the latest any of them can
/// be alive.
#[derive(Debug, Clone, Copy)]
struct Span {
    start: u64,
    until: f64,
}

/// The CPU half of the particle system (see the module docs).
#[derive(Debug)]
pub struct ParticlePlanner {
    slots: Vec<Slot>,
    /// Each slot's generation, bumped as it is freed.
    generations: Vec<u32>,
    /// Each slot's latest particle death: when a retiring slot frees.
    until: Vec<f64>,
    /// Simulated seconds advanced so far.
    time: f64,
    /// Particles ever spawned: the ring slot of the next is this modulo its size.
    head: u64,
    /// Particles the GPU has had spawned into it, up to the last frame taken.
    on_gpu: u64,
    /// The batches whose particles may still be alive, oldest first.
    spans: VecDeque<Span>,
    /// The frame being gathered: its time, its shift, and its batches.
    dt: f32,
    shift: Vec3,
    spawns: Vec<SpawnBatch>,
    spawn_count: u32,
    /// The next batch's seed.
    seed: u32,
    /// Bursts dropped for lack of room.
    dropped: u64,
}

impl Default for ParticlePlanner {
    fn default() -> Self {
        Self::new()
    }
}

/// The handle for slot `index` at `generation`.
fn handle(index: usize, generation: u32) -> EmitterHandle {
    EmitterHandle((generation << 8) | index as u32)
}

/// A handle that names no slot: what a create past [`MAX_EMITTERS`] returns.
const NO_EMITTER: EmitterHandle = EmitterHandle(u32::MAX);

impl ParticlePlanner {
    /// An empty planner: no emitters, no particles.
    #[must_use]
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            generations: Vec::new(),
            until: Vec::new(),
            time: 0.0,
            head: 0,
            on_gpu: 0,
            spans: VecDeque::new(),
            dt: 0.0,
            shift: Vec3::ZERO,
            spawns: Vec::new(),
            spawn_count: 0,
            seed: 0x9e37_79b9,
            dropped: 0,
        }
    }

    /// The live emitter `handle` names, if it names one.
    fn live(&mut self, handle: EmitterHandle) -> Option<(usize, &mut Emitter)> {
        let index = (handle.0 & 0xff) as usize;
        if self.generations.get(index) != Some(&(handle.0 >> 8)) {
            return None;
        }
        match self.slots.get_mut(index) {
            Some(Slot::Live(emitter)) => Some((index, emitter)),
            _ => None,
        }
    }

    /// Create an emitter: see
    /// [`ParticleSystem::create_emitter`](stickshift_render_api::ParticleSystem::create_emitter).
    pub fn create(&mut self, params: &EmitterParams) -> EmitterHandle {
        self.retire();
        let emitter = Emitter {
            params: params.sanitized(),
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            velocity: Vec3::ZERO,
            last: None,
            owed: 0.0,
        };
        let index = match self.slots.iter().position(|s| matches!(s, Slot::Free)) {
            Some(index) => index,
            None if self.slots.len() < MAX_EMITTERS => {
                self.slots.push(Slot::Free);
                self.generations.push(0);
                self.until.push(0.0);
                self.slots.len() - 1
            }
            None => return NO_EMITTER,
        };
        self.slots[index] = Slot::Live(emitter);
        self.until[index] = self.time;
        handle(index, self.generations[index])
    }

    /// Replace an emitter's params.
    pub fn set(&mut self, emitter: EmitterHandle, params: &EmitterParams) {
        if let Some((_, live)) = self.live(emitter) {
            let params = params.sanitized();
            if params.rate == 0.0 {
                live.owed = 0.0;
            }
            live.params = params;
        }
    }

    /// Destroy an emitter: it stops emitting, and its slot frees once its
    /// particles have died.
    pub fn destroy(&mut self, emitter: EmitterHandle) {
        if let Some((index, live)) = self.live(emitter) {
            let params = live.params;
            self.slots[index] = Slot::Retiring(params);
            self.retire();
        }
    }

    /// Place an emitter.
    pub fn place(
        &mut self,
        emitter: EmitterHandle,
        position: Vec3,
        rotation: Quat,
        velocity: Vec3,
    ) {
        if let Some((_, live)) = self.live(emitter) {
            live.position = position;
            live.rotation = rotation.normalize();
            live.velocity = velocity;
        }
    }

    /// Fire a burst of `count` from where the emitter is placed.
    pub fn emit(&mut self, emitter: EmitterHandle, count: u32) {
        let born = self.dt;
        if let Some((index, live)) = self.live(emitter) {
            let live = *live;
            self.batch(index, &live, live.position, live.position, born, 0.0, count);
        }
    }

    /// Move everything on by `dt` seconds: what continuous emitters owe is
    /// spawned, spread over the time and along their paths.
    pub fn advance(&mut self, dt: f32) {
        if !(dt.is_finite() && dt > 0.0) {
            return;
        }
        let born = self.dt;
        for index in 0..self.slots.len() {
            let Slot::Live(mut emitter) = self.slots[index] else {
                continue;
            };
            let from = emitter.last.unwrap_or(emitter.position);
            if emitter.params.rate > 0.0 {
                let owed = emitter.owed + emitter.params.rate * dt;
                let count = owed.floor();
                emitter.owed = owed - count;
                self.batch(
                    index,
                    &emitter,
                    from,
                    emitter.position,
                    born,
                    dt,
                    count as u32,
                );
            }
            emitter.last = Some(emitter.position);
            self.slots[index] = Slot::Live(emitter);
        }
        self.dt += dt;
        self.time += f64::from(dt);
        self.retire();
    }

    /// Shift every particle and every emitter by `offset`.
    pub fn shift(&mut self, offset: Vec3) {
        if !offset.is_finite() {
            return;
        }
        self.shift += offset;
        for batch in &mut self.spawns {
            for end in [&mut batch.from, &mut batch.to] {
                let moved = Vec3::from_slice(&end[..3]) + offset;
                end[..3].copy_from_slice(&moved.to_array());
            }
        }
        for slot in &mut self.slots {
            if let Slot::Live(emitter) = slot {
                emitter.position += offset;
                emitter.last = emitter.last.map(|last| last + offset);
            }
        }
    }

    /// Record a batch of `count` from the emitter in `index`, along `from` to
    /// `to`, born from `born` seconds into the frame over `span` seconds.
    #[allow(clippy::too_many_arguments)]
    fn batch(
        &mut self,
        index: usize,
        emitter: &Emitter,
        from: Vec3,
        to: Vec3,
        born: f32,
        span: f32,
        count: u32,
    ) {
        let room = MAX_PARTICLES as u32 - self.spawn_count;
        let count = count.min(room);
        if count == 0 {
            return;
        }
        if self.spawns.len() >= MAX_SPAWN_BATCHES {
            self.dropped += 1;
            return;
        }
        let seed = self.seed;
        self.seed = self
            .seed
            .wrapping_mul(0x0101_0101)
            .wrapping_add(0x9e37_79b9);
        self.spawns.push(SpawnBatch {
            from: from.extend(born).to_array(),
            to: to.extend(span).to_array(),
            rotation: emitter.rotation.to_array(),
            velocity: emitter.velocity.extend(0.0).to_array(),
            meta: [index as u32, self.spawn_count, count, seed],
        });
        self.spawn_count += count;
        // `born` is into the frame, which ends at `self.time`: a burst is born
        // now, and a continuous batch over the `span` being advanced.
        let until = self.time + f64::from(span) + f64::from(emitter.params.lifetime[1]);
        self.spans.push_back(Span {
            start: self.head,
            until,
        });
        self.head += u64::from(count);
        self.until[index] = self.until[index].max(until);
    }

    /// Drop the batches whose particles have all died, and free the slots of
    /// destroyed emitters whose particles have.
    fn retire(&mut self) {
        // Only from the front: a later batch that died sooner is kept until the
        // ones before it go, which only makes the window a little longer.
        while self
            .spans
            .front()
            .is_some_and(|span| span.until <= self.time)
        {
            self.spans.pop_front();
        }
        for index in 0..self.slots.len() {
            if matches!(self.slots[index], Slot::Retiring(_)) && self.until[index] <= self.time {
                self.slots[index] = Slot::Free;
                self.generations[index] = self.generations[index].wrapping_add(1) & 0x00ff_ffff;
            }
        }
    }

    /// The window from the oldest batch that may be alive to `end`.
    fn window(&self, end: u64) -> Window {
        let oldest = self.spans.front().map_or(end, |span| span.start);
        let first = oldest.max(end.saturating_sub(MAX_PARTICLES as u64));
        let first = first.min(end);
        Window {
            first: (first % MAX_PARTICLES as u64) as u32,
            count: (end - first) as u32,
        }
    }

    /// Take the frame gathered since the last one: the emitter table, the
    /// particles to move on and by how much, the batches to spawn, and the
    /// window to draw. Clears the frame.
    pub fn take_frame(&mut self) -> ParticleFrame {
        let used = self
            .slots
            .iter()
            .rposition(|slot| !matches!(slot, Slot::Free))
            .map_or(0, |i| i + 1);
        let mut blends = (false, false);
        let emitters = self.slots[..used]
            .iter()
            .map(|slot| {
                let params = match slot {
                    Slot::Free => return EmitterUniforms::default(),
                    Slot::Live(emitter) => &emitter.params,
                    Slot::Retiring(params) => params,
                };
                match params.blend {
                    ParticleBlend::Additive => blends.0 = true,
                    ParticleBlend::Alpha => blends.1 = true,
                }
                EmitterUniforms::from_params(params)
            })
            .collect();
        let frame = ParticleFrame {
            emitters,
            update: self.window(self.on_gpu),
            dt: self.dt,
            shift: self.shift,
            spawns: std::mem::take(&mut self.spawns),
            spawn_first: (self.on_gpu % MAX_PARTICLES as u64) as u32,
            spawn_count: self.spawn_count,
            draw: self.window(self.head),
            blends,
        };
        self.on_gpu = self.head;
        self.dt = 0.0;
        self.shift = Vec3::ZERO;
        self.spawn_count = 0;
        self.retire();
        frame
    }

    /// Bursts dropped because a frame already held [`MAX_SPAWN_BATCHES`].
    #[must_use]
    pub fn dropped_bursts(&self) -> u64 {
        self.dropped
    }

    /// Slots of the emitter table in use, by live and retiring emitters.
    #[must_use]
    pub fn emitters_in_use(&self) -> usize {
        self.slots
            .iter()
            .filter(|s| !matches!(s, Slot::Free))
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;

    fn sparks(rate: f32, life: f32) -> EmitterParams {
        EmitterParams {
            rate,
            lifetime: [life * 0.5, life],
            ..EmitterParams::default()
        }
    }

    #[test]
    fn a_burst_spawns_from_where_the_emitter_is_placed_with_its_motion() {
        let mut p = ParticlePlanner::new();
        let e = p.create(&sparks(0.0, 1.0));
        let at = Vec3::new(1.0, 2.0, 3.0);
        p.place(
            e,
            at,
            Quat::from_rotation_y(0.5),
            Vec3::new(0.0, 0.0, -30.0),
        );
        p.emit(e, 40);
        p.advance(DT);
        let frame = p.take_frame();
        assert_eq!(frame.spawns.len(), 1);
        let batch = frame.spawns[0];
        assert_eq!(
            batch.from,
            [1.0, 2.0, 3.0, 0.0],
            "born at the frame's start"
        );
        assert_eq!(batch.to, [1.0, 2.0, 3.0, 0.0], "all at once");
        assert_eq!(batch.velocity[..3], [0.0, 0.0, -30.0]);
        assert_eq!(batch.meta[..3], [0, 0, 40]);
        assert_eq!(frame.spawn_count, 40);
        assert_eq!(frame.dt, DT);
        assert_eq!(
            frame.update,
            Window { first: 0, count: 0 },
            "nothing was there yet"
        );
        assert_eq!(
            frame.draw,
            Window {
                first: 0,
                count: 40
            }
        );
    }

    #[test]
    fn a_rate_emits_exactly_its_rate_on_average_spread_along_the_path() {
        let mut p = ParticlePlanner::new();
        // 90 a second at 60 frames a second: one and a half a frame.
        let e = p.create(&sparks(90.0, 1.0));
        let mut spawned = 0;
        for frame in 0..60 {
            p.place(
                e,
                Vec3::new(frame as f32, 0.0, 0.0),
                Quat::IDENTITY,
                Vec3::ZERO,
            );
            p.advance(DT);
            let taken = p.take_frame();
            for batch in &taken.spawns {
                if frame > 0 {
                    assert_eq!(batch.from[0], frame as f32 - 1.0, "from the last placement");
                    assert_eq!(batch.to[0], frame as f32, "to this one");
                }
                assert_eq!(batch.to[3], DT, "over the whole advance");
            }
            spawned += taken.spawn_count;
        }
        assert_eq!(spawned, 90, "a second's worth, fractions carried");
    }

    #[test]
    fn a_frame_of_several_advances_starts_each_spawn_later() {
        let mut p = ParticlePlanner::new();
        let e = p.create(&sparks(600.0, 1.0));
        p.advance(DT);
        p.emit(e, 3);
        p.advance(DT);
        let frame = p.take_frame();
        let born: Vec<f32> = frame.spawns.iter().map(|b| b.from[3]).collect();
        assert_eq!(born, vec![0.0, DT, DT], "the burst after the first advance");
        assert_eq!(frame.dt, 2.0 * DT);
        let firsts: Vec<u32> = frame.spawns.iter().map(|b| b.meta[1]).collect();
        assert_eq!(firsts, vec![0, 10, 13], "the batches follow one another");
    }

    #[test]
    fn the_window_follows_the_live_particles_and_lets_go_of_dead_ones() {
        let mut p = ParticlePlanner::new();
        let e = p.create(&sparks(0.0, 0.5));
        p.emit(e, 100);
        p.advance(DT);
        let first = p.take_frame();
        assert_eq!(
            first.draw,
            Window {
                first: 0,
                count: 100
            }
        );
        p.emit(e, 50);
        p.advance(DT);
        let second = p.take_frame();
        assert_eq!(
            second.update,
            Window {
                first: 0,
                count: 100
            },
            "the first burst moves on"
        );
        assert_eq!(second.spawn_first, 100);
        assert_eq!(
            second.draw,
            Window {
                first: 0,
                count: 150
            }
        );
        // Past the first burst's longest life, only the second is left.
        for _ in 0..28 {
            p.advance(DT);
        }
        let later = p.take_frame();
        assert_eq!(
            later.draw,
            Window {
                first: 100,
                count: 50
            }
        );
        for _ in 0..30 {
            p.advance(DT);
        }
        let empty = p.take_frame();
        assert_eq!(empty.draw.count, 0, "every particle has died");
    }

    #[test]
    fn past_the_ring_the_newest_particles_replace_the_oldest() {
        let mut p = ParticlePlanner::new();
        let e = p.create(&sparks(0.0, 10.0));
        let ring = MAX_PARTICLES as u32;
        p.emit(e, ring - 10);
        p.advance(DT);
        p.take_frame();
        p.emit(e, 30);
        p.advance(DT);
        let frame = p.take_frame();
        assert_eq!(frame.spawn_first, ring - 10);
        assert_eq!(
            frame.draw,
            Window {
                first: 20,
                count: ring
            },
            "the ring is full"
        );
        // One frame never spawns more than the ring holds.
        p.emit(e, ring);
        p.emit(e, ring);
        p.advance(DT);
        assert_eq!(p.take_frame().spawn_count, ring);
    }

    #[test]
    fn a_destroyed_emitter_stops_and_frees_its_slot_once_its_particles_die() {
        let mut p = ParticlePlanner::new();
        let a = p.create(&sparks(60.0, 0.5));
        p.emit(a, 10);
        p.advance(DT);
        p.destroy(a);
        assert_eq!(p.emitters_in_use(), 1, "kept while its particles live");
        p.emit(a, 10);
        p.advance(DT);
        let frame = p.take_frame();
        assert_eq!(
            frame.spawn_count, 11,
            "the burst and the first advance's one, nothing after it was destroyed"
        );
        let b = p.create(&sparks(0.0, 1.0));
        assert_ne!(b.0 & 0xff, a.0 & 0xff, "a retiring slot is not handed out");
        for _ in 0..40 {
            p.advance(DT);
        }
        p.take_frame();
        assert_eq!(
            p.emitters_in_use(),
            1,
            "its slot is free once they have died"
        );
        let c = p.create(&sparks(0.0, 1.0));
        assert_eq!(c.0 & 0xff, a.0 & 0xff, "and reused");
        assert_ne!(c, a, "under a new generation");
        p.emit(a, 5);
        p.advance(DT);
        assert!(
            p.take_frame().spawns.is_empty(),
            "the old handle reaches nothing"
        );
    }

    #[test]
    fn past_the_emitter_limit_a_create_returns_a_handle_that_does_nothing() {
        let mut p = ParticlePlanner::new();
        for _ in 0..MAX_EMITTERS {
            p.create(&sparks(0.0, 1.0));
        }
        let extra = p.create(&sparks(60.0, 1.0));
        p.emit(extra, 10);
        p.advance(DT);
        assert!(p.take_frame().spawns.is_empty());
    }

    #[test]
    fn a_shift_moves_the_particles_the_pending_spawns_and_the_emitters() {
        let mut p = ParticlePlanner::new();
        let e = p.create(&sparks(60.0, 1.0));
        p.place(e, Vec3::new(0.0, 0.0, -1000.0), Quat::IDENTITY, Vec3::ZERO);
        p.advance(DT);
        p.emit(e, 5);
        let offset = Vec3::new(0.0, 0.0, 1000.0);
        p.shift(offset);
        p.advance(DT);
        let frame = p.take_frame();
        assert_eq!(frame.shift, offset);
        for batch in &frame.spawns {
            assert_eq!(
                batch.from[2], 0.0,
                "spawns gathered before the shift move with it"
            );
            assert_eq!(batch.to[2], 0.0);
        }
    }

    #[test]
    fn the_table_carries_each_emitters_params_and_blends() {
        let mut p = ParticlePlanner::new();
        p.create(&sparks(0.0, 1.0));
        let smoke = p.create(&EmitterParams {
            blend: ParticleBlend::Alpha,
            spread_deg: 90.0,
            direction: Vec3::new(0.0, 0.0, -2.0),
            ..EmitterParams::default()
        });
        let frame = p.take_frame();
        assert_eq!(frame.emitters.len(), 2);
        assert_eq!(frame.blends, (true, true));
        let table = frame.emitters[(smoke.0 & 0xff) as usize];
        assert_eq!(table.look[3], 1.0);
        assert_eq!(
            table.direction_spread[..3],
            [0.0, 0.0, -1.0],
            "a unit direction"
        );
        assert!(table.direction_spread[3].abs() < 1e-6, "cos 90°");
    }

    #[test]
    fn the_same_calls_give_the_same_frames() {
        let run = || {
            let mut p = ParticlePlanner::new();
            let e = p.create(&sparks(45.0, 1.0));
            (0..20)
                .map(|i| {
                    p.place(e, Vec3::splat(i as f32), Quat::IDENTITY, Vec3::X);
                    if i % 7 == 0 {
                        p.emit(e, 12);
                    }
                    p.advance(DT);
                    p.take_frame()
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn gpu_struct_sizes_match_the_shader() {
        assert_eq!(EMITTER_UNIFORMS_SIZE, 128);
        assert_eq!(SPAWN_BATCH_SIZE, 80);
        assert_eq!(std::mem::size_of::<ParticleStep>(), 48);
        assert_eq!(std::mem::size_of::<ParticleView>(), 96);
    }
}
