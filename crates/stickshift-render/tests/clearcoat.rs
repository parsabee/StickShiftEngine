// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! The clear coat (`MaterialParams::clearcoat`) on the real Metal backend.
//!
//! - **It mirrors the sky without the sun**, so a dark surface keeps its shape
//!   in the shade: with the sun off, a coated dark cube is brighter than a
//!   matt one on every face, and most on the face seen most edge-on
//!   (Fresnel). Its top mirrors the blue sky; nothing outside the cube
//!   changes, since the coat is not a light.
//! - **It catches the sun in a glint**, which a sunless sky does not give.
//! - **Both pipelines** coat.
//! - **No coat is no change:** zero, negative and NaN coats all draw exactly
//!   as a default material does (the golden frames in the other tests pin
//!   that default to the frames from before the coat existed).
//!
//! Like the other backend tests these skip without a Metal device.

use stickshift_math::glam::{Mat4, Quat, Vec3};
use stickshift_math::Transform;
use stickshift_render::MetalRenderer;
use stickshift_render_api::{
    ColorSpace, FrameRecorder, MaterialParams, MeshData, MeshHandle, PipelineDescriptor,
    PipelineHandle, PipelineKind, RenderDevice, SunSky, TextureData, TextureHandle,
};

mod common;

const WIDTH: u32 = 256;
const HEIGHT: u32 = 144;

/// A near-black paint, the colour the coat is for.
const DARK: [f32; 3] = [0.03, 0.03, 0.032];

/// A clear afternoon sky, with the sun at `sun_intensity` (`0` stands in for
/// the shade: only the sky lights the scene).
fn afternoon(sun_intensity: f32) -> SunSky {
    SunSky {
        sun_elevation_deg: 32.0,
        sun_azimuth_deg: 160.0,
        sun_color: [1.0, 0.96, 0.88],
        sun_intensity,
        sky_fill: 0.22,
        sky_zenith_color: [0.11, 0.27, 0.56],
        sky_horizon_color: [0.62, 0.67, 0.74],
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

/// A unit cube, flat-shaded, coloured `color` per vertex.
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

/// A unit-radius sphere with smooth normals, coloured `color`: somewhere on it
/// faces halfway between the sun and the camera, where a glint shows.
fn sphere(color: [f32; 3]) -> (Vec<u8>, Vec<u32>) {
    let (rings, segments) = (48u32, 96u32);
    let mut bytes = Vec::new();
    for i in 0..=rings {
        let theta = std::f32::consts::PI * i as f32 / rings as f32;
        for j in 0..=segments {
            let phi = std::f32::consts::TAU * j as f32 / segments as f32;
            let n = Vec3::new(
                theta.sin() * phi.cos(),
                theta.cos(),
                theta.sin() * phi.sin(),
            );
            push_vertex(
                &mut bytes,
                &[n.x, n.y, n.z, n.x, n.y, n.z, color[0], color[1], color[2]],
            );
        }
    }
    let mut indices = Vec::new();
    let row = segments + 1;
    for i in 0..rings {
        for j in 0..segments {
            let (a, b) = (i * row + j, (i + 1) * row + j);
            indices.extend_from_slice(&[a, a + 1, b, b, a + 1, b + 1]);
        }
    }
    (bytes, indices)
}

/// A textured flat square on the ground, facing up, its UVs across it.
fn textured_square() -> (Vec<u8>, Vec<u32>) {
    let mut bytes = Vec::new();
    for (x, z, u, v) in [
        (-3.0, -3.0, 0.0, 0.0),
        (3.0, -3.0, 1.0, 0.0),
        (3.0, 3.0, 1.0, 1.0),
        (-3.0, 3.0, 0.0, 1.0),
    ] {
        push_vertex(&mut bytes, &[x, 0.0, z, 0.0, 1.0, 0.0, u, v]);
    }
    (bytes, vec![0, 2, 1, 0, 3, 2])
}

struct Scene {
    r: MetalRenderer,
    untextured: PipelineHandle,
    textured: PipelineHandle,
    white: TextureHandle,
    cube: MeshHandle,
    sphere: MeshHandle,
    square: MeshHandle,
}

impl Scene {
    fn new() -> Option<Self> {
        let mut r = MetalRenderer::new_offscreen(WIDTH, HEIGHT)?;
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
        let cube = mesh(&mut r, cube(DARK), PipelineKind::VertexColor);
        let sphere = mesh(&mut r, sphere(DARK), PipelineKind::VertexColor);
        let square = mesh(&mut r, textured_square(), PipelineKind::Textured);
        Some(Self {
            r,
            untextured,
            textured,
            white,
            cube,
            sphere,
            square,
        })
    }

    /// One frame under `sky`: `mesh` (untextured) at `at` with `material`.
    fn render(
        &mut self,
        sky: &SunSky,
        mesh: MeshHandle,
        at: &Transform,
        material: &MaterialParams,
    ) -> Vec<u8> {
        let r = &mut self.r;
        r.set_sun_sky(sky);
        r.begin_frame();
        r.set_pipeline(self.untextured);
        r.draw_mesh(mesh, at, material);
        r.submit();
        r.read_pixels().expect("offscreen")
    }

    /// One frame under `sky`: the textured square, white times `base`, with a
    /// `clearcoat`.
    fn render_textured(&mut self, sky: &SunSky, base: [f32; 3], clearcoat: f32) -> Vec<u8> {
        let r = &mut self.r;
        r.set_sun_sky(sky);
        r.begin_frame();
        r.set_pipeline(self.textured);
        r.bind_texture(self.white);
        let material = MaterialParams {
            base_color: [base[0], base[1], base[2], 1.0],
            clearcoat,
            ..MaterialParams::default()
        };
        r.draw_mesh(
            self.square,
            &Transform::from_position(Vec3::new(0.0, 0.0, -6.0)),
            &material,
        );
        r.submit();
        r.read_pixels().expect("offscreen")
    }
}

fn coated(clearcoat: f32) -> MaterialParams {
    MaterialParams {
        clearcoat,
        ..MaterialParams::default()
    }
}

/// The cube the tests look at: a 2-unit cube on the ground 6 ahead, turned so
/// the camera sees its top and two sides, one more square on than the other.
fn cube_at() -> Transform {
    Transform {
        position: Vec3::new(0.0, 1.0, -6.0),
        rotation: Quat::from_rotation_y(0.6),
        scale: Vec3::splat(2.0),
    }
}

/// The middle of the cube's face whose outward normal is `normal` before it is
/// turned, and that face's normal after.
fn face(normal: Vec3) -> (Vec3, Vec3) {
    let at = cube_at();
    let n = at.rotation * normal;
    (at.position + n, n)
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

/// A BGRA pixel's colour bytes as `[r, g, b]`.
fn rgb(pixels: &[u8], (x, y): (u32, u32)) -> [f32; 3] {
    let i = ((y * WIDTH + x) * 4) as usize;
    [pixels[i + 2], pixels[i + 1], pixels[i]].map(f32::from)
}

/// The mean of a pixel's colour bytes.
fn brightness(pixels: &[u8], at: (u32, u32)) -> f32 {
    rgb(pixels, at).iter().sum::<f32>() / 3.0
}

fn differing(a: &[u8], b: &[u8]) -> usize {
    a.chunks_exact(4)
        .zip(b.chunks_exact(4))
        .filter(|(p, q)| p != q)
        .count()
}

#[test]
fn a_clear_coat_mirrors_the_sky_without_the_sun_most_where_seen_edge_on() {
    let Some(mut scene) = Scene::new() else {
        common::no_gpu();
        return;
    };
    let shade = afternoon(0.0);
    let (cube, at) = (scene.cube, cube_at());
    let matt = scene.render(&shade, cube, &at, &MaterialParams::default());
    let coat = scene.render(&shade, cube, &at, &coated(1.0));

    // Each visible face: how squarely the camera sees it, and how much
    // brighter the coat makes it.
    let faces = [Vec3::Y, Vec3::Z, Vec3::NEG_X].map(|normal| {
        let (centre, n) = face(normal);
        let facing = n.dot((eye() - centre).normalize());
        let pixel = projected(centre);
        let gain = brightness(&coat, pixel) - brightness(&matt, pixel);
        (facing, gain)
    });
    for (facing, gain) in faces {
        assert!(facing > 0.0, "every face looked at is in view");
        assert!(gain > 2.0, "a face seen {facing:.2} square on gains {gain}");
    }
    // The top, seen nearly edge-on, mirrors the most; of the sides (which
    // mirror the ground below the horizon), the one seen more edge-on gains
    // more than the one seen more square on.
    let [top, front, side] = faces;
    assert!(top.0 < side.0 && side.0 < front.0, "{faces:?}");
    assert!(top.1 > side.1 && side.1 > front.1, "{faces:?}");

    // The top mirrors the sky: it gains more blue than red.
    let pixel = projected(face(Vec3::Y).0);
    let (before, after) = (rgb(&matt, pixel), rgb(&coat, pixel));
    assert!(
        after[2] - before[2] > after[0] - before[0],
        "{before:?} → {after:?}"
    );

    // The coat lights nothing else: every pixel that differs is one the cube
    // covers.
    let empty = scene.render(&shade, cube, &at, &MaterialParams::default());
    let nothing = {
        let r = &mut scene.r;
        r.set_sun_sky(&shade);
        r.begin_frame();
        r.submit();
        r.read_pixels().expect("offscreen")
    };
    assert_eq!(differing(&empty, &matt), 0, "the same frame twice");
    for (i, (c, m)) in coat.chunks_exact(4).zip(matt.chunks_exact(4)).enumerate() {
        if c != m {
            assert_ne!(
                m,
                &nothing[i * 4..i * 4 + 4],
                "pixel {i} changed but the cube does not cover it"
            );
        }
    }
}

#[test]
fn a_clear_coat_catches_the_sun_in_a_glint() {
    let Some(mut scene) = Scene::new() else {
        common::no_gpu();
        return;
    };
    let (sphere, at) = (
        scene.sphere,
        Transform::from_position(Vec3::new(0.0, 1.5, -6.0)),
    );
    // The brightest the coat makes any pixel, over the matt sphere in the same
    // light.
    let mut peak_gain = |sky: &SunSky| {
        let matt = scene.render(sky, sphere, &at, &MaterialParams::default());
        let coat = scene.render(sky, sphere, &at, &coated(1.0));
        matt.chunks_exact(4)
            .zip(coat.chunks_exact(4))
            .map(|(m, c)| {
                let mean =
                    |p: &[u8]| (u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2])) as f32 / 3.0;
                mean(c) - mean(m)
            })
            .fold(0.0, f32::max)
    };
    let sunlit = peak_gain(&afternoon(1.15));
    let sunless = peak_gain(&afternoon(0.0));
    assert!(
        sunlit > sunless + 40.0,
        "a glint in the sun: {sunlit} brighter at best, against {sunless} with no sun"
    );
}

#[test]
fn both_pipelines_coat() {
    let Some(mut scene) = Scene::new() else {
        common::no_gpu();
        return;
    };
    let shade = afternoon(0.0);
    let matt = scene.render_textured(&shade, DARK, 0.0);
    let coat = scene.render_textured(&shade, DARK, 1.0);
    let pixel = projected(Vec3::new(0.0, 0.0, -6.0));
    let gain = brightness(&coat, pixel) - brightness(&matt, pixel);
    assert!(gain > 10.0, "the textured square gains {gain}");
}

#[test]
fn a_zero_negative_or_nan_coat_draws_exactly_as_no_coat() {
    let Some(mut scene) = Scene::new() else {
        common::no_gpu();
        return;
    };
    let sky = afternoon(1.15);
    let (cube, at) = (scene.cube, cube_at());
    let plain = scene.render(&sky, cube, &at, &MaterialParams::default());
    for clearcoat in [0.0, -1.0, f32::NAN] {
        let frame = scene.render(&sky, cube, &at, &coated(clearcoat));
        assert_eq!(differing(&plain, &frame), 0, "a coat of {clearcoat}");
    }
    let plain = scene.render_textured(&sky, DARK, 0.0);
    let frame = scene.render_textured(&sky, DARK, -1.0);
    assert_eq!(differing(&plain, &frame), 0, "a textured coat of -1");
}
