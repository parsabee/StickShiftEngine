// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import CStickShift

/// The player-facing graphics settings: what the Graphics menu chooses, and
/// what ``Game/graphicsSettingsChanged(_:_:)`` receives.
public struct GraphicsSettings: Hashable, Sendable {
    /// How far shadows reach.
    public enum ShadowDistance: UInt32, Hashable, Sendable, CaseIterable {
        /// As far as the draw distance (the default).
        case matchDrawDistance = 0
        /// Part of the way.
        case medium = 1
        /// Only nearby.
        case near = 2
    }

    /// How far the world is drawn before the fog closes in.
    public enum DrawDistance: UInt32, Hashable, Sendable, CaseIterable {
        /// The nearest.
        case near = 0
        /// In between.
        case medium = 1
        /// The full distance (the default).
        case far = 2
    }

    /// The resolution the 3D scene renders at.
    public enum RenderScale: UInt32, Hashable, Sendable, CaseIterable {
        /// Half the view's points per side, times its pixels per point (the default).
        case half = 0
        /// Three quarters.
        case threeQuarters = 1
        /// The view's full physical resolution.
        case full = 2
    }

    /// The shadow tier.
    public var shadows: RenderSettings.ShadowQuality
    /// How far shadows reach.
    public var shadowDistance: ShadowDistance
    /// How far the world is drawn.
    public var drawDistance: DrawDistance
    /// The 3D scene's resolution.
    public var renderScale: RenderScale

    /// Creates settings; each defaults to the engine's default.
    public init(
        shadows: RenderSettings.ShadowQuality = .high,
        shadowDistance: ShadowDistance = .matchDrawDistance,
        drawDistance: DrawDistance = .far,
        renderScale: RenderScale = .half
    ) {
        self.shadows = shadows
        self.shadowDistance = shadowDistance
        self.drawDistance = drawDistance
        self.renderScale = renderScale
    }

    /// The engine's defaults: high shadows as far as the draw distance, the
    /// far draw distance, and half resolution.
    public static let `default` = GraphicsSettings()

    init(_ c: StickShiftGraphicsSettings) {
        shadows = RenderSettings.ShadowQuality(rawValue: c.shadows) ?? .high
        shadowDistance = ShadowDistance(rawValue: c.shadow_distance) ?? .matchDrawDistance
        drawDistance = DrawDistance(rawValue: c.draw_distance) ?? .far
        renderScale = RenderScale(rawValue: c.render_scale) ?? .half
    }

    var c: StickShiftGraphicsSettings {
        var c = StickShiftGraphicsSettings()
        c.shadows = shadows.rawValue
        c.shadow_distance = shadowDistance.rawValue
        c.draw_distance = drawDistance.rawValue
        c.render_scale = renderScale.rawValue
        return c
    }
}
