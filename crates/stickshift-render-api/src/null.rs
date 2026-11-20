// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! [`NullRenderer`] — a headless, GPU-free test double for the render seam.

use std::collections::{HashMap, HashSet};

use stickshift_math::glam::{Mat4, Quat, Vec3};
use stickshift_math::Transform;

use crate::descriptor::{MaterialParams, PipelineKind};
use crate::device::{MeshData, PipelineDescriptor, RenderDevice, TextureData};
use crate::handles::{EmitterHandle, MeshHandle, PipelineHandle, TextureHandle};
use crate::light::{Light, MAX_LIGHTS};
use crate::overlay::OverlayQuad;
use crate::particles::{EmitterParams, ParticleSystem, MAX_EMITTERS};
use crate::recorder::{CapturedFrame, FrameRecorder};
use crate::settings::RenderSettings;
use crate::sun::SunSky;

/// A single recorded draw, captured by [`NullRenderer`] for later assertion.
///
/// Not `PartialEq`: [`stickshift_math::Transform`] does not implement `PartialEq`, so tests
/// assert on individual fields (`mesh`, `pipeline`, `texture`, `material`,
/// `transform.position`, …) rather than comparing whole draws.
#[derive(Debug, Clone)]
pub struct RecordedDraw {
    /// The mesh that was drawn.
    pub mesh: MeshHandle,
    /// The pipeline bound when the draw was recorded, if any had been set.
    pub pipeline: Option<PipelineHandle>,
    /// The texture bound when the draw was recorded, if any.
    pub texture: Option<TextureHandle>,
    /// The model-to-world matrix of the draw (`transform.to_matrix()` for a
    /// [`draw_mesh`](FrameRecorder::draw_mesh)).
    pub model: Mat4,
    /// The material parameters passed to the draw.
    pub material: MaterialParams,
    /// The view-projection the seam held when the draw was recorded (the one
    /// the Metal backend would build this draw's MVP from), if any had been set.
    pub view_projection: Option<Mat4>,
}

/// A burst of particles fired with [`ParticleSystem::emit`], captured by
/// [`NullRenderer`] with where its emitter was when it fired.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RecordedBurst {
    /// The emitter that fired.
    pub emitter: EmitterHandle,
    /// How many particles it fired.
    pub count: u32,
    /// Where the emitter was placed when it fired.
    pub position: Vec3,
    /// How it was turned.
    pub rotation: Quat,
    /// How fast it was moving.
    pub velocity: Vec3,
    /// Its params when it fired.
    pub params: EmitterParams,
}

/// A live emitter as [`NullRenderer`] holds it: its params and where it was
/// last placed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RecordedEmitter {
    /// Its params, as last created or set.
    pub params: EmitterParams,
    /// Where it was last placed (the origin until placed).
    pub position: Vec3,
    /// How it was last turned.
    pub rotation: Quat,
    /// How fast it was last said to move.
    pub velocity: Vec3,
}

/// A call the Metal backend would ignore, which [`NullRenderer`] therefore
/// refused instead of recording. Listed by
/// [`protocol_violations`](NullRenderer::protocol_violations).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProtocolViolation {
    /// [`begin_frame`](FrameRecorder::begin_frame) while a frame was already
    /// open. The open frame continues.
    BeginWhileOpen,
    /// [`submit`](FrameRecorder::submit) with no frame open.
    SubmitWithoutFrame,
    /// A draw, an overlay quad, a pipeline selection or a texture binding
    /// recorded while no frame was open.
    OutsideFrame,
    /// A handle that was never created, or was already destroyed: used in a
    /// call, or destroyed a second time.
    UnknownMesh(MeshHandle),
    /// See [`UnknownMesh`](Self::UnknownMesh).
    UnknownTexture(TextureHandle),
    /// See [`UnknownMesh`](Self::UnknownMesh).
    UnknownPipeline(PipelineHandle),
    /// A draw of a mesh whose vertex layout is not the selected pipeline's (the
    /// untextured pipeline when none was selected this frame), or matches
    /// neither built-in pipeline.
    MeshPipelineMismatch(MeshHandle),
    /// A draw of a mesh with no whole, in-range triangle: the Metal backend has
    /// nothing to draw for it.
    EmptyMesh(MeshHandle),
    /// A light that is not [valid](Light::is_valid).
    InvalidLight,
    /// A light added to a frame that already held [`MAX_LIGHTS`].
    TooManyLights,
    /// An emitter handle that was never created, or was already destroyed:
    /// used in a call, or destroyed a second time.
    UnknownEmitter(EmitterHandle),
    /// Emitter params that are not [valid](EmitterParams::is_valid). The
    /// emitter is still created or set, as a backend brings them into range.
    InvalidEmitter,
    /// An emitter created while [`MAX_EMITTERS`] were already live. A backend
    /// hands it a handle that emits nothing.
    TooManyEmitters,
}

/// A headless implementation of both [`RenderDevice`] and [`FrameRecorder`] that
/// **records** every call instead of touching a GPU.
///
/// `NullRenderer` lets scene and game code be unit-tested without a Metal device:
/// resource creation hands out sequential handles, and frame recording captures the
/// ordered stream of draws (each with the pipeline/texture bound at the time).
/// Tests introspect the recorder through its accessors — [`draw_count`](Self::draw_count),
/// [`draws`](Self::draws), [`created_meshes`](Self::created_meshes), and friends — to
/// assert what the code under test asked the renderer to do.
///
/// It accepts exactly the calls the Metal backend acts on. A call the backend
/// would ignore (a draw outside a frame, a destroyed or unknown handle, a mesh on
/// the wrong pipeline, a double destroy, …) is not recorded as a draw; it is
/// listed in [`protocol_violations`](Self::protocol_violations) instead, so a
/// headless test cannot pass on code that draws nothing on a GPU.
///
/// Lights are recorded per frame: [`lights`](Self::lights) holds the ones the
/// open (or last submitted) frame added, so a test can assert which headlights
/// and streetlights a game lit a frame with.
///
/// The sticky frame-wide state is recorded the same way:
/// [`view_projection`](Self::view_projection),
/// [`camera_position`](Self::camera_position) and [`sun_sky`](Self::sun_sky) each
/// hold the last value pushed, so a test can assert *which camera and which sun* a
/// game asked for — a headless way to check lighting intent that would otherwise
/// need a GPU and a pixel hash.
///
/// It also tracks live vs. destroyed resources so tests can assert correct cleanup,
/// and records the frame protocol (frames begun/submitted, whether a frame is open)
/// so tests can catch ordering mistakes. It never panics on protocol violations —
/// it records them faithfully so a test can assert the *observed* behaviour.
#[derive(Debug, Default)]
pub struct NullRenderer {
    // --- resource bookkeeping (RenderDevice side) ---
    next_mesh: u32,
    next_texture: u32,
    next_pipeline: u32,
    created_meshes: Vec<MeshHandle>,
    created_textures: Vec<TextureHandle>,
    created_pipelines: Vec<PipelineHandle>,
    destroyed_meshes: Vec<MeshHandle>,
    destroyed_textures: Vec<TextureHandle>,
    destroyed_pipelines: Vec<PipelineHandle>,
    /// Live meshes: which built-in pipeline each one's layout matches (if any),
    /// and whether it has anything to draw.
    live_meshes: HashMap<MeshHandle, (Option<PipelineKind>, bool)>,
    live_textures: HashSet<TextureHandle>,
    /// Live pipelines and the built-in pipeline each one names.
    live_pipelines: HashMap<PipelineHandle, PipelineKind>,
    violations: Vec<ProtocolViolation>,

    // --- frame bookkeeping (FrameRecorder side) ---
    frame_open: bool,
    frames_begun: u32,
    frames_submitted: u32,
    current_pipeline: Option<PipelineHandle>,
    current_texture: Option<TextureHandle>,
    draws: Vec<RecordedDraw>,
    /// Overlay quads recorded this run, in record order, so headless
    /// tests can assert what a HUD drew without a GPU.
    overlay_quads: Vec<OverlayQuad>,
    /// The lights the open frame (or, between frames, the last one) added, in
    /// call order. Cleared by `begin_frame`: lights are not sticky.
    lights: Vec<Light>,
    /// The most recent view-projection pushed via
    /// [`set_view_projection`](FrameRecorder::set_view_projection), if any. `None`
    /// until the first push, then **sticky** (retained across frames) — mirroring
    /// the seam contract that a real backend keeps the last view-projection until
    /// it is replaced.
    view_projection: Option<Mat4>,
    /// The most recent camera world position pushed via
    /// [`set_camera_position`](FrameRecorder::set_camera_position).
    /// `None` until the first push, then sticky — like `view_projection`, which it
    /// must agree with.
    camera_position: Option<Vec3>,
    /// The most recent sun/sky pushed via
    /// [`set_sun_sky`](FrameRecorder::set_sun_sky). `None` until the first
    /// push, then sticky, so a headless test can assert *which sun* a game asked
    /// for without a GPU.
    sun_sky: Option<SunSky>,
    /// The most recent render settings pushed via
    /// [`set_render_settings`](RenderDevice::set_render_settings). `None`
    /// until the first push, then sticky.
    render_settings: Option<RenderSettings>,
    /// The drawable size last requested via
    /// [`resize_surface`](RenderDevice::resize_surface), or `None` for the fixed
    /// [`NULL_SURFACE_WIDTH`] × [`NULL_SURFACE_HEIGHT`] default.
    surface_size: Option<(u32, u32)>,
    /// The UI scale last passed to [`resize_surface`](RenderDevice::resize_surface);
    /// `None` reports `1.0`.
    surface_scale: Option<f32>,
    /// A capture asked for the next submit ([`capture_frame`](FrameRecorder::capture_frame)):
    /// its size and time.
    capture: Option<(u32, u32, f64)>,
    /// Captures made at submit and not yet taken.
    captured: Vec<CapturedFrame>,

    // --- particle bookkeeping (ParticleSystem side) ---
    next_emitter: u32,
    created_emitters: Vec<EmitterHandle>,
    destroyed_emitters: Vec<EmitterHandle>,
    live_emitters: HashMap<EmitterHandle, RecordedEmitter>,
    /// Every burst fired, in call order, across frames.
    bursts: Vec<RecordedBurst>,
    /// The particle time advanced in total.
    particle_time: f64,
    /// The particle shifts applied in total.
    particle_shift: Vec3,
}

impl NullRenderer {
    /// Create an empty recorder with no resources and no recorded frames.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    // --- resource introspection ---

    /// Handles of every mesh created, in creation order (including any later destroyed).
    #[must_use]
    pub fn created_meshes(&self) -> &[MeshHandle] {
        &self.created_meshes
    }

    /// Handles of every texture created, in creation order.
    #[must_use]
    pub fn created_textures(&self) -> &[TextureHandle] {
        &self.created_textures
    }

    /// Handles of every pipeline created, in creation order.
    #[must_use]
    pub fn created_pipelines(&self) -> &[PipelineHandle] {
        &self.created_pipelines
    }

    /// Handles passed to [`destroy_mesh`](RenderDevice::destroy_mesh), in call order.
    #[must_use]
    pub fn destroyed_meshes(&self) -> &[MeshHandle] {
        &self.destroyed_meshes
    }

    /// Handles passed to [`destroy_texture`](RenderDevice::destroy_texture), in call order.
    #[must_use]
    pub fn destroyed_textures(&self) -> &[TextureHandle] {
        &self.destroyed_textures
    }

    /// Handles passed to [`destroy_pipeline`](RenderDevice::destroy_pipeline), in call order.
    #[must_use]
    pub fn destroyed_pipelines(&self) -> &[PipelineHandle] {
        &self.destroyed_pipelines
    }

    /// Number of meshes still live: created and not yet destroyed. A second
    /// destroy of the same mesh, or a destroy of an unknown handle, changes
    /// nothing (it is a [`ProtocolViolation`]).
    #[must_use]
    pub fn live_mesh_count(&self) -> usize {
        self.live_meshes.len()
    }

    /// Number of textures still live (see [`live_mesh_count`](Self::live_mesh_count)).
    #[must_use]
    pub fn live_texture_count(&self) -> usize {
        self.live_textures.len()
    }

    /// Number of pipelines still live (see [`live_mesh_count`](Self::live_mesh_count)).
    #[must_use]
    pub fn live_pipeline_count(&self) -> usize {
        self.live_pipelines.len()
    }

    /// Every call that was refused because the Metal backend would ignore it, in
    /// call order. Empty for code that uses the seam correctly.
    #[must_use]
    pub fn protocol_violations(&self) -> &[ProtocolViolation] {
        &self.violations
    }

    // --- frame introspection ---

    /// Every draw recorded across every frame, in order.
    #[must_use]
    pub fn draws(&self) -> &[RecordedDraw] {
        &self.draws
    }

    /// Every overlay quad recorded across all frames, in record order.
    #[must_use]
    pub fn overlay_quads(&self) -> &[OverlayQuad] {
        &self.overlay_quads
    }

    /// Total number of overlay quads recorded across all frames.
    #[must_use]
    pub fn overlay_quad_count(&self) -> usize {
        self.overlay_quads.len()
    }

    /// The lights the open frame added, in call order; between frames, the last
    /// submitted frame's. Empty after `begin_frame` until the frame adds one,
    /// because lights are per frame, not sticky.
    #[must_use]
    pub fn lights(&self) -> &[Light] {
        &self.lights
    }

    /// Total number of draws recorded across all frames.
    #[must_use]
    pub fn draw_count(&self) -> usize {
        self.draws.len()
    }

    /// Number of times [`begin_frame`](FrameRecorder::begin_frame) was called.
    #[must_use]
    pub fn frames_begun(&self) -> u32 {
        self.frames_begun
    }

    /// Number of times [`submit`](FrameRecorder::submit) was called.
    #[must_use]
    pub fn frames_submitted(&self) -> u32 {
        self.frames_submitted
    }

    /// Whether a frame is currently open (begun but not yet submitted).
    #[must_use]
    pub fn frame_open(&self) -> bool {
        self.frame_open
    }

    /// The most recent view-projection matrix recorded via
    /// [`set_view_projection`](FrameRecorder::set_view_projection).
    ///
    /// `None` before the first push, then sticky (the last value pushed). Lets
    /// tests assert the engine/game pushed the expected camera before drawing.
    #[must_use]
    pub fn view_projection(&self) -> Option<Mat4> {
        self.view_projection
    }

    /// The most recent camera world position recorded via
    /// [`set_camera_position`](FrameRecorder::set_camera_position).
    ///
    /// `None` before the first push, then sticky. Pairs with
    /// [`view_projection`](Self::view_projection): a test can assert the engine
    /// pushed a position that matches the matrix it pushed.
    #[must_use]
    pub fn camera_position(&self) -> Option<Vec3> {
        self.camera_position
    }

    /// The most recent [`SunSky`] recorded via
    /// [`set_sun_sky`](FrameRecorder::set_sun_sky).
    ///
    /// `None` before the first push, then sticky. Lets a headless test assert the
    /// *sun a game asked for* — its elevation, azimuth, colour and sky — and, via
    /// [`SunSky::direction`], the light vector that follows from it.
    #[must_use]
    pub fn sun_sky(&self) -> Option<SunSky> {
        self.sun_sky
    }

    /// The most recent [`RenderSettings`] pushed via
    /// [`set_render_settings`](RenderDevice::set_render_settings), or
    /// `None` if none was. Sticky, so a headless test can assert which shadow
    /// quality, shadow range and draw-distance scale the engine asked for.
    #[must_use]
    pub fn render_settings(&self) -> Option<RenderSettings> {
        self.render_settings
    }

    // --- particle introspection ---

    /// Handles of every emitter created, in creation order (including any later
    /// destroyed).
    #[must_use]
    pub fn created_emitters(&self) -> &[EmitterHandle] {
        &self.created_emitters
    }

    /// Handles of every emitter destroyed, in destruction order.
    #[must_use]
    pub fn destroyed_emitters(&self) -> &[EmitterHandle] {
        &self.destroyed_emitters
    }

    /// The number of emitters created and not destroyed.
    #[must_use]
    pub fn live_emitter_count(&self) -> usize {
        self.live_emitters.len()
    }

    /// A live emitter's params and last placement, or `None` if `handle` is not
    /// live.
    #[must_use]
    pub fn emitter(&self, handle: EmitterHandle) -> Option<&RecordedEmitter> {
        self.live_emitters.get(&handle)
    }

    /// Every burst fired with [`emit`](ParticleSystem::emit), in call order,
    /// across frames.
    #[must_use]
    pub fn bursts(&self) -> &[RecordedBurst] {
        &self.bursts
    }

    /// The total time [`advance_particles`](ParticleSystem::advance_particles)
    /// has moved particles on, in seconds.
    #[must_use]
    pub fn particle_time(&self) -> f64 {
        self.particle_time
    }

    /// The total of every [`shift_particles`](ParticleSystem::shift_particles).
    #[must_use]
    pub fn particle_shift(&self) -> Vec3 {
        self.particle_shift
    }
}

/// Whether a mesh has at least one whole triangle whose indices are all in range,
/// i.e. whether the Metal backend uploads anything to draw for it.
fn has_drawable_triangle(data: &MeshData<'_>) -> bool {
    let stride = data.layout.stride as usize;
    if stride == 0 {
        return false;
    }
    let vertex_count = data.vertices.len() / stride;
    data.indices
        .chunks_exact(3)
        .any(|t| t.iter().all(|&i| (i as usize) < vertex_count))
}

impl RenderDevice for NullRenderer {
    fn create_mesh(&mut self, data: &MeshData<'_>) -> MeshHandle {
        let handle = MeshHandle(self.next_mesh);
        self.next_mesh += 1;
        self.created_meshes.push(handle);
        self.live_meshes.insert(
            handle,
            (
                PipelineKind::for_layout(&data.layout),
                has_drawable_triangle(data),
            ),
        );
        handle
    }

    fn destroy_mesh(&mut self, handle: MeshHandle) {
        self.destroyed_meshes.push(handle);
        if self.live_meshes.remove(&handle).is_none() {
            self.violations.push(ProtocolViolation::UnknownMesh(handle));
        }
    }

    fn create_texture(&mut self, _data: &TextureData<'_>) -> TextureHandle {
        let handle = TextureHandle(self.next_texture);
        self.next_texture += 1;
        self.created_textures.push(handle);
        self.live_textures.insert(handle);
        handle
    }

    fn destroy_texture(&mut self, handle: TextureHandle) {
        self.destroyed_textures.push(handle);
        if !self.live_textures.remove(&handle) {
            self.violations
                .push(ProtocolViolation::UnknownTexture(handle));
        }
    }

    fn create_pipeline(&mut self, desc: &PipelineDescriptor) -> PipelineHandle {
        let handle = PipelineHandle(self.next_pipeline);
        self.next_pipeline += 1;
        self.created_pipelines.push(handle);
        self.live_pipelines.insert(handle, desc.kind);
        handle
    }

    fn destroy_pipeline(&mut self, handle: PipelineHandle) {
        self.destroyed_pipelines.push(handle);
        if self.live_pipelines.remove(&handle).is_none() {
            self.violations
                .push(ProtocolViolation::UnknownPipeline(handle));
        }
    }

    fn surface_size(&self) -> (u32, u32) {
        // A fixed, GPU-less drawable size so headless HUD layout is deterministic,
        // unless a test resized it.
        self.surface_size
            .unwrap_or((NULL_SURFACE_WIDTH, NULL_SURFACE_HEIGHT))
    }

    fn safe_area_insets(&self) -> [f32; 4] {
        // No notches or rounded corners off-device.
        [0.0; 4]
    }

    fn resize_surface(&mut self, width: u32, height: u32, pixels_per_point: f32) {
        self.surface_size = Some((width.max(1), height.max(1)));
        self.surface_scale = Some(pixels_per_point);
    }

    fn surface_scale(&self) -> f32 {
        self.surface_scale.unwrap_or(1.0)
    }

    fn set_render_settings(&mut self, settings: &RenderSettings) {
        self.render_settings = Some(*settings);
    }
}

/// Drawable width the [`NullRenderer`] reports from
/// [`surface_size`](RenderDevice::surface_size), so headless HUD layout is
/// deterministic.
pub const NULL_SURFACE_WIDTH: u32 = 1280;
/// Drawable height the [`NullRenderer`] reports from
/// [`surface_size`](RenderDevice::surface_size).
pub const NULL_SURFACE_HEIGHT: u32 = 720;

impl FrameRecorder for NullRenderer {
    fn begin_frame(&mut self) {
        if self.frame_open {
            self.violations.push(ProtocolViolation::BeginWhileOpen);
            return;
        }
        self.frame_open = true;
        self.frames_begun += 1;
        // Per-frame state resets at the start of each frame. The camera and the
        // sun/sky are deliberately NOT reset: they are sticky across frames (seam
        // contract), so an engine loop can push them once per frame before the game
        // records — or, for a fixed sun, just once at load.
        self.current_pipeline = None;
        self.current_texture = None;
        self.lights.clear();
    }

    fn set_view_projection(&mut self, view_proj: Mat4) {
        self.view_projection = Some(view_proj);
    }

    fn set_camera_position(&mut self, position: Vec3) {
        self.camera_position = Some(position);
    }

    fn set_sun_sky(&mut self, sun: &SunSky) {
        self.sun_sky = Some(*sun);
    }

    fn add_light(&mut self, light: &Light) {
        if !self.frame_open {
            self.violations.push(ProtocolViolation::OutsideFrame);
        } else if !light.is_valid() {
            self.violations.push(ProtocolViolation::InvalidLight);
        } else if self.lights.len() >= MAX_LIGHTS {
            self.violations.push(ProtocolViolation::TooManyLights);
        } else {
            self.lights.push(*light);
        }
    }

    fn set_pipeline(&mut self, handle: PipelineHandle) {
        if !self.frame_open {
            self.violations.push(ProtocolViolation::OutsideFrame);
        } else if !self.live_pipelines.contains_key(&handle) {
            self.violations
                .push(ProtocolViolation::UnknownPipeline(handle));
        } else {
            self.current_pipeline = Some(handle);
        }
    }

    fn bind_texture(&mut self, handle: TextureHandle) {
        if !self.frame_open {
            self.violations.push(ProtocolViolation::OutsideFrame);
        } else if !self.live_textures.contains(&handle) {
            self.violations
                .push(ProtocolViolation::UnknownTexture(handle));
        } else {
            self.current_texture = Some(handle);
        }
    }

    fn draw_mesh(&mut self, mesh: MeshHandle, transform: &Transform, material: &MaterialParams) {
        self.draw_mesh_matrix(mesh, transform.to_matrix(), material);
    }

    fn draw_mesh_matrix(&mut self, mesh: MeshHandle, model: Mat4, material: &MaterialParams) {
        if !self.frame_open {
            self.violations.push(ProtocolViolation::OutsideFrame);
            return;
        }
        let Some(&(mesh_kind, drawable)) = self.live_meshes.get(&mesh) else {
            self.violations.push(ProtocolViolation::UnknownMesh(mesh));
            return;
        };
        // The untextured pipeline is selected until the frame selects another.
        let pipeline_kind = self
            .current_pipeline
            .and_then(|p| self.live_pipelines.get(&p).copied())
            .unwrap_or(PipelineKind::VertexColor);
        if mesh_kind != Some(pipeline_kind) {
            self.violations
                .push(ProtocolViolation::MeshPipelineMismatch(mesh));
            return;
        }
        if !drawable {
            self.violations.push(ProtocolViolation::EmptyMesh(mesh));
            return;
        }
        self.draws.push(RecordedDraw {
            mesh,
            pipeline: self.current_pipeline,
            texture: self.current_texture,
            model,
            material: *material,
            view_projection: self.view_projection,
        });
    }

    fn draw_overlay_quad(&mut self, quad: &OverlayQuad) {
        if !self.frame_open {
            self.violations.push(ProtocolViolation::OutsideFrame);
            return;
        }
        self.overlay_quads.push(*quad);
    }

    fn submit(&mut self) {
        if !self.frame_open {
            self.violations.push(ProtocolViolation::SubmitWithoutFrame);
            return;
        }
        self.frame_open = false;
        self.frames_submitted += 1;
        // A capture asked for: there are no pixels, so its every byte is the
        // frame's number (mod 256), opaque, so a test can tell frames apart.
        if let Some((width, height, time)) = self.capture.take() {
            let shade = (self.frames_submitted % 256) as u8;
            let pixel = [shade, shade, shade, 255];
            let bgra8 = pixel.repeat(width as usize * height as usize);
            self.captured.push(CapturedFrame {
                width,
                height,
                time,
                bgra8,
            });
        }
    }

    fn capture_frame(&mut self, width: u32, height: u32, time: f64) -> bool {
        if width == 0 || height == 0 {
            return false;
        }
        self.capture = Some((width, height, time));
        true
    }

    fn take_captured_frames(&mut self) -> Vec<CapturedFrame> {
        std::mem::take(&mut self.captured)
    }
}

impl ParticleSystem for NullRenderer {
    fn create_emitter(&mut self, params: &EmitterParams) -> EmitterHandle {
        if !params.is_valid() {
            self.violations.push(ProtocolViolation::InvalidEmitter);
        }
        if self.live_emitters.len() >= MAX_EMITTERS {
            self.violations.push(ProtocolViolation::TooManyEmitters);
        }
        let handle = EmitterHandle(self.next_emitter);
        self.next_emitter += 1;
        self.created_emitters.push(handle);
        self.live_emitters.insert(
            handle,
            RecordedEmitter {
                params: params.sanitized(),
                position: Vec3::ZERO,
                rotation: Quat::IDENTITY,
                velocity: Vec3::ZERO,
            },
        );
        handle
    }

    fn set_emitter(&mut self, emitter: EmitterHandle, params: &EmitterParams) {
        if !params.is_valid() {
            self.violations.push(ProtocolViolation::InvalidEmitter);
        }
        match self.live_emitters.get_mut(&emitter) {
            Some(live) => live.params = params.sanitized(),
            None => self
                .violations
                .push(ProtocolViolation::UnknownEmitter(emitter)),
        }
    }

    fn destroy_emitter(&mut self, emitter: EmitterHandle) {
        if self.live_emitters.remove(&emitter).is_some() {
            self.destroyed_emitters.push(emitter);
        } else {
            self.violations
                .push(ProtocolViolation::UnknownEmitter(emitter));
        }
    }

    fn place_emitter(&mut self, emitter: EmitterHandle, transform: &Transform, velocity: Vec3) {
        match self.live_emitters.get_mut(&emitter) {
            Some(live) => {
                live.position = transform.position;
                live.rotation = transform.rotation;
                live.velocity = velocity;
            }
            None => self
                .violations
                .push(ProtocolViolation::UnknownEmitter(emitter)),
        }
    }

    fn emit(&mut self, emitter: EmitterHandle, count: u32) {
        match self.live_emitters.get(&emitter) {
            Some(live) => self.bursts.push(RecordedBurst {
                emitter,
                count,
                position: live.position,
                rotation: live.rotation,
                velocity: live.velocity,
                params: live.params,
            }),
            None => self
                .violations
                .push(ProtocolViolation::UnknownEmitter(emitter)),
        }
    }

    fn advance_particles(&mut self, dt: f32) {
        if dt.is_finite() && dt > 0.0 {
            self.particle_time += f64::from(dt);
        }
    }

    fn shift_particles(&mut self, offset: Vec3) {
        self.particle_shift += offset;
        for live in self.live_emitters.values_mut() {
            live.position += offset;
        }
    }
}

#[cfg(test)]
mod particle_tests {
    use super::*;

    #[test]
    fn a_burst_is_recorded_from_where_its_emitter_was_placed() {
        let mut r = NullRenderer::new();
        let sparks = r.create_emitter(&EmitterParams::default());
        r.place_emitter(
            sparks,
            &Transform::from_position(Vec3::new(1.0, 2.0, 3.0)),
            Vec3::new(0.0, 0.0, -20.0),
        );
        r.emit(sparks, 40);
        r.place_emitter(sparks, &Transform::from_position(Vec3::X), Vec3::ZERO);
        r.emit(sparks, 5);
        let bursts = r.bursts();
        assert_eq!(bursts.len(), 2);
        assert_eq!(
            (bursts[0].count, bursts[0].position),
            (40, Vec3::new(1.0, 2.0, 3.0))
        );
        assert_eq!(bursts[0].velocity, Vec3::new(0.0, 0.0, -20.0));
        assert_eq!((bursts[1].count, bursts[1].position), (5, Vec3::X));
        assert!(r.protocol_violations().is_empty());
    }

    #[test]
    fn a_destroyed_or_unknown_emitter_is_a_violation_and_records_nothing() {
        let mut r = NullRenderer::new();
        let e = r.create_emitter(&EmitterParams::default());
        r.destroy_emitter(e);
        r.emit(e, 10);
        r.destroy_emitter(e);
        r.place_emitter(EmitterHandle(99), &Transform::identity(), Vec3::ZERO);
        assert!(r.bursts().is_empty());
        assert_eq!(r.live_emitter_count(), 0);
        assert_eq!(
            r.protocol_violations(),
            &[
                ProtocolViolation::UnknownEmitter(e),
                ProtocolViolation::UnknownEmitter(e),
                ProtocolViolation::UnknownEmitter(EmitterHandle(99)),
            ]
        );
    }

    #[test]
    fn invalid_params_are_a_violation_but_still_take_effect_in_range() {
        let mut r = NullRenderer::new();
        let e = r.create_emitter(&EmitterParams {
            rate: -1.0,
            ..EmitterParams::default()
        });
        assert_eq!(
            r.protocol_violations(),
            &[ProtocolViolation::InvalidEmitter]
        );
        assert_eq!(r.emitter(e).map(|live| live.params.rate), Some(0.0));
        r.set_emitter(
            e,
            &EmitterParams {
                rate: 30.0,
                ..EmitterParams::default()
            },
        );
        assert_eq!(r.emitter(e).map(|live| live.params.rate), Some(30.0));
    }

    #[test]
    fn time_and_shifts_add_up_and_a_shift_moves_the_emitters() {
        let mut r = NullRenderer::new();
        let e = r.create_emitter(&EmitterParams::default());
        r.place_emitter(e, &Transform::from_position(Vec3::Z), Vec3::ZERO);
        r.advance_particles(0.5);
        r.advance_particles(0.25);
        r.advance_particles(-1.0);
        r.shift_particles(Vec3::new(0.0, 0.0, 100.0));
        assert_eq!(r.particle_time(), 0.75);
        assert_eq!(r.particle_shift(), Vec3::new(0.0, 0.0, 100.0));
        assert_eq!(
            r.emitter(e).map(|live| live.position),
            Some(Vec3::new(0.0, 0.0, 101.0))
        );
    }

    #[test]
    fn past_the_emitter_limit_a_create_is_a_violation() {
        let mut r = NullRenderer::new();
        for _ in 0..MAX_EMITTERS {
            r.create_emitter(&EmitterParams::default());
        }
        assert!(r.protocol_violations().is_empty());
        r.create_emitter(&EmitterParams::default());
        assert_eq!(
            r.protocol_violations(),
            &[ProtocolViolation::TooManyEmitters]
        );
    }
}
