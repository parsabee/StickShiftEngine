// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Foundation
import StickShift
import Testing

@testable import OfframpGame

/// The procedural meshes against the Rust ones (`scenery.txt`): counts,
/// FNV-1a 64 of the vertex bytes and the indices, sample vertices, and the
/// terrain height samples.
@Suite("scenery")
struct SceneryTests {
    private func checksum(_ values: [Float]) -> UInt64 {
        var h = FNV1a64()
        for v in values {
            h.eat(v)
        }
        return h.value
    }

    private func checksum(_ values: [UInt32]) -> UInt64 {
        var h = FNV1a64()
        for v in values {
            h.eat(littleEndian: v)
        }
        return h.value
    }

    /// The fixture's summary line and sample vertices for `name`.
    private func golden(_ name: String) throws -> (summary: String, samples: [String], indices: String) {
        let lines = try fixtureLines("scenery.txt")
        let start = try #require(lines.firstIndex { $0.hasPrefix(name) })
        let rest = lines[(start + 1)...].prefix { $0.hasPrefix("  ") }
        let samples = rest.filter { $0.hasPrefix("  v") }
        let indices = try #require(rest.first { $0.hasPrefix("  indices") })
        return (lines[start], Array(samples), indices)
    }

    private func check(_ mesh: ColorMesh, against name: String) throws {
        let (summary, samples, indices) = try golden(name)
        #expect(mesh.vertexCount == (try integer("vertices=", in: summary)))
        #expect(mesh.vertices.count == (try integer("floats=", in: summary)))
        #expect(mesh.indices.count == (try integer("indices=", in: summary)))
        let vertexSum = try #require(UInt64(try token("fnv64(vertex bytes LE)=", in: summary), radix: 16))
        let indexSum = try #require(UInt64(try token("fnv64(index u32 LE)=", in: summary), radix: 16))
        #expect(checksum(mesh.vertices) == vertexSum, "vertex bytes")
        #expect(checksum(mesh.indices) == indexSum, "indices")
        for sample in samples {
            let n = try #require(Int(sample.dropFirst(3).prefix { $0 != ":" }))
            let expected = bitPatterns(in: Substring(sample))
            #expect(expected.count == 9)
            #expect(mesh.vertices[(n * 9)..<(n * 9 + 9)].map(\.bitPattern) == expected, "\(sample)")
        }
        // "  indices[0..12]=[0, 1, 2, ...]"
        let first = try after("=[", in: indices).prefix { $0 != "]" }.split(separator: ",")
        #expect(Array(mesh.indices.prefix(12)) == first.compactMap { UInt32($0.trimmingCharacters(in: .whitespaces)) })
    }

    @Test func theGuardrailIsTheRustMesh() throws {
        let mesh = guardrailGeometry(length: StreamingConfig.default.spawnInterval)
        // Two rails and four posts, six faces each, four corners a face.
        #expect(mesh.vertexCount == 6 * 6 * 4)
        try check(mesh, against: "guardrail_geometry(6.0)")
    }

    @Test func theTerrainIsTheRustMesh() throws {
        let mesh = terrainGeometry()
        #expect(mesh.vertexCount == 2 * (Int(TERRAIN_COLUMNS) + 1))
        try check(mesh, against: "terrain_geometry()")
        // Normals are flat +Y everywhere, slopes included.
        for v in 0..<mesh.vertexCount {
            #expect(Array(mesh.vertices[(v * 9 + 3)..<(v * 9 + 6)]) == [0, 1, 0])
        }
    }

    @Test func theTerrainHeightSamplesHaveTheRustBits() throws {
        let lines = try fixtureLines("scenery.txt").filter { $0.hasPrefix("terrain_height(") }
        #expect(lines.count == 9)
        for line in lines {
            let x = try #require(Float(line.dropFirst("terrain_height(".count).prefix { $0 != ")" }))
            #expect(bits(terrainHeight(x)) == bitPatterns(in: Substring(line))[0], "\(line)")
        }
        // The valley floor is flat out to TERRAIN_FLAT_HALF.
        #expect(terrainHeight(0) == GROUND_Y && terrainHeight(13) == GROUND_Y && terrainHeight(-13) == GROUND_Y)
    }

    @Test func aBoxHasSixOutwardQuads() {
        var mesh = ColorMesh()
        pushBox(&mesh, center: [1, 2, 3], half: [0.5, 1, 2], color: [0.1, 0.2, 0.3])
        #expect(mesh.vertexCount == 24)
        #expect(mesh.indices == (0..<6).flatMap { f -> [UInt32] in
            let b = UInt32(f * 4)
            return [b, b + 1, b + 2, b, b + 2, b + 3]
        })
        // The first face is +Z, its first corner (-1, -1, 1) scaled and moved.
        #expect(Array(mesh.vertices[0..<9]) == [0.5, 1, 5, 0, 0, 1, 0.1, 0.2, 0.3])
        // A second box continues the indices.
        pushBox(&mesh, center: [0, 0, 0], half: [1, 1, 1], color: [1, 1, 1])
        #expect(mesh.indices[36] == 24)
    }
}
