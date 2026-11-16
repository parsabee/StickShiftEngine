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

    private var displayLink: CADisplayLink?
    /// Whether ``start()`` was called (and ``stop()`` not since).
    private var wantsToRun = false

    /// Creates the view, its layer, and a Metal context sized to `size` (in
    /// points). Put it in a window, as a window's content view or a subview.
    ///
    /// - Throws: ``StickShiftError/Code-swift.struct/noDevice`` without a Metal
    ///   device.
    public init(size: NSSize) throws {
        let frame = NSRect(origin: .zero, size: size)
        let layer = CAMetalLayer()
        let scale = NSScreen.main?.backingScaleFactor ?? 2
        layer.contentsScale = scale
        let (width, height) = Self.pixels(frame.size, scale: scale)
        metalLayer = layer
        context = try Context(layer: layer, width: width, height: height, scale: Float(scale))
        super.init(frame: frame)
        wantsLayer = true
        self.layer = layer
        layerContentsRedrawPolicy = .duringViewResize
    }

    @available(*, unavailable)
    public required init?(coder: NSCoder) {
        fatalError("StickShiftView is created in code: init(size:)")
    }

    /// The view's size in drawable pixels at `scale` pixels per point, at least 1.
    private static func pixels(_ size: NSSize, scale: CGFloat) -> (Int, Int) {
        (max(1, Int((size.width * scale).rounded())), max(1, Int((size.height * scale).rounded())))
    }

    private var scale: CGFloat {
        window?.backingScaleFactor ?? metalLayer.contentsScale
    }

    private func resizeDrawable() {
        metalLayer.contentsScale = scale
        let (width, height) = Self.pixels(bounds.size, scale: scale)
        try? context.resize(width: width, height: height, scale: Float(scale))
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
        // A display link belongs to a screen, so it is rebuilt for each window.
        // It also retains the view: dropping it while the view is out of a
        // window is what lets an unused view be freed.
        displayLink?.invalidate()
        displayLink = nil
        attachDisplayLink()
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
}
#endif
