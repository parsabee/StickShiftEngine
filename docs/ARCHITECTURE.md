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
touching ECS, physics, scene, or game code.

## 2. The render seam (blast-radius firewall)

`stickshift-render-api` defines only:
- Opaque resource handles (`MeshHandle`, `TextureHandle`, `PipelineHandle`, `EmitterHandle`).
- `RenderDevice` — resource create/destroy, called at load time.
- `FrameRecorder` — per-frame `set_view_projection` / `set_pipeline` / `bind` / `draw` / `submit`.
- `ParticleSystem` — particle emitters: create, place, `emit` bursts, at any time (§4d).

No crate above the seam may import `metal` types. This is enforced by crate boundaries so a
full renderer rewrite is provably contained.

**Camera crosses the seam as a matrix.** `FrameRecorder::set_view_projection(Mat4)`
carries the view — a plain `stickshift_math::glam::Mat4`, **no GPU type**. The engine owns a
`stickshift_camera::Camera`; the driver pushes its `view_projection_matrix()` through the seam once per
frame before `Game::render`, and the backend forms `mvp = view_proj * model` per draw. The value is
sticky (retained until replaced) so the engine's push survives the game's `begin_frame`. This
**replaced and deleted** the minimal camera that was temporarily inlined into `stickshift-render`
(`stickshift-render/src/camera.rs`); the real camera now lives in `stickshift-camera`, which also provides the
`ChaseController` follow camera. `stickshift-core → stickshift-camera → stickshift-math` keeps
the dependency direction acyclic and `stickshift-core` metal-free.

The concrete backend lives **below** this seam: `stickshift-render` provides
`MetalRenderer`, a raw-Metal type implementing both `RenderDevice` and `FrameRecorder` (and
therefore `stickshift-core`'s `Renderer` marker). `stickshift-render` is the *only* crate that depends on
`metal`. Crucially, `stickshift-core` does **not** depend on `stickshift-render`: the windowed entry
`stickshift-core::run_with_backend` takes a backend **factory**
(`FnOnce(&Window, u32, u32) -> Box<dyn Renderer>`), and the game binary (which does
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
depends on `stickshift-assets`) and no `metal` enters the tree. A graphics-only consumer turns the
`ecs` feature off. Imported geometry packs onto `[pos,normal,color]` with a default vertex color so it renders
through the existing pipeline immediately; parsed UVs are retained on `MeshAsset::uvs` for the
textured pipeline. A game loads every mesh it draws this way; the import fixtures are in
`crates/stickshift-assets/tests/fixtures`.

**Modern-look rendering stack.** Below the seam, `stickshift-render` applies a small,
mobile/TBDR-safe look stack — nothing above the seam learns about it. In pass order per frame:
(1) **sRGB + tonemap** — lighting is computed in linear space and presented through an explicit
ACES filmic tonemap + sRGB encode in the fragment shaders (`present_color`), so colors are correct
and not washed out even though both render targets are `*Unorm`. Colour textures are uploaded as
`RGBA8Unorm_sRGB` (`ColorSpace::Srgb`), so sampling decodes them to linear before lighting;
data textures such as the SDF font atlas stay `RGBA8Unorm` (`ColorSpace::Linear`); (2) a **gradient sky** fullscreen
triangle replaces the flat clear, drawn depth-test/write-disabled and graded by each pixel's view-ray
**elevation** (not its height on screen): it is the horizon colour at the horizon and below, deepening
to the zenith colour above, so far geometry fogged to the horizon colour meets it without a seam
whatever the camera's pitch; (3) **distance fog** blends far
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
a game's "summer 4pm" sun is a set of named constants in *its* config, pushed through
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
seam-driven sun's — one source of truth for shading, sun disc and shadows. Each fit **carries the last
frame's orientation over**, turned by the light's own turn, so a sun that moves (a day/night cycle) turns
the map without rolling it about the light's axis, which would re-rasterise every shadow edge each
frame; a sun that stands still keeps its map bit for bit. On a TBDR GPU the map is the
one depth attachment that must be **stored** (the MSAA scene targets are resolved in-tile and
discarded), which the backend asserts. Receivers are biased by their own triangle's plane plus a
constant 2 cm, and filtered with 4×4 tent PCF. The fitting and bias math is GPU-free and unit-tested
in `stickshift-render/src/shadow.rs`; a new shadow pixel-hash proves occlusion by re-rendering with the
casters disabled. A GPU-less runner never constructs the Metal backend, so it never reaches the pass.

**Point and spot lights, and emission.** Next to the one sun, a frame can add up to 256 point and
spot lights (`FrameRecorder::add_light`), and any draw can glow with `MaterialParams::emissive`.
Lights are shaded with clustered forward shading; §4c has the design and the budget.

**Atmosphere.** Weather is a game's policy too, so the seam carries only its physical half: an
`Atmosphere`, pushed with `FrameRecorder::set_atmosphere` and sticky like the sun. Its
`visibility` scales the distance fog on top of the draw-distance setting, folded in when the frame
captures its light (so mist and rain draw the fog in without touching the player's settings, and
the shadow slab follows the fog as ever). Its `Rainbow` and lightning `Bolt` are drawn by the sky
pass from a small uniform block of their own: a rainbow's two bows around a centre the game
places, laid over the sky in colours as bright as the sky behind them, and a bolt's jagged path
hashed from its seed. A storm's flash over the whole scene is the game's, through the `SunSky`'s
sky colours and fill. The default is clear air with nothing in the sky, and draws bit for bit
what was drawn before.

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
│   ├── stickshift-physics       # rapier3d wrapper (+ removal API)        (refactor)
│   ├── stickshift-camera        # camera + controller math                (refactor)
│   ├── stickshift-scene         # hecs World + PhysicsWorld + streaming   (refactor)
│   ├── stickshift-render-api    # RenderDevice/FrameRecorder traits       (new seam)
│   ├── stickshift-render        # raw-Metal impl of stickshift-render-api      (refactor)
│   ├── stickshift-core          # app/lifecycle + platform (#[cfg])       (refactor)
│   ├── stickshift-assets        # glTF + textures                         (new)
│   ├── stickshift-audio         # kira wrapper: sounds/one-shots/loops    (new)
│   └── stickshift-video         # movie writer (AVFoundation H.264)       (new)
├── docs/                   # architecture, design, guides, roadmap
└── scripts/                # preflight, doc-anchor check, GIF assembly
```

The Metal shader source lives in `crates/stickshift-render/shaders/`.

Dependency direction is acyclic: `stickshift-math` is a leaf; `stickshift-core` sits at the
top. Game concepts (car, road, score) never enter engine crates — the boundary is a
`Game` trait + `EngineCtx` seam, and self-scanning guard tests in `stickshift-core`,
`stickshift-scene`, `stickshift-assets`, `stickshift-audio` and `stickshift-ecs` fail if a game-named symbol appears in
them.

For standing up a game on it, see [GETTING_STARTED.md](GETTING_STARTED.md).

### 3a. Layers: the host and the loop

A game implements `stickshift_core::Game`; the engine runs it the same way under every host:

```
  Rust game (its own crate)
    impl stickshift_core::Game
            |
  stickshift_core::run (feature `windowed`)        a host that owns its own view and
    winit window and event loop,                    display link (an iOS app, say)
    the AppKit Graphics menu                                 |
    winit events -> HostEvent                                |
            \_____________________   _____________________/
                                  \ /
                    stickshift_core::session::Session
        the frame clock, the drawable size, settings restored and saved, menu choices,
        Escape, focus loss and resizes; what only the platform can do comes back as Requests
                                   |
               stickshift_core::driver::Loop and drive_frame
        Accumulator, InputState, Camera, Audio, GraphicsState, Scene (World + PhysicsWorld)
                                   |
                    Renderer (the stickshift-render-api seam)
                       MetalRenderer (stickshift-render)
```

- **One loop, one session.** The winit runner is an adapter: it maps winit's window events to
  `HostEvent`s and acts on the returned `Requests` (exit, full screen, the native menu). A host
  that owns its view does the same. Both drive `Session`, which runs `drive_frame`, so the frame
  cadence, the settings side effects and the clock are one implementation, not two that agree.
- **The host drives.** Nothing in the engine owns a thread, a window or a timer. The winit runner
  calls `Session::frame` on each redraw; another host calls it from its display link.
- **A device's screen on a Mac.** `RunConfig::device` makes the winit runner's window an
  iPhone's or an iPad mini's size in points (`stickshift_core::IPHONES`, `IPADS`), held either
  way, and report it to the session as that device would: a view whose scale is the window's
  pixels per device point (so the game lays out in the device's points at any zoom), and the
  device's safe area as a
  `HostEvent::SafeArea`. The session passes a safe area on to the renderer in drawable pixels
  (`RenderDevice::set_safe_area_insets`), whoever the host is, so a HUD reads the same insets on a
  phone and in a phone-sized window.
- **No windowing without the feature.** `stickshift-core` without `windowed` has neither winit nor
  the AppKit menu (a CI dependency firewall checks), and settings persistence is Foundation only,
  so it builds for iOS. `stickshift-render` still links AppKit through its `cocoa` dependency;
  gating that on macOS comes with iOS.

## 4. Engine stack

| Concern | Choice | Notes |
|---|---|---|
| Rendering | raw Metal (`metal`) | Apple-only; behind `stickshift-render-api` |
| Windowing | `winit` (macOS, in the Rust runner) | A host that owns its view and display link drives the same `Session` (§3a); iOS (v0.9) works that way |
| Math | `glam` | leaf crate |
| ECS | `hecs` | already integrated in the prototype |
| Physics | `rapier3d` | behind `stickshift-physics`, with a body/collider removal API |
| Audio | `kira` | behind `stickshift-audio`; see §4b |
| Assets | `gltf` + `image` | static meshes only for v1 |

### 4a. Fixed-timestep loop

`stickshift-core` drives simulation on a **fixed timestep decoupled from the display
rate**. `FIXED_DT = 1/60 s` is the single source of truth (`stickshift-core`'s
`timestep.rs`); physics (`stickshift-physics`) steps at the same rate so its behavior
is deterministic. Each display frame:

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

`stickshift-audio` wraps `kira` in a handful of operations — `load` a file into an opaque
`SoundHandle`, `play_once`, `play_looping`, `stop_looping`, the layer calls `play_layer` /
`set_layer_volume` / `set_layer_rate` / `stop_layer`, and `set_master_volume` — with levels in decibels (`Volume`). It is a **leaf**: its only dependency is `kira`, so `cpal`/CoreAudio reach the process
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

**Layers** are every other continuous sound: an ambient bed under the music, an engine's drone, a
second drone faded in over it while the throttle is open. `play_layer` starts one alongside the loop
channel and any other layers and returns a `LayerHandle`; `set_layer_volume` moves it to a level over
a fade of any length (a fade under way is taken over from where it has got to); `set_layer_rate`
glides its playback rate, and with it its pitch, the same way (an engine's note rising as it revs);
`stop_layer` fades it out and ends it. Layers are deliberately *not* idempotent the way the loop channel is: the caller
holds a handle per layer, which already says which is which. A handle names one start of its slot,
so a stopped layer's handle never reaches whatever is layered in that slot next.

**It never needs an audio device and never fails.** Creating a `kira` mixer fails with no audio
output (CI, a remote shell, `--smoke`); rather than push an `if audio_available` branch into every
game, the layer degrades to a **silent mode** that still accepts loads and plays and still hands back
valid handles — and that mode is constructible directly (`Audio::silent`), which is what `Loop::new`
holds. So the headless driver and `--smoke` open no device at all, and only the windowed entry binds
the mixer to the default output. Because the silent mode still *records* every request
(`is_silent`, `looping`, `loop_starts`, `one_shots_played`, `one_shots_of`, `decode_count`, and for
layers `layer_count`, `layer_starts`, `layer_volume`, `layer_sound`), audio
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
each light's sphere of influence (a point light's range around it; a spot's, the smallest sphere
around its cone, so a headlight's lies ahead of its lamp), projects the box, and lists the light in
every cluster the box overlaps; a lit fragment finds its cluster from its pixel and view depth and
shades only that list.
This fits a tile-based GPU: the scene stays one forward pass with MSAA resolved in tile memory, with
no G-buffer to write out and read back and no depth pre-pass. The binning is plain CPU arithmetic in
`stickshift-render/src/lights.rs`, conservative by construction (a light may be listed where it
contributes nothing, never left out where it contributes), and a property test checks that against
the shader's own cluster formula. A GPU test does the same end to end: every pixel the seam's
`Light::radiance_at` says is lit is lit, and every pixel out of reach is byte-identical to the frame
without lights.

**Budget.** 256 lights per frame (`MAX_LIGHTS`; the seam ignores more)
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

**Clear coat** is a per-draw strength (`MaterialParams::clearcoat`, `0` by default) of a glossy
lacquer over the surface, like a car's paint or glass. Both lit shaders mirror the frame's sky down
the reflected view ray (the same horizon-to-zenith gradient the sky pass draws, and below the horizon
a dim share of the horizon colour standing in for the ground), weighted by Schlick's Fresnel (4%
square on, all of it edge-on), and add a tight glint of the sun. The sky's reflection is not
shadowed, since the sky is still overhead in a shadow, so dark paint keeps its shape in the shade;
the glint is. It rides in the spare lane of the per-draw emissive (`w`), so it needs no extra
binding, and a draw without one skips it and shades exactly as before.

**Not yet:** shadows from local lights (a shadowed spot for headlights is the likely first), light
cookies, and bloom, which would make bright emitters bleed light onto their surroundings.

### 4d. Particles

Particle effects cross the seam as **emitters, never particles**. A game creates an emitter from
plain data (`stickshift_render_api::EmitterParams`: how particles leave it, how they move, how they
look over their lives), places it each frame with `ParticleSystem::place_emitter` (a transform and
a velocity), and fires bursts with `emit` or lets a continuous `rate` run. Every call is allowed at
any time, frame open or not, because effects are fired where they happen: in a fixed update. The
particles are drawn with every frame's scene without the game recording anything.

**Simulated on the GPU.** The particles live only in one `Private` ring buffer of `MAX_PARTICLES`
slots that the CPU never reads back. At `submit`, ahead of the shadow and scene passes, a compute
encoder runs `particle_update` over the particles already there (drag, gravity, the floor, and any
floating-origin shift) and `particle_spawn` over the frame's new ones. The scene pass then draws
the live slots as one instanced six-vertex quad each, per blend in use, after the geometry and
before the overlay: depth-tested against the scene but never writing depth, fogged like it, and
either camera-facing, stretched into a streak along the particle's motion, or (`ParticleFacing::Flat`)
lying flat in the horizontal plane, turned to the heading its emitter pointed when it was made,
which the spawn kernel stores with the particle: marks on the ground. Additive particles need
no ordering; alpha particles draw in spawn order, unsorted.

**Planned on the CPU.** What the CPU does is plain arithmetic in `stickshift-render/src/particles.rs`,
tested without a device: the emitter table (generational handles; a destroyed emitter's slot is
kept until its last particle dies, since the draw reads its params), turning bursts and rates into
spawn batches (a rate carries its fractional particle to the next frame, and its particles are
spread over the frame's time and along the emitter's path from its last placement, so a fast
emitter leaves a trail), and the **live window**. Particles are written into the ring in order, so
the live ones are always the newest; per batch the planner keeps the latest any of its particles can
live, and the kernels and the draw cover only the slots from the oldest batch still alive to the
newest. Work follows the particles alive, not the ring's size; past `MAX_PARTICLES` the newest
replace the oldest. Each particle's random numbers are hashed from its batch's seed and its index,
so the same calls give the same particles, and a GPU test checks two runs draw identical frames.

**Time.** `advance_particles(dt)` moves everything on. The engine loop calls it once per frame with
the fixed steps that frame ran (`steps * FIXED_DT`), right where it pushes the camera, so particles
move on simulated time: a stalled frame clamped to `MAX_STEPS_PER_FRAME` moves them only as far as
the game moved, and they keep falling while a game holds its own state still. A spawn batch records
how far into the frame its particles were born, and the spawn kernel moves each one on by the rest
of it. A game that rebases its floating origin passes the same offset to `shift_particles`.

**Budget.** 65,536 particles and 256 emitters (`MAX_PARTICLES`, `MAX_EMITTERS`), and 4,096 spawn
batches a frame (bursts past that are dropped and counted by `MetalRenderer::dropped_burst_count`).
The ring is 3 MB; the emitter table and spawn batches each have one region per in-flight frame,
written once per frame like the light ring, so particles add no per-frame allocation. On an M3 Max,
60,000 live particles add about 0.23 ms of GPU time to a 1080p frame ([BENCHMARKS.md](BENCHMARKS.md)).

**Not yet:** depth-sorted alpha particles, soft particles (fading where they meet geometry), and
textured or flipbook particles.

### 4e. Recording

A run can be **recorded to a movie** for diagnosis: `RunConfig::record` takes a `Recording` (a
path, a height, at most how many frames a second), and the session records every frame it shows
until the run ends. It crosses the render seam as **pixels, never GPU types**:

- Before a frame that is due (at most `fps` a second, give or take a quarter of a frame so a
  display's jitter does not halve the rate), the session asks the renderer to capture the next
  frame submitted: `FrameRecorder::capture_frame(width, height, time)`. The size is fixed at the
  first frame from the view's shape; the time is the session's own clock since the recording began.
- The Metal renderer, at that submit and after everything that drew the frame, scales the finished
  frame (HUD and all) into a capture texture and blits it to one of `MAX_FRAMES_IN_FLIGHT + 1`
  shared read-back buffers, flagged done by the frame's completion handler. No CPU wait: a request
  with every slot still holding an untaken frame is refused, and that frame skipped. A layer's
  drawables are display-only (`framebufferOnly`) until the first capture asks otherwise.
- After the frame, the session takes the captures the GPU has finished (`take_captured_frames`)
  and writes them to `stickshift-video`'s `VideoWriter`: AVFoundation's `AVAssetWriter`, the
  system's H.264 encoder, taking frames in real time. A frame the encoder is not ready for within
  a few milliseconds is dropped rather than waited for; frames carry their times, so a movie with
  dropped frames still plays at the game's pace.
- When the run ends, the windowed runner's `exiting` waits for the captures still on the GPU
  (`finish_captures`), writes them, finishes the movie (only a finished movie plays) and reports it
  on stderr.

`stickshift-video` is the only crate that binds AVFoundation, through the objc2 0.6 generation (the
first to bind CoreMedia and CoreVideo), which the audio stack already builds; `stickshift-core`
depends on it, with no windowing and no Metal, so a host that owns its view records the same way.

### 4f. Diagnostics capture

A **diagnostics capture** records a stretch of a session in enough detail to tell what went wrong
in it and when. The Graphics menu's last section starts and stops one (*Start Diagnostics
Capture*, ⌃⌘D) and marks moments in it (*Mark Moment*, ⌃⌘M, enabled only while one runs); the
`Session` owns it, so a host of its own drives it the same way (`start_diagnostics`,
`stop_diagnostics`, `mark_moment`, or the menu's commands as `HostEvent::Menu`).

Each capture is a folder under `DiagnosticsConfig::dir`, named for the game and the local time:

- `footage.mp4`, taken by a second `Recorder` exactly as §4e describes (none while a recording of
  the whole run is under way: the capture slot is the renderer's one, and that movie has the
  frames).
- `frames.csv`, a row for every frame shown, on the footage's clock, so a row and a moment in the
  movie line up. The engine's columns come from what already exists: the perf tracker's last
  frame (`PerfTracker::last_frame`: frame, CPU, GPU, physics and render times), the renderer's
  `RenderDevice::frame_stats` (draws, overlay quads, lights and those left out of the light budget,
  live meshes and textures, the device's allocated memory: counts the Metal backend keeps as it
  records a frame), the process's resident memory (Mach `task_info`), the scene's entity, body and
  streamed counts, and the camera's world position with the scene's rebase count and total
  (`Scene::rebase_count`, `rebase_total`), so a drift from the origin shows as it grows. The game's
  own columns follow (`Game::diagnostics`), named on the first frame.
- `events.csv`: the capture starting and stopping, marked moments, rebases, hitches (frames over
  `HITCH`, 50 ms), lights starting to be left out of the budget, settings changes, resizes and
  focus loss.
- `session.json`: the machine (`sysctl`: model, processor, memory, OS), the engine, the view and
  drawable, the graphics settings and the game's facts (`Game::diagnostics_info`), and, once the
  capture stops, a summary: frame-time percentiles and the worst frames, hitches, the furthest the
  camera got from the origin, peak memory, draws and lights, rebases and the marked moments.

While no capture runs, nothing is measured and neither hook is called. The capture writes through
buffered files on the main thread; a write that fails stops the capture and says why on stderr,
and the game runs on.

## 5. Physics decision

The engine keeps `rapier3d`, wrapped in `stickshift-physics`. The scene owns a `PhysicsWorld` next
to its hecs `World`, steps it once after every fixed update (`Scene::step_physics`), frees a body
together with its entity on despawn, and shifts every body on a floating-origin rebase. The wrapper
adds the **body/collider removal API** an endless world needs to despawn what falls behind, and it
hides rapier, so replacing it later would not ripple through the engine; no replacement is planned.
A game's own motion (a car's lane changes, say) can stay kinematic and game-owned next to it.

**Status:** the wrapper has migrated into `crates/stickshift-physics` and the removal
API has landed — `PhysicsWorld::remove_body` (removes the body and its attached colliders)
and `remove_collider`. Both uphold a **stale-handle-safety** invariant: after removal every
query on the freed handle returns `None` / is a no-op and never derefs freed storage
(rapier's generational handles + tests enforce this). The companion **handle-ownership**
invariant — a handle in a `PhysicsBodyComponent` is removed from physics atomically with
clearing the component — is documented here and in the crate; the despawn orchestration that
upholds it lives in `stickshift-scene` (below). `PhysicsWorld::step` advances at the crate-local `FIXED_DT`, which
mirrors `stickshift-core`'s `FIXED_DT` (`stickshift-physics` does **not** depend on `stickshift-core`, to
avoid a dependency cycle; a test pins the value).

**Status:** `crates/stickshift-scene` now owns the hecs `World` + `PhysicsWorld` and adds
engine-generic **streaming** (spawn-ahead / despawn-behind by focus distance) and **floating-origin
rebase**. Despawn is **atomic** — the physics body is removed before the ECS entity, upholding the
handle-ownership invariant above. Rebase shifts ECS transforms and physics bodies together, only
**between** physics steps (`stream → step_physics → maybe_rebase`), so relative positions are
preserved and the solver never sees a mid-step discontinuity. It pulls the focus back across the
travel axis as well as along it, a whole number of thresholds each way, so a focus that winds off
the axis stays near the origin too; only the part along the axis moves slot indexing. `stickshift-core` owns a `Scene` in its
loop and exposes it via `EngineCtx`; the dependency direction stays acyclic
(`stickshift-core → stickshift-scene → {stickshift-ecs, stickshift-physics, stickshift-math}`).

## 6. Provenance

Modules originate from the `ProjectRigor` prototype and are migrated here under the process
in [INTEGRATION.md](INTEGRATION.md). Provenance is recorded in docs only; no `ProjectRigor`
identifiers remain in migrated code.
