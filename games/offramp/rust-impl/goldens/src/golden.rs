// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! The golden values of the world, the scenery and three headless runs, from the
//! demo's unmodified modules (only crate-visible API): `vectors.txt`,
//! `scenery.txt`, `init_world.txt`, `smoke_trace.txt`, `record_trace.txt`,
//! `record_view_projection.txt`, `record_frame150_world.txt` and
//! `long_run_events.txt`. A float prints as `value[bits]`.

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use stickshift_camera::ChaseController;
use stickshift_core::driver::{drive_frame, Loop};
use stickshift_core::input::Key;
use stickshift_core::{FIXED_DT, FIXED_DT_DURATION};
use stickshift_ecs::hecs::{Entity, World};
use stickshift_ecs::TransformComponent;
use stickshift_math::glam::{Quat, Vec3};
use stickshift_render_api::NullRenderer;

use crate::components::{BuildingVariant, GuardrailTag, Obstacle, TrafficVariant};
use crate::game::{CarRunner, GameState};
use crate::{asset_root, config, rng, scenery};

fn b(v: f32) -> String {
    format!("{:?}[{:08x}]", v, v.to_bits())
}
fn v3(v: Vec3) -> String {
    format!("({}, {}, {})", b(v.x), b(v.y), b(v.z))
}
fn q4(q: Quat) -> String {
    format!("({}, {}, {}, {})", b(q.x), b(q.y), b(q.z), b(q.w))
}

/// Entity kind code: P=player 0, R=road 1, O=obstacle 2, B=building 3, G=guardrail 4.
fn kind(world: &World, e: Entity, player: Option<Entity>) -> (u8, char, u32) {
    if Some(e) == player {
        (0, 'P', 0)
    } else if let Ok(v) = world.get::<&TrafficVariant>(e) {
        (2, 'O', v.0 as u32)
    } else if let Ok(v) = world.get::<&BuildingVariant>(e) {
        (3, 'B', v.0 as u32)
    } else if world.get::<&GuardrailTag>(e).is_ok() {
        (4, 'G', 0)
    } else {
        (1, 'R', 0)
    }
}

/// FNV-1a 64 over every entity with a TransformComponent, in hecs query order:
/// kind byte, variant u32 LE, position xyz, rotation xyzw, scale xyz (f32 bits LE).
fn digest(world: &World, player: Option<Entity>) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for &x in bytes {
            h ^= u64::from(x);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for (e, t) in world.query::<&TransformComponent>().iter() {
        let (code, _, variant) = kind(world, e, player);
        eat(&[code]);
        eat(&variant.to_le_bytes());
        let tr = t.transform;
        for f in [
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
            eat(&f.to_bits().to_le_bytes());
        }
    }
    h
}

fn dump_entities(out: &mut String, world: &World, player: Option<Entity>, limit: usize) {
    for (i, (e, t)) in world.query::<&TransformComponent>().iter().enumerate() {
        if i >= limit {
            let _ = writeln!(out, "  ... ({} entities total)", world.len());
            break;
        }
        let (_, k, variant) = kind(world, e, player);
        let tr = t.transform;
        let _ = writeln!(
            out,
            "  #{i:3} {k} id={:3} variant={variant} pos={} rot={} scale={}",
            e.id(),
            v3(tr.position),
            q4(tr.rotation),
            v3(tr.scale)
        );
    }
}

fn obstacles(world: &World) -> usize {
    world.query::<&Obstacle>().iter().count()
}

fn state_name(s: GameState) -> &'static str {
    match s {
        GameState::Ready => "Ready",
        GameState::Playing => "Playing",
        GameState::GameOver => "GameOver",
    }
}

fn frame_line(frame: u32, tap: Option<Key>, lp: &Loop, game: &CarRunner) -> String {
    format!(
        "f={frame:5} tap={:<6} state={:<8} player={} speed={} score={} best={} fade={} cam.pos={} cam.target={} streamed={} entities={} obstacles={} digest={:016x}",
        tap.map_or("-".to_string(), |k| format!("{k:?}")),
        state_name(game.state),
        v3(game.player_position()),
        b(game.current_speed()),
        game.score(),
        game.best_score(),
        b(game.fade_alpha()),
        v3(lp.camera.position()),
        v3(lp.camera.target()),
        lp.scene.streamed_count(),
        lp.scene.world().len(),
        obstacles(lp.scene.world()),
        digest(lp.scene.world(), game.player),
    )
}

// ---- record_demo's autopilot, verbatim (games/offramp/rust-impl/examples/record_demo.rs) ----
const REACTION_SECONDS: f32 = 0.45;
const MARGIN: f32 = 5.0;
const COOLDOWN: u32 = 12;

fn lane_of(x: f32) -> i32 {
    (x / config::LANE_WIDTH + (config::LANES as f32 - 1.0) / 2.0).round() as i32
}
fn nearest_in_lane(cars: &[(i32, f32)], lane: i32, behind: f32) -> f32 {
    cars.iter()
        .filter(|&&(l, ahead)| l == lane && ahead >= behind)
        .map(|&(_, ahead)| ahead)
        .fold(f32::INFINITY, f32::min)
}
fn steer(lp: &Loop, game: &CarRunner, speed: f32) -> Option<Key> {
    let player = game.player_position();
    let lane = lane_of(player.x);
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
        .map(|(l, key)| (nearest_in_lane(&cars, l, -6.0), key))
        .filter(|&(clear, _)| clear > 8.0)
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, key)| key)
}

/// Write the eight golden files to `out_dir`.
pub(crate) fn write(out_dir: &Path) {
    fs::create_dir_all(out_dir).unwrap();
    asset_root::init().expect("assets");

    // ---------------------------------------------------------------- vectors
    let mut s = String::new();
    let _ = writeln!(s, "# Lane PRNG: Rng::new(SEED = 0xC0FFEE0012345678)");
    let mut r = rng::Rng::new(config::SEED);
    let mut raw = rng::Rng::new(config::SEED);
    for i in 0..32 {
        let u = raw.next_u64();
        let lane = r.next_below(config::LANES as u32);
        let _ = writeln!(s, "draw {i:2}: next_u64={u:016x} next_below(3)={lane}");
    }
    let _ = writeln!(s, "\n# Per-slot scenery hashes");
    for slot in 0i64..=40 {
        let variant = rng::variant_for_slot(slot, config::TRAFFIC_CAR_ASSETS.len());
        let mut line = format!(
            "slot {slot:2}: hash_u64(slot)={:016x} traffic_variant={variant}",
            rng::hash_u64(slot as u64)
        );
        for side in 0..2u64 {
            let bv = rng::building_for_slot(slot, side);
            let jh = rng::hash_u64(slot as u64 ^ (side << 40) ^ 0xB57D);
            let jitter = (jh % 1000) as f32 / 1000.0;
            let sign = if side == 0 { -1.0f32 } else { 1.0 };
            let x = sign * (config::BUILDING_SIDE_X + jitter * config::BUILDING_SIDE_JITTER);
            let _ = write!(
                line,
                " | side{side}: building={bv} jitter={} x={}",
                b(jitter),
                b(x)
            );
        }
        let _ = writeln!(s, "{line}");
    }
    let _ = writeln!(
        s,
        "\n# Negative-slot checks (never reached in the demo, for the hash's u64 cast)"
    );
    for slot in [-1i64, -2, -7] {
        let _ = writeln!(
            s,
            "slot {slot}: traffic_variant={} building(0)={} building(1)={}",
            rng::variant_for_slot(slot, 3),
            rng::building_for_slot(slot, 0),
            rng::building_for_slot(slot, 1)
        );
    }
    let _ = writeln!(s, "\n# Constants");
    let _ = writeln!(s, "FIXED_DT = {}", b(FIXED_DT));
    let chase = ChaseController::new(config::CHASE_DISTANCE, config::CHASE_HEIGHT)
        .with_look_at_height(config::CHASE_LOOK_AT_HEIGHT)
        .with_half_life(config::CHASE_HALF_LIFE);
    let _ = writeln!(
        s,
        "chase follow_fraction(FIXED_DT) = {}",
        b(chase.follow_fraction(FIXED_DT))
    );
    let _ = writeln!(
        s,
        "-FIXED_DT/HALF_LIFE = {}",
        b(-FIXED_DT / config::CHASE_HALF_LIFE)
    );
    let _ = writeln!(
        s,
        "fov_y = 45.0f32.to_radians() = {}",
        b(45.0_f32.to_radians())
    );
    for sign in [-1.0f32, 1.0] {
        let angle = -sign * std::f32::consts::FRAC_PI_2;
        let _ = writeln!(
            s,
            "building facing sign={sign}: angle={} quat={}",
            b(angle),
            q4(Quat::from_rotation_y(angle))
        );
    }
    for lane in 0..3usize {
        let x = (lane as f32 - (config::LANES as f32 - 1.0) / 2.0) * config::LANE_WIDTH;
        let _ = writeln!(s, "lane_x({lane}) = {}", b(x));
    }
    let _ = writeln!(
        s,
        "\n# Speed ramp (current_speed at run_time = k * FIXED_DT accumulated)"
    );
    let mut run_time = 0.0f32;
    let mut travel = 0.0f32;
    for step in 1..=4000u32 {
        run_time += FIXED_DT;
        let factor = config::SPEED_RAMP.powf(run_time / config::SPEED_RAMP_SECONDS);
        let speed = (config::SPEED * factor).min(config::SPEED_MAX);
        travel += speed * FIXED_DT;
        if step <= 3 || step % 300 == 0 || step == 3676 || step == 3677 {
            let _ = writeln!(
                s,
                "step {step:4}: run_time={} speed={} travel={}",
                b(run_time),
                b(speed),
                b(travel)
            );
        }
    }
    fs::write(out_dir.join("vectors.txt"), s).unwrap();

    // ------------------------------------------------------- scenery meshes
    let fnv = |bytes: &[u8]| -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &x in bytes {
            h ^= u64::from(x);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    };
    let mut s = String::new();
    for (name, (bytes, indices)) in [
        ("guardrail_geometry(6.0)", scenery::guardrail_geometry(6.0)),
        ("terrain_geometry()", scenery::terrain_geometry()),
    ] {
        let idx_bytes: Vec<u8> = indices.iter().flat_map(|i| i.to_le_bytes()).collect();
        let _ = writeln!(
            s,
            "{name}: vertices={} floats={} indices={} fnv64(vertex bytes LE)={:016x} fnv64(index u32 LE)={:016x}",
            bytes.len() / 36,
            bytes.len() / 4,
            indices.len(),
            fnv(&bytes),
            fnv(&idx_bytes)
        );
        let floats: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|c| f32::from_ne_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        for v in [0usize, 1, 2, 3, 4, bytes.len() / 36 - 1] {
            let f = &floats[v * 9..v * 9 + 9];
            let _ = writeln!(
                s,
                "  v{v}: pos=({}, {}, {}) n=({}, {}, {}) c=({}, {}, {})",
                b(f[0]),
                b(f[1]),
                b(f[2]),
                b(f[3]),
                b(f[4]),
                b(f[5]),
                b(f[6]),
                b(f[7]),
                b(f[8])
            );
        }
        let _ = writeln!(s, "  indices[0..12]={:?}", &indices[..12]);
    }
    for x in [
        -130.0f32, -60.0, -13.0, 0.0, 12.0, 13.0, 20.0, 47.395832, 130.0,
    ] {
        let _ = writeln!(s, "terrain_height({x}) = {}", b(scenery::terrain_height(x)));
    }
    fs::write(out_dir.join("scenery.txt"), s).unwrap();

    // ------------------------------------------------------- init world dump
    let mut s = String::new();
    let mut lp = Loop::new();
    let mut renderer = NullRenderer::new();
    let mut game = CarRunner::new();
    lp.init_once(&mut game, &mut renderer);
    let cfg = lp.scene.config();
    let _ = writeln!(
        s,
        "after init_once: config axis={} spawn_interval={} spawn_ahead={} despawn_behind={} rebase_threshold={}",
        v3(cfg.axis), b(cfg.spawn_interval), b(cfg.spawn_ahead), b(cfg.despawn_behind), b(cfg.rebase_threshold)
    );
    let _ = writeln!(
        s,
        "camera pos={} target={} up={} clip={:?}",
        v3(lp.camera.position()),
        v3(lp.camera.target()),
        v3(lp.camera.up()),
        lp.camera.clip_planes()
    );
    let _ = writeln!(
        s,
        "streamed_count={} world.len={} obstacles={} digest={:016x}",
        lp.scene.streamed_count(),
        lp.scene.world().len(),
        obstacles(lp.scene.world()),
        digest(lp.scene.world(), game.player)
    );
    let _ = writeln!(
        s,
        "audio: sounds={} master={:?} looping={:?}",
        lp.audio.sound_count(),
        lp.audio.master_volume(),
        lp.audio.looping()
    );
    let _ = writeln!(s, "entities in hecs query order (archetype by archetype):");
    dump_entities(&mut s, lp.scene.world(), game.player, 400);
    fs::write(out_dir.join("init_world.txt"), s).unwrap();

    // ------------------------------------------------------------ smoke run
    // main.rs run_smoke: Space held for frame 0, released, then 119 more frames;
    // continued here to 900 frames, replaying on GameOver frames divisible by 60.
    let mut s = String::new();
    let mut lp = Loop::new();
    let mut renderer = NullRenderer::new();
    let mut game = CarRunner::new();
    lp.init_once(&mut game, &mut renderer);
    let mut held: Option<Key> = None;
    for frame in 0..900u32 {
        if let Some(k) = held.take() {
            lp.input.release_key(k);
        }
        let tap = if frame == 0 || (game.state == GameState::GameOver && frame % 60 == 0) {
            Some(Key::Space)
        } else {
            None
        };
        if let Some(k) = tap {
            lp.input.press_key(k);
            held = Some(k);
        }
        drive_frame(&mut lp, &mut game, &mut renderer, FIXED_DT_DURATION);
        let _ = writeln!(s, "{}", frame_line(frame, tap, &lp, &game));
    }
    let _ = writeln!(
        s,
        "audio: loop_starts={} one_shots={}",
        lp.audio.loop_starts(),
        lp.audio.one_shots_played()
    );
    fs::write(out_dir.join("smoke_trace.txt"), s).unwrap();

    // ------------------------------------------------ record_demo replica
    let mut s = String::new();
    let mut lp = Loop::new();
    let mut renderer = NullRenderer::new();
    lp.camera.set_aspect_ratio(1280.0 / 720.0);
    let mut game = CarRunner::new();
    lp.init_once(&mut game, &mut renderer);
    let mut held: Option<Key> = None;
    let mut cooldown = 0;
    let mut last_z = game.player_position().z;
    let mut speed = config::SPEED;
    let mut vp = String::new();
    for frame in 0..390u32 {
        if let Some(key) = held.take() {
            lp.input.release_key(key);
        }
        let tap = if frame == 0 {
            Some(Key::Space)
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
            }
        }
        drive_frame(&mut lp, &mut game, &mut renderer, FIXED_DT_DURATION);
        let z = game.player_position().z;
        let travelled = (last_z - z) / FIXED_DT_DURATION.as_secs_f32();
        if (0.0..=config::SPEED_MAX * 1.5).contains(&travelled) {
            speed = speed.max(travelled);
        }
        last_z = z;
        let _ = writeln!(
            s,
            "{} autopilot.speed={}",
            frame_line(frame, tap, &lp, &game),
            b(speed)
        );
        if [0u32, 1, 2, 149, 150, 151, 389].contains(&frame) {
            let m = lp.camera.view_projection_matrix().to_cols_array();
            let _ = writeln!(vp, "frame {frame} view_projection (column-major):");
            for c in 0..4 {
                let _ = writeln!(
                    vp,
                    "  col{c}: {} {} {} {}",
                    b(m[c * 4]),
                    b(m[c * 4 + 1]),
                    b(m[c * 4 + 2]),
                    b(m[c * 4 + 3])
                );
            }
        }
        if frame == 150 {
            let mut d = String::new();
            dump_entities(&mut d, lp.scene.world(), game.player, 400);
            fs::write(out_dir.join("record_frame150_world.txt"), d).unwrap();
        }
    }
    fs::write(out_dir.join("record_trace.txt"), s).unwrap();
    fs::write(out_dir.join("record_view_projection.txt"), vp).unwrap();

    // ------------------------------------------- long run across rebases
    let mut s = String::new();
    let mut lp = Loop::new();
    let mut renderer = NullRenderer::new();
    lp.camera.set_aspect_ratio(1280.0 / 720.0);
    let mut game = CarRunner::new();
    lp.init_once(&mut game, &mut renderer);
    let mut held: Option<Key> = None;
    let mut cooldown = 0;
    let mut last_z = game.player_position().z;
    let mut speed = config::SPEED;
    let mut max_entities = 0;
    for frame in 0..12000u32 {
        if let Some(key) = held.take() {
            lp.input.release_key(key);
        }
        let before_state = game.state;
        let tap = if frame == 0 || before_state == GameState::GameOver {
            Some(Key::Space)
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
            }
        }
        let cam_rel_before = lp.camera.position() - game.player_position();
        let z_before = game.player_position().z;
        drive_frame(&mut lp, &mut game, &mut renderer, FIXED_DT_DURATION);
        let z = game.player_position().z;
        max_entities = max_entities.max(lp.scene.world().len());
        let travelled = (last_z - z) / FIXED_DT_DURATION.as_secs_f32();
        if (0.0..=config::SPEED_MAX * 1.5).contains(&travelled) {
            speed = speed.max(travelled);
        }
        last_z = z;
        let rebased = z - z_before > 500.0;
        if before_state != game.state || rebased || frame % 600 == 0 {
            let note = if rebased {
                let rel = lp.camera.position() - game.player_position();
                format!(
                    " REBASE cam_rel_before={} cam_rel_after={}",
                    v3(cam_rel_before),
                    v3(rel)
                )
            } else if before_state != game.state {
                format!(
                    " TRANSITION {}->{}",
                    state_name(before_state),
                    state_name(game.state)
                )
            } else {
                String::new()
            };
            let _ = writeln!(s, "{}{note}", frame_line(frame, tap, &lp, &game));
        }
    }
    let _ = writeln!(
        s,
        "max entities={max_entities} loop_starts={} one_shots={}",
        lp.audio.loop_starts(),
        lp.audio.one_shots_played()
    );
    fs::write(out_dir.join("long_run_events.txt"), s).unwrap();
    println!("wrote golden vectors to {}", out_dir.display());
}
