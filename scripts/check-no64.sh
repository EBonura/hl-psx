#!/bin/sh
# Fail when guest source names a 64/128-bit integer or a float type: the SDK's own scanner
# (psoxide-dev no64-scan) on the game's sources. Wide arithmetic is psx_math::Wide, enforced in
# the SDK; there is no exempt file. Run from anywhere, after the SDK is hydrated into .psoxide.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
psoxide=${PSOXIDE:-$root/.psoxide}
cargo run --locked --manifest-path "$psoxide/tools/psoxide-dev/Cargo.toml" --release -- no64-scan "$root/game/src"
