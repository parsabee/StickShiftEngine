# Stick Shift Engine Roadmap

Stick Shift Engine is a lean, **rasterization-only** Metal graphics engine for Apple Silicon, written in
Rust and usable from other languages through a C API. The host application owns the window or
view, the run loop and input; the engine renders.

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

## After v0.1

- **Physics leaves the engine.** The C API has never exposed physics, and the playable demo only
  uses it to tag obstacles. A later release removes `stickshift-physics` (and `rapier3d`),
  `PhysicsBodyComponent` and the loop's physics step. The scene keeps streaming and the
  floating-origin rebase, and passes the rebase offset to the game. A game that wants physics
  uses `rapier3d` directly.

## v0.2 — iOS

Release the library for iOS as well as macOS: iOS build targets, an XCFramework, a Swift sample
app, tile-memory (TBDR) tuning, compressed textures, precompiled shaders for the iOS SDKs and
on-device performance validation. Tracked in
[#2 iOS support (v0.2)](https://github.com/parsabee/StickShiftEngine/issues/2) and its sub-issues.

## Not planned

- **A scene editor.** Not part of this repository.
- **Ray tracing.** The engine is rasterization-only.
