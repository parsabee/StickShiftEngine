// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import StickShift

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
    /// The current graphics settings' render scale, as its C value.
    var renderScaleCode: UInt32 {
        graphics.current.renderScale.rawValue
    }
}
