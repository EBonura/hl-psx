#!/bin/sh
# Fail when guest source names a 64/128-bit integer or a float type (the rules
# and the allow marker are in scripts/no64_guard.rs). game/src/wide.rs is the one
# file whose job is wide arithmetic. Run from anywhere.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
out="${TMPDIR:-/tmp}/no64_guard.$$"
trap 'rm -f "$out"' EXIT
rustc -O --edition 2021 "$root/scripts/no64_guard.rs" -o "$out"
"$out" --exempt "$root/game/src/wide.rs" "$root/game/src"
