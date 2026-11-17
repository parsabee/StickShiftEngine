// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import CStickShift
import Foundation

/// The engine: the game loop, attached to a ``Context``, running a ``Game``.
///
/// The host drives it. A tool or a test initializes it and calls
/// ``advance(readback:)`` once per frame; a view calls ``resume(_:)`` when it
/// appears, forwards its events with ``push(_:)``, and calls
/// ``tick(_:readback:)`` from its display link. Each tick runs the engine's own
/// frame, the same code a Rust game runs: the fixed-step accumulator decides
/// how many times ``Game/update(_:dt:)`` runs, then ``Game/render(_:into:)``
/// draws the frame the engine opened, and the engine submits it.
///
/// ```swift
/// let engine = try Engine.headless(Spinner())
/// try engine.initialize()
/// for _ in 0..<60 {
///     engine.advance()          // one fixed step, one frame
/// }
/// ```
///
/// An engine and its context belong to the main actor. Dropping the engine
/// destroys it on the main thread, even if the last reference goes elsewhere;
/// its context outlives it.
@MainActor
public final class Engine {
    /// How an engine is set up.
    public struct Options: Sendable, Equatable {
        /// Where sound goes: ``AudioOutput/silent`` (tools and tests) or the
        /// default ``AudioOutput/device``.
        public var audio: AudioOutput
        /// Restore the player's graphics settings at create, and save them on
        /// every change.
        public var persistsSettings: Bool
        /// With ``persistsSettings``: the user-defaults suite to keep them in,
        /// or `nil` for the standard one. Tests pass a throwaway suite.
        public var settingsSuite: String?
        /// An Escape key-down asks the host to exit (and still presses the key).
        public var quitsOnEscape: Bool
        /// After init, run no update or render while a model is still loading.
        public var holdsUntilLoaded: Bool
        /// What a tick does when the GPU has no free frame slot.
        public var framePacing: FramePacing

        /// Creates options.
        public init(
            audio: AudioOutput = .silent,
            persistsSettings: Bool = false,
            settingsSuite: String? = nil,
            quitsOnEscape: Bool = false,
            holdsUntilLoaded: Bool = false,
            framePacing: FramePacing = .wait
        ) {
            self.audio = audio
            self.persistsSettings = persistsSettings
            self.settingsSuite = settingsSuite
            self.quitsOnEscape = quitsOnEscape
            self.holdsUntilLoaded = holdsUntilLoaded
            self.framePacing = framePacing
        }

        /// Tools and tests: silent, nothing saved, frames waited for.
        public static let headless = Options()

        /// An app: the default sound device, saved settings, Escape quits.
        public static let app = Options(audio: .device, persistsSettings: true, quitsOnEscape: true)
    }

    /// Where an engine's sound goes.
    public enum AudioOutput: Sendable, Equatable {
        /// Nowhere: every audio call is accepted and counted, nothing plays.
        case silent
        /// The default output device, or silent (with one line on stderr) if
        /// there is none.
        case device
    }

    /// What a tick does when the GPU has no free frame slot.
    public enum FramePacing: Sendable, Equatable {
        /// Wait (at most a second) for one before rendering. Nothing is lost.
        case wait
        /// Never wait: render still runs, into a frame that is then discarded.
        case skip
    }

    /// Where a tick's elapsed time comes from.
    public enum Clock: Sendable, Equatable {
        /// Exactly one fixed step: tools, traces and tests.
        case fixedStep
        /// The given time passed since the last tick.
        case elapsed(nanoseconds: UInt64)
        /// The host's monotonic clock reads `nanoseconds` now (on Apple
        /// platforms, `clock_gettime_nsec_np(CLOCK_UPTIME_RAW)`); elapsed is
        /// the time since the last such tick. The first tick, and the first
        /// after a menu choice, ``Engine/resetClock()`` or a held tick, banks
        /// exactly one fixed step.
        case timestamp(nanoseconds: UInt64)
        /// As ``timestamp(nanoseconds:)``, with the engine reading the
        /// monotonic clock itself, where the Rust windowed runner reads it.
        /// What a display link should use.
        case engine
    }

    /// The context the engine renders through. It outlives the engine.
    public var context: Context { core.context }
    /// The C engine. Used by `deinit`, which is not isolated.
    nonisolated(unsafe) let raw: OpaquePointer
    /// The game box. Unowned: the C engine's retain, which `stickshift_engine_destroy`
    /// gives back through the `release` hook, is its only owner. So the game,
    /// and every resource it holds, is released where the engine is destroyed
    /// (on the main thread), never on a thread that dropped the last reference.
    unowned let core: EngineCore
    /// A test's stand-in for the thread that drops the engine being preempted:
    /// run by an off-main `deinit` after it has sent the destroy to the main thread.
    nonisolated(unsafe) var afterSendingDestroy: (@Sendable () -> Void)?

    /// Create an engine on `context` that runs `game`. No hook runs yet: call
    /// ``initialize()`` (or ``resume(_:)``, for a view) next.
    ///
    /// - Throws: ``StickShiftError/Code-swift.struct/unsupported`` if the
    ///   linked library is older than this package;
    ///   ``StickShiftError/Code-swift.struct/wrongState`` if `context` already
    ///   has an engine or an open frame;
    ///   ``StickShiftError/Code-swift.struct/invalidArgument`` for a
    ///   ``Options/settingsSuite`` without ``Options/persistsSettings``.
    public init(context: Context, game: some Game, options: Options = .headless) throws {
        try Self.requireLibrary(minor: Self.linkedAPIMinor())
        let core = EngineCore(game: game, context: context)
        var hooks = core.retainedHooks()
        var desc = options.c
        var out: OpaquePointer?
        let result: StickShiftResult
        if let suite = options.settingsSuite {
            result = suite.withCString { suite in
                desc.settings_suite = suite
                return stickshift_engine_create(context.raw, &desc, &hooks, &out)
            }
        } else {
            result = stickshift_engine_create(context.raw, &desc, &hooks, &out)
        }
        guard result >= 0, let out else {
            // A failed create calls no hook, release included: balance the retain here.
            Unmanaged<EngineCore>.fromOpaque(hooks.user).release()
            try check(result)
            throw StickShiftError(code: .internal, message: "stickshift_engine_create returned no engine")
        }
        // From here the C retain owns the box; this local reference ends with init.
        self.core = core
        raw = out
    }

    /// An engine with no GPU, as Rust's `Headless` harness sets one up: a
    /// null context of 1280 × 720 pixels at scale 1, and ``Options/headless``.
    public static func headless(_ game: some Game) throws -> Engine {
        try Engine(context: Context(offscreenWidth: 1280, height: 720, backend: .null), game: game)
    }

    deinit {
        // The C retain keeps `core` alive until the destroy, so reading it here
        // (on any thread) is safe, and never its last release.
        let doomed = DoomedEngine(raw: raw, context: core.context)
        if Thread.isMainThread {
            doomed.destroy()
        } else {
            // The C engine is bound to the thread that created it, the main
            // one: destroy it there, then drop the context it was attached to.
            DispatchQueue.main.async { doomed.destroy() }
            afterSendingDestroy?()
        }
    }

    // MARK: - Versions and the fixed step

    /// The library's additive level within ``Context/apiVersion``
    /// (`stickshift_api_minor()`); an engine needs at least this package's.
    public nonisolated static var libraryAPIMinor: UInt32 { stickshift_api_minor() }

    /// The fixed simulation step in seconds, exactly as the engine uses it:
    /// the `Float` nearest 1/60. Every ``Game/update(_:dt:)`` gets it.
    public nonisolated static var fixedDeltaTime: Float { stickshift_fixed_dt() }

    /// The fixed step in whole nanoseconds: what ``Clock/fixedStep`` banks.
    public nonisolated static var fixedStepNanoseconds: UInt64 { UInt64(STICKSHIFT_FIXED_DT_NS) }

    /// The most updates one tick runs; time owed past them is dropped.
    public nonisolated static var maxStepsPerFrame: Int { Int(STICKSHIFT_MAX_STEPS_PER_FRAME) }

    /// The linked library's API minor, as `init` checks it. A test swaps it to
    /// stand in for an older library, inside one synchronous test body.
    static var linkedAPIMinor: () -> UInt32 = { stickshift_api_minor() }

    /// Throw unless a library reporting API minor `minor` has everything this
    /// package calls.
    nonisolated static func requireLibrary(minor: UInt32) throws(StickShiftError) {
        let needed = UInt32(STICKSHIFT_API_MINOR)
        guard minor >= needed else {
            throw StickShiftError(
                code: .unsupported,
                message: "libstickshift is API \(STICKSHIFT_API_VERSION).\(minor), but this package needs "
                    + "\(STICKSHIFT_API_VERSION).\(needed): link a newer libstickshift"
            )
        }
    }

    // MARK: - Starting

    /// Run the game's optional ``Game/load(_:)`` phase, once. Call it before
    /// ``initialize()`` or ``resume(_:)``; after it has completed, later calls
    /// do nothing.
    public func load() async throws {
        switch core.loadPhase {
        case .loaded: return
        case .loading: fatalError("StickShift: Engine.load() was called while the game's load(_:) is running")
        case .notLoaded: break
        }
        if state.isInitialized {
            fatalError("StickShift: Engine.load() was called after the game was initialized; load runs first")
        }
        core.loadPhase = .loading
        do {
            try await core.load(raw)
        } catch {
            core.loadPhase = .notLoaded
            throw error
        }
        core.loadPhase = .loaded
    }

    /// Run the game's ``Game/initialize(_:)``, once, for an engine with no
    /// view (tools, tests): settings queued before it become current, then
    /// the hook runs, then the engine applies the settings to the scene and
    /// the renderer. Later calls do nothing.
    ///
    /// - Throws: what ``Game/initialize(_:)`` threw, which fails the engine;
    ///   ``StickShiftError/Code-swift.struct/wrongState`` once it has failed.
    public func initialize() throws {
        requireNotLoading("Engine.initialize()")
        try rethrowingInit(stickshift_engine_init(raw))
    }

    /// ``load()``, then ``initialize()``.
    public func start() async throws {
        try await load()
        try initialize()
    }

    /// Run the game as Rust's `Headless::run` does: ``initialize()`` if it has
    /// not run, then up to `frames` ticks of one fixed step each, stopping
    /// before a tick once exit has been requested.
    public func runHeadless(frames: Int) throws {
        try initialize()
        for _ in 0..<max(0, frames) {
            if state.exitRequested { break }
            advance()
        }
    }

    /// The view appeared, or reappeared: mark the engine hosted, size the
    /// context's drawable for `view` and the render scale, run the game's
    /// ``Game/initialize(_:)`` if it has not run, and record the safe area. The
    /// first call also sets the camera's aspect to the view's.
    ///
    /// - Returns: ``HostRequests/syncMenu``, plus ``HostRequests/exit`` if
    ///   exit was requested.
    /// - Throws: what ``Game/initialize(_:)`` threw, as ``initialize()`` does,
    ///   and ``StickShiftError/Code-swift.struct/invalidArgument`` for a scale
    ///   factor that is not finite and positive, or an inset that is negative
    ///   or not finite.
    @discardableResult
    public func resume(_ view: ViewMetrics) throws -> HostRequests {
        requireNotLoading("Engine.resume(_:)")
        var desc = try view.c
        var requests: UInt32 = 0
        try rethrowingInit(stickshift_engine_resume(raw, &desc, &requests))
        return HostRequests(rawValue: requests)
    }

    /// Throw for an init (or resume) result: the game's own error when its
    /// hook threw, else the C error.
    private func rethrowingInit(_ result: StickShiftResult) throws {
        if result == STICKSHIFT_ERROR_ABORTED, let error = core.initError {
            throw error
        }
        try check(result)
    }

    private func requireNotLoading(_ what: String) {
        if core.loadPhase == .loading {
            fatalError("StickShift: \(what) was called while the game's load(_:) is running; await Engine.load() first")
        }
    }

    // MARK: - Ticking

    /// Run one display frame: the engine's housekeeping, then (unless
    /// ``Options/holdsUntilLoaded`` holds it) as many ``Game/update(_:dt:)``
    /// calls as `clock` banked fixed steps, at most ``maxStepsPerFrame``,
    /// then ``Game/render(_:into:)`` on a frame the engine opens and submits.
    ///
    /// Ticking before ``initialize()`` or ``resume(_:)``, from inside a hook,
    /// or with a `readback` the context cannot fill is a programmer error and
    /// traps.
    ///
    /// - Parameter readback: Filled with the frame's pixels before this
    ///   returns, if ``TickResult/readbackReady``: offscreen contexts with
    ///   ``FramePacing/wait`` only, and the context's size.
    @discardableResult
    public func tick(_ clock: Clock = .fixedStep, readback: PixelBuffer? = nil) -> TickResult {
        requireNotLoading("Engine.tick(_:readback:)")
        var desc = StickShiftTickDesc()
        desc.size = UInt32(MemoryLayout<StickShiftTickDesc>.size)
        switch clock {
        case .fixedStep:
            desc.clock = StickShiftClock(STICKSHIFT_CLOCK_FIXED_STEP)
        case let .elapsed(nanoseconds):
            desc.clock = StickShiftClock(STICKSHIFT_CLOCK_ELAPSED)
            desc.nanoseconds = nanoseconds
        case let .timestamp(nanoseconds):
            desc.clock = StickShiftClock(STICKSHIFT_CLOCK_TIMESTAMP)
            desc.nanoseconds = nanoseconds
        case .engine:
            desc.clock = StickShiftClock(STICKSHIFT_CLOCK_ENGINE)
        }
        if let readback {
            desc.readback = readback.storage.baseAddress
            desc.readback_len = readback.storage.count
        }
        var out = StickShiftTickResult()
        out.size = UInt32(MemoryLayout<StickShiftTickResult>.size)
        let result = stickshift_engine_tick(raw, &desc, &out)
        if result < 0 {
            misuse("Engine.tick(_:readback:)", result)
        }
        return TickResult(out)
    }

    /// One tick of exactly one fixed step: ``tick(_:readback:)`` with
    /// ``Clock/fixedStep``. What tools, traces and tests drive the game with.
    @discardableResult
    public func advance(readback: PixelBuffer? = nil) -> TickResult {
        tick(.fixedStep, readback: readback)
    }

    /// Apply host events in order: keys, mouse buttons and the cursor go to
    /// the game's input; focus loss releases everything; a resize sizes the
    /// drawable; a menu choice changes the graphics settings at once (and may
    /// run ``Game/graphicsSettingsChanged(_:_:)`` inside this call).
    ///
    /// An event with a value the engine refuses (a scale factor that is not
    /// finite and positive, a negative inset), or a call from inside a hook,
    /// is a programmer error and traps; then none of the events is applied.
    ///
    /// - Returns: What the events ask the host to do.
    @discardableResult
    public func push(_ events: HostEvent...) -> HostRequests {
        push(contentsOf: events)
    }

    /// Apply host events in order, as ``push(_:)`` does.
    @discardableResult
    public func push(contentsOf events: some Sequence<HostEvent>) -> HostRequests {
        let c = events.map(\.c)
        var requests: UInt32 = 0
        let result = c.withUnsafeBufferPointer { events in
            stickshift_engine_push_events(
                raw,
                events.baseAddress,
                UInt32(events.count),
                UInt32(MemoryLayout<StickShiftHostEvent>.stride),
                &requests
            )
        }
        if result < 0 {
            misuse("Engine.push(_:)", result)
        }
        return HostRequests(rawValue: requests)
    }

    /// Make the next ``Clock/timestamp(nanoseconds:)`` or ``Clock/engine``
    /// tick bank exactly one fixed step: for an app returning to the
    /// foreground, or after a modal sheet.
    public func resetClock() {
        handle.call("Engine.resetClock()") { stickshift_engine_reset_clock($0) }
    }

    /// Ask to stop: every later tick reports ``HostRequests/exit``. The
    /// engine never quits by itself.
    public func requestExit() {
        handle.call("Engine.requestExit()") { stickshift_engine_request_exit($0) }
    }

    // MARK: - Inspection

    /// The engine's flags and counters. Readable on a poisoned engine too,
    /// where every other call traps.
    public var state: EngineState {
        var out = StickShiftEngineState()
        out.size = UInt32(MemoryLayout<StickShiftEngineState>.size)
        let result = stickshift_engine_get_state(raw, &out)
        if result == STICKSHIFT_ERROR_INTERNAL {
            // A library that answers nothing on a poisoned engine, this call
            // included: all that is known is that it is poisoned.
            out = StickShiftEngineState()
            out.poisoned = 1
        } else if result < 0 {
            misuse("Engine.state", result)
        }
        return EngineState(out)
    }

    /// The latest frame timing.
    public var perf: PerfSnapshot {
        PerfSnapshot(handle.perf("Engine.perf"))
    }

    /// What the latest ``Game/render(_:into:)`` threw, or `nil` if it
    /// returned normally. The frame it was drawing was submitted all the same.
    public var lastRenderError: (any Error)? {
        core.lastRenderError
    }

    /// The engine's own view handle, valid while the engine lives.
    var handle: EngineHandle {
        EngineHandle(raw: raw, core: core, scope: nil, owner: self)
    }
}

/// An engine being dropped: destroyed on the main thread, then its context dropped.
private final class DoomedEngine: @unchecked Sendable {
    // Touched only on the main thread: in `Engine.deinit` there, or in the
    // block it sends there.
    let raw: OpaquePointer
    /// Held until the C engine is gone, so the context it is attached to dies
    /// after it, and (if this was the last reference) on the main thread.
    private var context: Context?

    init(raw: OpaquePointer, context: Context) {
        self.raw = raw
        self.context = context
    }

    func destroy() {
        let result = stickshift_engine_destroy(raw)
        assert(result == STICKSHIFT_OK, "stickshift_engine_destroy returned \(result)")
        context = nil
    }
}

extension Engine.Options {
    /// The C descriptor for these options, without the settings suite, whose
    /// C string ``Engine/init(context:game:options:)`` lends for the create call alone.
    var c: StickShiftEngineDesc {
        var desc = StickShiftEngineDesc()
        desc.api_version = UInt32(STICKSHIFT_API_VERSION)
        desc.size = UInt32(MemoryLayout<StickShiftEngineDesc>.size)
        desc.audio = StickShiftAudioOutput(audio == .device ? STICKSHIFT_AUDIO_DEVICE : STICKSHIFT_AUDIO_SILENT)
        desc.pacing = StickShiftFramePacing(
            framePacing == .skip ? STICKSHIFT_FRAME_PACING_SKIP : STICKSHIFT_FRAME_PACING_WAIT
        )
        if persistsSettings { desc.flags |= UInt32(STICKSHIFT_ENGINE_PERSIST_SETTINGS) }
        if quitsOnEscape { desc.flags |= UInt32(STICKSHIFT_ENGINE_QUIT_ON_ESCAPE) }
        if holdsUntilLoaded { desc.flags |= UInt32(STICKSHIFT_ENGINE_HOLD_UNTIL_LOADED) }
        return desc
    }
}

/// What one tick did.
public struct TickResult: Sendable, Equatable {
    /// How many times ``Game/update(_:dt:)`` ran.
    public var steps: Int
    /// Whether ``Game/render(_:into:)`` ran.
    public var rendered: Bool
    /// Whether the frame went to the GPU; `false` if it was discarded (with
    /// ``Engine/FramePacing/skip`` and no free slot) or had no drawable.
    public var submitted: Bool
    /// Whether the tick's read-back buffer holds this frame.
    public var readbackReady: Bool
    /// Whether ``Engine/Options/holdsUntilLoaded`` held this tick: nothing ran.
    public var held: Bool
    /// What the engine asks the host to do.
    public var requests: HostRequests
    /// The submitted frame's number, or `nil`.
    public var frame: UInt64?
    /// The graphics settings' revision after the tick: it grows by one with
    /// every change.
    public var graphicsRevision: UInt64

    init(_ r: StickShiftTickResult) {
        steps = Int(r.steps)
        rendered = r.rendered != 0
        submitted = r.submitted != 0
        readbackReady = r.readback_ready != 0
        held = r.held != 0
        requests = HostRequests(rawValue: r.requests)
        frame = r.frame == 0 ? nil : r.frame
        graphicsRevision = r.graphics_revision
    }
}

/// What the engine asks its host to do, after a tick or an event.
public struct HostRequests: OptionSet, Sendable, Hashable {
    /// The raw `STICKSHIFT_REQUEST_*` bits.
    public let rawValue: UInt32

    /// Creates requests from raw bits.
    public init(rawValue: UInt32) {
        self.rawValue = rawValue
    }

    /// Close the view and stop ticking: the view was closed, Escape was
    /// pressed with ``Engine/Options/quitsOnEscape``, or the game asked to exit.
    public static let exit = HostRequests(rawValue: UInt32(STICKSHIFT_REQUEST_EXIT))
    /// Enter or leave full screen: the menu's full-screen item was chosen.
    public static let toggleFullScreen = HostRequests(rawValue: UInt32(STICKSHIFT_REQUEST_TOGGLE_FULLSCREEN))
    /// The graphics settings or the full-screen state may have changed:
    /// refresh the host's Graphics menu.
    public static let syncMenu = HostRequests(rawValue: UInt32(STICKSHIFT_REQUEST_MENU_SYNC))
}

/// An engine's flags and counters, from ``Engine/state``.
public struct EngineState: Sendable, Equatable {
    /// The game's ``Game/initialize(_:)`` has run.
    public var isInitialized: Bool
    /// ``Engine/resume(_:)`` has run.
    public var isHosted: Bool
    /// The game's ``Game/initialize(_:)`` threw: the engine can only be dropped.
    public var hasFailed: Bool
    /// A panic (a bug in the engine) was caught in an engine call: every
    /// later call on it but ``Engine/state`` traps with the engine's message,
    /// and only dropping it works.
    public var isPoisoned: Bool
    /// The game, the host or an event asked to exit.
    public var exitRequested: Bool
    /// The last tick was held by ``Engine/Options/holdsUntilLoaded``.
    public var isHolding: Bool
    /// Ticks that ran a frame.
    public var frames: UInt64
    /// ``Game/update(_:dt:)`` calls.
    public var updates: UInt64
    /// ``Game/render(_:into:)`` calls.
    public var framesRendered: UInt64
    /// Frames rendered but not submitted.
    public var framesDiscarded: UInt64

    init(_ s: StickShiftEngineState) {
        isInitialized = s.initialized != 0
        isHosted = s.hosted != 0
        hasFailed = s.failed != 0
        isPoisoned = s.poisoned != 0
        exitRequested = s.exit_requested != 0
        isHolding = s.holding != 0
        frames = s.frames
        updates = s.updates
        framesRendered = s.frames_rendered
        framesDiscarded = s.frames_discarded
    }
}

/// Frame timing, from ``Engine/perf`` or ``EngineContext/perf``.
public struct PerfSnapshot: Sendable, Equatable {
    /// Frames measured.
    public var frameCount: UInt64
    /// Frames per second over the last frame.
    public var currentFPS: Double
    /// Frames per second over the measuring window.
    public var averageFPS: Double
    /// The mean frame time.
    public var averageFrameTime: Duration
    /// The shortest frame time.
    public var minFrameTime: Duration
    /// The longest frame time.
    public var maxFrameTime: Duration
    /// The mean CPU time per frame.
    public var averageCPUTime: Duration
    /// The mean GPU time per frame, where the backend measures it.
    public var averageGPUTime: Duration
    /// The mean time in ``Game/render(_:into:)``.
    public var averageRenderTime: Duration

    init(_ p: StickShiftPerf) {
        func duration(_ nanoseconds: UInt64) -> Duration {
            .nanoseconds(Int64(clamping: nanoseconds))
        }
        frameCount = p.frame_count
        currentFPS = p.current_fps
        averageFPS = p.avg_fps
        averageFrameTime = duration(p.avg_frame_ns)
        minFrameTime = duration(p.min_frame_ns)
        maxFrameTime = duration(p.max_frame_ns)
        averageCPUTime = duration(p.avg_cpu_ns)
        averageGPUTime = duration(p.avg_gpu_ns)
        averageRenderTime = duration(p.avg_render_ns)
    }
}

/// A buffer a tick reads its frame's pixels back into: BGRA8, `width * 4`
/// bytes per row, top row first. See ``Engine/tick(_:readback:)``.
public final class PixelBuffer {
    /// Width, in pixels.
    public let width: Int
    /// Height, in pixels.
    public let height: Int
    let storage: UnsafeMutableRawBufferPointer

    /// Creates a zeroed buffer for `width` × `height` pixels: the size of the
    /// context it reads back from.
    public init(width: Int, height: Int) {
        precondition(width >= 0 && height >= 0, "a pixel buffer's width and height must not be negative")
        self.width = width
        self.height = height
        storage = .allocate(byteCount: width * height * 4, alignment: 16)
        storage.initializeMemory(as: UInt8.self, repeating: 0)
    }

    deinit {
        storage.deallocate()
    }

    /// The pixels as the engine wrote them: BGRA8, top row first.
    public var bgra: [UInt8] {
        Array(storage)
    }

    /// The pixels with red and blue swapped: RGBA8, top row first, as image
    /// encoders want them.
    public func rgba() -> [UInt8] {
        var out = Array(storage)
        var i = 0
        while i + 3 < out.count {
            out.swapAt(i, i + 2)
            i += 4
        }
        return out
    }
}
