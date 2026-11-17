// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import AppKit
import Testing
import simd

@testable import StickShift

/// On the Metal backend; each test skips without a device (and fails instead
/// with `STICKSHIFT_REQUIRE_GPU=1`).
@MainActor
@Suite("Metal")
struct MetalTests {
    /// Render the triangle into a 64 × 48 frame and return its BGRA pixels.
    private func render(
        _ context: Context,
        sun: SunSky = .default,
        lights: [Light] = [],
        emissive: SIMD3<Float> = .zero
    ) async throws -> [UInt8] {
        let (vertices, indices) = triangle()
        let mesh = try context.makeMesh(layout: .positionNormalColor, vertices: vertices, indices: indices)
        var readback: PixelReadback?
        try context.frame { frame in
            try frame.setCamera(camera(aspect: 64.0 / 48.0))
            try frame.setSunSky(sun)
            for light in lights {
                try frame.addLight(light)
            }
            try frame.draw(mesh, material: Material(emissive: emissive))
            readback = try frame.readPixels()
        }
        return try await readback!.pixels()
    }

    @Test func anOffscreenFrameRendersAndReadsBack() async throws {
        guard let context = try metalContext() else { return }
        let pixels = try await render(context)
        #expect(pixels.count == 64 * 48 * 4)
        #expect(Set(pixels).count > 8, "the frame has content")
        // The triangle's centre is red-dominated (BGRA).
        let centre = (24 * 64 + 32) * 4
        #expect(pixels[centre + 2] > pixels[centre])
    }

    @Test func lightsAndEmissionLightTheNight() async throws {
        guard let context = try metalContext() else { return }
        var night = SunSky.default
        night.sunIntensity = 0
        night.skyFill = 0.02
        let dark = try await render(context, sun: night)
        let lit = try await render(context, sun: night, lights: [.point(at: [0, 0, 0.6], intensity: 2, range: 3)])
        let glowing = try await render(context, sun: night, emissive: [0.6, 0.3, 0.1])
        let centre = (24 * 64 + 32) * 4
        #expect(lit[centre + 2] > dark[centre + 2])
        #expect(glowing[centre + 2] > dark[centre + 2])
    }

    @Test func aReadbackArrivesThroughFlushToo() throws {
        guard let context = try metalContext() else { return }
        var readback: PixelReadback?
        try context.frame { readback = try $0.readPixels() }
        try context.flush()
        #expect(readback?.state == .complete)
        #expect(readback?.bytes?.count == 64 * 48 * 4)
    }

    @Test func aViewHostsAContextSizedInPixels() throws {
        guard try metalContext() != nil else { return }
        let view = try StickShiftView(size: NSSize(width: 64, height: 48))
        let scale = Int(view.metalLayer.contentsScale)
        #expect(view.renderScale == 1)
        #expect(view.context.size == (64 * scale, 48 * scale))
        #expect(view.context.scale == Float(view.metalLayer.contentsScale))
        view.setFrameSize(NSSize(width: 100, height: 50))
        #expect(view.context.size == (100 * scale, 50 * scale))
        // Out of a window, the render loop does not run.
        view.start()
        #expect(!view.isRunning)
        view.stop()
    }

    /// Record a frame from the triangle test's camera with `record`, and return
    /// its BGRA pixels.
    private func pixels(_ context: Context, _ record: (Frame) throws -> Void) async throws -> [UInt8] {
        var readback: PixelReadback?
        try context.frame { frame in
            try frame.setCamera(camera(aspect: 64.0 / 48.0))
            try record(frame)
            readback = try frame.readPixels()
        }
        return try await readback!.pixels()
    }

    @Test func aModelDrawsAtWorldTimesLocal() async throws {
        guard let context = try metalContext() else { return }
        let cube = try await context.loadModel(contentsOf: texturedCubeFixture)
        // Values whose products are exact in any grouping, so the matrices
        // `world × local` and `world × (local × node)` are the same bits.
        let world = simd_float4x4(translation: [0.25, 0, -1])
        let local = simd_float4x4(translation: .zero, scale: [0.5, 0.5, 0.5])
        let red: SIMD4<Float> = [1, 0.25, 0.25, 1]

        let sky = try await pixels(context) { _ in }
        let placed = try await pixels(context) { try $0.draw(cube, world: world, local: local) }
        let byOneMatrix = try await pixels(context) { try $0.draw(cube, transform: StickShiftMath.multiply(world, local)) }
        let swapped = try await pixels(context) { try $0.draw(cube, transform: StickShiftMath.multiply(local, world)) }
        #expect(placed != sky)
        #expect(placed == byOneMatrix, "world applies after local")
        #expect(placed != swapped)

        let tinted = try await pixels(context) { try $0.draw(cube, world: world, local: local, tint: red) }
        let tintedByOneMatrix = try await pixels(context) {
            try $0.draw(cube, transform: StickShiftMath.multiply(world, local), tint: red)
        }
        #expect(tinted == tintedByOneMatrix)
        #expect(tinted != placed)

        // The cube's one part is textured.
        let textured = try await pixels(context) { try $0.draw(cube, world: world, local: local, parts: .textured) }
        let untextured = try await pixels(context) { try $0.draw(cube, world: world, local: local, parts: .untextured) }
        #expect(textured == placed)
        #expect(untextured == sky)
    }

    @Test func aViewRendersAtAFractionOfItsNativeResolution() throws {
        guard try metalContext() != nil else { return }
        let size = NSSize(width: 101, height: 51)
        let view = try StickShiftView(size: size, renderScale: 0.5)
        let backing = Double(view.metalLayer.contentsScale)
        // Native pixels, rounded; times the render scale, rounded half away
        // from zero; at least one pixel.
        func expected(_ renderScale: Double, _ size: NSSize) -> (Int, Int) {
            func side(_ points: CGFloat) -> Int {
                max(1, Int(((Double(points) * backing).rounded() * renderScale).rounded()))
            }
            return (side(size.width), side(size.height))
        }
        #expect(view.renderScale == 0.5)
        #expect(view.context.size == expected(0.5, size))
        #expect(view.context.scale == Float(backing * 0.5))
        if backing == 2 {
            #expect(view.context.size == (101, 51))
        }

        view.renderScale = 0.75
        #expect(view.context.size == expected(0.75, size))
        #expect(view.context.scale == Float(backing * 0.75))
        if backing == 2 {
            // 202 × 0.75 = 151.5 and 102 × 0.75 = 76.5 round up.
            #expect(view.context.size == (152, 77))
        }

        // A resize keeps the render scale.
        let wider = NSSize(width: 200, height: 100)
        view.setFrameSize(wider)
        #expect(view.context.size == expected(0.75, wider))

        view.renderScale = 1
        #expect(view.context.size == expected(1, wider))
        #expect(view.context.scale == Float(backing))

        // A tiny view still gets a drawable.
        view.renderScale = 0.5
        view.setFrameSize(NSSize(width: 0.2, height: 0.2))
        #expect(view.context.size == (1, 1))
    }

    @Test func aViewTakesTheKeyboardOnlyWithAHandler() throws {
        guard try metalContext() != nil else { return }
        let view = try StickShiftView(size: NSSize(width: 64, height: 48))
        let next = KeyRecorder()
        view.nextResponder = next
        func key(_ type: NSEvent.EventType, _ code: UInt16, repeating: Bool = false) -> NSEvent {
            NSEvent.keyEvent(
                with: type, location: .zero, modifierFlags: [], timestamp: 0, windowNumber: 0, context: nil,
                characters: " ", charactersIgnoringModifiers: " ", isARepeat: repeating, keyCode: code
            )!
        }

        // Without a handler the view is not a key view: keys go on up the chain.
        #expect(!view.acceptsFirstResponder)
        view.keyDown(with: key(.keyDown, 0))
        view.keyUp(with: key(.keyUp, 0))
        #expect(next.received == 2)

        // With one, every key goes to it and stops there.
        next.received = 0
        var keys: [(UInt16, Bool, Bool)] = []
        view.onKey = { keys.append(($0, $1, $2)) }
        #expect(view.acceptsFirstResponder)
        view.keyDown(with: key(.keyDown, 49))
        view.keyDown(with: key(.keyDown, 49, repeating: true))
        view.keyUp(with: key(.keyUp, 49))
        view.keyDown(with: key(.keyDown, 123))
        #expect(keys.map(\.0) == [49, 49, 49, 123])
        #expect(keys.map(\.1) == [true, true, false, true])
        #expect(keys.map(\.2) == [false, true, false, false])
        #expect(next.received == 0)

        // Dropping the handler passes keys on again.
        view.onKey = nil
        view.keyDown(with: key(.keyDown, 0))
        #expect(next.received == 1)
        #expect(keys.count == 4)
    }

    @Test func aViewWithoutAHandlerLeavesTheFocusAlone() throws {
        guard try metalContext() != nil else { return }
        _ = NSApplication.shared
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 200, height: 100), styleMask: [.titled], backing: .buffered,
            defer: true
        )
        window.isReleasedWhenClosed = false
        let container = NSView(frame: NSRect(x: 0, y: 0, width: 200, height: 100))
        let field = NSTextField(frame: NSRect(x: 0, y: 0, width: 100, height: 24))
        container.addSubview(field)
        window.contentView = container
        defer { window.contentView = nil }
        window.makeFirstResponder(field)
        let focused = window.firstResponder
        #expect(focused != nil && focused !== window)

        // Added without a handler, the view does not take the focus.
        let view = try StickShiftView(size: NSSize(width: 64, height: 48))
        container.addSubview(view)
        #expect(window.firstResponder === focused)

        // Setting a handler while it is in the window takes the keyboard.
        view.onKey = { _, _, _ in }
        #expect(window.firstResponder === view)

        // A view that has a handler takes it whenever it joins a window.
        window.makeFirstResponder(field)
        view.removeFromSuperview()
        #expect(window.firstResponder !== view)
        container.addSubview(view)
        #expect(window.firstResponder === view)
        view.removeFromSuperview()
    }

    @Test func aKeyReleasedWhileCommandIsHeldStillReachesTheView() throws {
        guard try metalContext() != nil else { return }
        let app = NSApplication.shared
        let view = try StickShiftView(size: NSSize(width: 64, height: 48))
        var keys: [(code: UInt16, isDown: Bool)] = []
        view.onKey = { code, isDown, _ in keys.append((code, isDown)) }
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 64, height: 48), styleMask: [.titled], backing: .buffered,
            defer: false
        )
        window.isReleasedWhenClosed = false
        window.contentView = view
        window.orderFront(nil)
        defer {
            window.orderOut(nil)
            window.contentView = nil
        }
        /// A ⌘→ key-up for the window, as the window server sends it.
        func commandKeyUp() -> NSEvent {
            NSEvent.keyEvent(
                with: .keyUp, location: .zero, modifierFlags: .command, timestamp: ProcessInfo.processInfo.systemUptime,
                windowNumber: window.windowNumber, context: nil, characters: "\u{F703}",
                charactersIgnoringModifiers: "\u{F703}", isARepeat: false, keyCode: 0x7C
            )!
        }

        // `NSApplication.sendEvent(_:)`, where the run loop hands every event,
        // gives a window no key-up while Command is held. The view's monitor
        // (which runs inside it) sends this one to the window itself, once.
        app.sendEvent(commandKeyUp())
        #expect(keys.map(\.code) == [0x7C])
        #expect(keys.map(\.isDown) == [false])

        // Not while another responder has the keyboard: that is AppKit's business.
        keys.removeAll()
        let other = KeyRecorder()
        window.makeFirstResponder(other)
        app.sendEvent(commandKeyUp())
        #expect(keys.isEmpty)
        #expect(other.received == 0)

        // Nor once the view has left the window.
        window.contentView = nil
        window.makeFirstResponder(view)
        app.sendEvent(commandKeyUp())
        #expect(keys.isEmpty)
    }

    @Test func theCommandKeyUpMonitorLastsOnlyWhileTheViewIsInAWindow() throws {
        guard try metalContext() != nil else { return }
        _ = NSApplication.shared
        func window() -> NSWindow {
            let window = NSWindow(
                contentRect: NSRect(x: 0, y: 0, width: 64, height: 48), styleMask: [.titled], backing: .buffered,
                defer: true
            )
            window.isReleasedWhenClosed = false
            return window
        }
        let base = StickShiftView.commandKeyUpMonitors
        func installed() -> Int { StickShiftView.commandKeyUpMonitors - base }

        let first = window()
        let second = window()
        let view = try StickShiftView(size: NSSize(width: 64, height: 48))
        view.onKey = { _, _, _ in }
        #expect(installed() == 0, "none outside a window")
        first.contentView = view
        #expect(installed() == 1, "one while it is in a window")
        first.contentView = nil
        #expect(installed() == 0, "removed when it leaves")
        first.contentView = view
        #expect(installed() == 1, "and back when it returns")
        // Moved straight to another window: still one, for the new window.
        second.contentView = view
        #expect(first.contentView !== view)
        #expect(installed() == 1, "the first window's is removed")
        second.contentView = nil
        #expect(installed() == 0)

        // A view freed with its window, however AppKit takes it out, leaves none.
        weak var freed: StickShiftView?
        autoreleasepool {
            let window = window()
            let view = try? StickShiftView(size: NSSize(width: 64, height: 48))
            window.contentView = view
            freed = view
            #expect(installed() == 1)
        }
        #expect(freed == nil, "the view was freed")
        #expect(installed() == 0, "its monitor went with it")
    }
}

/// A responder that counts the key events that reach it.
@MainActor
private final class KeyRecorder: NSResponder {
    var received = 0

    override func keyDown(with event: NSEvent) {
        received += 1
    }

    override func keyUp(with event: NSEvent) {
        received += 1
    }
}
