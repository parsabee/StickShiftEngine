# Stick Shift Engine Roadmap

Stick Shift Engine is a lean, **rasterization-only** Metal game engine for Apple Silicon, written in
Rust. A game is a Rust crate that implements `Game`; the engine runs its loop, world, streaming,
physics, audio and rendering, and the host (the engine's winit runner, or an app that owns its
view) drives it one tick per display refresh.

Open work is tracked as [GitHub issues](https://github.com/parsabee/StickShiftEngine/issues). Release
history is in [CHANGELOG.md](../CHANGELOG.md).

## v0.1 — macOS library

### `v0.1.0-beta.1`

- Remove the in-repo ticket system and the old phase plan; track work in GitHub issues.
- Make the docs and API docs match what the code actually does.
- **C API:** build the engine as a C library (static and dynamic) with a generated `stickshift.h`
  header, a renderer created from a host-supplied `CAMetalLayer`, and errors instead of panics at
  the boundary. CI compiles and runs a C program against the header.
- A test and bug-fix pass over everything shipped so far.
- **Correct colour and normals:** colour textures decoded from sRGB, flat normals for glTF meshes
  that have none, the inverse-transpose normal matrix under non-uniform scale, and exact glTF
  node matrices.

The ray tracer was removed for v0.1: the engine is rasterization-only.

### `v0.1.0-beta.2`

- **The rename to Stick Shift Engine:** crates, the C API (API version 3), environment variables,
  the Homebrew formula and tap, the repository and the docs site.

### `v0.1.0`

Everything in the betas, plus:

- **Precompiled shaders:** release builds compile the Metal shaders to a `.metallib` at build time
  and embed it, so the shipped library never compiles shaders at runtime. Building that way needs
  full Xcode; development builds still compile at runtime.
- **Benchmarking:** GPU frame timing (renderer, perf snapshot and C API) and a benchmark scene,
  with results recorded in [BENCHMARKS.md](BENCHMARKS.md). They called for no fixes.
- **Every bug from the beta audit fixed** (#22–#43).

## v0.2 — many lights

### `v0.2.0`

A frame can carry many lights, and the engine is Rust crates only.
[Milestone](https://github.com/parsabee/StickShiftEngine/milestone/1)

Engine:

- **Many light sources:** point and spot lights and emissive materials ([#82](https://github.com/parsabee/StickShiftEngine/issues/82)), for scenes
  at night.
- **A host-driven loop:** `stickshift_core::Session`, the platform-free half of the windowed
  runner, so a host that owns its own view (an iOS app, say) drives the same frame clock, settings
  and menu as the winit runner. `stickshift-core` builds without winit and AppKit for iOS.
- **The C API and its release packaging are retired:** the engine is used from source, as Rust
  crates.

## v0.3 — particles and instancing

[Milestone](https://github.com/parsabee/StickShiftEngine/milestone/2)

- **GPU particles** in the engine, used for sparks, tyre smoke and weather
  ([#85](https://github.com/parsabee/StickShiftEngine/issues/85)).
- **Instanced drawing** for dense scenes ([#89](https://github.com/parsabee/StickShiftEngine/issues/89)).

## v0.9 — iOS and full Xcode, right before 1.0

Release the engine for iOS as well as macOS: iOS build targets, an iOS host for a game, tile-memory
(TBDR) tuning, compressed textures, precompiled shaders for the iOS SDKs, on-device performance
validation and App Store readiness, plus building and debugging from Xcode. Tracked
in [#2 iOS support](https://github.com/parsabee/StickShiftEngine/issues/2) and its sub-issues. [Milestone](https://github.com/parsabee/StickShiftEngine/milestone/3)

## v1.0 — the goal

A competent engine for **native Apple games and graphics apps**: Metal-native, embedded in native
apps, fast on iPhone. That includes **AR** on iPhone and iPad (with ARKit) and
**immersive apps on Vision Pro** (with Compositor Services). Work toward it that is not yet
scheduled:

- **PBR materials and image-based lighting** ([#92](https://github.com/parsabee/StickShiftEngine/issues/92)).
- **Renderer groundwork for AR and Vision Pro:** external render targets, single-pass multi-view
  rendering, reverse-Z depth and foveation ([#91](https://github.com/parsabee/StickShiftEngine/issues/91)).

## Not planned

- **A scene editor.** Not part of this repository.
- **Ray tracing.** The engine is rasterization-only.
