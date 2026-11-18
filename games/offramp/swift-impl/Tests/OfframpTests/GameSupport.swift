// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Foundation
import StickShift
import Testing
import simd

@testable import OfframpGame

// The helpers of `game.rs`'s and `hud.rs`'s tests (spec 02 §2), over the engine
// run headlessly, as Rust's `Headless` runs the Rust game. A Rust `panic!` in a
// helper records an issue and throws.

/// Rust's `Headless` for one game: the engine over a null context (1280 × 720
/// at scale 1 unless the test gives another), silent, one fixed step per frame.
@MainActor
final class Headless {
    /// The game.
    let game: CarRunner
    /// The engine running it.
    let engine: Engine
    /// Frames that did not run one update and render a submitted frame without
    /// a render error: what Rust's `protocol_violations()` would hold.
    private(set) var lostFrames: [String] = []

    /// An engine for `game` on `context`, or on a fresh null context of 1280 ×
    /// 720 at scale 1. No hook runs yet.
    init(_ game: CarRunner, context: Context? = nil) throws {
        self.game = game
        engine = try Engine(context: context ?? Context(offscreenWidth: 1280, height: 720, backend: .null), game: game)
    }

    /// `Headless::run`: `initialize` if it has not run, then up to `frames`
    /// fixed-step frames, stopping before a frame once exit is requested.
    func run(frames: Int) throws {
        try engine.initialize()
        for _ in 0..<frames {
            if engine.state.exitRequested { break }
            let tick = engine.advance()
            if tick.steps != 1 || !tick.rendered || !tick.submitted || engine.lastRenderError != nil {
                let error = String(describing: engine.lastRenderError)
                lostFrames.append("frame \(engine.state.frames): \(tick), render error \(error)")
            }
        }
    }

    var context: Context { engine.context }
    var input: HostInput { engine.input }
    var world: World { engine.world }
    var scene: Scene { engine.scene }
    var camera: EngineCamera { engine.camera }
    var audio: Audio { engine.audio }
    /// The frames run.
    var framesRun: Int { Int(engine.state.frames) }

    /// Rust's `protocol_violations().is_empty()`: every frame updated once and
    /// rendered a submitted frame, nothing threw, and the engine skipped no draw
    /// and no frame.
    var isClean: Bool {
        let s = context.stats
        return lostFrames.isEmpty && s.drawsSkipped == 0 && s.framesSkipped == 0
    }

    /// The HUD's quads of the last frame, in record order.
    var lastQuads: [FrameRecord.RecordedQuad] { context.lastFrame.quads }
}

/// A helper's precondition failed (Rust panics).
struct HarnessError: Error, CustomStringConvertible {
    let description: String
}

/// Record `message` as an issue and throw it.
func fail(_ message: String) -> HarnessError {
    Issue.record(Comment(rawValue: message))
    return HarnessError(description: message)
}

/// `game.rs`'s `started`: a new game, Space pressed for its first frame (init,
/// then the Ready step that starts the run), then released.
@MainActor
func started() throws -> (CarRunner, Headless) {
    let game = CarRunner()
    let h = try Headless(game)
    h.input.press(.space)
    try h.run(frames: 1)
    h.input.release(.space)
    try #require(game.state == .playing, "the start key begins the run")
    return (game, h)
}

/// `hud.rs`'s `booted`: `initialize` only (`run(0)`), on its title screen, on
/// `context` (a default null one if `nil`).
@MainActor
func booted(_ context: Context? = nil) throws -> (CarRunner, Headless) {
    let game = CarRunner()
    let h = try Headless(game, context: context)
    try h.run(frames: 0)
    try #require(game.state == .ready, "the demo opens on its title screen")
    return (game, h)
}

/// `hud.rs`'s `started`: `booted`, then Space for one frame.
@MainActor
func hudStarted() throws -> (CarRunner, Headless) {
    let (game, h) = try booted()
    h.input.press(.space)
    try h.run(frames: 1)
    h.input.release(.space)
    try #require(game.state == .playing, "the start key begins the run")
    return (game, h)
}

/// `lane_input_clamps_at_both_edges`'s `tap`: press, one frame, release, one
/// frame, so each tap is one edge.
@MainActor
func tap(_ h: Headless, _ key: Key) throws {
    h.input.press(key)
    try h.run(frames: 1)
    h.input.release(key)
    try h.run(frames: 1)
}

/// `game.rs`'s `drive_to_game_over`: `started`, then the centre runner until it
/// crashes, at most 600 frames.
@MainActor
func driveToGameOver() throws -> (CarRunner, Headless) {
    let (game, h) = try started()
    for _ in 0..<600 {
        try h.run(frames: 1)
        if game.state == .gameOver {
            return (game, h)
        }
    }
    throw fail("the center runner never crashed in 600 frames")
}

/// `drive_until_over`: up to `limit` frames; whether the run ended.
@MainActor
func driveUntilOver(_ game: CarRunner, _ h: Headless, limit: Int) throws -> Bool {
    for _ in 0..<limit {
        try h.run(frames: 1)
        if game.state == .gameOver {
            return true
        }
    }
    return false
}

/// An obstacle's position in whole centimetres, ordered as Rust orders the
/// `(i64, i64)` tuple.
struct Centimetres: Comparable, CustomStringConvertible {
    let x: Int64
    let z: Int64

    static func < (a: Centimetres, b: Centimetres) -> Bool {
        (a.x, a.z) < (b.x, b.z)
    }

    var description: String { "(\(x), \(z))" }
}

/// `obstacle_layout`: the obstacles a seeded run streamed after Space and 120
/// frames, in centimetres (`(p * 100).round() as i64`), sorted.
@MainActor
func obstacleLayout(seed: UInt64) throws -> [Centimetres] {
    let game = CarRunner()
    game.rng = Rng(seed: seed)
    let h = try Headless(game)
    h.input.press(.space)
    try h.run(frames: 1)
    h.input.release(.space)
    try h.run(frames: 120)
    return h.world.query(Transform.self).with(Obstacle.self).map { _, t in
        let p = t.position
        return Centimetres(
            x: RustF32.toI64(Double((p.x * 100).rounded())), z: RustF32.toI64(Double((p.z * 100).rounded()))
        )
    }.sorted()
}

/// The dodge step of `a_dodging_run_never_ends_without_an_actual_collision`:
/// mark every lane with traffic in the danger window (2 behind to 14 ahead), and
/// if the car's lane is one, move it straight to the lowest free lane.
@MainActor
func dodge(_ game: CarRunner, _ h: Headless) {
    let player = game.playerPosition()
    var blocked = [Bool](repeating: false, count: LANES)
    for (_, t) in h.world.query(Transform.self).with(Obstacle.self) {
        let p = t.position
        if p.z < player.z + 2.0 && p.z > player.z - 14.0 {
            for lane in 0..<LANES where abs(p.x - CarRunner.laneX(lane)) < 1.0 {
                blocked[lane] = true
            }
        }
    }
    if blocked[game.lane], let free = (0..<LANES).first(where: { !blocked[$0] }) {
        game.lane = free
    }
}

/// `x` to `places` decimals, as Rust's `{:.N}`.
func fixed(_ x: Float, _ places: Int) -> String {
    String(format: "%.\(places)f", Double(x))
}

// MARK: - The HUD

/// One recorded HUD quad, as the trace and the probe see it.
typealias HudQuad = FrameRecord.RecordedQuad

extension FrameRecord.RecordedQuad.Fill {
    /// A glyph of text.
    var isSDF: Bool { self == .sdf }
}

/// `hud_frame`: drive one frame and return the overlay quads it recorded.
@MainActor
func hudFrame(_ h: Headless) throws -> [HudQuad] {
    try h.run(frames: 1)
    return h.lastQuads
}

/// `font_of`: the loaded HUD font, or a failure.
@MainActor
func fontOf(_ game: CarRunner) throws -> HudFont {
    try #require(game.font, "init loads the committed SDF atlas; without it draw_hud is a no-op")
}

/// The committed font's glyph table, for counting what a text draws.
func committedFontFile() throws -> FontFile {
    try FontFile.load(path: demoAssetPath(FONT_ASSET))
}

extension FontFile {
    /// The metrics for `c`, or the fallback outside the table's range (as the
    /// engine's `FontAtlas::glyph` looks them up).
    func glyph(_ c: Unicode.Scalar) -> Glyph {
        guard c.value >= firstChar.value else { return FontFile.fallbackGlyph }
        let i = Int(c.value - firstChar.value)
        return i < glyphs.count ? glyphs[i] : FontFile.fallbackGlyph
    }

    /// Whether `c` draws a quad (a glyph with a non-zero area).
    func isVisible(_ c: Unicode.Scalar) -> Bool {
        let g = glyph(c)
        return g.size.x > 0 && g.size.y > 0
    }
}

/// `visible_glyphs`: how many of `text`'s characters draw a quad.
func visibleGlyphs(_ text: String) throws -> Int {
    let file = try committedFontFile()
    return text.unicodeScalars.filter(file.isVisible).count
}

/// The surface `h`'s context lays the HUD out on.
@MainActor
func hudSurface(_ h: Headless) -> HudSurface {
    let s = h.context.surface
    return HudSurface(width: s.width, height: s.height, safeAreaInsets: s.safeArea, scale: s.scale)
}

/// `hud.rs`'s `drive_to_game_over`: from the first Playing frame until the run
/// ends, at most 600 frames; the last Playing frame's quads and the GameOver
/// frame's.
@MainActor
func driveToGameOverHUD() throws -> (CarRunner, Headless, playing: [HudQuad], gameOver: [HudQuad]) {
    let (game, h) = try hudStarted()
    var playing: [HudQuad]?
    for _ in 0..<600 {
        let quads = try hudFrame(h)
        if game.state == .gameOver {
            guard let playing else { throw fail("a Playing frame always precedes the crash frame") }
            return (game, h, playing, quads)
        }
        playing = quads
    }
    throw fail("the center runner never crashed in 600 frames")
}

/// The first index at which `a` and `b` differ, comparing up to the shorter.
func firstDifference<T: Equatable>(_ a: [T], _ b: [T]) -> Int? {
    (0..<min(a.count, b.count)).first { a[$0] != b[$0] }
}

/// The fixed step every update gets: the `Float` nearest 1/60.
let FIXED_DT = Engine.fixedDeltaTime

/// A headless engine for `game`, run for `frames` frames (0: `initialize` only).
@MainActor
func headlessRun(_ game: CarRunner, frames: Int) throws -> Headless {
    let h = try Headless(game)
    try h.run(frames: frames)
    return h
}

extension SunSky {
    /// The unit vector from the scene toward the sun (`SunSky::toward_sun`,
    /// `sun.rs:100-106`): azimuth 0 is -Z and turns toward +X, so
    /// `(sin az · cos el, sin el, -cos az · cos el)`.
    var towardSun: SIMD3<Float> {
        let el = RustF32.toRadians(elevation)
        let az = RustF32.toRadians(azimuth)
        return SIMD3(sinf(az) * cosf(el), sinf(el), -cosf(az) * cosf(el))
    }

    /// The direction the sunlight travels (`SunSky::direction`): `-towardSun`.
    var direction: SIMD3<Float> {
        -towardSun
    }
}
