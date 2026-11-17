// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! The engine's camera, input, audio, graphics settings, menu and persistence
//! through C, against the Rust they run.
//!
//! The probe differential runs one probe game twice, as a Rust `Game` on a core
//! `Session` and as C callbacks on an engine, with the same host events and
//! clock, and requires every line both print (from inside the hooks and
//! between frames) to be equal: chase follow, key edges (taps between frames, a
//! repeat, focus loss, zero-step frames), silent audio counters, mid-run
//! settings requests (the settings hook's view of the renderer and the reach)
//! and menu choices (composing on a pending request, full screen).
//!
//! This is the host's half of the probe differential: a short, dense script
//! of host events, run both unhosted and hosted, with no world. The world's
//! half, `tests/engine_parity.rs`, runs 3,000 frames of streaming, rebasing
//! and querying, with the camera carried and chased and settings requests
//! rescaling the reach; between them every engine service is compared with
//! the Rust it runs. Which scopes allow which calls is `tests/engine.rs`'s
//! scope matrix.
//!
//! The Swift package ports both probes (`bindings/swift/StickShift/Tests`) and
//! checks its lines against fixtures made from this file's `rust_run`: after
//! changing what either probe does or prints, regenerate them with
//! `bindings/swift/StickShift/Tests/Fixtures/generate-probe-fixtures.sh` and
//! change the Swift port to match.
//!
//! C hooks never assert: a panic cannot unwind out of an `extern "C"`
//! function. They record what they see, and the tests compare afterwards.

use std::cell::RefCell;
use std::ffi::{c_void, CStr};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr::{null, null_mut};
use std::rc::Rc;
use std::time::Duration;

use stickshift::*;
use stickshift_camera::{Camera, ChaseController};
use stickshift_core::driver::Loop;
use stickshift_core::graphics::{DrawDistance, RenderScale, ShadowDistance, ShadowQuality};
use stickshift_core::{
    Audio, Clock, EngineCtx, Game, GraphicsSettings, HostEvent, InputState, Key, MenuCommand,
    MouseButton, Requests, Session, SettingsStore, SoundHandle, View, Volume, FIXED_DT,
};
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

const DT_NS: u64 = STICKSHIFT_FIXED_DT_NS;

/// The keys in `STICKSHIFT_KEY_*` order.
const KEYS: [Key; 12] = [
    Key::W,
    Key::A,
    Key::S,
    Key::D,
    Key::Q,
    Key::E,
    Key::Left,
    Key::Right,
    Key::Up,
    Key::Down,
    Key::Space,
    Key::Escape,
];

/// The mouse buttons in `STICKSHIFT_MOUSE_*` order.
const BUTTONS: [MouseButton; 3] = [MouseButton::Left, MouseButton::Right, MouseButton::Middle];

fn c_key(key: Key) -> StickShiftKey {
    KEYS.iter().position(|k| *k == key).unwrap() as StickShiftKey
}

fn c_button(button: MouseButton) -> StickShiftMouseButton {
    BUTTONS.iter().position(|b| *b == button).unwrap() as StickShiftMouseButton
}

/// FNV-1a over a matrix's bits: equal matrices, equal hashes.
fn hash(m: Mat4) -> u64 {
    m.to_cols_array()
        .iter()
        .fold(0xcbf2_9ce4_8422_2325, |h, v| {
            (h ^ u64::from(v.to_bits())).wrapping_mul(0x0100_0000_01b3)
        })
}

fn v3(v: StickShiftVec3) -> Vec3 {
    Vec3::new(v.x, v.y, v.z)
}

fn m4(m: StickShiftMat4) -> Mat4 {
    Mat4::from_cols_array(&m.m)
}

/// Settings as their four C values.
fn c4(s: GraphicsSettings) -> [u32; 4] {
    fn at<T: PartialEq>(all: &[T], v: T) -> u32 {
        all.iter().position(|x| *x == v).unwrap() as u32
    }
    [
        at(
            &[ShadowQuality::Off, ShadowQuality::Low, ShadowQuality::High],
            s.shadows,
        ),
        at(&ShadowDistance::ALL, s.shadow_distance),
        at(&DrawDistance::ALL, s.draw_distance),
        at(&RenderScale::ALL, s.render_scale),
    ]
}

fn c_settings(s: GraphicsSettings) -> StickShiftGraphicsSettings {
    let [shadows, shadow_distance, draw_distance, render_scale] = c4(s);
    StickShiftGraphicsSettings {
        shadows,
        shadow_distance,
        draw_distance,
        render_scale,
        reserved: [0; 4],
    }
}

fn from_c(s: StickShiftGraphicsSettings) -> [u32; 4] {
    [
        s.shadows,
        s.shadow_distance,
        s.draw_distance,
        s.render_scale,
    ]
}

fn rust_settings(s: StickShiftGraphicsSettings) -> GraphicsSettings {
    GraphicsSettings {
        shadows: [ShadowQuality::Off, ShadowQuality::Low, ShadowQuality::High][s.shadows as usize],
        shadow_distance: ShadowDistance::ALL[s.shadow_distance as usize],
        draw_distance: DrawDistance::ALL[s.draw_distance as usize],
        render_scale: RenderScale::ALL[s.render_scale as usize],
    }
}

// ---------------------------------------------------------------------------
// What a probe line shows, from either side
// ---------------------------------------------------------------------------

/// A camera, field for field.
struct Cam {
    position: Vec3,
    target: Vec3,
    up: Vec3,
    fov_y: f32,
    aspect_ratio: f32,
    near: f32,
    far: f32,
    view: Mat4,
    projection: Mat4,
    view_projection: Mat4,
    forward: Vec3,
    right: Vec3,
}

impl Cam {
    fn rust(c: &Camera) -> Self {
        let (near, far) = c.clip_planes();
        Self {
            position: c.position(),
            target: c.target(),
            up: c.up(),
            fov_y: c.fov_y(),
            aspect_ratio: c.aspect_ratio(),
            near,
            far,
            view: c.view_matrix(),
            projection: c.projection_matrix(),
            view_projection: c.view_projection_matrix(),
            forward: c.forward(),
            right: c.right(),
        }
    }

    fn c(c: &StickShiftCameraState) -> Self {
        Self {
            position: v3(c.position),
            target: v3(c.target),
            up: v3(c.up),
            fov_y: c.fov_y,
            aspect_ratio: c.aspect_ratio,
            near: c.near_plane,
            far: c.far_plane,
            view: m4(c.view),
            projection: m4(c.projection),
            view_projection: m4(c.view_projection),
            forward: v3(c.forward),
            right: v3(c.right),
        }
    }

    fn line(&self) -> String {
        format!(
            "p{:?} t{:?} u{:?} fov {:?} a {:?} {:?}..{:?} v{:x} p{:x} vp{:x} f{:?} r{:?}",
            self.position.to_array(),
            self.target.to_array(),
            self.up.to_array(),
            self.fov_y,
            self.aspect_ratio,
            self.near,
            self.far,
            hash(self.view),
            hash(self.projection),
            hash(self.view_projection),
            self.forward.to_array(),
            self.right.to_array(),
        )
    }
}

/// The input, as `STICKSHIFT_KEY_STATE_*` bits per key.
struct Inp {
    keys: [u8; 12],
    mouse: [u8; 3],
    cursor: Option<(f64, f64)>,
}

impl Inp {
    fn rust(input: &InputState) -> Self {
        Self {
            keys: KEYS.map(|k| {
                u8::from(input.is_key_down(k))
                    | u8::from(input.is_key_just_pressed(k)) << 1
                    | u8::from(input.is_key_just_released(k)) << 2
            }),
            mouse: BUTTONS.map(|b| u8::from(input.is_mouse_down(b))),
            cursor: input.cursor_position(),
        }
    }

    fn c(input: &StickShiftInputState) -> Self {
        let mut keys = [0; 12];
        keys.copy_from_slice(&input.keys[..12]);
        let mut mouse = [0; 3];
        mouse.copy_from_slice(&input.mouse_buttons[..3]);
        Self {
            keys,
            mouse,
            cursor: (input.has_cursor == 1).then_some((input.cursor_x, input.cursor_y)),
        }
    }

    fn line(&self) -> String {
        format!("{:?} {:?} {:?}", self.keys, self.mouse, self.cursor)
    }
}

/// The audio counters, with each sound by its load index.
struct Aud {
    sounds: u32,
    decodes: u32,
    loop_starts: u32,
    one_shots: u32,
    master: f32,
    looping: Option<usize>,
    silent: bool,
    per_sound: Vec<u32>,
}

impl Aud {
    fn rust(audio: &Audio, sounds: &[SoundHandle]) -> Self {
        Self {
            sounds: audio.sound_count() as u32,
            decodes: audio.decode_count() as u32,
            loop_starts: audio.loop_starts(),
            one_shots: audio.one_shots_played(),
            master: audio.master_volume().0,
            looping: audio
                .looping()
                .and_then(|l| sounds.iter().position(|s| *s == l)),
            silent: audio.is_silent(),
            per_sound: sounds.iter().map(|s| audio.one_shots_of(*s)).collect(),
        }
    }

    fn line(&self) -> String {
        format!(
            "{} {} {} {} {:?} {:?} {} {:?}",
            self.sounds,
            self.decodes,
            self.loop_starts,
            self.one_shots,
            self.master,
            self.looping,
            self.silent,
            self.per_sound
        )
    }
}

/// What the renderer (the context) holds.
struct Rnd {
    settings: StickShiftRenderSettings,
    size: (u32, u32),
    scale: f32,
    camera: (Mat4, Vec3),
}

impl Rnd {
    fn line(&self) -> String {
        let s = self.settings;
        format!(
            "{} {:?} {:?} size={}x{} {:?} vp{:x} e{:?}",
            s.shadows,
            s.shadow_distance,
            s.draw_distance_scale,
            self.size.0,
            self.size.1,
            self.scale,
            hash(self.camera.0),
            self.camera.1.to_array()
        )
    }
}

// ---------------------------------------------------------------------------
// The probe
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum At {
    Init,
    Update,
    Render,
    Settings,
}

/// A probe hook's view of the engine: Rust's `EngineCtx` or the C engine.
trait Api {
    fn camera(&mut self) -> Cam;
    fn set_position(&mut self, p: Vec3);
    fn set_target(&mut self, p: Vec3);
    fn set_clip_planes(&mut self, near: f32, far: f32);
    fn set_fov_y(&mut self, fov_y: f32);
    fn follow(&mut self, target: Vec3, forward: Vec3, dt: f32);
    fn input(&mut self) -> Inp;
    fn load(&mut self, path: &str);
    fn play_once(&mut self, sound: usize, db: f32);
    fn play_looping(&mut self, sound: usize, db: f32);
    fn stop_looping(&mut self);
    fn set_master_volume(&mut self, db: f32);
    fn audio(&mut self) -> Aud;
    fn graphics(&mut self) -> GraphicsSettings;
    fn request(&mut self, settings: GraphicsSettings);
    /// A context call: in a hook, it pushes a moved camera first.
    fn renderer(&mut self) -> Rnd;
    fn reach(&mut self) -> f32;
}

/// The paths the probe loads, the third a repeat of the first.
const PATHS: [&str; 4] = [
    "probe/engine.ogg",
    "probe/music.ogg",
    "probe/engine.ogg",
    "probe/chime.wav",
];

/// The chase camera: a tilted, unnormalized up (so the up, the height offset
/// and the look-at height all depend on it), a look-at height and smoothing.
const CHASE: StickShiftChase = StickShiftChase {
    distance: 7.5,
    height: 3.0,
    look_at_height: 1.25,
    up: StickShiftVec3 {
        x: 0.3,
        y: 2.0,
        z: -0.2,
    },
    half_life: 0.15,
};

/// What the probe's hooks do, on either side.
fn script(api: &mut dyn Api, at: At, n: u32, log: &mut Vec<String>) {
    match at {
        At::Init => {
            api.set_clip_planes(0.5, 800.0);
            // Ignored, as in Rust: far is not beyond near.
            api.set_clip_planes(5.0, 1.0);
            api.set_fov_y(1.0);
            api.set_position(Vec3::new(0.0, 4.0, 9.0));
            api.set_target(Vec3::new(0.0, 0.0, -5.0));
            for path in PATHS {
                api.load(path);
            }
            log.push(format!(
                "I cam={} aud={} gfx={:?} reach={:?}",
                api.camera().line(),
                api.audio().line(),
                c4(api.graphics()),
                api.reach()
            ));
        }
        At::Update => {
            let t = n as f32;
            let target = Vec3::new((t * 0.01).sin() * 3.0, 0.0, -0.2 * t);
            let forward = Vec3::new((t * 0.01).cos() * 0.1, 0.0, -1.0);
            api.follow(target, forward, FIXED_DT);
            if n % 120 == 1 {
                api.play_once(0, -6.0);
            }
            if n % 500 == 10 || n % 500 == 11 {
                // The second asks for the sound already looping: nothing starts.
                api.play_looping(1, -3.0);
            }
            if n % 500 == 250 {
                api.stop_looping();
            }
            if n == 700 {
                // The same sound by its second load: one loop start.
                api.play_looping(0, 0.0);
                api.play_looping(2, 0.0);
            }
            if n == 900 {
                api.set_master_volume(-12.0);
            }
            if n % 333 == 7 {
                api.play_once(3, 0.0);
            }
            if n == 300 || n == 301 {
                let s = api.graphics();
                api.request(GraphicsSettings {
                    draw_distance: DrawDistance::Near,
                    ..s
                });
            }
            if n == 600 {
                // Equal to the settings in effect: no change, no hook.
                let s = api.graphics();
                api.request(s);
            }
            if n == 1500 {
                let s = api.graphics();
                api.request(GraphicsSettings {
                    shadows: ShadowQuality::Low,
                    render_scale: RenderScale::ThreeQuarters,
                    ..s
                });
            }
            log.push(format!(
                "U{n} in={} cam={}",
                api.input().line(),
                api.camera().line()
            ));
        }
        At::Render => {
            if n.is_multiple_of(3) {
                api.follow(Vec3::new(0.0, 0.0, -0.2 * n as f32), Vec3::NEG_Z, 0.0);
            }
            if n == 800 {
                let s = api.graphics();
                api.request(GraphicsSettings {
                    shadows: ShadowQuality::Low,
                    ..s
                });
            }
            if n == 2100 {
                // Still pending when the host's menu choice arrives: the
                // choice composes on it.
                let s = api.graphics();
                api.request(GraphicsSettings {
                    draw_distance: DrawDistance::Near,
                    ..s
                });
            }
            let renderer = if n.is_multiple_of(7) {
                api.renderer().line()
            } else {
                String::new()
            };
            log.push(format!(
                "R{n} cam={} aud={} gfx={:?} rnd={renderer}",
                api.camera().line(),
                api.audio().line(),
                c4(api.graphics())
            ));
        }
        At::Settings => {
            log.push(format!(
                "S{n} gfx={:?} reach={:?} rnd={}",
                c4(api.graphics()),
                api.reach(),
                api.renderer().line()
            ));
        }
    }
}

/// A [`NullRenderer`] the Rust probe can read inside its hooks.
#[derive(Clone)]
struct Spy(Rc<RefCell<NullRenderer>>);

impl Spy {
    fn new() -> Self {
        Self(Rc::new(RefCell::new(NullRenderer::new())))
    }

    fn rnd(&self) -> Rnd {
        let r = self.0.borrow();
        Rnd {
            settings: r.render_settings().unwrap_or_default().into(),
            size: r.surface_size(),
            scale: r.surface_scale(),
            camera: (
                r.view_projection().unwrap_or(Mat4::IDENTITY),
                r.camera_position().unwrap_or(Vec3::ZERO),
            ),
        }
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

/// The Rust side of a probe hook.
struct RustApi<'a, 'b> {
    ctx: &'a mut EngineCtx<'b>,
    spy: &'a Spy,
    sounds: &'a mut Vec<SoundHandle>,
    /// Built once, as a Rust game would; the C side builds it per call.
    chase: ChaseController,
}

impl Api for RustApi<'_, '_> {
    fn camera(&mut self) -> Cam {
        Cam::rust(self.ctx.camera())
    }
    fn set_position(&mut self, p: Vec3) {
        self.ctx.camera_mut().set_position(p);
    }
    fn set_target(&mut self, p: Vec3) {
        self.ctx.camera_mut().set_target(p);
    }
    fn set_clip_planes(&mut self, near: f32, far: f32) {
        self.ctx.camera_mut().set_clip_planes(near, far);
    }
    fn set_fov_y(&mut self, fov_y: f32) {
        self.ctx.camera_mut().set_fov_y(fov_y);
    }
    fn follow(&mut self, target: Vec3, forward: Vec3, dt: f32) {
        self.chase
            .follow(self.ctx.camera_mut(), target, forward, dt);
    }
    fn input(&mut self) -> Inp {
        Inp::rust(self.ctx.input())
    }
    fn load(&mut self, path: &str) {
        let sound = self.ctx.audio().load(path);
        self.sounds.push(sound);
    }
    fn play_once(&mut self, sound: usize, db: f32) {
        self.ctx.audio().play_once(self.sounds[sound], Volume(db));
    }
    fn play_looping(&mut self, sound: usize, db: f32) {
        self.ctx
            .audio()
            .play_looping(self.sounds[sound], Volume(db));
    }
    fn stop_looping(&mut self) {
        self.ctx.audio().stop_looping();
    }
    fn set_master_volume(&mut self, db: f32) {
        self.ctx.audio().set_master_volume(Volume(db));
    }
    fn audio(&mut self) -> Aud {
        Aud::rust(self.ctx.audio(), self.sounds)
    }
    fn graphics(&mut self) -> GraphicsSettings {
        self.ctx.graphics_settings()
    }
    fn request(&mut self, settings: GraphicsSettings) {
        self.ctx.set_graphics_settings(settings);
    }
    fn renderer(&mut self) -> Rnd {
        let _ = self.ctx.renderer();
        self.spy.rnd()
    }
    fn reach(&mut self) -> f32 {
        self.ctx.scene().config().spawn_ahead
    }
}

fn rust_chase() -> ChaseController {
    ChaseController::new(CHASE.distance, CHASE.height)
        .with_up(v3(CHASE.up))
        .with_look_at_height(CHASE.look_at_height)
        .with_half_life(CHASE.half_life)
}

/// The probe as a Rust game.
struct RustProbe {
    spy: Spy,
    sounds: Vec<SoundHandle>,
    log: Vec<String>,
    updates: u32,
    renders: u32,
    settings: u32,
}

impl RustProbe {
    fn run(&mut self, ctx: &mut EngineCtx, at: At, n: u32) {
        let mut lines = Vec::new();
        let mut api = RustApi {
            ctx,
            spy: &self.spy,
            sounds: &mut self.sounds,
            chase: rust_chase(),
        };
        script(&mut api, at, n, &mut lines);
        self.log.extend(lines);
    }
}

impl Game for RustProbe {
    fn init(&mut self, ctx: &mut EngineCtx) {
        self.run(ctx, At::Init, 0);
    }

    fn update(&mut self, ctx: &mut EngineCtx, _dt: f32) {
        self.updates += 1;
        self.run(ctx, At::Update, self.updates);
    }

    fn render(&mut self, ctx: &mut EngineCtx) {
        self.renders += 1;
        self.run(ctx, At::Render, self.renders);
    }

    fn graphics_settings_changed(&mut self, ctx: &mut EngineCtx, settings: &GraphicsSettings) {
        self.settings += 1;
        self.log.push(format!("S arg={:?}", c4(*settings)));
        self.run(ctx, At::Settings, self.settings);
    }
}

/// The C side of a probe hook. A failing call is recorded, not asserted.
struct CApi<'a> {
    e: *mut StickShiftEngine,
    sounds: &'a mut Vec<StickShiftSound>,
    errors: Vec<String>,
}

impl CApi<'_> {
    fn check(&mut self, what: &str, r: StickShiftResult) {
        if r != STICKSHIFT_OK {
            self.errors.push(format!("ERR {what} {r} {}", last_error()));
        }
    }

    fn sound(&self, i: usize) -> StickShiftSound {
        self.sounds.get(i).copied().unwrap_or_default()
    }

    fn ctx(&mut self) -> *mut StickShiftContext {
        let mut ctx = null_mut();
        // SAFETY: a live engine, on its thread.
        let r = unsafe { stickshift_engine_get_context(self.e, &mut ctx) };
        self.check("get_context", r);
        ctx
    }
}

impl Api for CApi<'_> {
    fn camera(&mut self) -> Cam {
        let mut c = StickShiftCameraState {
            size: std::mem::size_of::<StickShiftCameraState>() as u32,
            ..Default::default()
        };
        // SAFETY: a live engine, on its thread, and valid locals; so for every
        // call below.
        let r = unsafe { stickshift_camera_get(self.e, &mut c) };
        self.check("camera_get", r);
        Cam::c(&c)
    }
    fn set_position(&mut self, p: Vec3) {
        let r = unsafe { stickshift_camera_set_position(self.e, p.into()) };
        self.check("set_position", r);
    }
    fn set_target(&mut self, p: Vec3) {
        let r = unsafe { stickshift_camera_set_target(self.e, p.into()) };
        self.check("set_target", r);
    }
    fn set_clip_planes(&mut self, near: f32, far: f32) {
        let r = unsafe { stickshift_camera_set_clip_planes(self.e, near, far) };
        self.check("set_clip_planes", r);
    }
    fn set_fov_y(&mut self, fov_y: f32) {
        let r = unsafe { stickshift_camera_set_fov_y(self.e, fov_y) };
        self.check("set_fov_y", r);
    }
    fn follow(&mut self, target: Vec3, forward: Vec3, dt: f32) {
        let r =
            unsafe { stickshift_camera_follow(self.e, &CHASE, target.into(), forward.into(), dt) };
        self.check("follow", r);
    }
    fn input(&mut self) -> Inp {
        let mut input = StickShiftInputState {
            size: std::mem::size_of::<StickShiftInputState>() as u32,
            ..Default::default()
        };
        let r = unsafe { stickshift_input_get(self.e, &mut input) };
        self.check("input_get", r);
        Inp::c(&input)
    }
    fn load(&mut self, path: &str) {
        let path = std::ffi::CString::new(path).unwrap();
        let mut sound = StickShiftSound::default();
        let r = unsafe { stickshift_audio_load(self.e, path.as_ptr(), &mut sound) };
        self.check("audio_load", r);
        self.sounds.push(sound);
    }
    fn play_once(&mut self, sound: usize, db: f32) {
        let r = unsafe { stickshift_audio_play_once(self.e, self.sound(sound), db) };
        self.check("play_once", r);
    }
    fn play_looping(&mut self, sound: usize, db: f32) {
        let r = unsafe { stickshift_audio_play_looping(self.e, self.sound(sound), db) };
        self.check("play_looping", r);
    }
    fn stop_looping(&mut self) {
        let r = unsafe { stickshift_audio_stop_looping(self.e) };
        self.check("stop_looping", r);
    }
    fn set_master_volume(&mut self, db: f32) {
        let r = unsafe { stickshift_audio_set_master_volume(self.e, db) };
        self.check("set_master_volume", r);
    }
    fn audio(&mut self) -> Aud {
        let mut stats = StickShiftAudioStats {
            size: std::mem::size_of::<StickShiftAudioStats>() as u32,
            ..Default::default()
        };
        let r = unsafe { stickshift_audio_get_stats(self.e, &mut stats) };
        self.check("audio_get_stats", r);
        let mut per_sound = Vec::new();
        for i in 0..self.sounds.len() {
            let mut n = 0;
            let r = unsafe { stickshift_audio_one_shots_of(self.e, self.sound(i), &mut n) };
            self.check("one_shots_of", r);
            per_sound.push(n);
        }
        Aud {
            sounds: stats.sound_count,
            decodes: stats.decode_count,
            loop_starts: stats.loop_starts,
            one_shots: stats.one_shots_played,
            master: stats.master_volume_db,
            looping: (stats.looping.id != 0)
                .then(|| self.sounds.iter().position(|s| *s == stats.looping))
                .flatten(),
            silent: stats.is_silent == 1,
            per_sound,
        }
    }
    fn graphics(&mut self) -> GraphicsSettings {
        let mut s = StickShiftGraphicsSettings::default();
        let r = unsafe { stickshift_graphics_get_current(self.e, &mut s) };
        self.check("graphics_get_current", r);
        rust_settings(s)
    }
    fn request(&mut self, settings: GraphicsSettings) {
        let r = unsafe { stickshift_graphics_request(self.e, &c_settings(settings)) };
        self.check("graphics_request", r);
    }
    fn renderer(&mut self) -> Rnd {
        let ctx = self.ctx();
        let (rnd, results) = context_rnd(ctx);
        for r in results {
            self.check("context getter", r);
        }
        rnd
    }
    fn reach(&mut self) -> f32 {
        let mut config = StickShiftStreamingConfig::default();
        let r = unsafe { stickshift_scene_get_config(self.e, &mut config) };
        self.check("scene_get_config", r);
        config.spawn_ahead
    }
}

/// What a context holds, read through its getters (the first pushes a moved
/// camera inside a hook), and what each getter returned.
fn context_rnd(ctx: *mut StickShiftContext) -> (Rnd, [StickShiftResult; 4]) {
    let mut settings = StickShiftRenderSettings {
        shadows: 0,
        shadow_distance: 0.0,
        draw_distance_scale: 0.0,
    };
    let (mut w, mut h, mut scale) = (0, 0, 0.0);
    let mut camera = StickShiftCamera::default();
    // SAFETY: a live context, on its engine's thread, and valid locals.
    let results = unsafe {
        [
            stickshift_context_get_settings(ctx, &mut settings),
            stickshift_context_get_size(ctx, &mut w, &mut h),
            stickshift_context_get_scale(ctx, &mut scale),
            stickshift_context_get_camera(ctx, &mut camera),
        ]
    };
    (
        Rnd {
            settings,
            size: (w, h),
            scale,
            camera: (m4(camera.view_projection), v3(camera.position)),
        },
        results,
    )
}

/// The probe as a C game, behind the hooks' user pointer.
#[derive(Default)]
struct CProbe {
    sounds: Vec<StickShiftSound>,
    log: Vec<String>,
    updates: u32,
    renders: u32,
    settings: u32,
}

unsafe fn c_hook(
    user: *mut c_void,
    e: *mut StickShiftEngine,
    at: At,
    arg: Option<StickShiftGraphicsSettings>,
) {
    let p = &mut *user.cast::<CProbe>();
    let ran = catch_unwind(AssertUnwindSafe(|| {
        let n = match at {
            At::Init => 0,
            At::Update => {
                p.updates += 1;
                p.updates
            }
            At::Render => {
                p.renders += 1;
                p.renders
            }
            At::Settings => {
                p.settings += 1;
                p.log
                    .push(format!("S arg={:?}", arg.map(from_c).unwrap_or_default()));
                p.settings
            }
        };
        let mut lines = Vec::new();
        let mut api = CApi {
            e,
            sounds: &mut p.sounds,
            errors: Vec::new(),
        };
        script(&mut api, at, n, &mut lines);
        let errors = std::mem::take(&mut api.errors);
        p.log.extend(lines);
        p.log.extend(errors);
    }));
    if ran.is_err() {
        p.log.push(format!("PANIC in {at:?}"));
    }
}

unsafe extern "C" fn c_init(user: *mut c_void, e: *mut StickShiftEngine) -> StickShiftResult {
    c_hook(user, e, At::Init, None);
    STICKSHIFT_OK
}

unsafe extern "C" fn c_update(user: *mut c_void, e: *mut StickShiftEngine, _dt: f32) {
    c_hook(user, e, At::Update, None);
}

unsafe extern "C" fn c_render(
    user: *mut c_void,
    e: *mut StickShiftEngine,
    _: *mut StickShiftFrame,
) {
    c_hook(user, e, At::Render, None);
}

unsafe extern "C" fn c_settings_changed(
    user: *mut c_void,
    e: *mut StickShiftEngine,
    settings: *const StickShiftGraphicsSettings,
) {
    c_hook(user, e, At::Settings, Some(*settings));
}

// ---------------------------------------------------------------------------
// The host's side: events, clock, and what it reads between frames
// ---------------------------------------------------------------------------

/// A host action between frames.
#[derive(Clone, Copy)]
enum Ev {
    Event(HostEvent),
    /// A request from the host between engine calls, on the settings in
    /// effect.
    Request(fn(GraphicsSettings) -> GraphicsSettings),
}

fn menu(tag: isize) -> Ev {
    Ev::Event(HostEvent::Menu(MenuCommand::from_tag(tag).unwrap()))
}

/// The host's events before frame `f`.
fn host_events(f: u32) -> Vec<Ev> {
    let ev = Ev::Event;
    let mut v = Vec::new();
    if f % 37 == 5 {
        // A tap between two frames: both edges reach the next update.
        v.push(ev(HostEvent::KeyDown {
            key: Key::Space,
            repeat: false,
        }));
        v.push(ev(HostEvent::KeyUp(Key::Space)));
    }
    match f % 53 {
        10 => v.push(ev(HostEvent::KeyDown {
            key: Key::W,
            repeat: false,
        })),
        // Repeats of a held key make no edge, flagged or not.
        11 => v.push(ev(HostEvent::KeyDown {
            key: Key::W,
            repeat: true,
        })),
        12 => v.push(ev(HostEvent::KeyDown {
            key: Key::W,
            repeat: false,
        })),
        30 => v.push(ev(HostEvent::KeyUp(Key::W))),
        _ => {}
    }
    match f % 71 {
        20 => v.extend([
            ev(HostEvent::KeyDown {
                key: Key::Left,
                repeat: false,
            }),
            ev(HostEvent::KeyDown {
                key: Key::A,
                repeat: false,
            }),
            ev(HostEvent::MouseDown(MouseButton::Left)),
        ]),
        // Focus loss releases everything held, with edges.
        23 => v.push(ev(HostEvent::FocusLost)),
        _ => {}
    }
    match f % 89 {
        40 => v.push(ev(HostEvent::MouseDown(MouseButton::Right))),
        41 => v.push(ev(HostEvent::MouseUp(MouseButton::Right))),
        _ => {}
    }
    if f.is_multiple_of(41) {
        v.push(ev(HostEvent::Cursor(
            f as f64 * 0.5,
            f as f64 * 0.25 + 0.125,
        )));
    }
    match f {
        400 => v.push(menu(302)),
        700 => v.push(menu(901)),
        1000 => {
            // A host request still pending, then a menu choice: the choice
            // composes on it, and both apply now.
            v.push(Ev::Request(|s| GraphicsSettings {
                render_scale: RenderScale::Full,
                ..s
            }));
            v.push(menu(101));
        }
        1300 => v.push(menu(202)),
        1600 => v.push(menu(902)),
        // The render hook of the frame before requested a change.
        2100 => v.push(menu(403)),
        // Two choices in one batch: each applies at once.
        2500 => v.extend([menu(102), menu(303)]),
        _ => {}
    }
    v
}

/// Fixed steps of elapsed time before frame `f`: zero-step frames, several
/// steps, and more than the cap.
fn elapsed_ns(f: u32) -> u64 {
    const STEPS: [u64; 10] = [1, 0, 2, 1, 1, 3, 0, 1, 7, 1];
    STEPS[f as usize % 10] * DT_NS
}

fn c_event(e: HostEvent) -> StickShiftHostEvent {
    let mut c = StickShiftHostEvent::default();
    match e {
        HostEvent::KeyDown { key, repeat } => {
            c.kind = STICKSHIFT_HOST_KEY_DOWN;
            c.code = c_key(key);
            c.flags = if repeat {
                STICKSHIFT_HOST_EVENT_REPEAT
            } else {
                0
            };
        }
        HostEvent::KeyUp(key) => {
            c.kind = STICKSHIFT_HOST_KEY_UP;
            c.code = c_key(key);
        }
        HostEvent::MouseDown(b) => {
            c.kind = STICKSHIFT_HOST_MOUSE_DOWN;
            c.code = c_button(b);
        }
        HostEvent::MouseUp(b) => {
            c.kind = STICKSHIFT_HOST_MOUSE_UP;
            c.code = c_button(b);
        }
        HostEvent::Cursor(x, y) => {
            c.kind = STICKSHIFT_HOST_CURSOR;
            c.x = x;
            c.y = y;
        }
        HostEvent::FocusLost => c.kind = STICKSHIFT_HOST_FOCUS_LOST,
        HostEvent::Menu(command) => {
            c.kind = STICKSHIFT_HOST_MENU;
            c.code = command.tag() as u32;
        }
        HostEvent::Close => c.kind = STICKSHIFT_HOST_CLOSE,
        other => panic!("the probe sends no {other:?}"),
    }
    c
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// `stickshift_engine_init`, and the session's frame with no view: tools
    /// and traces.
    Unhosted,
    /// `stickshift_engine_resume` and the session's frame: a view.
    Hosted,
}

const VIEW: (u32, u32, f64) = (1600, 1200, 2.0);

/// A host line from the graphics state's parts.
fn gfx_line(
    current: [u32; 4],
    pending: Option<[u32; 4]>,
    revision: u64,
    base: Option<f32>,
) -> String {
    format!("{current:?} {pending:?} {revision} {base:?}")
}

fn rust_run(mode: Mode, frames: u32) -> Vec<String> {
    let spy = Spy::new();
    let mut renderer = spy.clone();
    let mut game = RustProbe {
        spy: spy.clone(),
        sounds: Vec::new(),
        log: Vec::new(),
        updates: 0,
        renders: 0,
        settings: 0,
    };
    let mut session = Session::new(Loop::new(), None, false);
    match mode {
        Mode::Unhosted => session.lp.init_once(&mut game, &mut renderer),
        Mode::Hosted => {
            let r = session.resume(View::new(VIEW.0, VIEW.1, VIEW.2), &mut game, &mut renderer);
            game.log.push(format!("resume {}", r.bits()));
        }
    }
    for f in 0..frames {
        let mut events = Requests::NONE;
        for e in host_events(f) {
            match e {
                Ev::Event(e) => events |= session.event(e, &mut game, &mut renderer),
                Ev::Request(map) => {
                    let s = map(session.lp.graphics.current());
                    session.lp.graphics.request(s);
                }
            }
        }
        let elapsed = Duration::from_nanos(elapsed_ns(f));
        // Hosted or not, the engine ticks the session's frame.
        let (steps, requests) = session.frame(Clock::Elapsed(elapsed), &mut game, &mut renderer);
        let lp = &mut session.lp;
        let line = format!(
            "F{f} ev={} tick={steps} {} gfx={} cam={} in={} aud={} rnd={}",
            events.bits(),
            requests.bits(),
            gfx_line(
                c4(lp.graphics.current()),
                lp.graphics.pending().map(c4),
                lp.graphics.revision(),
                lp.graphics.base_spawn_ahead(),
            ),
            Cam::rust(&lp.camera).line(),
            Inp::rust(&lp.input).line(),
            Aud::rust(&lp.audio, &game.sounds).line(),
            spy.rnd().line(),
        );
        game.log.push(line);
    }
    game.log
}

fn null_context(width: u32, height: u32) -> *mut StickShiftContext {
    let desc = StickShiftContextDesc {
        api_version: STICKSHIFT_API_VERSION,
        backend: STICKSHIFT_BACKEND_NULL,
        metal_layer: null_mut(),
        width,
        height,
        scale: 1.0,
    };
    let mut ctx = null_mut();
    // SAFETY: valid locals.
    assert_eq!(
        unsafe { stickshift_context_create(&desc, &mut ctx) },
        STICKSHIFT_OK
    );
    ctx
}

fn engine_desc(flags: u32, suite: *const std::ffi::c_char) -> StickShiftEngineDesc {
    StickShiftEngineDesc {
        api_version: STICKSHIFT_API_VERSION,
        size: std::mem::size_of::<StickShiftEngineDesc>() as u32,
        audio: STICKSHIFT_AUDIO_SILENT,
        pacing: STICKSHIFT_FRAME_PACING_WAIT,
        flags,
        reserved: 0,
        settings_suite: suite,
    }
}

fn view_desc(width: u32, height: u32, scale_factor: f64) -> StickShiftViewDesc {
    StickShiftViewDesc {
        size: std::mem::size_of::<StickShiftViewDesc>() as u32,
        width,
        height,
        reserved: 0,
        scale_factor,
        safe_area: [0.0; 4],
    }
}

fn tick_desc(clock: StickShiftClock, nanoseconds: u64) -> StickShiftTickDesc {
    StickShiftTickDesc {
        size: std::mem::size_of::<StickShiftTickDesc>() as u32,
        clock,
        nanoseconds,
        readback: null_mut(),
        readback_len: 0,
    }
}

fn tick_result() -> StickShiftTickResult {
    StickShiftTickResult {
        size: std::mem::size_of::<StickShiftTickResult>() as u32,
        ..Default::default()
    }
}

fn graphics_state(e: *mut StickShiftEngine) -> (StickShiftResult, StickShiftGraphicsState) {
    let mut state = StickShiftGraphicsState {
        size: std::mem::size_of::<StickShiftGraphicsState>() as u32,
        ..Default::default()
    };
    // SAFETY: a live engine, on its thread.
    let r = unsafe { stickshift_graphics_get_state(e, &mut state) };
    (r, state)
}

/// Push `events` in one call; the requests they raised.
fn push(e: *mut StickShiftEngine, events: &[StickShiftHostEvent]) -> u32 {
    let mut requests = 0;
    // SAFETY: a live engine; the events are 64 bytes apart.
    let r = unsafe {
        stickshift_engine_push_events(
            e,
            events.as_ptr(),
            events.len() as u32,
            std::mem::size_of::<StickShiftHostEvent>() as u32,
            &mut requests,
        )
    };
    assert_eq!(r, STICKSHIFT_OK, "{}", last_error());
    requests
}

fn c_game(probe: *mut CProbe) -> StickShiftGame {
    StickShiftGame {
        size: std::mem::size_of::<StickShiftGame>() as u32,
        reserved: 0,
        user: probe.cast(),
        init: Some(c_init),
        update: Some(c_update),
        render: Some(c_render),
        graphics_settings_changed: Some(c_settings_changed),
        release: None,
    }
}

fn c_run(mode: Mode, frames: u32) -> Vec<String> {
    let probe = Box::into_raw(Box::new(CProbe::default()));
    let ctx = null_context(1280, 720);
    let mut e = null_mut();
    // SAFETY: valid locals; the probe outlives the engine.
    let r =
        unsafe { stickshift_engine_create(ctx, &engine_desc(0, null()), &c_game(probe), &mut e) };
    assert_eq!(r, STICKSHIFT_OK, "{}", last_error());
    // The host's log lines go where the hooks' do. SAFETY (every `log()`):
    // between engine calls, so no hook holds the probe.
    let log = move || unsafe { &mut (*probe).log };
    match mode {
        // SAFETY: a live engine.
        Mode::Unhosted => assert_eq!(unsafe { stickshift_engine_init(e) }, STICKSHIFT_OK),
        Mode::Hosted => {
            let mut requests = 0;
            let view = view_desc(VIEW.0, VIEW.1, VIEW.2);
            // SAFETY: a live engine and valid locals.
            let r = unsafe { stickshift_engine_resume(e, &view, &mut requests) };
            assert_eq!(r, STICKSHIFT_OK, "{}", last_error());
            log().push(format!("resume {requests}"));
        }
    }
    for f in 0..frames {
        let mut events = 0;
        let mut batch = Vec::new();
        for ev in host_events(f) {
            match ev {
                Ev::Event(event) => batch.push(c_event(event)),
                Ev::Request(map) => {
                    events |= push(e, &std::mem::take(&mut batch));
                    let mut s = StickShiftGraphicsSettings::default();
                    // SAFETY: a live engine, between calls, valid locals.
                    unsafe {
                        assert_eq!(stickshift_graphics_get_current(e, &mut s), STICKSHIFT_OK);
                        let s = c_settings(map(rust_settings(s)));
                        assert_eq!(stickshift_graphics_request(e, &s), STICKSHIFT_OK);
                    }
                }
            }
        }
        events |= push(e, &batch);
        let mut out = tick_result();
        // SAFETY: a live engine and valid locals.
        let r = unsafe {
            stickshift_engine_tick(
                e,
                &tick_desc(STICKSHIFT_CLOCK_ELAPSED, elapsed_ns(f)),
                &mut out,
            )
        };
        assert_eq!(r, STICKSHIFT_OK, "{}", last_error());

        let (r, g) = graphics_state(e);
        assert_eq!(r, STICKSHIFT_OK);
        let mut api = CApi {
            e,
            // SAFETY: as for `log`.
            sounds: unsafe { &mut (*probe).sounds },
            errors: Vec::new(),
        };
        let (rnd, results) = context_rnd(ctx);
        assert_eq!(results, [STICKSHIFT_OK; 4]);
        let line = format!(
            "F{f} ev={events} tick={} {} gfx={} cam={} in={} aud={} rnd={}",
            out.steps,
            out.requests,
            gfx_line(
                from_c(g.current),
                (g.has_pending == 1).then_some(from_c(g.pending)),
                g.revision,
                (g.has_base_spawn_ahead == 1).then_some(g.base_spawn_ahead),
            ),
            api.camera().line(),
            api.input().line(),
            api.audio().line(),
            rnd.line(),
        );
        assert!(api.errors.is_empty(), "{:?}", api.errors);
        assert_eq!(out.graphics_revision, g.revision);
        log().push(line);
    }
    // SAFETY: the engine first, then its context; then the probe, which no
    // hook can reach any more.
    unsafe {
        assert_eq!(stickshift_engine_destroy(e), STICKSHIFT_OK);
        assert_eq!(stickshift_context_destroy(ctx), STICKSHIFT_OK);
        Box::from_raw(probe).log
    }
}

fn assert_same_lines(rust: &[String], c: &[String]) {
    for (i, (r, c)) in rust.iter().zip(c).enumerate() {
        assert_eq!(
            r,
            c,
            "line {i} differs; the lines before it:\n{:?}",
            &rust[i.saturating_sub(4)..i]
        );
    }
    assert_eq!(rust.len(), c.len(), "the line counts differ");
}

/// The indices of the lines that start with `prefix`.
fn lines_starting(log: &[String], prefix: &str) -> Vec<usize> {
    (0..log.len())
        .filter(|&i| log[i].starts_with(prefix))
        .collect()
}

fn host_line(log: &[String], f: u32) -> usize {
    lines_starting(log, &format!("F{f} "))[0]
}

#[test]
fn the_probe_prints_the_rust_lines_unhosted_and_hosted() {
    const FRAMES: u32 = 3000;
    for mode in [Mode::Unhosted, Mode::Hosted] {
        let rust = rust_run(mode, FRAMES);
        let c = c_run(mode, FRAMES);
        assert!(
            !c.iter()
                .any(|l| l.starts_with("ERR") || l.starts_with("PANIC")),
            "{mode:?}: {:?}",
            c.iter()
                .filter(|l| l.starts_with("ERR") || l.starts_with("PANIC"))
                .collect::<Vec<_>>()
        );
        assert_same_lines(&rust, &c);

        // What the run went through, so a probe that stopped exercising
        // something would fail here rather than pass vacuously.
        let settings = lines_starting(&c, "S arg=");
        assert_eq!(settings.len(), 9, "{mode:?}: settings changes");
        let updates = lines_starting(&c, "U").len();
        assert!(updates > FRAMES as usize, "{mode:?}: {updates} updates");
        let line = |f| &c[host_line(&c, f)];
        // The update-300 request: the next settings hook sees the renderer and
        // the reach already rescaled (half of the default 60).
        assert!(
            c[settings[0] + 1].contains("reach=30.0"),
            "{}",
            c[settings[0] + 1]
        );
        assert!(
            c[settings[0] + 1].contains(" 0.5 size="),
            "{}",
            c[settings[0] + 1]
        );
        // Full screen is the host's to carry out.
        assert!(line(700).contains(" ev=2 "), "{}", line(700));
        // Frame 1000: the update-1500 request was replaced by the host's Full,
        // and the menu's Shadows Off composed on that, applied inside the push.
        let s = lines_starting(&c, "S arg=[0, 0, 1, 2]");
        assert_eq!(s.len(), 1, "{mode:?}");
        assert!(s[0] > host_line(&c, 999) && s[0] < host_line(&c, 1000));
        assert!(line(999).contains("Some([1, 0, 1, 1])"), "{}", line(999));
        assert!(line(1000).contains(" ev=4 "), "{}", line(1000));
        if mode == Mode::Hosted {
            // The settings hook ran before the resize; the frame after it saw
            // the full-resolution drawable.
            assert!(c[s[0] + 1].contains("size=800x600"), "{}", c[s[0] + 1]);
            assert!(line(1000).contains("size=1600x1200"), "{}", line(1000));
        }
        // Frame 2100: the render hook's pending Near composed with Full.
        let s = lines_starting(&c, "S arg=[2, 0, 0, 2]");
        assert_eq!(s.len(), 1, "{mode:?}");
        assert!(s[0] > host_line(&c, 2099) && s[0] < host_line(&c, 2100));
        // Taps, repeats and focus loss reached the updates as edges.
        let space = |bits: u8| {
            c.iter()
                .filter(|l| l.starts_with('U'))
                .filter(|l| {
                    let keys = &l[l.find("in=[").unwrap() + 4..];
                    keys.split(", ").nth(10).unwrap().parse::<u8>().unwrap() == bits
                })
                .count()
        };
        assert!(space(6) > 0, "a tap shows both edges");
        // Silent audio decoded nothing and counted everything.
        let last = c.last().unwrap();
        assert!(last.contains(" aud=3 0 "), "{last}");
    }
}

// ---------------------------------------------------------------------------
// A small C game for the rest
// ---------------------------------------------------------------------------

type Script = Box<dyn FnMut(At, *mut StickShiftEngine, &mut Vec<String>)>;

/// A C game whose hooks run a script and count settings changes.
#[derive(Default)]
struct Mini {
    settings: Vec<[u32; 4]>,
    seen: Vec<String>,
    script: Option<Script>,
}

unsafe fn mini_run(user: *mut c_void, at: At, e: *mut StickShiftEngine) {
    let m = &mut *user.cast::<Mini>();
    if let Some(mut script) = m.script.take() {
        if catch_unwind(AssertUnwindSafe(|| script(at, e, &mut m.seen))).is_err() {
            m.seen.push(format!("PANIC in {at:?}"));
        }
        m.script = Some(script);
    }
}

unsafe extern "C" fn mini_init(user: *mut c_void, e: *mut StickShiftEngine) -> StickShiftResult {
    mini_run(user, At::Init, e);
    STICKSHIFT_OK
}

unsafe extern "C" fn mini_update(user: *mut c_void, e: *mut StickShiftEngine, _: f32) {
    mini_run(user, At::Update, e);
}

unsafe extern "C" fn mini_render(
    user: *mut c_void,
    e: *mut StickShiftEngine,
    _: *mut StickShiftFrame,
) {
    mini_run(user, At::Render, e);
}

unsafe extern "C" fn mini_settings(
    user: *mut c_void,
    e: *mut StickShiftEngine,
    settings: *const StickShiftGraphicsSettings,
) {
    (*user.cast::<Mini>()).settings.push(from_c(*settings));
    mini_run(user, At::Settings, e);
}

/// A null context with an engine running a [`Mini`] game, destroyed when
/// dropped.
struct Rig {
    ctx: *mut StickShiftContext,
    e: *mut StickShiftEngine,
    mini: *mut Mini,
}

impl Rig {
    fn new(
        desc: StickShiftEngineDesc,
        script: impl FnMut(At, *mut StickShiftEngine, &mut Vec<String>) + 'static,
    ) -> Self {
        let mini = Box::into_raw(Box::new(Mini {
            script: Some(Box::new(script)),
            ..Mini::default()
        }));
        let ctx = null_context(1280, 720);
        let game = StickShiftGame {
            size: std::mem::size_of::<StickShiftGame>() as u32,
            reserved: 0,
            user: mini.cast(),
            init: Some(mini_init),
            update: Some(mini_update),
            render: Some(mini_render),
            graphics_settings_changed: Some(mini_settings),
            release: None,
        };
        let mut e = null_mut();
        // SAFETY: valid locals; the game outlives the engine.
        let r = unsafe { stickshift_engine_create(ctx, &desc, &game, &mut e) };
        assert_eq!(r, STICKSHIFT_OK, "{}", last_error());
        Self { ctx, e, mini }
    }

    fn plain() -> Self {
        Self::new(engine_desc(0, null()), |_, _, _| {})
    }

    fn mini(&self) -> &Mini {
        // SAFETY: between engine calls, no hook holds it.
        unsafe { &*self.mini }
    }

    fn init(&self) {
        // SAFETY: a live engine.
        assert_eq!(
            unsafe { stickshift_engine_init(self.e) },
            STICKSHIFT_OK,
            "{}",
            last_error()
        );
    }

    fn resume(&self, width: u32, height: u32, scale_factor: f64) -> u32 {
        let mut requests = 0;
        // SAFETY: a live engine and valid locals.
        let r = unsafe {
            stickshift_engine_resume(
                self.e,
                &view_desc(width, height, scale_factor),
                &mut requests,
            )
        };
        assert_eq!(r, STICKSHIFT_OK, "{}", last_error());
        requests
    }

    fn tick(&self, clock: StickShiftClock, nanoseconds: u64) -> StickShiftTickResult {
        let mut out = tick_result();
        // SAFETY: a live engine and valid locals.
        let r = unsafe { stickshift_engine_tick(self.e, &tick_desc(clock, nanoseconds), &mut out) };
        assert_eq!(r, STICKSHIFT_OK, "{}", last_error());
        out
    }

    fn graphics(&self) -> StickShiftGraphicsState {
        let (r, state) = graphics_state(self.e);
        assert_eq!(r, STICKSHIFT_OK, "{}", last_error());
        state
    }

    fn request(&self, s: GraphicsSettings) {
        // SAFETY: a live engine and a valid local.
        assert_eq!(
            unsafe { stickshift_graphics_request(self.e, &c_settings(s)) },
            STICKSHIFT_OK
        );
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        // SAFETY: the engine first, then its context; then the game.
        unsafe {
            if !self.e.is_null() {
                assert_eq!(stickshift_engine_destroy(self.e), STICKSHIFT_OK);
            }
            assert_eq!(stickshift_context_destroy(self.ctx), STICKSHIFT_OK);
            drop(Box::from_raw(self.mini));
        }
    }
}

fn menu_event(tag: u32) -> StickShiftHostEvent {
    StickShiftHostEvent {
        kind: STICKSHIFT_HOST_MENU,
        code: tag,
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Menu choices and the clock
// ---------------------------------------------------------------------------

struct Idle;

impl Game for Idle {
    fn init(&mut self, _: &mut EngineCtx) {}
    fn update(&mut self, _: &mut EngineCtx, _: f32) {}
    fn render(&mut self, _: &mut EngineCtx) {}
}

/// A menu choice and `reset_clock` restart the timestamp clock, as the Rust
/// session does: the next tick banks one step, however long the menu was
/// open. Full screen is returned to the host and changes nothing else.
#[test]
fn a_menu_choice_restarts_the_clock_and_full_screen_is_the_hosts() {
    let rig = Rig::plain();
    assert_eq!(rig.resume(1600, 1200, 2.0), STICKSHIFT_REQUEST_MENU_SYNC);
    let mut session = Session::new(Loop::new(), None, false);
    let mut renderer = NullRenderer::new();
    let _ = session.resume(View::new(1600, 1200, 2.0), &mut Idle, &mut renderer);

    enum Step {
        Tick(u64),
        Menu(u32),
        Reset,
    }
    let t0 = 5_000_000_000;
    let script = [
        Step::Tick(t0),
        Step::Tick(t0 + 2 * DT_NS),
        Step::Menu(901),
        Step::Tick(t0 + 2 * DT_NS + 1_000_000_000),
        Step::Tick(t0 + 5 * DT_NS + 1_000_000_000),
        Step::Menu(101),
        Step::Tick(t0 + 20_000_000_000),
        Step::Tick(t0 + 20_000_000_000 + 4 * DT_NS),
        Step::Reset,
        Step::Tick(t0 + 30_000_000_000),
    ];
    let mut seen = Vec::new();
    let mut want = Vec::new();
    for step in script {
        match step {
            Step::Tick(now) => {
                seen.push(format!(
                    "steps {}",
                    rig.tick(STICKSHIFT_CLOCK_TIMESTAMP, now).steps
                ));
                let (steps, _) = session.frame(Clock::Timestamp(now), &mut Idle, &mut renderer);
                want.push(format!("steps {steps}"));
            }
            Step::Menu(tag) => {
                seen.push(format!("menu {}", push(rig.e, &[menu_event(tag)])));
                let command = MenuCommand::from_tag(tag as isize).unwrap();
                let requests = session.event(HostEvent::Menu(command), &mut Idle, &mut renderer);
                want.push(format!("menu {}", requests.bits()));
            }
            Step::Reset => {
                // SAFETY: a live engine, between calls.
                assert_eq!(
                    unsafe { stickshift_engine_reset_clock(rig.e) },
                    STICKSHIFT_OK
                );
                session.reset_clock();
            }
        }
    }
    assert_eq!(seen, want);
    assert_eq!(
        seen,
        [
            "steps 1", "steps 2", "menu 2", "steps 1", "steps 3", "menu 4", "steps 1", "steps 4",
            "steps 1",
        ]
    );
    // Full screen changed no setting; Shadows Off did, once.
    assert_eq!(rig.mini().settings, [[0, 0, 2, 0]]);
    assert_eq!(rig.graphics().revision, 1);
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

/// A defaults suite of the test's own, emptied before and after.
struct Suite(SettingsStore);

impl Suite {
    const NAME: &'static str = "dev.stickshift.capi-tests.persist";

    fn new() -> Self {
        let store = SettingsStore::suite(Self::NAME);
        store.clear();
        Self(store)
    }
}

impl Drop for Suite {
    fn drop(&mut self) {
        self.0.clear();
    }
}

#[test]
fn persisted_settings_round_trip_through_a_throwaway_suite() {
    let suite = Suite::new();
    let name = std::ffi::CString::new(Suite::NAME).unwrap();
    let persist = engine_desc(STICKSHIFT_ENGINE_PERSIST_SETTINGS, name.as_ptr());
    let off_near = GraphicsSettings {
        shadows: ShadowQuality::Off,
        draw_distance: DrawDistance::Near,
        ..GraphicsSettings::default()
    };
    {
        // Nothing saved: the defaults, nothing queued.
        let rig = Rig::new(persist, |_, _, _| {});
        let g = rig.graphics();
        assert_eq!((g.has_pending, g.revision), (0, 0));
        assert_eq!(rust_settings(g.current), GraphicsSettings::default());
        rig.resume(800, 600, 1.0);
        assert_eq!(suite.0.load(), None, "a resume saves nothing");
        // A menu choice is saved at once.
        push(rig.e, &[menu_event(101)]);
        assert_eq!(
            suite.0.load(),
            Some(GraphicsSettings {
                shadows: ShadowQuality::Off,
                ..GraphicsSettings::default()
            })
        );
        // So is the game's own request, when the next frame applies it.
        rig.request(off_near);
        assert_eq!(suite.0.load().unwrap().draw_distance, DrawDistance::Far);
        let out = rig.tick(STICKSHIFT_CLOCK_FIXED_STEP, 0);
        assert_eq!(out.requests, STICKSHIFT_REQUEST_MENU_SYNC);
        assert_eq!(suite.0.load(), Some(off_near));
        assert_eq!(rig.mini().settings.len(), 2);
    }
    {
        // A second engine restores them before init: a restore, not a change.
        let rig = Rig::new(persist, |_, _, _| {});
        let g = rig.graphics();
        assert_eq!((g.has_pending, g.revision), (1, 0));
        assert_eq!(rust_settings(g.pending), off_near);
        rig.init();
        let g = rig.graphics();
        assert_eq!((g.has_pending, g.revision), (0, 0));
        assert_eq!(rust_settings(g.current), off_near);
        assert!(rig.mini().settings.is_empty(), "no hook for a restore");
        // Unhosted, as hosted: the game's own request is saved when the next
        // tick applies it, and so is a menu choice, at once.
        rig.request(GraphicsSettings::default());
        assert_eq!(
            suite.0.load(),
            Some(off_near),
            "a request alone saves nothing"
        );
        let out = rig.tick(STICKSHIFT_CLOCK_FIXED_STEP, 0);
        assert_eq!(
            (out.graphics_revision, out.requests),
            (1, STICKSHIFT_REQUEST_MENU_SYNC)
        );
        assert_eq!(suite.0.load(), Some(GraphicsSettings::default()));
        // A tick that applies nothing saves nothing and asks for no sync.
        assert_eq!(rig.tick(STICKSHIFT_CLOCK_FIXED_STEP, 0).requests, 0);
        push(rig.e, &[menu_event(301)]);
        assert_eq!(
            suite.0.load(),
            Some(GraphicsSettings {
                draw_distance: DrawDistance::Near,
                ..GraphicsSettings::default()
            })
        );
    }
    {
        // Without the flag nothing is restored, and the suite is untouched by
        // a menu choice and an applied request. (That such an engine has no
        // store at all, so it cannot save to the standard defaults either, is
        // checked where the store is visible: the unit test in src/engine.rs.)
        let rig = Rig::plain();
        assert_eq!(rig.graphics().has_pending, 0);
        rig.init();
        let saved = suite.0.load();
        push(rig.e, &[menu_event(102)]);
        rig.request(off_near);
        rig.tick(STICKSHIFT_CLOCK_FIXED_STEP, 0);
        assert_eq!(rig.graphics().revision, 2);
        assert_eq!(suite.0.load(), saved);
    }
}

// ---------------------------------------------------------------------------
// Each service between engine calls
// ---------------------------------------------------------------------------

fn camera_of(e: *mut StickShiftEngine) -> StickShiftCameraState {
    let mut c = StickShiftCameraState {
        size: std::mem::size_of::<StickShiftCameraState>() as u32,
        ..Default::default()
    };
    // SAFETY: a live engine and a valid local.
    assert_eq!(unsafe { stickshift_camera_get(e, &mut c) }, STICKSHIFT_OK);
    c
}

#[test]
fn the_camera_and_chase_are_the_engines() {
    let rig = Rig::plain();
    let mut camera = Loop::new().camera;
    assert_eq!(Cam::c(&camera_of(rig.e)).line(), Cam::rust(&camera).line());
    assert_eq!(camera_of(rig.e).aspect_ratio, 4.0 / 3.0);
    assert_eq!(
        camera_of(rig.e).size as usize,
        std::mem::size_of::<StickShiftCameraState>()
    );

    // SAFETY: a live engine and valid locals.
    unsafe {
        let mut small = StickShiftCameraState::default();
        assert_eq!(
            stickshift_camera_get(rig.e, &mut small),
            STICKSHIFT_ERROR_INVALID_ARGUMENT
        );
        assert_eq!(
            stickshift_camera_get(rig.e, null_mut()),
            STICKSHIFT_ERROR_INVALID_ARGUMENT
        );
        let p = Vec3::new(1.0, 2.0, 3.0);
        stickshift_camera_set_position(rig.e, p.into());
        stickshift_camera_set_target(rig.e, Vec3::new(0.0, 1.0, -4.0).into());
        stickshift_camera_set_up(rig.e, Vec3::new(0.0, 1.0, 0.1).into());
        stickshift_camera_set_aspect_ratio(rig.e, 2.25);
        stickshift_camera_set_fov_y(rig.e, 0.75);
        stickshift_camera_set_clip_planes(rig.e, 0.25, 900.0);
        // Ignored, as in Rust.
        for (near, far) in [
            (-1.0, 10.0),
            (1.0, 1.0),
            (f32::NAN, 5.0),
            (1.0, f32::INFINITY),
        ] {
            assert_eq!(
                stickshift_camera_set_clip_planes(rig.e, near, far),
                STICKSHIFT_OK
            );
        }
        camera.set_position(p);
        camera.set_target(Vec3::new(0.0, 1.0, -4.0));
        camera.set_up(Vec3::new(0.0, 1.0, 0.1));
        camera.set_aspect_ratio(2.25);
        camera.set_fov_y(0.75);
        camera.set_clip_planes(0.25, 900.0);
        assert_eq!(Cam::c(&camera_of(rig.e)).line(), Cam::rust(&camera).line());

        // A zeroed chase (up {0,0,0}) is ChaseController::new's +Y.
        let mut chase = StickShiftChase::default();
        stickshift_chase_init(&mut chase, 6.0, 2.5);
        assert_eq!(chase.up, Vec3::Y.into());
        assert_eq!((chase.look_at_height, chase.half_life), (0.0, 0.0));
        stickshift_chase_init(null_mut(), 1.0, 1.0);
        let zeroed_up = StickShiftChase {
            up: StickShiftVec3::default(),
            ..chase
        };
        // A tilted, unnormalized up: the controller normalizes it.
        let tilted_up = Vec3::new(1.0, 3.0, 0.5);
        let tilted = StickShiftChase {
            up: tilted_up.into(),
            look_at_height: 0.75,
            ..chase
        };
        let rust = ChaseController::new(6.0, 2.5);
        let rust_tilted = ChaseController::new(6.0, 2.5)
            .with_up(tilted_up)
            .with_look_at_height(0.75);
        let smooth = rust_chase();
        for i in 0..60 {
            let target = Vec3::new(i as f32 * 0.3, 0.0, -(i as f32));
            let forward = Vec3::new(0.1, 0.0, -1.0);
            let (c, r) = match i % 4 {
                0 => (&chase, &rust),
                1 => (&zeroed_up, &rust),
                2 => (&tilted, &rust_tilted),
                _ => (&CHASE, &smooth),
            };
            let dt = if i % 5 == 0 { 0.0 } else { FIXED_DT };
            assert_eq!(
                stickshift_camera_follow(rig.e, c, target.into(), forward.into(), dt),
                STICKSHIFT_OK
            );
            r.follow(&mut camera, target, forward, dt);
            assert_eq!(
                Cam::c(&camera_of(rig.e)).line(),
                Cam::rust(&camera).line(),
                "follow {i}"
            );
        }
        assert_eq!(
            stickshift_camera_follow(rig.e, null(), zero3(), zero3(), FIXED_DT),
            STICKSHIFT_ERROR_INVALID_ARGUMENT
        );
        assert_eq!(Cam::c(&camera_of(rig.e)).line(), Cam::rust(&camera).line());
    }
}

fn zero3() -> StickShiftVec3 {
    StickShiftVec3::default()
}

#[test]
fn input_reads_what_the_host_pushed() {
    let rig = Rig::plain();
    let read = || {
        let mut input = StickShiftInputState {
            size: std::mem::size_of::<StickShiftInputState>() as u32,
            ..Default::default()
        };
        // SAFETY: a live engine and a valid local.
        assert_eq!(
            unsafe { stickshift_input_get(rig.e, &mut input) },
            STICKSHIFT_OK
        );
        input
    };
    let none = read();
    assert_eq!(
        none.size as usize,
        std::mem::size_of::<StickShiftInputState>()
    );
    assert_eq!(
        (none.keys, none.mouse_buttons, none.has_cursor),
        ([0; 32], [0; 8], 0)
    );
    push(
        rig.e,
        &[
            c_event(HostEvent::KeyDown {
                key: Key::Q,
                repeat: false,
            }),
            c_event(HostEvent::KeyDown {
                key: Key::Escape,
                repeat: false,
            }),
            c_event(HostEvent::KeyUp(Key::Escape)),
            c_event(HostEvent::MouseDown(MouseButton::Middle)),
            c_event(HostEvent::Cursor(12.5, -3.25)),
        ],
    );
    let input = read();
    let both = STICKSHIFT_KEY_STATE_JUST_PRESSED | STICKSHIFT_KEY_STATE_JUST_RELEASED;
    assert_eq!(
        input.keys[STICKSHIFT_KEY_Q as usize],
        STICKSHIFT_KEY_STATE_DOWN | STICKSHIFT_KEY_STATE_JUST_PRESSED
    );
    assert_eq!(input.keys[STICKSHIFT_KEY_ESCAPE as usize], both);
    assert_eq!(input.keys[12..], [0; 20]);
    assert_eq!(input.mouse_buttons, [0, 0, 1, 0, 0, 0, 0, 0]);
    assert_eq!(
        (input.has_cursor, input.cursor_x, input.cursor_y),
        (1, 12.5, -3.25)
    );
    // An update clears the edges; what is held stays.
    rig.init();
    rig.tick(STICKSHIFT_CLOCK_FIXED_STEP, 0);
    let input = read();
    assert_eq!(
        input.keys[STICKSHIFT_KEY_Q as usize],
        STICKSHIFT_KEY_STATE_DOWN
    );
    assert_eq!(input.keys[STICKSHIFT_KEY_ESCAPE as usize], 0);
    let mut small = StickShiftInputState::default();
    // SAFETY: a live engine and a valid local.
    assert_eq!(
        unsafe { stickshift_input_get(rig.e, &mut small) },
        STICKSHIFT_ERROR_INVALID_ARGUMENT
    );
}

#[test]
fn sounds_are_checked_handles_of_their_engine() {
    let rig = Rig::plain();
    let load = |e, path: &CStr| {
        let mut sound = StickShiftSound { id: 7 };
        // SAFETY: a live engine and valid locals.
        let r = unsafe { stickshift_audio_load(e, path.as_ptr(), &mut sound) };
        (r, sound)
    };
    let (r, a) = load(rig.e, c"sounds/a.ogg");
    assert_eq!(r, STICKSHIFT_OK);
    assert_ne!(a.id, 0);
    assert_eq!(load(rig.e, c"sounds/a.ogg").1, a, "a path loads once");
    // Compared as Rust's `Path`s: a repeated `/`, an interior `.` and a
    // trailing `/` collapse, a leading `./` stays. (get_path, below, still
    // gives the first spelling.)
    for same in [c"sounds//a.ogg", c"sounds/./a.ogg", c"sounds/a.ogg/"] {
        assert_eq!(load(rig.e, same).1, a, "{same:?} is the same path");
    }
    let (_, b) = load(rig.e, c"./sounds/a.ogg");
    assert_ne!(b, a, "a leading ./ is a different path, as in Rust");
    // SAFETY: a live engine and valid locals.
    unsafe {
        let mut sound = StickShiftSound { id: 7 };
        assert_eq!(
            stickshift_audio_load(rig.e, null(), &mut sound),
            STICKSHIFT_ERROR_INVALID_ARGUMENT
        );
        assert_eq!(sound.id, 0, "the output is cleared on failure");
        let bad = [0xFFu8, 0xFE, 0];
        assert_eq!(
            stickshift_audio_load(rig.e, bad.as_ptr().cast(), &mut sound),
            STICKSHIFT_ERROR_INVALID_ARGUMENT
        );
        assert_eq!(
            stickshift_audio_load(rig.e, c"x".as_ptr(), null_mut()),
            STICKSHIFT_ERROR_INVALID_ARGUMENT
        );

        let mut path = null();
        assert_eq!(
            stickshift_audio_get_path(rig.e, a, &mut path),
            STICKSHIFT_OK
        );
        assert_eq!(CStr::from_ptr(path), c"sounds/a.ogg", "the first spelling");
        stickshift_audio_play_once(rig.e, a, -6.0);
        stickshift_audio_play_once(rig.e, a, -6.0);
        stickshift_audio_play_looping(rig.e, b, 0.0);
        stickshift_audio_play_looping(rig.e, b, 0.0);
        stickshift_audio_set_master_volume(rig.e, STICKSHIFT_VOLUME_SILENT_DB);
        let mut n = 0;
        assert_eq!(
            stickshift_audio_one_shots_of(rig.e, a, &mut n),
            STICKSHIFT_OK
        );
        assert_eq!(n, 2);
        let mut stats = StickShiftAudioStats {
            size: std::mem::size_of::<StickShiftAudioStats>() as u32,
            ..Default::default()
        };
        assert_eq!(stickshift_audio_get_stats(rig.e, &mut stats), STICKSHIFT_OK);
        assert_eq!(
            (
                stats.sound_count,
                stats.decode_count,
                stats.loop_starts,
                stats.one_shots_played
            ),
            (2, 0, 1, 2)
        );
        assert_eq!(
            (stats.master_volume_db, stats.looping, stats.is_silent),
            (-60.0, b, 1)
        );
        stickshift_audio_stop_looping(rig.e);
        stickshift_audio_get_stats(rig.e, &mut stats);
        assert_eq!(stats.looping.id, 0);

        // Not this engine's: null, another kind, another engine's, a
        // destroyed engine's on the same context.
        let other = Rig::plain();
        let (_, theirs) = load(other.e, c"sounds/a.ogg");
        let mesh_like = StickShiftSound {
            id: (1u64 << 60) | 1,
        };
        for bad in [
            StickShiftSound::default(),
            mesh_like,
            theirs,
            StickShiftSound { id: a.id + 1 },
        ] {
            assert_eq!(
                stickshift_audio_play_once(rig.e, bad, 0.0),
                STICKSHIFT_ERROR_INVALID_HANDLE,
                "{bad:?}"
            );
            assert_eq!(
                stickshift_audio_play_looping(rig.e, bad, 0.0),
                STICKSHIFT_ERROR_INVALID_HANDLE
            );
            assert_eq!(
                stickshift_audio_one_shots_of(rig.e, bad, &mut n),
                STICKSHIFT_ERROR_INVALID_HANDLE
            );
            assert_eq!(
                stickshift_audio_get_path(rig.e, bad, &mut path),
                STICKSHIFT_ERROR_INVALID_HANDLE
            );
            assert!(path.is_null());
        }
        // A sound is not a context resource.
        assert_eq!(
            stickshift_resource_state(rig.ctx, a.id),
            STICKSHIFT_RESOURCE_STATE_INVALID
        );

        let ctx = other.ctx;
        let mini = other.mini;
        let first = std::mem::ManuallyDrop::new(other);
        assert_eq!(stickshift_engine_destroy(first.e), STICKSHIFT_OK);
        let game = StickShiftGame {
            size: std::mem::size_of::<StickShiftGame>() as u32,
            reserved: 0,
            user: mini.cast(),
            init: Some(mini_init),
            update: Some(mini_update),
            render: Some(mini_render),
            graphics_settings_changed: None,
            release: None,
        };
        let mut second = null_mut();
        assert_eq!(
            stickshift_engine_create(ctx, &engine_desc(0, null()), &game, &mut second),
            STICKSHIFT_OK
        );
        assert_eq!(
            stickshift_audio_play_once(second, theirs, 0.0),
            STICKSHIFT_ERROR_INVALID_HANDLE
        );
        let (_, again) = load(second, c"sounds/a.ogg");
        assert_ne!(again, theirs, "a new engine issues new handles");
        assert_eq!(stickshift_engine_destroy(second), STICKSHIFT_OK);
        assert_eq!(stickshift_context_destroy(ctx), STICKSHIFT_OK);
        drop(Box::from_raw(mini));
    }
}

#[test]
fn the_graphics_functions_match_rust() {
    let rig = Rig::plain();
    // SAFETY: valid locals throughout.
    unsafe {
        let mut s = StickShiftGraphicsSettings {
            shadows: 9,
            ..Default::default()
        };
        stickshift_graphics_settings_default(&mut s);
        assert_eq!(s, c_settings(GraphicsSettings::default()));
        stickshift_graphics_settings_default(null_mut());

        let all = (0..81).map(|i| StickShiftGraphicsSettings {
            shadows: i % 3,
            shadow_distance: i / 3 % 3,
            draw_distance: i / 9 % 3,
            render_scale: i / 27 % 3,
            reserved: [0; 4],
        });
        for c in all {
            let rust = rust_settings(c);
            let mut rs = StickShiftRenderSettings {
                shadows: 9,
                shadow_distance: 9.0,
                draw_distance_scale: 9.0,
            };
            assert_eq!(
                stickshift_graphics_settings_render_settings(&c, &mut rs),
                STICKSHIFT_OK
            );
            assert_eq!(rs, rust.render_settings().into());
            let mut codes = [0i64; 4];
            assert_eq!(
                stickshift_graphics_settings_to_codes(&c, codes.as_mut_ptr()),
                STICKSHIFT_OK
            );
            assert_eq!(codes, rust.to_codes());
            let mut back = StickShiftGraphicsSettings::default();
            stickshift_graphics_settings_from_codes(codes.as_ptr(), &mut back);
            assert_eq!(back, c);
        }
        let mut back = StickShiftGraphicsSettings::default();
        for codes in [[0i64; 4], [i64::MIN; 4], [i64::MAX, -1, 4, 99]] {
            stickshift_graphics_settings_from_codes(codes.as_ptr(), &mut back);
            assert_eq!(back, c_settings(GraphicsSettings::default()), "{codes:?}");
        }
        stickshift_graphics_settings_from_codes(null(), &mut back);

        let default = c_settings(GraphicsSettings::default());
        let mut rs = StickShiftRenderSettings {
            shadows: 0,
            shadow_distance: 0.0,
            draw_distance_scale: 0.0,
        };
        let mut codes = [0i64; 4];
        for bad in [
            StickShiftGraphicsSettings {
                shadows: 3,
                ..default
            },
            StickShiftGraphicsSettings {
                shadow_distance: 3,
                ..default
            },
            StickShiftGraphicsSettings {
                draw_distance: 3,
                ..default
            },
            StickShiftGraphicsSettings {
                render_scale: 3,
                ..default
            },
            StickShiftGraphicsSettings {
                reserved: [0, 0, 0, 1],
                ..default
            },
        ] {
            let ia = STICKSHIFT_ERROR_INVALID_ARGUMENT;
            assert_eq!(
                stickshift_graphics_settings_render_settings(&bad, &mut rs),
                ia
            );
            assert_eq!(
                stickshift_graphics_settings_to_codes(&bad, codes.as_mut_ptr()),
                ia
            );
            assert_eq!(stickshift_graphics_request(rig.e, &bad), ia);
        }
        assert_eq!(
            stickshift_graphics_request(rig.e, null()),
            STICKSHIFT_ERROR_INVALID_ARGUMENT
        );
        assert_eq!(
            rig.graphics().has_pending,
            0,
            "a refused request queues nothing"
        );

        for scale in RenderScale::ALL {
            for (w, h, f) in [
                (1600, 1200, 2.0),
                (1, 1, 1.0),
                (1333, 777, 1.5),
                (5, 3, 3.0),
            ] {
                let (mut ow, mut oh, mut ppp) = (0, 0, 0.0);
                let r = stickshift_render_scale_surface_for(
                    c4(GraphicsSettings {
                        render_scale: scale,
                        ..GraphicsSettings::default()
                    })[3],
                    w,
                    h,
                    f,
                    &mut ow,
                    &mut oh,
                    &mut ppp,
                );
                assert_eq!(r, STICKSHIFT_OK);
                assert_eq!((ow, oh, ppp.to_bits()), {
                    let (a, b, c) = scale.surface_for(w, h, f);
                    (a, b, c.to_bits())
                });
            }
        }
        let (mut ow, mut oh, mut ppp) = (0, 0, 0.0);
        assert_eq!(
            stickshift_render_scale_surface_for(3, 10, 10, 1.0, &mut ow, &mut oh, &mut ppp),
            STICKSHIFT_ERROR_INVALID_ARGUMENT
        );

        let label = |section, value| {
            let p = stickshift_graphics_label(section, value);
            (!p.is_null()).then(|| CStr::from_ptr(p).to_str().unwrap().to_owned())
        };
        assert_eq!(
            label(
                STICKSHIFT_GRAPHICS_SECTION_SHADOWS,
                STICKSHIFT_SHADOW_QUALITY_OFF
            )
            .as_deref(),
            Some("Off")
        );
        assert_eq!(
            label(
                STICKSHIFT_GRAPHICS_SECTION_SHADOWS,
                STICKSHIFT_SHADOW_QUALITY_HIGH
            )
            .as_deref(),
            Some("High")
        );
        assert_eq!(
            label(
                STICKSHIFT_GRAPHICS_SECTION_SHADOW_DISTANCE,
                STICKSHIFT_SHADOW_DISTANCE_MATCH_DRAW_DISTANCE
            )
            .as_deref(),
            Some("Match Draw Distance")
        );
        assert_eq!(
            label(
                STICKSHIFT_GRAPHICS_SECTION_DRAW_DISTANCE,
                STICKSHIFT_DRAW_DISTANCE_FAR
            )
            .as_deref(),
            Some("Far")
        );
        assert_eq!(
            label(
                STICKSHIFT_GRAPHICS_SECTION_RENDER_SCALE,
                STICKSHIFT_RENDER_SCALE_THREE_QUARTERS
            )
            .as_deref(),
            Some("75%")
        );
        for (section, value) in [(0, 0), (5, 0), (1, 3), (u32::MAX, 0), (4, u32::MAX)] {
            assert_eq!(label(section, value), None, "{section} {value}");
        }
    }
}

/// The menu as a host sees it: `(title, [(tag, label, checked, enabled)])` per
/// group.
type MenuData = (
    String,
    u64,
    Vec<(Option<String>, Vec<(i32, String, u8, u8)>)>,
);

fn menu_of(e: *mut StickShiftEngine, fullscreen: u8) -> MenuData {
    let mut menu = StickShiftMenu {
        size: std::mem::size_of::<StickShiftMenu>() as u32,
        group_count: 0,
        title: null(),
        groups: null(),
        group_stride: 0,
        reserved: 0,
        revision: 0,
    };
    let text = |p: *const std::ffi::c_char| {
        // SAFETY: engine-owned strings, valid until the next get_menu.
        (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_str().unwrap().to_owned())
    };
    // SAFETY: a live engine and a valid local; the menu's arrays are valid
    // until the next call, with the strides it gives.
    unsafe {
        assert_eq!(
            stickshift_engine_get_menu(e, fullscreen, &mut menu),
            STICKSHIFT_OK,
            "{}",
            last_error()
        );
        let groups = (0..menu.group_count as usize)
            .map(|g| {
                let group = menu
                    .groups
                    .cast::<u8>()
                    .add(g * menu.group_stride as usize)
                    .cast::<StickShiftMenuGroup>()
                    .read();
                let items = (0..group.item_count as usize)
                    .map(|i| {
                        let item = group
                            .items
                            .cast::<u8>()
                            .add(i * group.item_stride as usize)
                            .cast::<StickShiftMenuItem>()
                            .read();
                        (
                            item.tag,
                            text(item.label).unwrap(),
                            item.checked,
                            item.enabled,
                        )
                    })
                    .collect();
                (text(group.title), items)
            })
            .collect();
        (text(menu.title).unwrap(), menu.revision, groups)
    }
}

#[test]
fn the_menu_is_data_built_from_the_engines_model() {
    let rig = Rig::plain();
    let (title, revision, groups) = menu_of(rig.e, 0);
    assert_eq!((title.as_str(), revision), ("Graphics", 0));
    let titles: Vec<_> = groups.iter().map(|g| g.0.clone()).collect();
    assert_eq!(
        titles,
        [
            Some("Shadows"),
            Some("Shadow Distance"),
            Some("Draw Distance"),
            Some("Resolution"),
            None
        ]
        .map(|t| t.map(str::to_owned))
    );
    let tags: Vec<Vec<i32>> = groups
        .iter()
        .map(|g| g.1.iter().map(|i| i.0).collect())
        .collect();
    assert_eq!(
        tags,
        [
            vec![101, 102, 103],
            vec![201, 202, 203],
            vec![301, 302, 303],
            vec![401, 402, 403],
            vec![901, 902]
        ]
    );
    let checked: Vec<i32> = groups
        .iter()
        .flat_map(|g| &g.1)
        .filter(|i| i.2 == 1)
        .map(|i| i.0)
        .collect();
    assert_eq!(checked, [103, 201, 303, 401], "the defaults");
    let actions = &groups[4].1;
    assert_eq!(
        (actions[0].1.as_str(), actions[0].3),
        ("Enter Full Screen", 1)
    );
    assert_eq!(
        (actions[1].1.as_str(), actions[1].3),
        ("Reset to Defaults", 0),
        "nothing to reset"
    );
    assert_eq!(menu_of(rig.e, 1).2[4].1[0].1, "Exit Full Screen");
    // Each item is the Rust model's.
    for (_, items) in &groups {
        for (tag, label, checked, enabled) in items {
            let command = MenuCommand::from_tag(*tag as isize).unwrap();
            let s = GraphicsSettings::default();
            assert_eq!(
                (label.as_str(), *checked == 1, *enabled == 1),
                (
                    command.label(false),
                    command.is_checked(&s),
                    command.is_enabled(&s)
                )
            );
        }
    }

    // Shadows Off: the range is disabled, reset enabled, the revision moves.
    rig.init();
    assert_eq!(
        push(rig.e, &[menu_event(101)]),
        STICKSHIFT_REQUEST_MENU_SYNC
    );
    let (_, revision, groups) = menu_of(rig.e, 0);
    assert_eq!(revision, 1);
    assert_eq!(
        groups[0].1.iter().map(|i| i.2).collect::<Vec<_>>(),
        [1, 0, 0]
    );
    assert!(groups[1].1.iter().all(|i| i.3 == 0));
    assert_eq!(groups[4].1[1].3, 1);

    // A queued request is not in effect: the menu keeps the current checkmarks
    // and revision until the next frame applies it, as the Rust host's menu
    // syncs from the settings in effect.
    let draw_distance = |(_, revision, groups): MenuData| {
        let checked = groups[2].1.iter().filter(|i| i.2 == 1).map(|i| i.0);
        (revision, checked.collect::<Vec<_>>())
    };
    rig.request(GraphicsSettings {
        shadows: ShadowQuality::Off,
        draw_distance: DrawDistance::Near,
        ..GraphicsSettings::default()
    });
    assert_eq!(draw_distance(menu_of(rig.e, 0)), (1, vec![303]), "current");
    rig.tick(STICKSHIFT_CLOCK_FIXED_STEP, 0);
    assert_eq!(draw_distance(menu_of(rig.e, 0)), (2, vec![301]), "applied");

    // SAFETY: a live engine and valid locals.
    unsafe {
        let mut small = StickShiftMenu {
            size: 8,
            group_count: 0,
            title: null(),
            groups: null(),
            group_stride: 0,
            reserved: 0,
            revision: 0,
        };
        assert_eq!(
            stickshift_engine_get_menu(rig.e, 0, &mut small),
            STICKSHIFT_ERROR_INVALID_ARGUMENT
        );
        assert_eq!(
            stickshift_engine_get_menu(rig.e, 2, &mut small),
            STICKSHIFT_ERROR_INVALID_ARGUMENT
        );
        assert_eq!(
            stickshift_engine_get_menu(rig.e, 0, null_mut()),
            STICKSHIFT_ERROR_INVALID_ARGUMENT
        );
    }
    // Every tag the menu shows is a choice the engine takes.
    for tag in tags.iter().flatten() {
        let mut requests = 0;
        let event = menu_event(*tag as u32);
        // SAFETY: a live engine and a valid local.
        let r = unsafe { stickshift_engine_push_events(rig.e, &event, 1, 64, &mut requests) };
        assert_eq!(r, STICKSHIFT_OK, "{tag}");
    }
    assert_eq!(
        rust_settings(rig.graphics().current),
        GraphicsSettings::default(),
        "Reset was last"
    );
}
