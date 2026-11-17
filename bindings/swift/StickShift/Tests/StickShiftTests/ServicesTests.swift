// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import CStickShift
import Foundation
@testable import StickShift
import Testing
import simd

/// What a game's updates saw of a key.
@MainActor
final class KeyWatcher: Game {
    var presses = 0
    var releases = 0
    var held: [Bool] = []

    func initialize(_ ctx: EngineContext) throws {}

    func update(_ ctx: EngineContext, dt: Float) {
        if ctx.input.isKeyJustPressed(.space) { presses += 1 }
        if ctx.input.isKeyJustReleased(.space) { releases += 1 }
        held.append(ctx.input.isKeyDown(.space))
    }

    func render(_ ctx: EngineContext, into frame: Frame) throws {}
}

@Suite("The scene, camera, input, audio and graphics (null backend)")
@MainActor
struct ServicesTests {
    @Test func streamingFillsSlotsTracksAndDespawnsBehind() throws {
        let engine = try idleEngine()
        let scene = engine.scene
        let config = StreamingConfig(spawnInterval: 5, spawnAhead: 20, despawnBehind: 10, rebaseThreshold: 100)
        try scene.setConfig(config)
        #expect(scene.config == config)

        var slots: [(Int64, SIMD3<Float>)] = []
        scene.stream(focus: .zero) { slot in
            slots.append((slot.index, slot.position))
            slot.spawn(Transform(position: slot.position), StaticTag())
            // A second entity, spawned through the slot's world and tracked by hand.
            slot.track(slot.world.spawn(Transform(position: slot.position + [1, 0, 0])))
        }
        #expect(slots.map(\.0) == [0, 1, 2, 3, 4])
        #expect(slots.map(\.1) == [[0, 0, 0], [0, 0, -5], [0, 0, -10], [0, 0, -15], [0, 0, -20]])
        #expect(scene.streamedCount == 10 && engine.world.count == 10)

        // Far ahead: what fell behind is despawned, the frontier fills.
        slots = []
        scene.stream(focus: [0, 0, -22]) { slot in
            slots.append((slot.index, slot.position))
        }
        #expect(slots.map(\.0) == [5, 6, 7, 8])
        #expect(engine.world.count == 4 && scene.streamedCount == 4, "slots 0 to 2 fell more than 10 behind")

        // An axis change once streaming started is refused, and changes nothing.
        #expect(throws: StickShiftError.self) {
            try scene.setConfig(StreamingConfig(axis: [1, 0, 0], spawnInterval: 5))
        }
        #expect(scene.config == config)
        var reach = config
        reach.spawnAhead = 40
        try scene.setConfig(reach)
        #expect(scene.config.spawnAhead == 40)
    }

    @Test func theFloatingOriginRebasesEveryTransform() throws {
        let engine = try idleEngine()
        try engine.scene.setConfig(StreamingConfig(rebaseThreshold: 100))
        let entity = engine.world.spawn(Transform(position: [3, 0, -150]))
        #expect(engine.scene.maybeRebase(focus: [0, 0, -50]) == .zero)
        let offset = engine.scene.maybeRebase(focus: [0, 0, -150])
        #expect(offset != .zero)
        #expect(engine.world.get(Transform.self, of: entity)?.position == [3, 0, -150] + offset)
        #expect(engine.scene.despawn(entity) && !engine.scene.despawn(entity))
    }

    @Test func theDefaultsAreTheEngines() {
        var c = StickShiftStreamingConfig()
        stickshift_streaming_config_default(&c)
        #expect(StreamingConfig.default == StreamingConfig(c))
        #expect(StreamingConfig() == StreamingConfig.default)

        var chase = StickShiftChase()
        stickshift_chase_init(&chase, 8, 4)
        let swift = ChaseController(distance: 8, height: 4).c
        #expect(swift.distance == chase.distance && swift.height == chase.height)
        #expect(swift.look_at_height == chase.look_at_height && swift.half_life == chase.half_life)
        #expect(SIMD3(swift.up) == SIMD3(chase.up))

        #expect(Volume.unchanged.decibels == 0 && Volume.silent.decibels == -60)
        #expect(GraphicsSettings.default.renderSettings == RenderSettings.default)
    }

    @Test func theCameraIsTheEngines() throws {
        let camera = try idleEngine().camera
        camera.position = [1, 2, 3]
        camera.target = [0, 0, -5]
        camera.up = [0, 1, 0]
        camera.aspectRatio = 2
        camera.fieldOfView = 0.8
        camera.setClipPlanes(near: 0.5, far: 400)
        camera.setClipPlanes(near: 5, far: 1)  // ignored
        #expect(camera.position == [1, 2, 3] && camera.target == [0, 0, -5] && camera.up == [0, 1, 0])
        #expect(camera.aspectRatio == 2 && camera.fieldOfView == 0.8)
        #expect(camera.clipPlanes == (0.5, 400))
        #expect(bits(camera.viewProjection) == bits(StickShiftMath.multiply(camera.projection, camera.view)))
        #expect(abs(glamLength(camera.forward) - 1) < 1e-6 && abs(glamLength(camera.right) - 1) < 1e-6)

        // No smoothing: the camera snaps behind and above the target.
        camera.follow(ChaseController(distance: 8, height: 4), target: [0, 0, -1], forward: [0, 0, -1], dt: 1.0 / 60)
        #expect(camera.position == [0, 4, 7] && camera.target == [0, 0, -1])
    }

    @Test func theHostsEventsReachTheUpdatesAsEdges() throws {
        let game = KeyWatcher()
        let engine = try Engine.headless(game)
        try engine.initialize()
        // A tap between two frames, across a frame that runs no update.
        engine.input.press(.space)
        engine.input.release(.space)
        engine.tick(.elapsed(nanoseconds: 0))
        #expect(game.presses == 0)
        engine.advance()
        engine.advance()
        #expect(game.presses == 1 && game.releases == 1 && game.held == [false, false])

        // Held, with a repeat (no new edge), then focus loss (a release edge).
        engine.input.press(.space)
        engine.input.press(.space, isRepeat: true)
        engine.input.moveCursor(to: [10.5, 20.25])
        engine.input.pressMouse(.left)
        engine.advance()
        #expect(game.presses == 2 && engine.input.isKeyDown(.space) && !engine.input.isKeyJustPressed(.space))
        #expect(engine.input.cursorPosition == [10.5, 20.25] && engine.input.isMouseDown(.left))
        engine.input.releaseMouse(.left)
        #expect(!engine.input.isMouseDown(.left))
        engine.input.releaseAll()
        #expect(engine.input.isKeyJustReleased(.space))
        engine.advance()
        #expect(game.releases == 2 && game.held == [false, false, true, false])
    }

    @Test func theArrowKeysAreKeysNotMouseButtons() throws {
        // `Key` and `MouseButton` share `.left` and `.right`: the bare calls
        // press and release the keys, as a driver steering a car writes them.
        let engine = try idleEngine()
        engine.input.press(.left)
        engine.input.press(.right)
        #expect(engine.input.isKeyDown(.left) && engine.input.isKeyDown(.right))
        #expect(!engine.input.isMouseDown(.left) && !engine.input.isMouseDown(.right))
        engine.input.release(.left)
        engine.input.release(.right)
        #expect(!engine.input.isKeyDown(.left) && !engine.input.isKeyDown(.right))
        engine.input.pressMouse(.right)
        #expect(engine.input.isMouseDown(.right) && !engine.input.isKeyDown(.right))
        engine.input.releaseMouse(.right)
        #expect(!engine.input.isMouseDown(.right))
    }

    @Test func soundsAreCountedOnASilentEngine() throws {
        let audio = try idleEngine().audio
        let chime = audio.load("sounds/chime.wav")
        let music = audio.load("sounds/music.wav")
        #expect(audio.load("sounds/chime.wav") == chime, "a path loads once")
        #expect(audio.path(of: music) == "sounds/music.wav")
        audio.playOnce(chime, volume: Volume(decibels: -6))
        audio.playOnce(chime)
        audio.playLooping(music, volume: .silent)
        audio.playLooping(music)  // already looping: nothing starts
        audio.masterVolume = Volume(decibels: -12)
        var stats = audio.stats
        #expect(stats.soundCount == 2 && stats.decodeCount == 0 && stats.isSilent)
        #expect(stats.oneShotsPlayed == 2 && stats.loopStarts == 1 && stats.looping == music)
        #expect(stats.masterVolume == Volume(decibels: -12) && audio.masterVolume.decibels == -12)
        #expect(audio.oneShots(of: chime) == 2 && audio.oneShots(of: music) == 0)
        audio.stopLooping()
        stats = audio.stats
        #expect(stats.looping == nil)
    }

    @Test func graphicsRequestsApplyAtTheNextFrame() throws {
        var seen: [GraphicsSettings] = []
        final class Watcher: Game {
            let onSettings: (GraphicsSettings) -> Void
            init(_ onSettings: @escaping (GraphicsSettings) -> Void) { self.onSettings = onSettings }
            func initialize(_ ctx: EngineContext) throws {}
            func update(_ ctx: EngineContext, dt: Float) {}
            func render(_ ctx: EngineContext, into frame: Frame) throws {}
            func graphicsSettingsChanged(_ ctx: EngineContext, _ settings: GraphicsSettings) {
                #expect(ctx.graphics.current == settings)
                onSettings(settings)
            }
        }
        let engine = try Engine.headless(Watcher { seen.append($0) })
        try engine.initialize()
        let graphics = engine.graphics
        #expect(graphics.current == .default && graphics.pending == nil && graphics.revision == 0)
        #expect(graphics.baseSpawnAhead == StreamingConfig.default.spawnAhead)

        var near = GraphicsSettings.default
        near.drawDistance = .near
        graphics.request(near)
        #expect(graphics.pending == near && graphics.current == .default)
        engine.advance()
        #expect(graphics.current == near && graphics.pending == nil && graphics.revision == 1)
        #expect(seen == [near])
        #expect(engine.context.settings == near.renderSettings)
        #expect(engine.scene.config.spawnAhead < StreamingConfig.default.spawnAhead, "the reach follows the draw distance")
    }

    @Test func theMenuIsDataAndAChoiceIsAnEvent() throws {
        let engine = try idleEngine()
        let menu = engine.menu(fullScreen: false)
        #expect(menu.title == "Graphics" && menu.revision == 0)
        #expect(menu.groups.count == 5 && menu.groups.last?.title == nil)
        #expect(menu.groups.flatMap(\.items).filter(\.isChecked).count == 4)
        let drawDistance = try #require(menu.groups.first { $0.title == "Draw Distance" })
        let near = try #require(drawDistance.items.first { $0.title == "Near" })
        #expect(near.tag.rawValue == 301 && near.isEnabled && !near.isChecked)

        #expect(engine.push(.menu(near.tag)) == .syncMenu)
        #expect(engine.graphics.current.drawDistance == .near && engine.graphics.revision == 1)
        let after = engine.menu(fullScreen: true)
        #expect(after.revision == 1)
        #expect(after.groups.first { $0.title == "Draw Distance" }?.items.first { $0.isChecked }?.title == "Near")
        let actions = menu.groups.last!.items.map(\.title)
        let fullScreenActions = after.groups.last!.items.map(\.title)
        #expect(actions != fullScreenActions, "the full-screen item is titled for the state")
    }

    @Test func theLoadPhaseSeesTheStreamingConfig() async throws {
        final class Loader: Game {
            var streaming: StreamingConfig?
            func load(_ ctx: LoadContext) async throws { streaming = ctx.streaming }
            func initialize(_ ctx: EngineContext) throws {}
            func update(_ ctx: EngineContext, dt: Float) {}
            func render(_ ctx: EngineContext, into frame: Frame) throws {}
        }
        let game = Loader()
        try await Engine.headless(game).start()
        #expect(game.streaming == .default)
    }

    @Test func aResourceDroppedInAStreamClosureIsDestroyedAtOnce() throws {
        let context = try nullContext()
        let (vertices, indices) = triangle()
        var mesh: Mesh? = try context.makeMesh(layout: .positionNormalColor, vertices: vertices, indices: indices)
        let id = try #require(mesh?.id)
        func state() -> ResourceState { ResourceState(stickshift_resource_state(context.raw, id)) }
        let engine = try Engine(context: context, game: Scripted())
        try engine.initialize()
        // A stream pass's closure may make context calls: the destroy is
        // taken there, not sent for later.
        var stateInside: ResourceState?
        engine.scene.stream(focus: .zero) { _ in
            guard mesh != nil else { return }
            mesh = nil
            stateInside = state()
        }
        #expect(stateInside == .invalid)
    }
}
