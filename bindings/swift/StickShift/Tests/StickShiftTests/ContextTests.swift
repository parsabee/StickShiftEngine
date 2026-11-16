// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import StickShift
import Testing

@MainActor
@Suite("Contexts, resources and frames (null backend)")
struct ContextTests {
    @Test func versionsAreReported() {
        #expect(Context.apiVersion >= 4)
        #expect(!Context.engineVersion.isEmpty)
    }

    @Test func aNullContextHasItsSizeAndNamesItsBackend() throws {
        let context = try nullContext()
        #expect(context.size == (64, 48))
        #expect(context.deviceName.contains("null"))
        let events = try context.pollEvents()
        #expect(events.contains { $0.kind == .contextReady })
    }

    @Test func meshesAreReadyAndFreedWithTheirLastReference() throws {
        let context = try nullContext()
        let (vertices, indices) = triangle()
        var mesh: Mesh? = try context.makeMesh(layout: .positionNormalColor, vertices: vertices, indices: indices)
        #expect(mesh?.state == .ready)
        #expect(context.stats.meshes == 1)
        mesh = nil
        #expect(context.stats.meshes == 0)
    }

    @Test func badInputThrowsTheEnginesCodeAndMessage() throws {
        let context = try nullContext()
        let error = #expect(throws: StickShiftError.self) {
            try context.makeMesh(layout: .positionNormalColor, vertices: [1, 2, 3, 4, 5], indices: [0, 1, 2])
        }
        #expect(error?.code == .invalidArgument)
        #expect(error?.message.isEmpty == false)
        #expect(error?.description.hasPrefix("STICKSHIFT_ERROR_INVALID_ARGUMENT") == true)
    }

    @Test func framesRecordSubmitAndNumberThemselves() throws {
        let context = try nullContext()
        let (vertices, indices) = triangle()
        let mesh = try context.makeMesh(layout: .positionNormalColor, vertices: vertices, indices: indices)
        let first = try context.frame { frame in
            try frame.setCamera(camera(aspect: 64.0 / 48.0))
            try frame.setSunSky(.default)
            let drewAtOrigin = try frame.draw(mesh)
            #expect(drewAtOrigin)
            let drewBeside = try frame.draw(mesh, transform: .init(translation: [1, 0, 0]))
            #expect(drewBeside)
        }
        #expect(first == 1)
        let second = try context.frame { _ in }
        #expect(second == 2)
        try context.flush()
        let stats = context.stats
        #expect(stats.framesSubmitted == 2)
        #expect(stats.framesCompleted == 2)
        #expect(stats.drawsSubmitted == 2)
        let gpuTime = try context.gpuFrameTime(1)
        #expect(gpuTime == .zero)
    }

    @Test func aThrowingFrameIsDiscarded() throws {
        struct Oops: Error {}
        let context = try nullContext()
        let (vertices, indices) = triangle()
        let mesh = try context.makeMesh(layout: .positionNormalColor, vertices: vertices, indices: indices)
        #expect(throws: Oops.self) {
            try context.frame { frame in
                try frame.draw(mesh)
                throw Oops()
            }
        }
        #expect(context.stats.framesSubmitted == 0)
        // The discarded frame is closed: the next one opens.
        let next = try context.frame { _ in }
        #expect(next == 1)
    }

    @Test func aFrameUsedAfterItsClosureThrowsWrongState() throws {
        let context = try nullContext()
        let (vertices, indices) = triangle()
        let mesh = try context.makeMesh(layout: .positionNormalColor, vertices: vertices, indices: indices)
        var escaped: Frame?
        try context.frame { escaped = $0 }
        let error = #expect(throws: StickShiftError.self) {
            try escaped?.draw(mesh)
        }
        #expect(error?.code == .wrongState)
    }

    @Test func lightsAreValidatedAndLimitedPerFrame() throws {
        let context = try nullContext()
        let lamp = Light.point(at: [0, 2, 0], color: [1, 0.9, 0.7], intensity: 10, range: 5)
        let headlight = Light.spot(
            at: [0, 0.6, 1], direction: [0, 0, -1], intensity: 40, range: 20, innerAngle: 12, outerAngle: 22
        )
        try context.frame { frame in
            try frame.addLight(lamp)
            try frame.addLight(headlight)
            var noRange = lamp
            noRange.range = 0
            #expect(throws: StickShiftError(code: .invalidArgument, message: lastMessage(noRange, frame))) {
                try frame.addLight(noRange)
            }
            for _ in 2..<Light.maxPerFrame {
                try frame.addLight(lamp)
            }
            let error = #expect(throws: StickShiftError.self) { try frame.addLight(lamp) }
            #expect(error?.code == .limitExceeded)
        }
        // The next frame starts with none.
        try context.frame { frame in
            for _ in 0..<Light.maxPerFrame {
                try frame.addLight(lamp)
            }
        }
    }

    /// The message the engine gives for `light`, from a failed add.
    private func lastMessage(_ light: Light, _ frame: Frame) -> String {
        do {
            try frame.addLight(light)
            return ""
        } catch let error as StickShiftError {
            return error.message
        } catch {
            return ""
        }
    }

    @Test func materialsCarryEmission() throws {
        let context = try nullContext()
        let (vertices, indices) = triangle()
        let mesh = try context.makeMesh(layout: .positionNormalColor, vertices: vertices, indices: indices)
        try context.frame { frame in
            let drewGlowing = try frame.draw(mesh, material: Material(emissive: [3, 2, 1]))
            #expect(drewGlowing)
            let error = #expect(throws: StickShiftError.self) {
                try frame.draw(mesh, material: Material(emissive: [0, -1, 0]))
            }
            #expect(error?.code == .invalidArgument)
        }
    }

    @Test func settingsRoundTripAndAreValidated() throws {
        let context = try nullContext()
        #expect(context.settings == .default)
        let low = RenderSettings(shadows: .low, shadowDistance: 0.5, drawDistanceScale: 1.5)
        try context.setSettings(low)
        #expect(context.settings == low)
        let error = #expect(throws: StickShiftError.self) {
            try context.setSettings(RenderSettings(shadows: .high, shadowDistance: 1, drawDistanceScale: 0))
        }
        #expect(error?.code == .invalidArgument)
        #expect(context.settings == low)
    }

    @Test func fontsNeedALinearAtlasAndKeepItAlive() throws {
        let context = try nullContext()
        let glyph = Glyph(uv: [0, 0, 1, 1], size: [0.5, 0.7], bearing: [0.05, 0.7], advance: 0.6)
        let srgb = try context.makeTexture(width: 2, height: 2, rgba8: [UInt8](repeating: 255, count: 16))
        let error = #expect(throws: StickShiftError.self) {
            try context.makeFont(atlas: srgb, firstCodepoint: 32, glyphs: [glyph], lineHeight: 1.2, fallback: glyph)
        }
        #expect(error?.code == .invalidArgument)

        var atlas: Texture? = try context.makeTexture(
            width: 2, height: 2, rgba8: [UInt8](repeating: 128, count: 16), colorSpace: .linear
        )
        let font = try context.makeFont(
            atlas: atlas!, firstCodepoint: 32, glyphs: Array(repeating: glyph, count: 95), lineHeight: 1.2,
            fallback: glyph
        )
        atlas = nil
        // The font holds its atlas, so text still draws.
        let width = try font.width(of: "Score 42", pixelSize: 20)
        #expect(width > 0)
        try context.frame { frame in
            let drewText = try frame.drawText("Score 42", font: font, at: [8, 30], pixelSize: 20)
            #expect(drewText)
            let drewBar = try frame.draw(Quad(rect: [2, 2, 20, 4], color: [1, 1, 1, 0.8]))
            #expect(drewBar)
            let drewTextured = try frame.draw(Quad(rect: [2, 10, 8, 8], fill: .texture(srgb)))
            #expect(drewTextured)
        }
        #expect(context.stats.textures == 2)
    }
}
