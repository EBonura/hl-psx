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
`settle=N` lengthens the neutral wait, and the sample may carry buttons (`s40+duck`). Floors under a `func_friction`
(the c1a1 pipe room from x 76 to 720) keep the player sliding much longer and by an amount that depends on the
build's tick alignment: leave such a volume with `@align ... settle=60` and never chain full-stick holds whose
end position matters. A route is robust when every stop that a later leg depends on is read from RAM (`@align`,
`@face`, `@hold until`) and none counts ticks; `chain-new-game` is kept green on both the intro and the no-intro
disc, so check both after a route edit.
Time spent in the c1a1 pipe channel costs health: the headcrab dropped by the broken pipe walks after the player and
bites for 10 (a route that settles there for a few seconds dies, one that moves through arrives with 35 of 45). Keep the
legs between the pipe trigger (x 794) and the labs1 door short and do not idle there.

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
From c1a1a it continues through the zombie corridor into c1a1f and out of it into c1a1b (15 segments). c1a1f has no walkable standing-hull path from its arrival to its exit; the way is two crouch crawls and a crouch jump found by searching the BSP hulls (the duck hull shows the crawls): the arrival hall's outer ring to the east room, the vent hole in the inner room's east wall (y 498..530), the wall charger on the inner room's north wall (the corridor leaves 15 to 55 health), a crouch jump onto the 49 high ramp base at (-868,176) (the retail engine does it with the same input), the walkway above the hall (crouched, ceiling 46 above the floor), the crab room, the long south corridor and the exit alcove's wall hole at y -1084..-1072. Two of its holds are narrow: align x to -888..-878 with slow f38/f-38 holds before the jump (a f70 hold moves 30 units per call) and keep the alignment at the exit short (a slide zombie arrives within a few seconds at 35 health).
It then plays c1a1b and the first half of c1a1c (the chain ends on the lift, `arrive-c1a1c` to `qy1`).
c1a1b: the airlock button (stand off it: it fires on touch and locks the view), the aira doors (a sliding cycle of about 40 ticks;
the doorway is crossed on `@wait expr="i32(ENT_PHASE[38]) < 400"`, never on a count), the corridor, two houndeyes killed with
`@kill`, the first aid unit, the door corridor, the vortigaunt (`@kill prop=12`), the 44 wide hole (crouched, aligned on x from the
north), the canal ring (planner legs), the valve, the flooded swim to the exit grate (rise only until `floor_grate2` starts, wait
for it on `TRAIN_OFF[8]`, then climb) and the changelevel. After the climb the player stands on the north lip of the hole: step
east round it before walking south. c1a1c: the planner legs to the catwalk, the clip pickup, the lever, the jump over the south
lip onto the lift, and the walk back to the deck's north edge with the magazine reloaded. The headcrab gauntlet (`@fight`, 20
crabs) is in the chain as a gate loop: the guard round is written once as a `[[block]]` and one loop step repeats it until the
lift has arrived (`TRAIN_OFF[2] >= 1890`) and no crab is in range (psoxide-gate TESTING.md, "Loops and blocks"; the same fight
unrolled was a 42 MB journey). The chain ends at the bridge approach with a game save, which is the card the stage journeys resume from.
A `@face` that has to turn through its last 59 units can stall in a chain (the single-step nudge turns nothing): use a
`@hold t60 until expr=...` with a wide window when the exact yaw does not matter.


`hazard.route` (`tests/chain-hazard.toml`): title, Hazard Course row, t0a0 from the start lift through doors td1 and
td2 and the observation hall to the catwalk north of it. That catwalk ends at the door of the swimming shaft (`waterout`),
which opens from the water side; the course continues elsewhere and the rest of t0a0 is unwritten.

A closed-loop `@hold` that times out aborts the journey, so a leg that stops moving shows as a timeout with the value
the word held. Keep every leg's timeout generous and its target a few units short of the retail position (the player
can stop against a wall one unit short of the value the retail engine reached).

## Gate expressions and closed-loop aim

Gate requirement: a journey with `expr=` conditions (`chain-new-game.toml` and the `@aim`/`@kill` legs) needs a
`psoxide-gate` built from PSoXide-emulator 1d4f6d4e or later (358eae9a or later for the gauntlet's loop step), taken from `PATH` or `$PSOXIDE_GATE` (the build in
`PSoXide-emulator/target/pgo/release` after `make build-pgo`). The gate is not chosen by `components.lock.json`: the lock's
emulator revision only decides which emulator crates `.psoxide` hydrates, and it stays at 0e87b3a because 1d4f6d4e's frontend
needs the `psx-webdisc` workspace dependency, which the pinned editor component's root `Cargo.toml` does not declare
(`cargo hl-build sdk` fails on it). An older gate fails at once and says so (ABORTED, TOML parse error,
unknown field "expr", 0 checks and 0 ticks). Do not read that as a journey failure; use a newer gate.

Any `@hold`, `@wait` or `@assert` condition may be a psoxide-gate expression (`expr="..."`, integer reads of guest RAM,
`let`, `abs`, `min`/`max`, `any`/`all`/`min` over index ranges; psoxide-gate TESTING.md, "Expression conditions"). Symbols named
inside the expression are bound by `--map` like the `sym=` ones, and the text must not contain `#`. The c1a1a zombie corridor
waits for the nearest zombie (`PROP_POS` slots 3 and 4, 12 bytes each: x, up, y) to be within 150 units instead of waiting on a
position of the lead one, so the dash starts on the same gap on every build.

`@aim`, `@kill` and `@level` close the loop on a target instead of a remembered yaw: `@kill prop=N` turns (yaw from the cross and dot
product of the player's forward vector against the offset to `PROP_POS[N]`, pitch from the height difference over the horizontal
distance, eye = `LOGIC_PLAYER_POS` up + `LOGIC_PLAYER_VIEW_HEIGHT`), holds attack until `PROP_HEALTH[N]` reads 0 or the target leaves
the aim, aims again and repeats up to `rounds=` times (a round that already holds costs no ticks). `lat=`/`vlat=` are the miss in
units that still counts as on target (the angular floor is 2.9 degrees, above the 2.5 degree smallest turn). `aimup=` raises the
aim point above the prop's `PROP_POS` up. Look level again with `@level`. Use it for any fight: find the prop slot by probing
`PROP_HEALTH`/`PROP_POS` at a checkpoint, then `@kill prop=SLOT`.

`@kill` and `@aim` also take `near=R [kind=K] [below=B]` instead of `prop=N`: the target is the nearest living prop within R units
(3D, `PROP_POS` up is the feet), optionally of one `PROP_KIND` (2 is a headcrab, 6 a houndeye, 55 a zombie) and not more than B units
under the player's centre (a prop under the floor cannot be shot), and the slot is found by the expression itself, so one line serves
any slot numbering and costs no ticks while nothing is in range. `@aim rel=DX,DUP,DY [unless=R]` holds the view on a point fixed
relative to the player (a stance: where the enemies come from; `unless=R` ends it at once when a prop is within R).
`@fight near=R [kind=K] [below=B] [aimup=N] [lat=8] [stance=DX,DUP,DY] [reload=N] [for=300] [sleep=10]` is a guard for `for` polls:
each round shoots whatever is in range (`@kill near=`), sleeps, and goes back to the stance; `reload=N` reloads when the magazine
is under N. Without `until` the loop is unrolled at compile time; with `until="EXPR"` the round is written once as a gate block and
repeated until EXPR holds and nothing of that kind is in range, at most `for` polls (a gate that has loops is needed: PSoXide-emulator
358eae9a or later). c1a1c uses it for the headcrab gauntlet on the lift
(`@fight near=480 kind=2 below=70 aimup=6 stance=0,-20,-300 for=1500 sleep=3 reload=6`).

## Stages: save, load and resume

A long chain is awkward to author in one piece (the first lift is about 25 minutes of emulated play), so a route can
be cut at any point with a game save and continued from it.

`@save` pauses, opens Save Game, saves to Card 1 Manual and persists the card image (`cards/card-step-NNN.mcd`
under the run's report folder). Later checkpoints in the same journey stop asserting `card_unchanged`. A save keeps
what a changelevel keeps (weapons, ammunition, health, suit, skill) and where the player stands; the map itself
starts fresh, so triggers the player has already fired are armed again (a save taken at the bridge approach of c1a1c
has a lift that has not been called and a gauntlet that has not run).

A stage journey starts from such a card: `@journey ... prologue=load card_fixture=tests/cards/NAME.mcd
card_sha256=SHA`. The prologue boots, skips the intro, and chooses Load game on the title; the first block's map
wait then confirms what the card restored. `card_sha256=AUTO` is replaced by the file's hash when the journey is built
with `stage.sh`-style tooling; the gate itself needs the real hash.

Cards are bound to the save format of the build that wrote them. Regenerate them by running the stage that ends in
`@save` again; do not hand-edit them.

## Weapon and melee directives

`@weapon NAME [dir=prev|next] [timeout=60]` presses the cycle button once (L1 prev, R1 next: one rising edge, so one
step through the owned weapons) and holds it until `VM_LIVE_WM` reads that weapon's viewmodel, then rests 14 polls
for the deploy delay. It costs nothing if the weapon is already in hand. Read the live weapon from `VM_LIVE_WM`
(the viewmodel index of `WeaponDef::wm`: crowbar 4, glock 0), never from `CARRY_CURRENT`, which is only the snapshot a
changelevel or save writes and does not change when the player switches.

`@swing ent=N [duck=true] [pitch=P] [timeout=40]` holds attack (crouched if asked) until brush entity N is gone
(`ENT_ACTIVE[N] == 0`). `pitch=P` first brings the view to P within 48 units (crouched if asked): a crouched swing
at pitch 0 breaks a vent cover that a standing swing needs a 41 degree look down for.

`@hold SAMPLE until x>=V | x<=V | y>=V | y<=V | up>=V | up<=V timeout=N` is shorthand for a position leg:
the expression reads `LOGIC_PLAYER_POS`. A crawl is `@hold f127 duck until x<=-1004 timeout=100`.

`stage-c1a1c-vent.route` (`tests/stage-c1a1c-vent.toml`) is the first stage journey: it loads `tests/cards/c1a1c-bridge.mcd`
(a Manual save taken at the bridge approach of c1a1c, after the chain's lift ride and headcrab gauntlet), crosses the
pipes to the vent box, draws the crowbar (`@weapon crowbar`) and breaks the vent cover (`@swing ent=65`, BSP model *86),
and ends crawling into the vent. It runs in about 12 s. The card was written by the full chain with an `@save` after the
bridge approach (the gauntlet makes that route too large to commit until the gate has a loop step); when the save format
or an earlier map changes, regenerate the card from the chain and rebuild the journey. Check it with `hlroute verify tests/chain/stage-c1a1c-vent.route . tests/stage-c1a1c-vent.toml --tape-dir tests/chain/tapes-stage --map MAP`.
