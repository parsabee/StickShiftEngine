#!/usr/bin/env bash
# Build libstickshift with cargo, then build and test the Swift package
# (bindings/swift/StickShift) against it. SwiftPM cannot run cargo itself.
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
swift test --package-path "$package" ${swift_flags[@]+"${swift_flags[@]}"}
echo "Swift package: build and tests passed"
