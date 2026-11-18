// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import CStickShift

/// How a ``Scene`` streams: the engine's `StreamingConfig`.
///
/// Slots lie every ``spawnInterval`` along ``axis``. A stream pass fills the
/// slots up to ``spawnAhead`` ahead of the focus and despawns what it streamed
/// once it is ``despawnBehind`` behind. A focus that travels past
/// ``rebaseThreshold`` from the origin triggers a floating-origin rebase.
public struct StreamingConfig: Sendable, Equatable {
    /// The travel axis.
    public var axis: SIMD3<Float>
    /// The distance between slots along the axis.
    public var spawnInterval: Float
    /// How far ahead of the focus slots are filled. After init the engine
    /// scales it with the draw distance.
    public var spawnAhead: Float
    /// How far behind the focus streamed entities are despawned.
    public var despawnBehind: Float
    /// How far the focus may travel before the origin is rebased.
    public var rebaseThreshold: Float

    /// Creates a configuration; each value defaults to the engine's default.
    public init(
        axis: SIMD3<Float> = [0, 0, -1],
        spawnInterval: Float = 6,
        spawnAhead: Float = 60,
        despawnBehind: Float = 12,
        rebaseThreshold: Float = 1000
    ) {
        self.axis = axis
        self.spawnInterval = spawnInterval
        self.spawnAhead = spawnAhead
        self.despawnBehind = despawnBehind
        self.rebaseThreshold = rebaseThreshold
    }

    /// The engine's default: along `-Z`, a slot every 6 units, 60 ahead, 12
    /// behind, rebased past 1000.
    public static var `default`: StreamingConfig {
        var c = StickShiftStreamingConfig()
        stickshift_streaming_config_default(&c)
        return StreamingConfig(c)
    }

    init(_ c: StickShiftStreamingConfig) {
        axis = SIMD3(c.axis)
        spawnInterval = c.spawn_interval
        spawnAhead = c.spawn_ahead
        despawnBehind = c.despawn_behind
        rebaseThreshold = c.rebase_threshold
    }

    var c: StickShiftStreamingConfig {
        StickShiftStreamingConfig(
            axis: StickShiftVec3(axis),
            spawn_interval: spawnInterval,
            spawn_ahead: spawnAhead,
            despawn_behind: despawnBehind,
            rebase_threshold: rebaseThreshold
        )
    }
}

/// The engine's scene: streaming along an axis, and the floating origin.
///
/// Reach it from a hook's ``EngineContext/scene`` or, between ticks, from
/// ``Engine/scene``. Streaming, rebasing and configuring are not allowed in
/// ``Game/render(_:into:)``.
@MainActor
public struct Scene {
    let handle: EngineHandle

    /// The streaming configuration. After init, ``StreamingConfig/spawnAhead``
    /// is the one the engine scaled for the draw distance.
    public var config: StreamingConfig {
        var out = StickShiftStreamingConfig()
        handle.call("Scene.config") { stickshift_scene_get_config($0, &out) }
        return StreamingConfig(out)
    }

    /// Replace the streaming configuration.
    ///
    /// - Throws: ``StickShiftError/Code-swift.struct/wrongState``, changing
    ///   nothing, for a new ``StreamingConfig/axis`` or
    ///   ``StreamingConfig/spawnInterval`` once streaming has started (and in
    ///   ``Game/render(_:into:)``).
    public func setConfig(_ config: StreamingConfig) throws {
        handle.check("Scene.setConfig(_:)")
        var c = config.c
        try check(stickshift_scene_set_config(handle.raw, &c))
    }

    /// Run one streaming pass for `focus`: despawn the streamed entities
    /// fallen behind (in the order they were spawned), then call `fill` once
    /// per new slot, in slot order. The ``StreamSlot`` it gets spawns into
    /// the world and tracks what it spawns for despawning later.
    ///
    /// Inside `fill`, the world is the only engine service it may use:
    /// spawning, reading and changing it. The scene's other calls (another
    /// pass, a rebase), the camera, input, audio and graphics trap, and
    /// ``setConfig(_:)`` throws ``StickShiftError/Code-swift.struct/wrongState``.
    /// The ``Context``'s calls (making or dropping a mesh, a texture, a model)
    /// are allowed.
    public func stream(focus: SIMD3<Float>, _ fill: (borrowing StreamSlot) -> Void) {
        handle.check("Scene.stream(focus:_:)")
        withoutActuallyEscaping(fill) { fill in
            let pass = StreamPass(fill: fill, handle: handle)
            let result = withExtendedLifetime(pass) {
                stickshift_scene_stream(
                    handle.raw,
                    StickShiftVec3(focus),
                    streamSpawn,
                    Unmanaged.passUnretained(pass).toOpaque()
                )
            }
            if result < 0 {
                misuse("Scene.stream(focus:_:)", result)
            }
        }
    }

    /// Rebase the floating origin if `focus` has drifted past
    /// ``StreamingConfig/rebaseThreshold``: every ``Transform`` in the world
    /// shifts by the same offset. What the game keeps in world coordinates
    /// outside the world (the camera, its own state) is the game's to shift.
    ///
    /// - Returns: The offset every transform moved by, or zero if no rebase
    ///   was due.
    @discardableResult
    public func maybeRebase(focus: SIMD3<Float>) -> SIMD3<Float> {
        var offset = StickShiftVec3()
        handle.call("Scene.maybeRebase(focus:)") {
            stickshift_scene_maybe_rebase($0, StickShiftVec3(focus), &offset)
        }
        return SIMD3(offset)
    }

    /// How many streamed entities the scene tracks for despawning behind.
    public var streamedCount: Int {
        var out: UInt32 = 0
        handle.call("Scene.streamedCount") { stickshift_scene_streamed_count($0, &out) }
        return Int(out)
    }

    /// Despawn `entity`: ``World/despawn(_:)``, as Rust's `Scene::despawn` is.
    @discardableResult
    public func despawn(_ entity: Entity) -> Bool {
        World(handle: handle).despawn(entity)
    }
}

/// One slot a ``Scene/stream(focus:_:)`` pass is filling.
///
/// The closure borrows it for the slot's turn: it cannot be kept, and the
/// ``world`` it hands out traps if used after the closure returns.
@MainActor
public struct StreamSlot: ~Copyable {
    /// The slot's number along the axis: 0 at the origin, negative behind it.
    public let index: Int64
    /// The slot's world position.
    public let position: SIMD3<Float>
    let handle: EngineHandle

    /// The world the pass spawns into.
    public var world: World {
        World(handle: handle)
    }

    /// Spawn an entity with these components and track it, so the pass that
    /// finds it behind despawns it: ``World/spawn(_:)`` and then
    /// ``track(_:)``, Rust's `cx.world.spawn(..)` and `cx.spawned(e)`.
    @discardableResult
    public func spawn<each C: Component>(_ component: repeat each C) -> Entity {
        let entity = world.spawn(repeat each component)
        track(entity)
        return entity
    }

    /// Track `entity` for despawning behind: by its ``Transform`` along the
    /// axis, or at this slot's distance if it has none.
    public func track(_ entity: Entity) {
        handle.call("StreamSlot.track(_:)") { stickshift_stream_track($0, entity.c) }
    }
}

/// One stream pass's closure, behind the C callback's `user` pointer.
@MainActor
private final class StreamPass {
    let fill: (borrowing StreamSlot) -> Void
    let handle: EngineHandle

    init(fill: @escaping (borrowing StreamSlot) -> Void, handle: EngineHandle) {
        self.fill = fill
        self.handle = handle
    }

    func run(slot: Int64, position: StickShiftVec3) {
        let scope = HookScope(.streamFill)
        defer { scope.end() }
        let handle = EngineHandle(raw: handle.raw, core: handle.core, scope: scope, owner: handle.owner)
        fill(StreamSlot(index: slot, position: SIMD3(position), handle: handle))
    }
}

private let streamSpawn: StickShiftSpawnFn = { user, _, slot, position in
    guard let user else { fatalError("StickShift: a stream pass called back without its user pointer") }
    let pass = Unmanaged<StreamPass>.fromOpaque(user)
    EngineIsolation.assume { pass.takeUnretainedValue().run(slot: slot, position: position) }
}
