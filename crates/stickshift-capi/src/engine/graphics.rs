// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! Graphics settings, their persistence and the Graphics menu through C.
//!
//! A request is queued and applied at the start of the next frame, as
//! `EngineCtx::set_graphics_settings` does: the renderer gets the new settings,
//! the streaming reach is rescaled, the game's settings hook runs, and the
//! engine then resizes the drawable (once a view is reported) and saves the
//! settings, hosted or not. The menu is data:
//! the host builds its native menu from `stickshift_engine_get_menu` and sends
//! a choice back as a `STICKSHIFT_HOST_MENU` event, which the engine applies on
//! the pending-or-current settings. Persistence (`STICKSHIFT_ENGINE_PERSIST_SETTINGS`)
//! is the core session's: restored at create, saved on each change.

use std::ffi::{c_char, CString};
use std::sync::OnceLock;

use stickshift_core::graphics::{
    shadow_quality_label, DrawDistance, RenderScale, ShadowDistance, SHADOW_QUALITIES,
};
use stickshift_core::settings_menu::{self, MenuCommand, ACTIONS, MENU_TITLE};
use stickshift_core::GraphicsSettings;
use stickshift_render_api::ShadowQuality;

use super::{game_call, graphics_settings_to_c, host_call, Access, StickShiftEngine};
use crate::core::{call, input, output, write_sized, Error};
use crate::types::*;

/// `all[value]`, or `INVALID_ARGUMENT` naming `what`.
fn pick<T: Copy>(all: &[T; 3], value: u32, what: &str) -> Result<T, Error> {
    all.get(value as usize)
        .copied()
        .ok_or_else(|| Error::invalid_argument(format!("{value} is not a {what} value")))
}

/// The settings a C struct names, or why it names none.
fn settings_from_c(s: &StickShiftGraphicsSettings) -> Result<GraphicsSettings, Error> {
    if s.reserved != [0; 4] {
        return Err(Error::invalid_argument(
            "the settings' reserved fields must be 0",
        ));
    }
    Ok(GraphicsSettings {
        shadows: match s.shadows {
            STICKSHIFT_SHADOW_QUALITY_OFF => ShadowQuality::Off,
            STICKSHIFT_SHADOW_QUALITY_LOW => ShadowQuality::Low,
            STICKSHIFT_SHADOW_QUALITY_HIGH => ShadowQuality::High,
            other => {
                return Err(Error::invalid_argument(format!(
                    "{other} is not a STICKSHIFT_SHADOW_QUALITY_* value"
                )))
            }
        },
        shadow_distance: pick(
            &ShadowDistance::ALL,
            s.shadow_distance,
            "STICKSHIFT_SHADOW_DISTANCE_*",
        )?,
        draw_distance: pick(
            &DrawDistance::ALL,
            s.draw_distance,
            "STICKSHIFT_DRAW_DISTANCE_*",
        )?,
        render_scale: pick(
            &RenderScale::ALL,
            s.render_scale,
            "STICKSHIFT_RENDER_SCALE_*",
        )?,
    })
}

impl Access<'_> {
    /// The settings in effect, as the game sees them.
    fn graphics_settings(&self) -> GraphicsSettings {
        match self {
            Access::Idle(state) => state.session.lp.graphics.current(),
            Access::Hook(ctx) => ctx.graphics_settings(),
        }
    }

    /// Queue `settings` for the next frame.
    fn request_graphics(&mut self, settings: GraphicsSettings) {
        match self {
            Access::Idle(state) => state.session.lp.graphics.request(settings),
            Access::Hook(ctx) => ctx.set_graphics_settings(settings),
        }
    }
}

/// The graphics settings in effect. A request is not visible here until it is
/// applied. In a hook, or between engine calls.
///
/// # Safety
/// `engine` must be NULL or a live engine; `out` NULL or valid.
#[no_mangle]
pub unsafe extern "C" fn stickshift_graphics_get_current(
    engine: *mut StickShiftEngine,
    out: *mut StickShiftGraphicsSettings,
) -> StickShiftResult {
    game_call(engine, "stickshift_graphics_get_current", |_, access| {
        *output(out, "out")? = graphics_settings_to_c(&access.graphics_settings());
        Ok(STICKSHIFT_OK)
    })
}

/// Ask for new graphics settings, applied at the start of the next frame (or,
/// before init, by init without a settings hook). Requests before then collapse
/// to the last; one equal to the settings in effect changes nothing. In a hook
/// (the render hook too), or between engine calls.
///
/// # Safety
/// `engine` must be NULL or a live engine; `settings` NULL or valid.
#[no_mangle]
pub unsafe extern "C" fn stickshift_graphics_request(
    engine: *mut StickShiftEngine,
    settings: *const StickShiftGraphicsSettings,
) -> StickShiftResult {
    game_call(engine, "stickshift_graphics_request", |_, mut access| {
        let settings = settings_from_c(input(settings, "settings")?)?;
        access.request_graphics(settings);
        Ok(STICKSHIFT_OK)
    })
}

/// The settings in effect and queued, the revision (how many changes were
/// applied; a restore is not one) and the base streaming reach. Between engine
/// calls only: inside any hook or callback it is `STICKSHIFT_ERROR_WRONG_STATE`,
/// since a Rust game sees only the settings in effect. Set
/// `out->size = sizeof(StickShiftGraphicsState)`.
///
/// # Safety
/// `engine` must be NULL or a live engine; `out` NULL or valid for its
/// declared size.
#[no_mangle]
pub unsafe extern "C" fn stickshift_graphics_get_state(
    engine: *mut StickShiftEngine,
    out: *mut StickShiftGraphicsState,
) -> StickShiftResult {
    host_call(engine, "stickshift_graphics_get_state", |_, state| {
        let graphics = &state.session.lp.graphics;
        let pending = graphics.pending();
        let base = graphics.base_spawn_ahead();
        let value = StickShiftGraphicsState {
            size: 0,
            has_pending: u8::from(pending.is_some()),
            has_base_spawn_ahead: u8::from(base.is_some()),
            reserved: [0; 2],
            current: graphics_settings_to_c(&graphics.current()),
            pending: pending
                .map(|p| graphics_settings_to_c(&p))
                .unwrap_or_default(),
            base_spawn_ahead: base.unwrap_or(0.0),
            reserved2: 0,
            revision: graphics.revision(),
        };
        write_sized(out, value, "out")?;
        Ok(STICKSHIFT_OK)
    })
}

/// The default settings: high shadows reaching the draw distance, far draw
/// distance, half resolution. NULL is a no-op. Thread-safe.
///
/// # Safety
/// `out` must be NULL or valid for writes.
#[no_mangle]
pub unsafe extern "C" fn stickshift_graphics_settings_default(
    out: *mut StickShiftGraphicsSettings,
) {
    if let Some(out) = out.as_mut() {
        *out = graphics_settings_to_c(&GraphicsSettings::default());
    }
}

/// The render settings `settings` give the renderer: what an engine pushes to
/// its context when they apply. Thread-safe.
///
/// # Safety
/// `settings` and `out` must be NULL or valid.
#[no_mangle]
pub unsafe extern "C" fn stickshift_graphics_settings_render_settings(
    settings: *const StickShiftGraphicsSettings,
    out: *mut StickShiftRenderSettings,
) -> StickShiftResult {
    call(|| {
        let out = output(out, "out")?;
        let settings = settings_from_c(input(settings, "settings")?)?;
        *out = settings.render_settings().into();
        Ok(STICKSHIFT_OK)
    })
}

/// `settings` as the four integer codes the engine persists them as (shadows,
/// shadow distance, draw distance, render scale), each from 1, into `out[0..4]`.
/// Thread-safe.
///
/// # Safety
/// `settings` must be NULL or valid; `out` NULL or valid for 4 writes.
#[no_mangle]
pub unsafe extern "C" fn stickshift_graphics_settings_to_codes(
    settings: *const StickShiftGraphicsSettings,
    out: *mut i64,
) -> StickShiftResult {
    call(|| {
        if out.is_null() {
            return Err(Error::null("out"));
        }
        let settings = settings_from_c(input(settings, "settings")?)?;
        std::ptr::copy_nonoverlapping(settings.to_codes().as_ptr(), out, 4);
        Ok(STICKSHIFT_OK)
    })
}

/// The settings four persisted codes (`codes[0..4]`) name: an unknown or
/// missing code (0) decodes to that setting's default. NULL is a no-op.
/// Thread-safe.
///
/// # Safety
/// `codes` must be NULL or valid for 4 reads; `out` NULL or valid.
#[no_mangle]
pub unsafe extern "C" fn stickshift_graphics_settings_from_codes(
    codes: *const i64,
    out: *mut StickShiftGraphicsSettings,
) {
    if codes.is_null() {
        return;
    }
    if let Some(out) = out.as_mut() {
        let codes = codes.cast::<[i64; 4]>().read_unaligned();
        *out = graphics_settings_to_c(&GraphicsSettings::from_codes(codes));
    }
}

/// The drawable size and UI scale `render_scale` gives a view of
/// `physical_width` × `physical_height` pixels at `scale_factor` pixels per
/// point: each side scaled and rounded in double precision (at least 1), and
/// the pixels per point scaled too. What a hosted engine resizes its context
/// to. Thread-safe.
///
/// # Safety
/// The outputs must be NULL or valid.
#[no_mangle]
pub unsafe extern "C" fn stickshift_render_scale_surface_for(
    render_scale: StickShiftRenderScale,
    physical_width: u32,
    physical_height: u32,
    scale_factor: f64,
    out_width: *mut u32,
    out_height: *mut u32,
    out_pixels_per_point: *mut f32,
) -> StickShiftResult {
    call(|| {
        let out_width = output(out_width, "out_width")?;
        let out_height = output(out_height, "out_height")?;
        let out_pixels_per_point = output(out_pixels_per_point, "out_pixels_per_point")?;
        let scale = pick(&RenderScale::ALL, render_scale, "STICKSHIFT_RENDER_SCALE_*")?;
        let (width, height, pixels_per_point) =
            scale.surface_for(physical_width, physical_height, scale_factor);
        *out_width = width;
        *out_height = height;
        *out_pixels_per_point = pixels_per_point;
        Ok(STICKSHIFT_OK)
    })
}

/// The labels of each section's values, as NUL-terminated strings that live as
/// long as the process.
fn labels() -> &'static [[CString; 3]; 4] {
    static LABELS: OnceLock<[[CString; 3]; 4]> = OnceLock::new();
    LABELS.get_or_init(|| {
        let c = |s: &str| CString::new(s).unwrap_or_default();
        [
            SHADOW_QUALITIES.map(|q| c(shadow_quality_label(q))),
            ShadowDistance::ALL.map(|d| c(d.label())),
            DrawDistance::ALL.map(|d| c(d.label())),
            RenderScale::ALL.map(|r| c(r.label())),
        ]
    })
}

/// The menu's label for `value` of `section` (a
/// `STICKSHIFT_GRAPHICS_SECTION_*`): "Off", "Match Draw Distance", "Far",
/// "50%"... A static string, or NULL for an unknown section or value.
/// Thread-safe.
#[no_mangle]
pub extern "C" fn stickshift_graphics_label(
    section: StickShiftGraphicsSection,
    value: u32,
) -> *const c_char {
    let index = |n: u32| usize::try_from(n).ok();
    index(section)
        .and_then(|s| s.checked_sub(1))
        .and_then(|s| labels().get(s))
        .and_then(|values| values.get(index(value)?))
        .map_or(std::ptr::null(), |label| label.as_ptr())
}

/// The Graphics menu the host last asked for: strings and arrays the
/// [`StickShiftMenu`] it returned points into.
#[derive(Default)]
pub(crate) struct MenuCache {
    strings: Vec<CString>,
    items: Vec<Vec<StickShiftMenuItem>>,
    groups: Vec<StickShiftMenuGroup>,
}

impl MenuCache {
    /// Keep `s` and return a pointer to it, valid while the cache is.
    fn string(&mut self, s: &str) -> *const c_char {
        self.strings.push(CString::new(s).unwrap_or_default());
        self.strings.last().map_or(std::ptr::null(), |s| s.as_ptr())
    }

    /// Rebuild the menu for `settings`, with the full-screen item titled for
    /// `fullscreen`.
    fn build(&mut self, settings: &GraphicsSettings, fullscreen: bool) {
        self.strings.clear();
        self.items.clear();
        self.groups.clear();
        let sections = settings_menu::submenus()
            .into_iter()
            .map(|s| (Some(s.title), s.items))
            .chain([(None, ACTIONS.to_vec())]);
        for (title, commands) in sections {
            let items: Vec<StickShiftMenuItem> = commands
                .into_iter()
                .map(|command: MenuCommand| StickShiftMenuItem {
                    tag: i32::try_from(command.tag()).unwrap_or(0),
                    label: self.string(command.label(fullscreen)),
                    checked: u8::from(command.is_checked(settings)),
                    enabled: u8::from(command.is_enabled(settings)),
                    reserved: [0; 2],
                })
                .collect();
            let title = title.map_or(std::ptr::null(), |t| self.string(t));
            self.items.push(items);
            let items = self.items.last().expect("just pushed");
            self.groups.push(StickShiftMenuGroup {
                title,
                items: items.as_ptr(),
                item_count: items.len() as u32,
                item_stride: std::mem::size_of::<StickShiftMenuItem>() as u32,
            });
        }
    }
}

/// The Graphics menu for the settings in effect, as data: its title, then the
/// four settings submenus and the actions (full screen, reset) after a
/// separator, each item with its tag, label, checkmark and enabled state.
/// `fullscreen` (0 or 1) titles the full-screen item. The menu is engine-owned
/// and valid until the next call or the engine's destroy. Rebuild or sync the
/// native menu when a call returns `STICKSHIFT_REQUEST_MENU_SYNC`; send a choice
/// back as a `STICKSHIFT_HOST_MENU` event with the item's tag. Between engine
/// calls only. Set `out->size = sizeof(StickShiftMenu)`.
///
/// # Safety
/// `engine` must be NULL or a live engine; `out` NULL or valid for its
/// declared size.
#[no_mangle]
pub unsafe extern "C" fn stickshift_engine_get_menu(
    engine: *mut StickShiftEngine,
    fullscreen: u8,
    out: *mut StickShiftMenu,
) -> StickShiftResult {
    host_call(engine, "stickshift_engine_get_menu", |e, state| {
        if fullscreen > 1 {
            return Err(Error::invalid_argument(format!(
                "fullscreen is {fullscreen}; it must be 0 or 1"
            )));
        }
        // Check the output before replacing the menu a host may still read.
        write_sized(
            out,
            StickShiftMenu {
                size: 0,
                group_count: 0,
                title: std::ptr::null(),
                groups: std::ptr::null(),
                group_stride: 0,
                reserved: 0,
                revision: 0,
            },
            "out",
        )?;
        let graphics = &state.session.lp.graphics;
        let mut menu = e.menu.borrow_mut();
        menu.build(&graphics.current(), fullscreen == 1);
        let title = menu.string(MENU_TITLE);
        let value = StickShiftMenu {
            size: 0,
            group_count: menu.groups.len() as u32,
            title,
            groups: menu.groups.as_ptr(),
            group_stride: std::mem::size_of::<StickShiftMenuGroup>() as u32,
            reserved: 0,
            revision: graphics.revision(),
        };
        write_sized(out, value, "out")?;
        Ok(STICKSHIFT_OK)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_through_their_c_form() {
        for shadows in 0..3 {
            for shadow_distance in 0..3 {
                for draw_distance in 0..3 {
                    for render_scale in 0..3 {
                        let c = StickShiftGraphicsSettings {
                            shadows,
                            shadow_distance,
                            draw_distance,
                            render_scale,
                            reserved: [0; 4],
                        };
                        let rust = settings_from_c(&c).unwrap();
                        assert_eq!(graphics_settings_to_c(&rust), c);
                    }
                }
            }
        }
        let ok = graphics_settings_to_c(&GraphicsSettings::default());
        for bad in [
            StickShiftGraphicsSettings { shadows: 3, ..ok },
            StickShiftGraphicsSettings {
                shadow_distance: 3,
                ..ok
            },
            StickShiftGraphicsSettings {
                draw_distance: u32::MAX,
                ..ok
            },
            StickShiftGraphicsSettings {
                render_scale: 3,
                ..ok
            },
            StickShiftGraphicsSettings {
                reserved: [0, 0, 1, 0],
                ..ok
            },
        ] {
            assert!(settings_from_c(&bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_menu_points_into_its_cache() {
        let mut cache = MenuCache::default();
        cache.build(&GraphicsSettings::default(), true);
        assert_eq!(cache.groups.len(), 5);
        assert!(cache.groups[4].title.is_null());
        // SAFETY: the cache owns every string a group or item points to.
        let label = unsafe { std::ffi::CStr::from_ptr(cache.items[4][0].label) };
        assert_eq!(label.to_str().unwrap(), "Exit Full Screen");
    }
}
