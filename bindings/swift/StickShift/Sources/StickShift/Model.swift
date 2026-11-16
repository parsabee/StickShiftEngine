// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import CStickShift
import Foundation

/// A static glTF model: its meshes, materials and textures, uploaded and ready
/// to draw. Destroyed when you drop the last reference.
@MainActor
public final class Model {
    /// The context that owns it.
    public let context: Context
    /// The C handle's id.
    public let id: UInt64

    init(context: Context, id: UInt64) {
        self.context = context
        self.id = id
    }

    /// Where the model is in its life. A `Model` you get from
    /// ``Context/loadModel(contentsOf:)`` is ready.
    public var state: ResourceState {
        ResourceState(stickshift_resource_state(context.raw, id))
    }

    /// The model-space bounding box.
    public func bounds() throws -> (min: SIMD3<Float>, max: SIMD3<Float>) {
        var lo = StickShiftVec3()
        var hi = StickShiftVec3()
        try check(stickshift_model_get_bounds(context.raw, StickShiftModel(id: id), &lo, &hi))
        return (SIMD3(lo), SIMD3(hi))
    }

    deinit {
        stickshift_model_destroy(context.raw, StickShiftModel(id: id))
    }
}

/// A model load waiting for its completion. Retained for the C callback and
/// released by it; touched only on the context's thread (inside a poll).
private final class PendingLoad: @unchecked Sendable {
    var continuation: CheckedContinuation<Void, any Error>?
}

/// The completion for a model load: resume the waiting task.
private let modelLoaded: StickShiftCompletionFn = { user, event in
    guard let user, let event else { return }
    let load = Unmanaged<PendingLoad>.fromOpaque(user).takeRetainedValue()
    let e = event.pointee
    if Int32(bitPattern: e.kind) == STICKSHIFT_EVENT_RESOURCE_READY {
        load.continuation?.resume()
    } else {
        load.continuation?.resume(
            throwing: StickShiftError(
                code: StickShiftError.Code(rawValue: e.result),
                message: e.message.map { String(cString: $0) } ?? ""
            )
        )
    }
    load.continuation = nil
}

extension Context {
    /// Load a glTF model (`.gltf` with external or embedded buffers, or `.glb`).
    ///
    /// The file is read and parsed on an engine worker thread; this returns
    /// once it is uploaded and ready to draw. Completions arrive in
    /// ``pollEvents()``, which the context calls by itself while a load is
    /// pending, so this needs no render loop.
    ///
    /// - Throws: ``StickShiftError/Code-swift.struct/io`` if the file cannot be
    ///   read and ``StickShiftError/Code-swift.struct/parse`` if it is not a
    ///   model the engine can load.
    public func loadModel(contentsOf url: URL) async throws -> Model {
        try await load { completion, out throws in
            try check(stickshift_model_load_file(raw, url.path, &completion, &out))
        }
    }

    /// Load a self-contained glTF model from memory: a `.glb`, or a `.gltf`
    /// whose buffers and images are embedded. The bytes are copied.
    ///
    /// - Throws: ``StickShiftError/Code-swift.struct/parse`` if it is not a
    ///   model the engine can load, including one that refers to another file.
    public func loadModel(data: Data) async throws -> Model {
        try await load { completion, out throws in
            try data.withUnsafeBytes { bytes throws in
                let copied = StickShiftData(ptr: bytes.baseAddress, len: bytes.count, release: nil, user: nil)
                try check(stickshift_model_load_memory(raw, copied, &completion, &out))
            }
        }
    }

    private func load(
        _ start: (inout StickShiftCompletion, inout StickShiftModel) throws -> Void
    ) async throws -> Model {
        let pending = PendingLoad()
        var handle = StickShiftModel()
        pendingCompletions += 1
        defer { pendingCompletions -= 1 }
        do {
            try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, any Error>) in
                pending.continuation = continuation
                let user = Unmanaged.passRetained(pending).toOpaque()
                var completion = StickShiftCompletion(callback: modelLoaded, user: user)
                do {
                    try start(&completion, &handle)
                } catch {
                    // The engine took nothing: no callback will come.
                    Unmanaged<PendingLoad>.fromOpaque(user).release()
                    pending.continuation = nil
                    continuation.resume(throwing: error)
                    return
                }
                pumpWhilePending()
            }
        } catch {
            // A failed model's handle stays valid until destroyed.
            if handle.id != 0 {
                stickshift_model_destroy(raw, handle)
            }
            throw error
        }
        return Model(context: self, id: handle.id)
    }
}
