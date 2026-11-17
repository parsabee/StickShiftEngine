// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! The Stick Shift Engine **C API**: `libstickshift.a` / `libstickshift.dylib` and the generated
//! `include/stickshift.h`.
//!
//! The contract is `docs/C_API.md`; this crate implements it. In short:
//!
//! - One opaque [`StickShiftContext`] owns everything; meshes, textures, models and
//!   fonts are typed 64-bit handles with 32-bit generations, never reissued.
//! - The API is **asynchronous in shape**: calls return at once with a handle or
//!   a result code, resources go `PENDING` → `READY` / `FAILED`, and completions
//!   arrive as events, through `stickshift_poll_events` or per-call callbacks that
//!   always run on a host thread.
//! - In v0.1, mesh and texture uploads still happen inside their create
//!   calls; glTF models are parsed on worker threads and uploaded during
//!   housekeeping; frames are recorded CPU-side and submitted through the Metal
//!   backend's non-blocking path, so `stickshift_frame_acquire` never waits.
//! - A context is used by one thread at a time; an overlapping call returns
//!   `STICKSHIFT_ERROR_BUSY` instead of waiting. No panic crosses the boundary:
//!   every function catches one and returns `STICKSHIFT_ERROR_INTERNAL`.
//! - A [`StickShiftEngine`] attached to a context runs the engine's own loop
//!   (`stickshift-core`'s `drive_frame`, through its platform-free `Session`)
//!   for a host that calls `stickshift_engine_tick` once per display refresh.
//!   The game is C callbacks. An engine and its context are used on the
//!   engine's thread only (`STICKSHIFT_ERROR_WRONG_THREAD`).
//!
//! The crate links `stickshift-render`, `stickshift-render-api`, `stickshift-assets`
//! (without its ECS feature), `stickshift-math`, and `stickshift-core` without its
//! `windowed` feature, so no windowing crate and no AppKit menu reach it. On
//! platforms without Metal only the null backend exists, so the crate and its
//! tests build everywhere.
//!
//! # Safety
//!
//! Every exported function is `unsafe extern "C"`: pointer arguments must be
//! NULL or valid as `stickshift.h` documents (a context from `stickshift_context_create`
//! that has not been destroyed, a frame from `stickshift_frame_acquire`, readable
//! inputs, writable outputs, NUL-terminated strings). Everything else (NULL,
//! wrong handles, bad enum values, mismatched sizes, overlapping calls) is
//! checked and reported as a result code.

#![deny(missing_docs)]
// Each function documents its pointer requirements; the crate-level `# Safety`
// section above states the shared contract once instead of in every function.
#![allow(clippy::missing_safety_doc)]

mod backend;
mod context;
mod core;
mod engine;
mod fonts;
mod frame;
mod handles;
mod math;
#[cfg(target_os = "macos")]
mod metal;
mod models;
mod resources;
pub mod types;

pub use context::{
    stickshift_context_create, stickshift_context_destroy, stickshift_context_device_name,
    stickshift_context_flush, stickshift_context_get_camera, stickshift_context_get_last_frame,
    stickshift_context_get_safe_area, stickshift_context_get_scale,
    stickshift_context_get_settings, stickshift_context_get_size, stickshift_context_get_stats,
    stickshift_context_get_sun_sky, stickshift_context_gpu_frame_time, stickshift_context_resize,
    stickshift_context_set_camera, stickshift_context_set_safe_area,
    stickshift_context_set_settings, stickshift_context_set_sun_sky, stickshift_poll_events,
    stickshift_render_settings_default, stickshift_resource_state, StickShiftContext,
    StickShiftFrame,
};
pub use core::{
    stickshift_api_minor, stickshift_api_version, stickshift_fixed_dt,
    stickshift_last_error_message, stickshift_result_name, stickshift_version_string,
};
pub use engine::audio::{
    stickshift_audio_get_path, stickshift_audio_get_stats, stickshift_audio_load,
    stickshift_audio_one_shots_of, stickshift_audio_play_looping, stickshift_audio_play_once,
    stickshift_audio_set_master_volume, stickshift_audio_stop_looping,
};
pub use engine::camera::{
    stickshift_camera_follow, stickshift_camera_get, stickshift_camera_set_aspect_ratio,
    stickshift_camera_set_clip_planes, stickshift_camera_set_fov_y, stickshift_camera_set_position,
    stickshift_camera_set_target, stickshift_camera_set_up, stickshift_chase_init,
};
pub use engine::graphics::{
    stickshift_engine_get_menu, stickshift_graphics_get_current, stickshift_graphics_get_state,
    stickshift_graphics_label, stickshift_graphics_request, stickshift_graphics_settings_default,
    stickshift_graphics_settings_from_codes, stickshift_graphics_settings_render_settings,
    stickshift_graphics_settings_to_codes, stickshift_render_scale_surface_for,
};
pub use engine::input::stickshift_input_get;
pub use engine::{
    stickshift_engine_create, stickshift_engine_destroy, stickshift_engine_get_context,
    stickshift_engine_get_perf, stickshift_engine_get_state, stickshift_engine_init,
    stickshift_engine_push_events, stickshift_engine_request_exit, stickshift_engine_reset_clock,
    stickshift_engine_resume, stickshift_engine_tick, testing, StickShiftEngine,
};
pub use fonts::{stickshift_font_create, stickshift_font_destroy, stickshift_font_measure};
pub use frame::{
    stickshift_frame_acquire, stickshift_frame_add_light, stickshift_frame_discard,
    stickshift_frame_draw_mesh, stickshift_frame_draw_model, stickshift_frame_draw_model_ex,
    stickshift_frame_draw_quad, stickshift_frame_draw_text, stickshift_frame_read_pixels_async,
    stickshift_frame_set_camera, stickshift_frame_set_sun_sky, stickshift_frame_submit,
    stickshift_sun_sky_default,
};
pub use math::{
    stickshift_mat4_from_trs, stickshift_mat4_identity, stickshift_mat4_look_at,
    stickshift_mat4_mul, stickshift_mat4_perspective, stickshift_mat4_transform_point3,
    stickshift_mat4_transform_vector3, stickshift_quat_from_axis_angle,
    stickshift_quat_from_rotation_x, stickshift_quat_from_rotation_y,
    stickshift_quat_from_rotation_z, stickshift_quat_mul, stickshift_quat_mul_vec3,
    stickshift_transform_to_mat4, stickshift_vec3_cross, stickshift_vec3_dot,
    stickshift_vec3_length, stickshift_vec3_lerp, stickshift_vec3_normalize_or_zero,
};
pub use models::{
    stickshift_model_destroy, stickshift_model_get_bounds, stickshift_model_load_file,
    stickshift_model_load_memory, stickshift_model_wait,
};
pub use resources::{
    stickshift_mesh_create, stickshift_mesh_destroy, stickshift_texture_create,
    stickshift_texture_destroy,
};
pub use types::*;
