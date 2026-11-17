// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import StickShift

extension Key {
    /// The key at macOS virtual key code `keyCode` (`NSEvent.keyCode`, a
    /// physical key position whatever the keyboard layout), as winit maps it
    /// for the Rust runner, or `nil` for a key the engine does not know.
    public init?(macKeyCode keyCode: UInt16) {
        switch keyCode {
        case 0x00: self = .a
        case 0x01: self = .s
        case 0x02: self = .d
        case 0x0C: self = .q
        case 0x0D: self = .w
        case 0x0E: self = .e
        case 0x31: self = .space
        case 0x35: self = .escape
        case 0x7B: self = .left
        case 0x7C: self = .right
        case 0x7D: self = .down
        case 0x7E: self = .up
        default: return nil
        }
    }
}

extension MouseButton {
    /// The button of a macOS mouse event's `buttonNumber`, as winit maps the
    /// three it reports, or `nil` for any other.
    public init?(macButtonNumber number: Int) {
        switch number {
        case 0: self = .left
        case 1: self = .right
        case 2: self = .middle
        default: return nil
        }
    }
}
