// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! **Clustered light binning**: how a frame's point and spot lights reach the
//! shaders without every pixel shading every light.
//!
//! # The technique
//!
//! Clustered forward shading. The view is split into a fixed grid of
//! **clusters**: [`CLUSTER_TILES_X`] × [`CLUSTER_TILES_Y`] screen tiles, each cut
//! into [`CLUSTER_SLICES`] depth slices that grow exponentially with view depth
//! (thin near the camera, deep far away), [`CLUSTER_COUNT`] in all. Once per
//! frame the CPU works out which clusters each light can reach and writes, per
//! cluster, the list of lights that touch it. A lit fragment finds its own
//! cluster from its pixel position and view depth and shades only that list.
//!
//! It suits Apple's tile-based GPUs: the scene stays a single forward pass with
//! MSAA resolved in tile memory, with no G-buffer to write out and read back,
//! and no depth pre-pass (clusters are cut by depth ranges, not by a depth
//! buffer). The binning is plain arithmetic on the CPU, a few microseconds for a
//! street's worth of lights, and is unit-tested here without a GPU.
//!
//! # Conservative by construction
//!
//! A light may be listed in a cluster it does not actually reach (the shader's
//! falloff is exactly zero there, so that only costs time), but never left out
//! of one it does. Each light's sphere of influence (its position and
//! [`range`](Light::range); a spot's cone is bounded by the same sphere) is
//! boxed, the box's corners projected, and the screen rectangle and depth range
//! they span widened by a small margin to cover rounding on the GPU. A light
//! whose sphere reaches behind the camera covers the whole screen. The
//! `binning_never_misses_a_lit_point` test checks this against the same
//! cluster formula the shader uses.
//!
//! # Budget
//!
//! - [`MAX_LIGHTS`] lights per frame (the seam's limit).
//! - [`MAX_LIGHT_INDICES`] light-to-cluster entries per frame, summed over all
//!   lights. A light that covers `n` clusters uses `n` entries, so the budget is
//!   generous for the case it exists for: a street of lamps and headlights each
//!   covering a few tiles and slices. A frame of large, screen-filling lights
//!   reaches it sooner; a light that no longer fits is **dropped whole** (never
//!   lit in only some clusters), lights added earlier keeping priority, and the
//!   drop is counted
//!   ([`MetalRenderer::dropped_light_count`](crate::MetalRenderer::dropped_light_count)).
//! - Per fragment, the cost is the length of its cluster's list.
//!
//! # GPU layout
//!
//! Each frame in flight owns one [`LIGHT_REGION_SIZE`]-byte region of a ring,
//! selected like the uniform and light rings, holding a [`ClusterInfo`] header,
//! the frame's [`GpuLight`]s, the per-cluster `(offset, count)` table and the
//! `u16` light-index list, at the 256-byte-aligned offsets below. The scene pass
//! binds them at fragment buffers 3 to 6.

use stickshift_math::glam::{Mat4, Vec2, Vec3, Vec4};
use stickshift_render_api::light::cone_cosines;
use stickshift_render_api::{Light, LightKind, MAX_LIGHTS};

/// Screen tiles across. The grid is fixed, whatever the drawable's size.
pub const CLUSTER_TILES_X: u32 = 16;
/// Screen tiles down.
pub const CLUSTER_TILES_Y: u32 = 9;
/// Depth slices per tile.
pub const CLUSTER_SLICES: u32 = 24;
/// Clusters in the grid.
pub const CLUSTER_COUNT: usize = (CLUSTER_TILES_X * CLUSTER_TILES_Y * CLUSTER_SLICES) as usize;
/// View depth where the exponential slicing starts. Slice 0 also holds
/// everything nearer.
pub const CLUSTER_NEAR: f32 = 1.0;
/// View depth where the exponential slicing ends. The last slice also holds
/// everything farther.
pub const CLUSTER_FAR: f32 = 400.0;
/// Light-to-cluster entries one frame can hold (see [the budget](self#budget)).
pub const MAX_LIGHT_INDICES: usize = 32_768;

/// Relative widening of a light's depth range, to cover the GPU's `log2`.
const DEPTH_MARGIN: f32 = 1e-3;
/// Widening of a light's screen rectangle, in NDC, to cover rounding at tile
/// edges.
const NDC_MARGIN: f32 = 1e-3;
/// Below this clip `w` a box corner counts as at or behind the eye.
const MIN_CLIP_W: f32 = 1e-5;

/// One light as the shaders read it. Matches `LocalLight` in
/// `rasterization.metal`: four `float4`s, 64 bytes, no padding to get wrong.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GpuLight {
    /// `xyz` world position, `w` = `1 / range²`.
    pub position_inv_range_sq: [f32; 4],
    /// `xyz` = colour × intensity (linear), `w` unused.
    pub radiance: [f32; 4],
    /// `xyz` = the unit direction a spot's light travels, `w` = the cosine of
    /// its outer half angle. Zero for a point light.
    pub spot_direction: [f32; 4],
    /// `x` = the cosine of a spot's inner half angle, `y` = `1` for a spot and
    /// `0` for a point light.
    pub spot_params: [f32; 4],
}

impl GpuLight {
    /// Pack `light` for the GPU.
    #[must_use]
    pub fn from_light(light: &Light) -> Self {
        let p = light.position;
        let radiance = light.color.map(|c| c * light.intensity);
        let (spot_direction, spot_params) = match light.kind {
            LightKind::Point => ([0.0; 4], [0.0; 4]),
            LightKind::Spot {
                direction,
                inner_angle_deg,
                outer_angle_deg,
            } => {
                let d = direction.normalize();
                let (cos_inner, cos_outer) = cone_cosines(inner_angle_deg, outer_angle_deg);
                ([d.x, d.y, d.z, cos_outer], [cos_inner, 1.0, 0.0, 0.0])
            }
        };
        Self {
            position_inv_range_sq: [p.x, p.y, p.z, 1.0 / (light.range * light.range)],
            radiance: [radiance[0], radiance[1], radiance[2], 0.0],
            spot_direction,
            spot_params,
        }
    }
}

/// The per-frame header the shaders read first. Matches `ClusterInfo` in
/// `rasterization.metal`: a `float4` and a `uint4`, 32 bytes.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClusterInfo {
    /// Tiles per pixel across and down, then the slice scale and bias: a
    /// fragment's slice is `log2(depth) * scale + bias`.
    pub tile_and_slice: [f32; 4],
    /// `x` = how many lights the frame uploaded. `0` skips the light loop.
    pub counts: [u32; 4],
}

impl ClusterInfo {
    /// The header for a `width` × `height` target with `light_count` lights.
    #[must_use]
    pub fn new(width: u32, height: u32, light_count: usize) -> Self {
        let (scale, bias) = slice_scale_bias();
        Self {
            tile_and_slice: [
                CLUSTER_TILES_X as f32 / width.max(1) as f32,
                CLUSTER_TILES_Y as f32 / height.max(1) as f32,
                scale,
                bias,
            ],
            counts: [light_count as u32, 0, 0, 0],
        }
    }
}

/// Byte offset of the [`GpuLight`] array within a frame's region.
pub const LIGHTS_OFFSET: u64 = 256;
/// Byte offset of the per-cluster `(offset, count)` table.
pub const CLUSTERS_OFFSET: u64 = LIGHTS_OFFSET + (MAX_LIGHTS * size_of::<GpuLight>()) as u64;
/// Byte offset of the `u16` light-index list.
pub const INDICES_OFFSET: u64 = CLUSTERS_OFFSET + (CLUSTER_COUNT * 8) as u64;
/// Bytes per frame region, rounded up to 256 so every region starts aligned.
pub const LIGHT_REGION_SIZE: u64 =
    (INDICES_OFFSET + (MAX_LIGHT_INDICES * 2) as u64).div_ceil(256) * 256;

// Metal wants constant-buffer offsets 256-byte aligned on macOS; every binding
// is kept that way. Light indices are `u16`, so a frame's lights must fit one.
const _: () = assert!(size_of::<GpuLight>() == 64);
const _: () = assert!(size_of::<ClusterInfo>() == 32);
const _: () = assert!(CLUSTERS_OFFSET.is_multiple_of(256));
const _: () = assert!(INDICES_OFFSET.is_multiple_of(256));
const _: () = assert!(MAX_LIGHTS <= u16::MAX as usize);

/// The `(scale, bias)` that turn `log2(view depth)` into a slice coordinate.
fn slice_scale_bias() -> (f32, f32) {
    let scale = CLUSTER_SLICES as f32 / (CLUSTER_FAR / CLUSTER_NEAR).log2();
    (scale, -CLUSTER_NEAR.log2() * scale)
}

/// The depth slice holding `view_depth`, as the shader computes it.
#[must_use]
pub fn slice_of(view_depth: f32) -> u32 {
    let (scale, bias) = slice_scale_bias();
    let s = view_depth.max(1e-4).log2() * scale + bias;
    s.clamp(0.0, (CLUSTER_SLICES - 1) as f32) as u32
}

/// The cluster a fragment at pixel `(x, y)` (origin top-left) and `view_depth`
/// falls in, on a `width` × `height` target: the CPU twin of the shader's
/// `cluster_index`.
#[must_use]
pub fn cluster_of(x: f32, y: f32, view_depth: f32, width: u32, height: u32) -> usize {
    let info = ClusterInfo::new(width, height, 0);
    let tx = ((x * info.tile_and_slice[0]).max(0.0) as u32).min(CLUSTER_TILES_X - 1);
    let ty = ((y * info.tile_and_slice[1]).max(0.0) as u32).min(CLUSTER_TILES_Y - 1);
    cluster_index(tx, ty, slice_of(view_depth))
}

fn cluster_index(tx: u32, ty: u32, slice: u32) -> usize {
    ((slice * CLUSTER_TILES_Y + ty) * CLUSTER_TILES_X + tx) as usize
}

/// The block of clusters a light can reach: inclusive tile and slice ranges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClusterBox {
    /// First and last tile across.
    pub x: (u32, u32),
    /// First and last tile down (from the top).
    pub y: (u32, u32),
    /// First and last depth slice.
    pub z: (u32, u32),
}

impl ClusterBox {
    /// How many clusters the box holds.
    #[must_use]
    pub fn len(&self) -> usize {
        let span = |(a, b): (u32, u32)| (b - a + 1) as usize;
        span(self.x) * span(self.y) * span(self.z)
    }

    /// Whether the box holds no clusters (never, for a box [`cluster_box`] returns).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn clusters(self) -> impl Iterator<Item = usize> {
        (self.z.0..=self.z.1).flat_map(move |z| {
            (self.y.0..=self.y.1)
                .flat_map(move |y| (self.x.0..=self.x.1).map(move |x| cluster_index(x, y, z)))
        })
    }
}

/// The clusters `light` can reach seen through `view_projection`, or `None`
/// when it reaches nothing on screen (wholly behind the camera, or off to a
/// side). Conservative: see [the module docs](self#conservative-by-construction).
#[must_use]
pub fn cluster_box(light: &Light, view_projection: Mat4) -> Option<ClusterBox> {
    let r = light.range;
    // A fragment's view depth is its clip `w`: a linear function of the world
    // position, so the sphere's depth range is the centre's ± the range scaled
    // by that function's gradient.
    let w_row = view_projection.row(3);
    let depth = w_row.dot(light.position.extend(1.0));
    let reach = r * w_row.truncate().length();
    let (near, far) = (depth - reach, depth + reach);
    if far <= 0.0 {
        return None;
    }
    let z = (
        slice_of(near.max(0.0) * (1.0 - DEPTH_MARGIN)),
        slice_of(far * (1.0 + DEPTH_MARGIN)),
    );

    // The screen rectangle of the sphere's bounding box. If a corner is at or
    // behind the eye the projection wraps, so take the whole screen instead.
    let mut min = Vec2::splat(f32::INFINITY);
    let mut max = Vec2::splat(f32::NEG_INFINITY);
    let mut wraps = false;
    for corner in 0..8 {
        let offset = Vec3::new(
            if corner & 1 == 0 { -r } else { r },
            if corner & 2 == 0 { -r } else { r },
            if corner & 4 == 0 { -r } else { r },
        );
        let clip: Vec4 = view_projection * (light.position + offset).extend(1.0);
        if clip.w <= MIN_CLIP_W {
            wraps = true;
            break;
        }
        let ndc = Vec2::new(clip.x, clip.y) / clip.w;
        min = min.min(ndc);
        max = max.max(ndc);
    }
    let (x, y) = if wraps {
        ((0, CLUSTER_TILES_X - 1), (0, CLUSTER_TILES_Y - 1))
    } else {
        let (min, max) = (min - NDC_MARGIN, max + NDC_MARGIN);
        if max.x < -1.0 || min.x > 1.0 || max.y < -1.0 || min.y > 1.0 {
            return None;
        }
        let tile = |ndc: f32, tiles: u32| -> u32 {
            ((((ndc.clamp(-1.0, 1.0) + 1.0) * 0.5) * tiles as f32) as u32).min(tiles - 1)
        };
        // Tile rows count down from the top, where NDC y is +1.
        (
            (tile(min.x, CLUSTER_TILES_X), tile(max.x, CLUSTER_TILES_X)),
            (tile(-max.y, CLUSTER_TILES_Y), tile(-min.y, CLUSTER_TILES_Y)),
        )
    };
    Some(ClusterBox { x, y, z })
}

/// A frame's binned lights, ready to copy into its GPU region. Kept by the
/// backend and refilled every frame without allocating.
#[derive(Debug)]
pub struct LightBins {
    lights: Vec<GpuLight>,
    /// Per cluster: `(offset into indices, count)`.
    clusters: Vec<[u32; 2]>,
    indices: Vec<u16>,
    boxes: Vec<(ClusterBox, u16)>,
    dropped: usize,
}

impl Default for LightBins {
    fn default() -> Self {
        Self::new()
    }
}

impl LightBins {
    /// Empty bins with room for a full frame.
    #[must_use]
    pub fn new() -> Self {
        Self {
            lights: Vec::with_capacity(MAX_LIGHTS),
            clusters: vec![[0, 0]; CLUSTER_COUNT],
            indices: Vec::with_capacity(MAX_LIGHT_INDICES),
            boxes: Vec::with_capacity(MAX_LIGHTS),
            dropped: 0,
        }
    }

    /// Bin `lights` (at most [`MAX_LIGHTS`] are looked at) for a frame drawn
    /// through `view_projection`, replacing the previous frame's bins.
    ///
    /// Lights that reach nothing on screen are left out. Lights that no longer
    /// fit [`MAX_LIGHT_INDICES`] are dropped whole, in order, and counted in
    /// [`dropped`](Self::dropped). Each cluster lists its lights in the order
    /// they were added, so the same frame always bins the same way.
    pub fn bin(&mut self, lights: &[Light], view_projection: Mat4) {
        self.lights.clear();
        self.indices.clear();
        self.boxes.clear();
        self.dropped = 0;
        for cluster in &mut self.clusters {
            *cluster = [0, 0];
        }

        let mut total = 0;
        for light in lights.iter().take(MAX_LIGHTS) {
            let Some(cbox) = cluster_box(light, view_projection) else {
                continue;
            };
            let n = cbox.len();
            if total + n > MAX_LIGHT_INDICES {
                self.dropped += 1;
                continue;
            }
            total += n;
            let index = self.lights.len() as u16;
            self.lights.push(GpuLight::from_light(light));
            self.boxes.push((cbox, index));
            for cluster in cbox.clusters() {
                self.clusters[cluster][1] += 1;
            }
        }

        // Counts to offsets; each count restarts at 0 and is rebuilt as the
        // second pass writes the indices.
        let mut offset = 0;
        for cluster in &mut self.clusters {
            let count = cluster[1];
            *cluster = [offset, 0];
            offset += count;
        }
        self.indices.resize(total, 0);
        for &(cbox, index) in &self.boxes {
            for cluster in cbox.clusters() {
                let [start, count] = &mut self.clusters[cluster];
                self.indices[(*start + *count) as usize] = index;
                *count += 1;
            }
        }
    }

    /// The lights uploaded this frame, in the order they were added.
    #[must_use]
    pub fn lights(&self) -> &[GpuLight] {
        &self.lights
    }

    /// Per cluster, the `(offset, count)` of its run in [`indices`](Self::indices).
    #[must_use]
    pub fn clusters(&self) -> &[[u32; 2]] {
        &self.clusters
    }

    /// Every cluster's light indices, run after run.
    #[must_use]
    pub fn indices(&self) -> &[u16] {
        &self.indices
    }

    /// The indices (into [`lights`](Self::lights)) of the lights `cluster` lists.
    #[must_use]
    pub fn lights_in(&self, cluster: usize) -> &[u16] {
        let [start, count] = self.clusters[cluster];
        &self.indices[start as usize..(start + count) as usize]
    }

    /// Lights dropped this frame for want of index space.
    #[must_use]
    pub fn dropped(&self) -> usize {
        self.dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: u32 = 1280;
    const H: u32 = 720;

    fn camera() -> Mat4 {
        Mat4::perspective_rh(60f32.to_radians(), W as f32 / H as f32, 0.3, 300.0)
            * Mat4::look_at_rh(
                Vec3::new(0.0, 2.0, 6.0),
                Vec3::new(0.0, 1.0, -20.0),
                Vec3::Y,
            )
    }

    /// A small deterministic generator, so the property test is repeatable.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> f32 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 40) as f32 / (1u64 << 24) as f32
        }
        fn range(&mut self, lo: f32, hi: f32) -> f32 {
            lo + (hi - lo) * self.next()
        }
    }

    #[test]
    fn layouts_match_the_shader() {
        assert_eq!(size_of::<GpuLight>(), 64);
        assert_eq!(size_of::<ClusterInfo>(), 32);
        assert!(LIGHTS_OFFSET.is_multiple_of(256));
        assert!(LIGHT_REGION_SIZE.is_multiple_of(256));
        assert!(INDICES_OFFSET + (MAX_LIGHT_INDICES * 2) as u64 <= LIGHT_REGION_SIZE);
    }

    #[test]
    fn slices_cover_every_depth_in_order() {
        assert_eq!(slice_of(0.0), 0);
        assert_eq!(slice_of(CLUSTER_NEAR * 0.5), 0);
        assert_eq!(slice_of(CLUSTER_FAR * 10.0), CLUSTER_SLICES - 1);
        let mut previous = 0;
        for step in 1..4000 {
            let s = slice_of(step as f32 * 0.25);
            assert!(s >= previous && s - previous <= 1);
            previous = s;
        }
        assert_eq!(previous, CLUSTER_SLICES - 1);
    }

    #[test]
    fn every_pixel_maps_to_a_cluster_in_the_grid() {
        assert_eq!(cluster_of(0.0, 0.0, 0.5, W, H), 0);
        assert_eq!(
            cluster_of(W as f32 - 0.5, H as f32 - 0.5, 1e6, W, H),
            CLUSTER_COUNT - 1
        );
    }

    #[test]
    fn binning_never_misses_a_lit_point() {
        // Random lights in and around the view, and random points inside each
        // one's range: wherever such a point is visible, the cluster the shader
        // would put it in must list the light.
        let vp = camera();
        let mut rng = Lcg(7);
        let lights: Vec<Light> = (0..MAX_LIGHTS)
            .map(|i| {
                let position = Vec3::new(
                    rng.range(-30.0, 30.0),
                    rng.range(-2.0, 12.0),
                    rng.range(-150.0, 10.0),
                );
                let range = rng.range(0.5, 14.0);
                if i % 3 == 0 {
                    Light::spot(position, Vec3::NEG_Y, [1.0; 3], 10.0, range, 20.0, 35.0)
                } else {
                    Light::point(position, [1.0; 3], 10.0, range)
                }
            })
            .collect();
        let mut bins = LightBins::new();
        bins.bin(&lights, vp);
        // Lights near the camera cover much of the screen, so a frame this busy
        // may drop a few; the property is about the ones it keeps.
        assert!(
            bins.lights().len() > MAX_LIGHTS / 2,
            "{} kept",
            bins.lights().len()
        );

        // Map each uploaded light back to its source by position.
        let source_of = |gpu: &GpuLight| {
            lights
                .iter()
                .position(|l| l.position.to_array() == gpu.position_inv_range_sq[..3])
                .expect("uploaded lights come from the frame")
        };
        let uploaded: Vec<usize> = bins.lights().iter().map(source_of).collect();

        let mut checked = 0;
        for &light_index in &uploaded {
            let light = &lights[light_index];
            for _ in 0..200 {
                let dir = Vec3::new(
                    rng.range(-1.0, 1.0),
                    rng.range(-1.0, 1.0),
                    rng.range(-1.0, 1.0),
                );
                if dir.length_squared() > 1.0 || dir.length_squared() == 0.0 {
                    continue;
                }
                let point = light.position + dir * light.range * 0.999;
                let clip = vp * point.extend(1.0);
                if clip.w <= 0.3 {
                    continue;
                }
                let ndc = Vec2::new(clip.x, clip.y) / clip.w;
                if ndc.x.abs() >= 1.0 || ndc.y.abs() >= 1.0 {
                    continue;
                }
                let pixel = Vec2::new(
                    (ndc.x + 1.0) * 0.5 * W as f32,
                    (1.0 - ndc.y) * 0.5 * H as f32,
                );
                let cluster = cluster_of(pixel.x, pixel.y, clip.w, W, H);
                let listed = bins
                    .lights_in(cluster)
                    .iter()
                    .any(|&i| uploaded[i as usize] == light_index);
                assert!(
                    listed,
                    "light {light_index} reaches {point} (cluster {cluster}) but is not listed there"
                );
                checked += 1;
            }
        }
        assert!(
            checked > 10_000,
            "only {checked} visible points were checked"
        );
    }

    #[test]
    fn lights_out_of_view_are_left_out() {
        let vp = camera();
        // Far behind the camera (which looks toward -Z from z = 6).
        let behind = Light::point(Vec3::new(0.0, 2.0, 60.0), [1.0; 3], 10.0, 5.0);
        // Well off to the side.
        let aside = Light::point(Vec3::new(400.0, 2.0, -10.0), [1.0; 3], 10.0, 5.0);
        assert_eq!(cluster_box(&behind, vp), None);
        assert_eq!(cluster_box(&aside, vp), None);

        let mut bins = LightBins::new();
        bins.bin(&[behind, aside], vp);
        assert!(bins.lights().is_empty());
        assert!(bins.indices().is_empty());
        assert!(bins.clusters().iter().all(|&[_, count]| count == 0));
    }

    #[test]
    fn a_light_around_the_camera_covers_the_whole_screen() {
        let vp = camera();
        let around = Light::point(Vec3::new(0.0, 2.0, 6.0), [1.0; 3], 10.0, 4.0);
        let cbox = cluster_box(&around, vp).expect("it lights what is in front");
        assert_eq!(cbox.x, (0, CLUSTER_TILES_X - 1));
        assert_eq!(cbox.y, (0, CLUSTER_TILES_Y - 1));
        assert_eq!(cbox.z.0, 0);
    }

    #[test]
    fn a_small_light_covers_few_clusters() {
        let vp = camera();
        let lamp = Light::point(Vec3::new(4.0, 5.0, -40.0), [1.0; 3], 30.0, 3.0);
        let cbox = cluster_box(&lamp, vp).unwrap();
        assert!(cbox.len() <= 12, "{cbox:?} is {} clusters", cbox.len());
    }

    #[test]
    fn a_frame_over_budget_drops_whole_lights_in_order() {
        let vp = camera();
        // Lights around the camera cover the whole screen, a third or more of
        // every tile's slices: only a handful fit the index budget.
        let huge = Light::point(Vec3::new(0.0, 2.0, 0.0), [1.0; 3], 10.0, 60.0);
        let per_light = cluster_box(&huge, vp).unwrap().len();
        let fits = MAX_LIGHT_INDICES / per_light;
        let lights = vec![huge; MAX_LIGHTS];

        let mut bins = LightBins::new();
        bins.bin(&lights, vp);
        assert_eq!(bins.lights().len(), fits);
        assert_eq!(bins.dropped(), MAX_LIGHTS - fits);
        assert!(bins.indices().len() <= MAX_LIGHT_INDICES);

        // Each covered cluster lists the kept lights in the order they came.
        let listed = bins.lights_in(cluster_of(640.0, 360.0, 10.0, W, H));
        assert_eq!(listed, (0..fits as u16).collect::<Vec<_>>().as_slice());
    }

    #[test]
    fn rebinning_replaces_the_previous_frame() {
        let vp = camera();
        let lamp = Light::point(Vec3::new(4.0, 5.0, -40.0), [1.0; 3], 30.0, 3.0);
        let mut bins = LightBins::new();
        bins.bin(&[lamp, lamp, lamp], vp);
        assert_eq!(bins.lights().len(), 3);
        bins.bin(&[lamp], vp);
        assert_eq!(bins.lights().len(), 1);
        let total: u32 = bins.clusters().iter().map(|c| c[1]).sum();
        assert_eq!(total as usize, bins.indices().len());
        bins.bin(&[], vp);
        assert!(bins.lights().is_empty() && bins.indices().is_empty());
    }

    #[test]
    fn spot_lights_pack_their_cone() {
        let spot = Light::spot(
            Vec3::ZERO,
            Vec3::new(0.0, 0.0, -2.0),
            [1.0, 0.5, 0.25],
            4.0,
            10.0,
            10.0,
            20.0,
        );
        let gpu = GpuLight::from_light(&spot);
        assert_eq!(gpu.position_inv_range_sq[3], 0.01);
        assert_eq!(gpu.radiance, [4.0, 2.0, 1.0, 0.0]);
        assert_eq!(&gpu.spot_direction[..3], &[0.0, 0.0, -1.0]);
        assert!((gpu.spot_direction[3] - 20f32.to_radians().cos()).abs() < 1e-6);
        assert!((gpu.spot_params[0] - 10f32.to_radians().cos()).abs() < 1e-6);
        assert_eq!(gpu.spot_params[1], 1.0);
        let point = GpuLight::from_light(&Light::point(Vec3::ONE, [1.0; 3], 1.0, 2.0));
        assert_eq!(point.spot_params[1], 0.0);
    }
}
