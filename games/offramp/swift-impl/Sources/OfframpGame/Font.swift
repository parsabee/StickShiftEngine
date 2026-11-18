// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Foundation
import StickShift

// The HUD font: the reader for the committed `font.bin` SDF atlas, a port of
// Rust's `hud::load_font` (`hud.rs:101-165`), which uploads the distance field
// and hands the glyph table to the engine as a `Font`. The engine lays the text
// out and measures it (`FontAtlas::layout` and `measure`, the functions the Rust
// HUD calls).

/// A `font.bin` that cannot be used. Rust panics with these messages.
public enum FontError: Error, CustomStringConvertible, Equatable {
    /// The file could not be read.
    case unreadable(path: String, reason: String)
    /// Too short for the header, or the magic is not `KFNT`.
    case notKFNT(path: String)
    /// The glyph table or the distance field runs past the end of the file.
    case truncated(path: String)
    /// The first character is not a Unicode scalar.
    case invalidFirstChar(path: String)

    public var description: String {
        switch self {
        case let .unreadable(path, reason): "read font atlas \(path): \(reason)"
        case let .notKFNT(path): "\(path) is not a KFNT font atlas"
        case let .truncated(path): "\(path) is truncated"
        case .invalidFirstChar: "font atlas has a valid first char"
        }
    }
}

/// A parsed `font.bin`: the glyph table and the distance field.
///
/// The format (little-endian, written by `examples/gen_font.rs`): `"KFNT"`, a
/// `u32` version (not read), `u32` width, height, first character and glyph
/// count, an `f32` line height, then nine `f32`s per glyph (uv, size, bearing,
/// advance), then `width × height` distance bytes, top row first.
public struct FontFile: Sendable {
    /// The `FONT_ASSET` fallback for characters outside the range: draws nothing,
    /// advances a quarter em.
    public static let fallbackGlyph = Glyph(uv: .zero, size: .zero, bearing: .zero, advance: 0.25)

    /// The distance field's width, in pixels.
    public let width: Int
    /// The distance field's height, in pixels.
    public let height: Int
    /// The character `glyphs[0]` describes.
    public let firstChar: Unicode.Scalar
    /// The metrics in em units, densely indexed from `firstChar`.
    public let glyphs: [Glyph]
    /// The distance between baselines, in em units.
    public let lineHeight: Float
    /// One distance byte per pixel, row-major, top row first (128 is the edge).
    public let distances: [UInt8]

    /// Read and parse the font at `path`.
    public static func load(path: String) throws -> FontFile {
        let data: Data
        do {
            data = try Data(contentsOf: URL(fileURLWithPath: path))
        } catch {
            throw FontError.unreadable(path: path, reason: error.localizedDescription)
        }
        return try parse([UInt8](data), path: path)
    }

    /// Parse `bytes` (named `path` in errors), with the Rust loader's checks.
    public static func parse(_ bytes: [UInt8], path: String) throws -> FontFile {
        guard bytes.count >= 28, bytes[0..<4].elementsEqual("KFNT".utf8) else {
            throw FontError.notKFNT(path: path)
        }
        func u32(_ offset: Int) -> UInt32 {
            UInt32(bytes[offset]) | UInt32(bytes[offset + 1]) << 8 | UInt32(bytes[offset + 2]) << 16
                | UInt32(bytes[offset + 3]) << 24
        }
        func f32(_ offset: Int) -> Float {
            Float(bitPattern: u32(offset))
        }

        let width = Int(u32(8))
        let height = Int(u32(12))
        guard let firstChar = Unicode.Scalar(u32(16)) else {
            throw FontError.invalidFirstChar(path: path)
        }
        let count = Int(u32(20))
        let lineHeight = f32(24)

        let glyphsOffset = 28
        let pixelsOffset = glyphsOffset + count * 9 * 4
        guard bytes.count >= pixelsOffset + width * height else {
            throw FontError.truncated(path: path)
        }

        var glyphs: [Glyph] = []
        glyphs.reserveCapacity(count)
        for i in 0..<count {
            let base = glyphsOffset + i * 9 * 4
            func f(_ k: Int) -> Float { f32(base + k * 4) }
            glyphs.append(
                Glyph(
                    uv: SIMD4(f(0), f(1), f(2), f(3)),
                    size: SIMD2(f(4), f(5)),
                    bearing: SIMD2(f(6), f(7)),
                    advance: f(8)
                )
            )
        }

        return FontFile(
            width: width,
            height: height,
            firstChar: firstChar,
            glyphs: glyphs,
            lineHeight: lineHeight,
            distances: Array(bytes[pixelsOffset..<(pixelsOffset + width * height)])
        )
    }

    /// The distance field as RGBA8 for upload: each distance `d` becomes
    /// `(d, d, d, 255)` (the overlay's SDF mode reads red).
    public var rgba: [UInt8] {
        var out = [UInt8]()
        out.reserveCapacity(distances.count * 4)
        for d in distances {
            out += [d, d, d, 255]
        }
        return out
    }
}

/// The loaded HUD font (Rust's `FontAtlas`, which carries its texture): the
/// engine's font over the uploaded distance field, and the file's line height,
/// the same `f32` the engine's atlas holds.
@MainActor
public struct HudFont {
    /// The engine's font: what the HUD draws and measures text with.
    public let font: Font
    /// The distance between baselines, in em units.
    public let lineHeight: Float

    /// `hud::load_font`: read the committed SDF atlas at `path`, upload its
    /// distance field through `renderer` (linear: distances are data, not
    /// colour), and make the engine's font from its glyph table, with the
    /// fallback that advances a quarter em and draws nothing.
    ///
    /// - Throws: ``FontError`` for a file Rust would panic on, and the engine's
    ///   error if an upload fails.
    public static func load(_ renderer: Context, path: String) throws -> HudFont {
        let file = try FontFile.load(path: path)
        let atlas = try renderer.makeTexture(
            width: file.width, height: file.height, rgba8: file.rgba, colorSpace: .linear
        )
        let font = try renderer.makeFont(
            atlas: atlas,
            firstCodepoint: file.firstChar.value,
            glyphs: file.glyphs,
            lineHeight: file.lineHeight,
            fallback: FontFile.fallbackGlyph
        )
        return HudFont(font: font, lineHeight: file.lineHeight)
    }
}
