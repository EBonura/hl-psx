#!/usr/bin/env python3
"""Focused format tests for tools/model_asset_audit.py."""

from __future__ import annotations

import struct
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import model_asset_audit as audit  # noqa: E402


def _hltx(dimensions: tuple[tuple[int, int], ...]) -> bytes:
    out = bytearray(b"HLTX" + struct.pack("<I", len(dimensions)))
    for width, height in dimensions:
        out += struct.pack("<HH", width, height)
        out += bytes(32)  # 16-entry BGR555 CLUT
        out += bytes(width * height // 2)
    return bytes(out)


def _chunk(
    magic: bytes = b"HMD5",
    *,
    vertices: int = 3,
    frame_specs: tuple[tuple[int, int], ...] = ((0, 0), (1, 0)),
    frame_offsets: tuple[int, ...] | None = None,
    frame_payloads: tuple[bytes, ...] | None = None,
    clips: tuple[tuple[int, int], ...] = ((0, 2),),
    clip_durations_100ms: tuple[int, ...] | None = None,
    triangle_indices: tuple[int, int, int] = (0, 1, 2),
    triangle_texture: int = 0,
    declared_textures: int = 1,
    texture_dimensions: tuple[tuple[int, int], ...] = ((8, 8),),
    geometry_trailer: bytes = b"",
) -> bytes:
    if frame_offsets is not None and len(frame_offsets) != len(frame_specs):
        raise ValueError("frame_offsets length must match frame_specs")
    if frame_payloads is not None and len(frame_payloads) != len(frame_specs):
        raise ValueError("frame_payloads length must match frame_specs")
    if (
        clip_durations_100ms is not None
        and len(clip_durations_100ms) != len(clips)
    ):
        raise ValueError("clip_durations_100ms length must match clips")

    frame_data = bytearray()
    descriptors = bytearray()
    for index, (mode, base) in enumerate(frame_specs):
        offset = len(frame_data) if frame_offsets is None else frame_offsets[index]
        descriptors += struct.pack("<IBHB", offset, mode, base, 0)
        payload = (
            bytes(vertices * (3 if mode == 1 else 6))
            if frame_payloads is None
            else frame_payloads[index]
        )
        expected = vertices * (3 if mode == 1 else 6)
        if len(payload) != expected:
            raise ValueError(f"frame {index} payload is {len(payload)} bytes, expected {expected}")
        frame_data += payload

    geometry = bytearray(
        struct.pack(
            "<4s6I",
            magic,
            vertices,
            1,
            declared_textures,
            len(frame_specs),
            len(clips),
            len(frame_data),
        )
    )
    if magic in (b"HMD5", b"HMD6"):
        geometry += struct.pack("<HH", 4096, 0)
    for index, (first, count) in enumerate(clips):
        duration = 0 if clip_durations_100ms is None else clip_durations_100ms[index]
        if first & ~audit.CLIP_FIRST_FRAME_MASK:
            raise ValueError("clip first frame exceeds packed format")
        if duration > 0x1FF:
            raise ValueError("clip duration exceeds packed format")
        packed_first = first | (
            audit.CLIP_DURATION_EXT_BIT if duration & 0x100 else 0
        )
        packed = (count & audit.CLIP_FRAME_COUNT_MASK) | (
            (duration & 0xFF) << audit.CLIP_DURATION_SHIFT
        )
        geometry += struct.pack("<HH", packed_first, packed)
    geometry += descriptors
    geometry += frame_data
    geometry += struct.pack(
        "<4H6B", *triangle_indices, triangle_texture, 0, 0, 1, 1, 2, 2
    )
    if magic == b"HMD6":
        geometry += struct.pack("<bbbBH", 1, -2, 3, 0, 0)
    else:
        geometry += struct.pack("<H", 0)
    geometry += geometry_trailer

    textures = _hltx(texture_dimensions)
    return b"HMRG" + struct.pack("<I", len(geometry)) + geometry + textures


class ModelAssetFormatTests(unittest.TestCase):
    def assert_format_error(self, data: bytes, text: str) -> None:
        with self.assertRaisesRegex(audit.FormatError, text):
            audit.parse_hmrg_bytes(data)

    def test_accepts_hmd4_hmd5_and_hmd6(self) -> None:
        for magic, triangle_bytes in ((b"HMD4", 16), (b"HMD5", 16), (b"HMD6", 20)):
            with self.subTest(magic=magic):
                parsed = audit.parse_hmrg_bytes(_chunk(magic))
                self.assertEqual(parsed.magic, magic.decode())
                self.assertEqual(parsed.frames, 2)
                self.assertEqual(parsed.full_frames, 1)
                self.assertEqual(parsed.delta_frames, 1)
                self.assertEqual(parsed.triangle_bytes, triangle_bytes)

    def test_rejects_wrapper_overrun(self) -> None:
        data = bytearray(_chunk())
        struct.pack_into("<I", data, 4, len(data))
        self.assert_format_error(bytes(data), "HMRG geometry payload exceeds buffer")

    def test_rejects_clip_out_of_frame_bounds(self) -> None:
        self.assert_format_error(_chunk(clips=((1, 2),)), "clip 0 range")

    def test_decodes_packed_clip_frame_count_and_source_duration(self) -> None:
        parsed = audit.parse_hmrg_bytes(
            _chunk(clips=((0, 2),), clip_durations_100ms=(38,))
        )
        self.assertEqual(parsed.clip_records[0].frame_count, 2)
        self.assertEqual(parsed.clip_records[0].source_duration_100ms, 38)

    def test_decodes_extended_source_duration_from_first_frame_high_bit(self) -> None:
        parsed = audit.parse_hmrg_bytes(
            _chunk(clips=((0, 2),), clip_durations_100ms=(334,))
        )
        self.assertEqual(parsed.clip_records[0].first_frame, 0)
        self.assertEqual(parsed.clip_records[0].frame_count, 2)
        self.assertEqual(parsed.clip_records[0].source_duration_100ms, 334)

    def test_rejects_zero_low_byte_even_with_packed_source_duration(self) -> None:
        self.assert_format_error(
            _chunk(clips=((0, 0),), clip_durations_100ms=(38,)),
            "clip 0 has zero frames",
        )

    def test_rejects_unknown_frame_mode(self) -> None:
        self.assert_format_error(
            _chunk(frame_specs=((0, 0), (2, 0))), "unsupported compact mode 2"
        )

    def test_rejects_delta_base_that_is_not_full(self) -> None:
        self.assert_format_error(
            _chunk(frame_specs=((0, 0), (1, 1))), "which is not a full i16 base"
        )

    def test_rejects_frame_data_holes_or_overlap(self) -> None:
        self.assert_format_error(
            _chunk(frame_offsets=(0, 17)), "expected packed offset 18"
        )

    def test_rejects_triangle_vertex_and_texture_indices(self) -> None:
        self.assert_format_error(
            _chunk(triangle_indices=(0, 1, 3)), "references vertex 3 of 3"
        )
        self.assert_format_error(
            _chunk(triangle_texture=1), "references texture 1 of 1"
        )

    def test_rejects_geometry_trailer(self) -> None:
        self.assert_format_error(_chunk(geometry_trailer=b"x"), "trailing byte")

    def test_rejects_hltx_count_dimensions_and_trailer(self) -> None:
        self.assert_format_error(_chunk(declared_textures=2), "texture count mismatch")
        self.assert_format_error(
            _chunk(texture_dimensions=((10, 8),)), "invalid model dimensions 10x8"
        )
        self.assert_format_error(_chunk() + b"x", "HLTX has 1 trailing byte")

    def test_roster_and_clip_findings_are_nonfatal(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            modelpack = Path(directory)
            (modelpack / "chunk_1300.psxm").write_bytes(
                _chunk(frame_specs=((0, 0),), clips=((0, 1),))
            )
            result = audit.audit_modelpack(modelpack)
        self.assertFalse(result.errors)
        self.assertTrue(any("actor roster coverage" in w for w in result.warnings))
        self.assertTrue(
            any("pain[3]" in w and "death[4]" in w for w in result.warnings)
        )

    def test_rejects_actor_radius_smaller_than_cooked_frames(self) -> None:
        far_vertex = struct.pack("<hhh", 200, 0, 0)
        payload = far_vertex * 3
        with tempfile.TemporaryDirectory() as directory:
            modelpack = Path(directory)
            (modelpack / "chunk_1300.psxm").write_bytes(
                _chunk(
                    frame_specs=((0, 0),),
                    frame_payloads=(payload,),
                    clips=((0, 1),),
                )
            )
            result = audit.audit_modelpack(modelpack)
        self.assertTrue(
            any(
                "configured render radius 138" in error
                and "cooked-frame minimum 201" in error
                for error in result.errors
            )
        )

    def test_radius_decode_accepts_delta_before_its_full_base(self) -> None:
        delta = struct.pack("<bbb", 1, 0, 0) * 3
        base = struct.pack("<hhh", 100, 0, 0) * 3
        parsed = audit.parse_hmrg_bytes(
            _chunk(
                frame_specs=((1, 1), (0, 0)),
                frame_payloads=(delta, base),
                clips=((0, 2),),
            )
        )
        self.assertEqual(parsed.minimum_render_radius, 102)

    def test_radius_covers_integer_interpolation_overshoot(self) -> None:
        # At frac=1 the signed right shifts produce (-20,-20,-19), whose
        # ceil(norm) is 35 even though both endpoints ceil to only 34.
        a = struct.pack("<hhh", -20, -20, -18) * 3
        b = struct.pack("<hhh", -20, -19, -19) * 3
        parsed = audit.parse_hmrg_bytes(
            _chunk(
                frame_specs=((0, 0), (0, 0)),
                frame_payloads=(a, b),
                clips=((0, 2),),
            )
        )
        self.assertEqual(parsed.minimum_render_radius, 36)


if __name__ == "__main__":
    unittest.main()
