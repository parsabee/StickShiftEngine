// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import CStickShift
import QuartzCore

/// A rendering context: the GPU device, every resource, and the frames drawn
/// with them.
///
/// A context renders into a `CAMetalLayer` you own (see ``StickShiftView``), into
/// an offscreen texture you can read back, or, with ``Backend/null``, nowhere at
/// all, for tests on machines without a GPU.
///
/// The C API lets one thread at a time use a context, so a `Context` and
/// everything it creates are bound to the main actor. Resources (``Mesh``,
/// ``Texture``, ``Model``, ``Font``) keep their context alive and destroy
/// themselves when you drop the last reference to them.
///
/// ```swift
/// let context = try Context(offscreenWidth: 640, height: 360)
/// let mesh = try context.makeMesh(layout: .positionNormalColor, vertices: vertices, indices: indices)
/// try context.frame { frame in
///     try frame.setCamera(camera)
///     try frame.draw(mesh)
/// }
/// ```
@MainActor
public final class Context {
    /// Which renderer a context uses.
    public enum Backend: Sendable {
        /// The Metal renderer.
        case metal
        /// No GPU: resources and frames are tracked and draws counted, but
        /// nothing is drawn and read-backs are zeros. For tests.
        case null
    }

    /// The C context. Used by the resources' `deinit`, which is not isolated.
    nonisolated(unsafe) let raw: OpaquePointer
    /// The layer a layer context renders into, kept alive with it.
    private let layer: CAMetalLayer?
    /// Model loads and read-backs waiting for their completion, which arrives
    /// in a poll (see `pumpWhilePending`).
    var pendingCompletions = 0
    private var pumping = false

    /// Drawable pixels per point: the UI scale, as last given to
    /// ``init(layer:width:height:scale:)`` or ``resize(width:height:scale:)``.
    /// A value that is not finite and greater than zero counts as 1, as it does
    /// for the engine. Always 1 for an offscreen context.
    public private(set) var scale: Float

    /// Creates an offscreen context: it renders into a `width` × `height`
    /// texture whose pixels ``Frame/readPixels()`` returns.
    ///
    /// - Throws: ``StickShiftError/Code-swift.struct/noDevice`` without a Metal
    ///   device (use ``Backend/null`` for GPU-less tests).
    public init(offscreenWidth width: Int, height: Int, backend: Backend = .metal) throws {
        raw = try Self.create(backend: backend, layer: nil, width: width, height: height, scale: 1)
        layer = nil
        scale = 1
    }

    /// Creates a context that renders into `layer`, which you keep in your view
    /// hierarchy. The engine sets the layer's device, pixel format and drawable
    /// size; call ``resize(width:height:scale:)`` when the view changes size.
    ///
    /// - Parameters:
    ///   - width: The drawable width, in pixels.
    ///   - height: The drawable height, in pixels.
    ///   - scale: Pixels per point: the HUD's UI scale.
    public init(layer: CAMetalLayer, width: Int, height: Int, scale: Float) throws {
        raw = try Self.create(
            backend: .metal,
            layer: Unmanaged.passUnretained(layer).toOpaque(),
            width: width,
            height: height,
            scale: scale
        )
        self.layer = layer
        self.scale = Self.effective(scale)
    }

    /// The scale the engine uses for `scale`: 1 unless it is finite and positive.
    private static func effective(_ scale: Float) -> Float {
        scale.isFinite && scale > 0 ? scale : 1
    }

    private static func create(
        backend: Backend,
        layer: UnsafeMutableRawPointer?,
        width: Int,
        height: Int,
        scale: Float
    ) throws(StickShiftError) -> OpaquePointer {
        guard let width = UInt32(exactly: width), let height = UInt32(exactly: height) else {
            throw StickShiftError(code: .invalidArgument, message: "width and height must fit in UInt32")
        }
        var desc = StickShiftContextDesc(
            api_version: UInt32(STICKSHIFT_API_VERSION),
            backend: StickShiftBackend(backend == .metal ? STICKSHIFT_BACKEND_METAL : STICKSHIFT_BACKEND_NULL),
            metal_layer: layer,
            width: width,
            height: height,
            scale: scale
        )
        var out: OpaquePointer?
        try check(stickshift_context_create(&desc, &out))
        guard let out else {
            throw StickShiftError(code: .internal, message: "stickshift_context_create returned no context")
        }
        return out
    }

    deinit {
        // Resources hold their context, so none is left by now. Destroying
        // waits for submitted frames to finish.
        stickshift_context_destroy(raw)
    }

    // MARK: - About

    /// The C API version this package was built against
    /// (`STICKSHIFT_API_VERSION`), which the library must match.
    public nonisolated static var apiVersion: UInt32 { UInt32(STICKSHIFT_API_VERSION) }

    /// The engine release, for example `"0.1.0"`.
    public nonisolated static var engineVersion: String { String(cString: stickshift_version_string()) }

    /// The GPU's name.
    public var deviceName: String {
        String(cString: stickshift_context_device_name(raw))
    }

    // MARK: - Size and settings

    /// The drawable size, in pixels.
    public var size: (width: Int, height: Int) {
        var width: UInt32 = 0
        var height: UInt32 = 0
        stickshift_context_get_size(raw, &width, &height)
        return (Int(width), Int(height))
    }

    /// Resize a layer context's drawable: `width` × `height` pixels at `scale`
    /// pixels per point.
    ///
    /// - Throws: ``StickShiftError/Code-swift.struct/unsupported`` for an
    ///   offscreen context, whose size is fixed.
    public func resize(width: Int, height: Int, scale: Float) throws {
        guard let width = UInt32(exactly: width), let height = UInt32(exactly: height) else {
            throw StickShiftError(code: .invalidArgument, message: "width and height must fit in UInt32")
        }
        try check(stickshift_context_resize(raw, width, height, scale))
        self.scale = Self.effective(scale)
    }

    /// The render quality settings. Setting them applies from the next frame.
    public var settings: RenderSettings {
        var out = StickShiftRenderSettings()
        stickshift_context_get_settings(raw, &out)
        return RenderSettings(out)
    }

    /// Apply render quality settings from the next frame.
    ///
    /// - Throws: ``StickShiftError/Code-swift.struct/invalidArgument`` for a
    ///   value out of range.
    public func setSettings(_ settings: RenderSettings) throws {
        var c = settings.c
        try check(stickshift_context_set_settings(raw, &c))
    }

    /// Counters for diagnostics and tests.
    public var stats: Stats {
        var out = StickShiftStats()
        stickshift_context_get_stats(raw, &out)
        return Stats(out)
    }

    /// How long the GPU spent on completed frame `frame`, or `nil` if it has not
    /// completed (or completed more than 64 frames ago). The null backend
    /// reports zero.
    public func gpuFrameTime(_ frame: UInt64) throws -> Duration? {
        var nanoseconds: UInt64 = 0
        let code = try check(stickshift_context_gpu_frame_time(raw, frame, &nanoseconds))
        return code == STICKSHIFT_OK ? .nanoseconds(Int64(clamping: nanoseconds)) : nil
    }

    // MARK: - Events

    /// Do the context's housekeeping and return the events it queued.
    ///
    /// Housekeeping uploads finished model loads, notices finished GPU frames
    /// and copies finished read-backs; their completions (``loadModel(contentsOf:)``,
    /// ``PixelReadback``) arrive here. Call it once per frame:
    /// ``StickShiftView`` does. While a model load or read-back is pending, the
    /// context also polls by itself, so an `await` on one completes without a
    /// render loop.
    @discardableResult
    public func pollEvents() throws -> [Event] {
        var events: [Event] = []
        var buffer = [StickShiftEvent](repeating: StickShiftEvent(), count: 32)
        while true {
            var count: UInt32 = 0
            _ = try buffer.withUnsafeMutableBufferPointer { b throws in
                try check(stickshift_poll_events(raw, b.baseAddress, UInt32(b.count), &count))
            }
            // Messages are only valid until the next poll: copy them now.
            events += buffer[..<Int(count)].map(Event.init)
            if count < UInt32(buffer.count) {
                return events
            }
        }
    }

    /// Wait for every load and every submitted frame to finish, and deliver
    /// their completions. For tests and tools: it blocks the caller, for at
    /// most 60 seconds.
    ///
    /// - Throws: ``StickShiftError/Code-swift.struct/timeout`` if the work did
    ///   not finish in time.
    public func flush() throws {
        try check(stickshift_context_flush(raw))
    }

    /// Poll every few milliseconds until no load or read-back is pending, so
    /// their completions arrive even when nothing else polls.
    func pumpWhilePending() {
        guard !pumping else { return }
        pumping = true
        Task { @MainActor [weak self] in
            while let self, self.pendingCompletions > 0 {
                _ = try? self.pollEvents()
                try? await Task.sleep(for: .milliseconds(2))
            }
            self?.pumping = false
        }
    }

    // MARK: - Frames

    /// Record and submit one frame.
    ///
    /// `record` receives the open ``Frame`` and records into it: camera, sun,
    /// lights, draws. When it returns, the frame is submitted; if it throws, the
    /// frame is discarded and the error rethrown. One frame is open at a time.
    ///
    /// Never waits for the GPU. With three frames already in flight (or a
    /// zero-sized drawable) there is no frame to record: `record` is not called
    /// and the result is `nil`. Skip that tick's rendering and try again next tick.
    ///
    /// - Returns: The submitted frame's number (from 1), or `nil` if the frame
    ///   was skipped.
    @discardableResult
    public func frame(_ record: (Frame) throws -> Void) throws -> UInt64? {
        var raw: OpaquePointer?
        let acquired = try check(stickshift_frame_acquire(self.raw, &raw))
        guard acquired == STICKSHIFT_OK, let raw else { return nil }
        let frame = Frame(raw: raw, context: self)
        do {
            try record(frame)
        } catch {
            stickshift_frame_discard(raw)
            throw error
        }
        var number: UInt64 = 0
        let submitted = try check(stickshift_frame_submit(raw, nil, &number))
        return submitted == STICKSHIFT_OK ? number : nil
    }
}

/// Something the engine reports: a resource became ready or failed, a frame
/// finished, a read-back arrived.
public struct Event: Sendable, Equatable {
    /// What happened.
    public enum Kind: Sendable, Equatable {
        /// The context can render.
        case contextReady
        /// A resource (``object``) became ready.
        case resourceReady
        /// A resource (``object``) failed; ``error`` says why.
        case resourceFailed
        /// The GPU finished frame ``frame``.
        case frameCompleted
        /// The pixels requested for frame ``frame`` arrived.
        case readbackComplete
        /// The pixels requested for frame ``frame`` will never arrive.
        case readbackFailed
        /// A kind this package does not know yet.
        case other(UInt32)
    }

    /// What happened.
    public let kind: Kind
    /// Why it failed, for a failure.
    public let error: StickShiftError?
    /// The handle id the event is about, or 0.
    public let object: UInt64
    /// The frame number the event is about, or 0.
    public let frame: UInt64

    init(_ e: StickShiftEvent) {
        switch Int32(bitPattern: e.kind) {
        case STICKSHIFT_EVENT_CONTEXT_READY: kind = .contextReady
        case STICKSHIFT_EVENT_RESOURCE_READY: kind = .resourceReady
        case STICKSHIFT_EVENT_RESOURCE_FAILED: kind = .resourceFailed
        case STICKSHIFT_EVENT_FRAME_COMPLETED: kind = .frameCompleted
        case STICKSHIFT_EVENT_READBACK_COMPLETE: kind = .readbackComplete
        case STICKSHIFT_EVENT_READBACK_FAILED: kind = .readbackFailed
        default: kind = .other(e.kind)
        }
        error = e.result < 0
            ? StickShiftError(
                code: StickShiftError.Code(rawValue: e.result),
                message: e.message.map { String(cString: $0) } ?? ""
            )
            : nil
        object = e.object
        frame = e.frame
    }
}

/// Render quality settings.
public struct RenderSettings: Sendable, Equatable {
    /// How the sun's shadows are drawn.
    public enum ShadowQuality: UInt32, Sendable {
        /// No shadow pass.
        case off = 0
        /// A 2048² shadow map.
        case low = 1
        /// A 4096² shadow map (the default).
        case high = 2
    }

    /// The shadow tier.
    public var shadows: ShadowQuality
    /// How far shadows reach, as a fraction `0...1` of the draw distance.
    public var shadowDistance: Float
    /// A scale on the draw distance (where the fog closes in).
    public var drawDistanceScale: Float

    /// The engine's defaults.
    public nonisolated static var `default`: RenderSettings {
        var out = StickShiftRenderSettings()
        stickshift_render_settings_default(&out)
        return RenderSettings(out)
    }

    /// Creates settings.
    public init(shadows: ShadowQuality, shadowDistance: Float, drawDistanceScale: Float) {
        self.shadows = shadows
        self.shadowDistance = shadowDistance
        self.drawDistanceScale = drawDistanceScale
    }

    init(_ c: StickShiftRenderSettings) {
        shadows = ShadowQuality(rawValue: c.shadows) ?? .high
        shadowDistance = c.shadow_distance
        drawDistanceScale = c.draw_distance_scale
    }

    var c: StickShiftRenderSettings {
        StickShiftRenderSettings(
            shadows: shadows.rawValue,
            shadow_distance: shadowDistance,
            draw_distance_scale: drawDistanceScale
        )
    }
}

/// A context's counters.
public struct Stats: Sendable, Equatable {
    /// Frames handed to the GPU.
    public var framesSubmitted: UInt64
    /// Frames the GPU has finished.
    public var framesCompleted: UInt64
    /// Frames skipped at acquire or submit.
    public var framesSkipped: UInt64
    /// 3D draws handed to the GPU: one per mesh draw, one per model part.
    public var drawsSubmitted: UInt64
    /// Draw calls not made because a resource was pending, failed or destroyed.
    public var drawsSkipped: UInt64
    /// Live meshes.
    public var meshes: Int
    /// Live textures.
    public var textures: Int
    /// Live models.
    public var models: Int
    /// Live fonts.
    public var fonts: Int
    /// Model loads still in progress.
    public var loadsPending: Int

    init(_ s: StickShiftStats) {
        framesSubmitted = s.frames_submitted
        framesCompleted = s.frames_completed
        framesSkipped = s.frames_skipped
        drawsSubmitted = s.draws_submitted
        drawsSkipped = s.draws_skipped
        meshes = Int(s.meshes)
        textures = Int(s.textures)
        models = Int(s.models)
        fonts = Int(s.fonts)
        loadsPending = Int(s.loads_pending)
    }
}
