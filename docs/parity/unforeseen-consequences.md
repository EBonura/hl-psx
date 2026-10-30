# Unforeseen Consequences (c1a0c to c1a1d)

Retail order: c1a0c, c1a1, c1a1a, c1a1f, c1a1b, c1a1c, c1a1d (c1a1c also leads on to c1a2, Office Complex). The chapter title is shown on arrival in c1a0c. Method and evidence kinds: [README.md](README.md).

Chapter result: c1a0c and c1a1 are traversed by the chain, which arrives in c1a1a. None of c1a1a, c1a1f, c1a1b, c1a1c or c1a1d has a journey that plays the map yet. `tests/unstick-vent.toml` and `tests/present-cap.toml` replay a recorded playtest tape into c1a1b for two narrow checks (a hull embedded in the vent clip slab; display flip spacing) and are not segment journeys. The open-loop draft for c1a1 in `inbound-anomalous.route` after its `@stop` does not reach the exit (it ends at (263, 802) after 1,584 ticks).

State column: `fixed` = a commit or CHANGELOG entry on main fixes it; `limit-recorded` = a LIMITS-*.md file records it; `limit-claimed` = the inventory calls it a PS1 limit but no LIMITS file lists it (a bug under the completion standard until it is written down with evidence); `limit-unrecorded` = same, and no code comment records it either; `open` = the 10-15 inventory read the SDK and the port and found a difference that nobody has re-checked on this build.

### c1a0c

Play order 12 of 103, room id 9. Exits: c1a1@c1a0catoc1a1[], c1a0e@c1a0etoc[change_level].

Original contents: 5 actors, 14 scripted sequences or sentences, 16 multi_managers, 41 triggers, 9 buttons, 9 doors. Classes: infodecal 107, env_beam 52, info_target 45, func_wall 40, func_breakable 22, env_spark 19, trigger_once 17, multi_manager 16, ambient_generic 15, path_corner 10, func_button 9, scripted_sequence 9 ....

Journey: arrival, the first airlock, the pump room and the pipe corridor, through the lift, the console scientist, the retinal door and the control room to the exit. After the cart push, `tests/chain-new-game.toml` waits for the load of c1a0c (map word 5816); the player arrives at (1257, 614, z -348) facing east, the same state the diagnostic segment launch and the retail runtime start from. The route then runs closed loop (`@hold` on the player's x and y) through the first airlock panel, the pump room and the pipe corridor to the elevator lobby, with checkpoints `c-p1`, `c-p2`, `c-p3`, `c-u1`, `pipes-crossed`, `lobby-west` and `elevator-lobby`. Further checkpoints: `lift-called`, `lift-top`, `console-room`, `retinal-door-open`, `control-room-beams`. Evidence kind: emulator replay, CPU renderer, `--no-hw`.

Path parity against the retail engine (evidence kind: emulator replay against a trace of the Xash3D reference runtime, same route units, 20 Hz ticks; the trace logs the player and every fired target per tick). The route was derived from that trace and run closed loop on the PS1. Positions at the end of each leg, PS1 against retail:

| leg | what the player does | retail (x, y) | PS1 (x, y) |
|---|---|---|---|
| start | arrival, wake in the cascade room | (1257, 614) | (1257, 614) |
| 1 | strafe south and hop onto the ledge, run east, hop, strafe south to the first airlock panel | (1532, -69) | (1532, -69) |
| 2 | press the broken-airlock button, wait for the airlock to break (220 ticks) | (1532, -69) | (1532, -69) |
| 3 | west along the south corridor, north along the pump room | (816, 440) | (816, 433) |
| 4 | west through the pipe corridor (two fallen pipes cross it) | (284, 645) | (285, 635) |
| 5 | through the door at x 264 (`edoor_2`, fired by the trigger at x 292) to the elevator lobby | (250, 624) | (234, 629) |
| 6 | into the elevator cab, call button | (156, 623) | (158, 631) |
| 7 | ride to z -100, leave the cab south, walk to the console scientist | (157, 72) | (157, 72) |
| 8 | use the scientist, walk to the retinal scanner, wait for the door | (769, 255) | (783, 235) |
| 9 | through the control room beams to the exit at (574, 984) | changelevel at (574, 984) | reaches the changelevel |

The elevator is not a bug. An earlier version of this ledger said the cab started 216 units too high and a press sent it down onto the player. The probe was misread: `ENT_PHASE` of the cab is 4096 (the authored lobby position, z -218) from the first tick, as GoldSrc starts a named func_platrot at its TOP with its end angle. The route had a `use` at (234, 629) that the retail player never makes (retail presses use at (249, 624) and nothing happens, 65 units from the in-cab button); on the PS1 the player stood 15 units closer, inside the 64-unit search sphere, so the press called the cab away before the player boarded, and the next press sent it back down onto an empty shaft. With that press removed the cab carries the player to z -100 in 120 ticks, as retail does (checkpoints `lift-called`, `lift-top`).

The console scientist is the other gate. The retinal door opens only when the scripted sequence `control_retinal1` finds a scientist within 150 units of the scanner, and the only scientist is `console_guy`, who gets there because the player presses use on him at (157, 72) and he follows (a follower on the PS1 stops about 130 units behind the player, so the player has to stand at x 800 or beyond for him to come within range; the retail follower came closer, and why is not measured). The door is entity 64 of c1a0c (`ENT_PHASE` offset 256), waited on in the journey.

Differences still open here:

1. The fallen pipes in the west corridor (x about 572, y 625 to 669). Retail clears them with a plain jump (apex z -279.3, 44.7 units). Open-air jumps on the PS1 match retail exactly (apex -279, same per-tick rise 12.4, 10.4, 8.4 ...), so jump velocity, gravity and the frame step are not the cause. The PS1 player rests at x 572 against the pipe end (retail 571.0, flush with the hull face at x 570.97), slides along the hull edge during the rise, and lands on the flat shelf at z -299.5 (y 648 and up) instead of the slope (hull floor -284 at x 565, -268.5 at x 500). The PS1 lateral drift during the jump is about 2 units a tick in y against retail 0.8. The route uses a crouch jump. A trace of the slide legs (deep-reference-trace build) is the next step.
2. Leaving the cab, the PS1 player is pushed 30 to 35 units west (x 155 to 121 over y 584 to 472) in some runs and not in others; retail walks straight out at x 153. The route steers x closed loop. The cause (the rotated cab hull's door slot, or the rider carry) is not found.
3. The console scientist's follower stops at about 130 units from the player on the PS1, which makes the sequence radius of 150 unreachable from the retail standing point (see above).
4. Health after the beams: retail loses 57 in the control room; the PS1 journey only asserts health in 1..100.

Static entity contract (`hl-census map c1a0c`, current main):

- Gap, the port does not honour it: env_spark.maxdelay x18, func_breakable.explosion x22, func_door.health x6, path_corner.yaw_speed x10, scripted_sentence.listener x1
- Partial, honoured with a stated deviation: env_explosion (@class x6, flag:1 x5); env_fade (renderamt x1, rendercolor x1); env_render (rendercolor x2, renderfx x2); env_shake (@class x3, frequency x3, radius x3); env_sound (@class x7, roomtype x7); env_spark (@class x19); func_breakable (renderamt x22, rendercolor x19, renderfx x16, rendermode x22); func_button (renderamt x9, rendercolor x9, renderfx x9, rendermode x9); func_door (flag:4 x7, renderamt x6, rendercolor x6, renderfx x6, rendermode x6); func_door_rotating (renderamt x2, rendercolor x2, renderfx x2, rendermode x2); func_train (renderamt x2, rendercolor x2, renderfx x2, rendermode x2, skin x2); infodecal (@class x107); monstermaker (@class x1); trigger_hurt (damagetype x4)
- Unreviewed, no one has checked the key against retail: ambient_generic (@kill x0); env_beam (boltwidth x52, damage x45, flag:1 x7, flag:2 x13, flag:32 x9, flag:4 x1, framerate x52, framestart x52, life x52, lightningend x49, lightningstart x52, noiseamplitude x52, radius x47, renderamt x52, rendercolor x52, renderfx x33, striketime x52, texture x52, texturescroll x52); env_glow (renderamt x6, rendercolor x6, renderfx x6, rendermode x6); env_sprite (framerate x8, renderamt x8, rendercolor x8, renderfx x8, rendermode x8); func_illusionary (flag:8 x1, renderamt x1, rendercolor x1, renderfx x1, rendermode x1, skin x1); func_pendulum (damp x6, renderamt x5, rendercolor x6, renderfx x5, rendermode x5); func_platrot (@use x0, flag:1 x1, height x1, movesnd x1, renderamt x1, rendercolor x1, renderfx x1, rendermode x1, rotation x1, speed x1, stopsnd x1); func_rotating (renderamt x2, rendercolor x4, renderfx x4, rendermode x4, sounds x4); func_wall (flag:1 x4, flag:32 x1, flag:4 x1, renderamt x23, rendercolor x8, renderfx x10, rendermode x23); scripted_sentence (flag:4 x4); scripted_sequence (flag:64 x2)
- Cosmetic: 35 class/key pairs
  - env_spark.maxdelay: spark interval is fixed
  - func_breakable.explosion: the explosion gib mode is not read
  - func_door.health: a door with health opens when shot in retail; the port ignores it
  - path_corner.yaw_speed: trains turn at their own rate

Inventory rows first met here (10-15 inventory, 4 DIFF or MISSING): 2 fixed on main, 2 open or unverified.

Fixed: infodecal, static (no targetname) (CHANGELOG); Standard animated lightstyles 1 to 12 (CHANGELOG).

| inventory row | difference (inventory wording, shortened) | state |
|---|---|---|
| env_shake | The SDK shakes only players within radius (default 500) and standing on the ground, with frequency; the port shakes the screen wherever the player is, ignoring radius, fr | open |
| infodecal, targeted (Use stamps a decal) | 125 named decals stamp a resident atlas splat on Use; position and family chosen at cook; shares the 24-slot mark pool below. | open |

### c1a1

Play order 13 of 103, room id 12. Exits: c1a1a@c1a1[], c1a0c@c1a0catoc1a1[].

Original contents: 7 actors, 4 scripted sequences or sentences, 5 multi_managers, 17 triggers, 2 buttons, 19 doors. Classes: infodecal 68, func_wall 31, func_breakable 29, env_beam 24, info_target 23, func_door_rotating 14, env_spark 13, path_corner 13, ambient_generic 12, env_explosion 10, env_sound 9, trigger_once 6 ....

Journey: traversed by `tests/chain-new-game.toml` (block `map c1a1` of `inbound-anomalous.route`), from the arrival in c1a1 to the arrival in c1a1a. The route is closed loop (about 270 steps) and was authored in segment-start mode, then confirmed in the chain.

Path parity against the retail engine (evidence kind: emulator replay and Xash3D runs of the same inputs from a stated origin; the oracle logs the player, every func_train and the fired targets):

| leg | what the player does | retail | PS1 |
|---|---|---|---|
| 1 | start room, west through the pipe room, north corridor (labs1 door) | not run | (185, 1262) |
| 2 | first laser (`1_beam_target`, damage 5000, on from the start, target sweeps east along y 1305 at 64 u/s, cycle about 400 polls): run the north strip y 1584..1599, where the beam passes over the head | running the middle of the corridor kills (health -400) | same, alive at HP 90 |
| 3 | second laser (emitter 1160,1621, trigger x 996..1068): lethal on the north wall in most phases; pass south of the beam while its target is on the north side | retail run along y 1590 dies at x 1197 | same; the safe window is read from `BEAM_RAND_END[10]` |
| 4 | NE corridor, crowbar at (1745,1847), glass window (panes `*67..*70`, health 15) | blocked at y 1881.97, a standing swing hits nothing breakable | blocked at y 1881, same |
| 5 | crouched swing breaks the lower pane in two hits, crouch through, hall | pane `*68` hp 15, 5, -5, solid 0; y 1881 to 2206 | same, y 1961 on the first pass |
| 6 | hall west end, crouched swing at the shaft window (`*71..*74`, target `dethvmm`), crouch into the shaft | not run | (899, 2074), HP 90 |
| 7 | ladder `*105` (z -148 to 805), jump off backwards at the top, walk east out of the shaft, south ring west | not run | upper level z 758, then (176, 2108) |
| 8 | west to the changelevel plane x 24..28 | not run | arrival in c1a1a |

The window and the elevator hall are the gate nobody sees: no walkable path joins the start region to the hall (3D free-space flood and a stepping planner agree; the only link is the glass). The retail engine and the PS1 agree on every point that was run on both.

Differences still open here: the PS1 half-way points of the shaft and ladder legs were not run in retail; the dethvmm cab sequence was not watched on the PS1 frame by frame (it fires, HP stays 90, and the route waits no fixed time for it).

Static entity contract (`hl-census map c1a1`, current main):

- Gap, the port does not honour it: env_spark.maxdelay x10, func_breakable.explosion x29, func_door.health x4, func_friction.@class x2, path_corner.yaw_speed x4
- Partial, honoured with a stated deviation: env_explosion (@class x10, flag:1 x10); env_shake (@class x4, frequency x4, radius x4); env_shooter (@class x1); env_sound (@class x9, roomtype x9); env_spark (@class x13); func_breakable (renderamt x20, rendercolor x19, renderfx x19, rendermode x20); func_button (renderamt x2, rendercolor x2, renderfx x2, rendermode x2); func_door (flag:4 x2, renderamt x4, rendercolor x3, renderfx x3, rendermode x4); func_door_rotating (renderamt x14, rendercolor x14, renderfx x14, rendermode x14); func_train (renderamt x1, rendercolor x1, renderfx x1, rendermode x1, skin x3); gibshooter (@class x3); infodecal (@class x68); monstermaker (@class x2); trigger_hurt (damagetype x1)
- Unreviewed, no one has checked the key against retail: env_beam (boltwidth x24, damage x22, flag:1 x9, flag:128 x2, flag:2 x15, flag:32 x4, flag:4 x9, framerate x24, framestart x24, life x24, lightningend x23, lightningstart x24, noiseamplitude x24, radius x24, renderamt x24, rendercolor x24, renderfx x10, striketime x24, texture x24, texturescroll x24); env_laser (damage x2, endsprite x2, flag:1 x1, flag:32 x2, flag:64 x2, framestart x2, lasertarget x2, noiseamplitude x2, renderamt x2, rendercolor x2, renderfx x2, texture x2, texturescroll x2, width x2); env_shooter (delay x1, m_flvariance x1, m_flvelocity x1, m_igibs x1, renderamt x1, rendercolor x1, renderfx x1, rendermode x1, shootsounds x1); env_sprite (framerate x2, renderamt x2, rendercolor x2, renderfx x2, rendermode x2); func_friction (renderamt x2, rendercolor x2, renderfx x1, rendermode x2); func_illusionary (renderamt x1, rendermode x1, skin x1); func_pendulum (damp x2, renderamt x2, rendercolor x2, renderfx x2, rendermode x2); func_rotating (renderamt x1, rendercolor x3, renderfx x3, rendermode x3, sounds x3, spawnorigin x1, volume x3); func_wall (flag:32 x4, flag:4 x4, renderamt x11, rendercolor x6, renderfx x7, rendermode x10); gibshooter (delay x3, m_flvariance x3, m_flvelocity x3, m_igibs x3); scripted_sentence (flag:4 x1); scripted_sequence (flag:8 x2)
- Cosmetic: 38 class/key pairs
  - env_spark.maxdelay: spark interval is fixed
  - func_breakable.explosion: the explosion gib mode is not read
  - func_door.health: a door with health opens when shot in retail; the port ignores it
  - func_friction.@class: friction trigger volume is absent
  - path_corner.yaw_speed: trains turn at their own rate

Inventory rows first met here (10-15 inventory, 77 DIFF or MISSING): 3 fixed on main, 74 open or unverified.

Fixed: B11 (6e9e343); S22 (6e9e343); B11 (6e9e343).

| inventory row | difference (inventory wording, shortened) | state |
|---|---|---|
| C4 | > 0.3, source within 50 u lights all four, amber above 25 HP else red). Retail grades each edge by the dot and fades 2.0/s with a 0.4 floor; port is binary for a flat 10  | limit-claimed |
| F9 | PS1 LIMIT (recorded: comments at main.rs:25590-25610): screen-space cone on world vertices with a min reach 512; retail is a point light radius. Visual. | limit-claimed |
| J8 | PS1 LIMIT (recorded: audio.rs comments and per-map build log lines). Audit counted 45 maps dropping effects. Players hear lower sample rates; on a few maps NPC small talk | limit-claimed |
| Empty/not-empty reload and fire-empty animations (glock RELOAD vs RELO | Only the "full" variants exist; the model does not show the empty state. PS1 LIMIT (viewmodel RAM, per-weapon pose budget) recorded in `host/hl-build/main.rs:395-401, 410 | limit-claimed |
| Idle animation variety (glock 3, .357 4, shotgun 3, hand grenade 2, RP | No fidgets. PS1 LIMIT recorded in `hl-build/main.rs:395-470` (clip budgets). | limit-claimed |
| Viewmodel pose sampling (decimated key poses per clip) | Animation is resampled to a handful of poses per clip. PS1 LIMIT recorded in code comments (`main.rs:395-401`). | limit-claimed |
| Viewmodel: 3 hit and 3 miss swing variants | Always the same swing. PS1 LIMIT recorded in `hl-build/main.rs` crowbar comment. | limit-claimed |
| Actor carry (monsters, followers) | Monsters in the landmark PVS and volume carry with health, state and position; scientists keep their following, provoked and greeting bits. PS1 LIMIT: 15 mailbox rows, re | limit-claimed |
| F30 Turn rate: per monster `yaw_speed` (headcrab 30, zombie 120, grunt | Every monster turns at about 440 deg/s, 3-15x faster than retail. They pivot to face you almost instantly, so circle-strafing slow turners (zombie, garg, bullsquid) does  | limit-recorded |
| Z09 Opens doors in its path | See F25 | limit-recorded |
| B8 Shots can miss (2 degree cone), hit locations | Port Barney never misses a visible target and ignores the target's hit boxes. | limit-recorded |
| C11 | Retail picks among pl_pain2/4/5/6/7 and plays bone-crunch/fall variants. PS1 LIMIT, UNRECORDED (core bank size). | limit-unrecorded |
| F3 | Retail suit talks after every non-trivial hit and hazard. Port is silent except F5. Atmosphere loss, no gameplay loss. PS1 LIMIT, UNRECORDED. | limit-unrecorded |
| Glass/wood/metal-specific bullet decals (`DamageDecal`), scorch decals | Explosions leave no scorch mark on walls. PS1 LIMIT (resident decal tex) but UNRECORDED. | limit-unrecorded |
| Per-material bullet impact sounds (concrete, metal, dirt, vent, grate, | Every bullet wall hit plays the same ricochet. PS1 LIMIT (core SPU budget, `hl-content/audio.rs:430-441`), per-sound variety UNRECORDED. | limit-unrecorded |
| Hit sounds: flesh `cbar_hitbod1-3` for non-machine, world `cbar_hit1/2 | Volumes close; glass/computer 1/3 vs 0.2 and concrete 0.5 vs 0.6. One sample each instead of 2-3 variants (PS1 LIMIT, `audio.rs:441-460`; variants UNRECORDED). | limit-unrecorded |
| Gibbing: overkill below -30 hp (or `DMG_ALWAYSGIB`) turns the body int | Shotgun and explosive kills leave an intact corpse; no gibs showers, no head gibs, no splat sound. PS1 LIMIT (u8 health, model count) but UNRECORDED. | limit-unrecorded |
| Weapon sound set and variety | PS1 LIMIT (resident core budget). Missing entirely: hornet fire/hit/buzz, rocket flight loop, grenade/satchel bounce, xbow impact, tripmine deploy/charge/activate, snark  | limit-unrecorded |
| Projectile entities rendered as models with trails | PS1 LIMIT, UNRECORDED (no README/CHANGELOG mention). | limit-unrecorded |
| F17 Death: kill at health <= 0, gib when health < -30 or ALWAYSGIB (bl | No overkill gibbing, rockets and grenades do not gib, corpses cannot be shot apart, no headshot/gutshot/directional death clips, always three generic gore chunks for blee | limit-unrecorded |
| S17 Scream / fear sentences (SC_SCREAM, SC_FEAR, SC_PLFEAR) | Flee is silent. PS1 LIMIT (sound budget) likely: UNRECORDED. | limit-unrecorded |
| S27 Post-disaster idle chatter (SC_IDLE / SC_QUESTION / SC_ANSWER, 2.8 | Post-disaster scientists (c1a1 onward) only hello and use-replies; no idle talk or Q/A. PS1 LIMIT (voice budget) likely, UNRECORDED. | limit-unrecorded |
| B12 Barney idle/hello/question/answer lines, BA_KILL, BA_ATTACK ("Free | Silent Barney apart from use replies and pistol. PS1 LIMIT likely, UNRECORDED. | limit-unrecorded |
| B14 Pain / die sounds | One variant of each. PS1 LIMIT (core bank), UNRECORDED. | limit-unrecorded |
| A5 | Retail multiplies acceleration by pev->friction and adds a wall bounce when friction != 1; the port only scales the stopping drop. 14 shipped maps place func_friction (mo | open |
| A21 | Monsters notice the player only by sight, range, damage and squad alert (main.rs:17008). Sneaking past, or shooting out of sight, behaves differently: retail wakes monste | open |
| B10 | Retail: 3,4,5,5.. starting at 12.0 s. Port: 2,3,4,5.. starting at 13.0 s. After 16 s retail has taken 22 HP, port 14. Port is kinder. | open |
| C3 | Port kicks harder and sideways with randomness; retail is a fixed -2 pitch. Cosmetic. | open |
| C7 | Threshold 350 u/s matches; retail rolls the view up to ~13 degrees, port dips pitch. Cosmetic. | open |
| C8 | See A21. | open |
| C16 | About nine campaign trigger_hurts use DMG_FALL (32); retail ignores armour for them, port absorbs. Also DMG_CRUSH (8), DMG_BURN (37), DMG_SHOCK (61) and radiation (49) co | open |
| C12 | Retail waits for a button press then reloads the latest save or autosave. Port auto-restarts after 3 s from the room's entry handoff (level start), see D4. | open |
| F4 | No "power at N percent" voice. | open |
| F5 | Different thresholds (25/10 vs 20/6), no repeat timer. Fires on worsening state, so chip damage near 25 HP warns where retail stays quiet. | open |
| Last-weapon (`lastinv`) quick swap | No swap-back; PC command, pad has no binding. | open |
| Holster animation and holster-time lock | Weapon models change instantly; the 0.5 s lock is applied once through deploy so cadence matches. Python holster lock is 1.0 s in SDK. | open |
| Autoaim (`GetAutoaimVector`: 10 deg glock/357, 5 deg MP5/shotgun, red  | Retail aims toward visible monsters within the cone on Easy/Medium unless `sv_aim 0` (skipped on Hard, `player.cpp:4327`). Xash default `sv_aim 1` (`sv_main.c:25`), this  | open |
| Shot dispersion distribution (triangular, circular, per-weapon cone) | Port is uniform in a square and quantised to whole pixels (.357 cone is 1 px), SDK is centre-weighted. Cone values themselves match: glock 0.01 -> 2 px, alt 0.1 -> 16, .3 | open |
| Movement and air inaccuracy added to bullets | Retail bullets are as accurate running as standing. Port adds about 1-2 px at run speed, up to 11 px airborne (~4 deg). Invented difficulty. | open |
| Crowbar uses hit groups | Crowbar head hits never x3 in the port; in SDK a line-trace head hit does. Hull-trace hits are generic in both. | open |
| Player tracers: only the MP5 (every 2nd bullet) draws one | Glock/.357/shotgun draw streaks retail never shows; MP5 streaks every shot instead of every second. Ring is 8 entries, 2 ticks (`MAX_TRACERS`). | open |
| Muzzle flash: viewmodel star at model attachment 0; no world light | Overlay only; no dynamic light thrown on the room (no dlight system). Hornet/xbow/grenade correctly have none. UNRECORDED. | open |
| Recoil kick per weapon | .357 and the double barrel kick about 40% less than retail, the shotgun single kicks more; MP5 climbs steadily while retail jitters around zero; egon has kick but retail  | open |
| Auto-reload when the magazine empties and no button is held; dry-click | Retail reloads by itself after the last shot once you release the trigger; port waits for one more trigger pull (RPG and crossbow most noticeable: fire, then it stays emp | open |
| Damage 10 (all skills); half damage on follow-up swings | Damage 10/5 equal. Port restores full damage 1.0 s after the swing; SDK 1.25 s after a hit, 1.5 s after a miss. Slightly more full-damage hits when pacing swings. | open |
| Reach and aim model | Port reaches about twice as far and forgives aim; you can hit zombies you would miss. | open |
| Wall bash decal, delayed 0.2 s (`Smack`) | Mark appears 0.2 s early. Cosmetic. | open |
| Hand grenade throw velocity and gravity | A level throw: SDK about 650 u/s with 10 deg pitch-up and 400 u/s^2 gravity (lands about 500-600 units out, longer pitched up); port drops 28 units in about 4 ticks so it | open |
| Grenade bounce and rest | Grenades die in 2-3 bounces; no roll. No bounce sound (`grenade_hit1-3`), no 1 dmg thud. | open |
| Explosion: origin pulled off wall, scorch decal, smoke puff, debris so | Fireball/sparks present; no scorch decal, no smoke, no debris loop, single explosion sample vs a random of three (engine `SoundList_GetRandom`, `cl_tent.c:1613`). | open |
| Blast needs line of sight to the victim | Rockets, grenades and satchels hurt (and wake) enemies and the player **through walls**; retail needs a clear line. Common in corridors and Blast Pit/office fights. | open |
| Distance measured to the victim's hit surface, not centre | Port damage is slightly lower at close range on large actors. | open |
| Knockback of the player on damage (`velocity -= dir x DamageForce`, fo | No blast shove from rockets, grenades, satchels or monster hits (rocket-jump-style movement, being thrown by a satchel, melee hit pushes). | open |
| Corpses can be damaged and gibbed by blast-class damage (`DMG_GIB_CORP | Cannot gib bodies. | open |
| Autoaim (red) crosshair, zoom_autoaim | Follows from autoaim being absent. | open |
| HEV voice "ammunition depleted" (`!HEV_AMO0`) on last round | No spoken warning. | open |
| F11 Enemy selection: `BestVisibleEnemy` ranks by relationship level, t | The retention rule is the same idea (documented for c1a1's Barney-zombie fight). Ranking by hatred is absent, so a scientist at 300 units outranks the player at 400 | open |
| Z05 Bullet resistance: SDK takes 30% only if the damage type is exactl | Player bullets: full damage in port and (per the Xash3D bench, parity.rs:786, zombie head hit = 24 = 8 x 3) in the reference, consistent with the SDK condition. Barney's  | open |
| Z06 Sight: 60 degree half cone, look 2048 | Cone matches; range 700 vs 2048 | open |
| Z08 Flinch limiter (`ZOMBIE_FLINCH_DELAY` 2 s; ignores damage interrup | Zombies never flinch, so no stagger either way. Cosmetic | open |
| S12 Threat detection: fear of hostile monsters | Port range is less than half the retail look distance and ignores the field of view; retail also reacts to combat/danger SOUNDS (S16). Late reactions to far enemies. UNRE | open |
| S13 Flee: run directly away from the threat for the fear time | Retail: scream, point, find real cover up to 1200 away, run to it (run_scared 315 u/s, parsed), crouch and cower up to 10 s. Port: straight away from the enemy at 140 u/s | open |
| S14 Fear memory duration | Port forgets a threat after 4 s of not seeing it; retail 15 s. Scientists come out of hiding too early. | open |
| S15 Following but scared: stay with the player, do not flee (SCHED_TAR | Port: a following scientist abandons you when a headcrab appears. Retail: stays at your side unless the relationship is hate/dislike AND combat state takes over. Breaks e | open |
| S16 React to danger/combat sounds (startle every 3 s, take cover from  | Scientists ignore explosions and gunfire they cannot see. | open |
| S21 Player damage provokes the scientist and ends following | Port also provokes from blast/splash damage the player caused. A scientist caught in your grenade stops following forever. | open |
| S30 Walking through closed doors (CAP_OPEN_DOORS / AUTO_DOORS opens do | Followers wait on the near side of a closed door that you opened and that re-closed; they path only through what the player left open. Not seen in a repro here: UNVERIFIE | open |
| B10 Hit locations on Barney: head x`sk_monster_head` (3), chest/stomac | Head x3 matches; the chest/stomach 0.5 and the helmet rule are missing, so Barney dies faster to body shots. | open |
| B13 Cover when badly hurt (SCHED_TAKE_COVER_FROM_ENEMY on heavy damage | Barney trades shots to the death. | open |
| CC2 Talk-monster pain/danger/noise reactions need an AI sound list (CS | Root cause of S16 (see CC2); Barney/scientist react by sight only. | open |
| CC3 Monster hit locations beyond the humanoid group list | Apache cockpit, osprey engines, Gonarch sack, turret armor, Barney vest: no per-box rules. Documented in the module comment ("Other models keep every box at face value un | open |
| func_friction | Only friction drop is scaled; the SDK also multiplies ground and air acceleration by the modifier, so on ice (modifier 5: 84 placements; 30 and 20 elsewhere) the original | open |
| func_button idle sparks (SPARK_IF_OFF, flag 64) | 13 buttons in 9 maps never spark. | open |
| Dynamic lights (explosions, rockets, muzzle) | No dynamic light is cast on world or models. | open |

### c1a1a

Play order 14 of 103, room id 13. Exits: c1a1f@c1a1atof[], c1a1@c1a1[].

Original contents: 3 actors, 1 scripted sequences or sentences, 2 multi_managers, 10 triggers, 12 buttons, 8 doors. Classes: infodecal 154, func_wall 25, func_breakable 12, func_button 12, ambient_generic 10, env_spark 6, func_door 6, multisource 5, env_beverage 3, monster_scientist_dead 3, trigger_multiple 3, ammo_glockclip 2 ....

Journey: arrival only (the chain checks the arrival point within 30 units; see the findings below). Transition arrivals checked against the retail landmark formula: c1a0c to c1a1 retail (1030.3, 622) against PS1 (1027, 621) with the same offset (retail leaves c1a0c at x 579, the PS1 at about 576; the 3-unit difference is the carried velocity over the first ticks); c1a1 to c1a1a as above.

Findings from a first walk (evidence kind: emulator replay from the arrival, Xash3D run of the same transition):

- Arrival matches retail. Both sides apply landmark(c1a1 in c1a1a) = (-2460, -752, -280) plus the player's offset from c1a1's landmark (84, 2112, 736). Same tick (the first simulation tick), same stance (standing, the origin height is -252 in both, so there is no crouch or eye-height term): retail walking into the plane from (120, 2108) arrives at (x -2511.97, y -756, z -252); the PS1 chain, leaving the plane at y 2146 (offset 34 instead of -4), arrives at (x -2512, up -252, y -717). The 38-unit y difference is exactly the 38-unit difference in where the two runs touched the plane (-752 + (2108-2112) = -756 and -752 + (2146-2112) = -718, the PS1 reads -717). An earlier note here claimed a 500-unit error: that was the journey's own 60-poll walk tape playing on inside c1a1a, not a game defect.
- From the PS1 arrival and from the retail arrival the planner reaches the same long east-west corridor and (-1696, 320), where a gentle ramp (floor -252 to -204, nodes (-1688,320) and (-1408,320)) climbs to the exit plane at x -1356..-1350.
- Three zombies (`monster_zombie`, health 50 on both sides) wait in that corridor, at (-1938,2), (-1794,54) and one near (-2003,-1017), plus a headcrab at (-2402,-64). Measured against the retail engine (Xash3D, same corridor, player standing at (-2325,35), skill 2, one tick = 50 ms):

  | number | retail | PS1 |
  |---|---|---|
  | walk speed | 3.6 u/tick (325 units in 91 ticks) | about 4 u/tick |
  | attack starts at | about 62 units from the player | 64 units (`tick_zombie`), observed about 62 |
  | claw lands | both-claw `attack` (seq 9) 11 ticks after the start, 40 damage; one-claw (seq 8) 15 and 27 ticks, 20 damage each | the same: heavy swing at 11, double at 15 and 27, counted from the first tick the attack state shows on both sides (the PS1 zombie had them at 7 and 13 and 25, 2 to 4 ticks off; measured on the chain's c1a1a arrival walked to the retail measurement's standing point, fixed in `species.rs`) |
  | attack animation / next attack | 28 to 52 ticks, next attack 40 to 60 ticks after the start | heavy bout 40 ticks, double bout 52, the next attack starts when the bout ends (80% double, 20% heavy) |
  | player dead after first contact (standing, no weapon) | 118 ticks | not measured standing |
  | crowbar reach to an actor's centre | 32 line + 16 swept head hull + 16 actor half-extent = 64 | 48 (fixed on branch fix/hl-crowbar-reach-2026-10-29: 48 + 16 hull = 64) |
  | crowbar damage | 10 first swing, 5 on a quick second (CCrowbar::Swing) | same |

  The zombie stops at 62 and claws from there, so a retail player standing still hits it (reach 64) while the PS1 player could not (48): a port bug, fixed in the reach. The claw ticks and bout lengths were 3 to 5 ticks early in `species::tick_zombie` (an earlier note here blamed the headcrab's 21-tick bite schedule, which the zombie does not use: zombies are dispatched to `tick_zombie`); they now follow the reference runtime. What stays open: the 80/20 split between the two swings is the model's, not measured on the oracle. A crowbar-only walk with 35 HP still loses to two zombies on both sides; the retail player arrives with more health.

Static entity contract (`hl-census map c1a1a`, current main):

- Gap, the port does not honour it: env_spark.maxdelay x5, func_breakable.explosion x12, func_door.health x6
- Partial, honoured with a stated deviation: env_render (rendercolor x1, renderfx x1); env_shooter (@class x1); env_spark (@class x6); func_breakable (renderamt x9, rendercolor x9, renderfx x7, rendermode x9); func_button (renderamt x12, rendercolor x12, renderfx x12, rendermode x12); func_door (flag:4 x2, renderamt x6, rendercolor x6, renderfx x6, rendermode x6); func_door_rotating (renderamt x2, rendercolor x2, renderfx x2, rendermode x2); infodecal (@class x154)
- Unreviewed, no one has checked the key against retail: env_beam (boltwidth x1, flag:2 x1, framerate x1, framestart x1, life x1, lightningend x1, lightningstart x1, noiseamplitude x1, radius x1, renderamt x1, rendercolor x1, striketime x1, texture x1, texturescroll x1); env_beverage (skin x2); env_shooter (delay x1, m_flvariance x1, m_flvelocity x1, m_igibs x1, renderamt x1, rendercolor x1, renderfx x1, rendermode x1, shootsounds x1); func_pendulum (damp x1, renderamt x1, rendercolor x1, renderfx x1, rendermode x1); func_rotating (renderamt x2, rendercolor x2, renderfx x2, rendermode x2, sounds x2, spawnorigin x2, volume x2); func_wall (flag:1 x2, renderamt x13, rendercolor x5, renderfx x4, rendermode x13)
- Cosmetic: 33 class/key pairs
  - env_spark.maxdelay: spark interval is fixed
  - func_breakable.explosion: the explosion gib mode is not read
  - func_door.health: a door with health opens when shot in retail; the port ignores it

### c1a1f

Play order 15 of 103, room id 17. Exits: c1a1a@c1a1atof[], c1a1b@c1a1b[].

Original contents: 11 actors, 8 scripted sequences or sentences, 18 multi_managers, 25 triggers, 4 buttons, 12 doors. Classes: infodecal 109, func_breakable 89, func_wall 30, ambient_generic 25, env_spark 19, multi_manager 18, env_explosion 13, info_target 13, trigger_once 13, func_door 8, scripted_sequence 8, env_beam 7 ....

Journey: none.

Static entity contract (`hl-census map c1a1f`, current main):

- Gap, the port does not honour it: env_spark.maxdelay x16, func_breakable.explosion x89, func_door.health x8
- Partial, honoured with a stated deviation: env_explosion (@class x13, flag:1 x10); env_render (renderfx x1); env_shake (@class x4, frequency x4, radius x4); env_shooter (@class x3); env_sound (@class x3, roomtype x3); env_spark (@class x19); func_breakable (renderamt x70, rendercolor x72, renderfx x66, rendermode x68); func_button (renderamt x2, rendercolor x2, renderfx x2, rendermode x2); func_door (flag:4 x2, renderamt x8, rendercolor x8, renderfx x7, rendermode x8); func_door_rotating (renderamt x4, rendercolor x4, renderfx x4, rendermode x4); infodecal (@class x109); speaker (@class x1)
- Unreviewed, no one has checked the key against retail: env_beam (boltwidth x7, damage x7, flag:1 x1, flag:16 x1, flag:2 x6, flag:32 x7, flag:4 x1, framerate x7, framestart x7, life x7, lightningend x7, lightningstart x7, noiseamplitude x7, radius x7, renderamt x7, rendercolor x7, renderfx x7, striketime x7, texture x7, texturescroll x7); env_shooter (delay x3, m_flvariance x3, m_flvelocity x3, m_igibs x3, renderamt x2, rendercolor x2, renderfx x2, rendermode x2, shootsounds x3, skin x2); env_sprite (framerate x1, renderamt x1, rendercolor x1, renderfx x1, rendermode x1); func_illusionary (renderamt x3, rendercolor x2, renderfx x1, rendermode x3, skin x6); func_pendulum (damp x1, renderamt x1, rendercolor x1, renderfx x1, rendermode x1); func_pushable (buoyancy x6, renderamt x1, size x6); func_rotating (@kill x0, health x4, lip x4, renderamt x4, rendercolor x4, renderfx x1, rendermode x4, sounds x4, spawnorigin x1, volume x1, wait x4); func_wall (@kill x0, flag:1 x1, renderamt x11, rendercolor x11, renderfx x11, rendermode x11); func_wall_toggle (renderamt x1, rendercolor x1, renderfx x1, rendermode x1); scripted_sequence (entity x3, flag:64 x5, flag:8 x7, idle x3, moveto x3, play x3, radius x3, range x3, repeat x3, scriptedentity x3); speaker (health x1, preset x1)
- Cosmetic: 36 class/key pairs
  - env_spark.maxdelay: spark interval is fixed
  - func_breakable.explosion: the explosion gib mode is not read
  - func_door.health: a door with health opens when shot in retail; the port ignores it

Inventory rows first met here (10-15 inventory, 1 DIFF or MISSING): 0 fixed on main, 1 open or unverified.

| inventory row | difference (inventory wording, shortened) | state |
|---|---|---|
| Synchronous chain depth cap | A zero-delay chain started by a touch trigger reaches only two follow-on entities; the third is silently dropped. The shipped maps hold 90 such chains: c3a1 trigger_once  | open |

### c1a1b

Play order 16 of 103, room id 14. Exits: c1a1c@c1a1bc[], c1a1f@c1a1b[].

Original contents: 17 actors, 9 scripted sequences or sentences, 17 multi_managers, 29 triggers, 3 buttons, 10 doors. Classes: infodecal 43, ambient_generic 26, func_breakable 18, func_pendulum 17, multi_manager 17, func_wall 16, func_pushable 14, env_beam 13, info_target 13, func_illusionary 11, path_corner 11, env_glow 10 ....

Journey: none for the segment. Two narrow replays of a recorded playtest tape enter c1a1b (`tests/unstick-vent.toml`, `tests/present-cap.toml`).

Static entity contract (`hl-census map c1a1b`, current main):

- Gap, the port does not honour it: env_spark.maxdelay x2, func_breakable.explosion x18, func_door.health x7, func_pushable.explosion x14
- Partial, honoured with a stated deviation: env_explosion (@class x8); env_render (rendercolor x1); env_shake (@class x2, frequency x2, radius x2); env_sound (@class x7, roomtype x7); env_spark (@class x2); func_breakable (renderamt x16, rendercolor x15, renderfx x14, rendermode x16); func_button (renderamt x1, rendercolor x1, renderfx x1, rendermode x1); func_door (flag:4 x2, renderamt x4, rendercolor x4, renderfx x4, rendermode x4); func_door_rotating (renderamt x3, rendercolor x3, renderfx x3, rendermode x3); func_rot_button (flag:64 x1); func_train (renderamt x2, rendercolor x2, renderfx x2, rendermode x2, skin x3); gibshooter (@class x3); infodecal (@class x43); monstermaker (@class x1)
- Unreviewed, no one has checked the key against retail: env_beam (boltwidth x13, damage x13, flag:2 x13, flag:32 x1, framerate x13, framestart x13, life x13, lightningend x12, lightningstart x13, noiseamplitude x13, radius x13, renderamt x13, rendercolor x13, renderfx x1, striketime x13, texture x13, texturescroll x13); env_bubbles (master x2); env_glow (renderamt x10, rendercolor x10, renderfx x10, rendermode x10); env_sprite (framerate x2, renderamt x2, rendercolor x2, renderfx x2, rendermode x2); func_illusionary (flag:1 x1, renderamt x5, rendercolor x3, renderfx x2, rendermode x5, skin x11); func_pendulum (damp x17, flag:32 x1, renderamt x17, rendercolor x17, renderfx x17, rendermode x17); func_pushable (buoyancy x14, delay x14, globalname x2, renderamt x14, rendercolor x14, renderfx x14, rendermode x14, size x14); func_rotating (flag:32 x1, renderamt x3, rendercolor x3, renderfx x2, rendermode x3, sounds x3, spawnorigin x1, volume x3); func_wall (@kill x0, flag:1 x1, renderamt x6, rendercolor x2, renderfx x2, rendermode x6); gibshooter (delay x3, m_flvariance x3, m_flvelocity x3, m_igibs x3); scripted_sequence (flag:1 x1, flag:16 x1, flag:2 x1, flag:64 x4, flag:8 x1)
- Cosmetic: 35 class/key pairs
  - env_spark.maxdelay: spark interval is fixed
  - func_breakable.explosion: the explosion gib mode is not read
  - func_door.health: a door with health opens when shot in retail; the port ignores it

Inventory rows first met here (10-15 inventory, 14 DIFF or MISSING): 0 fixed on main, 14 open or unverified.

| inventory row | difference (inventory wording, shortened) | state |
|---|---|---|
| HE04 Squad bonus: damage x (1 + 1.1 (n-1)) for a squad of n (up to 4,  | Port: 2 hounds 2x, 3 hounds 3x, never above 3x. Retail: 2.1x, 3.2x, 4.3x. Weaker for big packs (c2a4d, c4a1 makers x5) | limit-recorded |
| HE08 Pack behaviour: attack slots (3), AGITATED threat display when no | No synchronised group blast or group threat circle; hounds never sleep | limit-recorded |
| VO07 Cover and flee when health < 20 or bravery < 0 | Vortigaunts fight to the death at any health | limit-recorded |
| Weapon weight / next-best-weapon on empty (exhaustible weapons are rem | Hand grenade, snark, tripmine, satchel stay in the inventory at 0 ammo and the player is never auto-switched off them. The empty glock/MP5 does not auto-switch in either  | open |
| HE05 Line of sight: full damage if visible, half if the player is behi | Port hurts through walls and floors at full falloff damage; retail halves it. A hound in the next room can hurt | open |
| HE06 The blast hurts everything damageable except houndeyes (scientist | Hounds never kill NPCs or break crates/glass with the sonic wave | open |
| HE07 Hit while charging: the range-attack schedule is interrupted by l | Cancel-on-hit agrees; the 60% cover branch and 40% roll are replaced by a deterministic retreat. Close to retail feel | open |
| VO03 Zap range 64-784 and facing 0.5; port 150-650 | Port stops zapping inside 150 and beyond 650; it claws inside 70 and walks between 70 and 150 | open |
| VO04 Zap cadence: next attack random 0.5-4.0 s after the shoot event;  | Metronomic zapping instead of irregular gaps; Hard is not faster | open |
| VO05 Claws: `sk_islave_dmg_claw` 8/10/10 and `sk_islave_dmg_clawrake`  | Melee is weaker if the rake is part of the retail sequence | open |
| VO08 Charge up, sound set (SLV_ALERT / IDLE sentences) | Charge/beam cues exist; sentence barks do not | open |
| Entity GLOBAL_DEAD tracking (globalname on removal) | 204 globalname func_breakable placements in 20 maps (airlock glass c1a1b/c1a1f, crates c1a2, boards c1a4, library c2a2b1, glass c2a4e/g) respawn intact in the next map in | open |
| Door crush (Blocked) | Reverse-on-block and the wait -1 squash agree; damage lands 20 times a second instead of per frame, and linked halves reverse on their own pass. | open |
| func_wall_toggle | A USE_ON to a visible wall or USE_OFF to a hidden one flips it in the port, nothing in the SDK. Shipped feeders are breakables (TOGGLE), one relay (ON) and one auto; smal | open |

### c1a1c

Play order 17 of 103, room id 15. Exits: c1a1b@c1a1bc[], c1a1d@c1a1c/d[], c1a1d@c1a1dtoc1a1c[], c1a2@a1a2[changetoc1a2].

Original contents: 11 actors, 0 scripted sequences or sentences, 9 multi_managers, 34 triggers, 3 buttons, 11 doors. Classes: info_target 27, env_beam 26, func_wall 22, ambient_generic 21, func_breakable 21, func_pushable 21, env_sound 17, func_rotating 16, env_glow 12, monstermaker 10, trigger_push 10, multi_manager 9 ....

Journey: none.

Static entity contract (`hl-census map c1a1c`, current main):

- Gap, the port does not honour it: func_breakable.explosion x21, func_door.health x3, func_friction.@class x1, func_pushable.explosion x21, path_corner.yaw_speed x2
- Partial, honoured with a stated deviation: env_shake (@class x4, flag:1 x3, frequency x4, radius x4); env_sound (@class x17, roomtype x17); func_breakable (renderamt x20, rendercolor x20, renderfx x20, rendermode x20); func_button (renderamt x2, rendercolor x2, renderfx x2, rendermode x2); func_door (renderamt x3, rendercolor x3, renderfx x3, rendermode x3); func_door_rotating (renderamt x7, rendercolor x7, renderfx x7, rendermode x7); func_rot_button (flag:64 x1); func_train (renderamt x2, rendercolor x2, renderfx x2, rendermode x2, skin x3); infodecal (@class x8); monstermaker (@class x10); trigger_hurt (damagetype x2); trigger_teleport (flag:1 x1)
- Unreviewed, no one has checked the key against retail: env_beam (boltwidth x26, damage x26, flag:2 x26, flag:32 x2, framerate x26, framestart x26, life x26, lightningend x24, lightningstart x26, noiseamplitude x26, radius x26, renderamt x26, rendercolor x26, renderfx x2, striketime x26, texture x26, texturescroll x26); env_glow (renderamt x12, rendercolor x12, renderfx x12, rendermode x12); env_sprite (framerate x4, renderamt x4, rendercolor x4, renderfx x4, rendermode x4); func_friction (renderamt x1, rendercolor x1, rendermode x1); func_illusionary (renderamt x6, rendercolor x3, renderfx x2, rendermode x6, skin x7); func_pendulum (damp x6, renderamt x6, rendercolor x6, renderfx x6, rendermode x6); func_pushable (buoyancy x21, delay x21, gibmodel x4, globalname x2, renderamt x21, rendercolor x21, renderfx x21, rendermode x21, size x21); func_rotating (flag:256 x2, flag:32 x5, friction x10, renderamt x16, rendercolor x16, renderfx x15, rendermode x16, sounds x16, spawnorigin x3, volume x6); func_wall (@kill x0, flag:1 x1, renderamt x11, rendercolor x5, renderfx x4, rendermode x11); func_water (dmg x1, flag:8 x1, health x1, lip x1, locked_sentence x1, locked_sound x1, movesnd x1, renderamt x1, rendercolor x1, renderfx x1, rendermode x1, skin x1, speed x1, stopsnd x1, unlocked_sentence x1, unlocked_sound x1, wait x1)
- Cosmetic: 36 class/key pairs
  - func_breakable.explosion: the explosion gib mode is not read
  - func_door.health: a door with health opens when shot in retail; the port ignores it
  - func_friction.@class: friction trigger volume is absent
  - path_corner.yaw_speed: trains turn at their own rate

Inventory rows first met here (10-15 inventory, 9 DIFF or MISSING): 0 fixed on main, 9 open or unverified.

| inventory row | difference (inventory wording, shortened) | state |
|---|---|---|
| BS06 Swerve when hurt while chasing, sprint inside 256, ignore headcra | Squids charge straight; never eat; headcrabs and squids ignore each other | limit-recorded |
| F24 Spawnflags: WAIT_TILL_SEEN (1), GAG (2), MONSTERCLIP (4), PRISONER | PRISONER and MONSTERCLIP implemented. WAIT_TILL_SEEN (87 hostile placements) only approximated by PVS gating, so an unseen monster can still act at range; WAIT_UNTIL_PROV | open |
| BS03 Spit cadence: 0.5 s between spits while stationary, 5 s after a s | Port stops spitting after two; retail keeps spitting every 0.5 s while it stands and every 5 s while chasing. Less ranged pressure than retail | open |
| BS04 Melee: tail whip (reach 85, damage whip 25/35/35) only when the e | `sk_bullsquid_dmg_whip` is not read (skill.rs:119, listed unwired). Low-health finisher missing; bite damage correct | open |
| BS05 Bite knockback (-100 forward, +100 up) | Roughly 3x stronger shove than the SDK states (port value comes from the retail trace) | open |
| BN02 Hauling speed 80 u/s | Port hauls slower; the source says 80. The comment cites the retail trace, so the SDK build may differ from the reference | open |
| BN03 Prey: any enemy by relationship (player, scientists, Barney, mari | Marines are never eaten by barnacles (c1a3 and Surface Tension rooms). Column is a radius instead of the box | open |
| BN05 NPC prey: dangles 10 s while being chewed (bite sound ~1 per 5 s) | NPCs die as soon as they reach the mouth; no gib shower | open |
| BN06 Health 25; any DMG_CLUB hit kills it outright | One crowbar swing kills a barnacle in retail; port needs several (crowbar 10 per hit). Makes the crowbar a weaker barnacle answer | open |

### c1a1d

Play order 18 of 103, room id 16. Exits: c1a1c@c1a1c/d[], c1a1c@c1a1dtoc1a1c[].

Original contents: 0 actors, 0 scripted sequences or sentences, 0 multi_managers, 4 triggers, 0 buttons, 2 doors. Classes: func_wall 21, func_illusionary 9, func_ladder 3, infodecal 3, ambient_generic 2, env_sound 2, func_door_rotating 2, info_landmark 2, trigger_autosave 2, trigger_changelevel 2, info_player_start 1.

Journey: none.

Static entity contract (`hl-census map c1a1d`, current main):

- Partial, honoured with a stated deviation: env_sound (@class x2, roomtype x2); func_door_rotating (renderamt x2, rendercolor x2, renderfx x2, rendermode x2); infodecal (@class x3)
- Unreviewed, no one has checked the key against retail: func_illusionary (renderamt x2, rendercolor x1, renderfx x1, rendermode x3, skin x9); func_wall (renderamt x13, rendercolor x11, renderfx x11, rendermode x13)
- Cosmetic: 19 class/key pairs
