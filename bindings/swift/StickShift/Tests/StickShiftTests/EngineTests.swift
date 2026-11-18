// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import CStickShift
import Foundation
@testable import StickShift
import Testing

/// A game that records every hook the engine runs, and can be told to fail
/// or to ask for an exit.
@MainActor
final class Recorder: Game {
    struct Failure: Error, Equatable {
        let hook: String
    }

    var log: [String] = []
    var dts: [Float] = []
    var alphas: (update: [Float], render: [Float]) = ([], [])
    var settings: [GraphicsSettings] = []
    var surfaces: [Surface] = []
    var throwInInitialize = false
    var throwInRender = false
    var exitOnUpdate: Int?
    var exitSeen: [Bool] = []

    func initialize(_ ctx: EngineContext) throws {
        log.append("init")
        if throwInInitialize { throw Failure(hook: "initialize") }
    }

    func update(_ ctx: EngineContext, dt: Float) {
        log.append("update")
        dts.append(dt)
        alphas.update.append(ctx.alpha)
        if exitOnUpdate == dts.count { ctx.requestExit() }
        exitSeen.append(ctx.exitRequested)
    }

    func render(_ ctx: EngineContext, into frame: Frame) throws {
        log.append("render")
        alphas.render.append(ctx.alpha)
        surfaces.append(frame.surface)
        try frame.draw(Quad(rect: [0, 0, 10, 10], color: [1, 1, 1, 1]))
        if throwInRender { throw Failure(hook: "render") }
    }

    func graphicsSettingsChanged(_ ctx: EngineContext, _ settings: GraphicsSettings) {
        log.append("settings")
        self.settings.append(settings)
    }

    /// The log since the last call.
    func take() -> [String] {
        defer { log = [] }
        return log
    }
}

/// A game that keeps the context of its init hook, which it must not.
@MainActor
final class Keeper: Game {
    var kept: EngineContext?
    var keptLoad: LoadContext?

    func load(_ ctx: LoadContext) async throws {
        keptLoad = ctx
    }

    func initialize(_ ctx: EngineContext) throws {
        kept = ctx
    }

    func update(_ ctx: EngineContext, dt: Float) {}
    func render(_ ctx: EngineContext, into frame: Frame) throws {}
}

/// Where a game was deinitialized: on the main thread or not.
final class Witness: @unchecked Sendable {
    private let lock = NSLock()
    private var onMain: Bool?

    var value: Bool? { lock.withLock { onMain } }

    func record() {
        let main = Thread.isMainThread
        lock.withLock { onMain = main }
    }
}

/// A game that tells its witness where it died. The engine releases its game
/// when it is destroyed.
@MainActor
final class Mortal: Game {
    let witness: Witness

    init(witness: Witness) {
        self.witness = witness
    }

    deinit {
        witness.record()
    }

    func initialize(_ ctx: EngineContext) throws {}
    func update(_ ctx: EngineContext, dt: Float) {}
    func render(_ ctx: EngineContext, into frame: Frame) throws {}
}

/// Holds an engine so another thread can drop the last reference to it.
final class EngineHolder: @unchecked Sendable {
    var engine: Engine?

    init(_ engine: Engine) {
        self.engine = engine
    }

    /// Drop the engine here; returns whether here is the main thread.
    func drop() -> Bool {
        engine = nil
        return Thread.isMainThread
    }
}

/// A game with an asynchronous load phase.
@MainActor
final class Loader: Game {
    var loads = 0
    var mesh: Mesh?
    var meshAtInit: Mesh?

    func load(_ ctx: LoadContext) async throws {
        loads += 1
        let (vertices, indices) = triangle()
        mesh = try ctx.renderer.makeMesh(layout: .positionNormalColor, vertices: vertices, indices: indices)
    }

    func initialize(_ ctx: EngineContext) throws {
        meshAtInit = mesh
    }

    func update(_ ctx: EngineContext, dt: Float) {}

    func render(_ ctx: EngineContext, into frame: Frame) throws {
        if let mesh { try frame.draw(mesh) }
    }
}

/// A game that loads a model synchronously in init, as a Rust game does.
@MainActor
final class SyncLoader: Game {
    var model: Model?
    var stateAtInit: ResourceState?

    func initialize(_ ctx: EngineContext) throws {
        let model = try ctx.renderer.startLoadingModel(contentsOf: cubeFixture)
        try model.wait()
        stateAtInit = model.state
        self.model = model
    }

    func update(_ ctx: EngineContext, dt: Float) {}

    func render(_ ctx: EngineContext, into frame: Frame) throws {
        if let model { try frame.draw(model) }
    }
}

/// A game whose initialize starts loading a model and does not wait for it.
@MainActor
final class EagerLoader: Game {
    let url: URL
    var model: Model?
    var hooks: [String] = []

    init(url: URL) {
        self.url = url
    }

    func initialize(_ ctx: EngineContext) throws {
        model = try ctx.renderer.startLoadingModel(contentsOf: url)
    }

    func update(_ ctx: EngineContext, dt: Float) { hooks.append("update") }
    func render(_ ctx: EngineContext, into frame: Frame) throws { hooks.append("render") }
}

/// A named pipe the engine's model loader blocks on until `closeEmpty` opens it.
struct Fifo: ~Copyable {
    let url: URL

    init() throws {
        url = FileManager.default.temporaryDirectory
            .appendingPathComponent("stickshift-swift-\(UUID().uuidString).gltf")
        guard mkfifo(url.path, 0o600) == 0 else {
            throw StickShiftError(code: .io, message: "mkfifo \(url.path) failed: errno \(errno)")
        }
    }

    deinit {
        try? FileManager.default.removeItem(at: url)
    }

    /// Open the pipe's write end from another thread (opening blocks until the
    /// loader has the read end) and close it at once: the loader reads an empty file.
    func closeEmpty() {
        let path = url.path
        Thread.detachNewThread {
            let fd = open(path, O_WRONLY)
            if fd >= 0 { close(fd) }
        }
    }
}

let fixedStep = Engine.fixedStepNanoseconds

@MainActor
@Suite("The engine and its game hooks (null backend)")
struct EngineTests {
    @Test func aTickRunsUpdatesThenOneRender() throws {
        let game = Recorder()
        let engine = try Engine.headless(game)
        #expect(game.take().isEmpty, "creating an engine runs no hook")
        try engine.initialize()
        try engine.initialize()
        #expect(game.take() == ["init"], "initialize runs the hook once")

        for frame in 1...3 {
            let tick = engine.advance()
            #expect(tick.steps == 1)
            #expect(tick.rendered && tick.submitted)
            #expect(tick.frame == UInt64(frame))
            #expect(!tick.held && tick.requests.isEmpty)
            #expect(game.take() == ["update", "render"])
        }
        #expect(game.dts.allSatisfy { $0.bitPattern == 0x3C88_8889 }, "dt is the f32 nearest 1/60")
        #expect(Engine.fixedDeltaTime.bitPattern == 0x3C88_8889)
        #expect(game.alphas.update.allSatisfy { $0 == 0 })
        #expect(game.surfaces.last == Surface(width: 1280, height: 720, scale: 1))

        let state = engine.state
        #expect(state.isInitialized && !state.isHosted && !state.hasFailed)
        #expect(state.frames == 3 && state.updates == 3 && state.framesRendered == 3)
        #expect(state.framesDiscarded == 0)
        #expect(engine.perf.frameCount == 3)
        #expect(engine.lastRenderError == nil)

        let record = engine.context.lastFrame
        #expect(record.submitted && record.frameNumber == 3)
        #expect(record.quads.count == 1)
    }

    @Test func theElapsedClockBanksStepsAsTheRustLoopDoes() throws {
        let game = Recorder()
        let engine = try Engine.headless(game)
        try engine.initialize()
        _ = game.take()

        // No step banked: render still runs, once.
        #expect(engine.tick(.elapsed(nanoseconds: 0)).steps == 0)
        #expect(game.take() == ["render"])
        // Half a step, twice: the second tick completes one.
        #expect(engine.tick(.elapsed(nanoseconds: fixedStep / 2)).steps == 0)
        #expect(engine.tick(.elapsed(nanoseconds: fixedStep / 2 + 1)).steps == 1)
        _ = game.take()
        // Three steps run three updates, then one render.
        #expect(engine.tick(.elapsed(nanoseconds: 3 * fixedStep)).steps == 3)
        #expect(game.take() == ["update", "update", "update", "render"])
        // The spiral guard: at most five, the rest dropped.
        #expect(engine.tick(.elapsed(nanoseconds: 100 * fixedStep)).steps == Engine.maxStepsPerFrame)
        #expect(engine.tick(.elapsed(nanoseconds: 0)).steps == 0, "the dropped time is gone")

        // One and a half steps: render sees the half as its alpha.
        game.alphas = ([], [])
        #expect(engine.tick(.elapsed(nanoseconds: fixedStep + fixedStep / 2)).steps == 1)
        let alpha = try #require(game.alphas.render.last)
        #expect(abs(alpha - 0.5) < 1e-3)
        #expect(game.alphas.update == [0], "alpha is 0 outside render")
    }

    @Test func theTimestampClocksBankOneStepFirstAndAfterAReset() throws {
        let game = Recorder()
        let engine = try Engine.headless(game)
        try engine.initialize()

        let start: UInt64 = 5_000_000_000
        #expect(engine.tick(.timestamp(nanoseconds: start)).steps == 1, "the first tick banks one step")
        #expect(engine.tick(.timestamp(nanoseconds: start + 2 * fixedStep)).steps == 2)
        #expect(engine.tick(.timestamp(nanoseconds: start + 2 * fixedStep)).steps == 0)
        engine.resetClock()
        #expect(engine.tick(.timestamp(nanoseconds: start + 900 * fixedStep)).steps == 1, "after a reset, one step")

        engine.resetClock()
        #expect(engine.tick(.engine).steps == 1, "the engine's own clock banks one step first too")
    }

    @Test func aThrownInitializeIsRethrownAndFailsTheEngine() throws {
        let game = Recorder()
        game.throwInInitialize = true
        let engine = try Engine.headless(game)
        #expect(throws: Recorder.Failure(hook: "initialize")) { try engine.initialize() }
        #expect(engine.state.hasFailed)
        let again = #expect(throws: StickShiftError.self) { try engine.initialize() }
        #expect(again?.code == .wrongState)

        // resume runs the same init, and rethrows the same way.
        let failing = Recorder()
        failing.throwInInitialize = true
        let other = try Engine(context: nullContext(), game: failing)
        #expect(throws: Recorder.Failure(hook: "initialize")) {
            try other.resume(ViewMetrics(width: 64, height: 48, scaleFactor: 1))
        }
    }

    @Test func aThrownRenderStillSubmitsItsFrame() throws {
        let game = Recorder()
        game.throwInRender = true
        let engine = try Engine.headless(game)
        try engine.initialize()

        let tick = engine.advance()
        #expect(tick.rendered && tick.submitted)
        #expect(engine.lastRenderError as? Recorder.Failure == Recorder.Failure(hook: "render"))
        let record = engine.context.lastFrame
        #expect(record.submitted && record.frameNumber == tick.frame)
        #expect(record.quads.count == 1, "what render recorded before it threw is drawn")

        game.throwInRender = false
        engine.advance()
        #expect(engine.lastRenderError == nil)
    }

    @Test func anExitRequestReachesTheHostAndStopsAHeadlessRun() throws {
        let game = Recorder()
        game.exitOnUpdate = 3
        let engine = try Engine.headless(game)
        try engine.runHeadless(frames: 10)
        #expect(engine.state.frames == 3, "the run stops before the tick after the request")
        #expect(engine.state.exitRequested)
        #expect(game.exitSeen == [false, false, true])
        #expect(engine.advance().requests.contains(.exit), "every later tick says so")

        let host = try Engine.headless(Recorder())
        try host.initialize()
        host.requestExit()
        #expect(host.advance().requests == .exit)
    }

    @Test func hostEventsComeBackAsRequests() throws {
        let game = Recorder()
        let engine = try Engine(context: nullContext(), game: game, options: Engine.Options(quitsOnEscape: true))
        try engine.initialize()
        _ = game.take()

        #expect(engine.push(.keyDown(.space), .keyUp(.space), .cursor([3, 4]), .focusLost).isEmpty)
        #expect(engine.push(.keyDown(.escape)) == .exit)
        #expect(engine.push(.close) == .exit)

        // Shadows: Off, from the Graphics menu: applied at once, hook and all.
        #expect(engine.push(.menu(GraphicsMenu.Tag(rawValue: 101))) == .syncMenu)
        #expect(game.take() == ["settings"])
        #expect(game.settings == [GraphicsSettings(shadows: .off)])
        #expect(engine.advance().graphicsRevision == 1)

        let quiet = try Engine.headless(Recorder())
        try quiet.initialize()
        #expect(quiet.push(.keyDown(.escape)).isEmpty, "Escape quits only with quitsOnEscape")
    }

    @Test func resumeHostsTheEngineAndSizesTheDrawable() throws {
        let game = Recorder()
        let context = try nullContext()
        let engine = try Engine(context: context, game: game)
        let requests = try engine.resume(ViewMetrics(width: 1600, height: 1200, scaleFactor: 2))
        #expect(requests == .syncMenu)
        #expect(game.take() == ["init"])
        #expect(engine.state.isHosted && engine.state.isInitialized)
        // Half render scale: half the view's points per side, at its pixels per point.
        #expect(context.size == (800, 600))

        try engine.resume(ViewMetrics(width: 1600, height: 1200, scaleFactor: 2))
        #expect(game.take().isEmpty, "a second resume does not initialize again")
        #expect(engine.tick(.engine).steps == 1)

        #expect(engine.push(.viewResized(ViewMetrics(width: 800, height: 400, scaleFactor: 1))) == .syncMenu)
        #expect(context.size == (400, 200))
    }

    @Test func loadRunsOnceBeforeInitialize() async throws {
        let game = Loader()
        let engine = try Engine.headless(game)
        try await engine.start()
        try await engine.load()
        #expect(game.loads == 1)
        #expect(game.meshAtInit != nil, "load ran before initialize")
        engine.advance()
        #expect(engine.context.lastFrame.draws.count == 1)
    }

    @Test func initializeCanLoadAModelSynchronously() throws {
        let game = SyncLoader()
        let engine = try Engine.headless(game)
        try engine.initialize()
        #expect(game.stateAtInit == .ready)
        engine.advance()
        #expect(engine.context.lastFrame.draws.count == 1, "drawn from the first frame on")
    }

    @Test func aContextTakesOneEngine() throws {
        let context = try nullContext()
        let first = try Engine(context: context, game: Recorder())
        let second = #expect(throws: StickShiftError.self) { try Engine(context: context, game: Recorder()) }
        #expect(second?.code == .wrongState)
        _ = first
    }

    @Test func aSettingsSuiteNeedsPersistence() throws {
        let error = #expect(throws: StickShiftError.self) {
            try Engine(context: nullContext(), game: Recorder(), options: Engine.Options(settingsSuite: "unused"))
        }
        #expect(error?.code == .invalidArgument)
    }

    @Test func anOlderLibraryIsRefused() throws {
        try Engine.requireLibrary(minor: UInt32(STICKSHIFT_API_MINOR))
        let error = #expect(throws: StickShiftError.self) {
            try Engine.requireLibrary(minor: UInt32(STICKSHIFT_API_MINOR) - 1)
        }
        #expect(error?.code == .unsupported)
        #expect(Engine.libraryAPIMinor >= UInt32(STICKSHIFT_API_MINOR))
    }

    // Synchronous, so no other main-actor test makes an engine while the
    // library's minor is swapped.
    @Test func anEngineIsNotCreatedOnAnOlderLibrary() throws {
        let context = try nullContext()
        let linked = Engine.linkedAPIMinor
        defer { Engine.linkedAPIMinor = linked }
        Engine.linkedAPIMinor = { UInt32(STICKSHIFT_API_MINOR) - 1 }
        weak var released: Recorder?
        do {
            let game = Recorder()
            released = game
            let error = #expect(throws: StickShiftError.self) { try Engine(context: context, game: game) }
            #expect(error?.code == .unsupported)
        }
        #expect(released == nil, "the refused engine's game was released")
        Engine.linkedAPIMinor = linked
        _ = try Engine(context: context, game: Recorder())
    }

    @Test func optionsCrossAsTheirCValues() {
        let headless = Engine.Options.headless.c
        #expect(headless.api_version == UInt32(STICKSHIFT_API_VERSION))
        #expect(headless.size == UInt32(MemoryLayout<StickShiftEngineDesc>.size))
        #expect(headless.flags == 0 && headless.settings_suite == nil)
        #expect(headless.audio == StickShiftAudioOutput(STICKSHIFT_AUDIO_SILENT))
        #expect(headless.pacing == StickShiftFramePacing(STICKSHIFT_FRAME_PACING_WAIT))

        #expect(Engine.Options(audio: .device).c.audio == StickShiftAudioOutput(STICKSHIFT_AUDIO_DEVICE))
        #expect(Engine.Options(framePacing: .skip).c.pacing == StickShiftFramePacing(STICKSHIFT_FRAME_PACING_SKIP))
        #expect(Engine.Options(persistsSettings: true).c.flags == UInt32(STICKSHIFT_ENGINE_PERSIST_SETTINGS))
        #expect(Engine.Options(quitsOnEscape: true).c.flags == UInt32(STICKSHIFT_ENGINE_QUIT_ON_ESCAPE))
        #expect(Engine.Options(holdsUntilLoaded: true).c.flags == UInt32(STICKSHIFT_ENGINE_HOLD_UNTIL_LOADED))
        let app = Engine.Options.app.c
        #expect(app.audio == StickShiftAudioOutput(STICKSHIFT_AUDIO_DEVICE))
        #expect(app.flags == UInt32(STICKSHIFT_ENGINE_PERSIST_SETTINGS | STICKSHIFT_ENGINE_QUIT_ON_ESCAPE))
    }

    @Test func holdsUntilLoadedRunsNoHookWhileAModelLoads() throws {
        let fifo = try Fifo()
        let game = EagerLoader(url: fifo.url)
        let engine = try Engine(context: nullContext(), game: game, options: Engine.Options(holdsUntilLoaded: true))
        try engine.initialize()
        // The loader is blocked on the empty pipe, so the model is still loading.
        for _ in 0..<3 {
            let tick = engine.advance()
            #expect(tick.held && tick.steps == 0 && !tick.rendered && !tick.submitted)
            #expect(engine.state.isHolding)
        }
        #expect(game.hooks.isEmpty)

        // Close the pipe empty. The load fails (glTF import cannot read from a
        // pipe anyway), and a load that failed ends the hold as one that finished does.
        fifo.closeEmpty()
        let model = try #require(game.model)
        #expect(throws: StickShiftError.self) { try model.wait() }
        let tick = engine.advance()
        #expect(!tick.held && tick.steps == 1 && tick.rendered)
        #expect(!engine.state.isHolding)
        #expect(game.hooks == ["update", "render"])
    }

    @Test func persistedSettingsComeBackInTheNextEngine() throws {
        let suite = "dev.stickshift.swift-tests.\(UUID().uuidString)"
        defer { UserDefaults(suiteName: suite)?.removePersistentDomain(forName: suite) }
        let options = Engine.Options(persistsSettings: true, settingsSuite: suite)

        let first = try Engine(context: nullContext(), game: Recorder(), options: options)
        try first.initialize()
        // Shadows: Off, from the Graphics menu: saved at once.
        first.push(.menu(GraphicsMenu.Tag(rawValue: 101)))

        let context = try nullContext()
        let second = try Engine(context: context, game: Recorder(), options: options)
        try second.initialize()
        #expect(context.settings.shadows == .off, "the next engine restored the saved choice")

        let unsaved = try nullContext()
        let third = try Engine(context: unsaved, game: Recorder())
        try third.initialize()
        #expect(unsaved.settings.shadows == .high, "an engine that does not persist starts from the defaults")
    }

    @Test func aTickReadsItsFrameBack() throws {
        let engine = try Engine(context: nullContext(width: 8, height: 4), game: Recorder())
        try engine.initialize()
        let pixels = PixelBuffer(width: 8, height: 4)
        let tick = engine.advance(readback: pixels)
        #expect(tick.readbackReady)
        #expect(pixels.bgra.count == 8 * 4 * 4)
        #expect(engine.advance().readbackReady == false)
    }

    @Test func aKeptContextKeepsNothingAlive() throws {
        weak var released: Keeper?
        do {
            let game = Keeper()
            released = game
            let engine = try Engine.headless(game)
            try engine.initialize()
            #expect(game.kept != nil)
        }
        #expect(released == nil, "the game kept its init context, and was released with the engine all the same")
    }

    @Test func aMetalTickReadsItsFrameBack() throws {
        guard let context = try metalContext(width: 32, height: 16) else { return }
        let engine = try Engine(context: context, game: Recorder())
        try engine.initialize()
        let pixels = PixelBuffer(width: 32, height: 16)
        let tick = engine.advance(readback: pixels)
        #expect(tick.submitted && tick.readbackReady, "WAIT pacing: the frame is read back before the tick returns")
        // The recorder's opaque white 10 × 10 quad, at the top-left, over the sky.
        func pixel(_ x: Int, _ y: Int, _ bytes: [UInt8]) -> [UInt8] {
            Array(bytes[(y * 32 + x) * 4..<(y * 32 + x) * 4 + 4])
        }
        let bgra = pixels.bgra
        #expect(pixel(5, 5, bgra) == [255, 255, 255, 255])
        let sky = pixel(20, 8, bgra)
        #expect(sky != [255, 255, 255, 255])
        #expect(pixel(20, 8, pixels.rgba()) == [sky[2], sky[1], sky[0], sky[3]])
    }

    @Test func droppingTheEngineOffTheMainThreadDestroysItOnMain() async throws {
        let witness = Witness()
        let context = try nullContext()
        let holder = EngineHolder(try Engine(context: context, game: Mortal(witness: witness)))
        try holder.engine?.initialize()

        // Stand in for the dropping thread being preempted right after it sends
        // the destroy: it waits (up to two seconds) for the main thread to run it.
        // Its own release of the engine's storage then comes last; the game must
        // already be gone.
        holder.engine?.afterSendingDestroy = { [witness] in
            for _ in 0..<1000 where witness.value == nil {
                usleep(2000)
            }
        }

        let droppedOnMain = await Task.detached { holder.drop() }.value
        #expect(!droppedOnMain)
        for _ in 0..<1000 where witness.value == nil {
            try await Task.sleep(for: .milliseconds(2))
        }
        #expect(witness.value == true, "the engine was destroyed, and its game released, on the main thread")
        // The engine was destroyed, not leaked: the context takes another.
        _ = try Engine(context: context, game: Recorder())
    }

    // Exit tests need Swift 6.2; the package builds with Swift 6.0.
    #if compiler(>=6.2)
    @Test func aStaleEngineContextTraps() async {
        let result = await #expect(processExitsWith: .failure, observing: [\.standardErrorContent]) {
            await MainActor.run {
                let game = Keeper()
                let engine = try! Engine.headless(game)
                try! engine.initialize()
                // The init hook's context, after the hook returned.
                game.kept!.requestExit()
            }
        }
        let stderr = String(decoding: result?.standardErrorContent ?? [], as: UTF8.self)
        #expect(stderr.contains("EngineContext.requestExit() was used after the game's initialize(_:) hook returned"))
    }

    @Test func aReadBackNeedsWaitPacing() async {
        let result = await #expect(processExitsWith: .failure, observing: [\.standardErrorContent]) {
            await MainActor.run {
                let engine = try! Engine(
                    context: nullContext(width: 8, height: 4),
                    game: Recorder(),
                    options: Engine.Options(framePacing: .skip)
                )
                try! engine.initialize()
                engine.advance(readback: PixelBuffer(width: 8, height: 4))
            }
        }
        let stderr = String(decoding: result?.standardErrorContent ?? [], as: UTF8.self)
        #expect(stderr.contains("a tick read-back needs an offscreen context and STICKSHIFT_FRAME_PACING_WAIT"))
    }

    @Test func aStaleLoadContextTraps() async {
        let result = await #expect(processExitsWith: .failure, observing: [\.standardErrorContent]) {
            await Task { @MainActor in
                let game = Keeper()
                let engine = try! Engine.headless(game)
                try! await engine.load()
                // The load phase's context, after load returned.
                _ = game.keptLoad!.renderer
            }.value
        }
        let stderr = String(decoding: result?.standardErrorContent ?? [], as: UTF8.self)
        #expect(stderr.contains("LoadContext.renderer was used after the game's load(_:) hook returned"))
    }
    #endif

    @Test func hostEventsCrossAsTheirCValues() {
        #expect(Key.allCases.map(\.rawValue) == [
            STICKSHIFT_KEY_W, STICKSHIFT_KEY_A, STICKSHIFT_KEY_S, STICKSHIFT_KEY_D, STICKSHIFT_KEY_Q,
            STICKSHIFT_KEY_E, STICKSHIFT_KEY_LEFT, STICKSHIFT_KEY_RIGHT, STICKSHIFT_KEY_UP,
            STICKSHIFT_KEY_DOWN, STICKSHIFT_KEY_SPACE, STICKSHIFT_KEY_ESCAPE,
        ].map(UInt32.init))
        #expect(MouseButton.allCases.map(\.rawValue) == [
            STICKSHIFT_MOUSE_LEFT, STICKSHIFT_MOUSE_RIGHT, STICKSHIFT_MOUSE_MIDDLE,
        ].map(UInt32.init))

        let press = HostEvent.keyDown(.d, isRepeat: true).c
        #expect(press.kind == UInt32(STICKSHIFT_HOST_KEY_DOWN))
        #expect(press.code == UInt32(STICKSHIFT_KEY_D))
        #expect(press.flags == UInt32(STICKSHIFT_HOST_EVENT_REPEAT))
        let resize = HostEvent.scaleChanged(ViewMetrics(width: 640, height: 480, scaleFactor: 2)).c
        #expect(resize.kind == UInt32(STICKSHIFT_HOST_SCALE_CHANGED))
        #expect(resize.width == 640 && resize.height == 480 && resize.scale_factor == 2)
        let cursor = HostEvent.cursor([1.5, -2]).c
        #expect(cursor.kind == UInt32(STICKSHIFT_HOST_CURSOR) && cursor.x == 1.5 && cursor.y == -2)
        let insets = HostEvent.safeArea([1, 2, 3, 4]).c
        #expect(insets.kind == UInt32(STICKSHIFT_HOST_SAFE_AREA))
        #expect(insets.insets.0 == 1 && insets.insets.1 == 2 && insets.insets.2 == 3 && insets.insets.3 == 4)
        #expect(MemoryLayout<StickShiftHostEvent>.stride == 64)

        for settings in [GraphicsSettings.default, GraphicsSettings(shadows: .low, shadowDistance: .near,
                                                                    drawDistance: .medium, renderScale: .full)] {
            #expect(GraphicsSettings(settings.c) == settings)
        }
        #expect(GraphicsSettings.default.c.shadows == UInt32(STICKSHIFT_SHADOW_QUALITY_HIGH))
        #expect(GraphicsSettings.default.c.render_scale == UInt32(STICKSHIFT_RENDER_SCALE_HALF))
        #expect(GraphicsSettings.default.c.draw_distance == UInt32(STICKSHIFT_DRAW_DISTANCE_FAR))
    }
}
