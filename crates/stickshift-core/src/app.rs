// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! The windowed entry: run a [`Game`] inside a `winit` window on
//! macOS.
//!
//! This is the winit half of the engine loop, behind the default `windowed`
//! feature. It opens a window, translates `winit` events into the engine's
//! backend-agnostic [`HostEvent`]s, and hands them to a [`Session`]: the
//! platform-free runner every host shares, which drives the *same* shared
//! [`drive_frame`](crate::driver::drive_frame) as the headless driver. The only difference is the clock. Here the per-frame
//! elapsed time comes from a real monotonic [`Instant`](std::time::Instant)
//! ([`Clock::Engine`]), so the fixed-timestep
//! [`Accumulator`](crate::timestep::Accumulator) may run 0..N `update`s before the
//! single `render`; per-frame drawing goes through the backend the game binary
//! supplies via a **backend factory**.
//!
//! What only a window can do, the session asks for with [`Requests`]: this
//! runner exits the event loop, toggles full screen, and refreshes the AppKit
//! menu.
//!
//! # The backend seam (metal stays out of `stickshift-core`)
//!
//! `stickshift-core` must never depend on `metal` or on the concrete `stickshift-render`
//! crate (ARCHITECTURE §2, enforced by CI). So the windowed entry does not
//! construct a backend itself: [`run_with_backend`] takes a **factory** (a
//! `FnOnce(&Window, u32, u32) -> Box<dyn Renderer>`) that the *game binary*
//! (which does depend on `stickshift-render`) provides. The factory is invoked once
//! in `resumed`, right after the window is created, and
//! the returned `Box<dyn Renderer>` drives every frame through the seam. The
//! headless driver and the plain [`run`] entry keep the GPU-free
//! [`NullRenderer`].
//!
//! # Audio
//!
//! This is also the only entry that opens an **audio output device**: it replaces
//! the loop's silent [`Audio`](stickshift_audio::Audio) layer with
//! [`Audio::with_output_device`](stickshift_audio::Audio::with_output_device) at
//! startup. The headless driver keeps the silent layer, so tests and `--smoke`
//! never touch audio hardware; and since the device-bound constructor falls back
//! to silence when there is no output, even this path cannot fail on that account.
//!
//! # Graphics settings and the built-in menu
//!
//! By default the windowed entry installs a native **Graphics** menu in the
//! macOS menu bar (Shadows, Shadow Distance, Draw Distance, Resolution, Enter Full
//! Screen, Reset to Defaults). It also restores the player's graphics settings
//! from the last launch and saves them on every change (`NSUserDefaults`). Both
//! are on by default and can each be turned off with [`RunConfig`] via
//! [`run_with_config`]. A game with its own settings UI turns the menu off and
//! drives the same [`GraphicsSettings`](crate::GraphicsSettings) through
//! [`EngineCtx::set_graphics_settings`](crate::EngineCtx::set_graphics_settings).
//!
//! Whatever made the change, this runner reacts to it the same way:
//!
//! - it resizes the drawable for the render scale;
//! - it refreshes the menu's checkmarks;
//! - it persists the new value.
//!
//! The headless driver never touches the menu bar or stored preferences.
//!
//! # macOS isolation
//!
//! The window-creation and event-translation helpers are kept as small free
//! functions so a `#[cfg(target_os = ...)]` platform split can wrap them without
//! disturbing the loop structure. The engine loop itself
//! (`drive_frame`) is platform-neutral.

use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton as WinitMouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Fullscreen, Window, WindowId};

use stickshift_render_api::NullRenderer;

use crate::context::Renderer;
use crate::driver::Loop;
use crate::game::Game;
use crate::input::{Key, MouseButton};
use crate::platform::SettingsMenu;
use crate::record::{Recording, VideoError, VideoSummary};
use crate::session::{Clock, HostEvent, Requests, Session, SettingsStore, View};
use crate::settings_menu::MenuCommand;

/// A factory that constructs the render backend for a freshly-created window.
///
/// The engine calls this once, in `resumed`, after the
/// window exists, passing the window plus its pixel size. The game binary (the
/// only place that depends on `stickshift-render`) returns a boxed
/// [`Renderer`] — a type that is both a
/// [`RenderDevice`](stickshift_render_api::RenderDevice) and a
/// [`FrameRecorder`](stickshift_render_api::FrameRecorder). This is the single seam
/// that keeps `metal` out of `stickshift-core`'s dependency tree while still letting
/// the windowed loop drive a real GPU backend.
pub type BackendFactory<'f> = Box<dyn FnOnce(&Window, u32, u32) -> Box<dyn Renderer> + 'f>;

/// Open a window and run `game` against a headless [`NullRenderer`] until the
/// window is closed (macOS).
///
/// This convenience entry wires **no GPU backend**; the window is present but
/// nothing is drawn to it (the game still records into the null seam). Use
/// [`run_with_backend`] to supply a real backend. Kept for tests and callers
/// that only need the input/loop bring-up.
///
/// # Panics
///
/// Panics if the platform event loop cannot be created (no display available).
///
/// # Example
///
/// ```no_run
/// use stickshift_core::{Game, EngineCtx, run};
///
/// struct MyGame;
/// impl Game for MyGame {
///     fn init(&mut self, _: &mut EngineCtx) {}
///     fn update(&mut self, _: &mut EngineCtx, _dt: f32) {}
///     fn render(&mut self, _: &mut EngineCtx) {}
/// }
///
/// run(&mut MyGame);
/// ```
pub fn run<G: Game>(game: &mut G) {
    run_with_config(game, None, RunConfig::default());
}

/// Open a window and run `game`, constructing the render backend from `factory`.
///
/// Identical to [`run`] except that the engine calls `factory` once after the
/// window is created and drives every frame through the returned
/// `Box<dyn Renderer>` — the real GPU present path. The factory is the seam that
/// lets the game binary inject a `metal` backend without `stickshift-core` depending
/// on `metal`.
///
/// # Panics
///
/// Panics if the platform event loop cannot be created (no display available).
///
/// # Example
///
/// ```no_run
/// use stickshift_core::{Game, EngineCtx, run_with_backend, Renderer};
/// use stickshift_render_api::NullRenderer;
///
/// struct MyGame;
/// impl Game for MyGame {
///     fn init(&mut self, _: &mut EngineCtx) {}
///     fn update(&mut self, _: &mut EngineCtx, _dt: f32) {}
///     fn render(&mut self, _: &mut EngineCtx) {}
/// }
///
/// // A real game passes a Metal backend here; this doc uses NullRenderer.
/// run_with_backend(&mut MyGame, Box::new(|_win, _w, _h| {
///     Box::new(NullRenderer::new()) as Box<dyn Renderer>
/// }));
/// ```
pub fn run_with_backend<G: Game>(game: &mut G, factory: BackendFactory<'_>) {
    run_with_config(game, Some(factory), RunConfig::default());
}

/// Options for the windowed entry. [`run`] and [`run_with_backend`] use
/// [`RunConfig::default`], where everything is on.
///
/// # Example
///
/// A game that ships its own graphics-settings UI turns the built-in menu off.
/// It keeps persistence, and drives the settings through
/// [`EngineCtx::set_graphics_settings`](crate::EngineCtx::set_graphics_settings):
///
/// ```no_run
/// use stickshift_core::{run_with_config, EngineCtx, Game, RunConfig};
///
/// struct MyGame;
/// impl Game for MyGame {
///     fn init(&mut self, _: &mut EngineCtx) {}
///     fn update(&mut self, _: &mut EngineCtx, _dt: f32) {}
///     fn render(&mut self, _: &mut EngineCtx) {}
/// }
///
/// run_with_config(
///     &mut MyGame,
///     None, // or Some(backend factory), as with `run_with_backend`
///     RunConfig {
///         native_settings_menu: false,
///         ..RunConfig::default()
///     },
/// );
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunConfig {
    /// Install the built-in **Graphics** menu in the macOS menu bar. Default
    /// `true`. Turn it off when the game provides its own settings UI.
    pub native_settings_menu: bool,
    /// Restore the graphics settings from the previous launch at start-up and
    /// save them whenever they change (in `NSUserDefaults` on macOS). Default
    /// `true`. This covers every change, whether it came from the built-in menu
    /// or from the game's own UI.
    pub persist_graphics_settings: bool,
    /// Quit when Escape is pressed. Default `true`. Turn it off to use Escape
    /// in the game (a pause menu, closing a dialog); the key then reaches the
    /// game like any other, and the game quits with
    /// [`EngineCtx::request_exit`](crate::EngineCtx::request_exit). The close
    /// button and Cmd-Q quit either way.
    pub quit_on_escape: bool,
    /// Record what the window shows to an H.264 movie for the whole run (see
    /// [`Recording`]). Default `None`. When the run ends, the runner finishes
    /// the movie and reports it on stderr: where it is, its size, how many
    /// frames it holds and how many were dropped (or why it failed).
    pub record: Option<Recording>,
}

impl Default for RunConfig {
    fn default() -> Self {
        Self {
            native_settings_menu: true,
            persist_graphics_settings: true,
            quit_on_escape: true,
            record: None,
        }
    }
}

/// Open a window and run `game` with explicit [`RunConfig`] options.
///
/// `factory` is the render backend factory, exactly as for [`run_with_backend`].
/// `None` runs against the headless [`NullRenderer`], as [`run`] does.
///
/// # Panics
///
/// Panics if the platform event loop cannot be created (no display available).
pub fn run_with_config<G: Game>(
    game: &mut G,
    factory: Option<BackendFactory<'_>>,
    config: RunConfig,
) {
    let event_loop = EventLoop::<EngineEvent>::with_user_event()
        .build()
        .expect("failed to create winit event loop");
    let proxy = event_loop.create_proxy();
    let mut app: EngineApp<'_, '_, G> = EngineApp::new(game, factory, config, proxy);
    event_loop
        .run_app(&mut app)
        .expect("winit event loop failed");
}

/// Events posted into the winit loop from outside it: today, a click on the
/// built-in Graphics menu, sent from the menu's action target through an
/// [`EventLoopProxy`].
#[derive(Debug, Clone, Copy)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) enum EngineEvent {
    /// A Graphics menu item was chosen.
    Menu(MenuCommand),
}

/// Window title used by the windowed entry.
const WINDOW_TITLE: &str = "Stick Shift Engine";
/// Initial window width in logical pixels.
const WINDOW_WIDTH: u32 = 800;
/// Initial window height in logical pixels.
const WINDOW_HEIGHT: u32 = 600;

/// The `winit` [`ApplicationHandler`] that drives the game: an adapter over a
/// [`Session`].
///
/// The session holds the shared [`Loop`] (the scene and its ECS
/// [`World`](stickshift_ecs::hecs::World), input, perf, and the fixed-timestep
/// [`Accumulator`](crate::timestep::Accumulator)) and everything a window adds to
/// it: the frame clock, the drawable size, saved settings, menu choices. This
/// runner owns the window, the render seam, the game reference and the AppKit
/// menu; it turns winit's events into [`HostEvent`]s and carries out the
/// [`Requests`] that come back. The render seam is a `Box<dyn Renderer>`: a
/// [`NullRenderer`] until (and unless) the backend factory replaces it with a
/// real backend on first resume.
struct EngineApp<'g, 'f, G: Game> {
    game: &'g mut G,
    window: Option<Window>,
    session: Session,
    renderer: Box<dyn Renderer>,
    factory: Option<BackendFactory<'f>>,
    /// Runner options.
    config: RunConfig,
    /// Where the built-in menu posts clicks; handed to the menu when it is
    /// installed.
    proxy: EventLoopProxy<EngineEvent>,
    /// The built-in Graphics menu, once installed (and only if enabled).
    menu: Option<SettingsMenu>,
}

impl<'g, 'f, G: Game> EngineApp<'g, 'f, G> {
    fn new(
        game: &'g mut G,
        factory: Option<BackendFactory<'f>>,
        config: RunConfig,
        proxy: EventLoopProxy<EngineEvent>,
    ) -> Self {
        // A fresh `Loop` is silent (it opens no audio device, which is what keeps
        // headless runs and tests quiet). The windowed entry is the one place that
        // *wants* speakers, so bind the mixer to the system's default output here —
        // exactly once, at startup, before the game's `init` can load a sound.
        //
        // `with_output_device` cannot fail: a machine with no output device falls
        // back to the same silent no-op, so a windowed run on a headless CI box
        // still runs, just quietly. Audio is intentionally *not* deferred to
        // `resumed` like the render backend is — it needs no window.
        let mut lp = Loop::new();
        lp.audio = stickshift_audio::Audio::with_output_device();

        // With persistence, the session restores last launch's graphics
        // settings. Queued, so they take effect before `Game::init` (see
        // `Loop::init_once`).
        let store = config
            .persist_graphics_settings
            .then(SettingsStore::standard);
        let mut session = Session::new(lp, store, config.quit_on_escape);
        if let Some(recording) = config.record.clone() {
            session.record(recording);
        }

        Self {
            game,
            window: None,
            session,
            renderer: Box::new(NullRenderer::new()),
            factory,
            config,
            proxy,
            menu: None,
        }
    }

    /// The window's native pixels and scale factor, as the session sizes the
    /// drawable from them.
    fn view_of(window: &Window) -> View {
        let size = window.inner_size();
        View::new(size.width, size.height, window.scale_factor())
    }

    /// Hand `event` to the session, then carry out what it asks for.
    fn event(&mut self, event_loop: &ActiveEventLoop, event: HostEvent) {
        let requests = self.session.event(event, self.game, self.renderer.as_mut());
        self.carry_out(event_loop, requests);
    }

    /// Do what the session asked: what only the window can.
    fn carry_out(&self, event_loop: &ActiveEventLoop, requests: Requests) {
        if requests.contains(Requests::TOGGLE_FULLSCREEN) {
            self.toggle_fullscreen();
        }
        if requests.contains(Requests::MENU_SYNC) {
            self.sync_menu();
        }
        if requests.contains(Requests::EXIT) {
            event_loop.exit();
        }
    }

    /// Refresh the built-in menu's checkmarks, enabled states and full-screen
    /// title.
    fn sync_menu(&self) {
        if let Some(menu) = &self.menu {
            let fullscreen = self
                .window
                .as_ref()
                .is_some_and(|w| w.fullscreen().is_some());
            menu.sync(&self.session.lp.graphics.current(), fullscreen);
        }
    }

    /// Enter or leave (borderless, native-Space) full screen.
    fn toggle_fullscreen(&self) {
        if let Some(window) = &self.window {
            let next = match window.fullscreen() {
                Some(_) => None,
                None => Some(Fullscreen::Borderless(None)),
            };
            window.set_fullscreen(next);
        }
    }
}

impl<G: Game> ApplicationHandler<EngineEvent> for EngineApp<'_, '_, G> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let window = match self.window.take() {
            Some(window) => window,
            None => {
                let window = create_window(event_loop);
                // Construct the real backend now that a window exists.
                // Consuming the factory (an `Option`) makes this happen exactly
                // once.
                if let Some(factory) = self.factory.take() {
                    let size = window.inner_size();
                    self.renderer = factory(&window, size.width, size.height);
                }
                window.request_redraw();
                window
            }
        };
        let view = Self::view_of(&window);
        self.window = Some(window);

        // On first resume the session matches the engine camera's aspect to the
        // real window before the first frame (resize events keep it current
        // thereafter). Then it sizes the drawable for the render scale, runs the
        // game's one-time init against the live seam, and sizes the drawable
        // again for the settings now in effect.
        let requests = self.session.resume(view, self.game, self.renderer.as_mut());

        // Add the built-in Graphics menu next to winit's application menu,
        // before the session's request to show the settings on it.
        if self.config.native_settings_menu && self.menu.is_none() {
            self.menu = SettingsMenu::install(self.proxy.clone());
        }
        self.carry_out(event_loop, requests);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: EngineEvent) {
        match event {
            // A settings change is applied right away, between frames, and the
            // frame clock restarts: AppKit's menu tracking held the main thread
            // while the menu was open.
            EngineEvent::Menu(command) => self.event(event_loop, HostEvent::Menu(command)),
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        // A movie is only playable once finished: finish it here, while the
        // renderer still holds the frames on their way back from the GPU.
        if let Some(result) = self.session.finish_recording(self.renderer.as_mut()) {
            report_recording(&result);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => self.event(event_loop, HostEvent::Close),
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    if let Some(key) = translate_key(code) {
                        // Escape quits (unless the game handles it) and still
                        // reaches the input: the session decides.
                        let host = if event.state.is_pressed() {
                            HostEvent::KeyDown {
                                key,
                                repeat: event.repeat,
                            }
                        } else {
                            HostEvent::KeyUp(key)
                        };
                        self.event(event_loop, host);
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if let Some(button) = translate_mouse_button(button) {
                    let host = match state {
                        ElementState::Pressed => HostEvent::MouseDown(button),
                        ElementState::Released => HostEvent::MouseUp(button),
                    };
                    self.event(event_loop, host);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let scale = self.window.as_ref().map_or(1.0, |w| w.scale_factor());
                let (x, y) = cursor_to_logical(position, scale);
                self.event(event_loop, HostEvent::Cursor(x, y));
            }
            WindowEvent::Focused(false) => {
                // macOS sends no key-up for a key released while another window
                // has focus; let go of everything so nothing stays held.
                self.event(event_loop, HostEvent::FocusLost);
            }
            WindowEvent::Resized(size) => {
                // The camera's aspect follows the window, the drawable is
                // resized, and the menu's full-screen title refreshed (the
                // window may have entered or left full screen).
                let scale = self.window.as_ref().map_or(1.0, |w| w.scale_factor());
                let view = View::new(size.width, size.height, scale);
                self.event(event_loop, HostEvent::Resized(view));
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                // Moved to a display with a different pixel density.
                if let Some(window) = &self.window {
                    let view = Self::view_of(window);
                    self.event(event_loop, HostEvent::ScaleChanged(view));
                }
            }
            WindowEvent::RedrawRequested => {
                // Applies a graphics change the game queued, and resizes for
                // it, before the frame (see `Session::frame`).
                let (_, requests) =
                    self.session
                        .frame(Clock::Engine, self.game, self.renderer.as_mut());
                self.carry_out(event_loop, requests);
                if requests.contains(Requests::EXIT) {
                    return;
                }
                // Request the next frame to keep the loop pumping.
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            _ => {}
        }
    }
}

// --- macOS-isolated helpers -------------------------------------------------
//
// These are deliberately small free functions. A platform split (iOS, v0.9) can
// wrap them (or provide iOS variants) without touching `EngineApp`'s loop.

/// Say on stderr what a finished recording holds, or why it failed: the
/// runner's one report ([`RunConfig::record`]).
fn report_recording(result: &Result<VideoSummary, VideoError>) {
    match result {
        Ok(movie) => eprintln!(
            "recorded {} frames ({} dropped), {:.1} s at {}x{}, to {}",
            movie.frames,
            movie.dropped,
            movie.seconds,
            movie.width,
            movie.height,
            movie.path.display()
        ),
        Err(e) => eprintln!("recording failed: {e}"),
    }
}

/// Create the application window. Isolated so the platform split can
/// swap window construction per target.
fn create_window(event_loop: &ActiveEventLoop) -> Window {
    let attrs = Window::default_attributes()
        .with_title(WINDOW_TITLE)
        .with_inner_size(winit::dpi::LogicalSize::new(WINDOW_WIDTH, WINDOW_HEIGHT));
    event_loop
        .create_window(attrs)
        .expect("failed to create window")
}

/// Translate a `winit` physical key into the engine's backend-agnostic [`Key`].
///
/// Returns `None` for keys outside the supported subset (see [`Key`]). Isolated so
/// the mapping is a single, testable place and so the platform split can extend
/// it per target.
fn translate_key(code: KeyCode) -> Option<Key> {
    Some(match code {
        KeyCode::KeyW => Key::W,
        KeyCode::KeyA => Key::A,
        KeyCode::KeyS => Key::S,
        KeyCode::KeyD => Key::D,
        KeyCode::KeyQ => Key::Q,
        KeyCode::KeyE => Key::E,
        KeyCode::ArrowLeft => Key::Left,
        KeyCode::ArrowRight => Key::Right,
        KeyCode::ArrowUp => Key::Up,
        KeyCode::ArrowDown => Key::Down,
        KeyCode::Space => Key::Space,
        KeyCode::Escape => Key::Escape,
        _ => return None,
    })
}

/// Convert a `winit` cursor position, which is in physical pixels, to the window
/// logical pixels (points) [`InputState::cursor_position`] reports.
///
/// [`InputState::cursor_position`]: crate::input::InputState::cursor_position
fn cursor_to_logical(position: winit::dpi::PhysicalPosition<f64>, scale_factor: f64) -> (f64, f64) {
    let logical = position.to_logical::<f64>(scale_factor);
    (logical.x, logical.y)
}

/// Translate a `winit` mouse button into the engine's [`MouseButton`].
///
/// Returns `None` for buttons outside the supported subset.
fn translate_mouse_button(button: WinitMouseButton) -> Option<MouseButton> {
    Some(match button {
        WinitMouseButton::Left => MouseButton::Left,
        WinitMouseButton::Right => MouseButton::Right,
        WinitMouseButton::Middle => MouseButton::Middle,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_quits_by_default() {
        assert!(RunConfig::default().quit_on_escape);
    }

    #[test]
    fn cursor_positions_are_reported_in_logical_pixels() {
        let physical = winit::dpi::PhysicalPosition::new(800.0, 600.0);
        // The centre of an 800×600-point window on a 2× display.
        assert_eq!(cursor_to_logical(physical, 2.0), (400.0, 300.0));
        assert_eq!(cursor_to_logical(physical, 1.0), (800.0, 600.0));
    }

    #[test]
    fn key_translation_covers_the_supported_keys() {
        assert_eq!(translate_key(KeyCode::KeyW), Some(Key::W));
        assert_eq!(translate_key(KeyCode::ArrowLeft), Some(Key::Left));
        assert_eq!(translate_key(KeyCode::Escape), Some(Key::Escape));
        // A key outside the subset is ignored, not mistranslated.
        assert_eq!(translate_key(KeyCode::F1), None);
    }

    #[test]
    fn mouse_button_translation() {
        assert_eq!(
            translate_mouse_button(WinitMouseButton::Left),
            Some(MouseButton::Left)
        );
        assert_eq!(translate_mouse_button(WinitMouseButton::Back), None);
    }

    /// The backend factory type must produce a live `Box<dyn Renderer>` and the
    /// game must be able to record a frame against it. This exercises the seam
    /// wiring the windowed path uses, without opening a window (which needs a
    /// display). A `NullRenderer` stands in for a real GPU backend.
    #[test]
    fn backend_factory_yields_a_usable_renderer() {
        use stickshift_math::Transform;
        use stickshift_render_api::{MaterialParams, NullRenderer};

        // A factory shaped exactly like the windowed entry expects. We can't
        // pass a real `&Window` here, so call the closure's body directly with
        // the same output type it must produce.
        let make: fn() -> Box<dyn Renderer> = || Box::new(NullRenderer::new());
        let mut renderer = make();

        let layout = stickshift_render_api::PipelineKind::VertexColor.vertex_layout();
        let vertices = vec![0u8; 3 * layout.stride as usize];
        let mesh = renderer.create_mesh(&stickshift_render_api::MeshData {
            vertices: &vertices,
            indices: &[0, 1, 2],
            layout,
        });
        renderer.begin_frame();
        let pipeline = renderer.create_pipeline(&stickshift_render_api::PipelineDescriptor::new(
            stickshift_render_api::PipelineKind::VertexColor,
        ));
        renderer.set_pipeline(pipeline);
        renderer.draw_mesh(mesh, &Transform::identity(), &MaterialParams::default());
        renderer.submit();
        // If we got here, the trait object satisfied both seam halves.
    }
}
