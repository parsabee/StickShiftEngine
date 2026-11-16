// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! The [`Scene`] — owner of the ECS [`World`] — plus the engine-generic
//! **world-streaming** layer for Stick Shift Engine.
//!
//! On top of the [`World`] it provides three things an endless-world game needs,
//! all **engine-generic** (there are deliberately no game concepts here — no
//! vehicle, track, tally, hazard, or gameplay-lattice types; a test scans this
//! crate's source to keep it that way):
//!
//! 1. **Spawn-ahead / despawn-behind streaming** (`stream`(Scene::stream)). The
//!    *game* supplies a **focus point** (typically the player's position) and a
//!    **spawn callback** that says *what* to spawn; the *scene* owns the
//!    bookkeeping — which slots ahead of the focus are already filled and which
//!    streamed entities have fallen behind the despawn threshold.
//! 2. **Despawn** ([`despawn`](Scene::despawn), used by streaming).
//! 3. **Floating-origin rebase** ([`maybe_rebase`](Scene::maybe_rebase)). When the
//!    focus drifts past a threshold from the origin, every ECS
//!    [`TransformComponent`] is shifted by a fixed offset back toward the origin,
//!    so world coordinates never grow into float-precision error. Relative
//!    positions are preserved, so gameplay is unaffected. The offset is returned,
//!    so the game can shift whatever it keeps outside the ECS (a camera, its own
//!    physics).
//!
//! The scene has no physics. A game that wants rigid-body simulation runs its own
//! (for example `rapier3d`), steps it in its fixed update, and shifts it by the
//! offset [`maybe_rebase`](Scene::maybe_rebase) returns.
//!
//! # Streaming is deterministic
//!
//! Every streaming decision is a pure function of the focus position and the
//! [`StreamingConfig`]: the same focus trajectory always spawns the same slots and
//! despawns the same entities, and rebase fires at the same crossings. This keeps
//! a run reproducible (a prerequisite for any future golden test).

#![deny(missing_docs)]

use stickshift_ecs::hecs::{Entity, World};
use stickshift_ecs::TransformComponent;
use stickshift_math::glam::Vec3;

/// Configuration for the [`Scene`]'s spawn-ahead / despawn-behind streaming and
/// its floating-origin rebase, along a single travel axis.
///
/// Streaming is one-dimensional: content is generated *ahead* of a focus point
/// along [`axis`](Self::axis) and discarded once it falls *behind*. All distances
/// are measured as the signed projection of a position onto that axis. The
/// defaults ([`StreamingConfig::default`]) stream forward along `-Z` — the
/// convention an endless runner heading "into the screen" uses — but any unit
/// axis works.
///
/// # Example
///
/// ```rust
/// use stickshift_scene::StreamingConfig;
/// use stickshift_math::glam::Vec3;
///
/// let cfg = StreamingConfig {
///     axis: Vec3::new(0.0, 0.0, -1.0),
///     spawn_interval: 6.0,
///     spawn_ahead: 60.0,
///     despawn_behind: 12.0,
///     rebase_threshold: 1000.0,
/// };
/// assert!(cfg.spawn_ahead > cfg.spawn_interval);
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StreamingConfig {
    /// Unit direction of travel. Positions are projected onto this axis to get a
    /// scalar "distance along the world". Should be normalized; a zero axis
    /// disables spawning/despawning (every projection collapses to `0`).
    pub axis: Vec3,
    /// Spacing between spawn slots along [`axis`](Self::axis), in world units.
    /// Streaming fills one slot per multiple of this interval ahead of the focus.
    pub spawn_interval: f32,
    /// How far ahead of the focus (along the axis) to keep filled, in world units.
    pub spawn_ahead: f32,
    /// How far behind the focus (along the axis) a streamed entity may fall before
    /// it is despawned, in world units.
    pub despawn_behind: f32,
    /// Distance the focus may travel from the origin (along the axis) before a
    /// floating-origin rebase shifts everything back by that much.
    pub rebase_threshold: f32,
}

impl Default for StreamingConfig {
    fn default() -> Self {
        Self {
            axis: Vec3::new(0.0, 0.0, -1.0),
            spawn_interval: 6.0,
            spawn_ahead: 60.0,
            despawn_behind: 12.0,
            rebase_threshold: 1000.0,
        }
    }
}

/// The engine-generic scene: an ECS [`World`], with spawn/despawn streaming and
/// floating-origin rebase.
///
/// Construct with [`new`](Self::new) (default [`StreamingConfig`]) or
/// [`with_config`](Self::with_config). Access the world through
/// [`world`](Self::world) / [`world_mut`](Self::world_mut), and drive streaming
/// each fixed update with `stream`(Self::stream) and
/// [`maybe_rebase`](Self::maybe_rebase).
///
/// # Example
///
/// ```rust
/// use stickshift_scene::Scene;
/// use stickshift_ecs::TransformComponent;
/// use stickshift_math::glam::Vec3;
///
/// let mut scene = Scene::new();
/// scene.world_mut().spawn((TransformComponent::from_position(Vec3::ZERO),));
/// assert_eq!(scene.world().len(), 1);
/// ```
pub struct Scene {
    world: World,
    config: StreamingConfig,
    /// Entities the scene created via `stream`; candidates for despawn-behind.
    /// Ownership: the scene tracks these so it can remove them atomically. Each
    /// is kept with its spawn slot's distance along the axis since the start of
    /// the run (rebases included), which places an entity without a
    /// [`TransformComponent`].
    streamed: Vec<(Entity, f64)>,
    /// The furthest slot index (along the axis) that has already been spawned.
    /// `None` until the first `stream` call seeds the frontier from the focus.
    spawn_frontier: Option<i64>,
    /// Total offset (along the axis, in slot-distance) that rebases have shifted
    /// the world by, so slot indexing stays consistent across a rebase. It is
    /// `f64` because it grows with total distance travelled: in `f32`, slot
    /// positions would lose precision on a long run, which is exactly what the
    /// floating origin exists to prevent.
    rebase_shift_units: f64,
}

impl Scene {
    /// Creates an empty scene with the default [`StreamingConfig`].
    #[must_use]
    pub fn new() -> Self {
        Self::with_config(StreamingConfig::default())
    }

    /// Creates an empty scene with an explicit streaming configuration.
    #[must_use]
    pub fn with_config(config: StreamingConfig) -> Self {
        Self {
            world: World::new(),
            config,
            streamed: Vec::new(),
            spawn_frontier: None,
            rebase_shift_units: 0.0,
        }
    }

    /// Shared access to the ECS [`World`].
    #[must_use]
    pub fn world(&self) -> &World {
        &self.world
    }

    /// Mutable access to the ECS [`World`] — spawn, despawn, mutate components.
    #[must_use]
    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    /// The active [`StreamingConfig`].
    #[must_use]
    pub fn config(&self) -> StreamingConfig {
        self.config
    }

    /// Replaces the streaming configuration.
    ///
    /// Meant for a game that owns a scene it did not construct (the engine loop
    /// builds it with [`Scene::new`]) to set its own reach — typically once, in
    /// `init`, before the first [`stream`](Self::stream). Changing
    /// [`spawn_ahead`](StreamingConfig::spawn_ahead),
    /// [`despawn_behind`](StreamingConfig::despawn_behind) or
    /// [`rebase_threshold`](StreamingConfig::rebase_threshold) later is also safe:
    /// the next pass simply fills to / trims at the new distances.
    ///
    /// # Panics
    ///
    /// If streaming has already started and the new config changes
    /// [`axis`](StreamingConfig::axis) or
    /// [`spawn_interval`](StreamingConfig::spawn_interval): the spawn frontier is
    /// a slot index along that axis at that spacing, so either change would
    /// silently re-spawn or skip slots.
    ///
    /// # Example
    ///
    /// ```rust
    /// use stickshift_scene::{Scene, StreamingConfig};
    ///
    /// let mut scene = Scene::new();
    /// scene.set_config(StreamingConfig {
    ///     spawn_ahead: 180.0,
    ///     ..scene.config()
    /// });
    /// assert_eq!(scene.config().spawn_ahead, 180.0);
    /// ```
    pub fn set_config(&mut self, config: StreamingConfig) {
        assert!(
            self.spawn_frontier.is_none()
                || (config.axis == self.config.axis
                    && config.spawn_interval == self.config.spawn_interval),
            "cannot change the streaming axis or spawn_interval once streaming has started"
        );
        self.config = config;
    }

    /// Number of entities the scene is currently tracking for despawn-behind.
    ///
    /// This is the count of entities spawned through `stream` that have not yet
    /// fallen behind the despawn threshold. Bounded streaming keeps this flat.
    #[must_use]
    pub fn streamed_count(&self) -> usize {
        self.streamed.len()
    }

    /// Despawns an entity from the ECS world.
    ///
    /// Returns `true` if the entity existed and was removed, `false` if the handle
    /// was already gone (a safe no-op).
    pub fn despawn(&mut self, entity: Entity) -> bool {
        self.world.despawn(entity).is_ok()
    }

    /// Runs one streaming pass for the current `focus` position.
    ///
    /// Two phases, both pure functions of `focus` and the [`StreamingConfig`]:
    ///
    /// 1. **Despawn-behind.** Every streamed entity whose distance along the axis
    ///    has fallen more than [`despawn_behind`](StreamingConfig::despawn_behind)
    ///    behind the focus is removed atomically via [`despawn`](Self::despawn).
    ///    An entity's distance is its [`TransformComponent`]'s, or, for an
    ///    entity without one (a marker or logic entity), its spawn slot's. An
    ///    entity the game already despawned is dropped from tracking.
    /// 2. **Spawn-ahead.** For each not-yet-filled slot between the current
    ///    frontier and `focus + spawn_ahead` (slots spaced
    ///    [`spawn_interval`](StreamingConfig::spawn_interval) apart along the
    ///    axis), `spawn` is invoked with a [`SpawnCtx`] positioned at that slot.
    ///    Whatever entities the callback reports via
    ///    [`SpawnCtx::spawned`](SpawnCtx::spawned) become tracked streamed
    ///    entities (candidates for a later despawn). If the focus jumped past
    ///    the frontier (a teleport or a respawn far ahead), the slots it jumped
    ///    over are skipped: filling resumes from the focus, as on the first
    ///    pass, instead of spawning every slot in between at once.
    ///
    /// The `spawn` closure receives mutable access to the ECS world (through
    /// [`SpawnCtx`]) plus the world-space slot position, and decides *what* to
    /// spawn — the scene stays free of game content.
    ///
    /// # Example
    ///
    /// ```rust
    /// use stickshift_scene::{Scene, StreamingConfig};
    /// use stickshift_ecs::TransformComponent;
    /// use stickshift_math::glam::Vec3;
    ///
    /// let mut scene = Scene::with_config(StreamingConfig {
    ///     spawn_interval: 10.0,
    ///     spawn_ahead: 30.0,
    ///     despawn_behind: 10.0,
    ///     ..Default::default()
    /// });
    ///
    /// scene.stream(Vec3::ZERO, |cx| {
    ///     let e = cx.world.spawn((TransformComponent::from_position(cx.position),));
    ///     cx.spawned(e);
    /// });
    /// assert!(scene.streamed_count() > 0);
    /// ```
    pub fn stream<F>(&mut self, focus: Vec3, mut spawn: F)
    where
        F: FnMut(&mut SpawnCtx<'_>),
    {
        let axis = self.config.axis;
        let focus_d = focus.dot(axis);
        // Distances since the very start of the run, in `f64`: the running
        // rebase shift keeps them consistent across an origin rebase.
        let focus_total = f64::from(focus_d) + self.rebase_shift_units;

        // --- Step 1: despawn-behind ----------------------------------------
        let cutoff = focus_d - self.config.despawn_behind;
        let cutoff_total = focus_total - f64::from(self.config.despawn_behind);
        // Collect victims first so we don't mutate the world while borrowing it.
        let mut victims = Vec::new();
        let world = &self.world;
        self.streamed.retain(|&(e, slot_total)| {
            let behind = match world.get::<&TransformComponent>(e) {
                Ok(t) => t.transform.position.dot(axis) < cutoff,
                // No transform: place it at the slot it was spawned for.
                Err(_) if world.contains(e) => slot_total < cutoff_total,
                // Entity already gone (removed by game): drop from tracking.
                Err(_) => true,
            };
            if behind {
                victims.push(e);
            }
            !behind
        });
        for e in victims {
            self.despawn(e);
        }

        // --- Step 2: spawn-ahead -------------------------------------------
        let interval = self.config.spawn_interval;
        if interval <= 0.0 || self.config.spawn_ahead <= 0.0 {
            return;
        }

        let interval_f64 = f64::from(interval);
        let target = focus_total + f64::from(self.config.spawn_ahead);
        let target_slot = (target / interval_f64).floor() as i64;

        // Seed the frontier just behind the focus on the first pass so we don't
        // retroactively fill the whole world behind the player. A focus that
        // jumped past the frontier is seeded the same way: the slots it skipped
        // are behind it, so spawning them would only despawn them again.
        let seed = (focus_total / interval_f64).floor() as i64;
        let start = match self.spawn_frontier {
            Some(f) => (f + 1).max(seed),
            None => seed,
        };

        for slot in start..=target_slot {
            let slot_total = slot as f64 * interval_f64;
            // Subtract in `f64`, then narrow: the result is near the origin.
            let along = (slot_total - self.rebase_shift_units) as f32;
            let position = axis * along;
            let mut cx = SpawnCtx {
                world: &mut self.world,
                position,
                slot,
                out: Vec::new(),
            };
            spawn(&mut cx);
            let spawned = cx.out;
            self.streamed
                .extend(spawned.into_iter().map(|e| (e, slot_total)));
            self.spawn_frontier = Some(slot);
        }
    }

    /// Applies a floating-origin rebase if the focus has drifted past the
    /// threshold, shifting every ECS transform back toward the origin by a fixed
    /// offset.
    ///
    /// The shift is a whole multiple of the focus's distance along the axis,
    /// quantized to [`rebase_threshold`](StreamingConfig::rebase_threshold), so
    /// the focus is pulled back to within one threshold of the origin. Relative
    /// positions between any two entities are preserved exactly (every position
    /// moves by the same vector), so gameplay never notices.
    ///
    /// Every [`TransformComponent`] is shifted, so every transform must be in
    /// world space. Anything the game keeps outside the ECS in world coordinates
    /// (a camera, its own physics world) must be shifted by the returned offset
    /// too; a physics world should be shifted between its steps, not inside one.
    ///
    /// Returns the offset vector actually applied (`Vec3::ZERO` if no rebase was
    /// due this call).
    pub fn maybe_rebase(&mut self, focus: Vec3) -> Vec3 {
        let axis = self.config.axis;
        let threshold = self.config.rebase_threshold;
        if threshold <= 0.0 {
            return Vec3::ZERO;
        }

        let focus_d = focus.dot(axis);
        if focus_d.abs() < threshold {
            return Vec3::ZERO;
        }

        // Quantize the shift to whole thresholds so the operation is
        // deterministic and slot indexing stays aligned.
        let steps = (focus_d / threshold).trunc();
        let shift_along = steps * threshold;
        if shift_along == 0.0 {
            return Vec3::ZERO;
        }
        // Move everything back by `shift_along` along the axis.
        let offset = axis * (-shift_along);

        // Shift ECS transforms.
        for (_e, transform) in self.world.query_mut::<&mut TransformComponent>() {
            transform.transform.position += offset;
        }

        // Track the shift so slot indexing stays consistent with pre-rebase space.
        self.rebase_shift_units += f64::from(shift_along);

        offset
    }
}

impl Default for Scene {
    fn default() -> Self {
        Self::new()
    }
}

/// The handle a `stream`(Scene::stream) spawn callback uses to populate one slot.
///
/// It grants mutable access to the scene's ECS [`World`] and carries the
/// world-space [`position`](Self::position) of the slot being filled. The
/// callback creates whatever entities it wants and reports each one it
/// wants the scene to *track* (so it participates in despawn-behind) via
/// [`spawned`](Self::spawned). Entities the callback creates but does not report
/// are left untracked (e.g. permanent scenery).
pub struct SpawnCtx<'a> {
    /// Mutable access to the ECS world for spawning entities.
    pub world: &'a mut World,
    /// World-space position of the slot being filled.
    pub position: Vec3,
    /// The integer slot index being filled (monotonic along the axis).
    pub slot: i64,
    out: Vec<Entity>,
}

impl SpawnCtx<'_> {
    /// Registers `entity` as a streamed entity so the scene despawns it once it
    /// falls behind the despawn threshold.
    pub fn spawned(&mut self, entity: Entity) {
        self.out.push(entity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stickshift_ecs::TransformComponent;
    use stickshift_math::glam::Vec3;

    /// Helper: spawn an entity with a transform at `position`.
    fn spawn_at(scene: &mut Scene, position: Vec3) -> Entity {
        scene
            .world_mut()
            .spawn((TransformComponent::from_position(position),))
    }

    /// Streaming across floating-origin rebases fills every slot exactly once:
    /// no slot is spawned twice and none is skipped.
    #[test]
    fn stream_across_rebases_has_no_duplicate_or_missing_slots() {
        let mut scene = Scene::with_config(StreamingConfig {
            despawn_behind: 1e9,
            ..StreamingConfig::default()
        });
        let mut slots: Vec<i64> = Vec::new();
        let mut travel = 0.0f32;
        let mut rebases = 0;
        for _ in 0..3000 {
            travel += 0.75;
            let offset = scene.maybe_rebase(Vec3::new(0.0, 0.0, -travel));
            if offset != Vec3::ZERO {
                rebases += 1;
            }
            travel += offset.dot(Vec3::NEG_Z);
            scene.stream(Vec3::new(0.0, 0.0, -travel), |cx| {
                slots.push(cx.slot);
                let e = cx
                    .world
                    .spawn((TransformComponent::from_position(cx.position),));
                cx.spawned(e);
            });
        }
        assert!(rebases > 0, "the run crossed at least one rebase");
        for w in slots.windows(2) {
            assert_eq!(w[1], w[0] + 1, "slots are contiguous and unique");
        }
    }

    /// Despawn removes the entity, and despawning it again is a safe no-op.
    #[test]
    fn despawn_removes_the_entity() {
        let mut scene = Scene::new();
        let e = spawn_at(&mut scene, Vec3::ZERO);
        assert!(scene.despawn(e));
        assert!(!scene.world().contains(e));
        // double-despawn is a safe no-op
        assert!(!scene.despawn(e));
    }

    /// Streaming spawns ahead of the focus and despawns behind it, atomically.
    #[test]
    fn stream_spawns_ahead_and_despawns_behind() {
        let mut scene = Scene::with_config(StreamingConfig {
            axis: Vec3::new(0.0, 0.0, -1.0),
            spawn_interval: 10.0,
            spawn_ahead: 30.0,
            despawn_behind: 10.0,
            rebase_threshold: 100_000.0,
        });

        // First pass at origin: fills slots ahead (along -Z).
        scene.stream(Vec3::ZERO, |cx| {
            let e = cx
                .world
                .spawn((TransformComponent::from_position(cx.position),));
            cx.spawned(e);
        });
        let after_first = scene.streamed_count();
        assert!(after_first > 0, "spawned ahead on the first pass");

        // All streamed entities are ahead (negative Z, matching -Z axis).
        for &(e, _) in &scene.streamed {
            let z = scene
                .world()
                .get::<&TransformComponent>(e)
                .unwrap()
                .transform
                .position
                .z;
            assert!(z <= 0.0, "streamed entity is ahead along -Z, z={z}");
        }

        // Advance the focus one interval per pass (realistic streaming), so the
        // frontier never has to close a large gap in one call.
        let mut last_focus = Vec3::ZERO;
        for step in 1..=20 {
            last_focus = Vec3::new(0.0, 0.0, -(step as f32) * 10.0);
            scene.stream(last_focus, |cx| {
                let e = cx
                    .world
                    .spawn((TransformComponent::from_position(cx.position),));
                cx.spawned(e);
            });
        }

        // Under gradual advance, every surviving entity sits within the streaming
        // window: no further behind than `despawn_behind` and no further ahead
        // than `spawn_ahead` of the focus.
        let axis = scene.config().axis;
        let focus_along = last_focus.dot(axis);
        for &(e, _) in &scene.streamed {
            let along = scene
                .world()
                .get::<&TransformComponent>(e)
                .unwrap()
                .transform
                .position
                .dot(axis);
            assert!(
                along >= focus_along - scene.config().despawn_behind - 1e-3,
                "survivor {along} must not be behind the despawn cutoff {}",
                focus_along - scene.config().despawn_behind
            );
            assert!(
                along <= focus_along + scene.config().spawn_ahead + 1e-3,
                "survivor {along} must not be beyond the spawn-ahead horizon"
            );
        }
    }

    /// `set_config` before the first pass changes how far streaming reaches.
    #[test]
    fn set_config_extends_the_spawn_reach() {
        let spawn = |cx: &mut SpawnCtx<'_>| {
            let e = cx
                .world
                .spawn((TransformComponent::from_position(cx.position),));
            cx.spawned(e);
        };
        let mut near = Scene::new();
        near.stream(Vec3::ZERO, spawn);
        let mut far = Scene::new();
        far.set_config(StreamingConfig {
            spawn_ahead: near.config().spawn_ahead * 3.0,
            ..far.config()
        });
        far.stream(Vec3::ZERO, spawn);
        // Slots 0..=ahead/interval inclusive: 11 at 60, 31 at 180.
        assert_eq!(near.streamed_count(), 11);
        assert_eq!(far.streamed_count(), 31);
    }

    /// The frontier is a slot index, so the spacing cannot change mid-stream.
    #[test]
    #[should_panic(expected = "spawn_interval")]
    fn set_config_rejects_a_new_interval_once_streaming() {
        let mut scene = Scene::new();
        scene.stream(Vec3::ZERO, |_| {});
        scene.set_config(StreamingConfig {
            spawn_interval: 3.0,
            ..scene.config()
        });
    }

    /// No unbounded growth: driving many frames of advancing focus keeps the ECS
    /// entity count bounded.
    #[test]
    fn bounded_counts_over_many_frames() {
        let cfg = StreamingConfig {
            axis: Vec3::new(0.0, 0.0, -1.0),
            spawn_interval: 5.0,
            spawn_ahead: 50.0,
            despawn_behind: 10.0,
            rebase_threshold: 100_000.0,
        };
        let mut scene = Scene::with_config(cfg);

        // Theoretical bound: slots in [focus - despawn_behind, focus + spawn_ahead]
        // = (despawn_behind + spawn_ahead) / spawn_interval, plus a small margin.
        let window = cfg.despawn_behind + cfg.spawn_ahead;
        let bound = (window / cfg.spawn_interval).ceil() as usize + 3;

        let mut max_entities = 0usize;

        // Advance the focus one interval per "frame" for many frames.
        for frame in 0..2000i64 {
            let focus = Vec3::new(0.0, 0.0, -(frame as f32) * cfg.spawn_interval);
            scene.stream(focus, |cx| {
                let e = cx
                    .world
                    .spawn((TransformComponent::from_position(cx.position),));
                cx.spawned(e);
            });
            max_entities = max_entities.max(scene.world().len() as usize);
        }

        assert!(
            max_entities <= bound,
            "entity count stayed bounded: max={max_entities}, bound={bound}"
        );
        // And nothing leaked: every live entity is a tracked one.
        assert_eq!(scene.world().len() as usize, scene.streamed_count());
    }

    /// Origin rebase preserves relative positions of two entities across the shift.
    #[test]
    fn rebase_preserves_relative_positions() {
        let cfg = StreamingConfig {
            axis: Vec3::new(0.0, 0.0, -1.0),
            spawn_interval: 10.0,
            spawn_ahead: 10.0,
            despawn_behind: 1e9, // never despawn during this test
            rebase_threshold: 1000.0,
        };
        let mut scene = Scene::with_config(cfg);

        // Two entities far out along -Z.
        let a_pos = Vec3::new(1.0, 2.0, -1500.0);
        let b_pos = Vec3::new(4.0, 2.0, -1490.0);
        let a = spawn_at(&mut scene, a_pos);
        let b = spawn_at(&mut scene, b_pos);

        let rel_ecs_before = b_pos - a_pos;

        // Focus is out past the threshold -> rebase fires.
        let offset = scene.maybe_rebase(Vec3::new(0.0, 0.0, -1500.0));
        assert_ne!(offset, Vec3::ZERO, "a rebase should have occurred");

        // ECS relative position preserved.
        let a_after = scene
            .world()
            .get::<&TransformComponent>(a)
            .unwrap()
            .transform
            .position;
        let b_after = scene
            .world()
            .get::<&TransformComponent>(b)
            .unwrap()
            .transform
            .position;
        assert!((b_after - a_after - rel_ecs_before).length() < 1e-3);

        // Both moved back toward the origin (smaller |z|), by the returned offset.
        assert!(a_after.z.abs() < a_pos.z.abs());
        assert!((a_after - (a_pos + offset)).length() < 1e-3);
    }

    /// No rebase fires while the focus stays within the threshold.
    #[test]
    fn no_rebase_within_threshold() {
        let mut scene = Scene::new();
        let offset = scene.maybe_rebase(Vec3::new(0.0, 0.0, -10.0));
        assert_eq!(offset, Vec3::ZERO);
    }

    /// Slot positions stay exact however far the world has travelled. A
    /// 20,000 km shift puts slot coordinates where `f32` spacing is 2 units, so
    /// computing them in `f32` misplaced every other slot.
    #[test]
    fn streamed_positions_stay_exact_far_from_the_start() {
        const INTERVAL: f32 = 3.0;
        let mut scene = Scene::with_config(StreamingConfig {
            axis: Vec3::NEG_Z,
            spawn_interval: INTERVAL,
            spawn_ahead: 30.0,
            despawn_behind: 1e9,
            rebase_threshold: 1000.0,
        });
        let shift = scene.maybe_rebase(Vec3::new(0.0, 0.0, -2.0e7));
        assert_eq!(shift, Vec3::new(0.0, 0.0, 2.0e7), "one big rebase");

        let mut spawned = Vec::new();
        scene.stream(Vec3::ZERO, |cx| spawned.push((cx.slot, cx.position)));
        assert!(spawned.len() >= 10, "{} slots", spawned.len());
        for (slot, position) in spawned {
            // Distance along the axis since the very start, minus the shift.
            let along = (slot as f64 * f64::from(INTERVAL) - 2.0e7) as f32;
            assert_eq!(position, Vec3::NEG_Z * along, "slot {slot}");
        }
    }

    /// Engine/game boundary guard: `stickshift-scene` must stay engine-generic — no game-named
    /// symbol may appear anywhere in its source. Mirrors the `stickshift-ecs` /
    /// `stickshift-core` guards: forbidden words are built from ASCII byte codes so
    /// this test's own body contains no literal occurrence, letting it scan the
    /// whole file (including itself) with no exclusion window.
    #[test]
    fn no_game_specific_symbols() {
        let src = include_str!("lib.rs");

        // Codes spell the forbidden game concepts:
        //   [99,97,114], [114,111,97,100], [115,99,111,114,101],
        //   [111,98,115,116,97,99,108,101], [108,97,110,101]
        let forbidden: Vec<String> = [
            &[99u8, 97, 114][..],
            &[114, 111, 97, 100][..],
            &[115, 99, 111, 114, 101][..],
            &[111, 98, 115, 116, 97, 99, 108, 101][..],
            &[108, 97, 110, 101][..],
        ]
        .iter()
        .map(|bytes| String::from_utf8(bytes.to_vec()).unwrap())
        .collect();

        for token in src.split(|c: char| !c.is_ascii_alphanumeric()) {
            if token.is_empty() {
                continue;
            }
            let lower = token.to_ascii_lowercase();
            for bad in &forbidden {
                assert_ne!(
                    &lower, bad,
                    "game-specific identifier `{token}` found in lib.rs: \
                     stickshift-scene must stay engine-generic (engine/game boundary)",
                );
            }
        }
    }

    /// A streaming config with one slot per unit along -Z.
    fn unit_slots(spawn_ahead: f32, despawn_behind: f32) -> StreamingConfig {
        StreamingConfig {
            spawn_interval: 1.0,
            spawn_ahead,
            despawn_behind,
            ..StreamingConfig::default()
        }
    }

    /// A streamed entity without a transform is kept while its slot is in
    /// range, and despawned once its slot falls behind, like any other.
    #[test]
    fn a_streamed_entity_without_a_transform_follows_its_slot() {
        let mut scene = Scene::with_config(unit_slots(3.0, 2.0));
        let mut markers = Vec::new();
        scene.stream(Vec3::ZERO, |cx| {
            // A logic entity with no transform.
            let e = cx.world.spawn(());
            markers.push(e);
            cx.spawned(e);
        });
        let first = markers[0];
        // Another pass at the same focus: it used to be despawned here.
        scene.stream(Vec3::ZERO, |_| {});
        assert!(
            scene.world().contains(first),
            "kept while its slot is in range"
        );
        // Far enough ahead that its slot is behind the despawn threshold.
        scene.stream(Vec3::new(0.0, 0.0, -10.0), |_| {});
        assert!(!scene.world().contains(first), "despawned once behind");
    }

    /// A focus that jumps far past the frontier spawns only the slots in
    /// range of its new position, not every slot it jumped over.
    #[test]
    fn a_teleport_spawns_only_the_slots_in_range() {
        let mut scene = Scene::with_config(unit_slots(10.0, 5.0));
        let mut spawned = 0;
        let mut spawn = |cx: &mut SpawnCtx<'_>| {
            let e = cx
                .world
                .spawn((TransformComponent::from_position(cx.position),));
            cx.spawned(e);
            spawned += 1;
        };
        scene.stream(Vec3::ZERO, &mut spawn);
        scene.stream(Vec3::new(0.0, 0.0, -100_000.0), &mut spawn);
        // The first pass fills 0..=10 and the jump fills 100000..=100010:
        // 22 slots, not 100,011.
        assert_eq!(spawned, 22);
        assert_eq!(scene.streamed_count(), 11, "the old slots fell behind");
    }
}
