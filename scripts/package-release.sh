#!/usr/bin/env bash
# Copyright (c) 2026 Parsa Bagheri
#
# This software is released under the Apache-2.0 License.
#
# Package a prebuilt Apple Silicon release of Stick Shift Engine.
#
#   scripts/package-release.sh <version> <outdir>
#
# <version> is the workspace version without the leading `v` (e.g. 0.1.0-alpha.2).
# Expects the C library (`cargo build --release -p stickshift-capi --locked`) in
# $STICKSHIFT_LIB_DIR (default: target/release), built with the Cargo features in $STICKSHIFT_FEATURES (the release
# workflow sets `precompiled-shaders`). Produces
#
#   <outdir>/stickshift-engine-<version>-aarch64-apple-darwin.tar.gz
#   <outdir>/stickshift-engine-<version>-aarch64-apple-darwin.tar.gz.sha256
#
# laid out for Homebrew:
#
#   stickshift-engine-<version>-aarch64-apple-darwin/
#     lib/libstickshift.a  lib/libstickshift.dylib the C library (static, and dynamic with an
#                                        @rpath install name)
#     lib/pkgconfig/stickshift.pc             pkg-config file (relocatable)
#     include/stickshift/stickshift.h              the C header
#     share/stickshift-engine/src/            the engine source (tracked files only)
#     LICENSE  NOTICE  README.md
#
# Before tarring it checks the libraries are arm64 and the dylib's install name. After
# tarring it unpacks a copy in a temp dir and, from there, builds and runs the C smoke test
# against the static library and the C sample against the dynamic one.
# Used by .github/workflows/release.yml; runnable locally.
set -euo pipefail

if [[ $# -ne 2 ]]; then
    echo "usage: $0 <version> <outdir>" >&2
    exit 2
fi
version=$1
outdir=$2
repo=$(cd "$(dirname "$0")/.." && pwd)
libdir=${STICKSHIFT_LIB_DIR:-$repo/target/release}
name="stickshift-engine-${version}-aarch64-apple-darwin"
tarball="${name}.tar.gz"

# The version must match the workspace's, or the tarball would lie about its contents.
ws_version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$repo/Cargo.toml" | head -n1)
if [[ "$version" != "$ws_version" ]]; then
    echo "error: version $version does not match the workspace version $ws_version" >&2
    exit 1
fi

for artefact in "$libdir/libstickshift.a" "$libdir/libstickshift.dylib"; do
    [[ -f "$artefact" ]] || { echo "error: $artefact is missing (cargo build --release -p stickshift-capi --locked)" >&2; exit 1; }
    archs=$(lipo -archs "$artefact")
    if [[ "$archs" != "arm64" ]]; then
        echo "error: $artefact is '$archs', expected arm64 only" >&2
        exit 1
    fi
    file "$artefact"
done
install_name=$(otool -D "$libdir/libstickshift.dylib" | tail -n 1)
if [[ "$install_name" != "@rpath/libstickshift.dylib" ]]; then
    echo "error: libstickshift.dylib's install name is '$install_name', expected @rpath/libstickshift.dylib" >&2
    exit 1
fi

# The native libraries and frameworks a program linking libstickshift.a statically needs, as
# rustc reports them (touch the crate root so rustc runs and prints them).
if [[ -z "${STICKSHIFT_NATIVE_LIBS:-}" ]]; then
    touch "$repo/crates/stickshift-capi/src/lib.rs"
    # Same features as the build that produced the library: this rebuilds libstickshift.a.
    STICKSHIFT_NATIVE_LIBS=$(cd "$repo" && cargo rustc --quiet --release --locked -p stickshift-capi --lib \
        --features "${STICKSHIFT_FEATURES:-}" --crate-type staticlib -- --print native-static-libs 2>&1 \
        | sed -n 's/.*native-static-libs: //p' | tail -n 1)
fi
[[ -n "$STICKSHIFT_NATIVE_LIBS" ]] || { echo "error: could not determine the native link libraries" >&2; exit 1; }

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
stage="$work/$name"
mkdir -p "$stage/lib/pkgconfig" "$stage/include/stickshift" "$stage/share/stickshift-engine/src"

# The C library, header and pkg-config file.
cp "$libdir/libstickshift.a" "$libdir/libstickshift.dylib" "$stage/lib/"
cp "$repo/crates/stickshift-capi/include/stickshift.h" "$stage/include/stickshift/"
cat > "$stage/lib/pkgconfig/stickshift.pc" <<PC
prefix=\${pcfiledir}/../..
libdir=\${prefix}/lib
includedir=\${prefix}/include

Name: stickshift
Description: Stick Shift Engine C API: a Metal graphics engine for Apple Silicon
Version: $version
Cflags: -I\${includedir}/stickshift
Libs: -L\${libdir} -lstickshift
Libs.private: $STICKSHIFT_NATIVE_LIBS
PC

# Engine source for developers (Cargo path dependencies): tracked files only, so
# target/, .git and local clutter never ship.
src_paths=(Cargo.toml Cargo.lock rust-toolchain.toml crates examples LICENSE NOTICE README.md CHANGELOG.md docs)
(cd "$repo" && git ls-files -z -- "${src_paths[@]}" | tar --null -T - -cf -) \
    | tar -xf - -C "$stage/share/stickshift-engine/src"

cp "$repo/LICENSE" "$repo/NOTICE" "$repo/README.md" "$stage/"

mkdir -p "$outdir"
outdir=$(cd "$outdir" && pwd)
# Deterministic-ish archive: no macOS resource forks / xattrs.
COPYFILE_DISABLE=1 tar --no-xattrs -czf "$outdir/$tarball" -C "$work" "$name"
(cd "$outdir" && shasum -a 256 "$tarball" > "$tarball.sha256")

# Verify the packaged artefact, not the staging dir: unpack it into a fresh temp dir.
check="$work/check"
mkdir -p "$check"
tar -xzf "$outdir/$tarball" -C "$check"

# Build the C programs against the unpacked copy, not the build tree: the smoke test
# links the static library with the pkg-config file's private libraries, the sample
# links the dynamic library through an rpath. Both render through Metal.
packaged="$check/$name"
private_libs=$(sed -n 's/^Libs.private: //p' "$packaged/lib/pkgconfig/stickshift.pc")
cflags=(-std=c11 -Wall -Wextra -pedantic -Werror -I "$packaged/include/stickshift")
fixture="$repo/crates/stickshift-assets/tests/fixtures/cube.gltf"
# shellcheck disable=SC2086 # the library list is meant to split into words
cc "${cflags[@]}" "$repo/crates/stickshift-capi/tests/c/smoke.c" "$packaged/lib/libstickshift.a" \
    $private_libs -o "$work/c-smoke"
"$work/c-smoke" "$fixture"
cc "${cflags[@]}" "$repo/examples/c/offscreen.c" -L "$packaged/lib" -lstickshift \
    -Wl,-rpath,"$packaged/lib" -o "$work/c-offscreen"
"$work/c-offscreen" "$fixture" "$work/offscreen.ppm"
[[ -s "$work/offscreen.ppm" ]] || { echo "error: the C sample wrote no image" >&2; exit 1; }

echo "packaged $outdir/$tarball ($(du -h "$outdir/$tarball" | cut -f1))"
cat "$outdir/$tarball.sha256"
