// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import CStickShift

/// A game the ``Engine`` runs: Rust's `Game` trait, hook for hook.
///
/// The engine calls the hooks from inside its own calls, on the main actor:
/// ``initialize(_:)`` once, from ``Engine/initialize()`` or ``Engine/resume(_:)``;
/// then, on every ``Engine/tick(_:readback:)``, ``update(_:dt:)`` zero or more
/// times (one per fixed step the tick's clock banked, at most
/// ``Engine/maxStepsPerFrame``) and ``render(_:into:)`` exactly once.
///
/// Each hook gets an ``EngineContext``: the engine as that hook may use it. It
/// is valid only until the hook returns. Keep game state in the game object,
/// never the context.
///
/// ```swift
/// @MainActor
/// final class Spinner: Game {
///     var angle: Float = 0
///
///     func initialize(_ ctx: EngineContext) throws {
///         try ctx.renderer.setSunSky(.default)
///     }
///
///     func update(_ ctx: EngineContext, dt: Float) {
///         angle += dt
///     }
///
///     func render(_ ctx: EngineContext, into frame: Frame) throws {
///         try frame.draw(Quad(rect: [10, 10, 40 + 20 * angle, 20], color: [1, 0.5, 0, 1]))
///     }
/// }
/// ```
@MainActor
public protocol Game: AnyObject {
    /// Create GPU resources asynchronously, before ``initialize(_:)``: an
    /// optional phase for Swift games that keeps an app's launch responsive.
    /// ``Engine/load()`` runs it. It must not touch game state that
    /// ``initialize(_:)`` sets up. A port of a Rust game leaves it out and loads
    /// synchronously in ``initialize(_:)`` instead, with
    /// ``Context/startLoadingModel(contentsOf:)`` and ``Model/wait(timeout:)``.
    /// The default does nothing.
    func load(_ ctx: LoadContext) async throws

    /// Set up the game: Rust's `Game::init`. Called exactly once, before the
    /// first ``update(_:dt:)``.
    ///
    /// A thrown error fails the engine: ``Engine/initialize()`` (or
    /// ``Engine/resume(_:)``) rethrows it, and the engine can only be dropped.
    func initialize(_ ctx: EngineContext) throws

    /// Advance the game by one fixed step: Rust's `Game::update`. Called zero
    /// or more times per tick, always before that tick's ``render(_:into:)``.
    /// `dt` is always ``Engine/fixedDeltaTime``.
    func update(_ ctx: EngineContext, dt: Float)

    /// Draw the frame the engine opened: Rust's `Game::render`. Called exactly
    /// once per tick, after its updates. Don't change game state here.
    ///
    /// The engine submits the frame when this returns. If it throws, the frame
    /// is still submitted (with whatever was recorded) and the error is kept as
    /// ``Engine/lastRenderError``.
    func render(_ ctx: EngineContext, into frame: Frame) throws

    /// New graphics settings took effect: Rust's `Game::graphics_settings_changed`.
    /// The engine has already applied them to the renderer. Called between
    /// frames, never for the settings in effect at start-up. The default does
    /// nothing.
    func graphicsSettingsChanged(_ ctx: EngineContext, _ settings: GraphicsSettings)
}

extension Game {
    /// Does nothing.
    public func load(_ ctx: LoadContext) async throws {}

    /// Does nothing.
    public func graphicsSettingsChanged(_ ctx: EngineContext, _ settings: GraphicsSettings) {}
}

/// The engine as one of a ``Game``'s hooks may use it: Rust's `EngineCtx`.
///
/// A context is valid only during the hook it was passed to. Using it after
/// that hook returns is a programmer error and traps, with a message naming
/// the hook. It is a plain value, so helpers can take it as a parameter.
@MainActor
public struct EngineContext {
    let handle: EngineHandle

    init(core: EngineCore, raw: OpaquePointer, scope: HookScope) {
        handle = EngineHandle(raw: raw, core: core, scope: scope, owner: nil)
    }

    /// The context the engine renders through: make meshes, textures, models
    /// and fonts with it. Rust's `EngineCtx::renderer`: if the game moved the
    /// camera in this hook, the first call on it pushes the camera first.
    public var renderer: Context {
        handle.check("EngineContext.renderer")
        return handle.core.context
    }

    /// How far the time banked since the last fixed step is toward the next
    /// one, from 0 to 1: in ``Game/render(_:into:)``, for drawing between two
    /// fixed states. 0 in every other hook.
    public var alpha: Float {
        handle.perf("EngineContext.alpha").alpha
    }

    /// The frame timing this hook's frame started with.
    public var perf: PerfSnapshot {
        PerfSnapshot(handle.perf("EngineContext.perf"))
    }

    /// Ask the host to stop: every later tick reports
    /// ``HostRequests/exit``. The engine itself never quits.
    public func requestExit() {
        handle.call("EngineContext.requestExit()") { stickshift_engine_request_exit($0) }
    }

    /// Whether the game, the host or an event asked to exit.
    public var exitRequested: Bool {
        handle.state("EngineContext.exitRequested").exit_requested != 0
    }
}

/// The engine as ``Game/load(_:)`` may use it: the renderer, for creating
/// resources. Valid only until `load` returns.
@MainActor
public struct LoadContext {
    /// Unowned, so a context a game keeps by mistake keeps nothing alive; the
    /// scope check traps before any use of it.
    unowned let core: EngineCore
    let scope: HookScope

    /// The context the engine renders through.
    public var renderer: Context {
        scope.check("LoadContext.renderer")
        return core.context
    }
}

// MARK: - Scopes

/// One hook's run. An ``EngineContext`` or ``LoadContext`` carries the scope of
/// the hook it was passed to, and checks it before every use.
@MainActor
final class HookScope {
    /// Which hook.
    enum Kind: String, Sendable {
        case load = "load(_:)"
        case initialize = "initialize(_:)"
        case update = "update(_:dt:)"
        case render = "render(_:into:)"
        case graphicsSettingsChanged = "graphicsSettingsChanged(_:_:)"
    }

    let kind: Kind
    private(set) var isLive = true

    init(_ kind: Kind) {
        self.kind = kind
    }

    /// The hook returned.
    func end() {
        isLive = false
    }

    /// Trap unless the hook is still running.
    func check(_ what: @autoclosure () -> String) {
        guard isLive else {
            fatalError(
                "StickShift: \(what()) was used after the game's \(kind.rawValue) hook returned. "
                    + "A context is valid only during the hook it is passed to; don't keep it."
            )
        }
    }
}

/// How a view of the engine reaches it: the C engine, the state its hooks
/// share, and the hook scope the view is valid in (`nil` for the ``Engine``'s
/// own views, which are valid while the engine lives). Every service view
/// (`World`, `Scene`, ...) is built on one.
@MainActor
struct EngineHandle {
    let raw: OpaquePointer
    /// Unowned, so a context a game keeps by mistake keeps nothing alive; the
    /// scope check traps before any use of it.
    unowned let core: EngineCore
    let scope: HookScope?
    /// Keeps the engine, and so `raw`, alive: the ``Engine`` for its own views.
    /// A hook's views need none, since their scope ends before the engine can.
    let owner: AnyObject?

    /// Trap if the view's hook has returned.
    func check(_ what: @autoclosure () -> String) {
        scope?.check(what())
    }

    /// Make the C call `body` for `what`, trapping if the view's hook has
    /// returned or the call fails: misuse is a programmer error, as with
    /// Swift's collections. Returns a result that is not an error.
    @discardableResult
    func call(_ what: @autoclosure () -> String, _ body: (OpaquePointer) -> StickShiftResult) -> StickShiftResult {
        check(what())
        let result = body(raw)
        if result < 0 {
            misuse(what(), result)
        }
        return result
    }

    /// The engine's flags and counters.
    func state(_ what: @autoclosure () -> String) -> StickShiftEngineState {
        var out = StickShiftEngineState()
        out.size = UInt32(MemoryLayout<StickShiftEngineState>.size)
        call(what()) { stickshift_engine_get_state($0, &out) }
        return out
    }

    /// The frame timing and alpha, as the hook (or the engine between calls)
    /// sees them.
    func perf(_ what: @autoclosure () -> String) -> StickShiftPerf {
        var out = StickShiftPerf()
        out.size = UInt32(MemoryLayout<StickShiftPerf>.size)
        call(what()) { stickshift_engine_get_perf($0, &out) }
        return out
    }
}

/// Trap for a C call that failed through misuse, with the engine's message.
func misuse(_ what: String, _ result: StickShiftResult) -> Never {
    let error = StickShiftError(
        code: StickShiftError.Code(rawValue: result),
        message: String(cString: stickshift_last_error_message())
    )
    fatalError("StickShift: \(what) failed: \(error)")
}

// MARK: - The game box and the hooks

/// What an ``Engine`` and its C hooks share: the game, the context, and what
/// the hooks leave for the engine to report. Retained for C as the game's
/// `user` pointer at create, and released by the `release` hook.
@MainActor
final class EngineCore {
    let game: any Game
    let context: Context
    /// What ``Game/initialize(_:)`` threw, for ``Engine/initialize()`` to rethrow.
    var initError: (any Error)?
    /// What the latest ``Game/render(_:into:)`` threw.
    var lastRenderError: (any Error)?
    /// Where ``Engine/load()`` is.
    var loadPhase: LoadPhase = .notLoaded

    enum LoadPhase {
        case notLoaded, loading, loaded
    }

    init(game: any Game, context: Context) {
        self.game = game
        self.context = context
    }

    /// The C game: these hooks, with this box as `user`, retained. The
    /// `release` hook (or a failed create) balances the retain.
    func retainedHooks() -> StickShiftGame {
        var hooks = StickShiftGame()
        hooks.size = UInt32(MemoryLayout<StickShiftGame>.size)
        hooks.user = Unmanaged.passRetained(self).toOpaque()
        hooks.`init` = initHook
        hooks.update = updateHook
        hooks.render = renderHook
        hooks.graphics_settings_changed = settingsHook
        hooks.release = releaseHook
        return hooks
    }

    /// Run `body` with a context for a hook of `kind`, which is dead once it returns.
    private func run<T>(_ kind: HookScope.Kind, _ raw: OpaquePointer, _ body: (EngineContext) throws -> T) rethrows -> T {
        let scope = HookScope(kind)
        defer { scope.end() }
        return try body(EngineContext(core: self, raw: raw, scope: scope))
    }

    func initialize(_ raw: OpaquePointer) -> StickShiftResult {
        do {
            try run(.initialize, raw) { try game.initialize($0) }
            return STICKSHIFT_OK
        } catch {
            initError = error
            return STICKSHIFT_ERROR_ABORTED
        }
    }

    func update(_ raw: OpaquePointer, dt: Float) {
        run(.update, raw) { game.update($0, dt: dt) }
    }

    func render(_ raw: OpaquePointer, frame: OpaquePointer) {
        lastRenderError = nil
        do {
            try run(.render, raw) { try game.render($0, into: Frame(raw: frame, context: context)) }
        } catch {
            lastRenderError = error
        }
    }

    func graphicsSettingsChanged(_ raw: OpaquePointer, _ settings: GraphicsSettings) {
        run(.graphicsSettingsChanged, raw) { game.graphicsSettingsChanged($0, settings) }
    }

    /// Run ``Game/load(_:)`` with a context that is dead once it returns.
    func load() async throws {
        let scope = HookScope(.load)
        defer { scope.end() }
        try await game.load(LoadContext(core: self, scope: scope))
    }
}

/// Where the engine's hooks run, as Swift sees it.
///
/// The C hooks run on the thread that created the engine, inside the engine
/// call that runs them. ``Engine`` is created on the main actor, so every hook
/// is already on it; this is the one place that says so. A host that runs the
/// engine on a render thread (visionOS) changes only this.
enum EngineIsolation {
    /// Run `body` on the main actor, which the calling thread is already on.
    static func assume<T: Sendable>(_ body: @MainActor () throws -> T) rethrows -> T {
        try MainActor.assumeIsolated(body)
    }
}

/// The box behind a hook's `user` pointer, not retained.
private func core(_ user: UnsafeMutableRawPointer?) -> Unmanaged<EngineCore> {
    guard let user else { fatalError("StickShift: a game hook was called without its user pointer") }
    return Unmanaged<EngineCore>.fromOpaque(user)
}

/// A C pointer a hook was called with, carried into `EngineIsolation.assume`,
/// which uses it on the same thread before the hook returns.
private struct HookPointer: @unchecked Sendable {
    let raw: OpaquePointer

    init(_ raw: OpaquePointer?, _ what: StaticString) {
        guard let raw else { fatalError("StickShift: a game hook was called without its \(what)") }
        self.raw = raw
    }
}

private let initHook: StickShiftGameInitFn = { user, raw in
    let box = core(user)
    let engine = HookPointer(raw, "engine")
    return EngineIsolation.assume { box.takeUnretainedValue().initialize(engine.raw) }
}

private let updateHook: StickShiftGameUpdateFn = { user, raw, dt in
    let box = core(user)
    let engine = HookPointer(raw, "engine")
    EngineIsolation.assume { box.takeUnretainedValue().update(engine.raw, dt: dt) }
}

private let renderHook: StickShiftGameRenderFn = { user, raw, frame in
    let box = core(user)
    let engine = HookPointer(raw, "engine")
    let frame = HookPointer(frame, "frame")
    EngineIsolation.assume { box.takeUnretainedValue().render(engine.raw, frame: frame.raw) }
}

private let settingsHook: StickShiftGameSettingsFn = { user, raw, settings in
    let box = core(user)
    let engine = HookPointer(raw, "engine")
    guard let settings else { fatalError("StickShift: a game hook was called without its settings") }
    let value = GraphicsSettings(settings.pointee)
    EngineIsolation.assume { box.takeUnretainedValue().graphicsSettingsChanged(engine.raw, value) }
}

private let releaseHook: StickShiftGameReleaseFn = { user in
    core(user).release()
}
