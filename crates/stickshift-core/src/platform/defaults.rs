// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! Graphics-settings persistence in `NSUserDefaults`, on every Apple target.
//!
//! This is Foundation only: no AppKit and no window, so iOS has it as well as
//! macOS, and a host built without the `windowed` feature keeps it. The
//! [`SettingsStore`](crate::session::SettingsStore) a
//! [`Session`](crate::session::Session) holds is the only caller.
//!
//! The settings are stored as the four small integer codes of
//! [`GraphicsSettings::to_codes`], under the [`DEFAULTS_KEYS`]. Every host of the
//! engine uses the same keys, so the Rust runner and a game over the C API share
//! the player's saved settings.
//!
//! A suite name opens `initWithSuiteName:` instead of the standard user
//! defaults: an app group that shares settings between apps, or a throwaway
//! domain for a test.

use objc2::rc::Retained;
use objc2::ClassType;
use objc2_foundation::{NSString, NSUserDefaults};

use crate::graphics::GraphicsSettings;

/// `NSUserDefaults` keys for the four settings, in
/// [`GraphicsSettings::to_codes`] order.
const DEFAULTS_KEYS: [&str; 4] = [
    "StickShiftEngine.graphics.shadows",
    "StickShiftEngine.graphics.shadowDistance",
    "StickShiftEngine.graphics.drawDistance",
    "StickShiftEngine.graphics.renderScale",
];

/// The standard user defaults, or the defaults of the suite `suite`. `None` if
/// the suite cannot be opened (Foundation refuses the app's own bundle
/// identifier and `NSGlobalDomain` as suite names).
fn open(suite: Option<&str>) -> Option<Retained<NSUserDefaults>> {
    match suite {
        // SAFETY: `NSUserDefaults` is thread-safe; the shared instance always
        // exists.
        None => Some(unsafe { NSUserDefaults::standardUserDefaults() }),
        // SAFETY: the designated suite initializer, called once on a fresh
        // allocation; it returns nil (`None`) for a refused name.
        Some(name) => unsafe {
            NSUserDefaults::initWithSuiteName(
                NSUserDefaults::alloc(),
                Some(&NSString::from_str(name)),
            )
        },
    }
}

/// Read the settings a previous launch saved, or `None` if none were saved
/// (all four codes read back as 0) or the suite cannot be opened.
///
/// Stored values are small integer codes (see [`GraphicsSettings::to_codes`]);
/// an unknown or missing code decodes to that setting's default.
pub(crate) fn load_settings(suite: Option<&str>) -> Option<GraphicsSettings> {
    let defaults = open(suite)?;
    // SAFETY: `NSUserDefaults` is thread-safe; reading integers by key.
    let codes =
        DEFAULTS_KEYS.map(|key| unsafe { defaults.integerForKey(&NSString::from_str(key)) } as i64);
    (codes != [0; 4]).then(|| GraphicsSettings::from_codes(codes))
}

/// Save `settings` for the next launch. Does nothing if the suite cannot be
/// opened.
pub(crate) fn save_settings(suite: Option<&str>, settings: &GraphicsSettings) {
    let Some(defaults) = open(suite) else {
        return;
    };
    for (key, code) in DEFAULTS_KEYS.iter().zip(settings.to_codes()) {
        // SAFETY: `NSUserDefaults` is thread-safe; writing integers by key.
        unsafe { defaults.setInteger_forKey(code as isize, &NSString::from_str(key)) };
    }
}

/// Remove the saved settings' keys, so [`load_settings`] finds none. Does
/// nothing if the suite cannot be opened.
pub(crate) fn clear_settings(suite: Option<&str>) {
    let Some(defaults) = open(suite) else {
        return;
    };
    for key in DEFAULTS_KEYS {
        // SAFETY: `NSUserDefaults` is thread-safe; removing a value by key.
        unsafe { defaults.removeObjectForKey(&NSString::from_str(key)) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphics::{DrawDistance, RenderScale, ShadowDistance, ShadowQuality};

    /// A defaults domain of its own, emptied when made and when dropped, so no
    /// test reads or writes the user's real settings. Each test uses its own
    /// fixed name, so repeated runs reuse the same few domains rather than
    /// leaving a new (empty) preferences file behind each time.
    struct Scratch {
        name: String,
    }

    impl Scratch {
        fn new(label: &str) -> Self {
            let scratch = Self {
                name: format!("dev.stickshift.core-tests.{label}"),
            };
            scratch.clear();
            scratch
        }

        fn clear(&self) {
            if let Some(defaults) = open(Some(&self.name)) {
                // SAFETY: removing the scratch domain this test owns.
                unsafe { defaults.removePersistentDomainForName(&NSString::from_str(&self.name)) };
            }
        }

        /// The raw integer stored under each key, as a launch would read it.
        fn codes(&self) -> [i64; 4] {
            let defaults = open(Some(&self.name)).expect("a scratch suite opens");
            // SAFETY: as in `load_settings`.
            DEFAULTS_KEYS
                .map(|key| unsafe { defaults.integerForKey(&NSString::from_str(key)) } as i64)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            self.clear();
        }
    }

    #[test]
    fn a_suite_round_trips_the_settings_as_four_codes() {
        let scratch = Scratch::new("round-trip");
        assert_eq!(
            load_settings(Some(&scratch.name)),
            None,
            "a fresh suite holds nothing"
        );

        let settings = GraphicsSettings {
            shadows: ShadowQuality::Off,
            shadow_distance: ShadowDistance::Near,
            draw_distance: DrawDistance::Medium,
            render_scale: RenderScale::Full,
        };
        save_settings(Some(&scratch.name), &settings);
        assert_eq!(scratch.codes(), settings.to_codes());
        assert_eq!(scratch.codes(), [1, 3, 2, 3]);
        assert_eq!(load_settings(Some(&scratch.name)), Some(settings));

        // The defaults are saved as codes too, so they read back as saved.
        save_settings(Some(&scratch.name), &GraphicsSettings::default());
        assert_eq!(scratch.codes(), [3, 1, 3, 1]);
        assert_eq!(
            load_settings(Some(&scratch.name)),
            Some(GraphicsSettings::default())
        );
    }

    #[test]
    fn suites_are_separate_domains() {
        let a = Scratch::new("suite-a");
        let b = Scratch::new("suite-b");
        let settings = GraphicsSettings {
            render_scale: RenderScale::ThreeQuarters,
            ..GraphicsSettings::default()
        };
        save_settings(Some(&a.name), &settings);
        assert_eq!(load_settings(Some(&a.name)), Some(settings));
        assert_eq!(load_settings(Some(&b.name)), None);
    }

    #[test]
    fn clearing_forgets_the_saved_settings() {
        let scratch = Scratch::new("clear");
        save_settings(Some(&scratch.name), &GraphicsSettings::default());
        assert!(load_settings(Some(&scratch.name)).is_some());
        clear_settings(Some(&scratch.name));
        assert_eq!(scratch.codes(), [0; 4]);
        assert_eq!(load_settings(Some(&scratch.name)), None);
    }
}
