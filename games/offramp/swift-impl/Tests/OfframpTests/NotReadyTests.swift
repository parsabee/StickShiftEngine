// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Foundation
import StickShift
import Testing
import simd

@testable import OfframpGame

/// A draw of a model that is still loading: the engine answers
/// `STICKSHIFT_NOT_READY`, draws nothing and counts a skipped draw, which is why
/// the game waits for every model in `initialize`, as Rust's synchronous load
/// does, and why the trace and the recorder check that no draw was skipped.
@MainActor
@Suite("not ready")
struct NotReadyTests {
    @Test func aPendingModelDrawsNothingAndCountsASkippedDraw() throws {
        let context = try nullContext()
        let pending = try context.startLoadingModel(contentsOf: URL(fileURLWithPath: demoAssetPath(PLAYER_CAR_ASSET)))
        // Loads finish only in the context's housekeeping, which nothing runs yet.
        try #require(pending.state == .pending)

        let record = try recordedFrame(context) { frame in
            let drawn = try frame.draw(pending, world: matrix_identity_float4x4)
            #expect(!drawn, "nothing to draw yet")
        }
        #expect(record.draws.isEmpty, "a refused draw is not recorded as drawn")
        #expect(context.stats.drawsSkipped == 1)
        #expect(context.stats.framesSubmitted == 1, "the frame itself went on")

        // Once waited for, it draws: what `DemoModels.load` does for every model.
        try pending.wait()
        #expect(pending.state == .ready)
        let drawn = try recordedFrame(context) { frame in
            let ok = try frame.draw(pending, world: matrix_identity_float4x4)
            #expect(ok)
        }
        #expect(drawn.draws.count == 6, "the player car's six parts")
    }

    @Test func theGamesModelsAreReadyBeforeItsFirstFrame() throws {
        // `initialize` waits for every load, so the first frame draws them all.
        let (game, h) = try booted()
        let models = try #require(game.resources).models
        for model in [models.road, models.skyline, models.playerCar.model] + models.trafficCars.map(\.model)
            + models.buildings.map(\.model)
        {
            #expect(model.state == .ready)
        }
        try h.run(frames: 1)
        #expect(h.context.stats.drawsSkipped == 0 && h.isClean)
        #expect(h.context.lastFrame.draws.count == 563, "probe \"draws 563\"")
    }
}
