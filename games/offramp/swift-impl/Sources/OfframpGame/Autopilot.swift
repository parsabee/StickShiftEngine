// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

// The recorder's autopilot: a port of the steering in
// `games/offramp/rust-impl/examples/record_demo.rs` (spec 05 §3.6), shared by the
// recorder, the trace and their tests.
//
// It taps Space on the first frame, then taps Left or Right only when a car ahead
// in the player's lane is about to reach it, moving to the adjacent lane whose
// nearest car is furthest away, and waits `cooldown` frames between lane
// changes. Each tap is held for one frame and released before the next frame's
// choice, so every press is a fresh edge. Two `Float` details are Rust's: the
// speed is estimated with the fixed step's nanoseconds as `Duration::as_secs_f32`
// gives them, not the `Float` step, and of two lanes equally clear the later
// candidate (Right) wins, as Rust's `max_by` returns the last of equal maxima.
//
// `RecordDemo.record` is the recorder's whole loop (`main` in `record_demo.rs`),
// which `offramp --record` runs and writes out as PNG files.

import StickShift

/// `record_demo.rs`'s frame size and cadence (`:60-68`).
public enum RecordDemo {
    /// The rendered frame's width, in pixels.
    public static let width = 1280
    /// The rendered frame's height, in pixels.
    public static let height = 720
    /// Frames simulated before recording starts: past the fade, once the speed
    /// has ramped up and the traffic is busy.
    public static let warmUpFrames = 150
    /// Frames simulated while recording: four seconds at 60 per second.
    public static let recordFrames = 60 * 4
    /// Every `stride`th recorded frame is written: 20 per second.
    public static let stride = 3
    /// Every frame the recorder simulates.
    public static let frames = warmUpFrames + recordFrames
    /// The camera's aspect ratio, `1280 / 720` in `Float` (`0x3FE38E39`).
    public static let aspectRatio = Float(width) / Float(height)
    /// The frames written by default: every `stride`th of the `recordFrames`.
    public static let defaultFrames = recordFrames / stride

    /// What a recording did.
    public struct Recording: Equatable, Sendable {
        /// The frames simulated and rendered.
        public let simulated: Int
        /// The frames written.
        public let written: Int
        /// The autopilot's lane changes while recording.
        public let laneChanges: Int
    }

    /// A frame the recorder could not render or read back.
    public struct FrameLost: Error, CustomStringConvertible {
        /// The simulated frame, from 0.
        public let frame: Int
        /// What went wrong.
        public let reason: String

        public var description: String {
            "frame \(frame) was not recorded: \(reason)"
        }
    }

    /// Run `record_demo.rs`'s loop (`:96-160`) on `context`, an offscreen Metal
    /// context of `width` × `height` at scale 1, and hand every kept frame to
    /// `write` as RGBA8, top row first, with its number from 0.
    ///
    /// The game runs on a fresh engine on `context` (default settings, silent
    /// audio) at the recorder's 1280/720 aspect, one fixed step per frame. The
    /// autopilot taps Space on frame 0 and then steers. After `warmUpFrames`,
    /// every `stride`th frame is kept, until `frames` are written (the Rust
    /// recorder's 80 by default), so the run is `warmUpFrames + stride × frames`
    /// frames long. The engine waits for a free frame slot before rendering, as
    /// Rust's blocking submit does, and reads a kept frame back within its tick,
    /// so no frame is skipped; a frame that is skipped, or that loses a draw,
    /// throws `FrameLost` instead of recording something Rust would not.
    @MainActor
    public static func record(
        context: Context,
        frames: Int = defaultFrames,
        write: (_ number: Int, _ rgba: [UInt8]) throws -> Void
    ) throws -> Recording {
        let game = CarRunner()
        let engine = try Engine(context: context, game: game, options: Engine.Options(framePacing: .wait))
        engine.camera.aspectRatio = aspectRatio
        try engine.initialize()

        let (width, height) = context.size
        let pixels = PixelBuffer(width: width, height: height)
        var autopilot = Autopilot(start: game.playerPosition())
        var written = 0
        var laneChanges = 0
        let simulated = warmUpFrames + stride * frames
        for frame in 0..<simulated {
            let tap = autopilot.beforeFrame(frame, engine: engine, player: game.playerPosition())
            if let tap, tap != .space, frame >= warmUpFrames {
                laneChanges += 1
            }

            let recording = frame >= warmUpFrames && (frame - warmUpFrames) % stride == 0
            let skipped = context.stats.drawsSkipped
            let tick = engine.advance(readback: recording ? pixels : nil)
            guard tick.steps == 1, tick.rendered, tick.submitted, engine.lastRenderError == nil,
                context.stats.drawsSkipped == skipped
            else {
                let error = engine.lastRenderError.map { "\($0)" } ?? "none"
                throw FrameLost(frame: frame, reason: "\(tick), render error \(error), \(context.stats)")
            }
            autopilot.afterFrame(playerZ: game.playerPosition().z)

            if recording {
                guard tick.readbackReady else {
                    throw FrameLost(frame: frame, reason: "its pixels were not read back (\(tick))")
                }
                try write(written, pixels.rgba())
                written += 1
            }
        }
        return Recording(simulated: simulated, written: written, laneChanges: laneChanges)
    }
}

/// The recorder's steering, frame by frame.
public struct Autopilot: Sendable {
    /// How soon, in seconds at the estimated speed, a car ahead in the lane must
    /// be to make the autopilot change lanes.
    public static let reactionSeconds: Float = 0.45
    /// Extra distance on top of that, about a car length.
    public static let margin: Float = 5.0
    /// Frames to wait between lane changes.
    public static let cooldownFrames: UInt32 = 12

    /// Frames left before another lane change.
    public private(set) var cooldown: UInt32 = 0
    /// The player's speed, estimated from its travel each frame: the fastest seen.
    public private(set) var speed: Float = SPEED
    /// The key pressed for the current frame, released before the next.
    public private(set) var held: Key?
    /// The player's z after the last frame.
    private var lastZ: Float

    /// An autopilot for a game whose player starts at `start` (its position
    /// after `initialize`).
    public init(start: SIMD3<Float>) {
        lastZ = start.z
    }

    /// Before frame `frame` (`record_demo.rs:105-131`): release the last frame's
    /// key, choose this frame's tap and press it, all through `engine`'s input.
    /// `player` is the player's position, and `engine`'s world the one the last
    /// frame left. With `replayOnGameOver` the autopilot also taps Space while
    /// the game is in `GameOver` (the trace's long run; the recorder never
    /// crashes).
    ///
    /// - Returns: The key tapped, if any.
    @MainActor
    @discardableResult
    public mutating func beforeFrame(
        _ frame: Int,
        engine: Engine,
        player: SIMD3<Float>,
        gameOver: Bool = false,
        replayOnGameOver: Bool = false
    ) -> Key? {
        if let key = held {
            engine.input.release(key)
            held = nil
        }
        let tap: Key?
        if frame == 0 || (replayOnGameOver && gameOver) {
            tap = .space
        } else if cooldown > 0 {
            cooldown -= 1
            tap = nil
        } else {
            tap = Autopilot.steer(world: engine.world, player: player, speed: speed)
        }
        if let key = tap {
            engine.input.press(key)
            held = key
            if key != .space {
                cooldown = Autopilot.cooldownFrames
            }
        }
        return tap
    }

    /// After a frame (`record_demo.rs:133-142`): update the speed estimate from the
    /// player's new z. A rebase makes the step jump, so an estimate outside
    /// `0...SPEED_MAX * 1.5` is ignored.
    public mutating func afterFrame(playerZ z: Float) {
        let travelled = (lastZ - z) / RustF32.seconds(nanoseconds: Engine.fixedStepNanoseconds)
        if travelled >= 0 && travelled <= SPEED_MAX * 1.5 {
            speed = RustF32.max(speed, travelled)
        }
        lastZ = z
    }

    /// The lane of world `x` (`lane_of`): rounded half away from zero, as Rust's
    /// `round() as i32`.
    public static func laneOf(_ x: Float) -> Int32 {
        let r = (x / LANE_WIDTH + (Float(LANES) - 1.0) / 2.0).rounded()
        if r.isNaN { return 0 }
        // 2^31 is the first `Float` past `Int32.max`.
        if r <= -2_147_483_648 { return .min }
        if r >= 2_147_483_648 { return .max }
        return Int32(r)
    }

    /// How far ahead of the player the nearest car in `lane` is, counting cars at
    /// least `behind` ahead (negative is behind), or infinity if there is none
    /// (`nearest_in_lane`: an `f32::min` fold from infinity).
    public static func nearestInLane(_ cars: [(lane: Int32, ahead: Float)], lane: Int32, behind: Float) -> Float {
        var nearest = Float.infinity
        for car in cars where car.lane == lane && car.ahead >= behind {
            nearest = RustF32.min(nearest, car.ahead)
        }
        return nearest
    }

    /// Each traffic car's lane and distance ahead of `player` (the game
    /// travels toward -Z), in the order of Rust's
    /// `query::<(&TransformComponent, &TrafficVariant)>()`.
    @MainActor
    public static func cars(in world: World, player: SIMD3<Float>) -> [(lane: Int32, ahead: Float)] {
        world.query(Transform.self, TrafficVariant.self).map { _, t, _ in
            (laneOf(t.position.x), player.z - t.position.z)
        }
    }

    /// `steer` on `world`'s traffic.
    @MainActor
    public static func steer(world: World, player: SIMD3<Float>, speed: Float) -> Key? {
        steer(cars: cars(in: world, player: player), player: player, speed: speed)
    }

    /// The key to tap (`steer`): nothing while the lane ahead is clear for
    /// `speed * reactionSeconds + margin`; otherwise the adjacent lane, of those
    /// with no car within 8 and none right beside the player, whose nearest car is
    /// furthest, the later (Right) winning a tie. `cars` are the traffic cars'
    /// lanes and distances ahead.
    public static func steer(cars: [(lane: Int32, ahead: Float)], player: SIMD3<Float>, speed: Float) -> Key? {
        let lane = laneOf(player.x)
        if nearestInLane(cars, lane: lane, behind: -3.0) > speed * reactionSeconds + margin {
            return nil
        }
        var best: (clear: Float, key: Key)?
        for (l, key) in [(lane - 1, Key.left), (lane + 1, Key.right)] where l >= 0 && l < Int32(LANES) {
            let clear = nearestInLane(cars, lane: l, behind: -6.0)
            guard clear > 8.0 else { continue }
            // `max_by` keeps the earlier only if it is strictly greater.
            if let b = best, totalOrderLess(clear, b.clear) {
                continue
            }
            best = (clear, key)
        }
        return best?.key
    }
}

/// Rust's `f32::total_cmp(a, b) == .Less`: IEEE totalOrder on the bit patterns.
func totalOrderLess(_ a: Float, _ b: Float) -> Bool {
    func key(_ x: Float) -> Int32 {
        let bits = Int32(bitPattern: x.bitPattern)
        return bits ^ Int32(bitPattern: UInt32(bitPattern: bits >> 31) >> 1)
    }
    return key(a) < key(b)
}
