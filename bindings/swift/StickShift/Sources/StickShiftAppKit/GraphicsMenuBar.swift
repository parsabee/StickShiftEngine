// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import AppKit
import CStickShift
@_spi(CInterop) import StickShift

/// The engine's Graphics menu in the menu bar: the `NSMenu` the Rust runner
/// installs (`platform/macos.rs`), built from the engine's description of it.
///
/// ```text
/// Graphics
///   Shadows          ▸ Off · Low · High
///   Shadow Distance  ▸ Match Draw Distance · Medium · Near
///   Draw Distance    ▸ Near · Medium · Far
///   Resolution       ▸ 50% · 75% · 100%
///   ─────────────
///   Enter Full Screen            ⌃⌘F
///   Reset to Defaults
/// ```
///
/// Every item has the engine's titles, checkmarks and enabled states, and its
/// tag; all of them share one target (this object) and one action, which hands
/// the chosen item's tag to `choose`. Send it to the engine as a
/// ``HostEvent/menu(_:)`` event; ``EngineView/makeMenuBar()`` wires that up.
/// The menus enable their items only as the engine says (`autoenablesItems`
/// is off). Call ``sync(engine:fullScreen:)`` when the engine returns
/// ``HostRequests/syncMenu``, and when the window enters or leaves full screen.
@MainActor
public final class GraphicsMenuBar: NSObject {
    /// The Graphics menu.
    public let menu: NSMenu
    /// The item that opens it from the menu bar.
    public let menuBarItem: NSMenuItem
    /// Every item that chooses something, by tag.
    private var items: [GraphicsMenu.Tag: NSMenuItem] = [:]
    private let choose: (GraphicsMenu.Tag) -> Void

    /// The tag of the full-screen item, Rust's `MenuCommand::ToggleFullScreen`
    /// (`settings_menu.rs`): the one item with a key equivalent, ⌃⌘F.
    static let fullScreenTag = GraphicsMenu.Tag(rawValue: 901)

    /// The Graphics menu for `engine`'s settings, its full-screen item titled
    /// for `fullScreen`, handing each choice's tag to `choose`. Between engine
    /// calls only, as every engine call outside a hook is.
    public convenience init(engine: Engine, fullScreen: Bool = false, choose: @escaping (GraphicsMenu.Tag) -> Void) {
        self.init(MenuData(engine: engine, fullScreen: fullScreen), choose: choose)
    }

    init(_ data: MenuData, choose: @escaping (GraphicsMenu.Tag) -> Void) {
        self.choose = choose
        menu = Self.titledMenu(data.title)
        menuBarItem = Self.parentItem(data.title, menu)
        super.init()
        for group in data.groups {
            if let title = group.title {
                let submenu = Self.titledMenu(title)
                for item in group.items {
                    submenu.addItem(self.item(item))
                }
                menu.addItem(Self.parentItem(title, submenu))
            } else {
                // The actions, after a separator.
                menu.addItem(.separator())
                for item in group.items {
                    menu.addItem(self.item(item))
                }
            }
        }
    }

    /// Append the Graphics menu to `menuBar` (the application's main menu),
    /// after what it has.
    public func install(in menuBar: NSMenu) {
        menuBar.addItem(menuBarItem)
    }

    /// Show `engine`'s settings: every item's checkmark and enabled state, and
    /// the full-screen item's title for `fullScreen`.
    public func sync(engine: Engine, fullScreen: Bool) {
        sync(MenuData(engine: engine, fullScreen: fullScreen))
    }

    func sync(_ data: MenuData) {
        for group in data.groups {
            for entry in group.items {
                guard let item = items[entry.tag] else { continue }
                item.title = entry.title
                item.state = entry.isChecked ? .on : .off
                item.isEnabled = entry.isEnabled
            }
        }
    }

    /// A menu whose items the engine enables and disables, not AppKit.
    private static func titledMenu(_ title: String) -> NSMenu {
        let menu = NSMenu(title: title)
        menu.autoenablesItems = false
        return menu
    }

    /// An item with no action that opens `submenu`.
    private static func parentItem(_ title: String, _ submenu: NSMenu) -> NSMenuItem {
        let item = NSMenuItem(title: title, action: nil, keyEquivalent: "")
        item.submenu = submenu
        return item
    }

    /// The item for `entry`, with the shared target and action.
    private func item(_ entry: MenuData.Item) -> NSMenuItem {
        let fullScreen = entry.tag == Self.fullScreenTag
        let item = NSMenuItem(
            title: entry.title, action: #selector(itemSelected(_:)), keyEquivalent: fullScreen ? "f" : ""
        )
        if fullScreen {
            item.keyEquivalentModifierMask = [.control, .command]
        }
        item.target = self
        item.tag = Int(entry.tag.rawValue)
        item.state = entry.isChecked ? .on : .off
        item.isEnabled = entry.isEnabled
        items[entry.tag] = item
        return item
    }

    @objc private func itemSelected(_ sender: NSMenuItem) {
        guard let raw = UInt32(exactly: sender.tag) else { return }
        choose(GraphicsMenu.Tag(rawValue: raw))
    }
}

/// The engine's Graphics menu as data, read from `stickshift_engine_get_menu`.
///
/// A stand-in until the core target describes the menu itself
/// (`Engine.menu(fullScreen:)`); the menu bar needs only these fields.
struct MenuData: Equatable {
    struct Item: Equatable {
        var tag: GraphicsMenu.Tag
        var title: String
        var isChecked: Bool
        var isEnabled: Bool
    }

    struct Group: Equatable {
        /// The submenu's title, or `nil` for the actions after a separator.
        var title: String?
        var items: [Item]
    }

    var title: String
    var groups: [Group]
    /// The settings revision the checkmarks show.
    var revision: UInt64

    @MainActor
    init(engine: Engine, fullScreen: Bool) {
        var menu = StickShiftMenu()
        menu.size = UInt32(MemoryLayout<StickShiftMenu>.size)
        let result = stickshift_engine_get_menu(engine.cEngine, fullScreen ? 1 : 0, &menu)
        guard result == STICKSHIFT_OK else {
            fatalError(
                "StickShift: reading the engine's Graphics menu failed (\(result)): "
                    + String(cString: stickshift_last_error_message())
            )
        }
        title = String(cString: menu.title)
        revision = menu.revision
        groups = Self.elements(menu.groups, count: menu.group_count, stride: menu.group_stride).map { group in
            let items = Self.elements(group.items, count: group.item_count, stride: group.item_stride).map { item in
                Item(
                    tag: GraphicsMenu.Tag(rawValue: UInt32(bitPattern: item.tag)),
                    title: String(cString: item.label),
                    isChecked: item.checked != 0,
                    isEnabled: item.enabled != 0
                )
            }
            return Group(title: group.title.map { String(cString: $0) }, items: items)
        }
    }

    /// `count` C values from `base`, `stride` bytes apart, as the engine lays
    /// its arrays out.
    private static func elements<T>(_ base: UnsafePointer<T>?, count: UInt32, stride: UInt32) -> [T] {
        guard let base, count > 0 else { return [] }
        let raw = UnsafeRawPointer(base)
        return (0..<Int(count)).map { raw.load(fromByteOffset: $0 * Int(stride), as: T.self) }
    }
}
