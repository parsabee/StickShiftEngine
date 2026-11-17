// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! An engine's scene through the C API: the streaming configuration, the
//! stream pass with a C spawn callback (`stickshift_scene_stream`,
//! `stickshift_stream_track`) and the floating-origin rebase.
//!
//! The pass is the scene's own `Scene::stream`: the C callback runs inside its
//! closure, once per new slot, with the closure's `SpawnCtx` on the engine's
//! scope stack. While it runs, world calls reach `SpawnCtx::world`,
//! `stickshift_stream_track` is `SpawnCtx::spawned`, and every other game call
//! is refused, since the pass holds the whole scene.

use stickshift_math::glam::Vec3;
use stickshift_scene::{Scene, StreamingConfig};

use crate::core::{call_or, input, output, Error};
use crate::engine::{in_spawn_callback, poisoned, scene_call, SceneAccess, StickShiftEngine};
use crate::types::*;
use crate::world::entity_of;

impl From<StreamingConfig> for StickShiftStreamingConfig {
    fn from(c: StreamingConfig) -> Self {
        Self {
            axis: c.axis.into(),
            spawn_interval: c.spawn_interval,
            spawn_ahead: c.spawn_ahead,
            despawn_behind: c.despawn_behind,
            rebase_threshold: c.rebase_threshold,
        }
    }
}

impl From<StickShiftStreamingConfig> for StreamingConfig {
    fn from(c: StickShiftStreamingConfig) -> Self {
        Self {
            axis: c.axis.into(),
            spawn_interval: c.spawn_interval,
            spawn_ahead: c.spawn_ahead,
            despawn_behind: c.despawn_behind,
            rebase_threshold: c.rebase_threshold,
        }
    }
}

/// A streaming or rebase focus, which must be finite: a NaN or infinite one
/// would make every later slot, cutoff and transform non-finite.
fn focus_of(focus: StickShiftVec3) -> Result<Vec3, Error> {
    let focus = Vec3::from(focus);
    if focus.is_finite() {
        Ok(focus)
    } else {
        Err(Error::invalid_argument(format!(
            "focus {focus} must be finite"
        )))
    }
}

/// The scene, for a call that reads it: between engine calls or in any hook,
/// not in a spawn callback.
fn scene_ref<'a>(access: SceneAccess<'a>, what: &str) -> Result<&'a Scene, Error> {
    match access {
        SceneAccess::Scene { scene, .. } => Ok(scene),
        SceneAccess::Spawn(_) => Err(in_spawn_callback(what)),
    }
}

/// The scene, for a call that changes it: between engine calls, or in an
/// init, update or settings hook.
fn scene_mut<'a>(access: SceneAccess<'a>, what: &str) -> Result<&'a mut Scene, Error> {
    match access {
        SceneAccess::Scene { render: true, .. } => Err(Error::wrong_state(format!(
            "{what} changes the scene, which the render hook may only read"
        ))),
        SceneAccess::Scene { scene, .. } => Ok(scene),
        SceneAccess::Spawn(_) => Err(in_spawn_callback(what)),
    }
}

/// The default streaming configuration. Thread-safe; NULL does nothing.
///
/// # Safety
/// `out` must be NULL or valid.
#[no_mangle]
pub unsafe extern "C" fn stickshift_streaming_config_default(out: *mut StickShiftStreamingConfig) {
    call_or((), || {
        if let Some(out) = out.as_mut() {
            *out = StreamingConfig::default().into();
        }
    });
}

/// The scene's streaming configuration, a copy. After init, the engine has
/// scaled `spawn_ahead` for the draw distance. Not in a spawn callback.
///
/// # Safety
/// `engine` must be NULL or a live engine; `out` NULL or valid.
#[no_mangle]
pub unsafe extern "C" fn stickshift_scene_get_config(
    engine: *mut StickShiftEngine,
    out: *mut StickShiftStreamingConfig,
) -> StickShiftResult {
    scene_call(engine, "stickshift_scene_get_config", |_, access| {
        let out = output(out, "out")?;
        *out = scene_ref(access, "stickshift_scene_get_config")?
            .config()
            .into();
        Ok(STICKSHIFT_OK)
    })
}

/// Replace the streaming configuration. Once streaming has started, changing
/// `axis` or `spawn_interval` is `STICKSHIFT_ERROR_WRONG_STATE` and changes
/// nothing; other values are taken as given. Not in the render hook or a
/// spawn callback.
///
/// # Safety
/// `engine` must be NULL or a live engine; `config` NULL or valid.
#[no_mangle]
pub unsafe extern "C" fn stickshift_scene_set_config(
    engine: *mut StickShiftEngine,
    config: *const StickShiftStreamingConfig,
) -> StickShiftResult {
    scene_call(engine, "stickshift_scene_set_config", |_, access| {
        let config = StreamingConfig::from(*input(config, "config")?);
        scene_mut(access, "stickshift_scene_set_config")?
            .try_set_config(config)
            .map_err(|error| Error::wrong_state(error.to_string()))?;
        Ok(STICKSHIFT_OK)
    })
}

/// One streaming pass for `focus`: first the streamed entities fallen behind
/// are despawned, in the order they were tracked, then `spawn` is called once
/// per new slot, in slot order, from inside the pass. `spawn` may be NULL (the
/// frontier still advances). A `focus` that is not finite is
/// `STICKSHIFT_ERROR_INVALID_ARGUMENT`. Not in the render hook or a spawn
/// callback.
///
/// # Safety
/// `engine` must be NULL or a live engine; `spawn` NULL or a function that
/// may be called with `user` during this call.
#[no_mangle]
pub unsafe extern "C" fn stickshift_scene_stream(
    engine: *mut StickShiftEngine,
    focus: StickShiftVec3,
    spawn: StickShiftSpawnFn,
    user: *mut std::ffi::c_void,
) -> StickShiftResult {
    scene_call(engine, "stickshift_scene_stream", |e, access| {
        let focus = focus_of(focus)?;
        let scene = scene_mut(access, "stickshift_scene_stream")?;
        let this = std::ptr::from_ref(e).cast_mut();
        scene.stream(focus, |cx| {
            let Some(spawn) = spawn else { return };
            if e.is_poisoned() {
                return;
            }
            let (slot, position) = (cx.slot, cx.position.into());
            let _scope = e.enter_spawn(cx);
            // SAFETY: the caller's callback, with its user pointer and this
            // engine; the spawn context is on the scope stack until it returns.
            unsafe { spawn(user, this, slot, position) };
        });
        if e.is_poisoned() {
            return Err(poisoned());
        }
        Ok(STICKSHIFT_OK)
    })
}

/// Track `entity` for despawning behind, at the current slot's distance when
/// it has no transform: `SpawnCtx::spawned`. Only in a spawn callback; the
/// null entity is `STICKSHIFT_ERROR_INVALID_ARGUMENT`.
///
/// # Safety
/// `engine` must be NULL or a live engine.
#[no_mangle]
pub unsafe extern "C" fn stickshift_stream_track(
    engine: *mut StickShiftEngine,
    entity: StickShiftEntity,
) -> StickShiftResult {
    scene_call(engine, "stickshift_stream_track", |_, access| {
        let SceneAccess::Spawn(cx) = access else {
            return Err(Error::wrong_state(
                "stickshift_stream_track can only be called from a stream spawn callback",
            ));
        };
        let entity = entity_of(entity)
            .ok_or_else(|| Error::invalid_argument("entity is the null entity"))?;
        cx.spawned(entity);
        Ok(STICKSHIFT_OK)
    })
}

/// Rebase the floating origin if `focus` has drifted past the threshold:
/// every transform component is shifted by the same offset, written to
/// `*out_offset` (zero when no rebase was due). Anything the game keeps in
/// world coordinates outside the world (the camera, its own state) is the
/// game's to shift. A `focus` that is not finite is
/// `STICKSHIFT_ERROR_INVALID_ARGUMENT`. Not in the render hook or a spawn
/// callback.
///
/// # Safety
/// `engine` must be NULL or a live engine; `out_offset` NULL or valid.
#[no_mangle]
pub unsafe extern "C" fn stickshift_scene_maybe_rebase(
    engine: *mut StickShiftEngine,
    focus: StickShiftVec3,
    out_offset: *mut StickShiftVec3,
) -> StickShiftResult {
    scene_call(engine, "stickshift_scene_maybe_rebase", |_, access| {
        let out = output(out_offset, "out_offset")?;
        let focus = focus_of(focus)?;
        *out = scene_mut(access, "stickshift_scene_maybe_rebase")?
            .maybe_rebase(focus)
            .into();
        Ok(STICKSHIFT_OK)
    })
}

/// How many streamed entities the scene tracks for despawning behind. Not in a
/// spawn callback.
///
/// # Safety
/// `engine` must be NULL or a live engine; `out` NULL or valid.
#[no_mangle]
pub unsafe extern "C" fn stickshift_scene_streamed_count(
    engine: *mut StickShiftEngine,
    out: *mut u32,
) -> StickShiftResult {
    scene_call(engine, "stickshift_scene_streamed_count", |_, access| {
        let out = output(out, "out")?;
        let count = scene_ref(access, "stickshift_scene_streamed_count")?.streamed_count();
        *out = u32::try_from(count).unwrap_or(u32::MAX);
        Ok(STICKSHIFT_OK)
    })
}
