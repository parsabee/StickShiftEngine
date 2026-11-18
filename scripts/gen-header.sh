#!/usr/bin/env bash
# Regenerate crates/stickshift-capi/include/stickshift.h from the Rust sources with cbindgen.
#
#   ./scripts/gen-header.sh           write the header
#   ./scripts/gen-header.sh --check   fail if the committed header is out of date (CI)
#
# Needs cbindgen on PATH at the pinned version:
#   cargo install cbindgen --version 0.29.0 --locked
set -euo pipefail

CBINDGEN_VERSION="0.29.0"
root="$(cd "$(dirname "$0")/.." && pwd)"
crate="$root/crates/stickshift-capi"
header="$crate/include/stickshift.h"

if ! command -v cbindgen >/dev/null; then
    echo "cbindgen not found; install it with: cargo install cbindgen --version $CBINDGEN_VERSION --locked" >&2
    exit 1
fi
found="$(cbindgen --version | awk '{print $2}')"
if [[ "$found" != "$CBINDGEN_VERSION" ]]; then
    echo "cbindgen $found found, but the header is generated with $CBINDGEN_VERSION" >&2
    exit 1
fi

generated="$(mktemp)"
trap 'rm -f "$generated"' EXIT
cbindgen --quiet --config "$crate/cbindgen.toml" --crate stickshift-capi --output "$generated" "$crate"
# Parenthesise negative constants (integer and decimal) so `x - STICKSHIFT_ERROR_BUSY`
# stays one expression.
sed -E -i.bak 's/^(#define [A-Z0-9_]+) (-[0-9]+(\.[0-9]+)?)$/\1 (\2)/' "$generated" && rm -f "$generated.bak"
# cbindgen drops a constant's type, and an unsuffixed decimal is a double in C and in Swift,
# which then cannot pass it as a float: give each `f32` constant its `f` suffix.
for name in $(grep -rhoE 'pub const [A-Z0-9_]+: f32' "$crate/src" | awk '{print $3}' | tr -d ':'); do
    sed -E -i.bak "s/^(#define $name) (\(?-?[0-9]+\.[0-9]+)(\)?)$/\1 \2f\3/" "$generated"
    rm -f "$generated.bak"
    if ! grep -qE "^#define $name \(?-?[0-9]+\.[0-9]+f\)?$" "$generated"; then
        echo "gen-header.sh: could not give the f32 constant $name its f suffix" >&2
        exit 1
    fi
done

if [[ "${1:-}" == "--check" ]]; then
    if ! diff -u "$header" "$generated"; then
        echo "crates/stickshift-capi/include/stickshift.h is out of date: run ./scripts/gen-header.sh" >&2
        exit 1
    fi
    echo "stickshift.h is up to date"
else
    mkdir -p "$(dirname "$header")"
    cp "$generated" "$header"
    echo "wrote $header"
fi
