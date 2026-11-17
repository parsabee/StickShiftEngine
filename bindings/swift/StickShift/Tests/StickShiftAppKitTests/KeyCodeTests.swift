// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import StickShift
import StickShiftAppKit
import Testing

/// The macOS keycode table: winit's mapping of the keys the engine knows
/// (`translate_key` in the Rust runner), by physical position.
@Suite("keycodes")
struct KeyCodeTests {
    @Test func everyKeyHasItsMacKeyCode() {
        let expected: [UInt16: Key] = [
            0x00: .a, 0x01: .s, 0x02: .d, 0x0C: .q, 0x0D: .w, 0x0E: .e,
            0x31: .space, 0x35: .escape, 0x7B: .left, 0x7C: .right, 0x7D: .down, 0x7E: .up,
        ]
        for (code, key) in expected {
            #expect(Key(macKeyCode: code) == key, "keycode \(code)")
        }
        // Every key the engine knows, once, and no other code.
        var mapped: [Key: UInt16] = [:]
        for code in UInt16(0)...UInt16(0xFF) {
            guard let key = Key(macKeyCode: code) else { continue }
            #expect(mapped[key] == nil, "\(key) has two keycodes")
            mapped[key] = code
        }
        #expect(Set(mapped.keys) == Set(Key.allCases))
        #expect(mapped.count == expected.count)
        #expect(Key(macKeyCode: 0x24) == nil, "Return is not an engine key")
        #expect(Key(macKeyCode: .max) == nil)
    }

    @Test func mouseButtonsAreTheThreeWinitReports() {
        #expect(MouseButton(macButtonNumber: 0) == .left)
        #expect(MouseButton(macButtonNumber: 1) == .right)
        #expect(MouseButton(macButtonNumber: 2) == .middle)
        #expect(MouseButton(macButtonNumber: 3) == nil)
        #expect(MouseButton(macButtonNumber: -1) == nil)
    }
}
