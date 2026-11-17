// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Foundation
import StickShift
import Testing
import simd

/// The services probe of `crates/stickshift-capi/tests/engine_services.rs`,
/// written as a Swift game and host: chase follow, key edges (taps between
/// frames, a repeat, focus loss, zero-step frames), silent audio counters,
/// mid-run settings requests and menu choices. Its lines must be the Rust
/// probe's (Tests/Fixtures/probe-services-*.txt), unhosted and hosted.
@MainActor
final class ServicesProbe: Game {
    enum At {
        case initialize, update, render, settings
    }

    static let paths = ["probe/engine.ogg", "probe/music.ogg", "probe/engine.ogg", "probe/chime.wav"]
    static let chase = ChaseController(distance: 7.5, height: 3, lookAtHeight: 1.25, up: [0.3, 2, -0.2], halfLife: 0.15)

    let log: ProbeLog
    var sounds: [Sound] = []
    var updates = 0
    var renders = 0
    var settings = 0

    init(log: ProbeLog) {
        self.log = log
    }

    // MARK: What a line shows

    static func settingsLine(_ s: GraphicsSettings) -> String {
        rust([s.shadows.rawValue, s.shadowDistance.rawValue, s.drawDistance.rawValue, s.renderScale.rawValue])
    }

    static func cameraLine(_ c: EngineCamera) -> String {
        let (near, far) = c.clipPlanes
        return "p\(rust(c.position)) t\(rust(c.target)) u\(rust(c.up)) fov \(rust(c.fieldOfView)) "
            + "a \(rust(c.aspectRatio)) \(rust(near))..\(rust(far)) v\(matrixHash(c.view)) "
            + "p\(matrixHash(c.projection)) vp\(matrixHash(c.viewProjection)) f\(rust(c.forward)) r\(rust(c.right))"
    }

    static func inputLine(
        isDown: (Key) -> Bool, justPressed: (Key) -> Bool, justReleased: (Key) -> Bool,
        mouse: (MouseButton) -> Bool, cursor: SIMD2<Double>?
    ) -> String {
        let keys = Key.allCases.map { k in
            (isDown(k) ? 1 : 0) | (justPressed(k) ? 2 : 0) | (justReleased(k) ? 4 : 0)
        }
        let buttons = MouseButton.allCases.map { mouse($0) ? 1 : 0 }
        return "\(rust(keys)) \(rust(buttons)) \(rust(cursor.map { "(\(rust($0.x)), \(rust($0.y)))" }))"
    }

    static func inputLine(_ i: Input) -> String {
        inputLine(
            isDown: i.isKeyDown, justPressed: i.isKeyJustPressed, justReleased: i.isKeyJustReleased,
            mouse: i.isMouseDown, cursor: i.cursorPosition
        )
    }

    static func inputLine(_ i: HostInput) -> String {
        inputLine(
            isDown: i.isKeyDown, justPressed: i.isKeyJustPressed, justReleased: i.isKeyJustReleased,
            mouse: i.isMouseDown, cursor: i.cursorPosition
        )
    }

    static func audioLine(_ audio: Audio, _ sounds: [Sound]) -> String {
        let s = audio.stats
        let looping = s.looping.flatMap { sounds.firstIndex(of: $0) }.map { "\($0)" }
        return "\(s.soundCount) \(s.decodeCount) \(s.loopStarts) \(s.oneShotsPlayed) \(rust(s.masterVolume.decibels)) "
            + "\(rust(looping)) \(s.isSilent) \(rust(sounds.map { audio.oneShots(of: $0) }))"
    }

    /// What the context holds, read through its getters (the first pushes a
    /// moved camera inside a hook).
    static func rendererLine(_ context: Context) -> String {
        let settings = context.settings
        let size = context.size
        let scale = context.scale
        let camera = context.camera
        return "\(settings.shadows.rawValue) \(rust(settings.shadowDistance)) \(rust(settings.drawDistanceScale)) "
            + "size=\(size.width)x\(size.height) \(rust(scale)) vp\(matrixHash(camera.viewProjection)) "
            + "e\(rust(camera.position))"
    }

    // MARK: The hooks

    func run(_ ctx: EngineContext, _ at: At, _ n: Int) {
        let camera = ctx.camera
        let audio = ctx.audio
        let graphics = ctx.graphics
        switch at {
        case .initialize:
            camera.setClipPlanes(near: 0.5, far: 800)
            // Ignored, as in Rust: far is not beyond near.
            camera.setClipPlanes(near: 5, far: 1)
            camera.fieldOfView = 1
            camera.position = [0, 4, 9]
            camera.target = [0, 0, -5]
            for path in Self.paths {
                sounds.append(audio.load(path))
            }
            log.add(
                "I cam=\(Self.cameraLine(camera)) aud=\(Self.audioLine(audio, sounds)) "
                    + "gfx=\(Self.settingsLine(graphics.current)) reach=\(rust(ctx.scene.config.spawnAhead))"
            )
        case .update:
            let t = Float(n)
            let target: SIMD3<Float> = [sin(t * 0.01) * 3, 0, -0.2 * t]
            let forward: SIMD3<Float> = [cos(t * 0.01) * 0.1, 0, -1]
            camera.follow(Self.chase, target: target, forward: forward, dt: Engine.fixedDeltaTime)
            if n % 120 == 1 {
                audio.playOnce(sounds[0], volume: Volume(decibels: -6))
            }
            if n % 500 == 10 || n % 500 == 11 {
                // The second asks for the sound already looping: nothing starts.
                audio.playLooping(sounds[1], volume: Volume(decibels: -3))
            }
            if n % 500 == 250 {
                audio.stopLooping()
            }
            if n == 700 {
                // The same sound by its second load: one loop start.
                audio.playLooping(sounds[0], volume: .unchanged)
                audio.playLooping(sounds[2], volume: .unchanged)
            }
            if n == 900 {
                audio.masterVolume = Volume(decibels: -12)
            }
            if n % 333 == 7 {
                audio.playOnce(sounds[3])
            }
            if n == 300 || n == 301 {
                var s = graphics.current
                s.drawDistance = .near
                graphics.request(s)
            }
            if n == 600 {
                // Equal to the settings in effect: no change, no hook.
                graphics.request(graphics.current)
            }
            if n == 1500 {
                var s = graphics.current
                s.shadows = .low
                s.renderScale = .threeQuarters
                graphics.request(s)
            }
            log.add("U\(n) in=\(Self.inputLine(ctx.input)) cam=\(Self.cameraLine(camera))")
        case .render:
            if n % 3 == 0 {
                camera.follow(Self.chase, target: [0, 0, -0.2 * Float(n)], forward: [0, 0, -1], dt: 0)
            }
            if n == 800 {
                var s = graphics.current
                s.shadows = .low
                graphics.request(s)
            }
            if n == 2100 {
                // Still pending when the host's menu choice arrives: the
                // choice composes on it.
                var s = graphics.current
                s.drawDistance = .near
                graphics.request(s)
            }
            let renderer = n % 7 == 0 ? Self.rendererLine(ctx.renderer) : ""
            log.add(
                "R\(n) cam=\(Self.cameraLine(camera)) aud=\(Self.audioLine(audio, sounds)) "
                    + "gfx=\(Self.settingsLine(graphics.current)) rnd=\(renderer)"
            )
        case .settings:
            log.add(
                "S\(n) gfx=\(Self.settingsLine(graphics.current)) reach=\(rust(ctx.scene.config.spawnAhead)) "
                    + "rnd=\(Self.rendererLine(ctx.renderer))"
            )
        }
    }

    func initialize(_ ctx: EngineContext) throws {
        run(ctx, .initialize, 0)
    }

    func update(_ ctx: EngineContext, dt: Float) {
        updates += 1
        run(ctx, .update, updates)
    }

    func render(_ ctx: EngineContext, into frame: Frame) throws {
        renders += 1
        run(ctx, .render, renders)
    }

    func graphicsSettingsChanged(_ ctx: EngineContext, _ settings: GraphicsSettings) {
        self.settings += 1
        log.add("S arg=\(Self.settingsLine(settings))")
        run(ctx, .settings, self.settings)
    }

    // MARK: The host

    /// A host action between frames.
    enum HostAction {
        case event(HostEvent)
        /// A request from the host between engine calls, on the settings in effect.
        case request((GraphicsSettings) -> GraphicsSettings)
    }

    static func menu(_ tag: UInt32) -> HostAction {
        .event(.menu(GraphicsMenu.Tag(rawValue: tag)))
    }

    /// The host's events before frame `f`.
    static func hostActions(_ f: Int) -> [HostAction] {
        var v: [HostAction] = []
        if f % 37 == 5 {
            // A tap between two frames: both edges reach the next update.
            v += [.event(.keyDown(.space)), .event(.keyUp(.space))]
        }
        switch f % 53 {
        case 10: v.append(.event(.keyDown(.w)))
        // Repeats of a held key make no edge, flagged or not.
        case 11: v.append(.event(.keyDown(.w, isRepeat: true)))
        case 12: v.append(.event(.keyDown(.w)))
        case 30: v.append(.event(.keyUp(.w)))
        default: break
        }
        switch f % 71 {
        case 20: v += [.event(.keyDown(.left)), .event(.keyDown(.a)), .event(.mouseDown(.left))]
        // Focus loss releases everything held, with edges.
        case 23: v.append(.event(.focusLost))
        default: break
        }
        switch f % 89 {
        case 40: v.append(.event(.mouseDown(.right)))
        case 41: v.append(.event(.mouseUp(.right)))
        default: break
        }
        if f % 41 == 0 {
            v.append(.event(.cursor([Double(f) * 0.5, Double(f) * 0.25 + 0.125])))
        }
        switch f {
        case 400: v.append(menu(302))
        case 700: v.append(menu(901))
        case 1000:
            // A host request still pending, then a menu choice: the choice
            // composes on it, and both apply now.
            v.append(
                .request { s in
                    var s = s
                    s.renderScale = .full
                    return s
                })
            v.append(menu(101))
        case 1300: v.append(menu(202))
        case 1600: v.append(menu(902))
        // The render hook of the frame before requested a change.
        case 2100: v.append(menu(403))
        // Two choices in one batch: each applies at once.
        case 2500: v += [menu(102), menu(303)]
        default: break
        }
        return v
    }

    /// Fixed steps of elapsed time before frame `f`: zero-step frames, several
    /// steps, and more than the cap.
    static func elapsed(_ f: Int) -> UInt64 {
        let steps: [UInt64] = [1, 0, 2, 1, 1, 3, 0, 1, 7, 1]
        return steps[f % 10] * Engine.fixedStepNanoseconds
    }

    /// Run the probe for `frames` frames, as the C probe's host does.
    static func run(hosted: Bool, frames: Int, log: ProbeLog) throws {
        let game = ServicesProbe(log: log)
        let engine = try Engine.headless(game)
        if hosted {
            let requests = try engine.resume(ViewMetrics(width: 1600, height: 1200, scaleFactor: 2))
            log.add("resume \(requests.rawValue)")
        } else {
            try engine.initialize()
        }
        for f in 0..<frames {
            var events: UInt32 = 0
            var batch: [HostEvent] = []
            for action in hostActions(f) {
                switch action {
                case let .event(event):
                    batch.append(event)
                case let .request(map):
                    events |= engine.push(contentsOf: batch).rawValue
                    batch = []
                    engine.graphics.request(map(engine.graphics.current))
                }
            }
            events |= engine.push(contentsOf: batch).rawValue
            let tick = engine.tick(.elapsed(nanoseconds: elapsed(f)))

            let g = engine.graphics
            let gfx = "\(settingsLine(g.current)) \(rust(g.pending.map(settingsLine))) \(g.revision) "
                + rust(g.baseSpawnAhead.map { rust($0) })
            #expect(tick.graphicsRevision == g.revision)
            log.add(
                "F\(f) ev=\(events) tick=\(tick.steps) \(tick.requests.rawValue) gfx=\(gfx) "
                    + "cam=\(cameraLine(engine.camera)) in=\(inputLine(engine.input)) "
                    + "aud=\(audioLine(engine.audio, game.sounds)) rnd=\(rendererLine(engine.context))"
            )
        }
    }
}

@Suite("The probe differential, engine services (null backend)")
@MainActor
struct ServicesProbeTests {
    @Test(arguments: [false, true])
    func theSwiftProbePrintsTheRustProbesLines(hosted: Bool) throws {
        let log = try ProbeLog(hosted ? "services-hosted" : "services-unhosted", keeping: ["S arg=", "F2999 "])
        try ServicesProbe.run(hosted: hosted, frames: 3000, log: log)
        log.finish()
        // What the run went through, so a probe that stopped exercising
        // something would fail here rather than pass vacuously.
        #expect(log.kept.filter { $0.hasPrefix("S arg=") }.count == 9)
        #expect(log.kept.last?.contains(" aud=3 0 ") == true, "\(log.kept.last ?? "")")
    }
}
