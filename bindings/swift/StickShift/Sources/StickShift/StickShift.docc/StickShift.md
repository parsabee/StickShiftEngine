# ``StickShift``

Swift for Stick Shift Engine: a Metal renderer for Apple platforms, wrapped so Swift code never
touches a raw handle or a result code.

## Overview

Create a ``Context`` (or a ``StickShiftView``, which makes one for its layer), create resources
from it, and record frames:

```swift
let view = try StickShiftView(size: window.contentLayoutRect.size)
let context = view.context
let car = try await context.loadModel(contentsOf: carURL)

view.onFrame = { context, _ in
    try? context.frame { frame in
        try frame.setCamera(.perspective(eye: [0, 3, 8], target: .zero, fovY: 0.9, aspect: aspect))
        try frame.addLight(.spot(at: [0, 0.7, -2], direction: [0, -0.1, -1],
                                 intensity: 80, range: 30, innerAngle: 12, outerAngle: 22))
        try frame.draw(car)
    }
}
window.contentView = view
view.start()
```

Resources destroy themselves when you drop the last reference. Failures throw
``StickShiftError``, which carries the C API's code and message. A context is bound to the main
actor, as the C API's one-thread-at-a-time rule asks.

A model can also be drawn part by part: ``Frame/draw(_:world:local:parts:tint:)`` places each
part at `world × (local × node)` and can record only its textured or only its untextured
parts. For results that match the Rust engine bit for bit, multiply matrices with
``StickShiftMath`` rather than simd's `*`, which fuses multiplies and adds.

To run a game, implement ``Game`` and hand it to an ``Engine``, which runs the engine's own
loop on a context: ``Engine/advance(readback:)`` for one fixed step in a tool, or
``Engine/tick(_:readback:)`` from a view's display link. Each hook gets an ``EngineContext``,
valid until it returns.

The context reaches the engine's services: ``EngineContext/world`` (the engine's own ECS, with
any bitwise-copyable struct as a ``Component``, spawned and queried with parameter packs),
``EngineContext/scene`` (streaming through a ``StreamSlot``, and the floating origin),
``EngineContext/camera``, ``EngineContext/input``, ``EngineContext/audio`` and
``EngineContext/graphics``. Between ticks the ``Engine`` reaches the same services, and the
Graphics menu as data (``Engine/menu(fullScreen:)``).

## Topics

### Running a game

- ``Engine``
- ``Game``
- ``EngineContext``
- ``LoadContext``
- ``TickResult``
- ``HostRequests``
- ``EngineState``
- ``PerfSnapshot``
- ``PixelBuffer``

### The world

- ``World``
- ``Entity``
- ``Component``
- ``Transform``
- ``RenderComponent``
- ``StaticTag``
- ``DynamicTag``
- ``Query``
- ``QueryTerm``

### The scene

- ``Scene``
- ``StreamSlot``
- ``StreamingConfig``

### Camera, input, sound and graphics

- ``EngineCamera``
- ``ChaseController``
- ``Input``
- ``HostInput``
- ``Audio``
- ``Sound``
- ``Volume``
- ``AudioStats``
- ``Graphics``
- ``GraphicsControl``

### Host events

- ``HostEvent``
- ``ViewMetrics``
- ``Key``
- ``MouseButton``
- ``GraphicsMenu``
- ``GraphicsSettings``

### Rendering

- ``Context``
- ``Frame``
- ``StickShiftView``

### Resources

- ``Mesh``
- ``Texture``
- ``Model``
- ``Font``

### Describing a frame

- ``Camera``
- ``SunSky``
- ``Light``
- ``Material``
- ``ModelParts``
- ``Quad``
- ``Surface``
- ``PixelReadback``

### Math

- ``StickShiftMath``
- ``RustF32``
- ``glamDot(_:_:)``
- ``glamCross(_:_:)``
- ``glamLength(_:)``
- ``glamNormalizeOrZero(_:)``
- ``glamLerp(_:_:_:)``

### Errors and diagnostics

- ``StickShiftError``
- ``Event``
- ``Stats``
