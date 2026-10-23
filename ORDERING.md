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
