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
    clips: tuple[tuple[int, int], ...] = ((0, 2),),
    triangle_indices: tuple[int, int, int] = (0, 1, 2),
    triangle_texture: int = 0,
    declared_textures: int = 1,
    texture_dimensions: tuple[tuple[int, int], ...] = ((8, 8),),
    geometry_trailer: bytes = b"",
) -> bytes:
    if frame_offsets is not None and len(frame_offsets) != len(frame_specs):
        raise ValueError("frame_offsets length must match frame_specs")

    frame_data = bytearray()
    descriptors = bytearray()
    for index, (mode, base) in enumerate(frame_specs):
        offset = len(frame_data) if frame_offsets is None else frame_offsets[index]
        descriptors += struct.pack("<IBHB", offset, mode, base, 0)
        frame_data += bytes(vertices * (3 if mode == 1 else 6))

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
    geometry += b"".join(struct.pack("<HH", first, count) for first, count in clips)
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


if __name__ == "__main__":
    unittest.main()
