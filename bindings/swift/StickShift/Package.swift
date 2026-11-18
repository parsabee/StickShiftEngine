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
                // What `libstickshift.a` itself links against.
                .linkedFramework("AppKit"),
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
