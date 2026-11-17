// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Darwin
import StickShift
import simd

// The game: a port of `games/playable-demo/src/game.rs` (`CarRunner`, spec 01).
//
// The car drives forward along -Z; three lanes run along X and Left/Right move it
// one lane per press. The road, the traffic and the scenery are streamed around
// the car, and a crash into traffic ends the run until Space replays it. Every
// rule, and the order of every step, is Rust's, and the world, the scene, the
// camera, the input and the audio are the engine's own, so the same inputs give
// the same world, bit for bit. Two of Rust's quirks come with it (contract
// decision 6): on the frame of a replay the car is still drawn in its crash lane,
// because its stored transform is written only by a Playing step; and
// `initialize` streams the full 180 ahead whatever draw distance a player
// restored, because the engine scales the reach only after `initialize` returns.

/// Where a run is (Rust's `GameState`, `game.rs:144-153`).
public enum GameState: Equatable, Sendable, CustomStringConvertible {
    /// The title screen: frozen until the start key.
    case ready
    /// Driving.
    case playing
    /// Crashed: frozen until the replay key.
    case gameOver

    public var description: String {
        switch self {
        case .ready: "Ready"
        case .playing: "Playing"
        case .gameOver: "GameOver"
        }
    }
}

/// The endless runner (Rust's `CarRunner`).
///
/// Two distances: `travel` is the car's world position along the axis and never
/// goes back (except by a rebase's shift), which keeps the scene's spawn frontier
/// valid; `distance` is the score of this run, and a replay resets it to zero.
@MainActor
public final class CarRunner: Game {
    /// The player's entity, spawned in `initialize`.
    public internal(set) var player: Entity?
    /// The lane the car is in, `0..<LANES`.
    public internal(set) var lane = START_LANE
    /// How far the car has travelled along the axis since the last rebase.
    public internal(set) var travel: Float = 0
    /// This run's distance, which the score is: reset by a replay.
    public internal(set) var distance: Float = 0
    /// The best run's distance this session.
    public internal(set) var best: Float = 0
    /// The lane PRNG: one `nextBelow(3)` per obstacle, in slot order.
    public internal(set) var rng = Rng(seed: SEED)
    /// Where the run is.
    public internal(set) var state = GameState.ready
    /// The HUD font, loaded in `initialize`; without it the HUD draws nothing.
    public private(set) var font: HudFont?
    /// The driving music, loaded in `initialize` and started by the first Space.
    public private(set) var music: Sound?
    /// The impact, loaded in `initialize` and played once per ended run.
    public private(set) var impact: Sound?
    /// Seconds of the opening fade still to play; counts down on the fixed step.
    public internal(set) var fadeRemaining: Float = 0
    /// Seconds driven in this run: drives the speed ramp, reset by a replay.
    public internal(set) var runTime: Float = 0
    /// Whether the last Playing step shifted the world back (a test hook).
    public private(set) var rebasedLastStep = false
    /// What the render pass draws with, made in `initialize`.
    public private(set) var resources: RenderResources?

    /// Where the game's two messages go (Rust `println!`): stdout unless a driver
    /// collects them, as the trace does.
    public var output: (String) -> Void = { print($0) }

    /// The path of asset `name`.
    private let assetPath: (String) -> String

    /// The chase camera: 12 behind, 6 above, aiming 1.5 above the car, easing with
    /// a 0.052 s half-life.
    private let chase = ChaseController(
        distance: CHASE_DISTANCE, height: CHASE_HEIGHT, lookAtHeight: CHASE_LOOK_AT_HEIGHT, halfLife: CHASE_HALF_LIFE
    )

    /// A game not yet initialized, on its title screen, reading its assets
    /// through `assetPath` (the resolved asset root by default).
    public init(assetPath: @escaping (String) -> String = AssetRoot.path) {
        self.assetPath = assetPath
    }

    // MARK: - Pure helpers

    /// The x of lane `lane`'s centre: lanes centred on x = 0, `LANE_WIDTH` apart.
    public nonisolated static func laneX(_ lane: Int) -> Float {
        (Float(lane) - (Float(LANES) - 1.0) / 2.0) * LANE_WIDTH
    }

    /// The car's position, from its lane and `travel` (its z is `-travel`, so -0
    /// at the start). The streaming focus, the overlap test and the camera-locked
    /// terrain and skyline use it; the car's own draw uses its stored transform.
    public func playerPosition() -> SIMD3<Float> {
        SIMD3(CarRunner.laneX(lane), PLAYER_Y, -travel)
    }

    /// The forward speed this step: `SPEED` grown by `SPEED_RAMP` every
    /// `SPEED_RAMP_SECONDS` of the run, continuously, and capped at `SPEED_MAX`.
    public func currentSpeed() -> Float {
        let factor = powf(SPEED_RAMP, runTime / SPEED_RAMP_SECONDS)
        return RustF32.min(SPEED * factor, SPEED_MAX)
    }

    /// How opaque the opening fade is: 1 when a run starts, 0 once it is gone.
    public func fadeAlpha() -> Float {
        if HUD_FADE_SECONDS <= 0 {
            return 0
        }
        return RustF32.clamp(fadeRemaining / HUD_FADE_SECONDS, 0, 1)
    }

    /// The session's best score.
    public func bestScore() -> UInt64 {
        RustF32.toU64(RustF32.max(best, 0))
    }

    /// This run's score: one point per unit of distance.
    public func score() -> UInt64 {
        RustF32.toU64(RustF32.max(distance, 0))
    }

    /// Lane input for this step: edge-triggered, one lane per press, clamped at
    /// both edges; Left and Right together cancel.
    private func applyLaneInput(_ input: Input) {
        let left = input.isKeyJustPressed(.left)
        let right = input.isKeyJustPressed(.right)
        switch (left, right) {
        case (true, false): lane = lane > 0 ? lane - 1 : 0
        case (false, true): lane = Swift.min(lane + 1, LANES - 1)
        default: break
        }
    }

    /// Whether the car overlaps an obstacle at `obstaclePosition`: a box test in
    /// x and z only, strict at the edges.
    func overlaps(_ obstaclePosition: SIMD3<Float>) -> Bool {
        let p = playerPosition()
        let dx = abs(p.x - obstaclePosition.x)
        let dz = abs(p.z - obstaclePosition.z)
        return dx < PLAYER_HALF.x + OBSTACLE_HALF.x && dz < PLAYER_HALF.z + OBSTACLE_HALF.z
    }

    /// Whether the car overlaps any obstacle (`game.rs:455-460`): the query's
    /// entities in order, stopping at the first hit.
    private func hitAnyObstacle(_ ctx: EngineContext) -> Bool {
        ctx.world.query(Transform.self).with(Obstacle.self).contains { _, transform in overlaps(transform.position) }
    }

    // MARK: - Spawning

    /// Fill one streaming slot (`game.rs:370-448`), every entity tracked so the
    /// scene despawns it once it falls behind, in this order: the road tile; on
    /// every second slot but slot 0, an obstacle in a lane drawn from `rng`; a
    /// building on each side (prefab, jitter and facing from the slot alone); a
    /// guardrail segment on each side. Each `slot.spawn` is Rust's
    /// `cx.world.spawn(..)` and then `cx.spawned(e)`, with the same set of
    /// components as Rust's tuple.
    static func spawnSlot(rng: inout Rng, tileDepth: Float, _ slot: borrowing StreamSlot) {
        let roadZ = slot.position.z
        let roadWidth = Float(LANES) * LANE_WIDTH + LANE_WIDTH
        slot.spawn(
            Transform(position: SIMD3(0, -0.5, roadZ), scale: SIMD3(roadWidth, 0.4, tileDepth)),
            RenderComponent.cube(SIMD3(0.16, 0.16, 0.2)),
            StaticTag()
        )

        // Rust's `rem_euclid`, which is never negative.
        let remainder = ((slot.index % OBSTACLE_EVERY) + OBSTACLE_EVERY) % OBSTACLE_EVERY
        if slot.index != 0 && remainder == 0 {
            let lane = Int(rng.nextBelow(UInt32(LANES)))
            // From the slot, not the lane PRNG, so variety never moves an obstacle.
            let variant = variantForSlot(slot.index, TRAFFIC_CAR_ASSETS.count)
            slot.spawn(
                Transform(position: SIMD3(laneX(lane), PLAYER_Y, roadZ)),
                RenderComponent.cube(SIMD3(0.95, 0.8, 0.1)),
                StaticTag(),
                Obstacle(),
                TrafficVariant(variant)
            )
        }

        // A building on each side, on the ground below the raised road, turned to
        // face it: the left row (x < 0) faces +X, the right row -X.
        for side: UInt64 in 0..<2 {
            let sign: Float = side == 0 ? -1.0 : 1.0
            let variant = buildingForSlot(slot.index, side)
            let jitter = buildingJitter(slot.index, side)
            let x = sign * (BUILDING_SIDE_X + jitter * BUILDING_SIDE_JITTER)
            // glam's `from_rotation_y`, with Rust's `FRAC_PI_2` bits.
            let facing = simd_quatf(glamRotationY: -sign * RustF32.fracPi2)
            slot.spawn(
                Transform(position: SIMD3(x, GROUND_Y, roadZ), rotation: facing),
                RenderComponent.cube(SIMD3(0.5, 0.5, 0.5)),
                StaticTag(),
                BuildingVariant(variant)
            )
        }

        // A guardrail segment along each road edge, on the deck.
        for side: UInt64 in 0..<2 {
            let sign: Float = side == 0 ? -1.0 : 1.0
            slot.spawn(
                Transform(position: SIMD3(sign * GUARDRAIL_X, GUARDRAIL_DECK_Y, roadZ)),
                RenderComponent.cube(SIMD3(0.6, 0.6, 0.6)),
                StaticTag(),
                GuardrailTag()
            )
        }
    }

    /// One stream pass around `focus`, filling new slots with `spawnSlot`.
    /// `rng` advances in slot order, as Rust's `&mut self.rng` does.
    private func stream(_ ctx: EngineContext, focus: SIMD3<Float>) {
        let tileDepth = ctx.scene.config.spawnInterval
        var rng = self.rng
        ctx.scene.stream(focus: focus) { slot in CarRunner.spawnSlot(rng: &rng, tileDepth: tileDepth, slot) }
        self.rng = rng
    }

    // MARK: - Transitions

    /// Start the music (from the Ready → Playing edge only). It then loops for
    /// the rest of the session: a crash does not stop it and a replay does not
    /// restart it, and the audio's single loop channel would refuse a second copy
    /// anyway.
    private func startMusic(_ ctx: EngineContext) {
        if let music {
            ctx.audio.playLooping(music, volume: MUSIC_VOLUME)
        }
    }

    /// End the run: the impact (once: the next update returns before the
    /// collision test), the best score, the report, and `GameOver`.
    private func gameOver(_ ctx: EngineContext) {
        if let impact {
            ctx.audio.playOnce(impact, volume: IMPACT_VOLUME)
        }
        best = RustF32.max(best, distance)
        state = .gameOver
        output("GAME OVER — score \(score()) — best \(RustF32.toU64(best)). Press Space to replay.")
    }

    /// Replay from `GameOver`: clear the obstacles within `CLEAR_AHEAD` of the car
    /// (either way), recentre the lane, zero the score and the run clock, and play.
    /// `travel`, the PRNG, the fade, the camera and the music are left alone, so
    /// the road keeps scrolling unbroken.
    private func startNewRun(_ ctx: EngineContext) {
        let axis = ctx.scene.config.axis
        // Measured before the lane is reset.
        let playerAlong = glamDot(playerPosition(), axis)
        // Collected first, despawned after, in query order (`game.rs:337-350`).
        let victims = ctx.world.query(Transform.self).with(Obstacle.self)
            .filter { _, transform in abs(glamDot(transform.position, axis) - playerAlong) <= CLEAR_AHEAD }
            .map { entity, _ in entity }
        for entity in victims {
            ctx.scene.despawn(entity)
        }

        lane = START_LANE
        distance = 0
        runTime = 0
        state = .playing
        output("replay — best \(RustF32.toU64(best)). go!")
    }

    // MARK: - Game

    /// Build the world (`game.rs:464-612`), in Rust's order: the streaming
    /// reach and the camera's clip range, the player, the sounds, the sun,
    /// every model, the font and the two procedural meshes, then the first
    /// stream pass. Synchronous, as Rust's is: the models are loaded before it
    /// returns, so the first frame draws them.
    ///
    /// Only the streaming configuration, the player and the stream pass change
    /// the world, in that order; the loads change nothing it holds.
    ///
    /// - Throws: ``AssetLoadError`` for a model, ``FontError`` for the font, or
    ///   the engine's error, where Rust panics.
    public func initialize(_ ctx: EngineContext) throws {
        var streaming = ctx.scene.config
        streaming.spawnAhead = SPAWN_AHEAD
        streaming.despawnBehind = DESPAWN_BEHIND
        try ctx.scene.setConfig(streaming)
        ctx.camera.setClipPlanes(near: CAMERA_NEAR, far: CAMERA_FAR)

        // `game.rs:267-273`.
        player = ctx.world.spawn(
            Transform(position: playerPosition()),
            RenderComponent.cube(SIMD3(0.9, 0.15, 0.1)),
            DynamicTag()
        )

        // Loaded here, once, and only played on a game event.
        let audio = ctx.audio
        audio.masterVolume = MASTER_VOLUME
        music = audio.load(assetPath(MUSIC_ASSET))
        impact = audio.load(assetPath(IMPACT_ASSET))

        let tileDepth = ctx.scene.config.spawnInterval
        // The first renderer call pushes the camera the clip planes moved, as
        // Rust's `renderer()` does.
        let renderer = ctx.renderer
        // The sun is sticky: set once, before the first frame opens.
        try renderer.setSunSky(DEMO_SUN)

        let models = try DemoModels.load(renderer, assetPath: assetPath)
        font = try HudFont.load(renderer, path: assetPath(FONT_ASSET))
        resources = try RenderResources.make(models: models, tileDepth: tileDepth, renderer: renderer)

        // Prime the road ahead so the first frame is not empty.
        stream(ctx, focus: playerPosition())
    }

    /// One fixed step (`game.rs:614-733`).
    ///
    /// Ready and GameOver only wait for Space and keep the camera on the car.
    /// Playing, in this order: the fade, the lane input, the run clock and the
    /// move, the rebase (and the same shift for `travel` and the camera), the
    /// car's transform, the camera follow, the stream pass, the collision test.
    public func update(_ ctx: EngineContext, dt: Float) {
        if state == .ready {
            if ctx.input.isKeyJustPressed(.space) {
                state = .playing
                fadeRemaining = HUD_FADE_SECONDS
                startMusic(ctx)
            }
            ctx.camera.follow(chase, target: playerPosition(), forward: FORWARD, dt: dt)
            return
        }

        if state == .gameOver {
            if ctx.input.isKeyJustPressed(.space) {
                startNewRun(ctx)
            }
            ctx.camera.follow(chase, target: playerPosition(), forward: FORWARD, dt: dt)
            return
        }

        fadeRemaining = RustF32.max(fadeRemaining - dt, 0)
        applyLaneInput(ctx.input)

        // The clock first, then this step's speed.
        runTime += dt
        let speed = currentSpeed()
        travel += speed * dt
        distance += speed * dt

        // Rebase first, against the focus (the engine shifts every transform);
        // fold the same shift into `travel`, and carry the camera through the
        // coordinate change (translating it keeps the follow's lag, so the rebase
        // is invisible).
        let axis = ctx.scene.config.axis
        let offset = ctx.scene.maybeRebase(focus: playerPosition())
        rebasedLastStep = offset != .zero
        if offset != .zero {
            travel += glamDot(offset, axis)
            let camera = ctx.camera
            // Lane-wise IEEE adds, as glam's `Vec3 + Vec3`.
            let position = camera.position + offset
            let target = camera.target + offset
            camera.position = position
            camera.target = target
        }

        // The car's transform, now consistent with any rebase.
        let playerPos = playerPosition()
        if let player {
            ctx.world.modify(Transform.self, of: player) { $0.position = playerPos }
        }

        ctx.camera.follow(chase, target: playerPos, forward: FORWARD, dt: dt)
        stream(ctx, focus: playerPos)

        if hitAnyObstacle(ctx) {
            gameOver(ctx)
        }
    }

    /// Draw the frame the engine opened: the scene from the world, then the HUD
    /// (`game.rs:735-737`, `render.rs:40-163`).
    public func render(_ ctx: EngineContext, into frame: Frame) throws {
        guard let resources else {
            preconditionFailure("render resources are made in initialize")
        }
        let list = SceneDrawList(world: ctx.world, player: player, playerPosition: playerPosition())
        try recordScene(list, resources: resources, frame: frame)
        try drawHud(
            HudState(state: state, score: score(), bestScore: bestScore(), fadeAlpha: fadeAlpha()),
            font: font,
            frame: frame
        )
    }
}
