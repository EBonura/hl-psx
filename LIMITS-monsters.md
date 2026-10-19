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
lists what the port plays at each point and what it does not. The gaps have three causes.

The RAM floor. The executable is linked at the highest PGO hot-callsite rung that leaves
2,048 B free; inlining moves `.text` by kilobytes on tiny source or profile changes, and at
rung 125 main has 2,276 B free. What this batch adds (sparks, and the pain and death cries
below) leaves 2,084 B free at rung 125. Everything else the catalogue found missing costs
RAM that rung 125 does not have: the alert, idle and footstep logic in the actor
loop (`monster_voice::think`) costs 784 B at rung 125, the headbite, bite, claw miss, punch,
Gargantua slash, barnacle, leech, roach and turret hooks together about 800 B more, and the
full set (1.6 KB in all) leaves 500 B free at rung 125 (and overflows the image by 572 B with a freshly
collected profile), which drops the link to rung 60, 2 to 6 percent slower on the profiled
maps (hl-features measured that cost with the Gonarch batch and the walker gravity batch).
The full set is built, probed and was gated (at rung 60) before the rebase onto the walker gravity batch on `feat/hl-sound-cues-full-2026-10-19`; it lands
when the executable has about 2 KB more room. Rows left out for this reason are `limit`
rows of the catalogue: headcrab headbite, idle and alert cries (headcrab, zombie,
houndeye, bullsquid, alien grunt, controller, ichthyosaur, turret, leech), zombie claw
miss, alien grunt punch, ichthyosaur attack and bite, barnacle death, leech bite, roach
death, `monster_turret` fire (`tu_fire1`), Gargantua slash, the marine's M203 launch and
the npc footsteps of scientists, Barney and zombies.

SPU RAM. A map's bank has to fit what is left of the 512 KiB after the resident core, and
the rate allocator spends every byte it has on the map's own sounds (0 to 8 B left in the
maps that hold an alien grunt, controller or turret; a pain cry needs 880 to 1,232 B at its
lowest rate). The cries added here compete for rate with a ladder capped at 6,000 Hz and
never take rate from an authored dialogue line; 206 of the 2,564 other bank entries (63
loops, 132 one-shot effects and 11 chatter lines) give up one or more ladder steps for them,
none below the floor the previous cooker set, and 26 gain a step where a cry was left out.
Where a cry still does not fit the runtime stays silent: it never plays another sample.
Maps that place the class and carry the sample: alien grunt pain and death 11 of 14,
controller pain and death 6 of 8, scientist pain 28 of 32, turret death 15 of 16, sparks
20 of 20, ichthyosaur pain and death 5 of 5, barnacle bite 21 of 21.
`hl-catalogue limits` regenerates the per-slot table from a cook.

Spoken lines. The marine, vortigaunt, Barney and scientist voices of the original are
sentence groups of whole recordings (`HG_*`, `SLV_*`, `BA_*`, `SC_*`): 116, 11, 160 and
304 distinct files, 1.3 MB, 107 KB, 4.7 MB and 12.1 MB of source audio. The marines' and
vortigaunts' callouts (alert, cover, grenade, idle) and the autonomous Barney and scientist
lines beyond the authored ones are not played: even the smallest group, the marines' 1.3 MB
of source, is far more than any bank has left.

Sequences the retail models carry but the port does not play: the alien grunt's footsteps
(`player/pl_ladder*`), the vortigaunt's gibberish idle events, tentacle, snark, tripmine
and Gargantua breath sounds, Big Momma's claw, alert and footsteps (`gon_step1` is cooked
and never played), the apache and osprey crash `mortarhit`, the turrets' deploy, ping and
spin sounds. They stay open rows of the catalogue.
