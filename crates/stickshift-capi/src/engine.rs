// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! [`StickShiftEngine`]: the engine's loop, run for a host through the C API.
//!
//! An engine owns a core [`Session`] (which owns the [`Loop`]) and renders
//! through the context it is attached to. The host drives it: one
//! `stickshift_engine_tick` per display refresh runs the real [`drive_frame`],
//! through two adapters:
//!
//! - [`CGame`] is a [`Game`] whose hooks call the C game's callbacks, so
//!   [`Loop::init_once`] and [`drive_frame`] call the C game exactly where they
//!   call a Rust game. Its render opens the context's frame, hands it to the
//!   render hook, and submits it.
//! - [`CapiRenderer`] is the [`Renderer`] the loop pushes the camera, the
//!   render settings and drawable sizes through. Each method enters the
//!   context's busy guard by itself, so a tick never holds the guard across a
//!   hook, and a hook's own context calls never see `STICKSHIFT_ERROR_BUSY`.
//!
//! # Scopes
//!
//! Rust's borrows become run-time scopes. During a tick, the tick holds the
//! engine's state mutably (`drive_frame` holds `&mut Loop`), so a call a hook
//! makes reaches engine data only through the [`EngineCtx`] that `drive_frame`
//! passed that hook, which [`CGame`] records on the scope stack while the
//! callback runs. A host-only call made inside any engine call is refused with
//! `STICKSHIFT_ERROR_WRONG_STATE` before it touches the state.
//!
//! A body that holds a hook's `EngineCtx` (an [`Access::Hook`]) must not make a
//! context call through `with_context`: the push rule there would reborrow the
//! same `EngineCtx`.
//!
//! # The push rule
//!
//! Inside a hook, the first context call after the game moved the camera
//! pushes the camera to the context first, as Rust's `EngineCtx::renderer` does
//! ([`before_context_call`]).

use std::cell::{Cell, RefCell, UnsafeCell};
use std::ffi::c_void;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use stickshift_core::driver::{drive_frame, Loop};
use stickshift_core::graphics::{DrawDistance, RenderScale, ShadowDistance};
use stickshift_core::{
    Audio, Clock, EngineCtx, Game, GraphicsSettings, HostEvent, Key, MenuCommand, MouseButton,
    Requests, Session, SettingsStore, View,
};
use stickshift_math::glam::{Mat4, Vec3};
use stickshift_math::Transform;
use stickshift_render_api::{
    FrameRecorder, GpuFrameTime, Light, MaterialParams, MeshData, MeshHandle, OverlayQuad,
    PipelineDescriptor, PipelineHandle, RenderDevice, RenderSettings, ShadowQuality, SunSky,
    TextureData, TextureHandle,
};

use crate::context::{
    effective_scale, frame_of, with_context, Inner, StickShiftContext, MAX_DIMENSION,
};
use crate::core::{call, input, output, read_sized, utf8, write_sized, Error, Outcome};
use crate::frame::{self, Close};
use crate::types::*;

/// How long `STICKSHIFT_FRAME_PACING_WAIT` waits for a free frame slot.
const SLOT_WAIT: Duration = Duration::from_secs(1);

/// How long a tick read-back waits for its frame to finish on the GPU.
const READBACK_WAIT: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------------------
// Threads
// ---------------------------------------------------------------------------

static NEXT_THREAD: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static THREAD: u64 = NEXT_THREAD.fetch_add(1, Ordering::Relaxed);
}

/// A number for the calling thread, unique for the process's life and never 0.
pub(crate) fn current_thread() -> u64 {
    THREAD.with(|id| *id)
}

fn wrong_thread() -> Error {
    Error::new(
        STICKSHIFT_ERROR_WRONG_THREAD,
        "an engine, and a context with an engine attached, may only be used on the thread \
         that created the engine",
    )
}

// ---------------------------------------------------------------------------
// The engine object
// ---------------------------------------------------------------------------

/// An engine: the game loop, attached to one context. See `docs/C_API.md`.
pub struct StickShiftEngine {
    /// The creating thread ([`current_thread`]); any other gets
    /// `STICKSHIFT_ERROR_WRONG_THREAD`.
    thread: u64,
    /// The attached context, which outlives the engine.
    ctx: *mut StickShiftContext,
    /// The game's hooks, copied at create.
    game: StickShiftGame,
    pacing: StickShiftFramePacing,
    hold_until_loaded: bool,
    /// A panic was caught in an engine call: every later call but destroy is
    /// `STICKSHIFT_ERROR_INTERNAL`.
    poisoned: Cell<bool>,
    /// The init hook failed.
    failed: Cell<bool>,
    /// Mirrors of the state's flags, readable inside a tick.
    initialized: Cell<bool>,
    hosted: Cell<bool>,
    holding: Cell<bool>,
    /// Inside an engine call that holds the state (init, resume, tick,
    /// push_events, ...), including every hook and callback it runs.
    in_call: Cell<bool>,
    /// The hooks running now, innermost last.
    scopes: RefCell<Vec<Scope>>,
    counters: Cell<Counters>,
    /// What this tick's render did.
    frame_out: Cell<FrameOut>,
    /// A test asked for a panic inside the next update ([`testing`]).
    panic_in_update: Cell<bool>,
    /// The session, the game adapter and the renderer adapter. Borrowed
    /// mutably by the engine call in progress (see the module docs).
    state: UnsafeCell<State>,
}

struct State {
    session: Session,
    game: CGame,
    renderer: CapiRenderer,
}

#[derive(Debug, Clone, Copy, Default)]
struct Counters {
    frames: u64,
    updates: u64,
    rendered: u64,
    discarded: u64,
}

#[derive(Debug, Clone, Copy, Default)]
struct FrameOut {
    rendered: bool,
    submitted: bool,
    readback_ready: bool,
    frame: u64,
}

/// Which game hook a scope is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hook {
    Init,
    Update,
    Render,
    Settings,
}

/// A hook running now, with the `EngineCtx` `drive_frame` passed it.
#[derive(Clone, Copy)]
enum Scope {
    /// The pointer is a reborrow of the `&mut EngineCtx` [`CGame`] received,
    /// which it does not use while the hook runs. The kind is for the calls a
    /// hook kind allows (world and scene changes are not allowed in render);
    /// the engine's own calls allow every hook.
    Hook(*mut EngineCtx<'static>, #[allow(dead_code)] Hook),
}

/// How a game call reaches the engine's data.
enum Access<'a> {
    /// Between engine calls: the state itself.
    Idle(&'a mut State),
    /// Inside a hook: its `EngineCtx`.
    Hook(&'a mut EngineCtx<'static>),
}

impl StickShiftEngine {
    fn bump(&self, f: impl FnOnce(&mut Counters)) {
        let mut counters = self.counters.get();
        f(&mut counters);
        self.counters.set(counters);
    }

    /// Run `f` (a C hook) with `hook` on the scope stack, unless the engine is
    /// poisoned, in which case no hook runs and the result is `None`.
    fn run_hook<R>(
        &self,
        ctx: &mut EngineCtx<'_>,
        hook: Hook,
        f: impl FnOnce(*mut c_void, *mut StickShiftEngine) -> R,
    ) -> Option<R> {
        if self.poisoned.get() {
            return None;
        }
        let pointer = std::ptr::from_mut(ctx).cast::<EngineCtx<'static>>();
        let depth = {
            let mut scopes = self.scopes.borrow_mut();
            scopes.push(Scope::Hook(pointer, hook));
            scopes.len()
        };
        let _pop = PopScope {
            scopes: &self.scopes,
            depth,
        };
        Some(f(self.game.user, std::ptr::from_ref(self).cast_mut()))
    }

    /// The init hook's outcome, after `init_once` ran (possibly earlier).
    fn finish_init(&self, state: &State, ran_now: bool) -> Outcome {
        self.initialized.set(state.session.lp.initialized);
        if self.poisoned.get() {
            return Err(poisoned());
        }
        if ran_now && state.game.init_result < 0 {
            self.failed.set(true);
            return Err(Error::new(
                STICKSHIFT_ERROR_ABORTED,
                format!(
                    "the game's init hook returned {}: the engine is failed; destroy it",
                    state.game.init_result
                ),
            ));
        }
        Ok(STICKSHIFT_OK)
    }
}

/// Pops a scope on every exit from a hook, unwinding too.
struct PopScope<'a> {
    scopes: &'a RefCell<Vec<Scope>>,
    depth: usize,
}

impl Drop for PopScope<'_> {
    fn drop(&mut self) {
        let mut scopes = self.scopes.borrow_mut();
        debug_assert_eq!(scopes.len(), self.depth, "a hook's scope was not its own");
        scopes.pop();
    }
}

/// Marks an engine call in progress, and clears it on every exit.
struct InCall<'a>(&'a StickShiftEngine);

impl<'a> InCall<'a> {
    fn enter(engine: &'a StickShiftEngine) -> Self {
        engine.in_call.set(true);
        Self(engine)
    }
}

impl Drop for InCall<'_> {
    fn drop(&mut self) {
        debug_assert!(
            self.0.scopes.borrow().is_empty(),
            "an engine call returned inside a hook's scope"
        );
        self.0.in_call.set(false);
    }
}

fn poisoned() -> Error {
    Error::new(
        STICKSHIFT_ERROR_INTERNAL,
        "the engine is poisoned: a panic (a bug in Stick Shift Engine) was caught in an \
         earlier engine call; destroy it",
    )
}

/// Run an engine function's body on the engine's thread: a poisoned engine is
/// `STICKSHIFT_ERROR_INTERNAL`, and a panic in the body poisons it.
///
/// # Safety
/// `engine` must be NULL or a live engine.
unsafe fn engine_call(
    engine: *mut StickShiftEngine,
    body: impl FnOnce(&StickShiftEngine) -> Outcome,
) -> StickShiftResult {
    call(|| {
        let engine = input(engine.cast_const(), "engine")?;
        if engine.thread != current_thread() {
            return Err(wrong_thread());
        }
        if engine.poisoned.get() {
            return Err(poisoned());
        }
        match catch_unwind(AssertUnwindSafe(|| body(engine))) {
            Ok(outcome) => outcome,
            Err(payload) => {
                engine.poisoned.set(true);
                resume_unwind(payload)
            }
        }
    })
}

/// A host-only (`[H]`) engine function: refused inside any engine call, else
/// `body` gets the state for the call.
///
/// # Safety
/// `engine` must be NULL or a live engine.
unsafe fn host_call(
    engine: *mut StickShiftEngine,
    what: &str,
    body: impl FnOnce(&StickShiftEngine, &mut State) -> Outcome,
) -> StickShiftResult {
    engine_call(engine, |engine| {
        if engine.in_call.get() {
            return Err(Error::wrong_state(format!(
                "{what} cannot be called inside an engine call (from a hook or a callback)"
            )));
        }
        let _in_call = InCall::enter(engine);
        // SAFETY: no other engine call is in progress, so nothing else borrows
        // the state; the hooks this call runs reach it only through their scope.
        let state = &mut *engine.state.get();
        body(engine, state)
    })
}

/// A game (`[G][R]`) engine function: between engine calls, or inside any hook.
///
/// # Safety
/// `engine` must be NULL or a live engine.
unsafe fn game_call(
    engine: *mut StickShiftEngine,
    what: &str,
    body: impl FnOnce(&StickShiftEngine, Access<'_>) -> Outcome,
) -> StickShiftResult {
    engine_call(engine, |engine| {
        let top = engine.scopes.borrow().last().copied();
        match top {
            // SAFETY: the hook's `EngineCtx` is live until it returns, and its
            // owner does not use it meanwhile; nothing else reborrows it while
            // `body` runs, which makes no context call.
            Some(Scope::Hook(ctx, _)) => body(engine, Access::Hook(&mut *ctx)),
            None if !engine.in_call.get() => {
                let _in_call = InCall::enter(engine);
                // SAFETY: no engine call is in progress.
                body(engine, Access::Idle(&mut *engine.state.get()))
            }
            None => Err(Error::wrong_state(format!(
                "{what} can be called inside a game hook or between engine calls, not from a \
                 callback the engine delivers outside a hook"
            ))),
        }
    })
}

/// The checks every context function makes when an engine is attached: the
/// engine's thread only, and, inside a hook, the push rule.
///
/// # Safety
/// `ctx` must be a live context.
pub(crate) unsafe fn before_context_call(ctx: &StickShiftContext) -> Result<(), Error> {
    let engine = ctx.engine.load(Ordering::Acquire);
    if engine.is_null() {
        return Ok(());
    }
    if ctx.engine_thread.load(Ordering::Acquire) != current_thread() {
        return Err(wrong_thread());
    }
    // SAFETY: an attached engine is live, and this is its thread.
    let engine = &*engine;
    let top = engine.scopes.borrow().last().copied();
    if let Some(Scope::Hook(hook_ctx, _)) = top {
        // `EngineCtx::renderer` pushes the camera if the game moved it since
        // the last push, through `CapiRenderer`, which takes the busy guard
        // itself; this call takes it after.
        // SAFETY: as in `game_call`; the returned borrow ends here.
        let _ = (*hook_ctx).renderer();
    }
    Ok(())
}

/// Whether `ctx` has an engine attached that is in a call on this thread.
pub(crate) fn in_engine_call(ctx: &StickShiftContext) -> bool {
    let engine = ctx.engine.load(Ordering::Acquire);
    // SAFETY: an attached engine is live; only its own thread reads it.
    !engine.is_null()
        && ctx.engine_thread.load(Ordering::Acquire) == current_thread()
        && unsafe { (*engine).in_call.get() }
}

// ---------------------------------------------------------------------------
// The game adapter
// ---------------------------------------------------------------------------

/// The C game as a [`Game`], so the loop calls it where it calls a Rust game.
struct CGame {
    engine: *const StickShiftEngine,
    /// What the init hook returned.
    init_result: StickShiftResult,
    /// This tick's read-back: where, and how many bytes (already checked).
    readback: Option<(*mut u8, usize)>,
}

impl CGame {
    fn engine(&self) -> &StickShiftEngine {
        // SAFETY: set at create to the engine that owns this adapter.
        unsafe { &*self.engine }
    }
}

impl Game for CGame {
    fn init(&mut self, ctx: &mut EngineCtx<'_>) {
        let engine = self.engine();
        let init = engine.game.init.expect("checked at create");
        // SAFETY: the host's hook, with its user pointer and this engine.
        let result = engine.run_hook(ctx, Hook::Init, |user, e| unsafe { init(user, e) });
        self.init_result = result.unwrap_or(STICKSHIFT_ERROR_INTERNAL);
    }

    fn update(&mut self, ctx: &mut EngineCtx<'_>, dt: f32) {
        let engine = self.engine();
        assert!(
            !engine.panic_in_update.take(),
            "a panic a test asked for, inside the engine's update step"
        );
        engine.bump(|c| c.updates += 1);
        let update = engine.game.update.expect("checked at create");
        // SAFETY: as in `init`.
        engine.run_hook(ctx, Hook::Update, |user, e| unsafe { update(user, e, dt) });
    }

    fn render(&mut self, ctx: &mut EngineCtx<'_>) {
        let readback = self.readback.take();
        let out = self.render_frame(ctx, readback);
        self.engine().frame_out.set(out);
    }

    fn graphics_settings_changed(&mut self, ctx: &mut EngineCtx<'_>, settings: &GraphicsSettings) {
        let engine = self.engine();
        if let Some(hook) = engine.game.graphics_settings_changed {
            let settings = graphics_settings_to_c(settings);
            // SAFETY: as in `init`; `settings` lives until the hook returns.
            engine.run_hook(ctx, Hook::Settings, |user, e| unsafe {
                hook(user, e, &settings)
            });
        }
    }
}

impl CGame {
    /// The render step, inside `drive_frame`'s render: wait per the pacing,
    /// open the context's frame, run the render hook with it, then submit it
    /// (or discard it, with no free slot), and copy a tick read-back.
    fn render_frame(
        &self,
        ectx: &mut EngineCtx<'_>,
        readback: Option<(*mut u8, usize)>,
    ) -> FrameOut {
        let engine = self.engine();
        let mut out = FrameOut::default();
        if engine.poisoned.get() {
            return out;
        }
        let wait = engine.pacing == STICKSHIFT_FRAME_PACING_WAIT;
        // SAFETY: the attached context outlives the engine; no scope is active,
        // so this is the engine's own call.
        let opened = unsafe {
            with_context(engine.ctx, |inner| {
                if wait && !frame::has_free_slot(inner) {
                    inner.backend.wait_for_slot(SLOT_WAIT);
                }
                let slot = frame::has_free_slot(inner);
                frame::open(inner);
                if let (true, Some((dst, len))) = (slot, readback) {
                    frame::request_engine_readback(inner, dst, len);
                }
                Ok(slot)
            })
        };
        let Ok(slot) = opened else {
            debug_assert!(false, "the engine could not open its context's frame");
            return out;
        };

        out.rendered = true;
        engine.bump(|c| c.rendered += 1);
        let render = engine.game.render.expect("checked at create");
        // SAFETY: as above.
        let frame = frame_of(unsafe { &*engine.ctx });
        // SAFETY: the host's hook; the frame is open until it returns.
        engine.run_hook(ectx, Hook::Render, |user, e| unsafe {
            render(user, e, frame)
        });

        let close = if slot { Close::Engine } else { Close::Discard };
        // SAFETY: as above.
        let finished = unsafe { with_context(engine.ctx, |inner| Ok(frame::finish(inner, close))) };
        let Ok(finished) = finished else {
            debug_assert!(false, "the engine could not close its context's frame");
            return out;
        };
        out.submitted = finished.code == STICKSHIFT_OK;
        out.frame = finished.number;
        if !out.submitted {
            engine.bump(|c| c.discarded += 1);
        }
        if let (Some((dst, len)), Some(copy)) = (readback, finished.readback) {
            // SAFETY: as above.
            let done = unsafe {
                with_context(engine.ctx, |inner| {
                    Ok(inner.backend.wait_for_frame(finished.number, READBACK_WAIT))
                })
            };
            if done.unwrap_or(false) && copy.is_complete() {
                // SAFETY: the tick's caller lent `len` writable bytes until the
                // tick returns.
                let dst = unsafe { std::slice::from_raw_parts_mut(dst, len) };
                out.readback_ready = copy.copy_to(dst);
            }
        }
        out
    }
}

/// The C form of `settings`.
fn graphics_settings_to_c(settings: &GraphicsSettings) -> StickShiftGraphicsSettings {
    fn index<T: PartialEq>(all: &[T], value: T) -> u32 {
        all.iter().position(|v| *v == value).unwrap_or(0) as u32
    }
    StickShiftGraphicsSettings {
        shadows: match settings.shadows {
            ShadowQuality::Off => STICKSHIFT_SHADOW_QUALITY_OFF,
            ShadowQuality::Low => STICKSHIFT_SHADOW_QUALITY_LOW,
            ShadowQuality::High => STICKSHIFT_SHADOW_QUALITY_HIGH,
        },
        shadow_distance: index(&ShadowDistance::ALL, settings.shadow_distance),
        draw_distance: index(&DrawDistance::ALL, settings.draw_distance),
        render_scale: index(&RenderScale::ALL, settings.render_scale),
        reserved: [0; 4],
    }
}

// ---------------------------------------------------------------------------
// The renderer adapter
// ---------------------------------------------------------------------------

/// The [`Renderer`](stickshift_core::Renderer) the loop pushes the camera,
/// render settings and drawable sizes through, onto the attached context.
///
/// Games draw through the C frame, so the loop never calls the resource,
/// pipeline or draw methods; they are inert, and count a violation.
struct CapiRenderer {
    ctx: *const StickShiftContext,
    /// The view's safe-area insets in points, converted to drawable pixels on
    /// every resize.
    safe_area_points: [f32; 4],
    violations: u32,
}

impl CapiRenderer {
    /// Run `f` on the context behind its busy guard.
    fn inner<R>(&self, f: impl FnOnce(&mut Inner) -> R) -> Option<R> {
        // SAFETY: the context outlives its engine: stickshift_context_destroy
        // refuses while one is attached.
        let ctx = unsafe { &*self.ctx };
        match ctx.enter() {
            Ok(mut guard) => Some(f(guard.inner())),
            Err(_) => {
                debug_assert!(false, "the engine's context was busy");
                None
            }
        }
    }

    fn inert(&mut self, what: &str) {
        self.violations += 1;
        debug_assert!(false, "the engine loop called the C renderer's {what}");
    }

    /// The view's insets changed: store them, and convert them at the
    /// drawable's current scale.
    fn set_safe_area_points(&mut self, points: [f32; 4]) {
        self.safe_area_points = points;
        self.inner(|inner| inner.safe_area = points.map(|p| p * inner.scale));
    }
}

impl RenderDevice for CapiRenderer {
    fn create_mesh(&mut self, _: &MeshData<'_>) -> MeshHandle {
        self.inert("create_mesh");
        MeshHandle(0)
    }

    fn destroy_mesh(&mut self, _: MeshHandle) {
        self.inert("destroy_mesh");
    }

    fn create_texture(&mut self, _: &TextureData<'_>) -> TextureHandle {
        self.inert("create_texture");
        TextureHandle(0)
    }

    fn destroy_texture(&mut self, _: TextureHandle) {
        self.inert("destroy_texture");
    }

    fn create_pipeline(&mut self, _: &PipelineDescriptor) -> PipelineHandle {
        self.inert("create_pipeline");
        PipelineHandle(0)
    }

    fn destroy_pipeline(&mut self, _: PipelineHandle) {
        self.inert("destroy_pipeline");
    }

    fn surface_size(&self) -> (u32, u32) {
        self.inner(|inner| inner.surface_size()).unwrap_or((0, 0))
    }

    fn safe_area_insets(&self) -> [f32; 4] {
        self.inner(|inner| inner.safe_area).unwrap_or_default()
    }

    fn resize_surface(&mut self, width: u32, height: u32, pixels_per_point: f32) {
        let points = self.safe_area_points;
        self.inner(|inner| {
            if inner.backend.can_resize() {
                inner.resize(
                    width.clamp(1, MAX_DIMENSION),
                    height.clamp(1, MAX_DIMENSION),
                    pixels_per_point,
                );
            } else {
                // A Metal offscreen target keeps its size; the UI scale still
                // follows the view.
                inner.scale = effective_scale(pixels_per_point);
            }
            inner.safe_area = points.map(|p| p * inner.scale);
        });
    }

    fn surface_scale(&self) -> f32 {
        self.inner(|inner| inner.scale).unwrap_or(1.0)
    }

    fn last_gpu_frame_time(&self) -> Option<GpuFrameTime> {
        self.inner(|inner| {
            let frame = inner.backend.completed_frames();
            (frame > 0)
                .then(|| inner.backend.gpu_frame_time(frame))
                .flatten()
                .map(|duration| GpuFrameTime { frame, duration })
        })
        .flatten()
    }

    fn set_render_settings(&mut self, settings: &RenderSettings) {
        self.inner(|inner| inner.backend.set_settings(settings));
    }
}

impl FrameRecorder for CapiRenderer {
    fn begin_frame(&mut self) {
        self.inert("begin_frame");
    }

    fn set_view_projection(&mut self, view_proj: Mat4) {
        self.inner(|inner| inner.camera.0 = view_proj);
    }

    fn set_camera_position(&mut self, position: Vec3) {
        self.inner(|inner| inner.camera.1 = position);
    }

    fn set_sun_sky(&mut self, sun: &SunSky) {
        self.inner(|inner| inner.sun = *sun);
    }

    fn add_light(&mut self, _: &Light) {
        self.inert("add_light");
    }

    fn set_pipeline(&mut self, _: PipelineHandle) {
        self.inert("set_pipeline");
    }

    fn bind_texture(&mut self, _: TextureHandle) {
        self.inert("bind_texture");
    }

    fn draw_mesh(&mut self, _: MeshHandle, _: &Transform, _: &MaterialParams) {
        self.inert("draw_mesh");
    }

    fn draw_mesh_matrix(&mut self, _: MeshHandle, _: Mat4, _: &MaterialParams) {
        self.inert("draw_mesh_matrix");
    }

    fn draw_overlay_quad(&mut self, _: &OverlayQuad) {
        self.inert("draw_overlay_quad");
    }

    fn submit(&mut self) {
        self.inert("submit");
    }
}

// ---------------------------------------------------------------------------
// Host events
// ---------------------------------------------------------------------------

fn key(code: u32) -> Option<Key> {
    Some(match code {
        STICKSHIFT_KEY_W => Key::W,
        STICKSHIFT_KEY_A => Key::A,
        STICKSHIFT_KEY_S => Key::S,
        STICKSHIFT_KEY_D => Key::D,
        STICKSHIFT_KEY_Q => Key::Q,
        STICKSHIFT_KEY_E => Key::E,
        STICKSHIFT_KEY_LEFT => Key::Left,
        STICKSHIFT_KEY_RIGHT => Key::Right,
        STICKSHIFT_KEY_UP => Key::Up,
        STICKSHIFT_KEY_DOWN => Key::Down,
        STICKSHIFT_KEY_SPACE => Key::Space,
        STICKSHIFT_KEY_ESCAPE => Key::Escape,
        _ => return None,
    })
}

fn button(code: u32) -> Option<MouseButton> {
    Some(match code {
        STICKSHIFT_MOUSE_LEFT => MouseButton::Left,
        STICKSHIFT_MOUSE_RIGHT => MouseButton::Right,
        STICKSHIFT_MOUSE_MIDDLE => MouseButton::Middle,
        _ => return None,
    })
}

fn check_scale_factor(scale_factor: f64) -> Result<(), String> {
    if scale_factor.is_finite() && scale_factor > 0.0 {
        Ok(())
    } else {
        Err(format!(
            "scale_factor {scale_factor} must be finite and positive"
        ))
    }
}

fn check_points(insets: &[f32; 4]) -> Result<(), String> {
    if insets.iter().all(|v| v.is_finite() && *v >= 0.0) {
        Ok(())
    } else {
        Err("safe-area insets must be finite and >= 0".into())
    }
}

/// A C host event as the session's, or why it is invalid.
fn decode(event: &StickShiftHostEvent) -> Result<HostEvent, String> {
    if event.reserved != 0 {
        return Err("reserved must be 0".into());
    }
    let allowed = if event.kind == STICKSHIFT_HOST_KEY_DOWN {
        STICKSHIFT_HOST_EVENT_REPEAT
    } else {
        0
    };
    if event.flags & !allowed != 0 {
        return Err(format!(
            "flags 0x{:x} has bits this kind does not define",
            event.flags
        ));
    }
    let key = || key(event.code).ok_or_else(|| format!("{} is not a STICKSHIFT_KEY_*", event.code));
    let button =
        || button(event.code).ok_or_else(|| format!("{} is not a STICKSHIFT_MOUSE_*", event.code));
    let view = || {
        check_scale_factor(event.scale_factor)?;
        Ok::<_, String>(View::new(event.width, event.height, event.scale_factor))
    };
    Ok(match event.kind {
        STICKSHIFT_HOST_KEY_DOWN => HostEvent::KeyDown {
            key: key()?,
            repeat: event.flags & STICKSHIFT_HOST_EVENT_REPEAT != 0,
        },
        STICKSHIFT_HOST_KEY_UP => HostEvent::KeyUp(key()?),
        STICKSHIFT_HOST_MOUSE_DOWN => HostEvent::MouseDown(button()?),
        STICKSHIFT_HOST_MOUSE_UP => HostEvent::MouseUp(button()?),
        STICKSHIFT_HOST_CURSOR => {
            if !(event.x.is_finite() && event.y.is_finite()) {
                return Err("the cursor position must be finite".into());
            }
            HostEvent::Cursor(event.x, event.y)
        }
        STICKSHIFT_HOST_FOCUS_LOST => HostEvent::FocusLost,
        STICKSHIFT_HOST_VIEW_RESIZED => HostEvent::Resized(view()?),
        STICKSHIFT_HOST_SCALE_CHANGED => HostEvent::ScaleChanged(view()?),
        STICKSHIFT_HOST_SAFE_AREA => {
            check_points(&event.insets)?;
            HostEvent::SafeArea(event.insets)
        }
        STICKSHIFT_HOST_MENU => HostEvent::Menu(
            MenuCommand::from_tag(event.code as isize)
                .ok_or_else(|| format!("{} is not a Graphics menu tag", event.code))?,
        ),
        STICKSHIFT_HOST_CLOSE => HostEvent::Close,
        other => return Err(format!("{other} is not a STICKSHIFT_HOST_* kind")),
    })
}

// ---------------------------------------------------------------------------
// The functions
// ---------------------------------------------------------------------------

/// Create an engine on `ctx`, which must outlive it. The calling thread
/// becomes the engine's thread. `game` is copied (its `user` is borrowed until
/// `release`). No hook runs. `STICKSHIFT_ERROR_WRONG_STATE` if `ctx` already has
/// an engine or a frame open. On failure nothing is created and `release` is
/// not called.
///
/// # Safety
/// `ctx` must be NULL or a live context; `desc`, `game` and `out` NULL or
/// valid for their declared sizes.
#[no_mangle]
pub unsafe extern "C" fn stickshift_engine_create(
    ctx: *mut StickShiftContext,
    desc: *const StickShiftEngineDesc,
    game: *const StickShiftGame,
    out: *mut *mut StickShiftEngine,
) -> StickShiftResult {
    call(|| {
        let out = output(out, "out")?;
        *out = std::ptr::null_mut();
        let context = input(ctx.cast_const(), "ctx")?;
        if desc.is_null() {
            return Err(Error::null("desc"));
        }
        let api_version = desc.cast::<u32>().read_unaligned();
        if api_version != STICKSHIFT_API_VERSION {
            return Err(Error::new(
                STICKSHIFT_ERROR_UNSUPPORTED,
                format!(
                    "the header is API version {api_version} but the library is \
                     {STICKSHIFT_API_VERSION}"
                ),
            ));
        }
        let desc = read_sized(desc, 4, "desc")?;
        let game = read_sized(game, 0, "game")?;
        if !matches!(
            desc.audio,
            STICKSHIFT_AUDIO_SILENT | STICKSHIFT_AUDIO_DEVICE
        ) {
            return Err(Error::invalid_argument(format!(
                "{} is not a STICKSHIFT_AUDIO_* value",
                desc.audio
            )));
        }
        if !matches!(
            desc.pacing,
            STICKSHIFT_FRAME_PACING_WAIT | STICKSHIFT_FRAME_PACING_SKIP
        ) {
            return Err(Error::invalid_argument(format!(
                "{} is not a STICKSHIFT_FRAME_PACING_* value",
                desc.pacing
            )));
        }
        let known = STICKSHIFT_ENGINE_PERSIST_SETTINGS
            | STICKSHIFT_ENGINE_QUIT_ON_ESCAPE
            | STICKSHIFT_ENGINE_HOLD_UNTIL_LOADED;
        if desc.flags & !known != 0 {
            return Err(Error::invalid_argument(format!(
                "flags 0x{:x} has bits that are not STICKSHIFT_ENGINE_*",
                desc.flags
            )));
        }
        if desc.reserved != 0 || game.reserved != 0 {
            return Err(Error::invalid_argument("reserved fields must be 0"));
        }
        let persist = desc.flags & STICKSHIFT_ENGINE_PERSIST_SETTINGS != 0;
        let suite = if desc.settings_suite.is_null() {
            None
        } else if persist {
            Some(utf8(desc.settings_suite, "desc.settings_suite")?.to_owned())
        } else {
            return Err(Error::invalid_argument(
                "settings_suite needs STICKSHIFT_ENGINE_PERSIST_SETTINGS",
            ));
        };
        if game.init.is_none() || game.update.is_none() || game.render.is_none() {
            return Err(Error::invalid_argument(
                "game.init, game.update and game.render are required",
            ));
        }
        if !context.engine.load(Ordering::Acquire).is_null() {
            return Err(Error::wrong_state("this context already has an engine"));
        }
        let frame_open = with_context(ctx, |inner| Ok(inner.frame_open))?;
        if frame_open {
            return Err(Error::wrong_state(
                "a frame is open on this context: submit or discard it first",
            ));
        }

        let mut lp = Loop::new();
        if desc.audio == STICKSHIFT_AUDIO_DEVICE {
            lp.audio = Audio::with_output_device();
        }
        let store =
            persist.then(|| suite.map_or_else(SettingsStore::standard, SettingsStore::suite));
        let session = Session::new(
            lp,
            store,
            desc.flags & STICKSHIFT_ENGINE_QUIT_ON_ESCAPE != 0,
        );
        let engine = Box::into_raw(Box::new(StickShiftEngine {
            thread: current_thread(),
            ctx,
            game,
            pacing: desc.pacing,
            hold_until_loaded: desc.flags & STICKSHIFT_ENGINE_HOLD_UNTIL_LOADED != 0,
            poisoned: Cell::new(false),
            failed: Cell::new(false),
            initialized: Cell::new(false),
            hosted: Cell::new(false),
            holding: Cell::new(false),
            in_call: Cell::new(false),
            scopes: RefCell::new(Vec::new()),
            counters: Cell::new(Counters::default()),
            frame_out: Cell::new(FrameOut::default()),
            panic_in_update: Cell::new(false),
            state: UnsafeCell::new(State {
                session,
                game: CGame {
                    engine: std::ptr::null(),
                    init_result: STICKSHIFT_OK,
                    readback: None,
                },
                renderer: CapiRenderer {
                    ctx,
                    safe_area_points: [0.0; 4],
                    violations: 0,
                },
            }),
        }));
        // SAFETY: just allocated, not shared yet.
        (*(*engine).state.get()).game.engine = engine;
        if context
            .engine
            .compare_exchange(
                std::ptr::null_mut(),
                engine,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            drop(Box::from_raw(engine));
            return Err(Error::wrong_state("this context already has an engine"));
        }
        context
            .engine_thread
            .store(current_thread(), Ordering::Release);
        *out = engine;
        Ok(STICKSHIFT_OK)
    })
}

/// Destroy an engine: call the game's `release` exactly once, detach the
/// context, and drop the loop (which stops the audio device). NULL is a no-op.
/// `STICKSHIFT_ERROR_WRONG_STATE` from inside a hook or callback,
/// `STICKSHIFT_ERROR_WRONG_THREAD` from another thread. Works on a poisoned
/// engine.
///
/// # Safety
/// `engine` must be NULL or a live engine; it is invalid after this returns
/// `STICKSHIFT_OK`.
#[no_mangle]
pub unsafe extern "C" fn stickshift_engine_destroy(
    engine: *mut StickShiftEngine,
) -> StickShiftResult {
    call(|| {
        let Some(e) = engine.as_ref() else {
            return Ok(STICKSHIFT_OK);
        };
        if e.thread != current_thread() {
            return Err(wrong_thread());
        }
        if e.in_call.get() {
            return Err(Error::wrong_state(
                "stickshift_engine_destroy cannot be called inside an engine call (from a hook \
                 or a callback)",
            ));
        }
        {
            // Engine calls the release hook makes are refused.
            let _in_call = InCall::enter(e);
            if let Some(release) = e.game.release {
                release(e.game.user);
            }
        }
        let engine = Box::from_raw(engine);
        let context = &*engine.ctx;
        context.engine_thread.store(0, Ordering::Release);
        context
            .engine
            .store(std::ptr::null_mut(), Ordering::Release);
        drop(engine);
        Ok(STICKSHIFT_OK)
    })
}

/// The context the engine is attached to, borrowed.
///
/// # Safety
/// `engine` must be NULL or a live engine; `out` NULL or valid.
#[no_mangle]
pub unsafe extern "C" fn stickshift_engine_get_context(
    engine: *mut StickShiftEngine,
    out: *mut *mut StickShiftContext,
) -> StickShiftResult {
    engine_call(engine, |e| {
        *output(out, "out")? = e.ctx;
        Ok(STICKSHIFT_OK)
    })
}

/// Run the game's init once, for an engine with no view (tools, tests): a
/// queued settings request becomes current, then the init hook runs, then the
/// streaming reach and the render settings are applied. Idempotent. If the
/// hook fails, `STICKSHIFT_ERROR_ABORTED`, and the engine is failed.
///
/// # Safety
/// `engine` must be NULL or a live engine.
#[no_mangle]
pub unsafe extern "C" fn stickshift_engine_init(engine: *mut StickShiftEngine) -> StickShiftResult {
    host_call(engine, "stickshift_engine_init", |e, state| {
        if e.failed.get() {
            return Err(failed());
        }
        if e.initialized.get() {
            return Ok(STICKSHIFT_OK);
        }
        state
            .session
            .lp
            .init_once(&mut state.game, &mut state.renderer);
        e.finish_init(state, true)
    })
}

fn failed() -> Error {
    Error::wrong_state("the game's init hook failed: the engine is failed; destroy it")
}

/// The view appeared (or reappeared): mark the engine hosted, size the drawable
/// for the view and the render scale in effect, run the game's init once,
/// size the drawable again, and record the safe area. The first call also sets
/// the camera's aspect to the view's. `*out_requests` (optional) gets
/// `STICKSHIFT_REQUEST_MENU_SYNC`. A Metal offscreen context keeps its size.
///
/// # Safety
/// `engine` must be NULL or a live engine; `view` NULL or valid for its
/// declared size; `out_requests` NULL or valid.
#[no_mangle]
pub unsafe extern "C" fn stickshift_engine_resume(
    engine: *mut StickShiftEngine,
    view: *const StickShiftViewDesc,
    out_requests: *mut u32,
) -> StickShiftResult {
    host_call(engine, "stickshift_engine_resume", |e, state| {
        if let Some(out) = out_requests.as_mut() {
            *out = 0;
        }
        let view = read_sized(view, 0, "view")?;
        if view.reserved != 0 {
            return Err(Error::invalid_argument("view.reserved must be 0"));
        }
        check_scale_factor(view.scale_factor).map_err(Error::invalid_argument)?;
        check_points(&view.safe_area).map_err(Error::invalid_argument)?;
        if e.failed.get() {
            return Err(failed());
        }
        let ran_init = !e.initialized.get();
        let State {
            session,
            game,
            renderer,
        } = state;
        let _ = session.event(HostEvent::SafeArea(view.safe_area), game, renderer);
        renderer.safe_area_points = view.safe_area;
        let mut requests = session.resume(
            View::new(view.width, view.height, view.scale_factor),
            game,
            renderer,
        );
        e.hosted.set(true);
        if session.lp.exit_requested {
            requests |= Requests::EXIT;
        }
        if let Some(out) = out_requests.as_mut() {
            *out = requests.bits();
        }
        e.finish_init(state, ran_init)
    })
}

/// One display frame. See `docs/C_API.md`: housekeeping, the load hold, then
/// the session's frame (hosted) or the loop's (unhosted), whose render step
/// opens the context's frame, runs the render hook and submits it. `out` is
/// optional.
///
/// # Safety
/// `engine` must be NULL or a live engine; `tick` NULL or valid for its
/// declared size; `tick->readback` NULL or writable for `readback_len` bytes;
/// `out` NULL or valid for its declared size.
#[no_mangle]
pub unsafe extern "C" fn stickshift_engine_tick(
    engine: *mut StickShiftEngine,
    tick: *const StickShiftTickDesc,
    out: *mut StickShiftTickResult,
) -> StickShiftResult {
    host_call(engine, "stickshift_engine_tick", |e, state| {
        let desc = read_sized(tick, 0, "tick")?;
        if !out.is_null() {
            // Check it now, so a tick that runs can always report.
            write_sized(out, StickShiftTickResult::default(), "out")?;
        }
        let clock = match desc.clock {
            STICKSHIFT_CLOCK_FIXED_STEP => Clock::FixedStep,
            STICKSHIFT_CLOCK_ELAPSED => Clock::Elapsed(Duration::from_nanos(desc.nanoseconds)),
            STICKSHIFT_CLOCK_TIMESTAMP => Clock::Timestamp(desc.nanoseconds),
            STICKSHIFT_CLOCK_ENGINE => Clock::Engine,
            other => {
                return Err(Error::invalid_argument(format!(
                    "{other} is not a STICKSHIFT_CLOCK_* value"
                )))
            }
        };
        if e.failed.get() {
            return Err(failed());
        }
        if !e.initialized.get() {
            return Err(Error::wrong_state(
                "the game has not been initialized: call stickshift_engine_init or \
                 stickshift_engine_resume first",
            ));
        }
        // Housekeeping, as a poll does: finished loads, frame completions,
        // read-back copies. Callbacks run as this returns.
        let (loads, frame_open, offscreen, need) = with_context(e.ctx, |inner| {
            inner.pump();
            let (width, height) = inner.surface_size();
            Ok((
                inner.pending().0,
                inner.frame_open,
                inner.backend.is_offscreen(),
                width as usize * height as usize * 4,
            ))
        })?;
        if frame_open {
            return Err(Error::wrong_state(
                "a frame is open on the engine's context: submit or discard it before ticking",
            ));
        }
        let readback = if desc.readback.is_null() {
            None
        } else {
            if !offscreen || e.pacing != STICKSHIFT_FRAME_PACING_WAIT {
                return Err(Error::new(
                    STICKSHIFT_ERROR_UNSUPPORTED,
                    "a tick read-back needs an offscreen context and STICKSHIFT_FRAME_PACING_WAIT",
                ));
            }
            if desc.readback_len < need {
                return Err(Error::invalid_argument(format!(
                    "readback holds {} bytes; the frame needs {need}",
                    desc.readback_len
                )));
            }
            Some((desc.readback.cast::<u8>(), need))
        };

        let State {
            session,
            game,
            renderer,
        } = state;
        let mut result = StickShiftTickResult {
            size: 0,
            ..StickShiftTickResult::default()
        };
        let code = if e.hold_until_loaded && loads > 0 {
            // Nothing runs and no time is banked; the first tick after the hold
            // banks one step.
            e.holding.set(true);
            session.reset_clock();
            result.held = 1;
            STICKSHIFT_NOT_READY
        } else {
            e.holding.set(false);
            game.readback = readback;
            let (steps, requests) = if e.hosted.get() {
                session.frame(clock, game, renderer)
            } else {
                let elapsed = session.read_clock(clock);
                (
                    drive_frame(&mut session.lp, game, renderer, elapsed),
                    Requests::NONE,
                )
            };
            game.readback = None;
            e.bump(|c| c.frames += 1);
            let frame = e.frame_out.take();
            if e.poisoned.get() {
                return Err(Error::new(
                    STICKSHIFT_ERROR_INTERNAL,
                    "a panic (a bug in Stick Shift Engine) was caught in a call the game made \
                     during this tick: the engine is poisoned; destroy it",
                ));
            }
            result.steps = steps;
            result.rendered = u8::from(frame.rendered);
            result.submitted = u8::from(frame.submitted);
            result.readback_ready = u8::from(frame.readback_ready);
            result.frame = frame.frame;
            result.requests = requests.bits();
            STICKSHIFT_OK
        };
        if session.lp.exit_requested {
            result.requests |= STICKSHIFT_REQUEST_EXIT;
        }
        result.graphics_revision = session.lp.graphics.revision();
        if !out.is_null() {
            write_sized(out, result, "out")?;
        }
        Ok(code)
    })
}

/// Apply `count` host events in array order, `stride` bytes apart (at least
/// `sizeof(StickShiftHostEvent)`, 64). Every event is checked first, and none
/// is applied if one is invalid. A menu event may run the settings hook inside
/// this call. `*out_requests` (optional) gets the `STICKSHIFT_REQUEST_*` bits
/// the events raised.
///
/// # Safety
/// `engine` must be NULL or a live engine; `events` NULL (with `count` 0) or
/// valid for `count` events `stride` bytes apart; `out_requests` NULL or valid.
#[no_mangle]
pub unsafe extern "C" fn stickshift_engine_push_events(
    engine: *mut StickShiftEngine,
    events: *const StickShiftHostEvent,
    count: u32,
    stride: u32,
    out_requests: *mut u32,
) -> StickShiftResult {
    host_call(engine, "stickshift_engine_push_events", |e, state| {
        if let Some(out) = out_requests.as_mut() {
            *out = 0;
        }
        if count == 0 {
            return Ok(STICKSHIFT_OK);
        }
        if events.is_null() {
            return Err(Error::null("events"));
        }
        let size = std::mem::size_of::<StickShiftHostEvent>();
        if (stride as usize) < size {
            return Err(Error::invalid_argument(format!(
                "stride {stride} is below sizeof(StickShiftHostEvent), {size}"
            )));
        }
        let decoded = (0..count as usize)
            .map(|i| {
                let event = events
                    .cast::<u8>()
                    .add(i * stride as usize)
                    .cast::<StickShiftHostEvent>()
                    .read_unaligned();
                decode(&event).map_err(|why| Error::invalid_argument(format!("event {i}: {why}")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if e.failed.get() {
            return Err(failed());
        }
        let State {
            session,
            game,
            renderer,
        } = state;
        let mut requests = Requests::NONE;
        for event in decoded {
            requests |= session.event(event, game, renderer);
            if let HostEvent::SafeArea(points) = event {
                renderer.set_safe_area_points(points);
            }
            if e.poisoned.get() {
                return Err(poisoned());
            }
        }
        if let Some(out) = out_requests.as_mut() {
            *out = requests.bits();
        }
        Ok(STICKSHIFT_OK)
    })
}

/// The next `STICKSHIFT_CLOCK_TIMESTAMP` or `_ENGINE` tick banks exactly one
/// fixed step: for an app returning to the foreground, or a modal sheet.
///
/// # Safety
/// `engine` must be NULL or a live engine.
#[no_mangle]
pub unsafe extern "C" fn stickshift_engine_reset_clock(
    engine: *mut StickShiftEngine,
) -> StickShiftResult {
    host_call(engine, "stickshift_engine_reset_clock", |_, state| {
        state.session.reset_clock();
        Ok(STICKSHIFT_OK)
    })
}

/// Ask to stop: every later tick reports `STICKSHIFT_REQUEST_EXIT`. The engine
/// never quits by itself. In a hook, or between engine calls.
///
/// # Safety
/// `engine` must be NULL or a live engine.
#[no_mangle]
pub unsafe extern "C" fn stickshift_engine_request_exit(
    engine: *mut StickShiftEngine,
) -> StickShiftResult {
    game_call(engine, "stickshift_engine_request_exit", |_, access| {
        match access {
            Access::Idle(state) => state.session.lp.exit_requested = true,
            Access::Hook(ctx) => ctx.request_exit(),
        }
        Ok(STICKSHIFT_OK)
    })
}

/// The engine's flags and counters. In a hook, or between engine calls. Set
/// `out->size = sizeof(StickShiftEngineState)`.
///
/// # Safety
/// `engine` must be NULL or a live engine; `out` NULL or valid for its
/// declared size.
#[no_mangle]
pub unsafe extern "C" fn stickshift_engine_get_state(
    engine: *mut StickShiftEngine,
    out: *mut StickShiftEngineState,
) -> StickShiftResult {
    game_call(engine, "stickshift_engine_get_state", |e, access| {
        let exit_requested = match access {
            Access::Idle(state) => state.session.lp.exit_requested,
            Access::Hook(ctx) => ctx.exit_requested(),
        };
        let counters = e.counters.get();
        let state = StickShiftEngineState {
            size: 0,
            initialized: u8::from(e.initialized.get()),
            hosted: u8::from(e.hosted.get()),
            failed: u8::from(e.failed.get()),
            poisoned: u8::from(e.poisoned.get()),
            exit_requested: u8::from(exit_requested),
            holding: u8::from(e.holding.get()),
            reserved: [0; 2],
            frames: counters.frames,
            updates: counters.updates,
            frames_rendered: counters.rendered,
            frames_discarded: counters.discarded,
        };
        write_sized(out, state, "out")?;
        Ok(STICKSHIFT_OK)
    })
}

/// Frame timing: in a hook, the snapshot the hook's frame started with and,
/// in the render hook, the interpolation alpha; between engine calls, the
/// latest snapshot and alpha 0. Set `out->size = sizeof(StickShiftPerf)`.
///
/// # Safety
/// `engine` must be NULL or a live engine; `out` NULL or valid for its
/// declared size.
#[no_mangle]
pub unsafe extern "C" fn stickshift_engine_get_perf(
    engine: *mut StickShiftEngine,
    out: *mut StickShiftPerf,
) -> StickShiftResult {
    game_call(engine, "stickshift_engine_get_perf", |_, access| {
        let (snapshot, alpha) = match access {
            Access::Idle(state) => (state.session.lp.perf.snapshot(), 0.0),
            Access::Hook(ctx) => (ctx.perf(), ctx.alpha()),
        };
        let ns = |d: Duration| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX);
        let perf = StickShiftPerf {
            size: 0,
            alpha,
            frame_count: snapshot.frame_count as u64,
            current_fps: snapshot.current_fps,
            avg_fps: snapshot.avg_fps,
            avg_frame_ns: ns(snapshot.avg_frame_time),
            min_frame_ns: ns(snapshot.min_frame_time),
            max_frame_ns: ns(snapshot.max_frame_time),
            avg_cpu_ns: ns(snapshot.avg_cpu_time),
            avg_gpu_ns: ns(snapshot.avg_gpu_time),
            avg_render_ns: ns(snapshot.avg_render_time),
        };
        write_sized(out, perf, "out")?;
        Ok(STICKSHIFT_OK)
    })
}

/// Hooks for the crate's own tests, reaching what later stages expose through
/// the C API (the camera, the input) through the same scope rules. Not part of
/// the C API.
#[doc(hidden)]
pub mod testing {
    use super::{game_call, Access, StickShiftEngine};
    use crate::context::{with_context, StickShiftContext};
    use crate::core::{call, Error};
    use crate::types::*;
    use stickshift_math::glam::{Mat4, Vec3};

    /// Move the engine camera as a game does: `EngineCtx::camera_mut` in a hook
    /// (which arms the push rule), the loop's camera between calls.
    ///
    /// # Safety
    /// `engine` must be NULL or a live engine.
    pub unsafe fn set_camera_position(
        engine: *mut StickShiftEngine,
        position: Vec3,
    ) -> StickShiftResult {
        game_call(engine, "set_camera_position", |_, access| {
            match access {
                Access::Idle(state) => state.session.lp.camera.set_position(position),
                Access::Hook(ctx) => ctx.camera_mut().set_position(position),
            }
            Ok(STICKSHIFT_OK)
        })
    }

    /// The engine camera's view-projection and position, as the game sees them.
    ///
    /// # Safety
    /// `engine` must be NULL or a live engine.
    pub unsafe fn camera(
        engine: *mut StickShiftEngine,
        out: &mut (Mat4, Vec3),
    ) -> StickShiftResult {
        game_call(engine, "camera", |_, access| {
            let camera = match access {
                Access::Idle(state) => &state.session.lp.camera,
                Access::Hook(ctx) => ctx.camera(),
            };
            *out = (camera.view_projection_matrix(), camera.position());
            Ok(STICKSHIFT_OK)
        })
    }

    /// The camera's aspect ratio, as the game sees it.
    ///
    /// # Safety
    /// `engine` must be NULL or a live engine.
    pub unsafe fn camera_aspect(engine: *mut StickShiftEngine, out: &mut f32) -> StickShiftResult {
        game_call(engine, "camera_aspect", |_, access| {
            *out = match access {
                Access::Idle(state) => state.session.lp.camera.aspect_ratio(),
                Access::Hook(ctx) => ctx.camera().aspect_ratio(),
            };
            Ok(STICKSHIFT_OK)
        })
    }

    /// `key`'s state as the game's input shows it: down, just pressed, just
    /// released.
    ///
    /// # Safety
    /// `engine` must be NULL or a live engine.
    pub unsafe fn key_state(
        engine: *mut StickShiftEngine,
        key: StickShiftKey,
        out: &mut [bool; 3],
    ) -> StickShiftResult {
        game_call(engine, "key_state", |_, access| {
            let key = super::key(key).ok_or_else(|| Error::invalid_argument("not a key"))?;
            let input = match access {
                Access::Idle(state) => &state.session.lp.input,
                Access::Hook(ctx) => ctx.input(),
            };
            *out = [
                input.is_key_down(key),
                input.is_key_just_pressed(key),
                input.is_key_just_released(key),
            ];
            Ok(STICKSHIFT_OK)
        })
    }

    /// Hold `slots` of a null context's frame slots, as if that many frames
    /// were on a GPU, so frames find no free slot.
    ///
    /// # Safety
    /// `ctx` must be NULL or a live null context.
    pub unsafe fn hold_null_frame_slots(
        ctx: *mut StickShiftContext,
        slots: u64,
    ) -> StickShiftResult {
        call(|| {
            with_context(ctx, |inner| {
                let null = inner
                    .backend
                    .as_null()
                    .ok_or_else(|| Error::invalid_argument("not a null context"))?;
                null.held_slots = slots;
                Ok(STICKSHIFT_OK)
            })
        })
    }

    /// Make the engine's next update step panic inside Rust (not inside a
    /// hook), as a bug in the engine would.
    ///
    /// # Safety
    /// `engine` must be a live engine, used on its own thread.
    pub unsafe fn panic_in_next_update(engine: *mut StickShiftEngine) {
        (*engine).panic_in_update.set(true);
    }

    /// How many times the loop called one of the C renderer's inert methods
    /// (which it never should).
    ///
    /// # Safety
    /// `engine` must be a live engine, between engine calls on its own thread.
    pub unsafe fn renderer_violations(engine: *mut StickShiftEngine) -> u32 {
        (*(*engine).state.get()).renderer.violations
    }
}
