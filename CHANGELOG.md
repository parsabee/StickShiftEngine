# Changelog

All notable changes to Stick Shift Engine are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html). While the version is below
`1.0.0`, and especially for `-alpha` and `-beta` pre-releases, **any release may break the public
API**.

## [Unreleased]

### Added

- **Diagnostics capture**, for chasing down a glitch. The Graphics menu gains a last section:
  *Start Diagnostics Capture* (⌃⌘D, *Stop…* while one runs) and *Mark Moment* (⌃⌘M, enabled only
  while one runs). Each capture is a folder of its own (`diagnostics/<game>-<time>/` by default),
  holding:
  - `footage.mp4`, what the window showed, at 720 pixels high by default;
  - `frames.csv`, a row for every frame on the footage's clock: its CPU, GPU, physics and render
    times, updates, draws, lights and lights left out of the budget, live meshes and textures, GPU
    and process memory, entities and bodies, the camera's position and distance from the origin,
    the rebases and where the camera would be unrebased, then columns the game adds;
  - `events.csv`, the hitches, rebases, marked moments, settings changes, resizes and focus loss;
  - `session.json`, the machine, the settings, the game's facts and a summary (frame-time
    percentiles, the worst frames, peaks, the marks).

  `RunConfig::diagnostics` (a `DiagnosticsConfig`) says where captures go, how high the footage is,
  and whether one starts at launch. A game adds columns with `Game::diagnostics` and facts with
  `Game::diagnostics_info` (both default to nothing); a host of its own uses
  `Session::start_diagnostics`, `stop_diagnostics`, `mark_moment` and `capture_folder`.
- **`RenderDevice::frame_stats`** (default `None`): what the last frame drew and what the backend
  holds, as a `FrameStats`. The Metal backend and `NullRenderer` report it.
- **`PerfTracker::last_frame`**: the latest frame's own times (`FrameSample`), not averaged.
- **`Scene::rebase_count` and `Scene::rebase_total`**: how many floating-origin rebases there have
  been, and the sum of their offsets in `f64`.

- **Clear coat**, for car paint and glass: `MaterialParams::clearcoat` (`0.0` by default, up to
  `1.0` for a full coat). Both lit pipelines mirror the frame's sky down the reflected view ray (its
  horizon-to-zenith gradient, and the ground, darker, below the horizon), weighted by Fresnel: about
  4% seen square on, all of it edge-on. They also add a tight glint of the sun. Shadows dim the
  glint but not the sky's reflection, as the sky is still overhead in a shadow, so dark paint keeps
  its shape in the shade, where it used to go flat black. The coat rides in the spare lane of the
  per-draw uniform, and a draw without one shades exactly as before.

### Fixed

- **The floating-origin rebase keeps a winding focus near the origin.** `Scene::maybe_rebase` only
  ever shifted along the streaming axis, so a focus that wandered off it (Offramp's road winds up to
  60° either side of `-Z`) drifted further from the origin across it for as long as the session
  ran. In a long session the car was over 10,000 units out. That far out, the combined
  model-view-projection matrix each draw carries in `f32` loses enough to shift a whole object's
  depth by tenths of a unit at a typical viewing distance. Surfaces close together then drew
  through each other: the ground plane through the road, trees through buildings, cars through the
  deck. `maybe_rebase` now also shifts across the axis, by whole thresholds of each world
  component of the focus's offset from the axis's line. Only the part along the axis counts toward
  slot indexing, as before, so streaming is unchanged.
- **Headlights stop flickering in busy night scenes.** The Metal renderer bounded a spot light by
  the whole sphere of its range when binning it into clusters, though a spot lights only its cone. A
  car's headlight beam just ahead of the camera (the camera inside its range, though behind its
  lamp) counted as covering every screen tile from the camera out, so a few of them used up most of
  the frame's 32,768 light-to-cluster entries. Past that budget the renderer drops whole lights, and
  which ones fit changed from frame to frame, so lights blinked on and off. Offramp's two-way roads
  at night, with traffic both ways and more so in curves, dropped lights on up to one frame in
  seven. A spot is now bounded by the smallest sphere around its cone (`lights::bounding_sphere`):
  for a beam, a sphere ahead of the lamp. The same scenes use about 40% fewer entries, drop nothing,
  and shade fewer lights per pixel; a frame that dropped nothing renders exactly as before.

### Added

- **iPhone and iPad mini screens**, to see a game at a device's size and shape on a Mac.
  `stickshift_core::IPHONES` lists every iPhone from the iPhone 13 on (the 13 mini to the 2026
  iPhone 18 Pro, 18 Pro Max and Duo, closed and open), and `IPADS` every iPad mini from 2021 on (the
  6th generation, the A17 Pro and the 2026 A20 Pro); `DeviceScreen::all` gives them all, and
  `DeviceScreen::find` one by its id. Each `DeviceScreen` has an id for a command line
  (`iphone-15-pro`), its name, its year, its size in points upright, the scale it renders at, its
  panel's pixels, and its safe-area insets upright and on its side, as UIKit reports them.
  `RunConfig::device` takes a `DeviceWindow` (a screen and an `Orientation`), and the windowed
  runner opens a window that many points across instead of the default one (shrunk, its shape kept,
  to fit the main display), titled with the device and not resizable. The game sees the device's
  points at any zoom, and its safe area.
- **Safe areas reach the renderer**: `RenderDevice::set_safe_area_insets` sets what
  `safe_area_insets` reports (in drawable pixels), and the session passes its host's safe area
  (`HostEvent::SafeArea`, in points) to it whenever that or the drawable's scale changes. Before,
  the session kept the insets to itself and every backend reported zeros, so a HUD never saw a
  notch. The default `set_safe_area_insets` ignores them; `MetalRenderer` and `NullRenderer` keep
  them. `RunConfig` is no longer `Eq` (a device's insets are floats).

### Added

- **Recording to a movie**, for diagnosing what a game did: `RunConfig::record` takes a
  `Recording` (a path, a height such as 480 or 720, and at most how many frames a second, 60 by
  default), and the windowed runner records what the window shows, HUD and all, to an H.264 `.mp4`
  for the whole run, finishes it when the run ends and reports it on stderr. A host driving its own
  `Session` records with `Session::record` and stops with `Session::finish_recording`. The movie's
  width follows the view's shape at its first frame, rounded to even. Nothing waits on the GPU or
  the encoder while the game runs: a frame whose capture or encoding is not ready is skipped, and
  each frame carries the time it was shown, so the movie plays at the game's pace. No sound is
  recorded.
  - The render seam gains frame capture: `FrameRecorder::capture_frame(width, height, time)` asks
    for the next frame submitted to be scaled to that size and read back, and
    `take_captured_frames` / `finish_captures` return the `CapturedFrame`s (BGRA8) once the GPU has
    finished them. The defaults capture nothing; `NullRenderer` makes a frame of its frame number at
    each submit asked for. The Metal renderer scales the finished frame (four bilinear taps over
    each pixel's footprint) into a capture texture and copies it to one of a ring of read-back
    buffers, flagged by the frame's completion; a layer's drawables become readable from the first
    capture asked for.
  - A new crate, `stickshift-video`: `VideoWriter` writes BGRA8 frames, each stamped with its time,
    to an H.264 movie through AVFoundation's `AVAssetWriter` (the system's encoder, nothing bundled),
    dropping a frame rather than waiting long for the encoder. Off Apple platforms it reports
    `VideoError::Unsupported`.
  - `RunConfig` is no longer `Copy` (it holds the recording's path); it is still `Clone`.

### Added

- **Audio layers**: loops that play alongside the loop channel (the music) and each other, each
  faded on its own. `Audio::play_layer(sound, volume)` starts one and returns a `LayerHandle`;
  `set_layer_volume(layer, volume, fade)` moves it to a level over a fade (zero for at once), and
  `stop_layer(layer, fade)` fades it out and ends it. For an ambient bed under the music, an
  engine's drone, or a second drone faded in over it. Like the rest of `Audio`, layers work in
  silent mode and are recorded there (`layer_count`, `layer_starts`, `layer_volume`,
  `layer_sound`), so a game's use of them is testable without a device. A stopped layer's handle
  is ignored, even after its slot is reused.

### Added

- **Flat particles**, for marks on the ground: `EmitterParams::facing` takes
  `ParticleFacing::Flat` to lay each particle flat in the horizontal plane, a square of its size
  turned to the heading its emitter pointed when it was made, instead of facing the camera. An
  emitter placed just above the ground and moved along it lays a mark: tyre marks, puddles, scorch
  marks. `ParticleFacing::Camera` is the default, so existing emitters are unchanged; a struct
  literal of `EmitterParams` that names every field must now set `facing`.

### Added

- **GPU particles.** A third seam trait, `ParticleSystem`, gives a game particle effects without
  touching a particle: it creates emitters from plain-data `EmitterParams` (rate or bursts,
  lifetime, speed, direction and spread, spawn radius, inherited velocity, gravity, drag, an
  optional `Floor` to bounce off, size and colour over life, a stretch that draws streaks along the
  motion, additive or alpha blending), places them each frame with a velocity, and fires bursts with
  `emit`, at any time, frame open or not. The Metal renderer simulates the particles in two compute
  kernels and draws them as instanced quads in the scene pass, depth-tested against the scene and
  fogged like it, up to `MAX_PARTICLES` (65,536) alive and `MAX_EMITTERS` (256) emitters. The CPU
  cost is per emitter and per burst: on an M3 Max, 60,000 live particles add about 0.23 ms of GPU
  time to a 1080p frame and nothing measurable on the CPU (`docs/BENCHMARKS.md`). Continuous
  emission is spread over each frame's time and along the emitter's path, so a fast emitter leaves
  a trail rather than clumps, and the same calls give the same particles.
- **The engine loop advances particles** once per frame, by the fixed steps that frame ran
  (`advance_particles`), so they move on simulated time and keep moving while a game holds its own
  state still. A game that rebases its floating origin calls `shift_particles` with the same offset.
- **`NullRenderer` records particles**: emitters, every burst with where its emitter was placed,
  the time advanced and the shifts, so a game's effects are testable without a GPU. A destroyed or
  unknown emitter, invalid params, and emitters past the limit are protocol violations.
- **Particle scenarios in the renderer benchmark**: 10,000 and 60,000 additive particles, and
  60,000 alpha particles, over 1,000 cubes.

### Changed

- **`Renderer` (in `stickshift-core`) now also requires `ParticleSystem`.** A renderer of your own
  that wraps or replaces the Metal one must implement it; delegating every call to an inner
  renderer is enough. `MetalRenderer` and `NullRenderer` implement it.
- **The Metal renderer allocates three more buffers at construction** (the particle ring, and the
  per-frame emitter table and spawn batch rings: about 4.5 MB together), so
  `MetalRenderer::allocation_count` starts at 6, not 3. Nothing more is allocated per frame.

## [0.2.2] - 2026-11-18

### Changed

- **The sky grades by elevation, so the world meets it without a seam.** The Metal renderer's sky
  gradient followed each pixel's height on screen, so with a camera looking down (a chase camera
  over a road) the sky just above the horizon was already most of the way to its zenith colour,
  while distance fog fades far geometry to the horizon colour: where the world ran out, its fogged
  edge stood against the sky as a hard line. The sky now grades by each pixel's view-ray
  elevation: the horizon colour at the horizon and below it, deepening to the zenith colour over
  the first few degrees above it. A camera that sees only a little sky now sees mostly the horizon
  colour. All six golden images are re-blessed; only sky pixels changed.

## [0.2.1] - 2026-11-18

### Added

- **`MeshAsset::material_name`**: the name of each imported primitive's glTF material, so a game
  can pick out a model's parts by what they are (its windows, its lamps) and give them their own
  treatment, such as an emissive colour at night. `None` when the primitive has no material or the
  material is unnamed. It is a new public field, so code that builds a `MeshAsset` with a struct
  literal must now set it. The `gltf` dependency gains its `names` feature, which adds no crates.

### Fixed

- **The sun's highlights follow its intensity.** The Metal renderer scaled the sun's specular term
  by a constant rather than by `SunSky::sun_intensity`, so a dim sun (or a moon standing in for it)
  had highlights as bright as noon's, and a sun at zero intensity still put highlights on every face
  turned toward it. Moving such a sun made its highlights jump. Now a sun at zero lights nothing,
  so its direction can change while it is dark without anything on screen changing. The night
  street, shadow and textured-quad golden images are re-blessed: each change is darker highlights
  and nothing else.
- **Shadows no longer shimmer when the sun moves.** Each shadow-map fit chose a fresh up vector,
  so a change in the sun's bearing rolled the map about the light and every shadow edge
  rasterised on a turned grid each frame. Fits now carry the previous frame's orientation over,
  turned by the light's own turn (`ShadowFit` gains `direction` and `up`; `fit_shadow_frustum`
  takes the previous fit and `light_view` an up vector). A still sun's map is unchanged, bit for
  bit, so no golden image moved for this.

## [0.2.0] - 2026-11-17

### Added

- **Point and spot lights** (#82). A frame can add up to 256 lights next to its sun with
  `FrameRecorder::add_light(&Light)`. Lights last one frame: a game re-adds them every frame from
  its own state. Brightness falls off with the inverse square of the distance, windowed to zero at
  the light's range; a spot fades between an inner and an outer half angle. `Light::falloff`,
  `Light::cone` and `Light::radiance_at` compute the same curve on the CPU. The Metal renderer
  shades them with clustered forward shading: lights are binned on the CPU into 16 × 9 screen
  tiles × 24 depth slices, and each fragment shades only its cluster's lights. On an M3 Max, 256
  lights add about 0.1 ms of GPU time to a 1080p frame. ARCHITECTURE §4c has the design and the
  per-frame budget.
- **Emissive materials.** `MaterialParams::emissive` is light a surface gives off itself, added
  after lighting and before fog: lit windows and lamps glow at night without lighting anything
  around them. Imported glTF meshes carry their material's `emissiveFactor` (times
  `KHR_materials_emissive_strength`) as `MeshAsset::emissive`.
- The renderer benchmark has 64- and 256-light scenarios; `docs/BENCHMARKS.md` has the numbers.
- **`stickshift_core::Session`**, the platform-free half of the windowed runner: the frame clock
  (the first frame, and the first after a menu choice or `reset_clock`, banks exactly one step),
  the drawable sized for the view and the render scale, graphics settings restored at start-up
  and saved on every change, the Graphics menu's choices, Escape, focus loss and resizes. It
  takes the platform's events as `HostEvent`s and returns what only the platform can do as
  `Requests` (`EXIT`, `TOGGLE_FULLSCREEN`, `MENU_SYNC`). The winit runner is now an adapter over
  it, with unchanged behaviour, so a host that owns its own view drives the same code.
  `SettingsStore` keeps the settings in `NSUserDefaults` (the standard defaults or a named
  suite), on every Apple target, and `SettingsStore::clear` forgets them. The Graphics menu model,
  `stickshift_core::settings_menu` (`MenuCommand`, `submenus`, `ACTIONS`), is public for such a
  host to build its menu from.
- **`stickshift-core` builds without a platform.** Its new default feature, `windowed`, holds
  `winit` and the AppKit menu (`run`, `run_with_backend`, `run_with_config`). With
  `default-features = false` the crate has neither, and builds for `aarch64-apple-ios`.
- `Camera::fov_y`, `Camera::set_fov_y` and `Camera::aspect_ratio`.
- `Scene::try_set_config`, which returns `StreamingConfigError::AxisOrIntervalChanged` where
  `Scene::set_config` panics, and changes nothing.

### Changed

- `MaterialParams` gained the `emissive` field, so a struct literal must name it or end with
  `..MaterialParams::default()`. `FrameRecorder` gained the required `add_light` method, which a
  custom renderer must implement.
- The Homebrew formula (`brew install parsabee/stickshift/stickshift-engine`) installs only the
  engine source, from the release tag, for use as Cargo path dependencies.

### Fixed

- `Scene::stream` no longer overflows its spawn frontier (a debug panic, a release re-spawn every
  pass) once a far focus saturates the slot index at `i64::MAX`.
- `GraphicsSettings::from_codes` no longer overflows (a debug-build panic) on a stored code of
  `i64::MIN`; it decodes to the default like any other unknown code.

### Removed

- **The C API** (`stickshift-capi`: `libstickshift` and `stickshift.h`), its C examples and
  `docs/C_API.md`. The engine is Rust crates: a game implements `Game` and depends on them from
  source, as Cargo git dependencies pinned to a release tag.
- **The release tarball**, which carried the C library and the demo. A release is now a tag and
  its notes.
- **The playable demo** (`games/playable-demo` and the `stickshift-demo` binary) is no longer part
  of the engine; the engine's tests use its own fixtures.

## [0.1.0] - 2026-11-15

The first release: a Metal rendering engine for macOS on Apple Silicon, usable from Rust or, as
`libstickshift`, from C and Swift, with the playable demo as its reference game. Since 0.1.0-beta.2
it adds GPU frame timing and a renderer benchmark, and ships its release builds with precompiled
shaders. It also fixes every bug the beta audit filed (#22–#43), several of them in the C API's
contract. The C API keeps `STICKSHIFT_API_VERSION` 3: its additions (`STICKSHIFT_ERROR_TIMEOUT`,
`stickshift_context_gpu_frame_time`) don't change any existing type.

### Added

- **GPU frame timing.** The Metal backend records each frame's GPU execution time from its command
  buffer, in the completion handler that already counts finished frames, so it costs no
  allocation. The time is available in four places:
  - `MetalRenderer::gpu_frame_time(frame)`, for the last 64 completed frames;
  - the render seam's new `RenderDevice::last_gpu_frame_time`, a default method returning `None`
    for backends without a GPU;
  - `PerfSnapshot::avg_gpu_time`, which the `stickshift-core` loop fills each frame;
  - the C API's `stickshift_context_gpu_frame_time`.
- **A renderer benchmark** (`cargo run --release -p stickshift-render --example bench`). It renders
  fixed synthetic scenes offscreen at 1920 × 1080 with 4× MSAA: 100 to 5,000 draws, shadows on or
  off, and a million-triangle scene. It reports CPU and GPU frame times (mean and 95th
  percentile) and throughput. `docs/BENCHMARKS.md` records the results on an Apple M3 Max and what
  they show.
- **Release builds embed precompiled shaders.** With the `precompiled-shaders` feature, the
  renderer compiles its Metal shaders into a `.metallib` at build time, embeds it, and never
  compiles a shader at runtime. Building that way needs full Xcode. The feature existed but was
  untested, and nothing shipped with it. Now:
  - `stickshift-capi` and `playable-demo` forward it;
  - the release workflow builds the Homebrew tarball's demo and `libstickshift` with it, and
    checks that the shader source isn't in them;
  - a new CI job runs the renderer, C API and demo tests (pixel hashes included) and the C
    programs with it.

  Development builds still compile at runtime, so the Command Line Tools stay enough.

### Fixed

- **A glTF file with a cyclic node hierarchy no longer crashes the process**
  ([#22](https://github.com/parsabee/StickShiftEngine/issues/22)). The importer walked node
  children recursively with no cycle check, so a node listed as its own child overflowed the stack.
  A stack overflow aborts the process, which `catch_unwind` cannot stop, so a C API host loading an
  untrusted model was killed. The walk now uses an explicit stack, so deep trees are also safe. Any
  node reached twice (a cycle, a node with two parents, or a scene root that is also a child) fails
  with the new `ImportError::InvalidHierarchy`. In the C API that is `RESOURCE_FAILED` with
  `STICKSHIFT_ERROR_PARSE`.
- **The floating-origin rebase keeps physics and rendering together**
  ([#23](https://github.com/parsabee/StickShiftEngine/issues/23)). `Scene::maybe_rebase` shifted
  every `TransformComponent`, but only the physics bodies it found through a
  `PhysicsBodyComponent`. A body no entity held (for example ground made through `physics_mut()`)
  was never shifted, and a body two entities shared was shifted twice. Both ended up away from
  the rendered world. The rebase now shifts the whole physics world once with the new
  `PhysicsWorld::shift_origin`, which also moves colliders that have no parent body. Streaming slot
  positions are now computed in `f64`, so they stay exact on very long runs: in `f32`, a slot
  20,000 km from the start could land a unit off.
- **Polling from inside a callback no longer frees the outer poll's event messages**
  ([#28](https://github.com/parsabee/StickShiftEngine/issues/28)). `stickshift_poll_events` copied
  events out *before* delivering callbacks, despite documenting the reverse. A callback that polled
  again then freed the `message` strings the outer poll had just returned, so the host read freed
  memory. The poll now does housekeeping, delivers callbacks, and only then copies events, as
  documented. A poll made from a callback finishes before the outer poll hands anything out.
- **C API handles are tied to the context that issued them**
  ([#26](https://github.com/parsabee/StickShiftEngine/issues/26)). A handle id held only a kind, a
  slot index and a generation, so the first mesh of two contexts had the same id. Passing one
  context's handle to the other silently used the wrong object, although the header promised
  `STICKSHIFT_ERROR_INVALID_HANDLE`. Ids are now `kind (4) | context (12) | index (24) |
  generation (24)` bits, and every live context holds a different context tag, so another context's
  handle is rejected. This brings two new limits: at most 4,095 contexts can be alive at once
  (`stickshift_context_create` then returns `STICKSHIFT_ERROR_LIMIT_EXCEEDED`), and a slot retires
  after 16.7 million reuses instead of 4.3 billion. Handles stay opaque 64-bit values, so this is
  not an ABI change.
- **Destroying a pending model cancels its load**
  ([#27](https://github.com/parsabee/StickShiftEngine/issues/27)). `stickshift_model_destroy`
  documented that it cancels a pending load, but it only freed the handle. The worker still read,
  parsed and decoded the whole file before the result was thrown away. The only cancel flag was
  the one set when the context shut down. A host that changed its mind therefore paid the full IO
  and CPU cost, and its borrowed bytes stayed held until the parse finished. Each load now has its
  own cancel flag, which `destroy` sets. The parse checks it between stages: after the read, after
  the buffers load, and before each image decode and mesh primitive. Destroying a context now
  stops the parse in progress the same way, instead of letting it finish. The importer gains
  `import_gltf_cancellable`, `import_slice_cancellable` and `ImportError::Cancelled` for this.
- **A camera positioned in `Game::render` is used by that frame**
  ([#24](https://github.com/parsabee/StickShiftEngine/issues/24)). `EngineCtx::camera_mut`
  suggested moving the camera in `render`, but the loop pushed the camera across the render seam
  before `render` ran, so such a game drew every frame with the previous frame's camera.
  `EngineCtx::renderer` now pushes the camera again if it was moved since the last push, so draws
  recorded after the move use it. `NullRenderer`'s `RecordedDraw` gains `view_projection`, the
  camera each draw was recorded with.
- **`ChaseController` smoothing no longer depends on the frame rate (breaking)**
  ([#24](https://github.com/parsabee/StickShiftEngine/issues/24)). `follow` moved a fixed fraction
  of the way on every call, so following from `render` converged twice as fast at 120 Hz as at
  60 Hz. A small factor also trailed very loosely, while `0.0` snapped. Smoothing is now a
  half-life in seconds: `with_smoothing(fraction)` is replaced by `with_half_life(seconds)`, and
  `follow` takes the time step as a new last argument, `dt`. To keep an existing feel, a fraction
  `f` per 60 Hz step is a half-life of `ln 2 / (-60 ln(1 - f))` seconds (`0.2` is about
  `0.052`).
- **A camera set after `begin_frame` lights and shadows that frame**
  ([#25](https://github.com/parsabee/StickShiftEngine/issues/25)). The `FrameRecorder` protocol
  lists `set_view_projection` / `set_camera_position` after `begin_frame`, but the Metal backend
  captured the camera-dependent lighting inside `begin_frame`. A host following the documented
  order got shadows fitted to the previous frame's camera, and the sky and specular were a frame
  behind the geometry. On the very first frame the shadow fit came from the identity matrix. The
  backend now captures the frame's light block (camera, sun, shadow fit, and whether shadows are
  cast) at the frame's first draw, or at `submit` for a frame with no draws. A camera or sun set
  before that, before or after `begin_frame`, lights the frame. `stickshift-core` and the C API
  already set the camera before opening the frame, so they are unaffected.
- **`stickshift_context_flush` can no longer wait forever**
  ([#29](https://github.com/parsabee/StickShiftEngine/issues/29)). It looped until every model load,
  frame and read-back had finished, with no deadline, so a hung GPU or a load that never reported
  back hung the caller. It now gives up after 60 seconds with the new
  `STICKSHIFT_ERROR_TIMEOUT` (-11), and the last error message says what was still pending. Model
  loader jobs also turn a panic anywhere in their work into a failed load (`RESOURCE_FAILED` with
  `STICKSHIFT_ERROR_INTERNAL`), so a load always reports back.
- **An unused texture no longer refuses or drops a draw, and `draws_skipped` counts draw calls**
  ([#30](https://github.com/parsabee/StickShiftEngine/issues/30)). `stickshift_frame_draw_mesh`
  checked and tracked the material's texture even for a `POS_NORMAL_COLOR` mesh, which never samples
  it. A stale handle there returned `STICKSHIFT_ERROR_INVALID_HANDLE`, and destroying the texture
  before submit dropped the draw. The texture is now checked only for `POS_NORMAL_UV` meshes.
  `StickShiftStats.draws_skipped` also mixed units: a refused model draw counted 1, but a model
  dropped at submit counted once per part, and text once per glyph. It now counts draw calls
  everywhere, as the header documents.
- **The C example no longer writes uninitialised memory when its read-back fails**
  ([#31](https://github.com/parsabee/StickShiftEngine/issues/31)). `examples/c/offscreen.c` ignored
  the result of `stickshift_frame_read_pixels_async` and never checked the read-back's event, so a
  rejected or failed read-back still wrote the uninitialised buffer to the `.ppm` and printed
  success. It now checks the call's result, waits for the read-back's completion and writes the
  image only on `READBACK_COMPLETE`, reporting the event's message otherwise. The buffer is
  `calloc`ed, so a mistake shows up as black instead of noise.
- **FPS measures the frame rate, and physics and render time are recorded**
  ([#32](https://github.com/parsabee/StickShiftEngine/issues/32)). `PerfTracker` timed only
  `begin_frame` to `end_frame`, which the loop wraps around update and render, so its "FPS" was
  `1 / CPU work` and a light scene with vsync reported 1000+ FPS. Frame time is now the interval
  from one `end_frame` to the next, the real frame period, and FPS and the frame-time statistics
  come from it. The work inside a frame is the new `avg_cpu_time` (also on `PerfSnapshot`). The
  `stickshift-core` loop now records physics time around every fixed step and render time around
  `Game::render`; before, those fields were always zero. `mark_physics`, `mark_query` and
  `mark_render` add up within a frame instead of keeping only the last span.
- **Games can use Escape** ([#33](https://github.com/parsabee/StickShiftEngine/issues/33)). The
  windowed runner quit on every Escape press before the game saw the key, so a game couldn't use it
  for a pause menu or "press Escape again to quit". `RunConfig` gains `quit_on_escape` (default
  `true`, today's behaviour). With it off, Escape reaches the game like any other key, and the new
  `EngineCtx::request_exit` quits after the current frame; it also ends a headless run early. The
  close button and Cmd-Q always quit. `RunConfig` and `Loop` gain a public field, which breaks only
  code that builds them with a struct literal without `..Default::default()`.
- **A graphics change the game requests no longer renders one frame at the old size**
  ([#34](https://github.com/parsabee/StickShiftEngine/issues/34)). A change queued through
  `EngineCtx::set_graphics_settings` was applied at the start of a frame, but the windowed runner
  resized the drawable only after that frame, so a render-scale or full-screen change showed one
  stretched or cropped frame. The runner now applies the queued change and resizes for it before the
  frame, as the built-in menu already did.
- **The headless clock steps exactly one fixed step per frame**
  ([#35](https://github.com/parsabee/StickShiftEngine/issues/35)). `Headless::run` advanced its
  synthetic clock by `FIXED_DT` converted from `f32` (16,666,668 ns), while a fixed step consumes
  16,666,666 ns, so 2 ns were left over every frame: `alpha` crept up from zero, and after about
  8.3 million frames a frame ran two updates. The headless runner, and the windowed runner's first
  frame, now step by the new public `FIXED_DT_DURATION`, the exact step as a `Duration`.
- **`MetalRenderer::new` sizes its drawable in pixels**
  ([#36](https://github.com/parsabee/StickShiftEngine/issues/36)). The window constructor set the
  layer's `drawableSize` from the view's bounds, which are in points, and ignored its `width` /
  `height` arguments. On a 2x display a renderer built directly therefore rendered at half
  resolution until someone called `resize_surface`. (`stickshift-core` resizes straight away, so
  the demo was unaffected.) The drawable is now `width` × `height` physical pixels, falling back to
  the bounds times the window's backing scale factor for a zero dimension. The layer's
  `contentsScale` and `surface_scale` are set to that scale factor.
- **Shadows Off frees the shadow map** ([#37](https://github.com/parsabee/StickShiftEngine/issues/37)).
  `set_render_settings` replaced the shadow map only for a tier with a size, so switching to
  `ShadowQuality::Off` kept the previous map allocated: 64 MiB coming from High, 16 MiB from Low.
  Off, the obvious memory saver, saved nothing. Off now swaps in a 1×1 placeholder
  (`shadow::SHADOW_MAP_SIZE_OFF`), which keeps the shaders' binding valid, and a later tier
  re-creates a full map.
- **A completed frame has always returned its frames-in-flight permit**
  ([#38](https://github.com/parsabee/StickShiftEngine/issues/38)). The Metal completion handler
  bumped `completed_frame_count` and only then released the frame's permit. A thread that saw every
  frame completed could therefore still read `frames_in_flight() > 0`, which made the `async_frames`
  test flaky on a busy machine. The handler now returns the permit and flags the read-back first, and
  counts the frame last.
- **Scene streaming handles transform-less entities, removed body components and teleports**
  ([#39](https://github.com/parsabee/StickShiftEngine/issues/39)). Three edge cases in `Scene`:
  - A streamed entity without a `TransformComponent` (a marker or logic entity) was despawned on
    the next pass, as if it were already gone. It is now placed at the slot it was spawned for and
    despawned when that slot falls behind.
  - Removing only the `PhysicsBodyComponent` from a live entity leaked its rigid body for good,
    because the orphan sweep freed only bodies whose entity was gone. A body whose component was
    removed or replaced is now freed too, unless another entity holds it. The sweep's check no
    longer allocates on the common path.
  - A focus jump far past the spawn frontier (a teleport, or a respawn far ahead) spawned every slot
    in between in one call, and despawned them straight away. Filling now resumes from the focus,
    as on the first pass.
- **The asset cache parses and uploads a file once, whatever path names it**
  ([#40](https://github.com/parsabee/StickShiftEngine/issues/40)). `AssetCache` keyed entries by the
  path exactly as given, so `assets/model.glb`, `./assets/model.glb` and the file's absolute path
  were three entries, each parsed and uploaded to the GPU. Keys are now canonicalized, which also
  covers `..` and symlinks. A path that can't be canonicalized (a missing file) stays its own key,
  so the import error still names the path the caller gave.
- **Looping a broken sound keeps the music, and sound handles belong to their `Audio`**
  ([#41](https://github.com/parsabee/StickShiftEngine/issues/41)).
  - `Audio::play_looping` stopped the current loop before checking that the new sound could play.
    Asking for a sound whose file failed to decode left silence. It now checks first, and a sound
    that can't loop leaves the current one playing.
  - **(Breaking)** `SoundHandle(pub u32)` was a bare index: a handle from one `Audio` passed to
    another played whatever sound sat at that index, and callers could forge handles. A handle now
    carries the id of the `Audio` that issued it, which ignores anyone else's. Its fields are
    private, so handles come only from `Audio::load`.
- **The camera stays finite in degenerate poses**
  ([#42](https://github.com/parsabee/StickShiftEngine/issues/42)). `Camera::forward` and `right`,
  and through them the view matrix, were NaN in three cases: when the position equalled the target,
  when the view was parallel to `up` (looking straight down or up), and after
  `ChaseController::with_up(Vec3::ZERO)`. The NaN blanked the frame.
  - With no view direction the camera now looks along `-Z`.
  - With a zero or parallel `up`, the view uses the world axis least aligned with it.
  - `with_up` keeps `+Y` for an axis with no direction.
  - `set_clip_planes` ignores an invalid range in release builds too, instead of only
    `debug_assert`ing.

  Every other pose gives exactly the matrices it always did.
- **Doc comments match the code again** ([#43](https://github.com/parsabee/StickShiftEngine/issues/43)).
  - The physics crate no longer claims to sync transforms with the ECS, which is
    `Scene::step_physics`' job. Its claim that a test checks `FIXED_DT` against
    `stickshift_core::FIXED_DT` is now true: `stickshift-core` has the test.
  - `MeshAsset` documents both vertex layouts, untextured and textured.
  - `stickshift_frame_submit` says it can wait for a drawable on a layer context.
  - `stickshift-core` lists all its dependencies, describes the backend factory instead of an
    "isolated function the Metal backend fills", and lists everything `EngineCtx` exposes.
  - The audio mixer says which counters can differ between device and silent mode.

### Known limitations

- **macOS on Apple Silicon only.** iOS is planned for v0.2
  ([#2](https://github.com/parsabee/StickShiftEngine/issues/2)).
- **Some C API calls still do work on the caller's thread.** `stickshift_frame_submit` encodes the
  frame there and can briefly wait for a drawable; mesh, texture and font creation upload before
  returning; context creation, destruction and `stickshift_context_flush` wait. Later releases move
  this onto engine threads without API changes. See "What may wait in v0.1" in
  [docs/C_API.md](docs/C_API.md).
- **No ABI stability before 1.0.** `STICKSHIFT_API_VERSION` is 3; a context refuses a header from a
  different API version.
- **Draw calls are the main CPU cost.** There is no frustum culling or instancing yet, and indexed
  meshes are expanded to one vertex per index on upload. See [docs/BENCHMARKS.md](docs/BENCHMARKS.md).
- **Physics will leave the engine.** A later release removes `stickshift-physics` and the scene's
  physics step; games that want physics will use `rapier3d` directly (see
  [docs/ROADMAP.md](docs/ROADMAP.md)).
- **The crates are not published to crates.io.** Use them as path dependencies, or the C library
  from the release tarball or Homebrew.

## [0.1.0-beta.2] - 2026-11-14

The project takes the name **Stick Shift Engine**. Every public name changes (crates, the C API,
environment variables, Homebrew, the repository and the docs site); there are no functional
changes. The known limitations of 0.1.0-beta.1 still apply. This beta was not published as a
separate release; it shipped as part of 0.1.0.

### Changed

- **The project is renamed to Stick Shift Engine (breaking).** Every public name moves; nothing
  else changes:
  - **Crates:** `stickshift-*` (directories `crates/stickshift-*`; Rust paths `stickshift_core`,
    `stickshift_render`, …).
  - **C API:** `stickshift_*` functions, `StickShift*` types and `STICKSHIFT_*` constants. The
    library is `libstickshift` (`@rpath/libstickshift.dylib`), the header `stickshift.h`, the Swift
    module `StickShift` and the pkg-config file `stickshift.pc`. `STICKSHIFT_API_VERSION` is **3**,
    so a context refuses a header from before the rename.
  - **Environment variables:** `STICKSHIFT_*` (for example `STICKSHIFT_DEMO_ASSETS`,
    `STICKSHIFT_REQUIRE_GPU`).
  - **macOS settings:** the Graphics menu stores its choices under `StickShiftEngine.graphics.*`
    in `NSUserDefaults`, so saved Graphics settings reset once.
  - **Homebrew:** the formula `stickshift-engine` in the tap `parsabee/homebrew-stickshift`, the
    demo command `stickshift-demo`, and the release tarball
    `stickshift-engine-<version>-aarch64-apple-darwin.tar.gz`. The tap maps the previous formula to
    the new one, so `brew upgrade` moves existing installs.
  - **Repository and docs:** `github.com/parsabee/StickShiftEngine`, API docs at
    `parsabee.github.io/StickShiftEngine`.
  - **CI:** a new check (`scripts/check-renamed.sh`) fails if the previous name returns anywhere in
    the repository.

## [0.1.0-beta.1] - 2026-11-13

The first beta. Stick Shift Engine is now a rasterization-only Metal graphics library for Apple
Silicon with a **C API** that is asynchronous in shape: calls return at once with a handle or a
result, resources become ready later, and completions arrive as events. The beta also decodes colour
textures from sRGB, fixes normals under non-uniform scale and for meshes without them, makes the
docs match the code, and fixes the bugs and test gaps a full audit found. The C API and the Rust API
both have breaking changes; see Changed and Removed.

### Added

- **The C API.** A new `stickshift-capi` crate builds `libstickshift.a` and `libstickshift.dylib`
  with a cbindgen-generated `stickshift.h` and a Swift `module.modulemap`; the contract is
  [docs/C_API.md](docs/C_API.md). The API is **asynchronous in shape**:
  - calls return at once with a handle or a result code;
  - resources go `PENDING` → `READY` / `FAILED`;
  - completions arrive as events, through `stickshift_poll_events` or per-call callbacks that run on
    the host's thread;
  - glTF models are parsed on worker threads, and `stickshift_frame_acquire` never waits.

  Under the hood:
  - Handles are typed 64-bit ids with 32-bit generations and are never reissued.
  - A context is used by one thread at a time; an overlapping call returns `STICKSHIFT_ERROR_BUSY`.
  - No panic crosses the boundary.
  - The library is graphics-only: no windowing, audio, ECS or physics crate links into it, and
    CI checks that.
- **A null backend** for the C API (`STICKSHIFT_BACKEND_NULL`), so the API and host-side tests run
  without a GPU. It is the only backend off Apple platforms.
- **C programs in CI.** `crates/stickshift-capi/tests/c/smoke.c` and `examples/c/offscreen.c` are
  compiled with `-std=c11 -pedantic -Werror` against the header and run on the macOS runner's
  Metal device (`scripts/test-c-api.sh`). CI also fails if the committed header no longer
  matches the Rust sources (`scripts/gen-header.sh --check`).
- **The release ships the C library.** The tarball and the Homebrew formula now install
  `lib/libstickshift.a`, `lib/libstickshift.dylib` (install name `@rpath/libstickshift.dylib`),
  `include/stickshift/stickshift.h`, `include/stickshift/module.modulemap` and
  `lib/pkgconfig/stickshift.pc`. Packaging builds and runs the C programs against the unpacked
  tarball.
- **`stickshift-render`: fallible and non-blocking entry points**, alongside the existing ones:
  - `try_new`, `try_new_offscreen` and `try_from_layer` return a `RenderError` instead of
    panicking; `try_from_layer` takes a `CAMetalLayer` the host owns.
  - `try_create_mesh` and `try_create_texture` validate their input strictly.
  - `try_begin_frame` reports a `FrameSkip` instead of blocking.
  - `submit_async` never waits.
  - `request_readback` returns a `Readback` with a frame's own pixels.
  - `discard_frame`, frame completion counts, and `draw_mesh_matrix`.
- **`PipelineKind`** (`VertexColor`, `Textured`) in `stickshift-render-api`: the single source of
  truth for the two built-in pipelines and their vertex layouts.

### Changed

- **Imported nodes keep their exact world matrix (breaking, Rust).** `stickshift_assets::Node`
  replaces `transform: Transform` with `world: Mat4`, and `FrameRecorder` gains
  `draw_mesh_matrix(mesh, Mat4, material)` (moved up from the Metal backend) so a sheared matrix
  can be drawn through the seam. `NullRenderer`'s `RecordedDraw` records `model: Mat4` instead
  of `transform`. `spawn_scene` bakes a sheared node's matrix into the entity's vertices, since
  an ECS `TransformComponent` cannot hold shear. The per-draw `Uniforms` block grows from 128 to
  192 bytes (a normal matrix); it still fits one 256-byte ring slot.
- **Textures carry a colour space (breaking, Rust and C).** `TextureData` gains `color_space:
  ColorSpace` (`Srgb` or `Linear`), and the C API's `StickShiftTextureDesc` gains `color_space`
  (`STICKSHIFT_COLOR_SPACE_SRGB` = 0, `STICKSHIFT_COLOR_SPACE_LINEAR`), so `STICKSHIFT_API_VERSION`
  is now 2. sRGB textures upload as `RGBA8Unorm_sRGB` and are decoded to linear when sampled; linear
  ones are sampled as stored. glTF base-colour images (Rust `stickshift-assets` users and C API
  models alike) are sRGB. SDF font atlases must be linear: the C API refuses an sRGB atlas or
  `STICKSHIFT_FILL_SDF` texture with `STICKSHIFT_ERROR_INVALID_ARGUMENT`. HUD quads show the colours
  a texture stores in either colour space.
- **CI fails on unformatted code.** A new `cargo fmt --all -- --check` step runs first in the
  macOS CI job, and the whole workspace was reformatted with rustfmt's defaults (a
  formatting-only change). Run `cargo fmt --all` before pushing.
- **Pipelines and materials say what they do.** `PipelineDescriptor` is now
  `{ kind: PipelineKind }`; the shader-name strings it used to carry were ignored by the backend.
  `MaterialParams` keeps only `base_color`.
- **`stickshift-render` no longer waits on the CPU for texture mipmaps.** The mip blit runs on the
  same queue as every later frame. Frame opening, submission and the mip blit now run inside
  autorelease pools.
- **`stickshift-assets`' ECS spawn helper is behind a default `ecs` feature.** `stickshift-render`
  drops its unused `winit` dependency.
- **A new roadmap.** `docs/ROADMAP.md` now lists what v0.1 (beta.1, then 0.1.0) and v0.2 contain,
  replacing the phase/OKR plan. The native scene editor is no longer planned for this repository.
- **The docs match the code.** Docs and doc comments that described things the code does not do
  were corrected, among them:
  - the uniform ring that replaced per-draw uniform buffers, and the two built-in pipelines;
  - how `MaterialParams` is used: only `base_color`, and only by the textured pipeline;
  - that bloom, ASTC transcoding and an iOS path do not exist yet;
  - that CI's macOS runner runs the GPU pixel-hash tests, rather than skipping them;
  - workspace-tree entries (`shaders/`, `platform/`, Ray/AABB/Frustum in `stickshift-math`) that do
    not exist;
  - that there is no golden physics-trajectory test or coverage tooling yet.
- **`NullRenderer` accepts exactly the calls the Metal backend acts on.** A draw or HUD quad
  outside a frame, a destroyed or unknown handle, a mesh on the wrong pipeline, an empty mesh, a
  double destroy, `begin_frame` while open and `submit` without a frame are no longer recorded;
  each is listed in the new `protocol_violations()`. `live_*_count` counts live handles, so a
  double destroy no longer hides a leak.
- **GPU tests no longer pass silently without a GPU.** A skipped test prints a visible `SKIPPED`
  line, and with `STICKSHIFT_REQUIRE_GPU=1` (set in CI) a missing Metal device fails it. `BLESS`
  re-records the golden images only when set to `1` or `true`.
- **`ImportError` is `#[non_exhaustive]`** and gains `UnsupportedPrimitiveMode` and
  `InvalidPrimitive`.
- **`$STICKSHIFT_DEMO_ASSETS` is strict.** When set it is the only place the demo looks, and one
  that holds no assets is an error rather than a silent fall-through. Every candidate must hold
  `road.gltf`, so an unrelated `assets/` beside the binary is skipped.
- **`stickshift-core` builds warning-free off macOS.**

### Removed

- **The in-repo ticket system.** `tickets/`, `scripts/check-tickets.py` and the CI ticket-format
  lint are gone, and code comments no longer cite ticket IDs, phases or key results. Open work is
  tracked in GitHub issues, starting with
  [#2 iOS support (v0.2)](https://github.com/parsabee/StickShiftEngine/issues/2).

- **`MaterialParams::metallic` and `roughness`**, and `PipelineDescriptor`'s
  `vertex_shader` / `fragment_shader` / `vertex_layout` fields. No pipeline read any of them.

- **The `raytracer` feature.** Stick Shift Engine is rasterization-only. The off-by-default
  `stickshift-render` feature, its `raytracer` module (a CPU `trace_ray` and GPU triangle layouts
  that nothing called) and `shaders/raytracing.metal` (which no build ever compiled or loaded) are
  gone, along with the CI step that built them. Default builds are unchanged.

### Fixed

- **Normals were wrong under non-uniform scale.** The lit shaders transformed normals by the
  model matrix, which tilts them off their surfaces when an object is scaled differently per
  axis (a squashed sphere's highlights slid). They now use the inverse-transpose of the model's
  3×3; for rotation with uniform scale that is the model itself, so such draws (every pixel-hash
  scene) shade exactly as before.
- **Sheared glTF hierarchies were distorted.** Node world transforms were stored as
  position/rotation/scale, which drops the shear a rotated child gets under a non-uniformly
  scaled parent: `(1, 0, 0)` under a `(2, 1, 1)` parent and a 45° child landed at
  `(1.108, 1.061, 0)` instead of `(1.414, 0.707, 0)`. The full matrix is now kept, through the
  Rust API, the ECS spawn helper, the demo and the C API.
- **glTF meshes without normals were lit as if they faced up.** A primitive with no `NORMAL`
  attribute got `+Y` on every vertex, so walls and other non-horizontal faces were shaded like
  floors. The importer now computes flat face normals, as the glTF spec requires, giving each
  triangle of such a primitive its own vertices. Meshes that carry normals (every model the demo
  ships) are unchanged.
- **Colour textures were washed out.** Base-colour textures are sRGB-encoded, but they were
  sampled as linear and then sRGB-encoded again on output, so every texture rendered too light
  and desaturated. They are now decoded to linear before lighting. **Textured surfaces look
  different** (darker and richer; in the demo, the road and the skyline), and the textured pixel
  hash was re-recorded.
- **`create_texture` could read past the end of its pixel data.** It took any `rgba8` slice, even
  one shorter than `4 * width * height` bytes. Short data is now rejected: the seam method panics
  with a message, and `try_create_texture` returns an error.
- **Calling `begin_frame` twice without `submit`** leaked a frames-in-flight permit, and the
  third time deadlocked. A second call is now a no-op.
- **HUD quads recorded outside a frame** (before the first frame, after `submit`, or while a frame
  was skipped for want of a drawable) used to stack into the next frame. They are now ignored.
- **A mesh drawn with the other pipeline** made the GPU fetch vertices at the wrong stride, past
  the end of its buffer. Such a draw is now rejected and counted (`rejected_draw_count`).
- **The HUD vertex buffer was shared by all frames in flight,** so a frame could draw the next
  frame's HUD. Each in-flight frame now has its own buffer.
- **Physics reset every moving entity's scale to 1.** `Scene::step_physics` copied the whole
  body transform back, and rigid bodies have no scale; it now syncs only position and rotation.
- **Despawning through `Scene::world_mut()` left a ghost collider.** The entity's physics body
  stayed in the world and kept colliding. `step_physics` now frees the body of any entity the
  scene has seen that is gone (unless a live entity took the body over).
- **glTF triangle strips and fans were imported as triangle lists,** losing or inventing
  triangles. They now expand to proper lists; points and lines return
  `ImportError::UnsupportedPrimitiveMode`.
- **Malformed glTF could panic instead of returning an error:** float indices, a zero-count or
  VEC2 `POSITION` accessor, a stride shorter than one element, an accessor or image past the end
  of its buffer, an index past the last vertex. All are now `ImportError::InvalidPrimitive`.
- **A textured `.gltf` with `data:` images could not be loaded from memory** (`import_slice`,
  `stickshift_model_load_memory`). Embedded images now decode; external file references are still
  refused.
- **One bad mesh index shifted every later triangle.** The upload skipped just that index; it
  now drops the whole triangle it belongs to.
- **A stale mesh handle could alias a live mesh** after 4096 reuses of one registry slot. A slot
  is now retired before its generation wraps.
- **A settings change or resize between `begin_frame` and `submit` was half-applied** (the shadow
  pass skipped while the frame still sampled the map, or the HUD laid out for the new size). The
  frame now keeps what it opened with.
- **Keys stuck down after the window lost focus,** swallowing the next press, and **a tap
  shorter than one fixed step was lost.** Focus loss now releases everything, and press and
  release edges last until the next update sees them.
- **The cursor position was in physical pixels,** 2× the documented logical points on Retina.
- **The fixed-timestep loop dropped a legitimate sub-step remainder** whenever exactly
  `MAX_STEPS_PER_FRAME` steps fit, snapping the interpolation alpha to 0.
- **`Game::init` saw the drawable in points** with a UI scale of 1; it is now sized first.
- **`FontAtlas::measure` counted `\n` as an advance;** it now returns the widest line.
- **A music start that failed still claimed the loop channel,** so asking again did nothing.
- **The demo's replay message printed the best score labelled "score".**
- **Tests that could not fail.** `NullRenderer` now refuses the calls the Metal backend ignores
  (see Changed); demo tests that sat on the title screen now drive the game; render and ECS
  tests that only re-derived their own formula, drew nothing or asserted a tautology now check
  real output; and the HUD is part of the zero-allocation test.

### Known limitations

- **macOS on Apple Silicon only.** iOS is planned for v0.2
  ([#2](https://github.com/parsabee/StickShiftEngine/issues/2)).
- **Shaders still compile at runtime.** Precompiled, embedded `.metallib` shaders come in 0.1.0;
  until then the `precompiled-shaders` feature is off by default.
- **Some C API calls still do work on the caller's thread.** `stickshift_frame_submit` encodes the
  frame there and can briefly wait for a drawable; mesh, texture and font creation upload before
  returning; context creation, destruction and `stickshift_context_flush` wait. Later releases move
  this onto engine threads without API changes. See "What may wait in the beta" in
  [docs/C_API.md](docs/C_API.md).
- **No ABI stability before 1.0.** `STICKSHIFT_API_VERSION` is 2; a context refuses a header from a
  different API version.
- **Performance has not been measured yet.** GPU timing and benchmarks come in 0.1.0. Known costs
  today: no frustum culling or instancing, and indexed meshes are expanded to one vertex per index
  on upload.
- **The crates are not published to crates.io.** Use them as path dependencies, or the C library
  from the release tarball or Homebrew.

## [0.1.0-alpha.2] - 2026-09-27

### Added

- **Homebrew install (Apple Silicon).** `brew install parsabee/stickshift/stickshift-engine`
  installs a prebuilt `stickshift-demo` (the playable demo), its assets, and the engine source under
  `share/stickshift-engine/src` for use as Cargo path dependencies. The formula
  (`packaging/homebrew/stickshift-engine.rb`) requires arm64 macOS and refuses Intel Macs.
- **Release workflow.** `.github/workflows/release.yml` runs on every `v*` tag (or by hand for an
  existing tag): it builds `playable-demo` in release mode with the pinned toolchain, checks the
  binary is arm64 and passes `--smoke`, packages
  `stickshift-engine-<version>-aarch64-apple-darwin.tar.gz` plus a `.sha256` with
  `scripts/package-release.sh`, and attaches both to the GitHub release.
- **`stickshift-demo --print-assets-dir`** prints where the demo found its assets.

### Changed

- **The demo finds its assets at runtime** instead of through compile-time `CARGO_MANIFEST_DIR`
  paths, so an installed binary works without a source checkout. It tries, in order,
  `$STICKSHIFT_DEMO_ASSETS`, `<exe dir>/../share/stickshift-engine/assets` (Homebrew and the release
  tarball), `<exe dir>/assets`, then the crate's own `assets/` (so `cargo run` is unchanged). If
  none exists it prints what it tried and exits 1.

## [0.1.0-alpha.1] - 2026-09-27

The first public pre-release: a source release of the engine workspace and its reference game,
the playable demo. macOS on Apple Silicon is the supported platform. All crates share the
workspace version and are not published to crates.io.

### Added

- **Workspace and CI (Phase 0).** A Cargo workspace of focused `stickshift-*` crates plus
  `games/playable-demo`, built with Rust 1.91 (pinned in `rust-toolchain.toml`; MSRV enforced
  through `rust-version`). A macOS CI oracle builds, tests, lints (clippy `-D warnings`), checks
  rustdoc, lints the ticket format and runs the headless smoke oracle. `scripts/preflight.sh`
  checks the required toolchain. The engine was migrated from an earlier prototype, with its
  identifiers removed from shipped code.
- **Engine/game boundary and loop (Phases 1-2).** The `Game` trait and `EngineCtx` in
  `stickshift-core`; a fixed-timestep loop that drives both the windowed (winit) app and a headless
  driver, with `Game::update` on a fixed cadence.
- **Render seam and raw-Metal renderer (Phase 1).** `stickshift-render-api` defines the
  backend-agnostic `RenderDevice` / `FrameRecorder` traits, opaque handles and a `NullRenderer`
  test double; it never depends on `metal`, and CI enforces that. `stickshift-render` implements the
  seam in raw Metal, with persistent mesh buffers behind a generational handle registry, a
  uniform ring (no per-frame allocations), triple-buffered frames in flight, offscreen rendering
  with pixel read-back, and an off-by-default `raytracer` feature.
- **ECS, physics and streaming (Phase 2).** `stickshift-ecs` (a `hecs` wrapper and shared
  components), `stickshift-physics` (a `rapier3d` wrapper with a body/collider removal API),
  `stickshift-scene` (world streaming with spawn-ahead / despawn-behind and a floating-origin
  rebase) and `stickshift-camera` (perspective camera and chase controller).
- **Look and content (Phase 4).**
  - Static glTF (`.gltf` / `.glb`) import and an asset cache in `stickshift-assets`, with a flexible
    vertex layout.
  - Base-color textures with mipmaps (RGBA8; ASTC is behind an off-by-default feature).
  - A modern look stack: MSAA, a gradient sky with a drivable sun disc, specular lighting,
    ground-hugging horizon fog and ACES tonemapping.
  - Real fitted shadow maps with PCF filtering (4096² at the High setting). The shadow maps
    follow the draw distance.
  - A 2D HUD overlay seam with SDF text.
  - Audio in `stickshift-audio` over `kira`: load-once sounds, one-shots, looping music and master
    volume.
  - `stickshift-perf` frame-timing instrumentation with an injectable clock.
- **The playable demo (Phase 7).** `games/playable-demo`, an endless three-lane freeway runner
  and the reference example for building a game on the engine's public API. It has discrete lane
  control, title, game-over and replay states, and a score. Content: CC0 Quaternius car models,
  CC0 Kenney City Kit buildings, a textured asphalt road with yellow solid edge lines, a city
  skyline backdrop, guardrails and hill terrain. The draw distance and fog reach are three times
  the original. Also a HUD that scales with the render resolution, driving music and a crash
  sound effect.
- **Graphics settings and a native macOS Graphics menu (KE-0408).**
  - The windowed runner adds a **Graphics** menu to the macOS menu bar by default. It offers:
    - Shadows: Off / Low / High.
    - Shadow Distance: Match Draw Distance / Medium / Near.
    - Draw Distance: Near / Medium / Far.
    - Resolution: 50% / 75% / 100% of native pixels.
    - Enter/Exit Full Screen (⌃⌘F) and Reset to Defaults.
  - Choices persist across launches in `NSUserDefaults`.
  - Games can turn the menu off with `RunConfig { native_settings_menu: false, .. }` and drive the
    same public `GraphicsSettings` model from their own UI, via
    `EngineCtx::graphics_settings` / `set_graphics_settings` and the
    `Game::graphics_settings_changed` hook.
  - The render seam gains `RenderSettings`, `set_render_settings` and `resize_surface`.
- **Documentation.** Every engine crate compiles under `#![deny(missing_docs)]`, and the rustdoc
  API reference is published to <https://parsabee.github.io/StickShiftEngine/>. Guides:
  `docs/GETTING_STARTED.md` (build your own game), `docs/PLAYABLE_DEMO.md` (the demo as a
  reference example), `docs/DESIGN.md` (Mermaid diagrams), `docs/ARCHITECTURE.md`,
  `docs/ROADMAP.md` and `docs/INTEGRATION.md`.
- **Testing.** 316 tests: unit tests throughout; golden pixel-hash tests that pin the renderer's
  output (skipped when no Metal device is available); and a headless smoke oracle
  (`cargo run -p playable-demo -- --smoke`) that runs 120 frames with no GPU.

### Known limitations

- **macOS only.** iOS bring-up (Phase 3) has not started. It needs full Xcode and the
  `aarch64-apple-ios` rustup targets.
- **Shaders compile at runtime.** Precompiled `.metallib` shaders (KE-0107) are blocked on full
  Xcode, because the Command Line Tools do not ship the `metal` / `metallib` compiler.
- **Pre-release API.** The public API is unstable and may change in any release. The crates are
  not published to crates.io.
- **Source release only.** No prebuilt binaries. The demo resolves its assets through
  compile-time `CARGO_MANIFEST_DIR` paths, so it must be built and run from a clone of the
  repository (`cargo run -p playable-demo`).
- **Phases 5 and 6 have not started.** Phase 5 was re-scoped from the planned scripting
  layer to a native macOS scene editor for Apple Silicon. Phase 6 is the App Store release.
  Physics uses `rapier3d` for now; a custom arcade-physics layer is planned.
- **The Graphics menu is macOS-only.** It is installed only in windowed runs. Headless runs,
  tests and `--smoke` never read or write the saved settings.
- **Demo asset licences.** The demo's third-party models and textures are CC0. The music and
  crash WAVs were generated for this project and dedicated CC0 1.0. The HUD font is OFL. See
  `games/playable-demo/README.md`.

[Unreleased]: https://github.com/parsabee/StickShiftEngine/compare/v0.2.2...HEAD
[0.2.2]: https://github.com/parsabee/StickShiftEngine/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/parsabee/StickShiftEngine/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/parsabee/StickShiftEngine/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/parsabee/StickShiftEngine/compare/v0.1.0-beta.1...v0.1.0
[0.1.0-beta.2]: https://github.com/parsabee/StickShiftEngine/compare/v0.1.0-beta.1...f48f44a
[0.1.0-beta.1]: https://github.com/parsabee/StickShiftEngine/compare/v0.1.0-alpha.2...v0.1.0-beta.1
[0.1.0-alpha.2]: https://github.com/parsabee/StickShiftEngine/compare/v0.1.0-alpha.1...v0.1.0-alpha.2
[0.1.0-alpha.1]: https://github.com/parsabee/StickShiftEngine/releases/tag/v0.1.0-alpha.1
