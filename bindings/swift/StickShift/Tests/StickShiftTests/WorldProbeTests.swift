// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import StickShift
import Testing
import simd

/// The world and streaming probe of `crates/stickshift-capi/tests/engine_parity.rs`,
/// written as a Swift game: the same content and rules, through the Swift
/// API. Its lines must be the Rust probe's (Tests/Fixtures/probe-world.txt).
@MainActor
final class WorldProbe: Game {
    struct Obstacle: Component, Equatable {
        var lane: Int32
        var speed: Float
    }

    struct BuildingVariant: Component, Equatable {
        var value: UInt32
    }

    struct GuardrailTag: Component {}
    struct Hit: Component {}

    struct SlotMarker: Component, Equatable {
        var slot: Int64
    }

    enum Piece {
        case road(Transform, SIMD3<Float>)
        case lamp(Transform, SIMD3<Float>)
        case obstacle(Transform, Obstacle, SIMD3<Float>)
        case building(Transform, SIMD3<Float>, BuildingVariant)
        case rail(Transform)
        case marker(SlotMarker)
    }

    static let frames = 3000
    static let speed: Float = 25
    static let playerColor: SIMD3<Float> = [0.1, 0.4, 0.9]
    static let identity = simd_quatf(ix: 0, iy: 0, iz: 0, r: 1)
    static let config = StreamingConfig(
        axis: [0, 0, -1], spawnInterval: 6, spawnAhead: 60, despawnBehind: 12, rebaseThreshold: 150
    )
    static let playerStart = Transform(position: [0, 0.5, 0])
    /// The chase camera the probe follows the player with.
    static let chase = ChaseController(distance: 7.5, height: 3, lookAtHeight: 1.25, up: [0, 1, 0], halfLife: 0.15)

    /// The draw distance the probe requests at update `n`, if any: once
    /// before the reach change halfway, once after.
    static func drawDistance(at n: Int) -> GraphicsSettings.DrawDistance? {
        switch n {
        case frames / 3: .near
        case 2 * frames / 3: .medium
        default: nil
        }
    }

    var player: Entity?
    var updates = 0
    var rebases = 0
    var digest = fnvOffset

    static func remEuclid(_ a: Int64, _ b: Int64) -> Int64 {
        let r = a % b
        return r < 0 ? r + b : r
    }

    static func pieces(slot: Int64, at p: SIMD3<Float>) -> [Piece] {
        func at(_ offset: SIMD3<Float>, _ rotation: simd_quatf, _ scale: SIMD3<Float>) -> Transform {
            Transform(position: p + offset, rotation: rotation, scale: scale)
        }
        var pieces: [Piece] = [.road(at(.zero, identity, [8, 0.1, 6]), [0.2, 0.2, 0.2])]
        if remEuclid(slot, 3) == 2 {
            let lane = Int32(remEuclid(slot / 3, 3))
            pieces.append(
                .obstacle(
                    at([Float(lane - 1) * 2.5, 0.5, 0], identity, .one),
                    Obstacle(lane: lane, speed: 0),
                    [0.9, 0.1, 0.1]
                )
            )
        }
        for side: Float in [-1, 1] {
            pieces.append(
                .building(
                    at([side * 12, 4, 0], simd_quatf(glamRotationY: Float(slot) * 0.1), [4, 8, 4]),
                    [0.5, 0.5, 0.6],
                    BuildingVariant(value: UInt32(remEuclid(slot, 4)))
                )
            )
        }
        for side: Float in [-1, 1] {
            pieces.append(.rail(at([side * 5, 0.4, 0], identity, [0.2, 0.8, 6])))
        }
        if remEuclid(slot, 5) == 0 {
            pieces.append(.lamp(at([6, 3, 0], identity, SIMD3(repeating: 0.5)), [1, 0.9, 0.6]))
        }
        if remEuclid(slot, 4) == 1 {
            pieces.append(.marker(SlotMarker(slot: slot)))
        }
        return pieces
    }

    /// Fill one slot, spawning each piece with its whole bundle (in another
    /// order than Rust's tuple for some: the set is what counts).
    static func fill(_ slot: borrowing StreamSlot) {
        for piece in pieces(slot: slot.index, at: slot.position) {
            switch piece {
            case let .road(t, color):
                slot.spawn(t, RenderComponent.cube(color), StaticTag())
            case let .lamp(t, color):
                slot.spawn(t, RenderComponent.sphere(color), StaticTag())
            case let .obstacle(t, o, color):
                slot.spawn(StaticTag(), RenderComponent.cube(color), o, t)
            case let .building(t, color, b):
                slot.spawn(t, RenderComponent.cube(color), b, StaticTag())
            case let .rail(t):
                slot.spawn(t, RenderComponent.cube([0.7, 0.7, 0.7]), GuardrailTag(), StaticTag())
            case let .marker(m):
                slot.spawn(m)
            }
        }
    }

    static func obstaclesToDespawn(_ n: Int, _ obstacles: [(Entity, SIMD3<Float>, Obstacle)], playerZ: Float) -> [Entity] {
        guard n % 7 == 3 else { return [] }
        let lane = Int32((n / 7) % 3)
        return obstacles.filter { _, p, o in o.lane == lane && p.z < playerZ && p.z > playerZ - 40 }.map(\.0)
    }

    func initialize(_ ctx: EngineContext) throws {
        try ctx.scene.setConfig(Self.config)
        player = ctx.world.spawn(Self.playerStart, RenderComponent.cube(Self.playerColor), DynamicTag())
        ctx.scene.stream(focus: Self.playerStart.position, Self.fill)
    }

    func update(_ ctx: EngineContext, dt: Float) {
        let n = updates
        updates += 1
        let world = ctx.world
        let scene = ctx.scene
        let player = player!

        var t = world.get(Transform.self, of: player)!
        t.position.z -= Self.speed * dt
        world.set(t, for: player)
        let offset = scene.maybeRebase(focus: t.position)
        if offset != .zero {
            rebases += 1
            // Carry the camera through the coordinate change, as the demo does.
            let camera = ctx.camera
            let position = camera.position + offset
            let target = camera.target + offset
            camera.position = position
            camera.target = target
        }
        let pos = world.get(Transform.self, of: player)!.position
        ctx.camera.follow(Self.chase, target: pos, forward: [0, 0, -1], dt: dt)
        if n == Self.frames / 2 {
            var c = scene.config
            c.spawnAhead = 90
            c.despawnBehind = 18
            try! scene.setConfig(c)
        }
        if let drawDistance = Self.drawDistance(at: n) {
            var s = ctx.graphics.current
            s.drawDistance = drawDistance
            ctx.graphics.request(s)
        }
        scene.stream(focus: pos, Self.fill)

        // Query, then despawn.
        let obstacles = world.query(Transform.self, Obstacle.self).map { e, t, o in (e, t.position, o) }
        for e in Self.obstaclesToDespawn(n, obstacles, playerZ: pos.z) {
            world.despawn(e)
        }
        if n % 11 == 5, let (e, _, b) = world.query(Transform.self, BuildingVariant.self).without(Hit.self).first {
            world.insert(Hit(), BuildingVariant(value: b.value + 100), into: e)
        }
        if n % 13 == 6, let (e, _) = world.query(Hit.self).first {
            world.remove(Hit.self, from: e)
        }
        if n % 5 == 1 {
            for (e, _) in world.query(Obstacle.self) {
                world.modify(Obstacle.self, of: e) { $0.speed += 1 }
            }
        }
        if n % 17 == 8, let (e, _) = world.query(SlotMarker.self).without(Transform.self).first {
            world.despawn(e)
        }
        if n % 29 == 10 {
            world.spawn(Transform(position: pos + [0, 2, 0]), DynamicTag())
        }
        if n % 31 == 0 && n > 0,
            let (e, _) = world.query(Transform.self).with(DynamicTag.self).without(RenderComponent.self).first
        {
            world.despawn(e)
        }
    }

    func render(_ ctx: EngineContext, into frame: Frame) throws {
        var h = fnvOffset
        for (e, t) in ctx.world.query(Transform.self).with(RenderComponent.self) {
            h = digestRow(h, e.bits, t.position)
        }
        digest = h
    }

    func digestRow(_ hash: UInt64, _ bits: UInt64, _ p: SIMD3<Float>) -> UInt64 {
        var h = hash
        withUnsafeBytes(of: bits.littleEndian) { h = fnv(h, $0) }
        for v in [p.x, p.y, p.z] {
            withUnsafeBytes(of: v.bitPattern.littleEndian) { h = fnv(h, $0) }
        }
        return h
    }

    /// The probe's dump of the world: every entity with a transform, in query
    /// order, then the markers.
    static func dump(_ world: World, into log: ProbeLog) {
        func f(_ v: Float) -> String { hex(UInt64(v.bitPattern), width: 8) }
        let rows = world.query(
            Transform.self, RenderComponent?.self, StaticTag?.self, DynamicTag?.self,
            Obstacle?.self, BuildingVariant?.self, GuardrailTag?.self, Hit?.self
        )
        for (e, t, r, s, d, o, b, g, h) in rows {
            var line = "\(hex(e.bits, width: 16)) t \(f(t.position.x)) \(f(t.position.y)) \(f(t.position.z))"
            line += " | \(f(t.rotation.imag.x)) \(f(t.rotation.imag.y)) \(f(t.rotation.imag.z)) \(f(t.rotation.real))"
            line += " | \(f(t.scale.x)) \(f(t.scale.y)) \(f(t.scale.z))"
            if let r {
                line += " r \(f(r.color.x)) \(f(r.color.y)) \(f(r.color.z)) \(r.shape.rawValue)"
            }
            if s != nil { line += " static" }
            if d != nil { line += " dynamic" }
            if let o { line += " obstacle \(o.lane) \(f(o.speed))" }
            if let b { line += " building \(b.value)" }
            if g != nil { line += " rail" }
            if h != nil { line += " hit" }
            log.add(line)
        }
        for (e, m) in world.query(SlotMarker.self).without(Transform.self) {
            log.add("\(hex(e.bits, width: 16)) marker \(m.slot)")
        }
    }

    func report(_ frame: String, _ engine: Engine, into log: ProbeLog) {
        log.add(
            "\(frame) len \(engine.world.count) streamed \(engine.scene.streamedCount) "
                + "digest \(hex(digest, width: 16)) rebases \(rebases)"
        )
        func f(_ v: Float) -> String { hex(UInt64(v.bitPattern), width: 8) }
        func v(_ p: SIMD3<Float>) -> String { "\(f(p.x)) \(f(p.y)) \(f(p.z))" }
        func hashMatrix(_ m: simd_float4x4) -> String {
            var h = fnvOffset
            for c in 0..<4 {
                for r in 0..<4 {
                    withUnsafeBytes(of: m[c][r].bitPattern.littleEndian) { h = fnv(h, $0) }
                }
            }
            return hex(h, width: 16)
        }
        let config = engine.scene.config
        log.add("reach \(f(config.spawnAhead)) \(f(config.despawnBehind)) revision \(engine.graphics.revision)")
        // Between ticks the context's camera is what the engine pushed last;
        // nothing is pushed before the first frame.
        let camera = engine.camera
        var pushed = "none"
        if frame != "init" {
            let c = engine.context.camera
            pushed = "\(hashMatrix(c.viewProjection)) \(v(c.position))"
        }
        log.add(
            "camera \(v(camera.position)) | \(v(camera.target)) | \(hashMatrix(camera.viewProjection)) pushed \(pushed)"
        )
        Self.dump(engine.world, into: log)
    }
}

@Suite("The probe differential, world and streaming (null backend)")
@MainActor
struct WorldProbeTests {
    @Test func theSwiftProbePrintsTheRustProbesWorldFrameForFrame() throws {
        let log = try ProbeLog("world", keeping: ["frame 2999 "])
        let game = WorldProbe()
        let engine = try Engine.headless(game)
        try engine.initialize()
        game.report("init", engine, into: log)
        for frame in 0..<WorldProbe.frames {
            let tick = engine.advance()
            #expect(tick.steps == 1 && tick.rendered, "frame \(frame)")
            game.report("frame \(frame)", engine, into: log)
        }
        log.finish()
        // The run went through what it is meant to cover.
        #expect(game.rebases >= 5, "only \(game.rebases) rebases")
        // Both settings requests applied.
        #expect(engine.graphics.revision == 2)
        #expect(log.kept.count == 1)
    }
}
