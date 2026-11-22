// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! The [`Scene`] — owner of the ECS [`World`] and the [`PhysicsWorld`], plus the
//! engine-generic **world-streaming** layer for Stick Shift Engine.
//!
//! A [`Scene`] bundles the two simulation stores a running world needs — a hecs
//! [`World`] of entities/components and a
//! [`PhysicsWorld`] of rigid bodies — behind one
//! type, and drives them in lockstep at the engine's fixed timestep
//! ([`step_physics`](Scene::step_physics), stepping at
//! [`stickshift_physics::FIXED_DT`]).
//!
//! On top of that it provides three things an endless-world game needs, all
//! **engine-generic** (there are deliberately no game concepts here — no vehicle,
//! track, tally, hazard, or gameplay-lattice types; a test scans this crate's
//! source to keep it that way):
//!
//! 1. **Spawn-ahead / despawn-behind streaming** (`stream`(Scene::stream)). The
//!    *game* supplies a **focus point** (typically the player's position) and a
//!    **spawn callback** that says *what* to spawn; the *scene* owns the
//!    bookkeeping — which slots ahead of the focus are already filled and which
//!    streamed entities have fallen behind the despawn threshold.
//! 2. **Atomic despawn** ([`despawn`](Scene::despawn), used by streaming). Removing
//!    a streamed entity removes its ECS entity **and** its physics rigid body (via
//!    [`PhysicsWorld::remove_body`]) together, so no live
//!    [`PhysicsBodyComponent`] is ever left
//!    holding a freed handle (the physics/ECS handle-ownership invariant).
//! 3. **Floating-origin rebase** ([`maybe_rebase`](Scene::maybe_rebase)). When the
//!    focus drifts past a threshold from the origin, along the travel axis or off
//!    it to either side, every position — ECS [`TransformComponent`] *and* every
//!    physics body translation — is shifted by a fixed offset back toward the
//!    origin, so world coordinates never grow into float-precision error.
//!    Relative positions are preserved, so gameplay is unaffected.
//!
//! # Streaming is deterministic
//!
//! Every streaming decision is a pure function of the focus position and the
//! [`StreamingConfig`]: the same focus trajectory always spawns the same slots and
//! despawns the same entities, and rebase fires at the same crossings. This keeps
//! a run reproducible (a prerequisite for any future golden test).
//!
//! # Rebase-ordering invariant
//!
//! A rebase teleports rigid bodies ([`PhysicsWorld::shift_origin`]). To avoid
//! solver artifacts it must run **between** physics steps, never inside one:
//! [`step_physics`](Scene::step_physics) completes a full solve, then
//! [`maybe_rebase`](Scene::maybe_rebase) shifts everything, then the next
//! [`step_physics`](Scene::step_physics) begins from the shifted (consistent)
//! state. The recommended per-fixed-update order is therefore
//! `stream` → `step_physics` → `maybe_rebase`. Driving the shift mid-solve would
//! feed the constraint solver a discontinuous position within a single step; the
//! documented order prevents that.

#![deny(missing_docs)]

use std::collections::{HashMap, HashSet};

use stickshift_ecs::hecs::{Entity, World};
use stickshift_ecs::{DynamicTag, PhysicsBodyComponent, TransformComponent};
use stickshift_math::glam::{DVec3, Vec3};
use stickshift_physics::{PhysicsWorld, RigidBodyHandle, FIXED_DT};

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
    /// Distance the focus may travel from the origin before a floating-origin
    /// rebase shifts everything back by that much: along the axis, or off it,
    /// in any world direction across it.
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

/// Why [`Scene::try_set_config`] refused a [`StreamingConfig`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StreamingConfigError {
    /// Streaming has started, and the new config changes
    /// [`axis`](StreamingConfig::axis) or
    /// [`spawn_interval`](StreamingConfig::spawn_interval). The spawn frontier is
    /// a slot index along that axis at that spacing, so either change would
    /// silently re-spawn or skip slots.
    AxisOrIntervalChanged,
}

impl std::fmt::Display for StreamingConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AxisOrIntervalChanged => f.write_str(
                "cannot change the streaming axis or spawn_interval once streaming has started",
            ),
        }
    }
}

impl std::error::Error for StreamingConfigError {}

/// The engine-generic simulation scene: an ECS [`World`] plus a
/// [`PhysicsWorld`], with spawn/despawn streaming and floating-origin rebase.
///
/// Construct with [`new`](Self::new) (default [`StreamingConfig`]) or
/// [`with_config`](Self::with_config). Access the stores through
/// [`world`](Self::world) / [`world_mut`](Self::world_mut) and
/// [`physics`](Self::physics) / [`physics_mut`](Self::physics_mut), advance
/// physics with [`step_physics`](Self::step_physics), and drive streaming each
/// fixed update with `stream`(Self::stream) and [`maybe_rebase`](Self::maybe_rebase).
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
/// scene.step_physics();
/// assert_eq!(scene.world().len(), 1);
/// ```
pub struct Scene {
    world: World,
    physics: PhysicsWorld,
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
    /// How many rebases have shifted the world...
    rebase_count: u64,
    /// ...and by how much in all: the sum of every offset applied, in `f64`
    /// for the same reason as `rebase_shift_units`.
    rebase_total: DVec3,
    /// Which entity owns which rigid body, as last seen by a
    /// [`step_physics`](Self::step_physics) or [`stream`](Self::stream) pass.
    /// Lets the scene free a body whose entity was despawned through
    /// [`world_mut`](Self::world_mut) instead of [`despawn`](Self::despawn).
    body_owners: HashMap<Entity, RigidBodyHandle>,
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
            physics: PhysicsWorld::new(),
            config,
            streamed: Vec::new(),
            spawn_frontier: None,
            rebase_shift_units: 0.0,
            rebase_count: 0,
            rebase_total: DVec3::ZERO,
            body_owners: HashMap::new(),
        }
    }

    /// Shared access to the ECS [`World`].
    #[must_use]
    pub fn world(&self) -> &World {
        &self.world
    }

    /// Mutable access to the ECS [`World`] — spawn, despawn, mutate components.
    ///
    /// Prefer [`despawn`](Self::despawn) for an entity that carries a
    /// [`PhysicsBodyComponent`]: it frees the body in the same call. Despawning
    /// such an entity here directly is still safe once the scene has seen it in a
    /// [`step_physics`](Self::step_physics) or [`stream`](Self::stream) pass: the
    /// next `step_physics` frees the orphaned body before stepping, so no ghost
    /// collider is left behind. A body created and orphaned before the scene ever
    /// saw its entity is not tracked and stays in the physics world.
    #[must_use]
    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    /// Shared access to the [`PhysicsWorld`].
    #[must_use]
    pub fn physics(&self) -> &PhysicsWorld {
        &self.physics
    }

    /// Mutable access to the [`PhysicsWorld`] — create bodies, add colliders.
    #[must_use]
    pub fn physics_mut(&mut self) -> &mut PhysicsWorld {
        &mut self.physics
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
        if let Err(err) = self.try_set_config(config) {
            panic!("{err}");
        }
    }

    /// Replaces the streaming configuration, or refuses and changes nothing.
    ///
    /// [`set_config`](Self::set_config) without the panic, for a caller that
    /// cannot unwind (a host across a foreign-function boundary): the same
    /// check, reported as a [`StreamingConfigError`] instead. Every other value
    /// is taken as given, as `set_config` takes it.
    ///
    /// # Errors
    ///
    /// [`StreamingConfigError::AxisOrIntervalChanged`] if streaming has already
    /// started and the new config changes [`axis`](StreamingConfig::axis) or
    /// [`spawn_interval`](StreamingConfig::spawn_interval). The scene keeps its
    /// old config.
    ///
    /// # Example
    ///
    /// ```rust
    /// use stickshift_math::glam::Vec3;
    /// use stickshift_scene::{Scene, StreamingConfig, StreamingConfigError};
    ///
    /// let mut scene = Scene::new();
    /// scene.stream(Vec3::ZERO, |_| {});
    /// // The reach may change mid-stream; the spacing may not.
    /// let reach = StreamingConfig { spawn_ahead: 180.0, ..scene.config() };
    /// assert_eq!(scene.try_set_config(reach), Ok(()));
    /// let spacing = StreamingConfig { spawn_interval: 3.0, ..scene.config() };
    /// assert_eq!(
    ///     scene.try_set_config(spacing),
    ///     Err(StreamingConfigError::AxisOrIntervalChanged)
    /// );
    /// assert_eq!(scene.config(), reach);
    /// ```
    pub fn try_set_config(&mut self, config: StreamingConfig) -> Result<(), StreamingConfigError> {
        if self.spawn_frontier.is_some()
            && (config.axis != self.config.axis
                || config.spawn_interval != self.config.spawn_interval)
        {
            return Err(StreamingConfigError::AxisOrIntervalChanged);
        }
        self.config = config;
        Ok(())
    }

    /// Number of entities the scene is currently tracking for despawn-behind.
    ///
    /// This is the count of entities spawned through `stream` that have not yet
    /// fallen behind the despawn threshold. Bounded streaming keeps this flat.
    #[must_use]
    pub fn streamed_count(&self) -> usize {
        self.streamed.len()
    }

    /// How many floating-origin rebases ([`maybe_rebase`](Self::maybe_rebase))
    /// have shifted the world. For diagnostics.
    #[must_use]
    pub fn rebase_count(&self) -> u64 {
        self.rebase_count
    }

    /// The sum of every offset the rebases have applied, in `f64`: a position
    /// now, less this, is where it would be had the world never been shifted.
    /// For diagnostics: how far the focus has truly travelled.
    #[must_use]
    pub fn rebase_total(&self) -> DVec3 {
        self.rebase_total
    }

    /// Advances physics by one fixed timestep ([`FIXED_DT`]) and syncs transforms.
    ///
    /// Steps the [`PhysicsWorld`] once, then copies each dynamic body's simulated
    /// transform back into its entity's [`TransformComponent`] (only entities
    /// carrying a [`PhysicsBodyComponent`] and a
    /// [`DynamicTag`] are synced; static geometry keeps its
    /// authored transform). Call this exactly once per fixed engine update so the
    /// solver stays in lockstep with `stickshift_core`'s fixed-timestep driver.
    ///
    /// Only the position and rotation are synced; the entity's authored `scale`
    /// is kept (rigid bodies have no scale).
    ///
    /// Before stepping, any body whose owning entity has been despawned through
    /// [`world_mut`](Self::world_mut) is freed (see that method), so it cannot
    /// collide as a ghost.
    ///
    /// A stale handle on a component (should not occur under the atomic-despawn
    /// invariant) is skipped safely — [`PhysicsWorld::get_transform`] returns
    /// `None` — rather than panicking.
    pub fn step_physics(&mut self) {
        self.reclaim_orphaned_bodies();
        self.physics.step();

        for (_e, (transform, body, _dynamic)) in
            self.world
                .query_mut::<(&mut TransformComponent, &PhysicsBodyComponent, &DynamicTag)>()
        {
            if let Some(t) = self.physics.get_transform(body.handle) {
                transform.transform.position = t.position;
                transform.transform.rotation = t.rotation;
            }
        }
    }

    /// Frees the bodies their owners let go of, then records the current owner
    /// of every body still attached to a live entity.
    ///
    /// An owner lets go of its body when it is despawned, or when its
    /// [`PhysicsBodyComponent`] is removed or replaced, through
    /// [`world_mut`](Self::world_mut). The body is freed unless another live
    /// entity has since taken it over.
    fn reclaim_orphaned_bodies(&mut self) {
        let world = &self.world;
        let let_go = |(&entity, &handle): (&Entity, &RigidBodyHandle)| {
            world
                .get::<&PhysicsBodyComponent>(entity)
                .map_or(true, |body| body.handle != handle)
        };
        // The common case: every owner still holds its body. Checking costs one
        // lookup per owner and allocates nothing.
        if self.body_owners.iter().any(let_go) {
            let held: HashSet<RigidBodyHandle> = world
                .query::<&PhysicsBodyComponent>()
                .iter()
                .map(|(_e, b)| b.handle)
                .collect();
            let orphans: HashSet<RigidBodyHandle> = self
                .body_owners
                .values()
                .copied()
                .filter(|handle| !held.contains(handle))
                .collect();
            for handle in orphans {
                self.physics.remove_body(handle);
            }
        }
        self.body_owners.clear();
        for (entity, body) in self.world.query::<&PhysicsBodyComponent>().iter() {
            self.body_owners.insert(entity, body.handle);
        }
    }

    /// The fixed timestep [`step_physics`](Self::step_physics) advances by.
    ///
    /// Re-exported through the scene for callers that drive the loop; equal to
    /// [`stickshift_physics::FIXED_DT`] (and thus `stickshift_core::FIXED_DT`).
    #[must_use]
    pub const fn fixed_dt() -> f32 {
        FIXED_DT
    }

    /// Despawns an entity **atomically**: removes it from the ECS world and, if it
    /// carries a [`PhysicsBodyComponent`], removes that rigid body from the
    /// physics world in the same call.
    ///
    /// This upholds the handle-ownership invariant: the ECS
    /// entity and its physics body are torn down together, so no live component is
    /// ever left holding a freed [`RigidBodyHandle`](stickshift_physics::RigidBodyHandle),
    /// and no rigid body outlives the entity that owned it. The body is removed
    /// **before** the entity, so even a mid-teardown observer never sees the
    /// component without its body.
    ///
    /// Returns `true` if the entity existed and was removed, `false` if the handle
    /// was already gone (a safe no-op).
    pub fn despawn(&mut self, entity: Entity) -> bool {
        if !self.world.contains(entity) {
            return false;
        }
        self.body_owners.remove(&entity);
        // Remove the physics body first (if any), then the entity — the two
        // freed together, no stale handle in between.
        if let Ok(body) = self.world.get::<&PhysicsBodyComponent>(entity) {
            let handle = body.handle;
            drop(body);
            self.physics.remove_body(handle);
        }
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
    /// The `spawn` closure receives mutable access to the ECS world and physics
    /// world (through [`SpawnCtx`]) plus the world-space slot position, and decides
    /// *what* to spawn — the scene stays free of game content.
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
            // A focus so far along (or infinite) that its slot saturates at
            // `i64::MAX` leaves a frontier with no slot past it: nothing to spawn.
            Some(f) => match f.checked_add(1) {
                Some(next) => next.max(seed),
                None => return,
            },
            None => seed,
        };

        for slot in start..=target_slot {
            let slot_total = slot as f64 * interval_f64;
            // Subtract in `f64`, then narrow: the result is near the origin.
            let along = (slot_total - self.rebase_shift_units) as f32;
            let position = axis * along;
            let mut cx = SpawnCtx {
                world: &mut self.world,
                physics: &mut self.physics,
                position,
                slot,
                out: Vec::new(),
            };
            spawn(&mut cx);
            let spawned = cx.out;
            for &e in &spawned {
                if let Ok(body) = self.world.get::<&PhysicsBodyComponent>(e) {
                    self.body_owners.insert(e, body.handle);
                }
            }
            self.streamed
                .extend(spawned.into_iter().map(|e| (e, slot_total)));
            self.spawn_frontier = Some(slot);
        }
    }

    /// Applies a floating-origin rebase if the focus has drifted past the
    /// threshold, shifting **all** positions — ECS transforms *and* physics body
    /// translations — back toward the origin by a fixed offset.
    ///
    /// The shift is quantized to whole multiples of
    /// [`rebase_threshold`](StreamingConfig::rebase_threshold), so the focus is
    /// pulled back to within one threshold of the origin:
    ///
    /// - **Along the axis**, a whole number of thresholds of the focus's
    ///   distance along it. The scene keeps count of this part, so slot indexing
    ///   is unchanged by it ([`stream`](Self::stream) fills the same slots).
    /// - **Across the axis**, where a focus that does not travel straight down
    ///   it (one that winds) drifts too: a whole number of thresholds of each
    ///   world component of the focus's offset from the axis's line, with any
    ///   part along the axis taken out, so slot indexing never sees it.
    ///
    /// Relative positions between any two entities are preserved exactly
    /// (every position moves by the same vector), so gameplay never notices.
    ///
    /// Every [`TransformComponent`] is shifted, so every transform must be in
    /// world space. The whole physics world is shifted with
    /// [`PhysicsWorld::shift_origin`], so every rigid body moves exactly once,
    /// whether or not an entity holds it.
    ///
    /// Must be called **between** physics steps, not inside one (see the crate-
    /// level rebase-ordering invariant): it teleports rigid bodies via
    /// [`PhysicsWorld::shift_origin`], which is only safe outside a solve.
    ///
    /// Returns the offset vector actually applied (`Vec3::ZERO` if no rebase was
    /// due this call).
    pub fn maybe_rebase(&mut self, focus: Vec3) -> Vec3 {
        let axis = self.config.axis;
        let threshold = self.config.rebase_threshold;
        if threshold <= 0.0 {
            return Vec3::ZERO;
        }

        // Quantize the shift to whole thresholds so the operation is
        // deterministic and slot indexing stays aligned: along the axis...
        let focus_d = focus.dot(axis);
        let shift_along = (focus_d / threshold).trunc() * threshold;
        // ...and across it, each world component of the focus's offset from
        // the axis's line, less any part along the axis (none, for an axis
        // along a world axis), which slot indexing would otherwise see.
        let across = focus - axis * focus_d;
        let mut shift_across = (across / threshold).trunc() * threshold;
        shift_across -= axis * shift_across.dot(axis);
        if shift_along == 0.0 && shift_across == Vec3::ZERO {
            return Vec3::ZERO;
        }
        // Move everything back by both.
        let offset = -(axis * shift_along + shift_across);

        // Shift ECS transforms.
        for (_e, transform) in self.world.query_mut::<&mut TransformComponent>() {
            transform.transform.position += offset;
        }

        // Shift the whole physics world to match, between steps. Every body
        // moves exactly once: also bodies no entity holds (ground made through
        // `physics_mut()`), and a body several entities share is not moved
        // once per holder.
        self.physics.shift_origin(offset);

        // Track the shift so slot indexing stays consistent with pre-rebase space.
        self.rebase_shift_units += f64::from(shift_along);
        self.rebase_count += 1;
        self.rebase_total += offset.as_dvec3();

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
/// It grants mutable access to the scene's ECS [`World`] and [`PhysicsWorld`] and
/// carries the world-space [`position`](Self::position) of the slot being filled.
/// The callback creates whatever entities/bodies it wants and reports each one it
/// wants the scene to *track* (so it participates in despawn-behind) via
/// [`spawned`](Self::spawned). Entities the callback creates but does not report
/// are left untracked (e.g. permanent scenery).
pub struct SpawnCtx<'a> {
    /// Mutable access to the ECS world for spawning entities.
    pub world: &'a mut World,
    /// Mutable access to the physics world for creating bodies/colliders.
    pub physics: &'a mut PhysicsWorld,
    /// World-space position of the slot being filled.
    pub position: Vec3,
    /// The integer slot index being filled (monotonic along the axis).
    pub slot: i64,
    out: Vec<Entity>,
}

impl SpawnCtx<'_> {
    /// Registers `entity` as a streamed entity so the scene despawns it (with its
    /// physics body) once it falls behind the despawn threshold.
    pub fn spawned(&mut self, entity: Entity) {
        self.out.push(entity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stickshift_ecs::{DynamicTag, PhysicsBodyComponent, TransformComponent};
    use stickshift_math::glam::Vec3;

    /// Helper: spawn a dynamic entity with a physics body at `position`.
    fn spawn_body(scene: &mut Scene, position: Vec3) -> Entity {
        use stickshift_math::Transform;
        let handle = scene
            .physics_mut()
            .create_dynamic_body(Transform::from_position(position));
        scene
            .physics_mut()
            .add_box_collider(handle, Vec3::new(0.5, 0.5, 0.5));
        scene.world_mut().spawn((
            TransformComponent::from_position(position),
            PhysicsBodyComponent::new(handle),
            DynamicTag,
        ))
    }

    #[test]
    fn step_physics_syncs_dynamic_transform_from_body() {
        let mut scene = Scene::new();
        let pos = Vec3::new(0.0, 10.0, 0.0);
        let e = spawn_body(&mut scene, pos);

        for _ in 0..30 {
            scene.step_physics();
        }
        let y = scene
            .world()
            .get::<&TransformComponent>(e)
            .unwrap()
            .transform
            .position
            .y;
        assert!(y < 10.0, "body fell under gravity and synced to ECS: y={y}");
    }

    /// Rigid bodies have no scale, so syncing a body back must keep the entity's
    /// authored scale (it used to reset it to 1, halving a scale-2 mesh).
    #[test]
    fn step_physics_preserves_authored_scale() {
        use stickshift_math::glam::Quat;
        use stickshift_math::Transform;
        let mut scene = Scene::new();
        let authored = Transform {
            position: Vec3::new(0.0, 5.0, 0.0),
            rotation: Quat::IDENTITY,
            scale: Vec3::splat(2.0),
        };
        let h = scene.physics_mut().create_dynamic_body(authored);
        scene.physics_mut().add_box_collider(h, Vec3::splat(1.0));
        let e = scene.world_mut().spawn((
            TransformComponent::new(authored),
            PhysicsBodyComponent::new(h),
            DynamicTag,
        ));

        scene.step_physics();

        let t = scene
            .world()
            .get::<&TransformComponent>(e)
            .unwrap()
            .transform;
        assert_eq!(t.scale, Vec3::splat(2.0), "authored scale kept");
        assert!(t.position.y < 5.0, "position still syncs from the body");
    }

    /// An entity despawned through `world_mut()` (not `Scene::despawn`) must not
    /// leave its body behind as a ghost collider.
    #[test]
    fn world_mut_despawn_frees_the_orphaned_body() {
        let mut scene = Scene::new();
        let e = spawn_body(&mut scene, Vec3::new(0.0, 5.0, 0.0));
        scene.step_physics();
        assert_eq!(scene.physics().rigid_body_set.len(), 1);

        scene.world_mut().despawn(e).unwrap();
        scene.step_physics();

        assert_eq!(
            scene.physics().rigid_body_set.len(),
            0,
            "orphaned body freed"
        );
        assert_eq!(scene.physics().collider_set.len(), 0, "and its collider");
    }

    /// The orphan sweep only frees a body nobody holds: if a live entity took the
    /// body over before its first owner was despawned, the body stays.
    #[test]
    fn orphan_sweep_keeps_a_body_another_entity_took_over() {
        let mut scene = Scene::new();
        let first = spawn_body(&mut scene, Vec3::ZERO);
        scene.step_physics();
        let handle = scene
            .world()
            .get::<&PhysicsBodyComponent>(first)
            .unwrap()
            .handle;

        let second = scene.world_mut().spawn((
            TransformComponent::from_position(Vec3::ZERO),
            PhysicsBodyComponent::new(handle),
        ));
        scene.world_mut().despawn(first).unwrap();
        scene.step_physics();

        assert_eq!(scene.physics().rigid_body_set.len(), 1, "body kept");
        assert!(scene.physics().get_transform(handle).is_some());
        assert!(scene.world().contains(second));
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

    /// Despawn atomicity: after a streamed entity crosses the despawn threshold,
    /// (a) the ECS entity is gone, (b) its physics body handle is stale, and
    /// (c) no live `PhysicsBodyComponent` still references that handle.
    #[test]
    fn despawn_removes_entity_and_physics_body_atomically() {
        let mut scene = Scene::new();
        let pos = Vec3::new(0.0, 0.0, 0.0);
        let e = spawn_body(&mut scene, pos);
        let handle = scene
            .world()
            .get::<&PhysicsBodyComponent>(e)
            .unwrap()
            .handle;

        assert!(scene.physics().get_transform(handle).is_some());

        assert!(scene.despawn(e));

        // (a) entity gone
        assert!(!scene.world().contains(e));
        // (b) physics handle stale
        assert!(scene.physics().get_transform(handle).is_none());
        // (c) no live component references it
        let alive = scene
            .world()
            .query::<&PhysicsBodyComponent>()
            .iter()
            .any(|(_e, b)| b.handle == handle);
        assert!(
            !alive,
            "no live PhysicsBodyComponent may hold a freed handle"
        );
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

    /// The panic keeps the message it has always had.
    #[test]
    #[should_panic(
        expected = "cannot change the streaming axis or spawn_interval once streaming has started"
    )]
    fn set_config_rejects_a_new_axis_once_streaming() {
        let mut scene = Scene::new();
        scene.stream(Vec3::ZERO, |_| {});
        scene.set_config(StreamingConfig {
            axis: Vec3::X,
            ..scene.config()
        });
    }

    /// `try_set_config` makes the same check as `set_config`, and refuses
    /// without changing anything instead of panicking.
    #[test]
    fn try_set_config_refuses_what_set_config_rejects() {
        let mut scene = Scene::new();
        let start = scene.config();

        // Before the first pass anything goes, the axis and spacing included.
        let respaced = StreamingConfig {
            axis: Vec3::X,
            spawn_interval: 2.0,
            ..start
        };
        assert_eq!(scene.try_set_config(respaced), Ok(()));
        assert_eq!(scene.config(), respaced);
        assert_eq!(scene.try_set_config(start), Ok(()));

        scene.stream(Vec3::ZERO, |_| {});
        let refused = Err(StreamingConfigError::AxisOrIntervalChanged);
        for changed in [
            StreamingConfig {
                axis: Vec3::X,
                ..start
            },
            StreamingConfig {
                spawn_interval: 3.0,
                ..start
            },
            // A NaN never equals itself, so it is a change, as `set_config`'s
            // comparison has always treated it.
            StreamingConfig {
                spawn_interval: f32::NAN,
                ..start
            },
        ] {
            assert_eq!(scene.try_set_config(changed), refused, "{changed:?}");
            assert_eq!(scene.config(), start, "nothing changed");
        }

        // The reach, the trim and the rebase distance may change mid-stream.
        let reach = StreamingConfig {
            spawn_ahead: 180.0,
            despawn_behind: 30.0,
            rebase_threshold: 500.0,
            ..start
        };
        assert_eq!(scene.try_set_config(reach), Ok(()));
        assert_eq!(scene.config(), reach);
        // The same config again is no change.
        assert_eq!(scene.try_set_config(reach), Ok(()));
    }

    #[test]
    fn the_error_says_what_was_refused() {
        let err = StreamingConfigError::AxisOrIntervalChanged;
        assert_eq!(
            err.to_string(),
            "cannot change the streaming axis or spawn_interval once streaming has started"
        );
        let _: &dyn std::error::Error = &err;
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
        let mut max_bodies = 0usize;

        // Advance the focus one interval per "frame" for many frames.
        for frame in 0..2000i64 {
            let focus = Vec3::new(0.0, 0.0, -(frame as f32) * cfg.spawn_interval);
            scene.stream(focus, |cx| {
                use stickshift_math::Transform;
                let h = cx
                    .physics
                    .create_dynamic_body(Transform::from_position(cx.position));
                let e = cx.world.spawn((
                    TransformComponent::from_position(cx.position),
                    PhysicsBodyComponent::new(h),
                ));
                cx.spawned(e);
            });
            max_entities = max_entities.max(scene.world().len() as usize);
            max_bodies = max_bodies.max(scene.physics().rigid_body_set.len());
        }

        assert!(
            max_entities <= bound,
            "entity count stayed bounded: max={max_entities}, bound={bound}"
        );
        assert!(
            max_bodies <= bound,
            "physics body count stayed bounded: max={max_bodies}, bound={bound}"
        );
        // And no leaked bodies: live bodies == tracked entities at the end.
        assert_eq!(scene.physics().rigid_body_set.len(), scene.streamed_count());
    }

    /// Origin rebase preserves relative positions of two entities across the shift,
    /// for both ECS transforms and physics body translations.
    #[test]
    fn rebase_preserves_relative_positions() {
        use stickshift_math::Transform;
        let cfg = StreamingConfig {
            axis: Vec3::new(0.0, 0.0, -1.0),
            spawn_interval: 10.0,
            spawn_ahead: 10.0,
            despawn_behind: 1e9, // never despawn during this test
            rebase_threshold: 1000.0,
        };
        let mut scene = Scene::with_config(cfg);

        // Two entities with physics bodies, far out along -Z.
        let a_pos = Vec3::new(1.0, 2.0, -1500.0);
        let b_pos = Vec3::new(4.0, 2.0, -1490.0);
        let ha = scene
            .physics_mut()
            .create_dynamic_body(Transform::from_position(a_pos));
        let hb = scene
            .physics_mut()
            .create_dynamic_body(Transform::from_position(b_pos));
        let a = scene.world_mut().spawn((
            TransformComponent::from_position(a_pos),
            PhysicsBodyComponent::new(ha),
        ));
        let b = scene.world_mut().spawn((
            TransformComponent::from_position(b_pos),
            PhysicsBodyComponent::new(hb),
        ));

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

        // Both moved back toward the origin (smaller |z|).
        assert!(a_after.z.abs() < a_pos.z.abs());

        // Physics body relative position preserved and matches ECS.
        let pa = scene.physics().get_transform(ha).unwrap().position;
        let pb = scene.physics().get_transform(hb).unwrap().position;
        assert!((pb - pa - rel_ecs_before).length() < 1e-3);
        assert!(
            (pa - a_after).length() < 1e-3,
            "physics body tracks ECS after rebase"
        );
    }

    /// A focus that wanders off the axis, as one that winds does, is pulled
    /// back across it too, whichever side it is on, by whole thresholds, and
    /// everything moves with it.
    #[test]
    fn rebase_pulls_a_focus_back_across_the_axis_too() {
        use stickshift_math::Transform;
        let mut scene = Scene::new();
        let threshold = scene.config().rebase_threshold;
        let pos = Vec3::new(2_600.0, 0.5, -300.0);
        let body = scene
            .physics_mut()
            .create_dynamic_body(Transform::from_position(pos));
        let entity = scene.world_mut().spawn((
            TransformComponent::from_position(pos),
            PhysicsBodyComponent::new(body),
        ));
        // Off to the right, a little way along: pulled back across only.
        let offset = scene.maybe_rebase(pos);
        assert_eq!(offset, Vec3::new(-2.0 * threshold, 0.0, 0.0));
        let after = scene
            .world()
            .get::<&TransformComponent>(entity)
            .unwrap()
            .transform
            .position;
        assert_eq!(after, pos + offset);
        assert!(after.x.abs() < threshold && after.z.abs() < threshold);
        let moved = scene.physics().get_transform(body).unwrap().position;
        assert!((moved - after).length() < 1e-3, "the body moved with it");
        // Off to the left and along: pulled back both ways at once.
        let far = Vec3::new(-1_500.0, 0.0, -3_200.0);
        assert_eq!(
            scene.maybe_rebase(far),
            Vec3::new(threshold, 0.0, 3.0 * threshold)
        );
        // Counted, and summed.
        assert_eq!(scene.rebase_count(), 2);
        let total = f64::from(threshold);
        assert_eq!(scene.rebase_total(), DVec3::new(-total, 0.0, 3.0 * total));
        // Within the threshold either way: no rebase.
        assert_eq!(
            scene.maybe_rebase(Vec3::new(999.0, 0.0, -999.0)),
            Vec3::ZERO
        );
    }

    /// A rebase across the axis leaves slot indexing alone: a focus heading
    /// diagonally, rebased across and along as it goes, fills every slot along
    /// the axis once, in order.
    #[test]
    fn rebasing_across_the_axis_leaves_the_slots_alone() {
        let cfg = StreamingConfig {
            axis: Vec3::new(0.0, 0.0, -1.0),
            spawn_interval: 10.0,
            spawn_ahead: 30.0,
            despawn_behind: 20.0,
            rebase_threshold: 100.0,
        };
        let mut scene = Scene::with_config(cfg);
        let mut filled: Vec<i64> = Vec::new();
        let mut focus = Vec3::ZERO;
        let (mut across, mut along) = (0, 0);
        for _ in 0..2_000 {
            // Heading 60 degrees off the axis.
            focus += Vec3::new(0.866, 0.0, -0.5);
            let offset = scene.maybe_rebase(focus);
            across += i32::from(offset.x != 0.0);
            along += i32::from(offset.z != 0.0);
            focus += offset;
            scene.stream(focus, |cx| {
                filled.push(cx.slot);
                let e = cx
                    .world
                    .spawn((TransformComponent::from_position(cx.position),));
                cx.spawned(e);
            });
        }
        assert!(across > 0 && along > 0, "rebased both ways");
        assert!(focus.x.abs() < 100.0 && focus.z.abs() < 100.0);
        let expected: Vec<i64> = (filled[0]..filled[0] + filled.len() as i64).collect();
        assert_eq!(filled, expected, "every slot once, in order");
        // 2000 steps at half a unit along the axis: about 1000 units of it.
        let last = *filled.last().unwrap();
        assert!((last * 10 - 1_000 - 30).abs() <= 10, "{last}");
    }

    /// No rebase fires while the focus stays within the threshold.
    #[test]
    fn no_rebase_within_threshold() {
        let mut scene = Scene::new();
        let offset = scene.maybe_rebase(Vec3::new(0.0, 0.0, -10.0));
        assert_eq!(offset, Vec3::ZERO);
    }

    /// A rebase moves every physics body, including one no entity holds (such
    /// as ground made through `physics_mut()`), so it stays where the rendered
    /// world is.
    #[test]
    fn rebase_shifts_a_body_no_entity_holds() {
        use stickshift_math::Transform;
        let mut scene = Scene::new();
        let pos = Vec3::new(0.0, -1.0, -1500.0);
        let ground = scene
            .physics_mut()
            .create_static_body(Transform::from_position(pos));
        let marker = scene
            .world_mut()
            .spawn((TransformComponent::from_position(pos),));

        let offset = scene.maybe_rebase(Vec3::new(0.0, 0.0, -1500.0));
        assert_ne!(offset, Vec3::ZERO, "a rebase should have occurred");

        let drawn = scene
            .world()
            .get::<&TransformComponent>(marker)
            .unwrap()
            .transform
            .position;
        let body = scene.physics().get_transform(ground).unwrap().position;
        assert_eq!(drawn, pos + offset);
        assert_eq!(body, pos + offset, "the body moved with the world");
    }

    /// A body two entities hold is moved once, not once per holder.
    #[test]
    fn rebase_shifts_a_shared_body_once() {
        use stickshift_math::Transform;
        let mut scene = Scene::new();
        let pos = Vec3::new(0.0, 0.0, -1500.0);
        let handle = scene
            .physics_mut()
            .create_dynamic_body(Transform::from_position(pos));
        for _ in 0..2 {
            scene.world_mut().spawn((
                TransformComponent::from_position(pos),
                PhysicsBodyComponent::new(handle),
            ));
        }

        let offset = scene.maybe_rebase(Vec3::new(0.0, 0.0, -1500.0));
        assert_ne!(offset, Vec3::ZERO, "a rebase should have occurred");
        let body = scene.physics().get_transform(handle).unwrap().position;
        assert_eq!(body, pos + offset, "shifted exactly once");
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

    /// Removing only the body component (not the entity) frees the body.
    #[test]
    fn removing_the_body_component_frees_the_body() {
        let mut scene = Scene::new();
        let e = spawn_body(&mut scene, Vec3::ZERO);
        scene.step_physics();
        assert_eq!(scene.physics().rigid_body_set.len(), 1);

        scene
            .world_mut()
            .remove_one::<PhysicsBodyComponent>(e)
            .unwrap();
        scene.step_physics();

        assert!(scene.world().contains(e), "the entity lives on");
        assert_eq!(scene.physics().rigid_body_set.len(), 0, "its body is freed");
        assert_eq!(scene.physics().collider_set.len(), 0, "and its collider");
    }

    /// Replacing an entity's body frees the old one and keeps the new one.
    #[test]
    fn replacing_the_body_component_frees_the_old_body() {
        use stickshift_math::Transform;
        let mut scene = Scene::new();
        let e = spawn_body(&mut scene, Vec3::ZERO);
        scene.step_physics();
        let new_body = scene
            .physics_mut()
            .create_dynamic_body(Transform::from_position(Vec3::ONE));
        scene
            .world_mut()
            .insert_one(e, PhysicsBodyComponent::new(new_body))
            .unwrap();
        scene.step_physics();

        assert_eq!(scene.physics().rigid_body_set.len(), 1);
        assert!(scene.physics().get_transform(new_body).is_some());
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

    /// A focus whose slot saturates at `i64::MAX` (far but finite, or
    /// infinite) spawns that last slot once and never overflows the frontier.
    #[test]
    fn a_saturated_focus_spawns_the_last_slot_once() {
        for far in [-1.0e30, f32::NEG_INFINITY] {
            let mut scene = Scene::with_config(unit_slots(3.0, 2.0));
            let mut slots = Vec::new();
            for _ in 0..3 {
                scene.stream(Vec3::new(0.0, 0.0, far), |cx| slots.push(cx.slot));
            }
            assert_eq!(slots, [i64::MAX], "{far}");
        }
    }
}
