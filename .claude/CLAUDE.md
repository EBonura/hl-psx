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
make submodule        # init pinned PSoXide SDK (third_party/PSoXide @ bedcc21)
make cook MAP=c1a0    # cook a level -> data/maps/current.hlm (any of 125 maps)
make build            # -> game/target/mipsel-sony-psx/release/hl-psx.exe
make disc             # -> dist/hl-psx.{bin,cue}  (boot this in PSoXide)
make run              # build + install into the PSoXide game library
```

The runtime `include_bytes!`s `data/maps/current.hlm`; `make cook MAP=<name>`
writes it, so any map builds without editing source. Verified on c1a0 (tram
start) and c1a1a (Anomalous Materials).

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

## M3 -- lighting (DONE, verified)

c1a0 lit from its real BSP lightmaps. Extractor (`.hlm` "HLM3") averages each
face's base-style lightmap (luxels 16 texels apart; extents from texinfo) into a
per-face tint, boosted ~1.5x, stored as `tri_rgb`. Runtime emits
`TriTexturedGouraud` with that tint on all 3 vertices, so the GPU modulates the
texel (128 = 1.0x). Faces with no lightmap render neutral/full-bright.

Simplification: per-face flat shade (not per-vertex/per-pixel lightmaps); good
mood, but no smooth gradients within a face. Per-vertex lightmap sampling is the
upgrade.

## M4 -- PVS leaf culling (DONE, verified)

Each frame: walk the BSP tree to the camera's leaf, decompress its PVS, draw
only the faces of visible leaves (per-face draw-once dedup). Verified: identical
visible geometry to M3, correct across leaf transitions, no holes.

- Extractor (`.hlm` HLM4, header gained `bsp_off`): appends nodes (planes
  transformed to world space: swap Y/Z normal, dist/scale), leaves
  (visofs + marksurface range), marksurfaces (face indices), raw vis (RLE), and
  a per-face triangle range (`face_first`/`face_ntri`).
- Runtime (`main.rs`): `camera_leaf` tree walk (i64 dot >> 12 vs world plane),
  `decompress_vis` (Quake RLE, bit i -> leaf i+1), then per visible leaf draw
  its marksurfaces' faces -> triangles (project per-tri via RTPT).

Caveat: tri-count/fps delta not measured (no telemetry). Correct + active; the
win is largest in enclosed areas. Fallback draws everything when the camera is
in the solid/outside leaf 0.

## M5 -- player collision + walking (DONE, verified) -- FIRST GAMEPLAY

The fly-cam is now a grounded FPS player. Spawns at the real `info_player_start`
(parsed from the entity lump) and renders the actual HL tram-station start view.

- Extractor (`.hlm` HLM5, header gains `clip_off`): appends the clip hull
  (`CLIPNODES` planes transformed to world space + children), `hull1` headnode
  (model[0].headnode[1]), and the spawn origin+yaw (HL->world, yaw-90deg).
- Runtime (`game/src/phys.rs`): fixed-point port of Quake `SV_RecursiveHullCheck`
  (point trace through the pre-expanded hull), a 4-iteration slide-move, gravity,
  ground probe. `Player { pos, vel, on_ground }`. Camera eye = pos + 28 (world).
- Controls: D-pad up/down walk, left/right turn, L1/R1 strafe, Triangle/Cross
  look, Circle jump.
- Verified: spawn view = real HL start; walk forward stops dead at a wall (no
  void clip); grounded throughout.

Simplification: no step-up yet (can't climb stairs/thresholds); single standing
hull (no crouch); simple velocity (no accel/friction/aircontrol).

## M6 -- map selection (DONE)

Runtime builds from `data/maps/current.hlm`; `make cook MAP=<name>` writes it.
The full pipeline is map-general -- verified on c1a0 and c1a1a.

## M7 -- per-vertex lighting (DONE)

Replaced M3's per-face average with per-vertex lightmap sampling (`.hlm` now
HLM6; `tri_rgb` is 9 bytes/tri = one shade per corner). The runtime already drew
`TriTexturedGouraud`, so this is just feeding it three colours -> smooth gradients
across faces instead of flat-per-face.

## M9 -- robustness pass: clipping / framerate / controls (DONE)

Playtest flagged near-camera clipping artifacts, sluggish framerate, clunky
controls. Borrowed the proven oot-psx `room.rs` techniques (see `game/src/render.rs`):

- **Near-plane clipping**: triangles straddling the near plane are rebuilt in
  view space (`scene::transform_vertex`), Sutherland-Hodgman clipped to
  `z >= NEAR_Z`, software-reprojected (`project_soft`: one `(H<<12)/z` Q12
  reciprocal), then guard-band clipped to the GPU span limits -- instead of being
  dropped (which popped geometry out near walls). All fixed-point `(num<<12)/den`
  on i32 (the target miscompiles `i64 / runtime`).
- **Framerate**: M5 had regressed to projecting each visible triangle separately;
  reverted to projecting every vertex ONCE per frame into a cache (RTPT batched,
  `SCRATCH`), assembling world tris from it. Entities still project their few tris
  fresh (per-entity GTE translation). (Not fps-measured; undoes the regression.)
- **Controls**: analog stick (`enable_analog_port1` + `left_centered` + deadzone)
  with D-pad fallback; tunable turn rate; movement speed normalised (±127 input).

Backface cull stays ON (`CULL=true`, winding verified). Active stages: PVS leaf
cull -> backface cull -> near/guard clip.

## M8 -- brush entities + doors (DONE, verified)

Brush entities now render (they were invisible before -- the PVS path only draws
world/model-0 faces via leaf marksurfaces). `func_door`s slide open near the
player and close when they leave.

- Extractor (`.hlm` HLM7, header gains `ent_off`): a submodel face-range table
  (dmodel firstface/numface × n_models) + an entity table. Brush ents parsed from
  the entity lump (skipping invisible `trigger_*`/`func_ladder`); `func_door`
  records its world move vector (`door_move`: angle/size/lip), trigger centre, and
  radius². 64 ents in c1a0.
- Runtime (`main.rs`): after the world draw, each entity draws its submodel's
  faces with a per-entity GTE translation (base view shifted by the entity offset,
  so shared vertex data needs no copy). Doors lerp the offset by a proximity-driven
  phase (`ENT_PHASE`). Verified on c1a1a: blast door renders closed, opens on
  approach.

Simplifications: no door COLLISION yet (player walks through the panel; world
frame still blocks, and doors auto-open so you pass the gap); ALL func_doors open
on proximity (targetname/button triggering ignored); func_door_rotating treated
as static; no func_button/func_breakable logic.

## M10 -- affine subdivision (DONE)

`emit_cv` recursively splits big on-screen triangles at view-space midpoints
(`render::mid_cv`) before `project_soft`, fixing affine texture warp on large
floors/walls. Gated: only tris with screen span > SUBDIV_PX (96), depth 1
(<=4 sub-tris each), so the triangle count barely moves. Raise SUBDIV_DEPTH if
warp persists.

## M11 -- tram ride (func_tracktrain) (DONE, verified)

The real Half-Life opening. NOTE: **c0a0** (not c1a0) is the game's first map --
the Black Mesa Inbound tram ride. `make cook MAP=c0a0`.

- Cook: `collect_tram` finds the `func_tracktrain` submodel + speed and follows
  its `path_track` target chain into a waypoint list (.hlm HLM8, header gains
  `tram_off`; `func_tracktrain` skipped in the entity list). c0a0 = 30 waypoints.
- Runtime: scripted ride -- the player is locked to the tram (no physics), carried
  along the waypoints at `tram_speed/TRAM_STEP_DIV` units/frame; `ride_off` =
  tram displacement from its parked start. The tram submodel renders at `ride_off`
  (per-entity GTE translation); the player can still look (right stick). When the
  path ends, normal physics resume. Verified: camera rides through the tunnel,
  tram car visible, world scrolls past.

Simplifications: no walking on the moving tram (locked), no submodel collision
(ride bypasses it), no stop triggers / control lever / sounds, demo pace
(TRAM_STEP_DIV) faster than real.

## M12 -- perf pass + masked transparency (DONE)

Playtest: terrible framerate + transparent geometry rendered as opaque garbage.

- **Subdivision gated** (`SUBDIV_NEAR`): only big AND near triangles take the
  view-space subdivide path; far big tris emit straight from the cache. M10 had
  routed every big tri through it (CPU + 4x fill) -- the main regression.
- **Project only visible verts**: lazy per-frame `proj_vert` cache (`VERT_FRAME`)
  instead of projecting all ~5000 verts every frame.
- **Masked textures** (`{...`): cook maps the transparent key (palette index 255)
  to CLUT slot 0 = 0x0000, which the PS1 GPU skips -- grates/fences/railings are
  see-through (and cheaper) instead of opaque garbage. No runtime change.

fps not measured headlessly (frametest is digital-only; analog-only controls) --
user verifies. Further levers if still slow: fewer/smaller textured prims
(overdraw), distance/fog cull, smaller OT.

## Perf -- measured profiling pass (2.1 -> 6.9 fps on c0a0)

**Measure headlessly**: `frametest --fps` (counts real framebuffer swaps) and
`--profile [--profile-from N] --profile-out f.txt` (PC-sampling histogram). Map
PCs to code by disassembling the `.exe` (flat binary, base 0x80010000, +0x800
header) -- no symbols are kept. This is how the bottlenecks below were found.

Findings + fixes (each fps-verified):
- **Soft path was eating ~half the frame.** I routed every triangle with
  `sz < 32` (and earlier every big triangle) through the view-space near-clip +
  per-call array zeroing. In a tunnel that's most of the scene. Fix: `NEAR=2`
  (only true near-plane crossers go soft), fast-path all in-front on-screen tris
  straight from the cache, static near-clip scratch, and an in-band fast push in
  `emit_cv` (skip `guard_clip` when on-screen). Subdivision off (`SUBDIV_DEPTH=0`).
- **Processing all ~9000 PVS tris regardless of view direction.** Fix: per-leaf
  frustum cull -- the cook stores each leaf's world bounding sphere (centre+radius,
  `LEAF_SZ` 8->16); the runtime skips leaves behind the near plane, beyond
  `FAR_VIEW`, or outside the ~45deg horizontal FOV (conservative 2*r slack).
- **Project only visible verts** (lazy `proj_vert` cache) and **masked textures**
  (prior commits).
- Not fill-bound: emitting zero prims only gained ~0.6 fps. The remaining cost is
  flat per-triangle throughput + `gpu::vsync()` (~12%, the Timer1 frame sync).

Further levers (bigger): cut triangle count (geometry LOD / tighter PVS use),
reduce overdraw, batch RTPT projection, async OT submit (overlap CPU/GPU).

## Next (pick per value)

- **Door collision**: trace the door submodel's clip hull at its current offset.
- **More perf**: the levers above (throughput-bound now).
- **func_tracktrain**: the tram ride (the actual opening of Half-Life).
- **Buttons/triggers**: real targetname-based triggering instead of proximity.
- **UV subdivision + affine correction**: fix tiling/warp on large tris.
- **MDL models / skeletal animation**: the big wall -- NPCs, weapons, viewmodel.

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
