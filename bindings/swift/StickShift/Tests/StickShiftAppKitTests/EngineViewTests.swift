// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import AppKit
import CStickShift
import Testing

@_spi(CInterop) import StickShift
@testable import StickShiftAppKit

// The engine's view in a real, offscreen window (Offramp's `GameViewTests`,
// moved here with the host): AppKit's resizes and display changes reach the
// engine as the Rust runner reports winit's, keys and the mouse reach the
// game, and the view does what the engine asks. Needs a Metal device for the
// view's layer, and no display.

/// A window whose display density the test sets, standing in for a move between
/// a Retina and a 1x display, and that counts its full-screen toggles.
@MainActor
private final class DensityWindow: NSWindow {
    var density: CGFloat = 2
    var fullScreenToggles = 0
    /// Stands in for the window being full screen.
    var isFull = false

    override var backingScaleFactor: CGFloat { density }

    override var styleMask: NSWindow.StyleMask {
        get { isFull ? super.styleMask.union(.fullScreen) : super.styleMask }
        set { super.styleMask = newValue }
    }

    override func toggleFullScreen(_ sender: Any?) {
        fullScreenToggles += 1
    }
}

/// An `EngineView` running `Idle` in a `DensityWindow`, resumed, or `nil`
/// without a Metal device.
@MainActor
private struct Rig {
    let window: DensityWindow
    let view: EngineView
    let game: Idle
    /// Every event the view pushed since the rig was made.
    let events: Events

    final class Events {
        var all: [HostEvent] = []
    }

    static func make(size: NSSize = NSSize(width: 800, height: 600)) throws -> Rig? {
        guard try metalContext(width: 1, height: 1) != nil else { return nil }
        _ = NSApplication.shared
        let window = DensityWindow(
            contentRect: NSRect(origin: .zero, size: size), styleMask: [.titled, .resizable], backing: .buffered,
            defer: false
        )
        window.isReleasedWhenClosed = false
        let game = Idle()
        let view = try EngineView(size: size, game: game, options: testOptions)
        window.contentView = view
        let events = Events()
        view.onPush = { events.all.append($0) }
        try view.resume()
        return Rig(window: window, view: view, game: game, events: events)
    }

    /// The view's physical size in pixels at the window's density.
    var physical: (width: Int, height: Int) {
        let d = Double(window.density)
        return (Int((Double(view.bounds.width) * d).rounded()), Int((Double(view.bounds.height) * d).rounded()))
    }

    /// The drawable and the UI scale are what the engine's render scale gives
    /// for the window's native pixels and density (Rust's `resize_surface`).
    func expectDrawable(_ label: String, sourceLocation: SourceLocation = #_sourceLocation) {
        var width: UInt32 = 0
        var height: UInt32 = 0
        var scale: Float = 0
        let result = stickshift_render_scale_surface_for(
            view.engine.renderScaleCode, UInt32(physical.width), UInt32(physical.height), Double(window.density),
            &width, &height, &scale
        )
        #expect(result == STICKSHIFT_OK)
        #expect(
            view.context.size == (Int(width), Int(height)),
            "\(label): drawable \(view.context.size), expected \(width) × \(height)", sourceLocation: sourceLocation
        )
        #expect(view.context.scale == scale, "\(label): UI scale", sourceLocation: sourceLocation)
    }

    /// The camera's aspect is the view's, from its physical pixels.
    var viewAspect: Float {
        Float(physical.width) / Float(max(physical.height, 1))
    }

    /// Set the engine camera's aspect, to see whether an event changes it.
    func setAspect(_ aspect: Float) {
        #expect(stickshift_camera_set_aspect_ratio(view.engine.cEngine, aspect) == STICKSHIFT_OK)
    }

    /// A key event for the window.
    func key(_ type: NSEvent.EventType, _ code: UInt16, _ flags: NSEvent.ModifierFlags = [], repeating: Bool = false)
        -> NSEvent
    {
        NSEvent.keyEvent(
            with: type, location: .zero, modifierFlags: flags, timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: window.windowNumber, context: nil, characters: " ", charactersIgnoringModifiers: " ",
            isARepeat: repeating, keyCode: code
        )!
    }

    /// A mouse event at `point`, in the window's coordinates.
    func mouse(_ type: NSEvent.EventType, at point: NSPoint = .zero, button: Int = 0) -> NSEvent {
        NSEvent.mouseEvent(
            with: type, location: point, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: window.windowNumber, context: nil, eventNumber: 0, clickCount: 1, pressure: 1
        )!
    }
}

@MainActor
@Suite("EngineView")
struct EngineViewTests {
    @Test func resumeReportsTheViewAndRunsTheGameOnce() throws {
        guard let rig = try Rig.make() else { return }
        #expect(rig.view.isResumed)
        #expect(rig.game.initialized == 1)
        #expect(rig.view.engine.state.isHosted)
        // A Retina display at the 50% default: one drawable pixel per point.
        #expect(rig.view.metrics == ViewMetrics(width: 1600, height: 1200, scaleFactor: 2))
        #expect(rig.view.context.size == (800, 600))
        rig.expectDrawable("resumed, 2x at 50%")
        #expect(rig.view.engine.cameraAspect == rig.viewAspect)
        #expect(rig.events.all.isEmpty, "resume reports the view itself")

        // Again (the view back on screen): no second initialize.
        try rig.view.resume()
        #expect(rig.game.initialized == 1)
    }

    @Test func aResizeIsViewResizedAndADensityChangeIsScaleChanged() throws {
        guard let rig = try Rig.make() else { return }
        rig.setAspect(1)
        rig.window.setContentSize(NSSize(width: 1000, height: 700))
        #expect(rig.events.all == [.viewResized(ViewMetrics(width: 2000, height: 1400, scaleFactor: 2))])
        #expect(rig.view.context.size == (1000, 700))
        rig.expectDrawable("resized")
        #expect(rig.view.engine.cameraAspect == rig.viewAspect, "the aspect follows a resize")

        // Dragged to a 1x display: half a drawable pixel per point, and the
        // aspect left alone.
        rig.events.all.removeAll()
        rig.setAspect(1)
        rig.window.density = 1
        rig.view.viewDidChangeBackingProperties()
        #expect(rig.events.all == [.scaleChanged(ViewMetrics(width: 1000, height: 700, scaleFactor: 1))])
        #expect(rig.view.context.size == (500, 350))
        #expect(rig.view.context.scale == 0.5)
        rig.expectDrawable("moved to 1x")
        #expect(rig.view.engine.cameraAspect == 1, "a scale change leaves the aspect alone")

        // Nothing changed: nothing reported.
        rig.events.all.removeAll()
        rig.view.viewDidChangeBackingProperties()
        rig.view.setFrameSize(rig.view.frame.size)
        #expect(rig.events.all.isEmpty)

        // The Resolution menu at 100% sizes the drawable to the native pixels.
        let bar = rig.view.makeMenuBar()
        let resolution = try #require(bar.menu.items.first { $0.title == "Resolution" }?.submenu)
        resolution.performActionForItem(at: 2)
        #expect(rig.events.all == [.menu(GraphicsMenu.Tag(rawValue: 403))])
        #expect(rig.view.context.size == (1000, 700))
        rig.expectDrawable("100% on 1x")
        #expect(resolution.items[2].state == .on, "the menu synced")

        // Back on the Retina display, then an odd size.
        rig.window.density = 2
        rig.view.viewDidChangeBackingProperties()
        #expect(rig.view.context.size == (2000, 1400))
        #expect(rig.view.context.scale == 2)
        rig.window.setContentSize(NSSize(width: 801, height: 601))
        #expect(rig.view.context.size == (1602, 1202))
        rig.expectDrawable("an odd size")
    }

    @Test func keysGoThroughTheKeycodeTable() throws {
        guard let rig = try Rig.make() else { return }
        let engine = rig.view.engine
        #expect(rig.view.acceptsFirstResponder)
        #expect(rig.window.firstResponder === rig.view, "the view takes the keyboard in its window")

        rig.view.keyDown(with: rig.key(.keyDown, 0x31))
        rig.view.keyDown(with: rig.key(.keyDown, 0x31, repeating: true))
        rig.view.keyDown(with: rig.key(.keyDown, 0x24))  // Return: not an engine key
        #expect(rig.events.all == [.keyDown(.space), .keyDown(.space, isRepeat: true)])
        #expect(engine.isKeyDown(.space))
        rig.view.keyUp(with: rig.key(.keyUp, 0x31))
        #expect(!engine.isKeyDown(.space))
        #expect(rig.events.all.last == .keyUp(.space))
    }

    @Test func aKeyReleasedWhileCommandIsHeldReachesTheGame() throws {
        guard let rig = try Rig.make() else { return }
        rig.window.orderFront(nil)
        defer { rig.window.orderOut(nil) }
        // ⌘→ goes down (the view's key-down), and comes up through
        // `NSApplication.sendEvent(_:)`, which alone would drop it.
        rig.view.keyDown(with: rig.key(.keyDown, 0x7C, .command))
        #expect(rig.view.engine.isKeyDown(.right))
        rig.view.driveFrame()
        NSApplication.shared.sendEvent(rig.key(.keyUp, 0x7C, .command))
        #expect(!rig.view.engine.isKeyDown(.right), "the key-up reached the game")
    }

    @Test func theMouseAndTheCursorInPointsFromTheTopLeft() throws {
        guard let rig = try Rig.make() else { return }
        let engine = rig.view.engine
        rig.view.mouseDown(with: rig.mouse(.leftMouseDown))
        rig.view.rightMouseDown(with: rig.mouse(.rightMouseDown))
        #expect(engine.isMouseDown(.left) && engine.isMouseDown(.right))
        rig.view.mouseUp(with: rig.mouse(.leftMouseUp))
        rig.view.rightMouseUp(with: rig.mouse(.rightMouseUp))
        #expect(!engine.isMouseDown(.left) && !engine.isMouseDown(.right))

        // The view is the window's content: window coordinates are the view's,
        // from the bottom-left.
        rig.view.mouseMoved(with: rig.mouse(.mouseMoved, at: NSPoint(x: 100, y: 450)))
        #expect(rig.events.all.last == .cursor([100, 150]))
        let input = engine.inputState
        #expect(input.has_cursor == 1)
        #expect(input.cursor_x == 100 && input.cursor_y == 150)
        rig.view.mouseDragged(with: rig.mouse(.leftMouseDragged, at: NSPoint(x: 0, y: 600)))
        #expect(rig.events.all.last == .cursor([0, 0]))
    }

    @Test func losingTheKeyWindowReleasesEverything() throws {
        guard let rig = try Rig.make() else { return }
        rig.view.keyDown(with: rig.key(.keyDown, 0x0D))
        rig.view.mouseDown(with: rig.mouse(.leftMouseDown))
        #expect(rig.view.engine.isKeyDown(.w))
        NotificationCenter.default.post(name: NSWindow.didResignKeyNotification, object: rig.window)
        #expect(rig.events.all.last == .focusLost)
        #expect(!rig.view.engine.isKeyDown(.w))
        #expect(!rig.view.engine.isMouseDown(.left))

        // Another window's notifications are not the view's.
        rig.events.all.removeAll()
        let other = NSWindow(contentRect: .zero, styleMask: [], backing: .buffered, defer: true)
        other.isReleasedWhenClosed = false
        NotificationCenter.default.post(name: NSWindow.didResignKeyNotification, object: other)
        #expect(rig.events.all.isEmpty)

        // Nor, once the view has left it, its old window's.
        rig.window.contentView = nil
        NotificationCenter.default.post(name: NSWindow.didResignKeyNotification, object: rig.window)
        #expect(rig.events.all.isEmpty)
    }

    @Test func framesTickTheEngineOnlyOnceResumedAndUntilExit() throws {
        guard try metalContext(width: 1, height: 1) != nil else { return }
        let game = Idle()
        let view = try EngineView(size: NSSize(width: 64, height: 48), game: game, options: testOptions)
        view.driveFrame()
        #expect(view.engine.state.frames == 0, "no tick before resume")
        try view.resume()
        view.driveFrame()
        view.driveFrame()
        #expect(view.engine.state.frames == 2)
        #expect(game.renders == 2)
        #expect(game.updates >= 1, "the first engine-clock tick banks one step")

        // The game asks to exit: the view stops, and says so once.
        var exits = 0
        view.onExit = { exits += 1 }
        game.exitsOnUpdate = true
        view.resetEngineClock()
        view.driveFrame()
        #expect(view.hasExited)
        #expect(exits == 1)
        let frames = view.engine.state.frames
        view.driveFrame()
        view.start()
        #expect(!view.watchdog.isRunning)
        #expect(view.engine.state.frames == frames, "no tick after exit")
        #expect(exits == 1)
    }

    @Test func escapeQuitsAndSoDoesClosing() throws {
        guard let rig = try Rig.make() else { return }
        var exits = 0
        rig.view.onExit = { exits += 1 }
        rig.view.start()
        rig.view.keyDown(with: rig.key(.keyDown, 0x35))
        #expect(exits == 1)
        #expect(rig.view.hasExited)
        #expect(!rig.view.isRunning && !rig.view.watchdog.isRunning, "ticking stopped")
        #expect(rig.view.engine.isKeyDown(.escape), "the game still sees the key")

        guard let other = try Rig.make() else { return }
        other.view.onExit = { exits += 1 }
        #expect(other.view.push(.close).contains(.exit))
        #expect(exits == 2)
    }

    @Test func theWatchdogTicksWhileTheDisplayLinkIsSilent() throws {
        guard let rig = try Rig.make() else { return }
        var now: UInt64 = 5_000_000_000
        rig.view.watchdog.clock = { now }
        // Out of a window the display link stops; the watchdog does not.
        rig.window.orderOut(nil)
        rig.view.start()
        defer { rig.view.stop() }
        #expect(rig.view.watchdog.isRunning)
        let frames = rig.view.engine.state.frames
        now += 100_000_000
        rig.view.watchdog.timerFired()
        #expect(rig.view.engine.state.frames == frames, "100 ms is not a stall")
        now += 150_000_000
        rig.view.watchdog.timerFired()
        rig.view.watchdog.timerFired()
        #expect(rig.view.engine.state.frames == frames + 2, "each tick past 250 ms drives a frame")
        rig.view.stop()
        rig.view.watchdog.timerFired()
        #expect(rig.view.engine.state.frames == frames + 2)
    }

    @Test func theFullScreenRequestTogglesTheWindowAndTheMenuFollows() throws {
        guard let rig = try Rig.make() else { return }
        let bar = rig.view.makeMenuBar()
        #expect(rig.view.menuBar === bar)
        let fullScreen = try #require(bar.menu.items.first { $0.tag == 901 })
        #expect(fullScreen.title == "Enter Full Screen")
        bar.menu.performActionForItem(at: bar.menu.index(of: fullScreen))
        #expect(rig.window.fullScreenToggles == 1)

        // The window went full screen: the item is retitled.
        rig.window.isFull = true
        NotificationCenter.default.post(name: NSWindow.didEnterFullScreenNotification, object: rig.window)
        #expect(fullScreen.title == "Exit Full Screen")
        rig.window.isFull = false
        NotificationCenter.default.post(name: NSWindow.didExitFullScreenNotification, object: rig.window)
        #expect(fullScreen.title == "Enter Full Screen")
    }
}

extension EngineView {
    /// Make the next engine-clock tick bank exactly one fixed step.
    func resetEngineClock() {
        engine.resetClock()
    }
}
