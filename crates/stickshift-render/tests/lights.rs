// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! Point and spot lights, and emissive surfaces, on the real Metal backend.
//!
//! The light binning is unit-tested on the CPU (`src/lights.rs`); these tests
//! check the whole path on the GPU:
//!
//! - **Lit exactly where the CPU says.** For every ground pixel, the seam's own
//!   [`Light::radiance_at`] decides whether a light reaches it. Where it clearly
//!   does, the pixel must be brighter than in the same frame without lights;
//!   where it cannot, the pixel must be byte-identical. A cluster the binning
//!   missed on the GPU shows up as a pixel that should be lit and is not.
//! - **Spot cones, emission, both pipelines, per-frame lifetime, order.**
//! - **Cost:** lights add no per-frame allocation, and over-budget or invalid
//!   lights are counted, not drawn.
//! - **A golden night street** ([`NIGHT_STREET_REFERENCE_HASH`]): many lights,
//!   spot headlights and emissive windows in one frame. Exact on a real Apple
//!   GPU; within a tight tolerance of the committed frame on a paravirtualized
//!   one (a CI virtual machine), whose arithmetic rounds a little differently.
//!
//! Like the other backend tests these skip without a Metal device.

use stickshift_math::glam::{Mat4, Quat, Vec3, Vec4};
use stickshift_math::Transform;
use stickshift_render::backend::MAX_FRAMES_IN_FLIGHT;
use stickshift_render::MetalRenderer;
use stickshift_render_api::{
    ColorSpace, FrameRecorder, Light, LightKind, MaterialParams, MeshData, MeshHandle,
    PipelineDescriptor, PipelineHandle, PipelineKind, RenderDevice, SunSky, TextureData,
    TextureHandle, MAX_LIGHTS,
};

mod common;

/// 16 × 9 pixels per cluster tile, so lights cross many tiles.
const WIDTH: u32 = 256;
const HEIGHT: u32 = 144;

/// The ground's albedo (vertex colour, and the textured ground's white texture
/// times this factor).
const GROUND: f32 = 0.8;

/// Night: no direct sun, a little sky fill, a dark sky.
fn night() -> SunSky {
    SunSky {
        sun_elevation_deg: 30.0,
        sun_azimuth_deg: 200.0,
        sun_color: [1.0, 1.0, 1.0],
        sun_intensity: 0.0,
        sky_fill: 0.02,
        sky_zenith_color: [0.002, 0.004, 0.012],
        sky_horizon_color: [0.01, 0.012, 0.02],
    }
}

fn eye() -> Vec3 {
    Vec3::new(0.0, 4.0, 10.0)
}

fn view_projection() -> Mat4 {
    Mat4::perspective_rh(55f32.to_radians(), WIDTH as f32 / HEIGHT as f32, 0.3, 300.0)
        * Mat4::look_at_rh(eye(), Vec3::new(0.0, 0.0, -12.0), Vec3::Y)
}

fn push_vertex(bytes: &mut Vec<u8>, values: &[f32]) {
    for v in values {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
}

/// A flat quad at `y = 0` over `x0..x1` × `z0..z1`, facing up.
fn ground_quad(textured: bool, x0: f32, x1: f32, z0: f32, z1: f32) -> (Vec<u8>, Vec<u32>) {
    let mut bytes = Vec::new();
    for (x, z, u, v) in [
        (x0, z0, 0.0, 0.0),
        (x1, z0, 1.0, 0.0),
        (x1, z1, 1.0, 1.0),
        (x0, z1, 0.0, 1.0),
    ] {
        if textured {
            push_vertex(&mut bytes, &[x, 0.0, z, 0.0, 1.0, 0.0, u, v]);
        } else {
            push_vertex(
                &mut bytes,
                &[x, 0.0, z, 0.0, 1.0, 0.0, GROUND, GROUND, GROUND],
            );
        }
    }
    // Counter-clockwise seen from above.
    (bytes, vec![0, 3, 2, 0, 2, 1])
}

/// A unit cube centred on the origin, flat-shaded, in `color`.
fn cube(color: [f32; 3]) -> (Vec<u8>, Vec<u32>) {
    let faces: [(Vec3, Vec3, Vec3); 6] = [
        (Vec3::X, Vec3::Y, Vec3::Z),
        (Vec3::NEG_X, Vec3::Y, Vec3::NEG_Z),
        (Vec3::Y, Vec3::Z, Vec3::X),
        (Vec3::NEG_Y, Vec3::NEG_Z, Vec3::X),
        (Vec3::Z, Vec3::Y, Vec3::NEG_X),
        (Vec3::NEG_Z, Vec3::Y, Vec3::X),
    ];
    let mut bytes = Vec::new();
    let mut indices = Vec::new();
    for (n, up, right) in faces {
        let base = (indices.len() / 6 * 4) as u32;
        for (a, b) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let p = (n + right * a + up * b) * 0.5;
            push_vertex(
                &mut bytes,
                &[p.x, p.y, p.z, n.x, n.y, n.z, color[0], color[1], color[2]],
            );
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    (bytes, indices)
}

/// A renderer with the night sky, the camera, both pipelines, a white
/// texture, and the meshes the tests draw.
struct Scene {
    r: MetalRenderer,
    untextured: PipelineHandle,
    textured: PipelineHandle,
    white: TextureHandle,
    ground: MeshHandle,
    textured_ground: MeshHandle,
    cube: MeshHandle,
}

impl Scene {
    fn new() -> Option<Self> {
        Self::sized(WIDTH, HEIGHT)
    }

    /// A scene rendering into `width` × `height` (16:9, the camera's aspect).
    fn sized(width: u32, height: u32) -> Option<Self> {
        let mut r = MetalRenderer::new_offscreen(width, height)?;
        r.set_sun_sky(&night());
        r.set_view_projection(view_projection());
        r.set_camera_position(eye());
        let untextured = r.create_pipeline(&PipelineDescriptor::new(PipelineKind::VertexColor));
        let textured = r.create_pipeline(&PipelineDescriptor::new(PipelineKind::Textured));
        let white = r.create_texture(&TextureData {
            width: 1,
            height: 1,
            rgba8: &[255; 4],
            color_space: ColorSpace::Srgb,
        });
        let mesh = |r: &mut MetalRenderer,
                    (vertices, indices): (Vec<u8>, Vec<u32>),
                    kind: PipelineKind| {
            r.create_mesh(&MeshData {
                vertices: &vertices,
                indices: &indices,
                layout: kind.vertex_layout(),
            })
        };
        let ground = mesh(
            &mut r,
            ground_quad(false, -60.0, 60.0, -120.0, 9.0),
            PipelineKind::VertexColor,
        );
        let textured_ground = mesh(
            &mut r,
            ground_quad(true, -60.0, 60.0, -120.0, 9.0),
            PipelineKind::Textured,
        );
        let cube = mesh(&mut r, cube([0.3, 0.3, 0.32]), PipelineKind::VertexColor);
        Some(Self {
            r,
            untextured,
            textured,
            white,
            ground,
            textured_ground,
            cube,
        })
    }

    /// Render one frame: the ground (untextured or textured), any `cubes`
    /// (transform and material), and `lights`, added before or after the draws.
    fn render(
        &mut self,
        textured: bool,
        cubes: &[(Transform, MaterialParams)],
        lights: &[Light],
        lights_last: bool,
    ) -> Vec<u8> {
        let r = &mut self.r;
        r.begin_frame();
        if !lights_last {
            for light in lights {
                r.add_light(light);
            }
        }
        if textured {
            r.set_pipeline(self.textured);
            r.bind_texture(self.white);
            let material = MaterialParams {
                base_color: [GROUND, GROUND, GROUND, 1.0],
                ..MaterialParams::default()
            };
            r.draw_mesh(self.textured_ground, &Transform::identity(), &material);
        } else {
            r.set_pipeline(self.untextured);
            r.draw_mesh(
                self.ground,
                &Transform::identity(),
                &MaterialParams::default(),
            );
        }
        r.set_pipeline(self.untextured);
        for (transform, material) in cubes {
            r.draw_mesh(self.cube, transform, material);
        }
        if lights_last {
            for light in lights {
                r.add_light(light);
            }
        }
        r.submit();
        r.read_pixels().expect("offscreen")
    }
}

/// Where pixel `(x, y)`'s centre meets the ground, if it does in front of the
/// camera and within `max_distance` of it.
fn ground_point(x: u32, y: u32, max_distance: f32) -> Option<Vec3> {
    let inverse = view_projection().inverse();
    let ndc_x = (x as f32 + 0.5) / WIDTH as f32 * 2.0 - 1.0;
    let ndc_y = 1.0 - (y as f32 + 0.5) / HEIGHT as f32 * 2.0;
    let unproject = |z: f32| {
        let p: Vec4 = inverse * Vec4::new(ndc_x, ndc_y, z, 1.0);
        p.truncate() / p.w
    };
    let (near, far) = (unproject(0.0), unproject(1.0));
    let dir = far - near;
    if dir.y >= -1e-6 {
        return None;
    }
    let t = -near.y / dir.y;
    let p = near + dir * t;
    (p.distance(eye()) < max_distance && p.x.abs() < 55.0 && p.z > -115.0 && p.z < 8.0).then_some(p)
}

/// Luminance-ish: the mean of a BGRA pixel's colour bytes.
fn brightness(pixels: &[u8], x: u32, y: u32) -> f32 {
    let i = ((y * WIDTH + x) * 4) as usize;
    (u32::from(pixels[i]) + u32::from(pixels[i + 1]) + u32::from(pixels[i + 2])) as f32 / 3.0
}

fn pixel(pixels: &[u8], x: u32, y: u32) -> &[u8] {
    let i = ((y * WIDTH + x) * 4) as usize;
    &pixels[i..i + 4]
}

/// The light `light` delivers to an upward-facing surface at `p` (before the
/// albedo): its radiance there times the cosine to the light.
fn delivered(light: &Light, p: Vec3) -> f32 {
    let to_light = (light.position - p).normalize();
    let cos = to_light.y.max(0.0);
    let [r, g, b] = light.radiance_at(p);
    (r + g + b) / 3.0 * cos
}

/// Whether `light` cannot reach `p` even with some slack on its range and cone,
/// so a pixel there must be untouched whatever the rounding.
fn out_of_reach(light: &Light, p: Vec3) -> bool {
    let mut wider = *light;
    wider.range *= 1.03;
    if let LightKind::Spot {
        direction,
        inner_angle_deg,
        outer_angle_deg,
    } = light.kind
    {
        wider.kind = LightKind::Spot {
            direction,
            inner_angle_deg,
            outer_angle_deg: (outer_angle_deg + 2.0).min(90.0),
        };
    }
    wider.radiance_at(p) == [0.0; 3]
}

/// Lights spread over the view, near and far, so they cross many tiles and
/// depth slices: points of several sizes, and a spot shining down.
fn street_lights() -> Vec<Light> {
    vec![
        Light::point(Vec3::new(-3.0, 1.5, 2.0), [1.0, 0.8, 0.6], 8.0, 6.0),
        Light::point(Vec3::new(4.0, 2.0, -8.0), [0.6, 0.8, 1.0], 20.0, 10.0),
        Light::point(Vec3::new(-6.0, 3.0, -30.0), [1.0, 1.0, 1.0], 40.0, 14.0),
        Light::point(Vec3::new(8.0, 1.0, -55.0), [1.0, 0.5, 0.2], 30.0, 12.0),
        Light::spot(
            Vec3::new(0.0, 6.0, -16.0),
            Vec3::NEG_Y,
            [1.0, 1.0, 0.9],
            60.0,
            12.0,
            18.0,
            28.0,
        ),
    ]
}

#[test]
fn lights_reach_exactly_the_pixels_the_cpu_says() {
    let Some(mut scene) = Scene::new() else {
        common::no_gpu();
        return;
    };
    let lights = street_lights();
    for textured in [false, true] {
        let dark = scene.render(textured, &[], &[], false);
        let lit = scene.render(textured, &[], &lights, false);

        let (mut brighter, mut untouched) = (0, 0);
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let Some(p) = ground_point(x, y, 90.0) else {
                    continue;
                };
                let arriving: f32 = lights.iter().map(|l| delivered(l, p)).sum();
                if arriving * GROUND > 0.03 {
                    // Clearly lit: a missed cluster would leave it at the dark
                    // frame's value.
                    assert!(
                        brightness(&lit, x, y) > brightness(&dark, x, y) + 2.0,
                        "pipeline textured={textured}: pixel ({x}, {y}) at {p} should be lit \
                         ({arriving} arriving) but is {} vs {} unlit",
                        brightness(&lit, x, y),
                        brightness(&dark, x, y),
                    );
                    brighter += 1;
                } else if lights.iter().all(|l| out_of_reach(l, p)) {
                    assert_eq!(
                        pixel(&lit, x, y),
                        pixel(&dark, x, y),
                        "pipeline textured={textured}: pixel ({x}, {y}) at {p} is out of every \
                         light's reach but changed"
                    );
                    untouched += 1;
                }
            }
        }
        assert!(brighter > 1500, "only {brighter} lit pixels were checked");
        assert!(
            untouched > 1500,
            "only {untouched} unlit pixels were checked"
        );
    }
}

#[test]
fn a_frame_without_lights_in_reach_is_unchanged() {
    let Some(mut scene) = Scene::new() else {
        common::no_gpu();
        return;
    };
    let dark = scene.render(false, &[], &[], false);
    // Behind the camera, far off to the side, and hanging above the ground
    // with a range too short to touch it: none can light a visible pixel.
    let unreachable = [
        Light::point(Vec3::new(0.0, 2.0, 40.0), [1.0; 3], 100.0, 5.0),
        Light::point(Vec3::new(500.0, 2.0, -20.0), [1.0; 3], 100.0, 5.0),
        Light::point(Vec3::new(0.0, 6.0, -20.0), [1.0; 3], 100.0, 5.0),
    ];
    let lit = scene.render(false, &[], &unreachable, false);
    assert_same(&lit, &dark, "lights that reach nothing on screen");
}

#[test]
fn a_spot_light_lights_only_inside_its_cone() {
    let Some(mut scene) = Scene::new() else {
        common::no_gpu();
        return;
    };
    let spot = Light::spot(
        Vec3::new(0.0, 5.0, -10.0),
        Vec3::NEG_Y,
        [1.0; 3],
        60.0,
        20.0,
        25.0,
        35.0,
    );
    let as_point = Light {
        kind: LightKind::Point,
        ..spot
    };
    let dark = scene.render(false, &[], &[], false);
    let coned = scene.render(false, &[], &[spot], false);
    let round = scene.render(false, &[], &[as_point], false);

    let (mut inside, mut outside) = (0, 0);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let Some(p) = ground_point(x, y, 90.0) else {
                continue;
            };
            let axis_angle = (p - spot.position)
                .normalize()
                .angle_between(Vec3::NEG_Y)
                .to_degrees();
            if axis_angle < 23.0 {
                // Inside the inner cone a spot is exactly its point light.
                assert_eq!(pixel(&coned, x, y), pixel(&round, x, y), "({x}, {y})");
                inside += 1;
            } else if axis_angle > 37.0 {
                // Outside the outer cone it lights nothing, though the point
                // light of the same range does.
                assert_eq!(pixel(&coned, x, y), pixel(&dark, x, y), "({x}, {y})");
                if delivered(&as_point, p) * GROUND > 0.03 {
                    assert!(brightness(&round, x, y) > brightness(&dark, x, y) + 2.0);
                    outside += 1;
                }
            }
        }
    }
    assert!(
        inside > 50 && outside > 200,
        "inside {inside}, outside {outside}"
    );
}

#[test]
fn emission_glows_without_lighting_anything_else() {
    let Some(mut scene) = Scene::new() else {
        common::no_gpu();
        return;
    };
    let at = placed(
        Vec3::new(0.0, 1.0, -6.0),
        Quat::from_rotation_y(0.6),
        Vec3::splat(2.0),
    );
    let plain = scene.render(false, &[(at, MaterialParams::default())], &[], false);
    let glowing = MaterialParams {
        emissive: [3.0, 2.0, 0.8],
        ..MaterialParams::default()
    };
    let glow = scene.render(false, &[(at, glowing)], &[], false);

    // The cube's own pixels light up...
    let (cx, cy) = projected(Vec3::new(0.0, 1.0, -6.0));
    assert!(
        brightness(&glow, cx, cy) > brightness(&plain, cx, cy) + 60.0,
        "{} vs {}",
        brightness(&glow, cx, cy),
        brightness(&plain, cx, cy)
    );
    // ...and nothing else changes: emission is not a light. Every pixel that
    // differs is one the cube covers in both frames, i.e. one that is not
    // the plain frame's ground or sky.
    let dark = scene.render(false, &[], &[], false);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            if pixel(&glow, x, y) != pixel(&plain, x, y) {
                assert_ne!(
                    pixel(&plain, x, y),
                    pixel(&dark, x, y),
                    "pixel ({x}, {y}) changed but the cube does not cover it"
                );
            }
        }
    }
}

/// A transform from its parts.
fn placed(position: Vec3, rotation: Quat, scale: Vec3) -> Transform {
    Transform {
        position,
        rotation,
        scale,
    }
}

/// Assert two frames are byte-identical, saying how many pixels differ if not
/// (instead of printing both frames).
#[track_caller]
fn assert_same(a: &[u8], b: &[u8], what: &str) {
    let differing = a
        .chunks_exact(4)
        .zip(b.chunks_exact(4))
        .filter(|(p, q)| p != q)
        .count();
    assert_eq!(differing, 0, "{what}: {differing} pixels differ");
}

/// The pixel `p` lands on.
fn projected(p: Vec3) -> (u32, u32) {
    let clip = view_projection() * p.extend(1.0);
    let (x, y) = (clip.x / clip.w, clip.y / clip.w);
    (
        ((x + 1.0) * 0.5 * WIDTH as f32) as u32,
        ((1.0 - y) * 0.5 * HEIGHT as f32) as u32,
    )
}

#[test]
fn lights_last_one_frame_and_light_draws_recorded_before_them() {
    let Some(mut scene) = Scene::new() else {
        common::no_gpu();
        return;
    };
    let lights = street_lights();
    let dark = scene.render(false, &[], &[], false);
    let first = scene.render(false, &[], &lights, false);
    assert_ne!(first, dark, "the lights light the frame");

    // Lights are not sticky: the next frame without them is dark again.
    let next = scene.render(false, &[], &[], false);
    assert_same(&next, &dark, "the frame after the lit one");

    // Adding the lights after the draws lights the same frame identically.
    let after = scene.render(false, &[], &lights, true);
    assert_same(&after, &first, "lights added after the draws");
}

#[test]
fn many_lights_allocate_nothing_per_frame() {
    let Some(mut scene) = Scene::new() else {
        common::no_gpu();
        return;
    };
    let lights: Vec<Light> = (0..MAX_LIGHTS)
        .map(|i| {
            let (x, z) = ((i % 16) as f32 * 4.0 - 30.0, -((i / 16) as f32) * 6.0);
            Light::point(Vec3::new(x, 1.5, z), [1.0, 0.9, 0.8], 6.0, 3.5)
        })
        .collect();
    scene.render(false, &[], &lights, false);
    let before = scene.r.allocation_count();
    for _ in 0..(2 * MAX_FRAMES_IN_FLIGHT) {
        scene.render(false, &[], &lights, false);
    }
    assert_eq!(scene.r.allocation_count() - before, 0);
    assert_eq!(
        scene.r.dropped_light_count(),
        0,
        "a street of small lights fits"
    );
}

#[test]
fn over_budget_and_invalid_lights_are_counted_not_drawn() {
    let Some(mut scene) = Scene::new() else {
        common::no_gpu();
        return;
    };
    let lamp = Light::point(Vec3::new(0.0, 1.5, -10.0), [1.0; 3], 6.0, 3.0);
    let mut broken = lamp;
    broken.range = f32::NAN;

    let mut lights = vec![lamp; MAX_LIGHTS + 5];
    lights.push(broken);
    scene.render(false, &[], &lights, false);
    assert_eq!(scene.r.dropped_light_count(), 5);
    assert_eq!(scene.r.rejected_light_count(), 1);

    // Outside a frame a light is ignored without being counted.
    scene.r.add_light(&lamp);
    assert_eq!(scene.r.dropped_light_count(), 5);
}

// ---------------------------------------------------------------------------
// The golden night street
// ---------------------------------------------------------------------------

/// Committed hash (FNV-1a 64-bit) of [`render_night_street`]'s pixels.
///
/// # First blessing
///
/// The scene: the night sky, the untextured ground, a row of streetlamp posts
/// (cubes) with a warm point light over each, two rows of emissive "windows",
/// a pair of spot headlights on the road and a red tail light: 42 lights over
/// most of the view's tiles and slices. Re-bless (with a justification) only
/// for an intended change to how lights or emission shade:
///
/// ```text
/// BLESS=1 cargo test -p stickshift-render --test lights -- --nocapture
/// ```
///
/// Blessed 2026-11-15 on an Apple M3 Max, after looking at the frame
/// (`LIGHTS_DUMP=night.ppm`): warm pools under every lamp fading smoothly into
/// each other with no tile or slice seams, the headlights' pool on the road,
/// the tail light's red patch, and every window glowing against its dark wall.
///
/// # Re-blessed: the sun's highlights follow its intensity
///
/// `0x4983fc2460d51eef` → `0xd1c676c95af0bc4a`. The sun's specular term was
/// scaled by a constant, not by the sun's intensity, so this scene's sun, which
/// is switched off (`sun_intensity: 0.0`), still put highlights on every block
/// face turned toward it. Now it lights nothing, highlights included. Checked
/// before accepting it:
///
/// - **Only those highlights went.** 983 of 9216 pixels changed, every one of
///   them darker (by up to 61/255, 15 on average), all on the block faces
///   turned toward the switched-off sun; nothing got brighter. Looking at both
///   frames, the lamp pools, the headlight pool, the tail light and every
///   window are unchanged.
/// - **Nothing else moved.** With the specular term reverted to the old
///   constant scale, this hash and every other baseline pass unchanged, so the
///   shadow map's carried orientation (the same change set) does not touch a
///   still-sun frame.
///
/// # On a paravirtualized GPU
///
/// GitHub's macOS runners expose an "Apple Paravirtual device" that renders
/// this frame a hair differently (the first CI run hashed it
/// `0xb42dbebfdc84ec56`): per-light transcendentals (`pow`, `rsqrt`) round
/// differently there, and the frame sums dozens of lights. There, the frame
/// is compared with the committed image ([`NIGHT_STREET_REFERENCE_BGRA`])
/// instead: every channel within [`NIGHT_STREET_MAX_DELTA`], and at most
/// [`NIGHT_STREET_MAX_DIFFERING_PIXELS`] pixels off by more than one step.
/// `the_night_street_tolerance_is_not_vacuous` proves that rule still fails a
/// frame missing a single lamp.
const NIGHT_STREET_REFERENCE_HASH: u64 = 0xd1c676c95af0bc4a;

/// The night street's size: small, so its committed image stays small.
const NIGHT_WIDTH: u32 = 128;
const NIGHT_HEIGHT: u32 = 72;

/// The frame [`NIGHT_STREET_REFERENCE_HASH`] pins, BGRA8.
const NIGHT_STREET_REFERENCE_BGRA: &[u8] = include_bytes!("data/night_street_reference.bgra");
const NIGHT_STREET_REFERENCE_BGRA_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/data/night_street_reference.bgra"
);
/// The most any channel of any pixel may differ from the committed frame.
const NIGHT_STREET_MAX_DELTA: u8 = 4;
/// How many pixels may differ by more than one step (about 1% of the frame).
const NIGHT_STREET_MAX_DIFFERING_PIXELS: usize = 92;

/// Whether `frame` is within the paravirtualized-GPU tolerance of `reference`.
fn compare_night_street(reference: &[u8], frame: &[u8]) -> Result<(), String> {
    let mut worst = 0u8;
    let mut differing = 0;
    for (a, b) in reference.chunks_exact(4).zip(frame.chunks_exact(4)) {
        let delta = a
            .iter()
            .zip(b)
            .map(|(x, y)| x.abs_diff(*y))
            .max()
            .unwrap_or(0);
        worst = worst.max(delta);
        if delta > 1 {
            differing += 1;
        }
    }
    if worst > NIGHT_STREET_MAX_DELTA {
        return Err(format!(
            "a channel is {worst} steps off (at most {NIGHT_STREET_MAX_DELTA})"
        ));
    }
    if differing > NIGHT_STREET_MAX_DIFFERING_PIXELS {
        return Err(format!(
            "{differing} pixels are more than one step off (at most \
             {NIGHT_STREET_MAX_DIFFERING_PIXELS})"
        ));
    }
    Ok(())
}

/// Render the night street, optionally without lamp `missing_lamp`.
fn render_night_street(scene: &mut Scene, missing_lamp: Option<usize>) -> Vec<u8> {
    let mut cubes = Vec::new();
    let mut lights = Vec::new();
    for i in 0..18 {
        let z = 4.0 - i as f32 * 7.0;
        for side in [-1.0f32, 1.0] {
            let x = side * 7.0;
            if (i + usize::from(side > 0.0)).is_multiple_of(2) {
                // A lamp post, with its light just above it.
                cubes.push((
                    placed(
                        Vec3::new(x, 2.5, z),
                        Quat::IDENTITY,
                        Vec3::new(0.3, 5.0, 0.3),
                    ),
                    MaterialParams::default(),
                ));
                lights.push(Light::point(
                    Vec3::new(x * 0.85, 5.4, z),
                    [1.0, 0.78, 0.5],
                    22.0,
                    9.0,
                ));
            } else {
                // A building face with a lit window.
                cubes.push((
                    placed(
                        Vec3::new(x * 1.6, 3.0, z),
                        Quat::IDENTITY,
                        Vec3::new(3.0, 6.0, 4.0),
                    ),
                    MaterialParams::default(),
                ));
                cubes.push((
                    placed(
                        Vec3::new(x * 1.6 - side * 1.52, 3.4, z),
                        Quat::IDENTITY,
                        Vec3::new(0.05, 1.2, 1.6),
                    ),
                    MaterialParams {
                        emissive: [2.4, 1.9, 1.0],
                        ..MaterialParams::default()
                    },
                ));
            }
        }
    }
    for x in [-0.8f32, 0.8] {
        lights.push(Light::spot(
            Vec3::new(x, 0.7, -4.0),
            Vec3::new(0.0, -0.12, -1.0),
            [1.0, 0.97, 0.9],
            90.0,
            40.0,
            12.0,
            22.0,
        ));
    }
    lights.push(Light::point(
        Vec3::new(0.0, 0.6, -24.0),
        [1.0, 0.05, 0.02],
        3.0,
        4.0,
    ));
    if let Some(lamp) = missing_lamp {
        lights.remove(lamp);
    }
    scene.render(false, &cubes, &lights, false)
}

#[test]
fn night_street_matches_committed_hash() {
    let Some(mut scene) = Scene::sized(NIGHT_WIDTH, NIGHT_HEIGHT) else {
        common::no_gpu();
        return;
    };
    let device = scene.r.device_name();
    let pixels = render_night_street(&mut scene, None);
    // `LIGHTS_DUMP=frame.ppm` writes the frame out to look at, e.g. before a
    // re-bless.
    if let Ok(path) = std::env::var("LIGHTS_DUMP") {
        let mut ppm = format!("P6\n{NIGHT_WIDTH} {NIGHT_HEIGHT}\n255\n").into_bytes();
        for bgra in pixels.chunks_exact(4) {
            ppm.extend_from_slice(&[bgra[2], bgra[1], bgra[0]]);
        }
        std::fs::write(path, ppm).expect("write the dump");
    }
    if common::blessing() {
        std::fs::write(NIGHT_STREET_REFERENCE_BGRA_PATH, &pixels)
            .expect("BLESS: failed to write the night street reference image");
        println!("BLESS: rewrote {NIGHT_STREET_REFERENCE_BGRA_PATH}");
        common::bless_or_assert(
            &pixels,
            NIGHT_STREET_REFERENCE_HASH,
            "NIGHT_STREET_REFERENCE_HASH",
        );
        return;
    }

    // Runs on every device (trivially exact on a real GPU), so the tolerance
    // path CI depends on is exercised locally too.
    let tolerance = compare_night_street(NIGHT_STREET_REFERENCE_BGRA, &pixels);
    if common::is_paravirtual(&device) {
        eprintln!(
            "{device}: paravirtualized Metal device, so the night street is compared within \
             tolerance (hash here {:#018x}; the exact {NIGHT_STREET_REFERENCE_HASH:#018x} is \
             asserted on real Apple GPUs)",
            common::fnv1a_64(&pixels)
        );
        if let Err(why) = tolerance {
            common::dump_frame("NIGHT_STREET_REFERENCE_BGRA", &pixels);
            panic!(
                "the night street on {device} is outside the paravirtualized-GPU tolerance: {why}"
            );
        }
        return;
    }
    if common::fnv1a_64(&pixels) != NIGHT_STREET_REFERENCE_HASH {
        common::dump_frame("NIGHT_STREET_REFERENCE_BGRA", &pixels);
    }
    common::bless_or_assert(
        &pixels,
        NIGHT_STREET_REFERENCE_HASH,
        "NIGHT_STREET_REFERENCE_HASH",
    );
    assert_eq!(tolerance, Ok(()));
}

#[test]
fn night_street_reference_image_matches_committed_hash() {
    // GPU-free: the committed image is exactly the frame the hash pins.
    assert_eq!(
        NIGHT_STREET_REFERENCE_BGRA.len(),
        (NIGHT_WIDTH * NIGHT_HEIGHT * 4) as usize
    );
    assert_eq!(
        common::fnv1a_64(NIGHT_STREET_REFERENCE_BGRA),
        NIGHT_STREET_REFERENCE_HASH,
        "tests/data/night_street_reference.bgra is not the frame NIGHT_STREET_REFERENCE_HASH \
         pins; re-bless both together (BLESS=1 rewrites the image)"
    );
}

#[test]
fn the_night_street_tolerance_is_not_vacuous() {
    // Rendered and compared on the device under test, so it holds on any GPU:
    // the frame matches itself, and loses the tolerance when one lamp goes out.
    let Some(mut scene) = Scene::sized(NIGHT_WIDTH, NIGHT_HEIGHT) else {
        common::no_gpu();
        return;
    };
    let all = render_night_street(&mut scene, None);
    assert_eq!(compare_night_street(&all, &all), Ok(()));
    // Lamp 2 stands on the left, 20 units ahead, in full view.
    let one_out = render_night_street(&mut scene, Some(2));
    assert!(
        compare_night_street(&all, &one_out).is_err(),
        "a frame missing a lamp passed the tolerance"
    );
}

#[test]
fn a_sun_switched_off_lights_nothing_wherever_it_stands() {
    // The night street's sun is at zero intensity. Every term it contributes,
    // its highlights included, scales with that intensity, so turning it to
    // stand anywhere else must leave the frame bit-for-bit as it was. A game
    // relies on this to swap its sun for a moon (or back) while neither is lit,
    // with no jump on screen.
    let Some(mut scene) = Scene::sized(NIGHT_WIDTH, NIGHT_HEIGHT) else {
        common::no_gpu();
        return;
    };
    let reference = render_night_street(&mut scene, None);
    for (elevation, azimuth) in [(60.0, 20.0), (5.0, 290.0), (85.0, 120.0)] {
        scene.r.set_sun_sky(&SunSky {
            sun_elevation_deg: elevation,
            sun_azimuth_deg: azimuth,
            ..night()
        });
        assert!(
            render_night_street(&mut scene, None) == reference,
            "a switched-off sun at elevation {elevation}°, azimuth {azimuth}° changed the frame"
        );
    }
}
