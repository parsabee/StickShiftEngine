// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import StickShift
import Testing
import simd

@testable import OfframpGame

@Suite("config")
struct ConfigTests {
    /// `config.rs:104-107`, checked at compile time in Rust: new slots fill in
    /// behind the backdrop, the ground runs on under it, and the far plane
    /// (measured from the camera, `CHASE_DISTANCE` behind the car) covers all of it.
    @Test("config.rs compile-time asserts") func drawDistanceLayering() {
        #expect(BACKDROP_DIST < SPAWN_AHEAD)
        #expect(-TERRAIN_Z_FAR > BACKDROP_DIST)
        #expect(SPAWN_AHEAD + CHASE_DISTANCE < CAMERA_FAR)
        #expect(-TERRAIN_Z_FAR + CHASE_DISTANCE < CAMERA_FAR)
    }

    @Test func valuesHaveTheRustBits() {
        // The literals whose rounding matters (spec 01 §1, 03 §3).
        #expect(bits(PLAYER_Y) == 0x3DCC_CCCD)
        #expect(bits(SPEED_RAMP) == 0x3F8C_CCCD)
        #expect(bits(CHASE_HALF_LIFE) == 0x3D54_FDF4)
        #expect(bits(GUARDRAIL_X) == 0x40B6_6666)
        #expect(bits(GUARDRAIL_DECK_Y) == 0xBE99_999A)
        #expect(bits(CAR_YAW) == 0x4049_0FDB)
        #expect(bits(CAR_BOTTOM) == 0xBECC_CCCD)
        #expect(bits(CAR_LEN) == 0x4036_6666)
        #expect(bits(HUD_TEXT_COLOR.x) == 0x3F75_C28F && bits(HUD_TEXT_COLOR.z) == 0x3F7A_E148)
        #expect(bits(HUD_DIM_COLOR.w) == 0x3F0C_CCCD)
        #expect(bits(SUN_COLOR.z) == 0x3F61_47AE)
        #expect(bits(SUN_INTENSITY) == 0x3F93_3333)
        #expect(bits(SKY_FILL) == 0x3E61_47AE)
    }

    @Test func gameplayConstants() {
        #expect(SPEED == 14 && SPEED_RAMP_SECONDS == 5 && SPEED_MAX == 45)
        #expect(LANES == 3 && LANE_WIDTH == 3)
        #expect(START_LANE == 1, "integer division picks the centre lane")
        #expect(PLAYER_HALF == SIMD3(0.5, 0.5, 0.5) && OBSTACLE_HALF == SIMD3(0.5, 0.5, 0.5))
        #expect(OBSTACLE_EVERY == 2 && CLEAR_AHEAD == 10)
        #expect(SEED == 0xC0FF_EE00_1234_5678)
        #expect(CHASE_DISTANCE == 12 && CHASE_HEIGHT == 6 && CHASE_LOOK_AT_HEIGHT == 1.5)
        #expect(bits(FORWARD) == [0, 0, 0xBF80_0000])
        #expect(CAMERA_NEAR == 0.3 && CAMERA_FAR == 300)
        #expect(SPAWN_AHEAD == 180 && DESPAWN_BEHIND == 12)
        #expect(SPAWN_AHEAD == 3 * StreamingConfig.default.spawnAhead, "three times the engine default")
        #expect(DESPAWN_BEHIND == StreamingConfig.default.despawnBehind)
    }

    @Test func sceneryConstants() {
        #expect(BUILDING_FOOTPRINT == 8 && GROUND_Y == -10)
        #expect(BUILDING_SIDE_X == 11 && BUILDING_SIDE_JITTER == 4)
        #expect(GUARDRAIL_H == 0.6 && GUARDRAIL_POST_SPACING == 2)
        #expect(TERRAIN_W == 260 && TERRAIN_Z_NEAR == 30 && TERRAIN_Z_FAR == -240)
        #expect(TERRAIN_FLAT_HALF == 13 && HILL_CREST == 13 && TERRAIN_COLUMNS == 96)
        #expect(BACKDROP_DIST == 159 && BACKDROP_SCALE == 3)
        #expect(BACKDROP_W == 150 && BACKDROP_H == 24 && BACKDROP_Y == 1.5)
    }

    @Test func hudSunAndAudioConstants() {
        #expect(HUD_MARGIN == 24 && HUD_SCORE_PX == 30 && HUD_TITLE_PX == 64 && HUD_BANNER_PX == 30)
        #expect(HUD_TEXT_COLOR == SIMD4(0.96, 0.96, 0.98, 1))
        #expect(HUD_DIM_COLOR == SIMD4(0, 0, 0, 0.55))
        #expect(HUD_BLACK_COLOR == SIMD4(0, 0, 0, 1))
        #expect(HUD_FADE_SECONDS == 0.75)
        #expect(SUN_ELEVATION_DEG == 32 && SUN_AZIMUTH_DEG == 284)
        #expect(SUN_COLOR == SIMD3(1, 0.96, 0.88))
        #expect(SUN_INTENSITY == 1.15 && SKY_FILL == 0.22)
        #expect(SKY_ZENITH_COLOR == SIMD3(0.11, 0.27, 0.56))
        #expect(SKY_HORIZON_COLOR == SIMD3(0.62, 0.67, 0.74))
        #expect(MASTER_VOLUME == Volume(decibels: -3) && MUSIC_VOLUME == Volume(decibels: -14) && IMPACT_VOLUME == Volume(decibels: -3))
        #expect(MUSIC_VOLUME.decibels < IMPACT_VOLUME.decibels, "the music sits under the impact")
    }

    @Test func assetNames() {
        #expect(PLAYER_CAR_ASSET == "sports_car.glb")
        #expect(TRAFFIC_CAR_ASSETS == ["car.glb", "car2.glb", "police_car.glb"])
        #expect(
            BUILDING_ASSETS == [
                "skyscraper_a.glb", "skyscraper_b.glb", "large_a.glb", "large_b.glb",
                "large_c.glb", "small_a.glb", "small_b.glb", "low_a.glb",
            ]
        )
        #expect(BUILDING_WEIGHTS == [4, 4, 18, 18, 19, 12, 12, 13])
        #expect(BUILDING_WEIGHTS.reduce(0, +) == 100)
        #expect(BUILDING_WEIGHTS.count == BUILDING_ASSETS.count)
        #expect(ROAD_ASSET == "road.gltf" && SKYLINE_ASSET == "skyline.gltf" && FONT_ASSET == "font.bin")
        #expect(MUSIC_ASSET == "runner_loop.wav" && IMPACT_ASSET == "car_crash_impact_only.wav")
    }
}
