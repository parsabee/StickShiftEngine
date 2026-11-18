// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

// Deterministic randomness, a port of `games/offramp/rust-impl/src/rng.rs`.
//
// Two independent sources, kept apart on purpose:
// - `Rng` (SplitMix64) is seeded once with `SEED` and advanced only when a
//   streaming slot gets an obstacle (one `nextBelow(3)` per obstacle, in slot
//   order). That sequence fixes the obstacle lanes.
// - `hashU64` and the `*ForSlot` helpers derive the scenery (traffic car model,
//   building prefab, lateral jitter) from the slot index alone, so cosmetic
//   variety never perturbs the obstacle world.
//
// All arithmetic is on `UInt64`, wrapping, with logical shifts; the parentheses
// are explicit because Swift's operator precedences differ from Rust's.

/// The lane PRNG: SplitMix64, whose state starts at the seed itself.
public struct Rng: Equatable, Sendable {
    /// The running state, advanced by `nextU64()`.
    public private(set) var state: UInt64

    /// Seed the generator. The same seed always gives the same sequence.
    public init(seed: UInt64) {
        state = seed
    }

    /// The next 64-bit value.
    public mutating func nextU64() -> UInt64 {
        state = state &+ 0x9E37_79B9_7F4A_7C15
        var z = state
        z = (z ^ (z >> 30)) &* 0xBF58_476D_1CE4_E5B9
        z = (z ^ (z >> 27)) &* 0x94D0_49BB_1331_11EB
        return z ^ (z >> 31)
    }

    /// A value in `0..<n` (`n > 0`), by modulo, bias included as in Rust.
    public mutating func nextBelow(_ n: UInt32) -> UInt32 {
        UInt32(nextU64() % UInt64(n))
    }
}

/// The SplitMix64 finalizer over `x`: the same mix as one `Rng.nextU64()` from a
/// state of `x`, with no state kept.
public func hashU64(_ x: UInt64) -> UInt64 {
    var z = x &+ 0x9E37_79B9_7F4A_7C15
    z = (z ^ (z >> 30)) &* 0xBF58_476D_1CE4_E5B9
    z = (z ^ (z >> 27)) &* 0x94D0_49BB_1331_11EB
    return z ^ (z >> 31)
}

/// The traffic car variant (an index into `TRAFFIC_CAR_ASSETS`) for streaming
/// slot `slot`, out of `n` (0 when there are none). The slot's two's-complement
/// bits are hashed, as Rust's `slot as u64` does.
public func variantForSlot(_ slot: Int64, _ n: Int) -> Int {
    if n == 0 {
        return 0
    }
    return Int(hashU64(UInt64(bitPattern: slot)) % UInt64(n))
}

/// The building prefab (an index into `BUILDING_ASSETS`) for `slot` on `side`
/// (0 left, 1 right), weighted by `BUILDING_WEIGHTS`.
public func buildingForSlot(_ slot: Int64, _ side: UInt64) -> Int {
    let total = BUILDING_WEIGHTS.reduce(0, +)
    if total == 0 {
        return 0
    }
    let mixed = (UInt64(bitPattern: slot) &* 0x2545_F491_4F6C_DD1D) ^ (side + 1)
    let r = UInt32(hashU64(mixed) % UInt64(total))
    var acc: UInt32 = 0
    for (i, w) in BUILDING_WEIGHTS.enumerated() {
        acc += w
        if r < acc {
            return i
        }
    }
    return BUILDING_WEIGHTS.count - 1
}

/// A building's lateral jitter in `0...0.999` for `slot` on `side`: an exact
/// integer `0..<1000` converted to `Float`, then one `Float` division
/// (`game.rs:413`).
public func buildingJitter(_ slot: Int64, _ side: UInt64) -> Float {
    let h = hashU64((UInt64(bitPattern: slot) ^ (side << 40)) ^ 0xB57D)
    return Float(h % 1000) / 1000
}
