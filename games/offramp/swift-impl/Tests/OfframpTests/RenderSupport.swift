// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Foundation
import ImageIO
import StickShift
import Testing
import simd

@testable import OfframpGame

// Helpers for the render, asset and HUD tests: null and Metal contexts, a
// world to build test scenes in, the demo's assets, the recorder's frame 150
// rebuilt from the Rust dumps, and the probe's HUD quads.

/// `OFFRAMP_RUST_FRAMES`: a directory of the Rust recorder's frames
/// (`record_demo <dir>`), which the tests that compare with them need; `nil`
/// skips those tests.
let rustFramesDirectory = ProcessInfo.processInfo.environment["OFFRAMP_RUST_FRAMES"]

/// The path of the demo asset `name`.
func demoAssetPath(_ name: String) -> String {
    demoAssets.appendingPathComponent(name).path
}

/// A null-backend context, `width` × `height` pixels at `scale` pixels per
/// point: no GPU needed.
@MainActor
func nullContext(width: Int = 1280, height: Int = 720, scale: Float = 1) throws -> Context {
    let context = try Context(offscreenWidth: width, height: height, backend: .null)
    if scale != 1 {
        try context.resize(width: width, height: height, scale: scale)
    }
    return context
}

/// A game that does nothing.
@MainActor
final class NoGame: Game {
    func initialize(_ ctx: EngineContext) throws {}
    func update(_ ctx: EngineContext, dt: Float) {}
    func render(_ ctx: EngineContext, into frame: Frame) throws {}
}

/// An initialized engine of a game that does nothing, on a null context: its
/// world (`engine.world`, between ticks) holds a test's hand-built scene.
@MainActor
func scratchEngine() throws -> Engine {
    let engine = try Engine.headless(NoGame())
    try engine.initialize()
    return engine
}

/// Record one frame on `context` (with no engine attached) with `record`, and
/// return what the context recorded for it.
@MainActor
func recordedFrame(_ context: Context, _ record: (Frame) throws -> Void) throws -> FrameRecord {
    try #require(try context.frame(record) != nil, "the frame was acquired and submitted")
    return context.lastFrame
}

/// An offscreen Metal context, or `nil` without a Metal device. With
/// `STICKSHIFT_REQUIRE_GPU=1` a missing device is a failure instead.
@MainActor
func metalContext(width: Int, height: Int) throws -> Context? {
    do {
        return try Context(offscreenWidth: width, height: height)
    } catch let error as StickShiftError where error.code == .noDevice {
        let required = ProcessInfo.processInfo.environment["STICKSHIFT_REQUIRE_GPU"]
        if required == "1" || required?.lowercased() == "true" {
            throw error
        }
        FileHandle.standardError.write(Data("SKIPPED: no Metal device\n".utf8))
        return nil
    }
}

/// Every model the demo draws, loaded through `context`.
@MainActor
func loadDemoModels(_ context: Context) throws -> DemoModels {
    try DemoModels.load(context, assetPath: demoAssetPath)
}

/// The demo's render resources: the models and the two procedural meshes.
@MainActor
func loadResources(_ context: Context) throws -> RenderResources {
    try RenderResources.make(
        models: try loadDemoModels(context),
        tileDepth: StreamingConfig.default.spawnInterval,
        renderer: context
    )
}

/// The committed HUD font, uploaded through `context`.
@MainActor
func loadHudFont(_ context: Context) throws -> HudFont {
    try HudFont.load(context, path: demoAssetPath(FONT_ASSET))
}

/// A 4-vector's lanes as bit patterns.
func bits(_ v: SIMD4<Float>) -> [UInt32] {
    [v.x.bitPattern, v.y.bitPattern, v.z.bitPattern, v.w.bitPattern]
}

/// The engine's draws counted for one frame: how far `stats.drawsSubmitted`
/// moved over `body`.
@MainActor
func drawsSubmitted(_ context: Context, _ body: () throws -> Void) rethrows -> UInt64 {
    let before = context.stats.drawsSubmitted
    try body()
    return context.stats.drawsSubmitted - before
}

/// glam's `Mat4::from_scale_rotation_translation`, written out in scalars
/// (spec 03 §8), to check the engine's TRS against independently.
func glamTRS(_ t: Transform) -> [UInt32] {
    let (x, y, z, w) = (t.rotation.imag.x, t.rotation.imag.y, t.rotation.imag.z, t.rotation.real)
    let x2 = x + x
    let y2 = y + y
    let z2 = z + z
    let xx = x * x2
    let xy = x * y2
    let xz = x * z2
    let yy = y * y2
    let yz = y * z2
    let zz = z * z2
    let wx = w * x2
    let wy = w * y2
    let wz = w * z2
    let s = t.scale
    let p = t.position
    let columns: [Float] = [
        (1 - (yy + zz)) * s.x, (xy + wz) * s.x, (xz - wy) * s.x, 0 * s.x,
        (xy - wz) * s.y, (1 - (xx + zz)) * s.y, (yz + wx) * s.y, 0 * s.y,
        (xz + wy) * s.z, (yz - wx) * s.z, (1 - (xx + yy)) * s.z, 0 * s.z,
        p.x, p.y, p.z, 1,
    ]
    return columns.map(\.bitPattern)
}

// MARK: - The recorder's frame 150

/// The world, camera and HUD of `record_demo`'s frame 150 (its first recorded
/// frame, `frame_0000.png`), rebuilt from the Rust dumps. The world is
/// `engine`'s.
@MainActor
struct RecorderFrame {
    let engine: Engine
    let player: Entity
    let playerPosition: SIMD3<Float>
    let camera: Camera
    let hud: HudState
    let digest: UInt64

    var world: World { engine.world }

    /// The frame's draw list.
    var drawList: SceneDrawList {
        SceneDrawList(world: world, player: player, playerPosition: playerPosition)
    }
}

/// Spawn into `world` an entity of dumped kind `kind` at `transform`, with the
/// components `game.rs` spawns it with.
@MainActor
func spawnDumped(_ world: World, kind: Character, variant: UInt32, transform t: Transform) throws -> Entity {
    switch kind {
    case "P":
        return world.spawn(t, RenderComponent.cube(SIMD3(0.9, 0.15, 0.1)), DynamicTag())
    case "R":
        return world.spawn(t, RenderComponent.cube(SIMD3(0.16, 0.16, 0.2)), StaticTag())
    case "B":
        return world.spawn(t, RenderComponent.cube(SIMD3(0.5, 0.5, 0.5)), StaticTag(), BuildingVariant(Int(variant)))
    case "G":
        return world.spawn(t, RenderComponent.cube(SIMD3(0.6, 0.6, 0.6)), StaticTag(), GuardrailTag())
    case "O":
        return world.spawn(
            t, RenderComponent.cube(SIMD3(0.95, 0.8, 0.1)), StaticTag(), Obstacle(), TrafficVariant(Int(variant))
        )
    default:
        throw FixtureError(description: "unknown entity kind \(kind)")
    }
}

/// Rebuild `record_demo`'s frame 150 in a scratch engine's world: spawning the
/// dumped entities in query order recreates hecs's archetypes and rows in the
/// same order.
@MainActor
func recorderFrame150() throws -> RecorderFrame {
    let engine = try scratchEngine()
    var player: Entity?
    for e in try worldDump("record_frame150_world.txt") {
        let b = e.bits.map { Float(bitPattern: $0) }
        let transform = Transform(
            position: SIMD3(b[0], b[1], b[2]),
            rotation: simd_quatf(ix: b[3], iy: b[4], iz: b[5], r: b[6]),
            scale: SIMD3(b[7], b[8], b[9])
        )
        let entity = try spawnDumped(engine.world, kind: e.kind, variant: e.variant, transform: transform)
        if e.kind == "P" {
            player = entity
        }
    }

    let frame = try #require(try trace("record_trace.txt").first { $0.frame == 150 })
    let lines = try fixtureLines("record_view_projection.txt")
    let start = try #require(lines.firstIndex { $0.hasPrefix("frame 150 view_projection") })
    let columns = lines[(start + 1)...(start + 4)].map { bitPatterns(in: Substring($0)).map { Float(bitPattern: $0) } }
    let vp = simd_float4x4(
        SIMD4(columns[0][0], columns[0][1], columns[0][2], columns[0][3]),
        SIMD4(columns[1][0], columns[1][1], columns[1][2], columns[1][3]),
        SIMD4(columns[2][0], columns[2][1], columns[2][2], columns[2][3]),
        SIMD4(columns[3][0], columns[3][1], columns[3][2], columns[3][3])
    )
    #expect(frame.state == "Playing")
    // The trace prints `fade_alpha()`.
    let hud = HudState(state: .playing, score: UInt64(frame.score), bestScore: UInt64(frame.best), fadeAlpha: frame.fade)
    return RecorderFrame(
        engine: engine,
        player: try #require(player),
        playerPosition: frame.player,
        camera: Camera(viewProjection: vp, position: frame.cameraPosition),
        hud: hud,
        digest: frame.digest
    )
}

// MARK: - The probe's HUD quads

/// One quad the Rust probe printed: `[i] rect [x, y, w, h] color [r, g, b, a] fill F`
/// (color and fill are left out in some sections).
struct ProbeQuad {
    let rect: SIMD4<Float>
    let color: SIMD4<Float>?
    let fill: String?
}

/// The four floats in `[...]` after `key` in `line`, or `nil` if there is no `key`.
private func floats4(_ key: String, in line: String) throws -> SIMD4<Float>? {
    guard line.range(of: key) != nil else { return nil }
    let inner = try after(key, in: line).prefix { $0 != "]" }
    let values = inner.split(separator: ",").compactMap { Float($0.trimmingCharacters(in: .whitespaces)) }
    guard values.count == 4 else { throw FixtureError(description: "not four floats after \(key) in: \(line)") }
    return SIMD4(values[0], values[1], values[2], values[3])
}

/// The `count` quads listed under the probe section starting with `heading`.
func probeQuads(after heading: String, count: Int) throws -> [ProbeQuad] {
    let lines = try fixtureLines("probe_stdout.txt")
    let start = try #require(lines.firstIndex { $0.hasPrefix(heading) }, "no section \(heading)")
    return try lines[(start + 1)...].prefix(count).map { line in
        let rect = try #require(try floats4("rect [", in: line), "no rect in: \(line)")
        let fill = line.range(of: "fill ").map { String(line[$0.upperBound...]) }
        return ProbeQuad(rect: rect, color: try floats4("color [", in: line), fill: fill)
    }
}

// MARK: - Pixels

/// A PNG's pixels as RGBA8, top row first, exactly as stored (ImageIO's decoded
/// samples, without colour management).
func pngPixels(_ url: URL) throws -> (width: Int, height: Int, rgba: [UInt8]) {
    let source = try #require(CGImageSourceCreateWithURL(url as CFURL, nil), "cannot open \(url.path)")
    let image = try #require(CGImageSourceCreateImageAtIndex(source, 0, nil), "cannot decode \(url.path)")
    try #require(image.bitsPerComponent == 8 && image.bitsPerPixel == 32, "\(url.path) is not 8-bit RGBA")
    let data = try #require(image.dataProvider?.data as Data?)
    var rgba = [UInt8](repeating: 0, count: image.width * image.height * 4)
    for row in 0..<image.height {
        let from = row * image.bytesPerRow
        rgba.replaceSubrange((row * image.width * 4)..<((row + 1) * image.width * 4), with: data[from..<(from + image.width * 4)])
    }
    return (image.width, image.height, rgba)
}

/// BGRA8 from a read-back, as RGBA8.
func rgba(fromBGRA bytes: [UInt8]) -> [UInt8] {
    var out = bytes
    for i in stride(from: 0, to: out.count, by: 4) {
        out.swapAt(i, i + 2)
    }
    return out
}

/// Record one frame on `context` (with no engine attached) with `record`, read
/// it back, and return it as RGBA8, top row first.
@MainActor
func framePixels(_ context: Context, _ record: (Frame) throws -> Void) throws -> [UInt8] {
    var readback: PixelReadback?
    let number = try context.frame { frame in
        try record(frame)
        readback = try frame.readPixels()
    }
    try #require(number != nil, "the frame was rendered")
    // Wait for the GPU, then take the pixels.
    try context.flush()
    return rgba(fromBGRA: try #require(readback?.bytes, "the read-back arrived"))
}
