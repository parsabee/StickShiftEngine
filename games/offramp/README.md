# Offramp

Offramp is Stick Shift Engine's playable demo and reference game: an endless three-lane freeway
runner that uses nothing but the engine's public API.

| Folder | What it is |
|---|---|
| [`rust-impl`](rust-impl) | The game, the `offramp` crate, with its asset bakers and its recorder |
| [`shared`](shared) | What the game loads at run time: the models, textures, sounds and font in `shared/assets` |

```sh
cargo run -p offramp              # windowed, Metal (macOS)
cargo run -p offramp -- --smoke   # headless oracle: 120 frames, prints "smoke: 120 frames OK"
```

[`rust-impl/README.md`](rust-impl/README.md) has the controls, the code layout, the asset
bakers and the asset licences.
