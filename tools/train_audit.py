#!/usr/bin/env python3
"""Audit cooked func_train records against the runtime's fixed train pool.

The source of truth is the resident HLM room data.  This intentionally mirrors
``init_trains``: a record consumes a slot only when it is a func_train, has at
least one complete corner (two aux records), and references a cooked brush.
"""

from __future__ import annotations

import argparse
import re
import struct
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable


ROOT = Path(__file__).resolve().parent.parent
HLM_MAGICS = (b"HLMA", b"HLMB")
HLM_HEADER_SIZE = 52
ENT_SIZE = 56
LOGIC_SIZE = 64
LOGIC_AUX_SIZE = 4
LOGIC_FUNC_TRAIN = 25


class FormatError(ValueError):
    """A cooked room cannot be decoded safely using the runtime layout."""


@dataclass(frozen=True)
class RoomStats:
    name: str
    total: int
    valid: int
    invalid: int
    zero_speed: tuple[int, ...]
    repeated_cycles: tuple[str, ...]


def _require(data: bytes, offset: int, size: int, what: str) -> None:
    if offset < 0 or size < 0 or offset > len(data) or size > len(data) - offset:
        raise FormatError(
            f"{what} is out of bounds: offset={offset} size={size} file={len(data)}"
        )


def _u16(data: bytes, offset: int, what: str) -> int:
    _require(data, offset, 2, what)
    return struct.unpack_from("<H", data, offset)[0]


def _u32(data: bytes, offset: int, what: str) -> int:
    _require(data, offset, 4, what)
    return struct.unpack_from("<I", data, offset)[0]


def parse_maplist(path: Path) -> list[str]:
    text = path.read_text(encoding="utf-8")
    match = re.search(r"^MAPLIST := \\\n((?:\t.*\\?\n)+)", text, re.MULTILINE)
    if match is None:
        raise FormatError(f"MAPLIST not found in {path}")
    maps = re.sub(r"[\\\n\t]", " ", match.group(1)).split()
    if not maps:
        raise FormatError(f"MAPLIST is empty in {path}")
    return maps


def parse_max_trains(path: Path) -> int:
    text = path.read_text(encoding="utf-8")
    matches = re.findall(
        r"^\s*const\s+MAX_TRAINS\s*:\s*usize\s*=\s*([0-9][0-9_]*)\s*;",
        text,
        re.MULTILINE,
    )
    if len(matches) != 1:
        raise FormatError(
            f"expected one MAX_TRAINS usize constant in {path}, found {len(matches)}"
        )
    value = int(matches[0].replace("_", ""))
    if value <= 0:
        raise FormatError(f"MAX_TRAINS must be positive in {path}")
    return value


def _entity_count(data: bytes) -> int:
    ent_off = _u32(data, 28, "HLM entity offset")
    if ent_off < HLM_HEADER_SIZE:
        raise FormatError(f"HLM entity offset points into the header: {ent_off}")
    n_models = _u32(data, ent_off, "entity model count")
    n_ents_off = ent_off + 4 + n_models * 8
    n_ents = _u32(data, n_ents_off, "entity count")
    ent_leaf_count_off = n_ents_off + 4 + n_ents * ENT_SIZE
    n_ent_leafs = _u32(data, ent_leaf_count_off, "entity leaf count")
    _require(
        data,
        ent_leaf_count_off + 4,
        n_ent_leafs * 2,
        "entity leaf index table",
    )
    return n_ents


def _first_repeated_cycle(corners: list[tuple[int, int, int, int]]) -> tuple[int, int] | None:
    """Return a conservative candidate for a repeated path subsequence.

    Names are absent from HLM, so equal coordinates cannot prove node identity.
    Requiring three adjacent copies avoids flagging a single authored revisit;
    callers must still treat the result as advisory.
    """

    for start in range(len(corners)):
        remaining = len(corners) - start
        for period in range(1, remaining // 3 + 1):
            block = corners[start : start + period]
            if (
                block == corners[start + period : start + period * 2]
                and block == corners[start + period * 2 : start + period * 3]
            ):
                return start, period
    return None


def parse_room(data: bytes, name: str = "<room>") -> RoomStats:
    _require(data, 0, HLM_HEADER_SIZE, "HLM header")
    if data[:4] not in HLM_MAGICS:
        raise FormatError(
            f"{name}: bad magic {data[:4]!r}, expected one of {HLM_MAGICS!r}"
        )

    n_ents = _entity_count(data)
    logic_off = _u32(data, 48, "HLM logic offset")
    if logic_off < HLM_HEADER_SIZE:
        raise FormatError(f"{name}: HLM logic offset points into the header: {logic_off}")
    _require(data, logic_off, 8, "logic header")
    n_logic, n_aux, n_names, name_bytes = struct.unpack_from("<HHHH", data, logic_off)
    records_off = logic_off + 8
    aux_off = records_off + n_logic * LOGIC_SIZE
    name_offsets_off = aux_off + n_aux * LOGIC_AUX_SIZE
    names_off = name_offsets_off + n_names * 2
    _require(data, records_off, n_logic * LOGIC_SIZE, "logic record table")
    _require(data, aux_off, n_aux * LOGIC_AUX_SIZE, "logic aux table")
    _require(data, name_offsets_off, n_names * 2, "logic name offset table")
    _require(data, names_off, name_bytes, "logic name blob")

    total = 0
    valid = 0
    zero_speed: list[int] = []
    repeated_cycles: list[str] = []
    for index in range(n_logic):
        offset = records_off + index * LOGIC_SIZE
        first_aux = _u16(data, offset + 12, f"logic[{index}] first_aux")
        aux_count = data[offset + 14]
        if aux_count and first_aux + aux_count > n_aux:
            raise FormatError(
                f"{name}: logic[{index}] aux range {first_aux}+{aux_count} "
                f"exceeds {n_aux}"
            )
        if data[offset] != LOGIC_FUNC_TRAIN:
            continue

        total += 1
        brush = _u16(data, offset + 10, f"logic[{index}] brush")
        is_valid = aux_count >= 2 and brush < n_ents
        if not is_valid:
            continue
        valid += 1
        speed = _u16(data, offset + 20, f"logic[{index}] speed")
        if speed == 0:
            zero_speed.append(index)

        # Each corner is two aux records: (x,y), then (z,wait).  Detect only
        # repeated subsequences, a possible signature of a serialized cycle
        # reaching the cooker's hop limit. HLM drops path-corner names, so this
        # remains advisory: distinct authored nodes may share a position/wait.
        corners: list[tuple[int, int, int, int]] = []
        for corner in range(aux_count // 2):
            first = aux_off + (first_aux + corner * 2) * LOGIC_AUX_SIZE
            second = first + LOGIC_AUX_SIZE
            x, y = struct.unpack_from("<hh", data, first)
            z = struct.unpack_from("<h", data, second)[0]
            wait = _u16(data, second + 2, f"logic[{index}] corner[{corner}] wait")
            corners.append((x, y, z, wait))
        repeated = _first_repeated_cycle(corners)
        if repeated is not None:
            start, period = repeated
            repeated_cycles.append(
                f"logic[{index}] has a possible repeated {period}-corner "
                f"subsequence at corner {start}"
            )

    return RoomStats(
        name=name,
        total=total,
        valid=valid,
        invalid=total - valid,
        zero_speed=tuple(zero_speed),
        repeated_cycles=tuple(repeated_cycles),
    )


def evaluate_rooms(rows: Iterable[RoomStats], max_trains: int) -> list[str]:
    failures: list[str] = []
    for row in rows:
        if row.valid > max_trains:
            failures.append(f"{row.name}: valid trains {row.valid}>{max_trains}")
        if row.zero_speed:
            failures.append(
                f"{row.name}: valid zero-speed trains at logic "
                + ",".join(str(i) for i in row.zero_speed)
            )
    return failures


def audit_campaign(
    maps: list[str], rooms: Path, max_trains: int
) -> tuple[list[RoomStats], list[str]]:
    rows: list[RoomStats] = []
    failures: list[str] = []
    for index, name in enumerate(maps):
        path = rooms / f"room_{index * 2}.psxc"
        try:
            rows.append(parse_room(path.read_bytes(), name))
        except (OSError, FormatError) as exc:
            failures.append(f"{name}: {exc}")
    failures.extend(evaluate_rooms(rows, max_trains))
    return rows, failures


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--makefile", type=Path, default=ROOT / "Makefile")
    parser.add_argument("--runtime", type=Path, default=ROOT / "game/src/main.rs")
    parser.add_argument("--rooms", type=Path, default=ROOT / "data/rooms")
    args = parser.parse_args(argv)

    try:
        maps = parse_maplist(args.makefile)
        max_trains = parse_max_trains(args.runtime)
        rows, failures = audit_campaign(maps, args.rooms, max_trains)
    except (OSError, FormatError) as exc:
        print(f"train audit: {exc}", file=sys.stderr)
        return 1

    total = sum(row.total for row in rows)
    valid = sum(row.valid for row in rows)
    invalid = sum(row.invalid for row in rows)
    advisories = [
        f"{row.name}: {issue}"
        for row in rows
        for issue in row.repeated_cycles
    ]
    peak = max(rows, key=lambda row: row.valid) if rows else None
    print(
        f"maps audited: {len(rows)}/{len(maps)}; train failures: "
        f"{len(failures)}"
    )
    print(f"func_train: total={total} valid={valid} invalid={invalid}")
    if peak is not None:
        print(f"peak: {peak.name} {peak.valid}/{max_trains} valid trains")
    print(f"possible repeated-path advisories: {len(advisories)}")
    for advisory in advisories:
        print(f"ADVISORY: {advisory}")
    for failure in failures:
        print(f"ERROR: {failure}", file=sys.stderr)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
