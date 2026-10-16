# Deliberate differences: map decals and light styles (lane L5)

Each entry is an inventory row (see the row id) that the port keeps different from the original on purpose, with the
evidence. The integrator folds this file into the repository limits document.

## Map-painted decals (E "infodecal, static", gap #3)

Retail applies a static `infodecal` when the map loads: `CDecal::StaticDecal` hands its origin to the engine, whose
`R_DecalShoot` walks the BSP, takes every surface within `DECAL_DISTANCE` (4 units) of the origin, projects the decal
along the surface's texture axes and clips it to the polygon (Xash3D `ref/gl/gl_decals.c`). The port cooks the same
choice of surface and axes and draws one flat quad per decal (`host/hl-bsp/src/l5.rs`, `game/src/main.rs`
`queue_impact_marks`). 1,542 placements in 72 maps:

| outcome | placements |
|---|---:|
| drawn | 1,320 |
| on a moving brush (`func_door`, `func_breakable`, trains, buttons, named walls), not drawn | 99 |
| no surface within 4 units + 24 units of slack, so retail applies it to nothing either | 89 |
| dropped to keep a tight room's weapon-cache margin (c5a1 27, c2a4e 7, see below) | 34 |

- **Flat quad, not clipped to the polygon.** A decal near an edge is slid inside the host face's texture-space bounds
  instead of being cut off, so it keeps its size and moves by a few units. A decal larger than its face overhangs the
  neighbour. Retail clips it and would put the rest on the adjoining face when that face is within 4 units too.
- **Moving brushes carry no decals** (99 of 1,542). Each would have to follow its entity every frame; the cost is a
  per-entity record and a transform per decal, and no shipped map puts a decal where it is the only cue.
- **Axes snapped to the 26 neighbours of a cube.** Both texture axes are cooked as one of the cube's 26 neighbour
  directions: exact on axial surfaces and 45 degree chamfers (1,230 of 1,354 hosts are axial; of the other 124, 51 are
  chamfers), up to about 30 degrees off on the other 73 (curved or odd slants). This keeps a record at 10 bytes.
- **Three coverage levels.** A `{` decal's texel index is its alpha; it is reduced to clear, half (semi-transparent) and
  full, the same as the bullet and blood marks. Each room's decals share one 128x128 4-bit texture; a room whose
  decals do not fit at native size halves the largest first (c2a1: 28 textures).
- **Tint is flat per texture per room.** GoldSrc lights a decal with the lightmap of the surface it lies on. The port
  multiplies the palette colour by that lightmap level at the decal, averaged over the room's uses of the texture, into
  one vertex colour. A texture used both in a lit and a dark spot of one room takes the average.
- **Tight rooms.** The weapon-cache margin is the map staging window minus the cooked room (`cargo hl-build audit`),
  and c5a1 (580 B) and c2a4e (648 B) have almost none. `l5::room_budget` caps what lane L5 adds to them: c5a1 draws 27
  of its 54 decals (the 27 nearest the spawn) and keeps 300 B of margin; c2a4e draws 21 of 28 and keeps 400 B. Each
  10 B more is one more decal. Every other room grows by at most 7.9 KB and keeps at least 3.7 KB.

## Light styles (E "Named switchable lights", "Standard animated lightstyles 1 to 12", gap #4)

Retail: styles 1 to 12 are the standard animated patterns (`CWorld::Precache`); a targeted `light` of style 32 or more
toggles between "a" and its pattern or "m" on Use (`CLight::Use`, HLSDK `dlls/lights.cpp`), and a light without a
targetname is removed at spawn, so it never animates. The engine steps a pattern at 10 Hz and scales each lightmap
plane by `(letter - 'a') * 22` (Xash3D `ref/common/ref_light.c` `CL_RunLightStyles`, interpolation off).

The cooker keeps the map's start state baked into the vertex colours and stores, per face lit by an animated or
targeted style, the size of the change in the final colour at each corner (`dynamic_corner_delta`, a 64 colour
palette). The runtime scales that by how far the style's level is from its start level, so an untouched light costs a
flag test per face and a light left alone looks exactly as before.

- **Linear between two stored ends.** A plane that starts lit stores what it contributes (the colour with and without
  it, after the gamma ramp); a plane that starts dark stores what it adds at "m". Levels in between, and an animated
  pattern's peaks above its average, interpolate or extrapolate that. The gamma ramp is not linear, so a half-lit
  plane is a little off the engine's sum-then-ramp result.
- **10 Hz steps on the game clock, no interpolation, no restart.** Retail restarts a style's local time when it is
  set; the port's pattern phase follows the simulation tick (two ticks per step) and a switched-on pattern continues
  from the current step.
- **Up to 31 styles per room.** The most any room uses is 24 (t0a0b).
- **Switch records and saves keep their meaning.** `LIGHTSTYLE_ACTIVE_MASK` is still one bit per targeted style, so a
  save or a map walked back into restores the switches. Animated styles are not switchable and are not saved.
- **c5a1 and c2a4e bake their lights at the start state.** Their lights would add 4.2 KB (c5a1) and 7.4 KB (c2a4e) to
  rooms with 580 B and 648 B of margin. Everything stays lit as at the map's start; the three animated lights of c5a1
  and the switchable ones of c2a4e do not change. Lever: give those two rooms about 8 KB of weapon-cache margin (the
  model pool they share, or the staging floor) and they take the same path as every other map.
- **Models stay flat-lit** (inventory "Studio model lighting", unchanged): a switched-off room darkens the world and not
  the monsters in it.
- **Brightness setting.** The 64 delta colours go through the same brightness curve as the base palette, so a changed
  option moves a flicker's amplitude a little.

## Cost

Resident RAM: 616 B of code and tables for both features (free RAM 3,308 B at b5a58b2, 2,692 B now, floor 2,048 B,
same inlining rung 60 and the b5a58b2 profile). The painted-decal draw is the larger part (about 750 B); the light
styles cost less than the dynamic-light code they replace. In the room: 12 B of header, 10 B per decal, 9 B per decal
texture, 6 B per light style plus its pattern, and the dynamic appendix (12 B and 1 B per corner per plane for each
lit face; 25,052 faces and 379 KB of corner data over 103 maps, 36 KB in the worst, c0a0c). Rooms lose the 48 B
census-only world pipeline header the section replaces. A decal is culled by the view's PVS leaf and a sideways
test before any division. Stack gate: static bound 19,952 B (was 19,768 B) of the 25,600 B reserve.

Frame rate (shipped disc, port-1 polls 2400..3400, fps against b5a58b2; the same build with the b5a58b2 rooms, that
is without any L5 data, in brackets): c0a0 +1.8% (-0.1% from the data), c1a0 +2.2% (0), c1a2b -2.3% (-0.7%), c4a3 +0.3%
(-0.1%), t0a0b +3.4% (-9.4%), c1a1a with 145 decals -1.0% (-0.7%), c1a0c with 104 decals -1.4% (0), c1a1f -0.4%
(-0.3%), c0a0c with 58 targeted lights -0.2% (-0.4%). Two things sit under those numbers. First, any code change
moves the instruction-cache layout by a few percent either way on the same work (a profile recollected on this branch
moved c1a2b by 10% and c0a0 by 2%); the no-data twin separates that from the cost of the data. Second, the rewritten
dynamic-light path is cheaper than the one it replaces: on t0a0b the new code runs 14% fewer instructions per frame
than b5a58b2 on b5a58b2's own rooms. The one real data cost is t0a0b, where all 53 Hazard Course lights became
switchable and 1,048 faces now go through the flagged-face path (12% more instructions per frame than the same code
with no flagged faces); c1a2b pays 1.5% instructions for the same reason on a smaller scale.
