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

C-F25, C-F27, C-F28, C-Z09. The original builds a node route for each far goal
with hull-class link masks and opens doors on the route. The port steers toward
the goal and uses the cooked flee goals; there is one hull class and monsters do
not use doors.

## Movement

C-HC03. A headcrab leaps by throwing itself along a ballistic arc. The port
glides it to the target over the eight ticks of the leap animation: an actor has
no velocity or gravity state, and adding one for the leap costs a field in every
prop.
