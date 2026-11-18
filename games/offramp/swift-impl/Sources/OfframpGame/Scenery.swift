// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Darwin
import StickShift

// The procedural geometry, a port of `games/offramp/rust-impl/src/scenery.rs`
// (spec 01 §11): the guardrail segment and the hill terrain. Both are built once
// at start-up on the `positionNormalColor` layout (nine floats per vertex:
// position, normal, linear RGB), then placed each frame by transform alone.

/// Interleaved `positionNormalColor` vertices and their triangle indices.
public struct ColorMesh: Equatable, Sendable {
    /// Nine floats per vertex.
    public var vertices: [Float] = []
    /// Three per triangle.
    public var indices: [UInt32] = []

    public init() {}

    /// The number of vertices.
    public var vertexCount: Int {
        vertices.count / 9
    }
}

extension Context {
    /// Upload `geometry` as a `positionNormalColor` mesh.
    public func makeMesh(_ geometry: ColorMesh) throws -> Mesh {
        try makeMesh(layout: .positionNormalColor, vertices: geometry.vertices, indices: geometry.indices)
    }
}

/// The six faces of a unit box, in emit order: each face's normal and its four
/// corners.
private let boxFaces: [(normal: [Float], corners: [[Float]])] = [
    ([0, 0, 1], [[-1, -1, 1], [1, -1, 1], [1, 1, 1], [-1, 1, 1]]),
    ([0, 0, -1], [[1, -1, -1], [-1, -1, -1], [-1, 1, -1], [1, 1, -1]]),
    ([0, 1, 0], [[-1, 1, 1], [1, 1, 1], [1, 1, -1], [-1, 1, -1]]),
    ([0, -1, 0], [[-1, -1, -1], [1, -1, -1], [1, -1, 1], [-1, -1, 1]]),
    ([1, 0, 0], [[1, -1, 1], [1, -1, -1], [1, 1, -1], [1, 1, 1]]),
    ([-1, 0, 0], [[-1, -1, -1], [-1, -1, 1], [-1, 1, 1], [-1, 1, -1]]),
]

/// Append an axis-aligned box at `center` with half-extents `half` in a flat
/// `color`: six outward quads, each two triangles.
public func pushBox(_ mesh: inout ColorMesh, center: [Float], half: [Float], color: [Float]) {
    for face in boxFaces {
        let base = UInt32(mesh.vertexCount)
        for c in face.corners {
            mesh.vertices += [
                center[0] + c[0] * half[0],
                center[1] + c[1] * half[1],
                center[2] + c[2] * half[2],
            ]
            mesh.vertices += face.normal
            mesh.vertices += color
        }
        mesh.indices += [base, base + 1, base + 2, base, base + 2, base + 3]
    }
}

/// One guardrail segment `length` long along Z, its origin at its base centre:
/// two rails and evenly spaced posts, both ends included.
public func guardrailGeometry(length: Float) -> ColorMesh {
    let railColor: [Float] = [0.62, 0.63, 0.66]
    let postColor: [Float] = [0.40, 0.41, 0.44]
    let halfLen = length / 2

    var mesh = ColorMesh()
    pushBox(&mesh, center: [0, GUARDRAIL_H, 0], half: [0.05, 0.07, halfLen], color: railColor)
    pushBox(&mesh, center: [0, GUARDRAIL_H * 0.6, 0], half: [0.05, 0.055, halfLen], color: railColor)

    // Rust's `round` is half away from zero, as is Swift's `rounded()`.
    let posts = Swift.max(Int32((length / GUARDRAIL_POST_SPACING).rounded()), 1)
    for i in 0...posts {
        let z = -halfLen + (Float(i) / Float(posts)) * length
        pushBox(
            &mesh,
            center: [0, GUARDRAIL_H * 0.5, z],
            half: [0.06, GUARDRAIL_H * 0.5, 0.06],
            color: postColor
        )
    }
    return mesh
}

/// The ground height at lateral position `x`: flat at `GROUND_Y` across the
/// valley floor, then rising into rolling hills toward `HILL_CREST`.
public func terrainHeight(_ x: Float) -> Float {
    let ax = abs(x)
    let span = RustF32.max(TERRAIN_W * 0.5 - TERRAIN_FLAT_HALF, RustF32.epsilon)
    let t = RustF32.clamp(RustF32.max(ax - TERRAIN_FLAT_HALF, 0) / span, 0, 1)
    let climb = powf(t, 1.4) * (HILL_CREST - GROUND_Y)
    let roll = 2.2 * (sinf(x * 0.11) * 0.6 + sinf(x * 0.29 + 1.3) * 0.4)
    return (GROUND_Y + climb) + t * roll
}

/// The terrain sheet: one quad strip of `TERRAIN_COLUMNS` columns from
/// `TERRAIN_Z_NEAR` to `TERRAIN_Z_FAR`, each column's height from
/// `terrainHeight`, green going lighter up the hills. The normals are flat +Y
/// everywhere, slopes included.
public func terrainGeometry() -> ColorMesh {
    let valley: [Float] = [0.20, 0.28, 0.17]
    let hill: [Float] = [0.36, 0.42, 0.33]

    var mesh = ColorMesh()
    for i in 0...TERRAIN_COLUMNS {
        let u = Float(i) / Float(TERRAIN_COLUMNS)
        let x = (u - 0.5) * TERRAIN_W
        let y = terrainHeight(x)
        let t = RustF32.clamp((y - GROUND_Y) / (HILL_CREST - GROUND_Y), 0, 1)
        let color = (0..<3).map { k in valley[k] + (hill[k] - valley[k]) * t }
        for z in [TERRAIN_Z_NEAR, TERRAIN_Z_FAR] {
            mesh.vertices += [x, y, z]
            mesh.vertices += [0, 1, 0]
            mesh.vertices += color
        }
    }
    for i in 0..<TERRAIN_COLUMNS {
        let n = 2 * i  // near, this column
        let f = n + 1  // far, this column
        let n2 = n + 2  // near, next column
        let f2 = n + 3  // far, next column
        mesh.indices += [n, f, f2, n, f2, n2]
    }
    return mesh
}
