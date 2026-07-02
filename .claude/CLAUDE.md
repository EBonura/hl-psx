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

## M6 -- map selection (DONE; superseded by M15 streaming)

Runtime builds from `data/maps/current.hlm`; `make cook MAP=<name>` writes it.
The full pipeline is map-general -- verified on c1a0 and c1a1a. **M15 replaced the
baked map with runtime CD streaming + an in-game menu.**

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

Then (2nd pass, -> 7.3 fps): vertical FOV leaf cull, and **per-face backface
cull** before any per-tri work -- the cook stores each face's side-adjusted world
plane (`face_plane`), runtime skips a face if `dot(n,eye) <= dist`. Modest in a
tube (most visible faces face you), bigger in open rooms. Emitting zero prims
caps at ~8 fps, so the wall is now in-frustum per-triangle throughput (data
reads + projection), not emit/fill.

Further levers (bigger, diminishing): QUADS (cook-pair fan tris -> halve prim
count + per-tri data work), faster/aligned per-tri data layout, geometry LOD.

## M13 -- brush/door collision (DONE; walking unverified headlessly)

On foot, the player now collides with brush entities, not just the world. Cook
stores each submodel's hull-1 clipnode root (`Ent.head`, `tram_head`). Runtime
builds a per-frame `Mover` list (every brush entity at its current offset -- doors
at their open amount, statics at origin, tram at ride_off) and `phys::trace_all`
traces the world hull plus each mover hull (shifted by -offset) per slide/step/
ground probe, taking the nearest hit. So closed doors and `func_wall`s block you;
the parked tram is solid. Reuses the verified `SV_RecursiveHullCheck` trace.

Can't headlessly verify walking-into-walls (analog-only controls; the frametest
harness is digital). Verified instead: ride + render intact, fps unchanged, build
clean. **User to playtest the actual blocking.**

Deferred -- **walk on the MOVING tram**: attempted (carry player by tram delta +
full physics) but the player fell through (moving-platform gravity desync). The
ride stays locked-carry. Needs proper moving-platform physics (re-seat rider on
the platform floor each frame) -- a focused follow-up.

## M14 -- MDL static models (DONE, pipeline proven)

Half-Life studio models render in-engine. `hl-bsp --mdl <in.mdl> <out.hlmdl>`
(`make models MODEL=<name>`) parses the studio format, builds the bind-pose bone
world matrices (host floats: AngleQuaternion -> matrix, hierarchy concat), bakes
each vertex by its bone into a posed mesh, decodes the tri-strip/fan commands,
and crushes the embedded textures to 4-bit -- emitting a `.hlmdl` with the same
geometry+texture layout as `.hlm`. Runtime: `model.rs` loader + `vram::upload_tex_blob`
(now generic; VRAM gained a 2nd texture band) + `draw_model` (project + textured
emit). Proven with `can.mdl` (a textured soda can renders correctly in the tram).

## M14b -- external textures + entity placement (DONE)

- **External textures**: `cook_mdl` loads `<base>T.mdl` when the main file has
  `numtextures==0` (scientist/barney/gman); uses the texture file's textureindex
  + skin table. `make models MODEL=scientist` -> textured scientist.hlmdl.
- **Placement**: the map cook emits a props section (`.hlm` HLM9, header gains
  `prop_off`): `collect_props` -> (model_type, world origin, yaw) for
  monster_scientist/sitting (type 0) + barney (type 1). Runtime includes
  scientist.hlmdl, and `draw_model`s it at each type-0 prop (×1), frustum-culled
  by forward depth + horizontal FOV. c0a0 = 13 props.

Verified the scientist RENDERS (geometry + bind pose + external textures) via an
isolated capture (clear human figure, white lab coat). NOT captured in-situ: the
placed scientists are scattered deep in levels + occluded by the tram car, and
the analog-only controls can't be driven headlessly to walk up to one -- user
verifies in-game.

## M14c -- model polish (DONE, in-game verify pending)

- **Per-prop yaw**: `draw_model` rotates the model by the entity yaw
  (`mr = view ∘ rotate_y(yaw)`). Facing convention may need an in-game tweak.
- **Shade**: `MODEL_SHADE = 110` (dimmer than 128) to blend with the lit world.
- **Winding**: the MDL cook now reverses tri winding (c,b,a) to match the BSP
  Y/Z-swap convention.
- **Backface cull**: `MODEL_CULL = false` (left OFF). Couldn't verify the winding
  headlessly (debug model kept off-screen/occluded; analog-only blocks walking to
  a placed scientist). Flip `MODEL_CULL` true and check in-game; if scientists go
  invisible/inside-out, the winding reverse needs undoing.
- **Barney** (type 1): parsed but not included/drawn (needs a 2nd model + slot
  range + VRAM check).

Next: animation (runtime skeleton + sequence playback) is the big follow-up.

## M17 -- studio animation system (DONE, verified)

Cook-time **baked vertex frames** (Crash-Bandicoot style) -- no skinning on the
PS1 (no FPU, no heap in render path). The cook decodes the studio animation and
pre-poses the vertices per frame; the runtime just selects a frame.
- **Cook** (`cook_mdl`, `--mdl <in> <out> [seq]`): reads bone metadata
  (parent + value[6] pos/rot defaults + scale[6]); picks sequence `seq` (default
  0, seqgroup 0 only); for each baked frame decodes each bone's 6 DOF channels
  (`mstudioanim` 12 B/bone at animindex+bone*12; offset 0 = use default, else RLE
  via `anim_value`: `[valid:u8][total:u8]` + `valid` i16s), builds the bone matrix
  (euler->quat->concat hierarchy), poses verts. Bakes up to **16 frames** (evenly
  sampled). Output is **HMD2**: `magic | n_verts,n_tris,n_texs,n_frames |
  n_frames x vertset | tris | uvs | tex`.
- **Runtime** (`model.rs`): `Model.n_frames` + `vert(frame, i)`; `draw_model` and
  `draw_viewmodel` take a frame; the loop cycles `frame_no / ANIM_DIV % n_frames`.
- **Verified**: the viewmodel went from splayed (bind pose) to a coherent posed
  pistol (idle), and a 2-frame diff on a static-camera map (c1a0) localized all
  motion to the gun region -> the idle animation cycles. Scientists animate via
  the same path.
- **Viewmodel** ON (`SHOW_VIEWMODEL=true`), v_9mmhandgun. Scanned the MDL:
  **seq 0 = idle1** (61 frames -- its sway is why cycling lurched; we draw static
  **frame 0**), **seq 3 = shoot** (bake this for R2's real fire anim next).
  Textures verified correct (the orange is the brass magazine, not corruption).
  Faithful transform `viewmodel_rot()` = `VM_BASE` (HL forward -> view +Z, up ->
  screen up -Y, left -> screen left -X; det -1 reflection, RH model -> LH view)
  x rotate_y(VM_YAW) x rotate_x(VM_PITCH) x `VM_SCALE`, placed by `VM_OFF`
  (right,down,fwd). Backface cull sign = `VM_CULL_POS`. **Key fix: HL renders
  v_models near 1:1 -- the old 5-6x scale caused extreme foreshortening (huge
  near, tiny far). VM_SCALE ~3 now.** Model/textures/cull are right; the exact
  3/4 angle + corner placement is the last 10% -- can't judge it from static
  headless crops, so VM_YAW/VM_PITCH/VM_SCALE/VM_OFF are left as live knobs.
- **Controls** (analog twin-stick, required): left stick move/strafe, right stick
  look, R2 fires (viewmodel recoil), Select -> menu.
  - **Camera-roll bug FIXED**: `view_rotation` composed `rotY(yaw)*rotX(pitch)`,
    which rolls the horizon when you pitch while turned (the "up/down rotate it"
    complaint). Correct order is **`rotX(pitch)*rotY(yaw)`** (pitch in camera
    space). Verified by hardcoding a turned+pitched camera: walls were tilted ~35
    deg before, level after.
  - **"Player doesn't move" was a collision jam (FIXED)**: the hull trace landed
    the player EXACTLY on the floor plane, leaving them `startsolid` -> `slide_move`
    broke every frame (no walk, no fall, `on_ground=0`). Added Quake's
    **DIST_EPSILON** (`EPS=1`) to `recurse` so impacts stop just SHORT of the
    plane. Diagnosed with on-screen probes: pad LX/LY/RX/RY (input fine), then a
    vertical `point_contents` sweep (floor present), then frame-counter + pos +
    on_ground (player wedged at spawn-12, on_ground=0). After EPS: on_ground=1,
    pos walks. NB headless diffs were misleading -- the game runs ~6 fps, so
    consecutive harness frames are the SAME game frame (0% diff != "not moving").
  - **"Left stick does nothing" was also the tram**: c0a0 locked the player on
    rails (`riding`); `TRAM_RIDE=false` now -> walk everywhere.
  - **"Walks a few steps then stops dead" (FIXED)**: `trace_all` OR'd every
    mover's `startsolid` into the result, so the moment the player stepped inside
    ANY brush-entity hull (a non-solid func_illusionary, or slight penetration)
    `slide_move` broke and froze them forever. Fix: movers no longer contribute
    `startsolid` -- they still block ENTRY via `frac`; only the WORLD hull's
    startsolid counts as truly stuck. Verified: forced-forward walks the full
    484->3 with movers on (same as movers off).
  - **Open perf issue**: gameplay runs ~6 fps (fill-bound) -- makes controls feel
    laggy regardless. Next thing to attack for feel.
  - Re-asserts `enable_analog_port1()` if the pad drops analog; radial deadzone;
    `YAW_RATE=130`/`PITCH_RATE=95`. NB look speed is per-frame -> scales with fps.
- `make models` cooks scientist + v_9mmhandgun (MODELLIST). EXE grew (~16x vert
  data): scientist 40 KB, pistol 67 KB.

## M16 -- HUD + weapon viewmodel (HUD DONE; viewmodel blocked on animation)

- **HUD** (`game/src/hud.rs`): crosshair + health + ammo, drawn as immediate
  prims AFTER `OT.submit()` so they overlay the world. The digits + health cross
  are the REAL HL HUD sprites: `tools/extract_menu.py` decodes `sprites/640hud7.spr`
  (a v2 IDSP sprite sheet; digit rects from hud.txt's `640 640hud7` lines:
  number_d at `(d*24,0,20,24)`, cross at `(80,24,32,32)`) into `data/menu/hud.tex`
  (256x64 4bpp, HEV amber baked into the CLUT, index 0 = transparent). Uploaded
  once to a free gameplay tpage (VRAM X=960, band-1's last page -- maps use ~15 of
  22 pages so it's never clobbered) + CLUT at (960,504); each digit/icon is one
  `draw_quad_textured_material` sampling its sub-rect. Crosshair is still drawn
  (the real crosshair sprite isn't wired). Values static (no combat yet).
  **DO NOT invent assets that exist in the install** -- always extract the real
  one (font/logo/bg/HUD all came from the player's files); see lesson below.
- **Weapon viewmodel** (`draw_viewmodel`, gated off by `SHOW_VIEWMODEL=false`):
  renders a `v_*.mdl` (e.g. `v_9mmhandgun`) in VIEW space (fixed `VM_ROT`+`VM_OFF`,
  on top of the world). BUT the MDL cook poses the **static bind pose**, and HL
  v-models are only correct in their **idle animation frame** -- the bind pose is
  splayed (pistol = jumble, crowbar = invisible). So the viewmodel needs the
  studio animation-pose system (decode seq 0 frame 0: seqdesc->mstudioanim RLE ->
  per-bone Euler -> rebuild bone matrices, in `cook_mdl`). That same system is
  what animates NPCs, so it's the shared next step. Plumbing (extract, upload to
  WEAPON_SLOTS, view-space draw) is in place behind the flag.

## M15 -- main menu + WORLD.PAK map streaming (DONE, verified)

The map is no longer `include_bytes!`'d into the EXE. It streams from the disc's
WORLD.PAK at runtime, picked from an in-game menu.

- **Host pack** (`make rooms` + `make disc`): `MAPLIST` in the Makefile cooks each
  map to `data/rooms/room_<N>.psxc`; mkisopsx `--world-pack-rooms-dir` packs them
  into `WORLD.PAK` at fixed **LBA 1024** (chunk id == N). Verified: pack header
  (magic `PSOXWPAK`, v1) + 24-byte entry table, byte sizes match the room files.
  Keep maps **< ~900 KB** cooked (see RAM note).
- **Runtime CD read** (`game/src/cdstream.rs`): the CD-DMA sector primitives
  (SETLOC 0x02 + READN 0x06, manual DMA ch3 chcr `0x1140_0100`, IRQ-flag polling)
  are ported from PSoXide's editor-playtest `cd_stream/hw.rs`. `load_chunk(id, dst)`
  reads the header sector at LBA 1024, scans the table for `id`, then streams its
  payload sectors into `dst`. Needs `psx-io` (added to Cargo.toml). A ~700 KB
  chunk takes **~155 frames (~2.5 s)** at 2x.
- **Menu** (`game/src/menu.rs`, `psx-font`): styled after Half-Life's original
  (WON) main menu, modelled on a reference screenshot + the install's own assets.
  Dark background, faint drawn lambda watermark, a white wide "HALF-LIFE" wordmark
  across the top with the A drawn as a lambda, a left-aligned chapter list in HL
  menu orange (hovered row brightened), and a grey detail line (map code + a short
  description) under a rule. Chapter names are the real ones (c0a0 Black Mesa
  Inbound, c1a0 Anomalous Materials, c1a1a Unforeseen Consequences, c1a3a We've
  Got Hostiles). NB **HL1 has no chapter/level select** -- "New Game" just starts
  c0a0; this picker is an original screen in HL style.
  - **Follow the source, don't reconstruct.** The WON menu is defined by
    `valve/640_textscheme.txt` "Primary Button Text": **FontName "Arial"**,
    FontSize 16, FgColor `255 170 0` (orange), FgColorArmed `255 255 255`
    (selected = white), BgColorArmed `255 170 0 @67`. The menu now uses those
    exactly. (The fonts.wad qfont is the HUD/console font, NOT the menu font --
    that was a wrong turn.)
  - **Real font**: `tools/extract_menu.py` (`make menu-assets`) rasterizes the
    system **Arial.ttf** (`MENU_FONT` to override) into `data/menu/hlfont.bin`
    (git-ignored) -- `[u8 gw,gh | u16 count,first,pad | advances | 1bpp MSB
    bitmap]`. The runtime `include_bytes!`s it and builds a `psx_font::BitmapFont`
    at boot.
  - **Real logo**: the same extractor converts `resource/logo.tga` (the actual
    HALF-LIFE wordmark, λ for the A) into `data/menu/logo.tex` -- a 224-wide 4bpp
    texture (CLUT index 0 = `0x0000` = transparent, so it composites over the bg).
    Uploaded to a free tpage (VRAM X=640, clear of framebuffers/font atlas) and
    drawn as one `draw_quad_textured_material` quad. So the wordmark is the real
    image, not font-rendered.
  - **Layout** (WON main-menu look, per a reference screenshot): white wide
    "HALF-LIFE" wordmark (A drawn as a lambda) with soft light-streak glows
    behind it; left-aligned chapter list in orange with the first letter
    underlined (the WON mnemonic accent) + a grey description on the same line;
    version string bottom-right. NB at 320px the long names + descriptions only
    fit in the 12px FONT1, not FONT2.
  - **Background**: the WON grungy `gfx/shell/splash.bmp` is NOT in a Steam
    install. The only full-screen art it ships is `gfx/conback.lmp` (the console
    background -- a Quake LMP: u32 w,h | indices | **embedded** 768-byte palette).
    The extractor reads it, desaturates + darkens it, and emits `data/menu/bg.tex`
    (256x240 4bpp), drawn stretched to 320x240 as one textured quad behind
    everything. So the backdrop is the real conback grunge, not procedural. (The
    earlier procedural lambda-circle/green-code/streaks were removed.)
  - **VRAM map (menu)**: framebuffers X<320; font atlas tpage (320,0); logo tpage
    (640,0); bg tpage (704,0); CLUTs at Y=256 under each. All re-uploaded per menu
    entry (gameplay clobbers them).
  - D-pad/left-stick up/down, Cross/Start to confirm. Drawn with GP0 rects + flat
    polys (no OT); NB POLY_F4 needs Z-order verts (TL,TR,BL,BR), not perimeter
    order, or quads draw as bowties. A loading screen is swapped in before
    `play()` streams. Font VRAM (Tpage 320,0 / Clut 320,256) overlaps world
    textures, so the atlas re-uploads each menu entry.
- **Boot flow** (`main.rs`): `loop { sel = menu::run(); play(&sci, sel) }`.
  `play()` streams into a static `MAP_BUF` (235_520 u32 = 920 KB), `Map::load`s it
  in place, uploads textures, runs the renderer/physics, and returns to the menu
  on **Select**. Analog is enabled in `play()` (the menu runs on the digital pad).
- **Spawn fallback** (cook): mid-chapter maps (changelevel targets like c1a3a)
  have no `info_player_start` -> the cook now falls back to the world bbox center
  near the top (gravity drops the player to the floor). Was a black screen before.
- **VRAM reset**: `vram::upload_textures` re-inits the atlas + CLUT allocators so
  reloading a map (return-to-menu) starts clean instead of overflowing.
- **RAM**: EXE dropped 1.3 MB -> 635 KB (map no longer baked). Static footprint
  `__bss_end` = 0x801a6384 => ~327 KB free below the stack. Fits with margin.
- **Verified headlessly** (frametest `--script`): menu renders, nav (early+late),
  select all 4 chunks (c0a0/c1a0/c1a1a/c1a3a each render, pixel-distinct), LOADING
  screen, and a full round trip play -> Select -> menu -> pick another -> play
  (2nd stream + allocator reset OK). NB: streaming is ~155 frames, so give late
  selections >200 capture frames or they look "stuck" on the menu mid-load.

Menu maps are curated in two places that MUST stay in sync: Makefile `MAPLIST`
(chunk order) and `menu::MAPS` (labels, index == chunk id).

## M18 -- missing-geometry ROOT CAUSE + NPC placement + corpses (DONE, verified)

- **The map-wide view-invariant missing geometry** (floors vanishing, reported
  across levels) was a cook-writer vs runtime-reader **alignment mismatch**: the
  cook 4-aligns before the marksurface array; `map.rs` computed `marks_off`
  unaligned. On parity-2 maps every leaf's mark window shifted one entry (lost
  its last face); with odd n_marks the vis offset shifted too (garbage PVS
  rows). One-line reader fix (`align4`) heals all cooked rooms without
  recooking. NB the prior session's "arena overflow + banding" theory was a
  misdiagnosis (its green=0.000 checks were actually black screens); the
  adaptive banding it added is kept (harmless, real overflow safety).
  Debug method: host python replicating camera_leaf/vis/marks on the cooked
  file, diffed against the source BSP per leaf.
- **Headless testing is now PSoXide-native**: `make psoxide-map-smoke
  MAP_INDEX=N` boots any room directly (feature `debug-map-boot`, pad mask
  0x400|N) and dumps display/profile/counter captures. frametest is retired.
- **NPC placement**: `monster_sitting_scientist` is model type 25 (own baked
  sit pose from scientist.mdl seqs 89/73/39) and keeps its authored seat height
  (no ground snap -- chairs are brush entities the world-tree probe can't see).
  AI walkers refuse steps with no floor under them (HL CheckLocalMove), so
  chasers stop hovering off ledges (flying controller exempt).
- **Authored corpses**: `monster_*_dead` cooks as live type | 0x8000; runtime
  spawns them dead (death clip final frame). 54 corpses dress chapters 1-3.

## M19 -- sound (DONE, verified headlessly)

- `tools/extract_sfx.py` (local-only, like the other extractors) cooks 22 core
  HL sounds (own install) to SPU-ADPCM via PSoXide's `psxed audio-pack`, packed
  as WORLD.PAK chunk 3000 ("HSFX" header). `make sfx-assets`.
- `game/src/sfx.rs`: boot-time stream (stages through MAP_BUF pre-menu) ->
  SPU RAM upload (~295 KB of 512 KB; zero main-RAM afterwards); 16 rotating
  one-shot voices; `play` (local) / `play_world` (distance-attenuated vs the
  per-frame `set_ear`).
- Wired: per-weapon fire (crowbar hit/miss), world-impact ricochet, explosions,
  player pain (rate-limited), enemy attacks (grunt/turret MP5, vort zap, crab
  shriek, zombie swipe, houndeye blast, barney glock), death thud, door
  start/stop, buttons, suit/battery pickups, menu confirm.
- Verified via `frontend launch --dump-audio`: the mixed SPU WAV shows the
  glock shots exactly at scripted R2 presses. Chunk id space now: rooms 2N/2N+1,
  viewmodels geom 1000+wm / tex 2000+wm, enemies geom 1300+id / tex 1100+id,
  SFX 3000.

## M20 -- progression systems (DONE)

Everything a start-to-finish run needs, in one pass:

- **Ladders**: func_ladder cooks as ent kind 4 (invisible AABB, world
  half-extents in `mv`); overlapping it switches the player to
  `Player::update_climb` (gravity off; look up + forward climbs, look down
  descends, jump lets go). Kind 4 is excluded from draw/PVS/movers.
- **Movers report identity**: `phys::Mover` carries its ent id; traces return
  `mover` (RayHit too); the player tracks `ground_mover`. That enables:
- **Ride-carry**: a mover you stood on last tick that shifted (door phase)
  carries you by its delta (`ENT_PREV_OFF`) -- plats/elevators are ridable.
- **func_plat**: cooked as a door-machinery mover (authored at top, mv =
  straight down by `height` or size-8; untargeted = touch-activated).
- **func_breakable / func_pushable**: LOGIC_FUNC_BREAKABLE (health arg0,
  material arg1); hitscans and explosions hitting the brush damage it; at 0 HP
  the ent deactivates, fires its targets, and plays glass vs wood break.
  SF trigger-only respected.
- **trigger_teleport** (dest resolved at cook into 2 aux entries, applied
  mid-tick before the render eye), **trigger_push** (per-tick world vector in
  aux; vertical adds velocity, lateral nudges position), **trigger_gravity**
  (q12 scale -> `phys::set_gravity_scale`, reset each map load).
- **Chargers**: func_healthcharger / func_recharge are use-aimables; juice in
  LOGIC_COUNTER drains 4/pulse into health/armor with the medshot sound.
- **Drivable tracktrain**: standing on the train + use toggles it at the
  authored speed (On A Rail); triggers can still command it.

## M21 -- faithful pickups + RAM fit for every roster (DONE)

- **weapon_*/ammo_*/item_healthkit pickups** cook as prop types 26..48 with
  real single-frame w_* models. Touch = give weapon (dupes -> one magazine),
  capped ammo, or +15 health. `give_full_arsenal` is GONE: fresh starts are
  crowbar+glock; **changelevel carries the whole arsenal** (owned mask, clips,
  pools, selection) via CARRY_* statics; menu launches reset it.
- **RAM audit (all 96 maps)**: MODEL_BUF +32 KB; actors bake 4 frames/clip,
  render-only bosses 1 (statues); 22 resident type slots; POOL_FACE_CAP 5248.
  `stream_map_models` is two-pass: combat/interactive types load before
  AI_IDLE decoratives, so overflow drops a statue, never a fighting enemy.
  Verified: every combat/ally/pickup model resident on all 96 maps (only the
  c1a2b/c2a1 gman cameo + c4a3 background garg/icky statues drop). All maps
  fit MAP_BUF (96/96); static headroom 97.3 KiB (min 96); the VM reserve
  (182 K) is smaller than all-14 viewmodels (207 K) -- late-game switches can
  fall back to the glock visual, graceful.
- **Perf re-verified** post-systems: c1a0 direct boot ~18 fps (20 Hz pace with
  occasional misses), c2a5 spawn at pace. NB `--dump-hw` replays a
  capacity-truncated cmd log -- its display + GP1 05h swap trail are NOT
  whole-run evidence (cost a false "freeze" diagnosis); use a tty heartbeat +
  `--guest-debug-log` for liveness.

## Next (pick per value)

- **Real water transparency** (needs the underwater scene drawn behind the
  surface; blend alone renders near-black -- see M-notes above).
- **More perf on fill-bound open maps** (c2a5-class): geometry LOD or
  resolution tricks; CPU side is done to ~20fps.
- **c4a3 chapter spawn** lands in a near-black pocket (campaign arrives via
  teleport; brightness-aware spawn scoring is the noted fix).
- **More SFX polish**: footsteps, weapon reloads, ambient loops (voices 16-23
  reserved), fvox/HEV voice.
- **Scripted sequences / monster spawners** (monstermaker, scripted_sequence)
  for set pieces; func_rotating visuals.
- **VM pool**: 182 K reserve < 207 K all-14 viewmodels; either +25 K or accept
  the glock-visual fallback late-game.

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
