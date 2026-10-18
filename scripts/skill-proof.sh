#!/usr/bin/env bash
# Proof that the difficulty reaches the monsters: builds one probe disc per level
# (tests/probes/skill-hit.txt), runs c1a1 headlessly and compares what the
# emulator log shows with the user's skill.cfg:
#   - the attacking zombie's spawn health          sk_zombie_health
#   - the player's health after the first bout     two claws of sk_zombie_dmg_one_slash,
#                                                  or one heavy swing of sk_zombie_dmg_both_slash
#   - the zombie's health after one 60 point hit   spawn health - 60 (gone when that is <= 0)
#
#   HLPSX_FRONTEND=/path/to/frontend scripts/skill-proof.sh [WORKDIR]
#
# Needs the cooked data/ (data/skill.cfg) and the same Half-Life input as a normal
# pack. Probe discs are for timing and tracing only and are never shipped; each
# level's disc is deleted from dist/ once its log is read, so rebuild the shipping disc
# afterwards. Run it from a clean state: the probe builds replace the game's PGO layout
# while they run (put back on exit).
set -euo pipefail

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"
frontend=${HLPSX_FRONTEND:?set HLPSX_FRONTEND to a psoxide frontend binary}
work=${1:-$root/.hlpsx/skill-proof}
cfg=data/skill.cfg
room=12 # c1a1
mkdir -p "$work"

# cfg KEY LEVEL (0 Easy, 1 Medium, 2 Hard) -> the integer value skill.cfg gives that level
cfg_value() {
    awk -v k="$1$(($2 + 1))" '$1 == k { gsub(/"/, "", $2); print int($2) }' "$cfg"
}

fail=0
check() { # label expected actual
    if [ "$2" = "$3" ]; then
        printf '  PASS  %-34s expected %s, got %s\n' "$1" "$2" "$3"
    else
        printf '  FAIL  %-34s expected %s, got %s\n' "$1" "$2" "$3"
        fail=1
    fi
}

# A probe build is not the shipping image, so the shipped I-cache layout cannot bind to it.
layout=game/pgo/hl-psx.layout
if [ -f "$layout" ]; then
    mv "$layout" "$work/layout.keep"
    trap 'mv "$work/layout.keep" "$root/$layout"' EXIT
fi

names=(Easy Medium Hard)
for level in 0 1 2; do
    echo "${names[$level]} (level $level)"
    sed "s/@LEVEL@/$level/" tests/probes/skill-hit.txt > "$work/probe-$level.txt"
    HLPSX_PROBE="$work/probe-$level.txt" HLPSX_BENCH_MAPS=$room \
        cargo run --release --manifest-path host/hl-build/Cargo.toml -- \
        pack --no-intro --features interaction-probe ${HL_DIR:+--half-life "$HL_DIR"} \
        > "$work/pack-$level.log" 2>&1
    rm -rf "$work/run-$level"
    mkdir -p "$work/run-$level"
    (
        cd "$work/run-$level"
        "$frontend" launch --path "$root/dist/hl-psx.cue" --embedded-playtest \
            --steps 4000000000 --guest-frames 1800 --pad-pulses "$(printf '0x04%02x' $room)@1+180" \
            --guest-debug-log --no-memcard > out.txt 2> err.txt
    )
    rm -f dist/hl-psx.bin dist/hl-psx.cue
    log=$work/run-$level/err.txt

    probe_lines() { grep -a 'HLPSX|probe|' "$log" | grep -a "|tick=$1|event=$2|"; }
    spawn=$(probe_lines 3 prop | grep -a '|b=5|' | head -1 | sed -E 's/.*\|c=([0-9]+)\|.*/\1/' || true)
    # The player's health on the first `pos` line below 100.
    drop=$(grep -a 'HLPSX|probe|' "$log" | grep -a '|event=pos|' | sed -E 's/.*\|d=([0-9]+).*/\1/' | awk '$1 < 100 { print 100 - $1; exit }' || true)
    after=$(probe_lines 216 prop | grep -a '|b=5|' | head -1 | sed -E 's/.*\|c=([0-9]+)\|.*/\1/' || true)

    hp=$(cfg_value sk_zombie_health "$level")
    one=$(cfg_value sk_zombie_dmg_one_slash "$level")
    both=$(cfg_value sk_zombie_dmg_both_slash "$level")
    check "zombie spawn health (sk_zombie_health)" "$hp" "${spawn:-none}"
    if [ "${drop:-none}" = "$((2 * one))" ] || [ "${drop:-none}" = "$both" ]; then
        check "first bout (2 x $one or $both)" "$drop" "$drop"
    else
        check "first bout (2 x $one or $both)" "$((2 * one))" "${drop:-none}"
    fi
    left=$((hp - 60))
    [ $left -lt 0 ] && left=0
    check "zombie after a 60 point hit" "$left" "${after:-0}"
done
exit $fail
