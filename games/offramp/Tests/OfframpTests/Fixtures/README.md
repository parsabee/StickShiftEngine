# Golden fixtures

Values captured from the Rust demo (`games/playable-demo`), which the Swift tests compare
against bit for bit. They are printed by programs in the repository that compile the demo's own
modules (the `#[path]` trick `examples/record_demo.rs` uses) against the headless
`NullRenderer`, so they are the Rust code's results, not hand-written expectations. A float
prints as `value[bits]` (or as `value (0x<bits>)` in `probe_stdout.txt`); the tests compare the
bits.

| File | What it holds |
|---|---|
| `vectors.txt` | The first 32 lane draws from `SEED`; per-slot hash, traffic variant, building, jitter and x for slots 0-40 and three negative slots; constants' bits; speed-ramp samples |
| `scenery.txt` | The guardrail and terrain meshes: counts, FNV-1a 64 of the vertex bytes and the indices, sample vertices, `terrain_height` samples |
| `init_world.txt` | The world after `init`: the streaming config, the camera, and all 171 entities in hecs query order with their ids |
| `record_trace.txt` | The 390 frames of `record_demo`'s autopilot run at 1280×720: per frame the state, player, speed, score, fade, camera, counts and world digest |
| `record_view_projection.txt` | The view-projection at frames 0, 1, 2, 149, 150, 151 and 389 of that run |
| `record_frame150_world.txt` | The whole world at frame 150 of that run, showing swap-remove's reordering |
| `smoke_trace.txt` | 900 frames in the centre lane, Space at frame 0 and on every GameOver frame divisible by 60 |
| `probe_stdout.txt` | The headless tests' observed values: constants, the RNG, the font's metrics and widths, and every HUD quad of the Ready, Playing and GameOver frames |
| `long_run_events.txt` | 12 000 frames of the autopilot that replays after every crash: every 600th frame, each state change and each rebase (with the camera relative to the car around it), then the maximum entity count and the audio counters |
| `smoke_state.json` | The 120 frames of `--smoke` (numbered from 1), with the fields `game.rs` keeps private: lane, travel, distance, run clock and fade, read through the probe's accessors |
| `trace_smoke.jsonl`, `trace_autopilot.jsonl`, `trace_long_summary.jsonl` | The parity trace, printed by `games/playable-demo/examples/trace.rs` (see below) |

`probe_stdout.txt` is the probe's standard output, so it also holds the game's own "GAME OVER
..." and "replay ..." lines, where the game printed them.

The first ten files come from `playable-demo-goldens`
([`games/playable-demo/goldens`](../../../../playable-demo/goldens)), a workspace crate that
compiles the demo's modules unchanged, except for `game.rs`: its build script compiles a copy
with accessors appended, through which the probe reads and sets `CarRunner`'s private state
(lane, travel, run clock, fade, seed). `games/playable-demo/src` itself is never edited. The
trace files come from the demo's own `trace.rs` example. After a deliberate change to the Rust
demo, regenerate them all:

```sh
F=games/offramp/Tests/OfframpTests/Fixtures
cargo run -q --release -p playable-demo-goldens -- $F
cargo run -q --release -p playable-demo --example trace -- smoke > $F/trace_smoke.jsonl
cargo run -q --release -p playable-demo --example trace -- autopilot > $F/trace_autopilot.jsonl
cargo run -q --release -p playable-demo --example trace -- long --summary > $F/trace_long_summary.jsonl
```

To check that the committed files are still what the Rust demo prints, write them somewhere else
and compare. This exits 1 if any file differs, after `cmp` names each:

```sh
cargo run -q --release -p playable-demo-goldens -- /tmp/goldens && ok=1 &&
for f in /tmp/goldens/*; do cmp "$f" "$F/$(basename "$f")" || ok=0; done &&
for mode in smoke autopilot; do
    cargo run -q --release -p playable-demo --example trace -- $mode | cmp - $F/trace_$mode.jsonl || ok=0
done &&
{ cargo run -q --release -p playable-demo --example trace -- long --summary \
    | cmp - $F/trace_long_summary.jsonl || ok=0; } && [ $ok = 1 ]
```

`playable-demo-goldens golden <dir>` and `playable-demo-goldens probe <dir>` run one half each;
the probe's report goes to stdout.

A trace line is one JSON object per frame: `frame`, `tap` (the key pressed for it, or `null`),
`state`, `player`, `speed`, `score`, `best`, `fade`, `camera` (`position`, `target`), `vp`
(FNV-1a 64 of the view-projection's sixteen elements, column by column), `streamed`,
`entities`, `obstacles`, `rebased`, `loop_starts`, `one_shots`, `draws` (3D draws, counting a
model's parts), `hud` (the frame's overlay quad count and an FNV-1a 64 of their fill kind,
rect, uv and colour), `autopilot_speed` (not in the smoke run) and `digest`. Floats are their
bit patterns as eight hex digits. The game's own messages ("GAME OVER ...", "replay ...") are
the lines that do not start with `{`, where the game printed them. A summary ends with
`{"summary":{...}}`, holding the FNV-1a 64 of every frame's line (each with its newline).

The world digest is FNV-1a 64 over every entity with a transform, in query order: a kind byte
(player 0, road 1, obstacle 2, building 3, guardrail 4), the variant as a little-endian `u32`,
then the bits of position xyz, rotation xyzw and scale xyz, little-endian (`worldDigest`).
