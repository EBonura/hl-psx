# 96-map performance fleet

`fleet_audit.py` boots every map through hl-psx's `debug-map-boot` feature and
measures real rendered-frame delivery with PSoXide telemetry. It does not infer
FPS from the 20 Hz fixed-update clock.

## Authoritative run

From the hl-psx repository root:

```sh
python3 tools/perf/fleet_audit.py run \
  --build --build-emulator \
  --out captures/perf-fleet
```

The build uses `emulator-telemetry,debug-map-boot` once, emits an exact linker
map for static-RAM accounting, packs the disc, hashes every disc file and the
frontend binary, and records both repositories' commits and dirty-tree
fingerprints. The run defaults to 64 visual frames per map, discards four
visual warmup frames (direct boot settles its initial caches by frame four),
and requires 60 measured frames for the strict gate.

The guest-frame stop is a dead-render fallback, not the measurement target. It
automatically scales to `max(1200, 16 * visual_frames)` so update-heavy maps do
not terminate a long capture before its requested visual count. Override it
only when deliberately bounding a pathological run.

An interrupted run is safe to continue if the binary, disc, repositories, and
capture policy are unchanged:

```sh
python3 tools/perf/fleet_audit.py run --resume --out captures/perf-fleet
```

To measure an already-built disc, omit `--build` and supply its matching linker
map if static RAM should be authoritative:

```sh
python3 tools/perf/fleet_audit.py run \
  --disc dist/hl-psx.cue \
  --linker-map captures/hl-psx.map \
  --out captures/perf-fleet
```

Use `--maps c2a5e,64-72` for a subset. `--jobs 2` runs two independent emulator
processes; guest bus-cycle results remain deterministic, while the default of
one avoids host I/O contention. `--route forward` and `--route forward-run`
provide optional moving-spawn passes after the static baseline.

If source work continues after the fleet disc was built, bind a targeted rerun
to the frozen fleet provenance with `--base-manifest <fleet>/manifest.json`.
The runner first verifies that the complete disc and frontend hashes still
match, then inherits the original source identities instead of stamping the
newer worktree against an older binary.

Outputs include per-map profile/counter/hash CSVs, final PPMs, console logs and
capture metadata, plus `summary.csv`, `summary.json`, and `REPORT.md`.

## What the numbers mean

- Delivery p50/p95 is elapsed PS1 bus cycles between rendered visual endpoints,
  including skipped simulation-only rows and catch-up work.
- Render p50/p95 is the instrumented render stage on rows that produced a visual.
- Packets are transient mixed packet-arena slots.
- Triangle equivalents are `tri_primitives + room_surf_whole_quads`: each quad
  packet represents two triangles.
- Static RAM comes from `__bss_end` in the linker map and is shared by all maps.

Packet and triangle-equivalent figures are lower bounds. The persistent tram
and viewmodel packet caches are linked directly into ordering tables and are
not included in `TRI_PRIMITIVES`. Cycle, delivery, deadline, and overflow
metrics are unaffected by this telemetry gap.

## Current automation boundary

The direct-boot protocol is `L1 | map_index` in the pad's low byte, held during
the first 150 VBlanks. It reliably addresses the current 96-map registry.

PSoXide currently rejects `--input-tape` or `--sweep` when `--pad-pulses` is
present, and direct map boot requires a pad pulse. Therefore the fleet can
automate deterministic spawn views and simple held-forward routes, but not an
authored camera/player route per map in the same process. There is also no CLI
save-state checkpoint after common boot, so all 96 captures must start a fresh
emulator and reload shared assets.

Consequently, this fleet is a broad triage pass, not proof that every viewpoint
in each map meets 20 FPS. Rerun the reported worst maps for longer captures and
manually authored routes. Closing the remaining automation gap requires either:

1. allowing a boot pulse prefix before an input tape/sweep in PSoXide, or
2. adding an out-of-band map-index launch control/save-state checkpoint, and
3. exposing total submitted GPU packets/triangles including persistent caches.

Short 64-visual captures also do not exercise hl-psx's periodic stack watermark
report (first emitted after 256 simulation frames). RAM-stack fleet work should
use a separate longer pass of at least 257 simulation ticks per map.
