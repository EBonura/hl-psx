# HL-PSX provenance and release audit

Release audit refreshed: 2026-09-07. This is an engineering and provenance
review, not legal advice. The release policy is source-only, noncommercial,
bring your own assets, with no generated asset packs or disc images published.

## Distribution inventory

The tracked repository was enumerated with `git ls-files`, and every tracked
file was classified. No Valve-authored or converted binary game assets were
found: there are no tracked BSP maps, MDL models, WAD textures, sprites, sound
or music files, retail fonts, PAK/GCF archives, BIN/CUE images, PlayStation BIOS
images, or Sony system-area data.

The repository does contain material that should not be described as “no
Half-Life content”:

- Half-Life map, model, entity, weapon, chapter, and sound identifiers used for
  compatibility and local conversion;
- chapter titles and descriptive Half-Life references in the user interface;
- Rust gameplay, physics, and entity behaviour informed by Valve's public
  Half-Life 1 SDK; and
- a project-authored Bonnie Studios logo embedded as pixel data in
  `host/hl-content/src/bonnie_logo.rs`.

Generated data is written beneath git-ignored `data/`, `dist/`, and `.hlpsx/`
directories. The build orchestration contains no network upload client. Cargo
does need network access on the first build to fetch declared dependencies.

## Source provenance

### Valve Half-Life 1 SDK

The public SDK was used as a source and behavioural reference for entity logic,
movement constants, animation timing, map progression, and data formats. This
is source-informed Rust adaptation work, not a clean-room implementation. No
Valve C/C++ source files or compiled engine binaries are tracked in this
repository.

The SDK licence permits free source/object distribution of modified Valve
games running on the Half-Life 1 engine and permits free distribution of the
SDK and modifications under its notice requirements. It directs commercial
users to contact Valve. HL-PSX is deliberately noncommercial, distributes only
project source, and requires users to supply their own lawfully obtained game
data. This record does not claim that project policy settles rights outside
the scope of Valve's published terms.

Reference audited: ValveSoftware/halflife commit
`b1b5cf5892918535619b2937bb927e46cb097ba1` and its `LICENSE`.

Official source: <https://github.com/ValveSoftware/halflife>

### Quake

`game/src/phys.rs` contains fixed-point Rust adaptations of Quake's
`SV_RecursiveHullCheck` from `WinQuake/world.c` and of the multi-plane slide
move `SV_FlyMove` from `WinQuake/sv_phys.c`, modified for cooked GoldSrc hulls
and integer arithmetic. This provenance is now stated in the file. The
Quake source is GPL-2.0-or-later compatible with this repository's licence.

Reference audited: id-Software/Quake commit
`bf4ac424ce754894ac8f1dae6a3981954bc9852d`.

Official source: <https://github.com/id-Software/Quake>

Other BSP, PVS, clipping, and slide-movement concepts share the Quake/GoldSrc
lineage. Where a routine is translated or closely adapted rather than merely
implementing a file format or observed behaviour, it must carry an explicit
source note before release.

### Xash3D FWGS

Project notes identify Xash3D FWGS as a behavioural oracle. The current audit
found no Xash dependency, copied source file, distinctive verbatim source
block, or distinctive copied table in tracked HL-PSX code. It must not be
described as never used; the accurate statement is that it was consulted for
behaviour and no Xash code is known to be included.

Reference audited: FWGS/xash3d-fwgs commit
`f0ea3a194ab06d56032c5d26578254698e361655`.

Upstream source: <https://github.com/FWGS/xash3d-fwgs>

### PSoXide

The build pins PSoXide commit
`8df242b353b8a3664c1d2ed20622d692d1349306`. PSoXide crates and tools are
GPL-2.0-or-later. PSoXide documents that parts of its emulator derive from
GPL-licensed PCSX-Redux; that downstream provenance belongs to PSoXide and is
not evidence that HL-PSX itself contains PCSX-Redux code.

The default HL-PSX disc command does not pass PSoXide's optional `--system-area`
argument. Generated BIN/CUE images therefore omit Sony licensed system-area
data. No retail BIOS is tracked by either audited repository revision.

## Public-policy cross-check

- Valve's current Half-Life 1 SDK licence expressly describes a modified Valve
  game running on the Half-Life 1 engine. It does not expressly describe a
  source-informed implementation on a separate runtime.
- The Steam Subscriber Agreement permits certain noncommercial content made
  using Valve Developer Tools and certain noncommercial fan art, subject to
  separate terms and third-party rights. It is not a blanket permission to use
  Valve trademarks or assets merely because a project is distributed on Steam.
- Codename: Gordon is real, free, and hidden from the store under app ID 92,
  but Valve's Developer Community identifies it as officially licensed and
  published by Valve. It is evidence that Valve can approve a special case,
  not that unapproved fan games inherit the same permission.
- Valve's Video Policy permits noncommercial videos containing Valve game
  content and permits ordinary platform-partner monetization, but prohibits
  distributing game assets separately and does not cover third-party content.
- Sony identifies PlayStation as its trademark. Omitting BIOS, SDK, and system
  area material reduces the Sony-content issue but does not create a Sony
  licence or endorsement.

Official policies checked:

- <https://store.steampowered.com/agreement/?l=english>
- <https://store.steampowered.com/video_policy>
- <https://developer.valvesoftware.com/wiki/Codename:_Gordon>
- <https://sonyinteractive.com/en/copyright-and-trademark-notice/>

## Historical similarity review

The following similarity scan was performed on 2026-07-16. It remains useful
as a historical review, but the source has changed materially since that date
and the measured counts should not be presented as current release metrics.

Tracked Rust sources were compared against the three reference trees above
using identifier overlap, exact seven-word comment phrases, normalized
50-token windows, and numeric-sequence checks. The scan found no meaningful
verbatim comment block, distinctive copied source window, or distinctive
copied table. Shared identifiers and generic structural windows were expected
for implementations of the same formats and algorithms.

The measured identifier overlap was 514 of 4,827 HL-PSX identifiers with the
Valve tree, 395 with Quake, and 525 with Xash3D. Normalized-window hits were
manually reviewed and were generic getters, control flow, and format-handling
shapes rather than distinctive copied blocks. These counts are indicators for
manual review, not copyright conclusions.

This is not a finding of independence: it cannot detect semantic translation,
and the manual review confirms the Quake recursive hull adaptation and broad
Valve SDK-informed gameplay work. Those facts require attribution and careful
compliance with the published terms. They are not evidence that Valve source
or assets are present here.

## Dependency licence review

All six Cargo workspaces are covered by source CI. Declared
third-party licences include GPL-2.0-or-later, MIT, Apache-2.0, BSD-3-Clause,
0BSD, MPL-2.0, Unicode-3.0, Unlicense, and Zlib. The two HL-PSX packages that
lacked a licence field (`game` and `host/hl-bsp`) now declare
`GPL-2.0-or-later`. `deny.toml` records the accepted licence families and the
single expected git source. Source CI checks advisories, licences, and sources
for the root builder, game, BSP cooker, content compiler, host logic runner,
and shared format crate.

## Release gates

Public source publication requires all of these checks:

1. Every closely adapted source routine carries an SPDX or provenance note.
2. `cargo deny check -A unmatched-source --config PATH/deny.toml advisories
   licenses sources` passes in every Cargo workspace.
3. `scripts/check-source-only.sh` confirms that no retail, converted, archive,
   audio, image, or generated disc payload is tracked.
4. The build succeeds from an unauthenticated source checkout with only Rust,
   internet access for declared dependencies, and a local Half-Life install.
5. The public tree is created from the reviewed export rules as a fresh,
   one-commit source repository. Private handoffs and release working notes do
   not enter that tree.
6. No generated asset pack, BIN/CUE image, physical disc, Sony system area,
   SDK, BIOS, or standalone Valve-derived media is offered.
7. The project remains noncommercial. No build, access, feature, support, or
   recognition is exchanged for payment.

Valve cannot authorize Sony trademarks or technology, and neither company can
authorize third-party content owned by someone else.
