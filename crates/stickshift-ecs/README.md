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

## Dynamic components (`dynamic` feature)

`stickshift_ecs::dynamic` lets a host that cannot define Rust types (the C API,
and through it Swift) register components at run time and use them in an ordinary
hecs `World`. It is off by default; the crate's own tests turn it on.

- **Slot types.** Each registered component gets a Rust type of its own: `Tag<N>`
  (zero-sized) for a component with no value, `Blob<N>` (its bytes, inline up to
  32 and boxed above, at most 4096) for one with a value. Both families are
  macro-generated, 128 types each, and one registry holds at most 128 game
  components.
- **Engine components keep their types.** `Registry::register_native` gives a
  Rust component a byte form through a `Codec`, so Rust code that uses the type
  directly sees what a host wrote.
- **The same world as Rust.** A spawn is one `World::spawn` of an
  `EntityBuilder` bundle, which reaches the archetype a Rust tuple with the same
  set of types reaches. An insert is one `World::insert`, a remove is
  `World::remove_one` of the component's own type, and a query walks the
  archetypes and their rows in the order a typed query yields. Entities, id
  reuse and generations are hecs's own.
- **Queries copy.** `Registry::query` fills a `Rows` buffer (16-byte aligned, one
  row per entity, each value at an offset that honours its alignment), so the
  world may change while the rows are read.

A differential property test (`tests/dynamic.rs`) runs the same seeded script of
spawns, despawns, inserts, single removes and sets on a world of Rust types and on
a world of registered components, and checks that entity bits, `len`, every
archetype's component set and row order, every value and every query agree.

### What the slot types cost

Each slot type instantiates hecs generics (`World::remove_one` above all), about
5.6 KB of optimized code apiece. Measured on an M3 Max (16 cores, rustc
1.91.0), with a temporary probe in `stickshift-capi` that reaches every slot:

| Slots per family | `stickshift-ecs` release compile | `libstickshift.a` (release) | `libstickshift.dylib` (release) | clean release build of `stickshift-capi` |
|---|---|---|---|---|
| none (feature off) | 0.2 s | 30.9 MB | 2.6 MB | 16.2 s |
| 256 | 10.0 s | 38.3 MB (+7.3) | 5.5 MB (+3.0) | 20.7 s |
| **128 (used)** | 5.1 s | 34.8 MB (+3.8) | 4.1 MB (+1.5) | 16.0 s |

At 128 the crate compiles in parallel with the renderer, so a clean build takes no
longer. Debug builds carry the instantiations' debug info: the debug
`libstickshift.a` grows from 110 MB to 157 MB and the debug dylib from 8.4 MB to
20.1 MB.

## Guarding the boundary

A unit test (`test_no_game_specific_symbols`) greps this crate's own source
(`lib.rs` and `dynamic.rs`) via `include_str!` and fails if any game-specific
identifier (`car`/`road`/`score`/`obstacle`/`lane`) appears in a definition — the
A1 public surface stays engine-generic by construction.

[`hecs`]: https://docs.rs/hecs
[`stickshift-math`]: ../stickshift-math

Part of the [Stick Shift Engine](../../README.md) workspace. Apache-2.0.
