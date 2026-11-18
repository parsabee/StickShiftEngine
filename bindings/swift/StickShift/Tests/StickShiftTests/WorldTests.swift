// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import CStickShift
import Foundation
@testable import StickShift
import Testing
import simd

/// A game whose hooks run what a test gives them.
@MainActor
final class Scripted: Game {
    var onInitialize: (EngineContext) throws -> Void = { _ in }
    var onUpdate: (EngineContext) -> Void = { _ in }
    var onRender: (EngineContext, Frame) throws -> Void = { _, _ in }

    init(
        initialize: @escaping (EngineContext) throws -> Void = { _ in },
        update: @escaping (EngineContext) -> Void = { _ in },
        render: @escaping (EngineContext, Frame) throws -> Void = { _, _ in }
    ) {
        onInitialize = initialize
        onUpdate = update
        onRender = render
    }

    func initialize(_ ctx: EngineContext) throws { try onInitialize(ctx) }
    func update(_ ctx: EngineContext, dt: Float) { onUpdate(ctx) }
    func render(_ ctx: EngineContext, into frame: Frame) throws { try onRender(ctx, frame) }
}

/// An initialized headless engine running nothing.
@MainActor
func idleEngine() throws -> Engine {
    let engine = try Engine.headless(Scripted())
    try engine.initialize()
    return engine
}

/// Six game components of different sizes and alignments, for the arities,
/// and a tag.
enum Arity {
    struct A: Component, Equatable { var a: Int32 }
    struct B: Component, Equatable { var b: Double }
    struct C: Component, Equatable { var c: SIMD4<Float> }
    struct D: Component, Equatable { var d: UInt8 }
    struct E: Component, Equatable {
        var e: (Int16, Int16, Int16)
        var tag: UInt64

        static func == (l: E, r: E) -> Bool { l.e == r.e && l.tag == r.tag }
    }
    struct F: Component, Equatable { var f: Bool }
    struct Marker: Component {}

    /// A component registered under `A`'s name with `A`'s size: the same
    /// component to the engine.
    struct AliasOfA: Component, Equatable {
        static var componentName: String { A.componentName }
        var a: Int32
    }
}

typealias A = Arity.A
typealias B = Arity.B
typealias C6 = Arity.C
typealias D = Arity.D
typealias E = Arity.E
typealias F = Arity.F
typealias Marker = Arity.Marker
typealias AliasOfA = Arity.AliasOfA

@Suite("The world: components, entities and queries (null backend)")
@MainActor
struct WorldTests {
    let a = A(a: -7)
    let b = B(b: 2.5)
    let c = C6(c: [1, 2, 3, 4])
    let d = D(d: 200)
    let e = E(e: (1, -2, 3), tag: 0xdead_beef)
    let f = F(f: true)

    @Test func spawnAndQueryAtEveryArityFromOneToSix() throws {
        let world = try idleEngine().world
        let e1 = world.spawn(a)
        let e2 = world.spawn(a, b)
        let e3 = world.spawn(a, b, c)
        let e4 = world.spawn(a, b, c, d)
        let e5 = world.spawn(a, b, c, d, e)
        let e6 = world.spawn(a, b, c, d, e, f)
        #expect(world.count == 6)

        // Archetypes in creation order, so the entities come back in spawn order.
        #expect(world.query(A.self).map(\.0) == [e1, e2, e3, e4, e5, e6])
        #expect(world.query(A.self).map { _, value in value } == Array(repeating: a, count: 6))
        let two = world.query(A.self, B.self).map { $0 }
        #expect(two.map(\.0) == [e2, e3, e4, e5, e6] && two.allSatisfy { $0.1 == a && $0.2 == b })
        let three = world.query(A.self, B.self, C6.self).map { $0 }
        #expect(three.map(\.0) == [e3, e4, e5, e6] && three.allSatisfy { $0.3 == c })
        let four = world.query(A.self, B.self, C6.self, D.self).map { $0 }
        #expect(four.map(\.0) == [e4, e5, e6] && four.allSatisfy { $0.4 == d })
        let five = world.query(A.self, B.self, C6.self, D.self, E.self).map { $0 }
        #expect(five.map(\.0) == [e5, e6] && five.allSatisfy { $0.5 == e })
        let six = world.query(A.self, B.self, C6.self, D.self, E.self, F.self).map { $0 }
        #expect(six.count == 1)
        if let (entity, qa, qb, qc, qd, qe, qf) = six.first {
            #expect(entity == e6 && qa == a && qb == b && qc == c && qd == d && qe == e && qf == f)
        }
        // Terms in any order read the same components.
        let reversed = world.query(F.self, E.self, D.self, C6.self, B.self, A.self).map { $0 }
        #expect(reversed.count == 1 && reversed.first?.1 == f && reversed.first?.6 == a)
        #expect(world.query(A.self, B.self, C6.self, D.self, E.self, F.self).count == 1)
        #expect(world.query(A.self).count == 6)
    }

    @Test func aBundleIsOneSpawnWhateverItsOrder() throws {
        let world = try idleEngine().world
        let first = world.spawn(Transform(), RenderComponent.cube([1, 0, 0]), StaticTag())
        let other = world.spawn(StaticTag(), A(a: 1))
        let second = world.spawn(StaticTag(), Transform(position: [1, 2, 3]), RenderComponent.cube([0, 1, 0]))
        // Same set, same archetype: the two land next to each other, before
        // the archetype created after the first.
        #expect(world.query(StaticTag.self).map(\.0) == [first, second, other])
    }

    @Test func optionalTermsAndFiltersAreHecss() throws {
        let world = try idleEngine().world
        let plain = world.spawn(Transform(position: [1, 0, 0]))
        let red = world.spawn(Transform(position: [2, 0, 0]), RenderComponent.sphere([1, 0, 0]))
        let tagged = world.spawn(Transform(position: [3, 0, 0]), Marker())
        let alone = world.spawn(A(a: 3))

        let rows = world.query(Transform.self, RenderComponent?.self, Marker?.self).map { $0 }
        #expect(rows.map(\.0) == [plain, red, tagged])
        #expect(rows.map { $0.2 } == [nil, RenderComponent.sphere([1, 0, 0]), nil])
        #expect(rows.map { $0.3 != nil } == [false, false, true])
        #expect(world.query(Transform.self).with(Marker.self).map(\.0) == [tagged])
        #expect(world.query(Transform.self).without(Marker.self).without(RenderComponent.self).map(\.0) == [plain])
        // An optional term alone matches every entity, as Option<&T> does.
        #expect(world.query(A?.self).map(\.0) == [plain, red, tagged, alone])
        #expect(world.query(A?.self).map { _, value in value } == [nil, nil, nil, A(a: 3)])
        // A tag read as a term is a filter with an empty value.
        #expect(world.query(Marker.self).map(\.0) == [tagged])
        #expect(world.query(Transform.self).with(Marker.self).count == 1)
        #expect(world.query(Transform.self).first?.0 == plain)
        #expect(world.query(F.self).first == nil)
    }

    @Test func getSetHasInsertRemoveAndDespawn() throws {
        let world = try idleEngine().world
        let entity = world.spawn(Transform(position: [1, 2, 3]), a)
        #expect(world.contains(entity))
        #expect(world.has(A.self, entity) && !world.has(B.self, entity) && !world.has(Marker.self, entity))
        #expect(world.get(A.self, of: entity) == a)
        #expect(world.get(B.self, of: entity) == nil)
        #expect(world.get(Transform.self, of: entity)?.position == [1, 2, 3])

        #expect(world.set(A(a: 42), for: entity))
        #expect(world.get(A.self, of: entity) == A(a: 42))
        #expect(!world.set(b, for: entity), "it has no B")
        #expect(world.modify(Transform.self, of: entity) { $0.position.y += 1 })
        #expect(world.get(Transform.self, of: entity)?.position == [1, 3, 3])
        #expect(!world.modify(B.self, of: entity) { _ in Issue.record("no B to modify") })

        // Insert replaces values it has and adds the rest; tags read as values.
        #expect(world.insert(A(a: 5), b, Marker(), into: entity))
        #expect(world.get(A.self, of: entity) == A(a: 5) && world.get(B.self, of: entity) == b)
        #expect(world.get(Marker.self, of: entity) != nil && world.set(Marker(), for: entity))
        #expect(world.remove(Marker.self, from: entity))
        #expect(!world.remove(Marker.self, from: entity), "already removed")
        #expect(world.get(Marker.self, of: entity) == nil && !world.set(Marker(), for: entity))
        #expect(world.count == 1)

        #expect(world.despawn(entity))
        #expect(!world.despawn(entity), "already gone")
        #expect(!world.contains(entity) && world.get(A.self, of: entity) == nil)
        #expect(!world.insert(a, into: entity) && !world.remove(A.self, from: entity) && !world.set(a, for: entity))
        #expect(world.count == 0)

        // The id comes back, last freed first, with a bumped generation: the
        // old entity still reads as gone.
        let reused = world.spawn(a)
        #expect(reused.bits & 0xffff_ffff == entity.bits & 0xffff_ffff && reused.bits >> 32 == (entity.bits >> 32) + 1)
        #expect(!world.contains(entity) && world.contains(reused))
        let empty = world.spawn()
        #expect(world.contains(empty) && world.count == 2)
    }

    @Test func iteratingCopiesTheRowsSoTheBodyMayChangeTheWorld() throws {
        let world = try idleEngine().world
        for i in 0..<20 {
            world.spawn(A(a: Int32(i)), Transform())
        }
        var seen = 0
        for (entity, value) in world.query(A.self) {
            seen += 1
            if value.a.isMultiple(of: 2) {
                world.despawn(entity)
            } else {
                world.insert(Marker(), into: entity)
            }
            // Another query, nested.
            _ = world.query(Transform.self).map(\.0)
        }
        #expect(seen == 20)
        #expect(world.query(A.self).count == 10 && world.query(A.self).with(Marker.self).count == 10)
    }

    @Test func theEngineComponentsCrossFieldByField() throws {
        let world = try idleEngine().world
        let t = Transform(position: [1, -2, 3.5], rotation: simd_quatf(glamRotationY: 0.7), scale: [2, 3, 4])
        let r = RenderComponent(color: [0.25, 0.5, 0.75], shape: .sphere)
        let entity = world.spawn(t, r, DynamicTag())
        let back = world.get(Transform.self, of: entity)!
        #expect(back == t)
        #expect(world.get(RenderComponent.self, of: entity) == r)
        #expect(world.has(DynamicTag.self, entity) && !world.has(StaticTag.self, entity))
        #expect(world.get(DynamicTag.self, of: entity) != nil)
        #expect(bits(back.matrix) == bits(simd_float4x4(stickshift_transform_to_mat4(t.c))))
        #expect(RenderComponent.cube([1, 1, 1]).shape == .cube)
        #expect(Transform().rotation == simd_quatf(ix: 0, iy: 0, iz: 0, r: 1) && Transform().scale == .one)
    }

    @Test func registrationIsLazyPerEngineAndIdempotent() throws {
        let engine = try idleEngine()
        #expect(engine.registeredComponents == 0)
        // The engine's own components are never registered.
        engine.world.spawn(Transform(), RenderComponent.cube(.one), StaticTag(), DynamicTag())
        #expect(engine.registeredComponents == 0)

        let entity = engine.world.spawn(a)
        #expect(engine.registeredComponents == 1)
        #expect(engine.componentID(A.self) == UInt32(STICKSHIFT_FIRST_GAME_COMPONENT))
        _ = engine.world.get(A.self, of: entity)
        _ = engine.world.query(A.self).count
        #expect(engine.registeredComponents == 1, "later uses are free")
        _ = engine.world.has(Marker.self, entity)
        #expect(engine.registeredComponents == 2)
        #expect(engine.componentID(Marker.self) == UInt32(STICKSHIFT_FIRST_GAME_COMPONENT) + 1)

        // Another type under the same name and size is the same component.
        #expect(engine.componentID(AliasOfA.self) == engine.componentID(A.self))
        #expect(engine.world.get(AliasOfA.self, of: entity) == AliasOfA(a: a.a))

        // Each engine registers on its own first use.
        let other = try idleEngine()
        #expect(other.registeredComponents == 0)
        _ = other.world.has(Marker.self, other.world.spawn())
        #expect(other.componentID(Marker.self) == UInt32(STICKSHIFT_FIRST_GAME_COMPONENT))
        #expect(A.componentName == "StickShiftTests.Arity.A")
    }

    @Test func hooksReachTheWorldAndTheRenderHookReadsIt() throws {
        var rendered: [Entity] = []
        var spawned: Entity?
        let game = Scripted(
            initialize: { ctx in spawned = ctx.world.spawn(Transform(), Marker()) },
            update: { ctx in ctx.world.modify(Transform.self, of: spawned!) { $0.position.x += 1 } },
            render: { ctx, _ in rendered = ctx.world.query(Marker.self).map(\.0) }
        )
        let engine = try Engine.headless(game)
        try engine.initialize()
        engine.advance()
        engine.advance()
        #expect(rendered == [spawned!])
        #expect(engine.world.get(Transform.self, of: spawned!)?.position.x == 2)
    }
}
