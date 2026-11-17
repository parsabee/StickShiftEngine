// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import CStickShift
@_spi(CInterop) import StickShift

/// A game that does nothing, and counts its hooks.
@MainActor
final class Idle: Game {
    var initialized = 0
    var updates = 0
    var renders = 0
    /// Ask to exit from the next update.
    var exitsOnUpdate = false

    func initialize(_ ctx: EngineContext) throws {
        initialized += 1
    }

    func update(_ ctx: EngineContext, dt: Float) {
        updates += 1
        if exitsOnUpdate {
            ctx.requestExit()
        }
    }

    func render(_ ctx: EngineContext, into frame: Frame) throws {
        renders += 1
    }
}

/// Silent, nothing saved, Escape quitting: an app's options without the sound
/// device or the player's saved settings.
let testOptions = Engine.Options(quitsOnEscape: true)

extension Engine {
    /// The engine camera's aspect ratio, read through C (the core target's
    /// camera API is not this target's).
    var cameraAspect: Float {
        var state = StickShiftCameraState()
        state.size = UInt32(MemoryLayout<StickShiftCameraState>.size)
        let result = stickshift_camera_get(cEngine, &state)
        precondition(result == STICKSHIFT_OK, "stickshift_camera_get returned \(result)")
        return state.aspect_ratio
    }

    /// The input as the game sees it, read through C.
    var inputState: StickShiftInputState {
        var state = StickShiftInputState()
        state.size = UInt32(MemoryLayout<StickShiftInputState>.size)
        let result = stickshift_input_get(cEngine, &state)
        precondition(result == STICKSHIFT_OK, "stickshift_input_get returned \(result)")
        return state
    }

    /// Whether `key` is held.
    func isKeyDown(_ key: Key) -> Bool {
        withUnsafeBytes(of: inputState.keys) { $0[Int(key.rawValue)] } & UInt8(STICKSHIFT_KEY_STATE_DOWN) != 0
    }

    /// Whether `button` is held.
    func isMouseDown(_ button: MouseButton) -> Bool {
        withUnsafeBytes(of: inputState.mouse_buttons) { $0[Int(button.rawValue)] } != 0
    }

    /// The current graphics settings' render scale, as its C value.
    var renderScaleCode: UInt32 {
        var settings = StickShiftGraphicsSettings()
        let result = stickshift_graphics_get_current(cEngine, &settings)
        precondition(result == STICKSHIFT_OK, "stickshift_graphics_get_current returned \(result)")
        return settings.render_scale
    }
}
