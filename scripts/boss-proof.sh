#!/usr/bin/env bash
# Proof that Gonarch reads the difficulty: one probe disc per level (tests/probes/gonarch-proof.txt), c4a2 headless, then the
# first slash against the player is compared with skill.cfg (sk_bigmomma_dmg_slash) and the mortar rhythm with the 2 to 15 s rule.
#
#   HLPSX_FRONTEND=/path/to/frontend scripts/boss-proof.sh [WORKDIR]
#
# Probe discs are for tracing only and are never shipped; dist/ is cleared after each, so rebuild the shipping disc afterwards.
set -euo pipefail

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"
frontend=${HLPSX_FRONTEND:?set HLPSX_FRONTEND to a psoxide frontend binary}
work=${1:-$root/.hlpsx/boss-proof}
cfg=data/skill.cfg
room=91 # c4a2
mkdir -p "$work"

cfg_value() { # KEY LEVEL -> the integer value skill.cfg gives that level
    awk -v k="$1$(($2 + 1))" '$1 == k { gsub(/"/, "", $2); print int($2) }' "$cfg"
}

fail=0
check() { # label expected actual
    if [ "$2" = "$3" ]; then
        printf '  PASS  %-40s expected %s, got %s\n' "$1" "$2" "$3"
    else
        printf '  FAIL  %-40s expected %s, got %s\n' "$1" "$2" "$3"
        fail=1
    fi
}

layout=game/pgo/hl-psx.layout
if [ -f "$layout" ]; then
    mv "$layout" "$work/layout.keep"
    trap 'mv "$work/layout.keep" "$root/$layout"' EXIT
fi

names=(Easy Medium Hard)
for level in 0 1 2; do
    echo "${names[$level]} (level $level)"
    sed "s/@LEVEL@/$level/" tests/probes/gonarch-proof.txt > "$work/probe-$level.txt"
    HLPSX_PROBE="$work/probe-$level.txt" HLPSX_BENCH_MAPS=$room \
        cargo run --release --manifest-path host/hl-build/Cargo.toml -- \
        pack --no-intro --features interaction-probe ${HL_DIR:+--half-life "$HL_DIR"} \
        > "$work/pack-$level.log" 2>&1
    rm -rf "$work/run-$level"
    mkdir -p "$work/run-$level"
    (
        cd "$work/run-$level"
        "$frontend" launch --path "$root/dist/hl-psx.cue" --embedded-playtest \
            --steps 4000000000 --guest-frames 1400 --pad-pulses "$(printf '0x04%02x' $room)@1+180" \
            --guest-debug-log --no-memcard > out.txt 2> err.txt
    )
    rm -f dist/hl-psx.bin dist/hl-psx.cue
    log=$work/run-$level/err.txt

    # health of the player on the `pos` lines: the first value below 100 minus 100 is the first slash
    drop=$(grep -a 'HLPSX|probe|' "$log" | grep -a '|event=pos|' | sed -E 's/.*\|d=([0-9]+).*/\1/' | awk '$1 < 100 { print 100 - $1; exit }' || true)
    slash=$(cfg_value sk_bigmomma_dmg_slash "$level")
    check "first slash (sk_bigmomma_dmg_slash)" "$slash" "${drop:-none}"

    # the mortar is launched 2 to 15 s (40 to 300 ticks) apart
    gaps=$(grep -a 'HLPSX|probe|' "$log" | grep -a '|event=gonarch|name=mortar|' | sed -E 's/.*\|tick=([0-9]+)\|.*/\1/' | awk 'NR > 1 { print $1 - prev } { prev = $1 }' || true)
    ok=1
    for g in $gaps; do [ "$g" -ge 40 ] && [ "$g" -le 300 ] || ok=0; done
    check "mortar gaps inside 40..300 ticks" 1 "$ok"
done
exit $fail
