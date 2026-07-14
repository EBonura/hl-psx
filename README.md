# hl-psx

A bring-your-own-assets Half-Life renderer for the PlayStation 1, written in
Rust on the [PSoXide](../PSoXide) SDK. It cooks the maps, models, sounds, and
sprites out of your own Half-Life installation into PS1-native formats, packs
them onto a disc image, and plays the campaign on PS1 hardware or in the
PSoXide emulator. No Valve assets are included in or distributed with this
repository.

The build assumes a sibling-checkout layout: a PSoXide working tree next to
this one (`../PSoXide`). Override the location with `PSOXIDE=/path/to/PSoXide`
on any make invocation.

## Requirements

- A Half-Life installation (Steam or retail). Default path is the macOS Steam
  location; point `HL_DIR` elsewhere either on the command line or persistently
  via a git-ignored `config.mk`:
  `echo 'HL_DIR = /path/to/Half-Life' > config.mk`
- A sibling `../PSoXide` checkout (SDK, emulator, and the `mkisopsx` disc tool).
- Nightly Rust (see `rust-toolchain.toml`): the `mipsel-sony-psx` target has no
  prebuilt std, so the build uses `-Zbuild-std`.

No Python, external media converter, or C/C++ compiler is required. The host
content pipeline—including MP3 decoding—is built and run by Cargo.

## Quickstart

```sh
make psoxide-check   # verify the sibling PSoXide checkout is where we expect
make assets          # cook menu, rooms, models, sfx, voices, sprites from HL_DIR
make                 # build the PSX-EXE, pack the disc, install it into the
                     # PSoXide game library (boots from the emulator's library)
```

`make help` lists everything else, including `make compile` (fast EXE-only
rebuild) and `make disc` (burnable .bin/.cue into `dist/`).

## Layout

| Directory   | Contents |
| ----------- | -------- |
| `game/`     | The PS1 game crate (its own Cargo workspace, builds for `mipsel-sony-psx` by default) |
| `host/`     | Rust content compiler and BSP/model cooker (runs on the development machine) |
| `data/`     | Cooked assets from your install (git-ignored, never committed) |
| `dist/`     | Packed disc images (git-ignored) |
| `captures/` | Headless screenshots, profiles, and reports (git-ignored) |
| `docs/`     | Local working notes (git-ignored) |

## Headless verification

These drive the game in the PSoXide emulator without a GUI and drop artifacts
into `captures/`:

```sh
make psoxide-smoke                # boot to the menu, screenshot + hash
make psoxide-gameplay             # New Game into c1a0, gameplay screenshot + hash
make psoxide-profile              # telemetry build, per-vblank CSV + screenshot
make psoxide-map-smoke MAP_INDEX=N  # boot campaign map N directly
make psoxide-chart                # profile + self-contained HTML vblank chart
```

## Licensing

The code is GPL-2.0-or-later: it links against PSoXide's GPL-2.0 SDK crates
(see PSoXide's `docs/downstream-licensing.md` for the reasoning). No Valve
assets are included or distributed; the extractors and cookers read from your
own Half-Life installation at build time, and everything derived from it stays
git-ignored. You need to own Half-Life to play this.
