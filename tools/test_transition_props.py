from __future__ import annotations

import unittest

from gen_transition_props import (
    TransitionProp,
    actor_type,
    decompress_vis,
    landmark_transform,
    parse_vec3,
)


class TransitionPropTests(unittest.TestCase):
    def test_manifest_line_is_cooker_stable(self) -> None:
        prop = TransitionProp(
            destination="c0a0e",
            actor_type=1,
            targetname="barney1",
            origin=(-2017.0, -458.0, -221.0),
            yaw=90.0,
            source="c0a0d",
        )
        self.assertEqual(
            prop.line(),
            "c0a0e|1|barney1|-2017|-458|-221|90|c0a0d",
        )

    def test_vec3_rejects_wrong_arity(self) -> None:
        self.assertEqual(parse_vec3("1 2 3"), (1.0, 2.0, 3.0))
        self.assertIsNone(parse_vec3("1 2"))

    def test_landmark_transform_preserves_relative_offset(self) -> None:
        self.assertEqual(
            landmark_transform((12.0, 23.0, 34.0), (10.0, 20.0, 30.0), (100.0, 200.0, 300.0)),
            (102.0, 203.0, 304.0),
        )

    def test_visibility_rle_decompression(self) -> None:
        self.assertEqual(decompress_vis(bytes((0x05, 0, 2, 0x80)), 0, 4), b"\x05\0\0\x80")
        self.assertEqual(decompress_vis(b"", -1, 3), b"\xff\xff\xff")

    def test_loader_generic_resolves_by_normalized_model_path(self) -> None:
        self.assertEqual(
            actor_type(
                {
                    "classname": "monster_generic",
                    "model": r"models\loader.mdl",
                }
            ),
            52,
        )
        self.assertIsNone(
            actor_type({"classname": "monster_generic", "model": "models/other.mdl"})
        )

    def test_opening_generic_models_reuse_scientist_and_add_forklift(self) -> None:
        self.assertEqual(
            actor_type({"classname": "monster_generic", "model": "models/scientist.mdl"}),
            0,
        )
        self.assertEqual(
            actor_type({"classname": "monster_generic", "model": r"models\forklift.mdl"}),
            53,
        )


if __name__ == "__main__":
    unittest.main()
