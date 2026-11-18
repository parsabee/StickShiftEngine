// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Foundation
import StickShift
import Testing
import simd

@testable import OfframpGame

/// The asset layer (`assets.rs`): every model loads through the C API, the fits
/// have the Rust bits (spec 04 §10), and each model has the parts the Rust demo
/// draws (spec 03 §7.4).
@MainActor
@Suite("assets")
struct AssetsTests {
    @Test func everyModelLoadsAndTheBoundsAreRusts() throws {
        let context = try nullContext()
        let models = try loadDemoModels(context)
        for model in [models.road, models.skyline, models.playerCar.model] + models.trafficCars.map(\.model)
            + models.buildings.map(\.model)
        {
            #expect(model.state == .ready)
        }
        #expect(models.trafficCars.count == TRAFFIC_CAR_ASSETS.count && models.buildings.count == BUILDING_ASSETS.count)

        // Spec 04 §10.1, bit for bit.
        func expectBounds(_ model: Model, _ lo: SIMD3<Float>, _ hi: SIMD3<Float>, _ name: String) throws {
            let b = try model.bounds()
            #expect(bits(b.min) == bits(lo), "\(name) min \(b.min)")
            #expect(bits(b.max) == bits(hi), "\(name) max \(b.max)")
        }
        try expectBounds(models.road, SIMD3(-0.5, 0.5, -0.5), SIMD3(0.5, 0.5, 0.5), "road")
        try expectBounds(models.skyline, SIMD3(-0.5, -0.5, 0), SIMD3(0.5, 0.5, 56), "skyline")
        try expectBounds(
            models.playerCar.model, v3(0xBFAA_EB6F, 0xBC40_B0BD, 0xC02D_CF6A), v3(0x3FAA_EB67, 0x3FED_2DF7, 0x403C_1DBF),
            "sports_car"
        )
        try expectBounds(
            models.trafficCars[0].model, v3(0xBF51_B8AA, 0xBCE8_2F94, 0xBFD3_1BE7), v3(0x3F51_B8AA, 0x3F8F_02CD, 0x3FD4_85D3),
            "car"
        )
        try expectBounds(
            models.trafficCars[1].model, v3(0xBF67_5793, 0x3BC5_974E, 0xC006_DD6E), v3(0x3F67_5793, 0x3F97_5FBB, 0x4007_42CC),
            "car2"
        )
        try expectBounds(
            models.trafficCars[2].model, v3(0xBF63_8940, 0xBC85_35CA, 0xBFEE_FB1A), v3(0x3F63_8940, 0x3F9C_7A85, 0x3FEE_8598),
            "police_car"
        )
        let buildingBounds: [(SIMD3<Float>, SIMD3<Float>)] = [
            (SIMD3(-0.62, 0, -0.62), SIMD3(0.62, 3.15, 0.62)),
            (SIMD3(-0.62, 0, f(0xBF3F_75CE)), SIMD3(0.62, 5.5, 0.62)),
            (SIMD3(-0.62, 0, -0.72), SIMD3(0.62, 2.08, 0.62)),
            (SIMD3(-0.62, 0, -0.62), SIMD3(0.62, 1.68, 0.62)),
            (SIMD3(-0.42, 0, -0.42), SIMD3(0.42, 1.68, 0.42)),
            (v3(0xBF51_EB87, 0, 0xBF0C_429B), v3(0x3F51_EB87, 0x3F61_47AC, 0x3ED7_0A3D)),
            (SIMD3(-0.42, 0, -0.52), v3(0x3ED7_0A3D, 0x3F61_47AC, 0x3ED7_0A3D)),
            (SIMD3(-0.25, 0, -0.25), SIMD3(0.25, 1.75, 0.25)),
        ]
        for (i, (lo, hi)) in buildingBounds.enumerated() {
            try expectBounds(models.buildings[i].model, lo, hi, BUILDING_ASSETS[i])
        }
    }

    @Test func theFitsAreTheRustFits() throws {
        let context = try nullContext()
        let models = try loadDemoModels(context)

        // Spec 04 §10.3 and §10.4: the cars turn by Rust's π about +Y.
        let carRotation: [UInt32] = [0, 0x3F80_0000, 0, 0xB33B_BD2E]
        let cars: [(FittedModel, [UInt32], UInt32, String)] = [
            (models.playerCar, [0xB47C_BEC7, 0xBEC9_C3EB, 0x3D66_B68F], 0x3F01_042B, "sports_car"),
            (models.trafficCars[0], [0x2FE4_8EBE, 0xBEC0_4DBE, 0x3B9B_D47D], 0x3F5C_72B4, "car"),
            (models.trafficCars[1], [0x2F48_C8DE, 0xBECE_E27D, 0x3B08_E4FE], 0x3F2C_DC93, "car2"),
            (models.trafficCars[2], [0xAF03_ABC8, 0xBEC6_707F, 0xBAB3_8BC6], 0x3F43_93D8, "police_car"),
        ]
        for (fitted, position, scale, name) in cars {
            #expect(bits(fitted.fit.position) == position, "\(name) position")
            #expect(bits(fitted.fit.rotation) == carRotation, "\(name) rotation")
            #expect(bits(fitted.fit.scale) == [scale, scale, scale], "\(name) scale")
            #expect(bits(fitted.local) == glamTRS(fitted.fit), "\(name): local is the fit's TRS")
        }

        // Buildings: no turn, -0.0 where the pivot is 0.
        let negZero: UInt32 = 0x8000_0000
        let buildings: [(UInt32, UInt32)] = [
            (negZero, 0x40CE_739D), (0x3EBF_7A6C, 0x40BB_263D), (0x3E98_D5FA, 0x40BF_0B76), (negZero, 0x40CE_739D),
            (negZero, 0x4118_6186), (0x3E9F_B54B, 0x409C_18F8), (0x3ED9_DF52, 0x4108_2B94), (negZero, 0x4180_0000),
        ]
        for (i, (z, scale)) in buildings.enumerated() {
            let fit = models.buildings[i].fit
            #expect(bits(fit.position) == [negZero, negZero, z], "\(BUILDING_ASSETS[i]) position")
            #expect(bits(fit.rotation) == [0, 0, 0, 0x3F80_0000], "\(BUILDING_ASSETS[i]) rotation: none")
            #expect(bits(fit.scale) == [scale, scale, scale], "\(BUILDING_ASSETS[i]) scale")
        }

        // Spec 04 §10.2: the player car's fit matrix.
        #expect(
            bits(models.playerCar.local) == [
                0xBF01_042B, 0, 0x333D_3AC5, 0, 0, 0x3F01_042B, 0, 0, 0xB33D_3AC5, 0, 0xBF01_042B, 0,
                0xB47C_BEC7, 0xBEC9_C3EB, 0x3D66_B68F, 0x3F80_0000,
            ]
        )
    }

    @Test func theFitsHandleADegenerateFootprint() {
        // A model with no horizontal extent: the footprint clamps to EPSILON
        // instead of dividing by zero.
        let point = SIMD3<Float>(1, 2, 3)
        let car = fitTransform(min: point, max: point)
        #expect(car.scale.x == CAR_LEN / RustF32.epsilon)
        let building = buildingFit(min: point, max: point)
        #expect(building.scale.x == BUILDING_FOOTPRINT / RustF32.epsilon)
    }

    @Test func everyModelHasTheRustParts() throws {
        // Spec 03 §7.4: only the player car (and the road and skyline) has
        // textured parts, so the textured pass draws exactly its four; every
        // other part is on the vertex-colour pipeline. A re-exported asset that
        // changes this fails here.
        let context = try nullContext()
        let models = try loadDemoModels(context)
        func parts(_ model: Model, _ which: ModelParts) throws -> UInt64 {
            try drawsSubmitted(context) {
                _ = try recordedFrame(context) { try $0.draw(model, world: matrix_identity_float4x4, parts: which) }
            }
        }
        let expected: [(Model, UInt64, UInt64, String)] =
            [(models.road, 1, 0, "road"), (models.skyline, 1, 0, "skyline"), (models.playerCar.model, 4, 2, "sports_car")]
            + zip(models.trafficCars, [13, 12, 14] as [UInt64]).map { ($0.model, 0, $1, "traffic") }
            + zip(models.buildings, [6, 6, 5, 5, 5, 5, 4, 2] as [UInt64]).map { ($0.model, 0, $1, "building") }
        for (model, textured, untextured, name) in expected {
            #expect(try parts(model, .textured) == textured, "\(name) textured parts")
            #expect(try parts(model, .untextured) == untextured, "\(name) untextured parts")
            #expect(try parts(model, .all) == textured + untextured, "\(name) parts")
        }
        #expect(context.stats.drawsSkipped == 0)
    }

    @Test func pickModelFallsBackToTheFirst() throws {
        let context = try nullContext()
        let models = try loadDemoModels(context)
        #expect(pickModel(models.buildings, 7)?.model === models.buildings[7].model)
        #expect(pickModel(models.buildings, 8)?.model === models.buildings[0].model, "out of range falls back")
        #expect(pickModel(models.buildings, -1)?.model === models.buildings[0].model)
        #expect(pickModel([], 0) == nil, "nothing to pick from draws nothing")
        #expect(models.model(for: .player)?.model === models.playerCar.model)
        #expect(models.model(for: .traffic(2))?.model === models.trafficCars[2].model)
        #expect(models.model(for: .traffic(9))?.model === models.trafficCars[0].model)
        #expect(models.model(for: .building(3))?.model === models.buildings[3].model)
    }

    @Test func aMissingAssetIsAnAssetLoadError() throws {
        let context = try nullContext()
        do {
            _ = try DemoModels.load(context) { "/nonexistent/offramp/\($0)" }
            Issue.record("loading from a missing directory succeeded")
        } catch let error as AssetLoadError {
            // The first file in Rust's order is the road, a textured asset.
            #expect(error.path == "/nonexistent/offramp/road.gltf")
            #expect(error.description.hasPrefix("import textured asset /nonexistent/offramp/road.gltf: "))
            #expect((error.underlying as? StickShiftError)?.code == .io)
        }
    }
}
