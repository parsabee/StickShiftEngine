// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import AppKit
import StickShift
import StickShiftAppKit
import Testing

@testable import OfframpGame

/// A window on a 2× display, whatever the machine's, so the drawable's size is
/// known.
@MainActor
private final class RetinaWindow: NSWindow {
    override var backingScaleFactor: CGFloat { 2 }
}

/// The windowed game (`App.swift`): the window the Rust runner opens for the
/// demo, and the game running in the engine's view of it, as `StickShiftApp.run`
/// runs it. The view needs a Metal device and no display: the window is never
/// shown.
@MainActor
@Suite("the window")
struct WindowTests {
    @Test func theWindowIsTheRustRunners() {
        // `app.rs:252-255` and the Rust runner's engine.
        #expect(OfframpWindow.title == "Stick Shift Engine")
        #expect(OfframpWindow.size == CGSize(width: 800, height: 600))
        let options = OfframpWindow.options
        #expect(options == .app)
        #expect(options.audio == .device, "sound through the default output device")
        #expect(options.persistsSettings, "the player's graphics settings restored and saved")
        #expect(options.settingsSuite == nil, "in the standard user defaults, under the engine's keys")
        #expect(options.quitsOnEscape, "Escape quits")
        #expect(!options.holdsUntilLoaded, "the models load in initialize, so there is nothing to hold for")
        #expect(options.framePacing == .wait)
    }

    @Test func theGamePlaysInTheEnginesView() throws {
        guard try metalContext(width: 1, height: 1) != nil else { return }
        _ = NSApplication.shared
        let window = RetinaWindow(
            contentRect: NSRect(origin: .zero, size: OfframpWindow.size),
            styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false
        )
        window.isReleasedWhenClosed = false
        // The window's options without the sound device or the player's saved
        // settings, so the test opens no device and writes no defaults.
        var options = OfframpWindow.options
        options.audio = .silent
        options.persistsSettings = false
        #expect(options == Engine.Options(quitsOnEscape: true))
        let game = CarRunner()
        let view = try EngineView(size: OfframpWindow.size, game: game, options: options)
        window.contentView = view
        var exits = 0
        view.onExit = { exits += 1 }
        defer { view.stop() }

        // `StickShiftApp`'s start: the game's initialize, then the frames.
        try view.run()
        #expect(view.isResumed && view.engine.state.isHosted)
        #expect(game.resources != nil && game.font != nil, "initialize loaded the models and the font")
        #expect(game.state == .ready)
        // 800 × 600 points on a 2× display at the default half render scale:
        // one drawable pixel per point, as the Rust runner sizes it.
        #expect(view.metrics == ViewMetrics(width: 1600, height: 1200, scaleFactor: 2))
        #expect(view.context.size == (800, 600))
        #expect(view.context.scale == 1)
        #expect(view.engine.camera.aspectRatio == Float(1600) / Float(1200))

        func ticks(_ n: Int, _ label: String) {
            for _ in 0..<n {
                let tick = view.engine.advance()
                #expect(
                    tick.steps == 1 && tick.rendered && tick.submitted && view.engine.lastRenderError == nil,
                    "\(label): \(tick), render error \(String(describing: view.engine.lastRenderError))"
                )
            }
        }
        ticks(5, "the title screen")
        #expect(game.state == .ready, "frozen until the start key")

        // Keys arrive through the view's keycode table: Space (0x31) starts the run.
        view.keyDown(with: key(.keyDown, 0x31, window))
        ticks(1, "the start frame")
        view.keyUp(with: key(.keyUp, 0x31, window))
        #expect(game.state == .playing, "Space started the run")
        ticks(5, "driving")
        #expect(game.distance > 0)
        #expect(view.context.stats.drawsSkipped == 0 && view.context.stats.framesSkipped == 0)
        #expect(exits == 0)

        // Escape (0x35) quits: the view stops and says so once.
        view.keyDown(with: key(.keyDown, 0x35, window))
        #expect(exits == 1)
        #expect(view.hasExited && !view.isRunning && !view.watchdog.isRunning)
    }

    /// A key event for `window`.
    private func key(_ type: NSEvent.EventType, _ code: UInt16, _ window: NSWindow) -> NSEvent {
        NSEvent.keyEvent(
            with: type, location: .zero, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: window.windowNumber, context: nil, characters: " ", charactersIgnoringModifiers: " ",
            isARepeat: false, keyCode: code
        )!
    }
}
