// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! The probe: the headless tests' observed values (constants, the RNG, the font's
//! metrics, every HUD quad of the Ready, Playing and GameOver frames, the runs the
//! demo's tests make) as a report on stdout, `probe_stdout.txt`, with the game's
//! own "GAME OVER ..." and "replay ..." lines where it prints them; and
//! `smoke_state.json`, the `--smoke` run with the state `game.rs` keeps private,
//! read through the accessors `build.rs` appends to its copy of `game.rs`.

use std::fmt::Write as _;
use std::path::Path;

use crate::components::Obstacle;
use crate::game::{CarRunner, GameState};
use crate::{config, rng};
use stickshift_core::headless::Headless;
use stickshift_core::input::Key;
use stickshift_core::FIXED_DT;
use stickshift_ecs::TransformComponent;
use stickshift_math::glam::Vec3;
use stickshift_render_api::{OverlayFill, OverlayQuad, RenderDevice as _};

fn started() -> (CarRunner, Headless) {
    let mut game = CarRunner::new();
    let mut h = Headless::new();
    h.input_mut().press_key(Key::Space);
    h.run(&mut game, 1);
    h.input_mut().release_key(Key::Space);
    assert_eq!(game.state, GameState::Playing);
    (game, h)
}

fn hud_frame(game: &mut CarRunner, h: &mut Headless) -> Vec<OverlayQuad> {
    let before = h.renderer().overlay_quad_count();
    h.run(game, 1);
    h.renderer().overlay_quads()[before..].to_vec()
}

fn visible(font: &stickshift_render_api::FontAtlas, text: &str) -> usize {
    text.chars()
        .filter(|&c| {
            let g = font.glyph(c);
            g.size[0] > 0.0 && g.size[1] > 0.0
        })
        .count()
}

fn bits(v: f32) -> String {
    format!("{v:?} (0x{:08x})", v.to_bits())
}

fn v3(v: Vec3) -> String {
    format!("[{:?}, {:?}, {:?}]", v.x, v.y, v.z)
}

fn obstacle_layout(seed: u64) -> Vec<(i64, i64)> {
    let mut game = CarRunner::new();
    game.set_seed(seed);
    let mut h = Headless::new();
    h.input_mut().press_key(Key::Space);
    h.run(&mut game, 1);
    h.input_mut().release_key(Key::Space);
    h.run(&mut game, 120);
    let mut layout: Vec<(i64, i64)> = h
        .scene()
        .world()
        .query::<(&TransformComponent, &Obstacle)>()
        .iter()
        .map(|(_e, (t, _))| {
            let p = t.transform.position;
            ((p.x * 100.0).round() as i64, (p.z * 100.0).round() as i64)
        })
        .collect();
    layout.sort_unstable();
    println!(
        "  seed {seed:#x}: state {:?} after 121 frames, travel {}",
        game.state,
        bits(game.probe().travel)
    );
    layout
}

fn dodge(game: &mut CarRunner, h: &Headless) {
    let player = game.player_position();
    let mut blocked = [false; config::LANES];
    for (_e, (t, _)) in h.world().query::<(&TransformComponent, &Obstacle)>().iter() {
        let p = t.transform.position;
        if p.z < player.z + 2.0 && p.z > player.z - 14.0 {
            for (lane, slot) in blocked.iter_mut().enumerate() {
                if (p.x - CarRunner::lane_x_pub(lane)).abs() < 1.0 {
                    *slot = true;
                }
            }
        }
    }
    let lane = game.probe().lane;
    if blocked[lane] {
        if let Some(free) = (0..config::LANES).find(|&l| !blocked[l]) {
            game.set_lane(free);
        }
    }
}

/// Print the probe's report to stdout, the game's own messages among it, and
/// write `smoke_state.json` to `out_dir`.
pub(crate) fn run(out_dir: &Path) {
    // ---------------------------------------------------------------- constants
    println!("== constants");
    println!("FIXED_DT = {}", bits(FIXED_DT));
    let half = (config::HUD_FADE_SECONDS / (2.0 * FIXED_DT)).round() as u32;
    println!(
        "fade test half = {half} (raw {})",
        bits(config::HUD_FADE_SECONDS / (2.0 * FIXED_DT))
    );
    println!(
        "SPEED_MAX*FIXED_DT = {}",
        bits(config::SPEED_MAX * FIXED_DT)
    );
    let g = CarRunner::new();
    println!("current_speed(run_time 0) = {}", bits(g.current_speed()));
    let mut g2 = CarRunner::new();
    g2.set_run_time(config::SPEED_RAMP_SECONDS);
    println!(
        "current_speed(5s) = {}  expected {}",
        bits(g2.current_speed()),
        bits(config::SPEED * config::SPEED_RAMP)
    );
    g2.set_run_time(config::SPEED_RAMP_SECONDS * 2.0);
    println!(
        "current_speed(10s) = {}  expected {}",
        bits(g2.current_speed()),
        bits(config::SPEED * config::SPEED_RAMP * config::SPEED_RAMP)
    );
    g2.set_run_time(10_000.0);
    println!("current_speed(10000s) = {}", bits(g2.current_speed()));
    for l in 0..3 {
        println!("lane_x({l}) = {}", bits(CarRunner::lane_x_pub(l)));
    }

    // ---------------------------------------------------------------- rng
    println!("== rng");
    let mut r = rng::Rng::new(config::SEED);
    let lanes: Vec<u32> = (0..50).map(|_| r.next_below(3)).collect();
    println!("first 50 lanes (SEED) = {lanes:?}");
    let mut r = rng::Rng::new(config::SEED);
    let raw: Vec<String> = (0..4).map(|_| format!("{:#018x}", r.next_u64())).collect();
    println!("first 4 next_u64 (SEED) = {raw:?}");
    println!("hash_u64(0) = {:#018x}", rng::hash_u64(0));
    let v: Vec<usize> = (0..12).map(|s| rng::variant_for_slot(s, 3)).collect();
    println!("variant_for_slot(0..12, 3) = {v:?}");
    let b0: Vec<usize> = (0..12).map(|s| rng::building_for_slot(s, 0)).collect();
    let b1: Vec<usize> = (0..12).map(|s| rng::building_for_slot(s, 1)).collect();
    println!("building_for_slot(0..12, 0) = {b0:?}");
    println!("building_for_slot(0..12, 1) = {b1:?}");

    // ---------------------------------------------------------------- init
    println!("== init (0 frames)");
    let mut game = CarRunner::new();
    let h = stickshift_core::headless::run(&mut game, 0);
    let scene = h.scene();
    println!("streamed_count = {}", scene.streamed_count());
    println!("world.len = {}", scene.world().len());
    let axis = scene.config().axis;
    let player = game.player_position().dot(axis);
    let farthest = scene
        .world()
        .query::<&TransformComponent>()
        .iter()
        .map(|(_, t)| t.transform.position.dot(axis) - player)
        .fold(f32::MIN, f32::max);
    println!("farthest ahead = {}", bits(farthest));
    println!("config = {:?}", scene.config());
    println!("clip planes = {:?}", h.camera().clip_planes());
    println!(
        "camera pos {} target {}",
        v3(h.camera().position()),
        v3(h.camera().target())
    );
    let sun = h.renderer().sun_sky().unwrap();
    println!("sun = {sun:?}");
    println!("sun.direction = {}", v3(sun.direction()));
    println!(
        "created meshes {} textures {} pipelines {}",
        h.renderer().created_meshes().len(),
        h.renderer().created_textures().len(),
        h.renderer().created_pipelines().len()
    );
    println!("render_settings = {:?}", h.renderer().render_settings());
    println!(
        "audio: silent {} sounds {} decodes {} master {:?} looping {:?}",
        h.audio().is_silent(),
        h.audio().sound_count(),
        h.audio().decode_count(),
        h.audio().master_volume(),
        h.audio().looping()
    );
    println!(
        "frames_begun {} overlay quads {}",
        h.renderer().frames_begun(),
        h.renderer().overlay_quad_count()
    );

    // ---------------------------------------------------------------- started
    println!("== started()");
    let (game, h) = started();
    let p = game.probe();
    println!(
        "world.len = {} streamed {}",
        h.scene().world().len(),
        h.scene().streamed_count()
    );
    println!("probe = {p:?}  fade_alpha {}", bits(game.fade_alpha()));
    println!(
        "camera pos {} target {}",
        v3(h.camera().position()),
        v3(h.camera().target())
    );
    println!(
        "audio looping {:?} loop_starts {} one_shots {}",
        h.audio().looping(),
        h.audio().loop_starts(),
        h.audio().one_shots_played()
    );
    println!(
        "draws {} overlay {} frames {}",
        h.renderer().draw_count(),
        h.renderer().overlay_quad_count(),
        h.renderer().frames_submitted()
    );

    // ---------------------------------------------------------------- frozen
    println!("== opens frozen");
    let mut game = CarRunner::new();
    let mut h = Headless::new();
    h.run(&mut game, 30);
    println!(
        "after 30 Ready frames: {:?} camera {}",
        game.probe(),
        v3(h.camera().position())
    );

    // ---------------------------------------------------------------- fade
    println!("== fade");
    let (mut game, mut h) = started();
    let mut alphas = Vec::new();
    let mut n = 0;
    loop {
        h.run(&mut game, 1);
        n += 1;
        alphas.push(game.fade_alpha());
        if game.fade_alpha() == 0.0 || n > 100 {
            break;
        }
    }
    println!(
        "fade retires after {n} Playing frames (state {:?})",
        game.state
    );
    println!("first alphas: {:?}", &alphas[..5]);
    println!(
        "alpha after {half} frames = {}",
        bits(alphas[half as usize - 1])
    );
    println!(
        "alphas last 3 = {:?}",
        &alphas[alphas.len().saturating_sub(3)..]
    );

    // ---------------------------------------------------------------- drive_to_game_over
    println!("== drive_to_game_over (game.rs)");
    let (mut game, mut h) = started();
    let mut k = 0;
    for _ in 0..600 {
        h.run(&mut game, 1);
        k += 1;
        if game.state == GameState::GameOver {
            break;
        }
    }
    let p = game.probe();
    println!(
        "GameOver after {k} frames past started(); total frames_run {}",
        h.frames_run()
    );
    println!(
        "  lane {} travel {} distance {} best {} run_time {} score {} best_score {} speed {}",
        p.lane,
        bits(p.travel),
        bits(p.distance),
        bits(p.best),
        bits(p.run_time),
        game.score(),
        game.best_score(),
        bits(game.current_speed())
    );
    println!(
        "  audio loop_starts {} one_shots {}",
        h.audio().loop_starts(),
        h.audio().one_shots_played()
    );
    // Replay
    h.input_mut().press_key(Key::Space);
    h.run(&mut game, 1);
    h.input_mut().release_key(Key::Space);
    let p = game.probe();
    println!(
        "  after replay frame: state {:?} lane {} travel {} distance {} run_time {}",
        game.state,
        p.lane,
        bits(p.travel),
        bits(p.distance),
        bits(p.run_time)
    );
    let mut k2 = 0;
    for _ in 0..600 {
        h.run(&mut game, 1);
        k2 += 1;
        if game.state == GameState::GameOver {
            break;
        }
    }
    let p = game.probe();
    println!(
        "  second crash after {k2} more frames: score {} best {} travel {} loop_starts {} one_shots {}",
        game.score(),
        game.best_score(),
        bits(p.travel),
        h.audio().loop_starts(),
        h.audio().one_shots_played()
    );

    // ---------------------------------------------------------------- smoke
    println!("== smoke trace");
    let mut game = CarRunner::new();
    let mut h = Headless::new();
    let mut trace = String::from("[\n");
    for frame in 0..120u32 {
        if frame == 0 {
            h.input_mut().press_key(Key::Space);
        }
        h.run(&mut game, 1);
        if frame == 0 {
            h.input_mut().release_key(Key::Space);
        }
        let p = game.probe();
        let cam = h.camera();
        let obstacles = h.world().query::<&Obstacle>().iter().count();
        let _ = writeln!(
            trace,
            "  {{\"frame\": {}, \"state\": \"{:?}\", \"lane\": {}, \"travel\": {:?}, \"travel_bits\": \"0x{:08x}\", \"distance\": {:?}, \"distance_bits\": \"0x{:08x}\", \"run_time\": {:?}, \"fade_remaining\": {:?}, \"score\": {}, \"entities\": {}, \"streamed\": {}, \"obstacles\": {}, \"camera_position\": {}, \"camera_target\": {}, \"camera_position_bits\": [\"0x{:08x}\", \"0x{:08x}\", \"0x{:08x}\"]}}{}",
            frame + 1,
            game.state,
            p.lane,
            p.travel,
            p.travel.to_bits(),
            p.distance,
            p.distance.to_bits(),
            p.run_time,
            p.fade_remaining,
            game.score(),
            h.world().len(),
            h.scene().streamed_count(),
            obstacles,
            v3(cam.position()),
            v3(cam.target()),
            cam.position().x.to_bits(),
            cam.position().y.to_bits(),
            cam.position().z.to_bits(),
            if frame + 1 < 120 { "," } else { "" }
        );
    }
    trace.push_str("]\n");
    std::fs::create_dir_all(out_dir).unwrap();
    std::fs::write(out_dir.join("smoke_state.json"), &trace).unwrap();
    println!(
        "smoke frames_run {} final state {:?} score {} best {}",
        h.frames_run(),
        game.state,
        game.score(),
        game.best_score()
    );
    println!(
        "  renderer: frames_begun {} submitted {} draws {} overlay {} violations {}",
        h.renderer().frames_begun(),
        h.renderer().frames_submitted(),
        h.renderer().draw_count(),
        h.renderer().overlay_quad_count(),
        h.renderer().protocol_violations().len()
    );

    // ---------------------------------------------------------------- obstacle layouts
    println!("== obstacle_layout");
    let a = obstacle_layout(config::SEED);
    println!("  SEED layout ({}): {a:?}", a.len());
    let b = obstacle_layout(config::SEED ^ 0x9E37_79B9_7F4A_7C15);
    println!("  ALT layout ({}): {b:?}", b.len());

    // ---------------------------------------------------------------- dodging
    println!("== dodging run (20000 steps)");
    let (mut game, mut h) = started();
    let mut rebases = 0u32;
    let mut first_rebase = None;
    let mut ended = None;
    for step in 0..20_000u32 {
        dodge(&mut game, &h);
        let before_rel = h.camera().position() - game.player_position();
        let travel_before = game.probe().travel;
        h.run(&mut game, 1);
        if game.probe().rebased {
            rebases += 1;
            if first_rebase.is_none() {
                let car = game.player_position();
                let cam = h.camera().position();
                let gap = (cam - car).length();
                let jolt = ((cam - car) - before_rel).length();
                let per_step = game.current_speed() * FIXED_DT;
                first_rebase = Some((
                    step,
                    travel_before,
                    game.probe().travel,
                    gap,
                    jolt,
                    per_step,
                ));
            }
        }
        if game.state == GameState::GameOver && ended.is_none() {
            ended = Some(step);
            break;
        }
    }
    let p = game.probe();
    println!("  rebases {rebases}; first {first_rebase:?}; ended {ended:?}");
    println!(
        "  final travel {} distance {} speed {} run_time {} state {:?}",
        bits(p.travel),
        bits(p.distance),
        bits(game.current_speed()),
        bits(p.run_time),
        game.state
    );

    // ---------------------------------------------------------------- streaming bounded
    println!("== streaming bounded (5000)");
    let (mut game, mut h) = started();
    let start = h.scene().world().len();
    let (mut rebases, mut max_e, mut max_o, mut replays) = (0u32, 0u32, 0usize, 0u32);
    for _ in 0..5000 {
        if game.state == GameState::GameOver {
            replays += 1;
            h.input_mut().press_key(Key::Space);
            h.run(&mut game, 1);
            h.input_mut().release_key(Key::Space);
        }
        h.run(&mut game, 1);
        rebases += u32::from(game.probe().rebased);
        max_e = max_e.max(h.scene().world().len());
        max_o = max_o.max(h.scene().world().query::<&Obstacle>().iter().count());
    }
    println!(
        "  start {start} max_e {max_e} max_o {max_o} rebases {rebases} replays {replays} travel {} frames_run {} violations {}",
        bits(game.probe().travel),
        h.frames_run(),
        h.renderer().protocol_violations().len()
    );

    // ---------------------------------------------------------------- HUD
    println!("== HUD");
    let mut game = CarRunner::new();
    let mut h = Headless::new();
    h.run(&mut game, 0);
    {
        let font = game.font.as_ref().unwrap();
        println!(
            "font: first_char {:?} glyphs {} line_height {} fallback {:?} texture {:?}",
            font.first_char,
            font.glyphs.len(),
            bits(font.line_height),
            font.fallback,
            font.texture
        );
        println!("created textures {:?}", h.renderer().created_textures());
        for s in [
            "SCORE 0",
            "STICKSHIFT RUNNER",
            "PRESS SPACE TO START",
            "GAME OVER",
            "PRESS SPACE TO REPLAY",
            "SCORE 23",
            "BEST 23",
        ] {
            println!(
                "  visible({s:?}) = {}  measure@30 = {}  measure@64 = {}",
                visible(font, s),
                bits(font.measure(s, 30.0)),
                bits(font.measure(s, 64.0))
            );
        }
        let sg = font.glyph('S');
        println!("  glyph 'S' = {sg:?}");
        println!("  glyph ' ' = {:?}", font.glyph(' '));
    }
    let quads = hud_frame(&mut game, &mut h);
    println!("Ready frame quads: {}", quads.len());
    for (i, q) in quads.iter().enumerate() {
        println!(
            "  [{i}] rect [{:?}, {:?}, {:?}, {:?}] color {:?} fill {}",
            q.rect[0],
            q.rect[1],
            q.rect[2],
            q.rect[3],
            q.color,
            match q.fill {
                OverlayFill::Solid => "Solid".to_string(),
                OverlayFill::Sdf(t) => format!("Sdf({})", t.0),
                OverlayFill::Textured(t) => format!("Textured({})", t.0),
            }
        );
    }

    // Retina scale
    let mut game = CarRunner::new();
    let mut h = Headless::new();
    h.run(&mut game, 0);
    h.renderer_mut().resize_surface(2560, 1440, 2.0);
    let q2 = hud_frame(&mut game, &mut h);
    println!(
        "Ready frame at scale 2: {} quads; first glyph rect {:?}",
        q2.len(),
        q2[1].rect
    );

    // Fade frames HUD
    let (mut game, mut h) = started();
    for f in 0..3 {
        let q = hud_frame(&mut game, &mut h);
        println!(
            "fade frame {f}: {} quads, wash color {:?}, score {}",
            q.len(),
            q[0].color,
            game.score()
        );
    }
    // past the fade
    let (mut game, mut h) = started();
    let mut frames = 0;
    let quads = loop {
        let q = hud_frame(&mut game, &mut h);
        frames += 1;
        if game.fade_alpha() == 0.0 {
            break q;
        }
    };
    println!(
        "first fade-free Playing frame after {frames} frames: {} quads, score {}, state {:?}",
        quads.len(),
        game.score(),
        game.state
    );
    for (i, q) in quads.iter().enumerate() {
        println!("  [{i}] rect {:?}", q.rect);
    }

    // Game over HUD
    let (mut game, mut h) = started();
    let mut playing_len = 0;
    let mut frames = 0;
    let quads = loop {
        let q = hud_frame(&mut game, &mut h);
        frames += 1;
        if game.state == GameState::GameOver {
            break q;
        }
        playing_len = q.len();
    };
    println!(
        "GameOver frame after {frames} frames: {} quads (prev Playing {}), score {} best {}",
        quads.len(),
        playing_len,
        game.score(),
        game.best_score()
    );
    for (i, q) in quads.iter().enumerate() {
        println!("  [{i}] rect {:?} color {:?}", q.rect, q.color);
    }
}
