#!/usr/bin/env python3
"""Focused tests for the campaign map size report's cook contract."""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path
from unittest import mock

from tools import campaign_map_report as report


class CampaignMapReportTests(unittest.TestCase):
    def test_cook_matches_rooms_manifest_environment(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            bsp = root / "c1a0.bsp"
            bsp.write_bytes(b"bsp")
            clips = root / "clips.txt"
            voices = root / "voices.txt"
            sprites = root / "sprites.txt"

            def fake_run(command: list[str], **kwargs: object) -> None:
                self.assertEqual(command[:3], ["hl-bsp", "--cook", str(bsp)])
                Path(command[3]).write_bytes(b"world")
                Path(command[4]).write_bytes(b"texture")
                env = kwargs["env"]
                self.assertEqual(env["CLIPS_MANIFEST"], str(clips))
                self.assertEqual(env["VOICES_MANIFEST"], str(voices))
                self.assertEqual(env["SPRITES_MANIFEST"], str(sprites))
                self.assertEqual(env["MAP_INDEX"], "37")

            with mock.patch.object(report.subprocess, "run", side_effect=fake_run):
                sizes = report.cook_size(
                    Path("hl-bsp"),
                    bsp,
                    root,
                    37,
                    clips,
                    voices,
                    sprites,
                )

        self.assertEqual(sizes, (5, 7))


if __name__ == "__main__":
    unittest.main()
