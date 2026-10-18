# Changelevel transition sweep

Checks every `trigger_changelevel` of the 103 shipped maps the way a player meets it: the player
is stood on a free hull spot inside the volume, the level changes, and the arrival is compared
with the landmark arithmetic of the original game (new position = old position - source landmark
+ destination landmark). It also watches the next 60 ticks for a stuck, falling or dying player.

Needs the Half-Life install (retail BSPs) and a probe build of the game. Nothing here ships:
the probe commands live in `game/src/probe.rs` behind the `interaction-probe` feature.

1. Generate one probe script per exit index K (0 is each map's first changelevel in lump order,
   up to K=4). `maps.txt` holds the 103 map names in room order, the `MAPS` array of
   `game/src/map_catalog.rs`:

       rustc --edition=2021 -O tests/sweep/transitions.rs -o /tmp/transitions
       /tmp/transitions gen "$HL_DIR/maps" maps.txt out 0     # out/tr-0.txt (probe), out/tr-0.tsv (table)

2. Build a probe disc with that script. The trace build plus the script overflow the image when
   every map's pool is sized, so size the pools for a room range that holds each source and its
   destination (0..45, 40..85 and 70..102 cover every pair) and bake only the sections of the
   maps you run:

       HLPSX_PROBE=out/tr-0.txt HLPSX_BENCH_MAPS=0,1,2,...,45 cargo hl-build pack --features interaction-probe

3. Run each source map for about 170 guest frames from a direct launch (the pad pulse mask
   `0x04<room id>@1+180` starts a room), keep only the `tick`, `probe` and
   `changelevel`/`map_start` event lines of the guest trace in `<dir>/<map>.txt`, and print the
   verdicts:

       /tmp/transitions report out/tr-0.tsv <dir>

The probe commands it uses: `tppref X Y Z` names the point to search outward from (the source
landmark), `tpfree MINX MINY MINZ MAXX MAXY MAXZ` stands the player on the nearest spot of that
box where the hull is clear of the world, a floor is within 40 units and a changelevel is really
touched (bounds and clip hull; standing first, ducking for vents), and `tpfreeany` does the same
without the touch for a `trigger_transition` box. A fired changelevel first tests that the player
stands in the transition volume named after its landmark, so a named exit is entered through that
volume and then fired by name 4 ticks later.

Verdicts: OK, CHECK (notes follow), NO-CHANGELEVEL (the touch did not fire: use-only exits with
no name need a use press), NO-FREE-SPOT (no hull-sized free space in the volume: it sits in solid,
the exit is scripted; the event carries the free and touching grid-point counts), NORUN.
Notes: ARRIVAL-OFF (more than 24 units from the landmark arithmetic), AIRBORNE-AT-END (never
landed), HEALTH, FELL, DRIFT, SHORT-RUN (too few destination ticks to judge: big maps load slowly),
FAR-FROM-LANDMARK (the spot is more than 160 units from the source landmark, so a stuck or falling
arrival there says little), CHANGE-BEFORE-PROBE (the map changed by itself before the probe).
