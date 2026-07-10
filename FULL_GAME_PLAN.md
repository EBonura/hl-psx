# Full-game port plan

The goal is the full Half-Life campaign shape, not a PS1 room viewer. That
means the port must preserve source content and move the expensive parts into a
proper residency model. We should not randomly reduce geometry or make per-map
special cases; those would keep the demo alive while making the game harder to
finish.

## Current blockers

- `game/src/main.rs` owns one static `MAP_BUF` and streams exactly one
  `WORLD.PAK` chunk into it before gameplay starts.
- `game/build.rs` sizes `MAP_BUF`, face caches, leaf caches, entity caches, and
  texture slots from the largest cooked `data/rooms/room_*.psxc` currently on
  disk. Adding one large chapter map increases RAM pressure everywhere.
- `game/src/map.rs` parses a monolithic `HLMA`/`HLMB` blob: render geometry, textures,
  BSP/PVS, clip hulls, brush entities, tram data, and props all share one
  residency lifetime.
- `tools/hl-bsp/src/main.rs` emits all mip textures per map, so repeated
  textures and model assets cannot become shared resident resources.
- `game/src/cdstream.rs` can load a whole pack chunk, but it is not yet a
  general asset streamer. Its pack table reader also needs to support many more
  chunks than fit in the first sector.

PSoXide already has the pattern we want: fixed stream slots, a declared
resident window, LRU/pinning, per-use revalidation, and VRAM eviction tied to
the active set. Relevant source is in the sibling repo, especially
`docs/level-residency.md`, `docs/world-grid-architecture.md`,
`engine/examples/editor-playtest/src/active_room_streaming.rs`, and
`sdk/crates/psx-cache/src/lib.rs`.

## Production rules

- Static RAM must be bounded by budgets, not by the largest map or by total
  campaign content.
- The renderer and collision system consume resident pages; gameplay state is
  authoritative and persists even when render/model/collision pages are evicted.
- BSP/PVS is the streaming oracle. The original maps already tell us which
  leaves are visible from the current leaf.
- Chapter transitions may load a new level core, but active play must only keep
  the current local working set resident.
- Player speed and AI timing stay fixed-step; rendering and streaming may
  degrade, but game speed must not.

## Target runtime shape

```text
chapter select
  -> load level core
       BSP nodes/leaves/PVS directory
       entity table and persistent entity state
       spawn/changelevel/script metadata
       texture/model/animation dictionaries
  -> per frame
       find player/camera leaf
       expand BSP PVS into desired cells
       pin desired render/collision/model/texture working set
       stream missing pages through fixed slots
       draw/use only resident validated pages
```

### Level core

Always resident for the current BSP map:

- spawn points, changelevels, scripted sequence metadata, trigger boxes, and
  entity state records;
- BSP traversal records and enough leaf/PVS metadata to ask "what should be
  resident from here?";
- material/model dictionaries mapping GoldSrc names to streamed asset ids;
- current-map clip hull data for the first pass.

The first implementation can keep hull-1 collision resident inside the core,
because current clip data is much smaller than all render geometry plus
textures. If larger maps prove too tight, hull nodes become collision pages
selected from the same current-leaf/PVS working set.

### Streamed world cells

Render geometry should leave the monolithic HLM blob and become small cells.
Each cell contains:

- bounds;
- face/group records;
- local vertices/triangles;
- material ids, not copied texture blobs;
- optional local collision-page ids.

The cooker should build cells from BSP leaves/clusters, then merge neighboring
leaves until the payload is near a target slot size. A good first budget is
32-64 KiB per render chunk, matching the PSoXide streaming model, with four to
six resident chunks during gameplay.

### Shared asset banks

Textures, models, and animation clips should be independent assets:

- textures are keyed by GoldSrc texture name and uploaded to VRAM through a
  residency cache;
- NPC/player/weapon models are streamed by encounter or inventory, not baked
  into the executable as a full campaign set;
- the current view weapon stays resident, nearby NPC model/clip sets stay
  resident, and inactive chapter assets stay on disc.

This is where all weapons/NPCs become realistic: the game carries ids and state
for everything, but only the nearby assets occupy RAM/VRAM.

### Slot/cache layer

Use PSoXide infrastructure instead of a parallel custom cache:

- add the sibling `psx-cache` crate to `hl-psx` for fixed-capacity keyed caches;
- either reuse/extract PSoXide's room scheduler or mirror its contract exactly:
  pin desired set, reserve slots, stream into caller-owned buffers, validate
  identity on every use, evict LRU outside the window;
- expand `WORLD.PAK` from "room N" chunks to typed asset chunks:
  level core, render cell, collision page, texture page, model mesh, animation
  clip, audio/script banks.

## Cooker roadmap

1. Add a host-only streamed-map analysis mode to `tools/hl-bsp`.
   It should emit a manifest and per-cell size report without changing runtime
   behavior yet. This tells us the real cell budgets for every campaign map.
2. Add `HLM2`/streamed output:
   `level_<map>.core` plus `level_<map>_cell_<n>.psxc` chunks.
3. Replace copied per-map texture blobs with a texture dictionary and separate
   texture assets.
4. Emit entity/NPC/trigger records as level core state, with model references
   rather than resident model bytes.
5. Teach `mkisopsx`/`WORLD.PAK` ordering to pack typed chunks and generate a
   runtime TOC that supports hundreds or thousands of entries.

## Runtime roadmap

1. Load the level core at chapter start and remove the largest-map-sized
   `MAP_BUF` from static RAM.
2. Introduce fixed world-cell slots and render from resident cells only.
3. Choose the resident set from current BSP leaf/PVS, with a small prefetch ring
   so turning around does not stall every frame.
4. Move texture upload to a material residency cache keyed by texture id.
5. Move NPC/weapon model loading to a model residency cache keyed by model and
   clip set.
6. Only after this, expand the chapter menu to every map. The menu should select
   a chapter/core id; it should not imply all chapter content is resident.

## Acceptance tests

- Static RAM does not grow when a larger map is added to the disc.
- A campaign map can exceed the old monolithic `MAP_BUF` size and still start,
  because only its core and local cells are resident.
- Turning in place shows stable PVS/cell residency without missing resident
  walls, wrong-map bytes, or permanent black holes.
- The PSoXide performance capture stays near the 20 fps target while game logic
  advances at fixed speed.
- At least one chapter transition loads a new level core and preserves expected
  player/inventory state.
- NPCs can be active logically when off-screen, but their model/animation assets
  only become resident when their area is active.

## Next change

The texture split has removed map texture bytes from resident RAM. The next
gameplay-facing pass should use that headroom for actors: make NPC/enemy state
authoritative in the runtime, keep nearby model/animation assets resident, and
start with simple Half-Life-faithful behavior for scientists and headcrabs. The
next memory-facing pass after that is still the HLM2/core/cell boundary: add a
streamed-map analysis mode that partitions one BSP into BSP/PVS-derived cells
and reports cell sizes, texture refs, face counts, and worst-case resident
windows.
