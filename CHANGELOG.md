# Changelog

## Unreleased

- The startup movie program hands the game the disc bases the launcher gave it. It used to start the game with both at zero, so on a multi-program disc such as the demo disc the game read its packs at unshifted sectors and failed to stream its world and sound.
- Pushing the c1a0e sample cart no longer stalls three units short of it. The cart rides a lift with a one-unit lip at x 1180, so a player walking east from the lift's west margin stops flush against the lip's hull and can only reach the cart by a stair step. That step's contact with the cart's hull floored onto the hull plane, `slide_move` threw the whole leg away whenever such a point was not clear, the step was rejected, and the player stayed outside the cart's touch range (hull plus two units), so the cart never moved. Whether it happened depended on the approach (position, speed and the sub-unit state of the walk): in a sweep of 216 full-speed starts between x 1161 and 1163 it happened 96 times (44%), on the 5dac694 and b5a58b2 builds alike. A rejected axial contact now gives back one unit along the contact normal instead (never behind the start of the leg); the same sweep stalls 0 times. Costs 112 B of RAM (3,196 B free). `tests/probes/hl-cart-push.txt` replays 48 starts of the same kind (new probe op `cartreset`).
- Map decals are drawn. The maps paint 1,542 infodecals (blood, scorch marks,
  graffiti, stencilled signs such as the Hazard Course numbers and "STORAGE
  AREA 1") and the cook used to throw them away. 1,320 now show: each one flat
  quad on the surface retail puts it on (the dominant fixed surface within 4
  units, along that surface's own texture axes), sorted into the world like the
  bullet and blood marks and hidden outside the view's visibility and frustum.
  99 sit on moving brushes and 89 have no surface to land on, as in retail; c5a1
  and c2a4e draw only what their weapon-cache margin allows. 10 bytes a decal in
  the room and one 128x128 texture per room; see LIMITS-L5.md.
- Lights switch and flicker. Every targeted light (763 in 64 maps) switches the
  faces it lights on Use, both ways, and the standard animated styles and
  patterned lights step at 10 Hz (the c1a0c disaster flicker, c1a3's strobes). The
  map's start state stays baked and each lit face stores how its colours change
  at the other end of the style's swing, so a light left alone costs a flag test
  per face. The dynamic-light code is smaller than before, and the two features together cost 616 B of
  RAM (2,692 B free, was 3,308). c5a1 and c2a4e bake
  their lights at the start state: they have no weapon-cache margin to carry
  the 4 to 7 KB. The rooms lose the 48 byte world pipeline census header the new
  section replaces.
- The editor component pin carries one extra commit over editor main c096eee0 (df38b0fd, branch `fix/chainloader-cd-token-v2-2026-10-16`): the engine's `psx-chainloader` builds its sector reader from a `Cd` token and calls the SDK's safe reader methods without `unsafe` blocks, so the intro (which links it) builds without its five warnings. The game does not link that crate; its executable is byte-identical. Needs `cargo hl-build assets` (the cook digest covers the lock file). Landing: EBonura/PSoXide-editor branch `fix/chainloader-cd-token-v2-2026-10-16`.
- The disc opens with Half-Life's own startup movie (the one `media/StartupVids.txt` in your install names, `valve.webm`), skippable with Start or Cross, and then loads the game. The game image has 3,196 B of RAM free, so it carries no player: a 51 KB boot program (`intro/`, linked at 0x801E0000, `PSX.EXE` on the disc) streams the movie through the SDK's MDEC player (320x240, 15 fps, 15-bit, mono XA sound, 148 frames, all shown, none late in the emulator) and then the SDK's chain loader reads the game's executable, now the root file `HLGAME.EXE`, over the low RAM the player used, checks its checksum in RAM and jumps to it. The game's executable is byte-identical to the build without the intro (sha256 42c276c9...), so RAM, speed and every gate are unchanged. The disc grows by 1,852 sectors (4.2 MiB: the movie 1,478, the game executable 374); no pack LBA moves. In the emulator the title menu comes up at route tick 720 when the movie is skipped at once and 1,320 when it plays to the end, against 440 without the intro (60 ticks a second): 4.7 s for the handover, mostly the executable's 374 sectors at 2x, which the BIOS reads at boot on a console anyway. `host/hl-intro-cook` encodes the movie from your install with `psxavenc` (see the README); without the encoder or the movie the disc is built without the intro stage and boots directly. `--no-intro` leaves it off on purpose, and `stack`, `regress` and the PGO collection discs never carry it. Nothing derived from the movie is written outside `.hlpsx/intro/`. The cook fits the sound to the player: the CD sound loses 0.9 dB of loudness in the 37.8 to 44.1 kHz conversion on its way to the output, so it measures the movie's integrated loudness (-19.8 LUFS), raises the sound by 0.9 dB (through a look-ahead limiter at -1 dBFS that does nothing, the movie's peak being 2.5 dB under full scale) and re-encodes it, which makes the emulator's SPU output -16.8 LUFS, the level of the original played to both speakers; this level assumes the SPU main volume 0x3FFF is applied at unity (EBonura/PSoXide-emulator `fix/spu-main-volume-scale-2026-10-16`; before it the emulator played it at half scale). It also places each sound sector 24 sectors (160 ms) later than the encoder does, which took the picture from 170 ms behind the sound to within 5 ms (measured on the emulator by fitting the picture's luminance and cross-correlating the sound against the source movie), and needs `psxavenc` to encode the sound twice, no other tool. The journeys press Start early (a disc without the movie ignores it) and `tests/intro.toml`, `tests/intro-skip.toml` (Start) and `tests/intro-skip-cross.toml` (Cross) check the movie, its sound, the skip and the arrival at the title menu.
- Frame rate +7 to +16% from the profile-guided build, for 1.3 KB of RAM. The sample profile kept every function the replays touched and grew `.text` by 47 KB, which is why the committed one was stale; `pgo` now keeps the 32 hottest frame functions and the inliner's hot threshold is 50 instead of 3000. Rendered fps from the title menu, old -> new: c0a0 tram 14.67 -> 16.97, c1a0 6.22 -> 7.11, c1a2b 5.02 -> 5.77, c4a3 16.49 -> 17.70, and on maps the profile never saw c1a1 29.41 -> 32.03, c3a2b 8.46 -> 9.75, c2a2 11.50 -> 12.53, c1a3 12.58 -> 13.65. The I-cache layout is no longer shipped (see the README).
- The actor animation tracks use 1.4 grid steps (0.35 of a world unit of worst-case vertex error, was 0.5) and the map staging floor is 177,280 words (was 178,120): c5a1's weapon-cache margin is 14,164 B before the floor change and 10,804 B after it, with room for the authored HECU grenade clips. Needs `cargo hl-build assets`.
- The stack reserve is `game/stack-reserve` (25 KiB, was the SDK's 32 KiB), and `cargo hl-build stack` holds it: it replays the shipped disc on eight maps entered through the title menu and fails when the emulator's stack watermark (9.5 KB at most) comes within 25% of the reserve, and it relinks the shipping image as an ELF and bounds every call chain statically, with the BSP trace recursion taken to the campaign's deepest trees (c2a4a, 84 levels of 136 B): 19,664 B, which with the margin sets 25 KiB. `regress` runs the same check on its shipping rebuild. Free RAM +7,168 B. `cargo hl-build ram` now reads the reserve from the link map.
- Free RAM went from 4,008 B to 32,184 B with no change to what the game does. `cargo hl-build ram` links the shipping image and prints `free N B` (the SDK's check) with a per-section, per-crate and per-module breakdown; `--map PATH` reads an existing link map. `game/ram-budget` holds a floor (4,096 B) that every shipping compile, pack and disc build enforces, so a change that eats the headroom fails the build. The bytes came from the map staging floor (-8,192 B, weapon cache margin 13,472 to 5,280 B at c5a1), `#[optimize(size)]` on the per-tick logic, tram, scientist, leech, physics, save and setup code (-13 KB of text; the fixed update is 4.4% of a frame and measured +0.09% of the frame on c1a0 and +0.4% on c4a3), and a 768 B zero table. A fresh PGO profile does not link at any inlining rung (+55 KB at 1000, +27 KB at 125), so the committed profile stays.
- Death waits for you. The red death screen used to end by itself after three
  seconds. It now holds for two seconds (the cap retail puts on the death
  animation), then waits until every action button has been let go and one is
  pressed again, and only then reloads the latest save point (or the level
  entry when there is none). Stick movement does not count as a press.
- Autosave points and walking back. A trigger_autosave (a first touch by the
  player, once) now keeps the map as a save point in RAM, and dying reloads it,
  as retail does; the card is still written only by a manual save. Leaving a
  map by changelevel keeps it as it stood, so walking back finds dead monsters
  still dead, doors where they were and pending events pending, instead of a
  fresh level (one map back; a save point in the new map replaces it).
  Breakables with a globalname (the airlock glass, vents and busted light of
  c1a1b/c1a1c/c1a1f) stay broken in the maps that share the name. Dying with no
  save point restarts the map from its entry with its globals as they were on
  arrival. A save point or a map walked back into resumes the tick count it was
  taken at. Pushed crates (func_pushable) are kept
  where they were left. A saved map progress that held a removed trigger (state 255) was
  decoded wrongly and is fixed. Costs 3.2 KB of RAM over the previous commit
  (796 B free), most of it the second capture slot.
- A save keeps the map's progress, and dying reloads it. A manual save used to
  keep only the player, so loading respawned every monster and reset every
  door. It now also records which monsters are dead (and where a following
  scientist stands), the state of every door, button, trigger, counter,
  charger, monster maker and breakable, the removed sprites and the light
  switches, in 0.4 to 0.9 KB measured on seven maps (limit 1 KB; a map that
  needs more is not recorded and a death restarts it from its entry as
  before). The save is held in RAM as well, so a death after a save reloads
  that save, as retail does, instead of the level entry. Not recorded:
  trains and tracktrains, scripted sequences in flight, boss
  schedules, and where monsters other than followers stand (the living go
  back to their spawn with their health). Delayed events not yet fired are
  recorded.
  Saves made before this one load as they always did.
- Leeches swim and bite. The 105 leeches of c2a1a, c2a2a, c2a3, c2a3a, c2a3b,
  c3a1 and c3a1a were drawn but never moved. Each now drifts at 50 units a
  second inside the middle of its water column, hunts a player who is in the
  water and in view (750 units, 240 degree cone) at up to 100 units a second,
  chases his eye height, and bites 2 on every skill from 40 units away while it
  faces him; every bite shortens the hunt by 2 s. Out of the water it drops and
  stops. Like retail it only thinks while the player can see its room. At most
  eight leeches swim at once (RAM); a ninth in view holds its pose until one
  leaves. There are no leech cries yet (the sound banks carry no leech
  samples), and the swimming pose does not lean or bank.
- A scientist that follows you heals you. At half health or less, a following
  scientist within 128 units walks up to 50 units, takes out the needle and
  gives the shot 3.6 s later: 25 health, then a minute before that scientist
  heals again; walking more than 90 units away mid shot spoils it. The retail
  needle animation and the "SC_HEAL" line are not in the cooked scientist, so
  the scientist stands still while it happens.

- Explosions need a line to what they hurt, and stop at the water surface.
  Rockets, grenades, satchels, tripmines and the MP5 grenade damaged every
  monster and the player inside their radius, through walls and doors. Like
  retail's RadiusDamage, a blast now reaches a victim only when nothing solid
  lies between them, a blast set off in water spares anyone with dry feet, and
  a blast in air spares anyone whose eyes are under. Blast distance is still
  taken to the victim's centre rather than its nearest surface.
- One crowbar swing kills a barnacle. Retail turns any club hit into a hit for
  all its health; the port needed three swings.

- Xen's skies keep their colour. The sky was one 16 colour palette shared by
  all six cube faces, which turned the nebulae's oranges beige and their
  dither into grain. Each face is now an 8 bit texture against one 256 colour
  palette, so the blues, greens and oranges of the source images survive. A
  sky costs 48 KB more of VRAM (three texture pages instead of one and a half)
  and 49,472 B more in its room's texture chunk, which is staged in the map
  buffer and not kept; free RAM drops by 2,064 B for the loader. The cooker
  marks an 8 bit texture with the top bit of its width, so every other
  texture and every room without a sky is byte for byte what it was.

- No black wedges or hairline cracks in the sky. A sky polygon spanning
  several cube faces skipped any face that crossed it between corners lying
  on other faces, which left a wedge of clear colour at the screen's edge
  as the view turned. Pieces now grow outward along their edges' normals,
  and the screen's last column and row are covered.

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
