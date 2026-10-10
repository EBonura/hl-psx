# Changelog

## Unreleased

- Doors and trains the maps mark passable no longer block. Retail spawns a
  `func_door`, `func_train` or `func_tracktrain` with spawnflag 8 non-solid,
  and the cooker gave all 153 of them a collision hull (99 doors, 51 trains,
  3 tracktrains), so the player stood against walls the original lets him walk
  through and Interloper's invisible trains pushed him around.
- A moving door or train is no longer held still by a barnacle, tripmine,
  cabinet or Xen plant in its path. Retail's pusher skips every entity with
  MOVETYPE_NONE; here a small fungus froze Interloper's stalk doors for the
  whole map.
- Add `hl-census`, which checks the shipped maps against the port: a reviewed
  contract of every class, keyvalue and spawnflag, retail-versus-port
  differentials for target dispatch, brush solidity, trigger volumes and
  breakable health, and the changelevel graph.
- Add an off-by-default `interaction-probe` build feature that fires targets,
  kills monsters and teleports the player from a baked script.

- `cargo hl-build build` links again. Without a sample profile the game had
  outgrown RAM (`.bss` over by about 4 KB), so only `pgo` with a recording
  could make a disc. The profile and threshold `pgo` shipped with now live
  in `game/pgo/`, every other build links with them, and a plain build
  reproduces the profile-guided disc byte for byte.

- Building no longer needs Python or `mipsel-none-elf-objdump`. The
  scratchpad projection stack check runs through the pinned SDK's
  `stack-guard`, which reports the same depth from the linked image without
  a separate disassembler, and on Windows the builder no longer dies with
  `STATUS_STACK_OVERFLOW` after the asset cook.

- Start every chapter where normal play arrives in it. Chapter select used
  the first map's `info_player_start`, which in most retail maps is a
  developer spawn elsewhere in the level; each chapter now replays the
  landmark carry of the previous map's changelevel, with its view, its
  `changetarget` (Office Complex's elevator, Lambda Core's doors), and the
  suit, long jump and weapons a playthrough has collected by then.
  Unforeseen Consequences begins in c1a0c's black room after the resonance
  cascade instead of c1a1's corridors, Lambda Core in c3a2e instead of c3a2,
  and Gonarch's Lair is listed before Interloper, as in retail. Each
  chapter's map list follows play order.

- The builder is now `cargo hl-build ...` (an alias for
  `cargo run --release --manifest-path host/hl-build/Cargo.toml --`), run from
  the repository root. `cargo run --release -- ...` no longer works. The
  builder moved to its own workspace under `host/hl-build/`, and the
  repository root became the PS1 game's workspace, so every path dependency
  sits inside it and Cargo hashes it by relative path. The game image no
  longer depends on where the repository is checked out.

- Add `cargo hl-build pgo --tape PATH`, a profile-guided pack. PSoXide
  counts every guest instruction while it replays the tape, and the compiler
  lays the game out around those counts. The chapter-two recording improves
  from 12.22 to 12.94 rendered FPS and its laboratory from 6.75 to 7.49; a
  tram ride the profile never saw improves from 17.85 to 18.61. Gameplay state at the end of the recording is
  bit-identical.

- Let LLVM fill branch delay slots from the successor block and after calls.
  More than half of the executed branches carried a nop. The chapter-two
  recording improves from 11.99 to 12.22 rendered FPS with identical gameplay
  state, and the executable frees 10 KB of RAM.

- Check after every link that the model projection chain fits the scratchpad
  stack, instead of finding out at run time and falling back to main RAM.

- Reserve 80 bytes for entity dispatch indices so crowded maps retain their
  trigger, timed-entity, spark, and beam lists instead of repeatedly scanning
  every entity. On the full post-lift chapter-two recording, moving laboratory
  performance improves from 6.98 to 7.23 FPS; moving gameplay overall improves
  from 12.26 to 12.32 FPS. Menus, loading, and stationary intervals are excluded.

- Reuse the shared diagonal when checking GPU quad size limits. On the fresh
  chapter-two recording this improves 12.82 to 12.87 rendered FPS excluding
  loading, with identical final display, VRAM and gameplay state.

- Skip unrelated entity records in the spark and beam fallback scans. The recorded
  Anomalous Materials run improves from 12.46 to 12.98 rendered FPS excluding
  loading; its late laboratory section improves from 4.79 to 7.04 FPS in PSoXide.
- Check carried-player clearance on rotating lifts so coordinate rounding into
  a side wall can use the existing bounded collision recovery.
- Scientists and guards now answer when spoken to, including refusals to follow before the disaster.
- Fixed the canteen microwave's overlapping buttons so repeated presses register
  on the next button instead of the one that already moved away. The dish does
  not explode yet.
- Fixed floating seated scientists and incorrect poses for dead scientists.
- Fixed missing scientist heads in crowded scenes and the overturned cabinet drawing through a body.
- Removed the redundant button legend from the pause menu.

## Source 2026.09.05.1

- Removed the unused packet-ready world renderer and 4x4 tessellation experiment.
- Kept the production 2x2 refinement and the existing cooked-map format.

This source cleanup does not replace the validated discs. No new binary download
accompanies this tag.

## Source 2026.09.05

This source snapshot is tagged `source-2026.09.05`. Download versions are
listed separately below; source cleanup does not replace an already published disc.

- Improved actor depth ordering within the existing PSX ordering-table buckets.
- Preserved rectangular wall grids across boundary welding to reduce affine texture distortion.
- Kept host-only helpers out of guest builds and prepared a source-only public export.

No new binary download accompanies this source snapshot.

The public release is source only and requires a local Half-Life installation.
The private combined-disc build is not an itch.io download.
