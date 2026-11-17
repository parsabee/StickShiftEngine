# Offramp

Offramp is the playable demo ([`games/playable-demo`](../playable-demo)), an endless three-lane
freeway runner, rebuilt in Swift. It is game code only: the engine (its ECS, scene streaming and
floating origin, game loop, camera, input, audio and graphics settings) is the Rust engine,
reached through the C API by way of the StickShift Swift package
([`bindings/swift/StickShift`](../../bindings/swift/StickShift)). It is the reference for building
a native macOS game on Stick Shift Engine.

This first milestone is an exact port. Offramp has the same world, the same random sequence and
the same state on every step as the Rust demo. Its frames match the Rust recorder's pixel for
pixel, and its `--smoke` output and tests are the same. Its own features (presentation, a
day/night cycle, curved roads, car physics) come next; see the [roadmap](../../docs/ROADMAP.md).

## Running it

SwiftPM cannot run cargo, so build the C library first. Then build and run the package:

```sh
cargo build -p stickshift-capi                                # target/debug/libstickshift.a
swift run --package-path games/offramp offramp                # play, in a window (Metal)
swift run --package-path games/offramp offramp --smoke        # headless: 120 frames, no GPU
```

| Key | Action |
|---|---|
| `Left` / `Right` | Move one lane left or right (one lane per press) |
| `Space` | Start the run from the title screen; replay after a crash |
| `Escape` | Quit |

The window is titled "Stick Shift Engine", as the Rust demo's is. The **Graphics** menu in the
menu bar is the engine's: shadows, shadow distance, draw distance and resolution (50% by
default), plus full screen (⌃⌘F) and Reset to Defaults. The engine saves the choices under its
keys (`StickShiftEngine.graphics.*`) in Offramp's own defaults domain, `offramp`, so the two
programs do not share them. Sound plays through the engine's audio on the default output
device, or not at all when there is none.

The command line:

| Option | What it does |
|---|---|
| (none) | Play in a window |
| `--smoke` | The headless smoke oracle. It prints exactly what `cargo run -p playable-demo -- --smoke` prints, and exits 0 |
| `--print-assets-dir` | Print the asset directory in use and exit |
| `--record <DIR> [--frames N]` | The Rust `record_demo` example: the autopilot run, offscreen on Metal at 1280×720, written as `frame_0000.png`, `frame_0001.png`, ... (N frames, default 80: every third frame after a 150-frame warm-up) |
| `--trace <smoke\|autopilot\|long> [--summary]` | Print one JSON line of game state per frame of a headless run (below) |

A bad command line prints the usage and exits 2. As with clap, an option never takes an
argument that looks like a flag as its value: `--record --smoke` is a missing value, and
`--record=--smoke` names a directory called `--smoke`. A single-dash argument is a cluster of
short flags, so `-hx` prints the help and `-xh` is an unknown `-x`, and `--` ends the options:
nothing may follow it. If no asset directory is found, Offramp
says so and exits 1, before doing anything else. Standard output is line-buffered, as Rust's
is, so the game's "GAME OVER ..." and "replay ..." lines reach a pipe or a file as they happen.

## Assets

Offramp reads the Rust demo's committed assets in place, so there is one copy. The asset
directory is the first of these that holds `road.gltf`:

1. `$OFFRAMP_ASSETS`;
2. `<executable dir>/../share/offramp/assets`;
3. `<executable dir>/assets`;
4. the checkout's `games/playable-demo/assets`.

If `$OFFRAMP_ASSETS` is set, it is the only candidate.

## Layout

- `Sources/OfframpGame` holds the game and its tools, everything testable. Its files mirror the
  Rust modules they port, so the two can be read side by side.
  - `Config.swift` ports `config.rs`, `Rng.swift` `rng.rs`, `Scenery.swift` `scenery.rs`,
    `AssetRoot.swift` `asset_root.rs` and `Components.swift` `components.rs` (the game's four
    components, and the world digest the traces compare).
  - `Game.swift` (`game.rs`) is `CarRunner`, a `StickShift.Game`.
  - The drawing:
    - `Assets.swift` (`assets.rs`) loads and fits the thirteen models.
    - `Render.swift` (`render.rs`) gathers the draw list from the engine's world and records
      the draws in Rust's order into the frame the engine opened.
    - `HUD.swift` (`hud.rs`) lays out the HUD; the engine measures and draws the text.
    - `Font.swift` reads the `font.bin` atlas (`hud::load_font`) and makes the engine's font.
    - `PNG.swift` writes and reads the recorder's frames.
  - The tools: `Smoke.swift` is `--smoke` on a headless engine, `Autopilot.swift` is
    `record_demo.rs` (its steering, and its recording loop on an offscreen engine),
    `Trace.swift` is the parity trace, and `Arguments.swift` is the command line.
- `Sources/offramp` is the executable:
  - `main.swift` is the command line.
  - `App.swift` runs the game in a window with `StickShiftApp` (the package's
    `StickShiftAppKit`): the application and its menus, the window, and an `EngineView` that
    ticks the engine on the display link (and from a watchdog while the window is hidden, as
    Rust's loop never pauses) and forwards keys, the mouse and focus to it.
  - `Recorder.swift` writes the recorder's PNG files.
- `Tests/OfframpTests` holds the Swift Testing suites, many of them against golden values
  captured from the Rust demo (`Tests/OfframpTests/Fixtures`).

## How parity is checked

The engine Offramp runs on is the Rust engine itself: the frame cadence, the fixed step, the input
edges, hecs's archetypes and query order, streaming and the rebase, the chase camera and the
audio counters are the same code the Rust demo runs. What is left in Swift is the game's own
float math, which computes every `f32` of the Rust game in `Float`, one IEEE operation at a time,
in the same order, so the results have the same bits. The glam operations it needs come from
the engine through the package (`glamDot`, `glamRotate`, `simd_quatf(glamRotationY:)`, the
`Transform` matrix), never from `simd`'s reductions or fused products, and Rust's `f32` rules
from `RustF32` (π is Rust's, not `Float.pi`). A lint test keeps those out of the game. Each
entity is spawned with the same set of components as the Rust tuple, so the archetypes, and the
order every query walks, are Rust's.

Three layers check that this holds:

1. **The tests.** Every test of the Rust demo (`game.rs`, `hud.rs`, `main.rs`, `asset_root.rs`)
   is ported under its Rust name, with its assertions. (The engine's own tests are in Rust and
   in the package's suites.) On top of those, golden values from the Rust demo are checked bit
   for bit:
   - the random sequence and the per-slot scenery;
   - every entity of the world after init, and the world's digest on every frame of the
     recorder's and the smoke run's traces;
   - the camera and its view-projection on every frame;
   - the HUD's quads and the model fits.
2. **The trace.** `offramp --trace <mode>` and the Rust twin
   `cargo run -p playable-demo --example trace -- <mode>` print the same JSON line per frame. A
   line holds:
   - the state, the player, the speed, the scores and the fade;
   - the camera, and hashes of its view-projection and of the frame's HUD quads;
   - the entity counts, the audio counters, the 3D draws and the world digest.

   Floats are printed as their bit patterns. The modes are `smoke` (the `--smoke` run),
   `autopilot` (the recorder's 390 frames) and `long` (12 000 frames of the autopilot replaying
   after every crash, across four floating-origin rebases). The tests compare the Swift traces
   with the Rust ones in `Tests/OfframpTests/Fixtures` (`trace_*.jsonl`).
3. **The frames.** `offramp --record` and the Rust `record_demo` write the same 80 frames. The
   tests check that the recorder's first frame is the frame rebuilt from the Rust dumps.

`scripts/offramp-parity.sh` runs both programs side by side and compares them:

```sh
./scripts/offramp-parity.sh              # both halves, debug builds (as CI)
./scripts/offramp-parity.sh --traces     # no GPU: --smoke and the three traces, byte for byte
./scripts/offramp-parity.sh --frames     # Metal: all 80 recorded frames, pixel by pixel
./scripts/offramp-parity.sh --release    # optimised builds of both sides, libstickshift included
```

It reports the first differing line or frame, and how many differ, and exits 1 if anything
does. Its output is in `target/offramp-parity`. Frames are compared by their decoded pixels
(`scripts/compare-frames.swift`), never by PNG bytes, because the two recorders use different
PNG encoders. A pixel comparison needs both sides on the same GPU with the same shader build.
Both compile their shaders at run time unless the library is built with `precompiled-shaders`.
CI runs the tests and the whole parity script, both sides on the runner's GPU.

Parity is defined for the drivers that run one fixed update per frame: the tests, `--smoke`, the
trace and the recorder. The window runs on the real clock, as the Rust one does, so a frame
there may run zero or several updates.

## What it uses of the engine

Everything through the StickShift package, which wraps the C API:

- `Engine` and the `Game` protocol: the engine's loop calls `CarRunner`'s `initialize`, its
  fixed-step `update` and its `render`. The tools drive it a fixed step at a time
  (`Engine.headless` over a null context for `--smoke` and the trace, an offscreen Metal
  context for the recorder, which reads a frame back within its tick); the window drives it
  on the display link's clock.
- `World` and `Query` (hecs), `Scene` and `StreamSlot` (streaming, the floating origin),
  `EngineCamera` and `ChaseController`, `Input`, `Audio` (kira), and the graphics settings
  with their menu and persistence.
- The frame: model draws at `world × (local × node)` with a part filter
  (`Frame.draw(_:world:local:parts:tint:)`), the text drawing and measuring of the engine's
  `FontAtlas`, and `Context.lastFrame`, which the trace reads the HUD's quads from.

## Where it differs from the Rust demo

- **Missing assets.** A missing model or font makes `initialize` throw, and Offramp prints the
  error and exits 1. Rust panics.
- **A failed frame.** `--smoke`, like the trace and the recorder, checks every frame: one
  update, and a rendered, submitted frame with no render error, no draw skipped. A frame that
  fails those, or a run of the wrong length, prints the error and exits 1, never the
  `smoke: ... OK` line. Rust panics.

## Testing

```sh
./scripts/test-offramp.sh                          # cargo build -p stickshift-capi, then swift test
./scripts/test-offramp.sh --filter TraceTests      # one suite
```

The Metal tests skip without a Metal device, unless `STICKSHIFT_REQUIRE_GPU=1`. The longest tests
are the 20 000-step dodging runs, the 5000-frame streaming run and the 12 000-frame trace.
To compare the recorded frames with the Rust recorder's in the tests as well, record the Rust
frames and name their directory. Without it, those two tests are reported as skipped;
`scripts/offramp-parity.sh` compares all 80 frames either way.

```sh
cargo run --release -p playable-demo --example record_demo -- /tmp/rust-frames
OFFRAMP_RUST_FRAMES=/tmp/rust-frames ./scripts/test-offramp.sh --filter 'RecorderTests|RenderMetalTests'
```

`OFFRAMP_SAVE_FRAMES=<dir>` also writes the Swift frame 150 there as a PNG.

The tests run the engine's audio silent, which decodes nothing. The one that decodes the
committed WAVs on the default output device (ignored in Rust, as it needs sound hardware) runs
only with `OFFRAMP_DEVICE_TESTS=1`, at -60 dB:

```sh
OFFRAMP_DEVICE_TESTS=1 ./scripts/test-offramp.sh --filter GameAudioTests
```

To regenerate the fixtures after a deliberate change to the Rust demo (the
[fixtures README](Tests/OfframpTests/Fixtures/README.md) says what each holds):

```sh
F=games/offramp/Tests/OfframpTests/Fixtures
cargo run -q --release -p playable-demo-goldens -- $F
cargo run -q --release -p playable-demo --example trace -- smoke > $F/trace_smoke.jsonl
cargo run -q --release -p playable-demo --example trace -- autopilot > $F/trace_autopilot.jsonl
cargo run -q --release -p playable-demo --example trace -- long --summary > $F/trace_long_summary.jsonl
```

The Rust long run keeps every draw in its `NullRenderer`, about 1.4 GB.

`OFFRAMP_TRACE_OUT=<dir> ./scripts/test-offramp.sh --filter TraceTests` also writes the traces
the tests compute to `<dir>`, creating it: `swift_smoke.jsonl` and `swift_autopilot.jsonl`,
every line, and `swift_long_summary.jsonl`, the long run's summary (every 600th frame, the state
changes, the rebases and the hash of every line). To diff a whole long run line by line,
compare `offramp --trace long` with `cargo run --release -p playable-demo --example trace --
long`, as `scripts/offramp-parity.sh` does.

Some of the app's behaviour needs a display and a person, so it is checked by hand:

- Launch it from a terminal (`swift run --package-path games/offramp offramp`) and press Space
  without clicking the window: the run starts, because the app took the keyboard.
- In the centre lane, hold Command and tap Right, let go of Command, then tap Left and Right:
  the car goes right, back, and right again (a lost key-up would leave the last Right
  unanswered).
- Change a Graphics setting, quit and relaunch: the setting is restored.
