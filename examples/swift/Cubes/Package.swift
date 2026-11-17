// swift-tools-version: 6.0
// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import PackageDescription

// Cubes: a small Swift game on Stick Shift Engine, in a macOS window.
//
// The StickShift package links `libstickshift.a`, which SwiftPM cannot build:
// run `cargo build -p stickshift-capi` first (`scripts/test-swift.sh` does), then
// `swift run --package-path examples/swift/Cubes`.
let package = Package(
    name: "Cubes",
    platforms: [.macOS(.v14)],
    dependencies: [
        .package(path: "../../../bindings/swift/StickShift")
    ],
    targets: [
        .executableTarget(
            name: "Cubes",
            dependencies: [
                .product(name: "StickShift", package: "StickShift"),
                .product(name: "StickShiftAppKit", package: "StickShift"),
            ]
        )
    ]
)
