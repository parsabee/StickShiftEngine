# stickshift-scene

Owns the world: a hecs `World`, with **streaming** (spawn-ahead / despawn-behind) and
**floating-origin rebase** for an endless world. This crate is **engine-generic** — it holds no
game types (no car/road/lane/obstacle/score); the game supplies the focus point and decides
*what* to spawn, while the scene owns the bookkeeping.

Part of the [Stick Shift Engine](../../README.md) workspace. Apache-2.0.

## Responsibility

- `Scene` — owns the `World`; `world()/world_mut()`, `despawn()`.
- `StreamingConfig` — 1-D streaming policy along an `axis`: `spawn_interval`, `spawn_ahead`,
  `despawn_behind`, `rebase_threshold`. `set_config` replaces it on a scene the game did not
  construct (the engine loop's), e.g. to set its own reach in `init`; the axis and spacing are
  fixed once streaming has started. Changing them then panics in `set_config`, while
  `try_set_config` returns `StreamingConfigError::AxisOrIntervalChanged` and changes nothing.
- `stream(focus, spawn)` — despawns tracked entities that fell more than `despawn_behind` behind
  the focus, then invokes the game's `spawn` callback once per unfilled slot up to
  `focus + spawn_ahead`. `SpawnCtx` gives the callback `world`/`position`/`slot` and a
  `spawned(entity)` hook to mark an entity for despawn-behind (untracked spawns are permanent).
- `maybe_rebase(focus) -> Vec3` — when the focus passes `rebase_threshold`, shifts every ECS
  transform back toward the origin by a whole multiple of the threshold, transparently to
  gameplay, and returns the applied offset.

## No physics

The scene has no physics. A game that wants rigid-body simulation runs its own (for example
`rapier3d`), steps it in its fixed update, and shifts it by the offset `maybe_rebase` returns,
between its steps rather than inside one.

## Invariants

- **Rebase preserves relative positions:** every transform shifts by the same vector. Anything the
  game keeps in world coordinates outside the ECS (a camera, its own physics) must apply the
  returned offset too.
- **Bounded memory:** with a fixed streaming window, the entity count stays bounded as the focus
  advances forever (asserted over 2000 simulated frames).
- **Deterministic:** the same focus trajectory spawns the same slots, despawns the same entities
  and rebases at the same crossings.

## Dependency direction

`stickshift-scene → { stickshift-ecs, stickshift-math }` — acyclic; it does **not** depend on
`stickshift-core`. `stickshift-core` owns a `Scene` inside its loop and exposes it via `EngineCtx`.
