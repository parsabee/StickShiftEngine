// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! The engine camera and the chase controller through C.
//!
//! Inside a hook every write goes through `EngineCtx::camera_mut`, which marks
//! the camera moved, so the game's next context call pushes it to the context
//! first (the push rule), exactly as `EngineCtx::renderer` does in Rust. The
//! setters are plain stores, as `Camera`'s own are.

use stickshift_camera::{Camera, ChaseController};
use stickshift_math::glam::Vec3;

use super::{game_call, Access, StickShiftEngine};
use crate::core::{input, write_sized};
use crate::types::*;

impl Access<'_> {
    /// The engine camera as the game sees it.
    fn camera(&self) -> &Camera {
        match self {
            Access::Idle(state) => &state.session.lp.camera,
            Access::Hook(ctx) => ctx.camera(),
        }
    }

    /// The engine camera, to move. In a hook this marks it moved.
    fn camera_mut(&mut self) -> &mut Camera {
        match self {
            Access::Idle(state) => &mut state.session.lp.camera,
            Access::Hook(ctx) => ctx.camera_mut(),
        }
    }
}

/// Move the engine camera with `f`, in a hook or between engine calls.
///
/// # Safety
/// `engine` must be NULL or a live engine.
unsafe fn with_camera_mut(
    engine: *mut StickShiftEngine,
    what: &str,
    f: impl FnOnce(&mut Camera),
) -> StickShiftResult {
    game_call(engine, what, |_, mut access| {
        f(access.camera_mut());
        Ok(STICKSHIFT_OK)
    })
}

/// The engine camera: its pose, lens and the matrices it gives now. In a hook,
/// or between engine calls. Set `out->size = sizeof(StickShiftCameraState)`.
///
/// # Safety
/// `engine` must be NULL or a live engine; `out` NULL or valid for its
/// declared size.
#[no_mangle]
pub unsafe extern "C" fn stickshift_camera_get(
    engine: *mut StickShiftEngine,
    out: *mut StickShiftCameraState,
) -> StickShiftResult {
    game_call(engine, "stickshift_camera_get", |_, access| {
        let camera = access.camera();
        let (near_plane, far_plane) = camera.clip_planes();
        let state = StickShiftCameraState {
            size: 0,
            reserved: 0,
            position: camera.position().into(),
            target: camera.target().into(),
            up: camera.up().into(),
            fov_y: camera.fov_y(),
            aspect_ratio: camera.aspect_ratio(),
            near_plane,
            far_plane,
            view: camera.view_matrix().into(),
            projection: camera.projection_matrix().into(),
            view_projection: camera.view_projection_matrix().into(),
            forward: camera.forward().into(),
            right: camera.right().into(),
        };
        write_sized(out, state, "out")?;
        Ok(STICKSHIFT_OK)
    })
}

/// Put the camera's eye at `position`. In a hook, or between engine calls; in a
/// hook, the game's next context call pushes the moved camera first.
///
/// # Safety
/// `engine` must be NULL or a live engine.
#[no_mangle]
pub unsafe extern "C" fn stickshift_camera_set_position(
    engine: *mut StickShiftEngine,
    position: StickShiftVec3,
) -> StickShiftResult {
    with_camera_mut(engine, "stickshift_camera_set_position", |c| {
        c.set_position(position.into());
    })
}

/// Aim the camera at `target`. As `stickshift_camera_set_position`.
///
/// # Safety
/// `engine` must be NULL or a live engine.
#[no_mangle]
pub unsafe extern "C" fn stickshift_camera_set_target(
    engine: *mut StickShiftEngine,
    target: StickShiftVec3,
) -> StickShiftResult {
    with_camera_mut(engine, "stickshift_camera_set_target", |c| {
        c.set_target(target.into());
    })
}

/// Orient the camera by the world axis `up`. As
/// `stickshift_camera_set_position`.
///
/// # Safety
/// `engine` must be NULL or a live engine.
#[no_mangle]
pub unsafe extern "C" fn stickshift_camera_set_up(
    engine: *mut StickShiftEngine,
    up: StickShiftVec3,
) -> StickShiftResult {
    with_camera_mut(engine, "stickshift_camera_set_up", |c| c.set_up(up.into()))
}

/// Set the projection's width over height, stored as given. A view's resize
/// sets it too. As `stickshift_camera_set_position`.
///
/// # Safety
/// `engine` must be NULL or a live engine.
#[no_mangle]
pub unsafe extern "C" fn stickshift_camera_set_aspect_ratio(
    engine: *mut StickShiftEngine,
    aspect_ratio: f32,
) -> StickShiftResult {
    with_camera_mut(engine, "stickshift_camera_set_aspect_ratio", |c| {
        c.set_aspect_ratio(aspect_ratio);
    })
}

/// Set the vertical field of view in radians, stored as given. As
/// `stickshift_camera_set_position`.
///
/// # Safety
/// `engine` must be NULL or a live engine.
#[no_mangle]
pub unsafe extern "C" fn stickshift_camera_set_fov_y(
    engine: *mut StickShiftEngine,
    fov_y: f32,
) -> StickShiftResult {
    with_camera_mut(engine, "stickshift_camera_set_fov_y", |c| {
        c.set_fov_y(fov_y)
    })
}

/// Set the near and far clip distances. Planes that are not finite, not
/// positive, or not `far > near` are ignored and the camera keeps its own, as
/// in Rust: the call returns `STICKSHIFT_OK` either way. As
/// `stickshift_camera_set_position`.
///
/// # Safety
/// `engine` must be NULL or a live engine.
#[no_mangle]
pub unsafe extern "C" fn stickshift_camera_set_clip_planes(
    engine: *mut StickShiftEngine,
    near_plane: f32,
    far_plane: f32,
) -> StickShiftResult {
    with_camera_mut(engine, "stickshift_camera_set_clip_planes", |c| {
        c.set_clip_planes(near_plane, far_plane);
    })
}

/// A chase camera trailing `distance` behind and `height` above its target,
/// aiming at the target itself, `+Y` up, with no smoothing. NULL is a no-op.
/// Thread-safe.
///
/// # Safety
/// `out` must be NULL or valid for writes.
#[no_mangle]
pub unsafe extern "C" fn stickshift_chase_init(
    out: *mut StickShiftChase,
    distance: f32,
    height: f32,
) {
    if let Some(out) = out.as_mut() {
        *out = StickShiftChase {
            distance,
            height,
            look_at_height: 0.0,
            up: Vec3::Y.into(),
            half_life: 0.0,
        };
    }
}

/// Move the engine camera after a target at `target` facing `forward`, `dt`
/// seconds after the last follow: the engine's chase controller, built from
/// `*chase`. With a half-life the camera eases toward its pose behind and above
/// the target; without one it snaps there. A zero `forward` counts as `-Z`. As
/// `stickshift_camera_set_position`.
///
/// # Safety
/// `engine` must be NULL or a live engine; `chase` NULL or valid.
#[no_mangle]
pub unsafe extern "C" fn stickshift_camera_follow(
    engine: *mut StickShiftEngine,
    chase: *const StickShiftChase,
    target: StickShiftVec3,
    forward: StickShiftVec3,
    dt: f32,
) -> StickShiftResult {
    game_call(engine, "stickshift_camera_follow", |_, mut access| {
        let chase = *input(chase, "chase")?;
        // Built from the raw inputs on every call: `with_up` normalizes once
        // per build and `with_half_life` clamps idempotently, so this is the
        // controller a Rust game builds once.
        ChaseController::new(chase.distance, chase.height)
            .with_up(chase.up.into())
            .with_look_at_height(chase.look_at_height)
            .with_half_life(chase.half_life)
            .follow(access.camera_mut(), target.into(), forward.into(), dt);
        Ok(STICKSHIFT_OK)
    })
}
