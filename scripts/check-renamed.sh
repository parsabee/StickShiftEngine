#!/usr/bin/env bash
# Fail if the project's previous name comes back anywhere in the repository.
#
# The project was renamed to Stick Shift Engine in v0.1.0-beta.2, and the previous name appears
# nowhere in the repository, not even here: the script spells it from character codes so it
# never matches itself.
#
# Run from anywhere: ./scripts/check-renamed.sh (CI runs it on every push and PR).
set -euo pipefail

cd "$(dirname "$0")/.."
old="$(printf '\x6b\x61\x6d\x61\x6e')"

hits="$(git grep -n -i -I "$old" -- . || true)"

if [[ -n "$hits" ]]; then
    echo "The previous project name is still used here; rename it to Stick Shift Engine / stickshift:" >&2
    echo "$hits" >&2
    exit 1
fi
echo "no pre-rename names left"
