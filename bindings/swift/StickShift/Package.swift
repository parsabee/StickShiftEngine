// swift-tools-version: 6.0
// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Foundation
import PackageDescription

// SwiftPM cannot run cargo, so `libstickshift.a` is built first, by
// `scripts/test-swift.sh` or `cargo build -p stickshift-capi`, and linked statically from
// `STICKSHIFT_LIB_DIR` (default: this workspace's `target/debug`).
let packageDirectory = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
let workspace = packageDirectory
    .deletingLastPathComponent()  // bindings/swift
    .deletingLastPathComponent()  // bindings
    .deletingLastPathComponent()  // the workspace root
let libraryDirectory =
    Context.environment["STICKSHIFT_LIB_DIR"] ?? workspace.appendingPathComponent("target/debug").path

let package = Package(
    name: "StickShift",
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "StickShift", targets: ["StickShift"])
    ],
    targets: [
        // The C API, straight from `crates/stickshift-capi/include/stickshift.h`.
        .systemLibrary(name: "CStickShift", path: "Sources/CStickShift"),
        .target(
            name: "StickShift",
            dependencies: ["CStickShift"],
            linkerSettings: [
                .unsafeFlags(["\(libraryDirectory)/libstickshift.a"]),
                // What `libstickshift.a` itself links against: rustc's
                // `--print native-static-libs`, less the libraries every link
                // gets (System, c, m). `scripts/test-swift.sh` checks the two
                // lists match. The audio frameworks are the engine's sound
                // (kira over cpal); AppKit comes from the Metal renderer.
                .linkedFramework("AppKit"),
                .linkedFramework("AudioToolbox"),
                .linkedFramework("CoreAudio"),
                .linkedFramework("CoreFoundation"),
                .linkedFramework("CoreGraphics"),
                .linkedFramework("Foundation"),
                .linkedFramework("Metal"),
                .linkedFramework("QuartzCore"),
                .linkedLibrary("iconv"),
                .linkedLibrary("objc"),
            ]
        ),
        .testTarget(name: "StickShiftTests", dependencies: ["StickShift"]),
    ]
)
