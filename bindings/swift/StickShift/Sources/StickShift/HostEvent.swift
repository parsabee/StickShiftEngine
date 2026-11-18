// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import CStickShift

/// Something the platform told the host, for ``Engine/push(_:)``.
///
/// The engine applies the platform policy itself, for every host: Escape
/// quits with ``Engine/Options/quitsOnEscape``, focus loss releases every key,
/// a resize sizes the drawable for the render scale.
public enum HostEvent: Sendable, Equatable {
    /// A key went down. A press of a key already held (`isRepeat`, or a second
    /// press) makes no new edge.
    case keyDown(Key, isRepeat: Bool = false)
    /// A key went up.
    case keyUp(Key)
    /// A mouse button went down.
    case mouseDown(MouseButton)
    /// A mouse button went up.
    case mouseUp(MouseButton)
    /// The cursor moved to this point, in points from the view's top-left.
    case cursor(SIMD2<Double>)
    /// The view lost the keyboard: every key and button is released.
    case focusLost
    /// The view changed size: the camera's aspect follows, the drawable is
    /// resized, and the menu should be synced.
    case viewResized(ViewMetrics)
    /// The view moved to a display of another pixel density: the drawable is
    /// resized, the aspect is left alone.
    case scaleChanged(ViewMetrics)
    /// The view's safe-area insets changed, in points: top, right, bottom, left.
    case safeArea(SIMD4<Float>)
    /// An item of the Graphics menu was chosen.
    case menu(GraphicsMenu.Tag)
    /// The user asked to close the view.
    case close

    var c: StickShiftHostEvent {
        var e = StickShiftHostEvent()
        func kind(_ k: Int32) -> StickShiftHostEventKind { StickShiftHostEventKind(k) }
        func size(_ view: ViewMetrics) {
            guard let width = UInt32(exactly: view.width), let height = UInt32(exactly: view.height) else {
                fatalError("StickShift: a view's width and height must fit in UInt32, not \(view.width) × \(view.height)")
            }
            e.width = width
            e.height = height
            e.scale_factor = view.scaleFactor
        }
        switch self {
        case let .keyDown(key, isRepeat):
            e.kind = kind(STICKSHIFT_HOST_KEY_DOWN)
            e.code = key.rawValue
            e.flags = isRepeat ? UInt32(STICKSHIFT_HOST_EVENT_REPEAT) : 0
        case let .keyUp(key):
            e.kind = kind(STICKSHIFT_HOST_KEY_UP)
            e.code = key.rawValue
        case let .mouseDown(button):
            e.kind = kind(STICKSHIFT_HOST_MOUSE_DOWN)
            e.code = button.rawValue
        case let .mouseUp(button):
            e.kind = kind(STICKSHIFT_HOST_MOUSE_UP)
            e.code = button.rawValue
        case let .cursor(point):
            e.kind = kind(STICKSHIFT_HOST_CURSOR)
            e.x = point.x
            e.y = point.y
        case .focusLost:
            e.kind = kind(STICKSHIFT_HOST_FOCUS_LOST)
        case let .viewResized(view):
            e.kind = kind(STICKSHIFT_HOST_VIEW_RESIZED)
            size(view)
        case let .scaleChanged(view):
            e.kind = kind(STICKSHIFT_HOST_SCALE_CHANGED)
            size(view)
        case let .safeArea(insets):
            e.kind = kind(STICKSHIFT_HOST_SAFE_AREA)
            e.insets = (insets.x, insets.y, insets.z, insets.w)
        case let .menu(tag):
            e.kind = kind(STICKSHIFT_HOST_MENU)
            e.code = tag.rawValue
        case .close:
            e.kind = kind(STICKSHIFT_HOST_CLOSE)
        }
        return e
    }
}

/// A key the engine's input knows.
public enum Key: UInt32, Sendable, CaseIterable {
    /// `W`.
    case w = 0
    /// `A`.
    case a = 1
    /// `S`.
    case s = 2
    /// `D`.
    case d = 3
    /// `Q`.
    case q = 4
    /// `E`.
    case e = 5
    /// The left arrow.
    case left = 6
    /// The right arrow.
    case right = 7
    /// The up arrow.
    case up = 8
    /// The down arrow.
    case down = 9
    /// The space bar.
    case space = 10
    /// Escape.
    case escape = 11
}

/// A mouse button the engine's input knows.
public enum MouseButton: UInt32, Sendable, CaseIterable {
    /// The left (primary) button.
    case left = 0
    /// The right (secondary) button.
    case right = 1
    /// The middle button.
    case middle = 2
}

/// A view's size, as the platform reports it, for ``Engine/resume(_:)`` and
/// the resize events.
public struct ViewMetrics: Sendable, Equatable {
    /// The width, in physical pixels.
    public var width: Int
    /// The height, in physical pixels.
    public var height: Int
    /// Physical pixels per point (`backingScaleFactor`, `contentScaleFactor`).
    public var scaleFactor: Double
    /// The safe-area insets in points: top, right, bottom, left. Zero on macOS.
    /// Only ``Engine/resume(_:)`` reads it; later changes are
    /// ``HostEvent/safeArea(_:)`` events.
    public var safeArea: SIMD4<Float>

    /// Creates metrics.
    public init(width: Int, height: Int, scaleFactor: Double, safeArea: SIMD4<Float> = .zero) {
        self.width = width
        self.height = height
        self.scaleFactor = scaleFactor
        self.safeArea = safeArea
    }

    var c: StickShiftViewDesc {
        get throws(StickShiftError) {
            guard let width = UInt32(exactly: width), let height = UInt32(exactly: height) else {
                throw StickShiftError(code: .invalidArgument, message: "a view's width and height must fit in UInt32")
            }
            var desc = StickShiftViewDesc()
            desc.size = UInt32(MemoryLayout<StickShiftViewDesc>.size)
            desc.width = width
            desc.height = height
            desc.scale_factor = scaleFactor
            desc.safe_area = (safeArea.x, safeArea.y, safeArea.z, safeArea.w)
            return desc
        }
    }
}
