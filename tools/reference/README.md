# Deterministic GoldSrc reference

This harness runs the original Half-Life game DLL on Xash3D at a fixed 20 Hz
with a fixed RNG seed. It can use either neutral input or a map-segmented
`HLINPUT1` semantic route, and records authoritative player,
`func_tracktrain`, target-dispatch, once-per-second entity checkpoints, and
replayed-input state as stable
`HLREF|...` records. The instrumentation is opt-in; without the `HLREF_*`
environment variables the two upstream projects retain their normal behavior.

The captured patches are pinned to these upstream revisions:

- `xash3d-fwgs` `9f2b8954e9a787dd74271186fe2930e14e2d2e8e`
- `hlsdk-portable` `8c5b2846c2448e2b063f358f041d565dc0f076b1`

Use your own legally installed Half-Life assets. No Valve data belongs in this
repository or in a trace commit.

## Apply and build

Clone both upstream repositories at the revisions above. Check the patches
before applying them:

```sh
git -C "$XASH_SRC" apply --check "$HLPSX/tools/reference/patches/xash3d-fwgs-hlref.patch"
git -C "$HLSDK_SRC" apply --check "$HLPSX/tools/reference/patches/hlsdk-portable-hlref.patch"
git -C "$XASH_SRC" apply "$HLPSX/tools/reference/patches/xash3d-fwgs-hlref.patch"
git -C "$HLSDK_SRC" apply "$HLPSX/tools/reference/patches/hlsdk-portable-hlref.patch"
```

The following macOS/Apple Silicon example builds the same configuration used
for the opening-tram reference. `SDL_FRAMEWORK` is the full path to
`SDL2.framework`; `RUNTIME` is a disposable staging directory.

```sh
(cd "$XASH_SRC" && ./waf configure -T debug \
  --sdl2="$SDL_FRAMEWORK" --enable-bundled-deps --enable-null \
  --enable-stbtt --disable-mbedtls --disable-werror)
(cd "$XASH_SRC" && ./waf build)
(cd "$XASH_SRC" && ./waf install --destdir="$RUNTIME")

(cd "$HLSDK_SRC" && ./waf configure -T debug --disable-werror)
(cd "$HLSDK_SRC" && ./waf build)
(cd "$HLSDK_SRC" && ./waf install --destdir="$RUNTIME")
```

Other platforms can use the upstream-supported compiler/SDL configuration; the
trace protocol itself is platform-independent.

## Semantic input routes

`HLINPUT1` is the canonical input exchange format between hl-psx and the
GoldSrc reference. Unlike PSoXide's vblank-clocked `PXITAPE1`, it is divided at
map boundaries and indexed by each map's local 20 Hz gameplay tick. Loading
time therefore cannot consume or shift the route.

All integers are little-endian. The file consists of one header, its ordered
segment headers, and each segment's runs immediately after that header:

| Record | Layout | Meaning |
| --- | --- | --- |
| Header | `<8sHHI>` | `HLINPUT1`, tick rate `20`, segment count, reserved flags `0` |
| Segment | `<16s16sIIII>` | NUL-padded map, next map, expanded ticks, run count, neutral-tail ticks, flags `0` |
| Run | `<HbbbbH>` | duration, forward, strafe, turn, look, action bits |

Map names are 1-15 ASCII letters, digits, `_`, or `-`; the final segment has an
empty next-map field. Runs last 1-65,535 ticks and longer identical spans are
split. The decoder rejects unknown flags or action bits, bad padding, tick-total
mismatches, trailing data, and an out-of-order map chain.

The four axes are signed 8-bit semantic values. Xash scales forward/strafe by
the player's current maximum speed divided by 128. Turn and look are relative
per-tick changes matching hl-psx's controller gains: `130/128` and `95/128`
angle units, where one angle unit is `360/4096` degrees. Positive turn/look
subtract yaw/pitch respectively, and pitch is clamped to +/-89 degrees.

Action bits are:

| Bit | Action | Xash mapping |
| ---: | --- | --- |
| 0 | attack | `IN_ATTACK` |
| 1 | jump | `IN_JUMP` |
| 2 | duck | `IN_DUCK` |
| 3 | use | `IN_USE` |
| 4 | secondary attack | `IN_ATTACK2` |
| 5 | reload | `IN_RELOAD` |
| 6 | next weapon | rising-edge `invnext` |
| 7 | previous weapon | rising-edge `invprev` |
| 8 | flashlight | rising-edge impulse 100 |

The final three actions are edge-triggered so holding a semantic button cannot
cycle or toggle repeatedly. Every segment also carries a finite neutral tail.
It absorbs a small transition-timing difference without shifting the following
map; exhausting that tail, seeing the wrong map, or reaching an unlisted map is
a hard deterministic error.

The helper validates tapes, expands one segment back to trace rows, and builds
an RLE tape from guest input rows:

```sh
python3 "$HLPSX/tools/reference/hlinput.py" validate route.hlinput
python3 "$HLPSX/tools/reference/hlinput.py" expand route.hlinput \
  --map c0a0 > c0a0-input.trace
python3 "$HLPSX/tools/reference/hlinput.py" extract psx-playthrough.log \
  route.hlinput --neutral-tail-ticks 200
```

Extraction accepts a plain row or a row with a PSoXide log prefix:

```text
HLPSX|input|map=c0a0|tick=0|forward=127|strafe=0|turn=-8|look=0|actions=0x0008
```

Each map must begin at tick zero and remain contiguous. The helper groups
consecutive maps, records the required next-map order, and RLE-compresses
identical samples. A repeated visit to the same BSP is represented by another
ordered segment and can be selected for inspection with `expand --occurrence`.

### Anomalous Materials route

The checked-in route generator proves the retail c1a0 spawn through the c1a0d
HEV pickup and the real c1a0a airlock transition. It includes the security-desk
sequence, both map transitions, the lounge door in each direction, the suit-case
control, the suit pickup, Barney's retinal scan, and both airlock doors.

```sh
python3 "$HLPSX/tools/reference/anomalous_materials_route.py" \
  /tmp/anomalous-materials-through-airlock.hlinput
python3 "$HLPSX/tools/reference/run_goldsrc_reference.py" \
  --runtime-dir "$RUNTIME" --half-life-dir "$HALF_LIFE" \
  --framework-dir "$(dirname "$SDL_FRAMEWORK")" \
  --map c1a0 --semantic-input /tmp/anomalous-materials-through-airlock.hlinput \
  --max-ticks 2400 --entity-interval 100 \
  --output /tmp/gold-anomalous-materials-through-airlock.trace

HLPSX_SEMANTIC_INPUT=/tmp/anomalous-materials-through-airlock.hlinput \
  make -C "$HLPSX" disc FEATURES=semantic-input
"$PSOXIDE/target/release/frontend" launch \
  --path "$HLPSX/dist/hl-psx.cue" --embedded-playtest \
  --steps 100000000000 --guest-frames 7900 \
  --input-tape "$HLPSX/captures/reference/c1a0-neutral.pxitape" \
  --guest-debug-log 2>/tmp/psx-anomalous-materials-through-airlock.trace

python3 "$HLPSX/tools/reference/trace_tools.py" compare \
  /tmp/gold-anomalous-materials-through-airlock.trace \
  /tmp/psx-anomalous-materials-through-airlock.trace \
  --require-map c1a0 --require-map c1a0d --require-map c1a0a \
  --require-event c1a0d:target_fire:rr1 \
  --require-event c1a0d:target_fire:step \
  --require-event c1a0d:target_fire:hevmaster1 \
  --require-event c1a0d:target_fire:redspot \
  --require-event c1a0d:target_fire:airlockwalker \
  --require-event c1a0d:target_fire:airlockbarneymm1 \
  --require-event c1a0d:target_fire:control_retinal1mm \
  --require-event c1a0d:target_fire:airlockdoorbuzzmm1 \
  --require-event c1a0d:target_fire:lk1 \
  --require-event c1a0d:target_fire:lk2 \
  --require-input-parity

# Never leave a semantic-input tape in a shipping/test disc.
make -C "$HLPSX" disc
```

The proof fires `rr1` in both directions, `step` when opening the HEV case,
`hevmaster1` plus `redspot` when the suit is collected, the full retinal-scan
chain, and `lk1`/`lk2` before entering c1a0a. The two-tick south correction on
the return route is intentional: it centers both engines in the retail rr1
door clearance despite a 19-unit fixed-point landing difference.

## Capture and verify

The runner sets all determinism controls, invokes the null renderer, strips any
launcher prefix, and writes only `HLREF` records:

```sh
python3 "$HLPSX/tools/reference/run_goldsrc_reference.py" \
  --runtime-dir "$RUNTIME" \
  --half-life-dir "$HALF_LIFE" \
  --framework-dir "$(dirname "$SDL_FRAMEWORK")" \
  --max-ticks 6500 \
  --output "$HLPSX/captures/reference/goldsrc-c0a0-run1.trace"
```

To replay a captured semantic route through the original game, add:

```sh
  --semantic-input "$HLPSX/captures/reference/opening.hlinput"
```

Direct-loading an intermediate BSP is not normally equivalent to arriving
through a changelevel. Some maps have no `info_player_start`; others place it
inside a sealed transition pocket. For a deterministic chapter checkpoint,
declare the GoldSrc origin and view explicitly:

```sh
python3 "$HLPSX/tools/reference/run_goldsrc_reference.py" \
  --runtime-dir "$RUNTIME" --half-life-dir "$HALF_LIFE" \
  --map c1a1b --max-ticks 2520 \
  --initial-origin 1390 -230 -129 \
  --initial-angles 0 -90 0 \
  --semantic-input c1a1b-route.hlinput \
  --output goldsrc-c1a1b.trace
```

Coordinates and angles here are Half-Life/GoldSrc values. The patched DLL
applies them once immediately before the first player physics frame, clears
velocity, relinks the hull, and emits an `event=initial_state` row containing
the resulting state. Put one neutral tick at the front of a route that supplies
`--initial-angles`; GoldSrc acknowledges the forced client view after that
first server frame. The example is the current cooked standalone checkpoint
for `c1a1b`, whose PSX-space position `[1390,-129,-230]` maps back to GoldSrc
`[1390,-230,-129]`.

The runner validates the complete tape before launch and requires its first
segment to match `--map`. Xash consumes input only while the client is active,
resets the local input tick on each ordered map transition, and emits one
`HLREF|input|...` row per consumed sample. Host keyboard, mouse, and controller
input are suppressed during both semantic and neutral captures.

GoldSrc can emit a few server-spawn ticks before the local client becomes
active. Those ticks deliberately do not consume the route. Use
`HLREF|input_map|...|input_tick=0` (and the first `HLREF|input` row) as the
per-map input anchor rather than assuming input tick zero equals server map
tick zero.

The patched game DLL also emits `HLREF|entity` rows every 20 post-physics
player ticks. BSP submodel (`brush=*N`) is the stable identity for movers and
breakables; named point actors fall back to `targetname`. Rows include live
origin, collision-box center, velocity, animation state, health, solidity,
movement type, and spawn flags. Their independent `map_tick` begins with the
first `PlayerPostThink`, so they align with PSX without inheriting Xash's
pre-client server-frame offset.

Repeat with a second output path, then require byte-for-byte equality:

```sh
python3 "$HLPSX/tools/reference/run_goldsrc_reference.py" \
  --runtime-dir "$RUNTIME" \
  --half-life-dir "$HALF_LIFE" \
  --framework-dir "$(dirname "$SDL_FRAMEWORK")" \
  --max-ticks 6500 \
  --output "$HLPSX/captures/reference/goldsrc-c0a0-run2.trace"

python3 "$HLPSX/tools/reference/trace_tools.py" self-check \
  "$HLPSX/captures/reference/goldsrc-c0a0-run1.trace" \
  "$HLPSX/captures/reference/goldsrc-c0a0-run2.trace"
```

A successful opening capture visits
`c0a0,c0a0a,c0a0b,c0a0c,c0a0d,c0a0e` and ends with an
`HLREF|stop|...|reason=max_ticks` record. Two verified 6,500-tick runs contained
18,081 records each and had SHA-256
`cdfe43ddadfddfe2473851c590fd469a3a22d799854f71e91ccc33b0106e7562`.
Two extended 12,000-tick runs contained 29,081 records each and had SHA-256
`862b15f1e2050de87469f1a4855af2ed4ba5b9880a5f25c455d015538ac42540`.

The controls set by the runner are:

- `HLREF_TRACE=1`: emit post-physics snapshots and target fires.
- `HLREF_ENTITY_INTERVAL=20`: entity checkpoint cadence in fixed ticks; the
  runner's `--entity-interval 1` mode is intended for short lifecycle probes
  and also records angular velocity, local pusher time, and next-think time.
- `HLREF_SEED=1337`: reset the game RNG immediately before each map entity lump.
- `HLREF_NEUTRAL_INPUT=1`: remove host input when no semantic route is supplied.
- `HLREF_SEMANTIC_INPUT=/path/to/route.hlinput`: replay the validated route.
- `HLREF_INITIAL_ORIGIN="x y z"`: one-shot reference checkpoint position.
- `HLREF_INITIAL_ANGLES="pitch yaw roll"`: one-shot reference checkpoint view.
- `HLREF_MAX_TICKS=6500`: stop at a deterministic global server tick.
- `host_framerate 0.05`: advance the server in fixed 50 ms steps.

## Capture hl-psx in PSoXide

To replay the same HLINPUT1 route in hl-psx, build the diagnostic-only feature
with an absolute route path. The route is embedded in this test EXE and is
never present in a normal build; prefer chapter-sized tapes so diagnostic
`.rodata` does not consume the shipping RAM margin:

```sh
HLPSX_SEMANTIC_INPUT="$HLPSX/captures/reference/route.hlinput" \
  make -C "$HLPSX" disc FEATURES=semantic-input
```

The ordinary PSoXide tape is still used to select the first map at boot, but
once gameplay begins the guest does not poll controller state or permit pause.
It consumes the HLINPUT1 segment at exactly one sample per 20 Hz fixed tick,
resets to tick zero on each ordered changelevel, and panics on a malformed,
exhausted, skipped, or unexpected segment. `semantic-input` implies
`reference-trace`, so every consumed row is emitted as `HLPSX|input`.

Build the trace-only disc and generate a neutral direct-boot tape long enough
to reach the station door:

```sh
(cd "$HLPSX" && make disc FEATURES=reference-trace)
python3 "$HLPSX/tools/reference/make_input_tape.py" \
  "$HLPSX/captures/reference/c0a0-neutral.pxitape" \
  --frames 22000 --map-index 0
```

Run the tape from the PSoXide emulator workspace. Guest trace records are on
stderr, so keep stdout available for the emulator's completion summary:

```sh
(cd "$PSOXIDE/emu" && cargo run -p frontend --release -- launch \
  --path "$HLPSX/dist/hl-psx.cue" \
  --embedded-playtest --steps 100000000000 \
  --input-tape "$HLPSX/captures/reference/c0a0-neutral.pxitape" \
  --guest-debug-log \
  2> "$HLPSX/captures/reference/psx-c0a0-run1.log")
```

Repeat as `run2`, then verify that normalized guest traces are identical:

```sh
python3 "$HLPSX/tools/reference/trace_tools.py" self-check \
  "$HLPSX/captures/reference/psx-c0a0-run1.log" \
  "$HLPSX/captures/reference/psx-c0a0-run2.log"
```

Compare a capture with an hl-psx/PSoXide trace using:

```sh
python3 "$HLPSX/tools/reference/trace_tools.py" compare \
  "$HLPSX/captures/reference/goldsrc-c0a0-run1.trace" \
  "$HLPSX/captures/reference/psx-c0a0-run1.log" \
  --require-map c0a0e \
  --require-event c0a0c:target_fire:gate1mm \
  --require-event c0a0d:target_fire:levelchangetoemm \
  --require-event c0a0e:target_fire:traindoor \
  --require-carry c0a0d:out:8727 \
  --require-carry c0a0e:in:8727 \
  --require-attached-exit c0a0 \
  --require-attached-exit c0a0a \
  --require-attached-exit c0a0b \
  --require-attached-exit c0a0c \
  --require-attached-exit c0a0d \
  --require-attached-exit c0a0e \
  --output "$HLPSX/captures/reference/c0a0-diff.json"
```

For a driven HLINPUT1 route, also pass `--require-input-parity`. The comparator
requires the normalized input rows to match exactly and aligns player/train
snapshots at the first state tick *after* input tick zero in each engine. This
accounts for GoldSrc's pre-client server ticks without a hand-maintained offset:

```sh
python3 "$HLPSX/tools/reference/trace_tools.py" compare \
  goldsrc.trace psx.log --require-input-parity --max-entity-error 2 \
  --max-entity-angle-error 1 \
  --output driven-diff.json
```

The report's `entity_comparison` section is grouped by ordered map visit. Use
`--max-entity-error N` to gate matched checkpoint distance and
`--require-entity-parity` when an audited route is expected to have identical
brush/actor membership, active state, and directly comparable fields on both
engines. `--max-entity-angle-error N` separately gates `func_rotating` phase in
circular GoldSrc degrees; this catches a fan that keeps spinning after its
target fires even when its pivot/center remains nearly unchanged.

Map occurrences are tracked independently. Routes that revisit a BSP (for
example `c1a1c -> c1a1d -> c1a1c`) get separate input anchors, event timing,
and position reports instead of overwriting the first visit's local ticks.
Every route requirement accepts `MAP@N` to select an exact visit, for example
`--require-map c1a1c@2` or
`--require-event c1a1c@2:target_fire:eledoordelaymm`. An unqualified map keeps
the previous any-visit behavior (and attached-exit checks use its final visit).

This is the opening-route progression gate: it fails if the fork-lift gate,
final inter-map relay, station door, or real Barney transition did not occur, or
if neutral play fell off the tram before a map boundary. It deliberately does
not use `--strict`, because cosmetic target parity is tracked separately from
progression.
