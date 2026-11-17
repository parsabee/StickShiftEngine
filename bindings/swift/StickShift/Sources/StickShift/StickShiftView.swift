// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

#if canImport(AppKit)
import AppKit
import QuartzCore

/// An `NSView` the engine renders into: it hosts a `CAMetalLayer`, owns the
/// ``Context`` that draws to it, keeps the drawable sized to the view in pixels,
/// and drives a render loop from the display's refresh.
///
/// ```swift
/// let view = try StickShiftView(size: window.contentLayoutRect.size)
/// let mesh = try view.context.makeMesh(...)
/// view.onFrame = { context, time in
///     try? context.frame { frame in
///         try frame.setCamera(camera)
///         try frame.draw(mesh)
///     }
/// }
/// window.contentView = view
/// view.start()
/// ```
///
/// Each display refresh calls ``onFrame`` and then ``Context/pollEvents()``, so
/// model loads, finished frames and read-backs are handled without more code.
///
/// While it has an ``onKey`` handler, the view takes the keyboard: it becomes
/// its window's first responder (when it joins a window, or when the handler is
/// set while it is in one) and hands every key-down and key-up it receives to
/// ``onKey``, including the key-ups AppKit itself drops while Command is held.
/// Without a handler it does not take focus, and key events go on up the
/// responder chain as they would without the view.
@MainActor
open class StickShiftView: NSView {
    /// The context drawing into this view's layer.
    public let context: Context
    /// The layer the context renders into.
    public let metalLayer: CAMetalLayer

    /// Called once per display refresh while the view is ``start()``ed, with the
    /// context and the time the frame is expected on screen (seconds, on the
    /// `CACurrentMediaTime` clock). Record and submit a frame here.
    public var onFrame: ((Context, CFTimeInterval) -> Void)?

    /// Called for each key-down and key-up the view receives, with the key's
    /// virtual key code (`NSEvent.keyCode`, which names a physical key
    /// position, whatever the keyboard layout), `true` for a key-down, and
    /// `true` for the auto-repeats of a held key.
    ///
    /// Setting a handler makes the view take the keyboard: it becomes first
    /// responder now if it is in a window, and whenever it joins one. While it
    /// has a handler the view passes no key event on, so AppKit does not beep
    /// for keys the game ignores. Menu key equivalents such as ⌘Q still reach
    /// the menus, which see them first. With `nil` (the default), the view
    /// does not take focus and passes every key event on.
    ///
    /// A key released while Command is held still arrives here, so no key is
    /// left down. `NSApplication` never hands a window such a key-up (winit
    /// overrides `sendEvent(_:)` for the same reason); while the view is in a
    /// window, a local event monitor sends it to that window itself, which
    /// delivers it to the first responder. The view forwards only the key-ups
    /// meant for its window while it is that window's first responder.
    public var onKey: ((_ keyCode: UInt16, _ isDown: Bool, _ isRepeat: Bool) -> Void)? {
        didSet {
            if onKey != nil {
                takeKeyboard()
            }
        }
    }

    /// The drawable's resolution as a fraction of the view's native pixels:
    /// at 0.5, a Retina view renders one pixel per point and Core Animation
    /// scales the frame up to fill the view. The context's UI scale
    /// (``Context/scale``) becomes the backing scale × `renderScale`, so a HUD
    /// laid out in points keeps its size. Setting it resizes the drawable.
    /// Default 1; it must be finite and greater than zero.
    public var renderScale: Double {
        didSet {
            precondition(Self.isValid(renderScale), "renderScale must be finite and greater than zero")
            resizeDrawable()
        }
    }

    private var displayLink: CADisplayLink?
    /// Whether ``start()`` was called (and ``stop()`` not since).
    private var wantsToRun = false
    /// The local monitor that forwards Command key-ups, while the view is in a
    /// window. Removed in `deinit`, which is not on the main actor.
    private nonisolated(unsafe) var commandKeyUpMonitor: Any?
    /// How many Command key-up monitors are installed, across every view (for
    /// the tests).
    nonisolated(unsafe) static private(set) var commandKeyUpMonitors = 0

    /// Creates the view, its layer, and a Metal context sized to `size` (in
    /// points) at `renderScale` of its native resolution (see
    /// ``renderScale``). Put it in a window, as a window's content view or a
    /// subview.
    ///
    /// - Throws: ``StickShiftError/Code-swift.struct/noDevice`` without a Metal
    ///   device.
    public init(size: NSSize, renderScale: Double = 1) throws {
        precondition(Self.isValid(renderScale), "renderScale must be finite and greater than zero")
        let frame = NSRect(origin: .zero, size: size)
        let layer = CAMetalLayer()
        let backing = NSScreen.main?.backingScaleFactor ?? 2
        layer.contentsScale = backing
        let drawable = Self.drawable(frame.size, backing: backing, renderScale: renderScale)
        metalLayer = layer
        self.renderScale = renderScale
        context = try Context(layer: layer, width: drawable.width, height: drawable.height, scale: drawable.scale)
        super.init(frame: frame)
        wantsLayer = true
        self.layer = layer
        layerContentsRedrawPolicy = .duringViewResize
    }

    @available(*, unavailable)
    public required init?(coder: NSCoder) {
        fatalError("StickShiftView is created in code: init(size:)")
    }

    deinit {
        removeCommandKeyUpMonitor()
    }

    private static func isValid(_ renderScale: Double) -> Bool {
        renderScale.isFinite && renderScale > 0
    }

    /// The drawable for a view of `size` points on a display of `backing`
    /// pixels per point, at `renderScale`: each side's native pixels,
    /// `round(points × backing)`, times `renderScale`, rounded (at least 1);
    /// and the UI scale, `backing × renderScale`. Rounding is half away from
    /// zero, and the scale is multiplied in `Double` before narrowing.
    private static func drawable(_ size: NSSize, backing: CGFloat, renderScale: Double)
        -> (width: Int, height: Int, scale: Float)
    {
        func side(_ points: CGFloat) -> Int {
            let native = (Double(points) * Double(backing)).rounded()
            return max(1, Int((native * renderScale).rounded()))
        }
        return (side(size.width), side(size.height), Float(Double(backing) * renderScale))
    }

    private var backingScale: CGFloat {
        window?.backingScaleFactor ?? metalLayer.contentsScale
    }

    private func resizeDrawable() {
        let backing = backingScale
        metalLayer.contentsScale = backing
        let drawable = Self.drawable(bounds.size, backing: backing, renderScale: renderScale)
        try? context.resize(width: drawable.width, height: drawable.height, scale: drawable.scale)
    }

    open override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        resizeDrawable()
    }

    open override func viewDidChangeBackingProperties() {
        super.viewDidChangeBackingProperties()
        resizeDrawable()
    }

    open override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        resizeDrawable()
        if onKey != nil {
            takeKeyboard()
        }
        // A display link belongs to a screen, so it is rebuilt for each window.
        // It also retains the view: dropping it while the view is out of a
        // window is what lets an unused view be freed.
        displayLink?.invalidate()
        displayLink = nil
        attachDisplayLink()
        monitorCommandKeyUps()
    }

    /// Whether the render loop is running: started, and in a window.
    public var isRunning: Bool { displayLink != nil }

    /// Start calling ``onFrame`` on every display refresh. The loop runs while
    /// the view is in a window, and resumes when it is put back in one.
    public func start() {
        wantsToRun = true
        attachDisplayLink()
    }

    /// Stop the render loop.
    public func stop() {
        wantsToRun = false
        displayLink?.invalidate()
        displayLink = nil
    }

    private func attachDisplayLink() {
        guard wantsToRun, displayLink == nil, window != nil else { return }
        let link = displayLink(target: self, selector: #selector(tick(_:)))
        link.add(to: .main, forMode: .common)
        displayLink = link
    }

    @objc private func tick(_ link: CADisplayLink) {
        onFrame?(context, link.targetTimestamp)
        _ = try? context.pollEvents()
    }

    // MARK: - Keyboard

    /// Whether the view takes key events: while it has an ``onKey`` handler.
    open override var acceptsFirstResponder: Bool { onKey != nil }

    /// Become the window's first responder, if the view is in one and is not
    /// already.
    private func takeKeyboard() {
        guard let window, window.firstResponder !== self else { return }
        window.makeFirstResponder(self)
    }

    /// Hands the key to ``onKey`` and passes it no further; without a handler,
    /// passes it on.
    open override func keyDown(with event: NSEvent) {
        if let onKey {
            onKey(event.keyCode, true, event.isARepeat)
        } else {
            super.keyDown(with: event)
        }
    }

    /// Hands the key to ``onKey`` and passes it no further; without a handler,
    /// passes it on.
    open override func keyUp(with event: NSEvent) {
        if let onKey {
            onKey(event.keyCode, false, false)
        } else {
            super.keyUp(with: event)
        }
    }

    /// Watch for Command key-ups while the view is in a window, and not
    /// otherwise.
    private func monitorCommandKeyUps() {
        removeCommandKeyUpMonitor()
        guard window != nil else { return }
        commandKeyUpMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyUp) { [weak self] event in
            let forwarded = MainActor.assumeIsolated { self?.forwardCommandKeyUp(event) ?? false }
            return forwarded ? nil : event
        }
        if commandKeyUpMonitor != nil {
            Self.commandKeyUpMonitors += 1
        }
    }

    /// Remove the Command key-up monitor, if one is installed.
    private nonisolated func removeCommandKeyUpMonitor() {
        guard let monitor = commandKeyUpMonitor else { return }
        NSEvent.removeMonitor(monitor)
        commandKeyUpMonitor = nil
        Self.commandKeyUpMonitors -= 1
    }

    /// Send `event` to the view's window if it is a key-up that
    /// `NSApplication.sendEvent(_:)` would drop: Command held, meant for the
    /// view's window (the key window, or the event's own when no window is
    /// key), and the view that window's first responder with an ``onKey``
    /// handler. Returns whether it did, so the monitor can consume it.
    private func forwardCommandKeyUp(_ event: NSEvent) -> Bool {
        guard
            onKey != nil,
            event.type == .keyUp,
            event.modifierFlags.contains(.command),
            let window,
            (NSApp.keyWindow ?? event.window) === window,
            window.firstResponder === self
        else { return false }
        window.sendEvent(event)
        return true
    }
}
#endif
