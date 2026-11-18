// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import StickShift
import Testing
import simd

@testable import OfframpGame

/// The recorder's autopilot on hand-built worlds: the rules of `record_demo.rs`'s
/// `steer`, `lane_of` and `nearest_in_lane`, its tap timing and its speed
/// estimate. (Its runs against the Rust ones are in `TraceTests`.)
@MainActor
@Suite("autopilot")
struct AutopilotTests {
    /// An engine whose world holds one traffic car per `(lane, z)`, spawned as
    /// the game spawns them.
    private func engine(_ cars: [(Int, Float)]) throws -> Engine {
        let engine = try scratchEngine()
        for (lane, z) in cars {
            engine.world.spawn(
                Transform(position: SIMD3(CarRunner.laneX(lane), PLAYER_Y, z)),
                RenderComponent.cube(.zero),
                StaticTag(),
                Obstacle(),
                TrafficVariant(0)
            )
        }
        return engine
    }

    /// A world holding one traffic car per `(lane, z)`.
    private func world(_ cars: [(Int, Float)]) throws -> World {
        try engine(cars).world
    }

    private func player(lane: Int, z: Float = 0) -> SIMD3<Float> {
        SIMD3(CarRunner.laneX(lane), PLAYER_Y, z)
    }

    @Test func laneOfRoundsHalfAwayFromZero() {
        #expect([-3.0, 0.0, 3.0].map { Autopilot.laneOf(Float($0)) } == [0, 1, 2])
        // x / 3 + 1 half way between lanes rounds away from zero: 1.5 to 2, 0.5 to
        // 1 (not to the even 0), -0.5 to -1.
        #expect(Autopilot.laneOf(1.5) == 2 && Autopilot.laneOf(-1.5) == 1 && Autopilot.laneOf(-4.5) == -1)
        #expect(Autopilot.laneOf(1.4) == 1)
        #expect(Autopilot.laneOf(.nan) == 0 && Autopilot.laneOf(.infinity) == .max)
    }

    @Test func theNearestCarCountsOnlyItsLaneFromBehind() {
        let cars: [(lane: Int32, ahead: Float)] = [(0, 30), (0, 12), (1, -4), (1, 50), (2, -7)]
        #expect(Autopilot.nearestInLane(cars, lane: 0, behind: -3) == 12)
        #expect(Autopilot.nearestInLane(cars, lane: 1, behind: -3) == 50, "a car 4 behind is past")
        #expect(Autopilot.nearestInLane(cars, lane: 1, behind: -6) == -4, "but beside the player at -6")
        #expect(Autopilot.nearestInLane(cars, lane: 2, behind: -6) == .infinity)
    }

    @Test func aClearLaneAheadMeansNoTap() throws {
        // The car in the lane is past `speed * 0.45 + 5` = 11.3 at 14 u/s.
        let w = try world([(1, -11.4)])
        #expect(Autopilot.steer(world: w, player: player(lane: 1), speed: SPEED) == nil)
        // Within it, a move.
        let close = try world([(1, -11.2)])
        #expect(Autopilot.steer(world: close, player: player(lane: 1), speed: SPEED) != nil)
    }

    @Test func itMovesToTheLaneWhoseNearestCarIsFurthest() throws {
        let w = try world([(1, -8), (0, -20), (2, -40)])
        #expect(Autopilot.steer(world: w, player: player(lane: 1), speed: SPEED) == .right)
        let v = try world([(1, -8), (0, -40), (2, -20)])
        #expect(Autopilot.steer(world: v, player: player(lane: 1), speed: SPEED) == .left)
    }

    @Test func ofTwoEquallyClearLanesTheLaterRightWins() throws {
        // Rust's `max_by` returns the last of equal maxima.
        let empty = try world([(1, -8)])
        #expect(Autopilot.steer(world: empty, player: player(lane: 1), speed: SPEED) == .right, "both infinite")
        let tied = try world([(1, -8), (0, -30), (2, -30)])
        #expect(Autopilot.steer(world: tied, player: player(lane: 1), speed: SPEED) == .right, "both 30")
    }

    @Test func aLaneWithACarBesideOrWithin8IsNoOption() throws {
        // From lane 0 only lane 1 is a candidate; a car 7 ahead in it rules it out.
        let blocked = try world([(0, -8), (1, -7)])
        #expect(Autopilot.steer(world: blocked, player: player(lane: 0), speed: SPEED) == nil)
        // A car right beside the player (5 behind) in the other lane too.
        let beside = try world([(1, -8), (0, 5), (2, 5.5)])
        #expect(Autopilot.steer(world: beside, player: player(lane: 1), speed: SPEED) == nil)
        // Never off the road: from lane 2 there is no Right.
        let edge = try world([(2, -8)])
        #expect(Autopilot.steer(world: edge, player: player(lane: 2), speed: SPEED) == .left)
    }

    @Test func tapsAreHeldOneFrameAndLaneChangesCoolDown() throws {
        // The taps go through the engine's input, and a frame (one update)
        // clears their edges.
        var autopilot = Autopilot(start: player(lane: 1, z: -0.0))
        let e = try engine([(1, -8)])
        let input = e.input

        // Frame 0 is the start key, whatever the world.
        #expect(autopilot.beforeFrame(0, engine: e, player: player(lane: 1)) == .space)
        #expect(input.isKeyDown(.space) && autopilot.cooldown == 0)
        e.advance()

        // Frame 1: Space released, a lane change pressed, the cooldown armed.
        #expect(autopilot.beforeFrame(1, engine: e, player: player(lane: 1)) == .right)
        #expect(!input.isKeyDown(.space) && input.isKeyJustPressed(.right))
        #expect(autopilot.cooldown == Autopilot.cooldownFrames)
        e.advance()

        // Twelve frames of nothing, then it may steer again.
        for frame in 2..<14 {
            #expect(autopilot.beforeFrame(frame, engine: e, player: player(lane: 1)) == nil)
            #expect(!input.isKeyDown(.right), "frame \(frame): the tap was released")
            e.advance()
        }
        #expect(autopilot.beforeFrame(14, engine: e, player: player(lane: 1)) == .right)
    }

    @Test func theLongRunTapsSpaceWhileTheGameIsOver() throws {
        var autopilot = Autopilot(start: player(lane: 1))
        let e = try engine([])
        _ = autopilot.beforeFrame(0, engine: e, player: player(lane: 1))
        #expect(autopilot.beforeFrame(5, engine: e, player: player(lane: 1), gameOver: true) == nil)
        #expect(
            autopilot.beforeFrame(6, engine: e, player: player(lane: 1), gameOver: true, replayOnGameOver: true)
                == .space
        )
    }

    @Test func steeringReadsTheTrafficInQueryOrder() throws {
        // Only traffic cars count, each with its lane and how far ahead it is.
        let e = try engine([(0, -20), (2, -5)])
        let road = Transform(position: SIMD3(CarRunner.laneX(1), PLAYER_Y, -8))
        e.world.spawn(road, RenderComponent.cube(.zero), StaticTag())
        let cars = Autopilot.cars(in: e.world, player: player(lane: 1, z: -2))
        #expect(cars.map(\.lane) == [0, 2])
        #expect(cars.map(\.ahead) == [18, 3])
    }

    @Test func theSpeedEstimateUsesTheDurationsSecondsAndSkipsARebase() {
        // `FIXED_DT_DURATION.as_secs_f32()` is 0x3C888888, not the `Float` step.
        #expect(bits(RustF32.seconds(nanoseconds: Engine.fixedStepNanoseconds)) == 0x3C88_8888)
        #expect(bits(Engine.fixedDeltaTime) == 0x3C88_8889)
        var autopilot = Autopilot(start: SIMD3(0, PLAYER_Y, -0.0))
        // The first Playing step of the recorder's run: 14.00445, not 14.004448.
        autopilot.afterFrame(playerZ: -0.0)
        #expect(bits(autopilot.speed) == 0x4160_0000, "the Space frame does not move: 14")
        autopilot.afterFrame(playerZ: f(0xBE6F_025F))
        #expect(bits(autopilot.speed) == 0x4160_123A)
        // A rebase jumps the car 1000 back: ignored, the estimate is kept.
        autopilot.afterFrame(playerZ: 999)
        #expect(bits(autopilot.speed) == 0x4160_123A)
        // A slower step never lowers it.
        autopilot.afterFrame(playerZ: 998.9)
        #expect(bits(autopilot.speed) == 0x4160_123A)
    }
}
