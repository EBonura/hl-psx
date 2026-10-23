# Draw order: oracle, census and the BSP sequence prototype

Branch `feat/hl-sorting-2026-10-23`. Nothing here is on `main`: the sequence code is behind the
`seq-order` feature and does not fit in RAM or in the frame budget yet. This file records what was
measured, what works, and what has to happen before it can land.

## The instrument

`tools/hl-order-oracle` plus the `order-trace` feature of the game. A probe script poses the
camera (`tp`) and arms one frame (`osnap`); the guest streams every primitive of that frame over
the debug port in table-walk order with the depth its key came from. The host rasterizes the
frame twice from the same vertices, once painted in table order (what the GPU does) and once with
a per-pixel depth test, and counts the pixels where the visible surface differs ("wrong-order
pixels", major = more than 8 units and 3 % apart). Depth of a world face is recomputed from its
BSP plane, so patch children and tessellated faces are judged against the true surface.
`lab` replays the same packets in an order a BSP leaf sequence would draw them, offline.

Sampling: 89 maps, 2509 frames (poses from `oracle poses`, plus the tube poses of Manny's
video), 160.6 M covered pixels.

## Census of the shipping order (baseline, major pixels)

| painter over victim | px | share |
| --- | ---: | ---: |
| world over world | 255,962 | 48 % |
| brush over world | 144,869 | 27 % |
| world over brush | 99,675 | 19 % |
| brush over brush | 15,289 | 3 % |
| brush over itself | 12,459 | 2 % |
| model, beam, other | about 3,700 | 1 % |

Total 531,917 major (0.33 % of covered), 886,092 minor. Causes: a key that cannot order
overlapping depth ranges (span) 74 %, ties 9 %, thin slivers 9 %, inverted keys 8 %.

## What the sequence does

A back-to-front walk of the BSP from the eye visits visible leaves in an order in which, along any
ray, nearer geometry comes later. Every world face lies on a BSP plane, so it occludes everything
behind that plane and nothing in front of it; keying each face at the slot of the farthest visible
leaf that marks it reproduces the painter's order exactly for world geometry.

Offline (lab, packets of the baseline census): world over world 255,962 to 6,959 px (-97 %).
In the guest (`seq_order.rs`, 86 maps, disc `seqG`): 255,962 to 32,298 px (-87 %). On the five
`t0a0*` maps that hold the training tube: world over world 28,322 to 474 px (-98 %), all classes
42,912 to 26,680 px (-38 %).

## What does not work yet

Entities. A brush face, a studio model or an effect is not on a BSP plane, so the leaf walk does
not order it; it needs its own key in the same space as the world's leaf slots. What was tried,
as major pixels over the baseline census in the lab (baseline 531,917):

| anchoring | major px |
| --- | ---: |
| face centroid leaf, ties broken by the game's depth key | 428,982 |
| each packet's own centroid leaf | 314,003 (models left last) |
| leaf of the farthest / nearest vertex | 1,231,749 / 476,270 |
| keep the game's depth key, mapped by nearest world key | 1,221,328 |
| keep the game's depth key, mapped to the least-misordered position | 525,910 |
| world keys bent to be monotone in depth space, entities untouched | 1,040,090 |
| depth span of the entity mapped onto the leaves it spans | 1,173,087 |

The best idealized result still moves 27 % of the error from world over brush to brush over
world, because a brush face that spans depth (a strut, a long panel, a hallway the camera is
inside) cannot be ordered by one anchor. Only splitting such faces at the BSP planes is exact.
In the guest the per-face anchor and its single bucket make it worse: on the census subset the
entity classes grow (brush over itself 11 k to 94 k, brush over brush 12 k to 56 k) and total
major pixels rise from 252 k to 435 k even though world over world falls from 69 k to 13 k.

## Cost, measured on the tube poses (`costB` vs `costS`, 19 poses, last rendered frame of each)

| stage (cycles per frame) | before | after |
| --- | ---: | ---: |
| render | 2,859,633 | 4,003,519 (+40 %) |
| room visible list (sequence walk, rebuild) | 29,992 | 272,247 |
| room project + surface draw | 2,731,148 | 2,835,490 |
| model bounds (brush pass, one tree descent per face) | 1,114,653 | 1,694,822 |

Free RAM of the two probe links: 174,324 B without, 142,960 B with: 31.4 KB (FACE_KEY 17.4 KB,
LEAF_SLOT 7.2 KB, node bits, code). The shipping link has 2,060 B free.

Landing even the hooks alone is not free: with `order-trace` compiled out `hl_psx::play` still
reads as changed to `psoxide-pgo`, the committed profile no longer applies to it and the link
at hot-callsite-threshold 70 does not fit, so a profile and layout recollect (two blobs) plus a
re-measure is needed, and that moves the shipping binary.

## Plan

1. Entities first, because the world part is done. Cook-time splitting of brush faces that cross
   world node planes (so every fragment has one leaf), one anchor leaf per entity and a
   precomputed leaf for each fragment instead of a tree descent per face per frame. Models and
   effects anchor at their centre leaf with a window sized by their radius; slot pitch follows
   the visible leaf count (leaf slots needed per frame: median 18, p99 120, max 168).
2. Cost. Renumber leaves in tree order in the cooker (the leaves of a node are not contiguous
   today, `oracle leafrange`), so "any visible leaf under this node" is a prefix-popcount range
   query and the 370 k-cycle node-bit rebuild disappears.
3. RAM. Key per face group instead of per face (a group is a set of coplanar faces, so a key per group is as exact), leaf slots
   indexed by visible-leaf rank, drop the group chains in the PVS lists. About 31 KB has to
   become under 2 KB.
4. Recollect the profile and layout, run the full gate set, A/B on the tram bench.
5. If it works, move it to `psx-bsp` so CS and Quake share it.

## Phase 2 (2026-10-23, evening): does the plan hold?

Everything below is an offline replay (`hl-order-oracle lab2`) of the guest's own traced packets:
the same 89 maps, 2,509 frames and 160.6 M covered pixels as the census, and the same 19-map
subset (c0a0, c0a0a, c0a0d, c0a0e, c1a0c, c1a0e, c1a1d, c1a3d, c1a4j, c1a4k, c2a1b, c2a2c, c2a3d,
c2a3e, c2a5g, c3a1b, c3a2, c3a2f, c4a2b) that the guest prototype was measured on. The replay
reproduces the shipping numbers exactly when it keeps the game's own keys (`L2_LEGACY=game`:
531,917 census, 251,749 subset), so the instrument is the same one. No new guest build was made:
every step that pays needs RAM or cycles the game does not have (below), and the replay already
says which entity treatments can work.

### What changed since Phase 1

Phase 1 anchored an entity to the nearest visited leaf, which put anything inside rock (a brush
embedded in a wall, a door sliding into a pocket) in front of the wall it hides behind. The tree
has a slot for solid space too: every child of every node, leaf 0 included, has a place in the
back-to-front walk. Giving rock slots a rank lowers the entity error: per-packet anchors with
models left last, 314,003 on the census with the nearest open leaf and 270,403 with solid slots;
ideal per-pixel pieces on the subset, 164,806 and 103,286.

### Census, major wrong-order pixels (89 maps, 2,509 frames)

| class | shipping | per-packet anchors | statics cut at cook, movers per packet | ideal (every pixel cut) |
| --- | ---: | ---: | ---: | ---: |
| world over world | 255,962 | 6,959 | 6,956 | 6,959 |
| brush over world | 144,869 | 122,507 | 92,056 | 75,645 |
| world over brush | 99,675 | 65,297 | 36,367 | 351 |
| brush over brush | 15,289 | 17,111 | 13,424 | 10,996 |
| brush over itself | 12,459 | 21,316 | 20,999 | 11,783 |
| model over world | 1,810 | 0 | 0 | 0 |
| world over model | 1,381 | 7 | 7 | 2 |
| total | 531,917 | 233,710 | 170,310 | 106,198 |

19-map subset: shipping 251,749, Phase 1 guest 435,015, per-packet 165,714, statics cut at cook
and movers per packet 138,819 (world over world 69,042 to 1,637, world over brush 65,043 to
28,834, brush over world 92,810 to 77,239, brush over brush 12,451 to 11,308, brush over itself
11,192 to 19,539), ideal 87,281. A model anchored per packet is right (model over world 535 to 0).
Models left last (the Phase 1 lab) cost 10 k and 6 k.

A large share of what is left in the ideal column is not ordering: the c0a0 tram and the trains
of c0a0a and c0a0e (func_tracktrain, tens of thousands of wrong pixels each on the subset) are
identical before and after, because the tram's cached packets carry the depths of an earlier frame
and the oracle judges them against those.

### The plan, step by step

1. **Cut brush faces at the world planes when cooking.** Pays, for static entities only: the wrong
   pixels (minor and major, an entity pair counted for both) in which a static brush takes part go
   from 109 k to 52 k on the subset, against 93 k with per-packet anchors alone. It costs
   faces. Cutting every static brush face through the world tree at its cooked place turns 48,103
   faces on the 89 maps into 84,765 fragments, of which 15,659 lie in rock and can be dropped, so
   69,106 open ones: 44 % more faces, 14,017 of the faces cut. Movers cannot be cut at cook time:
   the planes a moving face crosses change as it moves. Cutting at the planes the motion is
   parallel to is exact but helps only sliding doors and lifts beside walls.
2. **One anchor leaf per entity, a window sized by its radius.** Does not pay. Entity at the centre
   slot: whole subset 269,409 against 251,749 shipping (movers 204 k to 308 k wrong pixels); at the
   nearest slot of any vertex 215,871 (world over brush gone, brush over world 175,498); at the
   farthest 1,323,545. Face centroid: 272,550.
   Per packet is the first placement that beats the game, and a packet that straddles a plane is
   still wrong (about one pixel in six of a brush packet sits in a different slot than its
   centroid). Cutting a brush face into cells so a packet is one cell needs cells of 32 units to
   come near the ideal (all brush entities cut into cells: 32 units 96,410; 64 units 138,546; 128
   units 283,999; 256 units 609,593), which is 4 to 50 times the brush faces at 48 units (c1a4k:
   779 faces to 19,172 cells of 48).
3. **Renumber leaves in tree order.** Needed before any of this is cheap, and not done. Today the
   leaves under a node are almost never a contiguous run (2 to 17 of 441 to 3,150 nodes per map,
   `leafrange`). Renumbering has to permute every compressed visibility row too. The nodes and
   leaves the cooker would need to carry (a face to node table, a parent or range table) are RAM
   at the largest map, see the budget below.
4. **A key per face group.** Does not pay: world over world goes from 69,042 to 775,192 on the
   subset, because a plane (the floor) is shared by faces that sit at very different places in the
   walk. A key per BSP node is as good as a key per leaf (136,781 against 138,819 for the whole
   configuration), but it needs a face to node table. Leaf slots indexed by visible-leaf rank would
   save 7 KB and is possible.
5. Other things tried and dropped: entities that span several slots drawing with the world faces
   of that range by the game's depth key, 194,825; a frame that shows such a mover falling back to
   the game's order, 185,784 (brush over world grows); the game's depth space with brush packets
   re-keyed by nearest or farthest vertex or a bias, 254 k to 415 k (the shipping keys are already
   the best of them); the screen centroid of a packet at its mean depth, which is all a guest can
   compute without the face plane, 197,765 against 165,714.

### Cost and RAM

The steady-state cost of the Phase 1 prototype, tube hall poses, frames that did not rebuild the
visibility list (costB against costS): render 2,546,592 to 3,200,668 cycles (+26 %), of which the
sequence and key writes inside `room` are +224 k and the brush pass +432 k. The walk is not the
cost, the marks are: every face is keyed once per visible leaf that marks it.

Tram bench (map play-order 0, the opening tram ride, polls 2400 to 3400, bench map set, same source
tree with and without `seq-order`, both without layout so the comparison is like for like):
15.796 fps (2,144,114 cycles per flip) shipping order, 9.105 fps (3,719,869) with the Phase 1
sequence, -42 % fps. The tram's packet cache is switched off under the sequence, because its keys
change every frame. The same pair links with 186,292 B free without the feature and 124,364 B with
it: 61.9 KB for this map set (31.4 KB on the smaller tube-hall set).

The RAM budget is not 2 KB of headroom. `game/ram-budget` commits a floor of 2,048 B, the shipping
link has 2,060 B free, so the allowance is 12 B and any growth has to be paid for by removing
bytes elsewhere or by lowering the floor on purpose. Per-face keys (2 B a face) and a face to node
table (2 B a face) are 17 KB each at the largest map. The group chains (`PVS_FACE_NEXT`, 2 B per
face) are the only structure of that size the sequence could replace, and they carry the per-plane
back-face test.

### Verdict

The world half works and is nearly free of error (255,962 to 6,959). What stops a landing is the
entity half and the budget, not the walk:

- No entity treatment that fits a few KB and a few thousand cycles lowers every class. The best
  realistic one (statics cut at cook, movers and models per packet) lowers every class but brush
  over itself (12,459 to 20,999, all of it func_water in c3a2f) on the census, and needs a face
  to node or parent table, 44 % more static brush faces and a tree descent per moving packet.
- Even the world half alone cannot ship: the entity keys live in depth space, and every way of
  mixing them with sequence keys is worse than either (1.0 M to 1.2 M).
- Recommendation: leave the shipping order. If the sequence is wanted later, the first step is not
  an ordering step: free tens of KB in the room data (the face lists and group chains) and give
  the cooker a leaf and face order that makes node ranges cheap.
