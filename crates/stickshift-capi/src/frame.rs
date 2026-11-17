// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! Frames: recorded CPU-side, re-checked and handed to the backend whole at
//! submit. Acquiring a frame never waits.

use stickshift_math::glam::{Mat4, Vec3};
use stickshift_render_api::{
    ColorSpace, Light, LightKind, OverlayFill, OverlayQuad, PipelineKind, RenderSettings, SunSky,
    MAX_LIGHTS,
};

use crate::backend::{DrawCall, FrameData, PendingReadback};
use crate::context::{frame_of, with_context, with_frame, Inner, OwnedEvent, PendingRead};
use crate::context::{StickShiftContext, StickShiftFrame, MAX_FRAMES_IN_FLIGHT};
use crate::core::{call, input, utf8, Error};
use crate::handles::{kind_of, Kind, Slots};
use crate::models::{ModelEntry, ModelState};
use crate::resources::{MeshEntry, TextureEntry};
use crate::types::*;

/// A frame's recorded contents. Kept on the context and cleared (not freed)
/// between frames.
#[derive(Default)]
pub(crate) struct Recording {
    draws: Vec<DrawCall>,
    /// Per draw: the mesh or model handle and the texture handle (or 0) it
    /// came from, re-checked at submit, the draw call that recorded it, and the
    /// model part it draws (0 for a mesh).
    draw_sources: Vec<(u64, u64, u32, u32)>,
    quads: Vec<OverlayQuad>,
    /// Per quad: the texture handle it samples (or 0), and the draw call that
    /// recorded it.
    quad_sources: Vec<(u64, u32)>,
    /// The frame's point and spot lights, already validated.
    pub(crate) lights: Vec<Light>,
    /// Draw calls recorded this frame, so the draws or glyph quads one call
    /// recorded count as one skipped draw if they are dropped at submit.
    calls: u32,
    /// The draws and quads still valid at submit.
    kept_draws: Vec<DrawCall>,
    kept_quads: Vec<OverlayQuad>,
    /// The kept draws and quads as the C records `stickshift_context_get_last_frame`
    /// returns; swapped into [`LastFrame`] at submit.
    kept_draw_records: Vec<StickShiftDrawRecord>,
    kept_quad_records: Vec<StickShiftQuad>,
    readback: Option<(*mut u8, usize, Option<StickShiftCompletion>)>,
    /// The read-back is the engine's tick read-back: the engine waits for it
    /// and copies it itself, and no event reports it.
    engine_readback: bool,
}

/// What the last frame to reach submit recorded, for
/// `stickshift_context_get_last_frame`.
#[derive(Default)]
pub(crate) struct LastFrame {
    pub(crate) submitted: bool,
    pub(crate) number: u64,
    pub(crate) draws: Vec<StickShiftDrawRecord>,
    pub(crate) quads: Vec<StickShiftQuad>,
    pub(crate) camera: (Mat4, Vec3),
    pub(crate) sun: SunSky,
    pub(crate) settings: RenderSettings,
    pub(crate) light_count: u32,
}

impl Recording {
    /// Number a new draw call; everything it records shares the number.
    fn next_call(&mut self) -> u32 {
        self.calls += 1;
        self.calls
    }

    fn clear(&mut self) {
        self.draws.clear();
        self.draw_sources.clear();
        self.quads.clear();
        self.quad_sources.clear();
        self.lights.clear();
        self.calls = 0;
        self.kept_draws.clear();
        self.kept_quads.clear();
        self.kept_draw_records.clear();
        self.kept_quad_records.clear();
        self.readback = None;
        self.engine_readback = false;
    }
}

/// Whether a draw's source handles are still live and ready.
fn live(
    meshes: &Slots<MeshEntry>,
    textures: &Slots<TextureEntry>,
    models: &Slots<ModelEntry>,
    (object, texture): (u64, u64),
) -> bool {
    let object_ok = match kind_of(object) {
        Some(Kind::Mesh) => meshes.get(object).is_some(),
        Some(Kind::Model) => models
            .get(object)
            .is_some_and(|m| m.state == ModelState::Ready),
        _ => false,
    };
    object_ok && (texture == 0 || textures.get(texture).is_some())
}

/// A texture handle's backend texture; `INVALID_HANDLE` if it is not live.
fn texture(
    inner: &Inner,
    id: u64,
    what: &str,
) -> Result<stickshift_render_api::TextureHandle, Error> {
    inner
        .textures
        .get(id)
        .map(|t| t.handle)
        .ok_or_else(|| Error::invalid_handle(what))
}

/// Report a requested read-back that will never arrive.
fn fail_readback(inner: &mut Inner, frame: u64, completion: Option<StickShiftCompletion>) {
    let event = OwnedEvent::failed(
        STICKSHIFT_EVENT_READBACK_FAILED,
        0,
        frame,
        &Error::new(STICKSHIFT_FRAME_SKIPPED, "the frame was not rendered"),
    );
    inner.emit(event, completion);
}

/// `WRONG_STATE` inside an engine call: the engine opens, submits and discards
/// the frames of its context itself.
///
/// # Safety
/// `ctx` must be NULL or a live context.
unsafe fn refuse_in_engine_call(ctx: *const StickShiftContext, what: &str) -> Result<(), Error> {
    match ctx.as_ref() {
        Some(ctx) if crate::engine::in_engine_call(ctx) => Err(Error::wrong_state(format!(
            "{what} cannot be called inside an engine call: the engine opens and submits \
             the render hook's frame itself"
        ))),
        _ => Ok(()),
    }
}

/// Whether a frame can be opened and submitted now: a drawable that is not
/// empty, and a free frame slot.
pub(crate) fn has_free_slot(inner: &Inner) -> bool {
    let (width, height) = inner.surface_size();
    width != 0 && height != 0 && inner.backend.frames_in_flight() < MAX_FRAMES_IN_FLIGHT
}

/// Open the context's frame for recording.
pub(crate) fn open(inner: &mut Inner) {
    inner.recording.clear();
    inner.frame_open = true;
}

/// Open a frame for recording. Never waits: `STICKSHIFT_FRAME_SKIPPED` (and `*out`
/// NULL) when the GPU has the maximum frames in flight or the drawable is empty.
///
/// # Safety
/// `ctx` must be NULL or a live context; `out` NULL or valid.
#[no_mangle]
pub unsafe extern "C" fn stickshift_frame_acquire(
    ctx: *mut StickShiftContext,
    out: *mut *mut StickShiftFrame,
) -> StickShiftResult {
    call(|| {
        let out = crate::core::output(out, "out")?;
        *out = std::ptr::null_mut();
        refuse_in_engine_call(ctx, "stickshift_frame_acquire")?;
        let code = with_context(ctx, |inner| {
            if inner.frame_open {
                return Err(Error::wrong_state(
                    "a frame is already open: submit or discard it first",
                ));
            }
            if !has_free_slot(inner) {
                inner.counters.frames_skipped += 1;
                return Ok(STICKSHIFT_FRAME_SKIPPED);
            }
            open(inner);
            Ok(STICKSHIFT_OK)
        })?;
        if code == STICKSHIFT_OK {
            *out = frame_of(&*ctx);
        }
        Ok(code)
    })
}

/// Set the camera. Sticky across frames; the last value before submit applies
/// to the whole frame.
///
/// # Safety
/// `frame` must be NULL or an open frame; `camera` NULL or valid.
#[no_mangle]
pub unsafe extern "C" fn stickshift_frame_set_camera(
    frame: *mut StickShiftFrame,
    camera: *const StickShiftCamera,
) -> StickShiftResult {
    call(|| {
        let camera = *input(camera, "camera")?;
        with_frame(frame, |inner| {
            inner.camera = (camera.view_projection.into(), camera.position.into());
            Ok(STICKSHIFT_OK)
        })
    })
}

impl From<StickShiftSunSky> for SunSky {
    fn from(s: StickShiftSunSky) -> Self {
        Self {
            sun_elevation_deg: s.sun_elevation_deg,
            sun_azimuth_deg: s.sun_azimuth_deg,
            sun_color: s.sun_color,
            sun_intensity: s.sun_intensity,
            sky_fill: s.sky_fill,
            sky_zenith_color: s.sky_zenith_color,
            sky_horizon_color: s.sky_horizon_color,
        }
    }
}

impl From<SunSky> for StickShiftSunSky {
    fn from(s: SunSky) -> Self {
        Self {
            sun_elevation_deg: s.sun_elevation_deg,
            sun_azimuth_deg: s.sun_azimuth_deg,
            sun_color: s.sun_color,
            sun_intensity: s.sun_intensity,
            sky_fill: s.sky_fill,
            sky_zenith_color: s.sky_zenith_color,
            sky_horizon_color: s.sky_horizon_color,
        }
    }
}

/// Set the sun and sky. Sticky across frames.
///
/// # Safety
/// `frame` must be NULL or an open frame; `sun` NULL or valid.
#[no_mangle]
pub unsafe extern "C" fn stickshift_frame_set_sun_sky(
    frame: *mut StickShiftFrame,
    sun: *const StickShiftSunSky,
) -> StickShiftResult {
    call(|| {
        let sun = SunSky::from(*input(sun, "sun")?);
        with_frame(frame, |inner| {
            inner.sun = sun;
            Ok(STICKSHIFT_OK)
        })
    })
}

/// The default sun and sky: a high, white afternoon sun over a blue gradient.
/// NULL is a no-op. Thread-safe.
///
/// # Safety
/// `out` must be NULL or valid.
#[no_mangle]
pub unsafe extern "C" fn stickshift_sun_sky_default(out: *mut StickShiftSunSky) {
    if let Some(out) = out.as_mut() {
        *out = SunSky::default().into();
    }
}

/// A C light as the seam's [`Light`], or why it cannot be drawn.
fn light_from(l: StickShiftLight) -> Result<Light, Error> {
    let kind = match l.kind {
        STICKSHIFT_LIGHT_POINT => LightKind::Point,
        STICKSHIFT_LIGHT_SPOT => LightKind::Spot {
            direction: l.direction.into(),
            inner_angle_deg: l.inner_angle_deg,
            outer_angle_deg: l.outer_angle_deg,
        },
        other => {
            return Err(Error::invalid_argument(format!(
                "{other} is not a STICKSHIFT_LIGHT_* value"
            )))
        }
    };
    let light = Light {
        kind,
        position: Vec3::from(l.position),
        color: l.color,
        intensity: l.intensity,
        range: l.range,
    };
    if !light.is_valid() {
        return Err(Error::invalid_argument(
            "a light needs finite values, range > 0, intensity >= 0 and colour channels \
             >= 0; a spot also needs a non-zero direction and \
             0 <= inner_angle_deg <= outer_angle_deg <= 90",
        ));
    }
    Ok(light)
}

/// Add a point or spot light to the frame. Lights are per frame: each frame
/// starts with none, and every light added before submit lights every draw in
/// the frame. `STICKSHIFT_ERROR_LIMIT_EXCEEDED` past `STICKSHIFT_MAX_LIGHTS`.
///
/// # Safety
/// `frame` must be NULL or an open frame; `light` NULL or valid.
#[no_mangle]
pub unsafe extern "C" fn stickshift_frame_add_light(
    frame: *mut StickShiftFrame,
    light: *const StickShiftLight,
) -> StickShiftResult {
    call(|| {
        let light = light_from(*input(light, "light")?)?;
        with_frame(frame, |inner| {
            let lights = &mut inner.recording.lights;
            if lights.len() >= MAX_LIGHTS {
                return Err(Error::new(
                    STICKSHIFT_ERROR_LIMIT_EXCEEDED,
                    format!("a frame holds at most {MAX_LIGHTS} lights"),
                ));
            }
            lights.push(light);
            Ok(STICKSHIFT_OK)
        })
    })
}

const WHITE: StickShiftColor = StickShiftColor {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 1.0,
};

/// Record a draw of `mesh` with model-to-world matrix `*model`. `material` may
/// be NULL (white, no texture).
///
/// # Safety
/// `frame` must be NULL or an open frame; `model` and `material` NULL or valid.
#[no_mangle]
pub unsafe extern "C" fn stickshift_frame_draw_mesh(
    frame: *mut StickShiftFrame,
    mesh: StickShiftMesh,
    model: *const StickShiftMat4,
    material: *const StickShiftMaterial,
) -> StickShiftResult {
    call(|| {
        let model = Mat4::from(*input(model, "model")?);
        let material = material.as_ref().copied().unwrap_or(StickShiftMaterial {
            base_color: WHITE,
            base_color_texture: StickShiftTexture { id: 0 },
            emissive: [0.0; 3],
        });
        if !material.emissive.iter().all(|c| c.is_finite() && *c >= 0.0) {
            return Err(Error::invalid_argument(
                "material.emissive channels must be finite and >= 0",
            ));
        }
        with_frame(frame, |inner| {
            let entry = inner
                .meshes
                .get(mesh.id)
                .ok_or_else(|| Error::invalid_handle("mesh"))?;
            let (kind, handle) = (entry.kind, entry.handle);
            // Only a textured mesh samples the texture, so only then must it be
            // live, now and at submit. A vertex-colour mesh ignores it.
            let texture_id = match kind {
                PipelineKind::Textured => material.base_color_texture.id,
                _ => 0,
            };
            let texture = match texture_id {
                0 => None,
                id => Some(texture(inner, id, "base_color_texture")?),
            };
            let recording = &mut inner.recording;
            let call = recording.next_call();
            recording.draws.push(DrawCall {
                kind,
                mesh: handle,
                model,
                base_color: material.base_color.to_array(),
                emissive: material.emissive,
                texture,
            });
            recording.draw_sources.push((mesh.id, texture_id, call, 0));
            Ok(STICKSHIFT_OK)
        })
    })
}

/// Record, in node order, the parts of `model` on the pipeline `only` names
/// (every part for `None`), each at `place(node matrix)` with its base colour
/// multiplied by `tint`. `STICKSHIFT_NOT_READY` (one skipped draw) until the
/// model loads.
fn record_model(
    inner: &mut Inner,
    model: StickShiftModel,
    only: Option<PipelineKind>,
    tint: [f32; 4],
    place: impl Fn(Mat4) -> Mat4,
) -> Result<StickShiftResult, Error> {
    let entry = inner
        .models
        .get(model.id)
        .ok_or_else(|| Error::invalid_handle("model"))?;
    if entry.state != ModelState::Ready {
        inner.counters.draws_skipped += 1;
        return Ok(STICKSHIFT_NOT_READY);
    }
    let call = inner.recording.next_call();
    for &(index, transform) in &entry.instances {
        let part = &entry.parts[index];
        if only.is_some_and(|kind| kind != part.kind) {
            continue;
        }
        let mut base_color = part.base_color;
        for (c, t) in base_color.iter_mut().zip(tint) {
            *c *= t;
        }
        inner.recording.draws.push(DrawCall {
            kind: part.kind,
            mesh: part.mesh,
            model: place(transform),
            base_color,
            emissive: part.emissive,
            texture: part.texture,
        });
        inner.recording.draw_sources.push((
            model.id,
            0,
            call,
            u32::try_from(index).unwrap_or(u32::MAX),
        ));
    }
    Ok(STICKSHIFT_OK)
}

/// Record a draw of every part of `model` at `*world`, its base colours
/// multiplied by `*tint` (NULL: white). `STICKSHIFT_NOT_READY` until it loads.
///
/// # Safety
/// `frame` must be NULL or an open frame; `world` and `tint` NULL or valid.
#[no_mangle]
pub unsafe extern "C" fn stickshift_frame_draw_model(
    frame: *mut StickShiftFrame,
    model: StickShiftModel,
    world: *const StickShiftMat4,
    tint: *const StickShiftColor,
) -> StickShiftResult {
    call(|| {
        let world = Mat4::from(*input(world, "world")?);
        let tint = tint.as_ref().copied().unwrap_or(WHITE).to_array();
        with_frame(frame, |inner| {
            record_model(inner, model, None, tint, |node| world * node)
        })
    })
}

/// Like `stickshift_frame_draw_model`, with a local matrix and a part filter:
/// record the parts of `model` that `draw->parts` selects, in node order, each
/// at `draw->world × (draw->local × node)` with its base colour multiplied by
/// `draw->tint`. Drawing a model's textured parts and its untextured parts in
/// two calls lets other draws go between them. `STICKSHIFT_NOT_READY` until the
/// model loads; `STICKSHIFT_ERROR_INVALID_ARGUMENT` for an unknown `parts`
/// value.
///
/// # Safety
/// `frame` must be NULL or an open frame; `draw` NULL or valid.
#[no_mangle]
pub unsafe extern "C" fn stickshift_frame_draw_model_ex(
    frame: *mut StickShiftFrame,
    model: StickShiftModel,
    draw: *const StickShiftModelDraw,
) -> StickShiftResult {
    call(|| {
        let draw = *input(draw, "draw")?;
        let only = match draw.parts {
            STICKSHIFT_MODEL_PARTS_ALL => None,
            STICKSHIFT_MODEL_PARTS_TEXTURED => Some(PipelineKind::Textured),
            STICKSHIFT_MODEL_PARTS_UNTEXTURED => Some(PipelineKind::VertexColor),
            other => {
                return Err(Error::invalid_argument(format!(
                    "{other} is not a STICKSHIFT_MODEL_PARTS_* value"
                )))
            }
        };
        let (world, local) = (Mat4::from(draw.world), Mat4::from(draw.local));
        with_frame(frame, |inner| {
            record_model(inner, model, only, draw.tint.to_array(), |node| {
                world * (local * node)
            })
        })
    })
}

/// Record a 2D overlay quad, composited over the 3D scene in record order.
///
/// # Safety
/// `frame` must be NULL or an open frame; `quad` NULL or valid.
#[no_mangle]
pub unsafe extern "C" fn stickshift_frame_draw_quad(
    frame: *mut StickShiftFrame,
    quad: *const StickShiftQuad,
) -> StickShiftResult {
    call(|| {
        let q = *input(quad, "quad")?;
        with_frame(frame, |inner| {
            let (fill, source) = match q.fill {
                STICKSHIFT_FILL_SOLID => (OverlayFill::Solid, 0),
                STICKSHIFT_FILL_TEXTURE => (
                    OverlayFill::Textured(texture(inner, q.texture.id, "texture")?),
                    q.texture.id,
                ),
                STICKSHIFT_FILL_SDF => {
                    let entry = inner
                        .textures
                        .get(q.texture.id)
                        .ok_or_else(|| Error::invalid_handle("texture"))?;
                    if entry.color_space != ColorSpace::Linear {
                        return Err(Error::invalid_argument(
                            "a STICKSHIFT_FILL_SDF texture holds distances, not colour: create it \
                             with STICKSHIFT_COLOR_SPACE_LINEAR",
                        ));
                    }
                    (OverlayFill::Sdf(entry.handle), q.texture.id)
                }
                other => {
                    return Err(Error::invalid_argument(format!(
                        "{other} is not a STICKSHIFT_FILL_* value"
                    )))
                }
            };
            inner.recording.quads.push(OverlayQuad {
                rect: [q.x, q.y, q.width, q.height],
                uv: [q.u0, q.v0, q.u1, q.v1],
                color: q.color.to_array(),
                fill,
            });
            let call = inner.recording.next_call();
            inner.recording.quad_sources.push((source, call));
            Ok(STICKSHIFT_OK)
        })
    })
}

/// Record `text` in `font` with the pen starting at `(x, y)` (the left end of
/// the first line's baseline) at `px` pixels per em. `STICKSHIFT_NOT_READY` if the
/// font's atlas texture has been destroyed.
///
/// # Safety
/// `frame` must be NULL or an open frame; `text` NULL or NUL-terminated.
#[no_mangle]
pub unsafe extern "C" fn stickshift_frame_draw_text(
    frame: *mut StickShiftFrame,
    font: StickShiftFont,
    text: *const std::ffi::c_char,
    x: f32,
    y: f32,
    px: f32,
    color: StickShiftColor,
) -> StickShiftResult {
    call(|| {
        let text = utf8(text, "text")?;
        with_frame(frame, |inner| {
            let entry = inner
                .fonts
                .get(font.id)
                .ok_or_else(|| Error::invalid_handle("font"))?;
            if inner.textures.get(entry.atlas_id).is_none() {
                inner.counters.draws_skipped += 1;
                return Ok(STICKSHIFT_NOT_READY);
            }
            let atlas_id = entry.atlas_id;
            let recording = &mut inner.recording;
            let call = recording.next_call();
            entry
                .atlas
                .layout(text, [x, y], px, color.to_array(), |quad| {
                    recording.quads.push(quad);
                    recording.quad_sources.push((atlas_id, call));
                });
            Ok(STICKSHIFT_OK)
        })
    })
}

/// Ask for this frame's pixels (offscreen contexts only): `width * height * 4`
/// bytes of BGRA8, top row first, into `dst`, which must stay valid until the
/// frame's `READBACK_COMPLETE` or `READBACK_FAILED` event.
///
/// # Safety
/// `frame` must be NULL or an open frame; `dst` valid for writes of `len`
/// bytes until the event; `completion` NULL or valid.
#[no_mangle]
pub unsafe extern "C" fn stickshift_frame_read_pixels_async(
    frame: *mut StickShiftFrame,
    dst: *mut std::ffi::c_void,
    len: usize,
    completion: *const StickShiftCompletion,
) -> StickShiftResult {
    call(|| {
        let completion = completion.as_ref().copied();
        if dst.is_null() {
            return Err(Error::null("dst"));
        }
        with_frame(frame, |inner| {
            if !inner.backend.is_offscreen() {
                return Err(Error::new(
                    STICKSHIFT_ERROR_UNSUPPORTED,
                    "read-back needs an offscreen context",
                ));
            }
            let (width, height) = inner.surface_size();
            let need = width as usize * height as usize * 4;
            if len < need {
                return Err(Error::invalid_argument(format!(
                    "dst holds {len} bytes; a {width}x{height} frame needs {need}"
                )));
            }
            if inner.recording.readback.is_some() {
                return Err(Error::wrong_state("this frame already has a read-back"));
            }
            inner.recording.readback = Some((dst.cast(), need, completion));
            Ok(STICKSHIFT_OK)
        })
    })
}

/// Encode, commit and present the frame, then close it. Never waits for the
/// GPU, but on a layer context it takes the next drawable from the layer, which
/// can wait (up to about a second, if the window is hidden).
/// `STICKSHIFT_FRAME_SKIPPED` if the layer gave no drawable. `completion`
/// receives the frame's `FRAME_COMPLETED`; `*out_frame_number` (optional) is
/// the frame's number, or 0 when skipped.
///
/// # Safety
/// `frame` must be NULL or an open frame; `completion` and `out_frame_number`
/// NULL or valid.
#[no_mangle]
pub unsafe extern "C" fn stickshift_frame_submit(
    frame: *mut StickShiftFrame,
    completion: *const StickShiftCompletion,
    out_frame_number: *mut u64,
) -> StickShiftResult {
    call(|| {
        let completion = completion.as_ref().copied();
        let mut number = 0;
        refuse_in_engine_call(
            frame.as_ref().map_or(std::ptr::null(), |f| f.ctx),
            "stickshift_frame_submit",
        )?;
        let code = with_frame(frame, |inner| {
            let finished = finish(inner, Close::Submit(completion));
            number = finished.number;
            Ok(finished.code)
        })?;
        if let Some(out) = out_frame_number.as_mut() {
            *out = number;
        }
        Ok(code)
    })
}

/// Close the frame without rendering it. A requested read-back fails.
///
/// # Safety
/// `frame` must be NULL or an open frame.
#[no_mangle]
pub unsafe extern "C" fn stickshift_frame_discard(frame: *mut StickShiftFrame) -> StickShiftResult {
    call(|| {
        refuse_in_engine_call(
            frame.as_ref().map_or(std::ptr::null(), |f| f.ctx),
            "stickshift_frame_discard",
        )?;
        with_frame(frame, |inner| {
            let readback = inner.recording.readback.take();
            inner.recording.clear();
            inner.frame_open = false;
            if let Some((_, _, completion)) = readback {
                fail_readback(inner, 0, completion);
            }
            Ok(STICKSHIFT_OK)
        })
    })
}

/// How [`finish`] closes the open frame.
pub(crate) enum Close {
    /// `stickshift_frame_submit`: hand it to the GPU, and report its completion
    /// as `FRAME_COMPLETED` (to the completion, or the poll queue).
    Submit(Option<StickShiftCompletion>),
    /// An engine frame: hand it to the GPU, and report nothing; frames the
    /// engine submits complete internally.
    Engine,
    /// An engine frame with no free slot: record what it kept, and drop it.
    Discard,
}

/// What [`finish`] did.
pub(crate) struct Finished {
    /// `STICKSHIFT_OK`, or `STICKSHIFT_FRAME_SKIPPED` if the frame was not
    /// submitted.
    pub(crate) code: StickShiftResult,
    /// The submitted frame's number, or 0.
    pub(crate) number: u64,
    /// The engine's tick read-back of this frame, for it to wait for and copy.
    pub(crate) readback: Option<Box<dyn PendingReadback>>,
}

/// Re-check the open frame's draws, keep what is still live, record it as the
/// context's last frame, and close it as `close` says.
pub(crate) fn finish(inner: &mut Inner, close: Close) -> Finished {
    // Drop anything destroyed (or a model that stopped being ready) since it
    // was recorded.
    let recording = &mut inner.recording;
    recording.kept_draws.clear();
    recording.kept_quads.clear();
    recording.kept_draw_records.clear();
    recording.kept_quad_records.clear();
    // `draws_skipped` counts draw calls, like the `NOT_READY` refusals: a
    // dropped model or text counts once, not once per part or glyph. A call's
    // draws (or quads) are recorded together, and calls are numbered across
    // both lists, so a new number is a new call.
    let mut skipped = 0;
    let mut last_dropped = None;
    let mut dropped = |call: u32| {
        if last_dropped != Some(call) {
            last_dropped = Some(call);
            skipped += 1;
        }
    };
    for (draw, &(object, texture, call, part)) in
        recording.draws.iter().zip(&recording.draw_sources)
    {
        if live(
            &inner.meshes,
            &inner.textures,
            &inner.models,
            (object, texture),
        ) {
            recording.kept_draws.push(*draw);
            recording.kept_draw_records.push(StickShiftDrawRecord {
                source: object,
                texture,
                part,
                layout: match draw.kind {
                    PipelineKind::VertexColor => STICKSHIFT_VERTEX_LAYOUT_POS_NORMAL_COLOR,
                    PipelineKind::Textured => STICKSHIFT_VERTEX_LAYOUT_POS_NORMAL_UV,
                },
                model: draw.model.into(),
                base_color: StickShiftColor {
                    r: draw.base_color[0],
                    g: draw.base_color[1],
                    b: draw.base_color[2],
                    a: draw.base_color[3],
                },
                emissive: draw.emissive,
                reserved: 0,
            });
        } else {
            dropped(call);
        }
    }
    for (quad, &(texture, call)) in recording.quads.iter().zip(&recording.quad_sources) {
        if texture == 0 || inner.textures.get(texture).is_some() {
            recording.kept_quads.push(*quad);
            recording.kept_quad_records.push(quad_record(quad, texture));
        } else {
            dropped(call);
        }
    }
    inner.counters.draws_skipped += skipped;

    let readback = inner.recording.readback.take();
    let engine_readback = inner.recording.engine_readback;
    let submitted = match close {
        Close::Discard => None,
        Close::Submit(_) | Close::Engine => inner.backend.submit(&FrameData {
            view_projection: inner.camera.0,
            camera_position: inner.camera.1,
            sun: inner.sun,
            lights: &inner.recording.lights,
            draws: &inner.recording.kept_draws,
            quads: &inner.recording.kept_quads,
            readback: readback.is_some(),
        }),
    };
    let drawn = inner.recording.kept_draws.len() as u64;

    // The last frame's record: swap the kept lists in, so the record costs no
    // copy; the old record's lists are cleared with the recording.
    let last = &mut inner.last_frame;
    std::mem::swap(&mut last.draws, &mut inner.recording.kept_draw_records);
    std::mem::swap(&mut last.quads, &mut inner.recording.kept_quad_records);
    last.submitted = submitted.is_some();
    last.number = submitted.as_ref().map_or(0, |s| s.frame);
    last.camera = inner.camera;
    last.sun = inner.sun;
    last.settings = inner.backend.settings();
    last.light_count = u32::try_from(inner.recording.lights.len()).unwrap_or(u32::MAX);

    inner.recording.clear();
    inner.frame_open = false;

    let Some(submitted) = submitted else {
        if !matches!(close, Close::Discard) {
            inner.counters.frames_skipped += 1;
        }
        if let Some((_, _, read_completion)) = readback.filter(|_| !engine_readback) {
            fail_readback(inner, 0, read_completion);
        }
        return Finished {
            code: STICKSHIFT_FRAME_SKIPPED,
            number: 0,
            readback: None,
        };
    };
    inner.counters.draws_submitted += drawn;
    if let Close::Submit(completion) = close {
        inner
            .pending_frames
            .push_back((submitted.frame, completion));
    }
    let mut engine_copy = None;
    match (readback, submitted.readback) {
        (Some(_), Some(copy)) if engine_readback => engine_copy = Some(copy),
        (Some((dst, len, read_completion)), Some(copy)) => {
            inner.pending_reads.push(PendingRead {
                frame: submitted.frame,
                copy,
                dst,
                len,
                completion: read_completion,
            });
        }
        (Some((_, _, read_completion)), None) if !engine_readback => {
            fail_readback(inner, submitted.frame, read_completion);
        }
        _ => {}
    }
    Finished {
        code: STICKSHIFT_OK,
        number: submitted.frame,
        readback: engine_copy,
    }
}

/// Ask for the open engine frame's pixels into `dst` (`len` bytes, already
/// checked), for the engine to wait for and copy itself.
pub(crate) fn request_engine_readback(inner: &mut Inner, dst: *mut u8, len: usize) {
    inner.recording.readback = Some((dst, len, None));
    inner.recording.engine_readback = true;
}

/// A kept overlay quad as `stickshift_context_get_last_frame` reports it.
fn quad_record(quad: &OverlayQuad, texture: u64) -> StickShiftQuad {
    let [x, y, width, height] = quad.rect;
    let [u0, v0, u1, v1] = quad.uv;
    let [r, g, b, a] = quad.color;
    StickShiftQuad {
        x,
        y,
        width,
        height,
        u0,
        v0,
        u1,
        v1,
        color: StickShiftColor { r, g, b, a },
        fill: match quad.fill {
            OverlayFill::Solid => STICKSHIFT_FILL_SOLID,
            OverlayFill::Textured(_) => STICKSHIFT_FILL_TEXTURE,
            OverlayFill::Sdf(_) => STICKSHIFT_FILL_SDF,
        },
        texture: StickShiftTexture { id: texture },
    }
}

#[cfg(test)]
mod tests {
    use stickshift_math::glam::Quat;

    use super::*;
    use crate::context::{stickshift_context_create, stickshift_context_destroy};
    use crate::context::{stickshift_context_flush, with_context};
    use crate::models::stickshift_model_load_memory;

    /// One triangle under two nodes turned -90° about X at scale 100, as the
    /// sports car's are, the second also moved.
    const TURNED_TRIANGLE: &[u8] = br#"{
        "asset": {"version": "2.0"},
        "buffers": [{"byteLength": 36,
            "uri": "data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAA"}],
        "bufferViews": [{"buffer": 0, "byteLength": 36}],
        "accessors": [{"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3",
            "min": [0, 0, 0], "max": [1, 1, 0]}],
        "meshes": [{"primitives": [{"attributes": {"POSITION": 0}}]}],
        "nodes": [
            {"mesh": 0, "rotation": [-0.7071068, 0, 0, 0.7071067], "scale": [100, 100, 100]},
            {"mesh": 0, "rotation": [-0.7071068, 0, 0, 0.7071067], "scale": [100, 100, 100],
             "translation": [0.37, 0.21, -0.93]}
        ],
        "scenes": [{"nodes": [0, 1]}],
        "scene": 0
    }"#;

    fn bits(m: Mat4) -> [u32; 16] {
        m.to_cols_array().map(f32::to_bits)
    }

    #[test]
    fn model_parts_are_placed_at_world_times_local_times_node() {
        let desc = StickShiftContextDesc {
            api_version: STICKSHIFT_API_VERSION,
            backend: STICKSHIFT_BACKEND_NULL,
            metal_layer: std::ptr::null_mut(),
            width: 4,
            height: 4,
            scale: 1.0,
        };
        let mut ctx = std::ptr::null_mut();
        let mut model = StickShiftModel::default();
        let bytes = StickShiftData {
            ptr: TURNED_TRIANGLE.as_ptr().cast(),
            len: TURNED_TRIANGLE.len(),
            release: None,
            user: std::ptr::null_mut(),
        };
        // A fit like the demo's car fit: a small uniform scale, a half turn,
        // and a lift.
        let local = Mat4::from_scale_rotation_translation(
            Vec3::splat(0.012_345_7),
            Quat::from_rotation_y(std::f32::consts::PI),
            Vec3::new(0.013, 0.31, -0.027),
        );
        // The car's placement at a run of positions down the road.
        let worlds: Vec<Mat4> = (0..16)
            .map(|i| {
                Mat4::from_scale_rotation_translation(
                    Vec3::ONE,
                    Quat::from_rotation_y(0.05 * i as f32),
                    Vec3::new(1.75, 0.3, -3.7 * i as f32 - 0.61),
                )
            })
            .collect();
        // SAFETY: valid pointers to locals; the bytes are copied.
        unsafe {
            assert_eq!(stickshift_context_create(&desc, &mut ctx), STICKSHIFT_OK);
            assert_eq!(
                stickshift_model_load_memory(ctx, bytes, std::ptr::null(), &mut model),
                STICKSHIFT_OK
            );
            assert_eq!(stickshift_context_flush(ctx), STICKSHIFT_OK);
            let mut frame = std::ptr::null_mut();
            assert_eq!(stickshift_frame_acquire(ctx, &mut frame), STICKSHIFT_OK);
            for &world in &worlds {
                let draw = StickShiftModelDraw {
                    world: world.into(),
                    local: local.into(),
                    tint: WHITE,
                    parts: STICKSHIFT_MODEL_PARTS_ALL,
                };
                assert_eq!(
                    stickshift_frame_draw_model_ex(frame, model, &draw),
                    STICKSHIFT_OK
                );
            }
            let (nodes, recorded) = with_context(ctx, |inner| {
                let entry = inner.models.get(model.id).expect("a live model");
                let nodes: Vec<Mat4> = entry.instances.iter().map(|&(_, node)| node).collect();
                let recorded: Vec<Mat4> = inner.recording.draws.iter().map(|d| d.model).collect();
                Ok((nodes, recorded))
            })
            .expect("a live context");
            assert_eq!(nodes.len(), 2);
            assert_eq!(recorded.len(), worlds.len() * nodes.len());

            let mut regrouped_differs = false;
            for (draw, (world, node)) in recorded.iter().zip(
                worlds
                    .iter()
                    .flat_map(|world| nodes.iter().map(move |node| (world, node))),
            ) {
                assert_eq!(bits(*draw), bits(*world * (local * *node)));
                regrouped_differs |= bits(*draw) != bits((*world * local) * *node);
            }
            assert!(
                regrouped_differs,
                "these matrices do not tell the two groupings apart"
            );
            assert_eq!(stickshift_frame_discard(frame), STICKSHIFT_OK);
            assert_eq!(stickshift_context_destroy(ctx), STICKSHIFT_OK);
        }
    }
}
