#!/usr/bin/env python3
"""Tests for tools/perf/fleet_audit.py (no emulator is launched)."""

from __future__ import annotations

import csv
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from fleet_audit import (
    CaptureConfig,
    FleetError,
    analyze_map_capture,
    build_launch_command,
    load_map_registry,
    map_boot_route,
    parse_map_selection,
    verify_boot,
)
from visual_perf_gate import GatePolicy


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
    "room_surf_whole_quads",
]


def profile_row(frame: int, end: int, packets: int, quads: int) -> dict[str, int]:
    return {
        "guest_frame": frame,
        "end_bus_cycles": end,
        "frame_cycles": 1_000,
        "visual_render_task": 900,
        "render": 800,
        "sim_ticks": 1,
        "visual_frames": 1,
        "visual_skipped_vblanks": 0,
        "visual_deadline_misses": 0,
        "visual_lateness_vblanks": 0,
        "tri_primitives": packets,
        "tri_primitive_remaining": 1_000 - packets,
        "room_submit_primitive_overflows": 0,
        "room_surf_whole_quads": quads,
    }


class FleetAuditTests(unittest.TestCase):
    def test_registry_is_exact_96_map_campaign(self) -> None:
        maps = load_map_registry()
        self.assertEqual(len(maps), 96)
        self.assertEqual(maps[0], "c0a0")
        self.assertEqual(maps[-1], "c5a1")

    def test_map_selection_names_indexes_and_ranges(self) -> None:
        maps = ["c0a0", "c0a0a", "c1a0", "c5a1"]
        self.assertEqual(parse_map_selection("c0a0,2-3,0", maps), [0, 2, 3])
        with self.assertRaises(FleetError):
            parse_map_selection("4", maps)

    def test_boot_route_carries_l1_and_low_byte_index(self) -> None:
        self.assertEqual(map_boot_route(0), "0x0400@1+180")
        self.assertEqual(map_boot_route(95), "0x045f@1+180")

    def test_console_boot_proof_is_exact_map_line(self) -> None:
        console = "[guest f0 c100] hl-psx: loading room\n[guest f0 c101] c0a0a\n"
        self.assertTrue(verify_boot(console, "c0a0a"))
        self.assertFalse(verify_boot(console, "c0a0"))

    def test_profile_analysis_uses_visual_delivery_and_triangle_equivalent(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            map_dir = Path(raw)
            with (map_dir / "profile.csv").open("w", newline="", encoding="utf-8") as handle:
                writer = csv.DictWriter(handle, fieldnames=FIELDS)
                writer.writeheader()
                for frame in range(6):
                    writer.writerow(profile_row(frame, (frame + 1) * 1_000, 100 + frame, 10))
            (map_dir / "console.log").write_text("[guest f0 c100] c0a0\n", encoding="utf-8")
            policy = GatePolicy(
                target_fps=20.0,
                clock_hz=20_000,
                tolerance_fraction=0.0,
                min_visual_samples=5,
                max_deadline_miss_rate=0.0,
                max_skipped_vblanks=0,
                max_packet_overflows=0,
                min_packet_slots_free=1,
            )
            result = analyze_map_capture(0, "c0a0", map_dir, policy, 1)
            self.assertEqual(result.capture_status, "ok")
            self.assertTrue(result.gate_pass)
            self.assertEqual(result.actual_visual_fps, 20.0)
            self.assertEqual(result.packet_peak, 105)
            self.assertEqual(result.triangle_equiv_peak, 115)

    def test_launch_command_has_all_authoritative_outputs(self) -> None:
        config = CaptureConfig(
            disc=Path("/tmp/game.cue"),
            frontend=Path("/tmp/frontend"),
            psoxide_root=Path("/tmp/PSoXide"),
            visual_frames=64,
            guest_frames=1200,
            steps=800_000_000,
            timeout_seconds=600,
            warmup_visuals=3,
            route="forward-run",
            screenshot=True,
        )
        command = build_launch_command(config, 95, Path("/tmp/out"))
        joined = " ".join(command)
        self.assertIn("--guest-visual-frames 64", joined)
        self.assertIn("--profile-log", command)
        self.assertIn("--counter-log", command)
        self.assertIn("--visual-hash-log", command)
        self.assertIn("0x045f@1+180", command)
        self.assertIn("--hold-forward", command)
        self.assertIn("--hold-run", command)


if __name__ == "__main__":
    unittest.main()
