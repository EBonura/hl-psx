#!/usr/bin/env python3
"""Verify independent cooked actor/sprite placement capacities campaign-wide.

The sprite source of truth is the original BSP entity lump intersected with the
per-map sprite manifest. Every eligible env_sprite/env_glow/cycler_sprite must
have one compact SpriteRec; unnamed sprites count too (GoldSrc starts them on).
"""

from __future__ import annotations

import os
import re
import struct
import sys
from collections import defaultdict
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
ROOMS = ROOT / "data/rooms"
SPRITE_MANIFEST = ROOT / "data/sprites/manifest.txt"
# Runtime reserves the final 15 PROP_NEAR_ENTS rows as the zero-BSS
# transition mailbox; live authored+carried actors occupy indices 0..112.
ACTOR_CAP = 113
SPRITE_CAP = 160
SPLIT_MARKER = 0x8000_0000
SPRITE_CLASSES = {"env_sprite", "env_glow", "cycler_sprite"}


def u32(data: bytes, offset: int) -> int:
    return struct.unpack_from("<I", data, offset)[0]


def maplist() -> list[str]:
    text = (ROOT / "Makefile").read_text(encoding="utf-8")
    match = re.search(r"^MAPLIST := \\\n((?:\t.*\\?\n)+)", text, re.MULTILINE)
    if match is None:
        raise RuntimeError("MAPLIST not found")
    return re.sub(r"[\\\n\t]", " ", match.group(1)).split()


def sprite_names_by_map() -> dict[int, set[str]]:
    names: dict[int, set[str]] = defaultdict(set)
    for line in SPRITE_MANIFEST.read_text(encoding="utf-8").splitlines():
        fields = line.strip().split("|")
        if len(fields) >= 3:
            names[int(fields[0])].add(fields[2].lower())
    return names


def entity_blocks(bsp: Path) -> list[dict[str, str]]:
    data = bsp.read_bytes()
    entity_offset, entity_len = struct.unpack_from("<ii", data, 4)
    text = data[entity_offset : entity_offset + entity_len].decode(
        "latin1", errors="replace"
    )
    return [
        dict(re.findall(r'"([^"]+)"\s+"([^"]*)"', block))
        for block in re.findall(r"\{(.*?)\}", text, re.DOTALL)
    ]


def eligible_sprite_count(bsp: Path, names: set[str]) -> int:
    count = 0
    for entity in entity_blocks(bsp):
        if entity.get("classname") not in SPRITE_CLASSES:
            continue
        model = entity.get("model", "").replace("\\", "/").rsplit("/", 1)[-1].lower()
        if model in names and "origin" in entity:
            count += 1
    return count


def main() -> int:
    hl_game = Path(
        os.environ.get(
            "HL_GAME",
            Path.home()
            / "Library/Application Support/Steam/steamapps/common/Half-Life/valve",
        )
    )
    maps = maplist()
    manifest = sprite_names_by_map()
    failures: list[str] = []
    rows: list[tuple[str, int, int]] = []

    for index, name in enumerate(maps):
        room = ROOMS / f"room_{index * 2}.psxc"
        bsp = hl_game / "maps" / f"{name}.bsp"
        if not room.is_file() or not bsp.is_file():
            failures.append(f"{name}: missing room or source BSP")
            continue
        data = room.read_bytes()
        prop_offset = u32(data, 36)
        counts = u32(data, prop_offset)
        if counts & SPLIT_MARKER == 0:
            failures.append(f"{name}: legacy shared prop table")
            continue
        actors = counts & 0xFFFF
        sprites = (counts >> 16) & 0x7FFF
        source_sprites = eligible_sprite_count(bsp, manifest.get(index, set()))
        rows.append((name, actors, sprites))
        if actors > ACTOR_CAP:
            failures.append(f"{name}: actors {actors}>{ACTOR_CAP}")
        if sprites > SPRITE_CAP:
            failures.append(f"{name}: sprites {sprites}>{SPRITE_CAP}")
        if sprites != source_sprites:
            failures.append(
                f"{name}: cooked sprites {sprites} != eligible source {source_sprites}"
            )

    if rows:
        actor_peak = max(rows, key=lambda row: row[1])
        sprite_peak = max(rows, key=lambda row: row[2])
        print(
            f"maps audited: {len(rows)}; placement failures: "
            f"{len(failures)} {failures}"
        )
        print(
            f"actors: total={sum(row[1] for row in rows)} "
            f"peak={actor_peak[0]} {actor_peak[1]}/{ACTOR_CAP}"
        )
        print(
            f"sprites: total={sum(row[2] for row in rows)} "
            f"peak={sprite_peak[0]} {sprite_peak[2]}/{SPRITE_CAP}"
        )
        split_required = [
            f"{name}={actors}+{sprites}"
            for name, actors, sprites in rows
            if actors + sprites > ACTOR_CAP
        ]
        print(
            f"maps requiring independent capacities: {', '.join(split_required) or 'none'}"
        )
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
