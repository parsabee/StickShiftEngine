// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Darwin
import StickShift
import simd

// The render pass, a port of `games/playable-demo/src/render.rs` (spec 03 §7):
// gather this frame's transforms from the world, then record the draws grouped as
// Rust groups them, in the same order:
//
// 1. The skyline backdrop.
// 2. The road tiles, then every model's textured parts (only the player car has
//    any).
// 3. The terrain, then the guardrails, then every model's untextured parts.
// 4. The HUD, over everything.
//
// Rust records each part at `entity.to_matrix() * (fit.to_matrix() * node)`. The
// C API draws a model's parts at `world × (local × node)` with a part filter
// (contract decision 2), so `world = entity`, `local = fit`, and the two passes
// draw `.textured` and then `.untextured`: the same draws, the same products, in
// the same order. Every matrix is the engine's own glam TRS; nothing is
// multiplied on the Swift side. No camera code either: the engine pushed its
// camera to the renderer just before the render hook, as `drive_frame` does.

/// One model entity in the draw list: its transform and what it draws.
public struct ModelPlacement: Equatable, Sendable {
    /// The entity's transform.
    public var transform: Transform
    /// Which model.
    public var model: PlacedModel

    /// Places `model` at `transform`.
    public init(transform: Transform, model: PlacedModel) {
        self.transform = transform
        self.model = model
    }
}

/// The transforms one frame draws, gathered from the world after the frame's
/// updates (`render.rs:48-108`).
public struct SceneDrawList: Equatable, Sendable {
    /// The skyline billboard.
    public var backdrop: Transform
    /// The road tiles, in query order.
    public var roads: [Transform]
    /// The terrain sheet.
    public var terrain: Transform
    /// The guardrail segments, in query order.
    public var guardrails: [Transform]
    /// The player, the buildings and the traffic cars, in query order.
    public var models: [ModelPlacement]

    /// Gather the draw list from `world`. `player` is the player entity, and
    /// `playerPosition` the game's `player_position()`, which places the
    /// camera-locked terrain and backdrop. The player's own draw uses its stored
    /// transform instead (`render.rs:82-83`), which lags the position for the one
    /// frame of a replay (contract decision 6).
    @MainActor
    public init(world: World, player: Entity?, playerPosition: SIMD3<Float>) {
        backdrop = backdropTransform(playerPosition: playerPosition)
        terrain = terrainTransform(playerPosition: playerPosition)

        // One pass over Rust's `(&TransformComponent, &RenderComponent)` query, in
        // its order; the per-entity checks are Rust's `get::<&T>(e)` calls
        // (`render.rs:54-97`). Every renderable that is not the player, a car, a
        // building or a guardrail is a road tile.
        var roads: [Transform] = []
        // The player, the traffic cars and the buildings, in the same query.
        var models: [ModelPlacement] = []
        for (entity, transform) in world.query(Transform.self).with(RenderComponent.self) {
            if entity == player {
                models.append(ModelPlacement(transform: transform, model: .player))
            } else if world.has(Obstacle.self, entity) {
                let variant = world.get(TrafficVariant.self, of: entity)?.index ?? 0
                models.append(ModelPlacement(transform: transform, model: .traffic(variant)))
            } else if let b = world.get(BuildingVariant.self, of: entity) {
                models.append(ModelPlacement(transform: transform, model: .building(b.index)))
            } else if !world.has(GuardrailTag.self, entity) {
                roads.append(transform)
            }
        }
        self.roads = roads
        self.models = models
        // `render.rs:68-73`.
        guardrails = world.query(Transform.self).with(GuardrailTag.self).map { _, transform in transform }
    }
}

/// `terrain_transform` (`render.rs:179-181`): the sheet is baked relative to the
/// player, so it follows the player's z and a stream or rebase never slides it.
public func terrainTransform(playerPosition: SIMD3<Float>) -> Transform {
    Transform(position: SIMD3(0, 0, playerPosition.z))
}

/// `backdrop_transform` (`render.rs:193-205`): the skyline `BACKDROP_DIST` ahead
/// of the player, centred on x = 0, drawn `BACKDROP_SCALE` times its authored size
/// about the chase camera's eye height.
public func backdropTransform(playerPosition: SIMD3<Float>) -> Transform {
    let eyeY = PLAYER_Y + CHASE_HEIGHT
    return Transform(
        position: SIMD3(0, eyeY + (BACKDROP_Y - eyeY) * BACKDROP_SCALE, playerPosition.z - BACKDROP_DIST),
        scale: SIMD3(BACKDROP_W, BACKDROP_H, 1) * BACKDROP_SCALE
    )
}

/// What the render pass draws with, created once in `init` (the Rust
/// `CarRunner`'s mesh, texture and model fields).
@MainActor
public struct RenderResources {
    /// The loaded models.
    public let models: DemoModels
    /// One guardrail segment (`scenery::guardrail_geometry`).
    public let guardrail: Mesh
    /// The hill terrain (`scenery::terrain_geometry`).
    public let terrain: Mesh

    /// Creates the set.
    public init(models: DemoModels, guardrail: Mesh, terrain: Mesh) {
        self.models = models
        self.guardrail = guardrail
        self.terrain = terrain
    }

    /// Build the two procedural meshes and pair them with `models`, as
    /// `game.rs:590-604` does. `tileDepth` is the streaming interval.
    public static func make(models: DemoModels, tileDepth: Float, renderer: Context) throws -> RenderResources {
        let guardrail = try renderer.makeMesh(guardrailGeometry(length: tileDepth))
        let terrain = try renderer.makeMesh(terrainGeometry())
        return RenderResources(models: models, guardrail: guardrail, terrain: terrain)
    }
}

/// Record `list`'s 3D draws into `frame`, in `render.rs`'s order.
@MainActor
public func recordScene(_ list: SceneDrawList, resources: RenderResources, frame: Frame) throws {
    let models = resources.models
    // `entity.to_matrix()` once per placement; both passes use it.
    let placed = list.models.compactMap { p in models.model(for: p.model).map { ($0, p.transform.matrix) } }

    // The backdrop first: far ahead, behind the gameplay, in front of the sky.
    try frame.draw(models.skyline, world: list.backdrop.matrix)

    // The textured pass: the road tiles, then every textured part.
    for tile in list.roads {
        try frame.draw(models.road, world: tile.matrix)
    }
    for (fitted, world) in placed {
        try frame.draw(fitted.model, world: world, local: fitted.local, parts: .textured)
    }

    // The untextured pass: the terrain, the guardrails, then every untextured part.
    try frame.draw(resources.terrain, transform: list.terrain.matrix)
    for rail in list.guardrails {
        try frame.draw(resources.guardrail, transform: rail.matrix)
    }
    for (fitted, world) in placed {
        try frame.draw(fitted.model, world: world, local: fitted.local, parts: .untextured)
    }
}

/// The demo's summer-afternoon sun (`game.rs:534-542`, `config.rs:302-367`).
public let DEMO_SUN = SunSky(
    elevation: SUN_ELEVATION_DEG,
    azimuth: SUN_AZIMUTH_DEG,
    sunColor: SUN_COLOR,
    sunIntensity: SUN_INTENSITY,
    skyFill: SKY_FILL,
    skyZenithColor: SKY_ZENITH_COLOR,
    skyHorizonColor: SKY_HORIZON_COLOR
)
