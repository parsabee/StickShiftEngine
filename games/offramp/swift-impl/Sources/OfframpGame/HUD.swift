// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import StickShift

// The HUD, a port of `games/offramp/rust-impl/src/hud.rs` (spec 03 §9): the
// full-screen wash, the live score and the centred banner, drawn as overlay
// quads after the 3D scene.
//
// The layout is game code and is here: where each run of text starts and how
// big it is, every position the same `Float` operations in the same
// association as the Rust code. The glyphs are the engine's: it measures the
// text and lays it out into SDF quads (`FontAtlas::measure` and `layout`, the
// functions the Rust HUD calls).

/// A fixed-capacity string (Rust's `StackStr<N>`): a chunk that does not fit
/// whole is dropped, never split, and never an error.
public struct StackString: Equatable, Sendable {
    /// The most bytes it holds.
    public let capacity: Int
    /// The text so far.
    public private(set) var string = ""
    private var length = 0

    /// An empty string holding at most `capacity` bytes.
    public init(capacity: Int) {
        self.capacity = capacity
    }

    /// Append `chunk` if all of its UTF-8 bytes fit; otherwise drop it.
    public mutating func write(_ chunk: String) {
        let end = length + chunk.utf8.count
        if end <= capacity {
            string += chunk
            length = end
        }
    }

    /// `write!(s, "{prefix}{value}")`: Rust's formatter hands the literal and the
    /// number over as two chunks.
    public mutating func write(_ prefix: String, _ value: UInt64) {
        write(prefix)
        write(String(value))
    }
}

/// What the HUD shows: the game state it reads each frame.
public struct HudState: Equatable, Sendable {
    /// The run's state.
    public var state: GameState
    /// `CarRunner::score`: this run's distance, truncated.
    public var score: UInt64
    /// `CarRunner::best_score`: the session's best distance, truncated.
    public var bestScore: UInt64
    /// `CarRunner::fade_alpha`: how much of the opening black is left, `0...1`.
    public var fadeAlpha: Float

    /// Creates the HUD's inputs.
    public init(state: GameState, score: UInt64 = 0, bestScore: UInt64 = 0, fadeAlpha: Float = 0) {
        self.state = state
        self.score = score
        self.bestScore = bestScore
        self.fadeAlpha = fadeAlpha
    }
}

/// The drawable the HUD lays out on.
public struct HudSurface: Equatable, Sendable {
    /// Width in pixels.
    public var width: Int
    /// Height in pixels.
    public var height: Int
    /// Top, right, bottom, left, in pixels.
    public var safeAreaInsets: SIMD4<Float>
    /// Drawable pixels per point.
    public var scale: Float

    /// Creates a surface.
    public init(width: Int, height: Int, safeAreaInsets: SIMD4<Float> = .zero, scale: Float = 1) {
        self.width = width
        self.height = height
        self.safeAreaInsets = safeAreaInsets
        self.scale = scale
    }
}

/// One thing the HUD draws, in record order.
public enum HudCommand: Equatable, Sendable {
    /// A solid quad.
    case solid(rect: SIMD4<Float>, color: SIMD4<Float>)
    /// A run of text, the pen starting at `origin` (the baseline's left end).
    case text(String, origin: SIMD2<Float>, px: Float, color: SIMD4<Float>)
}

/// The HUD of one frame, as commands in record order (`hud.rs:174-271`).
///
/// 1. The wash, over the whole drawable: opaque black on the title screen, the
///    retiring fade while playing (none once it is gone), the dim on game over.
/// 2. The live score in the top-left of the safe area, once a run has started.
/// 3. The banner, centred: the title and prompt, or the game-over lines.
///
/// `lineHeight` is the font's, in em units, and `measure` the width of a text
/// at a size in pixels per em (the engine's `FontAtlas::measure`). Rust
/// interleaves each banner line's measure with its layout; here every measure
/// runs first, which records the same quads, since measuring draws nothing.
public func hudCommands(
    _ hud: HudState,
    lineHeight: Float,
    surface: HudSurface,
    measure: (String, Float) throws -> Float
) rethrows -> [HudCommand] {
    let screenW = Float(surface.width)
    let screenH = Float(surface.height)
    let insetTop = surface.safeAreaInsets[0]
    let insetBottom = surface.safeAreaInsets[2]
    let insetLeft = surface.safeAreaInsets[3]
    // Point-authored sizes to drawable pixels.
    let ui = surface.scale
    let margin = HUD_MARGIN * ui
    let scorePx = HUD_SCORE_PX * ui
    let titlePx = HUD_TITLE_PX * ui
    let bannerPx = HUD_BANNER_PX * ui

    var commands: [HudCommand] = []

    // The wash first: overlay quads blend in record order.
    let wash: SIMD4<Float>?
    switch hud.state {
    case .ready:
        wash = HUD_BLACK_COLOR
    case .playing:
        let alpha = hud.fadeAlpha
        if alpha > 0 {
            var c = HUD_BLACK_COLOR
            c[3] = alpha
            wash = c
        } else {
            wash = nil
        }
    case .gameOver:
        wash = HUD_DIM_COLOR
    }
    if let wash {
        commands.append(.solid(rect: SIMD4(0, 0, screenW, screenH), color: wash))
    }

    // The live score: the layout origin is the baseline, one em below the inset.
    if hud.state != .ready {
        var scoreText = StackString(capacity: 32)
        scoreText.write("SCORE ", hud.score)
        commands.append(
            .text(scoreText.string, origin: SIMD2(insetLeft + margin, (insetTop + margin) + scorePx), px: scorePx, color: HUD_TEXT_COLOR)
        )
    }

    let banner: [(String, Float)]
    switch hud.state {
    case .playing:
        banner = []
    case .ready:
        banner = [("OFFRAMP", titlePx), ("PRESS SPACE TO START", bannerPx)]
    case .gameOver:
        var finalText = StackString(capacity: 32)
        var bestText = StackString(capacity: 32)
        finalText.write("SCORE ", hud.score)
        bestText.write("BEST ", hud.bestScore)
        banner = [
            ("GAME OVER", titlePx), (finalText.string, bannerPx), (bestText.string, bannerPx),
            ("PRESS SPACE TO REPLAY", bannerPx),
        ]
    }
    if banner.isEmpty {
        return commands
    }

    // The block of lines centred on the safe area's vertical centre; each line
    // centred on the full width.
    let centerY = insetTop + (((screenH - insetTop) - insetBottom) * 0.5)
    // Rust's `f32` sum folds from -0.0.
    var blockH: Float = -0.0
    for (_, px) in banner {
        blockH = blockH + (px * lineHeight)
    }
    var baseline = (centerY - (blockH * 0.5)) + banner[0].1
    for (text, px) in banner {
        let width = try measure(text, px)
        commands.append(.text(text, origin: SIMD2((screenW - width) * 0.5, baseline), px: px, color: HUD_TEXT_COLOR))
        baseline = baseline + (px * lineHeight)
    }
    return commands
}

/// `CarRunner::draw_hud`: draw this frame's HUD into `frame`, laid out on its
/// surface (`surface_size`, `safe_area_insets`, `surface_scale`). Without a
/// font it draws nothing, not even the wash, as in Rust (`hud.rs:175-177`).
@MainActor
public func drawHud(_ hud: HudState, font: HudFont?, frame: Frame) throws {
    guard let font else { return }
    let s = frame.surface
    let surface = HudSurface(width: s.width, height: s.height, safeAreaInsets: s.safeArea, scale: s.scale)
    let commands = try hudCommands(hud, lineHeight: font.lineHeight, surface: surface) { text, px in
        try font.font.width(of: text, pixelSize: px)
    }
    for command in commands {
        switch command {
        case let .solid(rect, color):
            // `OverlayQuad::solid`: uv [0, 0, 1, 1].
            try frame.draw(Quad(rect: rect, color: color))
        case let .text(text, origin, px, color):
            try frame.drawText(text, font: font.font, at: origin, pixelSize: px, color: color)
        }
    }
}
