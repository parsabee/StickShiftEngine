// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Foundation
import StickShift
import Testing
import simd

// What the probe tests print, formatted as the Rust probes format it, and the
// fixtures (Tests/Fixtures, from generate-probe-fixtures.sh) they compare with.

let fnvOffset: UInt64 = 0xcbf2_9ce4_8422_2325
let fnvPrime: UInt64 = 0x0100_0000_01b3

/// FNV-1a over `bytes`.
func fnv(_ hash: UInt64, _ bytes: some Sequence<UInt8>) -> UInt64 {
    var hash = hash
    for b in bytes {
        hash = (hash ^ UInt64(b)) &* fnvPrime
    }
    return hash
}

private let hexDigits = Array("0123456789abcdef".utf8)

/// `v` in lowercase hex, zero-padded to `width` digits: Rust's `{:0width$x}`.
func hex(_ v: UInt64, width: Int) -> String {
    var out = [UInt8](repeating: UInt8(ascii: "0"), count: width)
    var x = v
    var i = width - 1
    while i >= 0 {
        out[i] = hexDigits[Int(x & 15)]
        x >>= 4
        i -= 1
    }
    return String(decoding: out, as: UTF8.self)
}

/// `v` in lowercase hex without padding: Rust's `{:x}`.
func hex(_ v: UInt64) -> String {
    String(v, radix: 16)
}

/// A float as Rust's `{:?}` prints it: the shortest digits that round-trip,
/// in plain notation from 1e-4 up to 1e16 (with at least one decimal), and
/// as `1.5e-5` outside it. Checked against Rust over millions of values,
/// ties included (scratch comparison, not committed).
func rust<F: BinaryFloatingPoint & LosslessStringConvertible>(_ x: F) -> String {
    if x.isNaN { return "NaN" }
    if x.isInfinite { return x < 0 ? "-inf" : "inf" }
    let sign = x.sign == .minus ? "-" : ""
    if x == 0 { return sign + "0.0" }
    // Swift prints the same shortest digits, in its own notation.
    let swift = x.magnitude.description
    var mantissa = Substring(swift)
    var exponent = 0
    if let e = swift.firstIndex(of: "e") {
        mantissa = swift[..<e]
        exponent = Int(swift[swift.index(after: e)...].replacingOccurrences(of: "+", with: ""))!
    }
    let parts = mantissa.split(separator: ".", omittingEmptySubsequences: false)
    var digits = Array(parts[0].utf8) + (parts.count > 1 ? Array(parts[1].utf8) : [])
    var point = parts[0].count + exponent  // digits before the decimal point
    let zero = UInt8(ascii: "0")
    while digits.count > 1 && digits.first == zero {
        digits.removeFirst()
        point -= 1
    }
    while digits.count > 1 && digits.last == zero {
        digits.removeLast()
    }
    // When the value lies exactly halfway between two shortest candidates,
    // Swift takes the even one and Rust the one above. A tie needs the exact
    // value to have one digit more than the shortest, and a value with k
    // binary digits after the point has k decimal ones.
    if Int(x.significandWidth) - Int(x.exponent) <= digits.count + 1 {
        let exact = String(format: "%.\(F.significandBitCount > 23 ? 60 : 120)e", Double(x.magnitude))
        let e = exact.firstIndex(of: "e")!
        var exactDigits = Array(exact[..<e].utf8.filter { $0 != UInt8(ascii: ".") })
        while exactDigits.count > 1 && exactDigits.last == zero {
            exactDigits.removeLast()
        }
        if exactDigits.count == digits.count + 1 && exactDigits.last == UInt8(ascii: "5") {
            var up = Array(exactDigits.dropLast())
            point = Int(exact[exact.index(after: e)...])! + 1
            var i = up.count - 1
            while i >= 0 && up[i] == UInt8(ascii: "9") {
                up[i] = zero
                i -= 1
            }
            if i >= 0 {
                up[i] += 1
            } else {
                up.insert(UInt8(ascii: "1"), at: 0)
                point += 1
            }
            while up.count > 1 && up.last == zero {
                up.removeLast()
            }
            digits = up
        }
    }
    let text = String(decoding: digits, as: UTF8.self)
    if x.magnitude >= F(1e-4) && x.magnitude < F(1e16) {
        if point <= 0 {
            return sign + "0." + String(repeating: "0", count: -point) + text
        }
        if point >= text.count {
            return sign + text + String(repeating: "0", count: point - text.count) + ".0"
        }
        return sign + text.prefix(point) + "." + text.dropFirst(point)
    }
    let head = text.count == 1 ? text : text.prefix(1) + "." + text.dropFirst()
    return sign + head + "e\(point - 1)"
}

/// A vector as Rust prints `to_array()` with `{:?}`.
func rust(_ v: SIMD3<Float>) -> String {
    "[\(rust(v.x)), \(rust(v.y)), \(rust(v.z))]"
}

/// An integer array as Rust's `{:?}` prints it.
func rust(_ values: [some BinaryInteger]) -> String {
    "[" + values.map { "\($0)" }.joined(separator: ", ") + "]"
}

/// An option as Rust's `{:?}` prints it, its value already formatted.
func rust(_ value: String?) -> String {
    value.map { "Some(\($0))" } ?? "None"
}

/// FNV-1a over a matrix's 16 elements' bits, column by column, one element
/// at a time: the services probe's `hash`.
func matrixHash(_ m: simd_float4x4) -> String {
    var h = fnvOffset
    for c in 0..<4 {
        for r in 0..<4 {
            h = (h ^ UInt64(m[c][r].bitPattern)) &* fnvPrime
        }
    }
    return hex(h)
}

/// One probe's expected output: its line count, the digest of all its lines
/// and of every 1000 of them.
struct ProbeFixture {
    static let chunk = 1000
    let lines: Int
    let digest: UInt64
    let chunks: [UInt64]

    init(_ name: String) throws {
        let url = workspace.appendingPathComponent("bindings/swift/StickShift/Tests/Fixtures/probe-\(name).txt")
        let text = try String(contentsOf: url, encoding: .utf8)
        var rows = text.split(separator: "\n")
        let head = rows.removeFirst().split(separator: " ")
        lines = Int(head[1])!
        digest = UInt64(head[3], radix: 16)!
        chunks = rows.map { UInt64($0, radix: 16)! }
    }
}

/// A probe's lines as they come, digested and checked against its fixture
/// chunk by chunk; the first chunk that differs is kept, to show.
final class ProbeLog {
    let name: String
    let fixture: ProbeFixture
    private(set) var count = 0
    private var total = fnvOffset
    private var chunkDigest = fnvOffset
    private var chunk: [String] = []
    private var chunkIndex = 0
    private(set) var firstDifference: (chunk: Int, lines: [String])?
    /// Lines a test looks for afterwards, by prefix.
    private let keep: [String]
    private(set) var kept: [String] = []

    init(_ name: String, keeping prefixes: [String] = []) throws {
        self.name = name
        fixture = try ProbeFixture(name)
        keep = prefixes
    }

    func add(_ line: String) {
        var hashed = total
        var chunkHashed = chunkDigest
        for b in line.utf8 {
            hashed = (hashed ^ UInt64(b)) &* fnvPrime
            chunkHashed = (chunkHashed ^ UInt64(b)) &* fnvPrime
        }
        let newline = UInt64(UInt8(ascii: "\n"))
        total = (hashed ^ newline) &* fnvPrime
        chunkDigest = (chunkHashed ^ newline) &* fnvPrime
        chunk.append(line)
        count += 1
        if keep.contains(where: line.hasPrefix) {
            kept.append(line)
        }
        if chunk.count == ProbeFixture.chunk {
            endChunk()
        }
    }

    private func endChunk() {
        let expected = chunkIndex < fixture.chunks.count ? fixture.chunks[chunkIndex] : nil
        if firstDifference == nil && expected != chunkDigest {
            firstDifference = (chunkIndex, chunk)
        }
        chunk = []
        chunkDigest = fnvOffset
        chunkIndex += 1
    }

    /// Check everything against the fixture; on a difference, write the
    /// first chunk that differs to a file for comparing with the Rust lines
    /// (`generate-probe-fixtures.sh --lines DIR`).
    func finish(sourceLocation: SourceLocation = #_sourceLocation) {
        if !chunk.isEmpty {
            endChunk()
        }
        #expect(count == fixture.lines, "\(name): line count", sourceLocation: sourceLocation)
        #expect(total == fixture.digest, "\(name): digest of all lines", sourceLocation: sourceLocation)
        if let (index, lines) = firstDifference {
            let file = FileManager.default.temporaryDirectory
                .appendingPathComponent("probe-\(name)-chunk\(index).lines")
            try? (lines.joined(separator: "\n") + "\n").write(to: file, atomically: true, encoding: .utf8)
            Issue.record(
                "\(name): lines \(index * ProbeFixture.chunk + 1)... differ from the Rust probe's; this run's are in \(file.path)",
                sourceLocation: sourceLocation
            )
        }
    }
}
