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

## M22 -- deep RAM + perf pass (DONE)

- **Dead-tri repack**: the enemy pool draw reads topology from POOL_FACES
  (baked once) and only verts/clips from the blob, so `stream_map_models`
  keeps just each chunk's frame section (TriRec tail = 30% of an actor chunk,
  half of a boss chunk). draw_model's face bound now comes from the baked
  count (the old `.min(md.n_tris)` clamp read the zeroed header and drew
  nothing -- visual verify caught it). Freed bytes restored **6-frame actor
  animation** and moved 20 KB into POOL_FACES (6400 tris): every model incl.
  statues resident on 95/96 maps (c4a3 background garg alone over the tri cap).
- **PSoXide `--pc-sample`** (new, committed to PSoXide main): guest-PC
  histogram CSV, `--pc-sample-every` stride + `--pc-sample-from` offset;
  resolve against the linker map (build with `-Clink-arg=-Map`). No telemetry
  MMIO observer effect (the stage-CSV numbers are inflated ~2-3x by it).
- **Wins from the first profile** (c1a0, 4.1M samples): light palette
  pre-expanded at load (tri decode 19.9% -> 13.9%); logic scans kind-gated via
  a load-time cache + idle-skip (Map::logic off the profile); actor gates
  reordered (header parse after culls, PROP_LEAF for headcrabs, occlusion rays
  on a 4-tick stagger). ~8% CPU freed on the c1a0 scene, pixel-identical.
- **Profile shape after**: play 32% (includes the vsync idle spin),
  try_emit_tri_pair_quad_values 15.5%, loop/render_tri 14%, emits ~14%,
  face_plane 3.6%, ModelFrame::vert 3.3%. GTE is 1.8% -- the wall is data
  movement, not math.

## M23 -- per-face translucency, water sway, glass (DONE, verified)

- **Watervis ground truth**: HL1 maps mostly ship WITHOUT watervis, so real
  GoldSrc renders water opaque from above (the earlier all-or-nothing blend
  was chasing non-existent behaviour). The cook probes each liquid face
  (leaf above vs below + vis row) and keeps flags bit1 only where the far
  side is visible -- 619 faces (c1a4d/f blast pit, c2a4d/f toxic pools,
  c2a1a, c3a2b...). Everything else demotes to opaque.
- **Runtime**: EMIT_BLEND/EMIT_WAVE per-face statics set by the world/entity
  walkers; blended packet built on demand (one-entry BLEND_PACKET cache;
  TexSlot stays 1 packet -- 3 packets/slot cost 27 KB and broke headroom).
  Liquid UVs sway via WAVE_TAB (128-frame cycle; GP0-E2 windows wrap the
  byte adds safely). Verified: phase-shifted capture diff localizes on the
  waterfall faces.
- **Glass/glow**: ent kind high byte = blend class from HL rendermode
  (2/3 low-amt -> Average, 5 -> Add), parsed for all brush ents (845 across
  the campaign). Ent.blend on the runtime side (kind masks to low byte).
  Verified: c1a0 monitor-room window shows the room through the pane.
- **Perf follow-through (M22 targets)**: loop faces now decode each vertex
  once (fused Map::loop_vert + rolling fan window; loop_render_tri vanished
  from the PC profile; i32 bowtie cross products). Scratchpad investigated
  and SKIPPED: PSoXide charges flat memory timing (BIAS=2 + 1/access, no
  region penalty), so scratchpad moves measure zero in the target emulator.

## M24 -- ghost doors, monstermaker, suitless start, SFX batch 2 (DONE)

- **GHOST-DOOR ROOT CAUSE (the big one)**: every cooked map's world leaf
  MARKSURFACES included SUBMODEL faces (728 refs in c0a0; 113k campaign-wide;
  GoldSrc compilers leave them in, the real engine filters at draw time). So
  every brush entity rendered TWICE -- once static at base position via the PVS
  world walk (the "ghost"), once animated via the entity path. Doors visibly
  "duplicated" when opening and you walked through the static copy (collision
  always followed the real hull). Cook now keeps only model-0 faces in the
  marks (world_first..world_end filter in the leaf-mark compaction). Also a
  free perf win: brush-ent faces are no longer double-emitted every frame.
  Verified: 0 leaking marks on all 96 maps; static views pixel-identical
  (rest-position ghosts overlapped exactly); c1a1a locker banks (all doors)
  render intact via the ent path alone.
- **Door linking**: untargeted touching double-door halves open together
  (GoldSrc linked-door behaviour). Cook union-finds untargeted func_door AABBs
  (2u slack) into arg0 groups; runtime activates the whole group on touch/use.
  NB most HL doors are TARGETED (623 doors, only 40 untargeted) so this is a
  small fix -- the ghost fix above was the real "doors are weird" bug.
- **Monstermaker v1**: cook emits up to 4 DORMANT prop copies (type bit
  0x4000) per monstermaker at its origin + a LOGIC_MONSTERMAKER(22) rec; each
  fire wakes one dormant prop near the maker origin (ground-snapped). 400
  dormant spawns across 46 maps (grunt reinforcements, slave teleport-ins,
  test-chamber cascade). PROP_TYPE_MASK=0x3FFF everywhere kinds are read.
- **Suitless start**: Arsenal::new() = empty hands; crowbar+glock loadout only
  when launching with the suit (chapter select / post-suit changelevel).
  try_fire owns-gates, viewmodel + crosshair render only when armed (HUD
  draw gained has_weapon). Faithful: c0a0/c1a0 spawn bare-handed, HEV+weapon
  pickups now mean something.
- **Hitscan point traces**: Mover gained head0 (hull-0 root); trace_line and
  line_clear_movers use the EXACT hull vs movers (bullets no longer stop ~16u
  short of crates/doors); player movement keeps the inflated hull-1.
- **SFX batch 2** (43 samples, 345 KB SPU): footsteps (input-cadence,
  alternating), reload, dry-fire click, per-class monster pain/death vocals
  (crab/zombie/grunt/barney/hound/slave/squid; scientists stay silent rather
  than borrow a wrong species), HEV pickup bell. KEY: the extractor now cooks
  at each source's NATIVE rate (HL voices are 11025 Hz; upsampling to 22050
  doubled SPU cost for nothing -- the runtime honours per-sample rates).
- **GTE audit conclusion**: draw_model + draw_viewmodel already batch RTPT
  via project_triangle_scheduled; the world path's lazy per-vert RTPS cache is
  the right design (most verts shared across faces). GTE = 1.8% of the frame;
  data movement is the wall. No further GTE headroom worth chasing.
- Pool rebalance: MODEL_POOL_WORDS 99584 (headroom 101.1 KiB). Roster audit:
  24 maps drop tail pickup/background types under the 206 K enemy region (22
  already dropped before this pass; combat types unaffected).

## Systems inventory (final pass, M30)

Present: doors (+linking), buttons, plats, trains (tracktrain ride + use),
breakables, teleports, push, gravity, chargers, multi_manager, triggers
(once/multiple/hurt/changelevel), pickups (weapons/ammo/medkit/suit/battery),
monstermaker, ladders, water/glass blend, full weapon set + select-icon HUD,
enemy AI (melee/ranged/turret/ally/flee + squad alert), footstep/voice SFX,
scripted_sequence (spawn marks + triggered move-to), func_rotating fans,
anim interpolation, muzzle flash, intro tram ride, ending card.
Absent, with reasons (the honest close of the feature matrix):
- **scripted custom ANIMATIONS**: scripts move/teleport + face + chain-fire,
  but play generic idle/walk clips -- per-script sequence baking needs new
  chunks per (monster, sequence) pair; RAM/pipeline cost unscoped.
- **func_train** (brush platforms on path_corners): needs per-train path
  state + the tram's carry logic generalised; the campaign's mandatory
  rides are func_tracktrain/func_plat (done); func_trains are mostly decor
  or crushers.
- **ambient_generic loops**: SPU has ~130 KB free, but seamless loops need
  ADPCM loop flags psxed audio-pack does not emit; a retrigger hack seams
  audibly. Blocked on the cooker.
- **momentary_* (valve wheels)**: niche input rig (hold-to-rotate);
  affected doors open via their targets in practice.
- **env_render/env_glow/gibs**: cosmetic sprite/render-mode tweaks.
- **save/load**: memory-card infrastructure; changelevel carry covers a
  session run.
- **flashlight**: per-vertex dynamic light in the emit hot path -- the CPU
  frontier this port already sits on; suit power UI also missing.

## M25 -- CPU perf close-out (DONE; every measured scene paces at 20 fps)

Post-ghost-fix pc-sample round (pure build, c1a0, method in M22):
- **Per-frame stack zeroing killed**: the movers array memset ~7.7 KB/frame
  (play 32.8 -> 28.9%); emit_projected's inlined guard-clip buffer memset
  256 B in the PROLOGUE of every soft-path call (even when the in-band fast
  path returned early) + guard_clip zeroed 2x256 B ping-pong buffers per
  call. All statics now. LESSON: grep hot fns for `let mut x = [ZERO; N]` --
  LLVM hoists the zeroing to the prologue where it taxes every call path.
- **guard_clip skips untouched edges**: bounds once, then only the crossed
  band edges clip (was: 4 unconditional full copy passes). clip_edge
  4.8 -> 2.8%.
- **Per-tri vert bounds checks dropped**: the cook now asserts every cooked
  corner < n_verts (raw tris, loop faceverts, model tris) and rejects maps
  over MAX_VERTS; the runtime walks trust it. Implicit array bounds vs the
  static scratch sizes remain the fail-safe.
- **Quad-safe cook flag idea RETIRED**: the old 15.5% try_emit_tri_pair_
  quad_values figure was mostly fused decode; the pairing compares are
  already inlined-cheap, and the bowtie test is view-dependent (a planar
  convex quad CAN project to a bowtie near-grazing) so it must stay.
- **Verified paced**: c1a0, c1a1a, c2a4c, c2a5 spawn views all hold 20.00
  fps (counter-log tail). Remaining dips = combat transients + GPU fill.
  NB the counter CSV skips rows -- diff guest_frame BETWEEN rows or fake
  "spikes" appear (a 4.4 M "frame" was 2-3 paced frames).
- Profile shape now: play ~31% (pacer/DMA spin ~6.5% inside it),
  quad_corners 13.7%, face_loop 9.7%, face_tris 7.8%, render_tri 5.7%,
  soft path ~8%, ModelFrame::vert 3.9%, fog ~3.4%, GTE still ~2%.

## M26 -- non-CPU audit + WORLD.PAK compression (DONE)

"Squeeze the non-CPU side" round; conclusions are evidence, not guesses:
- **PSoXide charges NO cost for polygon rasterization** (busy credit only on
  VRAM copies/fills; see gpu.rs charge_busy). The DMA chain completes before
  the game even polls (DMA-wait spin = 0.00% of PC samples; the 7.5% "spin"
  cluster is pure 20 Hz vsync pacing idle). There is NO GPU/DMA frame cost
  to reclaim in-target -- "c2a5 is GPU-fill-bound" from earlier notes is
  OBSOLETE. Frame perf work ends at the CPU (M25).
- **CD loads**: sector cadence is CD_READ_TIME/2 = 225,792 cycles at 2x, but
  the emulator delivers faster than cadence under load (world chunk: 45M
  cycles for 350 sectors ~= 129k/sector). Load cost splits roughly: tex 27M
  + viewmodels 21M + world 45M + ~30-40 model chunks (~90M, dominated by
  per-chunk PAUSE ~1M + READN respin ~0.45M) + VRAM upload ~5M.
- **WORLD.PAK room chunks are LZ4-compressed** (mkisopsx
  --world-pack-compress-rooms; HLZC | raw_len | block). data/rooms stays RAW
  (build.rs + audits parse those). Runtime: read to MAP_BUF head, shift to
  tail, decode tail->head in place (build.rs pads MAP_WORDS +4 KB margin).
  Disc 62.5 -> 51.5 MB, ~2.2:1 on geometry chunks, pixel-identical, load
  time in-target UNCHANGED (saved sectors ~= decode cost under the lenient
  delivery); real wins appear on any faithful-cadence backend.
- **Open-READN chunk batching REVERTED**: keeping the read session across
  model chunks (skip per-chunk PAUSE) made loads 3.4x SLOWER -- after a
  chunk's last consumed sector the drive keeps delivering, and recovering
  alignment mid-session fights the FIFO model. The safe lever is FEWER
  chunks: merge each type's geom+tex chunk, or cook per-map bundles -- but
  mind the transient fit (geom full + tex must fit the pool tail during
  load; the resident audit does not model that).

## M27 -- stable-20 pass: walker-map collapse fixed + instruction-level decode (DONE)

Method that found it: per-map HEARTBEAT SWEEP (temp tty print every 64 frames,
pure build, same step budget -> comparable per-map frame counts) + lockstep
pc-sample. LESSON: rebuild the -Map exe and the disc in the SAME step or the
profile attributes to garbage symbols (bit twice).

- **Walker-heavy maps collapsed to 2 fps** (c1a2 office complex; c1a2b, c2a3b
  similar): NPC floor probes were 75-87% of the frame -- 70 props x 169 brush
  ents x ~27 probe points, each point walking the BSP AND sphere-testing every
  entity. Fixes, all in main.rs prop code: per-prop nearby-ent shortlist
  (8 slots, 8-tick stagger refresh, PROP_NEAR_SLACK 96 covers door travel);
  world floor = ONE hull-0 trace_line; ent scan only over the shortlist;
  walkers move on ALTERNATING ticks with doubled step (HL thinks at 10 Hz);
  blocked walkers back off 6 ticks (wedged crowds re-probed every tick);
  try_step floor-probes only the winning direction (sight line first);
  prop_set_pos_grounded skips the redundant re-ground after try_step.
  Sweep of 14 maps: ALL 14-18 hb (pace = 17). c1a2 2->14, c1a2b 4->16.
- **Aligned TriRec decode**: cook 4-aligns the tri array (reader mirrors --
  format change, recook needed); decode = four u32 loads (was ~14 byte loads
  + 4 per-field bounds checks). Light palette = u32/entry, one load/corner.
  render_tri disappeared from the profile; -14M cycles full-run on c1a0.
- **fog1**: branch on depth BEFORE unpacking (identity near, black far; the
  far path multiplied by zero).
- **Proj-token merge**: VERT_FRAME + SUBMODEL_VERT_TOKEN were two 24 KB
  arrays for disjoint phases; one PROJ_TOKEN space serves both (frame start
  takes a token, each submodel draw takes the next). -24 KB static.
  OT_LEN 512->128 (max real otz = FAR_VIEW>>4 + backdrop bias = 66).
- Headroom 108 KiB with POOL_FACE_CAP 6352 + pool 99,584 restored (a probe
  at POOL_FACE_CAP 5472 would have dropped models on 52 maps -- pickups grew
  per-map tri sums; the audit gates that trade now).
- SDK audit this round: packet ctors/OT insert are already minimal; the
  wins were all game-side data layout + algorithmic. GTE still ~2%.
- c1a2's black debug-boot view is the FAITHFUL blacked-out office intro
  corridor (game live at 14 hb; profile shows normal AI) -- not a hang.

## M28 -- absolute-limit pass: full-campaign sweep + the last probe fix (DONE)

First COMPLETE 96-map measurement (4-lane parallel heartbeat sweep, pure
builds, fixed step budget): 82/96 maps pace at 20 fps.

- **Office-class fix (final)**: the ent-column floor scan still walked ~2000
  brush-ent subtrees/frame (measured with temp call counters -- point_in_
  one_ent walks the ent's NODE tree, which is what the profile's "Map::node"
  actually was). Per-ent ANCHORED BISECTION (3 anchors in the ent's own
  vertical extent + boundary bisect) replaces the 30-point linear scan:
  ~360 walks/frame. c1a2 ended at ~16.5 fps -- it started this arc at 2.
- **Open-vista class = the faithful-geometry limit**: 13 maps (c2a5x, c3a1x,
  c4a1x spawns...) sit at 14-16 fps, emit-bound (quad packet build 18%,
  loop walker 13%, vblank quantization idle 12%). Their PVS FITS the packet
  arena (banding inactive), so the cost is per-triangle throughput on real
  geometry. The next lever is geometry LOD = a visual trade -- DECLINED to
  keep the faithful ethos. This is the honest PS1 limit for this renderer.
- **Model::load cache**: draw paths re-parsed chunk headers per prop draw
  (2.7% on prop-heavy frames); LOADED_MODEL_CACHE / VM_MODEL_CACHE hold the
  parsed struct per slot (Model is Copy; EMPTY const for statics).
- **Band-order bucketing**: overflow views counting-sort PVS faces into a
  near-to-far order once (PVS_BAND_ORDER, cap 2560 faces) instead of
  re-walking the face links per depth band; bigger views keep the old walk.
  Engages on huge-room + combat overflow scenes, not the vista class.
- Headroom 96.9 KiB; pool kept at 99,584 words (the roster audit CLIFFS
  below it: 99,072 would drop types on 52 maps -- always audit both caps).
- GOTCHA: `grep -c "^hb"` counted the "hb leaf=..." diag lines too -- keep
  heartbeat output byte-identical between sweeps or normalize before
  comparing.

## M29 -- finalise: interp animation, squad AI, weapons polish, ending (DONE)

- **Animation interpolation**: prop_anim_frame returns (frame, next, frac16);
  draw_model lerps vertices between the two baked frames -- smoothness back
  at CPU cost (no pool bytes), all measured maps still pace (17/16/16 hb on
  c1a0/c1a2/c2a3b). Attack clips (div=1) play raw. Restoring the CUT frames
  themselves was measured out: actors would need ~3x pool.
- **AI**: damage aggros combat AI + wakes same-species squadmates within
  400u (SQUAD_ALERT_RADIUS2). Scientists flee/barney ally/melee/ranged/
  turret audited fine as-is.
- **Weapons**: DEBUG_ALL_WEAPONS const (ships false) = full arsenal + 250
  of every pool for testing; additive muzzle-flash star (4 spikes + core,
  Add blend, first 2 recoil ticks, gun-class only via MUZZLE_FLASH_WEAPONS).
  Viewmodel NORMALS remain dropped -- HMD6 costs ~4B/tri x 14 resident
  viewmodels and the VM pool is the tightest region (documented trade).
  NB the debug-boot pad mask's L1 bit leaks into gameplay input during the
  180-frame hold and cycles weapons -- harness quirk, not a game bug.
- **scripted_sequence v1**: cook repositions a monster to its AUTO-START
  script mark (no-targetname scripts; 14 marks / 9 maps) -- how real HL
  poses intro actors from frame one. Triggered scripts (403 of 417) still
  absent (need move-to + per-sequence bakes).
- **Intro/outro**: c0a0 tram ride verified working headlessly (camera
  tracks the path, pauses at the station signal). Outro: c5a1 plays ~70s
  (ENDING_SCENE_TICKS 1400), fades white, menu::ending shows the end card
  (wordmark + THE END + credits), any button returns to the menu.
- Headroom 96.1 KiB: MAX_RENDER_PACKETS 2560/2304 -> 2432/2176 funds the
  new statics (emitted prims peak ~1800; banding is bucketed + cheap now).

## M30 -- weapon-select HUD, scripted move-to, rotating fans (DONE)

- **Weapon HUD complete**: all 14 real 320-res `weapon_s` select sprites in
  the HUD atlas (80x20, two per row from v=48, W_* order); L1/R1 flashes
  the icon top-right for 30 ticks (verified via a pinned-frame capture).
  hud.tex (160x188) now streams from WORLD.PAK chunk 3001 at play() start
  (MAP_BUF is free there) -- include_bytes cost 15 KB of static RAM.
- **scripted_sequence v2**: PropRec 24 B (targetname as a logic-name id,
  interned by the SAME LogicNames pass -- cook order: logic THEN props).
  Kind-24 recs carry m_iszEntity (arg0), m_flMoveTo (arg1), yaw (speed
  field). Firing sends the prop walking (4/tick) / running (8/tick) /
  teleporting to the mark; arrival faces the yaw + fires the script's
  target chain (SIM_NOW). Generic clips play during -- custom-anim baking
  is the documented residual.
- **func_rotating**: ent kind 5; mv[0] = q12 angle/tick (deg/s x 4096/360
  / 20), SF2 reverses, only Z-axis fans animate. Draw composes
  view x rotate_y(angle) about the PIVOT (ent origin; ent_draw_offset
  returns zero for kind 5 -- origin is NOT a translation). Plane/bounds
  gates skipped for rotating faces (normals rotate); the ent sphere cull
  stands. Collision hull stays at the authored pose.
- Headroom 96.8 KiB (hud.tex chunked -15 KB, PVS_BAND_CAP 1408, script
  goals i16). GOTCHA: pad-pulse frames tick at GAME rate (~20 Hz), not
  60 -- timing captures around short UI windows needs pinned-state builds.

## M31 -- bring it home: the campaign-completeness round (DONE)

Driven by a full entity-classname audit (every class in every cooked map vs
the cook's handled set). SP map coverage was already 96/96 (only MP maps
uncooked). What landed:

- **Swimming** (func_water x94): kind-6 volumes (half-extents in mv, still
  rendered + non-solid); phys::update_swim = look-direction move, jump
  paddles up, idle sinks. Deep water was a SOFTLOCK before.
- **func_train x243**: corner chains packed as logic aux pairs ((x,y) +
  (z,wait) per path_corner, cap 24); per-train state (MAX_TRAINS 12);
  trains spawn AT their first corner; corner waits honored; targeted
  trains toggle on fire, untargeted auto-run. ent_draw_offset override via
  ENT_TRAIN_SLOT; ride-carry works through the generic ENT_PREV_OFF delta.
- **func_conveyor x87** cooks as an always-on LOGIC_TRIGGER_PUSH volume
  (belt movedir x speed/2).
- **player_weaponstrip** (kind 26) empties the arsenal (Apprehension).
- **item_longjump** (type 49) + phys longjump: moving jumps launch 2.5x
  horizontal (Xen crossings). Reset on fresh starts, carried by statics.
- **killtarget kills NAMED PROPS** too (PROP_NAME match -> deactivate);
  the Blast Pit rocket now removes the tentacles (type 50, render-only in
  the silo). **monster_human_assassin** (51) = fast AI_RANGED shooter.
- **Roster = ZERO combat drops on 96/96 maps.** The unlock chain:
  interpolation justified 4-frame clips (pain 2, death 3, humanoid attacks
  3); VM reserve 46,720 -> 35,328 words (~10 switched guns resident, then
  glock-visual fallback); streaming is 3-TIER (combat > pickups/items >
  decoratives). Only statues/cameos trim on 5 heavy maps (c1a2b/c2a1 gman,
  c4a1b garg+barnacle, c4a3 statues, c3a2d w_hgun dupe).
- **Final sweep: all 96 maps boot, render, run** (parallel heartbeat
  sweep; 94 at pace, c2a5e + c3a2d at ~15 fps -- the known vista class).
- Audit tooling: model_pool_audit must mask PROP_TYPE (0x3FFF -- dormant
  bit!), stride 24, VM 35,328, 3-tier order. Absent still: momentary_*,
  env_* sprites, gibs, ambient loops (ADPCM loop flags), save/load,
  flashlight, scripted custom anims, func_tank -- none block progression.

## M32 -- compression-for-speed: merged model chunks + LZ4 boot chunks (DONE)

- **One HMRG chunk per model** ("HMRG" | u32 geom_len | geom | tex): the
  map load does ONE CD handshake per type (was two -- PAUSE ~1M + READN
  ~0.45M each dominated small chunks), and a weapon switch streams one
  chunk. Textures upload to VRAM from their in-chunk tail (no staging
  move); geometry starts +8 bytes (word-aligned); dead-tri repack
  unchanged. tools/merge_model_chunk.py (committed -- pure byte shuffler,
  no asset data) runs in `make models`.
- **LZ4 extras >= 3000** (SFX pack + HUD atlas) at pack time -- they stage
  through MAP_BUF where decompress_in_place already runs. Model chunks
  (1000-2000) stay raw (no in-place margin where they land).
- **Measured (c1a0 telemetry markers)**: boot+load 237.0M -> 208.2M bus
  cycles = 7.00s -> 6.13s (-12%); viewmodel stage 20.8M -> 14.0M.
- Verified: scientists + glock textured from merged chunks; roster audit
  ZERO combat drops (model_pool_audit reads the HMRG geom length -- and
  counts geometry bytes only, tex goes to VRAM); headroom 96.8 KiB.
- In-frame compression audit: TriRec/faceverts/frames/vis are already at
  their packed formats (aligned decode, i8-delta frames, RLE vis); the
  remaining CD lever is per-map bundles (all of a map's types in one
  chunk), which duplicates storage across maps -- disc has room, noted.

## M33 -- black-triangle ROOT CAUSE: GTE near-quotient saturation (DONE, verified)

The persistent "black wedges at the feet / walls vanish up close" (survived
the c7daf19 cull fix) was TWO stacked bugs, the big one in the GTE itself:

- **GTE RTPS quotient saturation**: the perspective divide H/SZ3 caps at ~2.0
  (UNR result limit 0x1FFFF). Any vertex closer than H/2 = **80 world units**
  projects SHRUNKEN toward the screen centre -- silently wrong screen coords
  for the entire 16..80 depth band. Near floor/wall faces rendered displaced,
  leaving their true screen area black (the wedges ARE that abandoned area).
  Fix (`project_vert_fixed`): when `NEAR <= sz < 80`, recompute sx/sy with a
  software reciprocal `(H<<12)/z` under `FIX_ROT`/`FIX_T` (mirrors of the live
  GTE transform, set beside every `load_rotation`/`load_translation` that
  feeds `proj_vert`/`proj_submodel_vert`), saturated to the GTE's +-1023 so
  `clamped()` routing is unchanged. `draw_model` is excluded on purpose: its
  xS vertex scaling already shrinks the saturation zone (~20 units at s=4)
  and the viewmodel is hand-tuned around the current projection.
- **Soft-path cull quantization**: c7daf19's `>>4` pre-scale (i32-overflow
  fix) collapsed the two on-screen verts of thin near-plane slivers into the
  same cell -> cross == 0 -> culled. Those slivers tile the floor strip at
  the player's feet. Now `culled_soft` = exact i64 cross (one MIPS `mult`;
  only i64 DIVISION is the known miscompile).
- Verified at the repro pose (c1a0 reception desk, DBG_CAM `[-90,-190,140]`
  yaw 0 pitch -200): bottom-strip black pixels **1737 -> 11**; the traced tri
  projects (330,356) in-runtime = host-replica exact. Debug method that
  cracked it: paint-don't-drop colors (NB textured tints can be invisible on
  dark texels -- add fixed-position counter squares), a host-side replica
  render of the cooked file, then per-tri tty traces of the emit chain, where
  the GTE's (257,255) vs true (330,356) exposed the x0.571 shrink instantly.

## M34 -- faithfulness round: intro, train transfer, scripted anims, music, voices (DONE)

- **Ride transfer**: RoomLaunch.riding (set from tram_active && attached at the
  changelevel) re-seats the player on the new map's tram at waypoint 0 and
  resumes the ride -- the Black Mesa Inbound chain (c0a0..c0a0e) is continuous.
  c0a0a-style trains are authored parked out-of-world (GoldSrc globalname
  transfer); startspeed already auto-runs them, placement was the missing half.
- **Intro presentation**: titles.txt parsed at cook (stateful $directives);
  env_message -> LOGIC_ENV_MESSAGE 27 (arg0 = text interned in the names pool,
  arg1 = effect|low_left|fade<<8, speed = hold); env_fade -> 28; worldspawn ->
  LOGIC_MAP_FLAGS 29 (startdark, gametitle bit noted-undrawn, chaptertitle text,
  music track bits 8..13). Runtime: tick_screen_fx/draw_screen_fx -- hltext
  overlays (typewriter effect 2, credits lower-left) + Subtract/Add blended
  fullscreen fade (startdark holds black until env_fade). Verified: CR27
  scan-out + Ted Backman credit on the c0a0 ride; chapter cards auto-fire ~1.5s
  after load.
- **Scripted animations**: seqspec entries accept sequence NAMES (cook_mdl
  resolves labels; alias tokens a=b / a=@slot feed tools/gen_clips_manifest.py
  -> data/modelpack/clips.txt); the map cook (CLIPS_MANIFEST env) resolves
  m_iszPlay/m_iszIdle to clip slots in a script aux pair; runtime plays a ~2s
  gesture window (play) or an AI-SUSPENDING idle pose until damage clears it.
  Auto-start scripts (no targetname) fire once at load. Verified: the c1a0 desk
  barney SITS (sit1). Baked: sci pondering/retina/beatdoor, barney sit1/
  standing_idle/intropush, zombie eatbody, islave grab, squid eat (+aliases).
  RAM: VM reserve 35,328 -> 24,192 words (~7 switched guns) funds it; roster
  audit ZERO combat/pickup drops (decor-only trims on 3 maps).
- **CD music**: tools/extract_music.py (media mp3 -> 44.1k raw via afconvert,
  git-ignored) + mkisopsx --cdda-track-list appends 17 audio tracks (disc
  ~330 MB); HL track numbers pass through (data=1, audio 2+). worldspawn
  sounds + trigger_cdaudio/target_cdaudio -> LOGIC_CDTRACK 30 (touch = play
  once + remove; -1 stops). music_apply() only acts when want != current;
  every CD data read kills CDDA so loads mark-interrupt and the next tick
  replays. Verified via --dump-audio: c0a0 plays track 3 through the ride.
- **Voice sentences**: 9 short lines resident (SC_GMORN/BIGDAY/SAMPLE/fear,
  BA_HEADDOWN/SCARED0/BUTTON/LATE, GM mumble) -- scripted_sentence ->
  LOGIC_SENTENCE 31 (arg0 = SFX id from the cook's table; long lines like
  BA_DESK 17s skipped -- SPU pack at 470 KB/512). play_world from the mark.
- Perf research round (web): ranked list lives in Next below. NB the SDK
  already REJECTED NCLIP deliberately (real-HW stale-MAC0 hazard, documented
  in psx-gte scene.rs) -- software cross products stay.

## M35 -- playtest round 2: state leaks between entity draws (DONE)

Four user reports, three root causes:

- **"Charger/grate moves with the door"**: the static-ent draw branch (kind
  != 1/3) projects through the WORLD proj token but never reloaded the GTE
  translation -- it inherited the PREVIOUS entity's offset (a mid-open door).
  Since M24 removed submodel faces from world marks, static-ent verts are
  first-projected in the ent phase, so they rendered displaced by the door's
  live offset. Fix: reload base_t + set_view_fix at the branch head (what
  every other branch already did).
- **"Intro train sometimes transparent"**: EMIT_BLEND leak -- the static-ent
  branch sets blend per face and never cleared it; the tram draws next and
  inherited a glass ent's Average blend (view-dependent, hence "sometimes").
  Fix: reset EMIT_BLEND/EMIT_WAVE after the static branch and before the tram.
- **"Sitting guy floats"**: floor_anchor_mdl_frames rebased EVERY frame's
  min_y to 0, lifting seated poses until their lowest vertex touched the
  script mark (mark = chair seat, feet should reach the floor BELOW it).
  Fix: canonical clips 0-4 keep per-frame anchoring (planted feet); named
  script clips shift by the constant clip-0 baseline, preserving authored
  root offsets. Desk barney verified seated.
- **"Models look weird / more precision"**: ENEMY_VERTEX_LOCAL_SCALE 2 -> 4
  (quarter-unit vertex grid, same i16 RAM; also halves the model near-GTE
  distortion zone to ~20 units). Chunk sizes +3%; VM reserve trimmed
  24,192 -> 20,224 words (~5 switched guns resident) -- roster audit: zero
  combat AND zero pickup drops on all 96 maps (c1a3 is the cliff map; its
  full need is ~306 KB, measure before trimming elsewhere).

## M36 -- interactables + per-map dialogue audio (DONE, verified)

Two playtest rounds:
- **Use-targets never activated** (buttons/chargers/use-doors): the aim-use LOS
  trace (eye->target centroid) was blocked by the TARGET'S OWN brush hull (it's
  a mover too). `phys::line_clear_movers_except(exclude=rec.brush)`. Then **low
  console buttons still unclickable** (the ~34deg cone can't reach a button you
  look DOWN at within reach): added a **forward crosshair ray** as the primary
  pick (`trace_line(eye -> eye + rot.m[2]*reach)`, match hit.mover to a
  use-target's brush), widened the fallback cone to ~56deg, reach 96->120.
  Verified A/B (0 acquisitions -> button+glass-door state cycles).
- **HEV suit auto-granted** (c1a0d): `standalone_room_starts_with_hev` used a
  stale `room>9` -- the MAPLIST grew and c1a0d is now room 10, so it handed
  over the suit AND applied to changelevel arrivals. `HEV_SUIT_ROOM=10` +
  `!preserve_view` gate. Verified: enter suitless, pick up on touch (0->1).
- **Per-map dialogue streaming**: dialogue no longer rides the global SFX pack
  (which truncated at MAX_SFX=43 -- the voice lines never loaded). Core SFX (43,
  337 KB) resident; per-map dialogue streams on load into the ~170 KB region
  above it (WORLD.PAK chunk 3100+idx), on a dedicated voice channel.
  `tools/extract_voices.py` cooks each map's scripted_sentence + speech
  ambient_generic to one pack at the highest rate that FITS the per-map budget
  (11025 where it fits, down to 4kHz for heavy maps -- PS1 SPU is fixed 4-bit
  ADPCM so sample RATE is the size lever), + a manifest the map cook resolves
  (VOICES_MANIFEST + MAP_INDEX). New LOGIC_AMBIENT(32); LOGIC_SENTENCE now plays
  a per-map voice. **Intro tram PA verified** (27s SPU voice, music off);
  **desk barney** (BA_DESK) cooked + trigger_once wired. 172 lines / 43 maps;
  c3a2d + c5a1 overflow and drop tail lines at runtime (graceful). The
  `speaker` PA-announcer presets (15 maps) are a separate sentence-group system,
  still absent.

## M37 -- entity-coverage audit + 3 missing systems (DONE, verified)

Full entity-classname audit (all 96 maps vs the cook's handled set) drove this.
Implemented, ranked by value:
- **func_door_rotating** (220 doors / 51 maps -- the biggest functional gap):
  swinging doors rendered STATIC and never opened. New ent kind 7: the door
  state machine drives ENT_PHASE, the draw rotates the brush about its pivot by
  phase*open_angle (reuses the func_rotating kind-5 fan path); mv[0] = signed
  open angle q12, logic_phase_step special-cased (mv is an angle, not a slide).
  Collision hull static: solid closed, dropped once open (phase>=2048) so you
  can walk through. Verified: ENT_PHASE cycles 0->4096 on trigger.
- **func_wall_toggle** (61 / 22 maps): removable barriers. kind-0 static +
  LOGIC_WALL_TOGGLE(34); fire flips ENT_ACTIVE (draw+collision) and forces a PVS
  re-gather so a re-shown wall draws that frame; SF bit0 = starts invisible.
- **env_shake** (186 / 44 maps): LOGIC_ENV_SHAKE(33). Decaying view jitter
  (yaw/pitch/eye by IMPACT_RNG, amp*remaining/duration) on scripted quakes.
  Verified: forced-shake frames differ 73k px.

The audit's REMAINING high-value gaps (ranked, for the next pass):
1. **multisource** (115/40, "the sleeper") + **env_global** (16/9): master/AND
   lock + persistent cross-map state (Blast Pit power). Logic-only. ~90
   entities are master-gated -- currently fire freely (puzzle-skip, rarely a
   hard block). `master` is only skipped as a common key today.
2. **momentary_rot_button + momentary_door** (10+9): valve-wheel doors -- the
   audit VERIFIED 3 are the SOLE opener (On A Rail c2a1b, c2a3a cage, c1a4g);
   hard progression blocks. Needs a hold-to-turn input rig (accumulate 0..1,
   door lerps). CLAUDE.md's old "open via targets in practice" is WRONG here.
3. **func_trackchange/traincontrols** (On A Rail junction routing) -- blocks
   c2a1/c2a2 train navigation.
4. **Sprite/beam renderer** unlocks the whole cosmetic FX tier at once:
   env_sprite (969), env_beam (677), env_glow (287), env_explosion (242),
   env_spark (181), env_laser (77). Needs a camera-facing billboard quad +
   `.spr` decode (biggest new-renderer investment; explosions have NO visual
   today). env_glow/explosion/spark fall out of the same billboard.
5. **func_tank** family (16+, mountable turrets) -- reuse hitscan + the +use ray.
6. **func_monsterclip** (147/23): cheap AI win (block NPCs, pass the player).
DEAD (safe to ignore): info_node/path_corner/path_track (consumed), infodecal
(baked), trigger_autosave/camera/transition (no-op without save-load).

## M38 -- the M37 roadmap's 5 remaining items (4 done, 1 partial)

Took on all 5 audit gaps. Ranked by what shipped:

- **multisource + env_global** (item 1, DONE): logic-only master/AND-lock +
  persistent cross-map global state. `GLOBAL_HASH/GLOBAL_ON[32]` (FNV-1a-16 key,
  matched cook<->runtime); LOGIC_MULTISOURCE(35) counts its inputs +
  fires its target once when `count >= arg0 || global_is_on(arg1)`;
  LOGIC_ENV_GLOBAL(36) sets/toggles a global then re-checks all multisources.
  Cook: `master` unhandled before; multisource input count = the number of ents
  targeting it. Verified the Blast Pit power chain hashes match cross-map
  (c1a4f env_global <-> c1a4b multisource, hash 1698). GLOBAL_COUNT resets on a
  fresh launch (not a changelevel carry).
- **momentary valve doors** (item 2, DONE): momentary_rot_button ->
  func_button (kind 3 +use) and momentary_door -> func_door in the cook, so the
  3 SOLE-opener valve doors (On A Rail c2a1b, c2a3a cage, c1a4g pipe) now open
  via +use instead of hard-blocking. ponytail: MVP treats the wheel as a use
  button + targeted door, not a hold-to-turn accumulate-0..1 rig.
- **func_trackchange** (item 3, DONE, cook-only): On A Rail train maps
  (c2a1/c2a2/c2a2b1) baked a TRUNCATED path chain -- the path_track chain
  dead-ends at the junction platform (which in HL rotates to splice the train
  onto a 2nd chain), so the driven train just stopped (hard block).
  `collect_tram` now stitches through the junction: when a ridden segment meets
  a func_trackchange node (at the segment START -- train parked on the platform,
  c2a1 -- OR its END -- train drives in, c2a2), continue onto the platform's
  other chain (toptrack<->bottomtrack). c2a1 3->15 waypts, c2a2 6->13, c2a2b1
  3->22; c0a0 intro tram + c2a2a (no junction) unchanged. ponytail: skips the
  rotate-the-platform puzzle + lever -- the train drives straight through to the
  exit. Faithful routing needs the full runtime trackchange rig.
- **env_explosion visual** (item 4, PARTIAL): explosions had NO visual (the
  audit's top cosmetic gap). `explode()` + LOGIC_ENV_EXPLOSION(37) now queue a
  world point; the render loop projects it and spawns a big additive-orange
  IMPACT_PARTICLES burst (cnt scales with magnitude). Verified: 260 orange
  particle px at the blast. This is a cheap stand-in for the .spr explosion
  sprite. The FULL sprite/beam billboard renderer (env_sprite 969, env_beam
  677, env_glow 287, env_spark, env_laser) is STILL the biggest single
  new-renderer investment -- needs `.spr` decode + a camera-facing billboard
  quad pipeline + a sprite VRAM atlas. Deferred as the largest residual.
- **func_tank** (item 5, DONE): the func_tank family cooks as LOGIC_TANK(38)
  (arg0 = per-shot damage, speed = fire cooldown from firerate). The brush
  already renders + collides as a kind-0 ent, so the +use crosshair ray acquires
  it like a func_button. Mount = look + use; while mounted the player is rooted
  (aim free) and R2 auto-fires hitscan from the barrel, rate-limited; any use
  press dismounts; MOUNTED_TANK never carries across maps. Verified on c2a2e: map
  renders clean + a forced-mount smoke fired hitscan on the cooldown cadence.
  ponytail: barrel doesn't visually track the aim; laser/rocket/mortar variants
  degrade to a bullet tank.

Logic kinds now through 38. Ent kinds unchanged (0-7). Remaining from the audit:
the FULL sprite/beam renderer (above), func_monsterclip (cheap AI win, not done),
traincontrols (rare). RAM: func_tank added ~6 B of statics; no roster/pool
change (cook-only trackchange, no new model types).

## M39 -- the sprite/beam renderer (billboards + beams DONE; explosions/sparks next)

The biggest single new-renderer investment, built in two committed phases. Covers
env_sprite (969) + env_glow (287) + env_beam (677) + env_laser (77) = ~2010 of the
~2400 sprite-tier entities.

- **`.spr` decode + per-map packs** (`tools/extract_sprites.py`, git-ignored like
  the other extractors): decodes each map's IDSP v2 sprites, crushes every frame
  to a 4bpp **power-of-two** texture (the VRAM atlas REQUIRES pow2 -- a 48px cap
  silently failed `allocate`). Additive sprites (texFormat 1) get the darkest
  cluster forced to black AND the **per-texel semi-transparency bit (0x8000)** set
  on every CLUT entry -- without it a textured additive prim draws OPAQUE, not
  blended. One pack per map (WORLD.PAK chunk 3200+idx) + a manifest.
- **Runtime** (`game/src/sprite.rs`): the pack streams into the now-free MAP_BUF
  BEFORE the world chunk (which then reuses MAP_BUF), decompresses (extras >=3000
  are LZ4), and appends its frame textures to the shared atlas between map textures
  and models (like voices stage through MAP_BUF for SPU). KEY BUG dodged: loading
  into MAP_BUF AFTER Map::load corrupts the live map -- sprites must load before
  the world chunk.
- **Billboards** (env_sprite START_ON / env_glow / cycler_sprite): cook as prop
  records with **SPRITE_PROP_BIT (0x2000 | local_id)** -- no header/format change,
  they ride the existing prop table (yaw field = world half-size). `draw_billboard`
  projects the center (project_world_point), sizes by perspective (world_half x
  H_PROJ / depth), and emits a camera-facing additive quad (two tris via the proven
  push_tri_uv_words path -- the quad ctor's vertex order flipped the UVs) into the
  OT. Verified c1a4: 23 glows render; a fixed-screen control draw confirmed the
  crushed flare1 is a correct bright-center glow. The "dark object in a glow" on
  c1a4 is real fixture geometry depth-sorted behind the billboard, not a bug.
- **Beams** (env_beam / env_laser START_ON): cook as **LOGIC_BEAM(39)** -- resolve
  LightningStart/End targetnames to world origins, store both endpoints in 4 aux
  entries (world coords fit i16), half-width in arg1, packed rendercolor in speed.
  `draw_beam` projects both endpoints + builds a screen-perpendicular flat additive
  quad (immediate, post-OT). Verified c1a0a: 9 clean thin beams; c4a1d: 21. ponytail:
  short + wide beams (Xen energy nodes, BoltWidth ~150 over ~40u) render as flat
  bars, not textured energy -- the faithful look needs a textured beam (lgtning.spr
  additive), a documented follow-up.

STILL the biggest residuals of the sprite tier:
- **env_explosion (242) animated sprites**: currently the M38 particle burst. The
  real fexplo sprite is cooked per-map; needs a transient billboard pool (spawn on
  explode() + cycle frames + expire). NB weapon-blast explosions happen anywhere,
  so a RESIDENT explosion sprite (fixed VRAM region) is needed, not per-map.
- **env_spark (181)**: intermittent spark billboards (timed spawns).
- **Toggled sprites/beams** (non-START_ON, ~1150 hidden until triggered): need
  logic wiring (a target sets ENT_ACTIVE-like state on the sprite/beam record).
- **Textured beams**: the beam sprite (additive glow) instead of the flat quad,
  for the wide Xen energy nodes + texture scroll/noise.

## M40 -- polish pass: the game-FEEL layer (Sprints 1-3, from a subsystem audit)

Four parallel comparison agents (weapons, AI/anim, movement/camera, presentation)
graded hl-psx vs GoldSrc's actual behaviour. Verdict: the SYSTEMS are complete;
the FEEL layer was stubbed. Implemented in three committed sprints + a shared
foundation.

- **Shared foundation -- view-punch** (`PUNCH_PITCH/YAW`, HL's punchangle): a
  decaying camera kick that recoil, landings, and damage all reuse; sprung back to
  centre in `tick_screen_fx`. `add_view_punch(pitch, yaw)`.
- **Sprint 1 (quick wins, all in the view/damage path)**: view **bob** (vertical
  head-bob + subtle strafe **roll** via a new `view_rotation_roll`/`rotate_z`,
  scaled by horizontal speed); per-weapon recoil **kick** (`WEAPON_KICK` table +
  random horizontal jolt); **damage flash** (subtract-blend red screen tint scaled
  by hit, verified rendering) + a view jolt; **low-health** red heartbeat pulse
  (<25); **fall damage + landing dip** (phys `Player.land_impact` captures the
  touchdown speed before it's zeroed); aim **expo curve** (`aim_curve`, 45%
  linear + 55% cubic).
- **Sprint 2 (movement + enemy motion)**: ground **accel/friction** (velocity
  ramps to the wish / decays to a stop, `GROUND_ACCEL`/`AIR_ACCEL`; wish clamped so
  diagonals aren't 1.41x); **smooth enemy turning** (`yaw_from_vec` -> full-res
  `atan2_q12`, `prop_face_point` **slews** via `turn_toward`/`PROP_TURN_RATE`
  instead of an 8-way snap); **death animation playthrough** (`PROP_DEATH_START`
  plays the death clip forward then holds, was an instant pop; authored corpses
  seeded in the past); ranged **attack windup** (`ATTACK_WINDUP` telegraph before
  the first shot).
- **Sprint 3 (identity)**: seated scientists **stand + flee** on a nearby fight
  (`damage_prop` alarms them, `tick_props` switches kind 25->0 when the standing
  model is resident); big melee aliens **stop leaping** (only headcrabs spring;
  zombie/bullsquid/ichy shamble in + swipe at melee reach); **HEV suit voice**
  (real fvox phrases: power-on on pickup, "health critical"/"near death" on
  threshold crossings) + **Geiger** crackle in `trigger_hurt` hazard volumes (SFX
  43->47, 405 KB core, ~107 KB left for dialogue -- heavy maps drop a few more
  tail lines).

Feel items are verified-by-construction + a boot regression (view level, no
crash); the damage flash + HEV voice were captured firing. ponytail residuals
from the audit table NOT done: viewmodel fire/reload/idle animation (RAM-tightest
region), flashlight (CPU frontier), shell casings, persistent decals, secondary
fire, houndeye sonic / bullsquid spit (ranged-AI path), crouch, per-weapon
crosshair, ambient loops. Doc-drift reconciled: the sprite/beam renderer (M39)
IS done -- earlier "Next" lines calling it deferred are stale.

## M41 -- Tier 1 combat-feel quick wins (8 items, from the 2nd polish audit)

Eight small, self-contained polish items (the audit's "Tier 1" row -- highest
feel-per-line). All game-side; no new assets, ~240 B of statics.

- **Persistent bullet/blood decals**: `decay_combat_fx` no longer ages the
  `IMPACT_MARKS` ttl -- marks stay full-colour until the 24-slot ring recycles
  the oldest (HL decals persist; ours used to fade in ~9 s). Cleared on map load.
- **Death blood pool**: `damage_prop` scatters 2 persistent blood decals at the
  corpse on death (world-space marks, so it covers every damage source -- bullets,
  projectiles, explosions, melee). Turrets don't bleed (`ai != AI_TURRET`).
- **Bullet tracers**: `fire_hitscan` pushes a muzzle->impact segment into a
  `TRACERS` ring (ttl 2 sim ticks, decayed in `decay_combat_fx`); the render draws
  each as one warm additive line via the existing `draw_beam`. Only long-range
  shots (`range > 1000`) leave one -- never a crowbar swing. Muzzle sits a little
  below/right of the eye so it streaks from the gun, not the crosshair.
- **Movement inaccuracy + shotgun scatter**: `fire_weapon` gained an `inacc` px
  arg (from player h-speed + airborne, computed at the fire site); a `jit(mag)`
  RNG closure wanders each hitscan's aim by it. The shotgun adds its own scatter
  (`inacc + spread/4`) per pellet so the pattern isn't a fixed rosette. Standing
  still = tight; run-and-gun / bunny-hop = spray. Matches HL's cone-of-fire.
- **Per-weapon crosshair**: `hud::draw` takes a `crosshair_px` size (0 = none) --
  the same sprite scaled by weapon class: crowbar 0 (HL melee has none), 357 /
  crossbow 12 (tight), shotgun 24 (wide), else 18. ponytail: scaled sprite, not
  per-weapon art (that needs the sprite-atlas pass).
- **Low-health HUD tint**: at `health < 25` the health cross + number modulate by
  a red tint (`with_tint((0x80,0x28,0x18))` -> amber texels go red-orange). Pairs
  with the existing red heartbeat pulse.
- **Enemy separation**: `prop_move_towards_point` fans each walker's goal sideways
  by a per-index amount (`((pi & 7) - 4) * 20` units, perpendicular to approach)
  so a group closes on the player in an arc instead of stacking on one point;
  drops to a straight line inside melee reach so a contact attack still lands.
  ponytail: O(1) index spread, NOT an O(n^2) neighbour scan -- the office maps are
  already near the frame budget.
- **Jump input buffer**: a Cross press up to `JUMP_BUFFER_TICKS` (2) before
  touchdown still jumps on the landing tick (`JUMP_BUFFER` latch in main, consumed
  when grounded) -- forgives an early press on a fall. On-foot update only.

Verified: clean build; c1a0 direct boot renders correctly (no regression). The
combat-feel items can't be judged from a static headless boot (no fire input), so
they're verified-by-construction + the boot check, per the session pattern.

## M42 -- the missing-features round: 16 items from the 3rd audit table

A big feature-completeness pass driven by the "missing features" table. All 16
requested items landed (each with a `ponytail:` note where scoped). Four
subsystem-mapping Explore agents seeded the implementation.

- **Ranged alien attacks**: houndeye (6) + bullsquid (7) are now AI_RANGED.
  Houndeye fires a sonic shockwave (radius damage centred on itself, player
  only); bullsquid lobs an acid-spit projectile (PROJ_SPIT). **Enemy projectiles
  + explosions can now hit the PLAYER** -- Projectile gained `from_enemy`;
  `explode()` applies falloff to the player (own rockets self-hurt); damage
  routes through a `PENDING_PLAYER_DAMAGE` accumulator drained each tick.
- **Secondary fire (L2)**: MP5 grenade, shotgun double-barrel, gauss charged
  slug, hornet burst; crossbow L2 = held zoom (half look-rate + pinpoint
  crosshair). `fire_secondary` spends its own ammo/cooldown.
- **Viewmodel animation (procedural)**: `viewmodel_offset` transforms the static
  bind pose -- fire kicks up+back, reload dips down+out, idle sways. Zero RAM
  (no baked frames -- the VM pool can't fit them).
- **Crouch (TRIANGLE)**: lower eye (13) + half speed. ponytail: no shrunk hull
  (needs a cooked crouch hull), so no low-gap fit yet.
- **Flashlight (L3, needs suit)**: near-depth brighten in the fog path (only near
  front geometry is drawn -> reads as a forward lamp). Fast path unchanged when
  off. ponytail: omnidirectional near-light, no suit-power drain.
- **Shared debris pool**: shell casings (bullet-weapon fire), gore gibs (on
  death), sparks (explosions + env_spark) -- world-space, gravity, ttl. New
  LOGIC_ENV_SPARK. env_explosion also throws sparks (the animated fireball
  sprite is the residual).
- **func_monsterclip**: LOGIC_MONSTERCLIP AABB (invisible, non-solid to player);
  NPC pathing refuses steps inside; player walks through.
- **Toggled sprites/beams**: the cook stopped skipping non-START_ON ones. Sprites
  = dormant named props (PROP_SPRITE_VIS, revealed on target fire); beams carry a
  START_ON flag + name and toggle via LOGIC_STATE. ~1150 hidden effects.
- **Momentary valve rig**: momentary_rot_button -> LOGIC_MOMENTARY; hold +use to
  ramp its target door open a notch at a time, stays where left (faithful
  hold-to-turn vs the M38 single-press MVP).
- **func_trackchange**: junction is a +use lever (M38 cook-stitch already routes
  the train through). Full rotate-platform rig deferred.
- **speaker PA**: recognized as a per-map voice (plays if its line is in the
  pack; sentence-group audio extraction is the residual).
- **gametitle**: the worldspawn gametitle bit flashes a large HALF-LIFE text card
  at c0a0 start. ponytail: text, not the logo.tga bitmap (VRAM reused in play).
- **c4a3 dark spawn**: `best_visibility_spawn` now prefers the brightest visible
  clear leaf (per-face lightmap mean). NB c4a3 also has an info_player_start +
  is startdark, so its debug-boot stays black (like c0a0); the fix helps the
  fallback path generally.

Verified: cook + game build clean; 96 maps recook; c1a0/c1a1a/c1a2/c4a3/c0a0
boot + render (no regression; c1a2/c4a3/c0a0 black in debug-boot are the
faithful startdark/dark-corridor views). Combat-feel items (fire/blood/ranged
AI) can't be driven headlessly -> verified-by-construction + the boot checks.

**RAM WARNING (needs a follow-up pass)**: static headroom is **32.5 KiB**, below
the 96 KiB gate. This was **already 50.8 KiB before this goal** -- a pre-existing
slide from M33-M41's features (the memory-report gate had gone unchecked; recent
CLAUDE.md "~96 KiB" figures were the model-pool audit, a different metric). This
round cost ~18 KiB: feature code (.text; `play()` is 115 KiB) + the recook
growing the auto-sized MAP_BUF (toggled sprites/beams/monsterclip/momentary add
per-map logic + prop records). The game boots + renders on every tested map at
64.5 KiB stack. Reclaiming to 96 KiB is a dedicated task: cap toggled-sprite
emission on the tightest map, tune core buffers (MAP_BUF/PRIMITIVE_PACKETS --
mind that combat now emits more prims), or code-size reduction. Do NOT add more
statics until this is addressed.

## M43 -- faithful door/button/charger operation (from the HL SDK source)

Cloned the Valve-released SDK into git-ignored `reference/halflife/` and surveyed
dlls/{doors,buttons,plats}.cpp + healthkit.cpp/h_battery.cpp + util.cpp for the
EXACT touch/use/trigger + master rules, then fixed three faithfulness gaps.

- **The reported bug: button-doors opened by direct +use.** HL rule: a func_door
  is player-+use-able ONLY when SF_DOOR_USE_ONLY is set (its FCAP_IMPULSE_USE);
  a targeted door opens solely via its button/trigger, a plain door via touch.
  `logic_try_use` now gates func_door on SF_DOOR_USE_ONLY (`logic_is_use_target`)
  for both the crosshair-ray and cone-fallback picks. The touch path already
  checked `targetname==0 && !USE_ONLY`, and the button->targeted-door trigger
  path is separate/unchanged, so the +use pick was the only leak. NB the
  momentary valve wheel (LOGIC_MOMENTARY) is handled by `tick_momentary`, not
  `logic_try_use`, so it is unaffected.
- **Master (multisource) lock.** HL gates open on `UTIL_IsMasterTriggered`. The
  cook stores each door/button's `master` targetname in arg1 (unused for these
  kinds before); `master_ok()` keeps them locked until that multisource is
  satisfied (LOGIC_STATE_TOP). FAIL-SAFE: master_name 0, or a name that isn't a
  present multisource, always allows -- only a present-but-unsatisfied
  multisource can lock, so an unknown master can never hard-block. Gated at
  `logic_activate_door_linked` (all touch/use/trigger door opens funnel through
  it) + `logic_activate_button`.
- **Charger suit gate.** CWallHealth/CRecharge deny a suitless player; the health
  + HEV chargers now require the HEV suit (`LOGIC_PLAYER_SUIT`), deny-click
  otherwise.

Verified: builds clean, 96 maps recook, c1a0/c1a1a render unchanged, headroom
34.5 KiB. **Door/button BEHAVIOR needs in-game playtesting** (button-doors need
the button; master-locked doors on the power-puzzle maps like Blast Pit) -- the
multisource puzzles can't be driven headlessly. Residual faithful polish (not
done): locked-door sentence/click feedback when the player tries a door they
can't open; SF_DOOR_ONEWAY rotating-door facing; charger recharge-after-delay
(currently refills only on map load); exact charger give-rate tuning.

## Next (pick per value)

- **PERF (researched, ranked -- the emit wall)**: 1) cook-time PRE-BAKED GPU
  packets (command byte/UVs/CLUT/tpage cooked as packet images; per-frame emit
  writes only tag+XY+RGB) -- attacks the 30% packet-build + 15% decode slices,
  the one big lever left for the 13 vista maps; 2) swc2 direct stores + RTPT
  shadow pipelining; 3) fog via banded light palettes (kills fog1 muls);
  4) GPF/GPL anim lerp. NCLIP rejected (SDK-documented HW hazard).
- **scripted custom anims v2**: per-map clip chunks for one-off set pieces
  (retina scan uses the baked retina clip already; CPR scene aliases to sit).
- **Load time**: per-chunk PAUSE dominates model streaming; merged geom+tex
  chunks or per-map bundles halve/collapse the count (watch transient fit).
  GPU fill is NOT a frontier (M26: rasterization is free in-target).
- **c4a3 chapter spawn** lands in a near-black pocket (campaign arrives via
  teleport; brightness-aware spawn scoring is the noted fix).
- **Ambient loops** (ambient_generic) + HEV fvox lines (~175 KB SPU free).
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
