// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! Helpers shared by the GPU integration tests: what a test does when there is
//! no Metal device, and how golden images are hashed, checked and re-recorded.

// Each test binary compiles this module and uses only part of it.
#![allow(dead_code)]

use std::io::Write;

/// Report that the calling test cannot run because there is no Metal device.
///
/// The notice goes straight to the stderr handle, past the test harness's output
/// capture, so a plain `cargo test` shows which tests did not run. With
/// `STICKSHIFT_REQUIRE_GPU=1` (CI sets it) a missing device **fails** the test
/// instead: a runner that lost its GPU must not turn the render suite green with
/// nothing checked.
pub fn no_gpu() {
    let test = std::thread::current()
        .name()
        .unwrap_or("a GPU test")
        .to_owned();
    assert!(
        !env_flag("STICKSHIFT_REQUIRE_GPU"),
        "{test}: no Metal device, and STICKSHIFT_REQUIRE_GPU is set"
    );
    let _ = writeln!(
        std::io::stderr(),
        "SKIPPED {test}: no Metal device (set STICKSHIFT_REQUIRE_GPU=1 to fail instead)"
    );
}

/// Whether `BLESS` asks to re-record the golden images. Only `1` or `true` do,
/// so `BLESS=0` (or an empty value) still checks them.
pub fn blessing() -> bool {
    env_flag("BLESS")
}

fn env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

/// FNV-1a 64-bit hash over a byte buffer. Self-contained (no external crate) so
/// the golden hash has no dependency surface.
pub fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x00000100000001b3);
    }
    hash
}

/// Hash `pixels` and either print a fresh baseline (`BLESS=1`) or assert it
/// matches `baseline`.
///
/// `constant` is the name of the `const` that holds `baseline`, so a `BLESS=1`
/// run that exercises several references says which constant each printed value
/// belongs in, and a failure says which one to look at. Shared by every golden
/// test so the blessing contract has exactly one implementation; the messages
/// name the calling test file, where the constant lives.
#[track_caller]
pub fn bless_or_assert(pixels: &[u8], baseline: u64, constant: &str) {
    let file = std::panic::Location::caller().file();
    let test = std::path::Path::new(file)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("pixel_hash");
    // A pixel hash only guards anything if the frame actually has content. A
    // regression that drew *nothing* would produce a perfectly stable hash, and
    // once blessed it would pass forever while testing nothing at all. Every
    // reference puts varied geometry over a gradient sky, so a frame this uniform
    // means the render broke — checked before blessing too, so a blank frame can
    // never be baked into a baseline.
    let distinct = pixels
        .chunks_exact(4)
        .collect::<std::collections::HashSet<_>>();
    assert!(
        distinct.len() > 8,
        "{constant}: the rendered frame has only {} distinct pixel value(s) — it is \
         effectively blank, so its hash would guard nothing",
        distinct.len(),
    );

    let hash = fnv1a_64(pixels);

    if blessing() {
        println!("BLESS: new {constant} = {hash:#018x}");
        println!("Update {constant} in {file} with a justification note.");
        return;
    }

    assert_eq!(
        hash, baseline,
        "{constant} pixel hash changed ({hash:#018x} != {baseline:#018x}). \
         If this is an intended visual change, re-bless with \
         `BLESS=1 cargo test -p stickshift-render --test {test}` and update {constant} \
         with a justification note."
    );
}

/// Whether `device_name` is a paravirtualized Metal device (a macOS virtual
/// machine, e.g. a GitHub Actions runner) rather than a real Apple GPU.
pub fn is_paravirtual(device_name: &str) -> bool {
    device_name.contains("Paravirtual")
}

/// Print `frame` base64-encoded on one line between `BEGIN <name>` / `END <name>`
/// markers, so a failing CI run carries the frame needed to inspect it or re-record a
/// reference image under `tests/data/` (decode the line into that file).
pub fn dump_frame(name: &str, frame: &[u8]) {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(frame.len().div_ceil(3) * 4);
    for chunk in frame.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                encoded.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                encoded.push('=');
            }
        }
    }
    eprintln!("BEGIN {name}\n{encoded}\nEND {name}");
}
