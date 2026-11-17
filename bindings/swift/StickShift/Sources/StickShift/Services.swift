// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import CStickShift

// The engine's services, as a hook reaches them and as the host does between
// ticks. Each view is a small value over the engine: making one costs
// nothing, and every call on it is one C call.

extension EngineContext {
    /// The world: entities and their components. Read-only in
    /// ``Game/render(_:into:)``.
    public var world: World { World(handle: handle) }

    /// The scene: streaming and the floating origin. Not in
    /// ``Game/render(_:into:)``.
    public var scene: Scene { Scene(handle: handle) }

    /// The camera every frame is drawn from.
    public var camera: EngineCamera { EngineCamera(handle: handle) }

    /// The keyboard, mouse and cursor, with this update's edges.
    public var input: Input { Input(handle: handle) }

    /// Sound.
    public var audio: Audio { Audio(handle: handle) }

    /// The graphics settings in effect, and requests for new ones.
    public var graphics: Graphics { Graphics(handle: handle) }
}

extension Engine {
    /// The world, between ticks: for tools, traces and tests.
    public var world: World { World(handle: handle) }

    /// The scene, between ticks.
    public var scene: Scene { Scene(handle: handle) }

    /// The camera, between ticks.
    public var camera: EngineCamera { EngineCamera(handle: handle) }

    /// The input: what the game reads, and presses and releases that are
    /// ``push(_:)`` calls.
    public var input: HostInput { HostInput(engine: self) }

    /// Sound, between ticks.
    public var audio: Audio { Audio(handle: handle) }

    /// The graphics settings, with what is queued and how often they
    /// changed. Between ticks only.
    public var graphics: GraphicsControl { GraphicsControl(handle: handle) }

    /// The Graphics menu for the settings in effect, as data to build the
    /// host's native menu from; `fullScreen` titles its full-screen item.
    /// Between ticks only. Get it again whenever a call returns
    /// ``HostRequests/syncMenu``.
    public func menu(fullScreen: Bool) -> GraphicsMenu {
        var out = StickShiftMenu()
        out.size = UInt32(MemoryLayout<StickShiftMenu>.size)
        handle.call("Engine.menu(fullScreen:)") { stickshift_engine_get_menu($0, fullScreen ? 1 : 0, &out) }
        return GraphicsMenu(out)
    }

    /// How many game components this engine has registered: each Swift type,
    /// once, on its first use.
    var registeredComponents: Int { core.components.registrations }

    /// `C`'s component id in this engine, registering it if it is new.
    func componentID<C: Component>(_: C.Type) -> UInt32 {
        core.components.info(C.self, raw).id.id
    }
}
