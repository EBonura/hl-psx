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

`LAB_BRUSH` picks how the lab anchors brush faces among the world packets it puts in exact BSP
leaf order: unset (face centroid), `cent` (centroid, rock handled by the nearest open leaf),
`pkt` (the packet's own centroid), `far` / `near` (leaf of the farthest / nearest vertex),
`legacy` / `legacy2` (keep the game's own depth key), `mono` (bend world keys monotone in depth
space), `span` (map the entity's depth span onto the leaves it spans). `LAB_PNG_DIR` writes the
mask of every sequenced frame with at least 1500 major pixels. `leafrange BSP_DIR MAP...` reports
whether the leaves under each BSP node are a contiguous run of numbers (they are not).

`lab2 BSP_DIR LOG...` is the second lab. It paints every pixel with its own painter key, so an
experiment can ask what would happen if each piece of a brush face were ordered on its own,
without cutting the traced packets. World faces draw at the farthest visible leaf that marks them
(`L2_NODE=1`: at the position of their BSP node), every other fragment at the position of the tree
slot that holds its 3D point. Rock counts: every child slot of the tree, solid ones included, has a
rank in the back-to-front walk, so a point inside a wall has a place in the order. Environment:

- `L2_BRUSH` (static brush entities: func_wall, func_illusionary, func_breakable ...) and
  `L2_MOVER` (every other brush entity): `frag` (the slot of each pixel's own 3D point, an ideal cut
  of the face at the tree planes), `pkt` (slot of the packet centroid), `pktz` (screen centroid at
  the mean vertex depth, what the guest can compute), `pktmin` / `pktmax`, `cent` (face centroid),
  `cell` (cells of `L2_CELL` units), `ent` (entity bounds centre), `entmin` / `entmax` (farthest /
  nearest slot of any vertex), `fallback` (an entity spanning several slots draws with the world
  faces of that slot range, all by the game's depth key), `last`.
- `L2_MODEL`: `frag`, `pkt` or `last` for studio models. `L2_WATER_LEGACY=1` keeps func_water at the
  slot of the world packet nearest in the game's key.
- `L2_LEGACY=game|min|mean|max|minb|meanb` keeps the game's depth space and re-keys brush packets
  (`game` reproduces the shipping census exactly, a check on the replay itself).
- `L2_GROUP=1` keys a world face at the farthest slot of any face on its plane in the frame.
- `L2_FRAME_HYBRID=1|entity` falls back to the game's order for frames that show a mover spanning
  several slots (or any entity).
- Output: `L2_FACES=1` worst face pairs, `L2_ENTITIES=1` and `L2_SELF=1` per entity tables,
  `L2_FRAMECSV=file` per frame class table, `L2_PAIR=map:frame,...` with `L2_PAIR_DIR=dir` writes
  four panes (game painter, game mask, lab painter, lab mask), `L2_DEBUG_PIX=frame,x,y` prints the
  fragments of one pixel.

`cellcount BSP_DIR MAP...` counts the brush faces and the cells they would make at 32 to 128 units.
`splitcount BSP_DIR MAP...` cuts every brush face through the world tree at its cooked place and
counts the fragments (`SPLIT_STATIC=1` for the static entities only).

```sh
cargo test --manifest-path tools/hl-order-oracle/Cargo.toml
```
