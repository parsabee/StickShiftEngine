// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import CStickShift
import simd
@_spi(CInterop) import StickShift

// A stand-in for the core target's world, scene and camera API, which lands
// alongside this example: just what `main.swift` uses, with the same names and
// call sites, made of C calls. Delete this file once `StickShift` has
// `EngineContext.world`, `.scene` and `.camera`, `Transform` and
// `RenderComponent`; `main.swift` then builds against those unchanged.

extension EngineContext {
    var world: World { World(engine: cEngine) }
    var scene: Scene { Scene(engine: cEngine) }
    var camera: EngineCamera { EngineCamera(engine: cEngine) }
}

/// Trap on a C call the engine refused: a programmer error, as in the package.
private func check(_ result: StickShiftResult, _ what: String) {
    guard result >= 0 else {
        fatalError("Cubes: \(what) failed (\(result)): \(String(cString: stickshift_last_error_message()))")
    }
}

struct Entity: Hashable {
    let bits: UInt64
}

/// The engine's `TransformComponent`.
struct Transform {
    var position: SIMD3<Float>
    var rotation = simd_quatf(ix: 0, iy: 0, iz: 0, r: 1)
    var scale: SIMD3<Float> = .one

    init(position: SIMD3<Float>, scale: SIMD3<Float> = .one) {
        self.position = position
        self.scale = scale
    }

    init(_ c: StickShiftTransform) {
        position = [c.position.x, c.position.y, c.position.z]
        rotation = simd_quatf(ix: c.rotation.x, iy: c.rotation.y, iz: c.rotation.z, r: c.rotation.w)
        scale = [c.scale.x, c.scale.y, c.scale.z]
    }

    var c: StickShiftTransform {
        StickShiftTransform(
            position: StickShiftVec3(x: position.x, y: position.y, z: position.z),
            rotation: StickShiftQuat(x: rotation.imag.x, y: rotation.imag.y, z: rotation.imag.z, w: rotation.real),
            scale: StickShiftVec3(x: scale.x, y: scale.y, z: scale.z)
        )
    }

    /// Scale, then rotation, then translation: the engine's own matrix.
    var matrix: simd_float4x4 { unsafeBitCast(stickshift_transform_to_mat4(c), to: simd_float4x4.self) }
}

/// The engine's `RenderComponent`: a colour and a built-in shape.
struct RenderComponent {
    var color: SIMD3<Float>
    static func cube(_ color: SIMD3<Float>) -> RenderComponent { RenderComponent(color: color) }
    var c: StickShiftRenderComponent {
        StickShiftRenderComponent(color: (color.x, color.y, color.z), shape: StickShiftRenderShape(STICKSHIFT_RENDER_SHAPE_CUBE))
    }
}

@MainActor
struct World {
    let engine: OpaquePointer

    @discardableResult
    func spawn(_ transform: Transform, _ render: RenderComponent) -> Entity {
        var t = transform.c
        var r = render.c
        var out = StickShiftEntity()
        withUnsafePointer(to: &t) { t in
            withUnsafePointer(to: &r) { r in
                let values = [
                    StickShiftComponentValue(
                        component: StickShiftComponent(id: UInt32(STICKSHIFT_COMPONENT_TRANSFORM)), data: t,
                        size: UInt32(MemoryLayout<StickShiftTransform>.size)),
                    StickShiftComponentValue(
                        component: StickShiftComponent(id: UInt32(STICKSHIFT_COMPONENT_RENDER)), data: r,
                        size: UInt32(MemoryLayout<StickShiftRenderComponent>.size)),
                ]
                check(stickshift_world_spawn(engine, values, 2, &out), "spawn")
            }
        }
        return Entity(bits: out.bits)
    }

    /// The entities with both, in the engine's query order, decoded first.
    func query(_: Transform.Type, _: RenderComponent.Type) -> [(Entity, Transform, RenderComponent)] {
        let terms = [
            StickShiftQueryTerm(component: StickShiftComponent(id: UInt32(STICKSHIFT_COMPONENT_TRANSFORM)), access: 0),
            StickShiftQueryTerm(component: StickShiftComponent(id: UInt32(STICKSHIFT_COMPONENT_RENDER)), access: 0),
        ]
        var rows = StickShiftQueryRows()
        rows.size = UInt32(MemoryLayout<StickShiftQueryRows>.size)
        check(stickshift_world_query(engine, terms, 2, &rows), "query")
        defer { check(stickshift_world_query_release(engine, rows.token), "query release") }
        return (0..<Int(rows.count)).map { i in
            let row = UnsafeRawPointer(rows.data!) + i * rows.stride
            let t = row.load(fromByteOffset: Int(rows.offsets[0]), as: StickShiftTransform.self)
            let r = row.load(fromByteOffset: Int(rows.offsets[1]), as: StickShiftRenderComponent.self)
            return (Entity(bits: rows.entities[i].bits), Transform(t), RenderComponent(color: [r.color.0, r.color.1, r.color.2]))
        }
    }
}

struct StreamingConfig {
    var axis: SIMD3<Float>
}

/// One slot a stream pass is filling.
@MainActor
struct StreamSlot {
    let engine: OpaquePointer
    let index: Int64
    let position: SIMD3<Float>

    /// Spawn into the slot, tracked for despawning behind.
    @discardableResult
    func spawn(_ transform: Transform, _ render: RenderComponent) -> Entity {
        let entity = World(engine: engine).spawn(transform, render)
        check(stickshift_stream_track(engine, StickShiftEntity(bits: entity.bits)), "track")
        return entity
    }
}

/// The C pointers a stream callback is called with, used on the main thread
/// before the callback returns.
private struct SlotCall: @unchecked Sendable {
    let user: UnsafeMutableRawPointer
    let engine: OpaquePointer
}

@MainActor
struct Scene {
    let engine: OpaquePointer

    var config: StreamingConfig {
        var c = StickShiftStreamingConfig()
        check(stickshift_scene_get_config(engine, &c), "scene config")
        return StreamingConfig(axis: [c.axis.x, c.axis.y, c.axis.z])
    }

    func stream(focus: SIMD3<Float>, _ fill: (StreamSlot) -> Void) {
        withoutActuallyEscaping(fill) { fill in
            var fill = fill
            withUnsafeMutablePointer(to: &fill) { user in
                let focus = StickShiftVec3(x: focus.x, y: focus.y, z: focus.z)
                check(
                    stickshift_scene_stream(engine, focus, { user, engine, index, p in
                        let call = SlotCall(user: user!, engine: engine!)
                        MainActor.assumeIsolated {
                            let fill = call.user.assumingMemoryBound(to: ((StreamSlot) -> Void).self).pointee
                            fill(StreamSlot(engine: call.engine, index: index, position: [p.x, p.y, p.z]))
                        }
                    }, user),
                    "stream"
                )
            }
        }
    }

    /// Rebase the world if `focus` has gone far; returns the shift applied to it.
    func maybeRebase(focus: SIMD3<Float>) -> SIMD3<Float> {
        var offset = StickShiftVec3()
        check(stickshift_scene_maybe_rebase(engine, StickShiftVec3(x: focus.x, y: focus.y, z: focus.z), &offset), "rebase")
        return [offset.x, offset.y, offset.z]
    }
}

@MainActor
struct EngineCamera {
    let engine: OpaquePointer

    var position: SIMD3<Float> {
        get { state.position }
        nonmutating set { check(stickshift_camera_set_position(engine, StickShiftVec3(x: newValue.x, y: newValue.y, z: newValue.z)), "camera") }
    }

    var target: SIMD3<Float> {
        get { state.target }
        nonmutating set { check(stickshift_camera_set_target(engine, StickShiftVec3(x: newValue.x, y: newValue.y, z: newValue.z)), "camera") }
    }

    private var state: (position: SIMD3<Float>, target: SIMD3<Float>) {
        var s = StickShiftCameraState()
        s.size = UInt32(MemoryLayout<StickShiftCameraState>.size)
        check(stickshift_camera_get(engine, &s), "camera")
        return ([s.position.x, s.position.y, s.position.z], [s.target.x, s.target.y, s.target.z])
    }
}
