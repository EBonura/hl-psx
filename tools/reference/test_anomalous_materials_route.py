#!/usr/bin/env python3

import tempfile
import unittest
from pathlib import Path

from anomalous_materials_route import build_route
from hlinput import (
    ACTION_JUMP,
    ACTION_USE,
    InputSample,
    decode,
    encode,
    read_route,
    write_route,
)


class AnomalousMaterialsRouteTests(unittest.TestCase):
    def test_route_shape_and_milestone_ticks_are_stable(self) -> None:
        route = build_route()
        route.validate()
        self.assertEqual(
            [segment.map for segment in route.segments], ["c1a0", "c1a0d", "c1a0a"]
        )
        self.assertEqual(
            [segment.next_map for segment in route.segments], ["c1a0d", "c1a0a", ""]
        )
        self.assertEqual([segment.total_ticks for segment in route.segments], [616, 1641, 40])
        self.assertEqual([len(segment.runs) for segment in route.segments], [14, 69, 1])
        self.assertEqual(decode(encode(route)), route)

    def test_required_interactions_are_not_lost(self) -> None:
        c1a0, c1a0d, c1a0a = (segment.expand() for segment in build_route().segments)
        self.assertEqual(sum(bool(sample.actions & ACTION_USE) for sample in c1a0), 3)
        self.assertEqual(sum(bool(sample.actions & ACTION_USE) for sample in c1a0d), 3)
        self.assertEqual(sum(bool(sample.actions & ACTION_JUMP) for sample in c1a0d), 60)

        aimed = [sample for sample in c1a0d if sample.turn or sample.look]
        self.assertEqual(len(aimed), 10)
        self.assertEqual(sum(sample.turn for sample in aimed), 0)
        self.assertEqual(sum(sample.look for sample in aimed), 0)
        self.assertTrue(all(sample == InputSample() for sample in c1a0a))

    def test_generator_output_is_a_readable_tape(self) -> None:
        route = build_route()
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "anomalous-materials-through-airlock.hlinput"
            write_route(output, route)
            self.assertEqual(read_route(output), route)


if __name__ == "__main__":
    unittest.main()
