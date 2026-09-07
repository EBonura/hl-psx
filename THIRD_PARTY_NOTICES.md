# Third-party notices

## Valve / Half-Life

Half-Life, its game assets, and the Half-Life 1 SDK are owned by Valve
Corporation and/or its licensors. HL-PSX uses Half-Life names and compatibility
metadata and contains Rust implementations informed by Valve's public SDK. It
does not track or distribute original or converted Valve binary game assets or
Valve source files. The project is distributed as source only, requires a
lawfully obtained local Half-Life installation, and is noncommercial.

Official SDK and licence: <https://github.com/ValveSoftware/halflife>

Steam Subscriber Agreement: <https://store.steampowered.com/subscriber_agreement/>

HL-PSX is an unofficial community project and is not affiliated with or
endorsed by Valve.

## Quake

The recursive BSP hull trace and the multi-plane slide move in
`game/src/phys.rs` adapt `SV_RecursiveHullCheck` and `SV_FlyMove` from the
GPL-released Quake source:

> Quake source code — Copyright (C) 1996-1997 Id Software, Inc.

Source: <https://github.com/id-Software/Quake>

Licence: GNU General Public License version 2 or later. The complete GPLv2 text
is included in `LICENSE`.

## PSoXide

HL-PSX links to and invokes PSoXide crates and tools pinned at commit
`8df242b353b8a3664c1d2ed20622d692d1349306`:

<https://github.com/EBonura/PSoXide>

PSoXide is GPL-2.0-or-later and has its own downstream notices, including its
documented PCSX-Redux-derived emulator work. PSoXide is fetched as a dependency;
its source is not vendored in this repository.

## Xash3D FWGS

Xash3D FWGS was consulted as a behavioural oracle. No Xash dependency or copied
source is known to be included in HL-PSX. Reference:
<https://github.com/FWGS/xash3d-fwgs>.

## Sony / PlayStation

PlayStation is a trademark or registered trademark of Sony Interactive
Entertainment Inc. HL-PSX and PSoXide are independent homebrew projects and are
not affiliated with, licensed by, or endorsed by Sony. HL-PSX does not include
Sony SDK components, BIOS firmware, or licensed system-area data. The default
disc-building command creates an image without a Sony system area.

## Project branding

The embedded Bonnie Studios logo in `host/hl-content/src/bonnie_logo.rs` is
project branding supplied by the HL-PSX author; it is not a Valve or Sony asset.
