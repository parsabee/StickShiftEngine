// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! The atmosphere, against the real Metal backend: its visibility draws the
//! distance fog in on top of the draw-distance setting, and the sky pass draws
//! its rainbow and its bolt of lightning where they are, and nothing else. A
//! clear atmosphere draws exactly the frame drawn without one, which the pixel
//! hashes pin for every reference scene.
//!
//! Like the other backend tests these **skip** when no Metal device is
//! available (GPU-less CI) and run + assert on a real Mac.

use stickshift_math::glam::{Mat4, Vec3};
use stickshift_render::MetalRenderer;
use stickshift_render_api::{
    Atmosphere, Bolt, FrameRecorder, Rainbow, RenderDevice, RenderSettings, SunSky,
};

mod common;

/// Offscreen render size: wide, like a game's view.
const WIDTH: u32 = 160;
/// Offscreen render size.
const HEIGHT: u32 = 90;

/// A backend looking due north (`-Z`) along the horizon, which runs across the
/// middle of the frame, under a plain daytime sun behind it to the south.
fn renderer() -> Option<MetalRenderer> {
    let Some(mut r) = MetalRenderer::new_offscreen(WIDTH, HEIGHT) else {
        common::no_gpu();
        return None;
    };
    let eye = Vec3::new(0.0, 0.0, 0.0);
    let view = Mat4::look_at_rh(eye, Vec3::NEG_Z, Vec3::Y);
    let projection =
        Mat4::perspective_rh(60f32.to_radians(), WIDTH as f32 / HEIGHT as f32, 0.3, 300.0);
    r.set_view_projection(projection * view);
    r.set_camera_position(eye);
    r.set_sun_sky(&SunSky {
        sun_elevation_deg: 30.0,
        sun_azimuth_deg: 180.0,
        ..SunSky::default()
    });
    Some(r)
}

/// One frame of sky alone under `atmosphere`, read back (BGRA8, row-major).
fn sky_under(r: &mut MetalRenderer, atmosphere: &Atmosphere) -> Vec<u8> {
    r.set_atmosphere(atmosphere);
    r.begin_frame();
    r.submit();
    r.read_pixels().expect("offscreen")
}

/// The pixel at `(x, y)` as `[b, g, r]`.
fn pixel(frame: &[u8], x: u32, y: u32) -> [u8; 3] {
    let i = ((y * WIDTH + x) * 4) as usize;
    [frame[i], frame[i + 1], frame[i + 2]]
}

/// The rows of the frame, top to bottom, on which any pixel differs between
/// `a` and `b`.
fn rows_differing(a: &[u8], b: &[u8]) -> Vec<u32> {
    (0..HEIGHT)
        .filter(|&y| (0..WIDTH).any(|x| pixel(a, x, y) != pixel(b, x, y)))
        .collect()
}

#[test]
fn visibility_draws_the_fog_in_on_top_of_the_draw_distance_setting() {
    let Some(mut r) = renderer() else { return };
    // The fog a frame captures in clear air is the setting's own.
    let frame_fog = |r: &mut MetalRenderer| {
        r.begin_frame();
        r.submit();
        let light = r.light_for_test();
        (light.fog_start, light.fog_density)
    };
    let (start, density) = frame_fog(&mut r);
    for (setting, visibility) in [(1.0, 0.5), (0.8, 0.5), (0.5, 0.25), (1.0, 1.0)] {
        r.set_render_settings(&RenderSettings {
            draw_distance_scale: setting,
            ..RenderSettings::default()
        });
        r.set_atmosphere(&Atmosphere {
            visibility,
            ..Atmosphere::default()
        });
        let (s, d) = frame_fog(&mut r);
        let scale = setting * visibility;
        assert!((s - start * scale).abs() < 1e-3, "start {s} at {scale}");
        assert!((d - density / scale).abs() < 1e-5, "density {d} at {scale}");
    }
    // Back in clear air with the default setting, exactly where it began.
    r.set_render_settings(&RenderSettings::default());
    r.set_atmosphere(&Atmosphere::default());
    assert_eq!(frame_fog(&mut r), (start, density));
}

#[test]
fn a_clear_atmosphere_draws_the_sky_it_drew_before() {
    let Some(mut r) = renderer() else { return };
    let before = {
        r.begin_frame();
        r.submit();
        r.read_pixels().expect("offscreen")
    };
    assert_eq!(sky_under(&mut r, &Atmosphere::default()), before);
    // A rainbow of no strength and a bolt with no light draw nothing either.
    let unseen = Atmosphere {
        rainbow: Some(Rainbow {
            elevation_deg: -20.0,
            azimuth_deg: 0.0,
            strength: 0.0,
        }),
        bolt: Some(Bolt {
            azimuth_deg: 0.0,
            height_deg: 20.0,
            seed: 3,
            light: [0.0; 3],
        }),
        ..Atmosphere::default()
    };
    assert_eq!(sky_under(&mut r, &unseen), before);
}

#[test]
fn a_rainbow_stands_above_the_horizon_red_outside_violet_in() {
    let Some(mut r) = renderer() else { return };
    let clear = sky_under(&mut r, &Atmosphere::default());
    // Centred due north, 30° below the horizon: its primary's top is 12° up,
    // above the middle of the frame (which looks along the horizon), and its
    // secondary's top 21° up.
    let rainbow = Atmosphere {
        rainbow: Some(Rainbow {
            elevation_deg: -30.0,
            azimuth_deg: 0.0,
            strength: 1.0,
        }),
        ..Atmosphere::default()
    };
    let bow = sky_under(&mut r, &rainbow);
    let rows = rows_differing(&clear, &bow);
    assert!(!rows.is_empty(), "the bow is drawn");
    let horizon = HEIGHT / 2;
    assert!(
        rows.iter().all(|&y| y <= horizon),
        "only above the horizon: rows {rows:?}"
    );

    // Down the middle column, where the bow crosses straight overhead of its
    // centre: going up the frame, out from the centre, violet comes before red.
    let x = WIDTH / 2;
    let added = |y: u32| {
        let (c, b) = (pixel(&clear, x, y), pixel(&bow, x, y));
        [
            b[0] as i32 - c[0] as i32,
            b[1] as i32 - c[1] as i32,
            b[2] as i32 - c[2] as i32,
        ]
    };
    let column: Vec<u32> = (0..horizon)
        .rev()
        .filter(|&y| added(y) != [0, 0, 0])
        .collect();
    assert!(!column.is_empty(), "the bow crosses the middle column");
    // Its reddest row is further out (higher, a smaller row) than its bluest.
    let reddest = *column
        .iter()
        .max_by_key(|&&y| added(y)[2] - added(y)[0])
        .expect("rows");
    let bluest = *column
        .iter()
        .filter(|&&y| y > 2)
        .max_by_key(|&&y| added(y)[0] - added(y)[2])
        .expect("rows");
    assert!(
        reddest < bluest,
        "red row {reddest} outside blue row {bluest}"
    );

    // Half as strong, it adds less.
    let faint = sky_under(
        &mut r,
        &Atmosphere {
            rainbow: rainbow.rainbow.map(|b| Rainbow { strength: 0.5, ..b }),
            ..Atmosphere::default()
        },
    );
    let total = |frame: &[u8]| -> i64 {
        frame
            .iter()
            .zip(&clear)
            .map(|(&a, &b)| (a as i64 - b as i64).abs())
            .sum()
    };
    assert!(total(&faint) < total(&bow) && total(&faint) > 0);
}

#[test]
fn a_bolt_of_lightning_lights_the_sky_along_its_bearing_and_nowhere_else() {
    let Some(mut r) = renderer() else { return };
    let clear = sky_under(&mut r, &Atmosphere::default());
    let bolt = |azimuth_deg: f32, seed: u32| Atmosphere {
        bolt: Some(Bolt {
            azimuth_deg,
            height_deg: 25.0,
            seed,
            light: [6.0, 6.0, 7.0],
        }),
        ..Atmosphere::default()
    };
    // Due north, in front of the camera: drawn, brightest near the middle
    // column, from the horizon up.
    let north = sky_under(&mut r, &bolt(0.0, 5));
    let rows = rows_differing(&clear, &north);
    assert!(!rows.is_empty(), "the bolt is drawn");
    let horizon = HEIGHT / 2;
    let brightest = |frame: &[u8], y: u32| {
        (0..WIDTH)
            .max_by_key(|&x| pixel(frame, x, y).iter().map(|&c| c as u32).sum::<u32>())
            .expect("columns")
    };
    let y = horizon - 4;
    let x = brightest(&north, y);
    assert!(
        (x as i32 - (WIDTH / 2) as i32).abs() < (WIDTH / 4) as i32,
        "the bolt stands near the bearing it was given, not column {x}"
    );
    assert!(
        pixel(&north, x, y).iter().all(|&c| c > 200),
        "it glows white"
    );
    // Due south, behind the camera: nothing in front of it changes.
    assert_eq!(
        sky_under(&mut r, &bolt(180.0, 5)),
        clear,
        "behind the camera"
    );
    // Another seed is another bolt.
    assert_ne!(sky_under(&mut r, &bolt(0.0, 6)), north);
}
