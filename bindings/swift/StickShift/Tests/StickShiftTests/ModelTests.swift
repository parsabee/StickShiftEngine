// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Foundation
import StickShift
import Testing
import simd

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

    @Test func aModelDrawCanSelectItsParts() async throws {
        let context = try nullContext()
        let cube = try await context.loadModel(contentsOf: cubeFixture)
        let textured = try await context.loadModel(contentsOf: texturedCubeFixture)
        // The 3D draws one frame recording `record` hands over.
        func drawn(_ record: (Frame) throws -> Bool) throws -> UInt64 {
            let before = context.stats.drawsSubmitted
            try context.frame { frame in
                let ready = try record(frame)
                #expect(ready)
            }
            return context.stats.drawsSubmitted - before
        }
        let placed = simd_float4x4(translation: [0, 0, -3])
        for (model, texturedParts, untexturedParts): (Model, UInt64, UInt64) in [(cube, 0, 1), (textured, 1, 0)] {
            #expect(try drawn { try $0.draw(model, transform: placed) } == texturedParts + untexturedParts)
            #expect(try drawn { try $0.draw(model, world: placed) } == texturedParts + untexturedParts)
            #expect(try drawn { try $0.draw(model, world: placed, parts: .all) } == texturedParts + untexturedParts)
            #expect(try drawn { try $0.draw(model, world: placed, parts: .textured) } == texturedParts)
            #expect(try drawn { try $0.draw(model, world: placed, parts: .untextured) } == untexturedParts)
        }
        #expect(context.stats.drawsSkipped == 0)
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
