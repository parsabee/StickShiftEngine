// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! Native platform services: graphics-settings persistence, and the windowed
//! runner's built-in Graphics menu.
//!
//! The two have different reach, so they are split:
//!
//! - **Persistence** (`defaults`) is Foundation only (`NSUserDefaults`). Every
//!   Apple target has it, with or without the `windowed` feature, so a
//!   [`Session`](crate::session::Session) driven by a host's own view keeps the
//!   player's settings exactly as the Rust runner does. Other targets store
//!   nothing.
//! - **The menu** (`macos`) is AppKit (`NSMenu` in the menu bar, via `objc2`), and
//!   only the winit runner ([`app`](crate::app), feature `windowed`) installs it.
//!   A host with its own menu bar builds its native menu from the same
//!   platform-neutral [`settings_menu`](crate::settings_menu) model instead.
//!
//! The headless driver uses neither, and no test touches the menu bar or the
//! user's stored preferences: the persistence tests use a throwaway defaults
//! suite of their own.

#[cfg(target_vendor = "apple")]
mod defaults;

#[cfg(target_vendor = "apple")]
pub(crate) use defaults::{clear_settings, load_settings, save_settings};

#[cfg(not(target_vendor = "apple"))]
pub(crate) use no_defaults::{clear_settings, load_settings, save_settings};

#[cfg(all(feature = "windowed", target_os = "macos"))]
mod macos;

#[cfg(all(feature = "windowed", target_os = "macos"))]
pub(crate) use macos::SettingsMenu;

#[cfg(all(feature = "windowed", not(target_os = "macos")))]
pub(crate) use no_menu::SettingsMenu;

/// Nothing is stored on targets without `NSUserDefaults`.
#[cfg(not(target_vendor = "apple"))]
mod no_defaults {
    use crate::graphics::GraphicsSettings;

    /// Nothing stored on this target.
    pub(crate) fn load_settings(_suite: Option<&str>) -> Option<GraphicsSettings> {
        None
    }

    /// Nothing stored on this target.
    pub(crate) fn save_settings(_suite: Option<&str>, _settings: &GraphicsSettings) {}

    /// Nothing stored on this target.
    pub(crate) fn clear_settings(_suite: Option<&str>) {}
}

/// No native menu on targets other than macOS.
#[cfg(all(feature = "windowed", not(target_os = "macos")))]
mod no_menu {
    use winit::event_loop::EventLoopProxy;

    use crate::app::EngineEvent;
    use crate::graphics::GraphicsSettings;

    /// No native menu on this target.
    pub(crate) struct SettingsMenu;

    impl SettingsMenu {
        /// Nothing to install.
        pub(crate) fn install(_proxy: EventLoopProxy<EngineEvent>) -> Option<Self> {
            None
        }

        /// Nothing to refresh.
        pub(crate) fn sync(&self, _settings: &GraphicsSettings, _fullscreen: bool) {}
    }
}
