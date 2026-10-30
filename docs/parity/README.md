# Parity with the original game, segment by segment

This directory is the full-game parity ledger for the port. It was started on main b3a64f2 (2026-10-23) and is
meant to be extended one segment at a time.

## The standard

Every original segment (one retail map: 96 campaign maps and 7 Hazard Course maps) must

1. exist in the port,
2. be playable from the previous segment's end state with the normal controller, and
3. have a deterministic `psoxide-gate` journey that is part of one full-game journey chained from the title, and
4. have a parity report: what differs from the original, with a repro or a `LIMITS-*.md` entry that proves a PS1
   hardware limit. A difference with neither is a bug.

The full-game journey is complete only when it is green on CPU1, HW1 and HW3 with labelled diffs and 1:1 crop review.

## What is here

| file | content |
|---|---|
| [SEGMENTS.md](SEGMENTS.md) | the 103 segments in play order with exits, content counts and the journey that covers each |
| [black-mesa-inbound.md](black-mesa-inbound.md) | c0a0 to c0a0e (tram ride and walk to the test chamber) |
| [anomalous-materials.md](anomalous-materials.md) | c1a0, c1a0d, c1a0a, c1a0b, c1a0e |
| [unforeseen-consequences.md](unforeseen-consequences.md) | c1a0c to c1a1d (c1a0c, c1a1 and c1a1a traversed) |
| [hazard-course.md](hazard-course.md) | t0a0 to t0a0d |

Every segment section has the same parts: position and exits, what the retail map contains, the journey evidence,
the static entity contract for that map (`hl-census map <name>`), and the behaviours the 10-15 inventory first meets
in that map with their state on main. The state vocabulary is in each chapter file.

## Status on main c84818f

| coverage | segments |
|---|---:|
| traversed by a controller-only journey from the previous segment (`tests/chain-new-game.toml`) | 14 |
| partly traversed: t0a0 from the start lift to the catwalk (`tests/chain-hazard.toml`) | 1 |
| entry and a short walk (c4a1a) | 1 |
| narrow replay or probe disc (t0a0b, c1a1b) | 2 |
| no journey | 85 |

The chain runs the New Game from the title through the tram ride and Anomalous Materials into c1a0c, through its
lift, the console scientist, the retinal door and the control room, through c1a1 (pipe room, two lasers, the glass window, the shaft ladder) and into the arrival in c1a1a
(`psoxide-gate run tests/chain-new-game.toml --no-hw`, about 11 minutes on a loaded machine). Controller input only;
the memory card is checked unchanged at every checkpoint. It plays c1a1 as well and stops at the arrival in c1a1a: no journey plays c1a1a yet.

### Evidence gaps against the standard

- The chain's checkpoints are liveness checks (`golden = false`: not flat, card unchanged, suit and health words,
  cart position). No frame of the chain is compared with a retail reference image.
- The full renderer matrix is not green. The same chain run without `--no-hw` (CPU1, HW1, HW3, 91 checks) passes the 54
  CPU-side checks and fails all 37 hardware comparisons: at 19 checkpoints HW1 differs from CPU1 in 27 to 6,315 pixels
  of 76,800 and HW3 in 594 to 159,957, none of it classified as 2D or 3D, so the strict comparison has nothing to
  relax. The failure is on every checkpoint including the title, so it is the fleet-wide strict-hardware gate issue
  (see the campaign registry), not a chapter-specific finding. It still blocks completion.
- There is no single `LIMITS.md`. The limits live in `LIMITS-L3.md` (world logic and movement), `LIMITS-L5.md` (decals
  and light styles) and `LIMITS-monsters.md`. The 10-15 inventory marks 49 further differences as PS1 limits that
  appear in none of them (31 with a code comment, 18 with no record at all); each counts as a bug until written down
  with evidence.
- The 10-15 inventory (`behaviour inventory, 666 rows`) predates about 200 commits (it was written on 2026-10-15). Rows its authors fixed are marked
  `fixed` in the chapter files with the commit; the rest are marked `open` and have not been re-verified on this build.
  A row becomes a bug report only with a repro; apart from the two c1a0c rows in the table below, none of the `open` rows has one yet.

## Parity findings that matter most for the first chapters

Rows 1 and 2 were investigated in this work (repro column). The others come from the 10-15 inventory or the census and are still unverified on b3a64f2.

| # | difference | first seen | evidence | state |
|---|---|---|---|---|
| 1 | c1a0c, west corridor: the fallen pipes at x 572 stop a plain jump (retail clears them at apex 44.7; open-air PS1 jumps match retail, the difference is the contact with the pipe's hull edge); a crouch jump passes | c1a0c | route lines `1 f-127 jump` in `inbound-anomalous.route`, retail trace (z -324 to -279) | open, narrowed (see unforeseen-consequences.md) |
| 2 | c1a0c, elevator `ele_2`: not a port bug. The cab does start at its authored position (phase 4096); the journey's own extra `use` called it away before the player boarded | c1a0c | `lift-called`, `lift-top` checkpoints (player rides to z -100 in 120 ticks, as retail) | closed, route error |
| 3 | Heads never turn toward the player or a friend (bone controllers); scientists and Barney look stiff | c0a0 | inventory D S24, E | open |
| 4 | Sequence cross-fade is lost on main: every idle, walk and attack change snaps (it exists only on `consistency/with-crossfade`, d7d8507, and costs model RAM) | c0a0 | inventory D CC1 | open, a RAM decision |
| 5 | The sitting scientist stands up and runs when alarmed (retail stays seated) and does not speak | c0a0 | inventory D S33 | open |
| 6 | Scientists die silently and have no pain sounds | c1a0 | inventory D S18 | open |
| 7 | A door with health opens when shot in retail; the port ignores it | 79 maps | census gap `func_door.health` (568 uses) | open, static |
| 8 | The explosion gib mode of `func_breakable` is not read | 89 maps | census gap `func_breakable.explosion` (1,937 uses) | open, static |
| 9 | `env_spark` ignores its interval (`maxdelay`) and the toggle and start-on flags | c0a0a | census gap (165 uses), inventory E | open, static |
| 10 | `trigger_hurt` never hurts monsters; the no-clients flag is ignored | c0a0e | inventory E, census gap flags 8 and 32 | open |
| 11 | Door default `wait` and `lip` for doors that carry no key (95 doors) differ from retail | c1a0a | inventory E | open |
| 12 | Scripted sequences start per actor on arrival, retail waits for every actor of a same-named group | c0a0 | inventory E | open |
| 13 | `scripted_sentence` picks its speaker differently from retail (retail chooses one that can speak and retries) and ignores the `listener` key (85 uses) | c0a0e | census gap, inventory E | open |
| 14 | 415 class and key pairs have never been reviewed against retail (mostly render keys of env_beam, env_glow, env_sprite, func_wall) | all | `hl-census report` | open |

Fixed on main since the inventory (commit or CHANGELOG entry): scientist heal, Barney fights back and his pistol
damage, follower speeds, turret tracking, blast line of sight, barnacle club kill, leech swimming, map decals (1,320 of
1,542), switchable and flickering lights, autosave points and walking back to a visited map, death flow, trigger hints,
changelevel ordering, jump press rule, explosion magnitudes. Monster senses, hearing, relationships and the alien
attacks are recorded in `LIMITS-monsters.md`.

## Tools

- `tests/chain/hlroute.rs`: compiles a text route into poll-bound tapes and a `psoxide-gate` journey. Added in this
  work: `prologue=hazard` (title, Hazard Course row, map list; `hazard_row=N` picks the map), `prologue=segment`
  (the Debug page's Segment select, authoring only), `@face YAW right|left` (turn until the yaw word is in a 60-unit
  band) and `@probe` (print position and yaw at a checkpoint).
- `tests/chain/segments.rs`: the segment table (see SEGMENTS.md).
- `hl-census map <name[,name...]|all>`: the per-map gap table from `host/hl-content/entity-contract.txt`.

### Authoring a segment route

1. Read the map: `LOGIC_PLAYER_POS` is (x, up, y) in GoldSrc units, `LOGIC_PLAYER_YAW` is 0..4095 with 0 facing +y and
   1024 facing +x. One tick of `t127` turns about 127 units, `t60` about 61, `t30` about 30.
2. Prefer closed loops to timed runs: `@hold f127 until sym=LOGIC_PLAYER_POS size=4 signed=true offset=8 op=ge value=...`
   waits for a door as well as walking to it. A run that depends on how long a door took is a flaky journey.
3. Pedestals with buttons stand in the middle of corridors (Hazard Course): strafe round them. `use` needs the button
   inside the cone in front of the eye, so look down (`l-127`) first.
4. Run one segment fast through the Debug page's Segment select (`prologue=segment segment_row=N`, N is the play-order
   row), then chain it: a segment is not done until the route works from the previous segment's end state.
5. Regenerate the journey for every build (`--map .hlpsx/hl-psx.map`), and keep the symbols the journey reads in
   `[addr]`.
