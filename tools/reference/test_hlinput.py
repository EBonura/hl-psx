from __future__ import annotations

import struct
import unittest

from hlinput import (
    ACTION_ATTACK,
    ACTION_FLASHLIGHT,
    ACTION_USE,
    HEADER,
    KNOWN_ACTIONS,
    RUN,
    SEGMENT_HEADER,
    InputRun,
    InputSample,
    Route,
    RouteCursor,
    Segment,
    TapeError,
    decode,
    encode,
    extract_trace,
    rle,
)


def sample(value: int, actions: int = 0) -> InputSample:
    return InputSample(value, -value, value // 2, -(value // 2), actions)


def route_fixture() -> Route:
    return Route(
        (
            Segment(
                "c1a0b",
                "c1a0c",
                (InputRun(2, sample(80, ACTION_USE)), InputRun(1, InputSample())),
                neutral_tail_ticks=3,
            ),
            Segment(
                "c1a0c",
                "",
                (InputRun(4, sample(127, ACTION_ATTACK)),),
                neutral_tail_ticks=2,
            ),
        )
    )


class HlInputTests(unittest.TestCase):
    def test_wire_layout_is_exact_little_endian(self) -> None:
        route = Route(
            (
                Segment(
                    "c0a0",
                    "",
                    (
                        InputRun(
                            3,
                            InputSample(
                                -128,
                                127,
                                -1,
                                1,
                                ACTION_ATTACK | ACTION_FLASHLIGHT,
                            ),
                        ),
                    ),
                    neutral_tail_ticks=4,
                ),
            )
        )
        expected = bytes.fromhex(
            "48 4c 49 4e 50 55 54 31 14 00 01 00 00 00 00 00 "
            "63 30 61 30 00 00 00 00 00 00 00 00 00 00 00 00 "
            "00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 "
            "03 00 00 00 01 00 00 00 04 00 00 00 00 00 00 00 "
            "03 00 80 7f ff 01 01 01"
        )
        self.assertEqual(encode(route), expected)
        self.assertEqual(decode(expected), route)

    def test_binary_roundtrip_and_expansion(self) -> None:
        route = route_fixture()
        loaded = decode(encode(route))
        self.assertEqual(loaded, route)
        self.assertEqual(
            loaded.segments[0].expand(),
            (sample(80, ACTION_USE), sample(80, ACTION_USE), InputSample()),
        )
        self.assertEqual(loaded.segments[1].total_ticks, 4)

    def test_rle_splits_at_u16_duration(self) -> None:
        runs = rle(InputSample() for _ in range(0x10000 + 2))
        self.assertEqual([run.ticks for run in runs], [0xFFFF, 3])

    def test_decode_rejects_header_truncation_magic_rate_and_trailing_data(self) -> None:
        data = bytearray(encode(route_fixture()))
        with self.assertRaisesRegex(TapeError, "shorter"):
            decode(bytes(data[:4]))
        bad = bytearray(data)
        bad[0:8] = b"BADTAPE!"
        with self.assertRaisesRegex(TapeError, "magic"):
            decode(bytes(bad))
        bad = bytearray(data)
        struct.pack_into("<H", bad, 8, 60)
        with self.assertRaisesRegex(TapeError, "tick rate"):
            decode(bytes(bad))
        with self.assertRaisesRegex(TapeError, "trailing"):
            decode(bytes(data) + b"x")

    def test_decode_rejects_truncated_run_and_declared_tick_mismatch(self) -> None:
        data = bytearray(encode(route_fixture()))
        with self.assertRaisesRegex(TapeError, "truncated"):
            decode(bytes(data[: HEADER.size + SEGMENT_HEADER.size + RUN.size - 1]))
        bad = bytearray(data)
        total_tick_offset = HEADER.size + 32
        struct.pack_into("<I", bad, total_tick_offset, 999)
        with self.assertRaisesRegex(TapeError, "declares 999"):
            decode(bytes(bad))

    def test_unknown_actions_fail_encode_and_decode(self) -> None:
        unknown = KNOWN_ACTIONS + 1
        route = Route((Segment("c1a0b", "", (InputRun(1, InputSample(actions=unknown)),)),))
        with self.assertRaisesRegex(TapeError, "unknown semantic action"):
            encode(route)

        bad = bytearray(encode(route_fixture()))
        first_actions = HEADER.size + SEGMENT_HEADER.size + 6
        struct.pack_into("<H", bad, first_actions, unknown)
        with self.assertRaisesRegex(TapeError, "unknown semantic action"):
            decode(bytes(bad))

    def test_map_names_and_order_are_strict(self) -> None:
        with self.assertRaisesRegex(TapeError, "invalid map"):
            encode(Route((Segment("bad/map", "", (InputRun(1, InputSample()),)),)))
        wrong = Route(
            (
                Segment("c1a0b", "c1a0d", (InputRun(1, InputSample()),)),
                Segment("c1a0c", "", (InputRun(1, InputSample()),)),
            )
        )
        with self.assertRaisesRegex(TapeError, "ordered next segment"):
            encode(wrong)

    def test_trace_extraction_is_map_local_and_rle_encoded(self) -> None:
        lines = [
            "noise",
            "[guest f1 c2] HLPSX|input|map=c1a0b|tick=0|forward=127|strafe=0|turn=0|look=0|actions=0x0008",
            "HLPSX|input|map=c1a0b|tick=1|forward=127|strafe=0|turn=0|look=0|actions=8",
            "HLPSX|input|map=c1a0b|tick=2|forward=0|strafe=0|turn=0|look=0|actions=0",
            "HLPSX|input|map=c1a0c|tick=0|forward=0|strafe=-40|turn=8|look=-9|actions=1",
        ]
        route = extract_trace(lines, neutral_tail_ticks=17)
        self.assertEqual([segment.map for segment in route.segments], ["c1a0b", "c1a0c"])
        self.assertEqual(route.segments[0].next_map, "c1a0c")
        self.assertEqual([run.ticks for run in route.segments[0].runs], [2, 1])
        self.assertEqual(route.segments[0].neutral_tail_ticks, 17)
        self.assertEqual(route.segments[1].expand()[0], InputSample(0, -40, 8, -9, 1))

    def test_trace_extraction_rejects_missing_and_noncontiguous_ticks(self) -> None:
        with self.assertRaisesRegex(TapeError, "no HLPSX"):
            extract_trace(["nothing here"])
        with self.assertRaisesRegex(TapeError, "expected 1"):
            extract_trace(
                [
                    "HLPSX|input|map=c1a0b|tick=0|forward=0|strafe=0|turn=0|look=0|actions=0",
                    "HLPSX|input|map=c1a0b|tick=2|forward=0|strafe=0|turn=0|look=0|actions=0",
                ]
            )

    def test_cursor_resets_at_map_boundary_and_enforces_neutral_tail(self) -> None:
        route = Route(
            (
                Segment("a", "b", (InputRun(1, sample(12)),), neutral_tail_ticks=2),
                Segment("b", "", (InputRun(1, sample(-7)),), neutral_tail_ticks=0),
            )
        )
        cursor = RouteCursor(route)
        with self.assertRaisesRegex(TapeError, "first map_start"):
            _ = cursor.segment
        cursor.begin_map("a")
        self.assertEqual(cursor.consume("a", 0), sample(12))
        self.assertEqual(cursor.consume("a", 1), InputSample())
        self.assertEqual(cursor.consume("a", 2), InputSample())
        with self.assertRaisesRegex(TapeError, "exhausted"):
            cursor.consume("a", 3)

        cursor.begin_map("b")
        self.assertEqual(cursor.consume("b", 0), sample(-7))
        with self.assertRaisesRegex(TapeError, "expected 1"):
            cursor.consume("b", 0)
        with self.assertRaisesRegex(TapeError, "final route"):
            cursor.begin_map("c")

    def test_cursor_rejects_unexpected_map_and_tick(self) -> None:
        cursor = RouteCursor(route_fixture())
        with self.assertRaisesRegex(TapeError, "requires 'c1a0b'"):
            cursor.begin_map("c1a0c")
        cursor.begin_map("c1a0b")
        with self.assertRaisesRegex(TapeError, "expected 0"):
            cursor.consume("c1a0b", 1)
        with self.assertRaisesRegex(TapeError, "while segment"):
            cursor.consume("c1a0c", 0)


if __name__ == "__main__":
    unittest.main()
