// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

// Compare two directories of recorded frames (frame_0000.png, frame_0001.png, ...)
// pixel by pixel, by their decoded RGBA samples, with macOS's ImageIO. PNG bytes
// are never compared: the Rust recorder and Offramp's use different encoders.
//
//   swiftc -O scripts/compare-frames.swift -o compare-frames
//   compare-frames <expected-dir> <actual-dir>
//
// Prints one line per frame that differs (its differing pixel count, the
// largest channel difference and the box that holds the differences) and a
// summary line, and exits 0 only if both directories hold the same frames with
// the same pixels. Used by scripts/offramp-parity.sh.

import CoreGraphics
import Foundation
import ImageIO

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data("compare-frames: \(message)\n".utf8))
    exit(2)
}

/// The frame files in `directory`, by name.
func frames(in directory: String) -> [String] {
    guard let names = try? FileManager.default.contentsOfDirectory(atPath: directory) else {
        fail("cannot read \(directory)")
    }
    return names.filter { $0.hasPrefix("frame_") && $0.hasSuffix(".png") }.sorted()
}

/// A PNG's samples as RGBA8, top row first, without colour management.
func pixels(_ path: String) -> (width: Int, height: Int, rgba: [UInt8]) {
    guard
        let source = CGImageSourceCreateWithURL(URL(fileURLWithPath: path) as CFURL, nil),
        let image = CGImageSourceCreateImageAtIndex(source, 0, nil),
        let data = image.dataProvider?.data as Data?
    else {
        fail("cannot decode \(path)")
    }
    let channels = image.bitsPerPixel / 8
    let order = image.bitmapInfo.intersection(.byteOrderMask)
    guard image.bitsPerComponent == 8, channels == 3 || channels == 4, order.rawValue == 0 || order == .byteOrder32Big
    else {
        fail("\(path) is not 8-bit RGB or RGBA")
    }
    let alpha = image.alphaInfo
    if alpha == .premultipliedLast || alpha == .premultipliedFirst {
        fail("\(path) decoded premultiplied")
    }
    let first = channels == 4 && (alpha == .first || alpha == .noneSkipFirst)
    let red = first ? 1 : 0
    let alphaAt: Int? = channels == 3 || alpha == .noneSkipLast || alpha == .noneSkipFirst ? nil : (first ? 0 : 3)
    var rgba = [UInt8](repeating: 255, count: image.width * image.height * 4)
    data.withUnsafeBytes { bytes in
        for y in 0..<image.height {
            for x in 0..<image.width {
                let from = y * image.bytesPerRow + x * channels
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

let args = CommandLine.arguments
guard args.count == 3 else {
    fail("usage: compare-frames <expected-dir> <actual-dir>")
}
let (expectedDir, actualDir) = (args[1], args[2])
let expected = frames(in: expectedDir)
let actual = frames(in: actualDir)
guard !expected.isEmpty else {
    fail("no frame_*.png in \(expectedDir)")
}

var same = true
if expected != actual {
    same = false
    let missing = Set(expected).subtracting(actual).sorted()
    let extra = Set(actual).subtracting(expected).sorted()
    print("frame sets differ: \(expected.count) expected, \(actual.count) recorded")
    if !missing.isEmpty { print("  missing: \(missing.joined(separator: " "))") }
    if !extra.isEmpty { print("  extra: \(extra.joined(separator: " "))") }
}

var compared = 0
var differingFrames: [String] = []
var totalPixels = 0
for name in expected where actual.contains(name) {
    let a = pixels(expectedDir + "/" + name)
    let b = pixels(actualDir + "/" + name)
    compared += 1
    guard a.width == b.width, a.height == b.height else {
        print("\(name): \(a.width)×\(a.height) expected, \(b.width)×\(b.height) recorded")
        differingFrames.append(name)
        continue
    }
    totalPixels += a.width * a.height
    if a.rgba == b.rgba {
        continue
    }
    var differing = 0
    var largest = 0
    var box = (minX: Int.max, minY: Int.max, maxX: Int.min, maxY: Int.min)
    for y in 0..<a.height {
        for x in 0..<a.width {
            let i = (y * a.width + x) * 4
            var d = 0
            for c in 0..<4 {
                d = max(d, abs(Int(a.rgba[i + c]) - Int(b.rgba[i + c])))
            }
            if d > 0 {
                differing += 1
                largest = max(largest, d)
                box = (min(box.minX, x), min(box.minY, y), max(box.maxX, x), max(box.maxY, y))
            }
        }
    }
    print(
        "\(name): \(differing) of \(a.width * a.height) pixels differ, largest channel difference \(largest), "
            + "within x \(box.minX)...\(box.maxX), y \(box.minY)...\(box.maxY)"
    )
    differingFrames.append(name)
}

if let first = differingFrames.first {
    same = false
    print("frames: \(differingFrames.count) of \(compared) differ; the first is \(first)")
} else {
    print("frames: \(compared) compared, \(totalPixels) pixels, all identical")
}
exit(same ? 0 : 1)
