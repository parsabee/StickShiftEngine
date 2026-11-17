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

    @Test func theGameUsesNoMathWhoseBitsDifferFromRusts() throws {
        // `simd`'s reductions sum in other orders than glam and fuse multiplies
        // with adds; `Float.pi` is one bit below Rust's `PI`; `fma` and
        // `addingProduct` fuse. Any of them in game code would break parity on
        // some input, so none may appear: use the engine's `glam*` operations
        // and `RustF32` instead.
        let banned = [
            "simd_dot(", "simd_act(", "simd_length(", "simd_normalize(", "simd_precise_normalize(", "simd_distance(",
            "simd_mix(", "simd_cross(", "simd_mul(", "Float.pi", "fma(", "fmaf(", "addingProduct(",
            "simd_quatf(angle:",
        ]
        let sources = try Self.gameSources()
        #expect(sources.count >= 15, "the lint reads the game's sources (\(sources.count) files)")
        var found: [String] = []
        for (file, lines) in sources {
            for (i, line) in lines.enumerated() {
                for term in banned where line.contains(term) {
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
