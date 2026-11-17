// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import AppKit
import QuartzCore
import StickShift

/// A view that runs an ``Engine``: the macOS host of a game, what the Rust
/// runner's winit window does for a Rust game.
///
/// ```swift
/// let view = try EngineView(size: NSSize(width: 800, height: 600), game: MyGame())
/// window.contentView = view
/// view.onExit = { NSApp.terminate(nil) }
/// try view.run()                  // the game's initialize, then the frames
/// view.makeMenuBar().install(in: NSApp.mainMenu!)
/// ```
///
/// ``StickShiftApp/run(title:size:options:makeGame:)`` does all of this, and
/// the window and the application, for a game that needs nothing else.
///
/// The view creates the engine on its own context and reports to it what the
/// platform reports to the view:
///
/// - **Its size**, in physical pixels (each side `round(points × backing
///   scale)`, winit's `inner_size`), with its backing scale: a change of size
///   is ``HostEvent/viewResized(_:)`` (the camera's aspect follows), a move to a
///   display of another density ``HostEvent/scaleChanged(_:)`` (the aspect is
///   left alone). The engine sizes the drawable for its Resolution setting, so
///   the view's ``StickShiftView/renderScale`` is not used.
/// - **Keys**, through the macOS keycode table (``Key/init(macKeyCode:)``),
///   repeats flagged; a key released while Command is held included. The view
///   takes the keyboard when it joins a window. Keys the engine does not know
///   are dropped, and no key event goes on up the responder chain.
/// - **The mouse**: the left, right and middle buttons, and the cursor in
///   points from the view's top-left.
/// - **Focus loss**: when its window stops being the key window, everything
///   held is released.
///
/// Its display link (in the run loop's common modes, so it keeps running while
/// a menu is open) ticks the engine on the engine's monotonic clock
/// (``Engine/Clock/engine``), where the Rust runner reads it, and then polls the
/// context's events. While the display link is stopped (the window minimised,
/// the app hidden), a ``FrameWatchdog`` ticks it instead, as winit's loop never
/// pauses. The view then does what the engine asks: it toggles full screen,
/// re-syncs its ``menuBar``, and on exit stops and calls ``onExit``.
///
/// The view sets its own ``StickShiftView/onFrame`` and ``StickShiftView/onKey``;
/// don't replace them.
@MainActor
public final class EngineView: StickShiftView {
    /// The engine, on this view's ``StickShiftView/context``.
    public let engine: Engine
    /// Ticks the engine while the display link is stopped.
    public let watchdog = FrameWatchdog()
    /// The Graphics menu kept in step with the engine's settings and the
    /// window's full-screen state, if any. Setting it syncs it.
    public var menuBar: GraphicsMenuBar? {
        didSet { syncMenu() }
    }
    /// Called once, when the engine asks to exit (Escape with
    /// ``Engine/Options/quitsOnEscape``, the window closed, the game's
    /// ``EngineContext/requestExit()``). The view has stopped ticking by then;
    /// an app terminates here.
    public var onExit: (() -> Void)?
    /// Whether ``resume()`` has run.
    public private(set) var isResumed = false
    /// Whether the engine asked to exit: the view ticks no more.
    public private(set) var hasExited = false

    /// The metrics last reported to the engine.
    private(set) var reportedMetrics: ViewMetrics?
    /// Sees every event the view pushes, in order (for the tests).
    var onPush: ((HostEvent) -> Void)?
    /// The window whose notifications the view observes.
    private weak var observedWindow: NSWindow?

    /// Creates the view, a Metal context for its layer at the view's full
    /// native resolution, and an engine on that context that runs `game` with
    /// `options`. No hook runs yet: call ``run()`` once the view is in its window.
    ///
    /// - Throws: ``StickShiftError/Code-swift.struct/noDevice`` without a Metal
    ///   device, and what ``Engine/init(context:game:options:)`` throws.
    public init(size: NSSize, game: some Game, options: Engine.Options = .app) throws {
        let (layer, context) = try Self.makeLayer(size: size, renderScale: 1)
        engine = try Engine(context: context, game: game, options: options)
        super.init(size: size, layer: layer, context: context, renderScale: 1)
        onKey = { [weak self] keyCode, isDown, isRepeat in
            self?.key(keyCode, isDown: isDown, isRepeat: isRepeat)
        }
        onFrame = { [weak self] _, _ in
            self?.displayLinkFrame()
        }
        watchdog.drive = { [weak self] in
            self?.watchdogFrame()
        }
        addTrackingArea(
            NSTrackingArea(rect: .zero, options: [.mouseMoved, .activeInKeyWindow, .inVisibleRect], owner: self)
        )
    }

    @available(*, unavailable)
    public required init?(coder: NSCoder) {
        fatalError("EngineView is created in code: init(size:game:options:)")
    }

    // MARK: - Running

    /// Report the view to the engine: on the first call the camera's aspect
    /// becomes the view's, the drawable is sized for the engine's settings,
    /// and the game's ``Game/initialize(_:)`` runs. Calling it again (the view
    /// back on screen) only reports the view's size. Run ``Engine/load()`` first
    /// for a game with a ``Game/load(_:)`` phase.
    ///
    /// - Throws: what ``Engine/resume(_:)`` throws, the game's own error included.
    @discardableResult
    public func resume() throws -> HostRequests {
        let current = metrics
        let requests = try engine.resume(current)
        reportedMetrics = current
        isResumed = true
        carryOut(requests)
        return requests
    }

    /// ``resume()`` if it has not run, then ``start()``: the frames begin.
    public func run() throws {
        if !isResumed {
            try resume()
        }
        start()
    }

    /// Tick the engine on every display refresh, and from the watchdog while
    /// the display link is stopped. Ticking waits for ``resume()``.
    public override func start() {
        guard !hasExited else { return }
        super.start()
        watchdog.start()
    }

    /// Stop ticking.
    public override func stop() {
        super.stop()
        watchdog.stop()
    }

    private func displayLinkFrame() {
        watchdog.displayLinkFired()
        driveFrame()
    }

    /// A frame the watchdog drives; the display link's are followed by the
    /// view's event poll, so this polls too.
    private func watchdogFrame() {
        driveFrame()
        _ = try? context.pollEvents()
    }

    /// One engine frame, then what it asks for.
    func driveFrame() {
        guard isResumed, !hasExited else { return }
        carryOut(engine.tick(.engine).requests)
    }

    // MARK: - Events

    /// Push `events` to the engine, in order, and do what they ask.
    @discardableResult
    public func push(_ events: HostEvent...) -> HostRequests {
        for event in events {
            onPush?(event)
        }
        let requests = engine.push(contentsOf: events)
        carryOut(requests)
        return requests
    }

    /// Do what the engine asked, in the Rust runner's order: full screen, the
    /// menu, exit.
    private func carryOut(_ requests: HostRequests) {
        if requests.contains(.toggleFullScreen) {
            window?.toggleFullScreen(nil)
        }
        if requests.contains(.syncMenu) {
            syncMenu()
        }
        if requests.contains(.exit), !hasExited {
            hasExited = true
            stop()
            onExit?()
        }
    }

    /// Whether the view's window is full screen.
    public var isFullScreen: Bool {
        window?.styleMask.contains(.fullScreen) ?? false
    }

    /// Show the engine's settings and the window's full-screen state on
    /// ``menuBar``.
    public func syncMenu() {
        menuBar?.sync(engine: engine, fullScreen: isFullScreen)
    }

    /// A Graphics menu for this view's engine, whose choices go to the engine,
    /// kept in sync as ``menuBar``. Install it in the menu bar.
    public func makeMenuBar() -> GraphicsMenuBar {
        let bar = GraphicsMenuBar(engine: engine, fullScreen: isFullScreen) { [weak self] tag in
            self?.push(.menu(tag))
        }
        menuBar = bar
        return bar
    }

    // MARK: - Size and scale

    /// The view's size in physical pixels, each side `round(points × backing
    /// scale)` (winit's `inner_size`), at its backing scale.
    public var metrics: ViewMetrics {
        let backing = Double(backingScale)
        func side(_ points: CGFloat) -> Int {
            let pixels = (Double(points) * backing).rounded()
            return pixels >= Double(UInt32.max) ? Int(UInt32.max) : Int(Swift.max(0, pixels))
        }
        return ViewMetrics(width: side(bounds.width), height: side(bounds.height), scaleFactor: backing)
    }

    /// The engine sizes the drawable, for its own render scale: report the
    /// change instead, once the engine has a view and only if the view's
    /// metrics changed. A new size is a resize; a new backing scale alone (a
    /// move to another display) is a scale change.
    override func layoutDrawable(_ change: DrawableChange) {
        guard isResumed else { return }
        let current = metrics
        guard current != reportedMetrics else { return }
        reportedMetrics = current
        switch change {
        case .size:
            push(.viewResized(current))
        case .backing, .window, .renderScale:
            push(.scaleChanged(current))
        }
    }

    // MARK: - Keyboard and mouse

    private func key(_ keyCode: UInt16, isDown: Bool, isRepeat: Bool) {
        guard let key = Key(macKeyCode: keyCode) else { return }
        push(isDown ? .keyDown(key, isRepeat: isRepeat) : .keyUp(key))
    }

    private func button(_ number: Int, isDown: Bool) {
        guard let button = MouseButton(macButtonNumber: number) else { return }
        push(isDown ? .mouseDown(button) : .mouseUp(button))
    }

    private func cursor(_ event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        push(.cursor([Double(p.x), Double(bounds.height - p.y)]))
    }

    public override func mouseDown(with event: NSEvent) { button(0, isDown: true) }
    public override func mouseUp(with event: NSEvent) { button(0, isDown: false) }
    public override func rightMouseDown(with event: NSEvent) { button(1, isDown: true) }
    public override func rightMouseUp(with event: NSEvent) { button(1, isDown: false) }
    public override func otherMouseDown(with event: NSEvent) { button(event.buttonNumber, isDown: true) }
    public override func otherMouseUp(with event: NSEvent) { button(event.buttonNumber, isDown: false) }
    public override func mouseMoved(with event: NSEvent) { cursor(event) }
    public override func mouseDragged(with event: NSEvent) { cursor(event) }
    public override func rightMouseDragged(with event: NSEvent) { cursor(event) }
    public override func otherMouseDragged(with event: NSEvent) { cursor(event) }

    // MARK: - The window

    public override func viewWillMove(toWindow newWindow: NSWindow?) {
        super.viewWillMove(toWindow: newWindow)
        if let observedWindow {
            let center = NotificationCenter.default
            center.removeObserver(self, name: NSWindow.didResignKeyNotification, object: observedWindow)
            center.removeObserver(self, name: NSWindow.didEnterFullScreenNotification, object: observedWindow)
            center.removeObserver(self, name: NSWindow.didExitFullScreenNotification, object: observedWindow)
            self.observedWindow = nil
        }
    }

    public override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        guard let window else { return }
        let center = NotificationCenter.default
        center.addObserver(
            self, selector: #selector(windowDidResignKey(_:)), name: NSWindow.didResignKeyNotification, object: window
        )
        center.addObserver(
            self, selector: #selector(windowFullScreenDidChange(_:)), name: NSWindow.didEnterFullScreenNotification,
            object: window
        )
        center.addObserver(
            self, selector: #selector(windowFullScreenDidChange(_:)), name: NSWindow.didExitFullScreenNotification,
            object: window
        )
        observedWindow = window
    }

    /// macOS sends no key-up for a key released while another window has the
    /// keyboard: let go of everything, as the Rust runner does.
    @objc private func windowDidResignKey(_ notification: Notification) {
        push(.focusLost)
    }

    @objc private func windowFullScreenDidChange(_ notification: Notification) {
        syncMenu()
    }
}
