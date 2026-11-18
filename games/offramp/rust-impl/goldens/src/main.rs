// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! Writes the golden fixtures Offramp's Swift tests compare against
//! (`games/offramp/swift-impl/Tests/OfframpTests/Fixtures`), from the playable demo's own
//! modules, compiled here as the `trace` and `record_demo` examples compile them,
//! against the headless `NullRenderer`. So they are the Rust code's results, not
//! hand-written expectations.
//!
//! ```sh
//! cargo run --release -p offramp-goldens -- games/offramp/swift-impl/Tests/OfframpTests/Fixtures
//! ```
//!
//! writes the ten files the trace example does not: eight from the demo's
//! crate-visible API (`golden`), and `probe_stdout.txt` and `smoke_state.json`
//! from the probe (`probe`), which also reads `CarRunner`'s private state. `game.rs`
//! is the one module compiled from a copy (see `build.rs`), with those accessors
//! appended; `games/offramp/rust-impl/src` is never edited. The modes run as child
//! processes of this one, because the game prints to stdout and the probe's
//! stdout is a fixture: `golden <dir>` and `probe <dir>` run one each, the probe's
//! report going to stdout.

// The game's modules, compiled as the demo binary compiles them. Some of their
// items are only used by the binary, hence the allow.
#![allow(dead_code)]

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

#[path = "../../src/asset_root.rs"]
mod asset_root;
#[path = "../../src/assets.rs"]
mod assets;
#[path = "../../src/components.rs"]
mod components;
#[path = "../../src/config.rs"]
mod config;
#[path = "../../src/hud.rs"]
mod hud;
#[path = "../../src/render.rs"]
mod render;
#[path = "../../src/rng.rs"]
mod rng;
#[path = "../../src/scenery.rs"]
mod scenery;

/// `game.rs`, from `build.rs`'s copy with the probe's accessors appended.
mod game {
    include!(concat!(env!("OUT_DIR"), "/game.rs"));
}

mod golden;
mod probe;

const USAGE: &str = "usage: offramp-goldens <dir> | golden <dir> | probe <dir>";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [dir] => write_all(Path::new(dir)),
        [mode, dir] if mode == "golden" => {
            use_demo_assets();
            golden::write(Path::new(dir));
            ExitCode::SUCCESS
        }
        [mode, dir] if mode == "probe" => {
            use_demo_assets();
            probe::run(Path::new(dir));
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// Point the demo's asset lookup at its committed assets, unless
/// `$STICKSHIFT_DEMO_ASSETS` already names some: its build-time fallback is this
/// crate's directory, which has none.
fn use_demo_assets() {
    if std::env::var_os(asset_root::ENV_VAR).is_none() {
        let assets: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../shared/assets");
        std::env::set_var(asset_root::ENV_VAR, assets);
    }
}

/// Run both modes into `dir`, each in a child process: the golden files with the
/// game's messages discarded, then the probe with its stdout in
/// `probe_stdout.txt`.
fn write_all(dir: &Path) -> ExitCode {
    if let Err(e) = fs::create_dir_all(dir) {
        eprintln!("cannot create {}: {e}", dir.display());
        return ExitCode::FAILURE;
    }
    let report = dir.join("probe_stdout.txt");
    let stdout = match File::create(&report) {
        Ok(file) => Stdio::from(file),
        Err(e) => {
            eprintln!("cannot create {}: {e}", report.display());
            return ExitCode::FAILURE;
        }
    };
    let me = std::env::current_exe().expect("the path of this executable");
    for (mode, out) in [("golden", Stdio::null()), ("probe", stdout)] {
        match Command::new(&me).arg(mode).arg(dir).stdout(out).status() {
            Ok(status) if status.success() => {}
            Ok(status) => {
                eprintln!("the {mode} run failed ({status})");
                return ExitCode::FAILURE;
            }
            Err(e) => {
                eprintln!("cannot run the {mode} mode: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    println!("wrote the golden fixtures to {}", dir.display());
    ExitCode::SUCCESS
}
