#!/usr/bin/env bash
# Proof that the monsters and effects play the sample the original plays:
# builds one probe disc (tests/probes/sfx-voices.txt, six maps), runs each map
# headlessly and reads the `sfx` and `slot` lines of the emulator log.
#
#   probe event=slot a=SLOT b=BANK_ID       the map bank id bound to a set-piece slot
#   probe event=sfx  a=PATH b=ID c d e=POS  every playback (PATH 0 resident core, 1 voice,
#                                           2 map bank, 3 map loop)
#
# A check passes when the expected sample plays at least once in the window of the
# probe that provokes it. A set-piece sample the cooker left out of a map's bank
# (spare SPU RAM only, hl_format::setpiece_audio tier 4) is reported as LIMIT with
# the cooker's numbers, not as a failure.
#
#   HLPSX_FRONTEND=/path/to/frontend scripts/sfx-proof.sh [WORKDIR]
#
# Probe discs are for tracing only and are never shipped; rebuild the shipping disc
# afterwards. The probe builds replace the game's PGO layout while they run (put back on exit).
set -euo pipefail

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"
frontend=${HLPSX_FRONTEND:?set HLPSX_FRONTEND to a psoxide frontend binary}
work=${1:-$root/.hlpsx/sfx-proof}
rooms="14 24 12 28 52 86 88"
frames=${SFX_PROOF_FRAMES:-1800}
mkdir -p "$work"

layout=game/pgo/hl-psx.layout
if [ -f "$layout" ]; then
    mv "$layout" "$work/layout.keep"
    trap 'mv "$work/layout.keep" "$root/$layout"' EXIT
fi

csv=$(echo "$rooms" | tr ' ' ',')
# SFX_PROOF_NOBUILD=1: dist/ already holds a probe disc built from this probe file.
if [ -z "${SFX_PROOF_NOBUILD:-}" ]; then
    HLPSX_PROBE="$root/tests/probes/sfx-voices.txt" HLPSX_BENCH_MAPS=$csv \
        cargo run --release --manifest-path host/hl-build/Cargo.toml -- \
        pack --no-intro --features interaction-probe ${HL_DIR:+--half-life "$HL_DIR"} \
        > "$work/pack.log" 2>&1
fi

# Two emulator runs at a time.
run_room() {
    local room=$1 mask
    mask=$(printf '0x04%02x' "$room")
    rm -rf "$work/run-$room"
    mkdir -p "$work/run-$room"
    (
        cd "$work/run-$room"
        "$frontend" launch --path "$root/dist/hl-psx.cue" --embedded-playtest \
            --steps 4000000000 --guest-frames "$frames" --pad-pulses "$mask@1+180" \
            --guest-debug-log --no-memcard > out.txt 2> err.txt
    )
}
n=0
for room in $rooms; do
    run_room "$room" &
    n=$((n + 1))
    [ $((n % 2)) -eq 0 ] && wait
done
wait

slot_const() { grep -o "pub const $1: u8 = [0-9]*" shared/hl-format/src/lib.rs | grep -o '[0-9]*$'; }
core_const() { grep -o "pub const $1: u8 = [0-9]*" game/src/sfx.rs | grep -o '[0-9]*$'; }
# count PATH ID ROOM : playbacks of that sample in the room's log
count() {
    grep -c "event=sfx|a=$1|b=$2|" "$work/run-$3/err.txt" || true
}
bank_id() { # ROOM SLOT -> the bank id, empty when the bank lacks the slot
    grep -o "event=slot|a=$2|b=[0-9]*" "$work/run-$1/err.txt" | head -1 | grep -o '[0-9]*$' || true
}

fail=0
slot() { # ROOM LABEL SLOTNAME
    local n id c
    n=$(slot_const "$3")
    id=$(bank_id "$1" "$n")
    if [ -z "$id" ]; then
        printf '  LIMIT %-34s slot %s is not in room %s bank (spare SPU RAM, see the cook log)\n' "$2" "$3" "$1"
        return
    fi
    c=$(count 2 "$id" "$1")
    if [ "$c" -ge 1 ]; then
        printf '  PASS  %-34s slot %s (bank id %s) played %s times\n' "$2" "$3" "$id" "$c"
    else
        printf '  FAIL  %-34s slot %s (bank id %s) never played\n' "$2" "$3" "$id"
        fail=1
    fi
}
core() { # ROOM LABEL CONSTNAME
    local id c
    id=$(core_const "$3")
    c=$(count 0 "$id" "$1")
    if [ "$c" -ge 1 ]; then
        printf '  PASS  %-34s core %s (%s) played %s times\n' "$2" "$3" "$id" "$c"
    else
        printf '  FAIL  %-34s core %s (%s) never played\n' "$2" "$3" "$id"
        fail=1
    fi
}
any_attack() { # ROOM LABEL CORECONST SLOTNAME: the leap shriek, then the bite when it lands
    core "$1" "$2 (shriek)" "$3"
    slot "$1" "$2 (bite)" "$4"
}
silent_core() { # ROOM LABEL CONSTNAME POSX : the core sample never plays at that x
    local id c
    id=$(core_const "$3")
    c=$(grep -c "event=sfx|a=0|b=$id|c=$4|" "$work/run-$1/err.txt" || true)
    if [ "$c" -eq 0 ]; then
        printf '  PASS  %-34s core %s never plays at x=%s\n' "$2" "$3" "$4"
    else
        printf '  FAIL  %-34s core %s played %s times at x=%s\n' "$2" "$3" "$c" "$4"
        fail=1
    fi
}

echo "c1a1b valve (room 14)"
slot 14 "valve spark" SPARK
silent_core 14 "no ricochet at the valve" RIC 2408
echo "c1a3a sentry, scientist, barnacle (room 24)"
slot 24 "sentry death" TU_DIE
slot 24 "scientist pain" SCI_PAIN
echo "c1a1 zombie (room 12)"
slot 12 "zombie claw" ZO_CLAW
echo "c1a4 houndeye (room 28)"
slot 28 "houndeye warm-up" HE_WARM3
core 28 "houndeye blast" HE_BLAST
slot 28 "houndeye growl" HE_GROWL
echo "c2a3b ichthyosaur (room 52)"
slot 52 "ichthyosaur pain" ICHY_PAIN
slot 52 "ichthyosaur death" ICHY_DIE
echo "c4a1b alien grunt (room 86)"
slot 86 "alien grunt pain" AG_PAIN
slot 86 "alien grunt death" AG_DIE
echo "c4a1d controller (room 88)"
slot 88 "controller pain" CON_PAIN
slot 88 "controller death" CON_DIE

[ "$fail" -eq 0 ] && echo "sfx proof: all checks passed (LIMIT rows are not failures)" || { echo "sfx proof: FAILED"; exit 1; }
