// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Foundation
import StickShift
import Testing
import simd

@testable import OfframpGame

/// The float math the game does itself, which must stay Rust's operation for
/// operation (design §3.2, risk 1): the glam operations it needs come from the
/// engine (`glamDot`, `glamRotate`, `simd_quatf(glamRotationY:)`), Rust's `f32`
/// rules from `RustF32`, and a lint keeps `simd`'s reductions, fused
/// multiply-adds and Swift's own π out of the game.
@Suite("game math")
struct GameMathTests {
    /// The game's sources, with their line comments removed.
    private static func gameSources() throws -> [(file: String, lines: [String])] {
        let directory = repositoryRoot.appendingPathComponent("games/offramp/Sources")
        let files = try #require(FileManager.default.enumerator(at: directory, includingPropertiesForKeys: nil))
        var out: [(String, [String])] = []
        for case let url as URL in files where url.pathExtension == "swift" {
            let text = try String(contentsOf: url, encoding: .utf8)
            let lines = text.split(separator: "\n", omittingEmptySubsequences: false).map { line in
                line.range(of: "//").map { String(line[..<$0.lowerBound]) } ?? String(line)
            }
            out.append((url.lastPathComponent, lines))
        }
        return out.sorted { $0.0 < $1.0 }
    }

    /// The float math the lint bans, by what it is, with the pattern that finds
    /// it in a line. `simd`'s functions and its overlay's reduce in other orders
    /// than glam and fuse multiplies with adds; a quaternion's `act` is not glam's
    /// `Quat * Vec3`; `Float.pi` (and `.pi`, and `M_PI` as a `Float`) is one bit
    /// below Rust's `PI`; `fma`, `addingProduct` and `addProduct` fuse. Any of
    /// them in game code would break parity on some input.
    static let bannedMath: [(term: String, pattern: NSRegularExpression)] = [
        // Any `simd_` function: `simd_dot`, `simd_act`, `simd_length_squared`,
        // `simd_reduce_add`, `simd_mul`, `simd_quaternion`... The types
        // (`simd_quatf`, `simd_float4x4`) are not functions.
        ("simd_*(", #"(?<![A-Za-z0-9_])simd_(?!quatf\b|float[0-9](x[0-9])?\b)[A-Za-z0-9_]+\s*\("#),
        ("matrix_*(", #"(?<![A-Za-z0-9_])matrix_[A-Za-z0-9_]+\s*\("#),
        // The `simd` overlay's free functions, which are those.
        (
            "simd overlay function",
            #"(?<![A-Za-z0-9_])(dot|length|length_squared|normalize|cross|distance|distance_squared|mix|reduce_add)\s*\("#
        ),
        ("simd_quatf(angle:)", #"simd_quatf\(\s*(angle|from):"#),
        ("quaternion .act(", #"\.act\s*\("#),
        ("SIMD .sum()", #"\.sum\s*\(\s*\)"#),
        ("Swift's pi", #"(?<!RustF32)\.pi\b"#),
        ("M_PI", #"(?<![A-Za-z0-9_])M_PI"#),
        ("fma(", #"(?<![A-Za-z0-9_])fmaf?\s*\("#),
        ("addingProduct(", #"\b(addingProduct|addProduct)\s*\("#),
    ].map { term, pattern in (term, try! NSRegularExpression(pattern: pattern)) }

    /// The banned math in `line` (with its comment already removed), by term.
    static func bannedMath(in line: String) -> [String] {
        let range = NSRange(line.startIndex..., in: line)
        return bannedMath.filter { $0.pattern.firstMatch(in: line, range: range) != nil }.map(\.term)
    }

    @Test func theLintFindsTheMathWhoseBitsDifferFromRusts() {
        // The lint can fail: each spelling of banned math is found...
        let banned: [(line: String, term: String)] = [
            ("let d = simd_dot(a, b)", "simd_*("),
            ("let v = simd_act(q, v)", "simd_*("),
            ("let l = simd_length_squared(v)", "simd_*("),
            ("let s = simd_reduce_add(v)", "simd_*("),
            ("let q = simd_quaternion(m)", "simd_*("),
            ("let m = matrix_multiply(a, b)", "matrix_*("),
            ("let d = dot(a, b)", "simd overlay function"),
            ("let n = normalize (v)", "simd overlay function"),
            ("let l = length(v) + distance(a, b)", "simd overlay function"),
            ("let m = mix(a, b, t: 0.5)", "simd overlay function"),
            ("let c = simd.cross(a, b)", "simd overlay function"),
            ("let q = simd_quatf(angle: a, axis: y)", "simd_quatf(angle:)"),
            ("let v = rotation.act(SIMD3<Float>(1, 0, 0))", "quaternion .act("),
            ("let s = v.sum()", "SIMD .sum()"),
            ("let a: Float = .pi", "Swift's pi"),
            ("let a = Float.pi / 2", "Swift's pi"),
            ("let a = Float(M_PI_2)", "M_PI"),
            ("let y = fmaf(a, b, c) + fma(a, b, c)", "fma("),
            ("let y = c.addingProduct(a, b)", "addingProduct("),
            ("y.addProduct(a, b)", "addingProduct("),
        ]
        for (line, term) in banned {
            #expect(Self.bannedMath(in: line).contains(term), "\(line) uses \(term)")
        }
        // ...and what the game uses instead is not.
        let allowed = [
            "let d = glamDot(a, b)", "let l = glamLength(v)", "let v = rotation.glamRotate(v)",
            "let q = simd_quatf(glamRotationY: -sign * RustF32.fracPi2)", "let yaw: Float = RustF32.pi",
            "let m: simd_float4x4 = matrix_identity_float4x4", "let q = simd_quatf(ix: 0, iy: 1, iz: 0, r: 0)",
            "ChaseController(distance: CHASE_DISTANCE, height: CHASE_HEIGHT)", "let p = fit.pivot",
            "let n = list.models.count", "RustF32.max(farthest, x)",
        ]
        for line in allowed {
            #expect(Self.bannedMath(in: line).isEmpty, "\(line) is allowed")
        }
    }

    @Test func theGameUsesNoMathWhoseBitsDifferFromRusts() throws {
        // Use the engine's `glam*` operations and `RustF32` instead.
        let sources = try Self.gameSources()
        #expect(sources.count >= 15, "the lint reads the game's sources (\(sources.count) files)")
        var found: [String] = []
        for (file, lines) in sources {
            for (i, line) in lines.enumerated() {
                for term in Self.bannedMath(in: line) {
                    found.append("\(file):\(i + 1): \(term)")
                }
            }
        }
        #expect(found.isEmpty, "float math whose bits are not Rust's: \(found)")
    }

    @Test func theBuildsKeepIEEEFloatSemantics() throws {
        // No `-Ounchecked` and no fast-math, which would let the compiler
        // reassociate or fuse the game's float math.
        let manifest = try String(
            contentsOf: repositoryRoot.appendingPathComponent("games/offramp/Package.swift"), encoding: .utf8
        )
        for flag in ["Ounchecked", "fast-math", "ffast-math", "unsafeFlags"] {
            #expect(!manifest.contains(flag), "Package.swift sets \(flag)")
        }
    }

    @Test func signedZerosSurviveTheAxisProducts() {
        let axis = SIMD3<Float>(0, 0, -1)
        // A slot position along -Z: `axis * along` (Rust `Vec3 * f32`).
        #expect(bits(axis * 0) == [0, 0, 0x8000_0000])
        #expect(bits(axis * 6) == [0, 0, 0xC0C0_0000])
        // The rebase offset: `axis * -1000`.
        #expect(bits(axis * -1000) == [0x8000_0000, 0x8000_0000, 0x447A_0000])
        // The player's z at travel 0 is `-travel`.
        let travel: Float = 0
        #expect(bits(-travel) == 0x8000_0000)
    }

    @Test func rotationsAboutYAreGlams() {
        // The buildings' facings (`game.rs:417`), golden from `vectors.txt`.
        for (sign, y): (Float, UInt32) in [(-1, 0x3F35_04F3), (1, 0xBF35_04F3)] {
            #expect(bits(simd_quatf(glamRotationY: -sign * RustF32.fracPi2)) == [0, y, 0, 0x3F35_04F3])
        }
        // The car fit's yaw, `Quat::from_rotation_y(PI)` (spec 04 §10.4).
        #expect(bits(simd_quatf(glamRotationY: CAR_YAW)) == [0, 0x3F80_0000, 0, 0xB33B_BD2E])
        // The engine's axis-angle gives the same for a positive angle, but -0
        // lanes for a negative one: why the buildings use `from_rotation_y`.
        #expect(bits(StickShiftMath.rotation(axis: SIMD3(0, 1, 0), angle: CAR_YAW)) == [0, 0x3F80_0000, 0, 0xB33B_BD2E])
        #expect(bits(StickShiftMath.rotation(axis: SIMD3(0, 1, 0), angle: -RustF32.fracPi2))[0] == 0x8000_0000)
    }

    @Test func theCarFitsTranslationIsGlamsRotationOfThePivot() {
        // The car fit's translation `-(rotation * (pivot * scale)) + (0, CAR_BOTTOM, 0)`
        // (spec 04 §7.4), from the bounds in §10.1, reproduces §10.3 bit for bit.
        let cars: [(min: [UInt32], max: [UInt32], position: [UInt32], scale: UInt32)] = [
            (
                [0xBFAA_EB6F, 0xBC40_B0BD, 0xC02D_CF6A], [0x3FAA_EB67, 0x3FED_2DF7, 0x403C_1DBF],
                [0xB47C_BEC7, 0xBEC9_C3EB, 0x3D66_B68F], 0x3F01_042B
            ),
            (
                [0xBF51_B8AA, 0xBCE8_2F94, 0xBFD3_1BE7], [0x3F51_B8AA, 0x3F8F_02CD, 0x3FD4_85D3],
                [0x2FE4_8EBE, 0xBEC0_4DBE, 0x3B9B_D47D], 0x3F5C_72B4
            ),
            (
                [0xBF67_5793, 0x3BC5_974E, 0xC006_DD6E], [0x3F67_5793, 0x3F97_5FBB, 0x4007_42CC],
                [0x2F48_C8DE, 0xBECE_E27D, 0x3B08_E4FE], 0x3F2C_DC93
            ),
            (
                [0xBF63_8940, 0xBC85_35CA, 0xBFEE_FB1A], [0x3F63_8940, 0x3F9C_7A85, 0x3FEE_8598],
                [0xAF03_ABC8, 0xBEC6_707F, 0xBAB3_8BC6], 0x3F43_93D8
            ),
        ]
        for car in cars {
            let lo = v3(car.min[0], car.min[1], car.min[2])
            let hi = v3(car.max[0], car.max[1], car.max[2])
            let fit = fitTransform(min: lo, max: hi)
            #expect(bits(fit.scale) == [car.scale, car.scale, car.scale])
            #expect(bits(fit.position) == car.position)
        }
    }
}
