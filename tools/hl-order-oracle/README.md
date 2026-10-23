# hl-order-oracle

Measures wrong-order pixels in the game's ordering-table render. The guest (built with the
`order-trace` feature) streams every primitive of one frame over the debug port, in draw order,
with the depth its key was taken from. This tool rasterizes that frame twice from the same
vertices, once painted in draw order (what the GPU does) and once with a per-pixel depth test
(what is right), and reports the pixels where the visible surface differs.

## Capture

Build a probe disc with the feature and a probe script that poses the camera and arms a capture:

```sh
HLPSX_PROBE=/abs/path/probe.txt cargo hl-build ... --features order-trace
```

```text
tp 1480 -320 112 64 0 pin
osnap
```

`osnap` arms the next rendered frame. Run the disc headless with the debug port logged, then:

```sh
cargo run --release --manifest-path tools/hl-order-oracle/Cargo.toml -- \
    analyze --out OUT_DIR [--bsp-dir BSP_DIR] [--face-map CSV_DIR] [--png-all] [--png-major N] LOG...
```

A log is named after its map (`t0a0a.log`, `t0a0a-p3.log`). With `--bsp-dir` (the retail `.bsp`
files) a world face gets its exact plane depth and brush entities are told from the world.
`--face-map` is a directory of `MAP.csv` files written by the cooker (`HLPSX_FACE_TOPOLOGY_REPORT`):
they map cooked face numbers back to source BSP faces. `OUT_DIR` receives `frames.csv`, the
summaries and the PNG masks (`--png-major N` writes a mask for frames with at least N major
pixels).

## What it reports

Per frame and in total: wrong-order pixels at two tolerances (minor: 1 unit + 0.4 %, major: 8 units
+ 3 %), split by the pair of classes involved (world, brush entity, studio model, effect) and by
cause (tie, span, thin, inverted), with a PNG mask of the major pixels.

`poses BSP_DIR MAP... [--spots N] [--yaws A,B,C]` writes probe-script poses spread over a map for
sweeps. `pvsmax BSP_DIR MAP...` prints the largest visible leaf and face sets. `inspect LOG MAP
FRAME_HEX FACE...` lists the packets of those source faces in draw order. `lab` replays traced
frames in the order a BSP leaf sequence would draw them (an offline experiment, set `FACE_MAP`
and `LAB_PAIRS`/`LAB_VERBOSE` to widen its output).

```sh
cargo test --manifest-path tools/hl-order-oracle/Cargo.toml
```
