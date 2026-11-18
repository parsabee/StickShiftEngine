// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! Records the playable demo to numbered PNG frames, for the README's GIF.
//!
//! The real game (the same modules the binary builds) runs against an
//! **offscreen** Metal renderer at a fixed timestep, and every few frames the
//! rendered image is read back and written out. No window or screen capture is
//! involved, so it runs from a plain terminal. A small autopilot steers: it only
//! taps Left or Right when a car is about to reach the player in its lane, so
//! every lane change in the clip answers a car right in front.
//! Traffic is seeded, so the same command records the same frames.
//!
//! ```sh
//! cargo run --release -p offramp --example record_demo -- <out-dir>
//! ```
//!
//! `scripts/record-demo-gif.sh` runs this and assembles `docs/images/demo.gif`.
//! macOS only (it needs a Metal device).

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

use std::path::PathBuf;

use stickshift_core::driver::{drive_frame, Loop};
use stickshift_core::input::Key;
use stickshift_core::FIXED_DT_DURATION;
use stickshift_ecs::TransformComponent;
#[cfg(target_os = "macos")]
use stickshift_render::MetalRenderer;
#[cfg(target_os = "macos")]
use stickshift_render_api::RenderDevice;

use components::TrafficVariant;
use game::CarRunner;

/// Rendered frame size, in pixels (the GIF is scaled down from this).
const WIDTH: u32 = 1280;
const HEIGHT: u32 = 720;
/// Simulated frames before recording starts: past the title screen's fade-in,
/// once the speed has ramped up and traffic is busy.
const WARM_UP_FRAMES: u32 = 150;
/// Simulated frames to record (at 60 per second): a 4-second loop.
const RECORD_FRAMES: u32 = 60 * 4;
/// Keep every `STRIDE`th frame: 60 / 3 = 20 frames per second in the GIF.
const STRIDE: u32 = 3;
/// How soon (seconds, at the current speed) a car ahead in the player's lane
/// must be to make the autopilot change lanes. Lane changes are instant, so a
/// short warning is enough.
const REACTION_SECONDS: f32 = 0.45;
/// Extra distance (world units) on top of that, about a car length.
const MARGIN: f32 = 5.0;
/// Frames the autopilot waits between lane changes.
const COOLDOWN: u32 = 12;

/// Off macOS there is no Metal backend to record with.
#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("record_demo needs macOS and a Metal device");
    std::process::exit(1);
}

#[cfg(target_os = "macos")]
fn main() {
    let out: PathBuf = std::env::args_os()
        .nth(1)
        .map_or_else(|| PathBuf::from("target/record-demo"), PathBuf::from);
    std::fs::create_dir_all(&out).expect("create the output directory");
    if let Err(e) = asset_root::init() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }

    let mut renderer =
        MetalRenderer::new_offscreen(WIDTH, HEIGHT).expect("a Metal device is required");
    renderer.resize_surface(WIDTH, HEIGHT, 1.0);
    let mut lp = Loop::new();
    lp.camera.set_aspect_ratio(WIDTH as f32 / HEIGHT as f32);
    let mut game = CarRunner::new();
    lp.init_once(&mut game, &mut renderer);

    let mut held: Option<Key> = None;
    let mut cooldown = 0;
    let mut written = 0;
    let mut lane_changes = 0;
    // The player's speed, estimated from its travel each frame.
    let mut last_z = game.player_position().z;
    let mut speed = config::SPEED;
    for frame in 0..WARM_UP_FRAMES + RECORD_FRAMES {
        // Release last frame's tap, so each press is a fresh edge.
        if let Some(key) = held.take() {
            lp.input.release_key(key);
        }
        let tap = if frame == 0 {
            Some(Key::Space) // start the run
        } else if cooldown > 0 {
            cooldown -= 1;
            None
        } else {
            steer(&lp, &game, speed)
        };
        if let Some(key) = tap {
            lp.input.press_key(key);
            held = Some(key);
            if key != Key::Space {
                cooldown = COOLDOWN;
                if frame >= WARM_UP_FRAMES {
                    lane_changes += 1;
                }
            }
        }

        drive_frame(&mut lp, &mut game, &mut renderer, FIXED_DT_DURATION);
        let z = game.player_position().z;
        let travelled = (last_z - z) / FIXED_DT_DURATION.as_secs_f32();
        // A floating-origin rebase jumps the position; keep the last estimate.
        if (0.0..=config::SPEED_MAX * 1.5).contains(&travelled) {
            speed = speed.max(travelled);
        }
        last_z = z;

        let recording = frame >= WARM_UP_FRAMES;
        if recording && (frame - WARM_UP_FRAMES).is_multiple_of(STRIDE) {
            let mut pixels = renderer.read_pixels().expect("an offscreen frame");
            for px in pixels.chunks_exact_mut(4) {
                px.swap(0, 2); // BGRA -> RGBA
            }
            let path = out.join(format!("frame_{written:04}.png"));
            image::save_buffer(&path, &pixels, WIDTH, HEIGHT, image::ColorType::Rgba8)
                .expect("write a frame");
            written += 1;
        }
    }
    println!(
        "wrote {written} frames ({lane_changes} lane changes) to {}",
        out.display()
    );
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
