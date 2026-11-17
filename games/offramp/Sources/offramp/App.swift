// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Foundation
import OfframpGame
import StickShift
import StickShiftAppKit

// The windowed game: Rust's `stickshift_core::run` for the demo (`main.rs`). The
// engine's macOS host (`StickShiftApp`) owns everything around the game, as the
// Rust runner does: a regular app activated over whatever launched it, winit's
// application menu and the engine's Graphics menu, the "Stick Shift Engine"
// window of 800 × 600 points (closing it quits), a view that ticks the engine on
// every display refresh and keeps it ticking while the window is hidden, keys
// and the mouse forwarded to the engine's input, Escape quitting, sound through
// the default output device, and the player's graphics settings restored at
// launch and saved on every change, under the engine's keys.

/// The window's title (`app.rs`'s `WINDOW_TITLE`).
let WINDOW_TITLE = "Stick Shift Engine"

/// Play the demo in a window until the player quits. Never returns.
@MainActor
func runWindowed() -> Never {
    StickShiftApp.run(title: WINDOW_TITLE, size: CGSize(width: 800, height: 600), options: .app) { CarRunner() }
}
