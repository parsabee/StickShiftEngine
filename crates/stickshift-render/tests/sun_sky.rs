// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! Drivable sun + per-frame light upload guards.
//!
//! These pin the three properties that make the sun *reachable* and *safe*, all
//! against the real Metal backend and without needing to read a pixel back:
//!
//! 1. **The sun arrives from above the seam.** A [`SunSky`] pushed with
//!    `set_sun_sky` becomes the exact bytes the fragment shader reads, and the
//!    light direction is the one `SunSky::direction()` derives — the single source
//!    of truth the sky's sun disc also uses. Before the sun moved into the seam, the light was written
//!    once at construction and nothing above the seam could change it.
//! 2. **It is honoured per frame, in a rotating ring slot.** The light is uploaded
//!    every frame into one slot per in-flight frame (`0, 256, 512, 0, …`), so the
//!    CPU never rewrites bytes a queued frame is still reading — the same argument
//!    as the per-draw uniform ring.
//! 3. **Per-frame upload costs no allocation.** The ring is created once, so the
//!    hot path's `new_buffer*` delta stays 0.
//!
//! Like the other backend tests these **skip** when no Metal device is available
//! (GPU-less CI) and run + assert on a real Mac.

use stickshift_math::glam::{Mat4, Vec3};
use stickshift_math::Transform;
use stickshift_render::backend::MAX_FRAMES_IN_FLIGHT;
use stickshift_render::{LightUniforms, MetalRenderer};
use stickshift_render_api::{
    FrameRecorder, MaterialParams, MeshData, MeshHandle, PipelineDescriptor, PipelineHandle,
    PipelineKind, RenderDevice, SunSky,
};

mod common;

/// Offscreen render size.
const WIDTH: u32 = 64;
/// Offscreen render size.
const HEIGHT: u32 = 64;

/// A low western sun, deliberately unlike the default so every field is proven to
/// travel (a value equal to the default would pass even if the push were dropped).
fn western_afternoon_sun() -> SunSky {
    SunSky {
        sun_elevation_deg: 32.0,
        sun_azimuth_deg: 284.0,
        sun_color: [1.0, 0.96, 0.88],
        sun_intensity: 1.15,
        sky_fill: 0.22,
        sky_zenith_color: [0.11, 0.27, 0.56],
        sky_horizon_color: [0.62, 0.67, 0.74],
    }
}

#[test]
fn pushed_sun_becomes_the_light_the_shader_reads() {
    let Some(mut r) = MetalRenderer::new_offscreen(WIDTH, HEIGHT) else {
        common::no_gpu();
        return;
    };

    // Until something pushes a sun, the backend uses the seam's own default.
    let default_light = r.light_for_test();
    assert_eq!(
        default_light.direction,
        SunSky::default().direction().to_array()
    );

    let sun = western_afternoon_sun();
    r.set_sun_sky(&sun);

    let light = r.light_for_test();
    // The property the sun disc depends on: the shaded direction *is*
    // `SunSky::direction()`, so the disc drawn at that direction cannot disagree
    // with the lighting.
    assert_eq!(light.direction, sun.direction().to_array());
    assert_eq!(light.color, sun.sun_color);
    assert_eq!(light.ambient_intensity, sun.sky_fill);
    assert_eq!(light.diffuse_intensity, sun.sun_intensity);
    assert_eq!(light.sky_top_color, sun.sky_zenith_color);
    assert_eq!(light.sky_horizon_color, sun.sky_horizon_color);

    // And it really is a different sun than the default, so the assertions above
    // are not passing by coincidence.
    assert_ne!(light.direction, default_light.direction);
}

#[test]
fn camera_reaches_the_fragment_shader_and_the_sun_is_sticky() {
    let Some(mut r) = MetalRenderer::new_offscreen(WIDTH, HEIGHT) else {
        common::no_gpu();
        return;
    };

    let sun = western_afternoon_sun();
    let eye = Vec3::new(2.0, 6.0, 14.0);
    r.set_sun_sky(&sun);
    r.set_camera_position(eye);

    // The camera block is folded in when the frame captures its light (at its
    // first draw, or at submit for a frame with none), so it is the frame — not
    // the push — that publishes it.
    r.begin_frame();
    r.submit();
    assert_eq!(
        r.light_for_test().camera_position,
        eye.to_array(),
        "specular needs the eye position; it must reach the light block"
    );

    // The sun survives the frame boundary: a game with a fixed sun pushes once at
    // load and every later frame is still lit by it.
    r.begin_frame();
    r.submit();
    assert_eq!(r.light_for_test().direction, sun.direction().to_array());
}

#[test]
fn light_upload_rotates_one_slot_per_in_flight_frame() {
    let Some(mut r) = MetalRenderer::new_offscreen(WIDTH, HEIGHT) else {
        common::no_gpu();
        return;
    };

    let stride = r.ring_stride_for_test();

    // Run past a full triple-buffer cycle: the light must land in a different slot
    // each frame and only reuse a slot after `MAX_FRAMES_IN_FLIGHT` frames — by
    // which point the semaphore has proven that frame retired.
    let frames = (2 * MAX_FRAMES_IN_FLIGHT) + 1;
    for i in 0..frames {
        let expected = (i % MAX_FRAMES_IN_FLIGHT) * stride;
        r.begin_frame();
        // The offset the frame actually wrote its light to and binds it at.
        let offset = r.light_offset_for_test().expect("a frame is open");
        assert_eq!(
            offset,
            expected,
            "frame {i}: the light must be written to slot {} of the ring",
            i % MAX_FRAMES_IN_FLIGHT
        );
        assert_eq!(
            offset % 256,
            0,
            "fragment-buffer offsets must be 256-byte aligned on Apple GPUs"
        );
        r.submit();
    }
    assert_eq!(r.light_offset_for_test(), None, "no frame open");
}

#[test]
fn shadow_distance_follows_the_draw_distance_unless_overridden() {
    use stickshift_math::glam::Mat4;
    use stickshift_render::shadow::{shadow_distance_for_fog, MIN_SHADOW_DISTANCE};

    let Some(mut r) = MetalRenderer::new_offscreen(WIDTH, HEIGHT) else {
        common::no_gpu();
        return;
    };
    // The playable demo's chase camera (0.3..300 clip), so the far plane never
    // cuts the slab short of the fog.
    r.set_view_projection(
        Mat4::perspective_rh(45f32.to_radians(), 16.0 / 9.0, 0.3, 300.0)
            * Mat4::look_at_rh(Vec3::new(0.0, 6.0, 12.0), Vec3::new(0.0, 1.5, 0.0), Vec3::Y),
    );
    let fog = {
        let l = r.light_for_test();
        shadow_distance_for_fog(l.fog_start, l.fog_density)
    };
    // The slab depth a frame actually fits (folded into the light block when
    // the frame captures its light).
    let frame_shadow_distance = |r: &mut MetalRenderer| {
        r.begin_frame();
        r.submit();
        r.light_for_test().shadow_distance
    };

    // Default: "match draw distance" — the slab is the fog's 165 units.
    assert_eq!(r.shadow_distance(), None);
    assert!((fog - 165.0).abs() < 1e-3, "default draw distance {fog}");
    assert_eq!(frame_shadow_distance(&mut r), fog);

    // A shorter fixed range is what the frame fits...
    r.set_shadow_distance(Some(40.0));
    assert_eq!(r.shadow_distance(), Some(40.0));
    assert_eq!(frame_shadow_distance(&mut r), 40.0);
    // ...a longer one is clamped to the fog (the getter keeps the request)...
    r.set_shadow_distance(Some(1000.0));
    assert_eq!(r.shadow_distance(), Some(1000.0));
    assert_eq!(frame_shadow_distance(&mut r), fog);
    // ...a degenerate one to the minimum...
    r.set_shadow_distance(Some(0.0));
    assert_eq!(frame_shadow_distance(&mut r), MIN_SHADOW_DISTANCE);
    // ...and `None` goes back to following the draw distance.
    r.set_shadow_distance(None);
    assert_eq!(r.shadow_distance(), None);
    assert_eq!(frame_shadow_distance(&mut r), fog);
}

#[test]
fn per_frame_light_upload_allocates_nothing() {
    let Some(mut r) = MetalRenderer::new_offscreen(WIDTH, HEIGHT) else {
        common::no_gpu();
        return;
    };

    r.set_sun_sky(&western_afternoon_sun());

    // Warm one frame so lazily-created per-frame state (the depth/MSAA textures)
    // exists before measuring steady state.
    r.begin_frame();
    r.submit();

    let before = r.allocation_count();
    for _ in 0..(2 * MAX_FRAMES_IN_FLIGHT) {
        // A per-frame *upload* must not become a per-frame *allocation*: the light
        // is written into the ring created at construction, never a fresh buffer.
        r.set_sun_sky(&western_afternoon_sun());
        r.begin_frame();
        r.submit();
    }
    let delta = r.allocation_count() - before;
    assert_eq!(
        delta, 0,
        "per-frame light upload allocated {delta} buffers; expected 0"
    );
}

/// A camera, as the seam carries it: a view-projection and the eye position.
#[derive(Clone, Copy)]
struct SeamCamera {
    view_projection: Mat4,
    position: Vec3,
}

impl SeamCamera {
    fn looking_from(position: Vec3) -> Self {
        Self {
            view_projection: Mat4::perspective_rh(60f32.to_radians(), 1.0, 0.3, 300.0)
                * Mat4::look_at_rh(position, Vec3::ZERO, Vec3::Y),
            position,
        }
    }

    fn push(self, r: &mut MetalRenderer) {
        r.set_view_projection(self.view_projection);
        r.set_camera_position(self.position);
    }
}

/// A renderer with one drawable triangle and its pipeline.
fn renderer_with_a_triangle() -> Option<(MetalRenderer, MeshHandle, PipelineHandle)> {
    let Some(mut r) = MetalRenderer::new_offscreen(WIDTH, HEIGHT) else {
        common::no_gpu();
        return None;
    };
    let kind = PipelineKind::VertexColor;
    let mut vertices = Vec::new();
    for corner in [[-0.5f32, 0.0, -0.5], [0.5, 0.0, -0.5], [0.0, 0.0, 0.5]] {
        let vertex = [
            corner[0], corner[1], corner[2], 0.0, 1.0, 0.0, 1.0, 1.0, 1.0,
        ];
        vertices.extend(vertex.iter().flat_map(|f| f.to_ne_bytes()));
    }
    let mesh = r.create_mesh(&MeshData {
        vertices: &vertices,
        indices: &[0, 1, 2],
        layout: kind.vertex_layout(),
    });
    let pipeline = r.create_pipeline(&PipelineDescriptor::new(kind));
    Some((r, mesh, pipeline))
}

/// The camera-dependent half of a light block: what the sky, specular and the
/// shadow fit take from the camera.
fn camera_half(l: &LightUniforms) -> ([f32; 3], [[f32; 4]; 4], [[f32; 4]; 4]) {
    (
        l.camera_position,
        l.inverse_view_projection,
        l.light_view_projection,
    )
}

/// The light a frame drawn from `camera`, pushed before `begin_frame` as the
/// engine loop does, ends up with.
fn reference_light(camera: SeamCamera) -> Option<LightUniforms> {
    let (mut r, mesh, pipeline) = renderer_with_a_triangle()?;
    camera.push(&mut r);
    r.begin_frame();
    r.set_pipeline(pipeline);
    r.draw_mesh(mesh, &Transform::identity(), &MaterialParams::default());
    r.submit();
    Some(r.light_for_test())
}

#[test]
fn a_camera_set_after_begin_frame_lights_and_shadows_that_frame() {
    let camera = SeamCamera::looking_from(Vec3::new(4.0, 6.0, 12.0));
    let Some(expected) = reference_light(camera) else {
        return;
    };
    let Some((mut r, mesh, pipeline)) = renderer_with_a_triangle() else {
        return;
    };
    // The documented order: open the frame, then set the camera. This is also
    // the renderer's first frame, so a light captured at `begin_frame` would be
    // fitted to the identity matrix.
    r.begin_frame();
    camera.push(&mut r);
    r.set_pipeline(pipeline);
    r.draw_mesh(mesh, &Transform::identity(), &MaterialParams::default());
    r.submit();
    let light = r.light_for_test();
    assert_eq!(
        camera_half(&light),
        camera_half(&expected),
        "the sky, specular and shadow fit use the camera the frame drew with"
    );
    assert_eq!(light.shadow_distance, expected.shadow_distance);
}

#[test]
fn a_frame_keeps_the_light_of_its_first_draw() {
    let first = SeamCamera::looking_from(Vec3::new(4.0, 6.0, 12.0));
    let second = SeamCamera::looking_from(Vec3::new(-20.0, 3.0, -2.0));
    let Some(expected) = reference_light(first) else {
        return;
    };
    let Some((mut r, mesh, pipeline)) = renderer_with_a_triangle() else {
        return;
    };
    r.begin_frame();
    first.push(&mut r);
    r.set_pipeline(pipeline);
    r.draw_mesh(mesh, &Transform::identity(), &MaterialParams::default());
    // A mid-frame switch moves the draws after it, not the frame's light.
    second.push(&mut r);
    r.draw_mesh(mesh, &Transform::identity(), &MaterialParams::default());
    r.submit();
    assert_eq!(camera_half(&r.light_for_test()), camera_half(&expected));
}

#[test]
fn a_frame_with_no_draws_is_lit_by_the_camera_at_submit() {
    let camera = SeamCamera::looking_from(Vec3::new(4.0, 6.0, 12.0));
    let Some(expected) = reference_light(camera) else {
        return;
    };
    let Some((mut r, _, _)) = renderer_with_a_triangle() else {
        return;
    };
    // Only the sky is drawn, and it still needs the camera's view rays.
    r.begin_frame();
    camera.push(&mut r);
    r.submit();
    assert_eq!(camera_half(&r.light_for_test()), camera_half(&expected));
}
