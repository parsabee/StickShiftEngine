// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import CStickShift
import simd

/// The engine's camera: the one every frame is drawn from.
///
/// Before ``Game/render(_:into:)``, the engine pushes it to the renderer, so a
/// game sets the camera in its updates and draws nothing camera-related
/// itself. Moving it in a hook pushes it again before the hook's next
/// ``EngineContext/renderer`` call, as in Rust.
///
/// Each property reads or writes the engine's camera directly; a setter
/// changes that one value. (``Camera`` is the plain view-projection value a
/// ``Frame`` is drawn from.)
@MainActor
public struct EngineCamera {
    let handle: EngineHandle

    private func state(_ what: @autoclosure () -> String) -> StickShiftCameraState {
        var out = StickShiftCameraState()
        out.size = UInt32(MemoryLayout<StickShiftCameraState>.size)
        handle.call(what()) { stickshift_camera_get($0, &out) }
        return out
    }

    /// The eye's world position.
    public var position: SIMD3<Float> {
        get { SIMD3(state("EngineCamera.position").position) }
        nonmutating set {
            handle.call("EngineCamera.position") { stickshift_camera_set_position($0, StickShiftVec3(newValue)) }
        }
    }

    /// The point looked at.
    public var target: SIMD3<Float> {
        get { SIMD3(state("EngineCamera.target").target) }
        nonmutating set {
            handle.call("EngineCamera.target") { stickshift_camera_set_target($0, StickShiftVec3(newValue)) }
        }
    }

    /// The world up axis the view is oriented by.
    public var up: SIMD3<Float> {
        get { SIMD3(state("EngineCamera.up").up) }
        nonmutating set {
            handle.call("EngineCamera.up") { stickshift_camera_set_up($0, StickShiftVec3(newValue)) }
        }
    }

    /// The projection's width over height: 4/3 until a view (or the game)
    /// sets it. A view's resize sets it to the view's.
    public var aspectRatio: Float {
        get { state("EngineCamera.aspectRatio").aspect_ratio }
        nonmutating set {
            handle.call("EngineCamera.aspectRatio") { stickshift_camera_set_aspect_ratio($0, newValue) }
        }
    }

    /// The vertical field of view, in radians (45° by default).
    public var fieldOfView: Float {
        get { state("EngineCamera.fieldOfView").fov_y }
        nonmutating set {
            handle.call("EngineCamera.fieldOfView") { stickshift_camera_set_fov_y($0, newValue) }
        }
    }

    /// The near and far clip distances.
    public var clipPlanes: (near: Float, far: Float) {
        let s = state("EngineCamera.clipPlanes")
        return (s.near_plane, s.far_plane)
    }

    /// Set the clip distances. Planes that are not finite, not positive, or
    /// not `far > near` are ignored, as in Rust: the camera keeps its own.
    public func setClipPlanes(near: Float, far: Float) {
        handle.call("EngineCamera.setClipPlanes(near:far:)") { stickshift_camera_set_clip_planes($0, near, far) }
    }

    /// World to view, computed now.
    public var view: simd_float4x4 {
        simd_float4x4(state("EngineCamera.view").view)
    }

    /// View to clip, computed now.
    public var projection: simd_float4x4 {
        simd_float4x4(state("EngineCamera.projection").projection)
    }

    /// World to clip: ``projection`` × ``view``, as the engine multiplies them.
    public var viewProjection: simd_float4x4 {
        simd_float4x4(state("EngineCamera.viewProjection").view_projection)
    }

    /// The unit direction from the eye to the target (`-Z` if they meet).
    public var forward: SIMD3<Float> {
        SIMD3(state("EngineCamera.forward").forward)
    }

    /// The unit direction to the eye's right.
    public var right: SIMD3<Float> {
        SIMD3(state("EngineCamera.right").right)
    }

    /// Move the camera after a target at `target` facing `forward`, `dt`
    /// seconds after the last follow: the engine's chase controller. With a
    /// ``ChaseController/halfLife`` the camera eases toward its place behind
    /// and above the target; without one it snaps there. A zero `forward`
    /// counts as `-Z`.
    public func follow(_ chase: ChaseController, target: SIMD3<Float>, forward: SIMD3<Float>, dt: Float) {
        var c = chase.c
        handle.call("EngineCamera.follow(_:target:forward:dt:)") {
            stickshift_camera_follow($0, &c, StickShiftVec3(target), StickShiftVec3(forward), dt)
        }
    }
}

/// A chase camera: where ``EngineCamera/follow(_:target:forward:dt:)`` puts
/// the camera relative to what it follows.
public struct ChaseController: Sendable, Equatable {
    /// How far behind the target the camera trails, along its facing.
    public var distance: Float
    /// How far above the target the camera sits, along ``up``.
    public var height: Float
    /// How far above the target the camera aims.
    public var lookAtHeight: Float
    /// The world up axis, normalized when used; a zero or non-finite axis
    /// means `+Y`.
    public var up: SIMD3<Float>
    /// Seconds for the camera to close half the gap to its place; 0 snaps,
    /// and a negative value counts as 0.
    public var halfLife: Float

    /// Creates a chase camera; by default aiming at the target itself, `+Y`
    /// up, with no smoothing (`stickshift_chase_init`).
    public init(
        distance: Float,
        height: Float,
        lookAtHeight: Float = 0,
        up: SIMD3<Float> = [0, 1, 0],
        halfLife: Float = 0
    ) {
        self.distance = distance
        self.height = height
        self.lookAtHeight = lookAtHeight
        self.up = up
        self.halfLife = halfLife
    }

    var c: StickShiftChase {
        StickShiftChase(
            distance: distance,
            height: height,
            look_at_height: lookAtHeight,
            up: StickShiftVec3(up),
            half_life: halfLife
        )
    }
}
