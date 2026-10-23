# Hazard Course (t0a0 to t0a0d)

Seven maps reached from the title's Hazard Course row, which lists them in file order (t0a0, t0a0a, t0a0b, t0a0b1, t0a0b2, ...); the changelevels chain t0a0, t0a0a, t0a0b1, t0a0b2, t0a0c, t0a0d (with t0a0b a side map reached from t0a0c). t0a0d ends the course with a `trigger_endsection`. Method and evidence kinds: [README.md](README.md).

Chapter result: the title menu path and the first map's entry are journey-gated, and `tests/chain-hazard.toml` (from `tests/chain/hazard.route`) plays t0a0 from the start lift through doors td1 and td2, the observation hall and the catwalk north of it, all controller input from the title. No journey plays any Hazard Course map to its exit, so none has a journey from the previous segment's end state. The course is a long scripted tutorial: doors open on timers after a narrator line, buttons sit on pedestals that the player walks around, and the lessons (jump, duck, long jump, ladders, water) gate the doors.

State column: `fixed` = a commit or CHANGELOG entry on main fixes it; `limit-recorded` = a LIMITS-*.md file records it; `limit-claimed` = the inventory calls it a PS1 limit but no LIMITS file lists it (a bug under the completion standard until it is written down with evidence); `limit-unrecorded` = same, and no code comment records it either; `open` = the 10-15 inventory read the SDK and the port and found a difference that nobody has re-checked on this build.

### t0a0

Play order 97 of 103, room id 96. Exits: t0a0a@t0a0t0a0a[].

Original contents: 17 actors, 14 scripted sequences or sentences, 15 multi_managers, 43 triggers, 6 buttons, 10 doors. Classes: func_wall 55, trigger_once 38, func_illusionary 31, infodecal 30, env_sound 18, multi_manager 15, ambient_generic 13, scripted_sentence 10, env_glow 9, func_door 9, func_breakable 8, monster_scientist 7 ....

Journey: `tests/chain-hazard.toml` (title, Hazard Course row, first map, then closed-loop legs on the player's x and y: round the first pedestal, door td1 (opens 15 s after the player passes the trigger at y -1680), the corridor to door td2, the observation hall, east along its rail, north up the catwalk to the swimming-shaft door at (-754, 623); 18 checks, checkpoints `hazard-start`, `td1-passed`, `observation-hall`, `hall-east-end`, `swim-shaft-door`). The catwalk is a dead end from this side: the door ahead (`waterout`) opens only from its north side, so the way on is elsewhere and unwritten. Entry journeys: `tests/journey.toml` (title, Hazard Course row, map list, first map, 1,180 ticks of load, then 160 ticks of forward stick, `frame_diff` and no-stall checks); `tests/save-repin.toml` and `tests/skill-save.toml` (walk, save to the card, cold boot and load); `tests/pause-overlay.toml` (pause menu and brightness over the live game); `tests/hazard-load.toml` (the loading strip stays alive until the first frame). None of them leaves the start corridor. Evidence kind: emulator replay, CPU renderer, `--no-hw`; the committed journeys carry their own goldens.

Static entity contract (`hl-census map t0a0`, current main):

- Gap, the port does not honour it: func_breakable.explosion x8, func_door.health x9, scripted_sentence.listener x3
- Partial, honoured with a stated deviation: env_render (rendercolor x6, renderfx x6); env_sound (@class x18, roomtype x18); func_breakable (renderamt x8, renderfx x8, rendermode x8); func_button (renderamt x6, rendercolor x6, renderfx x6, rendermode x6); func_door (renderamt x9, rendercolor x9, renderfx x9, rendermode x9); func_door_rotating (renderamt x1, rendercolor x1, renderfx x1, rendermode x1); func_train (renderamt x1, rendercolor x1, renderfx x1, rendermode x1, skin x1); infodecal (@class x30); trigger_hurt (damagetype x2)
- Unreviewed, no one has checked the key against retail: env_glow (renderamt x9, rendercolor x9, renderfx x9, rendermode x9); env_sprite (framerate x1, renderamt x1, rendercolor x1, renderfx x1, rendermode x1, scale x1); func_illusionary (renderamt x31, rendercolor x23, renderfx x1, rendermode x31, skin x31); func_wall (renderamt x47, rendermode x47); scripted_sentence (flag:4 x3)
- Cosmetic: 33 class/key pairs
  - func_breakable.explosion: the explosion gib mode is not read
  - func_door.health: a door with health opens when shot in retail; the port ignores it

Inventory rows first met here (10-15 inventory, 29 DIFF or MISSING): 4 fixed on main, 25 open or unverified.

Fixed: A8 (e7594b1); B3 (6e9e343); D2 (CHANGELOG); D5 (CHANGELOG).

| inventory row | difference (inventory wording, shortened) | state |
|---|---|---|
| B14 | PS1 LIMIT (recorded: comment at main.rs:11961-11966): subtractive colour grade instead of the refraction warp. The port adds an O2 bar for suited players that retail does | limit-claimed |
| D1 | PS1 LIMIT (recorded: save.rs module doc): memory-card file, two slots x two ports via the pause menu, one 8 KiB block. README.md:29 says saves are "implemented" without t | limit-claimed |
| F2 | Plays the concatenated sentence from the per-map voice pack. PS1 LIMIT (recorded: host/hl-content/src/audio.rs:2214-2240 and build log): on maps where mandatory dialogue  | limit-claimed |
| I3 | Pad has next/previous only; the full bucket menu is not drawn. Controls documented in README.md:128. PS1 LIMIT, recorded in README controls table only. | limit-claimed |
| J7 | PS1 LIMIT (recorded: hsfx.rs comment, not a hardware requirement). All positional sound is centred. | limit-claimed |
| J11 | PS1 LIMIT (recorded: env_sound_audio.rs header: "different filter graph; modulation, low-pass and stereo parity remain open"). Applied only when SPU RAM leaves a safe wor | limit-claimed |
| J14 | PS1 LIMIT (recorded: music.rs module doc and main.rs:5880-5890). Music stops during map loads and resumes at the same sector; shorter songs in a four-song file are follow | limit-claimed |
| K2 | PS1 LIMIT (recorded: settings.rs). Not a gameplay behaviour. | limit-claimed |
| A2 | Retail traces 34 units down, 16 units ahead along velocity; no floor doubles friction so you stop at ledges. Port never brakes extra, so the player slides further toward  | limit-recorded |
| A17 | PS1 LIMIT (recorded: ladder_logic.rs doc comments): strafe on a ladder is 1/3 of retail speed, mount uses a 2-unit tangential widening, ladders assumed axis-aligned (card | limit-recorded |
| A19 | PS1 LIMIT, UNRECORDED: the 70-sample resident core bank holds only pl_step1/2; every surface, ladder rung and puddle sounds like concrete. Cadence keyed to stick magnitud | limit-unrecorded |
| A9 | Only matters when a jump starts above 544 u/s (right after a long jump, or chained hops with air control). Port has no cap. Speedrun-only impact. | open |
| A12 | End values match; the port eases in 0.2 s linear, retail 0.4 s spline. Cosmetic. | open |
| A14 | Port launches along current velocity direction (not view) and allows it at any crouch duration; retail uses the view forward (so pitch shortens it and strafe-running laun | open |
| A16 | Climb speed and duck factor match. Dismount is quantised to whole units: 280 u/s vs 270 (3.7% faster). Cosmetic. | open |
| A20 | Jumping and hard landings make no footfall; only pain grunt plays when damage lands. | open |
| B4 | Port sinks 25% faster. Cosmetic. | open |
| B13 | Entering water, breathing at the surface and swimming are silent. | open |
| E2 | Holding Use re-triggers a door or button every 0.4 s (toggle doors flip back and forth). Chargers rely on this cadence. Tapping behaves like retail. | open |
| E3 | No audible feedback when pressing Use at nothing or at a usable object. | open |
| F6 | No "ammunition depleted" voice on empty weapons. | open |
| F7 | Wrong sound, no sentence. Cosmetic. | open |
| S24 Head turn toward player/friend (bone controller 0, CAP_TURN_HEAD) | Heads never turn; body turns slew instead (main.rs:15381 `prop_face_point`, 22 deg/tick). Scientists look stiffer. UNRECORDED. | open |
| S33 Sitting scientist behaviour: sits through the whole level, plays s | The seated NPC in the port stands up and runs around when alarmed. Retail stays seated. Port also does not speak. | open |
| CC1 Sequence cross-fade between clips (blend of idle/walk/attack) | Every state change snaps: scientists flinch, Barney draws/fires, Nihilanth swaps float to attack with no blend. | open |

### t0a0a

Play order 98 of 103, room id 97. Exits: t0a0@t0a0t0a0a[], t0a0b1@t0a0at0a0b[].

Original contents: 9 actors, 8 scripted sequences or sentences, 9 multi_managers, 35 triggers, 8 buttons, 3 doors. Classes: func_illusionary 33, func_wall 33, trigger_once 31, env_sound 28, infodecal 25, ambient_generic 14, env_glow 13, multi_manager 9, func_button 8, func_ladder 8, env_message 7, monster_generic 7 ....

Journey: none.

Static entity contract (`hl-census map t0a0a`, current main):

- Gap, the port does not honour it: func_breakable.explosion x3, func_door.health x3, func_pushable.explosion x1, path_corner.yaw_speed x2
- Partial, honoured with a stated deviation: env_message (flag:1 x1); env_render (flag:2 x2, flag:4 x4, flag:8 x4, rendercolor x6, renderfx x6); env_sound (@class x28, roomtype x28); func_breakable (renderamt x3, rendercolor x3, renderfx x3, rendermode x3); func_button (renderamt x8, rendercolor x8, renderfx x8, rendermode x8); func_door (renderamt x3, rendercolor x3, renderfx x3, rendermode x3); func_train (renderamt x1, rendercolor x1, renderfx x1, rendermode x1, skin x1); infodecal (@class x25); momentary_door (renderamt x1, rendercolor x1, renderfx x1, rendermode x1); momentary_rot_button (flag:64 x1); trigger_hurt (damagetype x1)
- Unreviewed, no one has checked the key against retail: env_glow (renderamt x13, rendercolor x13, renderfx x13, rendermode x13); env_sprite (framerate x4, renderamt x4, rendercolor x4, renderfx x4, rendermode x4, scale x4); func_illusionary (renderamt x33, rendercolor x8, renderfx x6, rendermode x33, skin x33); func_pushable (buoyancy x1, delay x1, renderamt x1, rendercolor x1, renderfx x1, rendermode x1, size x1); func_wall (renderamt x20, rendercolor x2, renderfx x2, rendermode x20); func_wall_toggle (renderamt x1, rendercolor x1, renderfx x1, rendermode x1)
- Cosmetic: 34 class/key pairs
  - func_breakable.explosion: the explosion gib mode is not read
  - func_door.health: a door with health opens when shot in retail; the port ignores it
  - path_corner.yaw_speed: trains turn at their own rate

### t0a0b

Play order 99 of 103, room id 98. Exits: t0a0a@t0a0at0a0b[], t0a0c@t0a0bt0a0c[], t0a0c@t0a0bt0a0c2[].

Original contents: 26 actors, 12 scripted sequences or sentences, 18 multi_managers, 30 triggers, 6 buttons, 10 doors. Classes: func_illusionary 39, infodecal 37, func_wall 36, func_breakable 30, trigger_once 25, ammo_mp5clip 23, env_sound 22, ambient_generic 21, multi_manager 18, env_glow 17, monster_cockroach 15, path_track 14 ....

Journey: `tests/hazard-panels.toml` runs a probe disc that stands the player at the number wall and checks the five panel lights (a diagnostic, not controller play).

Static entity contract (`hl-census map t0a0b`, current main):

- Gap, the port does not honour it: func_breakable.explosion x30, func_door.health x8, func_pushable.explosion x5
- Partial, honoured with a stated deviation: env_render (rendercolor x2, renderfx x2); env_sound (@class x22, roomtype x20); func_breakable (renderamt x9, rendercolor x9, renderfx x9, rendermode x9); func_button (renderamt x6, rendercolor x6, renderfx x6, rendermode x6); func_door (renderamt x8, rendercolor x8, renderfx x8, rendermode x8); func_door_rotating (renderamt x1, rendercolor x1, renderfx x1, rendermode x1); infodecal (@class x37); momentary_door (renderamt x1, rendercolor x1, renderfx x1, rendermode x1)
- Unreviewed, no one has checked the key against retail: env_glow (renderamt x17, rendercolor x17, renderfx x17, rendermode x17); func_guntarget (health x7, renderamt x4, rendercolor x4, renderfx x4, rendermode x4, speed x7); func_illusionary (renderamt x39, rendercolor x6, renderfx x6, rendermode x39, skin x39); func_pushable (buoyancy x5, delay x5, renderamt x5, rendercolor x5, renderfx x5, rendermode x5, size x5); func_wall (renderamt x28, rendercolor x2, renderfx x2, rendermode x28, skin x1)
- Cosmetic: 37 class/key pairs
  - func_breakable.explosion: the explosion gib mode is not read
  - func_door.health: a door with health opens when shot in retail; the port ignores it

Inventory rows first met here (10-15 inventory, 1 DIFF or MISSING): 1 fixed on main, 0 open or unverified.

Fixed: Trigger message hint (UTIL_ShowMessage) (74ede31).

### t0a0b1

Play order 100 of 103, room id 99. Exits: t0a0a@t0a0at0a0b[], t0a0b2@t0a0b12[].

Original contents: 14 actors, 5 scripted sequences or sentences, 3 multi_managers, 19 triggers, 3 buttons, 4 doors. Classes: func_wall 26, func_breakable 24, func_illusionary 24, env_sound 17, trigger_once 16, env_glow 10, monster_cockroach 10, infodecal 8, world_items 6, ambient_generic 5, env_message 5, func_pushable 5 ....

Journey: none.

Static entity contract (`hl-census map t0a0b1`, current main):

- Gap, the port does not honour it: func_breakable.explosion x24, func_door.health x3, func_pushable.explosion x5
- Partial, honoured with a stated deviation: env_sound (@class x17, roomtype x15); func_breakable (renderamt x7, rendercolor x7, renderfx x7, rendermode x7); func_button (renderamt x3, rendercolor x3, renderfx x3, rendermode x3); func_door (renderamt x3, rendercolor x3, renderfx x3, rendermode x3); infodecal (@class x8); momentary_door (renderamt x1, rendercolor x1, renderfx x1, rendermode x1)
- Unreviewed, no one has checked the key against retail: env_glow (renderamt x10, rendercolor x10, renderfx x10, rendermode x10); func_illusionary (renderamt x24, rendercolor x2, renderfx x2, rendermode x24, skin x24); func_pushable (buoyancy x5, delay x5, renderamt x5, rendercolor x5, renderfx x5, rendermode x5, size x5); func_wall (renderamt x21, rendercolor x2, renderfx x2, rendermode x21, skin x1)
- Cosmetic: 31 class/key pairs
  - func_breakable.explosion: the explosion gib mode is not read
  - func_door.health: a door with health opens when shot in retail; the port ignores it

### t0a0b2

Play order 101 of 103, room id 100. Exits: t0a0c@t0a0bt0a0c[], t0a0c@t0a0bt0a0c2[], t0a0b1@t0a0b12[].

Original contents: 12 actors, 7 scripted sequences or sentences, 15 multi_managers, 14 triggers, 3 buttons, 6 doors. Classes: infodecal 29, ammo_mp5clip 23, func_illusionary 21, ambient_generic 16, multi_manager 15, path_track 14, ammo_mp5grenades 11, func_wall 11, trigger_once 10, env_glow 7, env_sound 7, func_guntarget 7 ....

Journey: none.

Static entity contract (`hl-census map t0a0b2`, current main):

- Gap, the port does not honour it: func_breakable.explosion x6, func_door.health x5
- Partial, honoured with a stated deviation: env_render (rendercolor x2, renderfx x2); env_sound (@class x7, roomtype x7); func_breakable (renderamt x2, rendercolor x2, renderfx x2, rendermode x2); func_button (renderamt x3, rendercolor x3, renderfx x3, rendermode x3); func_door (renderamt x5, rendercolor x5, renderfx x5, rendermode x5); func_door_rotating (renderamt x1, rendercolor x1, renderfx x1, rendermode x1); infodecal (@class x29)
- Unreviewed, no one has checked the key against retail: env_glow (renderamt x7, rendercolor x7, renderfx x7, rendermode x7); func_guntarget (health x7, renderamt x4, rendercolor x4, renderfx x4, rendermode x4, speed x7); func_illusionary (renderamt x21, rendercolor x4, renderfx x4, rendermode x21, skin x21); func_wall (renderamt x8, rendercolor x1, renderfx x1, rendermode x8)
- Cosmetic: 30 class/key pairs
  - func_breakable.explosion: the explosion gib mode is not read
  - func_door.health: a door with health opens when shot in retail; the port ignores it

### t0a0c

Play order 102 of 103, room id 101. Exits: t0a0b2@t0a0bt0a0c[], t0a0b@t0a0bt0a0c2[], t0a0d@a1a2[changetoc1a2].

Original contents: 7 actors, 4 scripted sequences or sentences, 6 multi_managers, 19 triggers, 5 buttons, 8 doors. Classes: func_wall 20, env_sound 14, trigger_once 12, func_illusionary 11, env_glow 9, env_sprite 9, ambient_generic 8, func_door 8, multi_manager 6, func_button 5, monster_generic 4, env_message 3 ....

Journey: none.

Static entity contract (`hl-census map t0a0c`, current main):

- Gap, the port does not honour it: func_breakable.explosion x1, func_door.health x8
- Partial, honoured with a stated deviation: env_explosion (@class x1); env_shake (@class x1, frequency x1, radius x1); env_sound (@class x14, roomtype x14); func_button (renderamt x5, rendercolor x5, renderfx x5, rendermode x5); func_door (renderamt x8, rendercolor x8, renderfx x8, rendermode x8); infodecal (@class x2); trigger_hurt (damagetype x3)
- Unreviewed, no one has checked the key against retail: env_glow (renderamt x9, rendercolor x9, renderfx x9, rendermode x9); env_sprite (framerate x9, renderamt x9, rendercolor x9, renderfx x9, rendermode x9, scale x5); func_illusionary (renderamt x11, rendermode x11, skin x11); func_wall (renderamt x11, rendercolor x1, rendermode x11)
- Cosmetic: 25 class/key pairs
  - func_breakable.explosion: the explosion gib mode is not read
  - func_door.health: a door with health opens when shot in retail; the port ignores it

### t0a0d

Play order 103 of 103, room id 102. Exits: END, t0a0c@a1a2[changetoc1a1e].

Original contents: 7 actors, 4 scripted sequences or sentences, 4 multi_managers, 13 triggers, 14 buttons, 7 doors. Classes: func_button 14, ambient_generic 11, env_sound 11, func_wall 11, func_illusionary 9, func_door 6, trigger_once 5, multi_manager 4, env_beverage 3, env_message 3, func_breakable 3, scripted_sentence 3 ....

Journey: none.

Static entity contract (`hl-census map t0a0d`, current main):

- Gap, the port does not honour it: env_spark.maxdelay x1, func_breakable.explosion x3, func_door.health x5
- Partial, honoured with a stated deviation: env_fade (renderamt x1, rendercolor x1); env_shake (@class x1, frequency x1, radius x1); env_shooter (@class x1); env_sound (@class x11, roomtype x11); env_spark (@class x2); func_breakable (renderamt x2, rendercolor x2, renderfx x1, rendermode x2); func_button (renderamt x14, rendercolor x14, renderfx x14, rendermode x14); func_door (renderamt x4, rendercolor x4, renderfx x4, rendermode x4); func_door_rotating (renderamt x1, rendercolor x1, renderfx x1, rendermode x1); func_tracktrain (renderamt x2, rendercolor x2, renderfx x2, rendermode x2); trigger_hurt (damagetype x1)
- Unreviewed, no one has checked the key against retail: env_beverage (health x1, skin x2); env_shooter (delay x1, m_flvariance x1, m_flvelocity x1, m_igibs x1, renderamt x1, rendercolor x1, renderfx x1, rendermode x1, shootsounds x1); func_illusionary (renderamt x9, rendermode x9, skin x9); func_pushable (buoyancy x1, renderamt x1, rendercolor x1, renderfx x1, rendermode x1, size x1); func_wall (renderamt x1, rendercolor x1, renderfx x1, rendermode x1); scripted_sequence (flag:64 x1)
- Cosmetic: 39 class/key pairs
  - env_spark.maxdelay: spark interval is fixed
  - func_breakable.explosion: the explosion gib mode is not read
  - func_door.health: a door with health opens when shot in retail; the port ignores it
