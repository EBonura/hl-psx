#!/usr/bin/env bash
# The journey gate: every tests/*.toml, on the shipping disc, except the probe journeys listed in
# tests/probe-journeys.txt, which run on a probe disc this script builds itself (so no journey
# needs a disc somebody has to remember to make).
#
#   PSOXIDE_GATE=/path/to/psoxide-gate HL_DIR=<Half-Life valve folder> scripts/gate.sh [OUTDIR] [journey.toml ...]
#
# Needs the shipping disc and its link map (dist/hl-psx.cue, .hlpsx/hl-psx.map; a disc installed in
# the library works when copied there), the cooked data/ and the same Half-Life input as a normal
# pack. Environment:
#   GATE_ARGS   psoxide-gate flags (default: --no-hw --hardware real --second-hardware mister)
#   GATE_JOBS   journeys at a time (default 2)
#   GATE_PROBE  `skip` leaves the probe journeys out (default: build the probe disc if stale)
# The probe build runs on the shipping tree, swaps the shipping disc, map and PGO layout out of the
# way while it runs and puts them back on exit; the probe disc is cached in dist/probe and rebuilt
# only when game/, shared/, tests/probes/gate.txt or the cooked data change.
# Results: OUTDIR/results.txt, one `journey | PASS ...` line each, and the gate's own reports.
# Exit 1 when any journey is not a pass.
set -euo pipefail

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"
gate=${PSOXIDE_GATE:?set PSOXIDE_GATE to a psoxide-gate binary}
out=${1:-$root/.hlpsx/gate-run}
if [ $# -gt 0 ]; then shift; fi
gate_args=${GATE_ARGS:---no-hw --hardware real --second-hardware mister}
jobs=${GATE_JOBS:-2}
mkdir -p "$out"
keep="$out/ship-keep"
shipped="dist/hl-psx.bin dist/hl-psx.cue .hlpsx/hl-psx.map game/pgo/hl-psx.layout"

probe_list=$(grep -v '^#' tests/probe-journeys.txt | grep -v '^$' || true)

if [ $# -gt 0 ]; then
    list=("$@")
else
    list=()
    for t in tests/*.toml; do list+=("$(basename "$t")"); done
fi

is_probe() { printf '%s\n' "$probe_list" | grep -qx "$1"; }

need_probe=no
for n in "${list[@]}"; do
    is_probe "$n" && need_probe=yes
done
[ "${GATE_PROBE:-}" = skip ] && need_probe=no

stamp_of() {
    {
        git rev-parse HEAD:game HEAD:shared 2> /dev/null
        git diff HEAD --stat -- game shared | tail -1
        shasum -a 256 tests/probes/gate.txt
        cat data/.hlpsx-cook.json 2> /dev/null
    } | shasum -a 256 | cut -d' ' -f1
}

restore() {
    local f
    for f in $shipped; do
        if [ -f "$keep/${f//\//_}" ]; then mv "$keep/${f//\//_}" "$f"; fi
    done
}

build_probe() {
    local stamp
    stamp=$(stamp_of)
    if [ -f dist/probe/stamp ] && [ "$(cat dist/probe/stamp)" = "$stamp" ] \
        && [ -f dist/probe/hl-psx.cue ] && [ -f .hlpsx/probe/hl-psx.map ]; then
        echo "probe disc up to date (dist/probe)"
        return 0
    fi
    echo "building the probe disc (log $out/probe-build.log)"
    mkdir -p "$keep" dist/probe .hlpsx/probe
    for f in $shipped; do
        if [ -f "$f" ]; then mv "$f" "$keep/${f//\//_}"; fi
    done
    trap restore EXIT
    # Room ids of the probe sections: t0a0b 98, c1a1b 14.
    HLPSX_PROBE="$root/tests/probes/gate.txt" HLPSX_BENCH_MAPS=98,14 \
        cargo run --release --manifest-path host/hl-build/Cargo.toml -- \
        pack --no-intro --features interaction-probe ${HL_DIR:+--half-life "$HL_DIR"} \
        > "$out/probe-build.log" 2>&1
    mv dist/hl-psx.bin dist/hl-psx.cue dist/probe/
    mv .hlpsx/hl-psx.map .hlpsx/probe/hl-psx.map
    printf '%s\n' "$stamp" > dist/probe/stamp
    restore
    trap - EXIT
}

[ "$need_probe" = yes ] && build_probe

run_one() {
    local n=$1 res
    # shellcheck disable=SC2086
    if is_probe "$n"; then
        res=$("$gate" run "tests/$n" --repo "hl=$root" $gate_args --out "$out/${n%.toml}" 2>&1 \
            | grep -a -E '^(PASS|FAIL)' | head -2 | tr '\n' ' ' || true)
    else
        res=$("$gate" run "tests/$n" --repo "hl=$root" --disc dist/hl-psx.cue $gate_args \
            --out "$out/${n%.toml}" 2>&1 | grep -a -E '^(PASS|FAIL)' | head -2 | tr '\n' ' ' || true)
    fi
    echo "$n | ${res:-NORESULT}" >> "$out/results.txt"
}

: > "$out/results.txt"
export -f run_one is_probe
export gate root gate_args out probe_list
printf '%s\n' "${list[@]}" | xargs -P "$jobs" -I{} bash -c 'run_one {}'

fail=$(grep -c -E "FAIL|NORESULT|ABORT" "$out/results.txt" || true)
echo "$(wc -l < "$out/results.txt") journeys, $fail not passing (see $out/results.txt)"
[ "$fail" = 0 ]
