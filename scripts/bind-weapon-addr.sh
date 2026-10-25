#!/usr/bin/env bash
# The weapon journeys (tests/weapons-*.toml) read the HUD's last drawn weapon, clip,
# reserve and health, and the last resident sound asked for (sfx::LAST_CORE), from guest RAM. Those statics are split statics the gate's own symbol
# loader cannot read (the linker map prints them as `hl_psx::hud::PREV_CLIP (.0)`), so each
# journey carries them as fixed addresses in an [addr] table. An address is only valid for
# the build whose map it came from: run this after every build that changed the executable,
# before running those journeys.
#
#   scripts/bind-weapon-addr.sh [MAP]     (default .hlpsx/hl-psx.map)
set -euo pipefail
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"
map=${1:-.hlpsx/hl-psx.map}
for entry in hud::PREV_WEAPON hud::PREV_CLIP hud::PREV_RESERVE hud::PREV_HEALTH sfx::LAST_CORE CD_TRACK_CUR CD_TRACK_WANT; do
    name=${entry#*::}
    addr=$(awk -v a="hl_psx::$entry" '$5 == a && ($6 == "" || $6 == "(.0)") { print $1; exit }' "$map")
    [ -n "$addr" ] || { echo "no $name in $map" >&2; exit 1; }
    for f in tests/weapons-*.toml tests/music-swap.toml; do
        [ -f "$f" ] || continue
        sed -i.bak -E "s/^$name = 0x[0-9a-f]+$/$name = 0x$addr/" "$f" && rm -f "$f.bak"
    done
    echo "$name = 0x$addr"
done
