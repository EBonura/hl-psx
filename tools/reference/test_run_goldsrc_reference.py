#!/usr/bin/env python3

from __future__ import annotations

import argparse
import math
import unittest

from run_goldsrc_reference import entity_interval, format_vec3, parser


class GoldSrcReferenceRunnerTests(unittest.TestCase):
    def test_entity_interval_is_bounded_and_defaults_to_one_second(self) -> None:
        args = parser().parse_args(
            [
                "--runtime-dir",
                "/tmp/runtime",
                "--half-life-dir",
                "/tmp/half-life",
                "--output",
                "/tmp/reference.trace",
            ]
        )
        self.assertEqual(args.entity_interval, 20)
        self.assertEqual(entity_interval("1"), 1)
        self.assertEqual(entity_interval("1000"), 1000)
        for invalid in ("0", "1001", "nope"):
            with self.subTest(invalid=invalid), self.assertRaises(
                argparse.ArgumentTypeError
            ):
                entity_interval(invalid)

    def test_checkpoint_vectors_parse_as_three_values(self) -> None:
        args = parser().parse_args(
            [
                "--runtime-dir",
                "/tmp/runtime",
                "--half-life-dir",
                "/tmp/half-life",
                "--output",
                "/tmp/reference.trace",
                "--initial-origin",
                "1390",
                "-230",
                "-129",
                "--initial-angles",
                "0",
                "-90",
                "0",
            ]
        )
        self.assertEqual(args.initial_origin, [1390.0, -230.0, -129.0])
        self.assertEqual(args.initial_angles, [0.0, -90.0, 0.0])

    def test_checkpoint_vector_has_stable_float32_friendly_text(self) -> None:
        self.assertEqual(
            format_vec3([1390.0, -230.0, -129.0], "--initial-origin"),
            "1390 -230 -129",
        )
        self.assertEqual(
            format_vec3([1.23456789, -0.0, 1e-6], "--initial-origin"),
            "1.23456789 -0 1e-06",
        )

    def test_checkpoint_vector_rejects_non_finite_or_extreme_values(self) -> None:
        for values in ([math.nan, 0.0, 0.0], [math.inf, 0.0, 0.0], [1_000_001.0, 0.0, 0.0]):
            with self.subTest(values=values), self.assertRaises(SystemExit):
                format_vec3(values, "--initial-origin")


if __name__ == "__main__":
    unittest.main()
