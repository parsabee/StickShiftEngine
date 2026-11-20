// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! Apple device screens: every iPhone from the iPhone 13 on ([`IPHONES`]) and
//! every iPad mini from the same year, 2021, on ([`IPADS`]); all of them,
//! phones first, from [`DeviceScreen::all`]. To run a game in a window the
//! shape and size of one on a Mac
//! ([`RunConfig::device`](crate::RunConfig::device)).
//!
//! A [`DeviceScreen`] is what a layout needs of a device: its size in **points**
//! (portrait), the scale it renders at, its panel's pixels, and its **safe-area
//! insets** (the notch or Dynamic Island, the rounded corners, the home
//! indicator, an iPad's status bar) in portrait and in landscape. On a Mac the
//! runner opens a window that many points across, so a game sees the device's
//! points and its safe area while its pixels are the Mac's.
//!
//! # Where the numbers come from
//!
//! Point sizes, scales and panels from Apple's technical specifications and
//! its Human Interface Guidelines; safe-area insets as UIKit reports them on
//! each device (as collected by safearea.info and Use Your Loaf). Not yet
//! measured anywhere: the iPhone Duo's landscape insets, which are its portrait
//! insets turned with the phone; and the iPad mini (A20 Pro)'s points and
//! insets, which are its panel at 2x (as every 326 ppi iPad renders) and the
//! earlier minis' status bar and home indicator.

/// Which way up a device is held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Orientation {
    /// Upright: taller than wide.
    #[default]
    Portrait,
    /// On its side: wider than tall.
    Landscape,
}

/// One device's screen, as a layout sees it. See the [module docs](self).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeviceScreen {
    /// A name for a command line: `iphone-15-pro`.
    pub id: &'static str,
    /// Its name: `iPhone 15 Pro`.
    pub name: &'static str,
    /// The year it was announced.
    pub year: u16,
    /// Its size in points, upright: width, height.
    pub points: (u32, u32),
    /// The pixels per point it renders at.
    pub scale: f32,
    /// Its panel in pixels, upright: the points times the scale, or fewer on
    /// a phone that renders at that and scales down to its panel (the 13
    /// mini, the Duo's inner screen).
    pub pixels: (u32, u32),
    /// Its safe-area insets upright, in points: top, right, bottom, left.
    pub portrait_insets: [f32; 4],
    /// Its safe-area insets on its side, in points: top, right, bottom, left.
    pub landscape_insets: [f32; 4],
}

impl DeviceScreen {
    /// Its size in points held `orientation`: width, height.
    #[must_use]
    pub fn size(&self, orientation: Orientation) -> (u32, u32) {
        let (width, height) = self.points;
        match orientation {
            Orientation::Portrait => (width, height),
            Orientation::Landscape => (height, width),
        }
    }

    /// Its safe-area insets in points held `orientation`: top, right,
    /// bottom, left.
    #[must_use]
    pub fn safe_area(&self, orientation: Orientation) -> [f32; 4] {
        match orientation {
            Orientation::Portrait => self.portrait_insets,
            Orientation::Landscape => self.landscape_insets,
        }
    }

    /// The device named `id` ([`DeviceScreen::id`], any case), among
    /// [`all`](Self::all) of them.
    #[must_use]
    pub fn find(id: &str) -> Option<&'static DeviceScreen> {
        Self::all().find(|d| d.id.eq_ignore_ascii_case(id))
    }

    /// Every device: the [`IPHONES`], then the [`IPADS`], each in the order
    /// they came out.
    pub fn all() -> impl Iterator<Item = &'static DeviceScreen> {
        IPHONES.iter().chain(IPADS)
    }
}

/// A device screen held one way: what a window can be the size of
/// ([`RunConfig::device`](crate::RunConfig::device)).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeviceWindow {
    /// The device.
    pub screen: DeviceScreen,
    /// How it is held.
    pub orientation: Orientation,
}

impl DeviceWindow {
    /// Its size in points: width, height.
    #[must_use]
    pub fn points(&self) -> (u32, u32) {
        self.screen.size(self.orientation)
    }

    /// Its safe-area insets in points: top, right, bottom, left.
    #[must_use]
    pub fn safe_area(&self) -> [f32; 4] {
        self.screen.safe_area(self.orientation)
    }

    /// How far to shrink a window its size to fit in `available` points
    /// (width, height): `1` when it fits as it is, never more.
    #[must_use]
    pub fn zoom_to_fit(&self, available: (f64, f64)) -> f64 {
        let (width, height) = self.points();
        (available.0 / f64::from(width))
            .min(available.1 / f64::from(height))
            .clamp(0.1, 1.0)
    }
}

/// A notched or Dynamic Island iPhone's insets: `top` upright (with the home
/// indicator's 34 below), and on its side `side` either side (as UIKit
/// reports, on both) with the home indicator's `bottom` below.
const fn insets(top: f32, side: f32, bottom: f32) -> ([f32; 4], [f32; 4]) {
    ([top, 0.0, 34.0, 0.0], [0.0, side, bottom, side])
}

/// A phone `width` × `height` points at 3x, its panel exactly that, with
/// `insets`.
const fn phone(
    id: &'static str,
    name: &'static str,
    year: u16,
    (width, height): (u32, u32),
    (portrait_insets, landscape_insets): ([f32; 4], [f32; 4]),
) -> DeviceScreen {
    DeviceScreen {
        id,
        name,
        year,
        points: (width, height),
        scale: 3.0,
        pixels: (width * 3, height * 3),
        portrait_insets,
        landscape_insets,
    }
}

/// An iPad `width` × `height` points at 2x, its panel exactly that, with its
/// status bar's 24 above and its home indicator's 20 below, either way up.
const fn tablet(
    id: &'static str,
    name: &'static str,
    year: u16,
    (width, height): (u32, u32),
) -> DeviceScreen {
    DeviceScreen {
        id,
        name,
        year,
        points: (width, height),
        scale: 2.0,
        pixels: (width * 2, height * 2),
        portrait_insets: [24.0, 0.0, 20.0, 0.0],
        landscape_insets: [24.0, 0.0, 20.0, 0.0],
    }
}

/// The notch of the iPhone 13 to 14 Plus and the 16e and 17e.
const NOTCH: ([f32; 4], [f32; 4]) = insets(47.0, 47.0, 21.0);
/// The Dynamic Island of the iPhone 14 Pro to 16 Plus.
const ISLAND: ([f32; 4], [f32; 4]) = insets(59.0, 59.0, 21.0);
/// The Dynamic Island of the iPhone 16 Pro, 17 and 18 Pro.
const ISLAND_62: ([f32; 4], [f32; 4]) = insets(62.0, 62.0, 20.0);

/// Every iPhone from the iPhone 13 on, in the order they came out.
pub const IPHONES: &[DeviceScreen] = &[
    DeviceScreen {
        // Renders 1125 × 2436 and scales down to its panel.
        pixels: (1080, 2340),
        ..phone(
            "iphone-13-mini",
            "iPhone 13 mini",
            2021,
            (375, 812),
            insets(50.0, 50.0, 21.0),
        )
    },
    phone("iphone-13", "iPhone 13", 2021, (390, 844), NOTCH),
    phone("iphone-13-pro", "iPhone 13 Pro", 2021, (390, 844), NOTCH),
    phone(
        "iphone-13-pro-max",
        "iPhone 13 Pro Max",
        2021,
        (428, 926),
        NOTCH,
    ),
    phone("iphone-14", "iPhone 14", 2022, (390, 844), NOTCH),
    phone("iphone-14-plus", "iPhone 14 Plus", 2022, (428, 926), NOTCH),
    phone("iphone-14-pro", "iPhone 14 Pro", 2022, (393, 852), ISLAND),
    phone(
        "iphone-14-pro-max",
        "iPhone 14 Pro Max",
        2022,
        (430, 932),
        ISLAND,
    ),
    phone("iphone-15", "iPhone 15", 2023, (393, 852), ISLAND),
    phone("iphone-15-plus", "iPhone 15 Plus", 2023, (430, 932), ISLAND),
    phone("iphone-15-pro", "iPhone 15 Pro", 2023, (393, 852), ISLAND),
    phone(
        "iphone-15-pro-max",
        "iPhone 15 Pro Max",
        2023,
        (430, 932),
        ISLAND,
    ),
    phone("iphone-16", "iPhone 16", 2024, (393, 852), ISLAND),
    phone("iphone-16-plus", "iPhone 16 Plus", 2024, (430, 932), ISLAND),
    phone(
        "iphone-16-pro",
        "iPhone 16 Pro",
        2024,
        (402, 874),
        ISLAND_62,
    ),
    phone(
        "iphone-16-pro-max",
        "iPhone 16 Pro Max",
        2024,
        (440, 956),
        ISLAND_62,
    ),
    phone("iphone-16e", "iPhone 16e", 2025, (390, 844), NOTCH),
    phone("iphone-17", "iPhone 17", 2025, (402, 874), ISLAND_62),
    phone(
        "iphone-air",
        "iPhone Air",
        2025,
        (420, 912),
        insets(68.0, 68.0, 29.0),
    ),
    phone(
        "iphone-17-pro",
        "iPhone 17 Pro",
        2025,
        (402, 874),
        ISLAND_62,
    ),
    phone(
        "iphone-17-pro-max",
        "iPhone 17 Pro Max",
        2025,
        (440, 956),
        ISLAND_62,
    ),
    phone("iphone-17e", "iPhone 17e", 2026, (390, 844), NOTCH),
    phone(
        "iphone-18-pro",
        "iPhone 18 Pro",
        2026,
        (402, 874),
        ISLAND_62,
    ),
    phone(
        "iphone-18-pro-max",
        "iPhone 18 Pro Max",
        2026,
        (440, 956),
        ISLAND_62,
    ),
    // The Duo closed: its outer screen. Its camera sits at the right edge
    // upright; on its side, those insets turned with it (not yet measured).
    phone(
        "iphone-duo",
        "iPhone Duo (closed)",
        2026,
        (466, 678),
        ([0.0, 84.0, 34.0, 0.0], [84.0, 34.0, 0.0, 0.0]),
    ),
    // The Duo open: its inner screen, which renders 2007 × 2853 and scales
    // down to its panel. On its side, its insets turned with it (not yet
    // measured).
    DeviceScreen {
        pixels: (1878, 2670),
        ..phone(
            "iphone-duo-open",
            "iPhone Duo (open)",
            2026,
            (669, 951),
            ([82.0, 0.0, 34.0, 0.0], [0.0, 34.0, 0.0, 82.0]),
        )
    },
];

/// Every iPad mini from 2021 on, in the order they came out.
pub const IPADS: &[DeviceScreen] = &[
    tablet(
        "ipad-mini-6",
        "iPad mini (6th generation)",
        2021,
        (744, 1133),
    ),
    tablet(
        "ipad-mini-a17-pro",
        "iPad mini (A17 Pro)",
        2024,
        (744, 1133),
    ),
    // Its 8.5-inch OLED panel, 1512 × 2316 at 326 ppi; its points that at 2x
    // and its insets the earlier minis' (neither yet measured). Its front
    // camera is in the bezel on its landscape edge, not cut into the screen.
    tablet(
        "ipad-mini-a20-pro",
        "iPad mini (A20 Pro)",
        2026,
        (756, 1158),
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_iphone_from_the_13_on_is_there_once_by_a_name_of_its_own() {
        let ids: Vec<&str> = IPHONES.iter().map(|d| d.id).collect();
        for id in [
            "iphone-13-mini",
            "iphone-13",
            "iphone-13-pro",
            "iphone-13-pro-max",
            "iphone-14",
            "iphone-14-plus",
            "iphone-14-pro",
            "iphone-14-pro-max",
            "iphone-15",
            "iphone-15-plus",
            "iphone-15-pro",
            "iphone-15-pro-max",
            "iphone-16",
            "iphone-16-plus",
            "iphone-16-pro",
            "iphone-16-pro-max",
            "iphone-16e",
            "iphone-17",
            "iphone-air",
            "iphone-17-pro",
            "iphone-17-pro-max",
            "iphone-17e",
            "iphone-18-pro",
            "iphone-18-pro-max",
            "iphone-duo",
            "iphone-duo-open",
        ] {
            assert_eq!(ids.iter().filter(|i| **i == id).count(), 1, "{id}");
        }
        assert_eq!(ids.len(), 26);
        // In the order they came out.
        assert!(IPHONES.windows(2).all(|w| w[0].year <= w[1].year));
    }

    #[test]
    fn every_ipad_mini_from_2021_on_follows_the_iphones() {
        let ids: Vec<&str> = IPADS.iter().map(|d| d.id).collect();
        assert_eq!(
            ids,
            ["ipad-mini-6", "ipad-mini-a17-pro", "ipad-mini-a20-pro"]
        );
        assert!(IPADS.windows(2).all(|w| w[0].year <= w[1].year));
        let all: Vec<&str> = DeviceScreen::all().map(|d| d.id).collect();
        assert_eq!(all.len(), IPHONES.len() + IPADS.len());
        assert_eq!(
            (all[0], all[all.len() - 1]),
            ("iphone-13-mini", "ipad-mini-a20-pro")
        );
        // A name of its own each, across them all.
        for id in &all {
            assert_eq!(all.iter().filter(|i| *i == id).count(), 1, "{id}");
        }
        let mini = DeviceScreen::find("ipad-mini-a17-pro").expect("an iPad");
        assert_eq!(
            (mini.points, mini.scale, mini.pixels),
            ((744, 1133), 2.0, (1488, 2266))
        );
        assert_eq!(
            mini.safe_area(Orientation::Landscape),
            [24.0, 0.0, 20.0, 0.0]
        );
        let new = DeviceScreen::find("ipad-mini-a20-pro").expect("the newest");
        assert_eq!(new.pixels, (1512, 2316));
    }

    #[test]
    fn each_screen_is_upright_its_panel_no_bigger_than_its_points_at_its_scale() {
        for d in DeviceScreen::all() {
            let (w, h) = d.points;
            assert!(h > w, "{} upright", d.id);
            let rendered = (w as f32 * d.scale, h as f32 * d.scale);
            assert!(d.pixels.0 as f32 <= rendered.0 && d.pixels.1 as f32 <= rendered.1);
            // The same shape, panel and points, to within a pixel or two.
            let shape = |(a, b): (u32, u32)| a as f32 / b as f32;
            assert!((shape(d.pixels) - shape(d.points)).abs() < 0.01, "{}", d.id);
            // Insets inside the screen, a home indicator's worth below upright.
            for (insets, (sw, sh)) in [
                (d.portrait_insets, d.size(Orientation::Portrait)),
                (d.landscape_insets, d.size(Orientation::Landscape)),
            ] {
                assert!(insets.iter().all(|i| *i >= 0.0));
                assert!(insets[0] + insets[2] < sh as f32 / 2.0);
                assert!(insets[1] + insets[3] < sw as f32 / 2.0);
            }
        }
        // A phone has a home indicator's 34 below upright.
        for d in IPHONES {
            assert_eq!(d.portrait_insets[2], 34.0, "{}", d.id);
        }
    }

    #[test]
    fn a_screen_held_on_its_side_turns_its_size_and_insets() {
        let pro = DeviceScreen::find("iPhone-15-PRO").expect("by any case");
        assert_eq!(pro.name, "iPhone 15 Pro");
        assert_eq!(pro.size(Orientation::Portrait), (393, 852));
        assert_eq!(pro.size(Orientation::Landscape), (852, 393));
        assert_eq!(pro.safe_area(Orientation::Portrait), [59.0, 0.0, 34.0, 0.0]);
        assert_eq!(
            pro.safe_area(Orientation::Landscape),
            [0.0, 59.0, 21.0, 59.0]
        );
        assert_eq!(pro.pixels, (1179, 2556));
        assert!(DeviceScreen::find("iphone-12").is_none(), "from the 13 on");
        let window = DeviceWindow {
            screen: *pro,
            orientation: Orientation::Landscape,
        };
        assert_eq!(window.points(), (852, 393));
        assert_eq!(window.safe_area(), pro.landscape_insets);
        // Fits as it is on a big screen; shrunk, its shape kept, on a small one.
        assert_eq!(window.zoom_to_fit((1728.0, 1000.0)), 1.0);
        assert!((window.zoom_to_fit((600.0, 1000.0)) - 600.0 / 852.0).abs() < 1e-9);
    }
}
