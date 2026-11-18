// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

/// What a frame is drawn onto: the drawable's size, its pixels per point and
/// its safe area. A HUD lays itself out from it.
public struct Surface: Sendable, Equatable {
    /// The drawable's width, in pixels.
    public var width: Int
    /// The drawable's height, in pixels.
    public var height: Int
    /// Drawable pixels per point: the UI scale.
    public var scale: Float
    /// The safe-area insets in drawable pixels: top, right, bottom, left.
    public var safeArea: SIMD4<Float>

    /// Creates a surface.
    public init(width: Int, height: Int, scale: Float, safeArea: SIMD4<Float> = .zero) {
        self.width = width
        self.height = height
        self.scale = scale
        self.safeArea = safeArea
    }
}

extension Context {
    /// The drawable's size, scale and safe area now.
    public var surface: Surface {
        let (width, height) = size
        return Surface(width: width, height: height, scale: scale, safeArea: safeArea)
    }
}

extension Frame {
    /// What this frame is drawn onto: its context's ``Context/surface``.
    public var surface: Surface {
        context.surface
    }
}
