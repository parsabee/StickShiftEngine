// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! [`Session`]: the platform-free half of running a game on screen.
//!
//! A game on screen needs more than the shared [`drive_frame`]:
//!
//! - a real frame clock, whose first frame (and first after a pause) banks
//!   exactly one fixed step;
//! - a drawable sized for the view and the render scale in effect;
//! - the player's graphics settings restored at start-up and saved on every
//!   change;
//! - the Graphics menu's choices applied between frames;
//! - Escape, focus loss and resizes.
//!
//! None of that needs a window system, so it lives here. A `Session` owns the
//! [`Loop`] and takes the platform's events as plain [`HostEvent`] values. What
//! only the platform can do (exit, toggle full screen, refresh a native menu)
//! comes back to it as [`Requests`].
//!
//! Every host drives the same `Session`:
//!
//! - **The winit runner** ([`run`](crate::run), feature `windowed`) maps winit's
//!   window events to [`HostEvent`]s and acts on the [`Requests`] with
//!   `event_loop.exit()`, a full-screen toggle and the AppKit menu.
//! - **A host that owns its own view and display link** (an iOS app, say)
//!   builds this crate without the `windowed` feature
//!   and calls the same methods.
//!
//! So the Rust demo's window and every other host share one implementation of
//! the frame clock and the settings side effects.
//!
//! # The life of a session
//!
//! 1. [`Session::new`] takes a [`Loop`] and, with a [`SettingsStore`], queues the
//!    settings the last launch saved, so they apply before [`Game::init`].
//! 2. [`Session::resume`] when the view appears: the first time, it matches the
//!    camera's aspect to the view. Every time, it sizes the drawable, runs the
//!    game's init once ([`Loop::init_once`]), and sizes the drawable again.
//! 3. [`Session::frame`] once per display refresh. It applies a settings change
//!    the game queued, and reacts to it (resize, save) **before** the frame, so
//!    no frame renders new settings at the old size. Then it reads the [`Clock`]
//!    and runs [`drive_frame`].
//! 4. [`Session::event`] for each platform event, between frames.
//!
//! # Example
//!
//! A host's loop, on a clock the host reads:
//!
//! ```rust
//! use stickshift_core::driver::Loop;
//! use stickshift_core::session::{Clock, HostEvent, Requests, Session, View};
//! use stickshift_core::{EngineCtx, Game, Key};
//! use stickshift_render_api::NullRenderer;
//!
//! struct Idle;
//! impl Game for Idle {
//!     fn init(&mut self, _: &mut EngineCtx) {}
//!     fn update(&mut self, _: &mut EngineCtx, _dt: f32) {}
//!     fn render(&mut self, _: &mut EngineCtx) {}
//! }
//!
//! let (mut game, mut renderer) = (Idle, NullRenderer::new());
//! let mut session = Session::new(Loop::new(), None, true);
//!
//! // The view appeared: 1600 × 1200 pixels at 2 pixels per point.
//! let requests = session.resume(View::new(1600, 1200, 2.0), &mut game, &mut renderer);
//! assert!(requests.contains(Requests::MENU_SYNC));
//!
//! // Display refreshes, 50 ms apart on the host's monotonic clock.
//! let (steps, _) = session.frame(Clock::Timestamp(1_000_000_000), &mut game, &mut renderer);
//! assert_eq!(steps, 1, "the first frame banks exactly one step");
//! let (steps, _) = session.frame(Clock::Timestamp(1_050_000_000), &mut game, &mut renderer);
//! assert_eq!(steps, 3);
//!
//! // Escape asks the host to quit, and still reaches the game's input.
//! let escape = HostEvent::KeyDown { key: Key::Escape, repeat: false };
//! let requests = session.event(escape, &mut game, &mut renderer);
//! assert!(requests.contains(Requests::EXIT));
//! assert!(session.lp.input.is_key_down(Key::Escape));
//! ```

use std::ops::{BitOr, BitOrAssign};
use std::time::{Duration, Instant};

use crate::context::Renderer;
use crate::driver::{drive_frame, Loop};
use crate::game::Game;
use crate::graphics::GraphicsSettings;
use crate::input::{Key, MouseButton};
use crate::platform;
use crate::settings_menu::MenuCommand;
use crate::timestep::FIXED_DT_DURATION;

/// The size and pixel density of the view a game is drawn into.
///
/// The drawable is sized from it through
/// [`RenderScale::surface_for`](crate::RenderScale::surface_for), and the
/// camera's aspect ratio is its width over its height.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    /// Width in physical (native) pixels: winit's `inner_size`, or a view's
    /// bounds in points times its backing scale.
    pub width: u32,
    /// Height in physical pixels.
    pub height: u32,
    /// Physical pixels per point: winit's `scale_factor`, AppKit's
    /// `backingScaleFactor`, UIKit's `contentScaleFactor`.
    pub scale_factor: f64,
}

impl View {
    /// A view of `width` × `height` physical pixels at `scale_factor` pixels per
    /// point.
    #[must_use]
    pub const fn new(width: u32, height: u32, scale_factor: f64) -> Self {
        Self {
            width,
            height,
            scale_factor,
        }
    }

    /// The camera aspect ratio for this view: physical width over height, with a
    /// zero height counted as one pixel.
    #[must_use]
    pub fn aspect_ratio(self) -> f32 {
        self.width as f32 / self.height.max(1) as f32
    }
}

/// Where [`Session::frame`] gets the time that passed since the last frame.
///
/// Append-only: a match on it needs a wildcard arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Clock {
    /// Exactly one fixed step ([`FIXED_DT_DURATION`]): the headless driver's
    /// synthetic clock, for tools and tests.
    FixedStep,
    /// This much time passed since the last frame.
    Elapsed(Duration),
    /// The host's monotonic clock reads this many nanoseconds now. On Apple
    /// platforms that is `CLOCK_UPTIME_RAW`, the clock [`Instant`] reads. The
    /// frame banks the time since the last timestamp frame, saturating at zero.
    ///
    /// The first timestamp frame, and the first after a menu choice,
    /// [`Session::reset_clock`] or a frame on another clock, banks exactly one
    /// fixed step instead.
    Timestamp(u64),
    /// As [`Timestamp`](Self::Timestamp), but the session reads
    /// [`Instant::now`] itself, after the pre-frame settings step: exactly where
    /// the winit runner reads it.
    Engine,
}

/// One platform event, as [`Session::event`] takes it.
///
/// Append-only (touch and game-controller input are the expected additions): a
/// match on it needs a wildcard arm.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum HostEvent {
    /// A key went down. `repeat` says the platform marked it an auto-repeat;
    /// either way a press of a key already held makes no new edge
    /// ([`InputState::press_key`](crate::InputState::press_key)). With
    /// quit-on-Escape, Escape returns [`Requests::EXIT`] and is still pressed.
    KeyDown {
        /// The key, by physical position.
        key: Key,
        /// Whether the platform marked this press an auto-repeat.
        repeat: bool,
    },
    /// A key went up.
    KeyUp(Key),
    /// A mouse button went down.
    MouseDown(MouseButton),
    /// A mouse button went up.
    MouseUp(MouseButton),
    /// The cursor moved to `(x, y)`, in logical points from the view's top-left.
    Cursor(f64, f64),
    /// The view lost the keyboard: every key and button is released. (macOS
    /// sends no key-up for a key released while another window has focus.)
    FocusLost,
    /// The view changed size: the camera's aspect follows it, the drawable is
    /// resized, and [`Requests::MENU_SYNC`] asks the host to refresh its menu
    /// (the change may have been entering or leaving full screen).
    Resized(View),
    /// The view moved to a display with a different pixel density: the drawable
    /// is resized, and nothing else changes. The aspect is left alone.
    ScaleChanged(View),
    /// The view's safe-area insets changed: top, right, bottom, left, in points.
    /// Stored, for [`Session::safe_area`].
    SafeArea([f32; 4]),
    /// A Graphics menu item was chosen. A settings change composes on any
    /// request still queued and is applied at once, between frames, and then
    /// as a frame's would be: resized for, saved, [`Requests::MENU_SYNC`]. The
    /// full-screen item returns [`Requests::TOGGLE_FULLSCREEN`]. Either way the
    /// frame clock restarts (see [`Session::reset_clock`]), so the time the menu
    /// was open is not simulated in one burst.
    ///
    /// Before the game's init has run (before the first [`Session::resume`]),
    /// a settings change is not applied: it stays queued, and init makes it
    /// current, as it does settings restored from the store, with no
    /// [`Game::graphics_settings_changed`]. It is saved at once, and returns
    /// no [`Requests::MENU_SYNC`]; the resume that follows does.
    Menu(MenuCommand),
    /// The user asked to close the view: [`Requests::EXIT`].
    Close,
}

/// What a [`Session`] asks its host to do: the things only the platform can.
///
/// A set of flags. Each request is returned by the call that caused
/// it; [`Requests::EXIT`] is also returned by every frame while the game's
/// [`request_exit`](crate::EngineCtx::request_exit) stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[must_use = "a host acts on the requests a session returns"]
pub struct Requests(u32);

impl Requests {
    /// Nothing to do.
    pub const NONE: Self = Self(0);
    /// Stop running the game: the view was closed, Escape was pressed with
    /// quit-on-Escape, or the game asked to exit.
    pub const EXIT: Self = Self(1 << 0);
    /// Enter or leave full screen: the menu's full-screen item was chosen.
    pub const TOGGLE_FULLSCREEN: Self = Self(1 << 1);
    /// The settings in effect, or the full-screen state, may have changed:
    /// refresh the native menu's checkmarks, enabled states and full-screen
    /// title from the [`settings_menu`](crate::settings_menu) model.
    pub const MENU_SYNC: Self = Self(1 << 2);

    /// The flags as bits.
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Whether every flag of `other` is set.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether no flag is set.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl BitOr for Requests {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl BitOrAssign for Requests {
    fn bitor_assign(&mut self, other: Self) {
        self.0 |= other.0;
    }
}

/// Where a [`Session`] keeps the player's graphics settings between launches.
///
/// On Apple platforms the settings are four small integer codes
/// ([`GraphicsSettings::to_codes`]) in `NSUserDefaults`, under the
/// `StickShiftEngine.graphics.*` keys. Every host of the engine uses the same
/// keys, so every host reads and writes the same saved settings. Elsewhere nothing is stored: [`load`](Self::load) finds
/// nothing and [`save`](Self::save) does nothing.
#[derive(Debug, Clone)]
pub struct SettingsStore {
    backing: Backing,
}

#[derive(Debug, Clone)]
enum Backing {
    /// `NSUserDefaults`: the standard user defaults, or a named suite.
    Defaults { suite: Option<String> },
    /// An in-memory stand-in, so no test reads or writes real defaults.
    #[cfg(test)]
    Memory(std::rc::Rc<std::cell::RefCell<tests::Saved>>),
}

impl SettingsStore {
    /// The app's standard user defaults: what the winit runner uses.
    #[must_use]
    pub fn standard() -> Self {
        Self {
            backing: Backing::Defaults { suite: None },
        }
    }

    /// The defaults suite `name` (`NSUserDefaults initWithSuiteName:`), for an app
    /// group that shares settings, or a throwaway domain in a test. A name
    /// Foundation refuses (the app's own bundle identifier, `NSGlobalDomain`)
    /// stores nothing.
    #[must_use]
    pub fn suite(name: impl Into<String>) -> Self {
        Self {
            backing: Backing::Defaults {
                suite: Some(name.into()),
            },
        }
    }

    /// The suite's name, or `None` for the standard user defaults.
    #[must_use]
    pub fn suite_name(&self) -> Option<&str> {
        match &self.backing {
            Backing::Defaults { suite } => suite.as_deref(),
            #[cfg(test)]
            Backing::Memory(_) => None,
        }
    }

    /// The settings a previous launch saved, or `None` if none were saved.
    #[must_use]
    pub fn load(&self) -> Option<GraphicsSettings> {
        match &self.backing {
            Backing::Defaults { suite } => platform::load_settings(suite.as_deref()),
            #[cfg(test)]
            Backing::Memory(saved) => saved.borrow().load(),
        }
    }

    /// Save `settings` for the next launch.
    pub fn save(&self, settings: &GraphicsSettings) {
        match &self.backing {
            Backing::Defaults { suite } => platform::save_settings(suite.as_deref(), settings),
            #[cfg(test)]
            Backing::Memory(saved) => saved.borrow_mut().save(settings),
        }
    }

    /// Forget the saved settings, so the next launch starts from the defaults:
    /// [`load`](Self::load) finds nothing until the next [`save`](Self::save).
    /// For a game's own "reset" and for a test's throwaway suite.
    pub fn clear(&self) {
        match &self.backing {
            Backing::Defaults { suite } => platform::clear_settings(suite.as_deref()),
            #[cfg(test)]
            Backing::Memory(saved) => saved.borrow_mut().clear(),
        }
    }
}

/// The clock reading of the last timestamp frame.
#[derive(Debug, Clone, Copy)]
enum Mark {
    /// A [`Clock::Timestamp`] reading, in nanoseconds.
    Host(u64),
    /// A [`Clock::Engine`] reading.
    Engine(Instant),
}

/// The platform-free half of running a game on screen. See the
/// [module docs](self).
///
/// A session owns the [`Loop`] (in [`lp`](Self::lp)) but not the game or the
/// renderer: every method that runs the game takes both, as [`drive_frame`]
/// does.
pub struct Session {
    /// The engine loop: scene, input, camera, audio, graphics settings.
    pub lp: Loop,
    /// The last timestamp frame's clock reading; `None` before the first, after
    /// a menu choice or [`reset_clock`](Self::reset_clock), and after a frame on
    /// a clock that is not a timestamp.
    mark: Option<Mark>,
    /// The graphics-settings revision this session last reacted to.
    seen_revision: u64,
    /// Where the settings persist, if they do.
    store: Option<SettingsStore>,
    /// Whether Escape asks the host to exit.
    quit_on_escape: bool,
    /// The view, once the host has reported one.
    view: Option<View>,
    /// The view's safe-area insets in points: top, right, bottom, left.
    safe_area: [f32; 4],
    /// Whether [`resume`](Self::resume) has run.
    hosted: bool,
}

impl Session {
    /// A session over `lp`.
    ///
    /// With a `store`, the settings it holds from a previous launch are queued
    /// now with [`GraphicsState::request`](crate::GraphicsState::request), so
    /// [`Loop::init_once`] applies them before [`Game::init`] reads them. That is
    /// a restore, not a change: it bumps no revision, calls no
    /// [`Game::graphics_settings_changed`], and saves nothing back. Every change
    /// applied later is saved to the store.
    ///
    /// With `quit_on_escape`, an Escape [`HostEvent::KeyDown`] returns
    /// [`Requests::EXIT`].
    ///
    /// The session takes `lp` as it is. The winit runner first binds its audio to
    /// the output device ([`Audio::with_output_device`](crate::Audio::with_output_device));
    /// a fresh [`Loop`] is silent.
    #[must_use]
    pub fn new(mut lp: Loop, store: Option<SettingsStore>, quit_on_escape: bool) -> Self {
        if let Some(saved) = store.as_ref().and_then(SettingsStore::load) {
            lp.graphics.request(saved);
        }
        Self {
            lp,
            mark: None,
            seen_revision: 0,
            store,
            quit_on_escape,
            view: None,
            safe_area: [0.0; 4],
            hosted: false,
        }
    }

    /// The view appeared (or reappeared): what the winit runner does in
    /// `resumed`, minus creating the window.
    ///
    /// 1. On the first call only, the camera's aspect ratio becomes the view's
    ///    ([`View::aspect_ratio`]).
    /// 2. The drawable is sized for the view and the render scale in effect, a
    ///    queued one first, so settings restored at start-up size it before
    ///    [`Game::init`] runs.
    /// 3. [`Loop::init_once`] runs the game's init (once in the session's life).
    /// 4. The drawable is sized again, for the settings now in effect.
    ///
    /// Returns [`Requests::MENU_SYNC`]: the host shows the settings on its menu.
    /// Calling it again (an app returning to the foreground) re-sizes the
    /// drawable for the view it is given and changes nothing else.
    pub fn resume<G: Game>(
        &mut self,
        view: View,
        game: &mut G,
        renderer: &mut dyn Renderer,
    ) -> Requests {
        if !self.hosted {
            self.hosted = true;
            self.lp.camera.set_aspect_ratio(view.aspect_ratio());
        }
        self.view = Some(view);
        self.resize_surface(renderer);
        self.lp.init_once(game, renderer);
        self.resize_surface(renderer);
        self.seen_revision = self.lp.graphics.revision();
        Requests::MENU_SYNC
    }

    /// One display frame: what the winit runner does on each redraw.
    ///
    /// 1. A settings change the game queued last frame is applied
    ///    ([`Loop::apply_pending_graphics`]), and the session reacts to it:
    ///    it resizes the drawable for the new render scale, saves the settings,
    ///    and returns [`Requests::MENU_SYNC`]. All of this happens **before** the
    ///    frame, so no frame renders the new settings at the old size.
    /// 2. The elapsed time is read from `clock`.
    /// 3. [`drive_frame`] runs 0..N fixed updates and one render.
    ///
    /// Returns the number of updates run and the requests: also
    /// [`Requests::EXIT`] while the game's
    /// [`request_exit`](crate::EngineCtx::request_exit) stands.
    ///
    /// The game's init must have run first ([`resume`](Self::resume), or
    /// [`Loop::init_once`] on [`lp`](Self::lp)).
    pub fn frame<G: Game>(
        &mut self,
        clock: Clock,
        game: &mut G,
        renderer: &mut dyn Renderer,
    ) -> (u32, Requests) {
        self.lp.apply_pending_graphics(game, renderer);
        let mut requests = self.after_graphics_change(renderer);

        let elapsed = self.elapsed(clock);
        let steps = drive_frame(&mut self.lp, game, renderer, elapsed);
        if self.lp.exit_requested {
            requests |= Requests::EXIT;
        }
        (steps, requests)
    }

    /// Apply one platform event, between frames. See each [`HostEvent`] for what
    /// it does.
    ///
    /// Input goes to [`lp.input`](Loop::input), whose edges clear only after each
    /// fixed update, never per event or per frame: a tap between two frames
    /// reaches the next update, and a frame that runs no update keeps its edges
    /// for the next one.
    ///
    /// A [`HostEvent::Menu`] settings change may run
    /// [`Game::graphics_settings_changed`] inside this call.
    pub fn event<G: Game>(
        &mut self,
        event: HostEvent,
        game: &mut G,
        renderer: &mut dyn Renderer,
    ) -> Requests {
        let input = &mut self.lp.input;
        match event {
            HostEvent::KeyDown { key, repeat: _ } => {
                // Escape quits, matching the close button, unless the game
                // handles it; either way the game sees the key.
                let quit = key == Key::Escape && self.quit_on_escape;
                input.press_key(key);
                if quit {
                    Requests::EXIT
                } else {
                    Requests::NONE
                }
            }
            HostEvent::KeyUp(key) => {
                input.release_key(key);
                Requests::NONE
            }
            HostEvent::MouseDown(button) => {
                input.press_mouse(button);
                Requests::NONE
            }
            HostEvent::MouseUp(button) => {
                input.release_mouse(button);
                Requests::NONE
            }
            HostEvent::Cursor(x, y) => {
                input.set_cursor_position(x, y);
                Requests::NONE
            }
            HostEvent::FocusLost => {
                input.release_all();
                Requests::NONE
            }
            HostEvent::Resized(view) => {
                // The backend below the seam owns no camera, so the aspect is
                // kept here, on the engine's.
                self.lp.camera.set_aspect_ratio(view.aspect_ratio());
                self.view = Some(view);
                self.resize_surface(renderer);
                Requests::MENU_SYNC
            }
            HostEvent::ScaleChanged(view) => {
                self.view = Some(view);
                self.resize_surface(renderer);
                Requests::NONE
            }
            HostEvent::SafeArea(insets) => {
                self.safe_area = insets;
                Requests::NONE
            }
            HostEvent::Menu(command) => self.choose(command, game, renderer),
            HostEvent::Close => Requests::EXIT,
        }
    }

    /// Restart the frame clock: the next [`Clock::Timestamp`] or [`Clock::Engine`]
    /// frame banks exactly one fixed step, not the time since the last frame.
    ///
    /// For a pause the game should not catch up on in one burst: an app
    /// returning to the foreground, a modal sheet. A menu choice does this
    /// itself, because the platform's menu tracking held the thread while the
    /// menu was open.
    pub fn reset_clock(&mut self) {
        self.mark = None;
    }

    /// Read `clock` as [`frame`](Self::frame) does, without running a frame: the
    /// time to bank, with the same first-frame rule and the same mark kept for
    /// the next reading.
    ///
    /// For a host that drives [`drive_frame`] on [`lp`](Self::lp) itself, with
    /// none of the frame's settings side effects (no resize, no save): a tool
    /// or a test. A host with no view yet can still call [`frame`](Self::frame),
    /// which then has nothing to resize but saves an applied change; the C
    /// API's engine does that.
    pub fn read_clock(&mut self, clock: Clock) -> Duration {
        self.elapsed(clock)
    }

    /// The view, once [`resume`](Self::resume) or a resize event reported one.
    #[must_use]
    pub fn view(&self) -> Option<View> {
        self.view
    }

    /// Whether [`resume`](Self::resume) has run.
    #[must_use]
    pub fn is_hosted(&self) -> bool {
        self.hosted
    }

    /// The view's safe-area insets in points (top, right, bottom, left), from the
    /// last [`HostEvent::SafeArea`]; zero until then.
    #[must_use]
    pub fn safe_area(&self) -> [f32; 4] {
        self.safe_area
    }

    /// Whether an Escape key press returns [`Requests::EXIT`].
    #[must_use]
    pub fn quit_on_escape(&self) -> bool {
        self.quit_on_escape
    }

    /// Where the settings persist, if they do.
    #[must_use]
    pub fn settings_store(&self) -> Option<&SettingsStore> {
        self.store.as_ref()
    }

    /// A Graphics menu choice: a settings change is built on a request still
    /// queued (so two quick choices compose) and applied at once; the
    /// full-screen item is the host's to carry out.
    fn choose<G: Game>(
        &mut self,
        command: MenuCommand,
        game: &mut G,
        renderer: &mut dyn Renderer,
    ) -> Requests {
        let graphics = &self.lp.graphics;
        let base = graphics.pending().unwrap_or_else(|| graphics.current());
        let requests = match command.apply(base) {
            Some(settings) if self.lp.initialized => {
                // Applied right away, between frames, through the same path a
                // game's own request takes.
                self.lp.graphics.request(settings);
                self.lp.apply_pending_graphics(game, renderer);
                self.after_graphics_change(renderer)
            }
            Some(settings) => {
                // Before init nothing applies: the choice stays queued, and init
                // makes it current with no revision bump, as it does restored
                // settings. So no revision will ever announce it, but it is the
                // player's change and not a restore: save it now.
                self.lp.graphics.request(settings);
                if settings != base {
                    if let Some(store) = &self.store {
                        store.save(&settings);
                    }
                }
                Requests::NONE
            }
            None => Requests::TOGGLE_FULLSCREEN,
        };
        // The platform's menu tracking held the thread while the menu was open.
        // Restart the frame clock so that time is not simulated in one burst.
        self.reset_clock();
        requests
    }

    /// The time to bank this frame, from `clock`.
    fn elapsed(&mut self, clock: Clock) -> Duration {
        let (mark, elapsed) = match clock {
            Clock::FixedStep => (None, FIXED_DT_DURATION),
            Clock::Elapsed(elapsed) => (None, elapsed),
            Clock::Timestamp(now) => {
                let elapsed = match self.mark {
                    Some(Mark::Host(last)) => Duration::from_nanos(now.saturating_sub(last)),
                    // First frame: bank exactly one fixed step so a game still
                    // ticks once.
                    _ => FIXED_DT_DURATION,
                };
                (Some(Mark::Host(now)), elapsed)
            }
            Clock::Engine => {
                let now = Instant::now();
                let elapsed = match self.mark {
                    Some(Mark::Engine(last)) => now.duration_since(last),
                    _ => FIXED_DT_DURATION,
                };
                (Some(Mark::Engine(now)), elapsed)
            }
        };
        self.mark = mark;
        elapsed
    }

    /// Size the drawable for the view and the render scale in effect. A queued
    /// change counts, so this is right before [`Game::init`] has applied
    /// restored settings. Nothing to size before the host reports a view.
    fn resize_surface(&self, renderer: &mut dyn Renderer) {
        let Some(view) = self.view else {
            return;
        };
        let graphics = &self.lp.graphics;
        let (width, height, pixels_per_point) = graphics
            .pending()
            .unwrap_or_else(|| graphics.current())
            .render_scale
            .surface_for(view.width, view.height, view.scale_factor);
        renderer.resize_surface(width, height, pixels_per_point);
    }

    /// React to an applied settings change, whatever made it (the menu, or the
    /// game through `EngineCtx`): resize the drawable, persist, and ask the host
    /// to refresh its menu.
    fn after_graphics_change(&mut self, renderer: &mut dyn Renderer) -> Requests {
        let revision = self.lp.graphics.revision();
        if revision == self.seen_revision {
            return Requests::NONE;
        }
        self.seen_revision = revision;
        self.resize_surface(renderer);
        if let Some(store) = &self.store {
            store.save(&self.lp.graphics.current());
        }
        Requests::MENU_SYNC
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use stickshift_math::glam::{Mat4, Vec3};
    use stickshift_math::Transform;
    use stickshift_render_api::{
        EmitterHandle, EmitterParams, FrameRecorder, Light, MaterialParams, MeshData, MeshHandle,
        NullRenderer, OverlayQuad, ParticleSystem, PipelineDescriptor, PipelineHandle,
        RenderDevice, RenderSettings, SunSky, TextureData, TextureHandle,
    };

    use super::*;
    use crate::context::EngineCtx;
    use crate::graphics::{DrawDistance, RenderScale, ShadowQuality};
    use crate::timestep::MAX_STEPS_PER_FRAME;

    /// What an in-memory [`SettingsStore`] holds: the four codes as
    /// `NSUserDefaults` would (zero for a key never written), and how many times
    /// they were saved.
    #[derive(Debug, Default)]
    pub(super) struct Saved {
        codes: [i64; 4],
        saves: u32,
    }

    impl Saved {
        /// What `platform::load_settings` reads from the same codes.
        pub(super) fn load(&self) -> Option<GraphicsSettings> {
            (self.codes != [0; 4]).then(|| GraphicsSettings::from_codes(self.codes))
        }

        pub(super) fn save(&mut self, settings: &GraphicsSettings) {
            self.codes = settings.to_codes();
            self.saves += 1;
        }

        pub(super) fn clear(&mut self) {
            self.codes = [0; 4];
        }
    }

    /// An in-memory store holding `codes`, and a handle to watch it.
    fn memory_store(codes: [i64; 4]) -> (SettingsStore, Rc<RefCell<Saved>>) {
        let saved = Rc::new(RefCell::new(Saved { codes, saves: 0 }));
        let store = SettingsStore {
            backing: Backing::Memory(Rc::clone(&saved)),
        };
        (store, saved)
    }

    /// What happened, in order, shared by the game and the renderer.
    type Log = Rc<RefCell<Vec<String>>>;

    /// A [`NullRenderer`] that logs every drawable resize.
    struct Recording {
        inner: NullRenderer,
        log: Log,
        resizes: Vec<(u32, u32, f32)>,
    }

    impl Recording {
        fn new(log: &Log) -> Self {
            Self {
                inner: NullRenderer::new(),
                log: Rc::clone(log),
                resizes: Vec::new(),
            }
        }
    }

    impl RenderDevice for Recording {
        fn create_mesh(&mut self, data: &MeshData<'_>) -> MeshHandle {
            self.inner.create_mesh(data)
        }
        fn destroy_mesh(&mut self, handle: MeshHandle) {
            self.inner.destroy_mesh(handle);
        }
        fn create_texture(&mut self, data: &TextureData<'_>) -> TextureHandle {
            self.inner.create_texture(data)
        }
        fn destroy_texture(&mut self, handle: TextureHandle) {
            self.inner.destroy_texture(handle);
        }
        fn create_pipeline(&mut self, desc: &PipelineDescriptor) -> PipelineHandle {
            self.inner.create_pipeline(desc)
        }
        fn destroy_pipeline(&mut self, handle: PipelineHandle) {
            self.inner.destroy_pipeline(handle);
        }
        fn surface_size(&self) -> (u32, u32) {
            self.inner.surface_size()
        }
        fn safe_area_insets(&self) -> [f32; 4] {
            self.inner.safe_area_insets()
        }
        fn resize_surface(&mut self, width: u32, height: u32, pixels_per_point: f32) {
            self.log
                .borrow_mut()
                .push(format!("resize {width}x{height} @{pixels_per_point}"));
            self.resizes.push((width, height, pixels_per_point));
            self.inner.resize_surface(width, height, pixels_per_point);
        }
        fn surface_scale(&self) -> f32 {
            self.inner.surface_scale()
        }
        fn set_render_settings(&mut self, settings: &RenderSettings) {
            self.inner.set_render_settings(settings);
        }
    }

    impl FrameRecorder for Recording {
        fn begin_frame(&mut self) {
            self.inner.begin_frame();
        }
        fn set_view_projection(&mut self, view_proj: Mat4) {
            self.inner.set_view_projection(view_proj);
        }
        fn set_camera_position(&mut self, position: Vec3) {
            self.inner.set_camera_position(position);
        }
        fn set_sun_sky(&mut self, sun: &SunSky) {
            self.inner.set_sun_sky(sun);
        }
        fn add_light(&mut self, light: &Light) {
            self.inner.add_light(light);
        }
        fn set_pipeline(&mut self, handle: PipelineHandle) {
            self.inner.set_pipeline(handle);
        }
        fn bind_texture(&mut self, handle: TextureHandle) {
            self.inner.bind_texture(handle);
        }
        fn draw_mesh(
            &mut self,
            mesh: MeshHandle,
            transform: &Transform,
            material: &MaterialParams,
        ) {
            self.inner.draw_mesh(mesh, transform, material);
        }
        fn draw_mesh_matrix(&mut self, mesh: MeshHandle, model: Mat4, material: &MaterialParams) {
            self.inner.draw_mesh_matrix(mesh, model, material);
        }
        fn draw_overlay_quad(&mut self, quad: &OverlayQuad) {
            self.inner.draw_overlay_quad(quad);
        }
        fn submit(&mut self) {
            self.inner.submit();
        }
    }

    impl ParticleSystem for Recording {
        fn create_emitter(&mut self, params: &EmitterParams) -> EmitterHandle {
            self.inner.create_emitter(params)
        }
        fn set_emitter(&mut self, emitter: EmitterHandle, params: &EmitterParams) {
            self.inner.set_emitter(emitter, params);
        }
        fn destroy_emitter(&mut self, emitter: EmitterHandle) {
            self.inner.destroy_emitter(emitter);
        }
        fn place_emitter(&mut self, emitter: EmitterHandle, transform: &Transform, velocity: Vec3) {
            self.inner.place_emitter(emitter, transform, velocity);
        }
        fn emit(&mut self, emitter: EmitterHandle, count: u32) {
            self.inner.emit(emitter, count);
        }
        fn advance_particles(&mut self, dt: f32) {
            self.inner.advance_particles(dt);
        }
        fn shift_particles(&mut self, offset: Vec3) {
            self.inner.shift_particles(offset);
        }
    }

    /// A game that streams 180 ahead, as the demo does, and logs its hooks.
    struct Logging {
        log: Log,
        updates: u32,
        renders: u32,
        settings_at_init: Option<GraphicsSettings>,
        changes: Vec<GraphicsSettings>,
        /// Ask for these settings in the next update.
        request: Option<GraphicsSettings>,
        /// Ask to exit in the next update.
        quit: bool,
    }

    impl Logging {
        fn new(log: &Log) -> Self {
            Self {
                log: Rc::clone(log),
                updates: 0,
                renders: 0,
                settings_at_init: None,
                changes: Vec::new(),
                request: None,
                quit: false,
            }
        }
    }

    impl Game for Logging {
        fn init(&mut self, ctx: &mut EngineCtx) {
            self.settings_at_init = Some(ctx.graphics_settings());
            let config = stickshift_scene::StreamingConfig {
                spawn_ahead: 180.0,
                ..ctx.scene().config()
            };
            ctx.scene_mut().set_config(config);
            self.log.borrow_mut().push("init".into());
        }
        fn update(&mut self, ctx: &mut EngineCtx, _dt: f32) {
            self.updates += 1;
            if let Some(settings) = self.request.take() {
                ctx.set_graphics_settings(settings);
            }
            if self.quit {
                ctx.request_exit();
            }
        }
        fn render(&mut self, _ctx: &mut EngineCtx) {
            self.renders += 1;
            self.log.borrow_mut().push("render".into());
        }
        fn graphics_settings_changed(&mut self, _ctx: &mut EngineCtx, settings: &GraphicsSettings) {
            self.changes.push(*settings);
        }
    }

    /// A view of 1600 × 1200 pixels at 2 pixels per point: an 800 × 600 window
    /// on a Retina display, the winit runner's default.
    const RETINA: View = View::new(1600, 1200, 2.0);

    /// Where the test clock starts.
    const START: u64 = 5_000_000_000;

    /// One fixed step, in nanoseconds.
    const STEP: u64 = 16_666_666;

    /// A session, its game, renderer and store, and the shared log.
    struct Rig {
        session: Session,
        game: Logging,
        renderer: Recording,
        saved: Rc<RefCell<Saved>>,
        log: Log,
        /// The test's monotonic clock, in nanoseconds.
        now: u64,
    }

    impl Rig {
        /// A session whose store holds `codes` (all zero: nothing saved).
        fn with_saved(codes: [i64; 4]) -> Self {
            let log = Log::default();
            let (store, saved) = memory_store(codes);
            Self {
                session: Session::new(Loop::new(), Some(store), true),
                game: Logging::new(&log),
                renderer: Recording::new(&log),
                saved,
                log,
                now: START,
            }
        }

        fn new() -> Self {
            Self::with_saved([0; 4])
        }

        /// A rig that has resumed on [`RETINA`].
        fn resumed() -> Self {
            let mut rig = Self::new();
            let _ = rig.resume(RETINA);
            rig
        }

        fn resume(&mut self, view: View) -> Requests {
            self.session
                .resume(view, &mut self.game, &mut self.renderer)
        }

        /// A frame on the test clock.
        fn frame(&mut self) -> (u32, Requests) {
            self.session.frame(
                Clock::Timestamp(self.now),
                &mut self.game,
                &mut self.renderer,
            )
        }

        fn steps(&mut self) -> u32 {
            self.frame().0
        }

        fn event(&mut self, event: HostEvent) -> Requests {
            self.session
                .event(event, &mut self.game, &mut self.renderer)
        }

        fn choose(&mut self, command: MenuCommand) -> Requests {
            self.event(HostEvent::Menu(command))
        }

        fn key_down(&mut self, key: Key) -> Requests {
            self.event(HostEvent::KeyDown { key, repeat: false })
        }

        fn lines(&self) -> Vec<String> {
            self.log.borrow().clone()
        }

        fn codes(&self) -> [i64; 4] {
            self.saved.borrow().codes
        }

        fn saves(&self) -> u32 {
            self.saved.borrow().saves
        }

        fn current(&self) -> GraphicsSettings {
            self.session.lp.graphics.current()
        }

        fn revision(&self) -> u64 {
            self.session.lp.graphics.revision()
        }

        fn spawn_ahead(&self) -> f32 {
            self.session.lp.scene.config().spawn_ahead
        }

        fn aspect(&self) -> f32 {
            self.session.lp.camera.aspect_ratio()
        }

        fn last_resize(&self) -> (u32, u32, f32) {
            *self.renderer.resizes.last().expect("a resize")
        }
    }

    #[test]
    fn the_request_bits_are_stable() {
        assert_eq!(Requests::NONE.bits(), 0);
        assert_eq!(Requests::EXIT.bits(), 1);
        assert_eq!(Requests::TOGGLE_FULLSCREEN.bits(), 2);
        assert_eq!(Requests::MENU_SYNC.bits(), 4);
        let both = Requests::EXIT | Requests::MENU_SYNC;
        assert_eq!(both.bits(), 5);
        assert!(both.contains(Requests::EXIT) && both.contains(Requests::MENU_SYNC));
        assert!(!both.contains(Requests::TOGGLE_FULLSCREEN));
        assert!(both.contains(Requests::NONE));
        assert!(Requests::NONE.is_empty() && !both.is_empty());
        let mut acc = Requests::default();
        acc |= Requests::TOGGLE_FULLSCREEN;
        assert_eq!(acc, Requests::TOGGLE_FULLSCREEN);
    }

    #[test]
    fn resuming_sizes_the_drawable_around_init() {
        let mut rig = Rig::new();
        assert!(!rig.session.is_hosted());
        let requests = rig.resume(RETINA);
        assert_eq!(requests, Requests::MENU_SYNC, "the host shows the settings");
        assert_eq!(
            rig.lines(),
            ["resize 800x600 @1", "init", "resize 800x600 @1"]
        );
        assert!(rig.session.lp.initialized && rig.session.is_hosted());
        assert_eq!(rig.session.view(), Some(RETINA));
        // The camera's aspect is the view's physical pixels; the UI scale is the
        // backing scale at the 50% default.
        assert_eq!(rig.aspect(), 1600.0 / 1200.0);
        assert_eq!(rig.renderer.surface_scale(), 1.0);
        assert_eq!(rig.renderer.surface_size(), (800, 600));
        // Nothing was saved, and the reach is the game's.
        assert_eq!(rig.saves(), 0);
        assert_eq!(rig.codes(), [0; 4]);
        assert_eq!(rig.spawn_ahead(), 180.0);
        assert_eq!(rig.revision(), 0);
    }

    #[test]
    fn resuming_again_changes_only_the_drawable() {
        let mut rig = Rig::resumed();
        rig.log.borrow_mut().clear();
        // Back from the background, at a new size: init does not run again,
        // and the aspect stays what the resizes made it.
        let requests = rig.resume(View::new(3000, 1000, 1.0));
        assert_eq!(requests, Requests::MENU_SYNC);
        assert_eq!(
            rig.lines(),
            ["resize 1500x500 @0.5", "resize 1500x500 @0.5"]
        );
        assert_eq!(rig.aspect(), 1600.0 / 1200.0);
        assert_eq!(rig.session.view(), Some(View::new(3000, 1000, 1.0)));
    }

    #[test]
    fn restored_settings_apply_before_init_with_no_callback() {
        // `graphics.rs`: [1, 0, 1, 0] is shadows off, a missing shadow distance
        // (the default), draw distance near and a missing render scale.
        let mut rig = Rig::with_saved([1, 0, 1, 0]);
        let restored = GraphicsSettings {
            shadows: ShadowQuality::Off,
            draw_distance: DrawDistance::Near,
            ..GraphicsSettings::default()
        };
        assert_eq!(
            rig.session.lp.graphics.pending(),
            Some(restored),
            "queued at new"
        );

        let _ = rig.resume(RETINA);
        assert_eq!(rig.game.settings_at_init, Some(restored), "init sees them");
        assert_eq!(rig.current(), restored);
        assert_eq!(rig.revision(), 0);
        assert!(rig.game.changes.is_empty(), "a restore is not a change");
        // Sized for the queued settings before init, and the reach scaled after
        // it: 180 × 0.5.
        assert_eq!(rig.renderer.resizes, [(800, 600, 1.0), (800, 600, 1.0)]);
        assert_eq!(rig.spawn_ahead(), 90.0);
        assert_eq!(
            rig.renderer.inner.render_settings(),
            Some(restored.render_settings())
        );
        // A restore saves nothing back, and neither do frames.
        assert_eq!(rig.saves(), 0);
        for _ in 0..3 {
            let (_, requests) = rig.frame();
            assert_eq!(requests, Requests::NONE);
            rig.now += STEP;
        }
        assert_eq!(rig.saves(), 0);
        assert_eq!(rig.codes(), [1, 0, 1, 0]);
    }

    #[test]
    fn a_restored_render_scale_sizes_the_first_drawable() {
        // Saved at 100%: the drawable is full size from the first resize, before
        // init has made the setting current.
        let mut rig = Rig::with_saved([0, 0, 0, 3]);
        let _ = rig.resume(RETINA);
        assert_eq!(rig.renderer.resizes, [(1600, 1200, 2.0), (1600, 1200, 2.0)]);
        assert_eq!(rig.current().render_scale, RenderScale::Full);
    }

    #[test]
    fn each_frame_banks_the_time_since_the_last() {
        let mut rig = Rig::resumed();
        // The first frame banks exactly one step, whatever the clock says.
        assert_eq!(rig.steps(), 1);
        rig.now += STEP;
        assert_eq!(rig.steps(), 1);
        rig.now += 50_000_000;
        assert_eq!(rig.steps(), 3, "50 ms is three steps, 2 ns left over");
        assert_eq!(rig.steps(), 0, "no time, no step; the frame still renders");
        rig.now += 20_000_000;
        assert_eq!(rig.steps(), 1, "the remainder carried: 2 + 20 000 000 ns");
        rig.now += 10_000_000_000;
        assert_eq!(
            rig.steps(),
            MAX_STEPS_PER_FRAME,
            "a stall catches up five steps"
        );
        rig.now += STEP;
        assert_eq!(rig.steps(), 1, "and drops the rest");
        assert_eq!(rig.game.updates, 12);
        assert_eq!(rig.game.renders, 7);

        // A clock that goes backwards banks nothing.
        rig.now -= 1_000_000;
        assert_eq!(rig.steps(), 0);
    }

    #[test]
    fn reset_clock_makes_the_next_frame_bank_one_step() {
        let mut rig = Rig::resumed();
        assert_eq!(rig.steps(), 1);
        rig.session.reset_clock();
        rig.now += 10_000_000_000;
        assert_eq!(rig.steps(), 1, "the pause is not simulated");
        rig.now += 3 * STEP;
        assert_eq!(rig.steps(), 3, "and the clock runs on from there");
    }

    #[test]
    fn the_engine_clock_banks_one_step_first() {
        let mut rig = Rig::resumed();
        let (steps, _) = rig
            .session
            .frame(Clock::Engine, &mut rig.game, &mut rig.renderer);
        assert_eq!(
            steps, 1,
            "the first engine-clock frame banks exactly one step"
        );
        // A timestamp after an engine-clock frame is a different clock: one step.
        rig.now += 10_000_000_000;
        assert_eq!(rig.steps(), 1);
    }

    #[test]
    fn the_fixed_and_elapsed_clocks_bank_what_they_say() {
        let mut rig = Rig::resumed();
        let frame =
            |rig: &mut Rig, clock| rig.session.frame(clock, &mut rig.game, &mut rig.renderer).0;
        for _ in 0..100 {
            assert_eq!(frame(&mut rig, Clock::FixedStep), 1);
        }
        assert_eq!(
            frame(&mut rig, Clock::Elapsed(Duration::from_nanos(3 * STEP))),
            3
        );
        assert_eq!(frame(&mut rig, Clock::Elapsed(Duration::ZERO)), 0);
        // Neither is a timestamp, so the next timestamp frame banks one step.
        assert_eq!(rig.steps(), 1);
        rig.now += 2 * STEP;
        assert_eq!(rig.steps(), 2);
        assert_eq!(frame(&mut rig, Clock::FixedStep), 1);
        rig.now += 10_000_000_000;
        assert_eq!(rig.steps(), 1);
    }

    #[test]
    fn an_elapsed_frame_between_timestamps_restarts_the_clock() {
        let mut rig = Rig::resumed();
        assert_eq!(rig.steps(), 1);
        rig.now += STEP;
        assert_eq!(rig.steps(), 1);
        let (steps, _) = rig.session.frame(
            Clock::Elapsed(Duration::from_nanos(STEP)),
            &mut rig.game,
            &mut rig.renderer,
        );
        assert_eq!(steps, 1);
        // The timestamp mark did not survive the elapsed frame, so the time
        // since it is not banked.
        rig.now += 10_000_000_000;
        assert_eq!(rig.steps(), 1);
    }

    #[test]
    fn an_engine_clock_frame_after_a_timestamp_banks_one_step() {
        let mut rig = Rig::resumed();
        assert_eq!(rig.steps(), 1);
        // A timestamp is not an engine-clock reading, whatever its value.
        let (steps, _) = rig
            .session
            .frame(Clock::Engine, &mut rig.game, &mut rig.renderer);
        assert_eq!(steps, 1);
    }

    #[test]
    fn a_change_applied_before_resume_is_not_announced_after_it() {
        // A host that inits the loop and runs bare frames before reporting a
        // view: what that applies, resume takes as seen, as the winit runner
        // did. It is not saved, and the first frame announces nothing.
        let mut rig = Rig::new();
        let lp = &mut rig.session.lp;
        lp.init_once(&mut rig.game, &mut rig.renderer);
        rig.game.request = Some(GraphicsSettings {
            shadows: ShadowQuality::Low,
            ..GraphicsSettings::default()
        });
        let _ = drive_frame(lp, &mut rig.game, &mut rig.renderer, FIXED_DT_DURATION);
        let _ = drive_frame(lp, &mut rig.game, &mut rig.renderer, FIXED_DT_DURATION);
        assert_eq!(rig.revision(), 1);
        assert_eq!(rig.current().shadows, ShadowQuality::Low);

        assert_eq!(rig.resume(RETINA), Requests::MENU_SYNC);
        assert_eq!(rig.frame().1, Requests::NONE);
        assert_eq!(rig.saves(), 0);
    }

    #[test]
    fn a_menu_choice_applies_at_once_saves_and_restarts_the_clock() {
        let mut rig = Rig::resumed();
        let _ = rig.frame();
        rig.now += STEP;
        let _ = rig.frame();
        let resizes = rig.renderer.resizes.len();

        let requests = rig.choose(MenuCommand::DrawDistance(DrawDistance::Near));
        let near = GraphicsSettings {
            draw_distance: DrawDistance::Near,
            ..GraphicsSettings::default()
        };
        assert_eq!(requests, Requests::MENU_SYNC);
        assert_eq!(rig.current(), near);
        assert_eq!(rig.revision(), 1);
        assert_eq!(rig.session.lp.graphics.pending(), None);
        assert_eq!(rig.game.changes, [near]);
        assert_eq!(
            rig.renderer.inner.render_settings(),
            Some(near.render_settings())
        );
        assert_eq!(rig.spawn_ahead(), 90.0);
        assert_eq!(rig.renderer.resizes.len(), resizes + 1);
        assert_eq!(rig.codes(), [3, 1, 1, 1]);
        assert_eq!(rig.saves(), 1);

        // Ten seconds in the menu, then a frame: one step.
        rig.now += 10_000_000_000;
        assert_eq!(rig.frame(), (1, Requests::NONE), "and no second save");
        assert_eq!(rig.saves(), 1);

        // Choosing what is already in effect changes nothing and saves nothing,
        // but still restarts the clock.
        let requests = rig.choose(MenuCommand::DrawDistance(DrawDistance::Near));
        assert_eq!(requests, Requests::NONE);
        assert_eq!(rig.revision(), 1);
        assert_eq!(rig.game.changes.len(), 1);
        assert_eq!(rig.saves(), 1);
        rig.now += 10_000_000_000;
        assert_eq!(rig.steps(), 1);
    }

    #[test]
    fn a_menu_choice_before_init_waits_for_it() {
        // Nothing is applied before init; the choice stays queued and init
        // makes it current, as a restored setting would be. It is the player's
        // change, though, so it is saved at once: no revision will announce it.
        let mut rig = Rig::new();
        let requests = rig.choose(MenuCommand::Shadows(ShadowQuality::Low));
        assert_eq!(requests, Requests::NONE);
        assert_eq!(rig.revision(), 0);
        let low = GraphicsSettings {
            shadows: ShadowQuality::Low,
            ..GraphicsSettings::default()
        };
        assert_eq!(rig.saves(), 1);
        assert_eq!(rig.codes(), low.to_codes());

        // A second choice composes on the queued one and is saved too.
        let _ = rig.choose(MenuCommand::DrawDistance(DrawDistance::Near));
        let both = GraphicsSettings {
            draw_distance: DrawDistance::Near,
            ..low
        };
        assert_eq!(rig.saves(), 2);
        assert_eq!(rig.codes(), both.to_codes());

        // A choice that changes nothing saves nothing.
        let _ = rig.choose(MenuCommand::DrawDistance(DrawDistance::Near));
        assert_eq!(rig.saves(), 2);

        assert_eq!(rig.resume(RETINA), Requests::MENU_SYNC);
        assert_eq!(rig.current(), both);
        assert_eq!(rig.game.settings_at_init, Some(both));
        assert!(rig.game.changes.is_empty());
        assert_eq!(rig.revision(), 0);
        assert_eq!(rig.frame().1, Requests::NONE, "nothing more to announce");
        assert_eq!(rig.saves(), 2);
    }

    #[test]
    fn a_menu_choice_before_init_composes_on_restored_settings() {
        let restored = GraphicsSettings {
            shadows: ShadowQuality::Low,
            ..GraphicsSettings::default()
        };
        let mut rig = Rig::with_saved(restored.to_codes());
        let _ = rig.choose(MenuCommand::DrawDistance(DrawDistance::Near));
        let chosen = GraphicsSettings {
            draw_distance: DrawDistance::Near,
            ..restored
        };
        assert_eq!(rig.saves(), 1);
        assert_eq!(rig.codes(), chosen.to_codes());
        let _ = rig.resume(RETINA);
        assert_eq!(rig.current(), chosen);
        assert_eq!(rig.saves(), 1);
    }

    #[test]
    fn the_resolution_sizes_the_drawable_and_the_ui_scale() {
        let mut rig = Rig::resumed();
        let _ = rig.choose(MenuCommand::RenderScale(RenderScale::Full));
        assert_eq!(rig.last_resize(), (1600, 1200, 2.0));
        assert_eq!(rig.renderer.surface_scale(), 2.0);
        let _ = rig.choose(MenuCommand::RenderScale(RenderScale::ThreeQuarters));
        assert_eq!(rig.last_resize(), (1200, 900, 1.5));
        assert_eq!(rig.renderer.surface_scale(), 1.5);
        assert_eq!(rig.codes(), [3, 1, 3, 2]);
        // The render scale is not a renderer setting.
        assert_eq!(
            rig.renderer.inner.render_settings(),
            Some(GraphicsSettings::default().render_settings())
        );
    }

    #[test]
    fn two_quick_choices_compose() {
        let mut rig = Rig::resumed();
        // A request still queued (from the game) is the base for a menu choice.
        rig.session.lp.graphics.request(GraphicsSettings {
            render_scale: RenderScale::Full,
            ..GraphicsSettings::default()
        });
        let _ = rig.choose(MenuCommand::Shadows(ShadowQuality::Off));
        assert_eq!(
            rig.current(),
            GraphicsSettings {
                shadows: ShadowQuality::Off,
                render_scale: RenderScale::Full,
                ..GraphicsSettings::default()
            }
        );
        assert_eq!(rig.revision(), 1);
        assert_eq!(rig.game.changes.len(), 1, "one change, not two");
    }

    #[test]
    fn reset_restores_the_defaults() {
        let mut rig = Rig::with_saved([1, 3, 1, 3]);
        let _ = rig.resume(RETINA);
        assert_eq!(rig.spawn_ahead(), 90.0);
        let requests = rig.choose(MenuCommand::ResetToDefaults);
        assert_eq!(requests, Requests::MENU_SYNC);
        assert_eq!(rig.current(), GraphicsSettings::default());
        assert_eq!(rig.codes(), [3, 1, 3, 1]);
        assert_eq!(rig.spawn_ahead(), 180.0);
        assert_eq!(rig.last_resize(), (800, 600, 1.0));
    }

    #[test]
    fn full_screen_is_a_request_and_not_a_setting() {
        let mut rig = Rig::resumed();
        let _ = rig.frame();
        let requests = rig.choose(MenuCommand::ToggleFullScreen);
        assert_eq!(requests, Requests::TOGGLE_FULLSCREEN);
        assert_eq!(rig.revision(), 0);
        assert_eq!(rig.saves(), 0, "full screen is never saved");
        rig.now += 10_000_000_000;
        assert_eq!(rig.steps(), 1, "the clock restarted");

        // The menu's title follows on the resize that full screen brings.
        let requests = rig.event(HostEvent::Resized(View::new(3456, 2234, 2.0)));
        assert_eq!(requests, Requests::MENU_SYNC);
        assert_eq!(rig.aspect(), 3456.0 / 2234.0);
        assert_eq!(rig.last_resize(), (1728, 1117, 1.0));
        assert_eq!(rig.renderer.surface_scale(), 1.0);
    }

    #[test]
    fn a_change_the_game_asks_for_is_applied_before_the_next_frame() {
        let mut rig = Rig::resumed();
        let full = GraphicsSettings {
            render_scale: RenderScale::Full,
            ..GraphicsSettings::default()
        };
        rig.game.request = Some(full);
        let (_, requests) = rig.frame(); // the update asks; applied next frame
        assert_eq!(requests, Requests::NONE);
        assert_eq!(rig.session.lp.graphics.pending(), Some(full));
        assert_eq!(rig.saves(), 0);

        rig.log.borrow_mut().clear();
        rig.now += STEP;
        let (steps, requests) = rig.frame();
        assert_eq!((steps, requests), (1, Requests::MENU_SYNC));
        // Resized for it before the frame is drawn, so no frame renders the new
        // settings at the old size.
        assert_eq!(rig.lines(), ["resize 1600x1200 @2", "render"]);
        assert_eq!(rig.codes(), [3, 1, 3, 3]);
        assert_eq!(rig.saves(), 1);
        assert_eq!(rig.game.changes, [full]);
        assert_eq!(rig.renderer.surface_scale(), 2.0);

        // Settings save only on a revision change.
        for _ in 0..5 {
            rig.now += STEP;
            assert_eq!(rig.frame().1, Requests::NONE);
        }
        assert_eq!(rig.saves(), 1);
    }

    #[test]
    fn resizing_uses_the_pending_or_current_scale() {
        // Before init: the restored (queued) scale.
        let mut rig = Rig::with_saved([0, 0, 0, 3]);
        let requests = rig.event(HostEvent::Resized(View::new(1000, 500, 2.0)));
        assert_eq!(requests, Requests::MENU_SYNC);
        assert_eq!(rig.last_resize(), (1000, 500, 2.0), "the queued 100%");
        assert_eq!(rig.aspect(), 2.0);

        // After init, with nothing queued: the current scale.
        let _ = rig.resume(View::new(1000, 500, 2.0));
        let _ = rig.event(HostEvent::Resized(View::new(800, 400, 2.0)));
        assert_eq!(rig.last_resize(), (800, 400, 2.0));

        // After init, with a change queued by the game: the queued scale, so the
        // drawable already matches the frame that applies it.
        rig.game.request = Some(GraphicsSettings {
            render_scale: RenderScale::Half,
            ..GraphicsSettings::default()
        });
        let _ = rig.frame();
        assert_eq!(rig.current().render_scale, RenderScale::Full);
        let _ = rig.event(HostEvent::Resized(View::new(600, 300, 2.0)));
        assert_eq!(rig.last_resize(), (300, 150, 1.0), "the queued 50%");
        rig.now += STEP;
        let (_, requests) = rig.frame();
        assert_eq!(requests, Requests::MENU_SYNC);
        assert_eq!(rig.current().render_scale, RenderScale::Half);
        assert_eq!(rig.last_resize(), (300, 150, 1.0));
    }

    #[test]
    fn resizing_follows_the_views_physical_pixels() {
        let mut rig = Rig::resumed();
        let _ = rig.event(HostEvent::Resized(View::new(3000, 1000, 2.0)));
        assert_eq!(rig.aspect(), 3.0);
        assert_eq!(rig.last_resize(), (1500, 500, 1.0));
        let _ = rig.event(HostEvent::Resized(View::new(640, 0, 2.0)));
        assert_eq!(rig.aspect(), 640.0, "a zero height counts as one pixel");
        assert_eq!(
            rig.last_resize(),
            (320, 1, 1.0),
            "and the drawable is at least 1"
        );
    }

    #[test]
    fn a_scale_change_leaves_the_aspect_alone() {
        let mut rig = Rig::resumed();
        // Moved to a 1× display: the drawable follows the new pixel count, the
        // aspect does not move, and there is no menu to refresh.
        let requests = rig.event(HostEvent::ScaleChanged(View::new(3000, 1000, 1.0)));
        assert_eq!(requests, Requests::NONE);
        assert_eq!(rig.aspect(), 1600.0 / 1200.0);
        assert_eq!(rig.last_resize(), (1500, 500, 0.5));
        assert_eq!(rig.session.view(), Some(View::new(3000, 1000, 1.0)));
        // The next settings change sizes for the view the scale change reported.
        let _ = rig.choose(MenuCommand::RenderScale(RenderScale::Full));
        assert_eq!(rig.last_resize(), (3000, 1000, 1.0));
    }

    #[test]
    fn keys_reach_the_input_and_escape_exits() {
        let mut rig = Rig::new();
        assert_eq!(rig.key_down(Key::Left), Requests::NONE);
        let input = |rig: &Rig| rig.session.lp.input.clone();
        assert!(input(&rig).is_key_down(Key::Left));
        assert!(input(&rig).is_key_just_pressed(Key::Left));
        assert_eq!(rig.event(HostEvent::KeyUp(Key::Left)), Requests::NONE);
        assert!(!input(&rig).is_key_down(Key::Left));
        assert!(input(&rig).is_key_just_released(Key::Left));
        assert_eq!(rig.key_down(Key::Space), Requests::NONE);
        assert!(input(&rig).is_key_down(Key::Space));

        // Escape exits, and reaches the input too, as a repeat does.
        assert_eq!(rig.key_down(Key::Escape), Requests::EXIT);
        assert!(input(&rig).is_key_down(Key::Escape));
        let repeat = HostEvent::KeyDown {
            key: Key::Escape,
            repeat: true,
        };
        assert_eq!(rig.event(repeat), Requests::EXIT);
        assert_eq!(rig.event(HostEvent::KeyUp(Key::Escape)), Requests::NONE);
        assert!(
            !rig.session.lp.exit_requested,
            "the game did not ask to exit"
        );

        // Without quit-on-Escape it is just a key.
        let mut session = Session::new(Loop::new(), None, false);
        assert!(!session.quit_on_escape());
        let escape = HostEvent::KeyDown {
            key: Key::Escape,
            repeat: false,
        };
        assert_eq!(
            session.event(escape, &mut rig.game, &mut rig.renderer),
            Requests::NONE
        );
        assert!(session.lp.input.is_key_down(Key::Escape));
    }

    #[test]
    fn a_key_pressed_again_after_its_release_is_a_new_press() {
        let mut rig = Rig::resumed();
        let _ = rig.key_down(Key::Right);
        assert!(rig.session.lp.input.is_key_just_pressed(Key::Right));
        let _ = rig.frame();
        assert!(
            !rig.session.lp.input.is_key_just_pressed(Key::Right),
            "the update took the press"
        );
        let repeat = HostEvent::KeyDown {
            key: Key::Right,
            repeat: true,
        };
        let _ = rig.event(repeat);
        assert!(
            !rig.session.lp.input.is_key_just_pressed(Key::Right),
            "a key still down: a repeat, not a press"
        );
        let _ = rig.event(HostEvent::KeyUp(Key::Right));
        let _ = rig.key_down(Key::Right);
        assert!(
            rig.session.lp.input.is_key_just_pressed(Key::Right),
            "released, then pressed: a press"
        );
        assert!(rig.session.lp.input.is_key_down(Key::Right));
    }

    #[test]
    fn edges_wait_for_an_update() {
        let mut rig = Rig::resumed();
        let _ = rig.frame();
        // A tap between two frames, then a frame that runs no update: the edges
        // are kept for the next update.
        let _ = rig.key_down(Key::Space);
        let _ = rig.event(HostEvent::KeyUp(Key::Space));
        assert_eq!(rig.steps(), 0);
        assert!(rig.session.lp.input.is_key_just_pressed(Key::Space));
        rig.now += STEP;
        assert_eq!(rig.steps(), 1);
        assert!(!rig.session.lp.input.is_key_just_pressed(Key::Space));
    }

    #[test]
    fn losing_focus_lets_go_of_everything() {
        let mut rig = Rig::new();
        let _ = rig.key_down(Key::Right);
        let _ = rig.key_down(Key::Space);
        let _ = rig.event(HostEvent::MouseDown(MouseButton::Left));
        assert_eq!(rig.event(HostEvent::FocusLost), Requests::NONE);
        let input = &rig.session.lp.input;
        assert!(!input.is_key_down(Key::Right) && !input.is_key_down(Key::Space));
        assert!(input.is_key_just_released(Key::Right) && input.is_key_just_released(Key::Space));
        assert!(
            input.is_key_just_pressed(Key::Right),
            "the press edge is kept for the next update"
        );
        assert!(!input.is_mouse_down(MouseButton::Left));
    }

    #[test]
    fn the_mouse_and_cursor_reach_the_input() {
        let mut rig = Rig::new();
        let _ = rig.event(HostEvent::MouseDown(MouseButton::Right));
        assert!(rig.session.lp.input.is_mouse_down(MouseButton::Right));
        let _ = rig.event(HostEvent::MouseUp(MouseButton::Right));
        assert!(!rig.session.lp.input.is_mouse_down(MouseButton::Right));
        assert_eq!(rig.session.lp.input.cursor_position(), None);
        let _ = rig.event(HostEvent::Cursor(400.0, 300.5));
        assert_eq!(rig.session.lp.input.cursor_position(), Some((400.0, 300.5)));
    }

    #[test]
    fn the_safe_area_is_stored_in_points() {
        let mut rig = Rig::resumed();
        assert_eq!(rig.session.safe_area(), [0.0; 4]);
        let resizes = rig.renderer.resizes.len();
        let insets = [59.0, 0.0, 34.0, 0.0];
        assert_eq!(rig.event(HostEvent::SafeArea(insets)), Requests::NONE);
        assert_eq!(rig.session.safe_area(), insets);
        assert_eq!(rig.renderer.resizes.len(), resizes, "no resize for insets");
    }

    #[test]
    fn closing_or_the_game_asking_exits() {
        let mut rig = Rig::resumed();
        assert_eq!(rig.event(HostEvent::Close), Requests::EXIT);
        assert!(!rig.session.lp.exit_requested);

        assert_eq!(rig.frame().1, Requests::NONE);
        rig.game.quit = true;
        rig.now += STEP;
        assert_eq!(rig.frame().1, Requests::EXIT);
        assert!(rig.session.lp.exit_requested);
        // It stands: every later frame says so too.
        rig.game.quit = false;
        rig.now += STEP;
        assert_eq!(rig.frame().1, Requests::EXIT);
    }

    #[test]
    fn the_standard_store_and_a_suite_name_themselves() {
        assert_eq!(SettingsStore::standard().suite_name(), None);
        assert_eq!(
            SettingsStore::suite("dev.stickshift.tests").suite_name(),
            Some("dev.stickshift.tests")
        );
        let session = Session::new(Loop::new(), None, true);
        assert!(session.settings_store().is_none());
        assert_eq!(session.view(), None);
    }

    /// The same frames on a session and on the bare loop produce the same game
    /// state: the session adds the clock and the platform reactions, nothing
    /// to the cadence.
    #[test]
    fn a_session_runs_the_same_cadence_as_drive_frame() {
        let mut rig = Rig::resumed();
        let mut lp = Loop::new();
        let log = Log::default();
        let mut game = Logging::new(&log);
        let mut renderer = NullRenderer::new();
        lp.camera.set_aspect_ratio(RETINA.aspect_ratio());
        lp.init_once(&mut game, &mut renderer);

        let gaps = [0, STEP, 50_000_000, 0, 20_000_000, 10_000_000_000, STEP];
        let mut elapsed = FIXED_DT_DURATION;
        for (i, gap) in gaps.iter().enumerate() {
            rig.now += gap;
            if i > 0 {
                elapsed = Duration::from_nanos(*gap);
            }
            let session_steps = rig.steps();
            let loop_steps = drive_frame(&mut lp, &mut game, &mut renderer, elapsed);
            assert_eq!(session_steps, loop_steps, "frame {i}");
        }
        assert_eq!(rig.game.updates, game.updates);
        assert_eq!(rig.game.renders, game.renders);
        assert_eq!(
            rig.session.lp.camera.view_projection_matrix(),
            lp.camera.view_projection_matrix()
        );
        assert_eq!(
            rig.renderer.inner.view_projection(),
            renderer.view_projection()
        );
    }
}
