// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import StickShift
import Testing

@testable import OfframpGame

/// The HUD (`hud.rs`): its layout against the Rust probe's quads and spec 03
/// §9.3's fixtures, the `StackStr` tests, and the assertions of `hud.rs`'s
/// headless tests applied to the HUD of each state; then those tests themselves,
/// driving the game. The glyphs are the engine's (`FontAtlas::layout` and
/// `measure`): every quad checked here is one the engine recorded.
@MainActor
@Suite("hud")
struct HUDTests {
    private static let surface = HudSurface(width: 1280, height: 720)
    /// The committed font's glyph table.
    private let file: FontFile
    /// A null context of `surface`'s size, and the font uploaded to it.
    private let context: Context
    private let font: HudFont

    init() throws {
        file = try committedFontFile()
        context = try nullContext()
        font = try loadHudFont(context)
    }

    /// The quads `drawHud` records for `hud` on a drawable of `surface`.
    private func quads(_ hud: HudState, surface: HudSurface = surface) throws -> [HudQuad] {
        let (context, font) =
            surface == Self.surface
            ? (self.context, self.font)
            : try {
                let context = try nullContext(width: surface.width, height: surface.height, scale: surface.scale)
                try context.setSafeArea(surface.safeAreaInsets)
                return (context, try loadHudFont(context))
            }()
        return try recordedFrame(context) { try drawHud(hud, font: font, frame: $0) }.quads
    }

    /// The HUD's commands for `hud` on `surface`, measured by the engine.
    private func commands(_ hud: HudState, surface: HudSurface = surface) throws -> [HudCommand] {
        try hudCommands(hud, lineHeight: font.lineHeight, surface: surface) { text, px in
            try font.font.width(of: text, pixelSize: px)
        }
    }

    /// The engine's width of `text` at `px`.
    private func measure(_ text: String, px: Float) throws -> Float {
        try font.font.width(of: text, pixelSize: px)
    }

    /// `hud.rs`'s `visible_glyphs`: the characters of `text` that draw a quad.
    private func visible(_ text: String) -> Int {
        text.unicodeScalars.filter(file.isVisible).count
    }

    /// `hud.rs`'s `assert_wash_first`: the leading quad is the full-drawable wash
    /// and nothing after it is solid. Returns its colour.
    private func washFirst(_ quads: [HudQuad], _ surface: HudSurface, _ state: String) throws -> SIMD4<Float> {
        try #require(!quads.isEmpty, "\(state) records a background wash quad")
        #expect(quads[0].fill == .solid, "\(state): the wash is an untextured quad")
        #expect(
            quads[0].rect == SIMD4(0, 0, Float(surface.width), Float(surface.height)),
            "\(state): the wash covers the whole drawable"
        )
        #expect(
            quads.dropFirst().allSatisfy { $0.fill != .solid },
            "\(state): only the leading quad is the wash; everything after it is text"
        )
        return quads[0].color
    }

    /// `hud.rs`'s `assert_within_surface`.
    private func withinSurface(_ quads: [HudQuad], _ surface: HudSurface, _ state: String) {
        let (w, h) = (Float(surface.width), Float(surface.height))
        for (i, q) in quads.enumerated() {
            #expect(q.rect.z > 0 && q.rect.w > 0, "\(state): quad \(i) has an empty rect \(q.rect)")
            #expect(
                q.rect.x >= 0 && q.rect.y >= 0 && q.rect.x + q.rect.z <= w && q.rect.y + q.rect.w <= h,
                "\(state): quad \(i) at \(q.rect) falls outside the \(w)x\(h) drawable"
            )
        }
    }

    /// `hud.rs`'s `in_score_corner`: the left half and top quarter.
    private func inScoreCorner(_ q: HudQuad, _ surface: HudSurface) -> Bool {
        q.rect.x >= 0 && q.rect.x < Float(surface.width) * 0.5 && q.rect.y >= 0
            && q.rect.y + q.rect.w <= Float(surface.height) * 0.25
    }

    /// The quads equal the probe's, bit for bit where the probe printed them.
    private func expectProbe(_ quads: [HudQuad], _ probe: [ProbeQuad]) {
        #expect(quads.count == probe.count)
        for (i, (q, p)) in zip(quads, probe).enumerated() {
            #expect(bits(q.rect) == bits(p.rect), "quad \(i): \(q.rect) vs \(p.rect)")
            if let color = p.color {
                #expect(bits(q.color) == bits(color), "quad \(i): colour \(q.color) vs \(color)")
            }
            if let fill = p.fill {
                #expect(fill == (q.fill == .solid ? "Solid" : "Sdf(6)"), "quad \(i): fill \(q.fill) vs \(fill)")
            }
        }
    }

    // MARK: - StackStr

    @Test("stack_str_formats_without_allocating") func stackStrFormatsWithoutAllocating() {
        var s = StackString(capacity: 32)
        s.write("SCORE ", 1234)
        #expect(s.string == "SCORE 1234")
    }

    @Test("stack_str_drops_overflow_instead_of_panicking") func stackStrDropsOverflowInsteadOfPanicking() {
        // Capacity 8: the first write fits, the second does not and is dropped.
        var s = StackString(capacity: 8)
        s.write("12345678")
        s.write("overflow")
        #expect(s.string == "12345678", "the overflowing chunk is dropped")
        // A chunk is dropped whole, never split; a later one that fits still lands.
        var t = StackString(capacity: 8)
        t.write("SCORE ", 123)
        #expect(t.string == "SCORE ", "the number did not fit, so it was dropped")
        t.write("12")
        #expect(t.string == "SCORE 12")
        // The longest readout fits the HUD's 32 bytes.
        var longest = StackString(capacity: 32)
        longest.write("SCORE ", .max)
        #expect(longest.string == "SCORE 18446744073709551615")
    }

    // MARK: - Each state against the Rust frames

    @Test func theReadyHUDIsTheRustFrame() throws {
        let surface = Self.surface
        let quads = try quads(HudState(state: .ready))
        expectProbe(quads, try probeQuads(after: "Ready frame quads: 34", count: 34))

        // ready_washes_the_scene_black_and_draws_the_title_without_a_score
        let wash = try washFirst(quads, surface, "Ready")
        #expect(wash == HUD_BLACK_COLOR, "Ready washes to opaque black")
        withinSurface(quads, surface, "Ready")
        #expect(
            quads.count - 1 == visible("STICKSHIFT RUNNER") + visible("PRESS SPACE TO START"),
            "Ready draws exactly the title + prompt banner, and no score"
        )
        #expect(
            !quads.contains { $0.fill.isSDF && inScoreCorner($0, surface) },
            "Ready draws no text in the live-score corner"
        )
        let atlas = font.font.atlas.id
        #expect(
            quads.dropFirst().allSatisfy { $0.color == HUD_TEXT_COLOR && $0.fill == .sdf && $0.texture == atlas },
            "the text is SDF glyphs from the font's atlas"
        )
    }

    @Test func theOpeningFadeIsPartialAndRetiring() throws {
        // the_opening_fade_wash_is_partial_and_retiring, on the first three
        // Playing frames' alphas (the probe's `first alphas`), score 0.
        let surface = Self.surface
        var previous: Float = 1
        for (frame, alpha) in [Float(0.977_777_8), 0.955_555_6, 0.933_333_4].enumerated() {
            let quads = try quads(HudState(state: .playing, score: 0, fadeAlpha: alpha))
            #expect(quads.count == 7, "fade frame \(frame): the wash and \"SCORE 0\"")
            let wash = try washFirst(quads, surface, "Playing (mid-fade)")
            withinSurface(quads, surface, "Playing (mid-fade)")
            #expect(
                SIMD3(wash.x, wash.y, wash.z) == SIMD3(HUD_BLACK_COLOR.x, HUD_BLACK_COLOR.y, HUD_BLACK_COLOR.z),
                "the fade varies only black's alpha, never its color"
            )
            #expect(bits(wash.w) == bits(alpha), "the wash's alpha is fade_alpha()")
            #expect(wash.w > 0 && wash.w < 1, "fade frame \(frame): alpha is partial, got \(wash.w)")
            #expect(wash.w < previous, "fade frame \(frame): the fade retires (\(wash.w) is not below \(previous))")
            previous = wash.w
            #expect(
                quads.dropFirst().contains { inScoreCorner($0, surface) },
                "fade frame \(frame): the live score draws under the fade"
            )
        }
        // The last frame with any fade left (alpha 5.46e-8) still washes; 0 does not.
        #expect(try quads(HudState(state: .playing, fadeAlpha: 5.463_759_2e-8)).first?.fill == .solid)
        #expect(try quads(HudState(state: .playing, fadeAlpha: 0)).first?.fill != .solid)
    }

    @Test func thePlayingHUDPastTheFadeIsTheRustFrame() throws {
        // playing_past_the_fade_draws_the_score_and_no_wash: the first fade-free
        // frame drew "SCORE 10".
        let surface = Self.surface
        let quads = try quads(HudState(state: .playing, score: 10, fadeAlpha: 0))
        expectProbe(quads, try probeQuads(after: "first fade-free Playing frame", count: 7))
        #expect(
            quads.allSatisfy { $0.fill != .solid },
            "a fade-free Playing frame records no wash quad, only score glyphs"
        )
        withinSurface(quads, surface, "Playing")
        #expect(quads.count == visible("SCORE 10"), "Playing draws exactly the live score readout")
        #expect(quads.allSatisfy { $0.fill.isSDF }, "score text is SDF glyphs, not solid quads")
        #expect(quads.allSatisfy { inScoreCorner($0, surface) }, "the live score sits in the top-left safe corner")
    }

    @Test func theGameOverHUDIsTheRustFrame() throws {
        // game_over_dims_the_scene_and_adds_a_banner: the centre runner's crash,
        // score 23 and best 23.
        let surface = Self.surface
        let quads = try quads(HudState(state: .gameOver, score: 23, bestScore: 23))
        expectProbe(quads, try probeQuads(after: "GameOver frame after 98 frames", count: 47))

        let wash = try washFirst(quads, surface, "GameOver")
        #expect(wash == HUD_DIM_COLOR, "GameOver dims rather than blacks out")
        #expect(wash.w > 0 && wash.w < 1, "the game-over wash is partial so the scene reads through it, got \(wash.w)")
        withinSurface(quads, surface, "GameOver")
        let playing = try self.quads(HudState(state: .playing, score: 22))
        #expect(quads.count > playing.count, "the game-over banner adds quads (\(quads.count) vs \(playing.count))")
        let expected = ["SCORE 23", "GAME OVER", "SCORE 23", "BEST 23", "PRESS SPACE TO REPLAY"].map(visible).reduce(0, +)
        #expect(quads.count - 1 == expected, "GameOver draws the live score plus the four banner lines")
        let banner = quads.dropFirst().filter { !inScoreCorner($0, surface) }
        #expect(!banner.isEmpty, "the game-over banner is drawn")
        let height = Float(surface.height)
        #expect(
            banner.allSatisfy { $0.rect.y >= height * 0.25 && $0.rect.y + $0.rect.w <= height * 0.75 },
            "the banner block is centred, not top-aligned"
        )
    }

    // MARK: - Spec 03 §9.3

    @Test func theLayoutMatchesTheSpecFixtures() throws {
        // The live score's origin, and its first glyph 'S'.
        let playing = try commands(HudState(state: .playing, score: 0))
        #expect(playing == [.text("SCORE 0", origin: SIMD2(24, 54), px: 30, color: HUD_TEXT_COLOR)])
        let score = try quads(HudState(state: .playing, score: 0))
        #expect(score.count == 6)
        #expect(score[0].rect == SIMD4(20.875, 28.375, 23.75, 30.0))
        #expect(score[0].uv == SIMD4(0.386_718_75, 0.328_125, 0.460_937_5, 0.421_875))

        // Ready: centre 360, block 112.80000305; the two lines' origins.
        let ready = try commands(HudState(state: .ready))
        #expect(
            ready == [
                .solid(rect: SIMD4(0, 0, 1280, 720), color: HUD_BLACK_COLOR),
                .text("STICKSHIFT RUNNER", origin: SIMD2(333.015_625, 367.600_006_10), px: 64, color: HUD_TEXT_COLOR),
                .text("PRESS SPACE TO START", origin: SIMD2(472.246_093_75, 444.400_024_41), px: 30, color: HUD_TEXT_COLOR),
            ]
        )
        #expect(bits(try measure("STICKSHIFT RUNNER", px: 64)) == bits(613.968_75))
        #expect(bits(try measure("PRESS SPACE TO START", px: 30)) == bits(335.507_812_5))
        #expect(visible("STICKSHIFT RUNNER") == 16 && visible("PRESS SPACE TO START") == 17)
        // block_h = 64 * 1.2 + 30 * 1.2, so baseline 0 = (360 - block_h / 2) + 64.
        let blockH: Float = (-0.0 + 64 * font.lineHeight) + 30 * font.lineHeight
        #expect(bits(blockH) == bits(112.800_003_05))

        // GameOver with score 123 and best 456.
        let over = try commands(HudState(state: .gameOver, score: 123, bestScore: 456))
        #expect(
            over == [
                .solid(rect: SIMD4(0, 0, 1280, 720), color: HUD_DIM_COLOR),
                .text("SCORE 123", origin: SIMD2(24, 54), px: 30, color: HUD_TEXT_COLOR),
                .text("GAME OVER", origin: SIMD2(462.968_75, 331.600_006_10), px: 64, color: HUD_TEXT_COLOR),
                .text("SCORE 123", origin: SIMD2(564.230_957_03, 408.400_024_41), px: 30, color: HUD_TEXT_COLOR),
                .text("BEST 456", origin: SIMD2(575.261_230_47, 444.400_024_41), px: 30, color: HUD_TEXT_COLOR),
                .text("PRESS SPACE TO REPLAY", origin: SIMD2(463.977_050_78, 480.400_024_41), px: 30, color: HUD_TEXT_COLOR),
            ]
        )
    }

    @Test func atTwiceTheScaleEveryGlyphIsTwiceTheSize() throws {
        // hud_keeps_its_on_screen_size_at_every_render_scale, on a 2560×1440
        // drawable at 2 pixels per point.
        let big = HudSurface(width: 2560, height: 1440, scale: 2)
        let one = try quads(HudState(state: .ready)).filter { $0.fill.isSDF }.map(\.rect.w)
        let two = try quads(HudState(state: .ready), surface: big).filter { $0.fill.isSDF }.map(\.rect.w)
        #expect(!one.isEmpty)
        #expect(one.count == two.count, "the same glyphs are drawn")
        for (a, b) in zip(one, two) {
            #expect(abs(b - 2 * a) < 1e-3, "glyph \(a)px became \(b)px")
        }
        // The probe's "Ready frame at scale 2" first glyph.
        #expect(bits(try quads(HudState(state: .ready), surface: big)[1].rect) == bits(SIMD4(652.697_94, 625.866_7, 101.333_336, 128.0)))
        // Spec 03 §9.3: the score origin and the title's baseline double too.
        let playing = try commands(HudState(state: .playing, score: 0), surface: big)
        #expect(playing == [.text("SCORE 0", origin: SIMD2(48, 108), px: 60, color: HUD_TEXT_COLOR)])
        guard case let .text(_, origin, _, _) = try commands(HudState(state: .ready), surface: big)[1] else {
            Issue.record("the Ready banner's first line is text")
            return
        }
        #expect(bits(origin.y) == bits(735.200_012_21))
    }

    @Test func theSafeAreaShiftsTheScoreAndTheBannerButNotTheWash() throws {
        // Rust's formulas with non-zero insets (top, right, bottom, left): the
        // score moves in from the top-left, the banner centres on the safe area
        // vertically but on the full width, and the wash ignores the insets.
        let insets = SIMD4<Float>(40, 13, 20, 30)
        let surface = HudSurface(width: 1280, height: 720, safeAreaInsets: insets)
        let over = try commands(HudState(state: .gameOver, score: 7, bestScore: 9), surface: surface)
        #expect(over[0] == .solid(rect: SIMD4(0, 0, 1280, 720), color: HUD_DIM_COLOR))
        #expect(over[1] == .text("SCORE 7", origin: SIMD2(30 + 24, (40 + 24) + 30), px: 30, color: HUD_TEXT_COLOR))
        let centerY: Float = 40 + (((720 - 40) - 20) * 0.5)
        let blockH: Float = (((-0.0 + 64 * font.lineHeight) + 30 * font.lineHeight) + 30 * font.lineHeight) + 30 * font.lineHeight
        guard case let .text(text, origin, px, _) = over[2] else {
            Issue.record("the banner's first line is text")
            return
        }
        #expect(text == "GAME OVER" && px == 64)
        #expect(bits(origin.y) == bits((centerY - (blockH * 0.5)) + 64))
        #expect(bits(origin.x) == bits((1280 - (try measure("GAME OVER", px: 64))) * 0.5), "centred on the full width")
    }

    // MARK: - Through the engine

    @Test func theHUDDrawsItsCommandsThroughTheEngine() throws {
        // The atlas went through the context as a linear texture, the engine's
        // font draws from it, and a frame records, per command, the wash or one
        // SDF quad per visible glyph, with the glyph's atlas rectangle.
        let atlas = font.font.atlas
        #expect(atlas.state == .ready && atlas.colorSpace == .linear)
        #expect(atlas.width == 512 && atlas.height == 512)
        #expect(context.stats.textures == 1 && context.stats.fonts == 1)

        for hud in [
            HudState(state: .ready), HudState(state: .playing, score: 3, fadeAlpha: 0.5),
            HudState(state: .gameOver, score: 23, bestScore: 40),
        ] {
            let drawn = try quads(hud)
            #expect(drawn.count > 1, "\(hud.state) records a wash plus glyph quads, not nothing")
            var expected: [(fill: HudQuad.Fill, uv: SIMD4<Float>, color: SIMD4<Float>)] = []
            for command in try commands(hud) {
                switch command {
                case let .solid(_, color):
                    expected.append((.solid, SIMD4(0, 0, 1, 1), color))
                case let .text(text, _, _, color):
                    for c in text.unicodeScalars where file.isVisible(c) {
                        expected.append((.sdf, file.glyph(c).uv, color))
                    }
                }
            }
            #expect(drawn.count == expected.count, "\(hud.state)")
            for (q, e) in zip(drawn, expected) {
                #expect(q.fill == e.fill && q.uv == e.uv && q.color == e.color, "\(hud.state): \(q) vs \(e)")
            }
        }
        #expect(context.stats.drawsSkipped == 0)

        // Without a font nothing is drawn, not even the wash.
        #expect(try recordedFrame(context) { try drawHud(HudState(state: .ready), font: nil, frame: $0) }.quads.isEmpty)
    }

    @Test func theHUDReadsTheFramesSurface() throws {
        // A 2560×1440 drawable at 2 pixels per point, insets in pixels: the
        // score moves in by the left and top insets, and nothing else changes.
        let hud = HudState(state: .playing, score: 5)
        let plain = try quads(hud, surface: HudSurface(width: 2560, height: 1440, scale: 2))
        let inset = try quads(hud, surface: HudSurface(width: 2560, height: 1440, safeAreaInsets: SIMD4(10, 0, 0, 6), scale: 2))
        #expect(plain.count == visible("SCORE 5") && inset.count == plain.count)
        for (a, b) in zip(plain, inset) {
            #expect(b.rect - a.rect == SIMD4(6, 10, 0, 0), "\(a.rect) moved to \(b.rect)")
            #expect(b.uv == a.uv && b.color == a.color)
        }
        // And every glyph is twice its size at scale 1.
        let one = try quads(hud)
        #expect(zip(one, plain).allSatisfy { abs($1.rect.z - 2 * $0.rect.z) < 1e-3 && abs($1.rect.w - 2 * $0.rect.w) < 1e-3 })
    }

    @Test func aBadFontFileIsAFontError() throws {
        let context = try nullContext()
        #expect(throws: FontError.self) { try HudFont.load(context, path: demoAssetPath("road.gltf")) }
        #expect(context.stats.textures == 0 && context.stats.fonts == 0, "nothing was uploaded")
    }

    // MARK: - The headless HUD tests of `hud.rs`, driving the game

    @Test("the_hud_font_loads_against_the_headless_renderer")
    func theHudFontLoadsAgainstTheHeadlessRenderer() throws {
        // Without the atlas `drawHud` draws nothing and every HUD test would pass
        // vacuously, so pin that it loads headlessly.
        let (game, h) = try booted()
        let font = try fontOf(game)
        #expect(!file.glyphs.isEmpty, "the atlas carries glyph metrics")
        #expect(font.lineHeight > 0.0, "the atlas carries line metrics")
        #expect(try visibleGlyphs("SCORE 0") > 0, "the atlas can actually draw the score readout")
        // Its texture went through the game's context, so the SDF quads name a
        // real texture.
        #expect(font.font.context === h.context && font.font.atlas.context === h.context)
        #expect(font.font.atlas.state == .ready)

        let quads = try hudFrame(h)
        #expect(quads.count > 1, "the title screen records a wash plus glyph quads, not nothing")
        #expect(quads.dropFirst().allSatisfy { $0.texture == font.font.atlas.id })

        // Golden: 95 glyphs from ' ', line height 1.2, "SCORE 0" draws 6, the title
        // frame 34 quads.
        #expect(file.glyphs.count == 95 && file.firstChar == " " && bits(font.lineHeight) == 0x3F99_999A)
        #expect(try visibleGlyphs("SCORE 0") == 6 && quads.count == 34)
    }

    @Test("hud_keeps_its_on_screen_size_at_every_render_scale")
    func hudKeepsItsOnScreenSizeAtEveryRenderScale() throws {
        // At 2 drawable pixels per point the drawable is twice as large and every
        // glyph must be too. Each scale gets a null context resized to it, as
        // Rust resizes the null surface.
        func glyphHeights(_ scale: Float) throws -> [Float] {
            let (_, h) = try booted(try nullContext(width: Int(1280 * scale), height: Int(720 * scale), scale: scale))
            return try hudFrame(h).filter { $0.fill.isSDF }.map { $0.rect[3] }
        }
        let one = try glyphHeights(1.0)
        let two = try glyphHeights(2.0)
        #expect(!one.isEmpty)
        #expect(one.count == two.count, "the same glyphs are drawn")
        for (a, b) in zip(one, two) {
            #expect(abs(b - 2.0 * a) < 1e-3, "glyph \(a)px became \(b)px")
        }
        // Golden: 33 glyphs each.
        #expect(one.count == 33)
    }

    @Test("ready_washes_the_scene_black_and_draws_the_title_without_a_score")
    func readyWashesTheSceneBlackAndDrawsTheTitleWithoutAScore() throws {
        let (game, h) = try booted()
        let surface = hudSurface(h)
        let quads = try hudFrame(h)

        // The title hides the scene entirely: opaque black.
        let wash = try washFirst(quads, surface, "Ready")
        #expect(wash == HUD_BLACK_COLOR, "Ready washes to opaque black")
        withinSurface(quads, surface, "Ready")

        // Everything after the wash is a banner glyph, exactly the two lines.
        _ = try fontOf(game)
        let expected = try visibleGlyphs("STICKSHIFT RUNNER") + visibleGlyphs("PRESS SPACE TO START")
        #expect(quads.count - 1 == expected, "Ready draws exactly the title + prompt banner, and no score")
        #expect(
            !quads.contains { $0.fill.isSDF && inScoreCorner($0, surface) },
            "Ready draws no text in the live-score corner"
        )

        // Golden: the probe's 34 quads.
        expectProbe(quads, try probeQuads(after: "Ready frame quads: 34", count: 34))
    }

    @Test("playing_past_the_fade_draws_the_score_and_no_wash")
    func playingPastTheFadeDrawsTheScoreAndNoWash() throws {
        let (game, h) = try hudStarted()
        let surface = hudSurface(h)

        // Drive until the opening fade has fully retired, one frame at a time (a
        // longer drive eventually crashes into the deterministic traffic).
        var frames: UInt32 = 0
        var quads: [HudQuad] = []
        while true {
            let frame = try hudFrame(h)
            frames += 1
            try #require(
                game.state == .playing,
                """
                the run ended after \(frames) frames, before the \(HUD_FADE_SECONDS)s opening fade retired — so \
                this is not the fade-free Playing HUD under test
                """
            )
            if game.fadeAlpha() == 0.0 {
                quads = frame
                break
            }
            try #require(frames < 600, "the fade never retired in \(frames) frames (HUD_FADE_SECONDS = \(HUD_FADE_SECONDS))")
        }

        // Fade gone: no wash at all.
        #expect(
            quads.allSatisfy { $0.fill != .solid },
            "a fade-free Playing frame records no wash quad, only score glyphs"
        )
        withinSurface(quads, surface, "Playing")

        // The live score is drawn, as SDF text, in the top-left safe corner.
        _ = try fontOf(game)
        let text = "SCORE \(game.score())"
        #expect(
            quads.count == (try visibleGlyphs(text)),
            "Playing draws exactly the live score readout (\(text.debugDescription))"
        )
        #expect(quads.allSatisfy { $0.fill.isSDF }, "score text is SDF glyphs, not solid quads")
        #expect(quads.allSatisfy { inScoreCorner($0, surface) }, "the live score sits in the top-left safe corner")

        // Golden: the 46th frame, "SCORE 10", the probe's 7 quads.
        #expect(frames == 46 && game.score() == 10)
        expectProbe(quads, try probeQuads(after: "first fade-free Playing frame", count: 7))
    }

    @Test("the_opening_fade_wash_is_partial_and_retiring")
    func theOpeningFadeWashIsPartialAndRetiring() throws {
        // Starting a run hands the title's black to a timed fade: a partial wash
        // that weakens every frame.
        let (game, h) = try hudStarted()
        let surface = hudSurface(h)
        #expect(game.fadeAlpha() == 1.0, "the fade starts fully black")

        var previous: Float = 1.0
        let golden: [UInt32] = [0x3F7A_4FA5, 0x3F74_9F4B, 0x3F6E_EEF0]
        for frame in 0..<3 {
            let quads = try hudFrame(h)
            #expect(game.state == .playing, "still driving at fade frame \(frame)")

            let wash = try washFirst(quads, surface, "Playing (mid-fade)")
            withinSurface(quads, surface, "Playing (mid-fade)")
            #expect(
                [wash[0], wash[1], wash[2]] == [HUD_BLACK_COLOR[0], HUD_BLACK_COLOR[1], HUD_BLACK_COLOR[2]],
                "the fade varies only black's alpha, never its color"
            )
            #expect(wash[3] > 0.0 && wash[3] < 1.0, "fade frame \(frame): alpha is partial, got \(wash[3])")
            #expect(wash[3] < previous, "fade frame \(frame): the fade retires (\(wash[3]) is not below \(previous))")
            previous = wash[3]

            // The readout is already live under the fade.
            #expect(
                quads.dropFirst().contains { inScoreCorner($0, surface) },
                "fade frame \(frame): the live score draws under the fade"
            )

            // Golden: the alphas, and the wash plus "SCORE 0".
            #expect(bits(wash[3]) == golden[frame] && quads.count == 7)
        }
    }

    @Test("game_over_dims_the_scene_and_adds_a_banner")
    func gameOverDimsTheSceneAndAddsABanner() throws {
        let (game, h, playing, quads) = try driveToGameOverHUD()
        let surface = hudSurface(h)

        // A dim, not the title's blackout: the crashed car stays visible.
        let wash = try washFirst(quads, surface, "GameOver")
        #expect(wash == HUD_DIM_COLOR, "GameOver dims rather than blacks out")
        #expect(
            wash[3] > 0.0 && wash[3] < 1.0,
            "the game-over wash is partial so the scene reads through it, got \(wash[3])"
        )
        withinSurface(quads, surface, "GameOver")

        // The banner's four lines sit on top of the still-live score.
        #expect(
            quads.count > playing.count,
            "the game-over banner adds quads (\(quads.count) vs the preceding Playing frame's \(playing.count))"
        )

        // Exactly: the live readout plus the four banner lines.
        _ = try fontOf(game)
        let score = "SCORE \(game.score())"
        let best = "BEST \(game.bestScore())"
        let expected = try [score, "GAME OVER", score, best, "PRESS SPACE TO REPLAY"].map { try visibleGlyphs($0) }
            .reduce(0, +)
        #expect(quads.count - 1 == expected, "GameOver draws the live score plus the four banner lines")

        // Centred vertically: every glyph outside the score corner is in the
        // middle half of the drawable.
        let banner = quads.dropFirst().filter { !inScoreCorner($0, surface) }
        #expect(!banner.isEmpty, "the game-over banner is drawn")
        let height = Float(surface.height)
        #expect(
            banner.allSatisfy { $0.rect[1] >= height * 0.25 && $0.rect[1] + $0.rect[3] <= height * 0.75 },
            "the banner block is centred, not top-aligned"
        )

        // Golden: the 98th frame after the start, score 23, the probe's 47 quads
        // after a 7-quad "SCORE 22" frame.
        #expect(h.framesRun == 99 && game.score() == 23 && game.bestScore() == 23)
        #expect(playing.count == 7 && quads.count == 47)
        expectProbe(quads, try probeQuads(after: "GameOver frame after 98 frames", count: 47))
    }
}
