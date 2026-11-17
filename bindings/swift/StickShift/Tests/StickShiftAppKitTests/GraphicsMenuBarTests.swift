// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import AppKit
import StickShift
import Testing

@testable import StickShiftAppKit

/// The Graphics menu built from the engine's description of it, as the Rust
/// runner builds its own (`platform/macos.rs`), on a null-backend engine.
@MainActor
@Suite("Graphics menu bar")
struct GraphicsMenuBarTests {
    /// An initialized engine with no GPU. Makes the application too: a menu
    /// sends its items' actions through it, and does nothing without one.
    private func engine() throws -> Engine {
        _ = NSApplication.shared
        let engine = try Engine.headless(Idle())
        try engine.initialize()
        return engine
    }

    /// One row per item of `menu`, submenus expanded: the title (indented for a
    /// submenu's items), the tag, ✓ for a checkmark and ✗ for a disabled item.
    private func rows(_ menu: NSMenu, indent: String = "") -> [String] {
        menu.items.flatMap { item -> [String] in
            if item.isSeparatorItem {
                return ["\(indent)---"]
            }
            if let submenu = item.submenu {
                return ["\(indent)\(item.title) ▸"] + rows(submenu, indent: indent + "  ")
            }
            var row = "\(indent)\(item.title) #\(item.tag)"
            if item.state == .on { row += " ✓" }
            if !item.isEnabled { row += " ✗" }
            return [row]
        }
    }

    private var defaultRows: [String] {
        [
            "Shadows ▸",
            "  Off #101",
            "  Low #102",
            "  High #103 ✓",
            "Shadow Distance ▸",
            "  Match Draw Distance #201 ✓",
            "  Medium #202",
            "  Near #203",
            "Draw Distance ▸",
            "  Near #301",
            "  Medium #302",
            "  Far #303 ✓",
            "Resolution ▸",
            "  50% #401 ✓",
            "  75% #402",
            "  100% #403",
            "---",
            "Enter Full Screen #901",
            "Reset to Defaults #902 ✗",
        ]
    }

    @Test func theMenuIsBuiltFromTheEnginesData() throws {
        let engine = try engine()
        let bar = GraphicsMenuBar(engine: engine) { _ in }
        #expect(bar.menu.title == "Graphics")
        #expect(bar.menuBarItem.title == "Graphics")
        #expect(bar.menuBarItem.submenu === bar.menu)
        #expect(rows(bar.menu) == defaultRows)

        // The engine enables the items, not AppKit.
        func menus(_ menu: NSMenu) -> [NSMenu] { [menu] + menu.items.compactMap(\.submenu).flatMap(menus) }
        #expect(menus(bar.menu).count == 5)
        #expect(menus(bar.menu).allSatisfy { !$0.autoenablesItems })

        // Every choosable item has the one target and action; only full screen
        // has a key equivalent, ⌃⌘F.
        let leaves = menus(bar.menu).flatMap(\.items).filter { $0.submenu == nil && !$0.isSeparatorItem }
        #expect(leaves.count == 14)
        #expect(leaves.allSatisfy { $0.target === bar })
        #expect(Set(leaves.compactMap(\.action)).count == 1)
        let fullScreen = try #require(leaves.first { $0.tag == 901 })
        #expect(fullScreen.keyEquivalent == "f")
        #expect(fullScreen.keyEquivalentModifierMask == [.control, .command])
        #expect(leaves.filter { !$0.keyEquivalent.isEmpty }.count == 1)
        // The submenus' parent items choose nothing.
        #expect(bar.menu.items.compactMap(\.submenu).count == 4)
        #expect(bar.menu.items.filter { $0.submenu != nil }.allSatisfy { $0.target !== bar })
    }

    @Test func theFullScreenItemIsTitledForTheWindow() throws {
        let engine = try engine()
        let bar = GraphicsMenuBar(engine: engine, fullScreen: true) { _ in }
        #expect(rows(bar.menu).contains("Exit Full Screen #901"))
        bar.sync(engine: engine, fullScreen: false)
        #expect(rows(bar.menu) == defaultRows)
        bar.sync(engine: engine, fullScreen: true)
        #expect(rows(bar.menu).contains("Exit Full Screen #901"))
    }

    @Test func choosingAnItemSendsItsTagAndSyncShowsTheEnginesAnswer() throws {
        let engine = try engine()
        var chosen: [GraphicsMenu.Tag] = []
        var synced: [HostRequests] = []
        var bar: GraphicsMenuBar?
        bar = GraphicsMenuBar(engine: engine) { tag in
            chosen.append(tag)
            let requests = engine.push(.menu(tag))
            synced.append(requests)
            if requests.contains(.syncMenu) {
                bar?.sync(engine: engine, fullScreen: false)
            }
        }
        let menu = try #require(bar?.menu)

        // Shadows ▸ Off, through the item's own target and action.
        let shadows = try #require(menu.items[0].submenu)
        shadows.performActionForItem(at: 0)
        #expect(chosen == [GraphicsMenu.Tag(rawValue: 101)])
        #expect(synced == [.syncMenu])
        var expected = defaultRows
        expected[1] = "  Off #101 ✓"
        expected[3] = "  High #103"
        // No shadows: their distance means nothing, so it is disabled.
        expected[5] = "  Match Draw Distance #201 ✓ ✗"
        expected[6] = "  Medium #202 ✗"
        expected[7] = "  Near #203 ✗"
        expected[18] = "Reset to Defaults #902"
        #expect(rows(menu) == expected)

        // Full screen is the host's to do: no settings change, no sync.
        menu.performActionForItem(at: menu.items.firstIndex { $0.tag == 901 }!)
        #expect(chosen.last == GraphicsMenu.Tag(rawValue: 901))
        #expect(synced.last == .toggleFullScreen)

        // Reset to Defaults.
        menu.performActionForItem(at: menu.items.firstIndex { $0.tag == 902 }!)
        #expect(synced.last == .syncMenu)
        #expect(rows(menu) == defaultRows)
    }

    @Test func itIsInstalledAfterWhatTheMenuBarHas() throws {
        let engine = try engine()
        let menuBar = StickShiftApp.applicationMenu(named: "Cubes")
        let bar = GraphicsMenuBar(engine: engine) { _ in }
        bar.install(in: menuBar)
        #expect(menuBar.items.count == 2)
        #expect(menuBar.items.last === bar.menuBarItem)
    }

    /// winit's application menu, which `StickShiftApp` installs first.
    @Test func theApplicationMenuIsWinits() throws {
        _ = NSApplication.shared
        let bar = StickShiftApp.applicationMenu(named: "Cubes")
        let menu = try #require(bar.items.first?.submenu)
        #expect(
            menu.items.map { $0.isSeparatorItem ? "---" : $0.title } == [
                "About Cubes", "---", "Services", "Hide Cubes", "Hide Others", "Show All", "---", "Quit Cubes",
            ]
        )
        let quit = try #require(menu.items.last)
        #expect(quit.keyEquivalent == "q")
        #expect(quit.action == #selector(NSApplication.terminate(_:)))
        let hideOthers = try #require(menu.items.first { $0.title == "Hide Others" })
        #expect(hideOthers.keyEquivalentModifierMask == [.option, .command])
    }
}
