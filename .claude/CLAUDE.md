# hl-psx -- Project Guide

A **bring-your-own-assets** renderer for *Half-Life* (1998, GoldSrc) on the
**PlayStation 1**, written in Rust on the **PSoXide** SDK. Sibling of the
`oot-psx` project, which proves this exact toolchain (GTE + ordering table +
disc streaming). Sets out to be the *faithful, open* counterpart to XProger's
closed "Half-Life PSX" demake.

## Why this is feasible (and where the edge is)

- **BSP + PVS maps onto the PS1 ordering table for free.** GoldSrc derives from
  Quake; BSP back-to-front traversal *is* painter's order, exactly what the OT
  wants. PVS culls leaves before the GTE ever sees them. This is a more natural
  fit than oot-psx's N64 display lists were.
- **Faithful from official source, not eyeballed.** Quake's renderer/BSP/PVS is
  GPL open source; the **Half-Life SDK** (entities, monsters, weapons, scripted
  sequences) was officially released by Valve as real C++; **Xash3D** is a
  runnable ground-truth oracle. Port the real logic (float -> fixed,
  span-render -> OT), don't reverse-engineer by guessing.
- **The pipeline already exists.** Reuse oot-psx's GTE/OT/streaming/4-bit-crush
  experience. The hard parts are VRAM/texture budget and the sheer scope of the
  game, not the renderer.

## Hard rules

- **No Valve assets, ever.** The user supplies their own Half-Life install; the
  extracted `data/`, any `*.wad/*.bsp/*.mdl`, and the source game files are all
  git-ignored. Never commit, paste, or hard-code asset-derived bytes.
  - Source files are read from `HL_DIR` (Makefile var), defaulting to the macOS
    Steam path `~/Library/Application Support/Steam/steamapps/common/Half-Life`.
    GoldSrc content is under `$(HL_DIR)/valve` (`*.wad`, `maps/*.bsp`,
    `models/*.mdl`, `pak0.pak`). Override with `make ... HL_DIR=/path` or a
    git-ignored `config.mk`. `make check-assets` verifies the path resolves.
- **No floats in the render path** -- the PS1 has no FPU. Fixed-point + GTE only.
- **No heap in the render path** -- 2 MB total RAM; statically sized buffers.

## Build / run

```bash
make submodule   # init pinned PSoXide SDK (third_party/PSoXide @ bedcc21)
make build       # -> game/target/mipsel-sony-psx/release/hl-psx.exe
make disc        # -> dist/hl-psx.{bin,cue}  (boot this in PSoXide)
make run         # build + install into the PSoXide game library
```

Nightly toolchain + `mipsel-sony-psx` target via `-Zbuild-std` (see
`rust-toolchain.toml`, `game/.cargo/config.toml`). `game/build.rs` injects
PSoXide's `sdk/psoxide.ld` and emits a flat PSX-EXE. The crate lives under
`game/` (not the repo root) so its PSX-target `.cargo/config.toml` doesn't leak
onto host tools (mkisopsx). Same proven wiring as oot-psx.

## State at handoff

- **Milestone 0 (skeleton)**: `game/src/main.rs` is the GTE spinning-cube smoke
  test (adapted from the SDK's `hello-gte`). It boots and proves the toolchain.
  This is the next thing to replace with a real renderer.
- **Nothing else is wired yet** -- no asset extractor, no BSP parser, no
  streaming. The asset path (where Half-Life files come from, how they're
  cooked) is the first design decision; check feasibility before building it.

## Feasibility gate -- ANSWERED (green)

The texture/VRAM question is settled (see `docs/feasibility.md`, reproduce with
`make bsp-info`). Every single-player campaign map's full texture set fits PS1
VRAM at 4bpp **all-resident, no within-map streaming** (worst campaign map
~609 KB < ~704 KB). Textures are embedded in the BSP (no WAD step). Geometry is
trivial (3.7k-8k faces/map, far less after PVS). Only deathmatch/community maps
exceed the budget. The host inspector that measured this is `tools/hl-bsp`.

## M1 -- BSP map renderer (DONE, verified)

First light confirmed in the emulator: c1a0 (Black Mesa Inbound) renders as
GTE-projected, OT-depth-sorted flat triangles with a pad fly-cam. Pipeline:

- `tools/hl-bsp --cook <bsp> <hlm>` walks faces (surfedges->polygon->fan tris),
  remaps HL Z-up to world Y-up (winding reversed), and colours each triangle
  with its source texture's average RGB. Output `.hlm` (see `game/src/map.rs`).
  `make cook MAP=c1a0` -> `data/maps/c1a0.hlm`, `include_bytes!`'d into the EXE.
- `game/src/main.rs`: project every vertex (RTPS) into a scratch buffer, then
  per triangle near-cull + OT-insert a `TriFlat`. Fly-cam: D-pad move/turn,
  L1/R1 vertical, Triangle/Cross pitch. Camera = oot's view convention
  (rotY·rotX, rows 0/1 negated, T=-R·eye).
- Capture/verify: sibling `pico8-psx/tools/.../frametest --disc dist/hl-psx.cue
  --out f.ppm --frames N [--hold MASK]` (HLE fast-boot, homebrew-safe). PPM->PNG
  via PIL. Pad masks: UP 0x10, LEFT 0x80, CROSS 0x4000.

Known simplifications (deliberate, M1): `CULL=false` (backface cull off until
winding double-checked); no PVS (all in-front faces drawn); no collision
(fly-cam clips through walls); spawn = map-bbox centre, not info_player_start.

## M2 -- textured map rendering (DONE, verified)

c1a0 renders with real materials. Pipeline:

- Extractor (`cook` v2, `.hlm` "HLM2"): per miptex, nearest-downscale to a
  power-of-two <=64, median-cut the 256-colour palette to 16, pack 4-bit. UVs
  from the BSP texinfo planes, scaled to cooked size, per-face tile-shifted and
  saturated to u8. tex_id == miptex index. 164 texs, 370 KB cooked.
- Runtime (`game/src/vram.rs`): one-time upload via `psx-vram`
  `TextureWindowAtlas` (11 pages, band Y=0, X=320..) + `ClutRowAllocator`
  (Y=480..); per texture build a `TextureMaterial` (CLUT word + tpage word +
  GP0-E2 texture window). `main.rs` emits `TriTextured::with_material` per face.

Known simplifications (M2): textures capped at 64x64 (one VRAM band fits all
164); faces tiling >~4x clamp at the far edge (no UV subdivision); affine
texture warp on big tris (no perspective correction); flat full-bright tint (no
lighting); still `CULL=false`, no PVS, no collision, bbox-centre spawn.

## Next (pick per value)

- **PVS leaf culling**: decompress vis, draw only visible leaves -> the perf win
  and the architectural payoff (BSP+PVS -> OT). Also enables backface cull tuning.
- **UV subdivision + affine correction**: runtime split of large/grazing tris so
  tiling is correct and textures stop warping (oot's `subdiv_emit` pattern).
- **Vertex lighting**: bake BSP lightmaps (or face-normal shade) to per-vertex
  colours via `TriTexturedGouraud` -> depth + mood instead of full-bright.
- **Real spawn**: parse `info_player_start` from the entity lump.
- **Multi-map streaming**: cook all campaign maps, stream per level change.

## PSoXide SDK map (third_party/PSoXide/sdk/crates)

| Crate | Use |
|-------|-----|
| `psx-rt` | runtime/entry (`#[no_mangle] fn main`, `extern crate psx_rt`), `tty` |
| `psx-gpu` | `init`, `FrameBuffer`, `OrderingTable`, textured/gouraud tris + quads, `BlendMode`, texture pages, `draw_line_mono` |
| `psx-gte` / `psx-gte-core` | `math::{Vec3I16, Vec3I32, Mat3I16}`, `scene::{...}` project/transform helpers, light rigs, depth-cue/fog |
| `psx-pad` | controller input |
| `psx-vram` | VRAM texture/CLUT management |
| `psx-spu` | audio (SPU-ADPCM) |
| `psx-asset` | runtime asset blobs |
| `psx-io` | async CD-ROM streaming |

The SDK ships runnable examples under `third_party/PSoXide/sdk/examples/`
(`hello-gte`, `hello-ot`, `hello-tex`, `hello-tri`, `hello-input`,
`hello-audio`) -- read these first; the cube boot is adapted from `hello-gte`.

## PS1 hardware constraints (respect in all changes)

- **VRAM** 1024x512 @ 16bpp: framebuffers X=0..319 Y=0..479; textures
  X=320..1023; CLUTs Y=496..511. Textures must not cross TPage boundaries
  (64 px for 4-bit, 128 for 8-bit). Texture windows need power-of-2 dims.
- **GTE** for all vertex transforms (RTPS/RTPT). Never do matrix-vector math in
  software for rendering.
- **Ordering table** for depth (no Z-buffer): insert primitives back-to-front.
  BSP traversal gives this ordering naturally.
- **Double buffering**: primitive memory must stay stable until DMA completes.
- **GPU limits**: triangle spans <= 1023 px H, 511 px V.
- **CD-ROM**: all file I/O is async; never block on disc reads.

## Conventions

- Fixed-point: 4.12 for matrices/movement (match oot-psx + the SDK examples).
- Coordinates: Half-Life is Z-up right-handed -> PS1 X-right, Y-down, Z-forward
  (in the view transform). Pin the exact mapping when M1 lands.
- Naming/idioms: match the surrounding Rust; mirror the SDK examples' style.

## Reference (kept local, git-ignored under reference/)

- **Quake source** (GPL) -- BSP/PVS/renderer ground truth.
- **Half-Life SDK** (Valve, github.com/ValveSoftware/halflife) -- game logic.
- **Xash3D FWGS** -- runnable open GoldSrc engine to diff behaviour against.
- **`oot-psx`** (sibling repo) -- the proven PSoXide pipeline to lift patterns
  from (GTE projection, OT depth, VRAM allocation, 4-bit crush, CD streaming).
