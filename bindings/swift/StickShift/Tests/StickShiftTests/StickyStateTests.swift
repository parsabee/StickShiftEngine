// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import StickShift
import Testing
import simd

@MainActor
@Suite("A context's sticky state and last-frame record (null backend)")
struct StickyStateTests {
    @Test func theSunAndCameraCanBeSetOutsideAFrame() throws {
        let context = try nullContext()
        #expect(context.sunSky == .default)
        var sun = SunSky.default
        sun.elevation = 12
        try context.setSunSky(sun)
        #expect(context.sunSky == sun)

        let view = camera(aspect: 64.0 / 48.0)
        try context.setCamera(view)
        #expect(bits(context.camera.viewProjection) == bits(view.viewProjection))
        #expect(context.camera.position == view.position)
        // A frame set no camera of its own: it draws from the sticky one.
        try context.frame { _ in }
        #expect(context.lastFrame.camera == view)
        #expect(context.lastFrame.sunSky == sun)
    }

    @Test func theSafeAreaIsCheckedAndKeptInPixels() throws {
        let context = try nullContext()
        #expect(context.safeArea == .zero)
        try context.setSafeArea([4, 0, 2, 1])
        #expect(context.safeArea == [4, 0, 2, 1])
        let error = #expect(throws: StickShiftError.self) {
            try context.setSafeArea([0, -1, 0, 0])
        }
        #expect(error?.code == .invalidArgument)
        #expect(context.safeArea == [4, 0, 2, 1])
    }

    @Test func aNullContextResizesAndKeepsItsScale() throws {
        let context = try nullContext()
        try context.resize(width: 32, height: 16, scale: 2)
        #expect(context.size == (32, 16))
        #expect(context.scale == 2)
        try context.resize(width: 32, height: 16, scale: -1)
        #expect(context.scale == 1, "a scale that is not positive counts as 1")
    }

    @Test func theLastFrameRecordsWhatSurvivedSubmit() throws {
        let context = try nullContext()
        #expect(context.lastFrame.frameNumber == 0)
        #expect(context.lastFrame.draws.isEmpty)
        let (vertices, indices) = triangle()
        let kept = try context.makeMesh(layout: .positionNormalColor, vertices: vertices, indices: indices)
        var dropped: Mesh? = try context.makeMesh(layout: .positionNormalColor, vertices: vertices, indices: indices)
        let placed = simd_float4x4(translation: [1, 2, 3])
        let number = try context.frame { frame in
            _ = try frame.draw(dropped!, transform: placed)
            _ = try frame.draw(kept, transform: placed, material: Material(baseColor: [0.5, 0.25, 1, 1]))
            _ = try frame.draw(Quad(rect: [2, 3, 10, 4], color: [1, 0, 0, 1]))
            try frame.addLight(Light(kind: .point, position: [0, 1, 0], color: [1, 1, 1], intensity: 1, range: 5))
            dropped = nil
        }
        let record = context.lastFrame
        #expect(record.submitted)
        #expect(record.frameNumber == number)
        #expect(record.draws.count == 1, "the destroyed mesh's draw was dropped")
        #expect(record.draws.first?.source == kept.id)
        #expect(record.draws.first?.layout == .positionNormalColor)
        #expect(record.draws.first.map { bits($0.model) } == bits(placed))
        #expect(record.draws.first?.baseColor == [0.5, 0.25, 1, 1])
        #expect(record.quads.count == 1)
        #expect(record.quads.first?.rect == [2, 3, 10, 4])
        #expect(record.quads.first?.fill == .solid)
        #expect(record.lightCount == 1)
        #expect(record.settings == context.settings)
    }

    @Test func aLoadedModelWaitsAtOnce() async throws {
        let context = try nullContext()
        let model = try await context.loadModel(contentsOf: cubeFixture)
        try model.wait(timeout: .zero)
        #expect(model.state == .ready)
    }
}
