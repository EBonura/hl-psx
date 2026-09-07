# hl-psx

Half-Life for the original PlayStation, built in Rust on the
[PSoXide](https://github.com/EBonura/PSoXide) SDK.

HL-PSX is a bring-your-own-assets port. The build reads maps, models, sprites,
sounds, dialogue, music, and UI resources from your own Half-Life installation
and converts them into data suitable for the PlayStation. Valve assets and
generated disc images are not distributed by this repository.

> [!IMPORTANT]
> HL-PSX is a noncommercial, source-only compatibility project. Building it
> requires a lawfully obtained local Half-Life installation. Generated data and
> BIN/CUE images are local outputs and must not be redistributed as project
> releases. See [LICENSING.md](LICENSING.md),
> [PROVENANCE.md](PROVENANCE.md), and
> [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

## Status

HL-PSX is under active development and is tested in both PSoXide and on
original PlayStation hardware.

| Area | Current state |
| --- | --- |
| Content | The campaign and Hazard Course pipeline cooks and audits all 103 runtime maps. |
| Gameplay | GoldSrc-style movement, weapons, damage, scripted entities, doors, trains, NPCs, map transitions, saves, and menus are implemented. |
| Presentation | PlayStation-native world/model rendering, animated models, HUD, sprites, dialogue, sound effects, and CD audio are supported. |
| Target | 320×240 output with a 20 fps simulation target on original hardware. |
| Distribution | Source only. No original or converted Valve assets and no generated disc image are included. |

This is not a finished retail-quality release. Compatibility, visual accuracy,
performance, and real-hardware behaviour continue to be improved.

See [CHANGELOG.md](CHANGELOG.md) for recent performance measurements
and fixes.

## Build from source

You need:

1. A lawfully obtained Half-Life installation from Steam or retail.
2. Rust installed through [rustup](https://rustup.rs/). Use rustup rather than
   an operating-system Rust package so the repository can select its pinned
   nightly toolchain automatically.
3. Python 3 and `mipsel-none-elf-objdump` on `PATH`. The build uses them to
   patch and verify R3000 load-delay hazards in the final executable.
4. Host build tools/linker for Rust executables (Xcode Command Line Tools on
   macOS, a C/C++ toolchain on Linux, or Visual Studio Build Tools on Windows).
5. Internet access for the first build so Cargo can download that toolchain,
   the Rust dependencies, and the pinned PSoXide sources.

You do not need a separate PSoXide checkout, Make, or FFmpeg. On Windows,
ensure the Python executable is available as `python3`, as invoked by the
builder.

### 1. Get the source

Download and extract the source archive, or clone it with Git:

```sh
git clone https://github.com/EBonura/hl-psx.git
cd hl-psx
```

If you downloaded the archive, open a terminal in the extracted `hl-psx`
folder instead.

### 2. Build the game

Run this one command from the repository root:

```sh
cargo run --release -- build
```

The builder finds Half-Life automatically in the standard Steam location on
macOS, Linux, and Windows. If it says that Half-Life was not found, provide
either the Half-Life installation folder or its `valve` folder:

```sh
cargo run --release -- build --half-life "PATH_TO_HALF_LIFE"
```

Keep the quotation marks when the path contains spaces.

The first build takes longer because it converts the complete campaign and
Hazard Course, then compiles the PlayStation executable and packs the disc.
When it finishes, the playable image is:

```text
dist/hl-psx.cue
```

The adjacent `dist/hl-psx.bin` file is part of the same disc and must remain
beside the CUE file. To rebuild after updating the source, run the same
`cargo run --release -- build` command again.

All converted content stays inside this checkout under `data/`, build state
and reports stay under `.hlpsx/`, and the final image stays under `dist/`.
Nothing is copied elsewhere unless you explicitly use `install` with
`--games-dir`.

## Run

Open `dist/hl-psx.cue` in PSoXide or another compatible PlayStation emulator,
or use a suitable development or homebrew loader on original hardware.

The generated image does not include a licensed Sony system area. Hardware
boot methods may require an appropriate loader or a separately supplied system
area.

## Controls

| Input | Action |
| --- | --- |
| Left stick / D-pad | Move |
| Right stick | Look |
| Cross | Jump |
| Triangle | Duck |
| Square | Use |
| R2 | Primary attack |
| L2 | Secondary attack |
| Circle | Reload |
| R1 / L1 | Next / previous weapon |
| L3 | Flashlight |
| Start / Select | Pause |

In menus, use the D-pad to navigate, Cross or Start to confirm, and Circle or
Select to go back.

## Other commands

```sh
cargo run --release -- check                  # locate and validate Half-Life
cargo run --release -- sdk                    # hydrate the pinned PSoXide SDK
cargo run --release -- assets                 # recook all derived assets
cargo run --release -- models                 # recook and audit models
cargo run --release -- audit                  # audit map and transition residency
cargo run --release -- compile                # rebuild the PS1 executable
cargo run --release -- pack                   # rebuild the executable and disc
cargo run --release -- install --games-dir "DESTINATION_FOLDER" # build and copy
cargo run --release -- regress --psoxide "PATH_TO_PSOXIDE"      # test matrix
cargo run --release -- --help                 # show every option
```

`HL_DIR`, `PSOXIDE`, and `GAMES_DIR` are accepted as environment defaults.
Normal builds hydrate the pinned historical PSoXide revision, which includes
the SDK, engine and audio cooker. Their current source owners are
[the SDK](https://github.com/EBonura/PSoXide) and
[the editor/engine](https://github.com/EBonura/PSoXide-editor). Contributors
can test a bootstrapped editor checkout with
`--psoxide "PATH_TO_PSOXIDE_EDITOR"`; an SDK-only checkout does not contain
the required engine/cookers. Such overrides change the effective build input
and must be revalidated. The standalone emulator is maintained in
[PSoXide-emulator](https://github.com/EBonura/PSoXide-emulator).

## PlayStation design

HL-PSX reshapes GoldSrc data around the original console rather than treating
the PlayStation as a smaller PC:

- GoldSrc BSP visibility and room-local streaming keep world data bounded.
- World, brush, actor, and viewmodel projection use the Geometry
  Transformation Engine.
- Textures are converted into compact PlayStation-native pages and managed
  through explicit VRAM checkpoints.
- Animated models use map-local clip sets and fixed-point bone palettes.
- Maps, models, dialogue, sprites, and sound banks are streamed from CD as
  needed.
- A sector-aware packing policy compresses a chunk only when it saves disc
  sectors and passes its runtime-capacity checks.
- Build-time audits cover model RAM, animation mappings, entity support,
  transition residency, texture capacity, and disc packing.

The deterministic regression suite combines fixed visual probes, recorded
input routes, frame telemetry, and repeat renders. Original hardware remains
the final authority for timing, audio, controller, and CD-loading behaviour.

## Repository layout

| Path | Contents |
| --- | --- |
| `game/` | The `no_std` PlayStation game crate |
| `host/hl-build/` | End-to-end build orchestration |
| `host/hl-content/` | Menu, audio, sprite, and manifest compiler |
| `host/hl-bsp/` | GoldSrc BSP and studio-model cooker |
| `host/hl-logic-tests/` | Host-side gameplay and entity tests |
| `shared/hl-format/` | Formats shared by host tools and the game |
| `regression/` | Deterministic emulator scenarios and expectations |

## Validation

Useful source checks for contributors:

```sh
bash scripts/check-source-only.sh
cargo fmt --all -- --check
cargo test --locked
cargo test --manifest-path host/hl-bsp/Cargo.toml --locked
cargo test --manifest-path host/hl-content/Cargo.toml --locked
cargo test --manifest-path host/hl-logic-tests/Cargo.toml --locked
cargo test --manifest-path shared/hl-format/Cargo.toml
```

The full content and hardware validation passes require a local Half-Life
installation and a suitable PSoXide or PlayStation test setup.

## Licensing

Code that the project author is entitled to license is offered under
GPL-2.0-or-later. The project also contains an attributed adaptation of
Quake's GPL-licensed recursive hull trace, links to PSoXide's
GPL-2.0-or-later SDK, and includes compatibility code informed by Valve's
public Half-Life 1 SDK.

No original or converted Valve binary game assets are tracked or distributed.
The project licence does not grant rights in Valve, Sony, or other third-party
names or material. Half-Life is a trademark of Valve Corporation. PlayStation
is a trademark or registered trademark of Sony Interactive Entertainment Inc.
HL-PSX is unofficial and is not affiliated with or endorsed by Valve or Sony.

See [LICENSING.md](LICENSING.md) for the complete distribution policy and
primary licence links.

## Recent changes

Source snapshot **2026.09.05.1** removes two unused renderer experiments.
The previous snapshot improved actor depth ordering and rectangular wall grids.
See the [changelog](CHANGELOG.md) for details and download availability.
