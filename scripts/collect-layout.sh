#!/bin/bash
# Collect the I-cache layout (game/pgo/hl-psx.layout) from scripted maps.
#
#   HLPSX_FRONTEND=/path/to/frontend scripts/collect-layout.sh MAP...
#
# `cargo hl-build pgo --polls` can only count the gameplay window of one input
# tape, and a layout from one route made c1a2b 14% slower. This counts every
# instruction word over polls 2400..3400 of each given play-order map (entered
# through the menu by scripts/mapnav.sh, the same maps `pgo --script` profiles) and
# binds all of them to the link map in one layout, so no map is favoured.
#
# Steps: delete game/pgo/hl-psx.layout, `cargo hl-build pack` (a link without a
# layout), run this, then `cargo hl-build pack` again: the build places the
# functions with the layout and links twice. A layout that no longer binds to
# the code is reported and skipped, so collect it again after any code change
# and after every `pgo` run (the profile moves the code).
#
# 2026-10-16 (maps 0 6 11 12 18 20 23 56 60 88 94): with this layout every one of
# the 11 maps runs at least as fast as the build before the 64-bit clean-up
# (shipped disc, polls 2400..3400), where the profile alone left three slower
# by up to 2% on I-cache placement luck.
set -eu
repo=$(cd "$(dirname "$0")/.." && pwd)
frontend=${HLPSX_FRONTEND:?set HLPSX_FRONTEND to the retained emulator frontend}
disc="$repo/dist/hl-psx.cue"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
[ $# -gt 0 ] || { echo "usage: collect-layout.sh MAP..." >&2; exit 2; }
[ -f "$repo/game/pgo/hl-psx.layout" ] && { echo "remove game/pgo/hl-psx.layout and pack first" >&2; exit 2; }

count() {
    p=$1
    pulses=$("$repo/scripts/mapnav.sh" "$p")
    "$frontend" launch --path "$disc" --embedded-playtest --steps 40000000000 \
        --pad-pulses "$pulses" --no-memcard --stop-at-poll 2430 --route-log "$work/loc$p.csv" >/dev/null 2>&1
    start=$(awk -F, 'NR==1{for(i=1;i<=NF;i++){if($i=="route_tick")a=i;if($i=="port1_polls")b=i}} NR>1 && $b>=2400 {print $a; exit}' "$work/loc$p.csv")
    "$frontend" launch --path "$disc" --embedded-playtest --steps 40000000000 \
        --pad-pulses "$pulses" --no-memcard --stop-at-poll 3400 \
        --pc-log-words --pc-line-log "$work/words$p.csv" --pc-line-start-route-tick "$start" >/dev/null 2>&1
}
for p in "$@"; do
    count "$p" &
    while [ "$(jobs -r | wc -l)" -ge 4 ]; do sleep 5; done
done
wait
words=""
for p in "$@"; do words="$words $work/words$p.csv"; done
cd "$repo/.psoxide"
# shellcheck disable=SC2086
cargo run --release -q -p psoxide-pgo -- layout "$repo/.hlpsx/hl-psx.map" \
    "$repo/game/target/mipsel-sony-psx/release/hl-psx.exe" $words "$repo/game/pgo/hl-psx.layout"
echo "layout -> game/pgo/hl-psx.layout"
