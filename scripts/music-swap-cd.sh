#!/usr/bin/env bash
# The CD command order of a cached weapon swap on a map that plays music (play-order map 84,
# CD track 26): no data read may be issued while the song plays, and the song must start again.
# Runs the frontend on a built disc, writes the emulator's CD command log and replays it with
# tools/cdcheck (exit 1 on a data read over a playing song). The companion journey
# tests/music-swap.toml asserts the same from the game's own CD_TRACK_CUR/CD_TRACK_WANT.
#
#   scripts/music-swap-cd.sh FRONTEND DISC.cue OUTDIR
set -euo pipefail
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
frontend=$1; disc=$2; out=$3
mkdir -p "$out"
cargo build --release --manifest-path "$root/tools/cdcheck/Cargo.toml" >/dev/null
# R1 twice (two cold viewmodels), then L1 (the tail cache swaps the first back in).
pulses="0x0008@100+8,0x0008@170+8,$("$root/scripts/mapnav.sh" 84 1100),0x0800@3000+8,0x0800@3200+8,0x0400@3400+8"
"$frontend" launch --path "$disc" --embedded-playtest --steps 4000000000 --pad-pulses "$pulses" \
    --no-memcard --cd-command-log "$out/cd.csv" --stop-at-poll 3900 >"$out/run.log" 2>&1
"$root/tools/cdcheck/target/release/cdcheck" "$out/cd.csv" | tee "$out/cdcheck.txt"
