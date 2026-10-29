# Chain journeys

Journeys that play the campaign the way a person does, from the title, with controller input only:
no save is edited, no map is forced, no cheat is on. Each map's controller tape is replayed once the
map has loaded and ticked, so the next map is reached only by the game's own changelevel.

`hlroute.rs` compiles a text route into poll-bound `PXITAPE2` tapes (one stick and button state per
simulation tick) and the `psoxide-gate` journey that plays them:

    rustc --edition=2021 -O tests/chain/hlroute.rs -o /tmp/hlroute
    /tmp/hlroute build tests/chain/inbound-anomalous.route . tests/chain-new-game.toml \
        --map .hlpsx/hl-psx.map --until c1a0c
    psoxide-gate run tests/chain-new-game.toml --no-hw

Route units are the ones the game derives from a DualShock (forward, strafe, turn, look, buttons), so a
route written against the GoldSrc reference runtime (`hlroute export` writes HLINPUT1) can be tried on
both. Two controller limits are handled in the compiler and reported when they apply: a stick inside the
radial dead zone reads as nothing (the smallest real nudge is used instead), and a DualShock cannot reach
a right turn of +127 (view-angle error is tracked and corrected on the idle and turn ticks).

`map-ids.txt` lists each map's cooked header word, which the journey reads from `WORLD_ARENA+4` to know
which map is resident. It was read from the HLMK headers of the installed disc image in room order.

`--map` binds symbols the gate's own loader cannot read (the linker map prints `NAME (.0)` for split
statics) as fixed addresses, so a journey is written against one build.

Routes may also close the loop where a PS1 line cannot be written open loop: `@hold <input> until
sym=... offset=N size=4 signed op=le value=V timeout=T` presses that input until a RAM word crosses
a value (a gate `hold_until`, ordinary controller input throughout), and `@wait` waits on RAM without
input. The player position is read from `LOGIC_PLAYER_POS` (x, up, y). `@stop` ends the journey.

## Regenerate per build
`inbound-anomalous.route` binds RAM symbols through `--map`, so `tests/chain-new-game.toml` is only valid for the build whose
link map it was generated from: regenerate it for every disc (`hlroute build ... --map .hlpsx/hl-psx.map`). The c1a0e cart push
is closed loop on yaw, the player's y and the cart's own x (`ENT_CACHE+1732`, entity 27 of c1a0e; Ent is 64 bytes with the origin at +4):
each `@hold` pair is a dead band (a hold whose condition already holds ends at once), so no fixed nudge depends on where a
frame happened to end a hold. Yaw is held to 1010..1038 (east is 1024; the stick's smallest turn is 29 a tick,
2.5 degrees), the player walks west along y >= 700 to the lift's west margin (x 1152..1163) and strafes south along it,
settles 8 ticks, and pushes in legs ended by the cart's x. The first leg is forward 60: a full-speed start flush
against the lift's 1-unit lip stalled 3 units short of the cart on builds before the `slide_move` fix (branch
`fix/hl-cart-push-2026-10-16`), and nothing under forward 90 stalled. Passes on integ 5dac694, b5a58b2 and 219e7ff,
and with the push perturbed by one tick of turn or strafe before the first leg.

## Timing: polls, settling and staleness

Every route time is in controller polls (simulation ticks, 20 Hz when the frame is built in time), never in
video frames. A tape sample is one poll. `@hold`, `@wait` and `@face` take `timeout=N` in polls; the compiler
writes `6 * N` gate ticks (the gate counts video frames, three per poll nominally), so a slower build still
gets its legs and a leg that has really stopped still aborts the journey with the value it held. A step that
waits on the game (map load, the scientist reaching the scanner) waits on RAM, not on a count.

A full-stick hold leaves the player sliding for tens of units after the stick is released (about 50 at full
stick), so the position read at the end of a hold is not where he stops. Where a later step depends on the
resting position (a use cone, a doorway), use `@align SAMPLE sym=.. offset=.. signed=true lo=A hi=B timeout=N`:
slow nudges with a neutral settle (25 polls by default) after each, read again, ending inside the band.

`hlroute verify ROUTE REPO JOURNEY.toml --map ...` compiles the route into a scratch directory and compares the
journey and every tape with the committed ones. Run it after any route or compiler edit: a journey that no longer
matches its route fails here instead of as a red chain run (the committed journey was once a compile older than
its route and broke at a leg that the route had already fixed).

## Prologues, `@face`, `@probe` and the segment table

`@journey ... prologue=new_game` presses New Game on the title. Two more prologues exist for routes that do not start
at New Game: `prologue=hazard` (the title's Hazard Course row and the map list; `hazard_row=N` moves N rows down the
list, 0 is t0a0) and `prologue=segment` (the Debug page's Segment select, `segment_row=N` rows down in play order; a
diagnostic launch for fast authoring, never part of a shipping chain, because it does not come from the previous
segment's end state).

`@face YAW right|left [timeout=N]` turns the player until `LOGIC_PLAYER_YAW` (0..4095, 0 faces +y, 1024 faces +x)
lands in a 60-unit band that one tick of a t60 turn cannot jump. A band that crosses the 4095 to 0 wrap is refused:
pick the other direction. `@probe` after a `@checkpoint` adds four assertions that always pass and print the player's
position (x, up, y) and yaw in the report, which is how a route is surveyed.

`segments.rs` prints the 103 maps in play order with chapter, exits and content counts straight from the retail BSPs
and `game/src/map_catalog.rs` (`rustc --edition=2021 -O tests/chain/segments.rs -o /tmp/segments`,
`/tmp/segments "$HL_DIR/maps" game/src/map_catalog.rs`).

## What the committed chains cover

`inbound-anomalous.route` (`tests/chain-new-game.toml`): New Game, the tram ride (c0a0 to c0a0d), the walk to the test
chamber (c0a0e), Anomalous Materials (c1a0, c1a0d, c1a0a, c1a0b, c1a0e) and Unforeseen Consequences' first map (c1a0c): the
airlock, the pump room, the pipe corridor (a crouch jump: a plain jump peaks 3 units short of the fallen pipes on the PS1),
the lift, the console scientist (use him, he follows to the retinal scanner) and the control room, then c1a1 (the west detour to the north corridor, the two lasers, the crowbar and the glass window broken with crouched swings, the hall, the shaft window, the ladder and the upper level) to the arrival in c1a1a. The c1a1 legs were authored with a scratch waypoint planner and corrected by hand; the second laser's safe window is read from the live beam end (`BEAM_RAND_END`).
The c1a0c legs were derived from a trace of the retail engine and are closed loop on the player's x and y.

`hazard.route` (`tests/chain-hazard.toml`): title, Hazard Course row, t0a0 from the start lift through doors td1 and
td2 and the observation hall to the catwalk north of it. That catwalk ends at the door of the swimming shaft (`waterout`),
which opens from the water side; the course continues elsewhere and the rest of t0a0 is unwritten.

A closed-loop `@hold` that times out aborts the journey, so a leg that stops moving shows as a timeout with the value
the word held. Keep every leg's timeout generous and its target a few units short of the retail position (the player
can stop against a wall one unit short of the value the retail engine reached).
