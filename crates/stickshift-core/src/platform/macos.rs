// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! macOS: the Graphics menu as an `NSMenu` in the menu bar, for the winit
//! runner (feature `windowed`). Settings persistence is Foundation only and
//! lives apart from AppKit, in `platform::defaults`.
//!
//! # How a click gets back into the engine loop
//!
//! 1. Every settings item targets one `StickShiftMenuTarget`, a tiny `NSObject`
//!    subclass declared with `objc2`, and uses one action,
//!    `stickshiftGraphicsItemSelected:`. The item's integer `tag` encodes which
//!    [`MenuCommand`] it is (see [`MenuCommand::tag`]).
//! 2. AppKit calls that action on the main thread, from its own run loop, never
//!    in the middle of an engine frame. The target decodes the tag and posts an
//!    [`EngineEvent`] through winit's `EventLoopProxy`, which wakes the loop.
//! 3. The runner's `user_event` handler turns the command into a graphics-settings
//!    request, the same queue [`EngineCtx::set_graphics_settings`](crate::EngineCtx::set_graphics_settings)
//!    uses.
//!
//! There are no globals and no `static mut`: the proxy lives in the target's
//! instance variables, and the runner owns the target (an `NSMenuItem` holds its
//! target weakly).
//!
//! # Coexisting with winit's menu
//!
//! winit builds the standard application menu (About, Hide, Quit, …) before the
//! first `resumed`. The Graphics menu is **appended** to that menu bar rather
//! than replacing it, so the standard items keep working.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol};
use objc2::{declare_class, msg_send_id, mutability, sel, ClassType, DeclaredClass};
use objc2_app_kit::{
    NSApplication, NSControlStateValueOff, NSControlStateValueOn, NSEventModifierFlags, NSMenu,
    NSMenuItem,
};
use objc2_foundation::{MainThreadMarker, NSString};
use winit::event_loop::EventLoopProxy;

use crate::app::EngineEvent;
use crate::graphics::GraphicsSettings;
use crate::settings_menu::{submenus, MenuCommand, MenuState, ACTIONS, DIAGNOSTICS, MENU_TITLE};

/// Instance variables of [`StickShiftMenuTarget`]: where to post clicks.
pub(crate) struct TargetIvars {
    proxy: EventLoopProxy<EngineEvent>,
}

declare_class!(
    /// The action target shared by every Graphics menu item.
    struct StickShiftMenuTarget;

    unsafe impl ClassType for StickShiftMenuTarget {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "StickShiftMenuTarget";
    }

    impl DeclaredClass for StickShiftMenuTarget {
        type Ivars = TargetIvars;
    }

    unsafe impl StickShiftMenuTarget {
        #[method(stickshiftGraphicsItemSelected:)]
        fn item_selected(&self, sender: &NSMenuItem) {
            // SAFETY: reading an `NSMenuItem`'s tag on the main thread (this
            // class is main-thread-only).
            let tag = unsafe { sender.tag() };
            if let Some(command) = MenuCommand::from_tag(tag) {
                // Fails only if the event loop has already exited, and then
                // there is nothing left to apply the command to.
                let _ = self.ivars().proxy.send_event(EngineEvent::Menu(command));
            }
        }
    }

    unsafe impl NSObjectProtocol for StickShiftMenuTarget {}
);

impl StickShiftMenuTarget {
    fn new(mtm: MainThreadMarker, proxy: EventLoopProxy<EngineEvent>) -> Retained<Self> {
        let this = mtm.alloc::<Self>().set_ivars(TargetIvars { proxy });
        // SAFETY: `NSObject`'s designated initializer, called once on a fresh
        // allocation whose ivars are set.
        unsafe { msg_send_id![super(this), init] }
    }
}

/// The installed Graphics menu: the native items, and the target they call.
pub(crate) struct SettingsMenu {
    /// Kept alive here: menu items reference their target weakly.
    _target: Retained<StickShiftMenuTarget>,
    /// Every item that stands for a [`MenuCommand`], for [`sync`](Self::sync).
    items: Vec<(Retained<NSMenuItem>, MenuCommand)>,
}

impl SettingsMenu {
    /// Build the Graphics menu and append it to the application's menu bar.
    ///
    /// Returns `None` off the main thread (never the case in the runner's
    /// `resumed`, which winit calls on the main thread).
    pub(crate) fn install(proxy: EventLoopProxy<EngineEvent>) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let target = StickShiftMenuTarget::new(mtm, proxy);
        let action = sel!(stickshiftGraphicsItemSelected:);
        let target_obj: &AnyObject = &target;

        let make_item = |command: MenuCommand| -> Retained<NSMenuItem> {
            // The toggles and marking take Control-Command shortcuts.
            let key = command.shortcut().map(String::from).unwrap_or_default();
            let mask = command.shortcut().map(|_| {
                NSEventModifierFlags::NSEventModifierFlagControl
                    | NSEventModifierFlags::NSEventModifierFlagCommand
            });
            unsafe {
                let item = NSMenuItem::initWithTitle_action_keyEquivalent(
                    mtm.alloc(),
                    &NSString::from_str(command.label(MenuState::default())),
                    Some(action),
                    &NSString::from_str(&key),
                );
                if let Some(mask) = mask {
                    item.setKeyEquivalentModifierMask(mask);
                }
                item.setTarget(Some(target_obj));
                item.setTag(command.tag());
                item
            }
        };

        let titled_menu = |title: &str| -> Retained<NSMenu> {
            // SAFETY: main-thread AppKit construction. Items are enabled/disabled
            // explicitly by `sync`, so automatic enabling is turned off.
            unsafe {
                let menu = NSMenu::initWithTitle(mtm.alloc(), &NSString::from_str(title));
                menu.setAutoenablesItems(false);
                menu
            }
        };
        let submenu_item = |title: &str, submenu: &NSMenu| -> Retained<NSMenuItem> {
            // SAFETY: as above; a parent item with no action, only a submenu.
            unsafe {
                let item = NSMenuItem::initWithTitle_action_keyEquivalent(
                    mtm.alloc(),
                    &NSString::from_str(title),
                    None,
                    &NSString::from_str(""),
                );
                item.setSubmenu(Some(submenu));
                item
            }
        };

        let graphics = titled_menu(MENU_TITLE);
        let mut items = Vec::new();
        for sub in submenus() {
            let menu = titled_menu(sub.title);
            for command in sub.items {
                let item = make_item(command);
                menu.addItem(&item);
                items.push((item, command));
            }
            graphics.addItem(&submenu_item(sub.title, &menu));
        }
        graphics.addItem(&NSMenuItem::separatorItem(mtm));
        for command in ACTIONS {
            let item = make_item(command);
            graphics.addItem(&item);
            items.push((item, command));
        }
        graphics.addItem(&NSMenuItem::separatorItem(mtm));
        for command in DIAGNOSTICS {
            let item = make_item(command);
            graphics.addItem(&item);
            items.push((item, command));
        }

        let app = NSApplication::sharedApplication(mtm);
        // SAFETY: main-thread access to the shared application's menu bar.
        let menu_bar = unsafe { app.mainMenu() }.unwrap_or_else(|| {
            // Without winit's default menu there is no bar yet: make one.
            let bar = NSMenu::new(mtm);
            app.setMainMenu(Some(&bar));
            bar
        });
        menu_bar.addItem(&submenu_item(MENU_TITLE, &graphics));

        Some(Self {
            _target: target,
            items,
        })
    }

    /// Refresh checkmarks, enabled states and the two toggles' titles for
    /// `settings` and `state`: whether the window is full screen, and whether a
    /// diagnostics capture is running.
    pub(crate) fn sync(&self, settings: &GraphicsSettings, state: MenuState) {
        for (item, command) in &self.items {
            unsafe {
                item.setState(if command.is_checked(settings) {
                    NSControlStateValueOn
                } else {
                    NSControlStateValueOff
                });
                item.setEnabled(command.is_enabled(settings, state));
                if matches!(
                    command,
                    MenuCommand::ToggleFullScreen | MenuCommand::ToggleDiagnostics
                ) {
                    item.setTitle(&NSString::from_str(command.label(state)));
                }
            }
        }
    }
}
