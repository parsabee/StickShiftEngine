// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import StickShift

// The game's own components (`games/offramp/rust-impl/src/components.rs`), the
// render-time model tag, and the world digest the parity traces compare.
//
// The components are plain data the engine stores in its own ECS, registered on
// first use. The engine's components (`Transform`, `RenderComponent`,
// `StaticTag`, `DynamicTag`) come from the package. Every entity the game spawns
// carries the same set of components as the Rust tuple it ports, the vestigial
// `RenderComponent` included, so the archetypes, and with them the order every
// query walks, are Rust's.

/// Marks an obstacle: a traffic car the player crashes into. Collision tests
/// only entities with this marker.
public struct Obstacle: Component {
    public init() {}
}

/// Which traffic car an obstacle draws: an index into `TRAFFIC_CAR_ASSETS`,
/// fixed at spawn from its slot (Rust's `TrafficVariant(usize)`).
public struct TrafficVariant: Component, Equatable {
    /// The index into `TRAFFIC_CAR_ASSETS`.
    public var index: Int

    public init(_ index: Int) {
        self.index = index
    }
}

/// Which prefab a roadside building draws: an index into `BUILDING_ASSETS`,
/// fixed at spawn from its slot and side. Buildings do not collide.
public struct BuildingVariant: Component, Equatable {
    /// The index into `BUILDING_ASSETS`.
    public var index: Int

    public init(_ index: Int) {
        self.index = index
    }
}

/// Marks a streamed guardrail segment. Guardrails do not collide.
public struct GuardrailTag: Component {
    public init() {}
}

/// Which imported model a placed entity draws (a render-time tag, not a
/// component).
public enum PlacedModel: Equatable, Sendable {
    /// The player's car.
    case player
    /// A traffic car, by variant index.
    case traffic(Int)
    /// A roadside building, by prefab index.
    case building(Int)
}

// MARK: - The world digest

/// FNV-1a, 64-bit: the hash the parity traces use.
public struct FNV1a64: Sendable {
    /// The hash so far.
    public private(set) var value: UInt64 = 0xCBF2_9CE4_8422_2325

    public init() {}

    /// Hash `byte`.
    public mutating func eat(_ byte: UInt8) {
        value ^= UInt64(byte)
        value = value &* 0x0000_0100_0000_01B3
    }

    /// Hash `bytes`, in order.
    public mutating func eat<S: Sequence>(_ bytes: S) where S.Element == UInt8 {
        for byte in bytes {
            eat(byte)
        }
    }

    /// Hash `word`'s four bytes, little-endian.
    public mutating func eat(littleEndian word: UInt32) {
        eat(UInt8(truncatingIfNeeded: word))
        eat(UInt8(truncatingIfNeeded: word >> 8))
        eat(UInt8(truncatingIfNeeded: word >> 16))
        eat(UInt8(truncatingIfNeeded: word >> 24))
    }

    /// Hash `value`'s bit pattern, little-endian.
    public mutating func eat(_ value: Float) {
        eat(littleEndian: value.bitPattern)
    }
}

/// What kind of thing an entity is, for the digest and the world dumps.
public enum EntityKind: UInt8, Sendable {
    case player = 0
    case road = 1
    case obstacle = 2
    case building = 3
    case guardrail = 4

    /// The kind of `entity`, with `player` the player entity, given the
    /// variant and tag components it has, and its variant (0 when the kind has
    /// none). The checks run in `trace.rs`'s order: player, traffic variant,
    /// building variant, guardrail, else road.
    public static func of(
        _ entity: Entity,
        player: Entity?,
        traffic: TrafficVariant?,
        building: BuildingVariant?,
        guardrail: GuardrailTag?
    ) -> (kind: EntityKind, variant: UInt32) {
        if entity == player {
            return (.player, 0)
        }
        if let v = traffic {
            return (.obstacle, UInt32(truncatingIfNeeded: v.index))
        }
        if let v = building {
            return (.building, UInt32(truncatingIfNeeded: v.index))
        }
        if guardrail != nil {
            return (.guardrail, 0)
        }
        return (.road, 0)
    }
}

/// Every entity with a transform, in query order (the order of Rust's
/// `query::<&TransformComponent>()`: the optional terms filter nothing), with
/// its kind and variant: what the digest and the world dumps walk.
@MainActor
public func kindedTransforms(_ world: World, player: Entity?)
    -> [(entity: Entity, transform: Transform, kind: EntityKind, variant: UInt32)]
{
    world.query(Transform.self, TrafficVariant?.self, BuildingVariant?.self, GuardrailTag?.self).map {
        entity, transform, traffic, building, guardrail in
        let (kind, variant) = EntityKind.of(
            entity, player: player, traffic: traffic, building: building, guardrail: guardrail
        )
        return (entity, transform, kind, variant)
    }
}

/// The parity digest of `world` (`trace.rs`'s `digest`): FNV-1a 64 over every
/// entity with a transform, in query order; per entity its kind byte, its
/// variant as a little-endian `UInt32`, then the bits of its position xyz,
/// rotation xyzw and scale xyz, little-endian. Equal digests mean the same
/// entities, in the same order, at the same bits.
@MainActor
public func worldDigest(_ world: World, player: Entity?) -> UInt64 {
    var h = FNV1a64()
    for (_, t, kind, variant) in kindedTransforms(world, player: player) {
        h.eat(kind.rawValue)
        h.eat(littleEndian: variant)
        for f in [
            t.position.x, t.position.y, t.position.z,
            t.rotation.imag.x, t.rotation.imag.y, t.rotation.imag.z, t.rotation.real,
            t.scale.x, t.scale.y, t.scale.z,
        ] {
            h.eat(f)
        }
    }
    return h.value
}
