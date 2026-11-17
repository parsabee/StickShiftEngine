# Stick Shift Engine Architecture

Status: authoritative. Decisions here are intentional constraints, not defaults.

> For a **visual** walkthrough — component, UML class, and sequence diagrams — see
> [DESIGN.md](DESIGN.md). This document is the *why*; DESIGN.md is the *how it fits and moves*.

## 1. Targets and the rendering choice

Stick Shift Engine targets **macOS and iPhone only** (macOS today; iOS is planned for v0.9, right before 1.0). On both, Metal is the single native GPU
API, so the engine uses **raw Metal** (`metal` crate) directly — this is *one* rendering
code path across both targets, not two, and it gives direct access to TBDR features
(memoryless attachments, tile memory) that portable abstractions expose late or not at all.

We deliberately give up portability to Windows/Linux/Android/Web. For an App Store product
this is the correct trade. If portability is ever required, the `stickshift-render-api` trait
seam (below) is the single insertion point for an additional backend (e.g. wgpu) without
touching ECS, scene, or game code.

## 2. The render seam (blast-radius firewall)

`stickshift-render-api` defines only:
- Opaque resource handles (`MeshHandle`, `TextureHandle`, `PipelineHandle`).
- `RenderDevice` — resource create/destroy, called at load time.
- `FrameRecorder` — per-frame `set_view_projection` / `set_pipeline` / `bind` / `draw` / `submit`.

No crate above the seam may import `metal` types. This is enforced by crate boundaries so a
full renderer rewrite is provably contained.

**Camera crosses the seam as a matrix.** `FrameRecorder::set_view_projection(Mat4)`
carries the view — a plain `stickshift_math::glam::Mat4`, **no GPU type**. The engine owns a
`stickshift_camera::Camera`; the driver pushes its `view_projection_matrix()` through the seam once per
frame before `Game::render`, and the backend forms `mvp = view_proj * model` per draw. The value is
sticky (retained until replaced) so the engine's push survives the game's `begin_frame`. This
**replaced and deleted** the minimal camera that was temporarily inlined into `stickshift-render`
(`stickshift-render/src/camera.rs`); the real camera now lives in `stickshift-camera`, which also provides the
`ChaseController` follow camera the `playable-demo` uses. `stickshift-core → stickshift-camera → stickshift-math` keeps
the dependency direction acyclic and `stickshift-core` metal-free.

The concrete backend lives **below** this seam: `stickshift-render` provides
`MetalRenderer`, a raw-Metal type implementing both `RenderDevice` and `FrameRecorder` (and
therefore `stickshift-core`'s `Renderer` marker). `stickshift-render` is the *only* crate that depends on
`metal`. Crucially, `stickshift-core` does **not** depend on `stickshift-render`: the windowed entry
`stickshift-core::run_with_backend` takes a backend **factory**
(`FnOnce(&Window, u32, u32) -> Box<dyn Renderer>`), and the game binary (`playable-demo`, which does
depend on `stickshift-render`) constructs the Metal backend and injects it. So `metal` reaches the
process only through `stickshift-render` and the game binary — never through `stickshift-core` — and the CI
`cargo tree -p stickshift-core` shows no `metal` (CI enforces the stricter rule on `stickshift-render-api`
itself). The migrated renderer is
guarded by an offscreen render pixel-hash (`stickshift-render/tests/pixel_hash.rs`).

**Flexible vertex layout through the seam.** The backend now builds its Metal vertex
descriptor **from the `VertexLayout` in `MeshData`** — mapping each `VertexAttribute`'s
`location`/`offset`/`format` (including `Float32x2` for UVs) and taking the buffer stride from
`layout.stride` — instead of a hardcoded `[pos,normal,color]` 0/12/24 `Float3` triple. `upload_mesh`
likewise deindexes at the layout's byte stride. For the `[pos,normal,color]` layout the resulting
descriptor and bytes are identical to the prior hardcoded path, so the pixel hash was unchanged by
that refactor. This lets arbitrary imported layouts flow through the one seam.

**Static glTF import (`stickshift-assets`).** The new **engine-generic, metal-free**
`stickshift-assets` crate imports static glTF (`.gltf`/`.glb`) into a `SceneAsset` (meshes + a baked node
tree) packed onto the render seam's `MeshData`/`VertexLayout`, with a load-once/dedup `AssetCache`
("upload once, reference by handle", one level up from the renderer's mesh registry) and a `spawn_scene` ECS helper. It
depends only on `gltf`, `stickshift-math`, `stickshift-render-api`, and (for `spawn_scene`, behind the default
`ecs` feature) `stickshift-ecs` — the `stickshift-assets → stickshift-ecs` edge stays acyclic (`stickshift-ecs` never
depends on `stickshift-assets`) and no `metal` enters the tree. The C API uses the crate without the
`ecs` feature, so its `spawn_scene` glue stays out of `libstickshift`; the library's ECS is the
engine's own world (§3a). Imported geometry packs onto `[pos,normal,color]` with a default vertex color so it renders
through the existing pipeline immediately; parsed UVs are retained on `MeshAsset::uvs` for the
textured pipeline. `playable-demo` loads every mesh it draws this way: the player and
traffic are real committed CC0 car models (`assets/*.glb`), and the road, skyline, and scenery come
from committed glTF too — see the [demo guide](PLAYABLE_DEMO.md). The earlier placeholder,
`assets/cube.gltf`, is retained only as a `stickshift-assets` import fixture.

**Modern-look rendering stack.** Below the seam, `stickshift-render` applies a small,
mobile/TBDR-safe look stack — nothing above the seam learns about it. In pass order per frame:
(1) **sRGB + tonemap** — lighting is computed in linear space and presented through an explicit
ACES filmic tonemap + sRGB encode in the fragment shaders (`present_color`), so colors are correct
and not washed out even though both render targets are `*Unorm`. Colour textures are uploaded as
`RGBA8Unorm_sRGB` (`ColorSpace::Srgb`), so sampling decodes them to linear before lighting;
data textures such as the SDF font atlas stay `RGBA8Unorm` (`ColorSpace::Linear`); (2) a **gradient sky** fullscreen
triangle replaces the flat clear, drawn depth-test/write-disabled; (3) **distance fog** blends far
geometry into the sky horizon color, hiding the streaming spawn edge (pairs with world streaming);
(4) **real sun shadows** (which replaced an earlier fake blob shadow) — see below. **MSAA**
(4x) wraps all of it: the scene renders into a multisampled color + depth attachment
and resolves **in-tile** into the single-sample target via the `MultisampleResolve` store action, so
the multisampled buffers never spill to system memory. The MSAA color/depth attachments are
**memoryless-ready**: their storage mode is a single cfg hook (`MSAA_MEMORYLESS`, iOS ⇒
`Memoryless`), which the iOS build will use; macOS uses `Private`. The offscreen pixel-hash path
still produces a readable, resolved single-sample texture. **Bloom is not implemented**
(it needs extra half-resolution targets and ping-pong passes). Because the look changed intentionally, both pixel-hash references were re-blessed.

**Drivable sun + sky.** The look stack was originally hardcoded: the light was written into a
Metal buffer once at renderer construction and never updated, so nothing above the seam could set
the sun. The seam now carries an engine-generic `SunSky` — sun **elevation and azimuth in degrees**,
colour, intensity, an ambient **sky-fill** level, and the sky gradient's zenith/horizon colours —
pushed with `FrameRecorder::set_sun_sky` and uploaded **per frame** (into a ring with one slot per
in-flight frame, so the per-frame path stays allocation-free). Azimuth is a compass bearing with
`-Z` as north and `+X` as east; the convention is documented and unit-tested in `stickshift-render-api`
rather than left implicit. The light *direction* is derived once from the angles and is the single
source of truth for both the shading and the new **sun disc + glow** in the sky pass, so turning the
sun moves them together. The camera's **world position** crosses the seam too
(`set_camera_position`), making specular view-dependent instead of using a constant view vector, and
the ambient term is rebalanced as sky fill (0.6 → 0.2 against diffuse 0.8, which previously left
unlit faces at 43% of lit and read overcast). **Time of day is not an engine concept**: the
playable demo's "summer 4pm" sun is a set of named constants in the *demo's* config, pushed through
the seam. The two 3D pixel-hash baselines were re-blessed; the screen-space overlay baseline is
unchanged, which is the proof the change stayed in the 3D passes.

**Real shadows.** The fake blob shadow is gone. Each frame now runs a **depth-only shadow
pass** from the sun into a 4096² `Depth32Float` map *before* the scene pass, and both lit pipelines
(untextured and textured) sample it. To know every caster before the scene pass starts, the backend
records draws during the frame and encodes both passes at `submit` — invisible above the seam, where
a game still just draws meshes and sets a `SunSky`. The light's orthographic projection is refitted
every frame to the slab of camera frustum the fog leaves visible (the draw distance, 165 units by
default; a backend-level shadow-distance override may shorten it), enclosed in a sphere and **snapped
to whole shadow texels** so it follows the camera without shimmering, and its direction is the
seam-driven sun's — one source of truth for shading, sun disc and shadows. On a TBDR GPU the map is the
one depth attachment that must be **stored** (the MSAA scene targets are resolved in-tile and
discarded), which the backend asserts. Receivers are biased by their own triangle's plane plus a
constant 2 cm, and filtered with 4×4 tent PCF. The fitting and bias math is GPU-free and unit-tested
in `stickshift-render/src/shadow.rs`; a new shadow pixel-hash proves occlusion by re-rendering with the
casters disabled. A GPU-less runner never constructs the Metal backend, so it never reaches the pass.

**Point and spot lights, and emission.** Next to the one sun, a frame can add up to 256 point and
spot lights (`FrameRecorder::add_light`), and any draw can glow with `MaterialParams::emissive`.
Lights are shaded with clustered forward shading; §4c has the design and the budget.

**Graphics settings.** Quality settings cross the seam as plain data, like `SunSky`:

- `RenderDevice::set_render_settings(&RenderSettings)` carries a `ShadowQuality` tier, a shadow
  range as a fraction of the draw distance, and a draw-distance scale on the backend's fog. All of
  them are relative to the backend's defaults.
- `RenderDevice::resize_surface` / `surface_scale` size the drawable for a render scale and report
  the HUD's UI scale.

The user-facing `GraphicsSettings` presets live in `stickshift-core`, which applies them to the seam and
to `Scene` streaming. Its windowed runner installs a native macOS **Graphics** menu by default,
opt-out via `RunConfig`. That menu is built with `objc2` in `stickshift-core::platform`, so AppKit stays
out of every crate but `stickshift-core`, and `metal` stays out of `stickshift-core`. The winit runner and
the menu are `stickshift-core`'s default `windowed` feature. What the runner adds to the loop (the frame
clock, the drawable size, saved settings, menu choices, Escape and resizes) is the platform-free
`session::Session`, which the runner adapts winit's events to. A host with its own view builds the crate
without the feature and drives a `Session` itself, with no `winit` and no AppKit; settings persistence
(`NSUserDefaults`) is Foundation only and stays.

## 3. Workspace layout

```
stickshift-engine/
├── crates/
│   ├── stickshift-math          # glam re-export + Transform              (reuse)
│   ├── stickshift-perf          # profiling / frame timing                (reuse)
│   ├── stickshift-ecs           # hecs wrapper + components               (reuse)
│   ├── stickshift-camera        # camera + controller math                (refactor)
│   ├── stickshift-scene         # hecs World + streaming + rebase         (refactor)
│   ├── stickshift-render-api    # RenderDevice/FrameRecorder traits       (new seam)
│   ├── stickshift-render        # raw-Metal impl of stickshift-render-api      (refactor)
│   ├── stickshift-core          # app/lifecycle + platform (#[cfg])       (refactor)
│   ├── stickshift-assets        # glTF + textures                         (new)
│   ├── stickshift-audio         # kira wrapper: sounds/one-shots/loops    (new)
│   └── stickshift-capi          # the C API: libstickshift + stickshift.h           (new)
├── bindings/
│   └── swift/StickShift    # Swift package over the C API (SwiftPM)
├── games/
│   └── playable-demo       # reference title; only uses engine public API
├── docs/                   # architecture, design, guides, roadmap
├── scripts/                # preflight, release packaging, doc-anchor check
└── packaging/homebrew/     # the Homebrew formula
```

The Metal shader source lives in `crates/stickshift-render/shaders/`.

Dependency direction is acyclic: `stickshift-math` is a leaf; `stickshift-core` and `games/*` sit at
the top. Game concepts (car, road, score) never enter engine crates — the boundary is a
`Game` trait + `EngineCtx` seam, and self-scanning guard tests in `stickshift-core`,
`stickshift-scene`, `stickshift-assets`, `stickshift-audio` and `stickshift-ecs` fail if a game-named symbol appears in
them.

For how `games/playable-demo` sits on that boundary in practice, see
[PLAYABLE_DEMO.md](PLAYABLE_DEMO.md); for standing up a new game on it, see
[GETTING_STARTED.md](GETTING_STARTED.md).

### 3a. Layers: one engine, two ways in

A game reaches the engine in one of two ways, and both run the same loop:

```
  Rust game (games/playable-demo)            C or Swift game (examples/c/engine.c, a Swift Game)
    impl stickshift_core::Game                 StickShiftGame callbacks  (Swift: import StickShift)
            |                                          |  stickshift.h: API 4, minor 1
            |                                  crates/stickshift-capi (libstickshift)
            |                                    StickShiftEngine: the CGame adapter (a Game),
            |                                    CapiRenderer (a Renderer onto the context),
            |                                    run-time scopes, world / scene / camera / input /
            |                                    audio / graphics / menu calls
  stickshift_core::run (feature `windowed`)            |
    winit window and event loop,               the host's view and display link:
    the AppKit Graphics menu                   stickshift_engine_tick, _push_events
    winit events -> HostEvent                          |
            \_____________________   _______________/
                                  \ /
                    stickshift_core::session::Session
        the frame clock, the drawable size, settings restored and saved, menu choices,
        Escape, focus loss and resizes; what only the platform can do comes back as Requests
                                   |
               stickshift_core::driver::Loop and drive_frame
        Accumulator, InputState, Camera, Audio, GraphicsState, Scene (the hecs World)
                                   |
                    Renderer (the stickshift-render-api seam)
         MetalRenderer (stickshift-render)    CapiRenderer -> StickShiftContext -> Metal or null
```

- **One loop, one session.** The winit runner is an adapter: it maps winit's window events to
  `HostEvent`s and acts on the returned `Requests` (exit, full screen, the native menu). A C or
  Swift host does the same through `stickshift_engine_push_events` and the request bits of each
  call. Both drive `Session`, which runs `drive_frame`, so the frame cadence, the settings side
  effects and the clock are one implementation, not two that agree.
- **The host drives.** Nothing in the engine owns a thread, a window or a timer. The winit runner
  calls `Session::frame` on each redraw; a C host calls `stickshift_engine_tick` from its display
  link, which runs the same `Session::frame` (with nothing to resize before a view is reported).
  `STICKSHIFT_CLOCK_ENGINE` reads `Instant::now()` where the runner reads it; tools tick with a
  fixed step, as the headless driver does.
- **Adapters, not reimplementations.** `CGame` is a `Game` whose hooks call the C callbacks, so
  `Loop::init_once` and `drive_frame` call a C game exactly where they call a Rust one.
  `CapiRenderer` is the `Renderer` the loop pushes the camera, render settings and drawable sizes
  through, onto the attached context; the game itself draws through the C frame the engine opens
  for its render hook.
- **Borrows become scopes.** During a tick the loop holds the engine's state mutably, so a C hook
  reaches it only through the `EngineCtx` the loop passed that hook, and a stream spawn callback
  only through the world its pass lends it. The C layer records the running hook on a scope stack
  and refuses, with `STICKSHIFT_ERROR_WRONG_STATE`, any call Rust's borrows would not allow there
  (`docs/C_API.md`, "Scopes and threads").
- **The ECS is the engine's own.** C games register components at run time through
  `stickshift-ecs`'s `dynamic` feature, which gives each one a Rust type of its own, so hecs
  partitions archetypes, orders queries and reuses ids exactly as for a Rust game's types. The
  engine components keep their Rust types. Queries copy rows into snapshots, so nothing borrowed
  from the world crosses the boundary.
- **No windowing in the library.** `libstickshift` builds `stickshift-core` without `windowed`, so
  neither winit nor the AppKit menu reaches it (a CI dependency firewall checks), and settings
  persistence is Foundation only. The library still links AppKit through `stickshift-render`'s
  `cocoa` dependency; gating that on macOS comes with iOS.

## 4. Engine stack

| Concern | Choice | Notes |
|---|---|---|
| Rendering | raw Metal (`metal`) | Apple-only; behind `stickshift-render-api` |
| Windowing | `winit` (macOS, in the Rust runner) | A C or Swift host owns its view and display link and drives the same `Session` (§3a); iOS (v0.9) works that way |
| Math | `glam` | leaf crate |
| ECS | `hecs` | already integrated in the prototype |
| Physics | none | removed in v0.2; a game brings its own (see §5) |
| Audio | `kira` | behind `stickshift-audio`; see §4b |
| Assets | `gltf` + `image` | static meshes only for v1 |

### 4a. Fixed-timestep loop

`stickshift-core` drives simulation on a **fixed timestep decoupled from the display
rate**. `FIXED_DT = 1/60 s` is the single source of truth (`stickshift-core`'s
`timestep.rs`); a game that runs its own physics steps it from `update` at that rate, so its
behavior is deterministic. Each display frame:

1. banks the real elapsed time in an `Accumulator`,
2. calls `Game::update(ctx, FIXED_DT)` a whole number of times (0..N), draining
   the accumulator, then
3. calls `Game::render(ctx)` **once**, carrying an interpolation `alpha`
   (`remainder / FIXED_DT`, `0..1`) so rendering can lerp between fixed states.

This makes the `update` count per second of simulated time framerate-independent
(identical at 60 and 120 Hz). A **spiral-of-death guard** caps catch-up at
`MAX_STEPS_PER_FRAME` steps per frame and discards the surplus, so a stall slows
the sim rather than wedging the loop. The headless driver (synthetic clock, one
step/frame — deterministic tests + `--smoke`) and the winit windowed driver (real
monotonic clock, variable steps) share **one** loop implementation
(`driver::drive_frame`); only the clock source differs.

### 4b. Audio

`stickshift-audio` wraps `kira` in five operations — `load` a file into an opaque `SoundHandle`,
`play_once`, `play_looping`, `stop_looping`, `set_master_volume` — with levels in decibels
(`Volume`). It is a **leaf**: its only dependency is `kira`, so `cpal`/CoreAudio reach the process
through this crate and its single engine-side consumer, `stickshift-core`. Nothing below the render seam,
and no other engine crate, gains an audio dependency. `kira`'s feature set is trimmed the same way
`gltf`/`image` are (`default-features = false`, only `cpal`, `cpal-realtime`, `wav`, `pcm` — the
demo's assets are 16-bit PCM WAV and this engine is Apple-only, so mp3/ogg/vorbis/flac/aac and
`cpal-realtime-dbus` are dead weight).

Sounds are decoded **once, at load**, keyed by path (the "upload once, reference by handle" discipline again), so no
play call touches the filesystem and nothing on the per-frame path allocates. Playback is driven by
game *events*, never polled from `update`. There is exactly **one loop channel**: re-asking for the
sound already looping starts nothing, which makes a doubled music bed structurally impossible from
above the seam rather than a rule each game has to remember.

**It never needs an audio device and never fails.** Creating a `kira` mixer fails with no audio
output (CI, a remote shell, `--smoke`); rather than push an `if audio_available` branch into every
game, the layer degrades to a **silent mode** that still accepts loads and plays and still hands back
valid handles — and that mode is constructible directly (`Audio::silent`), which is what `Loop::new`
holds. So the headless driver and `--smoke` open no device at all, and only the windowed entry binds
the mixer to the default output. Because the silent mode still *records* every request
(`is_silent`, `looping`, `loop_starts`, `one_shots_played`, `one_shots_of`, `decode_count`), audio
behaviour is asserted in plain unit tests with no hardware and nothing to listen to — the same
approach as `NullRenderer` for draws. Which sound is "the music" and which is an effect is game
policy: the engine takes a handle and a level.

### 4c. Point and spot lights

Night scenes need many small lights: headlights, streetlights, lit windows. The seam carries them as
plain data (`stickshift_render_api::Light`: a point or a spot cone, position, linear colour,
intensity, range), added to the open frame with `FrameRecorder::add_light`. Unlike the sun and the
camera they are **not sticky**: a frame starts with none and the game re-adds them from its own
state, so a light can never outlive the thing that casts it. Every light added before `submit` lights
every draw in the frame, whatever the call order. Brightness falls off with the inverse square of the
distance and is windowed to exactly zero at the range (`Light::falloff`), so a light has a hard edge
the renderer can cull against, and `intensity` sits on the same scale as the sun's.

**Clustered forward shading.** The renderer splits the view into a fixed grid of clusters, 16 × 9
screen tiles × 24 depth slices that grow exponentially with distance. At `submit` the CPU boxes
each light's sphere of influence, projects the box, and lists the light in every cluster the box
overlaps; a lit fragment finds its cluster from its pixel and view depth and shades only that list.
This fits a tile-based GPU: the scene stays one forward pass with MSAA resolved in tile memory, with
no G-buffer to write out and read back and no depth pre-pass. The binning is plain CPU arithmetic in
`stickshift-render/src/lights.rs`, conservative by construction (a light may be listed where it
contributes nothing, never left out where it contributes), and a property test checks that against
the shader's own cluster formula. A GPU test does the same end to end: every pixel the seam's
`Light::radiance_at` says is lit is lit, and every pixel out of reach is byte-identical to the frame
without lights.

**Budget.** 256 lights per frame (`MAX_LIGHTS`; the seam ignores more, and the C API refuses them)
and 32,768 light-to-cluster entries per frame across all lights. A street lamp covering a few tiles
and slices uses a few dozen entries, so a street's worth fits easily; large lights near the camera
fill the budget sooner, and a light that no longer fits is dropped whole (earlier lights keep
priority) and counted by `MetalRenderer::dropped_light_count`. On an M3 Max, 256 lights add about
0.1 ms of GPU time to a 1080p frame ([BENCHMARKS.md](BENCHMARKS.md)). Each in-flight frame owns a
region of a third ring next to the uniform and light rings, written once per frame, so lights add no
per-frame allocation.

**Emission** is a per-draw colour (`MaterialParams::emissive`) added after lighting and before fog.
It costs nothing per light and lights nothing else: a window glows, the wall beside it stays dark.
It travels in the per-draw uniform slot, which had room, so it needs no extra binding. glTF models
bring theirs from the material's `emissiveFactor` (and `KHR_materials_emissive_strength`).

**Not yet:** shadows from local lights (a shadowed spot for headlights is the likely first), light
cookies, and bloom, which would make bright emitters bleed light onto their surroundings.

## 5. Physics decision

The engine has **no physics**. v0.1 wrapped `rapier3d` in a `stickshift-physics` crate, and the
scene owned a `PhysicsWorld` next to its hecs `World`; v0.2 removed both. The demo never needed a
solver (its car moves kinematically and its collisions are box overlaps), the C API never exposed
one, and a racing game wants vehicle physics tuned for it rather than a general rigid-body world
the engine picks. A game that wants physics links a library itself (for example `rapier3d` from
Rust, or a Swift library over the C API), steps it from `Game::update` at `FIXED_DT`, and keeps
its bodies next to its entities however it likes.

**Scene.** `crates/stickshift-scene` owns the hecs `World` and adds engine-generic **streaming**
(spawn-ahead / despawn-behind by focus distance) and **floating-origin rebase**. Rebase shifts
every ECS transform by the same whole multiple of the threshold and returns that offset, so a game
that keeps positions outside the ECS (a camera, its own physics bodies) shifts them by the same
vector; done between physics steps, the solver never sees a mid-step discontinuity.
`stickshift-core` owns a `Scene` in its loop and exposes it via `EngineCtx`; the dependency
direction stays acyclic (`stickshift-core → stickshift-scene → {stickshift-ecs, stickshift-math}`).

## 6. Provenance

Modules originate from the `ProjectRigor` prototype and are migrated here under the process
in [INTEGRATION.md](INTEGRATION.md). Provenance is recorded in docs only; no `ProjectRigor`
identifiers remain in migrated code.
