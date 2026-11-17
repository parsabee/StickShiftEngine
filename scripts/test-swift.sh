#!/usr/bin/env bash
# Build libstickshift with cargo, then build and test the Swift package
# (bindings/swift/StickShift) against it, and build the example apps (examples/swift).
# SwiftPM cannot run cargo itself.
#
#   ./scripts/test-swift.sh
#
# STICKSHIFT_FEATURES (e.g. `precompiled-shaders`) builds the library the way a release does.
# Set STICKSHIFT_REQUIRE_GPU=1 to fail, rather than skip, the Metal tests without a device.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
package="bindings/swift/StickShift"

cargo build --quiet -p stickshift-capi --features "${STICKSHIFT_FEATURES:-}"

# The link-line gate: Package.swift must link exactly what libstickshift.a needs, as rustc
# reports it, less the libraries every link gets. Touch the crate root so rustc runs (and
# prints them) even when the library is up to date.
touch crates/stickshift-capi/src/lib.rs
native_libs="$(cargo rustc --quiet -p stickshift-capi --lib --features "${STICKSHIFT_FEATURES:-}" \
    --crate-type staticlib -- \
    --print native-static-libs 2>&1 | sed -n 's/.*native-static-libs: //p' | tail -n 1)"
python3 -I - "$package/Package.swift" "$native_libs" <<'PY'
import re
import sys

package, native = open(sys.argv[1]).read(), sys.argv[2].split()
implicit = {"System", "c", "m"}
want_frameworks = {native[i + 1] for i, word in enumerate(native) if word == "-framework"}
want_libraries = {word[2:] for word in native if word.startswith("-l")} - implicit
have_frameworks = set(re.findall(r'\.linkedFramework\("([^"]+)"\)', package))
have_libraries = set(re.findall(r'\.linkedLibrary\("([^"]+)"\)', package))
problems = []
for kind, want, have in [
    ("framework", want_frameworks, have_frameworks),
    ("library", want_libraries, have_libraries),
]:
    for name in sorted(want - have):
        problems.append(f"Package.swift does not link the {kind} {name}, which libstickshift.a needs")
    for name in sorted(have - want):
        problems.append(f"Package.swift links the {kind} {name}, which libstickshift.a does not need")
if problems:
    print("\n".join(problems), file=sys.stderr)
    sys.exit(1)
print(f"link line: Package.swift matches libstickshift.a ({len(want_frameworks)} frameworks, "
      f"{len(want_libraries)} libraries)")
PY
# Link the library just built, wherever cargo builds: target/ unless
# $CARGO_TARGET_DIR or a build.target-dir setting moves it.
target_dir="$(cargo metadata --no-deps --format-version 1 \
    | python3 -I -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')"
export STICKSHIFT_LIB_DIR="$target_dir/debug"
[[ -f "$STICKSHIFT_LIB_DIR/libstickshift.a" ]] || { echo "$STICKSHIFT_LIB_DIR/libstickshift.a was not built" >&2; exit 1; }

# SwiftPM does not notice that libstickshift.a changed; touch a source so the
# package relinks against the library just built.
touch "$package/Sources/StickShift/Context.swift"

# Name the swift-testing macro plugin explicitly: with only the Command Line Tools,
# SwiftPM's build intermittently fails to find it ("plugin for module
# 'TestingMacros' not found"). Harmless where it is found anyway.
swift_flags=()
plugins="$(dirname "$(dirname "$(xcrun --find swift)")")/lib/swift/host/plugins/testing"
if [[ -d "$plugins" ]]; then
    swift_flags=(-Xswiftc -plugin-path -Xswiftc "$plugins")
fi
# The package builds in Swift 6 mode (its tools version), so with complete concurrency
# checking; any warning, a concurrency one included, fails the gate.
swift_flags+=(-Xswiftc -warnings-as-errors)
swift test --package-path "$package" ${swift_flags[@]+"${swift_flags[@]}"}
echo "Swift package: build and tests passed"

# The example apps build against the package, under the same rules.
for example in examples/swift/*/; do
    swift build --package-path "$example" ${swift_flags[@]+"${swift_flags[@]}"}
    echo "Swift example $(basename "$example"): built"
done
