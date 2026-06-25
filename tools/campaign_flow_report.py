#!/usr/bin/env python3
"""Report Half-Life campaign changelevel coverage for the cooked map registry."""

from __future__ import annotations

import argparse
import re
import struct
import sys
from pathlib import Path


def parse_maplist(makefile: Path) -> list[str]:
    maps: list[str] = []
    collecting = False
    for raw in makefile.read_text().splitlines():
        line = raw
        if line.startswith("MAPLIST :="):
            collecting = True
            line = line.split(":=", 1)[1]
        elif collecting and line and not line.startswith((" ", "\t")):
            break
        if collecting:
            maps.extend(line.replace("\\", "").split())
    return maps


def bsp_entities(path: Path) -> list[dict[str, str]]:
    data = path.read_bytes()
    if len(data) < 4 + 15 * 8:
        raise ValueError(f"{path} is too small to be a GoldSrc BSP")
    ent_ofs, ent_len = struct.unpack_from("<ii", data, 4)
    text = data[ent_ofs : ent_ofs + ent_len].decode("latin1", "ignore")
    out: list[dict[str, str]] = []
    for block in re.findall(r"\{([^{}]*)\}", text, re.S):
        out.append(dict(re.findall(r'"([^"]*)"\s*"([^"]*)"', block)))
    return out


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--hl-game", required=True, type=Path)
    parser.add_argument("--makefile", required=True, type=Path)
    parser.add_argument("--fail-on-blocked", action="store_true")
    args = parser.parse_args()

    maps = parse_maplist(args.makefile)
    if not maps:
        print(f"no MAPLIST found in {args.makefile}", file=sys.stderr)
        return 2

    cooked = set(maps)
    ready: list[tuple[str, str, str]] = []
    blocked: list[tuple[str, str, str]] = []
    edges = 0

    for src in maps:
        bsp = args.hl_game / "maps" / f"{src}.bsp"
        if not bsp.exists():
            print(f"missing BSP: {bsp}", file=sys.stderr)
            return 2
        for ent in bsp_entities(bsp):
            if ent.get("classname") != "trigger_changelevel":
                continue
            target = ent.get("map", "")
            landmark = ent.get("landmark", "")
            edges += 1
            if target in cooked:
                ready.append((src, target, landmark))
            else:
                blocked.append((src, target, landmark))

    print(f"cooked maps: {len(maps)}")
    print(f"changelevel edges: {edges}")
    print(f"ready edges: {len(ready)}")
    print(f"blocked edges: {len(blocked)}")
    if blocked:
        print()
        print("blocked uncooked targets:")
        for src, target, landmark in blocked:
            suffix = f" landmark={landmark}" if landmark else ""
            print(f"  {src:<7} -> {target:<7}{suffix}")

    if blocked and args.fail_on_blocked:
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
