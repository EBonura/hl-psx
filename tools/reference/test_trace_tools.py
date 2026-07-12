from __future__ import annotations

import struct
import tempfile
import unittest
from argparse import Namespace
from pathlib import Path

from make_input_tape import MAGIC, SAMPLE, build_tape
from trace_tools import (
    canonical_xyz,
    command_compare,
    comparison_report,
    entity_field_differences,
    entity_is_active,
    parse_map_occurrence,
    parse_pipe_line,
    read_records,
)


class InputTapeTests(unittest.TestCase):
    def test_direct_boot_then_neutral(self) -> None:
        data = build_tape(frame_count=4, boot_frames=2, map_index=7)
        self.assertEqual(data[:8], MAGIC)
        self.assertEqual(struct.unpack_from("<I", data, 8)[0], 4)
        first = SAMPLE.unpack_from(data, 12)
        third = SAMPLE.unpack_from(data, 12 + SAMPLE.size * 2)
        self.assertEqual(first, (0x0407, 0x80, 0x80, 0x80, 0x80))
        self.assertEqual(third, (0, 0x80, 0x80, 0x80, 0x80))


class TraceTests(unittest.TestCase):
    def test_map_occurrence_requirement_parser(self) -> None:
        self.assertEqual(parse_map_occurrence("c1a1c"), ("c1a1c", None))
        self.assertEqual(parse_map_occurrence("c1a1c@2"), ("c1a1c", 2))
        for invalid in ("c1a1c@0", "c1a1c@", "@2", "c1a1c@two"):
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                parse_map_occurrence(invalid)

    def test_strips_psoxide_prefix_and_swizzles_coordinates(self) -> None:
        record = parse_pipe_line(
            "[guest f1 c2] HLPSX|tick|map=c0a0|tick=3|px=10|py=30|pz=20"
        )
        self.assertIsNotNone(record)
        self.assertEqual(canonical_xyz(record, "p"), (10.0, 20.0, 30.0))

    def test_reports_missing_semantic_event(self) -> None:
        reference = [
            parse_pipe_line("HLREF|event|map=c0a0|tick=0|event=map_start"),
            parse_pipe_line(
                "HLREF|event|map=c0a0|tick=60|event=target_fire|target=train"
            ),
            parse_pipe_line(
                "HLREF|event|map=c0a0|tick=80|event=target_fire|target=forklift"
            ),
        ]
        psx = [
            parse_pipe_line("HLPSX|event|map=c0a0|tick=0|event=map_start"),
            parse_pipe_line(
                "HLPSX|event|map=c0a0|tick=60|event=target_fire|target=train"
            ),
        ]
        report = comparison_report(reference, psx)
        self.assertEqual(len(report["missing_events"]), 1)
        self.assertEqual(report["missing_events"][0]["target"], "forklift")

    def test_event_drift_is_relative_to_each_map_start(self) -> None:
        reference = [
            parse_pipe_line("HLREF|map|map=c0a0|seed=1337"),
            parse_pipe_line("HLREF|tick|map=c0a0|tick=3"),
            parse_pipe_line("HLREF|map|map=c0a0a|seed=1337"),
            parse_pipe_line("HLREF|tick|map=c0a0a|tick=777"),
            parse_pipe_line(
                "HLREF|event|map=c0a0a|tick=779|event=target_fire|target=cranemm"
            ),
        ]
        psx = [
            parse_pipe_line("HLPSX|event|map=c0a0|tick=0|event=map_start"),
            parse_pipe_line("HLPSX|event|map=c0a0a|tick=0|event=map_start"),
            parse_pipe_line(
                "HLPSX|event|map=c0a0a|tick=1|event=target_fire|target=cranemm"
            ),
        ]

        report = comparison_report(reference, psx)
        cranemm = next(
            event for event in report["event_timing"] if event["detail"] == "cranemm"
        )
        self.assertEqual(cranemm["reference_tick"], 2)
        self.assertEqual(cranemm["psx_tick"], 1)
        self.assertEqual(cranemm["drift_ticks"], -1)
        self.assertEqual(report["reference_maps"], ["c0a0", "c0a0a"])

    def test_semantic_input_aligns_first_post_physics_snapshot(self) -> None:
        reference = [
            parse_pipe_line("HLREF|map|map=c1a0|seed=1337"),
            parse_pipe_line("HLREF|tick|map=c1a0|tick=3|px=999|py=999|pz=999"),
            parse_pipe_line(
                "HLREF|input|map=c1a0|tick=0|input_tick=0|forward=127|"
                "strafe=0|turn=0|look=0|actions=0x0008"
            ),
            parse_pipe_line("HLREF|tick|map=c1a0|tick=4|px=10|py=20|pz=30"),
            parse_pipe_line(
                "HLREF|input|map=c1a0|tick=1|input_tick=1|forward=0|"
                "strafe=0|turn=0|look=0|actions=0x0000"
            ),
            parse_pipe_line("HLREF|tick|map=c1a0|tick=5|px=11|py=20|pz=30"),
        ]
        psx = [
            parse_pipe_line("HLPSX|event|map=c1a0|tick=0|event=map_start"),
            parse_pipe_line(
                "HLPSX|input|map=c1a0|tick=0|input_tick=0|forward=127|"
                "strafe=0|turn=0|look=0|actions=8"
            ),
            parse_pipe_line("HLPSX|tick|map=c1a0|tick=0|px=10|py=30|pz=20"),
            parse_pipe_line(
                "HLPSX|input|map=c1a0|tick=1|input_tick=1|forward=0|"
                "strafe=0|turn=0|look=0|actions=0"
            ),
            parse_pipe_line("HLPSX|tick|map=c1a0|tick=1|px=11|py=30|pz=20"),
        ]

        report = comparison_report(reference, psx)
        self.assertEqual(
            report["input_alignment"],
            [
                {
                    "map": "c1a0",
                    "occurrence": 1,
                    "reference_snapshot_tick": 4,
                    "psx_snapshot_tick": 0,
                }
            ],
        )
        self.assertTrue(report["input_comparison"][0]["identical"])
        self.assertEqual(report["positions"][0]["player"]["max"], 0.0)

    def test_input_comparison_reports_first_mismatch(self) -> None:
        reference = [
            parse_pipe_line(
                "HLREF|input|map=c1a0|tick=0|input_tick=0|forward=0|"
                "strafe=0|turn=0|look=0|actions=0x0001"
            )
        ]
        psx = [
            parse_pipe_line(
                "HLPSX|input|map=c1a0|tick=0|input_tick=0|forward=0|"
                "strafe=0|turn=0|look=0|actions=0"
            )
        ]
        report = comparison_report(reference, psx)
        mismatch = report["input_comparison"][0]
        self.assertFalse(mismatch["identical"])
        self.assertEqual(mismatch["first_mismatch_index"], 0)
        self.assertEqual(mismatch["reference"][-1], 1)

    def test_entity_checkpoints_match_by_brush_and_swizzle_centers(self) -> None:
        reference = [
            parse_pipe_line("HLREF|map|map=c1a0|seed=1"),
            parse_pipe_line(
                "HLREF|entity|map=c1a0|tick=4|map_tick=0|class=func_pushable|"
                "model=*9|cx=10|cy=20|cz=30|solid=0"
            ),
            parse_pipe_line(
                "HLREF|entity|map=c1a0|tick=24|map_tick=20|class=func_pushable|"
                "brush=9|cx=30|cy=20|cz=30|solid=2"
            ),
        ]
        psx = [
            parse_pipe_line("HLPSX|event|map=c1a0|tick=0|event=map_start"),
            parse_pipe_line(
                "HLPSX|entity|map=c1a0|tick=0|map_tick=0|class=func_pushable|"
                "brush=9|cx=10|cy=30|cz=20|active=1"
            ),
            parse_pipe_line(
                "HLPSX|entity|map=c1a0|tick=20|map_tick=20|class=func_pushable|"
                "brush=9|cx=30|cy=30|cz=20|active=1"
            ),
        ]

        entities = comparison_report(reference, psx)["entity_comparison"]
        self.assertEqual(len(entities), 1)
        self.assertEqual(entities[0]["matched_samples"], 2)
        self.assertEqual(entities[0]["position"]["max"], 0.0)
        self.assertEqual(
            entities[0]["position_by_entity"],
            [
                {
                    "entity": "brush:*9",
                    "samples": 2,
                    "rms": 0.0,
                    "max": 0.0,
                    "worst": {
                        "tick": 20,
                        "reference": [30.0, 20.0, 30.0],
                        "psx": [30.0, 20.0, 30.0],
                    },
                }
            ],
        )
        self.assertEqual(entities[0]["missing_entities"], [])
        self.assertEqual(entities[0]["active_mismatches"], [])

    def test_entity_position_ranking_exposes_outlier(self) -> None:
        reference = [
            parse_pipe_line("HLREF|map|map=c1a0|seed=1"),
            parse_pipe_line("HLREF|entity|map=c1a0|map_tick=0|brush=1|cx=0|cy=0|cz=0"),
            parse_pipe_line("HLREF|entity|map=c1a0|map_tick=0|brush=2|cx=0|cy=0|cz=0"),
        ]
        psx = [
            parse_pipe_line("HLPSX|event|map=c1a0|tick=0|event=map_start"),
            parse_pipe_line("HLPSX|entity|map=c1a0|map_tick=0|brush=1|cx=1|cy=0|cz=0"),
            parse_pipe_line("HLPSX|entity|map=c1a0|map_tick=0|brush=2|cx=9|cy=0|cz=0"),
        ]

        ranked = comparison_report(reference, psx)["entity_comparison"][0][
            "position_by_entity"
        ]
        self.assertEqual([row["entity"] for row in ranked], ["brush:*2", "brush:*1"])
        self.assertEqual([row["max"] for row in ranked], [9.0, 1.0])

    def test_func_rotating_phase_is_compared_in_canonical_goldsrc_degrees(self) -> None:
        reference = [
            parse_pipe_line("HLREF|map|map=c1a2|seed=1"),
            parse_pipe_line(
                "HLREF|entity|map=c1a2|map_tick=20|class=func_rotating|"
                "brush=20|spawnflags=151|roll=-78.4|x=10|y=20|z=30|"
                "cx=100|cy=200|cz=300"
            ),
            parse_pipe_line(
                "HLREF|entity|map=c1a2|map_tick=40|class=func_rotating|"
                "brush=20|spawnflags=151|roll=-90|x=10|y=20|z=30|"
                "cx=101|cy=201|cz=301"
            ),
        ]
        psx = [
            parse_pipe_line("HLPSX|event|map=c1a2|tick=0|event=map_start"),
            parse_pipe_line(
                "HLPSX|entity|map=c1a2|map_tick=20|class=func_rotating|"
                "brush=20|spawnflags=151|yaw_q12=892|x=10|y=30|z=20|"
                "cx=-100|cy=-300|cz=-200"
            ),
            parse_pipe_line(
                "HLPSX|entity|map=c1a2|map_tick=40|class=func_rotating|"
                "brush=20|spawnflags=151|yaw_q12=2048|x=10|y=30|z=20|"
                "cx=-101|cy=-301|cz=-201"
            ),
        ]

        row = comparison_report(reference, psx)["entity_comparison"][0]
        self.assertEqual(row["position"]["max"], 0.0)
        self.assertAlmostEqual(
            row["angle_by_entity"][0]["worst"]["psx_degrees"], -180.0
        )
        self.assertEqual(row["angle"]["max_degrees"], 90.0)
        self.assertLess(
            next(
                sample["rms_degrees"]
                for sample in row["angle_by_entity"]
                if sample["entity"] == "brush:*20"
            ),
            64.0,
        )

    def test_entity_checkpoints_report_missing_and_active_mismatch(self) -> None:
        reference = [
            parse_pipe_line("HLREF|map|map=c1a0|seed=1"),
            parse_pipe_line(
                "HLREF|entity|map=c1a0|tick=4|map_tick=0|brush=3|"
                "cx=0|cy=0|cz=0|solid=2"
            ),
            parse_pipe_line(
                "HLREF|entity|map=c1a0|tick=4|map_tick=0|brush=4|"
                "cx=0|cy=0|cz=0|solid=2"
            ),
        ]
        psx = [
            parse_pipe_line("HLPSX|event|map=c1a0|tick=0|event=map_start"),
            parse_pipe_line(
                "HLPSX|entity|map=c1a0|tick=0|map_tick=0|brush=3|"
                "cx=0|cy=0|cz=0|active=0"
            ),
        ]

        row = comparison_report(reference, psx)["entity_comparison"][0]
        self.assertEqual(row["missing_entities"], ["brush:*4"])
        self.assertEqual(row["active_mismatches"][0]["entity"], "brush:*3")

    def test_entity_fields_compare_class_aware_semantics(self) -> None:
        cases = [
            (
                "ordinary pushable inert health",
                "class=func_pushable|spawnflags=0|health=100",
                "class=func_pushable|spawnflags=0|health=0",
                [],
            ),
            (
                "breakable pushable live health",
                "class=func_pushable|spawnflags=128|health=10",
                "class=func_pushable|spawnflags=128|health=9",
                [("health", 10, 9)],
            ),
            (
                "trigger-only zero-health alive sentinel",
                "class=func_breakable|spawnflags=1|health=0",
                "class=func_breakable|spawnflags=1|health=1",
                [],
            ),
            (
                "damageable breakable health",
                "class=func_breakable|spawnflags=0|health=10",
                "class=func_breakable|spawnflags=0|health=9",
                [("health", 10, 9)],
            ),
            (
                "scientist health remains gameplay state",
                "class=monster_scientist|spawnflags=0|health=20",
                "class=monster_scientist|spawnflags=0|health=24",
                [("health", 20, 24)],
            ),
            (
                "train runtime retrigger bit",
                "class=func_train|spawnflags=9|health=0",
                "class=func_train|spawnflags=8|health=0",
                [],
            ),
            (
                "train passability remains meaningful",
                "class=func_train|spawnflags=9|health=0",
                "class=func_train|spawnflags=1|health=0",
                [("spawnflags", 8, 0)],
            ),
            (
                "ordinary wall ignores wall-toggle bit",
                "class=func_wall|spawnflags=1|health=0",
                "class=func_wall|spawnflags=0|health=0",
                [],
            ),
            (
                "wall-toggle start state remains meaningful",
                "class=func_wall_toggle|spawnflags=1|health=0",
                "class=func_wall_toggle|spawnflags=0|health=0",
                [("spawnflags", 1, 0)],
            ),
            (
                "rotating axis remains meaningful",
                "class=func_rotating|spawnflags=5|health=0",
                "class=func_rotating|spawnflags=1|health=0",
                [("spawnflags", 5, 1)],
            ),
            (
                "tracktrain runtime nocontrol bit",
                "class=func_tracktrain|spawnflags=15|health=0",
                "class=func_tracktrain|spawnflags=13|health=0",
                [],
            ),
            (
                "door runtime silent bit",
                "class=func_door|spawnflags=2147483656|health=0",
                "class=func_door|spawnflags=8|health=0",
                [],
            ),
            (
                "door passability remains meaningful",
                "class=func_door|spawnflags=8|health=0",
                "class=func_door|spawnflags=0|health=0",
                [("spawnflags", 8, 0)],
            ),
            (
                "raw monster spawnflags are not common representation",
                "class=monster_scientist|spawnflags=256|health=20",
                "class=monster_scientist|spawnflags=0|health=20",
                [],
            ),
            (
                "authored class mismatch remains visible",
                "class=func_plat|spawnflags=0|health=0",
                "class=func_door|spawnflags=0|health=0",
                [("class", "func_plat", "func_door")],
            ),
        ]
        for name, reference_fields, psx_fields, expected in cases:
            with self.subTest(name=name):
                reference = parse_pipe_line(f"HLREF|entity|{reference_fields}")
                psx = parse_pipe_line(f"HLPSX|entity|{psx_fields}")
                self.assertIsNotNone(reference)
                self.assertIsNotNone(psx)
                self.assertEqual(entity_field_differences(reference, psx), expected)

    def test_entity_active_normalizes_semantic_lifecycle(self) -> None:
        cases = [
            ("func_wall_toggle", "solid=0", False),
            ("func_wall_toggle", "solid=4", True),
            ("func_breakable", "solid=0", False),
            ("func_breakable", "solid=4", True),
            ("func_pushable", "solid=0|spawnflags=128", False),
            ("func_pushable", "solid=2|spawnflags=0", True),
            # A NOT_SOLID rotating visual still exists and remains enabled.
            ("func_rotating", "solid=0|spawnflags=64", True),
        ]
        for entity_class, fields, expected in cases:
            with self.subTest(entity_class=entity_class, fields=fields):
                record = parse_pipe_line(f"HLREF|entity|class={entity_class}|{fields}")
                self.assertIsNotNone(record)
                self.assertEqual(entity_is_active(record), expected)

        psx = parse_pipe_line("HLPSX|entity|class=func_wall_toggle|active=0")
        self.assertIsNotNone(psx)
        self.assertFalse(entity_is_active(psx))

    def test_start_off_wall_toggle_has_no_active_mismatch(self) -> None:
        reference = [
            parse_pipe_line("HLREF|map|map=c1a1b|seed=1"),
            parse_pipe_line(
                "HLREF|entity|map=c1a1b|map_tick=0|class=func_wall_toggle|"
                "brush=53|solid=0|spawnflags=1|cx=1|cy=2|cz=3"
            ),
        ]
        psx = [
            parse_pipe_line("HLPSX|event|map=c1a1b|tick=0|event=map_start"),
            parse_pipe_line(
                "HLPSX|entity|map=c1a1b|map_tick=0|class=func_wall_toggle|"
                "brush=53|active=0|spawnflags=1|cx=1|cy=3|cz=2"
            ),
        ]
        row = comparison_report(reference, psx)["entity_comparison"][0]
        self.assertEqual(row["active_mismatches"], [])
        self.assertEqual(row["field_mismatches"], [])

    def test_repeated_map_visits_keep_independent_input_and_state_anchors(self) -> None:
        reference = [
            parse_pipe_line("HLREF|map|map=c1a1c|seed=1"),
            parse_pipe_line(
                "HLREF|input|map=c1a1c|tick=0|input_tick=0|forward=1|"
                "strafe=0|turn=0|look=0|actions=0"
            ),
            parse_pipe_line("HLREF|tick|map=c1a1c|tick=101|px=10|py=20|pz=30"),
            parse_pipe_line("HLREF|map|map=c1a1d|seed=1"),
            parse_pipe_line("HLREF|tick|map=c1a1d|tick=201|px=20|py=20|pz=30"),
            parse_pipe_line("HLREF|map|map=c1a1c|seed=1"),
            parse_pipe_line(
                "HLREF|input|map=c1a1c|tick=0|input_tick=0|forward=-1|"
                "strafe=0|turn=0|look=0|actions=0"
            ),
            parse_pipe_line("HLREF|tick|map=c1a1c|tick=301|px=30|py=20|pz=30"),
        ]
        psx = [
            parse_pipe_line("HLPSX|event|map=c1a1c|tick=0|event=map_start"),
            parse_pipe_line(
                "HLPSX|input|map=c1a1c|tick=0|input_tick=0|forward=1|"
                "strafe=0|turn=0|look=0|actions=0"
            ),
            parse_pipe_line("HLPSX|tick|map=c1a1c|tick=0|px=10|py=30|pz=20"),
            parse_pipe_line("HLPSX|event|map=c1a1d|tick=0|event=map_start"),
            parse_pipe_line("HLPSX|tick|map=c1a1d|tick=0|px=20|py=30|pz=20"),
            parse_pipe_line("HLPSX|event|map=c1a1c|tick=0|event=map_start"),
            parse_pipe_line(
                "HLPSX|input|map=c1a1c|tick=0|input_tick=0|forward=-1|"
                "strafe=0|turn=0|look=0|actions=0"
            ),
            parse_pipe_line("HLPSX|tick|map=c1a1c|tick=0|px=30|py=30|pz=20"),
        ]

        report = comparison_report(reference, psx)
        self.assertEqual(report["reference_maps"], ["c1a1c", "c1a1d", "c1a1c"])
        visits = [p for p in report["positions"] if p["map"] == "c1a1c"]
        self.assertEqual([p["occurrence"] for p in visits], [1, 2])
        self.assertEqual([p["player"]["max"] for p in visits], [0.0, 0.0])
        inputs = [p for p in report["input_comparison"] if p["map"] == "c1a1c"]
        self.assertEqual([p["occurrence"] for p in inputs], [1, 2])
        self.assertTrue(all(p["identical"] for p in inputs))

    def test_late_old_map_input_does_not_create_phantom_visits(self) -> None:
        reference = [
            parse_pipe_line("HLREF|map|map=c0a0|seed=1"),
            parse_pipe_line(
                "HLREF|input|map=c0a0|tick=0|input_tick=0|forward=0|"
                "strafe=0|turn=0|look=0|actions=0"
            ),
            parse_pipe_line("HLREF|tick|map=c0a0|tick=100|px=1|py=2|pz=3"),
            parse_pipe_line("HLREF|map|map=c0a0a|seed=1"),
            # Xash can flush this final old-map command after publishing the
            # new map marker. It still belongs to c0a0 occurrence one.
            parse_pipe_line(
                "HLREF|input|map=c0a0|tick=1|input_tick=1|forward=0|"
                "strafe=0|turn=0|look=0|actions=0"
            ),
            parse_pipe_line("HLREF|tick|map=c0a0a|tick=200|px=4|py=5|pz=6"),
            parse_pipe_line("HLREF|input_map|map=c0a0a|input_tick=0|segment=1"),
            parse_pipe_line(
                "HLREF|input|map=c0a0a|tick=0|input_tick=0|forward=0|"
                "strafe=0|turn=0|look=0|actions=0"
            ),
            parse_pipe_line("HLREF|tick|map=c0a0a|tick=201|px=4|py=5|pz=6"),
        ]
        psx = [
            parse_pipe_line("HLPSX|event|map=c0a0|tick=0|event=map_start"),
            parse_pipe_line(
                "HLPSX|input|map=c0a0|tick=0|input_tick=0|forward=0|"
                "strafe=0|turn=0|look=0|actions=0"
            ),
            parse_pipe_line(
                "HLPSX|input|map=c0a0|tick=1|input_tick=1|forward=0|"
                "strafe=0|turn=0|look=0|actions=0"
            ),
            parse_pipe_line("HLPSX|tick|map=c0a0|tick=0|px=1|py=3|pz=2"),
            parse_pipe_line("HLPSX|event|map=c0a0a|tick=0|event=map_start"),
            parse_pipe_line("HLPSX|tick|map=c0a0a|tick=0|px=4|py=6|pz=5"),
            parse_pipe_line(
                "HLPSX|input|map=c0a0a|tick=0|input_tick=0|forward=0|"
                "strafe=0|turn=0|look=0|actions=0"
            ),
            parse_pipe_line("HLPSX|tick|map=c0a0a|tick=1|px=4|py=6|pz=5"),
        ]

        report = comparison_report(reference, psx)
        self.assertEqual(report["reference_maps"], ["c0a0", "c0a0a"])
        self.assertEqual(report["psx_maps"], ["c0a0", "c0a0a"])
        self.assertEqual(
            [
                (row["map"], row["occurrence"], row["reference_samples"])
                for row in report["input_comparison"]
            ],
            [("c0a0", 1, 2), ("c0a0a", 1, 1)],
        )
        self.assertTrue(all(row["identical"] for row in report["input_comparison"]))

    def test_reads_only_trace_lines(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "trace.log"
            path.write_text(
                "noise\n[guest f0 c1] HLPSX|event|map=c0a0|tick=0|event=map_start\n",
                encoding="utf-8",
            )
            records = read_records(path)
            self.assertEqual(len(records), 1)

    def test_strict_route_requirements_reject_detached_exit(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            reference = root / "reference.log"
            psx = root / "psx.log"
            reference.write_text(
                "HLREF|map|map=c0a0|seed=1\n"
                "HLREF|tick|map=c0a0|tick=3|train_x=0|train_y=0|train_z=0\n"
                "HLREF|event|map=c0a0|tick=4|event=target_fire|target=train\n",
                encoding="utf-8",
            )
            psx.write_text(
                "HLPSX|event|map=c0a0|tick=0|event=map_start\n"
                "HLPSX|event|map=c0a0|tick=1|event=target_fire|target=train\n"
                "HLPSX|tick|map=c0a0|tick=1|train_attached=0|"
                "train_x=0|train_y=0|train_z=0\n",
                encoding="utf-8",
            )
            args = Namespace(
                reference=reference,
                psx=psx,
                output=root / "report.json",
                require_map=["c0a0"],
                require_event=["c0a0:target_fire:train"],
                require_carry=[],
                require_attached_exit=["c0a0"],
                max_train_error=None,
                strict=False,
            )
            self.assertEqual(command_compare(args), 1)

    def test_required_actor_carry_checks_direction_and_identity(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            reference = root / "reference.log"
            psx = root / "psx.log"
            reference.write_text(
                "HLREF|map|map=c0a0d|seed=1\n" "HLREF|tick|map=c0a0d|tick=3\n",
                encoding="utf-8",
            )
            psx.write_text(
                "HLPSX|event|map=c0a0d|tick=0|event=map_start\n"
                "HLPSX|event|map=c0a0d|tick=10|event=actor_carry|"
                "direction=out|id=8727\n"
                "HLPSX|event|map=c0a0e|tick=0|event=map_start\n"
                "HLPSX|event|map=c0a0e|tick=0|event=actor_carry|"
                "direction=in|id=8727\n"
                "HLPSX|event|map=c1a1b|tick=20|event=actor_carry|"
                "direction=train-out|id=54157\n"
                "HLPSX|event|map=c1a1c|tick=0|event=actor_carry|"
                "direction=train-in|id=54157\n",
                encoding="utf-8",
            )
            args = Namespace(
                reference=reference,
                psx=psx,
                output=root / "report.json",
                require_map=["c0a0e"],
                require_event=[],
                require_carry=[
                    "c0a0d:out:8727",
                    "c0a0e:in:8727",
                    "c1a1b:train-out:54157",
                    "c1a1c:train-in:54157",
                ],
                require_attached_exit=[],
                max_train_error=None,
                strict=False,
            )
            self.assertEqual(command_compare(args), 0)
            args.require_carry.append("c0a0e:out:8727")
            self.assertEqual(command_compare(args), 1)

    def test_route_requirements_can_select_a_repeated_map_visit(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            reference = root / "reference.log"
            psx = root / "psx.log"
            reference.write_text(
                "HLREF|map|map=c1a1c|seed=1\n"
                "HLREF|map|map=c1a1d|seed=1\n"
                "HLREF|map|map=c1a1c|seed=1\n",
                encoding="utf-8",
            )
            psx.write_text(
                "HLPSX|event|map=c1a1c|tick=0|event=map_start\n"
                "HLPSX|event|map=c1a1c|tick=3|event=target_fire|target=first_only\n"
                "HLPSX|tick|map=c1a1c|tick=4|train_attached=0\n"
                "HLPSX|event|map=c1a1d|tick=0|event=map_start\n"
                "HLPSX|event|map=c1a1c|tick=0|event=map_start\n"
                "HLPSX|tick|map=c1a1c|tick=4|train_attached=1\n",
                encoding="utf-8",
            )
            args = Namespace(
                reference=reference,
                psx=psx,
                output=root / "report.json",
                require_map=["c1a1c@2"],
                require_event=["c1a1c@1:target_fire:first_only"],
                require_carry=[],
                require_attached_exit=["c1a1c@2"],
                max_train_error=None,
                strict=False,
            )
            self.assertEqual(command_compare(args), 0)
            args.require_event = ["c1a1c@2:target_fire:first_only"]
            self.assertEqual(command_compare(args), 1)
            args.require_event = []
            args.require_map = ["c1a1c@3"]
            self.assertEqual(command_compare(args), 1)


if __name__ == "__main__":
    unittest.main()
