# PS1 limits: the register

The standard for this port is that it matches the original as closely as PlayStation hardware allows. A
difference from the original is therefore one of three things: fixed, recorded here with evidence that the
hardware forces it, or a bug. A difference that claims to be a PS1 limit without evidence is a bug until the
evidence exists. This file is the single place that says which is which.

It does three jobs:

1. Index the limit files that already exist.
2. State the hardware envelope the evidence is measured against.
3. Register every claimed limit that none of those files covers, with the evidence the tree holds today or the
   word **unproven**.

Parity differences found by playing (as opposed to claimed limits) are in `docs/parity/`. A difference there with
no entry here is a bug.

## 1. The limit files

| file | covers |
|---|---|
| [LIMITS-L3.md](LIMITS-L3.md) | world logic and movement: delayed-output queue, global-state table, carried entities, ledge braking, ladders, door crush, split doors, track speed |
| [LIMITS-L5.md](LIMITS-L5.md) | map-painted decals and light styles, with the cost of the resident decal pack |
| [LIMITS-monsters.md](LIMITS-monsters.md) | monster behaviour: the simulation budget, think cadence, schedule engine, navigation, bosses, monster sound |

Those three files were written by the audit lanes of 2026-10-15 and each entry in them names its inventory row and its
evidence. They are taken as written here; the register below only points at them.

## 2. The hardware envelope

These are properties of the console, not measurements of this port. A claimed limit is **bound** when one of them
is the resource it runs out of and the tree names the cap. It is **proven** only when, in addition, the next unit
of the thing demonstrably does not fit (or the cap is shown never to be reached by the retail content).

| resource | figure |
|---|---|
| main RAM | 2 MiB |
| VRAM | 1 MiB |
| SPU RAM | 512 KiB |
| SPU voices | 24 |
| memory card block | 8 KiB |
| CPU | R3000A, no FPU, no 64-bit arithmetic |
| GPU blending | four fixed semi-transparency functions per primitive, vertex colour modulation, no per-pixel lighting |
| CD | data reads and CD-DA playback exclude each other (silicon test v1.28 of this campaign) |

## 3. Verdicts

- **recorded**: written up in one of the three files above.
- **proven**: bound, and the evidence shows the cap is the right one.
- **bound**: a hardware resource is named and a cap constant or build message exists in the tree, but nobody has
  shown the cap could not be higher. Counts as a bug until that is shown, but a cheap one to close.
- **unproven**: the claim has no evidence in the tree.
- **not a limit**: the hardware can do it; the difference is a choice or a gap.

Row ids are those of the 10-15 behaviour inventory (appendix letter and id, or the row title).

## 4. The register

### Player, input and UI (appendix A)

| row | difference | evidence in the tree | verdict |
|---|---|---|---|
| A8 | a jump press is queued up to 2 ticks before landing (retail has no buffer) | `JUMP_BUFFER_TICKS` was an input-feel choice per its own comment; no hardware reason | not a limit. Retail behaviour is pogo-free (fixed in e7594b1); the buffer stays a deliberate difference that needs a decision |
| A17 | ladder strafe is a third of retail speed, mount uses a 2-unit widening, ladders assumed axis-aligned | `ladder_logic.rs` doc comments (shared engine crate); LIMITS-L3 defers to it | unproven. No resource is named |
| A19 | every surface sounds like concrete | the resident core bank holds `pl_step1/2` only (`host/hl-content/src/audio.rs`, the `step1`/`step2` rows) | bound (SPU RAM). Byte cost of a full step set against the per-map headroom is not computed |
| B14 | underwater: subtractive grade instead of the refraction warp; an oxygen bar retail does not have | comment at `draw_screen_fx` | warp: unproven (no cost measured for a mesh warp). Oxygen bar: not a limit, an addition |
| C4 | damage arrows are binary flat triangles for 10 ticks | comment above `draw_damage_compass` | unproven (no VRAM or sprite cost given) |
| C11 | one pain grunt | `audio.rs` core bank | bound (SPU RAM), cost not computed |
| D1 | save is one checkpoint in one 8 KiB card block | `save.rs` module doc: the payload is a few hundred bytes and stages through projection scratch so it costs no resident RAM | bound (card block 8 KiB is a hardware figure; a save may span blocks, so the one-block design is a choice made to avoid RAM) |
| D2 | a save restores what a changelevel carries, not every entity | `save.rs`: "no more, and no less"; CHANGELOG: map progress is kept | map progress fixed. Full-entity restore: unproven (not costed) |
| D7, E "Actor carry" | 15 carried records, 113 live props | `CARRY_CAPACITY` comment: the mailbox borrows unused prop-neighbour rows; LIMITS-L3 "Carried entities across a changelevel, 15" | recorded. The maps that exceed 15 (c1a3, c2a4 per the inventory) are not counted, so the shortfall is unproven |
| D8, E "env_global" | 32 global slots keyed by a 16-bit hash | LIMITS-L3: the 103 maps use 15 distinct names with no hash collision | recorded and proven (the cap is never reached) |
| F2 | on maps where mandatory dialogue fills the SPU, the suit logon falls back to the short line | the cooker prints "full HEV logon cannot fit beside mandatory map audio; using resident pickup line" (`audio.rs`) | bound (SPU RAM, decided per map by the cooker). The list of affected maps is in the cook log, not in this file |
| F3 | no HEV damage reports | none | not a limit. Streamed voice banks exist (`audio.rs`, streamed bank for the logon); this is a gap |
| F9 | flashlight is a screen-space cone, minimum reach 512 | comments near the flashlight shading in `main.rs` | bound (no per-pixel dynamic light on the GPU; vertex lighting is the available mechanism). The 512 reach is a tuning choice, unproven |
| G8 | pickup history shows one icon for three item kinds | `hud.rs` | unproven (the "34 sprite draw cap" is not in the tree) |
| I3 | no bucket weapon menu, next and previous only | README controls | not a limit (a DualShock has L1, R1, L2, R2 and the face buttons; this is UI work) |
| J5, E "Ambient sound concurrency" | 7 map-loop voices, 15 pooled one-shots, 1 dialogue, 1 charger | `hsfx.rs` (engine crate `psx-goldsrc`): one-shot pool voices 0 to 14 (`VOICE_POOL` 15), `DIALOGUE_VOICE` 15, `CHARGER_VOICE` 16, map loops 17 to 23 (`MAP_LOOP_VOICE_COUNT` 7), `AUTHORED_LOOP_CAPACITY` 32; 15 + 1 + 1 + 7 = 24 | proven for the total (24 SPU voices, all assigned). The split between pool and loops is an allocation choice, unproven |
| J7 | positional sound is centred | `hsfx.rs` comment "Hsfx voices are mono" (the inventory notes it is not a hardware requirement) | not a limit. SPU voices have left and right volume; this is a bug |
| J8 | per-map sound bank with a rate ladder, set-piece sounds dropped when over budget | `audio.rs` (`CORE_LEVEL_*`, `CORE_RAISE_CAP`, per-map messages) | bound (SPU RAM). The audit counted 45 maps dropping effects, a later row says it was not reproduced; no current list exists. Unproven count |
| J11, E "env_sound room effect" | 29 DSP presets approximated with SPU reverb parameters | `env_sound_audio.rs` header: modulation, low-pass and stereo parity open | bound (the SPU reverb is one fixed algorithm). What the 29 presets could still gain is unproven |
| J14, E "trigger_cdaudio" | music is XA on the data track, suspended while data streams | `music.rs` module doc; silicon test v1.28: CD-DA dies on any read, about a second each way between music and data | proven (hardware behaviour measured on the console) |
| K2 | brightness is a DEFAULT/DARKER/BRIGHTER stepper | `settings.rs` | not a gameplay difference: the PC has gamma cvars, the console has none, the stepper replaces them |

### Weapons, projectiles and effects (appendix B)

| row | difference | evidence in the tree | verdict |
|---|---|---|---|
| bullet decals by material, scorch decals | three decal families, no scorch | `sprite.rs` families; `MAX_IMPACT_MARKS` 24, `IMPACT_MARK_TICKS` 180 (the constants carry no limit comment) | unproven (no VRAM or pack cost for more families) |
| per-material impact sounds | one ricochet for every wall hit | `audio.rs` core bank | bound (SPU RAM), cost not computed |
| empty and not-empty reload and fire animations, idle variety, swing variants, loaded and unloaded variants, empty-gun fidgets (five rows) | one clip per state | `host/hl-build/main.rs` `WEAPON_MODELS`: clips are resampled to a stated number of poses and each mesh is decimated to the lowest ratio whose worst pose changes at most 200 px of the 320 by 240 coverage and whose mean pose changes at most 60 px | bound (RAM per weapon) with a stated quality criterion. The hl-cd-stream lane changes this budget by streaming viewmodels; re-judge after it lands |
| viewmodel pose sampling | decimated key poses per clip | same `WEAPON_MODELS` criterion | bound, measured criterion; same re-judging |
| hit sounds | one sample instead of two or three | `audio.rs` | bound (SPU RAM), cost not computed |
| rocket flight loop, smoke trail, light | rockets are 6 px flat rectangles | `render_projectiles` | not a limit for the trail and the light (cheap to draw); sound: bound (SPU voices) |
| gibbing | no overkill gibbing, corpse stays | `damage_prop`: health is `u8`, so it cannot go negative | not a limit. Widening 128 prop health bytes (`MAX_PROPS` 128) costs 128 bytes. A bug |
| damage-class tags to big monsters | `DMG_HEAVY` and `DMG_BLAST` set for egon and explosions; monster-side tables unread | `damage_prop` scaling comments | unproven (the inventory itself marks it unverified) |
| weapon sound set and variety | one sample per event, many sounds absent (hornet, tripmine, snark, egon loop and others) | the weapon event table in `audio.rs` | bound (SPU RAM). Cost per missing sound not computed |
| projectiles as models with trails | flat rectangles | `render_projectiles` | unproven |
| projectile pool | 12 live projectiles, a full pool recycles slot 0 | `MAX_PROJECTILES` 12, no comment | unproven (per-projectile RAM not given) |

### Monsters and NPC speech (appendices C and D)

| row | difference | evidence in the tree | verdict |
|---|---|---|---|
| F01 think cadence | AI on alternate ticks, line-of-sight every 4th | comment above `AI_REACQUIRE_INTERVAL`: tracing the unchanged target every tick was over half of c2a4e's actor-update cost | recorded (LIMITS-monsters "Simulation rate and activity"); evidence is a measured CPU share |
| F17 | no gibbing, no negative health, generic gore | `u8` health | not a limit (see "gibbing") |
| F33 | at most four copies stocked per maker, extra spawns reuse dead children | comment in the monstermaker code; `MAX_PROPS` 128 | bound (actor arrays). 36 retail makers exceed four spawns; the effect is visible only as reused corpses vanishing. Unproven that more would not fit |
| IC01, GA07 | Hard health (200/400/800/1000) scaled onto `u8` | `garg.rs` `scale_damage` comment | unproven. The scaling preserves the ratio only to rounding, which is not bounded here |
| S5 | one use-reply sentence per map | `audio.rs` defaults | bound (per-map voice budget, inferred, not computed) |
| S17, S27, B12, B14 (Barney) | no scream or fear sentences, no post-disaster idle talk, Barney silent, one pain and one death sound | `audio.rs` | bound (SPU RAM), inferred and not computed |
| S26 | idle chatter variants capped per map | comment at the idle speech table in `scientist_logic.rs`: five idle, two question, two answer lines per map | bound (SPU residency cap), cost per line not given |
| S28 | one speaker at a time | the dialogue voice count is 1 of 24 (`hsfx.rs`) | bound (all 24 voices assigned, see J5); the allocation is a choice |

### Systems (appendix E)

| row | difference | evidence in the tree | verdict |
|---|---|---|---|
| sequence aliasing | several retail sequences share one baked pose clip | comment at the scripted-sequence aux token and `entity-support.txt` | bound (RAM for pose clips). The audit "confirmed the clip-slot limits"; the figures are not in the tree. Unproven |
| func_breakable shards | one shard per 432 square units, capped at 40 (client uses up to 100) | comment at the shard spawn | unproven (draw-packet cost not given) |
| func_pushable | only the player moves a cart; buoyancy eased | comment at `pushable_float_step` | not a limit for monsters pushing (logic); buoyancy: unproven |
| env_sprite and env_glow | fixed 10 fps, always additive, framerate, rendermode, renderamt, rendercolor, renderfx ignored; caps of 12 sprites, 49 frames, 160 instances | `sprite.rs` header: the full campaign peaks at 49 frame textures and the audited VRAM peak still fits | caps: bound (VRAM). Ignored keys: not a limit (CPU-cheap). The two halves of this row need splitting |
| bullet, blood and scorch decals | 24 marks, 9 s lifetime | `MAX_IMPACT_MARKS`, `IMPACT_MARK_TICKS`, no limit comment | unproven |
| water surface, fog, far view | no wave warp, colour grade underwater, fog and far cull at 1200 | `FAR_VIEW` comment: "a measured compromise", c0a0b and c2a5 costs of the wider views are quoted | far view: bound (frame time, measured). Surface warp and refraction: unproven |
| brush render modes and renderfx | four classes; intermediate renderamt, rendercolor, renderfx ignored | `brush_render_class`; `entity-contract.txt` | bound for arbitrary alpha (four fixed blend functions). Tint and pulse: not a limit |
| studio model lighting | one flat shade for every model | `MODEL_SHADE` 110 | unproven (the GTE does vertex lighting; the cost was not weighed) |
| lightstyles 1 to 12 | animated lights baked at the pattern average | CHANGELOG: lights switch and flicker; LIMITS-L5 | recorded |

## 5. How to close a row

A row leaves this table in one of three ways:

- the difference is fixed, with a journey or a parity note that shows it;
- the hardware bound is demonstrated: the next unit does not fit, shown by a build that fails to link, a measured
  frame time over budget, or a retail content count that stays under the cap; the row moves to a LIMITS file with
  that evidence;
- the row is accepted as a deliberate deviation by a decision that is written down with its date.

Until then every row marked bound, unproven or not a limit is on the bug list.
