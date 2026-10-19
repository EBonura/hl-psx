# Monster behaviour that differs from the original game on purpose

Each entry says what the retail game does, what this port does instead, and the
limit that forces it. Rows are the ids of the source-versus-original inventory.
The numbers come from the shipping link maps and the cook reports, so a rebuild
reproduces them.

## The budget that forces most of these

The executable (code, data and static buffers) has to fit the 2 MiB of main RAM
below the stack. With the authored HECU equipment merged, the shipping link
leaves 18,144 B free before any monster work, and 8,280 B free after the
senses, hearing, relationship table, the alien attacks and the marine shotgun
loadout. Every function the original has and the port lacks is a few hundred
bytes to a few kilobytes: `tick_grunt` alone is 6.2 KB, the largest function of
the guest after the renderer. A feature that needs a new engine rather than a
new branch does not fit next to the others.

The stack is reserved at the depth measured on the profiling tapes (17,760 B)
plus a 2 KiB margin, 19,968 B in all, which `host/hl-build` enforces on every
link; the SDK default would take another 12.8 KB.

## Simulation rate and activity

- C-F01. The original thinks a monster at 10 Hz. The port simulates at a fixed
  20 Hz and thinks a monster every few ticks through `ai_reacquire`; walkers step
  every tick. A 10 Hz think would only lose reaction time.
- Monsters outside the player's potentially visible set do not tick (the actor
  loop gates on `in_player_pvs || behavior_awake`). The original keeps every
  monster in its think list. Marines and aliens that fight out of the player's
  sight therefore wait until the player can see them. This is the CPU budget of a
  20 fps scene, not a missing rule.

## The schedule engine

C-F13, C-F14, C-F15, C-F21, C-F22, C-F23, C-HE08, C-BS06, C-HG11, C-AG05,
C-VO07, C-CO05. The original runs every monster through a shared state word, a
condition bit set, about ninety tasks and some forty-five schedules. The port
has one hand-written state machine per species (`species.rs`, `main.rs`), which
is why the pieces that need the engine are absent: damage conditions that choose
between flinch and cover, squad recruitment and engage slots, cover and flank
search, call for help, houndeye attack slots, the bullsquid swerve and the
alien grunt hornet slots. A generic engine in place of the state machines does
not fit in the memory left after the senses (see the budget above). The
pieces that are rules on one species are implemented as rules: the houndeye
pack bonus (C-HE04), the marine and alien relationships (C-F10), view cones
(C-F04), hearing (C-F07), the last known position (C-F12) and the yaw speeds
(C-F30).

## Navigation

C-F27, C-F28. The original builds a node route for each far goal with
hull-class link masks. The port steers toward the goal and uses the cooked flee
goals; there is one hull class.

C-F25, C-Z09. A walker whose step is blocked by a closed plain door (a func_door
with no targetname that is not USE-only, the doors the player opens by walking
into them) opens it (`monster_opens_door`). Button- and trigger-operated doors
stay shut for monsters, as they do for the player's own touch.

## Movement

C-HC03. A headcrab leaps by throwing itself along a ballistic arc. The port
glides it to the target over the eight ticks of the leap animation: an actor has
no velocity or gravity state, and adding one for the leap costs a field in every
prop.

## Bosses and the controller

BM02..BM06 (Gonarch). She fights while she waits at a node (for its delay or for
the damage a health node asks) and hunts once her path is over; between nodes the
walker owns her. A bullet only counts on her sack (hit group 1); acid does
nothing to her, her babies or headcrabs. Her babies are the six dormant stock
copies cooked beside her plus any corpse that has lain five seconds (the original
keeps up to twenty alive). The slash hurts what she is fighting, not the first
monster in the box.

C-CO03 (controller head ball). The original's ball homes on the enemy at up to
400 u/s; this one flies straight at where he stood when it left (a homing
projectile costs a ticker of its own in the executable). The damage
(`sk_controller_dmgzap`), the 37 tick wind-up and the 64..256 unit window are
the original's.

## Sound

The catalogue of every sound each placed monster and effect class makes against the
original (`host/hl-catalogue`, port side in `host/hl-content/sound-contract.txt`)
lists what the port plays at each point and what it cannot. Three limits come from
the sound banks and one from the speech system.

SPU RAM. A map's bank has to fit what is left of the 512 KiB after the resident core,
and the rate allocator spends every byte it has on the map's own sounds: in the maps
that hold an alien grunt, controller or turret the bank is full to the byte (0 to 8 B
left; a pain cry needs 880 to 1,232 B at its lowest rate). The cries added after the
original set therefore come in two kinds. The ones that replace a wrong sample or give
combat feedback (spark, bites, claw miss and strike, alien grunt, controller,
ichthyosaur, barnacle, leech, roach and turret death, scientist pain, Gargantua slash)
compete for rate with a ladder capped at 6,000 Hz, and never take rate from an authored
dialogue line; 351 of the 2,564 other bank entries (129 loops, 210 one-shot effects and 12
chatter lines; no dialogue) give up one or more ladder steps for them, none below the
floor the previous cooker set, and 16 gain a step where a cry was left out. Where the
cry still does not fit the runtime stays silent: it never plays another sample. Not
carried, as maps that place the class / maps carried: headcrab bite 7 / 49, zombie claw
miss 1 / 19, alien grunt pain, death and punch 4 / 14 each, controller pain and death
2 / 8, roach death 5 / 25, turret death 1 / 16, scientist pain 6 / 32.
The idle and alert cries and the npc footsteps are spare-RAM only and fit in few maps:
headcrab idle 18 and alert 17 of 49, zombie alert and idle 5 of 19, houndeye idle 1 of
11, bullsquid idle 5 of 20, alien grunt idle 2 of 14, controller idle 3 of 8,
ichthyosaur idle 1 of 5, leech alert 1 of 7, turret alert 5 of 16, footsteps 10 to 12
of 56. `hl-catalogue limits` regenerates the full per-slot table from a cook.

Spoken lines. The marine, vortigaunt, Barney and scientist voices of the original are
sentence groups of whole recordings (`HG_*`, `SLV_*`, `BA_*`, `SC_*`): 116, 11, 160 and
304 distinct files, 1.3 MB, 107 KB, 4.7 MB and 12.1 MB of source audio. The marines'
and vortigaunts' callouts (alert, cover, grenade, idle) and the autonomous Barney and
scientist lines beyond the authored ones are not played: even the smallest group, the
marines' 1.3 MB of source, is far more than any bank has left.

Steps. The original plays `common/npc_step*.wav` from the walk and run sequences of
scientists, Barney and zombies. The port animates the walk sequence at every speed, so it
plays the walk's two steps every 8 (scientist), 10 (Barney) or 9 (zombie) ticks while the
actor moves and never the run's.

Sequences the retail models carry but the port does not play: the alien grunt's footsteps
(`player/pl_ladder*`), the vortigaunt's gibberish idle events, tentacle, snark, tripmine
and Gargantua breath sounds, Big Momma's claw, alert and footsteps (`gon_step1` is cooked
and never played), the apache and osprey crash `mortarhit`, the turrets' deploy, ping and
spin sounds. They stay open rows of the catalogue.
