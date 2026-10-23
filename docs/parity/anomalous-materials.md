# Anomalous Materials (c1a0 to c1a0e)

Retail order inside the chapter: c1a0, c1a0d, c1a0a, c1a0b, c1a0e. The resonance cascade in c1a0e hands over to c1a0c, whose worldspawn carries the title of the next chapter (see [unforeseen-consequences.md](unforeseen-consequences.md)). Method and evidence kinds: [README.md](README.md).

Chapter result: traversed end to end by `tests/chain-new-game.toml`, controller input only. The c1a0e step is closed loop: it presses the console, rides the lift and pushes the sample cart into the beam with `hold_until` conditions on the player's yaw, height and the cart's own x (`ENT_CACHE+1732`), so the push does not depend on where a frame ended a hold. The route was written against the Xash3D reference runtime and re-solved on the PS1 where geometry or physics demanded it (comments in `tests/chain/inbound-anomalous.route`).

State column: `fixed` = a commit or CHANGELOG entry on main fixes it; `limit-recorded` = a LIMITS-*.md file records it; `limit-claimed` = the inventory calls it a PS1 limit but no LIMITS file lists it (a bug under the completion standard until it is written down with evidence); `limit-unrecorded` = same, and no code comment records it either; `open` = the 10-15 inventory read the SDK and the port and found a difference that nobody has re-checked on this build.

### c1a0

Play order 7 of 103, room id 6. Exits: c0a0e@c0a0toc1a0[changetoc1a0], c1a0d@c1a0toc1a0d[].

Original contents: 17 actors, 61 scripted sequences or sentences, 11 multi_managers, 15 triggers, 3 buttons, 16 doors. Classes: scripted_sequence 47, func_wall 32, ambient_generic 15, scripted_sentence 14, multi_manager 11, func_door 10, monster_scientist 9, trigger_once 9, func_breakable 7, func_door_rotating 6, monster_sitting_scientist 4, func_button 3 ....

Journey: controller-only. `tests/chain-new-game.toml` waits for the load (map word 5733) and first simulation tick, replays a poll-bound tape of 624 controller samples (`tests/chain/tapes/c1a0-00.pxtape`, compiled from `inbound-anomalous.route`), with a one-tick turn at the start to restore the heading the tram ride leaves 3.9 degrees off, and ends when the game itself changes level to c1a0d. Checkpoint `arrive-c1a0` asserts a non-flat frame and an unchanged card. Evidence kind: emulator replay, CPU renderer, `--no-hw`; no golden image.

Static entity contract (`hl-census map c1a0`, current main):

- Gap, the port does not honour it: func_breakable.explosion x7, func_door.health x10, scripted_sentence.listener x9
- Partial, honoured with a stated deviation: env_render (rendercolor x2, renderfx x2); env_sound (@class x2, roomtype x2); func_breakable (renderamt x7, rendercolor x7, renderfx x6, rendermode x6); func_button (renderamt x3, rendercolor x3, renderfx x3, rendermode x3); func_door (flag:4 x4, renderamt x8, rendercolor x8, renderfx x6, rendermode x8); func_door_rotating (renderamt x6, rendercolor x6, renderfx x6, rendermode x6)
- Unreviewed, no one has checked the key against retail: env_sprite (framerate x1, renderamt x1, rendercolor x1, renderfx x1, rendermode x1); func_illusionary (skin x1); func_pushable (buoyancy x2, size x2); func_rotating (renderamt x1, rendercolor x1, renderfx x1, rendermode x1, sounds x1); func_wall (renderamt x8, rendercolor x8, renderfx x7, rendermode x8); scripted_sentence (flag:4 x11, flag:8 x1); scripted_sequence (flag:16 x3)
- Cosmetic: 31 class/key pairs
  - func_breakable.explosion: the explosion gib mode is not read
  - func_door.health: a door with health opens when shot in retail; the port ignores it

Inventory rows first met here (10-15 inventory, 23 DIFF or MISSING): 4 fixed on main, 19 open or unverified.

Fixed: D4 (CHANGELOG); D10 (CHANGELOG); S7 (4f81bc1); S9 (8ff3b01).

| inventory row | difference (inventory wording, shortened) | state |
|---|---|---|
| env_sound room effect | PS1 LIMIT. The 29 GoldSrc DSP presets are approximated with SPU reverb parameters; entity-contract.txt and entity-support.txt record that filter, modulation and stereo to | limit-claimed |
| D8 | PS1 LIMIT (recorded: comment at main.rs:5953-5957): 32 entries keyed by hash; collision or overflow silently drops. Whether any map uses >32 globals is UNVERIFIED. | limit-recorded |
| F27 How routes are used: original builds a node route for every far go | Enemies can bump along a wall before routing and orbit obstacles; the graph handles corners but not general detours. Documented as a deliberate fix for orbiting in c1a0 ( | limit-recorded |
| G8 | Retail lists every ammo and weapon pickup with icons for 5 s; port shows one icon for three item types and nothing for ammo or weapons. PS1 LIMIT, UNRECORDED (draw cap 34 | limit-unrecorded |
| S5 Use replies SC_OK / SC_WAIT / SC_POK | One fixed sentence per map, the original picks a random sentence from the group. Hearing the same line every time. PS1 LIMIT (per-map SPU voice budget, inferred): UNRECOR | limit-unrecorded |
| Bullet, blood and scorch decals | PS1 LIMIT: only the last 24 marks live and each expires after 9 s; the SDK/engine keeps decals for the level (hundreds). Explosions leave no scorch mark. The constants ca | limit-unrecorded |
| H6 | All values are constant (head 3, others 1); monster head x3 present, player head x3 absent (C2). | open |
| I6 |  | open |
| J6 | Port sounds are quieter at mid range (400 u: 0.33 vs 0.68) and audible slightly farther. 21 `play_world` call sites. | open |
| S11 Player-hurt comments (SC_CUREA/B/C at 1/2, 1/4, 1/8 max health) wh | Scientist never remarks on your health. Cosmetic. | open |
| S18 Pain and death sounds (5 pain wavs, DeathSound = PainSound) | Scientists die silently. Barney has them (B14). UNRECORDED. | open |
| S19 Pain reaction: light-damage flinch sequence | Port flinch 0.2 s, retail about 1 s; retail also picks limb flinch by hit group. Minor. | open |
| S20 Death: sequence picked by hit direction / damage type | One death animation for every kill. Cosmetic. | open |
| S23 Nearby friend scolds the player when a talk monster is hurt by him | No reaction line. Cosmetic. | open |
| G3 G-Man head turns toward the player or talk target (bone controller  | He never looks at Gordon; cosmetic but memorable (c1a0, c5a1 endgame). UNRECORDED. | open |
| Script state gate (CanPlaySequence, OVERRIDESTATE) | The SDK refuses to possess a monster that is alerted or in combat unless the sequence sets flag 64; the port possesses regardless. 326 of 429 sequences lack flag 64. | open |
| env_render | Copies only a coarse blend class to brushes and hides actors; renderfx, rendercolor and mode masks (SF 1 to 8) are not applied (119 placements, 28 maps). | open |
| Muzzle flashes | Additive star at the viewmodel muzzle attachment for the 14 gun slots and a short streak for monsters instead of the muzzleflash1 to 3 sprite frames; weapons agent owns a | open |
| S10 Syringe visuals: pull_needle / return_needle sequences, body switc | Goes with S9. | partial |

### c1a0d

Play order 8 of 103, room id 10. Exits: c1a0a@c1a0dtoa[airlocktransition1], c1a0@c1a0toc1a0d[].

Original contents: 9 actors, 34 scripted sequences or sentences, 9 multi_managers, 19 triggers, 19 buttons, 14 doors. Classes: scripted_sequence 31, func_wall 30, func_button 19, ambient_generic 18, func_door 13, func_breakable 9, infodecal 9, multi_manager 9, multisource 8, trigger_once 8, monster_scientist 6, trigger_multiple 6 ....

Journey: controller-only. `tests/chain-new-game.toml` waits for the load (map word 5541) and first simulation tick, replays a poll-bound tape of 1641 controller samples (`tests/chain/tapes/c1a0d-00.pxtape`, compiled from `inbound-anomalous.route`); asserts the suit is not yet worn (`LOGIC_PLAYER_SUIT` = 0) at arrival, and ends when the game itself changes level to c1a0a. Checkpoint `arrive-c1a0d` asserts a non-flat frame and an unchanged card, and the suit and health words where the route claims them. Evidence kind: emulator replay, CPU renderer, `--no-hw`; no golden image.

Static entity contract (`hl-census map c1a0d`, current main):

- Gap, the port does not honour it: func_breakable.explosion x9, func_door.health x13
- Partial, honoured with a stated deviation: env_render (rendercolor x1, renderfx x1); func_breakable (renderamt x6, rendercolor x6, renderfx x4, rendermode x6); func_button (renderamt x19, rendercolor x19, renderfx x15, rendermode x19); func_door (flag:4 x8, renderamt x13, rendercolor x13, renderfx x12, rendermode x13); func_door_rotating (renderamt x1, rendercolor x1, renderfx x1, rendermode x1); infodecal (@class x9)
- Unreviewed, no one has checked the key against retail: env_beam (boltwidth x2, damage x1, flag:1 x1, flag:128 x1, flag:2 x1, framerate x2, framestart x2, life x2, lightningend x2, lightningstart x2, noiseamplitude x2, radius x2, renderamt x2, rendercolor x2, renderfx x1, striketime x2, texture x2, texturescroll x2); env_beverage (health x2, skin x2); func_illusionary (renderamt x1, rendercolor x1, renderfx x1, rendermode x1, skin x1); func_rotating (renderamt x2, rendercolor x2, renderfx x2, rendermode x2, sounds x2, spawnorigin x2, volume x2); func_wall (flag:1 x2, renderamt x13, rendercolor x4, renderfx x4, rendermode x13); scripted_sentence (@kill x0, flag:4 x3); scripted_sequence (flag:256 x1)
- Cosmetic: 30 class/key pairs
  - func_breakable.explosion: the explosion gib mode is not read
  - func_door.health: a door with health opens when shot in retail; the port ignores it

### c1a0a

Play order 9 of 103, room id 7. Exits: c1a0d@c1a0dtoa[airlocktransition1], c1a0b@c1a0atob[].

Original contents: 5 actors, 4 scripted sequences or sentences, 11 multi_managers, 14 triggers, 4 buttons, 45 doors. Classes: func_door 29, func_wall 27, func_door_rotating 16, ambient_generic 13, env_beam 11, multi_manager 11, info_target 10, env_sound 9, trigger_multiple 8, func_button 4, monster_scientist 4, scripted_sequence 4 ....

Journey: controller-only. `tests/chain-new-game.toml` waits for the load (map word 5414) and first simulation tick, replays a poll-bound tape of 2088 controller samples (`tests/chain/tapes/c1a0a-00.pxtape`, compiled from `inbound-anomalous.route`); asserts suit worn and health at least 100 at arrival, and ends when the game itself changes level to c1a0b. Checkpoint `arrive-c1a0a` asserts a non-flat frame and an unchanged card, and the suit and health words where the route claims them. Evidence kind: emulator replay, CPU renderer, `--no-hw`; no golden image.

Static entity contract (`hl-census map c1a0a`, current main):

- Gap, the port does not honour it: func_door.health x22
- Partial, honoured with a stated deviation: env_sound (@class x9, roomtype x9); func_button (renderamt x3, rendercolor x2, renderfx x2, rendermode x3); func_door (flag:4 x26, renderamt x24, rendercolor x22, renderfx x22, rendermode x24); func_door_rotating (renderamt x16, rendercolor x16, renderfx x16, rendermode x16); speaker (@class x1)
- Unreviewed, no one has checked the key against retail: env_beam (boltwidth x11, damage x11, flag:1 x11, flag:4 x11, framerate x11, framestart x11, life x11, lightningend x11, lightningstart x11, noiseamplitude x11, radius x11, renderamt x11, rendercolor x11, renderfx x11, striketime x11, texture x11, texturescroll x11); func_rotating (renderamt x1, rendercolor x1, renderfx x1, rendermode x1, sounds x1, spawnorigin x1, volume x1); func_wall (renderamt x7, rendercolor x2, renderfx x4, rendermode x7); speaker (health x1, preset x1)
- Cosmetic: 30 class/key pairs
  - func_door.health: a door with health opens when shot in retail; the port ignores it

Inventory rows first met here (10-15 inventory, 2 DIFF or MISSING): 0 fixed on main, 2 open or unverified.

| inventory row | difference (inventory wording, shortened) | state |
|---|---|---|
| func_door defaults for missing wait and lip keys | 31 func_doors carry no wait key and 64 doors no lip key (31 of them bare model-only doors; 20 more are real doors in c2a2e, c2a3a, c2a3b, c2a5c). The SDK closes a no-wait | open |
| Announcer, speaker and sentence ambients | Only lines present in the per-map voice pack play; the speaker's sentence groups are folded into compact ambience and its random 5 to 30 s repeat is not modelled. 31 spea | open |

### c1a0b

Play order 10 of 103, room id 8. Exits: c1a0a@c1a0atob[], c1a0c@c1a0btoc[change_level], c1a0e@c1a0btoe[change_to_c1a0e].

Original contents: 8 actors, 18 scripted sequences or sentences, 10 multi_managers, 12 triggers, 6 buttons, 23 doors. Classes: func_wall 51, ambient_generic 29, func_door 20, scripted_sequence 11, multi_manager 10, monster_scientist 7, scripted_sentence 7, env_sound 6, func_button 6, env_beam 5, env_explosion 4, func_breakable 4 ....

Journey: controller-only. `tests/chain-new-game.toml` waits for the load (map word 4954) and first simulation tick, replays a poll-bound tape of 1820 controller samples (`tests/chain/tapes/c1a0b-00.pxtape`, compiled from `inbound-anomalous.route`), and ends when the game itself changes level to c1a0e. Checkpoint `arrive-c1a0b` asserts a non-flat frame and an unchanged card, and the suit and health words where the route claims them. Evidence kind: emulator replay, CPU renderer, `--no-hw`; no golden image.

Static entity contract (`hl-census map c1a0b`, current main):

- Gap, the port does not honour it: env_spark.maxdelay x1, func_breakable.explosion x4, func_door.health x18, scripted_sentence.listener x6
- Partial, honoured with a stated deviation: env_explosion (@class x4, flag:1 x3, flag:32 x1); env_sound (@class x6, roomtype x6); env_spark (@class x1); func_breakable (renderamt x1, rendercolor x1, renderfx x1, rendermode x1); func_button (renderamt x6, rendercolor x6, renderfx x6, rendermode x6); func_door (flag:4 x12, renderamt x18, rendercolor x18, renderfx x17, rendermode x18); func_door_rotating (renderamt x3, rendercolor x3, renderfx x3, rendermode x3); func_train (renderamt x1, rendercolor x1, renderfx x1, rendermode x1, skin x1)
- Unreviewed, no one has checked the key against retail: env_beam (boltwidth x5, damage x2, flag:1 x3, flag:4 x2, framerate x5, framestart x5, life x5, lightningend x2, lightningstart x5, noiseamplitude x5, radius x5, renderamt x5, rendercolor x5, renderfx x2, striketime x5, texture x5, texturescroll x5); env_glow (renderamt x3, rendercolor x3, renderfx x3, rendermode x3); env_sprite (framerate x2, renderamt x2, rendercolor x2, renderfx x2, rendermode x2); func_platrot (@use x0, flag:1 x1, height x1, movesnd x1, renderamt x1, rendercolor x1, renderfx x1, rendermode x1, rotation x1, speed x1, stopsnd x1); func_rotating (renderamt x1, rendercolor x1, renderfx x1, rendermode x1, sounds x1, spawnorigin x1, volume x1); func_wall (flag:1 x4, renderamt x26, rendercolor x5, renderfx x12, rendermode x26); scripted_sentence (flag:4 x7)
- Cosmetic: 32 class/key pairs
  - env_spark.maxdelay: spark interval is fixed
  - func_breakable.explosion: the explosion gib mode is not read
  - func_door.health: a door with health opens when shot in retail; the port ignores it

Inventory rows first met here (10-15 inventory, 2 DIFF or MISSING): 1 fixed on main, 1 open or unverified.

Fixed: env_explosion (b51a812).

| inventory row | difference (inventory wording, shortened) | state |
|---|---|---|
| Delayed-event queue capacity | PS1 LIMIT. With all 64 slots busy a new delayed output is dropped, and a multi_manager that cannot queue completes at once. Recorded in a code comment (main.rs:12311) onl | limit-recorded |

### c1a0e

Play order 11 of 103, room id 11. Exits: c1a0c@c1a0etoc[change_level], c1a0b@c1a0btoe[].

Original contents: 9 actors, 16 scripted sequences or sentences, 45 multi_managers, 57 triggers, 3 buttons, 14 doors. Classes: ambient_generic 90, multi_manager 45, func_wall 32, trigger_relay 30, env_sprite 28, env_beam 25, cycler 21, info_target 18, env_explosion 12, path_corner 12, trigger_teleport 12, func_door 10 ....

Journey: controller-only with closed-loop holds. Tapes `c1a0e-00` (2,179 samples) and `c1a0e-01` (2,048), three 8-sample tapes, and `hold_until` steps on `LOGIC_PLAYER_YAW`, `LOGIC_PLAYER_POS` and the cart entity's x; checkpoints `cart-aligned`, `cart-delivered` (cart entity x in 1516..1700 and y 590..650 asserted), `cart-pushed`; then the game fires the cascade and changes level to c1a0c by itself (wait on map word 5816, timeout 30,000 ticks). Evidence kind: emulator replay, CPU renderer, `--no-hw`.

Static entity contract (`hl-census map c1a0e`, current main):

- Gap, the port does not honour it: env_spark.maxdelay x6, func_breakable.explosion x8, func_door.health x10, func_pushable.explosion x1, scripted_sentence.listener x7
- Partial, honoured with a stated deviation: env_explosion (@class x12, flag:1 x8, flag:32 x4); env_render (rendercolor x2, renderfx x2); env_shake (@class x5, flag:1 x5, frequency x5, radius x5); env_sound (@class x2, roomtype x2); env_spark (@class x6); func_breakable (renderamt x8, rendercolor x8, renderfx x8, rendermode x8); func_button (renderamt x3, rendercolor x3, renderfx x3, rendermode x3); func_door (renderamt x9, rendercolor x10, renderfx x8, rendermode x9); func_door_rotating (renderamt x4, rendercolor x4, renderfx x4, rendermode x4); func_train (renderamt x1, rendercolor x1, renderfx x1, rendermode x1, skin x1); monstermaker (@class x4); trigger_hurt (damagetype x3); trigger_teleport (flag:1 x4)
- Unreviewed, no one has checked the key against retail: ambient_generic (@kill x0); env_beam (boltwidth x25, damage x15, flag:1 x5, flag:2 x13, flag:32 x2, framerate x25, framestart x25, life x25, lightningend x17, lightningstart x25, noiseamplitude x25, radius x17, renderamt x25, rendercolor x25, renderfx x15, striketime x25, texture x25, texturescroll x25); env_glow (renderamt x9, rendercolor x9, renderfx x9, rendermode x9); env_sprite (framerate x28, renderamt x28, rendercolor x28, renderfx x26, rendermode x28, scale x10); func_illusionary (renderamt x3, rendercolor x3, renderfx x3, rendermode x3, skin x3); func_pushable (buoyancy x1, delay x1, renderamt x1, rendercolor x1, renderfx x1, rendermode x1, size x1); func_rotating (renderamt x1, rendercolor x1, renderfx x1, rendermode x1, sounds x1, spawnorigin x1, volume x1); func_wall (@kill x0, flag:1 x2, renderamt x21, rendercolor x2, renderfx x9, rendermode x21); func_water (dmg x1, health x1, lip x1, locked_sentence x1, locked_sound x1, movesnd x1, renderamt x1, rendercolor x1, rendermode x1, skin x1, speed x1, stopsnd x1, unlocked_sentence x1, unlocked_sound x1, wait x1); scripted_sentence (flag:4 x8, flag:8 x8); trigger_teleport (@kill x0)
- Cosmetic: 32 class/key pairs
  - env_spark.maxdelay: spark interval is fixed
  - func_breakable.explosion: the explosion gib mode is not read
  - func_door.health: a door with health opens when shot in retail; the port ignores it

Inventory rows first met here (10-15 inventory, 14 DIFF or MISSING): 1 fixed on main, 13 open or unverified.

Fixed: F36 (8ff3b01).

| inventory row | difference (inventory wording, shortened) | state |
|---|---|---|
| Water surface, fog and underwater look | PS1 LIMIT. No surface wave warp and no refraction; underwater is a colour grade; a depth fog and far cull at 1200 units (original 4096) are added. Recorded in code commen | limit-claimed |
| F01 Think cadence: AI thinks at 10 Hz in the original; port runs 20 Hz | PS1 LIMIT (trace budget). Recorded in code comment main.rs:3799-3806 and 16282; not in CHANGELOG/README. Reaction lag up to 0.2 s extra | limit-recorded |
| F04 Field of view per monster (`m_flFieldOfView`: grunt 0.2, headcrab/ | Only the zombie has a cone. All other monsters see 360 degrees within wake range, so approaching from behind never works and headcrabs notice at once. The cooker has per- | limit-recorded |
| F07 Hearing: CSoundEnt list (combat, world, player, danger, meat/carca | Gunfire and explosions never alert or turn monsters; grunts never take cover from a grenade; the tentacle's whole input is sound. Recorded in a cook comment only | limit-recorded |
| F12 Enemy memory: last known position (LKP), `PushEnemy`/`PopEnemy` st | Hunters know the player's true position through walls for as long as the target is alive (only the nav graph and local blocking limit them). Breaking line of sight does n | limit-recorded |
| F13 Monster states IDLE / ALERT / COMBAT / HUNT / SCRIPT / PRONE / DEA | No ALERT or HUNT. Idle monsters do not turn toward a sound, do not play the random idle voice (1 in 100 per think), and have no patrol/active-idle schedules. Scripted sta | limit-recorded |
| F14 Schedule/task/condition engine (about 90 TASK_*, 45 common schedul | Behaviour that falls out of schedules (investigate sound, standoff, fail-and-retry, victory dance, take cover, wait for script) must be reimplemented per case. Most such  | limit-recorded |
| F15 Damage conditions: LIGHT_DAMAGE on any hit, HEAVY_DAMAGE at 20 or  | A hit plays a 0.2 s pain pose but never interrupts an attack, move or aim. Hit-and-flinch pacing is absent; shotgun and rocket hits do not stagger anything | limit-recorded |
| HC03 Leap flight: velocity chosen to reach the enemy eye in sqrt(2h/g) | Flight time similar for a standing player; no arc, no 650 clamp, crabs do not overshoot or fall short | limit-recorded |
| F03 Sight range: `m_flDistLook` 2048 box for every monster (leech 750) | Port wakes monsters at roughly 700-1400 units depending on species instead of 2048. Long sight lines in big rooms (Xen, c2a5, c4a1 caves) engage later than retail. UNRECO | open |
| F16 Pain voice: per species, throttled per monster (zombie 2 in 6, agr | About a dozen type ids have pain and death cries (alien grunt and controller reuse the vortigaunt's); assassin, tentacle, roach and leech are silent. A shared 0.5 s gate  | open |
| HC02 Leap trigger: on ground, distance <= 256, facing dot >= 0.65 | Range matches; no facing test, so a crab will launch while still turning (turn rate F30). Leaps are visible a little earlier than retail | open |
| HC05 Attack cadence: next attack 2 s after the leap event, plus 0-0.5  | Slightly higher bite rate than retail (1.75 s vs 2.0-2.5 s) | open |
