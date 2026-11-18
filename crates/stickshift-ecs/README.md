# stickshift-ecs

The ECS component vocabulary for Stick Shift Engine — the stable surface that the
`Game` trait and any other engine consumer bind against.

## Responsibility

`stickshift-ecs` layers a small set of **engine-generic** components on top of the
[`hecs`] entity-component-system. Storage, entity IDs, spawning and querying all
come from `hecs` (re-exported as `stickshift_ecs::hecs`); this crate only defines the
component *types* the rest of the engine and its games agree on.

It builds on [`stickshift-math`] for spatial types (`Transform`, `glam`).

## The component model (binding surface)

This is the vocabulary a `Game` composes to describe a
scene. Everything here is deliberately **generic** — there are no game concepts
(no car, road, score, obstacle, or lane). Game meaning is built by *composing*
these primitives in the game crate, never by adding types here.

| Component | Kind | Purpose |
|---|---|---|
| `TransformComponent` | data | Wraps a `stickshift_math::Transform` (position / rotation / scale). Read by rendering, written by the game. |
| `RenderComponent` | data | Visual appearance: an RGB `color` plus a `RenderShape`. |
| `RenderShape` | enum (`#[non_exhaustive]`) | Geometry as a triangle `Mesh { vertices, indices }`. Mesh generators for cube and UV-sphere. |
| `StaticTag` | tag | Marks fixed geometry: its transform does not change. |
| `DynamicTag` | tag | Marks moving entities: the game changes their transform while they live. |

### Mesh-shape helpers

`RenderShape::cube(color)` and `RenderShape::sphere(color, segments, rings)`
generate flat/smooth triangle meshes; each vertex is nine floats
`[pos_xyz, normal_xyz, color_rgb]`. `RenderComponent::{cube, sphere, mesh}` are
the convenience constructors most callers use.

### Invariants

- **Static vs dynamic.** `StaticTag` and `DynamicTag` are mutually-exclusive
  intent markers: a static entity's transform is authored once and never changed;
  a dynamic entity's transform is updated by the game.
- **`RenderShape` is `#[non_exhaustive]`.** New built-in shape representations
  may be added later, so external `match`es must include a wildcard arm. This
  keeps the A1 boundary from silently hardening into an A2 break.

## Example

```rust
use stickshift_ecs::{TransformComponent, RenderComponent, DynamicTag};
use stickshift_ecs::hecs::World;
use stickshift_math::glam::Vec3;

let mut world = World::new();
let entity = world.spawn((
    TransformComponent::from_position(Vec3::new(0.0, 5.0, 0.0)),
    RenderComponent::cube([1.0, 0.0, 0.0]),
    DynamicTag,
));

for (_e, (t, r)) in world.query::<(&TransformComponent, &RenderComponent)>().iter() {
    let _ = (t.transform.position, &r.shape);
}
```

## Guarding the boundary

A unit test (`test_no_game_specific_symbols`) greps this crate's own source via
`include_str!` and fails if any game-specific identifier
(`car`/`road`/`score`/`obstacle`/`lane`) appears in a definition — the A1 public
surface stays engine-generic by construction.

[`hecs`]: https://docs.rs/hecs
[`stickshift-math`]: ../stickshift-math

Part of the [Stick Shift Engine](../../README.md) workspace. Apache-2.0.
