// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! GPU particles, end to end on the real Metal backend: the compute kernels
//! spawn and move them, and the scene pass draws them.
//!
//! Each test renders small offscreen frames and checks a property of the
//! pixels: particles light the frame where they are and nowhere else, hide
//! behind the scene, die on time, bounce off a floor, streak along their motion,
//! cover with alpha, move with a shift, and draw the same frame for the same
//! calls. The planning half (what spawns, which slots are live) is unit-tested
//! in `src/particles.rs`.
//!
//! Like the other backend tests these **skip** when no Metal device is available
//! (GPU-less CI) and run + assert on a real Mac.

use stickshift_math::glam::{Mat4, Vec3};
use stickshift_math::Transform;
use stickshift_render::MetalRenderer;
use stickshift_render_api::{
    EmitterHandle, EmitterParams, Floor, FrameRecorder, MaterialParams, MeshData, MeshHandle,
    ParticleBlend, ParticleSystem, PipelineDescriptor, PipelineHandle, PipelineKind, RenderDevice,
};

mod common;

/// Offscreen render size.
const SIZE: u32 = 64;
/// One fixed step.
const DT: f32 = 1.0 / 60.0;

/// A renderer looking at the origin from 5 units down `+Z`, or `None` (having
/// reported the skip) without a Metal device.
fn renderer() -> Option<MetalRenderer> {
    let Some(mut r) = MetalRenderer::new_offscreen(SIZE, SIZE) else {
        common::no_gpu();
        return None;
    };
    let eye = Vec3::new(0.0, 0.0, 5.0);
    r.set_view_projection(
        Mat4::perspective_rh(60f32.to_radians(), 1.0, 0.1, 100.0)
            * Mat4::look_at_rh(eye, Vec3::ZERO, Vec3::Y),
    );
    r.set_camera_position(eye);
    Some(r)
}

/// Render one frame, with a wall (a unit quad at `z = 0.5`, facing the camera)
/// if given, and read it back.
fn frame(r: &mut MetalRenderer, wall: Option<(PipelineHandle, MeshHandle)>) -> Vec<u8> {
    r.begin_frame();
    if let Some((pipeline, mesh)) = wall {
        r.set_pipeline(pipeline);
        r.draw_mesh(mesh, &Transform::identity(), &MaterialParams::default());
    }
    r.submit();
    r.read_pixels().expect("an offscreen frame")
}

/// Advance the particles a step and render it.
fn step(r: &mut MetalRenderer) -> Vec<u8> {
    r.advance_particles(DT);
    frame(r, None)
}

/// The wall: a unit quad at `z = 0.5`, facing `+Z`.
fn wall(r: &mut MetalRenderer) -> (PipelineHandle, MeshHandle) {
    let c = [0.2, 0.3, 0.8];
    let v = |p: [f32; 3]| [p[0], p[1], p[2], 0.0, 0.0, 1.0, c[0], c[1], c[2]];
    let verts = [
        v([-0.5, -0.5, 0.5]),
        v([0.5, -0.5, 0.5]),
        v([0.5, 0.5, 0.5]),
        v([-0.5, 0.5, 0.5]),
    ];
    let bytes: Vec<u8> = verts
        .iter()
        .flatten()
        .flat_map(|f: &f32| f.to_ne_bytes())
        .collect();
    let mesh = r.create_mesh(&MeshData {
        vertices: &bytes,
        indices: &[0, 1, 2, 0, 2, 3],
        layout: PipelineKind::VertexColor.vertex_layout(),
    });
    let pipeline = r.create_pipeline(&PipelineDescriptor::new(PipelineKind::VertexColor));
    (pipeline, mesh)
}

/// Bright, still, long-lived additive points.
fn glow() -> EmitterParams {
    EmitterParams {
        speed: [0.0, 0.0],
        lifetime: [10.0, 10.0],
        size: [0.6, 0.6],
        color: [[4.0, 4.0, 4.0, 1.0], [4.0, 4.0, 4.0, 1.0]],
        ..EmitterParams::default()
    }
}

/// An emitter of `params` placed at `at`.
fn emitter(r: &mut MetalRenderer, params: &EmitterParams, at: Vec3) -> EmitterHandle {
    let e = r.create_emitter(params);
    r.place_emitter(e, &Transform::from_position(at), Vec3::ZERO);
    e
}

/// The pixel at `(x, y)`, BGRA.
fn pixel(frame: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * SIZE + x) * 4) as usize;
    [frame[i], frame[i + 1], frame[i + 2], frame[i + 3]]
}

/// The brightness of a pixel: its three colour bytes summed.
fn brightness(frame: &[u8], x: u32, y: u32) -> u32 {
    pixel(frame, x, y)[..3].iter().map(|&b| u32::from(b)).sum()
}

/// The pixels of `frame` brighter than `base`'s by more than a rounding step.
fn lit(frame: &[u8], base: &[u8]) -> Vec<(u32, u32)> {
    (0..SIZE)
        .flat_map(|y| (0..SIZE).map(move |x| (x, y)))
        .filter(|&(x, y)| brightness(frame, x, y) > brightness(base, x, y) + 6)
        .collect()
}

#[test]
fn a_burst_lights_the_frame_where_its_particles_are_and_nowhere_else() {
    let Some(mut r) = renderer() else { return };
    let base = frame(&mut r, None);
    let e = emitter(&mut r, &glow(), Vec3::ZERO);
    r.emit(e, 50);
    let shown = step(&mut r);
    // The sky there is already pale: the particles take it to white.
    assert!(
        brightness(&shown, 32, 32) > brightness(&base, 32, 32) + 100,
        "the middle lights up: {:?} over {:?}",
        pixel(&shown, 32, 32),
        pixel(&base, 32, 32)
    );
    for (x, y) in [(2, 2), (61, 2), (2, 61), (61, 61)] {
        assert_eq!(
            pixel(&shown, x, y),
            pixel(&base, x, y),
            "the corners do not"
        );
    }
    assert_eq!(
        r.allocation_count(),
        6,
        "nothing allocated past construction"
    );
}

#[test]
fn particles_behind_the_scene_are_hidden_and_in_front_of_it_are_not() {
    let Some(mut r) = renderer() else { return };
    let wall = wall(&mut r);
    let bare = frame(&mut r, Some(wall));
    let behind = emitter(&mut r, &glow(), Vec3::ZERO);
    r.emit(behind, 50);
    r.advance_particles(DT);
    let hidden = frame(&mut r, Some(wall));
    assert_eq!(hidden, bare, "the wall hides the particles behind it");

    let front = emitter(&mut r, &glow(), Vec3::new(0.0, 0.0, 2.0));
    r.emit(front, 50);
    r.advance_particles(DT);
    let shown = frame(&mut r, Some(wall));
    assert!(
        brightness(&shown, 32, 32) > brightness(&bare, 32, 32) + 150,
        "the particles in front of it light it"
    );
}

#[test]
fn particles_die_at_the_end_of_their_lives() {
    let Some(mut r) = renderer() else { return };
    let base = frame(&mut r, None);
    let e = emitter(
        &mut r,
        &EmitterParams {
            lifetime: [0.5, 0.5],
            ..glow()
        },
        Vec3::ZERO,
    );
    r.emit(e, 50);
    assert!(!lit(&step(&mut r), &base).is_empty(), "alive");
    for _ in 0..20 {
        r.advance_particles(DT);
    }
    assert!(
        !lit(&step(&mut r), &base).is_empty(),
        "still alive at a third of a second"
    );
    r.advance_particles(0.5);
    assert_eq!(step(&mut r), base, "gone at half a second");
}

#[test]
fn a_floor_holds_falling_particles_up() {
    let fall = |floor: Option<Floor>| {
        let mut r = renderer()?;
        let base = frame(&mut r, None);
        let params = EmitterParams {
            gravity: Vec3::new(0.0, -10.0, 0.0),
            size: [0.3, 0.3],
            floor,
            ..glow()
        };
        let e = emitter(&mut r, &params, Vec3::new(0.0, 0.5, 0.0));
        r.emit(e, 20);
        let mut last = Vec::new();
        for _ in 0..60 {
            last = step(&mut r);
        }
        Some(lit(&last, &base))
    };
    let (Some(held), Some(fallen)) = (
        fall(Some(Floor {
            height: 0.0,
            bounce: 0.3,
            friction: 0.5,
        })),
        fall(None),
    ) else {
        return;
    };
    assert!(fallen.is_empty(), "without a floor they fall out of view");
    assert!(!held.is_empty(), "a floor holds them up");
    let lowest = held.iter().map(|&(_, y)| y).max().unwrap_or(0);
    assert!(
        (30..=40).contains(&lowest),
        "resting on the floor, in the middle of the view: row {lowest}"
    );
}

#[test]
fn the_same_calls_draw_the_same_frames() {
    let run = || {
        let mut r = renderer()?;
        let base = frame(&mut r, None);
        let e = emitter(
            &mut r,
            &EmitterParams {
                rate: 200.0,
                speed: [0.5, 2.0],
                spread_deg: 60.0,
                radius: 0.2,
                lifetime: [0.4, 1.2],
                gravity: Vec3::new(0.0, -2.0, 0.0),
                drag: 0.5,
                size: [0.15, 0.02],
                color: [[3.0, 1.5, 0.4, 1.0], [1.0, 0.2, 0.0, 0.0]],
                stretch: 0.05,
                ..EmitterParams::default()
            },
            Vec3::new(-1.0, 0.0, 0.0),
        );
        let mut last = Vec::new();
        for i in 0..30 {
            let at = Vec3::new(-1.0 + i as f32 * 0.07, 0.0, 0.0);
            r.place_emitter(e, &Transform::from_position(at), Vec3::new(4.0, 0.0, 0.0));
            if i % 10 == 0 {
                r.emit(e, 25);
            }
            last = step(&mut r);
        }
        Some((last, base))
    };
    let (Some((first, base)), Some((second, _))) = (run(), run()) else {
        return;
    };
    assert!(lit(&first, &base).len() > 20, "the effect shows");
    assert_eq!(first, second, "and draws the same pixels every time");
}

#[test]
fn a_streak_draws_longer_than_a_point() {
    let lit_by = |stretch: f32| {
        let mut r = renderer()?;
        let base = frame(&mut r, None);
        let params = EmitterParams {
            speed: [6.0, 6.0],
            direction: Vec3::X,
            spread_deg: 0.0,
            size: [0.15, 0.15],
            stretch,
            ..glow()
        };
        let e = emitter(&mut r, &params, Vec3::new(-0.5, 0.0, 0.0));
        r.emit(e, 1);
        Some(lit(&step(&mut r), &base).len())
    };
    let (Some(point), Some(streak)) = (lit_by(0.0), lit_by(0.25)) else {
        return;
    };
    assert!(point > 0, "the point shows");
    assert!(
        streak > point * 3,
        "a streak covers more: {streak} against {point}"
    );
}

#[test]
fn alpha_particles_cover_what_is_behind_them() {
    let Some(mut r) = renderer() else { return };
    let base = frame(&mut r, None);
    let soot = EmitterParams {
        color: [[0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 0.0, 1.0]],
        blend: ParticleBlend::Alpha,
        ..glow()
    };
    let e = emitter(&mut r, &soot, Vec3::ZERO);
    r.emit(e, 50);
    let covered = step(&mut r);
    assert!(
        brightness(&covered, 32, 32) + 100 < brightness(&base, 32, 32),
        "black smoke darkens the sky behind it: {:?} over {:?}",
        pixel(&covered, 32, 32),
        pixel(&base, 32, 32)
    );
}

#[test]
fn a_shift_moves_the_particles_with_the_world() {
    let Some(mut r) = renderer() else { return };
    let base = frame(&mut r, None);
    let centre = |frame: &[u8]| {
        let pixels = lit(frame, &base);
        pixels.iter().map(|&(x, _)| x as f32).sum::<f32>() / pixels.len().max(1) as f32
    };
    let e = emitter(
        &mut r,
        &EmitterParams {
            size: [0.3, 0.3],
            ..glow()
        },
        Vec3::ZERO,
    );
    r.emit(e, 20);
    let before = centre(&step(&mut r));
    r.shift_particles(Vec3::new(1.0, 0.0, 0.0));
    let after = centre(&step(&mut r));
    // One unit at five away, through a 60° view 64 pixels across: about 11.
    assert!(
        (after - before - 11.1).abs() < 1.5,
        "moved {} pixels right",
        after - before
    );
}

#[test]
fn a_continuous_emitter_stops_when_its_rate_is_turned_off() {
    let Some(mut r) = renderer() else { return };
    let base = frame(&mut r, None);
    let params = EmitterParams {
        rate: 120.0,
        lifetime: [0.2, 0.2],
        ..glow()
    };
    let e = emitter(&mut r, &params, Vec3::ZERO);
    for _ in 0..30 {
        step(&mut r);
    }
    assert!(!lit(&step(&mut r), &base).is_empty(), "emitting");
    r.set_emitter(
        e,
        &EmitterParams {
            rate: 0.0,
            ..params
        },
    );
    for _ in 0..20 {
        step(&mut r);
    }
    assert_eq!(step(&mut r), base, "its last particles have died");
}

#[test]
fn particles_add_no_allocations_per_frame() {
    let Some(mut r) = renderer() else { return };
    let e = emitter(
        &mut r,
        &EmitterParams {
            rate: 500.0,
            ..glow()
        },
        Vec3::ZERO,
    );
    for _ in 0..10 {
        step(&mut r);
    }
    let before = r.allocation_count();
    for i in 0..100 {
        r.place_emitter(
            e,
            &Transform::from_position(Vec3::new(0.0, (i as f32 * 0.1).sin(), 0.0)),
            Vec3::ZERO,
        );
        r.emit(e, 30);
        step(&mut r);
    }
    assert_eq!(
        r.allocation_count(),
        before,
        "no buffer allocated per frame"
    );
}
