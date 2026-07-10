#!/usr/bin/env python3
"""Verify resident SFX plus every map dialogue bank fit PS1 SPU RAM."""

from __future__ import annotations

import re
import struct
import sys
from dataclasses import dataclass
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
SPU_BYTES = 512 * 1024
SPU_SAMPLE_BASE = 0x1010
MAX_CORE_SFX = 47
MAX_MAP_VOICES = 40


@dataclass(frozen=True)
class Pack:
    path: Path
    rates: tuple[int, ...]
    adpcm_bytes: tuple[int, ...]

    @property
    def count(self) -> int:
        return len(self.adpcm_bytes)

    @property
    def resident_bytes(self) -> int:
        return sum(self.adpcm_bytes)


def u16(data: bytes, offset: int) -> int:
    return struct.unpack_from("<H", data, offset)[0]


def u32(data: bytes, offset: int) -> int:
    return struct.unpack_from("<I", data, offset)[0]


def parse_pack(path: Path) -> Pack:
    data = path.read_bytes()
    if len(data) < 8 or data[:4] != b"HSFX":
        raise ValueError(f"{path}: not an HSFX pack")
    count = u32(data, 4)
    if 8 + count * 8 > len(data):
        raise ValueError(f"{path}: truncated sample table")
    rates: list[int] = []
    adpcm_sizes: list[int] = []
    for index in range(count):
        offset = u32(data, 8 + index * 8)
        length = u32(data, 12 + index * 8)
        sample = data[offset : offset + length]
        if len(sample) != length or len(sample) < 32 or sample[:4] != b"PSAU":
            raise ValueError(f"{path}: invalid PSAU sample {index}")
        if u16(sample, 4) != 1 or u32(sample, 8) != len(sample) - 12:
            raise ValueError(f"{path}: invalid PSAU header {index}")
        block_count = u32(sample, 24)
        adpcm_size = block_count * 16
        if 32 + adpcm_size != len(sample):
            raise ValueError(f"{path}: invalid ADPCM extent {index}")
        rates.append(u32(sample, 16))
        adpcm_sizes.append(adpcm_size)
    return Pack(path, tuple(rates), tuple(adpcm_sizes))


def maplist() -> list[str]:
    text = (ROOT / "Makefile").read_text(encoding="utf-8")
    match = re.search(r"^MAPLIST := \\\n((?:\t.*\\?\n)+)", text, re.MULTILINE)
    if match is None:
        raise RuntimeError("MAPLIST not found")
    return re.sub(r"[\\\n\t]", " ", match.group(1)).split()


def voice_manifest() -> dict[int, list[int]]:
    by_map: dict[int, list[int]] = {}
    path = ROOT / "data/voices/manifest.txt"
    for line_number, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        fields = raw.split("|", 2)
        if len(fields) != 3:
            raise ValueError(f"{path}:{line_number}: malformed manifest row")
        map_index, local_id = int(fields[0]), int(fields[1])
        by_map.setdefault(map_index, []).append(local_id)
    return by_map


def main() -> int:
    failures: list[str] = []
    core_path = ROOT / "data/sfx/chunk_3000.psxa"
    if not core_path.is_file():
        print(f"missing core SFX pack: {core_path}", file=sys.stderr)
        return 2
    try:
        core = parse_pack(core_path)
    except ValueError as error:
        print(error, file=sys.stderr)
        return 2
    if core.count != MAX_CORE_SFX:
        failures.append(f"core sample count {core.count}!={MAX_CORE_SFX}")
    core_end = SPU_SAMPLE_BASE + core.resident_bytes
    if core_end > SPU_BYTES:
        failures.append(f"core bank exceeds SPU RAM by {core_end - SPU_BYTES} bytes")

    maps = maplist()
    try:
        manifest = voice_manifest()
    except (OSError, ValueError) as error:
        print(error, file=sys.stderr)
        return 2
    rows: list[tuple[str, int, int, int, int]] = []
    for index, name in enumerate(maps):
        path = ROOT / f"data/voices/chunk_{3100 + index}.psxa"
        if not path.is_file():
            if index in manifest:
                failures.append(f"{name}: manifest entries exist but voice pack is missing")
            continue
        try:
            pack = parse_pack(path)
        except ValueError as error:
            failures.append(str(error))
            continue
        end = core_end + pack.resident_bytes
        headroom = SPU_BYTES - end
        rows.append((name, pack.count, pack.resident_bytes, headroom, max(pack.rates)))
        manifest_ids = manifest.get(index, [])
        if manifest_ids != list(range(pack.count)):
            failures.append(
                f"{name}: manifest ids {manifest_ids} do not match pack 0..{pack.count - 1}"
            )
        if pack.count > MAX_MAP_VOICES:
            failures.append(f"{name}: voices {pack.count}>{MAX_MAP_VOICES}")
        if headroom < 0:
            remaining = SPU_BYTES - core_end
            loaded = 0
            used = 0
            for sample_bytes in pack.adpcm_bytes[:MAX_MAP_VOICES]:
                if used + sample_bytes > remaining:
                    break
                used += sample_bytes
                loaded += 1
            failures.append(
                f"{name}: SPU overflow {-headroom} bytes; runtime keeps "
                f"{loaded}/{pack.count} lines"
            )

    for index in sorted(set(manifest) - set(range(len(maps)))):
        failures.append(f"manifest references out-of-range map index {index}")

    worst = min(rows, key=lambda row: row[3], default=None)
    print(
        f"core: {core.count}/{MAX_CORE_SFX} samples, "
        f"resident={core.resident_bytes} bytes, end=0x{core_end:05x}"
    )
    print(f"dialogue maps audited: {len(rows)}/{len(maps)}")
    if worst is not None:
        name, count, resident, headroom, rate = worst
        print(
            f"tightest map: {name} voices={count}/{MAX_MAP_VOICES} "
            f"resident={resident} bytes max_rate={rate} Hz "
            f"SPU headroom={headroom} bytes"
        )
    print(f"SPU failures: {len(failures)}")
    for failure in failures:
        print(f"  {failure}")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
