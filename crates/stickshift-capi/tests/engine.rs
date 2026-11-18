// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! The engine object through its exported functions: a C-style test game (its
//! hooks are `extern "C"` functions over a user pointer) on null contexts,
//! checked against the Rust loop it runs and against the scope rules. The
//! scope matrix here covers every function that takes an engine (checked
//! against the header), in every scope: between engine calls, each hook, a
//! stream spawn callback, a load completion and the release hook.
//!
//! Hooks never assert: a panic cannot unwind out of an `extern "C"` function.
//! They record what they see, and the tests check it after the call.

use std::cell::RefCell;
use std::ffi::{c_void, CStr};
use std::ptr::{null, null_mut};
use std::rc::Rc;

use stickshift::testing;
use stickshift::*;
use stickshift_core::driver::{drive_frame, Loop};
use stickshift_core::{EngineCtx, Game, FIXED_DT, FIXED_DT_DURATION, MAX_STEPS_PER_FRAME};
use stickshift_math::glam::{Mat4, Vec3};
use stickshift_math::Transform;
use stickshift_render_api::{
    FrameRecorder, GpuFrameTime, Light, MaterialParams, MeshData, MeshHandle, NullRenderer,
    OverlayQuad, PipelineDescriptor, PipelineHandle, RenderDevice, RenderSettings, SunSky,
    TextureData, TextureHandle,
};

fn last_error() -> String {
    // SAFETY: never NULL; valid until the next error on this thread.
    unsafe { CStr::from_ptr(stickshift_last_error_message()) }
        .to_string_lossy()
        .into_owned()
}

/// Which hook a probe callback is running in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum At {
    Init,
    Update,
    Render,
    Settings,
}

type Script = Box<dyn FnMut(&mut Probe, At, *mut StickShiftEngine, *mut StickShiftFrame)>;

/// The test game's state, behind the hooks' user pointer.
#[derive(Default)]
struct Probe {
    inits: u32,
    updates: u32,
    renders: u32,
    settings: Vec<StickShiftGraphicsSettings>,
    releases: u32,
    init_result: StickShiftResult,
    /// What the hooks saw, for the test to check afterwards.
    seen: Vec<String>,
    script: Option<Script>,
    /// When set, the release hook makes every engine call on this engine
    /// ([`engine_calls`]) and records what each returned in `seen`.
    calls_in_release: Option<*mut StickShiftEngine>,
}

impl Probe {
    fn run(&mut self, at: At, engine: *mut StickShiftEngine, frame: *mut StickShiftFrame) {
        if let Some(mut script) = self.script.take() {
            script(self, at, engine, frame);
            self.script = Some(script);
        }
    }
}

unsafe fn probe<'a>(user: *mut c_void) -> &'a mut Probe {
    &mut *user.cast::<Probe>()
}

unsafe extern "C" fn init(user: *mut c_void, engine: *mut StickShiftEngine) -> StickShiftResult {
    let p = probe(user);
    p.inits += 1;
    p.run(At::Init, engine, null_mut());
    p.init_result
}

unsafe extern "C" fn update(user: *mut c_void, engine: *mut StickShiftEngine, dt: f32) {
    let p = probe(user);
    p.updates += 1;
    if dt.to_bits() != FIXED_DT.to_bits() {
        p.seen.push(format!("dt {dt}"));
    }
    p.run(At::Update, engine, null_mut());
}

unsafe extern "C" fn render(
    user: *mut c_void,
    engine: *mut StickShiftEngine,
    frame: *mut StickShiftFrame,
) {
    let p = probe(user);
    p.renders += 1;
    p.run(At::Render, engine, frame);
}

unsafe extern "C" fn settings_changed(
    user: *mut c_void,
    engine: *mut StickShiftEngine,
    settings: *const StickShiftGraphicsSettings,
) {
    let p = probe(user);
    p.settings.push(*settings);
    p.run(At::Settings, engine, null_mut());
}

unsafe extern "C" fn release(user: *mut c_void) {
    let p = probe(user);
    p.releases += 1;
    if let Some(e) = p.calls_in_release {
        for (name, result) in engine_calls(e, null_mut()) {
            p.seen.push(format!("Release {name} {result}"));
        }
    }
}

fn null_desc(width: u32, height: u32) -> StickShiftContextDesc {
    StickShiftContextDesc {
        api_version: STICKSHIFT_API_VERSION,
        backend: STICKSHIFT_BACKEND_NULL,
        metal_layer: null_mut(),
        width,
        height,
        scale: 1.0,
    }
}

fn engine_desc() -> StickShiftEngineDesc {
    StickShiftEngineDesc {
        api_version: STICKSHIFT_API_VERSION,
        size: std::mem::size_of::<StickShiftEngineDesc>() as u32,
        audio: STICKSHIFT_AUDIO_SILENT,
        pacing: STICKSHIFT_FRAME_PACING_WAIT,
        flags: 0,
        reserved: 0,
        settings_suite: null(),
    }
}

fn game_of(probe: &mut Probe) -> StickShiftGame {
    StickShiftGame {
        size: std::mem::size_of::<StickShiftGame>() as u32,
        reserved: 0,
        user: std::ptr::from_mut(probe).cast(),
        init: Some(init),
        update: Some(update),
        render: Some(render),
        graphics_settings_changed: Some(settings_changed),
        release: Some(release),
    }
}

/// A null context with an engine on it, both destroyed when dropped.
struct Harness {
    ctx: *mut StickShiftContext,
    engine: *mut StickShiftEngine,
    probe: Box<Probe>,
}

impl Harness {
    fn new(
        script: impl FnMut(&mut Probe, At, *mut StickShiftEngine, *mut StickShiftFrame) + 'static,
    ) -> Self {
        Self::with(engine_desc(), (1280, 720), script)
    }

    fn with(
        desc: StickShiftEngineDesc,
        (width, height): (u32, u32),
        script: impl FnMut(&mut Probe, At, *mut StickShiftEngine, *mut StickShiftFrame) + 'static,
    ) -> Self {
        let mut probe = Box::new(Probe {
            script: Some(Box::new(script)),
            ..Probe::default()
        });
        let mut ctx = null_mut();
        let mut engine = null_mut();
        let game = game_of(&mut probe);
        // SAFETY: valid pointers to locals; the probe is boxed, so `user`
        // stays valid as the harness moves.
        unsafe {
            assert_eq!(
                stickshift_context_create(&null_desc(width, height), &mut ctx),
                STICKSHIFT_OK
            );
            let r = stickshift_engine_create(ctx, &desc, &game, &mut engine);
            assert_eq!(r, STICKSHIFT_OK, "{}", last_error());
        }
        Self { ctx, engine, probe }
    }

    fn init(&self) -> StickShiftResult {
        // SAFETY: a live engine.
        unsafe { stickshift_engine_init(self.engine) }
    }

    fn tick_with(
        &self,
        clock: StickShiftClock,
        nanoseconds: u64,
    ) -> (StickShiftResult, StickShiftTickResult) {
        let desc = StickShiftTickDesc {
            size: std::mem::size_of::<StickShiftTickDesc>() as u32,
            clock,
            nanoseconds,
            readback: null_mut(),
            readback_len: 0,
        };
        let mut out = StickShiftTickResult {
            size: std::mem::size_of::<StickShiftTickResult>() as u32,
            ..Default::default()
        };
        // SAFETY: a live engine and valid locals.
        let r = unsafe { stickshift_engine_tick(self.engine, &desc, &mut out) };
        (r, out)
    }

    fn tick(&self) -> StickShiftTickResult {
        let (r, out) = self.tick_with(STICKSHIFT_CLOCK_FIXED_STEP, 0);
        assert_eq!(r, STICKSHIFT_OK, "{}", last_error());
        out
    }

    fn push(&self, events: &[StickShiftHostEvent]) -> (StickShiftResult, u32) {
        let mut requests = 0;
        // SAFETY: a live engine; the events are 64 bytes apart.
        let r = unsafe {
            stickshift_engine_push_events(
                self.engine,
                events.as_ptr(),
                events.len() as u32,
                std::mem::size_of::<StickShiftHostEvent>() as u32,
                &mut requests,
            )
        };
        (r, requests)
    }

    fn state(&self) -> StickShiftEngineState {
        state_of(self.engine).expect("a live engine")
    }

    fn context_camera(&self) -> (Mat4, Vec3) {
        let mut camera = StickShiftCamera::default();
        // SAFETY: a live context.
        assert_eq!(
            unsafe { stickshift_context_get_camera(self.ctx, &mut camera) },
            STICKSHIFT_OK
        );
        (
            camera.view_projection.into_mat4(),
            camera.position.into_vec3(),
        )
    }

    fn last_frame(&self) -> StickShiftFrameRecord {
        last_frame_of(self.ctx)
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        // SAFETY: the engine first, then its context.
        unsafe {
            if !self.engine.is_null() {
                assert_eq!(stickshift_engine_destroy(self.engine), STICKSHIFT_OK);
            }
            assert_eq!(stickshift_context_destroy(self.ctx), STICKSHIFT_OK);
        }
    }
}

fn state_of(engine: *mut StickShiftEngine) -> Result<StickShiftEngineState, StickShiftResult> {
    let mut state = StickShiftEngineState {
        size: std::mem::size_of::<StickShiftEngineState>() as u32,
        ..Default::default()
    };
    // SAFETY: a live engine.
    match unsafe { stickshift_engine_get_state(engine, &mut state) } {
        STICKSHIFT_OK => Ok(state),
        other => Err(other),
    }
}

fn last_frame_of(ctx: *mut StickShiftContext) -> StickShiftFrameRecord {
    // SAFETY: zero is a valid bit pattern for every field.
    let mut record: StickShiftFrameRecord = unsafe { std::mem::zeroed() };
    record.size = std::mem::size_of::<StickShiftFrameRecord>() as u32;
    // SAFETY: a live context.
    assert_eq!(
        unsafe { stickshift_context_get_last_frame(ctx, &mut record) },
        STICKSHIFT_OK,
        "{}",
        last_error()
    );
    record
}

fn quads_of(record: &StickShiftFrameRecord) -> Vec<StickShiftQuad> {
    (0..record.quad_count as usize)
        .map(|i| {
            // SAFETY: the record's quads, `quad_stride` apart, valid until the
            // next frame opens.
            unsafe {
                record
                    .quads
                    .cast::<u8>()
                    .add(i * record.quad_stride as usize)
                    .cast::<StickShiftQuad>()
                    .read_unaligned()
            }
        })
        .collect()
}

fn draws_of(record: &StickShiftFrameRecord) -> Vec<StickShiftDrawRecord> {
    (0..record.draw_count as usize)
        .map(|i| {
            // SAFETY: as in `quads_of`.
            unsafe {
                record
                    .draws
                    .cast::<u8>()
                    .add(i * record.draw_stride as usize)
                    .cast::<StickShiftDrawRecord>()
                    .read_unaligned()
            }
        })
        .collect()
}

trait IntoGlam {
    fn into_mat4(self) -> Mat4;
}
impl IntoGlam for StickShiftMat4 {
    fn into_mat4(self) -> Mat4 {
        Mat4::from_cols_array(&self.m)
    }
}
trait IntoVec {
    fn into_vec3(self) -> Vec3;
}
impl IntoVec for StickShiftVec3 {
    fn into_vec3(self) -> Vec3 {
        Vec3::new(self.x, self.y, self.z)
    }
}

fn bits(m: Mat4) -> [u32; 16] {
    m.to_cols_array().map(f32::to_bits)
}

fn quad() -> StickShiftQuad {
    StickShiftQuad {
        x: 0.0,
        y: 0.0,
        width: 10.0,
        height: 10.0,
        u0: 0.0,
        v0: 0.0,
        u1: 1.0,
        v1: 1.0,
        color: StickShiftColor {
            r: 1.0,
            g: 0.5,
            b: 0.25,
            a: 1.0,
        },
        fill: STICKSHIFT_FILL_SOLID,
        texture: StickShiftTexture { id: 0 },
    }
}

fn event(kind: StickShiftHostEventKind) -> StickShiftHostEvent {
    StickShiftHostEvent {
        kind,
        ..Default::default()
    }
}

fn key_down(code: StickShiftKey) -> StickShiftHostEvent {
    StickShiftHostEvent {
        code,
        ..event(STICKSHIFT_HOST_KEY_DOWN)
    }
}

/// `key`'s state as the game's input shows it (`stickshift_input_get`): down,
/// just pressed, just released. All false if the call fails, which a hook
/// cannot assert.
fn key_state(e: *mut StickShiftEngine, key: StickShiftKey) -> [bool; 3] {
    let mut input = StickShiftInputState {
        size: std::mem::size_of::<StickShiftInputState>() as u32,
        ..Default::default()
    };
    // SAFETY: a live engine, used on its thread, and a valid local.
    if unsafe { stickshift_input_get(e, &mut input) } != STICKSHIFT_OK {
        return [false; 3];
    }
    let bits = input.keys[key as usize];
    [
        STICKSHIFT_KEY_STATE_DOWN,
        STICKSHIFT_KEY_STATE_JUST_PRESSED,
        STICKSHIFT_KEY_STATE_JUST_RELEASED,
    ]
    .map(|bit| bits & bit != 0)
}

/// The engine camera as the game sees it (`stickshift_camera_get`), or zeroed
/// if the call fails.
fn camera_state(e: *mut StickShiftEngine) -> StickShiftCameraState {
    let mut camera = StickShiftCameraState {
        size: std::mem::size_of::<StickShiftCameraState>() as u32,
        ..Default::default()
    };
    // SAFETY: as in `key_state`.
    unsafe { stickshift_camera_get(e, &mut camera) };
    camera
}

/// The engine camera's view-projection and eye.
fn camera_of(e: *mut StickShiftEngine) -> (Mat4, Vec3) {
    let camera = camera_state(e);
    (
        camera.view_projection.into_mat4(),
        camera.position.into_vec3(),
    )
}

// ---------------------------------------------------------------------------
// Versions and constants
// ---------------------------------------------------------------------------

#[test]
fn the_version_functions_and_the_fixed_step_match_the_engine() {
    assert_eq!(stickshift_api_version(), STICKSHIFT_API_VERSION);
    assert_eq!(stickshift_api_minor(), STICKSHIFT_API_MINOR);
    assert_eq!(STICKSHIFT_API_MINOR, 1);
    assert_eq!(stickshift_fixed_dt().to_bits(), 0x3C88_8889);
    assert_eq!(stickshift_fixed_dt().to_bits(), FIXED_DT.to_bits());
    assert_eq!(
        std::time::Duration::from_nanos(STICKSHIFT_FIXED_DT_NS),
        FIXED_DT_DURATION
    );
    assert_eq!(STICKSHIFT_MAX_STEPS_PER_FRAME, MAX_STEPS_PER_FRAME);
    assert_eq!(std::mem::size_of::<StickShiftHostEvent>(), 64);
    for (code, name) in [
        (STICKSHIFT_ABSENT, "STICKSHIFT_ABSENT"),
        (
            STICKSHIFT_ERROR_WRONG_THREAD,
            "STICKSHIFT_ERROR_WRONG_THREAD",
        ),
        (STICKSHIFT_ERROR_ABORTED, "STICKSHIFT_ERROR_ABORTED"),
    ] {
        // SAFETY: static NUL-terminated names.
        let got = unsafe { CStr::from_ptr(stickshift_result_name(code)) };
        assert_eq!(got.to_str().unwrap(), name);
    }
}

// ---------------------------------------------------------------------------
// Create and destroy
// ---------------------------------------------------------------------------

#[test]
fn create_checks_its_arguments_and_creates_nothing_on_failure() {
    let mut probe = Probe::default();
    let game = game_of(&mut probe);
    let mut ctx = null_mut();
    // SAFETY: valid locals throughout.
    unsafe {
        assert_eq!(
            stickshift_context_create(&null_desc(8, 8), &mut ctx),
            STICKSHIFT_OK
        );
        let mut engine = null_mut();
        let refused = |desc: StickShiftEngineDesc, game: StickShiftGame, code| {
            let mut engine = null_mut();
            assert_eq!(
                stickshift_engine_create(ctx, &desc, &game, &mut engine),
                code
            );
            assert!(engine.is_null());
        };
        refused(
            StickShiftEngineDesc {
                api_version: 3,
                ..engine_desc()
            },
            game,
            STICKSHIFT_ERROR_UNSUPPORTED,
        );
        refused(
            StickShiftEngineDesc {
                size: 0,
                ..engine_desc()
            },
            game,
            STICKSHIFT_ERROR_INVALID_ARGUMENT,
        );
        assert!(
            last_error().contains("set .size = sizeof"),
            "{}",
            last_error()
        );
        refused(
            StickShiftEngineDesc {
                flags: 1 << 9,
                ..engine_desc()
            },
            game,
            STICKSHIFT_ERROR_INVALID_ARGUMENT,
        );
        refused(
            StickShiftEngineDesc {
                audio: 7,
                ..engine_desc()
            },
            game,
            STICKSHIFT_ERROR_INVALID_ARGUMENT,
        );
        refused(
            StickShiftEngineDesc {
                pacing: 7,
                ..engine_desc()
            },
            game,
            STICKSHIFT_ERROR_INVALID_ARGUMENT,
        );
        refused(
            StickShiftEngineDesc {
                settings_suite: c"test.suite".as_ptr(),
                ..engine_desc()
            },
            game,
            STICKSHIFT_ERROR_INVALID_ARGUMENT,
        );
        refused(
            engine_desc(),
            StickShiftGame {
                update: None,
                ..game
            },
            STICKSHIFT_ERROR_INVALID_ARGUMENT,
        );
        refused(
            engine_desc(),
            StickShiftGame { size: 8, ..game },
            STICKSHIFT_ERROR_INVALID_ARGUMENT,
        );

        // A context with a frame open cannot take an engine.
        let mut frame = null_mut();
        assert_eq!(stickshift_frame_acquire(ctx, &mut frame), STICKSHIFT_OK);
        refused(engine_desc(), game, STICKSHIFT_ERROR_WRONG_STATE);
        assert_eq!(stickshift_frame_discard(frame), STICKSHIFT_OK);

        assert_eq!(
            stickshift_engine_create(ctx, &engine_desc(), &game, &mut engine),
            STICKSHIFT_OK
        );
        // One engine per context, and the context outlives it.
        refused(engine_desc(), game, STICKSHIFT_ERROR_WRONG_STATE);
        assert_eq!(
            stickshift_context_destroy(ctx),
            STICKSHIFT_ERROR_WRONG_STATE
        );
        let mut attached = null_mut();
        assert_eq!(
            stickshift_engine_get_context(engine, &mut attached),
            STICKSHIFT_OK
        );
        assert_eq!(attached, ctx);

        assert_eq!(probe.inits, 0, "no hook runs at create");
        assert_eq!(stickshift_engine_destroy(engine), STICKSHIFT_OK);
        assert_eq!(probe.releases, 1);
        assert_eq!(stickshift_engine_destroy(null_mut()), STICKSHIFT_OK);
        // Detached: the context can take a new engine, or be destroyed.
        assert_eq!(
            stickshift_engine_create(ctx, &engine_desc(), &game, &mut engine),
            STICKSHIFT_OK
        );
        assert_eq!(stickshift_engine_destroy(engine), STICKSHIFT_OK);
        assert_eq!(probe.releases, 2);
        assert_eq!(stickshift_context_destroy(ctx), STICKSHIFT_OK);
    }
}

// ---------------------------------------------------------------------------
// Cadence
// ---------------------------------------------------------------------------

#[test]
fn each_tick_runs_zero_to_five_updates_then_one_render() {
    let h = Harness::new(|_, _, _, _| {});
    assert_eq!(h.init(), STICKSHIFT_OK);
    assert_eq!(h.probe.inits, 1);
    let dt = STICKSHIFT_FIXED_DT_NS;
    for (elapsed, steps) in [
        (0, 0),
        (dt, 1),
        (5 * dt, 5),
        (60 * dt, 5),
        (dt / 2, 0),
        (dt / 2, 1),
    ] {
        let before = (h.probe.updates, h.probe.renders);
        let (r, out) = h.tick_with(STICKSHIFT_CLOCK_ELAPSED, elapsed);
        assert_eq!(r, STICKSHIFT_OK);
        assert_eq!(out.steps, steps, "{elapsed} ns");
        assert_eq!(h.probe.updates - before.0, steps);
        assert_eq!(h.probe.renders - before.1, 1, "one render per tick");
        assert_eq!((out.rendered, out.submitted), (1, 1));
        assert_eq!(out.frame, u64::from(h.probe.renders));
    }
    let state = h.state();
    assert_eq!(state.frames, 6);
    assert_eq!(state.updates, u64::from(h.probe.updates));
    assert_eq!(state.frames_rendered, 6);
    assert_eq!(state.frames_discarded, 0);
    assert!(h.probe.seen.is_empty(), "{:?}", h.probe.seen);
}

#[test]
fn the_fixed_step_clock_runs_exactly_one_update_for_100000_ticks() {
    let h = Harness::new(|_, _, _, _| {});
    assert_eq!(h.init(), STICKSHIFT_OK);
    for _ in 0..100_000 {
        assert_eq!(h.tick().steps, 1);
    }
    assert_eq!(h.probe.updates, 100_000);
    assert_eq!(h.probe.renders, 100_000);
}

#[test]
fn the_timestamp_and_engine_clocks_bank_one_step_first() {
    let h = Harness::new(|_, _, _, _| {});
    assert_eq!(h.init(), STICKSHIFT_OK);
    let dt = STICKSHIFT_FIXED_DT_NS;
    let t0 = 5_000_000_000;
    assert_eq!(h.tick_with(STICKSHIFT_CLOCK_TIMESTAMP, t0).1.steps, 1);
    assert_eq!(
        h.tick_with(STICKSHIFT_CLOCK_TIMESTAMP, t0 + 3 * dt).1.steps,
        3
    );
    // A clock that went backwards banks nothing.
    assert_eq!(h.tick_with(STICKSHIFT_CLOCK_TIMESTAMP, t0).1.steps, 0);
    // SAFETY: a live engine.
    assert_eq!(
        unsafe { stickshift_engine_reset_clock(h.engine) },
        STICKSHIFT_OK
    );
    assert_eq!(
        h.tick_with(STICKSHIFT_CLOCK_TIMESTAMP, t0 + 999 * dt)
            .1
            .steps,
        1
    );
    // The engine's own clock: the first reading banks one step too.
    assert_eq!(h.tick_with(STICKSHIFT_CLOCK_ENGINE, 0).1.steps, 1);
    assert_eq!(
        h.tick_with(7, 0).0,
        STICKSHIFT_ERROR_INVALID_ARGUMENT,
        "an unknown clock"
    );
}

#[test]
fn an_edge_waits_for_an_update_and_only_the_first_sees_it() {
    let h = Harness::new(|p, at, e, _| {
        if at == At::Update {
            let state = key_state(e, STICKSHIFT_KEY_SPACE);
            p.seen.push(format!("{state:?}"));
        }
    });
    assert_eq!(h.init(), STICKSHIFT_OK);
    assert_eq!(h.push(&[key_down(STICKSHIFT_KEY_SPACE)]).0, STICKSHIFT_OK);
    // A frame with no update keeps the edge.
    assert_eq!(h.tick_with(STICKSHIFT_CLOCK_ELAPSED, 0).1.steps, 0);
    assert_eq!(
        h.tick_with(STICKSHIFT_CLOCK_ELAPSED, 3 * STICKSHIFT_FIXED_DT_NS)
            .1
            .steps,
        3
    );
    let probe = &h.probe;
    assert_eq!(
        probe.seen,
        [
            "[true, true, false]",
            "[true, false, false]",
            "[true, false, false]"
        ]
    );
}

// ---------------------------------------------------------------------------
// The camera push, against the Rust loop
// ---------------------------------------------------------------------------

/// Moves the camera in init (then touches the renderer), in every update (and
/// touches it in every second one), and in every third render; what a probe
/// game in Rust does, so the two can be compared.
fn camera_move(at: At, n: u32) -> Option<(Vec3, bool)> {
    let at_n = |k: f32| Vec3::new(0.25 * k, 1.5 + 0.125 * k, -3.0 * k);
    match at {
        At::Init => Some((at_n(-1.0), true)),
        At::Update => Some((at_n(n as f32), n.is_multiple_of(2))),
        At::Render if n.is_multiple_of(3) => Some((at_n(100.0 + n as f32), true)),
        _ => None,
    }
}

/// A second move after the touch, with no context call, in every update that
/// touched: the next hook's first read shows whether the hook's end pushed it.
fn camera_move_after_touch(at: At, n: u32) -> Option<Vec3> {
    (at == At::Update && n.is_multiple_of(2))
        .then(|| Vec3::new(-0.5 * n as f32, 4.0, 2.0 * n as f32))
}

/// A camera the render hook sets on its frame after its move and touch, in
/// every second render. It must win over the moved engine camera, as
/// `ctx.renderer().set_view_projection` does in Rust.
fn frame_camera(at: At, n: u32) -> Option<(Mat4, Vec3)> {
    (at == At::Render && n.is_multiple_of(2)).then(|| {
        let eye = Vec3::new(1.0 + n as f32, 2.0, 5.0);
        (
            Mat4::perspective_rh(0.8, 1.5, 0.1, 100.0) * Mat4::look_at_rh(eye, Vec3::ZERO, Vec3::Y),
            eye,
        )
    })
}

/// A hook's two reads of the renderer's camera, right after its move and right
/// after its touch, as one line.
fn push_point(at: At, n: u32, before: (Mat4, Vec3), after: (Mat4, Vec3)) -> String {
    let bits = |(vp, eye): (Mat4, Vec3)| (bits(vp), eye.to_array().map(f32::to_bits));
    format!("{at:?} {n} {:?} {:?}", bits(before), bits(after))
}

/// A [`NullRenderer`] the Rust probe can read inside its hooks, where
/// `EngineCtx::renderer` would push the moved camera first.
#[derive(Clone)]
struct Spy(Rc<RefCell<NullRenderer>>);

impl Spy {
    fn camera(&self) -> (Mat4, Vec3) {
        let r = self.0.borrow();
        (
            r.view_projection().unwrap_or(Mat4::IDENTITY),
            r.camera_position().unwrap_or(Vec3::ZERO),
        )
    }
}

impl RenderDevice for Spy {
    fn create_mesh(&mut self, data: &MeshData<'_>) -> MeshHandle {
        self.0.borrow_mut().create_mesh(data)
    }
    fn destroy_mesh(&mut self, handle: MeshHandle) {
        self.0.borrow_mut().destroy_mesh(handle);
    }
    fn create_texture(&mut self, data: &TextureData<'_>) -> TextureHandle {
        self.0.borrow_mut().create_texture(data)
    }
    fn destroy_texture(&mut self, handle: TextureHandle) {
        self.0.borrow_mut().destroy_texture(handle);
    }
    fn create_pipeline(&mut self, desc: &PipelineDescriptor) -> PipelineHandle {
        self.0.borrow_mut().create_pipeline(desc)
    }
    fn destroy_pipeline(&mut self, handle: PipelineHandle) {
        self.0.borrow_mut().destroy_pipeline(handle);
    }
    fn surface_size(&self) -> (u32, u32) {
        self.0.borrow().surface_size()
    }
    fn safe_area_insets(&self) -> [f32; 4] {
        self.0.borrow().safe_area_insets()
    }
    fn resize_surface(&mut self, width: u32, height: u32, pixels_per_point: f32) {
        self.0
            .borrow_mut()
            .resize_surface(width, height, pixels_per_point);
    }
    fn surface_scale(&self) -> f32 {
        self.0.borrow().surface_scale()
    }
    fn last_gpu_frame_time(&self) -> Option<GpuFrameTime> {
        self.0.borrow().last_gpu_frame_time()
    }
    fn set_render_settings(&mut self, settings: &RenderSettings) {
        self.0.borrow_mut().set_render_settings(settings);
    }
}

impl FrameRecorder for Spy {
    fn begin_frame(&mut self) {
        self.0.borrow_mut().begin_frame();
    }
    fn set_view_projection(&mut self, view_proj: Mat4) {
        self.0.borrow_mut().set_view_projection(view_proj);
    }
    fn set_camera_position(&mut self, position: Vec3) {
        self.0.borrow_mut().set_camera_position(position);
    }
    fn set_sun_sky(&mut self, sun: &SunSky) {
        self.0.borrow_mut().set_sun_sky(sun);
    }
    fn add_light(&mut self, light: &Light) {
        self.0.borrow_mut().add_light(light);
    }
    fn set_pipeline(&mut self, handle: PipelineHandle) {
        self.0.borrow_mut().set_pipeline(handle);
    }
    fn bind_texture(&mut self, handle: TextureHandle) {
        self.0.borrow_mut().bind_texture(handle);
    }
    fn draw_mesh(&mut self, mesh: MeshHandle, transform: &Transform, material: &MaterialParams) {
        self.0.borrow_mut().draw_mesh(mesh, transform, material);
    }
    fn draw_mesh_matrix(&mut self, mesh: MeshHandle, model: Mat4, material: &MaterialParams) {
        self.0.borrow_mut().draw_mesh_matrix(mesh, model, material);
    }
    fn draw_overlay_quad(&mut self, quad: &OverlayQuad) {
        self.0.borrow_mut().draw_overlay_quad(quad);
    }
    fn submit(&mut self) {
        self.0.borrow_mut().submit();
    }
}

struct RustProbe {
    updates: u32,
    renders: u32,
    spy: Spy,
    points: Vec<String>,
}

impl RustProbe {
    /// What the C probe's hooks do, in Rust.
    fn hook(&mut self, ctx: &mut EngineCtx, at: At, n: u32) {
        if let Some((p, touch)) = camera_move(at, n) {
            ctx.camera_mut().set_position(p);
            let before = self.spy.camera();
            if touch {
                let _ = ctx.renderer();
            }
            let after = self.spy.camera();
            self.points.push(push_point(at, n, before, after));
        }
        if let Some(p) = camera_move_after_touch(at, n) {
            ctx.camera_mut().set_position(p);
        }
        if let Some((view_projection, eye)) = frame_camera(at, n) {
            let renderer = ctx.renderer();
            renderer.set_view_projection(view_projection);
            renderer.set_camera_position(eye);
        }
    }
}

impl Game for RustProbe {
    fn init(&mut self, ctx: &mut EngineCtx) {
        self.hook(ctx, At::Init, 0);
    }

    fn update(&mut self, ctx: &mut EngineCtx, _dt: f32) {
        self.updates += 1;
        self.hook(ctx, At::Update, self.updates);
    }

    fn render(&mut self, ctx: &mut EngineCtx) {
        self.renders += 1;
        self.hook(ctx, At::Render, self.renders);
    }
}

/// The context's camera, read inside each hook without the push rule right
/// after the game moves the camera and right after its first context call, and
/// after every call, equals what the Rust loop's renderer holds at the same
/// points: the push happens at the first context call, not when the hook
/// returns.
#[test]
fn the_context_camera_follows_the_rust_push_points() {
    let h = Harness::new(|p, at, e, frame| {
        let n = match at {
            At::Update => p.updates,
            At::Render => p.renders,
            _ => 0,
        };
        // SAFETY: inside a hook, on the engine's thread.
        unsafe {
            let mut ctx = null_mut();
            stickshift_engine_get_context(e, &mut ctx);
            let unpushed = || {
                let mut camera = (Mat4::ZERO, Vec3::ZERO);
                testing::context_camera_unpushed(ctx, &mut camera);
                camera
            };
            if let Some((position, touch)) = camera_move(at, n) {
                stickshift_camera_set_position(e, position.into());
                let before = unpushed();
                if touch {
                    // Any context call: the push rule runs before it.
                    let (mut w, mut h) = (0, 0);
                    stickshift_context_get_size(ctx, &mut w, &mut h);
                }
                p.seen.push(push_point(at, n, before, unpushed()));
            }
            if let Some(position) = camera_move_after_touch(at, n) {
                stickshift_camera_set_position(e, position.into());
            }
            if let Some((view_projection, eye)) = frame_camera(at, n) {
                let camera = StickShiftCamera {
                    view_projection: view_projection.into(),
                    position: eye.into(),
                };
                stickshift_frame_set_camera(frame, &camera);
            }
        }
    });
    let mut lp = Loop::new();
    let spy = Spy(Rc::new(RefCell::new(NullRenderer::new())));
    let mut game = RustProbe {
        updates: 0,
        renders: 0,
        spy: spy.clone(),
        points: Vec::new(),
    };
    let mut renderer = spy.clone();

    let same = |h: &Harness, when: &str| {
        let (vp, eye) = h.context_camera();
        let (want_vp, want_eye) = spy.camera();
        assert_eq!(bits(vp), bits(want_vp), "view-projection {when}");
        assert_eq!(
            eye.to_array().map(f32::to_bits),
            want_eye.to_array().map(f32::to_bits),
            "eye {when}"
        );
    };
    assert_eq!(h.init(), STICKSHIFT_OK);
    lp.init_once(&mut game, &mut renderer);
    assert_eq!(h.probe.seen, game.points, "in init");
    same(&h, "after init");
    assert_ne!(h.context_camera().0, Mat4::IDENTITY, "init pushed a camera");
    // The render settings the loop pushed at init reached the context too.
    let mut settings = StickShiftRenderSettings {
        shadows: 9,
        shadow_distance: 0.0,
        draw_distance_scale: 0.0,
    };
    // SAFETY: a live context.
    unsafe { stickshift_context_get_settings(h.ctx, &mut settings) };
    let want = spy.0.borrow().render_settings().expect("pushed at init");
    assert_eq!(settings.draw_distance_scale, want.draw_distance_scale);

    let dt = STICKSHIFT_FIXED_DT_NS;
    for (i, elapsed) in [dt, 0, 3 * dt, dt, 2 * dt, 0, dt].into_iter().enumerate() {
        let (r, _) = h.tick_with(STICKSHIFT_CLOCK_ELAPSED, elapsed);
        assert_eq!(r, STICKSHIFT_OK);
        drive_frame(
            &mut lp,
            &mut game,
            &mut renderer,
            std::time::Duration::from_nanos(elapsed),
        );
        assert_eq!(h.probe.seen, game.points, "in the hooks of tick {i}");
        same(&h, &format!("after tick {i}"));
        // A frame camera set after a push stayed the frame's.
        if frame_camera(At::Render, game.renders).is_some() {
            let record = h.last_frame();
            assert_eq!(
                bits(record.camera.view_projection.into_mat4()),
                bits(frame_camera(At::Render, game.renders).unwrap().0),
                "the render hook's frame camera, tick {i}"
            );
        }
    }
    assert_eq!(h.probe.updates, game.updates);
    assert_eq!(h.probe.renders, game.renders);
    assert_eq!(
        h.probe.seen.len(),
        1 + 8 + 2,
        "init, 8 updates, renders 3 and 6"
    );
    let engine_camera = camera_of(h.engine);
    assert_eq!(
        bits(engine_camera.0),
        bits(lp.camera.view_projection_matrix())
    );
    // SAFETY: between calls.
    assert_eq!(unsafe { testing::renderer_violations(h.engine) }, 0);
}

#[test]
fn render_sees_the_camera_the_updates_left() {
    let h = Harness::new(|p, at, e, _| unsafe {
        // SAFETY: inside hooks, on the engine's thread.
        match at {
            At::Update => {
                // Moved, but no context call: drive_frame's push before render
                // carries it.
                stickshift_camera_set_position(e, Vec3::new(0.0, 2.0, p.updates as f32).into());
            }
            At::Render => {
                let mut ctx = null_mut();
                stickshift_engine_get_context(e, &mut ctx);
                let mut camera = StickShiftCamera::default();
                stickshift_context_get_camera(ctx, &mut camera);
                let engine = camera_of(e);
                p.seen.push(format!(
                    "{}",
                    bits(camera.view_projection.into_mat4()) == bits(engine.0)
                        && camera.position.into_vec3() == engine.1
                ));
            }
            _ => {}
        }
    });
    assert_eq!(h.init(), STICKSHIFT_OK);
    h.tick();
    h.tick();
    assert_eq!(h.probe.seen, ["true", "true"]);
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

fn menu(tag: u32) -> StickShiftHostEvent {
    StickShiftHostEvent {
        code: tag,
        ..event(STICKSHIFT_HOST_MENU)
    }
}

#[test]
fn a_menu_choice_before_init_applies_with_no_callback() {
    let h = Harness::new(|_, _, _, _| {});
    // Shadows: Off (section 1, item 1).
    let (r, requests) = h.push(&[menu(101)]);
    assert_eq!(r, STICKSHIFT_OK);
    assert_eq!(requests & STICKSHIFT_REQUEST_MENU_SYNC, 0);
    assert_eq!(h.init(), STICKSHIFT_OK);
    assert!(
        h.probe.settings.is_empty(),
        "no callback for a queued request"
    );
    let mut settings = StickShiftRenderSettings {
        shadows: 9,
        shadow_distance: 0.0,
        draw_distance_scale: 0.0,
    };
    // SAFETY: a live context.
    unsafe { stickshift_context_get_settings(h.ctx, &mut settings) };
    assert_eq!(settings.shadows, STICKSHIFT_SHADOW_QUALITY_OFF);
    assert_eq!(h.tick().graphics_revision, 0, "a restore is not a change");

    // After init a choice applies at once, inside the call: one callback.
    let (r, requests) = h.push(&[menu(103)]);
    assert_eq!(r, STICKSHIFT_OK);
    assert_ne!(requests & STICKSHIFT_REQUEST_MENU_SYNC, 0);
    assert_eq!(h.probe.settings.len(), 1);
    assert_eq!(h.probe.settings[0].shadows, STICKSHIFT_SHADOW_QUALITY_HIGH);
    assert_eq!(h.tick().graphics_revision, 1);
    // The full-screen item is the host's to carry out.
    let (_, requests) = h.push(&[menu(901)]);
    assert_eq!(requests, STICKSHIFT_REQUEST_TOGGLE_FULLSCREEN);
    assert_eq!(h.push(&[menu(905)]).0, STICKSHIFT_ERROR_INVALID_ARGUMENT);
}

// ---------------------------------------------------------------------------
// Host events, views and exit
// ---------------------------------------------------------------------------

#[test]
fn events_are_checked_first_and_applied_in_order() {
    let h = Harness::new(|_, _, _, _| {});
    assert_eq!(h.init(), STICKSHIFT_OK);
    let bad = [
        StickShiftHostEvent {
            code: 99,
            ..event(STICKSHIFT_HOST_KEY_DOWN)
        },
        StickShiftHostEvent {
            flags: 2,
            ..key_down(STICKSHIFT_KEY_W)
        },
        StickShiftHostEvent {
            reserved: 1,
            ..event(STICKSHIFT_HOST_CLOSE)
        },
        StickShiftHostEvent {
            scale_factor: 0.0,
            ..event(STICKSHIFT_HOST_VIEW_RESIZED)
        },
        StickShiftHostEvent {
            x: f64::NAN,
            ..event(STICKSHIFT_HOST_CURSOR)
        },
        StickShiftHostEvent {
            insets: [0.0, -1.0, 0.0, 0.0],
            ..event(STICKSHIFT_HOST_SAFE_AREA)
        },
        event(0),
        event(12),
    ];
    for bad in bad {
        // A valid close first: nothing applies if any event is invalid.
        let (r, requests) = h.push(&[event(STICKSHIFT_HOST_CLOSE), bad]);
        assert_eq!(r, STICKSHIFT_ERROR_INVALID_ARGUMENT, "{bad:?}");
        assert!(last_error().starts_with("event 1:"), "{}", last_error());
        assert_eq!(requests, 0);
    }
    // A stride below the first layout.
    let mut requests = 0;
    let events = [event(STICKSHIFT_HOST_CLOSE)];
    // SAFETY: a live engine.
    let r =
        unsafe { stickshift_engine_push_events(h.engine, events.as_ptr(), 1, 32, &mut requests) };
    assert_eq!(r, STICKSHIFT_ERROR_INVALID_ARGUMENT);

    // A wider stride, as a newer header's events would have.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Wider {
        event: StickShiftHostEvent,
        more: [u8; 16],
    }
    let wide = [
        Wider {
            event: key_down(STICKSHIFT_KEY_A),
            more: [0xFF; 16],
        },
        Wider {
            event: event(STICKSHIFT_HOST_CLOSE),
            more: [0xFF; 16],
        },
    ];
    // SAFETY: two events, `size_of::<Wider>()` apart.
    let r = unsafe {
        stickshift_engine_push_events(
            h.engine,
            wide.as_ptr().cast(),
            2,
            std::mem::size_of::<Wider>() as u32,
            &mut requests,
        )
    };
    assert_eq!(r, STICKSHIFT_OK);
    assert_eq!(requests, STICKSHIFT_REQUEST_EXIT);
    assert_eq!(key_state(h.engine, STICKSHIFT_KEY_A), [true, true, false]);
    // A close asks; it does not stop the engine.
    assert_eq!(h.tick().requests & STICKSHIFT_REQUEST_EXIT, 0);
}

#[test]
fn escape_exits_only_with_quit_on_escape_and_still_presses() {
    for (flags, exits) in [(0, false), (STICKSHIFT_ENGINE_QUIT_ON_ESCAPE, true)] {
        let h = Harness::with(
            StickShiftEngineDesc {
                flags,
                ..engine_desc()
            },
            (64, 64),
            |_, _, _, _| {},
        );
        let (r, requests) = h.push(&[key_down(STICKSHIFT_KEY_ESCAPE)]);
        assert_eq!(r, STICKSHIFT_OK);
        assert_eq!(requests == STICKSHIFT_REQUEST_EXIT, exits);
        assert!(key_state(h.engine, STICKSHIFT_KEY_ESCAPE)[0]);
        // Focus loss lets go of it.
        assert_eq!(
            h.push(&[event(STICKSHIFT_HOST_FOCUS_LOST)]).0,
            STICKSHIFT_OK
        );
        assert!(!key_state(h.engine, STICKSHIFT_KEY_ESCAPE)[0]);
    }
}

#[test]
fn a_game_or_host_exit_request_is_reported_by_every_tick() {
    let h = Harness::new(|p, at, e, _| {
        if at == At::Update && p.updates == 2 {
            // SAFETY: inside a hook.
            let r = unsafe { stickshift_engine_request_exit(e) };
            p.seen.push(format!("{r}"));
        }
    });
    assert_eq!(h.init(), STICKSHIFT_OK);
    assert_eq!(h.tick().requests, 0);
    assert_eq!(h.tick().requests, STICKSHIFT_REQUEST_EXIT);
    assert_eq!(
        h.tick().requests,
        STICKSHIFT_REQUEST_EXIT,
        "the engine never quits"
    );
    assert_eq!(h.probe.seen, ["0"]);
    assert_eq!(h.state().exit_requested, 1);

    let host = Harness::new(|_, _, _, _| {});
    // SAFETY: between calls.
    assert_eq!(
        unsafe { stickshift_engine_request_exit(host.engine) },
        STICKSHIFT_OK
    );
    assert_eq!(host.init(), STICKSHIFT_OK);
    assert_eq!(host.tick().requests, STICKSHIFT_REQUEST_EXIT);
}

fn view(width: u32, height: u32, scale_factor: f64, safe_area: [f32; 4]) -> StickShiftViewDesc {
    StickShiftViewDesc {
        size: std::mem::size_of::<StickShiftViewDesc>() as u32,
        width,
        height,
        reserved: 0,
        scale_factor,
        safe_area,
    }
}

fn size_and_scale(ctx: *mut StickShiftContext) -> (u32, u32, f32, [f32; 4]) {
    let (mut w, mut h, mut s, mut insets) = (0, 0, 0.0, [0.0; 4]);
    // SAFETY: a live context.
    unsafe {
        stickshift_context_get_size(ctx, &mut w, &mut h);
        stickshift_context_get_scale(ctx, &mut s);
        stickshift_context_get_safe_area(ctx, insets.as_mut_ptr());
    }
    (w, h, s, insets)
}

#[test]
fn resume_sizes_the_drawable_and_resizes_follow_the_view() {
    let h = Harness::new(|p, at, e, _| {
        if at == At::Init {
            let aspect = camera_state(e).aspect_ratio;
            p.seen.push(format!("init aspect {aspect}"));
        }
    });
    let mut requests = 0;
    // 1600 × 1200 physical pixels at 2 pixels per point, a notch inset.
    let v = view(1600, 1200, 2.0, [20.0, 0.0, 10.0, 0.0]);
    // SAFETY: a live engine and valid locals.
    assert_eq!(
        unsafe { stickshift_engine_resume(h.engine, &v, &mut requests) },
        STICKSHIFT_OK
    );
    assert_eq!(requests, STICKSHIFT_REQUEST_MENU_SYNC);
    assert_eq!(h.probe.inits, 1);
    assert_eq!(
        h.probe.seen,
        [format!("init aspect {}", 1600.0f32 / 1200.0)]
    );
    // Half resolution by default: 800 × 600 at 1 pixel per point.
    assert_eq!(
        size_and_scale(h.ctx),
        (800, 600, 1.0, [20.0, 0.0, 10.0, 0.0])
    );
    let state = h.state();
    assert_eq!((state.initialized, state.hosted), (1, 1));

    // Resumed again: the drawable follows; init does not run again.
    let v = view(1000, 500, 1.0, [0.0; 4]);
    // SAFETY: as above.
    assert_eq!(
        unsafe { stickshift_engine_resume(h.engine, &v, &mut requests) },
        STICKSHIFT_OK
    );
    assert_eq!(h.probe.inits, 1);
    assert_eq!(size_and_scale(h.ctx).0, 500);

    // A scale change resizes and leaves the aspect alone; a resize moves both.
    let resized = |w, hgt, s| StickShiftHostEvent {
        width: w,
        height: hgt,
        scale_factor: s,
        ..Default::default()
    };
    let (r, requests) = h.push(&[StickShiftHostEvent {
        kind: STICKSHIFT_HOST_SCALE_CHANGED,
        ..resized(2000, 1000, 2.0)
    }]);
    assert_eq!((r, requests), (STICKSHIFT_OK, 0));
    assert_eq!(camera_state(h.engine).aspect_ratio, 1600.0 / 1200.0);
    assert_eq!(size_and_scale(h.ctx), (1000, 500, 1.0, [0.0; 4]));
    let (r, requests) = h.push(&[
        StickShiftHostEvent {
            kind: STICKSHIFT_HOST_VIEW_RESIZED,
            ..resized(900, 300, 3.0)
        },
        StickShiftHostEvent {
            insets: [4.0, 0.0, 0.0, 2.0],
            ..event(STICKSHIFT_HOST_SAFE_AREA)
        },
    ]);
    assert_eq!((r, requests), (STICKSHIFT_OK, STICKSHIFT_REQUEST_MENU_SYNC));
    assert_eq!(camera_state(h.engine).aspect_ratio, 3.0);
    assert_eq!(size_and_scale(h.ctx), (450, 150, 1.5, [6.0, 0.0, 0.0, 3.0]));

    let bad = view(10, 10, f64::NAN, [0.0; 4]);
    // SAFETY: as above.
    assert_eq!(
        unsafe { stickshift_engine_resume(h.engine, &bad, null_mut()) },
        STICKSHIFT_ERROR_INVALID_ARGUMENT
    );
}

// ---------------------------------------------------------------------------
// Scopes, threads, panics and failures
// ---------------------------------------------------------------------------

/// Where a call is made from, for the scope matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// Between engine calls.
    Idle,
    /// Inside a game hook.
    Hook(At),
    /// Inside a stream pass's spawn callback (in an update hook).
    Spawn,
    /// Inside a load completion that a tick's housekeeping delivers, outside
    /// every hook.
    Completion,
    /// Inside the release hook, which destroy runs, outside every hook.
    Release,
}

/// What a call is allowed to do, which fixes its result in each scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rule {
    /// Touches no engine state: any scope (`get_context`, the registry).
    Any,
    /// The host's (`[H]`): between engine calls only.
    Host,
    /// The game's (`[G]`): between engine calls or in any hook.
    Game,
    /// Reads the scene: as `Game`.
    SceneRead,
    /// Changes the scene: as `Game`, but not in the render hook.
    SceneWrite,
    /// Reads the world: as `Game`, and in a spawn callback.
    WorldRead,
    /// Changes the world: as `WorldRead`, but not in the render hook.
    WorldWrite,
    /// Only in a spawn callback.
    Track,
    /// The engine owns the render hook's frame; elsewhere the frame is NULL.
    EngineFrame,
    /// Releases the snapshot this scope's query took: fine wherever the query
    /// was allowed, else there is no live token.
    QueryRelease,
    /// Never, with an engine attached (`context_destroy`).
    Never,
}

/// Every call that takes an engine, and the context calls an engine
/// constrains, in the order [`engine_calls`] makes them.
const CALLS: &[(&str, Rule)] = &[
    ("engine_get_context", Rule::Any),
    // The world and its components.
    ("component_register", Rule::Any),
    ("component_get_desc", Rule::Any),
    ("world_spawn", Rule::WorldWrite),
    ("world_set", Rule::WorldWrite),
    ("world_insert", Rule::WorldWrite),
    ("world_remove", Rule::WorldWrite),
    ("world_get", Rule::WorldRead),
    ("world_has", Rule::WorldRead),
    ("world_contains", Rule::WorldRead),
    ("world_len", Rule::WorldRead),
    ("world_query", Rule::WorldRead),
    ("world_query_release", Rule::QueryRelease),
    ("world_query_count", Rule::WorldRead),
    // The scene.
    ("scene_get_config", Rule::SceneRead),
    ("scene_set_config", Rule::SceneWrite),
    ("scene_streamed_count", Rule::SceneRead),
    ("scene_stream", Rule::SceneWrite),
    ("scene_maybe_rebase", Rule::SceneWrite),
    ("stream_track", Rule::Track),
    // The camera, input and audio.
    ("camera_get", Rule::Game),
    ("camera_set_position", Rule::Game),
    ("camera_set_target", Rule::Game),
    ("camera_set_up", Rule::Game),
    ("camera_set_aspect_ratio", Rule::Game),
    ("camera_set_fov_y", Rule::Game),
    ("camera_set_clip_planes", Rule::Game),
    ("camera_follow", Rule::Game),
    ("input_get", Rule::Game),
    ("audio_load", Rule::Game),
    ("audio_play_once", Rule::Game),
    ("audio_play_looping", Rule::Game),
    ("audio_stop_looping", Rule::Game),
    ("audio_set_master_volume", Rule::Game),
    ("audio_get_stats", Rule::Game),
    ("audio_one_shots_of", Rule::Game),
    ("audio_get_path", Rule::Game),
    // Graphics settings and the menu.
    ("graphics_get_current", Rule::Game),
    ("graphics_request", Rule::Game),
    ("graphics_get_state", Rule::Host),
    ("engine_get_menu", Rule::Host),
    // The engine.
    ("engine_get_state", Rule::Game),
    ("engine_get_perf", Rule::Game),
    ("engine_request_exit", Rule::Game),
    ("engine_reset_clock", Rule::Host),
    ("engine_init", Rule::Host),
    ("engine_resume", Rule::Host),
    ("engine_push_events", Rule::Host),
    ("engine_tick", Rule::Host),
    // The context, with an engine on it.
    ("context_destroy", Rule::Never),
    ("frame_acquire", Rule::Host),
    ("frame_submit", Rule::EngineFrame),
    ("frame_discard", Rule::EngineFrame),
    ("context_get_last_frame", Rule::Any),
    // Last: refused in every scope it is made from (never between calls).
    ("engine_destroy", Rule::Host),
    ("world_despawn", Rule::WorldWrite),
];

/// What `rule` returns in `scope`.
fn expected(rule: Rule, scope: Scope) -> StickShiftResult {
    let ws = STICKSHIFT_ERROR_WRONG_STATE;
    let ia = STICKSHIFT_ERROR_INVALID_ARGUMENT;
    let ok = STICKSHIFT_OK;
    let hook = matches!(scope, Scope::Hook(_));
    let render = scope == Scope::Hook(At::Render);
    let game = scope == Scope::Idle || hook;
    let pick = |allowed: bool| if allowed { ok } else { ws };
    match rule {
        Rule::Any => ok,
        Rule::Host => pick(scope == Scope::Idle),
        Rule::Game | Rule::SceneRead => pick(game),
        Rule::SceneWrite => pick(game && !render),
        Rule::WorldRead => pick(game || scope == Scope::Spawn),
        Rule::WorldWrite => pick((game && !render) || scope == Scope::Spawn),
        Rule::Track => pick(scope == Scope::Spawn),
        Rule::EngineFrame if render => ws,
        Rule::EngineFrame => ia,
        Rule::QueryRelease if expected(Rule::WorldRead, scope) == ok => ok,
        Rule::QueryRelease => ia,
        Rule::Never => ws,
    }
}

fn transform_value(t: &StickShiftTransform) -> StickShiftComponentValue {
    StickShiftComponentValue {
        component: StickShiftComponent {
            id: STICKSHIFT_COMPONENT_TRANSFORM,
        },
        data: std::ptr::from_ref(t).cast(),
        size: std::mem::size_of::<StickShiftTransform>() as u32,
    }
}

/// An entity the calls can name: a new one where spawning is allowed, else
/// the first with a transform, else (where the world is refused) any.
///
/// # Safety
/// `e` is a live engine of this thread.
unsafe fn scope_entity(e: *mut StickShiftEngine, t: &StickShiftTransform) -> StickShiftEntity {
    let mut entity = StickShiftEntity::default();
    if stickshift_world_spawn(e, &transform_value(t), 1, &mut entity) == STICKSHIFT_OK {
        return entity;
    }
    let term = StickShiftQueryTerm {
        component: StickShiftComponent {
            id: STICKSHIFT_COMPONENT_TRANSFORM,
        },
        access: STICKSHIFT_QUERY_READ,
    };
    let mut rows: StickShiftQueryRows = std::mem::zeroed();
    rows.size = std::mem::size_of::<StickShiftQueryRows>() as u32;
    if stickshift_world_query(e, &term, 1, &mut rows) == STICKSHIFT_OK && rows.count > 0 {
        let first = *rows.entities;
        stickshift_world_query_release(e, rows.token);
        return first;
    }
    StickShiftEntity { bits: 1 << 32 }
}

/// Every call in [`CALLS`], in order, with what it returned. The render hook
/// passes its open frame; elsewhere `frame` is NULL. Between engine calls
/// (`idle`) destroy is not made, and a frame the acquire opens is discarded.
fn engine_calls(
    e: *mut StickShiftEngine,
    frame: *mut StickShiftFrame,
) -> Vec<(&'static str, StickShiftResult)> {
    engine_calls_in(e, frame, false)
}

fn engine_calls_in(
    e: *mut StickShiftEngine,
    frame: *mut StickShiftFrame,
    idle: bool,
) -> Vec<(&'static str, StickShiftResult)> {
    let tick = StickShiftTickDesc {
        size: std::mem::size_of::<StickShiftTickDesc>() as u32,
        clock: STICKSHIFT_CLOCK_FIXED_STEP,
        nanoseconds: 0,
        readback: null_mut(),
        readback_len: 0,
    };
    let v = view(64, 64, 1.0, [0.0; 4]);
    let close = event(STICKSHIFT_HOST_CLOSE);
    let t = StickShiftTransform {
        position: StickShiftVec3::default(),
        rotation: StickShiftQuat {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        },
        scale: StickShiftVec3 {
            x: 1.0,
            y: 1.0,
            z: 1.0,
        },
    };
    let static_tag = StickShiftComponentValue {
        component: StickShiftComponent {
            id: STICKSHIFT_COMPONENT_STATIC_TAG,
        },
        data: null(),
        size: 0,
    };
    let transform = StickShiftComponent {
        id: STICKSHIFT_COMPONENT_TRANSFORM,
    };
    let term = StickShiftQueryTerm {
        component: transform,
        access: STICKSHIFT_QUERY_READ,
    };
    let chase = StickShiftChase {
        distance: 7.5,
        height: 3.0,
        look_at_height: 1.25,
        up: StickShiftVec3 {
            x: 0.0,
            y: 1.0,
            z: 0.0,
        },
        half_life: 0.15,
    };
    let late = c"scope.Late";
    let late = StickShiftComponentDesc {
        name: late.as_ptr(),
        size: 0,
        align: 1,
    };
    let sound_path = c"scope/sound.ogg";
    let zero = StickShiftVec3::default();
    let mut state = StickShiftEngineState {
        size: std::mem::size_of::<StickShiftEngineState>() as u32,
        ..Default::default()
    };
    let mut perf = StickShiftPerf {
        size: std::mem::size_of::<StickShiftPerf>() as u32,
        ..Default::default()
    };
    let mut camera = StickShiftCameraState {
        size: std::mem::size_of::<StickShiftCameraState>() as u32,
        ..Default::default()
    };
    let mut input = StickShiftInputState {
        size: std::mem::size_of::<StickShiftInputState>() as u32,
        ..Default::default()
    };
    let mut stats = StickShiftAudioStats {
        size: std::mem::size_of::<StickShiftAudioStats>() as u32,
        ..Default::default()
    };
    let mut graphics = StickShiftGraphicsState {
        size: std::mem::size_of::<StickShiftGraphicsState>() as u32,
        ..Default::default()
    };
    let mut menu = StickShiftMenu {
        size: std::mem::size_of::<StickShiftMenu>() as u32,
        group_count: 0,
        title: null(),
        groups: null(),
        group_stride: 0,
        reserved: 0,
        revision: 0,
    };
    let mut current = StickShiftGraphicsSettings::default();
    let mut config = StickShiftStreamingConfig::default();
    let mut component = StickShiftComponent::default();
    let mut desc = StickShiftComponentDesc {
        name: null(),
        size: 0,
        align: 0,
    };
    let mut sound = StickShiftSound::default();
    let mut spawned = StickShiftEntity::default();
    let mut got = t;
    let mut rows: StickShiftQueryRows;
    let (mut byte, mut n, mut path, mut offset) = (0u8, 0u32, null(), zero);
    let (mut ctx, mut new_frame, mut requests) = (null_mut(), null_mut(), 0);
    // SAFETY: inside a scope of a live engine, on its thread, with valid
    // locals; every call that a scope refuses checks before it touches them.
    unsafe {
        rows = std::mem::zeroed();
        rows.size = std::mem::size_of::<StickShiftQueryRows>() as u32;
        let entity = scope_entity(e, &t);
        let mut out = vec![
            (
                "engine_get_context",
                stickshift_engine_get_context(e, &mut ctx),
            ),
            (
                "component_register",
                stickshift_component_register(e, &late, &mut component),
            ),
            (
                "component_get_desc",
                stickshift_component_get_desc(e, transform, &mut desc),
            ),
            (
                "world_spawn",
                stickshift_world_spawn(e, &transform_value(&t), 1, &mut spawned),
            ),
            (
                "world_set",
                stickshift_world_set(e, entity, transform, std::ptr::from_ref(&t).cast(), 40),
            ),
            (
                "world_insert",
                stickshift_world_insert(e, entity, &static_tag, 1),
            ),
            (
                "world_remove",
                stickshift_world_remove(e, entity, static_tag.component),
            ),
            (
                "world_get",
                stickshift_world_get(
                    e,
                    entity,
                    transform,
                    std::ptr::from_mut(&mut got).cast(),
                    40,
                ),
            ),
            (
                "world_has",
                stickshift_world_has(e, entity, transform, &mut byte),
            ),
            (
                "world_contains",
                stickshift_world_contains(e, entity, &mut byte),
            ),
            ("world_len", stickshift_world_len(e, &mut n)),
            (
                "world_query",
                stickshift_world_query(e, &term, 1, &mut rows),
            ),
            (
                "world_query_release",
                stickshift_world_query_release(e, rows.token),
            ),
            (
                "world_query_count",
                stickshift_world_query_count(e, &term, 1, &mut n),
            ),
            (
                "scene_get_config",
                stickshift_scene_get_config(e, &mut config),
            ),
            // The configuration in effect again (zero where it was refused).
            ("scene_set_config", stickshift_scene_set_config(e, &config)),
            (
                "scene_streamed_count",
                stickshift_scene_streamed_count(e, &mut n),
            ),
            (
                "scene_stream",
                stickshift_scene_stream(e, zero, None, null_mut()),
            ),
            (
                "scene_maybe_rebase",
                stickshift_scene_maybe_rebase(e, zero, &mut offset),
            ),
            ("stream_track", stickshift_stream_track(e, entity)),
            ("camera_get", stickshift_camera_get(e, &mut camera)),
            (
                "camera_set_position",
                stickshift_camera_set_position(e, camera.position),
            ),
            (
                "camera_set_target",
                stickshift_camera_set_target(e, camera.target),
            ),
            ("camera_set_up", stickshift_camera_set_up(e, camera.up)),
            (
                "camera_set_aspect_ratio",
                stickshift_camera_set_aspect_ratio(e, 1.5),
            ),
            ("camera_set_fov_y", stickshift_camera_set_fov_y(e, 0.9)),
            (
                "camera_set_clip_planes",
                stickshift_camera_set_clip_planes(e, 0.2, 300.0),
            ),
            (
                "camera_follow",
                stickshift_camera_follow(e, &chase, zero, zero, FIXED_DT),
            ),
            ("input_get", stickshift_input_get(e, &mut input)),
            (
                "audio_load",
                stickshift_audio_load(e, sound_path.as_ptr(), &mut sound),
            ),
            ("audio_play_once", stickshift_audio_play_once(e, sound, 0.0)),
            (
                "audio_play_looping",
                stickshift_audio_play_looping(e, sound, 0.0),
            ),
            ("audio_stop_looping", stickshift_audio_stop_looping(e)),
            (
                "audio_set_master_volume",
                stickshift_audio_set_master_volume(e, 0.0),
            ),
            ("audio_get_stats", stickshift_audio_get_stats(e, &mut stats)),
            (
                "audio_one_shots_of",
                stickshift_audio_one_shots_of(e, sound, &mut n),
            ),
            (
                "audio_get_path",
                stickshift_audio_get_path(e, sound, &mut path),
            ),
            (
                "graphics_get_current",
                stickshift_graphics_get_current(e, &mut current),
            ),
            // The settings in effect again: queues nothing that changes.
            ("graphics_request", stickshift_graphics_request(e, &current)),
            (
                "graphics_get_state",
                stickshift_graphics_get_state(e, &mut graphics),
            ),
            (
                "engine_get_menu",
                stickshift_engine_get_menu(e, 0, &mut menu),
            ),
            (
                "engine_get_state",
                stickshift_engine_get_state(e, &mut state),
            ),
            ("engine_get_perf", stickshift_engine_get_perf(e, &mut perf)),
            ("engine_request_exit", stickshift_engine_request_exit(e)),
            ("engine_reset_clock", stickshift_engine_reset_clock(e)),
            ("engine_init", stickshift_engine_init(e)),
            (
                "engine_resume",
                stickshift_engine_resume(e, &v, &mut requests),
            ),
            (
                "engine_push_events",
                stickshift_engine_push_events(e, &close, 1, 64, &mut requests),
            ),
            ("engine_tick", stickshift_engine_tick(e, &tick, null_mut())),
            ("context_destroy", stickshift_context_destroy(ctx)),
            (
                "frame_acquire",
                stickshift_frame_acquire(ctx, &mut new_frame),
            ),
        ];
        if !new_frame.is_null() {
            // Between calls the acquire opened a frame: close it again.
            stickshift_frame_discard(new_frame);
        }
        out.extend([
            (
                "frame_submit",
                stickshift_frame_submit(frame, null(), null_mut()),
            ),
            ("frame_discard", stickshift_frame_discard(frame)),
            ("context_get_last_frame", {
                let mut record: StickShiftFrameRecord = std::mem::zeroed();
                record.size = std::mem::size_of::<StickShiftFrameRecord>() as u32;
                stickshift_context_get_last_frame(ctx, &mut record)
            }),
        ]);
        if !idle {
            out.push(("engine_destroy", stickshift_engine_destroy(e)));
        }
        out.push((
            "world_despawn",
            stickshift_world_despawn(e, spawned, null_mut()),
        ));
        out
    }
}

/// What every call in [`CALLS`] should return in `scope`.
fn expected_calls(scope: Scope) -> Vec<(&'static str, StickShiftResult)> {
    CALLS
        .iter()
        .filter(|(name, _)| !(scope == Scope::Idle && *name == "engine_destroy"))
        .map(|&(name, rule)| (name, expected(rule, scope)))
        .collect()
}

/// The spawn callback's view of every call, behind its user pointer.
unsafe extern "C" fn calls_from_spawn(
    user: *mut c_void,
    engine: *mut StickShiftEngine,
    _slot: i64,
    _position: StickShiftVec3,
) {
    let calls = &mut *user.cast::<Vec<(&'static str, StickShiftResult)>>();
    if calls.is_empty() {
        *calls = engine_calls(engine, null_mut());
    }
}

/// A load completion's view of the engine calls, behind its user pointer.
struct CompletionProbe {
    engine: *mut StickShiftEngine,
    calls: Vec<(&'static str, StickShiftResult)>,
}

unsafe extern "C" fn calls_from_completion(user: *mut c_void, _: *const StickShiftEvent) {
    let probe = &mut *user.cast::<CompletionProbe>();
    probe.calls = engine_calls(probe.engine, null_mut());
}

/// The header with its comments removed, so a parenthesis in a doc comment
/// cannot hide the declaration after it.
fn without_comments(header: &str) -> String {
    let mut code = String::with_capacity(header.len());
    let mut rest = header;
    while let Some(start) = rest.find("/*") {
        code.push_str(&rest[..start]);
        let end = rest[start..].find("*/").expect("every comment is closed");
        rest = &rest[start + end + 2..];
    }
    code.push_str(rest);
    code.lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every function in `header` whose first parameter is an engine, without
/// the `stickshift_` prefix (so not `engine_create`, which makes one).
fn engine_functions(header: &str) -> Vec<String> {
    let mut names = Vec::new();
    for declaration in without_comments(header).split(';') {
        let Some(open) = declaration.find('(') else {
            continue;
        };
        let (head, params) = declaration.split_at(open);
        let Some(name) = head
            .split_whitespace()
            .last()
            .and_then(|n| n.trim_start_matches('*').strip_prefix("stickshift_"))
        else {
            continue;
        };
        let first = params[1..].split(',').next().unwrap_or("");
        if first.contains("StickShiftEngine *") {
            names.push(name.to_owned());
        }
    }
    names
}

/// The generated header's engine functions ([`engine_functions`]).
fn header_engine_functions() -> Vec<String> {
    let header =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/include/stickshift.h"))
            .expect("the generated header");
    let names = engine_functions(&header);
    // Counted another way, so a parser that drops a declaration fails here:
    // every `(` that opens a parameter list on an engine, comments removed.
    let code = without_comments(&header);
    let opens = code.matches("(struct StickShiftEngine *").count()
        + code.matches("(const struct StickShiftEngine *").count();
    assert_eq!(names.len(), opens, "{names:?}");
    names
}

#[test]
fn the_header_parser_reads_past_a_parenthesis_in_a_comment() {
    let header = "/**\n * Run it (once); see stickshift_a(engine).\n */\n\
                  StickShiftResult stickshift_engine_init(struct StickShiftEngine *engine);\n\
                  // stickshift_b(struct StickShiftEngine *engine);\n\
                  StickShiftResult stickshift_engine_create(struct StickShiftContext *ctx,\n\
                  \x20   struct StickShiftEngine **out);\n\
                  /* (x) */ StickShiftResult stickshift_engine_get_state(const struct \
                  StickShiftEngine *engine, struct StickShiftEngineState *out);\n";
    assert_eq!(
        engine_functions(header),
        ["engine_init", "engine_get_state"]
    );
}

#[test]
fn the_scope_matrix_names_every_engine_function() {
    let header = header_engine_functions();
    // Each of these has a parenthesis in its doc comment.
    for name in [
        "engine_init",
        "world_remove",
        "world_query_release",
        "audio_play_once",
        "audio_play_looping",
    ] {
        assert!(header.iter().any(|h| h == name), "{name} in {header:?}");
    }
    let named: Vec<&str> = CALLS.iter().map(|(n, _)| *n).collect();
    for name in &header {
        assert!(
            named.contains(&name.as_str()),
            "the scope matrix does not cover stickshift_{name}"
        );
    }
    let mut unique = named.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), named.len(), "a call is named twice");
}

#[test]
fn the_scope_matrix() {
    // Init starts a load whose completion a later tick's housekeeping
    // delivers, inside the tick but outside every hook.
    let completion = Box::into_raw(Box::new(CompletionProbe {
        engine: null_mut(),
        calls: Vec::new(),
    }));
    let in_spawn = Rc::new(RefCell::new(Vec::new()));
    let spawn_calls = Rc::clone(&in_spawn);
    let mut h = Harness::new(move |p, at, e, frame| {
        if p.seen.iter().any(|s| s.starts_with(&format!("{at:?} "))) {
            return;
        }
        for (name, result) in engine_calls(e, frame) {
            p.seen.push(format!("{at:?} {name} {result}"));
        }
        // SAFETY: inside a hook of a live engine, on its thread; the
        // completion probe outlives the engine, and the spawn callback's
        // vector outlives the pass.
        unsafe {
            if at == At::Update {
                let mut calls: Vec<(&'static str, StickShiftResult)> = Vec::new();
                let focus = StickShiftVec3 {
                    x: 0.0,
                    y: 0.0,
                    z: -600.0,
                };
                let user = std::ptr::from_mut(&mut calls).cast();
                let r = stickshift_scene_stream(e, focus, Some(calls_from_spawn), user);
                p.seen.push(format!("Pass {r}"));
                *spawn_calls.borrow_mut() = calls;
            }
            if at == At::Init {
                (*completion).engine = e;
                let mut ctx = null_mut();
                stickshift_engine_get_context(e, &mut ctx);
                let callback = StickShiftCompletion {
                    callback: Some(calls_from_completion),
                    user: completion.cast(),
                };
                let cube = StickShiftData {
                    ptr: CUBE.as_ptr().cast(),
                    len: CUBE.len(),
                    release: None,
                    user: null_mut(),
                };
                let mut model = StickShiftModel::default();
                let r = stickshift_model_load_memory(ctx, cube, &callback, &mut model);
                p.seen.push(format!("load {r}"));
            }
        }
    });
    assert_eq!(h.init(), STICKSHIFT_OK);
    h.tick();
    // A menu choice after init runs the settings hook inside the push.
    assert_eq!(h.push(&[menu(102)]).0, STICKSHIFT_OK);
    // SAFETY: the callback has not run, or ran inside an earlier tick.
    let delivered = || unsafe { !(*completion).calls.is_empty() };
    for _ in 0..10_000 {
        if delivered() {
            break;
        }
        assert_eq!(h.tick().rendered, 1);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(delivered(), "the load's completion never arrived");
    let idle = engine_calls_in(h.engine, null_mut(), true);
    // Every refusal, and every call between engine calls, left the engine
    // working.
    let state = h.state();
    assert_eq!((state.poisoned, state.failed), (0, 0));
    assert_eq!(h.tick().rendered, 1);

    // The release hook, which destroy runs.
    h.probe.calls_in_release = Some(h.engine);
    // SAFETY: a live engine, between calls.
    assert_eq!(
        unsafe { stickshift_engine_destroy(h.engine) },
        STICKSHIFT_OK
    );
    h.engine = null_mut();
    assert_eq!(h.probe.releases, 1);

    let in_scope = |scope: &str| -> Vec<(String, StickShiftResult)> {
        h.probe
            .seen
            .iter()
            .filter_map(|s| s.strip_prefix(&format!("{scope} ")))
            .filter_map(|s| s.rsplit_once(' '))
            .map(|(name, code)| (name.to_owned(), code.parse().expect("a result code")))
            .collect()
    };
    let owned = |calls: &[(&'static str, StickShiftResult)]| -> Vec<(String, StickShiftResult)> {
        calls.iter().map(|&(n, r)| (n.to_owned(), r)).collect()
    };
    for at in [At::Init, At::Update, At::Render, At::Settings] {
        assert_eq!(
            in_scope(&format!("{at:?}")),
            owned(&expected_calls(Scope::Hook(at))),
            "{at:?}"
        );
    }
    assert!(
        h.probe.seen.contains(&"Pass 0".to_owned()),
        "{:?}",
        h.probe.seen
    );
    assert_eq!(*in_spawn.borrow(), expected_calls(Scope::Spawn), "Spawn");
    // SAFETY: the callback has returned, and the engine is gone.
    assert_eq!(
        unsafe { &(*completion).calls },
        &expected_calls(Scope::Completion),
        "Completion"
    );
    assert_eq!(
        in_scope("Release"),
        owned(&expected_calls(Scope::Release)),
        "Release"
    );
    assert_eq!(idle, expected_calls(Scope::Idle), "Idle");
    // SAFETY: allocated above; the engine that could call back is gone.
    drop(unsafe { Box::from_raw(completion) });
}

#[test]
fn a_panic_in_a_call_from_a_hook_poisons_the_rest_of_the_tick() {
    let dt = STICKSHIFT_FIXED_DT_NS;
    // Without the panic, the tick runs two updates and a render.
    let control = Harness::new(|_, _, _, _| {});
    assert_eq!(control.init(), STICKSHIFT_OK);
    let (r, out) = control.tick_with(STICKSHIFT_CLOCK_ELAPSED, 2 * dt);
    assert_eq!((r, out.steps, out.rendered), (STICKSHIFT_OK, 2, 1));

    let mut h = Harness::new(|p, at, e, _| {
        if at == At::Update && p.updates == 1 {
            // SAFETY: inside an update hook, on the engine's thread.
            let r = unsafe { testing::panic_in_call(e) };
            p.seen.push(format!("panic_in_call {r}"));
            p.seen.push(format!(
                "get_state {:?}",
                state_of(e).map(|s| (s.poisoned, s.updates))
            ));
            // Every other call is refused now.
            let mut perf = StickShiftPerf {
                size: std::mem::size_of::<StickShiftPerf>() as u32,
                ..Default::default()
            };
            // SAFETY: inside an update hook, on the engine's thread.
            let r = unsafe { stickshift_engine_get_perf(e, &mut perf) };
            p.seen.push(format!("get_perf {r}"));
        }
    });
    assert_eq!(h.init(), STICKSHIFT_OK);
    let (r, _) = h.tick_with(STICKSHIFT_CLOCK_ELAPSED, 2 * dt);
    assert_eq!(r, STICKSHIFT_ERROR_INTERNAL);
    assert!(
        last_error().contains("during this tick"),
        "{}",
        last_error()
    );
    assert_eq!(
        h.probe.seen,
        [
            format!("panic_in_call {STICKSHIFT_ERROR_INTERNAL}"),
            "get_state Ok((1, 1))".to_owned(),
            format!("get_perf {STICKSHIFT_ERROR_INTERNAL}"),
        ]
    );
    // The second update and the render hook never ran.
    assert_eq!((h.probe.updates, h.probe.renders), (1, 0));
    // The host learns why its calls fail.
    let state = state_of(h.engine).expect("a poisoned engine still answers get_state");
    assert_eq!(
        (
            state.poisoned,
            state.failed,
            state.initialized,
            state.updates
        ),
        (1, 0, 1, 1)
    );
    assert_eq!(
        h.tick_with(STICKSHIFT_CLOCK_FIXED_STEP, 0).0,
        STICKSHIFT_ERROR_INTERNAL
    );
    // SAFETY: a live engine.
    assert_eq!(
        unsafe { stickshift_engine_destroy(h.engine) },
        STICKSHIFT_OK
    );
    h.engine = null_mut();
    assert_eq!(h.probe.releases, 1);
}

#[test]
fn another_thread_gets_wrong_thread() {
    let h = Harness::new(|_, _, _, _| {});
    assert_eq!(h.init(), STICKSHIFT_OK);
    struct Send2(*mut StickShiftEngine, *mut StickShiftContext);
    // SAFETY: the other thread only makes calls that check the thread first.
    unsafe impl Send for Send2 {}
    let pointers = Send2(h.engine, h.ctx);
    let codes = std::thread::spawn(move || {
        let p = pointers;
        let mut w = 0;
        let mut state = StickShiftEngineState {
            size: std::mem::size_of::<StickShiftEngineState>() as u32,
            ..Default::default()
        };
        // SAFETY: live objects; every call refuses before touching them.
        unsafe {
            [
                stickshift_engine_init(p.0),
                stickshift_engine_get_state(p.0, &mut state),
                stickshift_engine_request_exit(p.0),
                stickshift_engine_destroy(p.0),
                stickshift_context_get_size(p.1, &mut w, &mut w),
                stickshift_mesh_destroy(p.1, StickShiftMesh { id: 1 }),
                stickshift_context_destroy(p.1),
            ]
        }
    })
    .join()
    .unwrap();
    assert_eq!(codes, [STICKSHIFT_ERROR_WRONG_THREAD; 7]);
    assert_eq!(h.tick().rendered, 1);
}

#[test]
fn a_panic_poisons_the_engine_and_release_still_runs_once() {
    let mut h = Harness::new(|_, _, _, _| {});
    assert_eq!(h.init(), STICKSHIFT_OK);
    h.tick();
    // SAFETY: a live engine, between calls.
    unsafe { testing::panic_in_next_update(h.engine) };
    let (r, _) = h.tick_with(STICKSHIFT_CLOCK_FIXED_STEP, 0);
    assert_eq!(r, STICKSHIFT_ERROR_INTERNAL);
    assert!(
        last_error().contains("a panic a test asked for"),
        "{}",
        last_error()
    );
    let state = state_of(h.engine).expect("a poisoned engine still answers get_state");
    assert_eq!(
        (state.poisoned, state.failed, state.exit_requested),
        (1, 0, 0)
    );
    // A size below the first layout is still refused.
    let mut small = StickShiftEngineState {
        size: 4,
        ..Default::default()
    };
    // SAFETY: a live engine; `small` is valid for its declared size.
    assert_eq!(
        unsafe { stickshift_engine_get_state(h.engine, &mut small) },
        STICKSHIFT_ERROR_INVALID_ARGUMENT
    );
    assert_eq!(
        h.tick_with(STICKSHIFT_CLOCK_FIXED_STEP, 0).0,
        STICKSHIFT_ERROR_INTERNAL
    );
    assert_eq!(h.init(), STICKSHIFT_ERROR_INTERNAL);
    // SAFETY: a live engine, between calls.
    assert_eq!(
        unsafe { stickshift_engine_request_exit(h.engine) },
        STICKSHIFT_ERROR_INTERNAL
    );
    // Still poisoned: get_state did not clear it.
    assert_eq!(state_of(h.engine).map(|s| s.poisoned), Ok(1));
    // Every function that takes the engine (from the header) but get_state
    // and destroy is refused.
    let calls = engine_calls_in(h.engine, null_mut(), true);
    let result = |name: &str| calls.iter().find(|(n, _)| *n == name).map(|&(_, r)| r);
    for name in header_engine_functions() {
        let expected = match name.as_str() {
            "engine_get_state" => STICKSHIFT_OK,
            // Made last, below.
            "engine_destroy" => continue,
            _ => STICKSHIFT_ERROR_INTERNAL,
        };
        assert_eq!(result(&name), Some(expected), "stickshift_{name}");
    }
    // The context still works, frames included, and destroy still releases
    // the game.
    let (mut w, mut hgt) = (0, 0);
    let mut frame = null_mut();
    // SAFETY: a live context, between calls.
    unsafe {
        assert_eq!(
            stickshift_context_get_size(h.ctx, &mut w, &mut hgt),
            STICKSHIFT_OK
        );
        assert_eq!(stickshift_frame_acquire(h.ctx, &mut frame), STICKSHIFT_OK);
        assert_eq!(stickshift_frame_discard(frame), STICKSHIFT_OK);
    }
    // SAFETY: a live engine.
    assert_eq!(
        unsafe { stickshift_engine_destroy(h.engine) },
        STICKSHIFT_OK
    );
    h.engine = null_mut();
    assert_eq!(h.probe.releases, 1);
}

#[test]
fn a_failed_init_aborts_and_fails_the_engine() {
    let mut h = Harness::new(|p, at, _, _| {
        if at == At::Init {
            p.init_result = STICKSHIFT_ERROR_IO;
        }
    });
    assert_eq!(h.init(), STICKSHIFT_ERROR_ABORTED);
    assert!(last_error().contains("returned -7"), "{}", last_error());
    assert_eq!(h.init(), STICKSHIFT_ERROR_WRONG_STATE);
    assert_eq!(
        h.tick_with(STICKSHIFT_CLOCK_FIXED_STEP, 0).0,
        STICKSHIFT_ERROR_WRONG_STATE
    );
    let v = view(64, 64, 1.0, [0.0; 4]);
    // SAFETY: a live engine.
    assert_eq!(
        unsafe { stickshift_engine_resume(h.engine, &v, null_mut()) },
        STICKSHIFT_ERROR_WRONG_STATE
    );
    assert_eq!(h.probe.inits, 1);
    assert_eq!(h.probe.updates, 0);
    let state = h.state();
    assert_eq!((state.failed, state.initialized), (1, 1));
    // SAFETY: a live engine.
    assert_eq!(
        unsafe { stickshift_engine_destroy(h.engine) },
        STICKSHIFT_OK
    );
    h.engine = null_mut();
    assert_eq!(h.probe.releases, 1);

    // Through resume too.
    let h = Harness::new(|p, at, _, _| {
        if at == At::Init {
            p.init_result = -1;
        }
    });
    // SAFETY: a live engine.
    assert_eq!(
        unsafe { stickshift_engine_resume(h.engine, &v, null_mut()) },
        STICKSHIFT_ERROR_ABORTED
    );
}

#[test]
fn ticking_before_init_is_wrong_state() {
    let h = Harness::new(|_, _, _, _| {});
    assert_eq!(
        h.tick_with(STICKSHIFT_CLOCK_FIXED_STEP, 0).0,
        STICKSHIFT_ERROR_WRONG_STATE
    );
    assert_eq!(h.probe.renders, 0);
    // A host frame left open blocks ticking until it is closed.
    assert_eq!(h.init(), STICKSHIFT_OK);
    let mut frame = null_mut();
    // SAFETY: a live context, between engine calls.
    unsafe {
        assert_eq!(stickshift_frame_acquire(h.ctx, &mut frame), STICKSHIFT_OK);
        assert_eq!(
            h.tick_with(STICKSHIFT_CLOCK_FIXED_STEP, 0).0,
            STICKSHIFT_ERROR_WRONG_STATE
        );
        assert_eq!(stickshift_frame_discard(frame), STICKSHIFT_OK);
    }
    assert_eq!(h.tick().rendered, 1);
    // Bad descriptors.
    let mut small = StickShiftTickDesc {
        size: 4,
        clock: 0,
        nanoseconds: 0,
        readback: null_mut(),
        readback_len: 0,
    };
    // SAFETY: valid locals.
    unsafe {
        assert_eq!(
            stickshift_engine_tick(h.engine, &small, null_mut()),
            STICKSHIFT_ERROR_INVALID_ARGUMENT
        );
        small.size = std::mem::size_of::<StickShiftTickDesc>() as u32;
        let mut out = StickShiftTickResult::default();
        assert_eq!(
            stickshift_engine_tick(h.engine, &small, &mut out),
            STICKSHIFT_ERROR_INVALID_ARGUMENT,
            "out.size is 0"
        );
        assert_eq!(
            stickshift_engine_tick(h.engine, null(), null_mut()),
            STICKSHIFT_ERROR_INVALID_ARGUMENT
        );
    }
}

// ---------------------------------------------------------------------------
// Frames
// ---------------------------------------------------------------------------

#[test]
fn the_render_hook_draws_into_a_frame_the_engine_submits() {
    let h = Harness::new(|p, at, _, frame| {
        if at == At::Render {
            // SAFETY: the open frame of the render hook.
            let r = unsafe { stickshift_frame_draw_quad(frame, &quad()) };
            p.seen.push(format!("{r}"));
        }
    });
    assert_eq!(h.init(), STICKSHIFT_OK);
    assert_eq!(h.last_frame().frame_number, 0, "no frame yet");
    for n in 1..=10 {
        let out = h.tick();
        assert_eq!((out.rendered, out.submitted, out.frame), (1, 1, n));
        let record = h.last_frame();
        assert_eq!((record.submitted, record.frame_number), (1, n));
        assert_eq!(quads_of(&record), [quad()]);
    }
    // Engine frames complete internally: no FRAME_COMPLETED piles up.
    let mut events = [StickShiftEvent {
        kind: 0,
        result: 0,
        object: 0,
        frame: 0,
        message: null(),
    }; 16];
    let mut count = 0;
    // SAFETY: a live context.
    unsafe { stickshift_poll_events(h.ctx, events.as_mut_ptr(), 16, &mut count) };
    let kinds: Vec<u32> = events[..count as usize].iter().map(|e| e.kind).collect();
    assert_eq!(kinds, [STICKSHIFT_EVENT_CONTEXT_READY]);
    let mut stats = StickShiftStats::default();
    // SAFETY: a live context.
    unsafe { stickshift_context_get_stats(h.ctx, &mut stats) };
    assert_eq!(
        (
            stats.frames_submitted,
            stats.frames_completed,
            stats.frames_skipped
        ),
        (10, 10, 0)
    );
    assert_eq!(h.probe.seen, ["0"; 10]);
}

#[test]
fn with_no_free_slot_skip_discards_the_frame_but_still_renders() {
    let h = Harness::with(
        StickShiftEngineDesc {
            pacing: STICKSHIFT_FRAME_PACING_SKIP,
            ..engine_desc()
        },
        (64, 64),
        |_, at, _, frame| {
            if at == At::Render {
                // SAFETY: the open frame of the render hook.
                unsafe { stickshift_frame_draw_quad(frame, &quad()) };
            }
        },
    );
    assert_eq!(h.init(), STICKSHIFT_OK);
    // SAFETY: a live null context.
    assert_eq!(
        unsafe { testing::hold_null_frame_slots(h.ctx, 3) },
        STICKSHIFT_OK
    );
    let out = h.tick();
    assert_eq!(
        (out.steps, out.rendered, out.submitted, out.frame),
        (1, 1, 0, 0)
    );
    let record = h.last_frame();
    assert_eq!(record.submitted, 0);
    assert_eq!(
        quads_of(&record).len(),
        1,
        "the discarded frame was recorded"
    );
    assert_eq!(h.state().frames_discarded, 1);
    // SAFETY: as above.
    assert_eq!(
        unsafe { testing::hold_null_frame_slots(h.ctx, 0) },
        STICKSHIFT_OK
    );
    let out = h.tick();
    assert_eq!((out.submitted, out.frame), (1, 1));
    assert_eq!(h.probe.renders, 2);
}

#[test]
fn with_no_free_slot_wait_gives_up_and_discards() {
    let h = Harness::new(|_, _, _, _| {});
    assert_eq!(h.init(), STICKSHIFT_OK);
    // SAFETY: a live null context.
    unsafe { testing::hold_null_frame_slots(h.ctx, 3) };
    let start = std::time::Instant::now();
    let out = h.tick();
    // The null backend never frees a held slot, so its wait returns at once;
    // the frame is discarded, and the render hook still ran.
    assert!(start.elapsed() < std::time::Duration::from_secs(2));
    assert_eq!((out.rendered, out.submitted), (1, 0));
}

#[test]
fn a_tick_readback_fills_the_buffer_before_the_tick_returns() {
    let h = Harness::with(engine_desc(), (8, 4), |_, _, _, _| {});
    assert_eq!(h.init(), STICKSHIFT_OK);
    let mut pixels = vec![0xABu8; 8 * 4 * 4];
    let mut desc = StickShiftTickDesc {
        size: std::mem::size_of::<StickShiftTickDesc>() as u32,
        clock: STICKSHIFT_CLOCK_FIXED_STEP,
        nanoseconds: 0,
        readback: pixels.as_mut_ptr().cast(),
        readback_len: pixels.len(),
    };
    let mut out = StickShiftTickResult {
        size: std::mem::size_of::<StickShiftTickResult>() as u32,
        ..Default::default()
    };
    // SAFETY: valid locals; `pixels` outlives the tick.
    unsafe {
        assert_eq!(
            stickshift_engine_tick(h.engine, &desc, &mut out),
            STICKSHIFT_OK
        );
        assert_eq!((out.submitted, out.readback_ready), (1, 1));
        assert!(
            pixels.iter().all(|&b| b == 0),
            "the null backend reads back zeros"
        );
        desc.readback_len -= 1;
        assert_eq!(
            stickshift_engine_tick(h.engine, &desc, &mut out),
            STICKSHIFT_ERROR_INVALID_ARGUMENT
        );
    }
    // No read-back event: the engine copied it itself.
    let mut events = [StickShiftEvent {
        kind: 0,
        result: 0,
        object: 0,
        frame: 0,
        message: null(),
    }; 4];
    let mut count = 0;
    // SAFETY: a live context.
    unsafe { stickshift_poll_events(h.ctx, events.as_mut_ptr(), 4, &mut count) };
    assert_eq!(count, 1, "only CONTEXT_READY");

    // SKIP pacing cannot read back.
    let skip = Harness::with(
        StickShiftEngineDesc {
            pacing: STICKSHIFT_FRAME_PACING_SKIP,
            ..engine_desc()
        },
        (8, 4),
        |_, _, _, _| {},
    );
    assert_eq!(skip.init(), STICKSHIFT_OK);
    desc.readback_len += 1;
    // SAFETY: as above.
    assert_eq!(
        unsafe { stickshift_engine_tick(skip.engine, &desc, &mut out) },
        STICKSHIFT_ERROR_UNSUPPORTED
    );
}

#[test]
fn the_last_frame_records_mesh_draws_and_quads_after_the_liveness_check() {
    let mut ctx = null_mut();
    // SAFETY: valid locals; a context with no engine.
    unsafe {
        assert_eq!(
            stickshift_context_create(&null_desc(16, 16), &mut ctx),
            STICKSHIFT_OK
        );
        let triangle: [f32; 27] = [
            0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, //
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, //
            0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0,
        ];
        let indices: [u32; 3] = [0, 1, 2];
        let data = |p: *const c_void, len| StickShiftData {
            ptr: p,
            len,
            release: None,
            user: null_mut(),
        };
        let desc = StickShiftMeshDesc {
            layout: STICKSHIFT_VERTEX_LAYOUT_POS_NORMAL_COLOR,
            vertices: data(triangle.as_ptr().cast(), std::mem::size_of_val(&triangle)),
            indices: data(indices.as_ptr().cast(), std::mem::size_of_val(&indices)),
        };
        let (mut kept, mut gone) = (StickShiftMesh::default(), StickShiftMesh::default());
        assert_eq!(
            stickshift_mesh_create(ctx, &desc, null(), &mut kept),
            STICKSHIFT_OK
        );
        assert_eq!(
            stickshift_mesh_create(ctx, &desc, null(), &mut gone),
            STICKSHIFT_OK
        );
        let model = Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0));
        let model_c: StickShiftMat4 = model.into();
        let material = StickShiftMaterial {
            base_color: StickShiftColor {
                r: 0.5,
                g: 0.25,
                b: 1.0,
                a: 1.0,
            },
            base_color_texture: StickShiftTexture { id: 0 },
            emissive: [0.1, 0.2, 0.3],
        };
        let sun = StickShiftSunSky {
            sun_intensity: 2.5,
            ..Default::default()
        };
        assert_eq!(stickshift_context_set_sun_sky(ctx, &sun), STICKSHIFT_OK);
        let mut frame = null_mut();
        assert_eq!(stickshift_frame_acquire(ctx, &mut frame), STICKSHIFT_OK);
        assert_eq!(
            stickshift_frame_draw_mesh(frame, gone, &model_c, &material),
            STICKSHIFT_OK
        );
        assert_eq!(
            stickshift_frame_draw_mesh(frame, kept, &model_c, &material),
            STICKSHIFT_OK
        );
        assert_eq!(stickshift_frame_draw_quad(frame, &quad()), STICKSHIFT_OK);
        assert_eq!(stickshift_mesh_destroy(ctx, gone), STICKSHIFT_OK);
        let mut number = 0;
        assert_eq!(
            stickshift_frame_submit(frame, null(), &mut number),
            STICKSHIFT_OK
        );
        let record = last_frame_of(ctx);
        assert_eq!((record.submitted, record.frame_number), (1, number));
        let draws = draws_of(&record);
        assert_eq!(draws.len(), 1, "the destroyed mesh's draw was dropped");
        let d = draws[0];
        assert_eq!((d.source, d.texture, d.part), (kept.id, 0, 0));
        assert_eq!(d.layout, STICKSHIFT_VERTEX_LAYOUT_POS_NORMAL_COLOR);
        assert_eq!(d.model.into_mat4(), model);
        assert_eq!(d.base_color, material.base_color);
        assert_eq!(d.emissive, material.emissive);
        assert_eq!(quads_of(&record), [quad()]);
        assert_eq!(record.sun.sun_intensity, 2.5);
        assert_eq!(record.light_count, 0);
        assert_eq!(
            record.draw_stride as usize,
            std::mem::size_of::<StickShiftDrawRecord>()
        );

        // A smaller size is refused; nothing is written.
        let mut small: StickShiftFrameRecord = std::mem::zeroed();
        small.size = 8;
        assert_eq!(
            stickshift_context_get_last_frame(ctx, &mut small),
            STICKSHIFT_ERROR_INVALID_ARGUMENT
        );
        assert_eq!(small.frame_number, 0);
        assert_eq!(stickshift_context_destroy(ctx), STICKSHIFT_OK);
    }
}

// ---------------------------------------------------------------------------
// Loads: the hold and model_wait
// ---------------------------------------------------------------------------

const CUBE: &[u8] = include_bytes!("../../stickshift-assets/tests/fixtures/cube.gltf");

/// A named pipe the loader blocks on until the test writes the model into it.
struct Fifo {
    path: std::path::PathBuf,
}

impl Fifo {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "stickshift-engine-{name}-{}.gltf",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let status = std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .expect("mkfifo");
        assert!(status.success());
        Self { path }
    }

    fn c_path(&self) -> std::ffi::CString {
        std::ffi::CString::new(self.path.to_str().unwrap()).unwrap()
    }

    /// Write the model, unblocking the loader.
    fn feed(&self, bytes: &'static [u8]) -> std::thread::JoinHandle<()> {
        let path = self.path.clone();
        std::thread::spawn(move || {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
            f.write_all(bytes).unwrap();
        })
    }
}

impl Drop for Fifo {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[test]
fn hold_until_loaded_holds_then_banks_one_step() {
    let first = Fifo::new("hold");
    let second = Fifo::new("hold-again");
    let path = first.c_path();
    let h = Harness::with(
        StickShiftEngineDesc {
            flags: STICKSHIFT_ENGINE_HOLD_UNTIL_LOADED,
            ..engine_desc()
        },
        (64, 64),
        move |p, at, e, _| {
            if at == At::Init {
                let mut ctx = null_mut();
                let mut model = StickShiftModel::default();
                // SAFETY: inside init, on the engine's thread.
                let r = unsafe {
                    stickshift_engine_get_context(e, &mut ctx);
                    stickshift_model_load_file(ctx, path.as_ptr(), null(), &mut model)
                };
                p.seen.push(format!("load {r}"));
            }
        },
    );
    assert_eq!(h.init(), STICKSHIFT_OK);
    assert_eq!(h.probe.seen, ["load 0"]);
    let dt = STICKSHIFT_FIXED_DT_NS;
    let t0 = 1_000_000_000;
    let held = |at: u64| {
        let (r, out) = h.tick_with(STICKSHIFT_CLOCK_TIMESTAMP, at);
        assert_eq!(r, STICKSHIFT_NOT_READY);
        assert_eq!((out.held, out.steps, out.rendered), (1, 0, 0));
        assert_eq!(h.state().holding, 1);
    };
    // The load init started holds the first ticks.
    for i in 0..3 {
        held(t0 + i * 100 * dt);
    }
    assert_eq!((h.probe.updates, h.probe.renders), (0, 0));
    first.feed(CUBE).join().unwrap();
    // SAFETY: a live context, between engine calls.
    assert_eq!(unsafe { stickshift_context_flush(h.ctx) }, STICKSHIFT_OK);
    let (r, out) = h.tick_with(STICKSHIFT_CLOCK_TIMESTAMP, t0 + 1000 * dt);
    assert_eq!(r, STICKSHIFT_OK);
    assert_eq!((out.held, out.steps, out.rendered), (0, 1, 1));
    assert_eq!(h.state().holding, 0);

    // A load the host starts between ticks holds too, and the hold resets the
    // clock: long after the last timestamp, the first tick after the hold
    // banks one step, not the capped five the gap would give.
    let mut model = StickShiftModel::default();
    let path = second.c_path();
    // SAFETY: a live context, between engine calls.
    assert_eq!(
        unsafe { stickshift_model_load_file(h.ctx, path.as_ptr(), null(), &mut model) },
        STICKSHIFT_OK
    );
    held(t0 + 1100 * dt);
    second.feed(CUBE).join().unwrap();
    // SAFETY: as above.
    assert_eq!(unsafe { stickshift_context_flush(h.ctx) }, STICKSHIFT_OK);
    let (r, out) = h.tick_with(STICKSHIFT_CLOCK_TIMESTAMP, t0 + 2000 * dt);
    assert_eq!(r, STICKSHIFT_OK);
    assert_eq!((out.held, out.steps, out.rendered), (0, 1, 1));
    assert_eq!((h.probe.updates, h.probe.renders), (2, 2));
}

#[test]
fn model_wait_waits_for_ready_failed_or_the_timeout() {
    let mut ctx = null_mut();
    let fifo = Fifo::new("wait");
    // SAFETY: valid locals throughout.
    unsafe {
        assert_eq!(
            stickshift_context_create(&null_desc(8, 8), &mut ctx),
            STICKSHIFT_OK
        );
        let bytes = |b: &'static [u8]| StickShiftData {
            ptr: b.as_ptr().cast(),
            len: b.len(),
            release: None,
            user: null_mut(),
        };
        let mut model = StickShiftModel::default();
        assert_eq!(
            stickshift_model_load_memory(ctx, bytes(CUBE), null(), &mut model),
            STICKSHIFT_OK
        );
        assert_eq!(
            stickshift_model_wait(ctx, model, 10_000_000_000),
            STICKSHIFT_OK
        );
        assert_eq!(
            stickshift_resource_state(ctx, model.id),
            STICKSHIFT_RESOURCE_STATE_READY
        );
        assert_eq!(
            stickshift_model_wait(ctx, model, 0),
            STICKSHIFT_OK,
            "ready stays ready"
        );

        let mut broken = StickShiftModel::default();
        assert_eq!(
            stickshift_model_load_memory(ctx, bytes(b"{ not gltf"), null(), &mut broken),
            STICKSHIFT_OK
        );
        assert_eq!(
            stickshift_model_wait(ctx, broken, 10_000_000_000),
            STICKSHIFT_ERROR_PARSE
        );
        assert!(!last_error().is_empty());

        let mut blocked = StickShiftModel::default();
        let path = fifo.c_path();
        assert_eq!(
            stickshift_model_load_file(ctx, path.as_ptr(), null(), &mut blocked),
            STICKSHIFT_OK
        );
        assert_eq!(
            stickshift_model_wait(ctx, blocked, 20_000_000),
            STICKSHIFT_ERROR_TIMEOUT
        );
        assert_eq!(
            stickshift_resource_state(ctx, blocked.id),
            STICKSHIFT_RESOURCE_STATE_PENDING
        );
        fifo.feed(CUBE).join().unwrap();
        // The importer reads a pipe as it can (the load may fail on it); either
        // way the wait now ends with the load's outcome.
        let outcome = stickshift_model_wait(ctx, blocked, 10_000_000_000);
        assert_ne!(outcome, STICKSHIFT_ERROR_TIMEOUT);
        assert_ne!(
            stickshift_resource_state(ctx, blocked.id),
            STICKSHIFT_RESOURCE_STATE_PENDING
        );
        assert_eq!(
            stickshift_model_wait(ctx, StickShiftModel { id: 12345 }, 0),
            STICKSHIFT_ERROR_INVALID_HANDLE
        );
        assert_eq!(stickshift_context_destroy(ctx), STICKSHIFT_OK);
    }
}

// ---------------------------------------------------------------------------
// Context additions
// ---------------------------------------------------------------------------

#[test]
fn the_sticky_camera_sun_scale_and_safe_area_round_trip() {
    let mut ctx = null_mut();
    // SAFETY: valid locals throughout.
    unsafe {
        let desc = StickShiftContextDesc {
            scale: 2.0,
            ..null_desc(8, 8)
        };
        assert_eq!(stickshift_context_create(&desc, &mut ctx), STICKSHIFT_OK);
        let mut scale = 0.0;
        assert_eq!(stickshift_context_get_scale(ctx, &mut scale), STICKSHIFT_OK);
        assert_eq!(scale, 2.0);
        let camera = StickShiftCamera {
            view_projection: Mat4::from_scale(Vec3::splat(3.0)).into(),
            position: StickShiftVec3 {
                x: 1.0,
                y: 2.0,
                z: 3.0,
            },
        };
        assert_eq!(stickshift_context_set_camera(ctx, &camera), STICKSHIFT_OK);
        let mut got = StickShiftCamera::default();
        assert_eq!(stickshift_context_get_camera(ctx, &mut got), STICKSHIFT_OK);
        assert_eq!(got, camera);
        let mut sun = StickShiftSunSky::default();
        stickshift_sun_sky_default(&mut sun);
        let mut got_sun = StickShiftSunSky::default();
        assert_eq!(
            stickshift_context_get_sun_sky(ctx, &mut got_sun),
            STICKSHIFT_OK
        );
        assert_eq!(got_sun, sun, "the default sun until one is set");
        sun.sun_elevation_deg = 12.0;
        assert_eq!(stickshift_context_set_sun_sky(ctx, &sun), STICKSHIFT_OK);
        assert_eq!(
            stickshift_context_get_sun_sky(ctx, &mut got_sun),
            STICKSHIFT_OK
        );
        assert_eq!(got_sun, sun);

        let mut insets = [9.0; 4];
        assert_eq!(
            stickshift_context_get_safe_area(ctx, insets.as_mut_ptr()),
            STICKSHIFT_OK
        );
        assert_eq!(insets, [0.0; 4]);
        let set = [1.0, 2.0, 3.0, 4.0];
        assert_eq!(
            stickshift_context_set_safe_area(ctx, set.as_ptr()),
            STICKSHIFT_OK
        );
        assert_eq!(
            stickshift_context_get_safe_area(ctx, insets.as_mut_ptr()),
            STICKSHIFT_OK
        );
        assert_eq!(insets, set);
        for bad in [[f32::NAN, 0.0, 0.0, 0.0], [0.0, -1.0, 0.0, 0.0]] {
            assert_eq!(
                stickshift_context_set_safe_area(ctx, bad.as_ptr()),
                STICKSHIFT_ERROR_INVALID_ARGUMENT
            );
        }
        assert_eq!(
            stickshift_context_set_safe_area(ctx, null()),
            STICKSHIFT_ERROR_INVALID_ARGUMENT
        );
        // The camera set outside a frame is the one a frame uses.
        let mut frame = null_mut();
        assert_eq!(stickshift_frame_acquire(ctx, &mut frame), STICKSHIFT_OK);
        assert_eq!(
            stickshift_frame_submit(frame, null(), null_mut()),
            STICKSHIFT_OK
        );
        assert_eq!(last_frame_of(ctx).camera, camera);
        assert_eq!(stickshift_context_destroy(ctx), STICKSHIFT_OK);
    }
}

#[test]
fn perf_and_state_report_from_hooks_and_between_calls() {
    let h = Harness::new(|p, at, e, _| {
        if at == At::Render {
            let mut perf = StickShiftPerf {
                size: std::mem::size_of::<StickShiftPerf>() as u32,
                ..Default::default()
            };
            // SAFETY: inside a hook.
            let r = unsafe { stickshift_engine_get_perf(e, &mut perf) };
            p.seen.push(format!("{r} {}", perf.alpha));
        }
    });
    assert_eq!(h.init(), STICKSHIFT_OK);
    h.tick_with(
        STICKSHIFT_CLOCK_ELAPSED,
        STICKSHIFT_FIXED_DT_NS + STICKSHIFT_FIXED_DT_NS / 2,
    );
    assert_eq!(h.probe.seen.len(), 1);
    let alpha: f32 = h.probe.seen[0].split(' ').nth(1).unwrap().parse().unwrap();
    assert!((alpha - 0.5).abs() < 0.01, "{alpha}");
    let mut perf = StickShiftPerf {
        size: std::mem::size_of::<StickShiftPerf>() as u32,
        ..Default::default()
    };
    // SAFETY: between calls.
    assert_eq!(
        unsafe { stickshift_engine_get_perf(h.engine, &mut perf) },
        STICKSHIFT_OK
    );
    assert_eq!((perf.alpha, perf.frame_count), (0.0, 1));
    assert_eq!(perf.size as usize, std::mem::size_of::<StickShiftPerf>());
}

// ---------------------------------------------------------------------------
// Metal
// ---------------------------------------------------------------------------

/// A Metal offscreen context, or `None` (reported as skipped) without a Metal
/// device; under `STICKSHIFT_REQUIRE_GPU=1` a missing device fails instead.
#[cfg(target_os = "macos")]
fn metal_context(width: u32, height: u32, test: &str) -> Option<*mut StickShiftContext> {
    use std::io::Write;
    let desc = StickShiftContextDesc {
        backend: STICKSHIFT_BACKEND_METAL,
        ..null_desc(width, height)
    };
    let mut ctx = null_mut();
    // SAFETY: valid locals.
    match unsafe { stickshift_context_create(&desc, &mut ctx) } {
        STICKSHIFT_OK => Some(ctx),
        STICKSHIFT_ERROR_NO_DEVICE => {
            let required = std::env::var("STICKSHIFT_REQUIRE_GPU")
                .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
            assert!(
                !required,
                "no Metal device, and STICKSHIFT_REQUIRE_GPU is set"
            );
            let _ = writeln!(std::io::stderr(), "SKIPPED {test}: no Metal device");
            None
        }
        other => panic!("stickshift_context_create: {other} ({})", last_error()),
    }
}

#[cfg(target_os = "macos")]
fn fixed_camera() -> StickShiftCamera {
    StickShiftCamera {
        view_projection: (Mat4::perspective_rh(0.9, 2.0, 0.1, 50.0)
            * Mat4::look_at_rh(Vec3::new(0.0, 1.0, 3.0), Vec3::ZERO, Vec3::Y))
        .into(),
        position: StickShiftVec3 {
            x: 0.0,
            y: 1.0,
            z: 3.0,
        },
    }
}

/// What a run of [`gpu_bound_ticks`] saw.
#[cfg(target_os = "macos")]
struct GpuBound {
    /// Each tick's `submitted`.
    submitted: Vec<u8>,
    discarded: u64,
    /// The context's counters right after a flush that followed the ticks.
    flushed: StickShiftStats,
}

/// Forty `FIXED_STEP` ticks of an engine on a large Metal offscreen context
/// whose render hook draws a hundred full-target quads, so the GPU, not the
/// tick, is the bottleneck; then a flush. `None` without a Metal device.
#[cfg(target_os = "macos")]
fn gpu_bound_ticks(pacing: StickShiftFramePacing, test: &str) -> Option<GpuBound> {
    const SIZE: u32 = 2048;
    let ctx = metal_context(SIZE, SIZE, test)?;
    let mut probe = Box::new(Probe {
        script: Some(Box::new(|_, at, _, frame| {
            if at == At::Render {
                let cover = StickShiftQuad {
                    width: SIZE as f32,
                    height: SIZE as f32,
                    ..quad()
                };
                for _ in 0..100 {
                    // SAFETY: the open frame of the render hook.
                    unsafe { stickshift_frame_draw_quad(frame, &cover) };
                }
            }
        })),
        ..Probe::default()
    });
    let game = game_of(&mut probe);
    let desc = StickShiftEngineDesc {
        pacing,
        ..engine_desc()
    };
    let mut engine = null_mut();
    let mut run = GpuBound {
        submitted: Vec::new(),
        discarded: 0,
        flushed: StickShiftStats::default(),
    };
    // SAFETY: valid locals; the engine is destroyed before its context.
    unsafe {
        assert_eq!(
            stickshift_engine_create(ctx, &desc, &game, &mut engine),
            STICKSHIFT_OK
        );
        assert_eq!(stickshift_engine_init(engine), STICKSHIFT_OK);
        let tick = StickShiftTickDesc {
            size: std::mem::size_of::<StickShiftTickDesc>() as u32,
            clock: STICKSHIFT_CLOCK_FIXED_STEP,
            nanoseconds: 0,
            readback: null_mut(),
            readback_len: 0,
        };
        for _ in 0..40 {
            let mut out = StickShiftTickResult {
                size: std::mem::size_of::<StickShiftTickResult>() as u32,
                ..Default::default()
            };
            assert_eq!(
                stickshift_engine_tick(engine, &tick, &mut out),
                STICKSHIFT_OK,
                "{}",
                last_error()
            );
            run.submitted.push(out.submitted);
        }
        run.discarded = state_of(engine).expect("a live engine").frames_discarded;
        assert_eq!(stickshift_context_flush(ctx), STICKSHIFT_OK);
        assert_eq!(
            stickshift_context_get_stats(ctx, &mut run.flushed),
            STICKSHIFT_OK
        );
        assert_eq!(stickshift_engine_destroy(engine), STICKSHIFT_OK);
        assert_eq!(stickshift_context_destroy(ctx), STICKSHIFT_OK);
    }
    Some(run)
}

#[cfg(target_os = "macos")]
#[test]
fn wait_pacing_waits_for_a_free_metal_slot() {
    let test = "wait_pacing_waits_for_a_free_metal_slot";
    let Some(skip) = gpu_bound_ticks(STICKSHIFT_FRAME_PACING_SKIP, test) else {
        return;
    };
    // The load fills the GPU's queue: never waiting discards frames.
    assert!(
        skip.discarded > 0,
        "SKIP discarded no frame, so this load does not fill the GPU queue"
    );
    let wait = gpu_bound_ticks(STICKSHIFT_FRAME_PACING_WAIT, test).expect("a Metal device");
    assert_eq!(wait.discarded, 0);
    assert!(
        wait.submitted.iter().all(|&s| s == 1),
        "{:?}",
        wait.submitted
    );
}

#[cfg(target_os = "macos")]
#[test]
fn a_flush_waits_for_the_engines_frames_too() {
    let test = "a_flush_waits_for_the_engines_frames_too";
    let Some(run) = gpu_bound_ticks(STICKSHIFT_FRAME_PACING_SKIP, test) else {
        return;
    };
    // The engine's frames have no completion to deliver; the flush still
    // waited for every one of them.
    assert!(run.flushed.frames_submitted > 0);
    assert_eq!(
        run.flushed.frames_completed, run.flushed.frames_submitted,
        "frames still on the GPU after a flush"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn a_metal_tick_readback_equals_the_frames_async_readback() {
    const W: u32 = 32;
    const H: u32 = 16;
    let Some(engine_ctx) = metal_context(
        W,
        H,
        "a_metal_tick_readback_equals_the_frames_async_readback",
    ) else {
        return;
    };
    let plain = metal_context(W, H, "metal").expect("a second Metal context");
    let mut probe = Box::new(Probe {
        script: Some(Box::new(|_, at, _, frame| {
            if at == At::Render {
                // SAFETY: the open frame of the render hook.
                unsafe {
                    stickshift_frame_set_camera(frame, &fixed_camera());
                    stickshift_frame_draw_quad(frame, &quad());
                }
            }
        })),
        ..Probe::default()
    });
    let game = game_of(&mut probe);
    let mut engine = null_mut();
    let mut ticked = vec![0xABu8; (W * H * 4) as usize];
    let mut polled = vec![0xCDu8; (W * H * 4) as usize];
    // SAFETY: valid locals; the buffers outlive the calls that fill them.
    unsafe {
        assert_eq!(
            stickshift_engine_create(engine_ctx, &engine_desc(), &game, &mut engine),
            STICKSHIFT_OK
        );
        assert_eq!(stickshift_engine_init(engine), STICKSHIFT_OK);
        let tick = StickShiftTickDesc {
            size: std::mem::size_of::<StickShiftTickDesc>() as u32,
            clock: STICKSHIFT_CLOCK_FIXED_STEP,
            nanoseconds: 0,
            readback: ticked.as_mut_ptr().cast(),
            readback_len: ticked.len(),
        };
        let mut out = StickShiftTickResult {
            size: std::mem::size_of::<StickShiftTickResult>() as u32,
            ..Default::default()
        };
        assert_eq!(
            stickshift_engine_tick(engine, &tick, &mut out),
            STICKSHIFT_OK,
            "{}",
            last_error()
        );
        assert_eq!((out.submitted, out.readback_ready), (1, 1));

        // The same frame on a context with no engine, read back the async way,
        // with the render settings the engine's loop pushed.
        let mut settings = StickShiftRenderSettings {
            shadows: 0,
            shadow_distance: 0.0,
            draw_distance_scale: 1.0,
        };
        assert_eq!(
            stickshift_context_get_settings(engine_ctx, &mut settings),
            STICKSHIFT_OK
        );
        assert_eq!(
            stickshift_context_set_settings(plain, &settings),
            STICKSHIFT_OK
        );
        let mut frame = null_mut();
        assert_eq!(stickshift_frame_acquire(plain, &mut frame), STICKSHIFT_OK);
        stickshift_frame_set_camera(frame, &fixed_camera());
        stickshift_frame_draw_quad(frame, &quad());
        assert_eq!(
            stickshift_frame_read_pixels_async(
                frame,
                polled.as_mut_ptr().cast(),
                polled.len(),
                null()
            ),
            STICKSHIFT_OK
        );
        assert_eq!(
            stickshift_frame_submit(frame, null(), null_mut()),
            STICKSHIFT_OK
        );
        assert_eq!(stickshift_context_flush(plain), STICKSHIFT_OK);
        assert!(
            ticked.iter().any(|&b| b != 0xAB),
            "the tick filled its buffer"
        );
        assert_eq!(ticked, polled);

        assert_eq!(stickshift_engine_destroy(engine), STICKSHIFT_OK);
        assert_eq!(stickshift_context_destroy(engine_ctx), STICKSHIFT_OK);
        assert_eq!(stickshift_context_destroy(plain), STICKSHIFT_OK);
    }
}
