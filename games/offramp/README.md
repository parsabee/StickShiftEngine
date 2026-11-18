# Offramp

Offramp is Stick Shift Engine's playable demo: an endless three-lane freeway runner. It is built
twice, once in each of the engine's two APIs, and the two builds play the same game frame for
frame.

| Folder | What it is |
|---|---|
| [`rust-impl`](rust-impl) | The game in Rust, over the engine crates (the `playable-demo` crate), with its trace and recorder examples and the `playable-demo-goldens` crate |
| [`swift-impl`](swift-impl) | The game in Swift, over the C API through the [`StickShift` Swift package](../../bindings/swift/StickShift) (a SwiftPM package) |
| [`shared`](shared) | What both load at run time: the models, textures, sounds and font in `shared/assets` |

```sh
cargo run -p playable-demo                                  # the Rust build, windowed
cargo build -p stickshift-capi                              # SwiftPM cannot run cargo
swift run --package-path games/offramp/swift-impl offramp   # the Swift build, windowed
./scripts/offramp-parity.sh                                 # the two, compared frame by frame
```

[`scripts/offramp-parity.sh`](../../scripts/offramp-parity.sh) checks that the Swift build's
`--smoke` output and its smoke, autopilot and long traces are byte-identical to the Rust
build's, and that the 80 recorded frames are pixel-identical. Each folder's README says more.
