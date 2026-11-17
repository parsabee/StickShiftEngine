# StickShift (Swift)

A Swift package over Stick Shift Engine's C API (`libstickshift`). Swift code gets classes that own
the engine's resources, errors it can `throw` and `catch`, `async` model loading, `simd` types,
and an `NSView` that hosts the renderer. It never sees a raw handle or a result code.

Offramp, the Swift reference game, is its first user. Requires macOS 14 and Swift 6.

## Building

SwiftPM cannot run cargo, so the library is built first. From the workspace root:

```sh
./scripts/test-swift.sh      # cargo build -p stickshift-capi, then swift test
```

or by hand:

```sh
cargo build -p stickshift-capi                     # target/debug/libstickshift.a
swift build --package-path bindings/swift/StickShift
```

The package links `libstickshift.a` statically from `STICKSHIFT_LIB_DIR`, which defaults to the
workspace's `target/debug`. Point it at a release build for a release app:

```sh
cargo build --release -p stickshift-capi --features precompiled-shaders
STICKSHIFT_LIB_DIR=$PWD/target/release swift build -c release --package-path bindings/swift/StickShift
```

To use it from another package or app, depend on it by path:

```swift
.package(path: "../StickShiftEngine/bindings/swift/StickShift")
```

When iOS support lands (v0.9), the same package will consume the engine as an XCFramework.

## Using it

```swift
import StickShift

let view = try StickShiftView(size: window.contentLayoutRect.size)
let context = view.context
let car = try await context.loadModel(contentsOf: carURL)

view.onFrame = { context, _ in
    try? context.frame { frame in
        try frame.setCamera(.perspective(eye: [0, 3, 8], target: .zero, fovY: 0.9, aspect: 16 / 9))
        try frame.setSunSky(.default)
        try frame.addLight(.point(at: [4, 6, -10], color: [1, 0.8, 0.6], intensity: 40, range: 15))
        try frame.draw(car, transform: .init(translation: [0, 0, -4]))
    }
}
window.contentView = view
view.start()
```

- **`Context`** owns the GPU device and everything created from it. Make one for a layer
  (`StickShiftView` does this), offscreen (`Context(offscreenWidth:height:)`, whose frames you
  can read back), or on the null backend (`backend: .null`) for tests on machines without a
  GPU.
- **Resources** (`Mesh`, `Texture`, `Model`, `Font`) keep their context alive and destroy
  themselves when you drop the last reference. A `Font` keeps its atlas texture alive too.
- **Errors** are `StickShiftError`: the C API's code (`.invalidArgument`, `.limitExceeded`,
  ...) and the engine's message.
- **Frames** are closure-scoped. `context.frame { frame in ... }` opens a frame, records into it
  and submits it when the closure returns, or discards it if the closure throws. It returns the
  frame's number, or `nil` when there was no frame to record: three frames already on the GPU,
  or a zero-sized view. That is not an error; skip that tick. A `Frame` used after its closure
  throws `.wrongState`.
- **The camera and the sun are sticky** across frames. **Lights last one frame**: add them every
  frame. `Material(emissive:)` makes a surface glow without lighting anything else.
- **Models** draw whole with `frame.draw(model, transform:)`, or with
  `frame.draw(model, world:local:parts:tint:)`, which places each part at
  `world × (local × node)` and can draw only the `.textured` or only the `.untextured` parts. A
  game that fits a model to its world passes the fit as `local`, and can draw every textured
  surface first and the untextured ones after its other draws.
- **Model loads are `async`.** The engine parses on its own threads and uploads during
  `pollEvents()`. While a load is pending the context polls by itself, so `await` works without
  a render loop. `StickShiftView` polls every frame anyway.
- **Read-back:** `frame.readPixels()` on an offscreen context returns a `PixelReadback`. Use
  `try await readback.pixels()` to get BGRA8 bytes once the GPU has finished.
- **Math** is plain `simd`: `simd_float4x4`, `SIMD3<Float>`, `simd_quatf`, with
  `.perspective(fovY:aspect:near:far:)`, `.lookAt(eye:target:up:)` and
  `init(translation:rotation:scale:)` matching the engine's conventions (right-handed, +Y up,
  column-major, depth `0...1`). simd's `*` on two matrices fuses multiplies and adds, so its
  products can differ in the last bit from the engine's. `StickShiftMath.multiply(_:_:)` and
  `StickShiftMath.rotation(axis:angle:)` call the engine's own math instead, for results that
  match a Rust game bit for bit; `Camera.perspective` multiplies with it.

### Threading

The C API lets one thread at a time use a context, so `Context`, its resources and `Frame` are
`@MainActor`. Create and use them on the main actor. `StickShiftView`'s display link calls
`onFrame` there.

### The host view

`StickShiftView` is an `NSView` backed by a `CAMetalLayer`. It owns the context that renders into
the layer, keeps the drawable sized to the view in pixels (on resize and when the window moves to
a screen with another backing scale), and runs a `CADisplayLink` while it is in a window and
started. Each refresh calls `onFrame` and then `pollEvents()`.

- **Render scale.** `renderScale` (default 1, or `StickShiftView(size:renderScale:)`) renders
  below the view's native resolution: each side's native pixels, `round(points × backing)`,
  times `renderScale`, rounded and at least 1. Core Animation scales the frame up to fill the
  view. `context.scale`, the HUD's pixels per point, becomes `backing × renderScale`.
- **Keyboard.** Set `onKey(keyCode, isDown, isRepeat)` and the view takes the keyboard: it
  becomes its window's first responder (at once, and whenever it joins a window) and hands it
  each key-down and key-up with the virtual key code (`NSEvent.keyCode`, a physical key
  position). It then passes no key event on, so AppKit does not beep; menu key equivalents
  such as ⌘Q still work. Without a handler (the default) the view does not take focus and key
  events go on up the responder chain. A key released while Command is held
  arrives too, so no key is left down: `NSApplication` never hands a window such a key-up, so
  while the view is in a window and its first responder, a local event monitor sends it to the
  window itself (winit overrides `sendEvent:` for the same reason).

## Tests

`./scripts/test-swift.sh` runs the package's Swift Testing suite:

- **Null backend:** contexts, resources and their lifetimes, errors, frames (including a
  discarded one and a frame used too late), lights and their limit, emission, settings, fonts
  and quads, `async` model loads (from a file, from memory, failing), and model draws that
  select their textured or untextured parts.
- **Math:** `StickShiftMath.multiply` matches the engine's unfused product bit for bit,
  `StickShiftMath.rotation` matches its quaternions (signed zeros included), and
  `Camera.perspective` multiplies with it.
- **Metal:** offscreen rendering with read-back, lights and emission changing the pixels, a model
  drawn at `world × local` (and tinted, and by part), and the host view: sizing its context at
  a render scale, its UI scale, and taking the keyboard, a key-up with Command held included.

The Metal tests skip without a device; CI sets `STICKSHIFT_REQUIRE_GPU=1` so a missing device
fails them instead.

## Layout

| Path | |
|---|---|
| `Package.swift` | The package: the `CStickShift` C module and the `StickShift` Swift target |
| `Sources/CStickShift/module.modulemap` | The C API as a Clang module, over `crates/stickshift-capi/include/stickshift.h` |
| `Sources/StickShift/` | The Swift API |
| `Tests/StickShiftTests/` | The tests |

Part of the [Stick Shift Engine](../../../README.md) workspace. Apache-2.0.
