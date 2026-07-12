#!/usr/bin/env python3
"""Create a deterministic neutral-input PSoXide tape for direct map boot."""

from __future__ import annotations

import argparse
import struct
from pathlib import Path

MAGIC = b"PXITAPE1"
SAMPLE = struct.Struct("<HBBBB")
L1 = 0x0400


def build_tape(frame_count: int, boot_frames: int, map_index: int) -> bytes:
    if frame_count <= 0:
        raise ValueError("frame_count must be positive")
    if not 0 <= boot_frames <= frame_count:
        raise ValueError("boot_frames must be within the tape")
    if not 0 <= map_index <= 0xFF:
        raise ValueError("map_index must fit the direct-boot low byte")

    data = bytearray(MAGIC)
    data += struct.pack("<I", frame_count)
    boot_buttons = L1 | map_index
    for frame in range(frame_count):
        buttons = boot_buttons if frame < boot_frames else 0
        data += SAMPLE.pack(buttons, 0x80, 0x80, 0x80, 0x80)
    return bytes(data)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("output", type=Path)
    parser.add_argument("--frames", type=int, default=30_000)
    parser.add_argument("--boot-frames", type=int, default=240)
    parser.add_argument("--map-index", type=int, default=0)
    args = parser.parse_args()

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_bytes(build_tape(args.frames, args.boot_frames, args.map_index))
    print(f"wrote {args.frames} deterministic samples to {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
