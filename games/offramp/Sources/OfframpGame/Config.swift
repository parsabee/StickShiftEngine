// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import StickShift
import simd

// Every tuning constant the demo uses, grouped by area: a port of
// `games/playable-demo/src/config.rs`, with the Rust names so the two read side
// by side. Swift float literals round to nearest as Rust's do, so each value has
// the Rust bits; π is the exception and is spelled from its bit pattern
// (`RustF32.pi`). Nothing here is read from the engine.

// MARK: - Lanes, speed, chase camera

/// Forward speed of the player along the streaming axis, in units per second.
public let SPEED: Float = 14.0
/// The speed is multiplied by this every `SPEED_RAMP_SECONDS` of driving.
public let SPEED_RAMP: Float = 1.1
/// How long the speed takes to grow by one `SPEED_RAMP` factor, in seconds.
public let SPEED_RAMP_SECONDS: Float = 5.0
/// The ceiling on the ramped speed. A correctness limit: the overlap test runs
/// once per step, so a step must move less than the combined half-extents (1).
public let SPEED_MAX: Float = 45.0
/// Number of lanes.
public let LANES = 3
/// Distance between adjacent lane centres along X.
public let LANE_WIDTH: Float = 3.0
/// The cars' transform Y (player and traffic), which puts the wheels on the road.
public let PLAYER_Y: Float = 0.1
/// Half-extents of the player box for the overlap test (only x and z are used).
public let PLAYER_HALF = SIMD3<Float>(0.5, 0.5, 0.5)
/// Half-extents of an obstacle box for the overlap test.
public let OBSTACLE_HALF = SIMD3<Float>(0.5, 0.5, 0.5)
/// An obstacle on every Nth streaming slot (never on slot 0).
public let OBSTACLE_EVERY: Int64 = 2
/// On a replay, obstacles this close to the player (either way) are cleared.
public let CLEAR_AHEAD: Float = 10.0
/// The start and reset lane (integer division: the centre lane).
public let START_LANE = LANES / 2
/// The lane PRNG's seed, so a headless run is reproducible.
public let SEED: UInt64 = 0xC0FF_EE00_1234_5678
/// How far behind the player the chase camera trails.
public let CHASE_DISTANCE: Float = 12.0
/// How high above the player the chase camera sits.
public let CHASE_HEIGHT: Float = 6.0
/// How far above the player the camera aims.
public let CHASE_LOOK_AT_HEIGHT: Float = 1.5
/// The chase camera's smoothing half-life, in seconds.
public let CHASE_HALF_LIFE: Float = 0.052
/// The direction of travel (the streaming axis, -Z).
public let FORWARD = SIMD3<Float>(0, 0, -1)
/// The chase camera's near clip distance.
public let CAMERA_NEAR: Float = 0.3
/// The chase camera's far clip distance: past the spawn edge, the backdrop and
/// the far edge of the terrain.
public let CAMERA_FAR: Float = 300.0

// MARK: - Streaming (draw distance)

/// How far ahead of the car the streamed world is kept filled.
public let SPAWN_AHEAD: Float = 180.0
/// How far behind the car a streamed entity may fall before it is despawned.
public let DESPAWN_BEHIND: Float = 12.0

// The Rust file checks the draw-distance layering at compile time
// (`config.rs:104-107`); Swift has no constant assertions, so `ConfigTests` checks
// the same four inequalities.

// MARK: - Car fit

/// The fitted car's longer horizontal extent, in world units.
public let CAR_LEN: Float = 2.85
/// The fitted car's underside Y in model space (it lands on the road at
/// `PLAYER_Y + CAR_BOTTOM = -0.3`).
public let CAR_BOTTOM: Float = -0.4
/// The yaw about +Y that turns the car models to face -Z: Rust's `PI`.
public let CAR_YAW: Float = RustF32.pi

// MARK: - Asset paths (relative to the asset root)

/// The player's car.
public let PLAYER_CAR_ASSET = "sports_car.glb"
/// The traffic cars; an obstacle's `TrafficVariant` indexes this.
public let TRAFFIC_CAR_ASSETS = ["car.glb", "car2.glb", "police_car.glb"]
/// The roadside building prefabs; a building's `BuildingVariant` indexes this.
public let BUILDING_ASSETS = [
    "skyscraper_a.glb",
    "skyscraper_b.glb",
    "large_a.glb",
    "large_b.glb",
    "large_c.glb",
    "small_a.glb",
    "small_b.glb",
    "low_a.glb",
]
/// Spawn weights per prefab (out of 100), parallel to `BUILDING_ASSETS`.
public let BUILDING_WEIGHTS: [UInt32] = [4, 4, 18, 18, 19, 12, 12, 13]
/// The asphalt road tile: a textured quad.
public let ROAD_ASSET = "road.gltf"
/// The skyline backdrop: a textured, curved billboard.
public let SKYLINE_ASSET = "skyline.gltf"
/// The driving music, looped from the first start for the whole session.
public let MUSIC_ASSET = "runner_loop.wav"
/// The impact, played once when a run ends.
public let IMPACT_ASSET = "car_crash_impact_only.wav"

// MARK: - Buildings

/// The horizontal footprint every building is fitted to.
public let BUILDING_FOOTPRINT: Float = 8.0
/// The ground plane the buildings stand on, well below the road deck.
public let GROUND_Y: Float = -10.0
/// The building rows' offset from x = 0.
public let BUILDING_SIDE_X: Float = 11.0
/// The scale of the per-slot jitter added to `BUILDING_SIDE_X`.
public let BUILDING_SIDE_JITTER: Float = 4.0

// MARK: - Guardrail

/// The guardrails' x on each side of the road.
public let GUARDRAIL_X: Float = 5.7
/// The road deck's top, where the guardrails stand.
public let GUARDRAIL_DECK_Y: Float = -0.3
/// The top rail's height above the deck.
public let GUARDRAIL_H: Float = 0.6
/// The spacing between guardrail posts.
public let GUARDRAIL_POST_SPACING: Float = 2.0

// MARK: - Terrain

/// The terrain sheet's width along X.
public let TERRAIN_W: Float = 260.0
/// The sheet's near edge, relative to the player's z.
public let TERRAIN_Z_NEAR: Float = 30.0
/// The sheet's far edge, relative to the player's z.
public let TERRAIN_Z_FAR: Float = -240.0
/// The half-width of the flat valley floor.
public let TERRAIN_FLAT_HALF: Float = 13.0
/// The hills' crest Y.
public let HILL_CREST: Float = 13.0
/// The number of columns across the terrain sheet.
public let TERRAIN_COLUMNS: UInt32 = 96

// MARK: - Backdrop

/// How far ahead of the player the skyline backdrop sits.
public let BACKDROP_DIST: Float = 159.0
/// How much larger than authored the backdrop is drawn, about the camera's eye.
public let BACKDROP_SCALE: Float = 3.0
/// The backdrop's authored width.
public let BACKDROP_W: Float = 150.0
/// The backdrop's authored height.
public let BACKDROP_H: Float = 24.0
/// The backdrop's authored centre Y.
public let BACKDROP_Y: Float = 1.5

// MARK: - HUD

/// The SDF font atlas the HUD draws with.
public let FONT_ASSET = "font.bin"
/// Padding between the safe-area edge and the live score, in points.
public let HUD_MARGIN: Float = 24.0
/// The live score's em size, in points.
public let HUD_SCORE_PX: Float = 30.0
/// The banner headline's em size, in points.
public let HUD_TITLE_PX: Float = 64.0
/// The banner's other lines' em size, in points.
public let HUD_BANNER_PX: Float = 30.0
/// The HUD text colour.
public let HUD_TEXT_COLOR = SIMD4<Float>(0.96, 0.96, 0.98, 1.0)
/// The dim drawn behind the game-over banner.
public let HUD_DIM_COLOR = SIMD4<Float>(0.0, 0.0, 0.0, 0.55)
/// How long the opening black screen takes to fade once a run starts, in seconds.
public let HUD_FADE_SECONDS: Float = 0.75
/// Opaque black: the title screen and the fade.
public let HUD_BLACK_COLOR = SIMD4<Float>(0.0, 0.0, 0.0, 1.0)

// MARK: - Sun and sky (a summer afternoon, about 4pm)

/// The sun's height above the horizon, in degrees.
public let SUN_ELEVATION_DEG: Float = 32.0
/// The sun's bearing in degrees from -Z (north) toward +X (east).
public let SUN_AZIMUTH_DEG: Float = 284.0
/// The sunlight's linear RGB: a gentle warm white.
public let SUN_COLOR = SIMD3<Float>(1.0, 0.96, 0.88)
/// The direct sun's strength.
public let SUN_INTENSITY: Float = 1.15
/// The ambient fill from the sky.
public let SKY_FILL: Float = 0.22
/// The sky's linear RGB at the zenith.
public let SKY_ZENITH_COLOR = SIMD3<Float>(0.11, 0.27, 0.56)
/// The sky's linear RGB at the horizon, which the distance fog fades toward.
public let SKY_HORIZON_COLOR = SIMD3<Float>(0.62, 0.67, 0.74)

// MARK: - Audio mix (decibels relative to each file's recorded level)

/// The master level everything is mixed through.
public let MASTER_VOLUME = Volume(decibels: -3.0)
/// The looping music, well under the impact.
public let MUSIC_VOLUME = Volume(decibels: -14.0)
/// The impact one-shot, the loudest sound in the mix.
public let IMPACT_VOLUME = Volume(decibels: -3.0)
