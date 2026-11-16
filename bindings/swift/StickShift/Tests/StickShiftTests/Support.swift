// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Foundation
import StickShift

/// The workspace root, from this file's path.
let workspace = URL(fileURLWithPath: #filePath)
    .deletingLastPathComponent()  // StickShiftTests
    .deletingLastPathComponent()  // Tests
    .deletingLastPathComponent()  // StickShift
    .deletingLastPathComponent()  // swift
    .deletingLastPathComponent()  // bindings
    .deletingLastPathComponent()

/// The committed glTF cube the asset tests use.
let cubeFixture = workspace.appendingPathComponent("crates/stickshift-assets/tests/fixtures/cube.gltf")

/// One triangle facing +Z, on the `positionNormalColor` layout, in `color`.
func triangle(_ color: SIMD3<Float> = [0.9, 0.2, 0.1]) -> (vertices: [Float], indices: [UInt32]) {
    let corners: [SIMD3<Float>] = [[-0.6, -0.5, 0], [0.6, -0.5, 0], [0, 0.6, 0]]
    var vertices: [Float] = []
    for p in corners {
        vertices += [p.x, p.y, p.z]
        vertices += [0, 0, 1]
        vertices += [color.x, color.y, color.z]
    }
    return (vertices, [0, 1, 2])
}

/// A camera two units in front of the origin, looking at it.
func camera(aspect: Float) -> Camera {
    .perspective(eye: [0, 0, 2], target: .zero, fovY: 0.9, aspect: aspect)
}

/// A null-backend context: no GPU needed.
@MainActor
func nullContext(width: Int = 64, height: Int = 48) throws -> Context {
    try Context(offscreenWidth: width, height: height, backend: .null)
}

/// An offscreen Metal context, or `nil` without a Metal device. With
/// `STICKSHIFT_REQUIRE_GPU=1` (CI) a missing device is a failure instead.
@MainActor
func metalContext(width: Int = 64, height: Int = 48) throws -> Context? {
    do {
        return try Context(offscreenWidth: width, height: height)
    } catch let error as StickShiftError where error.code == .noDevice {
        let required = ProcessInfo.processInfo.environment["STICKSHIFT_REQUIRE_GPU"]
        if required == "1" || required?.lowercased() == "true" {
            throw error
        }
        FileHandle.standardError.write(Data("SKIPPED: no Metal device\n".utf8))
        return nil
    }
}
