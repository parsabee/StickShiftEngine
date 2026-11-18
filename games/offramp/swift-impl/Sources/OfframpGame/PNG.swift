// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

// PNG files for the recorder's frames, through ImageIO. Frames are compared by
// their decoded samples, never by their bytes: the Rust recorder writes its PNGs
// with the `image` crate and this one with ImageIO, and the encoders differ.

/// A PNG that could not be written or read.
public struct PNGError: Error, CustomStringConvertible {
    public let description: String
}

/// Write RGBA8 pixels, `width` × `height`, top row first, as a PNG. The samples
/// are stored as they are, alpha unpremultiplied; the file is tagged sRGB, which
/// is what the frame holds.
public func writePNG(_ rgba: [UInt8], width: Int, height: Int, to url: URL) throws {
    guard rgba.count == width * height * 4 else {
        throw PNGError(description: "\(rgba.count) bytes are not \(width) × \(height) RGBA8 pixels")
    }
    guard
        let provider = CGDataProvider(data: Data(rgba) as CFData),
        let space = CGColorSpace(name: CGColorSpace.sRGB),
        let image = CGImage(
            width: width, height: height, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: width * 4,
            space: space, bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.last.rawValue),
            provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent
        ),
        let destination = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil)
    else {
        throw PNGError(description: "cannot encode \(url.path)")
    }
    CGImageDestinationAddImage(destination, image, nil)
    guard CGImageDestinationFinalize(destination) else {
        throw PNGError(description: "cannot write \(url.path)")
    }
}

/// A PNG's samples as RGBA8, top row first, as the file stores them: no colour
/// management, and an opaque alpha for a file without one.
public func readPNG(_ url: URL) throws -> (width: Int, height: Int, rgba: [UInt8]) {
    guard
        let source = CGImageSourceCreateWithURL(url as CFURL, nil),
        let image = CGImageSourceCreateImageAtIndex(source, 0, nil),
        let data = image.dataProvider?.data as Data?
    else {
        throw PNGError(description: "cannot decode \(url.path)")
    }
    let alpha = image.alphaInfo
    let channels = image.bitsPerPixel / 8
    // Samples in memory order: the default, or big-endian words.
    let order = image.bitmapInfo.intersection(.byteOrderMask)
    guard image.bitsPerComponent == 8, channels == 3 || channels == 4,
        order.rawValue == 0 || order == .byteOrder32Big
    else {
        throw PNGError(description: "\(url.path) is not 8-bit RGB or RGBA (bitmap info \(image.bitmapInfo.rawValue))")
    }
    // Where R sits in a pixel, and where alpha does (nil: none).
    let (red, alphaAt): (Int, Int?) =
        switch (channels, alpha) {
        case (3, _): (0, nil)
        case (_, .last), (_, .premultipliedLast): (0, 3)
        case (_, .noneSkipLast): (0, nil)
        case (_, .first), (_, .premultipliedFirst): (1, 0)
        case (_, .noneSkipFirst): (1, nil)
        default: (0, nil)
        }
    if alpha == .premultipliedLast || alpha == .premultipliedFirst {
        // Only an opaque image survives premultiplication unchanged.
        throw PNGError(description: "\(url.path) decoded premultiplied")
    }
    var rgba = [UInt8](repeating: 255, count: image.width * image.height * 4)
    data.withUnsafeBytes { (bytes: UnsafeRawBufferPointer) in
        for y in 0..<image.height {
            let row = y * image.bytesPerRow
            for x in 0..<image.width {
                let from = row + x * channels
                let to = (y * image.width + x) * 4
                rgba[to] = bytes[from + red]
                rgba[to + 1] = bytes[from + red + 1]
                rgba[to + 2] = bytes[from + red + 2]
                if let alphaAt {
                    rgba[to + 3] = bytes[from + alphaAt]
                }
            }
        }
    }
    return (image.width, image.height, rgba)
}
