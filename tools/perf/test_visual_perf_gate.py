#!/usr/bin/env python3
"""Tests for tools/perf/visual_perf_gate.py."""

from __future__ import annotations

import csv
import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from visual_perf_gate import (  # noqa: E402
    GatePolicy,
    PerfDataError,
    ScenarioSpec,
    analyze_scenario,
    compare_identity,
    compare_visuals,
    nearest_rank,
)


FIELDS = [
    "guest_frame",
    "end_bus_cycles",
    "frame_cycles",
    "visual_render_task",
    "render",
    "sim_ticks",
    "visual_frames",
    "visual_skipped_vblanks",
    "visual_deadline_misses",
    "visual_lateness_vblanks",
    "tri_primitives",
    "tri_primitive_remaining",
    "room_submit_primitive_overflows",
]


def row(
    frame: int,
    end: int,
    *,
    visual: int,
    sim: int = 1,
    render: int = 900,
    task: int = 950,
    primitives: int = 700,
    free: int = 300,
    misses: int = 0,
    skips: int = 0,
    late: int = 0,
    overflow: int = 0,
) -> dict[str, int]:
    return {
        "guest_frame": frame,
        "end_bus_cycles": end,
        "frame_cycles": 1_000,
        "visual_render_task": task if visual else 0,
        "render": render if visual else 0,
        "sim_ticks": sim,
        "visual_frames": visual,
        "visual_skipped_vblanks": skips,
        "visual_deadline_misses": misses,
        "visual_lateness_vblanks": late,
        "tri_primitives": primitives if visual else 0,
        "tri_primitive_remaining": free if visual else 0,
        "room_submit_primitive_overflows": overflow,
    }


class VisualPerfGateTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tempdir = tempfile.TemporaryDirectory()
        self.root = Path(self.tempdir.name)
        self.policy = GatePolicy(
            target_fps=20.0,
            clock_hz=20_000,
            tolerance_fraction=0.0,
            min_visual_samples=3,
            max_deadline_miss_rate=0.0,
            max_skipped_vblanks=0,
            max_packet_overflows=0,
            min_packet_slots_free=1,
        )

    def tearDown(self) -> None:
        self.tempdir.cleanup()

    def write_csv(self, name: str, rows: list[dict[str, int]]) -> Path:
        path = self.root / name
        with path.open("w", newline="", encoding="utf-8") as handle:
            writer = csv.DictWriter(handle, fieldnames=FIELDS)
            writer.writeheader()
            writer.writerows(rows)
        return path

    def test_visual_intervals_include_intervening_update_rows(self) -> None:
        path = self.write_csv(
            "profile.csv",
            [
                row(0, 1_000, visual=1),
                row(1, 1_100, visual=0),
                row(2, 2_000, visual=1),
                row(3, 3_000, visual=1),
                row(4, 4_000, visual=1),
            ],
        )
        result = analyze_scenario(
            ScenarioSpec("test", path), warmup_visuals=0, policy=self.policy
        )
        self.assertEqual(result.delivery_samples, 3)
        self.assertEqual(result.delivery_cycles_p50, 1_000)
        self.assertEqual(result.delivery_cycles_p95, 1_000)
        self.assertEqual(result.sim_ticks_between_visuals, 4)
        self.assertEqual(result.actual_visual_fps, 20.0)
        self.assertEqual(result.packet_capacity, 1_000)
        self.assertTrue(result.packet_capacity_consistent)
        self.assertFalse(result.gate_failures)

    def test_warmup_discards_visuals_not_csv_rows(self) -> None:
        path = self.write_csv(
            "warmup.csv",
            [
                row(0, 100, visual=0),
                row(1, 1_000, visual=1, render=9_000),
                row(2, 2_000, visual=1, render=900),
                row(3, 3_000, visual=1, render=900),
                row(4, 4_000, visual=1, render=900),
            ],
        )
        result = analyze_scenario(
            ScenarioSpec("test", path), warmup_visuals=1, policy=self.policy
        )
        self.assertEqual(result.visual_samples, 3)
        self.assertEqual(result.render_cycles_p95, 900)

    def test_gate_reports_cadence_and_packet_failures(self) -> None:
        path = self.write_csv(
            "fail.csv",
            [
                row(0, 1_000, visual=1),
                row(1, 3_000, visual=1, misses=1, skips=2, late=3),
                row(2, 5_000, visual=1, primitives=1_000, free=0, overflow=1),
                row(3, 7_000, visual=1),
            ],
        )
        result = analyze_scenario(
            ScenarioSpec("test", path), warmup_visuals=0, policy=self.policy
        )
        joined = "\n".join(result.gate_failures)
        self.assertIn("actual visual FPS", joined)
        self.assertIn("deadline miss rate", joined)
        self.assertIn("skipped VBlanks", joined)
        self.assertIn("packet overflows", joined)
        self.assertIn("minimum packet slots free", joined)

    def test_missing_required_column_is_rejected(self) -> None:
        path = self.root / "bad.csv"
        path.write_text("guest_frame,end_bus_cycles\n0,10\n", encoding="utf-8")
        with self.assertRaisesRegex(PerfDataError, "missing required"):
            analyze_scenario(
                ScenarioSpec("bad", path), warmup_visuals=0, policy=self.policy
            )

    def test_nearest_rank_p95_is_conservative(self) -> None:
        self.assertEqual(nearest_rank(list(range(1, 21)), 0.95), 19)
        self.assertEqual(nearest_rank(list(range(1, 22)), 0.95), 20)

    def test_compare_identity_requires_exact_binary_and_worktrees(self) -> None:
        path_a = self.write_csv(
            "a.csv", [row(i, (i + 1) * 1_000, visual=1) for i in range(4)]
        )
        path_b = self.write_csv(
            "b.csv", [row(i, (i + 1) * 1_000, visual=1) for i in range(4)]
        )
        results = [
            analyze_scenario(
                ScenarioSpec("a", path_a), warmup_visuals=0, policy=self.policy
            ),
            analyze_scenario(
                ScenarioSpec("b", path_b), warmup_visuals=0, policy=self.policy
            ),
        ]
        identity = {
            "artifact_sha256": "artifact",
            "git": {"commit": "game", "worktree_fingerprint": "game-tree"},
            "psoxide_git": {"commit": "emu", "worktree_fingerprint": "emu-tree"},
        }
        for result in results:
            result.metadata = json.loads(json.dumps(identity))
        self.assertEqual(compare_identity(results), (True, []))
        results[1].metadata["artifact_sha256"] = "different"
        verified, reasons = compare_identity(results)
        self.assertFalse(verified)
        self.assertIn("artifact SHA-256 differs", reasons)

    def test_compare_visuals_requires_stamped_equal_final_frames(self) -> None:
        path_a = self.write_csv(
            "visual-a.csv", [row(i, (i + 1) * 1_000, visual=1) for i in range(4)]
        )
        path_b = self.write_csv(
            "visual-b.csv", [row(i, (i + 1) * 1_000, visual=1) for i in range(4)]
        )
        results = [
            analyze_scenario(
                ScenarioSpec("a", path_a), warmup_visuals=0, policy=self.policy
            ),
            analyze_scenario(
                ScenarioSpec("b", path_b), warmup_visuals=0, policy=self.policy
            ),
        ]
        self.assertFalse(compare_visuals(results)[0])
        for result in results:
            result.metadata = {"visual_artifact_sha256": "same-frame"}
        self.assertEqual(compare_visuals(results), (True, []))
        results[1].metadata["visual_artifact_sha256"] = "different-frame"
        verified, reasons = compare_visuals(results)
        self.assertFalse(verified)
        self.assertIn("final visual artifact SHA-256 differs", reasons)


if __name__ == "__main__":
    unittest.main()
