// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Testing

@testable import OfframpGame

@Suite("randomness")
struct RngTests {
    @Test func theLaneDrawsMatchTheRustSequence() throws {
        // `vectors.txt`: 32 draws, each line's next_u64 from one generator and
        // next_below(3) from another seeded the same.
        let lines = try fixtureLines("vectors.txt").filter { $0.hasPrefix("draw ") }
        #expect(lines.count == 32)
        var raw = Rng(seed: SEED)
        var lanes = Rng(seed: SEED)
        for line in lines {
            let u = try #require(UInt64(try token("next_u64=", in: line), radix: 16))
            let lane = try #require(UInt32(try token("next_below(3)=", in: line)))
            #expect(raw.nextU64() == u, "\(line)")
            #expect(lanes.nextBelow(3) == lane, "\(line)")
        }
    }

    /// `game.rs:1175-1185`, with the golden list (spec 02 T15).
    @Test("the_lane_rng_is_deterministic_from_its_seed") func theLaneRngIsDeterministicFromItsSeed() {
        func lanes() -> [UInt32] {
            var rng = Rng(seed: SEED)
            return (0..<50).map { _ in rng.nextBelow(UInt32(LANES)) }
        }
        #expect(lanes() == lanes(), "same seed ⇒ same obstacle lanes")
        #expect(
            lanes() == [
                0, 1, 0, 2, 2, 2, 0, 1, 1, 2, 1, 1, 2, 0, 1, 2, 0, 0, 0, 1, 2, 1, 1, 0, 1,
                1, 2, 1, 1, 1, 1, 0, 1, 2, 2, 1, 0, 0, 0, 2, 2, 2, 2, 1, 0, 1, 0, 0, 2, 2,
            ]
        )
        var rng = Rng(seed: SEED)
        #expect(
            (0..<8).map { _ in rng.nextU64() } == [
                0xC940_9F09_51D8_DE1A, 0x0385_54CB_2712_FFEE, 0xCF21_B0D8_D5D5_0349, 0x4572_4D0F_094F_6569,
                0x4E1B_5145_E1E7_BD11, 0x112B_303E_0728_C280, 0x2A55_F53F_8B75_DE2C, 0x73EC_DA55_BBE4_4761,
            ]
        )
        var other = Rng(seed: SEED ^ 0x9E37_79B9_7F4A_7C15)
        var mine = Rng(seed: SEED)
        #expect((0..<8).map { _ in other.nextU64() } != (0..<8).map { _ in mine.nextU64() })
    }

    @Test func theHashMatchesRust() {
        #expect(
            (0..<6).map { hashU64($0) } == [
                0xE220_A839_7B1D_CDAF, 0x910A_2DEC_8902_5CC1, 0x9758_35DE_1C97_56CE,
                0x1D0B_14E4_DB01_8FED, 0x6E73_E372_E233_8ACA, 0x6303_3B0C_A389_C35A,
            ]
        )
        #expect(hashU64(UInt64(bitPattern: -1)) == 0xE4D9_7177_1B65_2C20)
        // One `nextU64` from a state of x is the same mix.
        var r = Rng(seed: 41)
        #expect(r.nextU64() == hashU64(41))
    }

    @Test func everySlotsSceneryMatchesTheRustDump() throws {
        // `vectors.txt`: slots 0-40, the hash, the traffic variant, and per side
        // the building, the jitter and the building's x.
        let lines = try fixtureLines("vectors.txt").filter { $0.hasPrefix("slot ") && $0.contains("hash_u64") }
        #expect(lines.count == 41)
        for line in lines {
            let slot = try #require(Int64(try token("slot", in: line).dropLast()))
            let hash = try #require(UInt64(try token("hash_u64(slot)=", in: line), radix: 16))
            #expect(hashU64(UInt64(bitPattern: slot)) == hash, "\(line)")
            #expect(variantForSlot(slot, TRAFFIC_CAR_ASSETS.count) == (try integer("traffic_variant=", in: line)))
            for side: UInt64 in 0..<2 {
                let part = String(try after("side\(side):", in: line))
                #expect(buildingForSlot(slot, side) == (try integer("building=", in: part)), "\(line)")
                let jitter = buildingJitter(slot, side)
                #expect(bits(jitter) == bitPatterns(in: try token("jitter=", in: part))[0], "\(line)")
                let sign: Float = side == 0 ? -1 : 1
                let x = sign * (BUILDING_SIDE_X + jitter * BUILDING_SIDE_JITTER)
                #expect(bits(x) == bitPatterns(in: try token("x=", in: part))[0], "\(line)")
            }
        }
    }

    @Test func negativeSlotsHashTheirTwosComplementBits() throws {
        // Never reached by the demo, but `slot as u64` must be the bit pattern.
        let lines = try fixtureLines("vectors.txt").filter { $0.hasPrefix("slot -") }
        #expect(lines.count == 3)
        for line in lines {
            let slot = try #require(Int64(try token("slot", in: line).dropLast()))
            #expect(variantForSlot(slot, 3) == (try integer("traffic_variant=", in: line)))
            #expect(buildingForSlot(slot, 0) == (try integer("building(0)=", in: line)))
            #expect(buildingForSlot(slot, 1) == (try integer("building(1)=", in: line)))
        }
        // Spec 04 §10.5.
        let variants = (-4...20).map { variantForSlot(Int64($0), 3) }
        #expect(variants == [0, 1, 2, 2, 1, 2, 1, 0, 1, 2, 2, 0, 1, 1, 1, 0, 0, 1, 2, 2, 2, 0, 1, 0, 0])
        let buildings = (-2...12).map { (buildingForSlot(Int64($0), 0), buildingForSlot(Int64($0), 1)) }
        let expected = [
            (2, 4), (3, 7), (5, 2), (4, 7), (2, 3), (7, 5), (4, 2), (7, 1), (2, 2), (3, 7), (2, 7), (2, 2),
            (4, 4), (2, 7), (3, 1),
        ]
        #expect(buildings.map(\.0) == expected.map(\.0) && buildings.map(\.1) == expected.map(\.1))
    }

    @Test func noVariantsMeansVariantZero() {
        #expect(variantForSlot(12, 0) == 0)
    }

    @Test func theObstacleSlotsOfTheFirstPassDrawTheGoldenLanes() {
        // Spec 01 §5: the obstacle in slot 2k (k >= 1) uses draw k-1; the init
        // world's obstacles and their traffic variants.
        var rng = Rng(seed: SEED)
        let slots = stride(from: Int64(2), through: 30, by: 2)
        let lanes = slots.map { _ in rng.nextBelow(3) }
        #expect(lanes == [0, 1, 0, 2, 2, 2, 0, 1, 1, 2, 1, 1, 2, 0, 1])
        #expect(slots.map { variantForSlot($0, 3) } == [1, 1, 2, 1, 1, 0, 2, 2, 1, 0, 2, 0, 1, 1, 2])
    }
}
