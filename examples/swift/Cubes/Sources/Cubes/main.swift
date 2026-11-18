// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

// Cubes: a Swift game on the engine, game code only. The camera flies down a
// road; the engine's scene streams a cube into each slot ahead (on alternate
// sides) and despawns those left behind, and rebases the world when the camera
// has gone far. `StickShiftApp` is the window, the menus and the frame loop.
//
//   cargo build -p stickshift-capi && swift run --package-path examples/swift/Cubes

import Foundation
import StickShift
import StickShiftAppKit

@MainActor
final class Cubes: Game {
    /// How fast the camera flies, in units per second.
    static let speed: Float = 14
    private var cube: Mesh?
    /// Where the camera is going, in the world's (rebased) coordinates.
    private var focus = SIMD3<Float>(0, 0, 0)

    func initialize(_ ctx: EngineContext) throws {
        try ctx.renderer.setSunSky(.default)
        cube = try ctx.renderer.makeMesh(layout: .positionNormalUV, vertices: Self.vertices, indices: Self.indices)
        // One cube at the start, then the road ahead.
        ctx.world.spawn(Transform(position: [0, 0, -4], scale: [1.5, 1.5, 1.5]), RenderComponent.cube([1, 0.8, 0.2]))
        stream(ctx)
    }

    func update(_ ctx: EngineContext, dt: Float) {
        focus += ctx.scene.config.axis * (Self.speed * dt)
        focus += ctx.scene.maybeRebase(focus: focus)  // the world moved: so does the focus
        stream(ctx)
        ctx.camera.position = focus + [0, 2.5, 7]
        ctx.camera.target = focus + [0, 1, -10]
    }

    /// Fill each new slot ahead with a cube, left and right in turn.
    private func stream(_ ctx: EngineContext) {
        ctx.scene.stream(focus: focus) { slot in
            let side: Float = slot.index % 2 == 0 ? -3 : 3
            let hue = Float(abs(slot.index % 6)) / 6
            slot.spawn(Transform(position: slot.position + [side, 0.5, 0]), RenderComponent.cube([hue, 0.5, 1 - hue]))
        }
    }

    func render(_ ctx: EngineContext, into frame: Frame) throws {
        guard let cube else { return }
        // The road, under the camera: drawn, not an entity.
        let road = Transform(position: focus + [0, -0.55, -40], scale: [10, 0.1, 120])
        try frame.draw(cube, transform: road.matrix, material: Material(baseColor: [0.25, 0.25, 0.27, 1]))
        for (_, transform, render) in ctx.world.query(Transform.self, RenderComponent.self) {
            try frame.draw(cube, transform: transform.matrix, material: Material(baseColor: SIMD4(render.color, 1)))
        }
    }

    /// A unit cube, untextured, so the material's base colour colours it: four
    /// vertices per face, so each face has its own normal; position, normal, uv.
    static let (vertices, indices): ([Float], [UInt32]) = {
        var vertices: [Float] = []
        var indices: [UInt32] = []
        let axes: [SIMD3<Float>] = [[1, 0, 0], [0, 1, 0], [0, 0, 1]]
        for (i, axis) in axes.enumerated() {
            for sign: Float in [1, -1] {
                let n = axis * sign, u = axes[(i + 1) % 3] * sign, v = axes[(i + 2) % 3]
                let base = UInt32(vertices.count / 8)
                for (corner, uv) in zip([-u - v, u - v, u + v, -u + v], [[0, 1], [1, 1], [1, 0], [0, 0]] as [[Float]]) {
                    let p = (n + corner) * 0.5
                    vertices += [p.x, p.y, p.z, n.x, n.y, n.z] + uv
                }
                indices += [base, base + 1, base + 2, base, base + 2, base + 3]
            }
        }
        return (vertices, indices)
    }()
}

// The app's options: the sound device, the player's graphics settings (kept
// under this executable's name; set STICKSHIFT_SETTINGS_SUITE=offramp to
// share the Rust demo's), and Escape quitting.
var options = Engine.Options.app
options.settingsSuite = ProcessInfo.processInfo.environment["STICKSHIFT_SETTINGS_SUITE"]
StickShiftApp.run(title: "Cubes", size: CGSize(width: 800, height: 600), options: options) { Cubes() }
