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

/// How many times a destroy was attempted, from whichever thread.
final class Attempts: @unchecked Sendable {
    private let lock = NSLock()
    private var value = 0

    /// Count one more attempt; its number, from 1.
    func next() -> Int {
        lock.withLock {
            value += 1
            return value
        }
    }

    var count: Int { lock.withLock { value } }
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
        #expect(!engine.advance().isPoisoned)
    }

    @Test func aPoisonedTickOrPushAsksTheHostToExit() {
        // Swift cannot poison an engine (the C tests do): this checks what a
        // tick and a push make of the engine's INTERNAL, which only a
        // poisoned engine turns into a result instead of a trap.
        #expect(Engine.isPoisonedFailure(STICKSHIFT_ERROR_INTERNAL) { true })
        #expect(!Engine.isPoisonedFailure(STICKSHIFT_ERROR_INTERNAL) { false })
        #expect(!Engine.isPoisonedFailure(STICKSHIFT_ERROR_WRONG_STATE) { true })
        let poisoned = TickResult.poisoned
        #expect(poisoned.isPoisoned && poisoned.requests == .exit)
        #expect(poisoned.steps == 0 && !poisoned.rendered && !poisoned.submitted && poisoned.frame == nil)
    }

    @Test func aDestroyRefusedAsBusyOnTheMainThreadIsRetriedAfterTheCall() async throws {
        let context = try nullContext()
        let (vertices, indices) = triangle()
        let mesh = try context.makeMesh(layout: .positionNormalColor, vertices: vertices, indices: indices)
        let id = mesh.id
        func state() -> ResourceState { ResourceState(stickshift_resource_state(context.raw, id)) }
        // The first attempt stands in for a call on the context in progress,
        // which the context refuses as busy: the destroy must run after it.
        let attempts = Attempts()
        destroyResource(of: context) { raw in
            attempts.next() == 1 ? STICKSHIFT_ERROR_BUSY : stickshift_mesh_destroy(raw, StickShiftMesh(id: id))
        }
        #expect(attempts.count == 1 && state() == .ready)
        for _ in 0..<1000 where state() != .invalid {
            try await Task.sleep(for: .milliseconds(2))
        }
        #expect(attempts.count == 2 && state() == .invalid)
        withExtendedLifetime(mesh) {}
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
