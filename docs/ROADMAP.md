# Stick Shift Engine Roadmap

Stick Shift Engine is a lean, **rasterization-only** Metal graphics engine for Apple Silicon, written in
Rust and usable from other languages through a C API (and, from v0.2, a Swift package). The host
application owns the window or view, the run loop and input; the engine renders.

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

## v0.2 — Offramp in Swift

The playable demo becomes **Offramp**, an open-source Swift app on macOS that drives the engine
only through the C API: the reference for building a native game on Stick Shift Engine.
[Milestone](https://github.com/parsabee/StickShiftEngine/milestone/1)

Engine:

- **A Swift package** wrapping the C API ([#76](https://github.com/parsabee/StickShiftEngine/issues/76)).
- **Many light sources:** point and spot lights and emissive materials ([#82](https://github.com/parsabee/StickShiftEngine/issues/82)), which the
  day/night cycle needs at night.
- **Physics leaves the engine** ([#81](https://github.com/parsabee/StickShiftEngine/issues/81)). The C API has never exposed physics, and the playable
  demo only uses it to tag obstacles. The release removes `stickshift-physics` (and `rapier3d`),
  `PhysicsBodyComponent` and the loop's physics step. The scene keeps streaming and the
  floating-origin rebase, and passes the rebase offset to the game. A game that wants physics
  uses `rapier3d` directly.

Offramp:

- **The port to Swift** ([#75](https://github.com/parsabee/StickShiftEngine/issues/75)), at parity with the Rust demo first.
- **Its presentation:** name, logo, font and menus ([#77](https://github.com/parsabee/StickShiftEngine/issues/77)).
- **A day/night cycle** ([#78](https://github.com/parsabee/StickShiftEngine/issues/78)), **curved roads** ([#79](https://github.com/parsabee/StickShiftEngine/issues/79)) and **car physics** ([#80](https://github.com/parsabee/StickShiftEngine/issues/80)),
  written in Swift as game code.

## v0.3 — Offramp grows

[Milestone](https://github.com/parsabee/StickShiftEngine/milestone/2)

- **GPU particles** in the engine and the C API, used for sparks, tyre smoke and weather
  ([#85](https://github.com/parsabee/StickShiftEngine/issues/85)).
- **Instanced drawing** for dense scenes ([#89](https://github.com/parsabee/StickShiftEngine/issues/89)).
- **Traffic AI** ([#86](https://github.com/parsabee/StickShiftEngine/issues/86)), **diverging roads** ([#87](https://github.com/parsabee/StickShiftEngine/issues/87)) and **procedurally generated city
  textures** ([#88](https://github.com/parsabee/StickShiftEngine/issues/88)).

## v0.9 — iOS and full Xcode, right before 1.0

Release the library for iOS as well as macOS: iOS build targets, an XCFramework, Offramp on iOS,
tile-memory (TBDR) tuning, compressed textures, precompiled shaders for the iOS SDKs, on-device
performance validation and App Store readiness, plus building and debugging from Xcode. Tracked
in [#2 iOS support](https://github.com/parsabee/StickShiftEngine/issues/2) and its sub-issues. [Milestone](https://github.com/parsabee/StickShiftEngine/milestone/3)

## v1.0 — the goal

A competent engine for **native Apple games and graphics apps**: Metal-native, embedded in Swift
and SwiftUI apps, fast on iPhone. That includes **AR** on iPhone and iPad (with ARKit) and
**immersive apps on Vision Pro** (with Compositor Services). Work toward it that is not yet
scheduled:

- **PBR materials and image-based lighting** ([#92](https://github.com/parsabee/StickShiftEngine/issues/92)).
- **Renderer groundwork for AR and Vision Pro:** external render targets, single-pass multi-view
  rendering, reverse-Z depth and foveation ([#91](https://github.com/parsabee/StickShiftEngine/issues/91)).

## Not planned

- **A scene editor.** Not part of this repository.
- **Ray tracing.** The engine is rasterization-only.
