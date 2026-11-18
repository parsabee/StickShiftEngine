# stickshift-capi

The Stick Shift Engine **C API**: `libstickshift.a` / `libstickshift.dylib` and the generated
[`include/stickshift.h`](include/stickshift.h), for hosts in C, Objective-C or C++.

The contract (concepts, threading, the engine and its scopes, what may wait in v0.1, how the
API grows, the reference) is [docs/C_API.md](../../docs/C_API.md). In short, a **context**
renders, and the API is **asynchronous in shape**: calls return at once with a handle or a
result code, resources go `PENDING` → `READY` / `FAILED`, completions arrive as events through
`stickshift_poll_events` or per-call callbacks on the host's thread, and `stickshift_frame_acquire`
never waits. An **engine** attached to a context runs the game loop for a game written as C
callbacks, with the engine's world, scene streaming, camera, input, audio and graphics settings,
while the host drives it with one `stickshift_engine_tick` per display refresh.
[`examples/c/engine.c`](../../examples/c/engine.c) is a small game on it.

## Layout

| File | |
|---|---|
| `src/types.rs` | Every C-visible type and constant |
| `src/core.rs` | The boundary: errors, the per-thread last error, panic containment, version |
| `src/context.rs` | `StickShiftContext`: the busy guard, events and callbacks, housekeeping |
| `src/engine.rs` | `StickShiftEngine`: the engine loop run for a host, its game hooks and their scopes |
| `src/engine/` | The engine's camera and chase, input, audio, graphics settings and menu |
| `src/world.rs`, `src/scene.rs` | Components, the world and query snapshots; streaming and the floating origin |
| `src/math.rs` | The math helpers: glam's own operations |
| `src/resources.rs`, `src/models.rs`, `src/fonts.rs` | Meshes and textures, glTF models (worker pool), SDF fonts |
| `src/frame.rs` | Recording and submitting frames, read-back |
| `src/backend.rs`, `src/metal.rs` | The backend trait, the null backend, and the Metal backend (macOS) |
| `src/handles.rs` | Generational handle tables (32-bit generations, never reissued) |

The crate depends on `stickshift-render` (macOS), `stickshift-render-api`, `stickshift-assets`
(without its `ecs` feature), `stickshift-math`, `stickshift-camera`, `stickshift-ecs` (with its
`dynamic` feature), `stickshift-scene` and `stickshift-core` without its `windowed` feature, so the
engine's loop, ECS (hecs) and audio (kira over cpal) are in the library, and the host owns the
window. CI fails if a windowing, AppKit-menu or physics crate (winit, objc2-app-kit, rapier)
reaches it.

## Building and testing

```sh
cargo build --release -p stickshift-capi     # target/release/libstickshift.{a,dylib}
cargo test -p stickshift-capi                # Rust tests through the exported functions
./scripts/test-c-api.sh                 # compile + run the C tests and samples
./scripts/gen-header.sh                 # regenerate include/stickshift.h (cbindgen 0.29.0)
./scripts/gen-header.sh --check         # CI: fail if the committed header is stale
```

The tests run anywhere: off macOS (or with `STICKSHIFT_BACKEND_NULL`) the null backend stands in
for Metal. On macOS one test also checks the C API renders the same pixels as the Rust API.
