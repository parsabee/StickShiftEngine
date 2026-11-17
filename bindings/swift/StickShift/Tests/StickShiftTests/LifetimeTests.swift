// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import CStickShift
import Foundation
@testable import StickShift
import Testing

/// A mesh handed to another thread to be dropped there.
final class MeshHolder: @unchecked Sendable {
    var mesh: Mesh?

    init(_ mesh: Mesh) {
        self.mesh = mesh
    }

    /// Drop the mesh on this thread; whether it was the main one.
    func drop() -> Bool {
        mesh = nil
        return Thread.isMainThread
    }
}

@Suite("Resource lifetimes and engine state with an engine attached (null backend)")
@MainActor
struct LifetimeTests {
    @Test func aPoisonedStateIsReported() throws {
        var c = StickShiftEngineState()
        c.poisoned = 1
        #expect(EngineState(c).isPoisoned)
        let engine = try Engine.headless(Recorder())
        try engine.initialize()
        #expect(!engine.state.isPoisoned)
    }

    @Test func aResourceDroppedOffTheMainThreadIsDestroyedOnIt() async throws {
        let context = try nullContext()
        let engine = try Engine(context: context, game: Recorder())
        try engine.initialize()
        let (vertices, indices) = triangle()
        let holder = MeshHolder(try context.makeMesh(layout: .positionNormalColor, vertices: vertices, indices: indices))
        let id = try #require(holder.mesh?.id)
        func state() -> ResourceState { ResourceState(stickshift_resource_state(context.raw, id)) }
        #expect(state() == .ready)

        // With an engine attached, the context refuses calls from other
        // threads: the destroy must be sent to the main thread, not lost.
        let droppedOnMain = await Task.detached { holder.drop() }.value
        #expect(!droppedOnMain)
        for _ in 0..<1000 where state() != .invalid {
            try await Task.sleep(for: .milliseconds(2))
        }
        #expect(state() == .invalid, "the mesh was destroyed on the main thread")
        _ = engine
    }
}
