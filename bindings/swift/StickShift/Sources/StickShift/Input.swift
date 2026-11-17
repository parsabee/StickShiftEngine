// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import CStickShift

/// The keyboard, mouse buttons and cursor as the game sees them: the
/// engine's `InputState`.
///
/// The host feeds it with ``HostEvent``s. A press or release is an edge that
/// lasts until the end of the next ``Game/update(_:dt:)``, so a tap between
/// two frames reaches exactly one update, even across frames that run none.
@MainActor
public struct Input {
    let handle: EngineHandle

    private func read(_ what: @autoclosure () -> String) -> StickShiftInputState {
        var out = StickShiftInputState()
        out.size = UInt32(MemoryLayout<StickShiftInputState>.size)
        handle.call(what()) { stickshift_input_get($0, &out) }
        return out
    }

    private func bits(_ key: Key, _ what: @autoclosure () -> String) -> Int32 {
        let state = read(what())
        return withUnsafeBytes(of: state.keys) { Int32($0[Int(key.rawValue)]) }
    }

    /// Whether `key` is held.
    public func isKeyDown(_ key: Key) -> Bool {
        bits(key, "Input.isKeyDown(_:)") & STICKSHIFT_KEY_STATE_DOWN != 0
    }

    /// Whether `key` went down since the last update ended.
    public func isKeyJustPressed(_ key: Key) -> Bool {
        bits(key, "Input.isKeyJustPressed(_:)") & STICKSHIFT_KEY_STATE_JUST_PRESSED != 0
    }

    /// Whether `key` went up since the last update ended.
    public func isKeyJustReleased(_ key: Key) -> Bool {
        bits(key, "Input.isKeyJustReleased(_:)") & STICKSHIFT_KEY_STATE_JUST_RELEASED != 0
    }

    /// Whether `button` is held.
    public func isMouseDown(_ button: MouseButton) -> Bool {
        let state = read("Input.isMouseDown(_:)")
        return withUnsafeBytes(of: state.mouse_buttons) { $0[Int(button.rawValue)] != 0 }
    }

    /// The cursor, in points from the view's top-left, or `nil` until the
    /// host has reported one.
    public var cursorPosition: SIMD2<Double>? {
        let state = read("Input.cursorPosition")
        return state.has_cursor != 0 ? SIMD2(state.cursor_x, state.cursor_y) : nil
    }
}

/// The input as the host drives it, from ``Engine/input``: what ``Input``
/// reads, and presses and releases that are ``Engine/push(_:)`` calls.
@MainActor
public struct HostInput {
    let engine: Engine

    private var input: Input { Input(handle: engine.handle) }

    /// Whether `key` is held.
    public func isKeyDown(_ key: Key) -> Bool { input.isKeyDown(key) }
    /// Whether `key` went down since the last update ended.
    public func isKeyJustPressed(_ key: Key) -> Bool { input.isKeyJustPressed(key) }
    /// Whether `key` went up since the last update ended.
    public func isKeyJustReleased(_ key: Key) -> Bool { input.isKeyJustReleased(key) }
    /// Whether `button` is held.
    public func isMouseDown(_ button: MouseButton) -> Bool { input.isMouseDown(button) }
    /// The cursor, in points from the view's top-left, or `nil` until reported.
    public var cursorPosition: SIMD2<Double>? { input.cursorPosition }

    /// Press `key`: ``HostEvent/keyDown(_:isRepeat:)``.
    @discardableResult
    public func press(_ key: Key, isRepeat: Bool = false) -> HostRequests {
        engine.push(.keyDown(key, isRepeat: isRepeat))
    }

    /// Release `key`: ``HostEvent/keyUp(_:)``.
    @discardableResult
    public func release(_ key: Key) -> HostRequests {
        engine.push(.keyUp(key))
    }

    /// Press `button`: ``HostEvent/mouseDown(_:)``.
    @discardableResult
    public func press(_ button: MouseButton) -> HostRequests {
        engine.push(.mouseDown(button))
    }

    /// Release `button`: ``HostEvent/mouseUp(_:)``.
    @discardableResult
    public func release(_ button: MouseButton) -> HostRequests {
        engine.push(.mouseUp(button))
    }

    /// Move the cursor: ``HostEvent/cursor(_:)``.
    @discardableResult
    public func moveCursor(to point: SIMD2<Double>) -> HostRequests {
        engine.push(.cursor(point))
    }

    /// Release every key and button, with edges: ``HostEvent/focusLost``.
    @discardableResult
    public func releaseAll() -> HostRequests {
        engine.push(.focusLost)
    }
}
