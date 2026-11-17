// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import StickShift
import Testing

/// A game that keeps what it must not: its init hook's context, and a stream
/// slot's world.
@MainActor
final class Hoarder: Game {
    var context: EngineContext?
    var slotWorld: World?
    var spawnInRender = false
    var sceneInFill = false

    func initialize(_ ctx: EngineContext) throws {
        context = ctx
        ctx.scene.stream(focus: .zero) { slot in
            slotWorld = slot.world
            if sceneInFill {
                _ = ctx.scene.streamedCount
            }
        }
    }

    func update(_ ctx: EngineContext, dt: Float) {}

    func render(_ ctx: EngineContext, into frame: Frame) throws {
        if spawnInRender {
            ctx.world.spawn(Transform())
        }
    }
}

/// A game component under another component's name, with another size.
struct Impostor: Component {
    static var componentName: String { "StickShiftTests.Impostor.name" }
    var value: Int64
}

struct SmallImpostor: Component {
    static var componentName: String { "StickShiftTests.Impostor.name" }
    var value: Int8
}

// Exit tests need Swift 6.2; the package builds with Swift 6.0.
#if compiler(>=6.2)
@Suite("ECS and service misuse traps (null backend)")
struct MisuseTests {
    @Test func aSpawnInRenderTraps() async {
        let result = await #expect(processExitsWith: .failure, observing: [\.standardErrorContent]) {
            await MainActor.run {
                let game = Hoarder()
                game.spawnInRender = true
                let engine = try! Engine.headless(game)
                try! engine.initialize()
                engine.advance()
            }
        }
        let stderr = String(decoding: result?.standardErrorContent ?? [], as: UTF8.self)
        #expect(stderr.contains("World.spawn(_:) failed: STICKSHIFT_ERROR_WRONG_STATE"), "\(stderr)")
    }

    @Test func aStaleContextsWorldTraps() async {
        let result = await #expect(processExitsWith: .failure, observing: [\.standardErrorContent]) {
            await MainActor.run {
                let game = Hoarder()
                let engine = try! Engine.headless(game)
                try! engine.initialize()
                game.context!.world.spawn(Transform())
            }
        }
        let stderr = String(decoding: result?.standardErrorContent ?? [], as: UTF8.self)
        #expect(stderr.contains("World.spawn(_:) was used after the game's initialize(_:) hook returned"), "\(stderr)")
    }

    @Test func aKeptStreamSlotsWorldTraps() async {
        let result = await #expect(processExitsWith: .failure, observing: [\.standardErrorContent]) {
            await MainActor.run {
                let game = Hoarder()
                let engine = try! Engine.headless(game)
                try! engine.initialize()
                _ = game.slotWorld!.count
            }
        }
        let stderr = String(decoding: result?.standardErrorContent ?? [], as: UTF8.self)
        #expect(
            stderr.contains("World.count was used after the Scene.stream(focus:_:) closure returned for its slot"),
            "\(stderr)"
        )
    }

    @Test func theSceneInAStreamClosureTraps() async {
        let result = await #expect(processExitsWith: .failure, observing: [\.standardErrorContent]) {
            await MainActor.run {
                let game = Hoarder()
                game.sceneInFill = true
                let engine = try! Engine.headless(game)
                try! engine.initialize()
            }
        }
        let stderr = String(decoding: result?.standardErrorContent ?? [], as: UTF8.self)
        #expect(stderr.contains("Scene.streamedCount failed: STICKSHIFT_ERROR_WRONG_STATE"), "\(stderr)")
    }

    @Test func aNameTakenWithAnotherSizeTraps() async {
        let result = await #expect(processExitsWith: .failure, observing: [\.standardErrorContent]) {
            await MainActor.run {
                let engine = try! Engine.headless(Hoarder())
                try! engine.initialize()
                let entity = engine.world.spawn(Impostor(value: 1))
                _ = engine.world.has(SmallImpostor.self, entity)
            }
        }
        let stderr = String(decoding: result?.standardErrorContent ?? [], as: UTF8.self)
        #expect(stderr.contains("registering the component StickShiftTests.Impostor.name (1 bytes) failed"), "\(stderr)")
    }

    @Test func aHostOnlyCallInAHookTraps() async {
        let result = await #expect(processExitsWith: .failure, observing: [\.standardErrorContent]) {
            await MainActor.run {
                // The engine's own graphics view, from inside an update.
                final class Peeker: Game {
                    var engine: Engine?
                    func initialize(_ ctx: EngineContext) throws {}
                    func update(_ ctx: EngineContext, dt: Float) { _ = engine!.graphics.revision }
                    func render(_ ctx: EngineContext, into frame: Frame) throws {}
                }
                let game = Peeker()
                let engine = try! Engine.headless(game)
                game.engine = engine
                try! engine.initialize()
                engine.advance()
            }
        }
        let stderr = String(decoding: result?.standardErrorContent ?? [], as: UTF8.self)
        #expect(stderr.contains("GraphicsControl.revision failed: STICKSHIFT_ERROR_WRONG_STATE"), "\(stderr)")
    }
}
#endif
