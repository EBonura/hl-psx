#!/usr/bin/env python3
"""Focused tests for the cooked func_train capacity audit."""

from __future__ import annotations

import struct
import sys
import tempfile
import unittest
from pathlib import Path


sys.path.insert(0, str(Path(__file__).resolve().parent))
import train_audit as audit  # noqa: E402


def _room(records: list[tuple[int, int, int, int]], n_ents: int = 2) -> bytes:
    """Build an HLMA shell; records are (kind, aux_count, brush, speed)."""

    data = bytearray(audit.HLMA_HEADER_SIZE)
    data[:4] = b"HLMA"

    ent_off = len(data)
    struct.pack_into("<I", data, 28, ent_off)
    data.extend(struct.pack("<II", 0, n_ents))
    data.extend(bytes(n_ents * audit.ENT_SIZE))
    data.extend(struct.pack("<I", 0))

    logic_off = len(data)
    struct.pack_into("<I", data, 48, logic_off)
    n_aux = sum(aux_count for _, aux_count, _, _ in records)
    data.extend(struct.pack("<HHHH", len(records), n_aux, 0, 0))
    first_aux = 0
    for kind, aux_count, brush, speed in records:
        rec = bytearray(audit.LOGIC_SIZE)
        rec[0] = kind
        struct.pack_into("<H", rec, 10, brush)
        struct.pack_into("<H", rec, 12, first_aux)
        rec[14] = aux_count
        struct.pack_into("<H", rec, 20, speed)
        data.extend(rec)
        first_aux += aux_count
    for i in range(n_aux):
        data.extend(struct.pack("<HH", i + 1, i + 2))
    return bytes(data)


class TrainAuditTests(unittest.TestCase):
    def test_runtime_validity_and_capacity_match_init_trains(self) -> None:
        room = _room(
            [
                (25, 2, 0, 100),
                (25, 2, 1, 100),
                (25, 2, 0, 100),
                (25, 1, 0, 100),  # too few aux records
                (25, 2, 2, 100),  # brush == n_ents
                (1, 2, 0, 100),   # not a train
            ]
        )
        stats = audit.parse_room(room, "cap_map")
        self.assertEqual((stats.total, stats.valid, stats.invalid), (5, 3, 2))
        self.assertEqual(
            audit.evaluate_rooms([stats], 2),
            ["cap_map: valid trains 3>2"],
        )

    def test_zero_speed_fails_only_for_runtime_valid_train(self) -> None:
        stats = audit.parse_room(
            _room([(25, 2, 0, 0), (25, 1, 0, 0), (25, 2, 9, 0)]),
            "speed_map",
        )
        self.assertEqual(stats.zero_speed, (0,))
        self.assertIn("valid zero-speed trains", audit.evaluate_rooms([stats], 8)[0])

    def test_rejects_format_bounds_and_aux_overrun(self) -> None:
        with self.subTest("short header"):
            with self.assertRaisesRegex(audit.FormatError, "HLMA header"):
                audit.parse_room(b"HLMA")

        with self.subTest("truncated logic record"):
            data = bytearray(_room([(25, 2, 0, 100)]))
            del data[-audit.LOGIC_AUX_SIZE * 2 - 1 :]
            with self.assertRaisesRegex(audit.FormatError, "logic record table"):
                audit.parse_room(bytes(data))

        with self.subTest("aux range"):
            data = bytearray(_room([(25, 2, 0, 100)]))
            logic_off = struct.unpack_from("<I", data, 48)[0]
            data[logic_off + 8 + 14] = 3
            with self.assertRaisesRegex(audit.FormatError, "aux range"):
                audit.parse_room(bytes(data), "bad_aux")

    def test_reads_runtime_cap_instead_of_duplicating_it(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            source = Path(tmp) / "main.rs"
            source.write_text("const MAX_TRAINS: usize = 1_024;\n", encoding="utf-8")
            self.assertEqual(audit.parse_max_trains(source), 1024)


if __name__ == "__main__":
    unittest.main()
