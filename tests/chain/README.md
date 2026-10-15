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
