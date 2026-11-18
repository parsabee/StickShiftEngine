// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Foundation
import StickShift
import Testing
import simd

@testable import OfframpGame

// Shared test helpers: the golden fixtures captured from the Rust demo, their
// parsers, and bit-level comparisons.

/// Thrown when a fixture does not have the expected shape.
struct FixtureError: Error, CustomStringConvertible {
    let description: String
}

/// The directory holding the golden fixtures (copied from the Rust probes'
/// output; see `Fixtures/README.md`).
let fixturesDirectory = URL(fileURLWithPath: #filePath)
    .deletingLastPathComponent()
    .appendingPathComponent("Fixtures")

/// The repository root, from this file's path.
let repositoryRoot = URL(fileURLWithPath: #filePath)
    .deletingLastPathComponent()  // OfframpTests
    .deletingLastPathComponent()  // Tests
    .deletingLastPathComponent()  // swift-impl
    .deletingLastPathComponent()  // offramp
    .deletingLastPathComponent()  // games
    .deletingLastPathComponent()

/// The Rust demo's committed assets, which Offramp reads in place.
let demoAssets = repositoryRoot.appendingPathComponent("games/offramp/shared/assets")

/// The text of fixture `name`.
func fixture(_ name: String) throws -> String {
    try String(contentsOf: fixturesDirectory.appendingPathComponent(name), encoding: .utf8)
}

/// The lines of fixture `name`, without empty ones.
func fixtureLines(_ name: String) throws -> [String] {
    try fixture(name).split(separator: "\n").map(String.init)
}

// MARK: - Bits

/// A float's bit pattern, so a comparison tells `-0` from `+0`.
func bits(_ x: Float) -> UInt32 {
    x.bitPattern
}

/// A vector's lanes as bit patterns.
func bits(_ v: SIMD3<Float>) -> [UInt32] {
    [v.x.bitPattern, v.y.bitPattern, v.z.bitPattern]
}

/// A quaternion's x, y, z, w as bit patterns.
func bits(_ q: simd_quatf) -> [UInt32] {
    [q.imag.x.bitPattern, q.imag.y.bitPattern, q.imag.z.bitPattern, q.real.bitPattern]
}

/// A matrix's 16 elements as bit patterns, column by column.
func bits(_ m: simd_float4x4) -> [UInt32] {
    (0..<4).flatMap { c in (0..<4).map { r in m[c][r].bitPattern } }
}

/// A float from its bits.
func f(_ bits: UInt32) -> Float {
    Float(bitPattern: bits)
}

/// A vector from three bit patterns.
func v3(_ x: UInt32, _ y: UInt32, _ z: UInt32) -> SIMD3<Float> {
    SIMD3(Float(bitPattern: x), Float(bitPattern: y), Float(bitPattern: z))
}

// MARK: - Fixture parsing

/// Every `[xxxxxxxx]` bit pattern in `text`, in order (how the Rust probes print
/// a float: `value[bits]`).
func bitPatterns(in text: Substring) -> [UInt32] {
    var out: [UInt32] = []
    var rest = text
    while let open = rest.firstIndex(of: "[") {
        let after = rest[rest.index(after: open)...]
        guard let close = after.firstIndex(of: "]") else { break }
        let inner = after[..<close]
        if inner.count == 8, let value = UInt32(inner, radix: 16) {
            out.append(value)
        }
        rest = after[after.index(after: close)...]
    }
    return out
}

/// What follows `key` in `line`.
func after(_ key: String, in line: String) throws -> Substring {
    guard let range = line.range(of: key) else {
        throw FixtureError(description: "no \(key) in: \(line)")
    }
    return line[range.upperBound...]
}

/// The space-delimited token after `key`, leading spaces skipped.
func token(_ key: String, in line: String) throws -> Substring {
    try after(key, in: line).drop { $0 == " " }.prefix { $0 != " " }
}

/// The integer after `key`.
func integer(_ key: String, in line: String) throws -> Int {
    let t = try token(key, in: line)
    guard let value = Int(t) else { throw FixtureError(description: "\(key) \(t) is not an integer") }
    return value
}

/// The bits of the `(x, y, z...)` tuple after `key`.
func tupleBits(_ key: String, in line: String) throws -> [UInt32] {
    let rest = try after(key + "(", in: line)
    return bitPatterns(in: rest.prefix { $0 != ")" })
}

/// The vector printed as `key(x[bits], y[bits], z[bits])`.
func vector(_ key: String, in line: String) throws -> SIMD3<Float> {
    let b = try tupleBits(key, in: line)
    guard b.count == 3 else { throw FixtureError(description: "\(key) is not a vector in: \(line)") }
    return v3(b[0], b[1], b[2])
}

/// The float printed as `key value[bits]`.
func scalar(_ key: String, in line: String) throws -> Float {
    let b = bitPatterns(in: try after(key, in: line).prefix { $0 != " " })
    guard let first = b.first else { throw FixtureError(description: "no bits after \(key) in: \(line)") }
    return Float(bitPattern: first)
}

/// One frame of a Rust trace (`smoke_trace.txt`, `record_trace.txt`).
struct TraceFrame {
    let frame: Int
    let tap: String
    let state: String
    let player: SIMD3<Float>
    let speed: Float
    let score: Int
    let best: Int
    let fade: Float
    let cameraPosition: SIMD3<Float>
    let cameraTarget: SIMD3<Float>
    let streamed: Int
    let entities: Int
    let obstacles: Int
    let digest: UInt64

    init(_ line: String) throws {
        frame = try integer("f=", in: line)
        tap = String(try token("tap=", in: line))
        state = String(try token("state=", in: line))
        player = try vector("player=", in: line)
        speed = try scalar("speed=", in: line)
        score = try integer("score=", in: line)
        best = try integer(" best=", in: line)
        fade = try scalar("fade=", in: line)
        cameraPosition = try vector("cam.pos=", in: line)
        cameraTarget = try vector("cam.target=", in: line)
        streamed = try integer("streamed=", in: line)
        entities = try integer("entities=", in: line)
        obstacles = try integer("obstacles=", in: line)
        guard let d = UInt64(try token("digest=", in: line), radix: 16) else {
            throw FixtureError(description: "bad digest in: \(line)")
        }
        digest = d
    }
}

/// Every frame of trace fixture `name`, in order.
func trace(_ name: String) throws -> [TraceFrame] {
    try fixtureLines(name).filter { $0.hasPrefix("f=") }.map(TraceFrame.init)
}

/// One entity line of a Rust world dump (`init_world.txt`,
/// `record_frame150_world.txt`): `#i K id=N variant=V pos=(...) rot=(...) scale=(...)`.
struct DumpedEntity: Equatable, CustomStringConvertible {
    let kind: Character
    let id: UInt32
    let variant: UInt32
    /// Position xyz, rotation xyzw, scale xyz, as bits.
    let bits: [UInt32]

    init(kind: Character, id: UInt32, variant: UInt32, bits: [UInt32]) {
        self.kind = kind
        self.id = id
        self.variant = variant
        self.bits = bits
    }

    init(_ line: String) throws {
        // The kind letter is the character just before " id=" (the index before
        // it is padded: "#  0 P", "#100 G").
        guard let idField = line.range(of: " id="), let k = line[..<idField.lowerBound].last else {
            throw FixtureError(description: "not an entity line: \(line)")
        }
        kind = k
        guard let i = UInt32(try token("id=", in: line)), let v = UInt32(try token("variant=", in: line)) else {
            throw FixtureError(description: "bad id or variant in: \(line)")
        }
        id = i
        variant = v
        bits = try tupleBits("pos=", in: line) + tupleBits("rot=", in: line) + tupleBits("scale=", in: line)
    }

    var description: String { "\(kind) id=\(id) variant=\(variant) \(bits.map { String($0, radix: 16) })" }
}

/// The entity lines of world dump fixture `name`, in order.
func worldDump(_ name: String) throws -> [DumpedEntity] {
    try fixtureLines(name).filter { $0.hasPrefix("  #") }.map(DumpedEntity.init)
}

/// The world as the Rust dumps print it, in query order, each entity with its
/// hecs id (the low half of its bits).
@MainActor
func dump(_ world: World, player: Entity?) -> [DumpedEntity] {
    kindedTransforms(world, player: player).map { row in
        let letter: Character =
            switch row.kind {
            case .player: "P"
            case .road: "R"
            case .obstacle: "O"
            case .building: "B"
            case .guardrail: "G"
            }
        let t = row.transform
        return DumpedEntity(
            kind: letter,
            id: UInt32(truncatingIfNeeded: row.entity.bits),
            variant: row.variant,
            bits: OfframpTests.bits(t.position) + OfframpTests.bits(t.rotation) + OfframpTests.bits(t.scale)
        )
    }
}

/// The demo's lane x (`CarRunner.laneX`, `game.rs:157-160`).
func laneX(_ lane: Int) -> Float {
    CarRunner.laneX(lane)
}
