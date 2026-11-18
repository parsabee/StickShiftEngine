// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! Writes `$OUT_DIR/game.rs`: the playable demo's `src/game.rs`, unchanged but for
//! two things, with `game_accessors.rs` appended. The probe reads `CarRunner`'s
//! private state through those accessors, and only code inside the module can.
//!
//! - The copy is compiled with `include!` inside `mod game { ... }`, which takes no
//!   inner doc comments, so its `//!` lines become plain `//` comments.
//! - Nothing else changes, and `games/offramp/rust-impl/src` is never written to.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
    let game = manifest.join("../src/game.rs");
    let accessors = manifest.join("game_accessors.rs");
    println!("cargo:rerun-if-changed={}", game.display());
    println!("cargo:rerun-if-changed={}", accessors.display());

    let source =
        fs::read_to_string(&game).unwrap_or_else(|e| panic!("cannot read {}: {e}", game.display()));
    let mut copy = String::with_capacity(source.len() + 2048);
    for line in source.lines() {
        match line.strip_prefix("//!") {
            Some(rest) => {
                copy.push_str("//");
                copy.push_str(rest);
            }
            None => copy.push_str(line),
        }
        copy.push('\n');
    }
    copy.push('\n');
    copy.push_str(
        &fs::read_to_string(&accessors)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", accessors.display())),
    );

    let out = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("game.rs");
    fs::write(&out, copy).unwrap_or_else(|e| panic!("cannot write {}: {e}", out.display()));
}
