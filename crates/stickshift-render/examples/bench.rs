// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! The renderer benchmark: fixed synthetic scenes rendered offscreen, timed on
//! the CPU and the GPU.
//!
//! Each scenario draws a grid of meshes (half untextured, half textured, grouped
//! by pipeline as a game would) under the default sun, with shadows on or off,
//! into a 1920 x 1080 offscreen target with 4x MSAA. After a warm-up, every
//! frame records:
//!
//! - **CPU time:** `begin_frame` through `submit_async`, the cost of recording
//!   and encoding the frame on the calling thread;
//! - **GPU time:** the frame's command-buffer execution time, as
//!   [`MetalRenderer::gpu_frame_time`] reports it;
//!
//! and the run reports **frames per second** as frames over wall time, with up
//! to three frames in flight. Nothing waits for vsync, so this is throughput,
//! not a display rate.
//!
//! ```sh
//! cargo run --release -p stickshift-render --example bench
//! ```
//!
//! Results for real hardware are recorded in `docs/BENCHMARKS.md`. Needs a
//! Metal device.

use std::time::{Duration, Instant};

use stickshift_math::glam::{Mat4, Quat, Vec3};
use stickshift_math::Transform;
use stickshift_render::MetalRenderer;
use stickshift_render_api::{
    ColorSpace, FrameRecorder, MaterialParams, MeshData, MeshHandle, PipelineDescriptor,
    PipelineHandle, PipelineKind, RenderDevice, RenderSettings, ShadowQuality, TextureData,
};

const WIDTH: u32 = 1920;
const HEIGHT: u32 = 1080;
const WARM_UP_FRAMES: u32 = 60;
const MEASURED_FRAMES: u32 = 300;

/// What a scenario draws.
#[derive(Clone, Copy)]
enum Shape {
    /// A unit cube: 12 triangles.
    Cube,
    /// A UV sphere of `segments` x `segments / 2` quads.
    Sphere { segments: u32 },
}

struct Scenario {
    name: &'static str,
    /// Draws per frame, laid out on a square grid.
    draws: u32,
    shape: Shape,
    shadows: ShadowQuality,
}

const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "100 cubes",
        draws: 100,
        shape: Shape::Cube,
        shadows: ShadowQuality::High,
    },
    Scenario {
        name: "1,000 cubes",
        draws: 1_000,
        shape: Shape::Cube,
        shadows: ShadowQuality::High,
    },
    Scenario {
        name: "1,000 cubes, shadows off",
        draws: 1_000,
        shape: Shape::Cube,
        shadows: ShadowQuality::Off,
    },
    Scenario {
        name: "5,000 cubes",
        draws: 5_000,
        shape: Shape::Cube,
        shadows: ShadowQuality::High,
    },
    Scenario {
        name: "16 dense spheres (1M triangles)",
        draws: 16,
        shape: Shape::Sphere { segments: 256 },
        shadows: ShadowQuality::High,
    },
];

/// One scenario's measurements.
struct Result {
    triangles: u64,
    cpu: Vec<Duration>,
    gpu: Vec<Duration>,
    fps: f64,
}

fn main() {
    let Some(mut renderer) = MetalRenderer::new_offscreen(WIDTH, HEIGHT) else {
        eprintln!("bench: no Metal device");
        std::process::exit(1);
    };
    println!("Device: {}", renderer.device_name());
    println!(
        "Target: {WIDTH} x {HEIGHT} offscreen, 4x MSAA; {WARM_UP_FRAMES} warm-up + \
         {MEASURED_FRAMES} measured frames per scenario"
    );
    if cfg!(debug_assertions) {
        println!("Warning: a debug build; run with --release for meaningful CPU times.");
    }
    println!();
    println!(
        "| Scenario | Draws | Triangles | CPU ms (mean / p95) | GPU ms (mean / p95) | Frames/s |"
    );
    println!("|---|---:|---:|---:|---:|---:|");
    for scenario in SCENARIOS {
        let r = run(&mut renderer, scenario);
        println!(
            "| {} | {} | {} | {:.2} / {:.2} | {:.2} / {:.2} | {:.0} |",
            scenario.name,
            thousands(u64::from(scenario.draws)),
            thousands(r.triangles),
            ms(mean(&r.cpu)),
            ms(percentile(&r.cpu, 0.95)),
            ms(mean(&r.gpu)),
            ms(percentile(&r.gpu, 0.95)),
            r.fps,
        );
    }
}

fn run(renderer: &mut MetalRenderer, scenario: &Scenario) -> Result {
    renderer.set_render_settings(&RenderSettings {
        shadows: scenario.shadows,
        ..RenderSettings::default()
    });

    // Geometry: the same shape on both pipelines.
    let (positions, normals, uvs, indices) = match scenario.shape {
        Shape::Cube => cube(),
        Shape::Sphere { segments } => sphere(segments),
    };
    let untextured_mesh = renderer.create_mesh(&MeshData {
        vertices: &interleave(&positions, &normals, |_| vec![0.8, 0.8, 0.8]),
        indices: &indices,
        layout: PipelineKind::VertexColor.vertex_layout(),
    });
    let textured_mesh = renderer.create_mesh(&MeshData {
        vertices: &interleave(&positions, &normals, |i| uvs[i].to_vec()),
        indices: &indices,
        layout: PipelineKind::Textured.vertex_layout(),
    });
    let untextured = renderer.create_pipeline(&PipelineDescriptor::new(PipelineKind::VertexColor));
    let textured = renderer.create_pipeline(&PipelineDescriptor::new(PipelineKind::Textured));
    let checker = checker_texture();
    let texture = renderer.create_texture(&TextureData {
        width: 256,
        height: 256,
        rgba8: &checker,
        color_space: ColorSpace::Srgb,
    });

    // A square grid, the camera looking down on it at an angle.
    let side = (scenario.draws as f32).sqrt().ceil() as u32;
    let spacing = match scenario.shape {
        Shape::Cube => 2.5,
        Shape::Sphere { .. } => 3.0,
    };
    let extent = side as f32 * spacing;
    let transforms: Vec<Transform> = (0..scenario.draws)
        .map(|i| {
            let (x, z) = ((i % side) as f32, (i / side) as f32);
            Transform::from_position_rotation(
                Vec3::new(
                    (x - side as f32 / 2.0) * spacing,
                    0.0,
                    (z - side as f32 / 2.0) * spacing,
                ),
                Quat::from_rotation_y(i as f32 * 0.37),
            )
        })
        .collect();
    let eye = Vec3::new(0.0, extent * 0.55 + 4.0, extent * 0.75 + 6.0);
    renderer.set_view_projection(
        Mat4::perspective_rh(
            50f32.to_radians(),
            WIDTH as f32 / HEIGHT as f32,
            0.5,
            extent * 4.0,
        ) * Mat4::look_at_rh(eye, Vec3::ZERO, Vec3::Y),
    );
    renderer.set_camera_position(eye);

    let half = transforms.len() / 2;
    let material = MaterialParams::default();
    let mut cpu = Vec::with_capacity(MEASURED_FRAMES as usize);
    let mut gpu = Vec::with_capacity(MEASURED_FRAMES as usize);
    let mut first_measured = 0;
    let mut collected = renderer.completed_frame_count();
    let mut wall_start = Instant::now();

    for frame in 0..WARM_UP_FRAMES + MEASURED_FRAMES {
        if frame == WARM_UP_FRAMES {
            first_measured = renderer.submitted_frame_count() + 1;
            wall_start = Instant::now();
        }
        let start = Instant::now();
        renderer.begin_frame();
        draw_group(
            renderer,
            untextured,
            None,
            untextured_mesh,
            &transforms[..half],
            &material,
        );
        draw_group(
            renderer,
            textured,
            Some(texture),
            textured_mesh,
            &transforms[half..],
            &material,
        );
        renderer
            .submit_async()
            .expect("an offscreen frame always opens");
        if frame >= WARM_UP_FRAMES {
            cpu.push(start.elapsed());
        }
        // Collect GPU times as frames complete: only the last 64 are kept.
        collect(renderer, &mut collected, first_measured, &mut gpu);
    }
    let submitted = renderer.submitted_frame_count();
    while renderer.completed_frame_count() < submitted {
        std::thread::sleep(Duration::from_micros(100));
    }
    let wall = wall_start.elapsed();
    collect(renderer, &mut collected, first_measured, &mut gpu);

    renderer.destroy_mesh(untextured_mesh);
    renderer.destroy_mesh(textured_mesh);
    renderer.destroy_texture(texture);
    renderer.destroy_pipeline(untextured);
    renderer.destroy_pipeline(textured);

    Result {
        triangles: u64::from(scenario.draws) * (indices.len() as u64 / 3),
        cpu,
        gpu,
        fps: f64::from(MEASURED_FRAMES) / wall.as_secs_f64(),
    }
}

fn draw_group(
    renderer: &mut MetalRenderer,
    pipeline: PipelineHandle,
    texture: Option<stickshift_render_api::TextureHandle>,
    mesh: MeshHandle,
    transforms: &[Transform],
    material: &MaterialParams,
) {
    renderer.set_pipeline(pipeline);
    if let Some(texture) = texture {
        renderer.bind_texture(texture);
    }
    for transform in transforms {
        renderer.draw_mesh(mesh, transform, material);
    }
}

/// Append the GPU times of measured frames completed since `collected`.
fn collect(
    renderer: &MetalRenderer,
    collected: &mut u64,
    first_measured: u64,
    gpu: &mut Vec<Duration>,
) {
    let completed = renderer.completed_frame_count();
    for frame in *collected + 1..=completed {
        if first_measured != 0 && frame >= first_measured {
            if let Some(time) = renderer.gpu_frame_time(frame) {
                gpu.push(time);
            }
        }
    }
    *collected = completed;
}

/// `[pos, normal, extra]` vertices as bytes, `extra` per vertex index.
fn interleave(
    positions: &[[f32; 3]],
    normals: &[[f32; 3]],
    extra: impl Fn(usize) -> Vec<f32>,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (i, (p, n)) in positions.iter().zip(normals).enumerate() {
        for f in p.iter().chain(n).copied().chain(extra(i)) {
            bytes.extend_from_slice(&f.to_ne_bytes());
        }
    }
    bytes
}

type Geometry = (Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<u32>);

/// A unit cube with per-face normals and UVs.
fn cube() -> Geometry {
    let (mut positions, mut normals, mut uvs, mut indices) = (vec![], vec![], vec![], vec![]);
    for (normal, up) in [
        (Vec3::X, Vec3::Y),
        (Vec3::NEG_X, Vec3::Y),
        (Vec3::Y, Vec3::Z),
        (Vec3::NEG_Y, Vec3::Z),
        (Vec3::Z, Vec3::Y),
        (Vec3::NEG_Z, Vec3::Y),
    ] {
        let right = up.cross(normal);
        let base = positions.len() as u32;
        for (u, v) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
            let p = (normal + right * (u * 2.0 - 1.0) + up * (v * 2.0 - 1.0)) * 0.5;
            positions.push(p.to_array());
            normals.push(normal.to_array());
            uvs.push([u, v]);
        }
        indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    (positions, normals, uvs, indices)
}

/// A unit-diameter UV sphere of `segments` x `segments / 2` quads.
fn sphere(segments: u32) -> Geometry {
    let rings = segments / 2;
    let (mut positions, mut normals, mut uvs, mut indices) = (vec![], vec![], vec![], vec![]);
    for r in 0..=rings {
        let v = r as f32 / rings as f32;
        let theta = v * std::f32::consts::PI;
        for s in 0..=segments {
            let u = s as f32 / segments as f32;
            let phi = u * std::f32::consts::TAU;
            let n = Vec3::new(
                theta.sin() * phi.cos(),
                theta.cos(),
                theta.sin() * phi.sin(),
            );
            positions.push((n * 0.5).to_array());
            normals.push(n.to_array());
            uvs.push([u, v]);
        }
    }
    let row = segments + 1;
    for r in 0..rings {
        for s in 0..segments {
            let (a, b) = (r * row + s, (r + 1) * row + s);
            indices.extend([a, b, a + 1, a + 1, b, b + 1]);
        }
    }
    (positions, normals, uvs, indices)
}

/// A 256 x 256 sRGB checkerboard.
fn checker_texture() -> Vec<u8> {
    (0..256u32 * 256)
        .flat_map(|i| {
            let (x, y) = (i % 256 / 32, i / 256 / 32);
            if (x + y).is_multiple_of(2) {
                [220, 220, 220, 255]
            } else {
                [60, 90, 140, 255]
            }
        })
        .collect()
}

fn mean(samples: &[Duration]) -> Duration {
    if samples.is_empty() {
        return Duration::ZERO;
    }
    samples.iter().sum::<Duration>() / samples.len() as u32
}

fn percentile(samples: &[Duration], p: f64) -> Duration {
    if samples.is_empty() {
        return Duration::ZERO;
    }
    let mut sorted = samples.to_vec();
    sorted.sort();
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// `n` with comma thousands separators.
fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}
