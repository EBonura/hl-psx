#!/usr/bin/env bash
# A few statics the skill and settings journeys read are split statics: the linker map prints them as
# `hl_psx::settings::SKILL (.0)` or `hl_psx::SIM_NOW (.0)`, which the gate's own symbol
# loader cannot read, so tests/skill-*.toml and tests/chain-skill-hard.toml bind them as fixed
# addresses in an [addr] table, like tests/chain-new-game.toml. An address is only valid for
# the build whose map it came from: run this after every build that changed the executable,
# before running those journeys.
#
#   scripts/bind-skill-addr.sh [MAP]     (default .hlpsx/hl-psx.map)
set -euo pipefail
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"
map=${1:-.hlpsx/hl-psx.map}
for name in SKILL SIM_NOW WORLD_ARENA MOVE_DEADZONE LOOK_DEADZONE BRIGHTNESS; do
    addr=$(awk -v a="hl_psx::$name" -v b="hl_psx::settings::$name" \
        '($5 == a || $5 == b) && ($6 == "" || $6 == "(.0)") { print $1; exit }' "$map")
    [ -n "$addr" ] || { echo "no $name in $map" >&2; exit 1; }
    for f in tests/skill-*.toml tests/chain-skill-hard.toml tests/settings-persist.toml tests/pause-overlay.toml; do
        [ -f "$f" ] || continue
        sed -i.bak -E "s/^$name = 0x[0-9a-f]+$/$name = 0x$addr/" "$f" && rm -f "$f.bak"
    done
    echo "$name = 0x$addr"
done
