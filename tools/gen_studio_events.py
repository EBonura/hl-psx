#!/usr/bin/env python3
"""Extract GoldSrc studio target events for the actor clips we actually cook.

The runtime only needs event 1003 (``SCRIPT_EVENT_FIREEVENT``): its option is
the target name fired when the source sequence crosses the authored frame.
The generated manifest is host-only metadata consumed by the BSP cooker:

    actor_type|script_clip_name|event_tick_20hz|period_ticks_20hz|target

Keeping these records in the room's existing LogicAux stream costs no static
PS1 RAM and avoids shipping strings or a general studio event table per model.
"""

from __future__ import annotations

import argparse
import math
import struct
from pathlib import Path


SEQDESC_BYTES = 176
EVENT_BYTES = 76
SCRIPT_EVENT_FIRE_TARGET = 1003


def _i32(data: bytes, offset: int) -> int:
    if offset < 0 or offset + 4 > len(data):
        raise ValueError(f"i32 at {offset} exceeds {len(data)}-byte MDL")
    return struct.unpack_from("<i", data, offset)[0]


def _f32(data: bytes, offset: int) -> float:
    if offset < 0 or offset + 4 > len(data):
        raise ValueError(f"f32 at {offset} exceeds {len(data)}-byte MDL")
    return struct.unpack_from("<f", data, offset)[0]


def _cstr(data: bytes) -> str:
    return data.split(b"\0", 1)[0].decode("latin1", errors="replace")


def _sequences(data: bytes) -> tuple[dict[str, int], int, int]:
    if len(data) < 212 or data[:4] != b"IDST":
        raise ValueError("not a GoldSrc studio MDL")
    count = _i32(data, 164)
    offset = _i32(data, 168)
    if count < 0 or offset < 0 or offset + count * SEQDESC_BYTES > len(data):
        raise ValueError("studio sequence table exceeds MDL")
    labels: dict[str, int] = {}
    for index in range(count):
        at = offset + index * SEQDESC_BYTES
        labels.setdefault(_cstr(data[at : at + 32]).lower(), index)
    return labels, count, offset


def extract_events(data: bytes, sequence: int) -> list[tuple[int, int, str]]:
    _labels, count, seq_offset = _sequences(data)
    if sequence < 0 or sequence >= count:
        return []
    desc = seq_offset + sequence * SEQDESC_BYTES
    fps = _f32(data, desc + 32)
    event_count = _i32(data, desc + 48)
    event_offset = _i32(data, desc + 52)
    frame_count = _i32(data, desc + 56)
    if not math.isfinite(fps) or fps <= 0.0 or event_count <= 0:
        return []
    if event_offset < 0 or event_offset + event_count * EVENT_BYTES > len(data):
        raise ValueError("studio event table exceeds MDL")

    period = max(1, math.ceil(max(frame_count - 1, 0) * 20.0 / fps))
    period = min(period, 0xFFFF)
    result: list[tuple[int, int, str]] = []
    for index in range(event_count):
        at = event_offset + index * EVENT_BYTES
        frame, event = struct.unpack_from("<ii", data, at)
        target = _cstr(data[at + 12 : at + EVENT_BYTES]).strip()
        if event != SCRIPT_EVENT_FIRE_TARGET or not target or "|" in target:
            continue
        tick = max(1, math.ceil(max(frame, 0) * 20.0 / fps))
        result.append((min(tick, 0xFFFF), period, target))
    return result


def generate(models_dir: Path, roster_text: str) -> list[str]:
    output: list[str] = []
    seen: set[tuple[int, str, int, int, str]] = set()
    for raw in roster_text.splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        fields = line.split("|", 2)
        if len(fields) != 3:
            raise ValueError(f"malformed roster line: {raw!r}")
        actor_type, model, specs = int(fields[0]), fields[1], fields[2]
        data = (models_dir / f"{model}.mdl").read_bytes()
        labels, sequence_count, _offset = _sequences(data)
        for token in specs.split(","):
            token = token.strip()
            if not token:
                continue
            label = token.split("=", 1)[0].strip() if "=" in token else token.split(":", 1)[0].strip()
            if label.lstrip("-").isdigit():
                sequence = int(label)
                script_name = ""
            else:
                sequence = labels.get(label.lower(), -1)
                script_name = label.lower()
            if 0 <= sequence < sequence_count:
                for tick, period, target in extract_events(data, sequence):
                    if not script_name:
                        continue
                    record = (actor_type, script_name, tick, period, target)
                    if record not in seen:
                        seen.add(record)
                        output.append("|".join(map(str, record)))
    return output


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("models_dir", type=Path)
    parser.add_argument("roster", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    lines = generate(args.models_dir, args.roster.read_text(encoding="utf-8"))
    args.output.write_text("\n".join(lines) + ("\n" if lines else ""), encoding="utf-8")
    print(f"studio target events: {len(lines)} -> {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
