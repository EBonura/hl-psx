#!/usr/bin/env python3
"""Cook and size every single-player Half-Life campaign BSP.

This is a planning/guardrail tool for the PSX map streaming budget. It reports
which campaign maps fit the current generated MAP_BUF size and which need the
next storage step: chunking, compression, or a smaller cooked representation.
"""

from __future__ import annotations

import argparse
import subprocess
import sys
import tempfile
from pathlib import Path


FALLBACK_MAP_WORDS = 255_000


def room_budget_bytes(rooms_dir: Path) -> int:
    max_bytes = 0
    if rooms_dir.is_dir():
        for path in rooms_dir.glob("room_*.psxc"):
            max_bytes = max(max_bytes, path.stat().st_size)
    if max_bytes == 0:
        return FALLBACK_MAP_WORDS * 4
    return ((max_bytes + 3) // 4) * 4


def campaign_maps(maps_dir: Path) -> list[Path]:
    return sorted(
        p
        for p in maps_dir.glob("c*.bsp")
        if len(p.stem) > 1 and p.stem[1].isdigit()
    )


def cook_size(hlbsp: Path, bsp: Path, tmp: Path) -> tuple[int, int]:
    world = tmp / f"{bsp.stem}.world.psxc"
    tex = tmp / f"{bsp.stem}.tex.psxc"
    subprocess.run(
        [str(hlbsp), "--cook", str(bsp), str(world), str(tex)],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        text=True,
        check=True,
    )
    return world.stat().st_size, tex.stat().st_size


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--hl-game", required=True, type=Path)
    parser.add_argument("--hlbsp-bin", required=True, type=Path)
    parser.add_argument("--rooms-dir", required=True, type=Path)
    parser.add_argument("--top", type=int, default=20)
    parser.add_argument("--fail-on-oversize", action="store_true")
    args = parser.parse_args()

    maps_dir = args.hl_game / "maps"
    maps = campaign_maps(maps_dir)
    if not maps:
        print(f"no campaign BSPs found in {maps_dir}", file=sys.stderr)
        return 2

    budget = room_budget_bytes(args.rooms_dir)
    rows: list[tuple[str, int, int, str]] = []
    with tempfile.TemporaryDirectory(prefix="hl-psx-map-sizes-") as td:
        tmp = Path(td)
        for bsp in maps:
            try:
                world, tex = cook_size(args.hlbsp_bin, bsp, tmp)
                fit = "ok" if world <= budget and tex <= budget else "too_big"
                rows.append((bsp.stem, world, tex, fit))
            except subprocess.CalledProcessError as err:
                print(f"{bsp.stem}: cook failed: {err.stderr.strip()}", file=sys.stderr)
                rows.append((bsp.stem, 0, 0, "fail"))

    oversized = [r for r in rows if r[3] == "too_big"]
    failures = [r for r in rows if r[3] == "fail"]
    rows_by_world = sorted(rows, key=lambda r: r[1], reverse=True)
    largest_fit = max((r for r in rows if r[3] == "ok"), key=lambda r: r[1], default=None)

    print(f"MAP_BUF budget: {budget} bytes ({budget / 1024:.1f} KiB)")
    print(f"campaign maps: {len(rows)}  ok: {len(rows) - len(oversized) - len(failures)}  oversized: {len(oversized)}  failed: {len(failures)}")
    if largest_fit:
        print(f"largest fitting map: {largest_fit[0]} world={largest_fit[1]} bytes")
    print()
    print("largest cooked world chunks:")
    for name, world, tex, fit in rows_by_world[: args.top]:
        print(f"  {name:<7} world={world:7d}  tex={tex:7d}  {fit}")
    if oversized:
        print()
        print("oversized campaign maps:")
        for name, world, tex, _fit in oversized:
            delta = (max(world, tex) - budget) / 1024.0
            print(f"  {name:<7} world={world:7d}  tex={tex:7d}  over={delta:6.1f} KiB")

    if failures:
        return 2
    if oversized and args.fail_on_oversize:
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
