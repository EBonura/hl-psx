#!/usr/bin/env python3
"""Focused tests for GoldSrc studio target-event extraction."""

from __future__ import annotations

import struct
import tempfile
import unittest
from pathlib import Path

from gen_studio_events import EVENT_BYTES, SEQDESC_BYTES, extract_events, generate


def _mdl(*, event: int = 1003, target: str = "intro") -> bytes:
    seq_offset = 244
    event_offset = seq_offset + SEQDESC_BYTES
    data = bytearray(event_offset + EVENT_BYTES)
    data[:4] = b"IDST"
    struct.pack_into("<ii", data, 164, 1, seq_offset)
    data[seq_offset : seq_offset + 8] = b"deskidle"
    struct.pack_into("<f", data, seq_offset + 32, 20.0)
    struct.pack_into("<iii", data, seq_offset + 48, 1, event_offset, 181)
    struct.pack_into("<iii", data, event_offset, 20, event, 0)
    encoded = target.encode("latin1")[:63]
    data[event_offset + 12 : event_offset + 12 + len(encoded)] = encoded
    return bytes(data)


class StudioEventTests(unittest.TestCase):
    def test_extracts_fire_target_at_20hz_tick(self) -> None:
        self.assertEqual(extract_events(_mdl(), 0), [(20, 180, "intro")])

    def test_ignores_non_target_studio_events(self) -> None:
        self.assertEqual(extract_events(_mdl(event=1004), 0), [])

    def test_roster_maps_named_clip_to_stable_slot_and_skips_alias(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            models = Path(td)
            (models / "scientist.mdl").write_bytes(_mdl())
            lines = generate(
                models,
                "0|scientist|0:1,deskidle:2,desk=deskidle\n",
            )
        self.assertEqual(lines, ["0|deskidle|20|180|intro"])


if __name__ == "__main__":
    unittest.main()
