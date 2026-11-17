// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Foundation
import StickShift
import Testing
import simd

@testable import OfframpGame

/// The render pass (`render.rs`): the draw list of a known world, and the draws
/// a frame records, in order, with their matrices. No GPU: the null backend
/// records the draws.
@MainActor
@Suite("render")
struct RenderTests {
    // MARK: - The draw list

    @Test func theDrawListOfTheInitWorldIsInQueryOrder() throws {
        let (game, h) = try booted()
        let world = h.world
        let player = try #require(game.player)
        let position = SIMD3<Float>(laneX(START_LANE), PLAYER_Y, -0.0)
        #expect(bits(game.playerPosition()) == bits(position))
        let list = SceneDrawList(world: world, player: player, playerPosition: position)

        // 31 slots: a road tile, two buildings and two guardrails each, and an
        // obstacle on the 15 even slots after 0.
        #expect(list.roads.count == 31)
        #expect(list.guardrails.count == 62)
        #expect(list.models.count == 1 + 62 + 15)
        // Query order: the player's archetype, then the buildings', then the
        // obstacles' (spec 03 §7.1).
        #expect(list.models[0] == ModelPlacement(transform: try #require(world.get(Transform.self, of: player)), model: .player))
        #expect(list.models[1...62].allSatisfy { if case .building = $0.model { true } else { false } })
        #expect(list.models[63...].allSatisfy { if case .traffic = $0.model { true } else { false } })

        // Each list is its query's order, filtered as render.rs filters it: the
        // same walk, as one query with optional terms.
        let rows = world.query(
            Transform.self, RenderComponent.self, Obstacle?.self, TrafficVariant?.self, BuildingVariant?.self,
            GuardrailTag?.self
        )
        var roads: [Transform] = []
        var models: [ModelPlacement] = []
        for (entity, t, _, obstacle, traffic, building, guardrail) in rows {
            if entity == player {
                models.append(ModelPlacement(transform: t, model: .player))
            } else if obstacle != nil {
                models.append(ModelPlacement(transform: t, model: .traffic(traffic?.index ?? 0)))
            } else if let building {
                models.append(ModelPlacement(transform: t, model: .building(building.index)))
            } else if guardrail == nil {
                roads.append(t)
            }
        }
        #expect(list.roads == roads)
        #expect(list.models == models)
        #expect(list.guardrails == world.query(GuardrailTag.self, Transform.self).map { _, _, t in t })

        // The camera-locked pieces, from player_position() (spec 03 §7.3).
        #expect(bits(list.terrain.position) == [0, 0, 0x8000_0000], "z is -travel = -0.0")
        #expect(bits(list.backdrop.position) == [0, 0xC0F6_6665, bits(Float(-159))], "y = -7.6999993, not -7.7")
        #expect(list.backdrop.scale == SIMD3(450, 72, 3))
        #expect(bits(list.backdrop.rotation) == [0, 0, 0, 0x3F80_0000])
    }

    @Test func theRoadListSkipsEverythingButRoadTiles() throws {
        // One entity of each kind, plus a bare renderable (a road by render.rs's
        // rule) and an obstacle without a variant (traffic car 0).
        let engine = try scratchEngine()
        let world = engine.world
        let t = { (z: Float) in Transform(position: SIMD3(0, 0, z)) }
        let player = world.spawn(t(0), RenderComponent.cube(.one), DynamicTag())
        world.spawn(t(1), RenderComponent.cube(.one), StaticTag())
        world.spawn(t(2), RenderComponent.cube(.one), StaticTag(), Obstacle())
        world.spawn(t(3), RenderComponent.cube(.one), BuildingVariant(4))
        world.spawn(t(4), RenderComponent.cube(.one), GuardrailTag())
        world.spawn(t(5), GuardrailTag())
        world.spawn(t(6))
        let list = SceneDrawList(world: world, player: player, playerPosition: .zero)
        #expect(list.roads.map(\.position.z) == [1])
        #expect(list.guardrails.map(\.position.z) == [4, 5], "a guardrail needs no render component")
        #expect(list.models.map(\.model) == [.player, .traffic(0), .building(4)])
        #expect(SceneDrawList(world: world, player: nil, playerPosition: .zero).roads.map(\.position.z) == [0, 1])
    }

    // MARK: - The draws of a frame

    /// The parts of each model the textured and untextured passes draw (spec
    /// 03 §7.4): only the player car (and the road and skyline) has textured
    /// parts.
    private static let texturedParts = (road: 1, skyline: 1, player: 4)
    private static let untexturedParts = (player: 2, traffic: [13, 12, 14], buildings: [6, 6, 5, 5, 5, 5, 4, 2])

    @Test func aFrameRecordsRustsDrawsInRustsOrder() throws {
        let context = try nullContext()
        let resources = try loadResources(context)
        let models = resources.models
        let (game, h) = try booted()
        let list = SceneDrawList(world: h.world, player: game.player, playerPosition: SIMD3(0, PLAYER_Y, -0.0))

        var record: FrameRecord?
        let parts = try drawsSubmitted(context) {
            record = try recordedFrame(context) { try recordScene(list, resources: resources, frame: $0) }
        }
        // The engine drew what Rust's first frame drew: 563 (probe "draws 563").
        #expect(parts == 563)
        #expect(context.stats.drawsSkipped == 0)
        let draws = try #require(record).draws
        #expect(draws.count == 563)

        // Each part the engine drew, in order: what it drew from, and for a mesh
        // its matrix (a model part's is `world × (local × node)`).
        var expected: [(source: UInt64, matrix: [UInt32]?)] = []
        func model(_ m: Model, parts: Int) {
            expected += Array(repeating: (m.id, nil), count: parts)
        }
        func mesh(_ m: Mesh, _ t: Transform) {
            expected.append((m.id, glamTRS(t)))
        }
        // 1. The backdrop.
        model(models.skyline, parts: Self.texturedParts.skyline)
        // 2. The road tiles, then each placement's textured parts.
        for _ in list.roads {
            model(models.road, parts: Self.texturedParts.road)
        }
        for p in list.models where p.model == .player {
            model(models.playerCar.model, parts: Self.texturedParts.player)
        }
        // 3. The terrain, the guardrails, then each placement's untextured parts.
        mesh(resources.terrain, list.terrain)
        for rail in list.guardrails {
            mesh(resources.guardrail, rail)
        }
        for p in list.models {
            switch p.model {
            case .player: model(models.playerCar.model, parts: Self.untexturedParts.player)
            case let .traffic(v): model(models.trafficCars[v].model, parts: Self.untexturedParts.traffic[v])
            case let .building(v): model(models.buildings[v].model, parts: Self.untexturedParts.buildings[v])
            }
        }
        #expect(draws.count == expected.count)
        for (i, (d, e)) in zip(draws, expected).enumerated() {
            #expect(d.source == e.source, "draw \(i)")
            if let matrix = e.matrix {
                #expect(bits(d.model) == matrix, "draw \(i): a mesh is drawn at its transform's TRS")
                #expect(d.baseColor == .one && d.texture == 0 && d.emissive == .zero, "MaterialParams::default()")
            } else {
                #expect(d.baseColor.w == 1, "draw \(i): the default tint keeps the base colour's alpha")
            }
        }
    }

    @Test func theRecordersFrame150DrawsWhatTheRustFrameDrew() throws {
        // The world of record_demo's frame 150, rebuilt from the Rust dump: the
        // same digest, and the per-frame draw count of spec 03 §7.4.
        let frame = try recorderFrame150()
        #expect(worldDigest(frame.world, player: frame.player) == frame.digest)
        let list = frame.drawList
        #expect(list.roads.count == 32 && list.guardrails.count == 64 && list.models.count == 1 + 64 + 16)

        let context = try nullContext()
        let resources = try loadResources(context)
        let font = try loadHudFont(context)
        let carParts = [13, 12, 14]
        let buildingParts = [6, 6, 5, 5, 5, 5, 4, 2]
        var expected = 1 + list.roads.count + 6 + 1 + list.guardrails.count
        for p in list.models {
            switch p.model {
            case .player: break
            case let .traffic(v): expected += carParts[v]
            case let .building(v): expected += buildingParts[v]
            }
        }
        try context.setCamera(frame.camera)
        try context.setSunSky(DEMO_SUN)
        var record: FrameRecord?
        let parts = try drawsSubmitted(context) {
            record = try recordedFrame(context) { f in
                try recordScene(list, resources: resources, frame: f)
                try drawHud(frame.hud, font: font, frame: f)
            }
        }
        #expect(parts == UInt64(expected))
        // The HUD: "SCORE 35" with the fade long gone.
        #expect(try #require(record).quads.count == (try visibleGlyphs("SCORE 35")))
        #expect(try #require(record).camera == frame.camera && record?.sunSky == DEMO_SUN)
        #expect(context.stats.drawsSkipped == 0)
    }

    @Test func theTexturedPassIsThePlayersTexturedParts() throws {
        // The textured pass draws the backdrop, the tiles and the player car's 4
        // textured parts; the untextured pass draws the rest (spec 03 §7.2).
        let context = try nullContext()
        let resources = try loadResources(context)
        let (game, h) = try booted()
        let list = SceneDrawList(world: h.world, player: game.player, playerPosition: SIMD3(0, PLAYER_Y, -0.0))
        func draws(_ list: SceneDrawList) throws -> UInt64 {
            try drawsSubmitted(context) { _ = try recordedFrame(context) { try recordScene(list, resources: resources, frame: $0) } }
        }
        var withoutRails = list
        withoutRails.guardrails = []
        #expect(try draws(list) - draws(withoutRails) == 62)
        let playerOnly = SceneDrawList(world: try scratchEngine().world, player: nil, playerPosition: .zero)
        var justThePlayer = playerOnly
        justThePlayer.models = [list.models[0]]
        // Backdrop 1 + no tiles + the car's 6 parts + terrain 1.
        #expect(try draws(justThePlayer) == 8)
        #expect(try draws(playerOnly) == 2)
    }

    // MARK: - The sun

    @Test func theDemosSunIsAWarmAfternoonSunInTheWest() {
        // As T3 checks it.
        #expect(DEMO_SUN.elevation == 32 && DEMO_SUN.azimuth == 284)
        #expect(DEMO_SUN.sunColor == SIMD3(1, 0.96, 0.88) && DEMO_SUN.sunIntensity == 1.15 && DEMO_SUN.skyFill == 0.22)
        #expect(DEMO_SUN.skyZenithColor == SIMD3(0.11, 0.27, 0.56) && DEMO_SUN.skyHorizonColor == SIMD3(0.62, 0.67, 0.74))
    }
}
