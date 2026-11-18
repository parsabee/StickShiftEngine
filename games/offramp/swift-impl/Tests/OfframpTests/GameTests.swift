// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Foundation
import StickShift
import Testing
import simd

@testable import OfframpGame

/// `game.rs`'s tests, one to one, with their names and messages, against the
/// Swift `Headless`. The audio tests are in `GameAudioTests`, the lane-PRNG test
/// in `RngTests`. Where the Rust probe captured the values a test reaches, they are
/// checked too, bit for bit ("golden").
@MainActor
@Suite("game")
struct GameTests {
    @Test("init_spawns_player_and_streams_road") func initSpawnsPlayerAndStreamsRoad() throws {
        let game = CarRunner()
        let harness = try headlessRun(game, frames: 0)
        #expect(game.player != nil, "player spawned in init")
        // Streaming primed some road and obstacles ahead.
        #expect(harness.scene.streamedCount > 0, "init primed streamed content ahead of the player")

        // Golden: 31 slots of road, two buildings and two rails, 15 obstacles.
        #expect(harness.scene.streamedCount == 170)
        #expect(harness.world.count == 171)
    }

    @Test("init_streams_the_full_draw_distance_and_the_camera_sees_it")
    func initStreamsTheFullDrawDistanceAndTheCameraSeesIt() throws {
        let game = CarRunner()
        let harness = try headlessRun(game, frames: 0)
        let scene = harness.scene
        #expect(scene.config.spawnAhead == SPAWN_AHEAD)
        #expect(scene.config.despawnBehind == DESPAWN_BEHIND)
        // Spacing is untouched, so traffic per unit of road is unchanged.
        #expect(scene.config.spawnInterval == StreamingConfig.default.spawnInterval)

        // The primed world reaches the spawn edge (folded from f32::MIN).
        let axis = scene.config.axis
        let player = glamDot(game.playerPosition(), axis)
        var farthest = RustF32.lowest
        for (_, t) in harness.world.query(Transform.self) {
            farthest = RustF32.max(farthest, glamDot(t.position, axis) - player)
        }
        #expect(farthest > SPAWN_AHEAD - scene.config.spawnInterval, "streamed content reaches only \(farthest) ahead")

        // The camera was given the demo's clip range.
        #expect(harness.camera.clipPlanes == (CAMERA_NEAR, CAMERA_FAR))

        // Golden.
        #expect(bits(farthest) == 0x4334_0000, "180 ahead")
    }

    @Test("init_asks_the_engine_for_a_summer_afternoon_sun") func initAsksTheEngineForASummerAfternoonSun() throws {
        // The demo's time of day is pushed through the seam, so it is checked
        // headlessly: no GPU, no pixels. It is the context's sticky sun, set in
        // init, before the first frame.
        let game = CarRunner()
        let harness = try headlessRun(game, frames: 0)
        #expect(harness.context.stats.framesSubmitted == 0, "no frame yet")
        let sun = harness.context.sunSky

        #expect(sun.elevation == SUN_ELEVATION_DEG)
        #expect(sun.azimuth == SUN_AZIMUTH_DEG)
        // A summer 4pm sun: 30-35 degrees up, from the west (270 +/- ~20).
        #expect((30.0...35.0).contains(sun.elevation))
        #expect((250.0...290.0).contains(sun.azimuth))
        // Warm, but only slightly.
        #expect(sun.sunColor[2] < sun.sunColor[0], "warm: less blue than red")
        #expect(sun.sunColor[2] > 0.8, "still nearly white at 4pm")
        // Sky fill stays fill; the direct sun dominates.
        #expect(sun.skyFill < sun.sunIntensity * 0.35)

        // Arriving from the west and above: the light travels east (+X) and down.
        let d = sun.direction
        #expect(d.x > 0.0, "light travels eastward, so the sun is in the west")
        #expect(d.y < 0.0, "light travels downward, so the sun is above the horizon")

        // Golden: the whole value, and the direction to float precision.
        #expect(sun == DEMO_SUN)
        #expect(abs(d.x - 0.822_857_44) < 1e-6 && abs(d.y + 0.529_919_27) < 1e-6 && abs(d.z - 0.205_161_44) < 1e-6)
    }

    @Test("lane_x_is_centered_and_ordered") func laneXIsCenteredAndOrdered() {
        // Three lanes centred on 0: -W, 0, +W.
        #expect(abs(CarRunner.laneX(0) + LANE_WIDTH) < 1e-6)
        #expect(abs(CarRunner.laneX(1)) < 1e-6)
        #expect(abs(CarRunner.laneX(2) - LANE_WIDTH) < 1e-6)
        // Golden: exactly -3, +0 and 3.
        #expect([0, 1, 2].map { bits(CarRunner.laneX($0)) } == [0xC040_0000, 0, 0x4040_0000])
    }

    @Test("lane_input_clamps_at_both_edges") func laneInputClampsAtBothEdges() throws {
        // Tap Left three times from the centre (lane 1): reach lane 0 and clamp.
        let (leftGame, hl) = try started()
        for _ in 0..<3 {
            try tap(hl, .left)
        }
        #expect(leftGame.lane == 0, "one lane per press, clamped at the left edge")

        // Tap Right three times from the centre: reach the last lane and clamp.
        let (rightGame, hr) = try started()
        for _ in 0..<3 {
            try tap(hr, .right)
        }
        #expect(rightGame.lane == LANES - 1, "clamped at the right edge")
    }

    @Test("a_held_key_moves_exactly_one_lane") func aHeldKeyMovesExactlyOneLane() throws {
        // Edge-triggered control: holding a key advances exactly one lane.
        let (game, h) = try started()
        game.lane = 0  // start at the left edge
        h.input.press(.right)
        try h.run(frames: 20)  // hold Right for many frames
        #expect(game.lane == 1, "a held key advances one lane (edge-triggered), not a glide to the far lane")
    }

    @Test("opens_frozen_until_the_start_key") func opensFrozenUntilTheStartKey() throws {
        // The demo opens on its title prompt: built and framed, but frozen.
        let game = CarRunner()
        let h = try Headless(game)
        #expect(game.state == .ready, "starts on the title prompt")

        try h.run(frames: 30)
        #expect(game.state == .ready, "still frozen without input")
        #expect(game.distance == 0.0, "no score accrues while frozen")
        #expect(game.travel == 0.0, "the world does not scroll while frozen")
        // Golden: the Ready frames still run the chase follow.
        #expect(bits(h.camera.position) == bits(SIMD3(0, 6.098_598, 11.994_9)), "the camera after 30 Ready frames")

        // Tapping start begins the run, and the world advances from there.
        h.input.press(.space)
        try h.run(frames: 1)
        #expect(game.state == .playing, "the start key begins the run")
        h.input.release(.space)
        try h.run(frames: 5)
        #expect(game.distance > 0.0, "the run advances once started")
    }

    @Test("opening_fade_retires_on_the_fixed_timestep") func openingFadeRetiresOnTheFixedTimestep() throws {
        // The title's black is the HUD's own wash for Ready; the fade timer only
        // runs once a run starts.
        let game = CarRunner()
        let h = try Headless(game)
        #expect(game.state == .ready)
        #expect(game.fadeAlpha() == 0.0, "the fade is idle until a run starts")

        h.input.press(.space)
        try h.run(frames: 1)
        h.input.release(.space)
        #expect(game.fadeAlpha() == 1.0, "the fade starts fully black")

        // Half the fade in: partially revealed. In `Float`, as Rust: 22.499998
        // rounds to 22 (a `Double` would give 22.5 and 23).
        let half = Int(RustF32.toU64((HUD_FADE_SECONDS / (2.0 * FIXED_DT)).rounded()))
        #expect(half == 22)
        try h.run(frames: half)
        let mid = game.fadeAlpha()
        #expect(mid > 0.0 && mid < 1.0, "mid-fade is partially revealed: \(mid)")
        #expect(bits(mid) == 0x3F02_D831, "golden: 0.5111113")

        // Well past the duration: fully revealed and clamped, not negative.
        try h.run(frames: half + 10)
        #expect(game.fadeAlpha() == 0.0, "the fade fully retires and clamps")
    }

    @Test("speed_ramps_with_time_survived_and_resets_on_replay") func speedRampsWithTimeSurvivedAndResetsOnReplay() {
        // The ramp is a pure function of the run clock, so drive it directly.
        let game = CarRunner()
        #expect(abs(game.currentSpeed() - SPEED) < 1e-3, "a run starts at the base speed")

        // Each interval survived multiplies the speed by exactly one factor.
        game.runTime = SPEED_RAMP_SECONDS
        let expected = SPEED * SPEED_RAMP
        #expect(
            abs(game.currentSpeed() - expected) < 1e-3,
            "one interval ⇒ ×\(SPEED_RAMP) (\(game.currentSpeed()) vs \(expected))"
        )
        #expect(bits(game.currentSpeed()) == 0x4176_6667, "golden: 15.400001")

        game.runTime = SPEED_RAMP_SECONDS * 2.0
        let twice = SPEED * SPEED_RAMP * SPEED_RAMP
        #expect(abs(game.currentSpeed() - twice) < 1e-3, "two intervals compound")
        #expect(bits(game.currentSpeed()) == 0x4187_851F, "golden: 16.94")

        // A fresh run drops back to the base speed.
        game.runTime = 0.0
        #expect(abs(game.currentSpeed() - SPEED) < 1e-3)
        #expect(bits(game.currentSpeed()) == 0x4160_0000)
    }

    @Test("the_run_clock_advances_only_while_playing") func theRunClockAdvancesOnlyWhilePlaying() throws {
        // Frozen states must not accrue difficulty.
        let game = CarRunner()
        let h = try Headless(game)
        try h.run(frames: 30)
        #expect(game.runTime == 0.0, "the title screen does not ramp difficulty")

        let (started, hs) = try started()
        try hs.run(frames: 5)
        #expect(started.runTime > 0.0, "driving advances the run clock")
    }

    @Test("speed_is_capped_below_the_tunnelling_threshold") func speedIsCappedBelowTheTunnellingThreshold() {
        // Collision is a per-step box test, so a step must move less than the
        // combined half-extents or the car could pass straight through traffic.
        let game = CarRunner()
        game.runTime = 10_000.0  // absurdly long run
        #expect(game.currentSpeed() == SPEED_MAX, "the ramp is clamped")

        let perStep = SPEED_MAX * FIXED_DT
        let combinedHalf = PLAYER_HALF.z + OBSTACLE_HALF.z
        #expect(
            perStep < combinedHalf,
            """
            at max speed the car moves \(perStep) per step, which must stay under the \(combinedHalf) \
            overlap window or collisions are missed
            """
        )
        #expect(bits(perStep) == 0x3F40_0001, "golden: 0.75000006")
    }

    @Test("a_dodging_run_never_ends_without_an_actual_collision", .timeLimit(.minutes(10)))
    func aDodgingRunNeverEndsWithoutAnActualCollision() throws {
        // Drive far enough to cross the floating-origin rebase, steering into a
        // clear lane each step: the run must never end.
        let (game, h) = try started()
        var rebases: UInt32 = 0
        var firstRebase: Int?

        for step in 0..<20_000 {
            dodge(game, h)
            try h.run(frames: 1)
            if game.rebasedLastStep {
                rebases += 1
                firstRebase = firstRebase ?? step
            }
            if game.state == .gameOver {
                Issue.record(
                    """
                    the run ended at step \(step) (travel \(fixed(game.travel, 1)), speed \
                    \(fixed(game.currentSpeed(), 1)), \(rebases) rebases so far) while dodging — a game-over \
                    without a real collision
                    """
                )
                return
            }
        }

        // The run must have crossed the rebase, or this proves nothing about it.
        #expect(rebases > 0, "the dodging run never triggered a rebase, so it does not cover it")

        // Golden: 13 rebases, the first on step 2704, and the end state.
        #expect(rebases == 13)
        #expect(firstRebase == 2704)
        #expect(bits(game.travel) == 0x4459_88A3, "travel 870.13495")
        #expect(bits(game.distance) == 0x4658_B88C, "distance 13870.137")
        #expect(bits(game.currentSpeed()) == 0x4234_0000, "speed 45")
        #expect(bits(game.runTime) == 0x43A6_A5E6, "run_time 333.29608")
        #expect(h.isClean, "\(h.lostFrames.prefix(5))")
    }

    @Test("the_camera_stays_with_the_car_across_a_rebase", .timeLimit(.minutes(10)))
    func theCameraStaysWithTheCarAcrossARebase() throws {
        // A rebase teleports every transform by a whole threshold; the smoothed
        // chase camera must come with it, without a jolt.
        let (game, h) = try started()

        var sawRebase = false
        for step in 0..<20_000 {
            // Keep dodging so the run survives long enough to rebase.
            dodge(game, h)

            // The camera's pose relative to the car is what the player sees.
            let before = h.camera.position - game.playerPosition()
            let travelBefore = game.travel

            try h.run(frames: 1)

            if game.rebasedLastStep {
                sawRebase = true
                let car = game.playerPosition()
                let cam = h.camera.position

                // 1. Still trailing the car, not stranded a threshold away.
                let gap = glamLength(cam - car)
                let allowed = CHASE_DISTANCE + CHASE_HEIGHT + 5.0
                #expect(
                    gap < allowed,
                    """
                    after a rebase the camera was \(fixed(gap, 1)) from the car (allowed \(fixed(allowed, 1))) — \
                    it did not move with the world
                    """
                )

                // 2. Continuous: the relative pose changes no more than in a step.
                let after = cam - car
                let jolt = glamLength(after - before)
                let perStep = game.currentSpeed() * FIXED_DT
                #expect(
                    jolt < perStep,
                    """
                    the rebase moved the camera \(fixed(jolt, 3)) relative to the car (more than one step of \
                    motion, \(fixed(perStep, 3))) — visible as a jolt
                    """
                )

                // Golden: the probe's step, travel, gap, jolt and step length.
                #expect(step == 2704)
                #expect(travelBefore == Float("999.6933")! && game.travel == Float("0.24438477")!)
                #expect(gap == Float("15.435726")! && jolt == Float("0.12961037")!)
                #expect(perStep == Float("0.5510572")!)
                break
            }
        }

        #expect(sawRebase, "the run never reached a rebase, so this proves nothing")
    }

    @Test("score_increases_with_distance") func scoreIncreasesWithDistance() throws {
        let (game, h) = try started()
        try h.run(frames: 1)
        let afterOne = game.distance
        #expect(afterOne > 0.0, "distance advanced after one step")
        try h.run(frames: 60)
        #expect(game.distance > afterOne, "distance keeps increasing")
        // Score tracks distance monotonically.
        #expect(game.score() >= RustF32.toU64(afterOne))
        // Golden: the first Playing step.
        #expect(bits(afterOne) == 0x3E6F_025F, "0.23340748")
    }

    @Test("overlap_detects_same_lane_hit_only") func overlapDetectsSameLaneHitOnly() {
        let game = CarRunner()
        game.lane = 1
        game.travel = 10.0
        let player = game.playerPosition()
        // Same lane, same z: a hit.
        #expect(game.overlaps(SIMD3(player.x, player.y, player.z)))
        // Another lane, same z: no hit.
        #expect(!game.overlaps(SIMD3(CarRunner.laneX(0), player.y, player.z)))
        // Same lane, far ahead: no hit.
        #expect(!game.overlaps(SIMD3(player.x, player.y, player.z - 5.0)))
    }

    @Test("collision_ends_the_run_at_game_over") func collisionEndsTheRunAtGameOver() throws {
        // A crash enters GameOver; the score is not reset (only a replay does).
        let (game, h) = try driveToGameOver()
        #expect(game.state == .gameOver)
        #expect(game.distance > 0.0, "distance is preserved at game over (score to report)")

        // Golden: the 99th frame's update, in the centre lane, score 23.
        #expect(h.framesRun == 99)
        #expect(game.lane == 1)
        #expect(bits(game.distance) == 0x41B9_D724 && bits(game.travel) == 0x41B9_D724, "23.23005")
        #expect(bits(game.runTime) == 0x3FD1_110A, "1.6333325")
        #expect(bits(game.currentSpeed()) == 0x4167_1579, "14.442742")
        #expect(game.score() == 23 && game.bestScore() == 23)
    }

    @Test("replay_from_game_over_resets_score_keeps_world") func replayFromGameOverResetsScoreKeepsWorld() throws {
        // Space in GameOver starts a fresh run: score zeroed, lane recentred, but
        // the world position keeps advancing so the road never breaks.
        let (game, h) = try driveToGameOver()
        let travelBefore = game.travel
        let scoreAtCrash = game.distance
        #expect(scoreAtCrash > 0.0)

        // Tap Space (edge-triggered) to replay.
        h.input.press(.space)
        try h.run(frames: 1)

        #expect(game.state == .playing, "replay resumes play")
        #expect(game.distance < scoreAtCrash, "score reset on replay")
        #expect(game.lane == START_LANE, "lane recentered")
        #expect(game.travel >= travelBefore, "world travel stays monotonic")

        // Golden: the replay frame runs the GameOver branch, which moves nothing.
        #expect(bits(game.distance) == 0 && bits(game.runTime) == 0)
        #expect(bits(game.travel) == bits(travelBefore))
    }

    @Test("streaming_stays_bounded_over_a_long_run", .timeLimit(.minutes(10)))
    func streamingStaysBoundedOverALongRun() throws {
        // Endless streaming must not leak over a long run that replays after every
        // crash and crosses a rebase.
        let (game, h) = try started()
        let entitiesAtStart = h.world.count
        var rebases: UInt32 = 0
        var maxEntities = 0
        var maxObstacles = 0
        var replays = 0
        for _ in 0..<5000 {
            if game.state == .gameOver {
                h.input.press(.space)
                try h.run(frames: 1)
                h.input.release(.space)
                replays += 1
            }
            try h.run(frames: 1)
            rebases += game.rebasedLastStep ? 1 : 0
            maxEntities = max(maxEntities, h.world.count)
            maxObstacles = max(maxObstacles, h.world.query(Obstacle.self).count)
        }
        print(
            """
            start \(entitiesAtStart), max entities \(maxEntities), max obstacles \(maxObstacles), \
            rebases \(rebases), travel \(game.travel)
            """
        )
        #expect(rebases > 0, "the run crossed a floating-origin rebase")
        #expect(
            maxEntities <= entitiesAtStart + entitiesAtStart / 10,
            "entity count stayed flat: \(maxEntities) (started at \(entitiesAtStart))"
        )
        #expect(maxObstacles > 0 && maxObstacles <= maxEntities, "\(maxObstacles) obstacles")
        // The Swift meaning of `protocol_violations().is_empty()`: nothing refused,
        // no draw and no frame skipped.
        #expect(
            h.isClean,
            "the demo made seam calls the Metal backend would ignore: \(Array(h.lostFrames.prefix(5)))"
        )

        // Golden.
        #expect(entitiesAtStart == 171 && maxEntities == 177 && maxObstacles == 16 && rebases == 1)
        #expect(replays == 35 && h.framesRun == 5036)
        #expect(bits(game.travel) == 0x434E_3C4D, "travel 206.23555")
    }

    @Test("the_obstacle_stream_is_a_function_of_the_seed") func theObstacleStreamIsAFunctionOfTheSeed() throws {
        // Same seed, same obstacles, over a run that streams past the first window.
        let layout = try obstacleLayout(seed: SEED)
        #expect(!layout.isEmpty, "the run streamed obstacles")
        let again = try obstacleLayout(seed: SEED)
        #expect(layout == again, "deterministic")
        // A different seed picks different lanes.
        let other = try obstacleLayout(seed: SEED ^ 0x9E37_79B9_7F4A_7C15)
        #expect(layout != other, "the seed decides the lanes")

        // Golden: both layouts, in centimetres.
        func cm(_ pairs: [(Int64, Int64)]) -> [Centimetres] { pairs.map { Centimetres(x: $0.0, z: $0.1) } }
        #expect(
            layout
                == cm([
                    (-300, -16800), (-300, -8400), (-300, -3600), (-300, -1200), (0, -18000), (0, -14400),
                    (0, -13200), (0, -10800), (0, -9600), (0, -2400), (300, -19200), (300, -15600), (300, -12000),
                    (300, -7200), (300, -6000), (300, -4800),
                ])
        )
        #expect(
            other
                == cm([
                    (-300, -20400), (-300, -19200), (-300, -18000), (-300, -13200), (-300, -12000), (-300, -8400),
                    (-300, -7200), (-300, -4800), (-300, -2400), (0, -16800), (0, -9600), (300, -15600),
                    (300, -14400), (300, -10800), (300, -6000), (300, -3600),
                ])
        )
    }
}
