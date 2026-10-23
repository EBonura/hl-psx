# Black Mesa Inbound (c0a0 to c0a0e)

The tram ride and the walk from the tram to the Anomalous Materials test chamber. Retail order: c0a0, c0a0a, c0a0b, c0a0c, c0a0d, c0a0e, then c1a0. Method, evidence kinds and the bug list live in [README.md](README.md); the segment table in [SEGMENTS.md](SEGMENTS.md).

Chapter result: all six segments are traversed by `tests/chain-new-game.toml` from the New Game press on the title, with no save, no cheat and no forced map. `tests/chain-skill-hard.toml` repeats the first three changelevels on Hard and checks the difficulty byte survives them. The checkpoints carry liveness checks only (`golden = false`), so no frame of this chapter has been compared with a retail reference image (see README, Evidence gaps).

State column: `fixed` = a commit or CHANGELOG entry on main fixes it; `limit-recorded` = a LIMITS-*.md file records it; `limit-claimed` = the inventory calls it a PS1 limit but no LIMITS file lists it (a bug under the completion standard until it is written down with evidence); `limit-unrecorded` = same, and no code comment records it either; `open` = the 10-15 inventory read the SDK and the port and found a difference that nobody has re-checked on this build.

### c0a0

Play order 1 of 103, room id 0. Exits: c0a0a@c0a0toa[].

Original contents: 15 actors, 17 scripted sequences or sentences, 6 multi_managers, 7 triggers, 0 buttons, 1 doors. Classes: path_track 30, scripted_sequence 17, env_glow 9, env_message 9, monster_scientist 8, multi_manager 6, func_wall 4, trigger_auto 4, func_illusionary 3, monster_barney 3, ambient_generic 2, func_tracktrain 2 ....

Journey: controller-only, passive. `tests/chain-new-game.toml` waits for the load (cooked map word 6050 at `WORLD_ARENA+4`) and the first simulation tick, sends no input (the tram carries the player), then waits for the game itself to change level to c0a0a. Checkpoint `arrive-c0a0`: frame not flat, card unchanged. Evidence kind: emulator replay, CPU renderer, `--no-hw`; no golden image.

Static entity contract (`hl-census map c0a0`, current main):

- Gap, the port does not honour it: func_breakable.explosion x1, func_door.health x1
- Partial, honoured with a stated deviation: env_fade (renderamt x1, rendercolor x1); env_message (flag:1 x9); func_breakable (renderamt x1, rendercolor x1, renderfx x1, rendermode x1); func_tracktrain (renderamt x2, rendercolor x2, renderfx x2, rendermode x2)
- Unreviewed, no one has checked the key against retail: env_glow (renderamt x9, rendercolor x9, renderfx x9, rendermode x9); func_illusionary (renderamt x3, rendermode x3, skin x3); func_wall (renderamt x4, rendercolor x2, rendermode x4)
- Cosmetic: 24 class/key pairs
  - func_breakable.explosion: the explosion gib mode is not read
  - func_door.health: a door with health opens when shot in retail; the port ignores it

Inventory rows first met here (10-15 inventory, 15 DIFF or MISSING): 3 fixed on main, 12 open or unverified.

Fixed: B3 (6e9e343); B15 (4f81bc1); trigger_changelevel touch and Use (3d2a17d).

| inventory row | difference (inventory wording, shortened) | state |
|---|---|---|
| S26 Pre-disaster idle chatter: statement vs question to a stationary f | Durations match; the 75% and 50% random gates are replaced by "always speaks when conditions hold" with a hashed line choice, and the variant list is capped per map. PS1  | limit-claimed |
| S28 One speaker at a time, global talk gap | Same idea. Port has ONE voice slot (even scripted_sentence cannot overlap); retail has per-entity channels. PS1 LIMIT recorded in code comment main.rs:18394 ("the one PS1 | limit-claimed |
| Sequence length, animation aliasing | PS1 LIMIT. Several retail sequences alias one baked pose clip to save RAM while their schedule timing stays exact. Recorded in the code comment at main.rs:10520 and in en | limit-claimed |
| Brush render modes and renderfx (glass, grates, fades) | PS1 LIMIT. Amount 0 hides; texture/glow below 250 becomes one half-alpha class; additive and solid-cutout map to two more; intermediate renderamt, rendercolor tint and re | limit-claimed |
| Touching one half of a split door | The port opens every untargeted door whose box touches the touched one; the SDK source has no linking code, so touching one leaf opens that leaf. A player usually touches | limit-recorded |
| Studio model lighting | Monsters and pickups are lit by one flat shade, not by the room light under them, so a grunt in a dark corridor looks as bright as in a lit hall. PS1 LIMIT, UNRECORDED be | limit-recorded |
| env_fade | Duration and hold agree; colour is reduced to black or white and renderamt/modulate/ONLYONE ignored, so Xen's green and blue fades (c4a1 family) and partial-alpha flashes | open |
| Concerted start of same-named scripts (DelayStart, m_iDelay) | Same-targetname scripts wait for every walking actor before any plays; the port starts each actor on its own arrival. Impact unmeasured: 290 sequences walk or run to a ma | open |
| Sequence cross-fade (engine studio blend) | CONFIRMED lost on this branch: every sequence change (idle to walk, script start, attack) snaps. The cross-fade alone is commit d7d8507 (+146 lines in main.rs and species | open |
| Player train control (func_traincontrols, PFLAG_ONTRAIN) | Quarter-speed notches match, but the port clamps the lowest notch to 0 so a drivable train cannot reverse (the SDK reverses unless FORWARDONLY), and control is not gated  | open |
| Texture animation (+0..+9) and alternate (+a..+j) chains | Alternate chains for func_wall frames and pressed buttons are right. Rate differs: the port advances a frame every 0.2 s (5 fps), the Xash oracle every 0.1 s (10 fps) for | open |
| Bone controllers and blend values (head turn, aim, gait) | Scientists and Barneys do not turn their heads toward the player and monsters do not blend aim pitch; only the mouth moves. | open |

### c0a0a

Play order 2 of 103, room id 1. Exits: c0a0@c0a0toa[], c0a0b@c0a0tob[].

Original contents: 1 actors, 1 scripted sequences or sentences, 6 multi_managers, 7 triggers, 0 buttons, 3 doors. Classes: path_track 55, func_wall 18, env_message 12, multi_manager 6, func_pushable 4, trigger_auto 4, env_beam 3, env_spark 3, info_landmark 3, info_target 3, path_corner 3, func_door 2 ....

Journey: controller-only, passive. `tests/chain-new-game.toml` waits for the load (cooked map word 6127 at `WORLD_ARENA+4`) and the first simulation tick, sends no input (the tram carries the player), then waits for the game itself to change level to c0a0b. Checkpoint `arrive-c0a0a`: frame not flat, card unchanged. Evidence kind: emulator replay, CPU renderer, `--no-hw`; no golden image.

Static entity contract (`hl-census map c0a0a`, current main):

- Gap, the port does not honour it: env_spark.maxdelay x3, func_door.health x2, func_pushable.explosion x4
- Partial, honoured with a stated deviation: env_message (flag:1 x12); env_spark (@class x3); func_door (renderamt x1, rendercolor x1, renderfx x1, rendermode x1); func_door_rotating (renderamt x1, rendercolor x1, renderfx x1, rendermode x1); func_tracktrain (renderamt x1, rendercolor x1, renderfx x1, rendermode x1); func_train (renderamt x1, rendercolor x1, renderfx x1, rendermode x1, skin x1)
- Unreviewed, no one has checked the key against retail: env_beam (boltwidth x3, flag:2 x3, framerate x3, framestart x3, life x3, lightningend x3, lightningstart x3, noiseamplitude x3, renderamt x3, rendercolor x3, striketime x3, texture x3, texturescroll x3); func_illusionary (renderamt x1, rendercolor x1, renderfx x1, rendermode x1, skin x1); func_pendulum (damp x1, renderamt x1, rendercolor x1, renderfx x1, rendermode x1); func_pushable (buoyancy x4, delay x4, renderamt x4, rendercolor x4, renderfx x4, rendermode x4, size x4); func_wall (renderamt x8, rendercolor x2, rendermode x8)
- Cosmetic: 32 class/key pairs
  - env_spark.maxdelay: spark interval is fixed
  - func_door.health: a door with health opens when shot in retail; the port ignores it

Inventory rows first met here (10-15 inventory, 7 DIFF or MISSING): 2 fixed on main, 5 open or unverified.

Fixed: Per-level state when returning to a visi (CHANGELOG); Named switchable lights (light/light_spo (CHANGELOG).

| inventory row | difference (inventory wording, shortened) | state |
|---|---|---|
| func_pushable push, pull, friction, buoyancy | Max speed 400 minus friction, factor 1 on the ground, pull on +use with backward movement, sizes 0..3 and the breakable variant follow the SDK. Differences: only the play | limit-claimed |
| Ambient sound concurrency | PS1 LIMIT. Only 7 voices serve authored loops; quieter ones are evicted by level. 24 maps start more than 7 looping ambients (c2a4a 26, c3a1a 19). The audit's "45 maps dr | limit-claimed |
| func_tracktrain on a path (player tram and brush trains) | Look-ahead speed*0.1, yaw error closed by half each 20 Hz frame (avelocity = delta*10), path_track speed override, pass target and dead end follow the SDK. The speed chan | open |
| env_beam / env_lightning | Start-on, toggled, random-area and life/strike beams, live endpoint entities, sprite texture, texture scroll and noise sway are implemented. Width is tuned (x0.6) and col | open |
| env_spark | About 3.2 s mean gap against ~0.85 s; the TOGGLE (32) and START_ON (64) flags and Use are ignored, so 96 sparks in 26 maps that stay silent until triggered spark from the | open |

### c0a0b

Play order 3 of 103, room id 2. Exits: c0a0a@c0a0tob[], c0a0c@c0a0btoc[transition].

Original contents: 2 actors, 4 scripted sequences or sentences, 6 multi_managers, 6 triggers, 0 buttons, 15 doors. Classes: path_track 37, env_message 12, func_door 12, env_glow 10, func_illusionary 10, multi_manager 6, ambient_generic 5, scripted_sequence 4, func_door_rotating 3, func_wall 3, trigger_auto 3, env_spark 2 ....

Journey: controller-only, passive. `tests/chain-new-game.toml` waits for the load (cooked map word 6296 at `WORLD_ARENA+4`) and the first simulation tick, sends no input (the tram carries the player), then waits for the game itself to change level to c0a0c. Checkpoint `arrive-c0a0b`: frame not flat, card unchanged. Evidence kind: emulator replay, CPU renderer, `--no-hw`; no golden image.

Static entity contract (`hl-census map c0a0b`, current main):

- Gap, the port does not honour it: env_spark.maxdelay x2, func_door.health x12
- Partial, honoured with a stated deviation: env_message (flag:1 x12); env_spark (@class x2); func_door (renderamt x8, rendercolor x6, renderfx x4, rendermode x4); func_door_rotating (renderamt x3, rendercolor x3, renderfx x3, rendermode x3); func_trackautochange (@class x1, bottomtrack x1, height x1, rotation x1, speed x1, toptrack x1, train x1); func_tracktrain (renderamt x1, rendercolor x1, renderfx x1, rendermode x1)
- Unreviewed, no one has checked the key against retail: env_glow (renderamt x10, rendercolor x10, renderfx x10, rendermode x10); env_sprite (framerate x2, renderamt x2, rendercolor x2, renderfx x2, rendermode x2, scale x2); func_illusionary (renderamt x10, rendercolor x4, renderfx x1, rendermode x10, skin x10); func_trackautochange (movesnd x1, renderamt x1, rendercolor x1, sounds x1, stopsnd x1); scripted_sequence (flag:16 x2)
- Cosmetic: 31 class/key pairs
  - env_spark.maxdelay: spark interval is fixed
  - func_door.health: a door with health opens when shot in retail; the port ignores it

Inventory rows first met here (10-15 inventory, 5 DIFF or MISSING): 0 fixed on main, 5 open or unverified.

| inventory row | difference (inventory wording, shortened) | state |
|---|---|---|
| env_sprite and env_glow rendering | Every sprite animates at a fixed 10 fps and draws additive, ignoring framerate (126 env_sprites use 20 to 40), rendermode (texture and solid modes drawn additive), render | limit-claimed |
| GN2 cycler (studio props): apache.mdl, base.mdl, rip.mdl, protozoa.mdl | c0a0b apache cycler, c1a4g base/rip, c1a0e protozoa, c1a4i rengine, c2a2h sat_globe are absent. Fungus, hair, pipe_bubbles cyclers are cooked. Recorded in entity-support. | open |
| GN3 cycler animation cycling / damage response (Use toggles, hits adva | Props are inert. Recorded. | open |
| func_trackchange and func_trackautochange | The platform never rotates or carries the train between tracks; a use of the junction just routes the cooked path. 6 placements (c0a0b, c0a0d, c2a1, c2a2, c2a2b1, c2a2e). | open |
| ambient_generic pitch, fade and LFO | 146 placements with a pitch other than 100 and 67 with fade, spin or LFO keys play at native pitch and level. Recorded as cosmetic in entity-contract.txt. | open |

### c0a0c

Play order 4 of 103, room id 3. Exits: c0a0d@c0a0ctod[transition2], c0a0b@c0a0btoc[transition].

Original contents: 2 actors, 4 scripted sequences or sentences, 12 multi_managers, 5 triggers, 0 buttons, 31 doors. Classes: path_corner 91, path_track 50, func_wall 34, env_beam 32, func_door 23, ambient_generic 17, multi_manager 12, func_train 10, func_door_rotating 8, env_message 7, env_spark 4, func_pushable 4 ....

Journey: controller-only, passive. `tests/chain-new-game.toml` waits for the load (cooked map word 7222 at `WORLD_ARENA+4`) and the first simulation tick, sends no input (the tram carries the player), then waits for the game itself to change level to c0a0d. Checkpoint `arrive-c0a0c`: frame not flat, card unchanged. Evidence kind: emulator replay, CPU renderer, `--no-hw`; no golden image.

Static entity contract (`hl-census map c0a0c`, current main):

- Gap, the port does not honour it: env_spark.maxdelay x4, func_door.health x23, func_pushable.explosion x4, path_corner.yaw_speed x91
- Partial, honoured with a stated deviation: env_message (flag:1 x7); env_spark (@class x4); func_door (renderamt x19, rendercolor x17, renderfx x7, rendermode x7); func_door_rotating (renderamt x8, rendercolor x8, renderfx x8, rendermode x8); func_tracktrain (renderamt x4, rendercolor x4, renderfx x4, rendermode x4); func_train (renderamt x10, rendercolor x10, renderfx x10, rendermode x10, skin x2)
- Unreviewed, no one has checked the key against retail: env_beam (boltwidth x32, flag:1 x8, framerate x32, framestart x32, life x32, lightningend x32, lightningstart x32, noiseamplitude x32, renderamt x32, rendercolor x32, striketime x32, texture x32, texturescroll x32); func_pushable (buoyancy x4, delay x4, renderamt x4, rendercolor x4, renderfx x4, rendermode x4, size x4); func_wall (renderamt x1, rendermode x1)
- Cosmetic: 32 class/key pairs
  - env_spark.maxdelay: spark interval is fixed
  - func_door.health: a door with health opens when shot in retail; the port ignores it
  - path_corner.yaw_speed: trains turn at their own rate

### c0a0d

Play order 5 of 103, room id 4. Exits: c0a0c@c0a0ctod[transition2], c0a0e@c0a0dtoe[levelchangetoe].

Original contents: 4 actors, 2 scripted sequences or sentences, 6 multi_managers, 8 triggers, 0 buttons, 6 doors. Classes: path_track 34, func_wall 22, func_pushable 9, multi_manager 6, ambient_generic 4, func_door 4, trigger_auto 4, env_shooter 2, func_door_rotating 2, info_landmark 2, monster_generic 2, scripted_sequence 2 ....

Journey: controller-only, passive. `tests/chain-new-game.toml` waits for the load (cooked map word 5713 at `WORLD_ARENA+4`) and the first simulation tick, sends no input (the tram carries the player), then waits for the game itself to change level to c0a0e. Checkpoint `arrive-c0a0d`: frame not flat, card unchanged. Evidence kind: emulator replay, CPU renderer, `--no-hw`; no golden image.

Static entity contract (`hl-census map c0a0d`, current main):

- Gap, the port does not honour it: func_door.health x4, func_pushable.explosion x9
- Partial, honoured with a stated deviation: env_shooter (@class x2); func_door (flag:4 x2, renderamt x3, rendercolor x3, renderfx x1, rendermode x3); func_door_rotating (renderamt x2, rendercolor x2, renderfx x2, rendermode x2); func_trackautochange (@class x1, bottomtrack x1, height x1, rotation x1, speed x1, toptrack x1, train x1); func_tracktrain (renderamt x1, rendercolor x1, renderfx x1, rendermode x1)
- Unreviewed, no one has checked the key against retail: env_shooter (delay x2, flag:1 x2, m_flvariance x2, m_flvelocity x2, m_igibs x2, pitch x1, renderamt x2, rendercolor x2, renderfx x2, rendermode x2, scale x2, shootsounds x2); func_pushable (buoyancy x9, delay x9, renderamt x9, rendercolor x9, renderfx x9, rendermode x9, size x9); func_trackautochange (renderamt x1, rendercolor x1, sounds x1); func_wall (renderamt x18, rendercolor x18, renderfx x14, rendermode x14)
- Cosmetic: 31 class/key pairs
  - func_door.health: a door with health opens when shot in retail; the port ignores it

Inventory rows first met here (10-15 inventory, 2 DIFF or MISSING): 0 fixed on main, 2 open or unverified.

| inventory row | difference (inventory wording, shortened) | state |
|---|---|---|
| trigger_push | Player and walking monsters are pushed; the ONCE bit (1) is ignored (8 placements) and so is the c2a5 flag-4 special case (barrels); vertical push is integrated as an acc | open |
| env_shooter and gibshooter | Count, delay, repeat flag, velocity and variance, shootsounds all drive the shower; the thrown pieces are material-coloured shards instead of the authored shootmodel/hgib | open |

### c0a0e

Play order 6 of 103, room id 5. Exits: c0a0d@c0a0dtoe[levelchangetoe], c1a0@c0a0toc1a0[changetoc1a0].

Original contents: 0 actors, 5 scripted sequences or sentences, 7 multi_managers, 8 triggers, 0 buttons, 8 doors. Classes: func_wall 17, ambient_generic 16, path_track 9, multi_manager 7, func_door 4, func_door_rotating 4, scripted_sequence 4, path_corner 3, info_landmark 2, trigger_auto 2, trigger_changelevel 2, trigger_once 2 ....

Journey: controller-only. `tests/chain-new-game.toml` waits for the load (map word 3370) and first simulation tick, replays a poll-bound tape of 1626 controller samples (`tests/chain/tapes/c0a0e-00.pxtape`, compiled from `inbound-anomalous.route`), and ends when the game itself changes level to c1a0. Checkpoint `arrive-c0a0e` asserts a non-flat frame and an unchanged card. Evidence kind: emulator replay, CPU renderer, `--no-hw`; no golden image.

The walk leaves the tram, crosses the platform and enters the test-chamber corridor; the next map is reached by the authored changelevel trigger.

Static entity contract (`hl-census map c0a0e`, current main):

- Gap, the port does not honour it: func_door.health x4, path_corner.yaw_speed x3
- Partial, honoured with a stated deviation: func_door (flag:4 x2, renderamt x3, rendercolor x3, renderfx x1, rendermode x3); func_door_rotating (renderamt x4, rendercolor x4, renderfx x4, rendermode x4); func_tracktrain (renderamt x1, rendercolor x1, renderfx x1, rendermode x1); func_train (renderamt x1, rendercolor x1, renderfx x1, rendermode x1, skin x1); trigger_hurt (damagetype x1)
- Unreviewed, no one has checked the key against retail: func_illusionary (flag:1 x1, renderamt x1, rendercolor x1, renderfx x1, rendermode x1, skin x1); func_wall (renderamt x14, rendercolor x10, renderfx x10, rendermode x14); scripted_sentence (flag:4 x1)
- Cosmetic: 30 class/key pairs
  - func_door.health: a door with health opens when shot in retail; the port ignores it
  - path_corner.yaw_speed: trains turn at their own rate

Inventory rows first met here (10-15 inventory, 3 DIFF or MISSING): 1 fixed on main, 2 open or unverified.

Fixed: trigger_autosave (CHANGELOG).

| inventory row | difference (inventory wording, shortened) | state |
|---|---|---|
| trigger_hurt on monsters and the NO_CLIENTS flag | Zombies, grunts and headcrabs standing in lasers, slime, radiation and death pits take no damage (253 placements in 67 maps). NO_CLIENTS (c2a4c, 1 placement) still hurts  | open |
| scripted_sentence speaker choice and flags | The SDK picks a speaker who CanPlaySentence (not mid-script, not already talking, FOLLOWERS flag needs a following actor, INTERRUPT forces) and retries every refire+0.5 s | open |
