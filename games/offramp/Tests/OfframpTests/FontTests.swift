// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Foundation
import StickShift
import Testing

@testable import OfframpGame

/// The committed `font.bin` as the KFNT reader (`hud::load_font`) reads it, the
/// font the engine makes from it, and the loader's checks.
@Suite("font")
struct FontTests {
    private func committedFont() throws -> FontFile {
        try FontFile.load(path: demoAssets.appendingPathComponent(FONT_ASSET).path)
    }

    @Test func theCommittedFontParsesAsRustReadsIt() throws {
        let font = try committedFont()
        #expect(font.width == 512 && font.height == 512)
        #expect(font.firstChar == " ")
        #expect(font.glyphs.count == 95, "' ' through '~'")
        #expect(bits(font.lineHeight) == 0x3F99_999A)
        #expect(FontFile.fallbackGlyph == Glyph(uv: .zero, size: .zero, bearing: .zero, advance: 0.25))
        #expect(font.distances.count == 512 * 512)

        let s = font.glyph("S")
        #expect(s.uv == SIMD4(0.386_718_75, 0.328_125, 0.460_937_5, 0.421_875))
        #expect(s.size == SIMD2(0.791_666_7, 1.0))
        #expect(s.bearing == SIMD2(-0.104_166_664, -0.854_166_7))
        #expect(s.advance == 0.593_75)

        let space = font.glyph(" ")
        #expect(space.size == .zero && space.uv == .zero)
        #expect(space.bearing == SIMD2(-0.125, -0.125))
        #expect(space.advance == 0.248_046_875, "space is in range: not the fallback's 0.25")
        #expect(!font.isVisible(" "))

        for digit in "0123456789".unicodeScalars {
            #expect(font.glyph(digit).advance == 0.562_011_72, "every digit advances alike")
        }
        #expect(font.glyph("é") == FontFile.fallbackGlyph, "outside the range")
        #expect(font.glyph("\u{1F}") == FontFile.fallbackGlyph, "below the first character")
    }

    @Test func theAtlasExpandsToOpaqueGreyRGBA() throws {
        let font = try committedFont()
        let rgba = font.rgba
        #expect(rgba.count == 512 * 512 * 4)
        for i in stride(from: 0, to: font.distances.count, by: 997) {
            let d = font.distances[i]
            #expect(Array(rgba[(i * 4)..<(i * 4 + 4)]) == [d, d, d, 255])
        }
        // The file is exactly header, glyphs and field: 28 + 95 * 36 + 512 * 512.
        let size = try FileManager.default.attributesOfItem(atPath: demoAssets.appendingPathComponent(FONT_ASSET).path)[.size]
        #expect((size as? Int) == 265_592)
    }

    @Test @MainActor func theFontUploadsAsALinearAtlasAndTheEngineMeasuresIt() throws {
        // `HudFont.load`: the field as a linear texture (the engine refuses an
        // sRGB atlas), and the engine's font over it, which measures as the Rust
        // HUD does (`probe_stdout.txt`'s widths), with the file's line height.
        let context = try Context(offscreenWidth: 64, height: 64, backend: .null)
        let hud = try HudFont.load(context, path: demoAssets.appendingPathComponent(FONT_ASSET).path)
        let atlas = hud.font.atlas
        #expect(atlas.width == 512 && atlas.height == 512 && atlas.colorSpace == .linear)
        #expect(atlas.state == .ready)
        #expect(bits(hud.lineHeight) == 0x3F99_999A)

        let file = try committedFont()
        let lines = try fixtureLines("probe_stdout.txt").filter { $0.hasPrefix("  visible(\"") }
        #expect(lines.count == 7)
        for line in lines {
            let text = String(try after("visible(\"", in: line).prefix { $0 != "\"" })
            let visible = try integer("\") = ", in: line)
            #expect(text.unicodeScalars.filter(file.isVisible).count == visible, "\(text)")
            // "measure@30 = 117.81738 (0x42eba280)  measure@64 = 251.34375 (0x437b5800)"
            let widths = line.components(separatedBy: "(0x").dropFirst().compactMap { UInt32($0.prefix(8), radix: 16) }
            #expect(widths.count == 2)
            #expect(bits(try hud.font.width(of: text, pixelSize: 30)) == widths.first, "\(text) at 30")
            #expect(bits(try hud.font.width(of: text, pixelSize: 64)) == widths.last, "\(text) at 64")
        }
    }

    // MARK: - The loader's checks

    @Test func aBadFileIsRefusedWithTheRustMessages() throws {
        #expect(throws: FontError.notKFNT(path: "x")) { try FontFile.parse(Array("KFNT".utf8), path: "x") }
        var header = [UInt8](repeating: 0, count: 28)
        header.replaceSubrange(0..<4, with: Array("KFNX".utf8))
        #expect(throws: FontError.notKFNT(path: "x")) { try FontFile.parse(header, path: "x") }

        header.replaceSubrange(0..<4, with: Array("KFNT".utf8))
        header[8] = 2  // width 2
        header[12] = 2  // height 2
        header[16] = 65  // 'A'
        header[20] = 1  // one glyph
        #expect(throws: FontError.truncated(path: "x")) { try FontFile.parse(header, path: "x") }
        let whole = header + [UInt8](repeating: 0, count: 36) + [10, 20, 30, 40]
        let font = try FontFile.parse(whole, path: "x")
        #expect(font.firstChar == "A" && font.glyphs.count == 1 && font.distances == [10, 20, 30, 40])
        #expect(font.rgba == [10, 10, 10, 255, 20, 20, 20, 255, 30, 30, 30, 255, 40, 40, 40, 255])

        var surrogate = whole
        surrogate[16] = 0x00
        surrogate[17] = 0xD8  // U+D800 is not a scalar
        #expect(throws: FontError.invalidFirstChar(path: "x")) { try FontFile.parse(surrogate, path: "x") }

        #expect(FontError.notKFNT(path: "f.bin").description == "f.bin is not a KFNT font atlas")
        #expect(FontError.truncated(path: "f.bin").description == "f.bin is truncated")
        #expect(throws: FontError.self) { try FontFile.load(path: "/nonexistent/font.bin") }
    }
}
