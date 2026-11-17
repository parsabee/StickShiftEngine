// swift-tools-version: 6.0
// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import PackageDescription

// Offramp: the playable demo (`games/playable-demo`) rebuilt in Swift as game
// code on the engine, through the StickShift package: the engine (its world,
// scene, loop, camera, input, audio and settings) is the Rust one, through the C
// API.
//
// The StickShift package links `libstickshift.a`, which SwiftPM cannot build:
// run `scripts/test-offramp.sh`, or `cargo build -p stickshift-capi` first.
let package = Package(
    name: "Offramp",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "offramp", targets: ["offramp"])
    ],
    dependencies: [
        .package(path: "../../bindings/swift/StickShift")
    ],
    targets: [
        // Everything testable: the game, its tools (the smoke run, the trace, the
        // recorder) and the assets it reads.
        .target(
            name: "OfframpGame",
            dependencies: [.product(name: "StickShift", package: "StickShift")]
        ),
        // The command line, and the windowed app on the engine's macOS host.
        .executableTarget(
            name: "offramp",
            dependencies: ["OfframpGame", .product(name: "StickShiftAppKit", package: "StickShift")]
        ),
        // The tests run the executable too (`ExecutableTests`), so it is built
        // with them, and the game in the engine's view (`WindowTests`).
        .testTarget(
            name: "OfframpTests",
            dependencies: ["OfframpGame", "offramp", .product(name: "StickShiftAppKit", package: "StickShift")],
            // Golden values captured from the Rust demo, read by path.
            exclude: ["Fixtures"]
        ),
    ]
)
