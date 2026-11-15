// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

// Assemble PNG frames into a looping animated GIF with macOS's ImageIO, scaling
// each frame to a given width. No third-party tools needed.
//
//   swift scripts/frames-to-gif.swift <out.gif> <width> <seconds-per-frame> <frame.png>...
//
// Used by scripts/record-demo-gif.sh.

import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

let args = CommandLine.arguments
guard args.count >= 5, let width = Int(args[2]), let delay = Double(args[3]) else {
    FileHandle.standardError.write(
        "usage: frames-to-gif.swift <out.gif> <width> <seconds-per-frame> <frame.png>...\n"
            .data(using: .utf8)!)
    exit(2)
}
let frames = Array(args[4...])
guard
    let destination = CGImageDestinationCreateWithURL(
        URL(fileURLWithPath: args[1]) as CFURL, UTType.gif.identifier as CFString, frames.count,
        nil)
else {
    fatalError("cannot create \(args[1])")
}
// Loop forever.
CGImageDestinationSetProperties(
    destination,
    [kCGImagePropertyGIFDictionary: [kCGImagePropertyGIFLoopCount: 0]] as CFDictionary)
let frameProperties =
    [
        kCGImagePropertyGIFDictionary: [
            kCGImagePropertyGIFDelayTime: delay,
            kCGImagePropertyGIFUnclampedDelayTime: delay,
        ]
    ] as CFDictionary
let sRGB = CGColorSpace(name: CGColorSpace.sRGB)!

for path in frames {
    guard
        let source = CGImageSourceCreateWithURL(URL(fileURLWithPath: path) as CFURL, nil),
        let image = CGImageSourceCreateImageAtIndex(source, 0, nil)
    else {
        fatalError("cannot read \(path)")
    }
    let height = image.height * width / image.width
    guard
        let context = CGContext(
            data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
            space: sRGB, bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue)
    else {
        fatalError("cannot scale \(path)")
    }
    context.interpolationQuality = .high
    context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
    CGImageDestinationAddImage(destination, context.makeImage()!, frameProperties)
}
guard CGImageDestinationFinalize(destination) else {
    fatalError("cannot write \(args[1])")
}
