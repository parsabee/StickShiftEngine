// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! The built-in **Graphics menu**, as platform-neutral data.
//!
//! This is everything about the menu that is not AppKit: its layout, the labels,
//! which item is checked or enabled for given settings, what choosing an item
//! does to the settings, and how an item is encoded in a native menu item's
//! integer tag. All behaviour here is unit-tested without a window.
//!
//! A host renders this model as its native menu and routes each choice back as
//! a [`MenuCommand`]:
//!
//! - The winit runner ([`run`](crate::run), feature `windowed`) builds an
//!   `NSMenu` from it in the macOS menu bar.
//! - A host that owns its own view (an AppKit app with its own menu bar, a
//!   `UIMenu` on iOS) builds its menu from [`submenus`] and [`ACTIONS`], and
//!   hands each choice to its [`Session`](crate::session::Session) as a
//!   [`HostEvent::Menu`](crate::session::HostEvent::Menu). The session applies
//!   it exactly as the winit runner does, and asks the host to refresh the menu
//!   ([`Requests::MENU_SYNC`](crate::session::Requests::MENU_SYNC)) or to toggle
//!   full screen.
//!
//! ```text
//! Graphics
//!   Shadows          ▸ Off · Low · High
//!   Shadow Distance  ▸ Match Draw Distance · Medium · Near
//!   Draw Distance    ▸ Near · Medium · Far
//!   Resolution       ▸ 50% · 75% · 100%
//!   ─────────────
//!   Enter Full Screen            ⌃⌘F
//!   Reset to Defaults
//! ```

use crate::graphics::{
    shadow_quality_label, DrawDistance, GraphicsSettings, RenderScale, ShadowDistance,
    ShadowQuality, SHADOW_QUALITIES,
};

/// Title of the top-level menu.
pub const MENU_TITLE: &str = "Graphics";

/// One choosable item of the Graphics menu.
///
/// A new setting adds items, so a match on it needs a wildcard arm. Its
/// [`tag`](Self::tag) values are stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MenuCommand {
    /// Set the shadow quality.
    Shadows(ShadowQuality),
    /// Set the shadow range.
    ShadowDistance(ShadowDistance),
    /// Set the draw distance.
    DrawDistance(DrawDistance),
    /// Set the render resolution.
    RenderScale(RenderScale),
    /// Enter or leave full screen (a window state, not a graphics setting: it is
    /// neither persisted nor part of [`GraphicsSettings`]).
    ToggleFullScreen,
    /// Restore [`GraphicsSettings::default`].
    ResetToDefaults,
}

/// A submenu of radio-style choices.
pub struct Submenu {
    /// The submenu's title.
    pub title: &'static str,
    /// Its items, in order.
    pub items: Vec<MenuCommand>,
}

/// The four settings submenus, in menu order.
pub fn submenus() -> Vec<Submenu> {
    vec![
        Submenu {
            title: "Shadows",
            items: SHADOW_QUALITIES.map(MenuCommand::Shadows).to_vec(),
        },
        Submenu {
            title: "Shadow Distance",
            items: ShadowDistance::ALL
                .map(MenuCommand::ShadowDistance)
                .to_vec(),
        },
        Submenu {
            title: "Draw Distance",
            items: DrawDistance::ALL.map(MenuCommand::DrawDistance).to_vec(),
        },
        Submenu {
            title: "Resolution",
            items: RenderScale::ALL.map(MenuCommand::RenderScale).to_vec(),
        },
    ]
}

/// The plain items below the submenus (after a separator), in order.
pub const ACTIONS: [MenuCommand; 2] = [MenuCommand::ToggleFullScreen, MenuCommand::ResetToDefaults];

impl MenuCommand {
    /// The item's title. The full-screen toggle's title depends on the window
    /// state, so it takes `fullscreen`.
    pub fn label(self, fullscreen: bool) -> &'static str {
        match self {
            Self::Shadows(q) => shadow_quality_label(q),
            Self::ShadowDistance(d) => d.label(),
            Self::DrawDistance(d) => d.label(),
            Self::RenderScale(r) => r.label(),
            Self::ToggleFullScreen if fullscreen => "Exit Full Screen",
            Self::ToggleFullScreen => "Enter Full Screen",
            Self::ResetToDefaults => "Reset to Defaults",
        }
    }

    /// Whether the item shows a checkmark for `settings`: exactly one item per
    /// submenu, the current choice. Actions are never checked.
    pub fn is_checked(self, settings: &GraphicsSettings) -> bool {
        match self {
            Self::Shadows(q) => settings.shadows == q,
            Self::ShadowDistance(d) => settings.shadow_distance == d,
            Self::DrawDistance(d) => settings.draw_distance == d,
            Self::RenderScale(r) => settings.render_scale == r,
            Self::ToggleFullScreen | Self::ResetToDefaults => false,
        }
    }

    /// Whether the item can be chosen for `settings`. The shadow range means
    /// nothing with shadows off, so those items are disabled then.
    pub fn is_enabled(self, settings: &GraphicsSettings) -> bool {
        match self {
            Self::ShadowDistance(_) => settings.shadows.casts_shadows(),
            Self::ResetToDefaults => *settings != GraphicsSettings::default(),
            _ => true,
        }
    }

    /// The settings after choosing this item from `settings`, or `None` for an
    /// item that is not a settings change (full screen).
    pub fn apply(self, settings: GraphicsSettings) -> Option<GraphicsSettings> {
        let mut s = settings;
        match self {
            Self::Shadows(q) => s.shadows = q,
            Self::ShadowDistance(d) => s.shadow_distance = d,
            Self::DrawDistance(d) => s.draw_distance = d,
            Self::RenderScale(r) => s.render_scale = r,
            Self::ResetToDefaults => s = GraphicsSettings::default(),
            Self::ToggleFullScreen => return None,
        }
        Some(s)
    }

    /// Encode as a native menu item's integer tag: `section * 100 + index`, with
    /// `section` 1–4 for the submenus and 9 for the actions, and `index` from 1.
    /// Zero (a native item's default tag) is never used.
    pub fn tag(self) -> isize {
        fn at<T: PartialEq>(all: &[T], v: T) -> isize {
            all.iter().position(|x| *x == v).unwrap_or(0) as isize + 1
        }
        match self {
            Self::Shadows(q) => 100 + at(&SHADOW_QUALITIES, q),
            Self::ShadowDistance(d) => 200 + at(&ShadowDistance::ALL, d),
            Self::DrawDistance(d) => 300 + at(&DrawDistance::ALL, d),
            Self::RenderScale(r) => 400 + at(&RenderScale::ALL, r),
            Self::ToggleFullScreen => 901,
            Self::ResetToDefaults => 902,
        }
    }

    /// Decode [`tag`](Self::tag); `None` for a tag no item uses.
    pub fn from_tag(tag: isize) -> Option<Self> {
        let index = usize::try_from(tag % 100 - 1).ok()?;
        match tag / 100 {
            1 => SHADOW_QUALITIES.get(index).copied().map(Self::Shadows),
            2 => ShadowDistance::ALL
                .get(index)
                .copied()
                .map(Self::ShadowDistance),
            3 => DrawDistance::ALL
                .get(index)
                .copied()
                .map(Self::DrawDistance),
            4 => RenderScale::ALL.get(index).copied().map(Self::RenderScale),
            9 => ACTIONS.get(index).copied(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every_command() -> Vec<MenuCommand> {
        let mut all: Vec<MenuCommand> = submenus().into_iter().flat_map(|s| s.items).collect();
        all.extend(ACTIONS);
        all
    }

    #[test]
    fn layout_has_the_four_settings_and_two_actions() {
        let titles: Vec<_> = submenus().iter().map(|s| s.title).collect();
        assert_eq!(
            titles,
            ["Shadows", "Shadow Distance", "Draw Distance", "Resolution"]
        );
        assert!(submenus().iter().all(|s| s.items.len() == 3));
        assert_eq!(every_command().len(), 14);
    }

    #[test]
    fn labels_are_what_the_menu_shows() {
        let labels = |i: usize| -> Vec<&str> {
            submenus()[i].items.iter().map(|c| c.label(false)).collect()
        };
        assert_eq!(labels(0), ["Off", "Low", "High"]);
        assert_eq!(labels(1), ["Match Draw Distance", "Medium", "Near"]);
        assert_eq!(labels(2), ["Near", "Medium", "Far"]);
        assert_eq!(labels(3), ["50%", "75%", "100%"]);
        assert_eq!(
            MenuCommand::ToggleFullScreen.label(false),
            "Enter Full Screen"
        );
        assert_eq!(
            MenuCommand::ToggleFullScreen.label(true),
            "Exit Full Screen"
        );
    }

    #[test]
    fn tags_round_trip_and_are_unique_and_nonzero() {
        let mut seen = std::collections::HashSet::new();
        for c in every_command() {
            let tag = c.tag();
            assert_ne!(tag, 0);
            assert!(seen.insert(tag), "duplicate tag {tag} for {c:?}");
            assert_eq!(MenuCommand::from_tag(tag), Some(c));
        }
        for bad in [0, 1, 100, 104, 199, 500, 900, 903, -101] {
            assert_eq!(MenuCommand::from_tag(bad), None, "tag {bad}");
        }
    }

    #[test]
    fn exactly_one_item_per_submenu_is_checked() {
        let s = GraphicsSettings {
            shadows: ShadowQuality::Low,
            shadow_distance: ShadowDistance::Near,
            draw_distance: DrawDistance::Medium,
            render_scale: RenderScale::Full,
        };
        for sub in submenus() {
            let checked: Vec<_> = sub.items.iter().filter(|c| c.is_checked(&s)).collect();
            assert_eq!(checked.len(), 1, "{}", sub.title);
        }
        assert!(MenuCommand::Shadows(ShadowQuality::Low).is_checked(&s));
        assert!(MenuCommand::RenderScale(RenderScale::Full).is_checked(&s));
        assert!(!MenuCommand::ResetToDefaults.is_checked(&s));
    }

    #[test]
    fn choosing_an_item_changes_only_its_setting() {
        let base = GraphicsSettings::default();
        let s = MenuCommand::DrawDistance(DrawDistance::Near)
            .apply(base)
            .unwrap();
        assert_eq!(
            s,
            GraphicsSettings {
                draw_distance: DrawDistance::Near,
                ..base
            }
        );
        let s = MenuCommand::Shadows(ShadowQuality::Off).apply(s).unwrap();
        assert_eq!(s.shadows, ShadowQuality::Off);
        assert_eq!(s.draw_distance, DrawDistance::Near, "earlier choice kept");
        assert_eq!(MenuCommand::ResetToDefaults.apply(s), Some(base));
        assert_eq!(MenuCommand::ToggleFullScreen.apply(s), None);
    }

    #[test]
    fn shadow_distance_is_disabled_while_shadows_are_off() {
        let off = GraphicsSettings {
            shadows: ShadowQuality::Off,
            ..GraphicsSettings::default()
        };
        for d in ShadowDistance::ALL {
            assert!(!MenuCommand::ShadowDistance(d).is_enabled(&off));
            assert!(MenuCommand::ShadowDistance(d).is_enabled(&GraphicsSettings::default()));
        }
        assert!(MenuCommand::Shadows(ShadowQuality::High).is_enabled(&off));
    }

    #[test]
    fn reset_is_enabled_only_when_something_changed() {
        assert!(!MenuCommand::ResetToDefaults.is_enabled(&GraphicsSettings::default()));
        let changed = GraphicsSettings {
            render_scale: RenderScale::Full,
            ..GraphicsSettings::default()
        };
        assert!(MenuCommand::ResetToDefaults.is_enabled(&changed));
    }
}
