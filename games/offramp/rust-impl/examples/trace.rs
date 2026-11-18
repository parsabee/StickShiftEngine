// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! Prints the playable demo's state, one JSON line per frame, for comparing the
//! Swift port (`games/offramp`, `offramp --trace`) with this, the Rust original.
//!
//! The real game (the same modules the binary builds) runs headlessly against the
//! `NullRenderer`, one fixed step per frame, so every run is identical and needs
//! no GPU:
//!
//! - `smoke`: the `--smoke` run, 120 frames, Space on the first (the headless 4:3
//!   camera).
//! - `autopilot`: `record_demo`'s run, 390 frames steered by its autopilot, at its
//!   1280/720 aspect (nothing is rendered or written).
//! - `long`: 12 000 frames of the same autopilot that also taps Space whenever the
//!   game is in `GameOver`: crashes, replays and four floating-origin rebases.
//!
//! ```sh
//! cargo run -p offramp --example trace -- smoke
//! cargo run --release -p offramp --example trace -- long --summary
//! ```
//!
//! Each line holds what the game's crate-visible API shows: the state, the
//! player's position, the speed, the scores and the fade; the camera's eye and
//! target and a hash of its view-projection; the streaming and entity counts;
//! whether the frame rebased; the audio counters; the 3D draws and a hash of the
//! HUD quads the frame recorded; the autopilot's speed estimate; and a digest of
//! the whole world. Floats are their bit patterns in hex. The game's own messages
//! ("GAME OVER ...", "replay ...") come out between the lines, where it prints
//! them. With `--summary` only every 600th frame, a frame whose state changed or
//! that rebased, and the last are printed, then a summary line with the FNV-1a 64
//! of every frame's line.
//!
//! The Swift port prints the identical bytes; its tests compare them with copies
//! of this output (`games/offramp/swift-impl/Tests/OfframpTests/Fixtures`). The long run
//! needs about 1.4 GB, because the `NullRenderer` keeps every draw it records;
//! build it with `--release`.

// The game's modules, compiled into this example exactly as the binary does.
// Some of their items are only used by the binary, hence the allow.
#![allow(dead_code)]

#[path = "../src/asset_root.rs"]
mod asset_root;
#[path = "../src/assets.rs"]
mod assets;
#[path = "../src/components.rs"]
mod components;
#[path = "../src/config.rs"]
mod config;
#[path = "../src/game.rs"]
mod game;
#[path = "../src/hud.rs"]
mod hud;
#[path = "../src/render.rs"]
mod render;
#[path = "../src/rng.rs"]
mod rng;
#[path = "../src/scenery.rs"]
mod scenery;

use std::fmt::Write as _;

use stickshift_core::driver::{drive_frame, Loop};
use stickshift_core::input::Key;
use stickshift_core::FIXED_DT_DURATION;
use stickshift_ecs::hecs::{Entity, World};
use stickshift_ecs::TransformComponent;
use stickshift_math::glam::{Mat4, Vec3};
use stickshift_render_api::{NullRenderer, OverlayFill, OverlayQuad};

use components::{BuildingVariant, GuardrailTag, Obstacle, TrafficVariant};
use game::{CarRunner, GameState};

/// `record_demo`'s frame size: its camera aspect.
const WIDTH: u32 = 1280;
const HEIGHT: u32 = 720;
/// `record_demo`'s frames: 150 of warm-up and 240 recorded.
const RECORD_DEMO_FRAMES: u32 = 150 + 60 * 4;
/// The `--smoke` run's frames (`main.rs`'s `SMOKE_FRAMES`).
const SMOKE_FRAMES: u32 = 120;
/// The long run's frames.
const LONG_FRAMES: u32 = 12_000;
/// With `--summary`, every this many frames is printed.
const SUMMARY_EVERY: u32 = 600;

// `record_demo`'s autopilot, as it is there (`examples/record_demo.rs`); keep the
// two in step.
const REACTION_SECONDS: f32 = 0.45;
const MARGIN: f32 = 5.0;
const COOLDOWN: u32 = 12;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Smoke,
    Autopilot,
    Long,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Smoke => "smoke",
            Mode::Autopilot => "autopilot",
            Mode::Long => "long",
        }
    }

    fn frames(self) -> u32 {
        match self {
            Mode::Smoke => SMOKE_FRAMES,
            Mode::Autopilot => RECORD_DEMO_FRAMES,
            Mode::Long => LONG_FRAMES,
        }
    }
}

const USAGE: &str = "usage: trace <smoke|autopilot|long> [--summary]";

fn main() {
    let mut mode = None;
    let mut summary = false;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "smoke" => mode = Some(Mode::Smoke),
            "autopilot" => mode = Some(Mode::Autopilot),
            "long" => mode = Some(Mode::Long),
            "--summary" => summary = true,
            _ => {
                eprintln!("error: unexpected argument '{arg}'\n{USAGE}");
                std::process::exit(2);
            }
        }
    }
    let Some(mode) = mode else {
        eprintln!("{USAGE}");
        std::process::exit(2);
    };
    if let Err(e) = asset_root::init() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
    run(mode, summary);
}

/// Drive `mode`'s run and print its trace.
fn run(mode: Mode, summary: bool) {
    let mut lp = Loop::new();
    let mut renderer = NullRenderer::new();
    if mode != Mode::Smoke {
        lp.camera.set_aspect_ratio(WIDTH as f32 / HEIGHT as f32);
    }
    let mut game = CarRunner::new();
    lp.init_once(&mut game, &mut renderer);

    let mut held: Option<Key> = None;
    let mut cooldown = 0;
    let mut last_z = game.player_position().z;
    let mut speed = config::SPEED;
    let mut all = Fnv::new();
    let mut rebases = 0u32;
    let mut max_entities = 0u32;
    for frame in 0..mode.frames() {
        // Release last frame's tap, so each press is a fresh edge.
        if let Some(key) = held.take() {
            lp.input.release_key(key);
        }
        let tap = match mode {
            Mode::Smoke => (frame == 0).then_some(Key::Space),
            Mode::Autopilot | Mode::Long => {
                if frame == 0 || (mode == Mode::Long && game.state == GameState::GameOver) {
                    Some(Key::Space)
                } else if cooldown > 0 {
                    cooldown -= 1;
                    None
                } else {
                    steer(&lp, &game, speed)
                }
            }
        };
        if let Some(key) = tap {
            lp.input.press_key(key);
            held = Some(key);
            if key != Key::Space {
                cooldown = COOLDOWN;
            }
        }

        let state_before = game.state;
        let z_before = game.player_position().z;
        let quads_before = renderer.overlay_quad_count();
        let draws_before = renderer.draw_count();
        drive_frame(&mut lp, &mut game, &mut renderer, FIXED_DT_DURATION);
        let player = game.player_position();
        if mode != Mode::Smoke {
            let travelled = (last_z - player.z) / FIXED_DT_DURATION.as_secs_f32();
            if (0.0..=config::SPEED_MAX * 1.5).contains(&travelled) {
                speed = speed.max(travelled);
            }
            last_z = player.z;
        }
        // A rebase moves the car back by a whole threshold; nothing else moves it
        // toward +z.
        let rebased = player.z - z_before > 500.0;

        let world = lp.scene.world();
        let mut line = String::new();
        let _ = write!(line, "{{\"frame\":{frame}");
        match tap {
            Some(key) => {
                let _ = write!(line, ",\"tap\":\"{key:?}\"");
            }
            None => line.push_str(",\"tap\":null"),
        }
        let _ = write!(line, ",\"state\":\"{}\"", state_name(game.state));
        let _ = write!(line, ",\"player\":{}", vec3(player));
        let _ = write!(line, ",\"speed\":{}", float(game.current_speed()));
        let _ = write!(
            line,
            ",\"score\":{},\"best\":{}",
            game.score(),
            game.best_score()
        );
        let _ = write!(line, ",\"fade\":{}", float(game.fade_alpha()));
        let _ = write!(
            line,
            ",\"camera\":{{\"position\":{},\"target\":{}}}",
            vec3(lp.camera.position()),
            vec3(lp.camera.target())
        );
        let _ = write!(
            line,
            ",\"vp\":{}",
            hash(matrix_hash(lp.camera.view_projection_matrix()))
        );
        let _ = write!(
            line,
            ",\"streamed\":{},\"entities\":{},\"obstacles\":{}",
            lp.scene.streamed_count(),
            world.len(),
            world.query::<&Obstacle>().iter().count()
        );
        let _ = write!(line, ",\"rebased\":{rebased}");
        let _ = write!(
            line,
            ",\"loop_starts\":{},\"one_shots\":{}",
            lp.audio.loop_starts(),
            lp.audio.one_shots_played()
        );
        let _ = write!(line, ",\"draws\":{}", renderer.draw_count() - draws_before);
        let quads = &renderer.overlay_quads()[quads_before..];
        let _ = write!(
            line,
            ",\"hud\":{{\"quads\":{},\"fnv\":{}}}",
            quads.len(),
            hash(quads_hash(quads))
        );
        if mode != Mode::Smoke {
            let _ = write!(line, ",\"autopilot_speed\":{}", float(speed));
        }
        let _ = write!(line, ",\"digest\":{}}}", hash(digest(world, game.player)));

        all.eat(line.as_bytes());
        all.eat(b"\n");
        if rebased {
            rebases += 1;
        }
        max_entities = max_entities.max(world.len());
        if !summary
            || frame % SUMMARY_EVERY == 0
            || game.state != state_before
            || rebased
            || frame == mode.frames() - 1
        {
            println!("{line}");
        }
    }
    if summary {
        println!(
            "{{\"summary\":{{\"mode\":\"{}\",\"frames\":{},\"fnv\":{},\"rebases\":{rebases},\"max_entities\":{max_entities},\"loop_starts\":{},\"one_shots\":{}}}}}",
            mode.name(),
            mode.frames(),
            hash(all.0),
            lp.audio.loop_starts(),
            lp.audio.one_shots_played()
        );
    }
}

fn state_name(state: GameState) -> &'static str {
    match state {
        GameState::Ready => "Ready",
        GameState::Playing => "Playing",
        GameState::GameOver => "GameOver",
    }
}

/// A float as its quoted bit pattern.
fn float(v: f32) -> String {
    format!("\"{:08x}\"", v.to_bits())
}

fn vec3(v: Vec3) -> String {
    format!("[{},{},{}]", float(v.x), float(v.y), float(v.z))
}

fn hash(h: u64) -> String {
    format!("\"{h:016x}\"")
}

/// FNV-1a 64.
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn eat(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    fn eat_f32(&mut self, v: f32) {
        self.eat(&v.to_bits().to_le_bytes());
    }
}

/// FNV-1a 64 of the sixteen elements, column by column.
fn matrix_hash(m: Mat4) -> u64 {
    let mut h = Fnv::new();
    for v in m.to_cols_array() {
        h.eat_f32(v);
    }
    h.0
}

/// FNV-1a 64 of overlay quads in record order: a fill byte (solid 0, textured 1,
/// SDF 2; not the texture handle), then the bits of the rect, uv and colour.
fn quads_hash(quads: &[OverlayQuad]) -> u64 {
    let mut h = Fnv::new();
    for q in quads {
        let fill = match q.fill {
            OverlayFill::Solid => 0u8,
            OverlayFill::Textured(_) => 1,
            OverlayFill::Sdf(_) => 2,
        };
        h.eat(&[fill]);
        for v in q.rect.iter().chain(&q.uv).chain(&q.color) {
            h.eat_f32(*v);
        }
    }
    h.0
}

/// The kind byte (player 0, road 1, obstacle 2, building 3, guardrail 4) and
/// variant of `e`.
fn kind(world: &World, e: Entity, player: Option<Entity>) -> (u8, u32) {
    if Some(e) == player {
        (0, 0)
    } else if let Ok(v) = world.get::<&TrafficVariant>(e) {
        (2, v.0 as u32)
    } else if let Ok(v) = world.get::<&BuildingVariant>(e) {
        (3, v.0 as u32)
    } else if world.get::<&GuardrailTag>(e).is_ok() {
        (4, 0)
    } else {
        (1, 0)
    }
}

/// FNV-1a 64 over every entity with a transform, in query order: its kind byte,
/// its variant as a little-endian `u32`, then the bits of its position xyz,
/// rotation xyzw and scale xyz, little-endian.
fn digest(world: &World, player: Option<Entity>) -> u64 {
    let mut h = Fnv::new();
    for (e, t) in world.query::<&TransformComponent>().iter() {
        let (code, variant) = kind(world, e, player);
        h.eat(&[code]);
        h.eat(&variant.to_le_bytes());
        let tr = t.transform;
        for v in [
            tr.position.x,
            tr.position.y,
            tr.position.z,
            tr.rotation.x,
            tr.rotation.y,
            tr.rotation.z,
            tr.rotation.w,
            tr.scale.x,
            tr.scale.y,
            tr.scale.z,
        ] {
            h.eat_f32(v);
        }
    }
    h.0
}

/// The lane index of world `x`.
fn lane_of(x: f32) -> i32 {
    (x / config::LANE_WIDTH + (config::LANES as f32 - 1.0) / 2.0).round() as i32
}

/// The distance ahead of the player of the nearest traffic car in `lane`, at
/// least `behind` units back (negative is behind), or `f32::INFINITY` if none.
fn nearest_in_lane(cars: &[(i32, f32)], lane: i32, behind: f32) -> f32 {
    cars.iter()
        .filter(|&&(l, ahead)| l == lane && ahead >= behind)
        .map(|&(_, ahead)| ahead)
        .fold(f32::INFINITY, f32::min)
}

/// The key the autopilot taps this frame, if any: when a car ahead in this lane
/// is about to reach the player at `speed`, move to the adjacent lane whose
/// nearest car is furthest away.
fn steer(lp: &Loop, game: &CarRunner, speed: f32) -> Option<Key> {
    let player = game.player_position();
    let lane = lane_of(player.x);
    // Each traffic car's lane and distance ahead (the game travels toward -Z).
    let cars: Vec<(i32, f32)> = lp
        .scene
        .world()
        .query::<(&TransformComponent, &TrafficVariant)>()
        .iter()
        .map(|(_, (t, _))| {
            let p = t.transform.position;
            (lane_of(p.x), player.z - p.z)
        })
        .collect();
    if nearest_in_lane(&cars, lane, -3.0) > speed * REACTION_SECONDS + MARGIN {
        return None;
    }
    [(lane - 1, Key::Left), (lane + 1, Key::Right)]
        .into_iter()
        .filter(|&(l, _)| (0..config::LANES as i32).contains(&l))
        // A lane with a car right beside the player is not an option.
        .map(|(l, key)| (nearest_in_lane(&cars, l, -6.0), key))
        .filter(|&(clear, _)| clear > 8.0)
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, key)| key)
}
