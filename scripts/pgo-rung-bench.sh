#!/bin/bash
# The bench hook of `psoxide-pgo ladder` (run by `cargo hl-build pgo`): build one hot call-site
# threshold the way it ships (I-cache layout collected on that very link) and print its frame
# rates as `score NAME VALUE` lines, higher is better.
#
# Needs PSOXIDE_PGO_THRESHOLD (set by the ladder), HL_DIR, HLPSX_FRONTEND, and the gate next to
# the frontend (or GATE). Leaves game/pgo/ as it found it; the layout collected for this rung
# is kept in .hlpsx/ladder/rung-N/hl-psx.layout for the caller to install if the rung is picked.
#
# Windows (polls after the first map load of the play-order map, intro disc):
#   c0a0   map 0, polls +139..+1139;   c4a3  map 94, the same;
#   chain  the Hazard Course journey, flips/s over polls 1200..2380 and 1800..2380.
set -eu
repo=$(cd "$(dirname "$0")/.." && pwd)
cd "$repo"
threshold=${PSOXIDE_PGO_THRESHOLD:?run by psoxide-pgo ladder}
frontend=${HLPSX_FRONTEND:?set HLPSX_FRONTEND}
gate=${GATE:-$(dirname "$frontend")/psoxide-gate}
host=$repo/host/hl-build/target/release/hl-psx-build
work=$repo/.hlpsx/ladder/rung-$threshold
rm -rf "$work"; mkdir -p "$work"

# The shipped threshold and layout are put back when the bench ends, so a ladder run leaves
# the tree as it found it; the layout collected for this rung is kept in $work.
saved=$work/saved
mkdir -p "$saved"
cp game/pgo/hot-callsite-threshold "$saved/"
[ -f game/pgo/hl-psx.layout ] && cp game/pgo/hl-psx.layout "$saved/"
restore() {
    cp "$saved/hot-callsite-threshold" game/pgo/hot-callsite-threshold
    if [ -f "$saved/hl-psx.layout" ]; then cp "$saved/hl-psx.layout" game/pgo/hl-psx.layout; else rm -f game/pgo/hl-psx.layout; fi
}
trap restore EXIT
echo "$threshold" > game/pgo/hot-callsite-threshold
rm -f game/pgo/hl-psx.layout
# stdout stays clean for the score lines: the build logs go to files.
"$host" pack --no-intro --half-life "$HL_DIR" > "$work/pack1.log" 2>&1
scripts/collect-layout.sh 0 6 11 12 18 20 23 56 60 88 94 0:760:1760:0b 94:1030:2030:94b > "$work/collect.log" 2>&1
cp game/pgo/hl-psx.layout "$work/hl-psx.layout"
"$host" pack --half-life "$HL_DIR" > "$work/pack2.log" 2>&1
grep -h "PGO shipped\|RAM budget" "$work/pack2.log" >&2 || true

cue=$repo/dist/hl-psx.cue
map=$repo/.hlpsx/hl-psx.map
vm=$(awk '$5 == "hl_psx::VM_ENTRY" {print $1; exit}' "$map")
watch=$(printf '%x' $((0x$vm + 8)))

# fps over polls anchor+139..anchor+1139 after the first load of play-order map $1.
window() {
    name=$1; p=$2
    pulses="0x0008@100+8,0x0008@170+8,$(scripts/mapnav.sh "$p" 1100)"
    nice -n 10 "$frontend" launch --path "$cue" --embedded-playtest --steps 4000000000 \
        --pad-pulses "$pulses" --route-log "$work/$name.csv" --route-watch-u32 "0x$watch" \
        --no-memcard --stop-at-poll 4600 > /dev/null 2>&1
    fps=$(awk -F, 'NR==1{for(i=1;i<=NF;i++){if($i=="port1_polls")p=i;if($i=="display_start_changed")f=i;if($i=="bus_cycle_delta")c=i} w=NF; next}
        {v=$w%256; if(v!=q){ if(v==1 && !n && $p>=100){n=1; a=$p} q=v } P[NR]=$p; F[NR]=$f; C[NR]=$c; N=NR}
        END{ for(r=2;r<=N;r++) if(n && P[r]>=a+139 && P[r]<=a+1139){fl+=F[r];cy+=C[r]} printf "%.3f", fl*33868800/cy }' "$work/$name.csv")
    echo "$name $fps" > "$work/$name.score"
}
window c0a0 0 &
window c4a3 94 &
wait

# The chain journey's [addr] table is bound to one build: rebind a copy to this link's map.
toml=$repo/tests/chain-hazard-rung.toml
cp tests/chain-hazard.toml "$toml"
for e in "LOGIC_PLAYER_POS:" "LOGIC_PLAYER_YAW:(.0)" "SIM_NOW:(.0)" "WORLD_ARENA:"; do
    n=${e%%:*}; suf=${e##*:}
    a=$(awk -v a="hl_psx::$n" -v s="$suf" '$5 == a && $6 == s { print $1; exit }' "$map")
    [ -n "$a" ] || { echo "no $n in $map" >&2; exit 1; }
    sed -i '' -E "s/^$n = 0x[0-9a-f]+$/$n = 0x$a/" "$toml"
done
"$gate" run "$toml" --repo hl="$repo" --disc "$cue" --no-hw --hardware real \
    --out "$work/chain" --tick-log "$work/chain.csv" > "$work/chain.log" 2>&1 || true
rm -f "$toml"
for w in "1200 2380 chain" "1800 2380 chainlate"; do
    set -- $w
    awk -F, -v a="$1" -v b="$2" -v n="$3" 'NR==1{for(i=1;i<=NF;i++){if($i=="polls")p=i;if($i=="flipped")f=i;if($i=="cycle_delta")c=i};next}
        $p>=a&&$p<=b{fl+=$f;cy+=$c} END{printf "%s %.3f\n", n, fl*33868800/cy}' "$work/chain.csv" > "$work/$3.score"
done
for n in c0a0 c4a3 chain chainlate; do
    read -r name value < "$work/$n.score"
    [ -n "$value" ] || { echo "no $n score" >&2; exit 1; }
    echo "score $name $value"
done
