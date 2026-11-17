// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import CStickShift

/// The graphics settings as a hook sees them, from
/// ``EngineContext/graphics``: the settings in effect, and requests for new
/// ones.
@MainActor
public struct Graphics {
    let handle: EngineHandle

    /// The settings in effect. A request shows here once it is applied.
    public var current: GraphicsSettings {
        var out = StickShiftGraphicsSettings()
        handle.call("Graphics.current") { stickshift_graphics_get_current($0, &out) }
        return GraphicsSettings(out)
    }

    /// Ask for new settings, applied at the start of the next frame (before
    /// init, by init). Requests before then collapse to the last; one equal
    /// to the settings in effect changes nothing. When they apply, the engine
    /// pushes them to the renderer and the scene, then runs
    /// ``Game/graphicsSettingsChanged(_:_:)``.
    public func request(_ settings: GraphicsSettings) {
        var c = settings.c
        handle.call("Graphics.request(_:)") { stickshift_graphics_request($0, &c) }
    }
}

/// The graphics settings as the host sees them, from ``Engine/graphics``:
/// what a hook sees, plus what is queued and how often they changed.
/// Between ticks only.
@MainActor
public struct GraphicsControl {
    let handle: EngineHandle

    private var graphics: Graphics { Graphics(handle: handle) }

    private func state(_ what: @autoclosure () -> String) -> StickShiftGraphicsState {
        var out = StickShiftGraphicsState()
        out.size = UInt32(MemoryLayout<StickShiftGraphicsState>.size)
        handle.call(what()) { stickshift_graphics_get_state($0, &out) }
        return out
    }

    /// The settings in effect.
    public var current: GraphicsSettings { graphics.current }

    /// Ask for new settings, as ``Graphics/request(_:)`` does.
    public func request(_ settings: GraphicsSettings) { graphics.request(settings) }

    /// The request the next frame applies, if one is queued.
    public var pending: GraphicsSettings? {
        let s = state("GraphicsControl.pending")
        return s.has_pending != 0 ? GraphicsSettings(s.pending) : nil
    }

    /// How many changes have applied since the engine was created (restoring
    /// saved settings is not one).
    public var revision: UInt64 {
        state("GraphicsControl.revision").revision
    }

    /// The streaming reach the game set up in init, which the draw distance
    /// scales; `nil` before init.
    public var baseSpawnAhead: Float? {
        let s = state("GraphicsControl.baseSpawnAhead")
        return s.has_base_spawn_ahead != 0 ? s.base_spawn_ahead : nil
    }
}

extension GraphicsSettings {
    /// What these settings give the renderer: what an engine pushes to its
    /// context when they apply.
    public var renderSettings: RenderSettings {
        var settings = c
        var out = StickShiftRenderSettings()
        stickshift_graphics_settings_render_settings(&settings, &out)
        return RenderSettings(out)
    }
}

/// The engine's Graphics menu, as data: a host builds its native menu from
/// it, and sends the engine a ``HostEvent/menu(_:)`` event with the ``Tag``
/// of the item the player chose. Get it from ``Engine/menu(fullScreen:)``,
/// again whenever a call returns ``HostRequests/syncMenu``.
public struct GraphicsMenu: Sendable, Equatable {
    /// A menu item's tag: what a ``HostEvent/menu(_:)`` event names.
    public struct Tag: RawRepresentable, Hashable, Sendable {
        /// The engine's number for the item: its group's section times 100,
        /// plus its place.
        public let rawValue: UInt32

        /// Creates a tag from the engine's number for an item.
        public init(rawValue: UInt32) {
            self.rawValue = rawValue
        }
    }

    /// One item.
    public struct Item: Sendable, Equatable {
        /// What choosing it sends.
        public var tag: Tag
        /// Its title: "Off", "Match Draw Distance", "50%", "Enter Full Screen"...
        public var title: String
        /// Whether it shows a checkmark: the current choice of its group.
        public var isChecked: Bool
        /// Whether it can be chosen.
        public var isEnabled: Bool
    }

    /// A group of items: a submenu of choices, or (with no title) the actions
    /// shown after a separator.
    public struct Group: Sendable, Equatable {
        /// The submenu's title, or `nil` for the actions after a separator.
        public var title: String?
        /// The items, in menu order.
        public var items: [Item]
    }

    /// The menu's title: "Graphics".
    public var title: String
    /// The groups, in menu order: the four settings submenus, then the
    /// actions (full screen, reset).
    public var groups: [Group]
    /// The settings revision the checkmarks show (``GraphicsControl/revision``).
    public var revision: UInt64

    /// Copy out the engine-owned menu `m`.
    init(_ m: StickShiftMenu) {
        title = m.title.map { String(cString: $0) } ?? ""
        revision = m.revision
        var groups: [Group] = []
        if let base = m.groups.map(UnsafeRawPointer.init) {
            for g in 0..<Int(m.group_count) {
                let group = (base + g * Int(m.group_stride)).loadUnaligned(as: StickShiftMenuGroup.self)
                var items: [Item] = []
                if let first = group.items.map(UnsafeRawPointer.init) {
                    for i in 0..<Int(group.item_count) {
                        let item = (first + i * Int(group.item_stride)).loadUnaligned(as: StickShiftMenuItem.self)
                        items.append(
                            Item(
                                tag: Tag(rawValue: UInt32(bitPattern: item.tag)),
                                title: item.label.map { String(cString: $0) } ?? "",
                                isChecked: item.checked != 0,
                                isEnabled: item.enabled != 0
                            )
                        )
                    }
                }
                groups.append(Group(title: group.title.map { String(cString: $0) }, items: items))
            }
        }
        self.groups = groups
    }
}
