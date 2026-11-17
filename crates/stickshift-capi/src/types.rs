// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! The C-visible types and constants: everything `stickshift.h` declares besides the
//! functions.
//!
//! Every enum-like type is a fixed-width integer alias with constants rather
//! than a Rust `enum`, so any value a C caller passes is defined behaviour; an
//! unknown value is rejected with [`STICKSHIFT_ERROR_INVALID_ARGUMENT`].

use std::ffi::{c_char, c_void};

use crate::engine::StickShiftEngine;

// ---------------------------------------------------------------------------
// Versioning
// ---------------------------------------------------------------------------

/// The API version this header describes. It increases with every incompatible
/// change; set [`StickShiftContextDesc::api_version`] to it.
pub const STICKSHIFT_API_VERSION: u32 = 4;

/// The additive level within [`STICKSHIFT_API_VERSION`]: it grows by one with
/// each release that only adds to the API. 0 is the surface before the engine
/// object; 1 adds it. `stickshift_api_minor()` reports the library's.
pub const STICKSHIFT_API_MINOR: u32 = 1;

// ---------------------------------------------------------------------------
// Results
// ---------------------------------------------------------------------------

/// A result code. `STICKSHIFT_OK` (0) is success, small positive values are non-error
/// outcomes, negative values are errors.
pub type StickShiftResult = i32;

/// Done.
pub const STICKSHIFT_OK: StickShiftResult = 0;
/// No frame this time: too many frames in flight, no drawable, or the context
/// is not ready. Not an error.
pub const STICKSHIFT_FRAME_SKIPPED: StickShiftResult = 1;
/// A resource the call needs is pending or failed; nothing was done.
pub const STICKSHIFT_NOT_READY: StickShiftResult = 2;
/// NULL where a pointer is required, a bad enum value, or sizes that don't match.
pub const STICKSHIFT_ERROR_INVALID_ARGUMENT: StickShiftResult = -1;
/// The handle is null, destroyed, of the wrong kind, or another context's.
pub const STICKSHIFT_ERROR_INVALID_HANDLE: StickShiftResult = -2;
/// The call is not allowed now (no frame open, frame already open, ...).
pub const STICKSHIFT_ERROR_WRONG_STATE: StickShiftResult = -3;
/// Another call on this context is in progress.
pub const STICKSHIFT_ERROR_BUSY: StickShiftResult = -4;
/// No Metal device, or Metal is unavailable on this platform.
pub const STICKSHIFT_ERROR_NO_DEVICE: StickShiftResult = -5;
/// Not supported by this context or build.
pub const STICKSHIFT_ERROR_UNSUPPORTED: StickShiftResult = -6;
/// A file could not be read.
pub const STICKSHIFT_ERROR_IO: StickShiftResult = -7;
/// A glTF or image could not be decoded.
pub const STICKSHIFT_ERROR_PARSE: StickShiftResult = -8;
/// An engine limit was hit (handle table, live contexts, texture size, buffer
/// length).
pub const STICKSHIFT_ERROR_LIMIT_EXCEEDED: StickShiftResult = -9;
/// A bug in the engine (a Rust panic caught at the boundary). Destroy the context.
pub const STICKSHIFT_ERROR_INTERNAL: StickShiftResult = -10;
/// A wait gave up before what it was waiting for finished (a hung GPU, or a
/// load that never completed).
pub const STICKSHIFT_ERROR_TIMEOUT: StickShiftResult = -11;

/// Not an error: the entity is gone or lacks the component, and nothing was
/// done.
pub const STICKSHIFT_ABSENT: StickShiftResult = 3;

/// An engine call, or a call on a context with an engine attached, from a
/// thread other than the one that created the engine.
pub const STICKSHIFT_ERROR_WRONG_THREAD: StickShiftResult = -12;

/// The game's init hook failed: the engine is failed, and every later
/// `stickshift_engine_init`, `_resume` and `_tick` is
/// `STICKSHIFT_ERROR_WRONG_STATE`. Destroy it.
pub const STICKSHIFT_ERROR_ABORTED: StickShiftResult = -13;

// ---------------------------------------------------------------------------
// Enum-like values
// ---------------------------------------------------------------------------

/// Which renderer a context uses.
pub type StickShiftBackend = u32;
/// The Metal renderer (Apple platforms).
pub const STICKSHIFT_BACKEND_METAL: StickShiftBackend = 0;
/// No GPU: resources and frames are tracked, draws counted, nothing drawn, and
/// read-backs return zeros. For host-side tests.
pub const STICKSHIFT_BACKEND_NULL: StickShiftBackend = 1;

/// The state of a resource handle.
pub type StickShiftResourceState = u32;
/// Not a live handle: never issued, destroyed, or another context's.
pub const STICKSHIFT_RESOURCE_STATE_INVALID: StickShiftResourceState = 0;
/// Accepted; the work to make it usable has not finished.
pub const STICKSHIFT_RESOURCE_STATE_PENDING: StickShiftResourceState = 1;
/// Usable in draws.
pub const STICKSHIFT_RESOURCE_STATE_READY: StickShiftResourceState = 2;
/// The work failed; draws using it are skipped. Destroy it.
pub const STICKSHIFT_RESOURCE_STATE_FAILED: StickShiftResourceState = 3;

/// What a [`StickShiftEvent`] reports.
pub type StickShiftEventType = u32;
/// The context can render.
pub const STICKSHIFT_EVENT_CONTEXT_READY: StickShiftEventType = 1;
/// A resource (`object`) became ready.
pub const STICKSHIFT_EVENT_RESOURCE_READY: StickShiftEventType = 2;
/// A resource (`object`) failed; `result` and `message` say why.
pub const STICKSHIFT_EVENT_RESOURCE_FAILED: StickShiftEventType = 3;
/// The GPU finished frame `frame`.
pub const STICKSHIFT_EVENT_FRAME_COMPLETED: StickShiftEventType = 4;
/// The pixels requested for frame `frame` are in the caller's buffer.
pub const STICKSHIFT_EVENT_READBACK_COMPLETE: StickShiftEventType = 5;
/// The pixels requested for frame `frame` will never arrive.
pub const STICKSHIFT_EVENT_READBACK_FAILED: StickShiftEventType = 6;

/// The vertex layout of a mesh, which also selects the pipeline that draws it.
pub type StickShiftVertexLayout = u32;
/// `float3 position, float3 normal, float3 linear RGB colour` (36 bytes).
pub const STICKSHIFT_VERTEX_LAYOUT_POS_NORMAL_COLOR: StickShiftVertexLayout = 0;
/// `float3 position, float3 normal, float2 uv` (32 bytes), drawn with a
/// base-colour texture.
pub const STICKSHIFT_VERTEX_LAYOUT_POS_NORMAL_UV: StickShiftVertexLayout = 1;

/// Shadow quality.
pub type StickShiftShadowQuality = u32;
/// No shadow pass.
pub const STICKSHIFT_SHADOW_QUALITY_OFF: StickShiftShadowQuality = 0;
/// A 2048² shadow map.
pub const STICKSHIFT_SHADOW_QUALITY_LOW: StickShiftShadowQuality = 1;
/// A 4096² shadow map (the default).
pub const STICKSHIFT_SHADOW_QUALITY_HIGH: StickShiftShadowQuality = 2;

/// How a texture's colour channels are encoded.
pub type StickShiftColorSpace = u32;
/// sRGB-encoded colour (photos, painted albedo, glTF base-colour images): the
/// sampler decodes it to linear before lighting. The default (0).
pub const STICKSHIFT_COLOR_SPACE_SRGB: StickShiftColorSpace = 0;
/// Data sampled as stored: SDF font atlases, masks. Required for font atlases
/// and `STICKSHIFT_FILL_SDF` quads.
pub const STICKSHIFT_COLOR_SPACE_LINEAR: StickShiftColorSpace = 1;

/// What shape of light a [`StickShiftLight`] is.
pub type StickShiftLightKind = u32;
/// Shines equally in every direction from its position.
pub const STICKSHIFT_LIGHT_POINT: StickShiftLightKind = 0;
/// Shines in a cone from its position, along `direction`.
pub const STICKSHIFT_LIGHT_SPOT: StickShiftLightKind = 1;

/// The most lights one frame can hold; `stickshift_frame_add_light` refuses
/// more with `STICKSHIFT_ERROR_LIMIT_EXCEEDED`.
pub const STICKSHIFT_MAX_LIGHTS: u32 = 256;

/// How a 2D quad is filled.
pub type StickShiftFill = u32;
/// The quad's colour, untextured.
pub const STICKSHIFT_FILL_SOLID: StickShiftFill = 0;
/// `texture` sampled as RGBA, times the colour.
pub const STICKSHIFT_FILL_TEXTURE: StickShiftFill = 1;
/// `texture` sampled as a signed-distance field (distance in red), tinted by
/// the colour. How text is drawn. The texture must be
/// `STICKSHIFT_COLOR_SPACE_LINEAR`.
pub const STICKSHIFT_FILL_SDF: StickShiftFill = 2;

/// Which parts of a model `stickshift_frame_draw_model_ex` records.
pub type StickShiftModelParts = u32;
/// Every part.
pub const STICKSHIFT_MODEL_PARTS_ALL: StickShiftModelParts = 0;
/// The parts with a base-colour texture: those on the `POS_NORMAL_UV` layout.
pub const STICKSHIFT_MODEL_PARTS_TEXTURED: StickShiftModelParts = 1;
/// The parts without one: those on the `POS_NORMAL_COLOR` layout.
pub const STICKSHIFT_MODEL_PARTS_UNTEXTURED: StickShiftModelParts = 2;

// ---------------------------------------------------------------------------
// Math
// ---------------------------------------------------------------------------

/// A 3-component vector.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StickShiftVec3 {
    /// X.
    pub x: f32,
    /// Y (up).
    pub y: f32,
    /// Z.
    pub z: f32,
}

/// A rotation quaternion.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StickShiftQuat {
    /// X.
    pub x: f32,
    /// Y.
    pub y: f32,
    /// Z.
    pub z: f32,
    /// W (1 for the identity).
    pub w: f32,
}

/// A 4×4 matrix, **column-major**: `m[0..4]` is the first column. The same
/// memory layout as Swift's `simd_float4x4`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StickShiftMat4 {
    /// The 16 elements, column by column.
    pub m: [f32; 16],
}

/// A translation, rotation and scale: `stickshift_math::Transform`, field for
/// field. Its matrix applies scale first, then rotation, then translation
/// (`stickshift_transform_to_mat4`). A frozen value type, with no `size` field.
#[repr(C)]
// No `Default`: all zeros is a degenerate transform, not the identity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StickShiftTransform {
    /// Position (translation).
    pub position: StickShiftVec3,
    /// Rotation, a unit quaternion; `{0, 0, 0, 1}` for none.
    pub rotation: StickShiftQuat,
    /// Scale along each axis; `{1, 1, 1}` for none.
    pub scale: StickShiftVec3,
}

/// A linear RGBA colour, each channel `0..1`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StickShiftColor {
    /// Red.
    pub r: f32,
    /// Green.
    pub g: f32,
    /// Blue.
    pub b: f32,
    /// Alpha.
    pub a: f32,
}

// ---------------------------------------------------------------------------
// Entities, components and the world (an engine's ECS)
// ---------------------------------------------------------------------------

/// The most game components one engine registers, tags and data together.
pub const STICKSHIFT_MAX_GAME_COMPONENTS: u32 = 128;
/// The most bytes in one game component value.
pub const STICKSHIFT_MAX_COMPONENT_SIZE: u32 = 4096;
/// The most components in one spawn or insert.
pub const STICKSHIFT_MAX_BUNDLE: u32 = 32;
/// The most terms in one query.
pub const STICKSHIFT_MAX_QUERY_TERMS: u32 = 16;
/// The most query snapshots one engine holds unreleased at once.
pub const STICKSHIFT_MAX_LIVE_QUERIES: u32 = 16;

/// An entity of an engine's world: hecs `Entity::to_bits`, `generation << 32 |
/// id`. 0 is the null entity (hecs never issues generation 0). Ids are reused
/// last-freed first with a bumped generation, so a stale entity reads as
/// absent, never as another entity. Unlike resource handles, an entity is not
/// tagged with its context: it is valid only in the engine that issued it.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct StickShiftEntity {
    /// The hecs bits; 0 for none.
    pub bits: u64,
}

/// A component: an engine component (`STICKSHIFT_COMPONENT_*`, 1..255) or a
/// game component from `stickshift_component_register` (from
/// `STICKSHIFT_FIRST_GAME_COMPONENT`).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct StickShiftComponent {
    /// The component's id.
    pub id: u32,
}

/// The engine's transform component; its value is a `StickShiftTransform`.
pub const STICKSHIFT_COMPONENT_TRANSFORM: u32 = 1;
/// The engine's render component; its value is a `StickShiftRenderComponent`.
pub const STICKSHIFT_COMPONENT_RENDER: u32 = 2;
/// The engine's static-entity tag; no value.
pub const STICKSHIFT_COMPONENT_STATIC_TAG: u32 = 3;
/// The engine's dynamic-entity tag; no value.
pub const STICKSHIFT_COMPONENT_DYNAMIC_TAG: u32 = 4;
/// The id of the first game component registered; ids 5..255 are reserved for
/// engine components.
pub const STICKSHIFT_FIRST_GAME_COMPONENT: u32 = 256;

/// The shape of a `StickShiftRenderComponent`.
pub type StickShiftRenderShape = u32;
/// A unit cube built by the engine, coloured with the component's colour.
pub const STICKSHIFT_RENDER_SHAPE_CUBE: StickShiftRenderShape = 0;
/// A unit-diameter sphere (20 segments, 20 rings) built by the engine.
pub const STICKSHIFT_RENDER_SHAPE_SPHERE: StickShiftRenderShape = 1;
/// Read only: a custom mesh a Rust caller made. Writing it is
/// `STICKSHIFT_ERROR_INVALID_ARGUMENT`.
pub const STICKSHIFT_RENDER_SHAPE_MESH: StickShiftRenderShape = 2;

/// The value of `STICKSHIFT_COMPONENT_RENDER`: a colour and a built-in shape. A
/// frozen value type, with no `size` field.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StickShiftRenderComponent {
    /// Linear RGB, `0..1`.
    pub color: [f32; 3],
    /// A `STICKSHIFT_RENDER_SHAPE_*` value.
    pub shape: StickShiftRenderShape,
}

/// A game component to register, or what one is.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct StickShiftComponentDesc {
    /// UTF-8 and unique per engine; copied when registering. Returned names
    /// are library-owned and valid for the engine's life.
    pub name: *const c_char,
    /// The bytes of one value: 0 makes a tag (no value), up to
    /// `STICKSHIFT_MAX_COMPONENT_SIZE`.
    pub size: u32,
    /// A power of two up to 16: where the value sits in query rows. Values are
    /// always copied, never borrowed.
    pub align: u32,
}

/// One component of a spawn or insert bundle.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct StickShiftComponentValue {
    /// The component.
    pub component: StickShiftComponent,
    /// `size` bytes, read during the call; NULL for a tag.
    pub data: *const c_void,
    /// The component's registered size (`sizeof` the C struct for an engine
    /// component); 0 for a tag.
    pub size: u32,
}

/// How a query term uses its component.
pub type StickShiftQueryAccess = u32;
/// Required, with the value in each row (for a tag, the same as `WITH`).
pub const STICKSHIFT_QUERY_READ: StickShiftQueryAccess = 0;
/// Required, with no value in the row.
pub const STICKSHIFT_QUERY_WITH: StickShiftQueryAccess = 1;
/// Excluded.
pub const STICKSHIFT_QUERY_WITHOUT: StickShiftQueryAccess = 2;
/// Not a filter: the value when the entity has it, and a presence flag.
pub const STICKSHIFT_QUERY_OPTIONAL: StickShiftQueryAccess = 3;

/// One term of a query.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StickShiftQueryTerm {
    /// The component.
    pub component: StickShiftComponent,
    /// A `STICKSHIFT_QUERY_*` value.
    pub access: StickShiftQueryAccess,
}

/// A query's snapshot: the matching entities in the world's query order, with
/// their values copied into rows. Engine-owned; valid until
/// `stickshift_world_query_release`, until the hook or callback that took it
/// returns, or (taken between engine calls) until the next tick.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct StickShiftQueryRows {
    /// `sizeof(StickShiftQueryRows)`.
    pub size: u32,
    /// The matching entities.
    pub count: u32,
    /// `count` entities, in query order.
    pub entities: *const StickShiftEntity,
    /// `count` rows of `stride` bytes, starting 16-byte aligned.
    pub data: *const u8,
    /// Bytes from one row to the next.
    pub stride: usize,
    /// Per term: its value's byte offset in a row (a multiple of the
    /// component's alignment), or `UINT32_MAX` if rows hold no value for it.
    pub offsets: *const u32,
    /// `count * term_count` flags, row by row: 1 where the entity has the
    /// term's component. An absent optional term's value bytes are zero.
    pub present: *const u8,
    /// The query's terms.
    pub term_count: u32,
    /// Reserved.
    pub reserved: u32,
    /// Pass to `stickshift_world_query_release`; never 0.
    pub token: u64,
}

/// A scene's streaming configuration: stickshift-scene's `StreamingConfig`,
/// field for field. A frozen value type, with no `size` field.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StickShiftStreamingConfig {
    /// The travel axis; `(0, 0, -1)` by default.
    pub axis: StickShiftVec3,
    /// Distance between spawn slots along the axis; 6 by default.
    pub spawn_interval: f32,
    /// How far ahead of the focus slots are filled; 60 by default.
    pub spawn_ahead: f32,
    /// How far behind the focus streamed entities are despawned; 12 by default.
    pub despawn_behind: f32,
    /// How far the focus may travel before a floating-origin rebase; 1000 by
    /// default.
    pub rebase_threshold: f32,
}

/// Called once for each new slot of a `stickshift_scene_stream` pass, from
/// inside the scene's own pass: `position` is the slot's world position.
/// World calls spawn into the scene; `stickshift_stream_track` reports an
/// entity for despawning behind.
pub type StickShiftSpawnFn = Option<
    unsafe extern "C" fn(
        user: *mut c_void,
        engine: *mut StickShiftEngine,
        slot: i64,
        position: StickShiftVec3,
    ),
>;

// ---------------------------------------------------------------------------
// Handles
// ---------------------------------------------------------------------------

/// A mesh owned by a context. `id == 0` is the null handle.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct StickShiftMesh {
    /// The handle's identity; meaningful only to the context that issued it.
    pub id: u64,
}

/// A texture owned by a context. `id == 0` is the null handle.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct StickShiftTexture {
    /// The handle's identity; meaningful only to the context that issued it.
    pub id: u64,
}

/// A glTF model owned by a context. `id == 0` is the null handle.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct StickShiftModel {
    /// The handle's identity; meaningful only to the context that issued it.
    pub id: u64,
}

/// An SDF font owned by a context. `id == 0` is the null handle.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct StickShiftFont {
    /// The handle's identity; meaningful only to the context that issued it.
    pub id: u64,
}

// ---------------------------------------------------------------------------
// Data ownership, events, completions
// ---------------------------------------------------------------------------

/// Called exactly once when the engine no longer needs borrowed [`StickShiftData`].
/// NULL means the data is copied.
pub type StickShiftReleaseFn =
    Option<unsafe extern "C" fn(user: *mut c_void, ptr: *const c_void, len: usize)>;

/// Bulk data passed to the engine: copied (`release` NULL) or borrowed until
/// `release` is called.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct StickShiftData {
    /// The bytes. May be NULL only when `len` is 0.
    pub ptr: *const c_void,
    /// Length in bytes.
    pub len: usize,
    /// NULL: the engine copies the bytes before the call returns. Otherwise the
    /// bytes are borrowed until the engine calls `release(user, ptr, len)`.
    pub release: StickShiftReleaseFn,
    /// Passed to `release`.
    pub user: *mut c_void,
}

/// A completion or status report. See `docs/C_API.md`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct StickShiftEvent {
    /// What happened: a `STICKSHIFT_EVENT_*` value.
    pub kind: StickShiftEventType,
    /// `STICKSHIFT_OK`, or why it failed.
    pub result: StickShiftResult,
    /// The handle id the event is about, or 0.
    pub object: u64,
    /// The frame number the event is about, or 0.
    pub frame: u64,
    /// Detail for failures (NUL-terminated UTF-8), or NULL. Valid until the
    /// callback returns, or until the next `stickshift_poll_events` on the context.
    pub message: *const c_char,
}

/// Receives one event, on a host thread inside a `stickshift_*` call. May be NULL.
pub type StickShiftCompletionFn =
    Option<unsafe extern "C" fn(user: *mut c_void, event: *const StickShiftEvent)>;

/// A per-call completion callback: the call's event goes to `callback` instead
/// of the poll queue.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct StickShiftCompletion {
    /// Receives the event. NULL behaves like passing no completion.
    pub callback: StickShiftCompletionFn,
    /// Passed to `callback`.
    pub user: *mut c_void,
}

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

/// How to create a context.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct StickShiftContextDesc {
    /// Must be `STICKSHIFT_API_VERSION`.
    pub api_version: u32,
    /// `STICKSHIFT_BACKEND_METAL` or `STICKSHIFT_BACKEND_NULL`.
    pub backend: StickShiftBackend,
    /// A `CAMetalLayer *` to render into, or NULL for an offscreen context.
    /// Must be NULL with `STICKSHIFT_BACKEND_NULL`.
    pub metal_layer: *mut c_void,
    /// Drawable width in pixels (offscreen: the fixed size), `1..=16384`.
    pub width: u32,
    /// Drawable height in pixels, `1..=16384`.
    pub height: u32,
    /// Drawable pixels per point; `<= 0` means 1.
    pub scale: f32,
}

/// Render quality settings.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StickShiftRenderSettings {
    /// A `STICKSHIFT_SHADOW_QUALITY_*` value.
    pub shadows: StickShiftShadowQuality,
    /// Shadow range as a fraction `(0, 1]` of the draw distance; `<= 0` matches
    /// the draw distance.
    pub shadow_distance: f32,
    /// Draw-distance scale; `1` is the default. Scales the fog and the shadow
    /// range. Must be finite and positive.
    pub draw_distance_scale: f32,
}

/// Counters for diagnostics and tests.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StickShiftStats {
    /// Frames handed to the GPU.
    pub frames_submitted: u64,
    /// Frames the GPU has finished.
    pub frames_completed: u64,
    /// `stickshift_frame_acquire` or `stickshift_frame_submit` calls that returned
    /// `STICKSHIFT_FRAME_SKIPPED`.
    pub frames_skipped: u64,
    /// 3D draws handed to the GPU: one per recorded mesh draw, one per model
    /// part.
    pub draws_submitted: u64,
    /// Draw calls (`stickshift_frame_draw_mesh`, `_draw_model`, `_draw_model_ex`,
    /// `_draw_quad`, `_draw_text`) not made because a resource was pending,
    /// failed or destroyed. Counted once per call, however many parts or glyphs
    /// it drew.
    pub draws_skipped: u64,
    /// Live mesh handles, any state.
    pub meshes: u32,
    /// Live texture handles, any state.
    pub textures: u32,
    /// Live model handles, any state.
    pub models: u32,
    /// Live font handles, any state.
    pub fonts: u32,
    /// Model loads that are neither ready nor failed yet.
    pub loads_pending: u32,
}

// ---------------------------------------------------------------------------
// Resources
// ---------------------------------------------------------------------------

/// How to create a mesh.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct StickShiftMeshDesc {
    /// A `STICKSHIFT_VERTEX_LAYOUT_*` value.
    pub layout: StickShiftVertexLayout,
    /// Interleaved vertices: a whole number of vertices of `layout`.
    pub vertices: StickShiftData,
    /// `uint32_t` triangle-list indices: a multiple of 3, each below the vertex
    /// count. Any alignment.
    pub indices: StickShiftData,
}

/// How to create a texture.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct StickShiftTextureDesc {
    /// Width in pixels, `1..=16384`.
    pub width: u32,
    /// Height in pixels, `1..=16384`.
    pub height: u32,
    /// `width * height * 4` bytes of RGBA8, rows top to bottom.
    pub rgba8: StickShiftData,
    /// How the colour channels are encoded: `STICKSHIFT_COLOR_SPACE_SRGB` for colour
    /// images, `STICKSHIFT_COLOR_SPACE_LINEAR` for data such as a font atlas.
    pub color_space: StickShiftColorSpace,
    /// Reserved; must be 0.
    pub flags: u32,
}

/// One glyph of an SDF font, in em units.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StickShiftGlyph {
    /// Atlas rectangle, left.
    pub u0: f32,
    /// Atlas rectangle, top.
    pub v0: f32,
    /// Atlas rectangle, right.
    pub u1: f32,
    /// Atlas rectangle, bottom.
    pub v1: f32,
    /// Quad width.
    pub width: f32,
    /// Quad height.
    pub height: f32,
    /// Pen to quad left edge.
    pub bearing_x: f32,
    /// Pen (baseline) to quad top edge; negative rises above the baseline.
    pub bearing_y: f32,
    /// Pen advance after the glyph.
    pub advance: f32,
}

/// How to create an SDF font.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct StickShiftFontDesc {
    /// The SDF atlas texture (distance in the red channel).
    pub atlas: StickShiftTexture,
    /// The code point `glyphs[0]` describes.
    pub first_codepoint: u32,
    /// `glyph_count` glyphs for consecutive code points. May be NULL when
    /// `glyph_count` is 0.
    pub glyphs: *const StickShiftGlyph,
    /// The number of glyphs.
    pub glyph_count: u32,
    /// Baseline-to-baseline distance, em units.
    pub line_height: f32,
    /// Used for code points outside the range.
    pub fallback: StickShiftGlyph,
}

// ---------------------------------------------------------------------------
// Frames
// ---------------------------------------------------------------------------

/// The camera a frame is seen from.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StickShiftCamera {
    /// World to clip.
    pub view_projection: StickShiftMat4,
    /// World-space eye position, for specular highlights.
    pub position: StickShiftVec3,
}

/// The sun and sky a frame is lit by.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StickShiftSunSky {
    /// Sun height above the horizon, degrees: 0 horizon, 90 overhead.
    pub sun_elevation_deg: f32,
    /// Sun bearing, degrees from -Z (north) toward +X (east).
    pub sun_azimuth_deg: f32,
    /// Linear RGB sunlight colour.
    pub sun_color: [f32; 3],
    /// Direct-light strength.
    pub sun_intensity: f32,
    /// Ambient fill; keep it well below `sun_intensity`.
    pub sky_fill: f32,
    /// Linear RGB sky colour straight up.
    pub sky_zenith_color: [f32; 3],
    /// Linear RGB sky colour at the horizon; the fog fades toward it.
    pub sky_horizon_color: [f32; 3],
}

/// A point or spot light, added to one frame with `stickshift_frame_add_light`.
///
/// Brightness falls off with the inverse square of the distance and reaches
/// exactly zero at `range`: `intensity` is roughly the brightness one world
/// unit away, on the scale of the sun's `sun_intensity`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StickShiftLight {
    /// A `STICKSHIFT_LIGHT_*` value.
    pub kind: StickShiftLightKind,
    /// World-space position.
    pub position: StickShiftVec3,
    /// Spot lights: the direction the light travels (need not be unit
    /// length). Ignored for point lights.
    pub direction: StickShiftVec3,
    /// Linear RGB colour, each channel `>= 0`.
    pub color: [f32; 3],
    /// Brightness, `>= 0`.
    pub intensity: f32,
    /// Distance at which the light has faded to zero, `> 0`. Shorter is cheaper.
    pub range: f32,
    /// Spot lights: half angle in degrees inside which the light is at full
    /// strength.
    pub inner_angle_deg: f32,
    /// Spot lights: half angle in degrees past which the light is gone;
    /// `inner_angle_deg <= outer_angle_deg <= 90`.
    pub outer_angle_deg: f32,
}

/// A mesh draw's material.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StickShiftMaterial {
    /// Multiplies the base-colour texture (`POS_NORMAL_UV` meshes).
    pub base_color: StickShiftColor,
    /// The base-colour texture for `POS_NORMAL_UV` meshes; `{0}` for white.
    pub base_color_texture: StickShiftTexture,
    /// Linear RGB light the surface gives off, added after lighting: a lit
    /// window or a lamp glows without lighting anything else. May exceed 1.
    /// `{0, 0, 0}` emits nothing.
    pub emissive: [f32; 3],
}

/// A model draw for `stickshift_frame_draw_model_ex`: where the model goes and
/// which of its parts to record.
///
/// Each selected part is drawn at `world × (local × node)`, where `node` is the
/// part's node matrix from the glTF file, multiplied in that grouping. Float
/// products are not associative, so this can differ in the last bit from
/// passing `world × local` as the world matrix of `stickshift_frame_draw_model`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StickShiftModelDraw {
    /// Model to world, applied last.
    pub world: StickShiftMat4,
    /// Applied to each node matrix first, for example the transform that fits
    /// a model to a game's scale. The identity places the model by `world`
    /// alone.
    pub local: StickShiftMat4,
    /// Multiplies each part's base colour, not its emission. `{1, 1, 1, 1}`
    /// leaves the model's colours as they are.
    pub tint: StickShiftColor,
    /// A `STICKSHIFT_MODEL_PARTS_*` value.
    pub parts: StickShiftModelParts,
}

/// A 2D overlay quad, in drawable pixels with the origin at the top-left.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StickShiftQuad {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub width: f32,
    /// Height.
    pub height: f32,
    /// Texture rectangle, left (ignored for `STICKSHIFT_FILL_SOLID`).
    pub u0: f32,
    /// Texture rectangle, top.
    pub v0: f32,
    /// Texture rectangle, right.
    pub u1: f32,
    /// Texture rectangle, bottom.
    pub v1: f32,
    /// Tint; alpha blends.
    pub color: StickShiftColor,
    /// A `STICKSHIFT_FILL_*` value.
    pub fill: StickShiftFill,
    /// The texture for `STICKSHIFT_FILL_TEXTURE` and `STICKSHIFT_FILL_SDF`.
    pub texture: StickShiftTexture,
}

// ---------------------------------------------------------------------------
// The engine: fixed timestep, loop, hooks and host events
// ---------------------------------------------------------------------------

/// The fixed simulation step in integer nanoseconds: `1_000_000_000 / 60`.
/// `STICKSHIFT_CLOCK_FIXED_STEP` banks exactly this much per tick.
pub const STICKSHIFT_FIXED_DT_NS: u64 = 16_666_666;

/// The most update hooks one tick runs; time past them is dropped.
pub const STICKSHIFT_MAX_STEPS_PER_FRAME: u32 = 5;

/// Where an engine's sound goes.
pub type StickShiftAudioOutput = u32;
/// Nowhere: every audio call is accepted and counted, nothing plays. Tools and
/// tests (the default).
pub const STICKSHIFT_AUDIO_SILENT: StickShiftAudioOutput = 0;
/// The default output device, or silent (with one line on stderr) if there is
/// none.
pub const STICKSHIFT_AUDIO_DEVICE: StickShiftAudioOutput = 1;

/// What a tick does when the GPU has no free frame slot.
pub type StickShiftFramePacing = u32;
/// Wait (at most a second) for a free frame slot before the render hook (the
/// default). A tick read-back also waits for its frame to finish.
pub const STICKSHIFT_FRAME_PACING_WAIT: StickShiftFramePacing = 0;
/// Never wait: with no free slot the render hook still runs, into a frame that
/// is then discarded and counted.
pub const STICKSHIFT_FRAME_PACING_SKIP: StickShiftFramePacing = 1;

/// Restore the player's graphics settings at create, and save them on every
/// change.
pub const STICKSHIFT_ENGINE_PERSIST_SETTINGS: u32 = 1 << 0;
/// An Escape key-down event returns `STICKSHIFT_REQUEST_EXIT` (and still
/// presses the key).
pub const STICKSHIFT_ENGINE_QUIT_ON_ESCAPE: u32 = 1 << 1;
/// After init, run no update or render while a model of the context is still
/// loading: such a tick returns `STICKSHIFT_NOT_READY`.
pub const STICKSHIFT_ENGINE_HOLD_UNTIL_LOADED: u32 = 1 << 2;

/// Close the view and stop ticking: the view was closed, Escape was pressed
/// with `STICKSHIFT_ENGINE_QUIT_ON_ESCAPE`, or the game asked to exit.
pub const STICKSHIFT_REQUEST_EXIT: u32 = 1 << 0;
/// Enter or leave full screen: the menu's full-screen item was chosen.
pub const STICKSHIFT_REQUEST_TOGGLE_FULLSCREEN: u32 = 1 << 1;
/// The settings or the full-screen state may have changed: refresh the
/// native Graphics menu.
pub const STICKSHIFT_REQUEST_MENU_SYNC: u32 = 1 << 2;

/// Where a tick's elapsed time comes from.
pub type StickShiftClock = u32;
/// Exactly `STICKSHIFT_FIXED_DT_NS`: one update per tick. Tools and tests.
pub const STICKSHIFT_CLOCK_FIXED_STEP: StickShiftClock = 0;
/// `nanoseconds` passed since the last tick.
pub const STICKSHIFT_CLOCK_ELAPSED: StickShiftClock = 1;
/// `nanoseconds` is the host's monotonic clock now (`CLOCK_UPTIME_RAW` on
/// Apple platforms); the tick banks the time since the last timestamp tick,
/// saturating at 0. The first timestamp tick, and the first after a menu
/// choice, `stickshift_engine_reset_clock` or a held tick, banks exactly
/// `STICKSHIFT_FIXED_DT_NS`.
pub const STICKSHIFT_CLOCK_TIMESTAMP: StickShiftClock = 2;
/// As `STICKSHIFT_CLOCK_TIMESTAMP`, with the engine reading the monotonic clock
/// itself, after the tick's settings step.
pub const STICKSHIFT_CLOCK_ENGINE: StickShiftClock = 3;

/// A host event's kind. Append-only.
pub type StickShiftHostEventKind = u32;
/// A key went down: `code` is a `STICKSHIFT_KEY_*` value, `flags` may hold
/// `STICKSHIFT_HOST_EVENT_REPEAT`. A press of a key already held makes no edge.
pub const STICKSHIFT_HOST_KEY_DOWN: StickShiftHostEventKind = 1;
/// A key went up: `code` is a `STICKSHIFT_KEY_*` value.
pub const STICKSHIFT_HOST_KEY_UP: StickShiftHostEventKind = 2;
/// A mouse button went down: `code` is a `STICKSHIFT_MOUSE_*` value.
pub const STICKSHIFT_HOST_MOUSE_DOWN: StickShiftHostEventKind = 3;
/// A mouse button went up.
pub const STICKSHIFT_HOST_MOUSE_UP: StickShiftHostEventKind = 4;
/// The cursor moved to `x`, `y`, in points from the view's top-left.
pub const STICKSHIFT_HOST_CURSOR: StickShiftHostEventKind = 5;
/// The view lost the keyboard: every key and button is released.
pub const STICKSHIFT_HOST_FOCUS_LOST: StickShiftHostEventKind = 6;
/// The view is now `width` × `height` physical pixels at `scale_factor`: the
/// camera's aspect follows, the drawable is resized, and
/// `STICKSHIFT_REQUEST_MENU_SYNC` is returned.
pub const STICKSHIFT_HOST_VIEW_RESIZED: StickShiftHostEventKind = 7;
/// The view moved to a display of another pixel density: the drawable is
/// resized, the aspect is left alone.
pub const STICKSHIFT_HOST_SCALE_CHANGED: StickShiftHostEventKind = 8;
/// The view's safe-area insets changed: `insets`, in points.
pub const STICKSHIFT_HOST_SAFE_AREA: StickShiftHostEventKind = 9;
/// A Graphics menu item was chosen: `code` is its tag.
pub const STICKSHIFT_HOST_MENU: StickShiftHostEventKind = 10;
/// The user asked to close the view: `STICKSHIFT_REQUEST_EXIT`.
pub const STICKSHIFT_HOST_CLOSE: StickShiftHostEventKind = 11;
/// `STICKSHIFT_HOST_KEY_DOWN`: the platform marked the press an auto-repeat.
pub const STICKSHIFT_HOST_EVENT_REPEAT: u32 = 1 << 0;

/// A key, by physical position. Append-only.
pub type StickShiftKey = u32;
/// The `W` key.
pub const STICKSHIFT_KEY_W: StickShiftKey = 0;
/// The `A` key.
pub const STICKSHIFT_KEY_A: StickShiftKey = 1;
/// The `S` key.
pub const STICKSHIFT_KEY_S: StickShiftKey = 2;
/// The `D` key.
pub const STICKSHIFT_KEY_D: StickShiftKey = 3;
/// The `Q` key.
pub const STICKSHIFT_KEY_Q: StickShiftKey = 4;
/// The `E` key.
pub const STICKSHIFT_KEY_E: StickShiftKey = 5;
/// The left arrow.
pub const STICKSHIFT_KEY_LEFT: StickShiftKey = 6;
/// The right arrow.
pub const STICKSHIFT_KEY_RIGHT: StickShiftKey = 7;
/// The up arrow.
pub const STICKSHIFT_KEY_UP: StickShiftKey = 8;
/// The down arrow.
pub const STICKSHIFT_KEY_DOWN: StickShiftKey = 9;
/// The space bar.
pub const STICKSHIFT_KEY_SPACE: StickShiftKey = 10;
/// The escape key.
pub const STICKSHIFT_KEY_ESCAPE: StickShiftKey = 11;

/// A mouse button. Append-only.
pub type StickShiftMouseButton = u32;
/// The left button.
pub const STICKSHIFT_MOUSE_LEFT: StickShiftMouseButton = 0;
/// The right button.
pub const STICKSHIFT_MOUSE_RIGHT: StickShiftMouseButton = 1;
/// The middle button.
pub const STICKSHIFT_MOUSE_MIDDLE: StickShiftMouseButton = 2;

/// How to create an engine.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct StickShiftEngineDesc {
    /// Must be `STICKSHIFT_API_VERSION`.
    pub api_version: u32,
    /// `sizeof(StickShiftEngineDesc)`.
    pub size: u32,
    /// A `STICKSHIFT_AUDIO_*` value.
    pub audio: StickShiftAudioOutput,
    /// A `STICKSHIFT_FRAME_PACING_*` value.
    pub pacing: StickShiftFramePacing,
    /// `STICKSHIFT_ENGINE_*` bits; any other bit is invalid.
    pub flags: u32,
    /// Reserved; must be 0.
    pub reserved: u32,
    /// With `STICKSHIFT_ENGINE_PERSIST_SETTINGS`: NULL for the standard user
    /// defaults, else the name of a defaults suite (copied). Must be NULL
    /// without the flag.
    pub settings_suite: *const c_char,
}

/// The view an engine draws into, for `stickshift_engine_resume`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct StickShiftViewDesc {
    /// `sizeof(StickShiftViewDesc)`.
    pub size: u32,
    /// The view's width in physical pixels.
    pub width: u32,
    /// The view's height in physical pixels.
    pub height: u32,
    /// Reserved; must be 0.
    pub reserved: u32,
    /// Physical pixels per point.
    pub scale_factor: f64,
    /// Safe-area insets in points: top, right, bottom, left.
    pub safe_area: [f32; 4],
}

/// One tick, for `stickshift_engine_tick`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct StickShiftTickDesc {
    /// `sizeof(StickShiftTickDesc)`.
    pub size: u32,
    /// A `STICKSHIFT_CLOCK_*` value.
    pub clock: StickShiftClock,
    /// `STICKSHIFT_CLOCK_ELAPSED` and `_TIMESTAMP`: the elapsed time or the
    /// clock reading, in nanoseconds. Otherwise 0.
    pub nanoseconds: u64,
    /// Optional: `width * height * 4` bytes of BGRA8, top row first, filled
    /// with this tick's frame before the call returns. Offscreen contexts with
    /// `STICKSHIFT_FRAME_PACING_WAIT` only.
    pub readback: *mut c_void,
    /// The length of `readback` in bytes.
    pub readback_len: usize,
}

/// What one tick did.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StickShiftTickResult {
    /// `sizeof(StickShiftTickResult)`.
    pub size: u32,
    /// Update hooks run.
    pub steps: u32,
    /// 1 if the render hook ran.
    pub rendered: u8,
    /// 1 if the frame went to the GPU; 0 if it was discarded or had no
    /// drawable.
    pub submitted: u8,
    /// 1 if the tick's `readback` holds this frame.
    pub readback_ready: u8,
    /// 1 if `STICKSHIFT_ENGINE_HOLD_UNTIL_LOADED` held this tick.
    pub held: u8,
    /// `STICKSHIFT_REQUEST_*` bits.
    pub requests: u32,
    /// The submitted frame's number, or 0.
    pub frame: u64,
    /// The graphics settings' revision after the tick.
    pub graphics_revision: u64,
}

/// One platform event, for `stickshift_engine_push_events`. 64 bytes; arrays
/// of them cross with an explicit stride, so it can grow.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StickShiftHostEvent {
    /// A `STICKSHIFT_HOST_*` value.
    pub kind: StickShiftHostEventKind,
    /// The key, mouse button or menu tag.
    pub code: u32,
    /// `STICKSHIFT_HOST_EVENT_*` bits.
    pub flags: u32,
    /// `VIEW_RESIZED`, `SCALE_CHANGED`: the view's width in physical pixels.
    pub width: u32,
    /// `VIEW_RESIZED`, `SCALE_CHANGED`: the view's height in physical pixels.
    pub height: u32,
    /// Reserved; must be 0.
    pub reserved: u32,
    /// `VIEW_RESIZED`, `SCALE_CHANGED`: physical pixels per point.
    pub scale_factor: f64,
    /// `CURSOR`: points from the view's left edge.
    pub x: f64,
    /// `CURSOR`: points from the view's top edge.
    pub y: f64,
    /// `SAFE_AREA`: top, right, bottom, left, in points.
    pub insets: [f32; 4],
}

/// An engine's flags and counters.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StickShiftEngineState {
    /// `sizeof(StickShiftEngineState)`.
    pub size: u32,
    /// 1 once the game's init hook has run.
    pub initialized: u8,
    /// 1 once `stickshift_engine_resume` has run.
    pub hosted: u8,
    /// 1 if the init hook failed.
    pub failed: u8,
    /// 1 if a panic was caught in an engine call: every later call that
    /// takes the engine (`stickshift_engine_*`, `_component_*`, `_world_*`,
    /// `_scene_*`, `_stream_*`, `_camera_*`, `_input_*`, `_audio_*`,
    /// `_graphics_*`) but `stickshift_engine_get_state` and
    /// `stickshift_engine_destroy` is now `STICKSHIFT_ERROR_INTERNAL`; calls
    /// on its context still work. Destroy the engine.
    pub poisoned: u8,
    /// 1 once the game, the host or an event asked to exit.
    pub exit_requested: u8,
    /// 1 if the last tick was held by `STICKSHIFT_ENGINE_HOLD_UNTIL_LOADED`.
    pub holding: u8,
    /// Reserved.
    pub reserved: [u8; 2],
    /// Ticks that ran the frame.
    pub frames: u64,
    /// Update hooks run.
    pub updates: u64,
    /// Render hooks run.
    pub frames_rendered: u64,
    /// Frames recorded but not submitted.
    pub frames_discarded: u64,
}

/// Frame timing, durations in nanoseconds.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StickShiftPerf {
    /// `sizeof(StickShiftPerf)`.
    pub size: u32,
    /// The interpolation factor `0..1` in the render hook, else 0.
    pub alpha: f32,
    /// Frames measured.
    pub frame_count: u64,
    /// Frames per second over the last frame.
    pub current_fps: f64,
    /// Frames per second over the window.
    pub avg_fps: f64,
    /// Mean frame time.
    pub avg_frame_ns: u64,
    /// Shortest frame time.
    pub min_frame_ns: u64,
    /// Longest frame time.
    pub max_frame_ns: u64,
    /// Mean CPU time per frame.
    pub avg_cpu_ns: u64,
    /// Mean GPU time per frame, where the backend measures it.
    pub avg_gpu_ns: u64,
    /// Mean time in the render hook.
    pub avg_render_ns: u64,
}

/// Shadow range, for [`StickShiftGraphicsSettings`].
pub type StickShiftShadowDistance = u32;
/// Shadows reach as far as the draw distance (the default).
pub const STICKSHIFT_SHADOW_DISTANCE_MATCH_DRAW_DISTANCE: StickShiftShadowDistance = 0;
/// Shadows reach part of the way.
pub const STICKSHIFT_SHADOW_DISTANCE_MEDIUM: StickShiftShadowDistance = 1;
/// Shadows reach only nearby.
pub const STICKSHIFT_SHADOW_DISTANCE_NEAR: StickShiftShadowDistance = 2;

/// Draw distance, for [`StickShiftGraphicsSettings`].
pub type StickShiftDrawDistance = u32;
/// The nearest draw distance.
pub const STICKSHIFT_DRAW_DISTANCE_NEAR: StickShiftDrawDistance = 0;
/// A medium draw distance.
pub const STICKSHIFT_DRAW_DISTANCE_MEDIUM: StickShiftDrawDistance = 1;
/// The full draw distance (the default).
pub const STICKSHIFT_DRAW_DISTANCE_FAR: StickShiftDrawDistance = 2;

/// Render resolution, for [`StickShiftGraphicsSettings`].
pub type StickShiftRenderScale = u32;
/// Half the view's points per side, times its pixels per point (the default).
pub const STICKSHIFT_RENDER_SCALE_HALF: StickShiftRenderScale = 0;
/// Three quarters.
pub const STICKSHIFT_RENDER_SCALE_THREE_QUARTERS: StickShiftRenderScale = 1;
/// The view's full physical resolution.
pub const STICKSHIFT_RENDER_SCALE_FULL: StickShiftRenderScale = 2;

/// The player-facing graphics settings. A value mirror: frozen, no size field.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StickShiftGraphicsSettings {
    /// A `STICKSHIFT_SHADOW_QUALITY_*` value.
    pub shadows: StickShiftShadowQuality,
    /// A `STICKSHIFT_SHADOW_DISTANCE_*` value.
    pub shadow_distance: StickShiftShadowDistance,
    /// A `STICKSHIFT_DRAW_DISTANCE_*` value.
    pub draw_distance: StickShiftDrawDistance,
    /// A `STICKSHIFT_RENDER_SCALE_*` value.
    pub render_scale: StickShiftRenderScale,
    /// Reserved for future settings; 0.
    pub reserved: [u32; 4],
}

/// The game's init hook: exactly once, before any update. A negative result
/// fails the engine (`STICKSHIFT_ERROR_ABORTED`).
pub type StickShiftGameInitFn = Option<
    unsafe extern "C" fn(user: *mut c_void, engine: *mut StickShiftEngine) -> StickShiftResult,
>;
/// The game's update hook: 0 to `STICKSHIFT_MAX_STEPS_PER_FRAME` times a
/// tick, `dt` always `stickshift_fixed_dt()`.
pub type StickShiftGameUpdateFn =
    Option<unsafe extern "C" fn(user: *mut c_void, engine: *mut StickShiftEngine, dt: f32)>;
/// The game's render hook: exactly once a tick, after the tick's updates, with
/// an open frame the engine submits when the hook returns.
pub type StickShiftGameRenderFn = Option<
    unsafe extern "C" fn(
        user: *mut c_void,
        engine: *mut StickShiftEngine,
        frame: *mut crate::context::StickShiftFrame,
    ),
>;
/// The game's settings hook: between frames, after a settings change reaches
/// the renderer and the scene.
pub type StickShiftGameSettingsFn = Option<
    unsafe extern "C" fn(
        user: *mut c_void,
        engine: *mut StickShiftEngine,
        settings: *const StickShiftGraphicsSettings,
    ),
>;
/// Called exactly once, from `stickshift_engine_destroy`.
pub type StickShiftGameReleaseFn = Option<unsafe extern "C" fn(user: *mut c_void)>;

/// A game: its hooks and their user pointer. Copied at create.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct StickShiftGame {
    /// `sizeof(StickShiftGame)`.
    pub size: u32,
    /// Reserved; must be 0.
    pub reserved: u32,
    /// Passed to every hook; borrowed until `release`.
    pub user: *mut c_void,
    /// Required.
    pub init: StickShiftGameInitFn,
    /// Required.
    pub update: StickShiftGameUpdateFn,
    /// Required.
    pub render: StickShiftGameRenderFn,
    /// Optional: NULL does nothing on a settings change.
    pub graphics_settings_changed: StickShiftGameSettingsFn,
    /// Optional.
    pub release: StickShiftGameReleaseFn,
}

// ---------------------------------------------------------------------------
// Inspection
// ---------------------------------------------------------------------------

/// One 3D draw of a recorded frame.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StickShiftDrawRecord {
    /// The mesh or model handle id the draw named.
    pub source: u64,
    /// The mesh draw's texture handle id, or 0 (always 0 for a model part).
    pub texture: u64,
    /// The model's node-part index, or 0 for a mesh.
    pub part: u32,
    /// The `STICKSHIFT_VERTEX_LAYOUT_*` value of the drawn mesh.
    pub layout: StickShiftVertexLayout,
    /// The matrix the backend draws with: for a model part,
    /// `world × (local × node)`.
    pub model: StickShiftMat4,
    /// The base colour, after the tint.
    pub base_color: StickShiftColor,
    /// Linear RGB emission.
    pub emissive: [f32; 3],
    /// Reserved.
    pub reserved: u32,
}

/// What the last frame to reach submit on a context recorded.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct StickShiftFrameRecord {
    /// `sizeof(StickShiftFrameRecord)`.
    pub size: u32,
    /// 1 if the frame went to the GPU; 0 if it was discarded or had no
    /// drawable.
    pub submitted: u8,
    /// Reserved.
    pub reserved: [u8; 3],
    /// The frame's number, or 0 (before any frame, or not submitted).
    pub frame_number: u64,
    /// The draws still valid at submit, in record order. Context-owned: valid
    /// until the next frame is opened on the context.
    pub draws: *const StickShiftDrawRecord,
    /// The number of `draws`.
    pub draw_count: u32,
    /// Bytes from one draw record to the next.
    pub draw_stride: u32,
    /// The overlay quads still valid at submit, in record order (text as its
    /// glyph quads). Valid as long as `draws`.
    pub quads: *const StickShiftQuad,
    /// The number of `quads`.
    pub quad_count: u32,
    /// Bytes from one quad to the next.
    pub quad_stride: u32,
    /// The camera the frame used.
    pub camera: StickShiftCamera,
    /// The sun and sky the frame used.
    pub sun: StickShiftSunSky,
    /// The render settings in effect.
    pub settings: StickShiftRenderSettings,
    /// The frame's point and spot lights.
    pub light_count: u32,
    /// Reserved.
    pub reserved2: u32,
}

// ---------------------------------------------------------------------------
// Camera, input, audio, graphics settings and the menu
// ---------------------------------------------------------------------------

/// The engine camera, as `stickshift_camera_get` reads it. The matrices are
/// computed when it is read.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StickShiftCameraState {
    /// `sizeof(StickShiftCameraState)`.
    pub size: u32,
    /// Reserved.
    pub reserved: u32,
    /// The eye's world position.
    pub position: StickShiftVec3,
    /// The point looked at.
    pub target: StickShiftVec3,
    /// The world up axis the view is oriented by.
    pub up: StickShiftVec3,
    /// The vertical field of view in radians (45° by default).
    pub fov_y: f32,
    /// Width over height (4/3 until a view or the game sets it).
    pub aspect_ratio: f32,
    /// The near clip distance.
    pub near_plane: f32,
    /// The far clip distance.
    pub far_plane: f32,
    /// World to view.
    pub view: StickShiftMat4,
    /// View to clip.
    pub projection: StickShiftMat4,
    /// World to clip: `projection × view`.
    pub view_projection: StickShiftMat4,
    /// The unit direction from the eye to the target (`-Z` if they meet).
    pub forward: StickShiftVec3,
    /// The unit direction to the eye's right.
    pub right: StickShiftVec3,
}

/// A chase camera: the inputs of the engine's chase controller, applied by
/// `stickshift_camera_follow`. A frozen value type, with no `size` field;
/// `stickshift_chase_init` fills in the defaults.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StickShiftChase {
    /// How far behind the target the camera trails, along its facing.
    pub distance: f32,
    /// How far above the target the camera sits, along `up`.
    pub height: f32,
    /// How far above the target the camera aims.
    pub look_at_height: f32,
    /// The world up axis, as given: it is normalized when used, and a zero or
    /// non-finite axis means `+Y`. `{0, 1, 0}` by default.
    pub up: StickShiftVec3,
    /// Seconds for the camera to close half the gap to its pose; `0` (the
    /// default) snaps, and a negative value counts as `0`.
    pub half_life: f32,
}

/// `StickShiftInputState::keys`: the key is held.
pub const STICKSHIFT_KEY_STATE_DOWN: u8 = 1 << 0;
/// `StickShiftInputState::keys`: the key went down since the last update.
pub const STICKSHIFT_KEY_STATE_JUST_PRESSED: u8 = 1 << 1;
/// `StickShiftInputState::keys`: the key went up since the last update.
pub const STICKSHIFT_KEY_STATE_JUST_RELEASED: u8 = 1 << 2;

/// The keyboard and mouse as the game sees them. Edges last until the end of
/// the next update, so a tap between two frames reaches the next update.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StickShiftInputState {
    /// `sizeof(StickShiftInputState)`.
    pub size: u32,
    /// Per `STICKSHIFT_KEY_*` value: `STICKSHIFT_KEY_STATE_*` bits. 0 past the
    /// last key.
    pub keys: [u8; 32],
    /// Per `STICKSHIFT_MOUSE_*` value: 1 while the button is held.
    pub mouse_buttons: [u8; 8],
    /// 1 once the host has reported a cursor position.
    pub has_cursor: u8,
    /// Reserved.
    pub reserved: [u8; 3],
    /// The cursor, in points from the view's left edge.
    pub cursor_x: f64,
    /// The cursor, in points from the view's top edge.
    pub cursor_y: f64,
}

/// A sound an engine loaded. Valid until the engine is destroyed (sounds are
/// never unloaded). `id == 0` is the null handle.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct StickShiftSound {
    /// The handle's identity; meaningful only to the engine that issued it.
    pub id: u64,
}

/// A volume of 0 dB: the level the sound was recorded at.
pub const STICKSHIFT_VOLUME_UNCHANGED_DB: f32 = 0.0;
/// -60 dB: at and below it a sound is inaudible.
pub const STICKSHIFT_VOLUME_SILENT_DB: f32 = -60.0;

/// The engine's audio counters: what the game asked for, counted in silent
/// mode too.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StickShiftAudioStats {
    /// `sizeof(StickShiftAudioStats)`.
    pub size: u32,
    /// Distinct sounds loaded (one per path).
    pub sound_count: u32,
    /// Sounds actually decoded: 0 in silent mode.
    pub decode_count: u32,
    /// Loops that began playing.
    pub loop_starts: u32,
    /// One-shots asked for.
    pub one_shots_played: u32,
    /// The master volume in dB.
    pub master_volume_db: f32,
    /// The sound on the loop channel, or the null handle.
    pub looping: StickShiftSound,
    /// 1 if the engine plays to no device.
    pub is_silent: u8,
    /// Reserved.
    pub reserved: [u8; 7],
}

/// The graphics settings, with what is queued and how often they changed.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StickShiftGraphicsState {
    /// `sizeof(StickShiftGraphicsState)`.
    pub size: u32,
    /// 1 if `pending` holds a request not yet applied.
    pub has_pending: u8,
    /// 1 if `base_spawn_ahead` is set (after init).
    pub has_base_spawn_ahead: u8,
    /// Reserved.
    pub reserved: [u8; 2],
    /// The settings in effect.
    pub current: StickShiftGraphicsSettings,
    /// The request the next frame applies, if `has_pending`.
    pub pending: StickShiftGraphicsSettings,
    /// The streaming reach the game set up in init, which the draw distance
    /// scales.
    pub base_spawn_ahead: f32,
    /// Reserved.
    pub reserved2: u32,
    /// How many changes have been applied since create (a restore is not
    /// one).
    pub revision: u64,
}

/// A section of the Graphics menu, and the setting it chooses: the hundreds of
/// a menu tag, for `stickshift_graphics_label`.
pub type StickShiftGraphicsSection = u32;
/// Shadow quality: a `STICKSHIFT_SHADOW_QUALITY_*` value.
pub const STICKSHIFT_GRAPHICS_SECTION_SHADOWS: StickShiftGraphicsSection = 1;
/// Shadow range: a `STICKSHIFT_SHADOW_DISTANCE_*` value.
pub const STICKSHIFT_GRAPHICS_SECTION_SHADOW_DISTANCE: StickShiftGraphicsSection = 2;
/// Draw distance: a `STICKSHIFT_DRAW_DISTANCE_*` value.
pub const STICKSHIFT_GRAPHICS_SECTION_DRAW_DISTANCE: StickShiftGraphicsSection = 3;
/// Render resolution: a `STICKSHIFT_RENDER_SCALE_*` value.
pub const STICKSHIFT_GRAPHICS_SECTION_RENDER_SCALE: StickShiftGraphicsSection = 4;

/// One item of the Graphics menu.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct StickShiftMenuItem {
    /// The item's tag, `section * 100 + index` (never 0): the `code` of the
    /// `STICKSHIFT_HOST_MENU` event that chooses it.
    pub tag: i32,
    /// The item's title. Engine-owned, as the menu is.
    pub label: *const c_char,
    /// 1 if the item shows a checkmark: the current choice of its section.
    pub checked: u8,
    /// 1 if the item can be chosen.
    pub enabled: u8,
    /// Reserved.
    pub reserved: [u8; 2],
}

/// A group of the Graphics menu: a submenu of choices, or the actions shown
/// after a separator.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct StickShiftMenuGroup {
    /// The submenu's title, or NULL for the actions after a separator.
    pub title: *const c_char,
    /// The group's items, `item_stride` bytes apart.
    pub items: *const StickShiftMenuItem,
    /// The number of `items`.
    pub item_count: u32,
    /// Bytes from one item to the next.
    pub item_stride: u32,
}

/// The engine's Graphics menu, as data for a host to build its native menu
/// from. Engine-owned: valid until the next `stickshift_engine_get_menu` or
/// the engine's destroy.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct StickShiftMenu {
    /// `sizeof(StickShiftMenu)`.
    pub size: u32,
    /// The number of `groups`.
    pub group_count: u32,
    /// The menu's title.
    pub title: *const c_char,
    /// The groups, in menu order, `group_stride` bytes apart.
    pub groups: *const StickShiftMenuGroup,
    /// Bytes from one group to the next.
    pub group_stride: u32,
    /// Reserved.
    pub reserved: u32,
    /// The settings revision the checkmarks show.
    pub revision: u64,
}
