// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import CStickShift

/// The engine's world: its entities and their ``Component``s, in the
/// engine's own ECS (hecs), so archetypes, ids and iteration order are the
/// ones a Rust game gets.
///
/// Reach it from a hook's ``EngineContext/world`` or, between ticks, from
/// ``Engine/world``. Changing the world is not allowed in
/// ``Game/render(_:into:)``; reading it is.
///
/// ```swift
/// let car = ctx.world.spawn(Transform(position: [0, 0.5, 0]), RenderComponent.cube([1, 0, 0]), DynamicTag())
/// for (entity, transform) in ctx.world.query(Transform.self).with(Obstacle.self) {
///     if transform.position.z > 10 { ctx.world.despawn(entity) }
/// }
/// ```
///
/// Misuse is a programmer error and traps, with the engine's message: a
/// change in `render`, a view kept past its hook, a component of more than
/// 4096 bytes. An entity that is gone is not misuse: ``get(_:of:)`` returns
/// `nil` and ``despawn(_:)`` returns `false`.
@MainActor
public struct World {
    let handle: EngineHandle

    func info<C: Component>(_: C.Type) -> ComponentInfo {
        handle.core.components.info(C.self, handle.raw)
    }

    /// Spawn an entity with these components, in one spawn: it lands in the
    /// archetype a Rust tuple of the same set of components does, whatever
    /// their order. At most 32 components, each at most once.
    @discardableResult
    public func spawn<each C: Component>(_ component: repeat each C) -> Entity {
        handle.check("World.spawn(_:)")
        var bundle = Bundle(world: self)
        repeat bundle.add(each component)
        var out = StickShiftEntity()
        bundle.withValues { values, count in
            _ = handle.call("World.spawn(_:)") { stickshift_world_spawn($0, values, count, &out) }
        }
        return Entity(bits: out.bits)
    }

    /// Despawn `entity`, freeing its id for reuse with a bumped generation.
    ///
    /// - Returns: `false` if it was already gone.
    @discardableResult
    public func despawn(_ entity: Entity) -> Bool {
        var existed: UInt8 = 0
        handle.call("World.despawn(_:)") { stickshift_world_despawn($0, entity.c, &existed) }
        return existed != 0
    }

    /// Whether `entity` is alive.
    public func contains(_ entity: Entity) -> Bool {
        var out: UInt8 = 0
        handle.call("World.contains(_:)") { stickshift_world_contains($0, entity.c, &out) }
        return out != 0
    }

    /// Whether `entity` has a `C`; `false` if it is gone.
    public func has<C: Component>(_ type: C.Type, _ entity: Entity) -> Bool {
        handle.check("World.has(_:_:)")
        let component = info(C.self).id
        var out: UInt8 = 0
        handle.call("World.has(_:_:)") { stickshift_world_has($0, entity.c, component, &out) }
        return out != 0
    }

    /// `entity`'s `C`, copied out; `nil` if it is gone or has none. A tag
    /// reads as its empty value.
    public func get<C: Component>(_ type: C.Type, of entity: Entity) -> C? {
        handle.check("World.get(_:of:)")
        let info = info(C.self)
        if info.isTag {
            return has(C.self, entity) ? info.decode(C.self, from: nil) : nil
        }
        let size = info.size
        return withUnsafeTemporaryAllocation(byteCount: size, alignment: 16) { buffer in
            let result = handle.call("World.get(_:of:)") {
                stickshift_world_get($0, entity.c, info.id, buffer.baseAddress, UInt32(size))
            }
            return result == STICKSHIFT_ABSENT ? nil : info.decode(C.self, from: buffer.baseAddress)
        }
    }

    /// Overwrite `entity`'s `C` in place; the entity does not move. For a
    /// tag, nothing changes.
    ///
    /// - Returns: `false` if `entity` is gone or has no `C`.
    @discardableResult
    public func set<C: Component>(_ value: C, for entity: Entity) -> Bool {
        handle.check("World.set(_:for:)")
        let info = info(C.self)
        if info.isTag {
            return has(C.self, entity)
        }
        var bytes: [UInt8] = []
        let start = info.encode(value, into: &bytes)
        let result = bytes.withUnsafeBytes { raw in
            handle.call("World.set(_:for:)") {
                stickshift_world_set($0, entity.c, info.id, raw.baseAddress! + start, UInt32(info.size))
            }
        }
        return result != STICKSHIFT_ABSENT
    }

    /// Change `entity`'s `C` in place: ``get(_:of:)``, `body`, then
    /// ``set(_:for:)``.
    ///
    /// - Returns: `false`, without calling `body`, if `entity` is gone or
    ///   has no `C`.
    @discardableResult
    public func modify<C: Component>(_ type: C.Type, of entity: Entity, _ body: (inout C) -> Void) -> Bool {
        guard var value = get(C.self, of: entity) else { return false }
        body(&value)
        return set(value, for: entity)
    }

    /// Add these components to `entity`, replacing the values of any it
    /// already has, in one insert. Unless its set of components stays the
    /// same, the entity moves to the end of its new archetype, as in Rust.
    ///
    /// - Returns: `false` if `entity` is gone.
    @discardableResult
    public func insert<each C: Component>(_ component: repeat each C, into entity: Entity) -> Bool {
        handle.check("World.insert(_:into:)")
        var bundle = Bundle(world: self)
        repeat bundle.add(each component)
        let result = bundle.withValues { values, count in
            handle.call("World.insert(_:into:)") { stickshift_world_insert($0, entity.c, values, count) }
        }
        return result != STICKSHIFT_ABSENT
    }

    /// Remove `entity`'s `C`, which moves it to another archetype, as in Rust.
    ///
    /// - Returns: `false` if `entity` is gone or has no `C`.
    @discardableResult
    public func remove<C: Component>(_ type: C.Type, from entity: Entity) -> Bool {
        handle.check("World.remove(_:from:)")
        let component = info(C.self).id
        let result = handle.call("World.remove(_:from:)") { stickshift_world_remove($0, entity.c, component) }
        return result != STICKSHIFT_ABSENT
    }

    /// The number of live entities.
    public var count: Int {
        var out: UInt32 = 0
        handle.call("World.count") { stickshift_world_len($0, &out) }
        return Int(out)
    }

    /// The entities that have every component `type` names, in the engine's
    /// iteration order (hecs's: archetypes in the order they were created,
    /// then rows in order), each with those components. An `Optional`
    /// component (`RenderComponent?.self`) does not filter: it is `nil` for
    /// the entities that lack it. ``Query/with(_:)`` and
    /// ``Query/without(_:)`` filter further.
    ///
    /// Nothing runs until the query is iterated (or counted). Iterating copies
    /// every row out first, so the loop body may change the world.
    public func query<each T: QueryTerm>(_ type: repeat (each T).Type) -> Query<repeat each T> {
        Query(world: self)
    }
}

/// One spawn's or insert's components, as C reads them.
@MainActor
private struct Bundle {
    let world: World
    var bytes: [UInt8] = []
    var parts: [(component: StickShiftComponent, start: Int, size: Int)] = []

    init(world: World) {
        self.world = world
    }

    mutating func add<C: Component>(_ value: C) {
        let info = world.info(C.self)
        let start = info.encode(value, into: &bytes)
        parts.append((info.id, start, info.size))
    }

    func withValues<R>(_ body: (UnsafePointer<StickShiftComponentValue>?, UInt32) -> R) -> R {
        bytes.withUnsafeBytes { raw in
            let values = parts.map { part in
                StickShiftComponentValue(
                    component: part.component,
                    data: part.size == 0 ? nil : raw.baseAddress! + part.start,
                    size: UInt32(part.size)
                )
            }
            return values.withUnsafeBufferPointer { body($0.baseAddress, UInt32($0.count)) }
        }
    }
}

/// The entities of a ``World/query(_:)``, each with the components it asked
/// for, in the engine's iteration order.
///
/// Iterating runs the query once and copies its rows out before the first
/// element, so the loop body may spawn, despawn and change components, and
/// may run other queries. A query can be iterated again, which runs it again.
/// Like the world, it is used on the main actor: iterating elsewhere traps.
public struct Query<each T: QueryTerm>: Sequence {
    /// An entity and the components the query asked for.
    public typealias Element = (Entity, repeat each T)

    /// The world and the filters. Only touched on the main actor, which
    /// `makeIterator()` (nonisolated, as `Sequence` asks) checks first.
    private struct State: @unchecked Sendable {
        let world: World
        var filters: [(type: any Component.Type, access: Int32)] = []
    }

    private var state: State

    @MainActor
    init(world: World) {
        state = State(world: world)
    }

    /// Only the entities that also have a `F` (hecs `With`).
    public func with<F: Component>(_ type: F.Type) -> Self {
        var query = self
        query.state.filters.append((F.self, STICKSHIFT_QUERY_WITH))
        return query
    }

    /// Only the entities that have no `F` (hecs `Without`).
    public func without<F: Component>(_ type: F.Type) -> Self {
        var query = self
        query.state.filters.append((F.self, STICKSHIFT_QUERY_WITHOUT))
        return query
    }

    /// How many entities match, without copying any.
    @MainActor
    public var count: Int {
        let world = state.world
        let (terms, _) = Self.terms(state, "Query.count")
        var out: UInt32 = 0
        world.handle.call("Query.count") { stickshift_world_query_count($0, terms, UInt32(terms.count), &out) }
        return Int(out)
    }

    /// The first matching entity, if any. The whole query runs.
    @MainActor
    public var first: Element? {
        Self.rows(state).first
    }

    /// Runs the query and iterates over a copy of its rows.
    public func makeIterator() -> IndexingIterator<[Element]> {
        let state = state
        return MainActor.assumeIsolated { Self.rows(state) }.makeIterator()
    }

    /// The query's terms for C, and the components its leading terms (the
    /// ones it returns) read.
    @MainActor
    private static func terms(_ state: State, _ what: String) -> ([StickShiftQueryTerm], [ComponentInfo]) {
        let world = state.world
        world.handle.check(what)
        var terms: [StickShiftQueryTerm] = []
        var infos: [ComponentInfo] = []
        func add<U: QueryTerm>(_: U.Type) {
            let info = world.info(U.Queried.self)
            infos.append(info)
            let access = U._isOptionalTerm ? STICKSHIFT_QUERY_OPTIONAL : STICKSHIFT_QUERY_READ
            terms.append(StickShiftQueryTerm(component: info.id, access: StickShiftQueryAccess(access)))
        }
        repeat add((each T).self)
        func id<C: Component>(_: C.Type) -> StickShiftComponent {
            world.info(C.self).id
        }
        for filter in state.filters {
            terms.append(StickShiftQueryTerm(component: id(filter.type), access: StickShiftQueryAccess(filter.access)))
        }
        return (terms, infos)
    }

    /// Run the query and copy its rows out, releasing the engine's snapshot.
    @MainActor
    private static func rows(_ state: State) -> [Element] {
        let world = state.world
        let (terms, infos) = Self.terms(state, "World.query(_:)")
        var rows = StickShiftQueryRows()
        rows.size = UInt32(MemoryLayout<StickShiftQueryRows>.size)
        world.handle.call("World.query(_:)") { stickshift_world_query($0, terms, UInt32(terms.count), &rows) }
        defer { stickshift_world_query_release(world.handle.raw, rows.token) }

        let count = Int(rows.count)
        guard count > 0, let entities = rows.entities, let offsets = rows.offsets, let present = rows.present else {
            return []
        }
        let termCount = Int(rows.term_count)
        let data = rows.data.map(UnsafeRawPointer.init)
        let stride = rows.stride
        var out: [Element] = []
        out.reserveCapacity(count)
        for row in 0..<count {
            var term = 0
            func next<U: QueryTerm>(_: U.Type) -> U {
                let t = term
                term += 1
                guard present[row * termCount + t] != 0 else { return U._term(nil) }
                let offset = offsets[t]
                let bytes = offset == UInt32.max ? nil : data.map { $0 + row * stride + Int(offset) }
                return U._term(infos[t].decode(U.Queried.self, from: bytes))
            }
            out.append((Entity(bits: entities[row].bits), repeat next((each T).self)))
        }
        return out
    }
}
