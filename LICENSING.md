# HL-PSX licensing and distribution policy

HL-PSX is a noncommercial, source-only compatibility project. This document
describes the project's release policy and is not legal advice.

## Project code

Copyright (C) 2025-2026 Emanuele Bonura and contributors.

Code that its copyright holders are entitled to license is offered under the
GNU General Public License version 2 or, at your option, any later version
(`GPL-2.0-or-later`). The complete GPLv2 text is in [LICENSE](LICENSE).

That licence does not grant rights in Half-Life, Valve assets, PlayStation,
Sony technology, or any other third-party material. File-level and dependency
attribution is recorded in [PROVENANCE.md](PROVENANCE.md) and
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

## Half-Life and the Valve SDK

The Rust compatibility code was informed by Valve's public Half-Life 1 SDK.
Valve's SDK licence permits free use and distribution within its stated scope
and instructs commercial users to contact Valve. The Steam Subscriber
Agreement separately describes noncommercial use of Valve Developer Tools and
fan art. HL-PSX is deliberately noncommercial.

Primary terms:

- <https://github.com/ValveSoftware/halflife/blob/master/LICENSE>
- <https://store.steampowered.com/subscriber_agreement/>

No Valve source file is copied into this repository. The provenance record
describes source-informed adaptations and the limits of the project's audit.

## Bring your own assets

The repository does not distribute original or converted Valve maps, models,
textures, sprites, fonts, audio, music, UI artwork, or screenshots. The local
builder reads a lawfully obtained Half-Life installation and writes converted
content under ignored local directories.

Public project releases contain source only. Generated asset packs, BIN/CUE
images, physical discs, and standalone Valve-derived media are not project
distribution artifacts and must not be uploaded as releases.

## Trademarks and affiliation

Half-Life is a trademark of Valve Corporation. PlayStation is a trademark or
registered trademark of Sony Interactive Entertainment Inc. HL-PSX is an
unofficial project and is not affiliated with, sponsored by, or endorsed by
Valve or Sony.
