#!/usr/bin/env bash
# Regenerate the probe fixtures the Swift probe tests compare against
# (ProbeTests.swift): the lines the Rust probes print, as digests.
#
#   bindings/swift/StickShift/Tests/Fixtures/generate-probe-fixtures.sh [--lines DIR]
#
# The Rust probes are the ones the C API's own differential tests run:
# `rust_run` in crates/stickshift-capi/tests/engine_parity.rs (the world and
# streaming) and in crates/stickshift-capi/tests/engine_services.rs (the camera,
# input, audio and graphics settings, unhosted and hosted). This builds a
# throwaway binary that includes those two files as they are and prints their
# lines, so the fixtures always come from the Rust the C tests check, never
# from a copy. Run it after changing either probe, and commit the result.
#
# Each fixture holds the line count and an FNV-1a digest of every line (each
# followed by "\n"), then one digest per chunk of 1000 lines, so a failing
# Swift test names the first chunk that differs. `--lines DIR` also writes the
# Rust lines themselves to DIR, to compare with the lines the failing test
# printed.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../../../../.." && pwd)"
lines_dir=""
if [[ "${1:-}" == "--lines" ]]; then
    lines_dir="$(mkdir -p "$2" && cd "$2" && pwd)"
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/src"
# The test files open with inner doc comments, which `include!` cannot take.
for probe in engine_parity engine_services; do
    grep -v '^//!' "$root/crates/stickshift-capi/tests/$probe.rs" > "$work/src/$probe.rs"
done
cp "$root/Cargo.lock" "$work/Cargo.lock"
cat > "$work/Cargo.toml" <<EOF
[package]
name = "probe-fixtures"
version = "0.0.0"
edition = "2021"
publish = false

[workspace]

[dependencies]
stickshift-capi = { path = "$root/crates/stickshift-capi" }
stickshift-camera = { path = "$root/crates/stickshift-camera" }
stickshift-core = { path = "$root/crates/stickshift-core", default-features = false }
stickshift-ecs = { path = "$root/crates/stickshift-ecs", features = ["dynamic"] }
stickshift-math = { path = "$root/crates/stickshift-math" }
stickshift-render-api = { path = "$root/crates/stickshift-render-api" }
stickshift-scene = { path = "$root/crates/stickshift-scene" }
EOF
cat > "$work/src/main.rs" <<'EOF'
#![allow(dead_code, unused_imports)]

use std::io::Write;

mod parity {
    include!("engine_parity.rs");
    pub fn lines() -> Vec<String> {
        rust_run()
    }
}

mod services {
    include!("engine_services.rs");
    pub fn lines(hosted: bool) -> Vec<String> {
        rust_run(if hosted { Mode::Hosted } else { Mode::Unhosted }, 3000)
    }
}

const CHUNK: usize = 1000;

fn fnv(mut hash: u64, bytes: &[u8]) -> u64 {
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

fn line_digest(hash: u64, line: &str) -> u64 {
    fnv(fnv(hash, line.as_bytes()), b"\n")
}

fn write(dir: &str, name: &str, lines: &[String], raw: Option<&str>) {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    let total = lines.iter().fold(OFFSET, |h, l| line_digest(h, l));
    let mut out = format!("lines {} digest {total:016x}\n", lines.len());
    for chunk in lines.chunks(CHUNK) {
        let d = chunk.iter().fold(OFFSET, |h, l| line_digest(h, l));
        out += &format!("{d:016x}\n");
    }
    std::fs::write(format!("{dir}/probe-{name}.txt"), out).unwrap();
    if let Some(raw) = raw {
        let mut f = std::fs::File::create(format!("{raw}/probe-{name}.lines")).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = &args[1];
    let raw = args.get(2).map(String::as_str);
    write(dir, "world", &parity::lines(), raw);
    write(dir, "services-unhosted", &services::lines(false), raw);
    write(dir, "services-hosted", &services::lines(true), raw);
}
EOF

# From the workspace, so its rust-toolchain.toml picks the compiler.
cd "$root"
CARGO_TARGET_DIR="$root/target/probe-fixtures" \
    cargo run --quiet --manifest-path "$work/Cargo.toml" -- "$here" ${lines_dir:+"$lines_dir"}
echo "probe fixtures written to $here"
