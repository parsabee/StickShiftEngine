// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import CoreGraphics
import StickShift

/// The window the game plays in, as the Rust runner opens it for the demo
/// (`stickshift_core::run`, `app.rs:252-255`): what the `offramp` executable
/// passes to `StickShiftApp.run`, here so the tests can check it.
public enum OfframpWindow {
    /// The window's title (`app.rs`'s `WINDOW_TITLE`).
    public static let title = "Stick Shift Engine"
    /// Its size in points (`WINDOW_WIDTH` × `WINDOW_HEIGHT`).
    public static let size = CGSize(width: 800, height: 600)
    /// The engine's options: an app's, as the Rust runner's. Sound through the
    /// default output device, the player's graphics settings restored at
    /// launch and saved on every change under the engine's keys (in the
    /// standard user defaults), Escape quitting, and frames waited for.
    public static let options = Engine.Options.app
}
