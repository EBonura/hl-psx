#!/usr/bin/env bash
# Proof that the Hazard Course number panels light up: builds a probe disc
# (tests/probes/hazard-panels.txt: the player stands in front of the seven numbered score panels
# of t0a0b and the probe switches their lights on in turn) and runs the journey
# tests/hazard-panels.toml on it. Each plate must be much brighter after its light comes on.
#
#   PSOXIDE_GATE=/path/to/psoxide-gate scripts/panel-proof.sh
#
# Needs the cooked data/ and the same Half-Life input as a normal pack. The probe disc is for
# the journey only and is deleted from dist/ afterwards, so rebuild the shipping disc after it.
# The probe build replaces the game's PGO layout while it runs (put back on exit).
set -euo pipefail

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"
gate=${PSOXIDE_GATE:?set PSOXIDE_GATE to a psoxide-gate binary}
out=${1:-$root/.hlpsx/panel-proof}
room=98 # t0a0b
mkdir -p "$out"

layout=game/pgo/hl-psx.layout
if [ -f "$layout" ]; then
    mv "$layout" "$out/layout.keep"
    trap 'mv "$out/layout.keep" "$root/$layout"' EXIT
fi

HLPSX_PROBE="$root/tests/probes/hazard-panels.txt" HLPSX_BENCH_MAPS=$room \
    cargo run --release --manifest-path host/hl-build/Cargo.toml -- \
    pack --no-intro --features interaction-probe ${HL_DIR:+--half-life "$HL_DIR"} \
    > "$out/pack.log" 2>&1
status=0
"$gate" run tests/hazard-panels.toml --no-hw --disc dist/hl-psx.cue --out "$out/run" -v \
    > "$out/gate.log" 2>&1 || status=$?
rm -f dist/hl-psx.bin dist/hl-psx.cue
grep -a -E '^\s+(PASS|FAIL)' "$out/gate.log" || true
exit $status
