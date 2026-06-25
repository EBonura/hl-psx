#!/usr/bin/env python3
"""Summarise a PSoXide/LLD linker map for hl-psx RAM budgeting."""

from __future__ import annotations

import argparse
import re
from pathlib import Path

LOAD_ADDR = 0x80010000
STATIC_LIMIT = 0x801F8000


def parse_hex(value: str) -> int:
    return int(value, 16)


def kib(value: int) -> str:
    return f"{value / 1024:.1f} KiB"


def clean_symbol(symbol: str) -> str:
    symbol = symbol.strip()
    symbol = symbol.replace("\t", " ")
    return re.sub(r"\s+", " ", symbol)


def parse_map(path: Path) -> tuple[dict[str, int], dict[str, int], list[tuple[int, str, str]]]:
    lines = path.read_text(errors="replace").splitlines()
    symbols: dict[str, int] = {}
    sections: dict[str, int] = {}
    entries: list[tuple[int, str, str]] = []

    for i, line in enumerate(lines):
        parts = line.split()
        if len(parts) >= 5 and parts[4].startswith("."):
            try:
                size = parse_hex(parts[2])
            except ValueError:
                continue
            name = parts[4]
            if name in {".text", ".data", ".rodata", ".bss"}:
                sections[name] = sections.get(name, 0) + size

        match = re.match(r"\s*([0-9a-fA-F]+)\s+[0-9a-fA-F]+\s+0\s+\d+\s+(__[A-Za-z0-9_]+)\s*=", line)
        if match:
            symbols[match.group(2)] = parse_hex(match.group(1))

        if ":(" not in line:
            continue
        parts = line.split()
        if len(parts) < 5:
            continue
        try:
            addr = parse_hex(parts[0])
        except ValueError:
            continue
        if not (LOAD_ADDR <= addr < STATIC_LIMIT):
            continue
        section_match = re.search(r":\((\.[^)]+)\)", line)
        if not section_match:
            continue
        mem_section = section_match.group(1)
        if not mem_section.startswith((".text", ".data", ".rodata", ".bss")):
            continue
        try:
            size = parse_hex(parts[2])
        except ValueError:
            continue
        if size < 4096:
            continue

        symbol = ""
        if i + 1 < len(lines):
            next_parts = lines[i + 1].split()
            if len(next_parts) >= 5:
                symbol = clean_symbol(" ".join(next_parts[4:]))
        if not symbol:
            symbol = clean_symbol(mem_section)
        entries.append((size, mem_section, symbol))

    return symbols, sections, entries


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("map", type=Path)
    parser.add_argument("--min-headroom-kb", type=int, default=0)
    parser.add_argument("--top", type=int, default=24)
    args = parser.parse_args()

    symbols, sections, entries = parse_map(args.map)
    text_start = symbols.get("__text_start", LOAD_ADDR)
    bss_end = symbols.get("__bss_end")
    if bss_end is None:
        raise SystemExit(f"{args.map}: could not find __bss_end")

    used = bss_end - LOAD_ADDR
    headroom = STATIC_LIMIT - bss_end

    print(f"map: {args.map}")
    print(f"static RAM used: {used} bytes ({kib(used)})")
    print(f"static headroom: {headroom} bytes ({kib(headroom)})")
    print()
    for name in (".text", ".data", ".rodata", ".bss"):
        value = sections.get(name, 0)
        if value:
            print(f"{name:8} {value:8d} {kib(value):>10}")
    if text_start != LOAD_ADDR:
        print(f"note: __text_start is 0x{text_start:08x}, expected 0x{LOAD_ADDR:08x}")

    print()
    print("top RAM entries:")
    for size, section, symbol in sorted(entries, reverse=True)[: args.top]:
        print(f"{size:8d} {kib(size):>10}  {symbol}  {section}")

    minimum = args.min_headroom_kb * 1024
    if minimum and headroom < minimum:
        print()
        print(f"ERROR: headroom {kib(headroom)} is below required {args.min_headroom_kb} KiB")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
