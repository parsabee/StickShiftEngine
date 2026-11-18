// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Foundation
import StickShift
import Testing

@MainActor
@Suite("Model loading (null backend)")
struct ModelTests {
    @Test func aModelLoadsWithoutARenderLoop() async throws {
        let context = try nullContext()
        let model = try await context.loadModel(contentsOf: cubeFixture)
        #expect(model.state == .ready)
        let bounds = try model.bounds()
        #expect(bounds.min.x < bounds.max.x)
        #expect(bounds.min.y < bounds.max.y)
        try context.frame { frame in
            let drawn = try frame.draw(model, transform: .init(translation: [0, 0, -3]))
            #expect(drawn)
        }
        #expect(context.stats.drawsSubmitted >= 1)
    }

    @Test func aModelLoadsFromMemory() async throws {
        let context = try nullContext()
        let model = try await context.loadModel(data: try Data(contentsOf: cubeFixture))
        #expect(model.state == .ready)
    }

    @Test func aMissingFileThrowsIO() async throws {
        let context = try nullContext()
        let error = await #expect(throws: StickShiftError.self) {
            try await context.loadModel(contentsOf: URL(fileURLWithPath: "/nonexistent/model.glb"))
        }
        #expect(error?.code == .io)
        // The failed model's handle is not leaked.
        #expect(context.stats.models == 0)
    }

    @Test func garbageThrowsParse() async throws {
        let context = try nullContext()
        let error = await #expect(throws: StickShiftError.self) {
            try await context.loadModel(data: Data("not a model".utf8))
        }
        #expect(error?.code == .parse)
    }

    @Test func modelsLoadConcurrentlyAndAreFreed() async throws {
        let context = try nullContext()
        do {
            async let a = context.loadModel(contentsOf: cubeFixture)
            async let b = context.loadModel(contentsOf: cubeFixture)
            let models = try await [a, b]
            #expect(models.allSatisfy { $0.state == .ready })
            #expect(context.stats.models == 2)
        }
        #expect(context.stats.models == 0)
    }
}
