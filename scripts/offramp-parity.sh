#!/usr/bin/env bash
# Copyright (c) 2026 Parsa Bagheri
#
# This software is released under the Apache-2.0 License.
#
# Check Offramp (games/offramp), the playable demo as a Swift game on the
# engine, against the Rust playable demo it ports, by running both and comparing
# what they print and draw.
#
#   ./scripts/offramp-parity.sh [--traces] [--frames] [--release]
#
# - Traces (no GPU): the `--smoke` output and exit status of both binaries, then
#   the per-frame state trace of each headless run (smoke, autopilot, long) from
#   `playable-demo`'s `trace` example and `offramp --trace`, byte for byte.
# - Frames (Metal): every frame `record_demo` writes against the same frame from
#   `offramp --record`, by decoded pixels (scripts/compare-frames.swift). Both
#   render on this machine's GPU with shaders compiled at run time, so they must
#   match exactly.
#
# With neither --traces nor --frames, both halves run. --release builds both
# sides optimised (the Rust binaries, libstickshift and Offramp); the default is
# debug, as CI builds. The binaries are taken from Cargo's target directory
# (target/, $CARGO_TARGET_DIR or build.target-dir). Everything is written to
# target/offramp-parity (or $OFFRAMP_PARITY_DIR). Before building, it checks
# that libstickshift and the Rust demo resolve the same builds (features) of
# glam and hecs. Exits 1 if anything differs, after reporting the first
# difference of each comparison, and before comparing anything if a build fails.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

traces=0
frames=0
release=0
for arg in "$@"; do
    case "$arg" in
        --traces) traces=1 ;;
        --frames) frames=1 ;;
        --release) release=1 ;;
        *) echo "usage: $0 [--traces] [--frames] [--release]" >&2; exit 2 ;;
    esac
done
if [[ $traces == 0 && $frames == 0 ]]; then
    traces=1
    frames=1
fi

out="${OFFRAMP_PARITY_DIR:-$root/target/offramp-parity}"
mkdir -p "$out"
package="games/offramp"

# Where cargo builds: target/ unless $CARGO_TARGET_DIR or a build.target-dir
# setting moves it.
target_dir="$(cargo metadata --no-deps --format-version 1 \
    | python3 -I -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')" \
    || { echo "cargo metadata failed" >&2; exit 1; }
if [[ $release == 1 ]]; then
    cargo_profile=(--release)
    rust_bin="$target_dir/release"
    swift_config=(-c release)
else
    cargo_profile=()
    rust_bin="$target_dir/debug"
    swift_config=()
fi
# The StickShift package links $STICKSHIFT_LIB_DIR/libstickshift.a (see its
# Package.swift); point it at the library this profile builds, for the build and
# the --show-bin-path call alike, so the manifest is evaluated the same way.
export STICKSHIFT_LIB_DIR="$rust_bin"

# The build's output without the Command Line Tools' harmless search-path
# warning. Only grep's "nothing selected" status is ignored, so with pipefail a
# failed build still fails the script.
quiet_build() {
    { grep -v "search path '/Library/Developer/CommandLineTools/Developer" || true; }
}

echo "== building"
# Offramp runs the engine through libstickshift, the Rust demo through its own
# build: two cargo invocations, whose features unify apart. A glam or hecs
# feature on one side only (fast-math, scalar-math, ...) would make "the same
# Rust" compute other bits, so both must resolve the same builds of each. Both
# invocations run here, with this environment and no --target, so they share
# the target and the profile.
resolved() {
    cargo tree --quiet -p "$1" -e normal,features -f '{p} {f}' --prefix none \
        | grep -E '^(glam|hecs) v' | sed 's/ (\*)$//' | sort -u
}
demo_deps="$(resolved playable-demo)" || { echo "cargo tree -p playable-demo failed" >&2; exit 1; }
capi_deps="$(resolved stickshift-capi)" || { echo "cargo tree -p stickshift-capi failed" >&2; exit 1; }
if [[ -z "$demo_deps" || "$demo_deps" != "$capi_deps" ]]; then
    echo "the Rust demo and libstickshift build glam or hecs differently:" >&2
    diff <(echo "$demo_deps") <(echo "$capi_deps") | sed 's/^/  /' >&2 || true
    exit 1
fi
echo "glam and hecs: the same builds on both sides ($(echo "$demo_deps" | wc -l | tr -d ' ') crates)"
cargo build --quiet ${cargo_profile[@]+"${cargo_profile[@]}"} -p playable-demo \
    --bin playable-demo --example trace --example record_demo \
    || { echo "the Rust demo's build failed" >&2; exit 1; }
# The Swift package links $STICKSHIFT_LIB_DIR/libstickshift.a, which SwiftPM
# cannot build; touch a source so it relinks against the library just built.
cargo build --quiet ${cargo_profile[@]+"${cargo_profile[@]}"} -p stickshift-capi \
    || { echo "the libstickshift build failed" >&2; exit 1; }
touch "$package/Sources/OfframpGame/Config.swift"
# --quiet keeps the progress lines out, but with it SwiftPM prints only "Build
# failed"; on a failure, build again without it (only the failure is redone) to
# show the compiler's errors.
swift_build=(swift build --package-path "$package" ${swift_config[@]+"${swift_config[@]}"} --product offramp)
"${swift_build[@]}" --quiet 2>&1 | quiet_build || {
    "${swift_build[@]}" 2>&1 | quiet_build || true
    echo "the offramp build failed" >&2
    exit 1
}
swift_bin="$(swift build --package-path "$package" ${swift_config[@]+"${swift_config[@]}"} --show-bin-path)" \
    || { echo "swift build --show-bin-path failed" >&2; exit 1; }
offramp="$swift_bin/offramp"
[[ -x "$offramp" ]] || { echo "the offramp executable was not built ($offramp)" >&2; exit 1; }

failures=()

# Compare two text files line by line: report the line count, and for files that
# differ, how many lines differ and the first that does.
compare_lines() {
    local name="$1" rust="$2" swift="$3"
    local lines
    lines=$(wc -l < "$rust" | tr -d ' ')
    if cmp -s "$rust" "$swift"; then
        echo "$name: identical ($lines lines)"
        return 0
    fi
    local differing first
    # FILENAME, not NR == FNR, tells the files apart: with an empty first file,
    # NR == FNR holds in the second.
    differing=$(awk 'FILENAME == ARGV[1] { a[FNR] = $0; n = FNR; next } { if (a[FNR] != $0) d++; m = FNR }
        END { if (n > m) d += n - m; print d + 0 }' "$rust" "$swift")
    first=$(awk 'FILENAME == ARGV[1] { a[FNR] = $0; n = FNR; next } a[FNR] != $0 { print FNR; exit }
        END { }' "$rust" "$swift")
    [[ -n "$first" ]] || first=$(( $(wc -l < "$swift") + 1 ))
    echo "$name: DIFFERENT: $differing of $lines lines differ; the first is line $first"
    echo "  rust:  $(sed -n "${first}p" "$rust" | cut -c1-400)"
    echo "  swift: $(sed -n "${first}p" "$swift" | cut -c1-400)"
    failures+=("$name")
}

if [[ $traces == 1 ]]; then
    echo "== traces (headless, no GPU)"

    # --smoke: stdout, stderr and the exit status.
    set +e
    "$rust_bin/playable-demo" --smoke > "$out/rust_smoke.out" 2> "$out/rust_smoke.err"
    rust_status=$?
    "$offramp" --smoke > "$out/swift_smoke.out" 2> "$out/swift_smoke.err"
    swift_status=$?
    set -e
    if [[ $rust_status == "$swift_status" ]] && cmp -s "$out/rust_smoke.out" "$out/swift_smoke.out" \
        && cmp -s "$out/rust_smoke.err" "$out/swift_smoke.err"; then
        echo "--smoke: identical (stdout $(wc -c < "$out/rust_smoke.out" | tr -d ' ') bytes, stderr $(wc -c < "$out/rust_smoke.err" | tr -d ' ') bytes, exit $rust_status)"
    else
        echo "--smoke: DIFFERENT (exit $rust_status from Rust, $swift_status from Swift)"
        diff "$out/rust_smoke.out" "$out/swift_smoke.out" | sed 's/^/  /' || true
        diff "$out/rust_smoke.err" "$out/swift_smoke.err" | sed 's/^/  /' || true
        failures+=("--smoke")
    fi

    for mode in smoke autopilot long; do
        "$rust_bin/examples/trace" "$mode" > "$out/rust_$mode.jsonl"
        "$offramp" --trace "$mode" > "$out/swift_$mode.jsonl"
        compare_lines "trace $mode" "$out/rust_$mode.jsonl" "$out/swift_$mode.jsonl"
    done
fi

if [[ $frames == 1 ]]; then
    echo "== frames (offscreen Metal)"
    rm -rf "$out/rust-frames" "$out/swift-frames"
    "$rust_bin/examples/record_demo" "$out/rust-frames" | sed "s|$out/rust-frames|<dir>|" > "$out/rust_record.out"
    "$offramp" --record "$out/swift-frames" | sed "s|$out/swift-frames|<dir>|" > "$out/swift_record.out"
    echo "record_demo: $(cat "$out/rust_record.out")"
    echo "offramp --record: $(cat "$out/swift_record.out")"
    if ! cmp -s "$out/rust_record.out" "$out/swift_record.out"; then
        echo "recorder output: DIFFERENT"
        failures+=("recorder output")
    fi
    swiftc -O scripts/compare-frames.swift -o "$out/compare-frames" 2>&1 \
        | quiet_build || { echo "compare-frames did not compile" >&2; exit 1; }
    if ! "$out/compare-frames" "$out/rust-frames" "$out/swift-frames"; then
        failures+=("frames")
    fi
fi

if [[ ${#failures[@]} -gt 0 ]]; then
    echo "Offramp parity: FAILED (${failures[*]}); the runs are in $out"
    exit 1
fi
echo "Offramp parity: identical to the Rust demo"
