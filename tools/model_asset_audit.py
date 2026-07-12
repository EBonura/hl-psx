#!/usr/bin/env python3
"""Strict structural audit for the merged actor and viewmodel assets.

The model pack stores one merged chunk per asset::

    HMRG | u32 geometry_length | HMD4/HMD5/HMD6 geometry | HLTX textures

This tool deliberately mirrors the binary contracts in ``game/src/model.rs``,
``game/src/vram.rs``, and the model writer in ``tools/hl-bsp``.  It validates
all offsets before reading records, then checks the relationships the runtime
trusts: clips, compact frames, triangle indices, and embedded textures.

``ClipRec`` remains four bytes. The low 15 bits of ``u16 first_frame`` select
the first pose and its high bit stores duration bit 8. The following packed
``u16`` stores baked-frame count in its low byte and duration bits 0..7 in its
high byte. Duration uses 100 ms quanta; zero denotes a legacy chunk.

Roster gaps and missing conventional pain/death clips are advisory for now.
Malformed files are structural errors and make the command fail.
"""

from __future__ import annotations

import argparse
import math
import re
import struct
import sys
from collections import Counter
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, Sequence


ROOT = Path(__file__).resolve().parent.parent
DEFAULT_MODELPACK = ROOT / "data" / "modelpack"

# Mirrored roster contracts.  Actor ids are the MODEL_DEFS/collect_props ids in
# game/src/main.rs and Makefile's model roster.  Viewmodel order mirrors
# Makefile's WEAPONLIST and game/src/main.rs N_WEAPONS.
ACTOR_CHUNK_BASE = 1300
ACTOR_COUNT = 53
VIEWMODEL_CHUNK_BASE = 1000
VIEWMODEL_COUNT = 16

ACTOR_NAMES: tuple[str, ...] = (
    "scientist",
    "barney",
    "headcrab",
    "w_suit",
    "w_battery",
    "zombie",
    "houndeye",
    "bullsquid",
    "hgrunt",
    "islave",
    "agrunt",
    "controller",
    "barnacle",
    "leech",
    "roach",
    "gman",
    "garg",
    "nihilanth",
    "big_mom",
    "icky",
    "sentry",
    "turret",
    "miniturret",
    "apache",
    "boid",
    "scientist_sitting",
    "w_crowbar",
    "w_9mmhandgun",
    "w_357",
    "w_9mmAR",
    "w_shotgun",
    "w_crossbow",
    "w_rpg",
    "w_gauss",
    "w_egon",
    "w_hgun",
    "w_grenade",
    "w_squeak",
    "w_satchel",
    "w_satchel_radio",
    "w_9mmclip",
    "w_9mmARclip",
    "w_shotbox",
    "w_357ammobox",
    "w_crossbow_clip",
    "w_rpgammo",
    "w_gaussammo",
    "w_ARgrenade",
    "w_medkit",
    "w_longjump",
    "tentacle2",
    "hassassin",
    "loader",
)

VIEWMODEL_NAMES: tuple[str, ...] = (
    "v_9mmhandgun",
    "v_357",
    "v_9mmar",
    "v_crossbow",
    "v_crowbar",
    "v_chub",
    "v_egon",
    "v_gauss",
    "v_grenade",
    "v_hgun",
    "v_rpg",
    "v_satchel",
    "v_satchel_radio",
    "v_shotgun",
    "v_squeak",
    "v_tripmine",
)

assert len(ACTOR_NAMES) == ACTOR_COUNT
assert len(VIEWMODEL_NAMES) == VIEWMODEL_COUNT

# Mirrored from game/src/main.rs MODEL_DEFS.  These are render/frustum radii,
# not physics hulls.  The audit below derives the minimum conservative radius
# from every cooked frame so an undersized hand-tuned value cannot make a model
# disappear at a view edge.  Each entry includes one world-unit rounding guard.
ACTOR_RENDER_RADII: tuple[int, ...] = (
    138, 77, 36, 70, 36, 90, 70, 91, 90, 90, 100, 100, 170,
    40, 30, 90, 360, 1748, 200, 223, 80, 80, 60, 408, 60, 72,
    36, 36, 36, 36, 36, 56, 45, 51, 40, 45, 36, 36, 36, 36,
    36, 36, 36, 36, 36, 36, 36, 36, 36, 38, 912, 90, 1242,
)
assert len(ACTOR_RENDER_RADII) == ACTOR_COUNT

# Positive-health entries in main.rs MODEL_DEFS.  The renderer requests clip 3
# during hit flash and clip 4 after death for every such type.  Keep this list
# synchronized with MODEL_DEFS until the definitions become generated data.
LIVING_ACTOR_IDS = frozenset((*range(0, 3), *range(5, 26), 50, 51))
PAIN_CLIP_INDEX = 3
DEATH_CLIP_INDEX = 4

# Runtime/cooker contracts, not arbitrary audit policy.
RUNTIME_MAX_MODEL_VERTICES = 1024  # model.rs MAX_VERTS / ten-bit face indices
VALID_MODEL_TEXTURE_DIMS = frozenset((8, 16, 32, 64))  # hl-bsp final_size()

HMRG_MAGIC = b"HMRG"
HLTX_MAGIC = b"HLTX"
COMPACT_MAGICS = frozenset((b"HMD4", b"HMD5", b"HMD6"))
FRAME_MODE_FULL_I16 = 0
FRAME_MODE_BASE_I8 = 1
FRAME_RECORD_BYTES = 8
CLIP_RECORD_BYTES = 4
CLIP_FRAME_COUNT_MASK = 0x00FF
CLIP_FIRST_FRAME_MASK = 0x7FFF
CLIP_DURATION_EXT_BIT = 0x8000
CLIP_DURATION_SHIFT = 8
TRIANGLE_BYTES = {b"HMD4": 16, b"HMD5": 16, b"HMD6": 20}


class FormatError(ValueError):
    """A model chunk violates a binary-format invariant."""


@dataclass(frozen=True)
class ClipRecord:
    first_frame: int
    frame_count: int
    source_duration_100ms: int


@dataclass(frozen=True)
class FrameRecord:
    data_offset: int
    mode: int
    base_frame: int
    data_length: int


@dataclass(frozen=True)
class TextureRecord:
    width: int
    height: int
    payload_bytes: int


@dataclass(frozen=True)
class ParsedModel:
    source: str
    magic: str
    wrapper_bytes: int
    geometry_bytes: int
    texture_chunk_bytes: int
    vertices: int
    triangles: int
    textures: int
    frames: int
    clips: int
    frame_data_bytes: int
    triangle_bytes: int
    local_to_world_q12: int
    minimum_render_radius: int
    flags: int
    clip_records: tuple[ClipRecord, ...]
    frame_records: tuple[FrameRecord, ...]
    texture_records: tuple[TextureRecord, ...]

    @property
    def full_frames(self) -> int:
        return sum(r.mode == FRAME_MODE_FULL_I16 for r in self.frame_records)

    @property
    def delta_frames(self) -> int:
        return sum(r.mode == FRAME_MODE_BASE_I8 for r in self.frame_records)

    @property
    def max_texture_area(self) -> int:
        return max((t.width * t.height for t in self.texture_records), default=0)

    @property
    def max_texture_width(self) -> int:
        return max((t.width for t in self.texture_records), default=0)

    @property
    def max_texture_height(self) -> int:
        return max((t.height for t in self.texture_records), default=0)


@dataclass(frozen=True)
class Asset:
    cohort: str
    asset_id: int
    name: str
    path: Path
    model: ParsedModel

    @property
    def label(self) -> str:
        return f"{self.cohort} {self.asset_id}:{self.name} ({self.path.name})"


@dataclass
class AuditResult:
    actors: list[Asset]
    viewmodels: list[Asset]
    errors: list[str]
    warnings: list[str]
    actor_present: set[int]
    viewmodel_present: set[int]


def _need(data: bytes, offset: int, length: int, what: str) -> None:
    if offset < 0 or length < 0 or offset + length > len(data):
        raise FormatError(
            f"{what} exceeds buffer: offset={offset}, length={length}, size={len(data)}"
        )


def _u16(data: bytes, offset: int) -> int:
    return struct.unpack_from("<H", data, offset)[0]


def _u32(data: bytes, offset: int) -> int:
    return struct.unpack_from("<I", data, offset)[0]


def _parse_hltx(data: bytes, expected_textures: int) -> tuple[TextureRecord, ...]:
    _need(data, 0, 8, "HLTX header")
    if data[:4] != HLTX_MAGIC:
        raise FormatError(
            f"embedded texture magic is {data[:4]!r}, expected {HLTX_MAGIC!r}"
        )
    count = _u32(data, 4)
    if count != expected_textures:
        raise FormatError(
            f"texture count mismatch: HMD declares {expected_textures}, HLTX contains {count}"
        )

    offset = 8
    textures: list[TextureRecord] = []
    for index in range(count):
        _need(data, offset, 36, f"texture {index} header/CLUT")
        width = _u16(data, offset)
        height = _u16(data, offset + 2)
        if (
            width not in VALID_MODEL_TEXTURE_DIMS
            or height not in VALID_MODEL_TEXTURE_DIMS
        ):
            allowed = ", ".join(str(v) for v in sorted(VALID_MODEL_TEXTURE_DIMS))
            raise FormatError(
                f"texture {index} has invalid model dimensions {width}x{height}; "
                f"expected power-of-two dimensions in {{{allowed}}}"
            )
        texels = width * height
        if texels & 1:
            raise FormatError(f"texture {index} has odd 4bpp texel count {texels}")
        payload_bytes = texels // 2
        payload_offset = offset + 36
        _need(data, payload_offset, payload_bytes, f"texture {index} pixel payload")
        textures.append(TextureRecord(width, height, payload_bytes))
        offset = payload_offset + payload_bytes

    if offset != len(data):
        raise FormatError(
            f"HLTX has {len(data) - offset} trailing byte(s) after texture {count}"
        )
    return tuple(textures)


def _parse_geometry(
    data: bytes,
) -> tuple[
    str,
    int,
    int,
    int,
    int,
    int,
    int,
    int,
    int,
    int,
    int,
    tuple[ClipRecord, ...],
    tuple[FrameRecord, ...],
]:
    _need(data, 0, 4, "HMD magic")
    magic = data[:4]
    if magic not in COMPACT_MAGICS:
        raise FormatError(
            f"geometry magic is {magic!r}; merged actor/viewmodel chunks require HMD4/HMD5/HMD6"
        )
    header_bytes = 28 if magic == b"HMD4" else 32
    _need(data, 0, header_bytes, f"{magic.decode()} header")

    vertices = _u32(data, 4)
    triangles = _u32(data, 8)
    textures = _u32(data, 12)
    frames = _u32(data, 16)
    clips = _u32(data, 20)
    frame_data_bytes = _u32(data, 24)
    if vertices == 0:
        raise FormatError("model has zero vertices")
    if vertices > RUNTIME_MAX_MODEL_VERTICES:
        raise FormatError(
            f"model has {vertices} vertices, above runtime limit {RUNTIME_MAX_MODEL_VERTICES}"
        )
    if frames == 0:
        raise FormatError(
            "model has zero frames (runtime would manufacture an invalid frame 0)"
        )
    if frames > 0x10000:
        raise FormatError(
            f"model has {frames} frames, but frame base references are u16"
        )
    if clips == 0:
        raise FormatError(
            "model has zero clips (runtime would manufacture an invalid clip 0)"
        )

    local_to_world_q12 = _u16(data, 28) if header_bytes == 32 else 4096
    flags = _u16(data, 30) if header_bytes == 32 else 0

    clips_offset = header_bytes
    clips_bytes = clips * CLIP_RECORD_BYTES
    _need(data, clips_offset, clips_bytes, "clip table")
    clip_records: list[ClipRecord] = []
    for index in range(clips):
        offset = clips_offset + index * CLIP_RECORD_BYTES
        packed_first = _u16(data, offset)
        first = packed_first & CLIP_FIRST_FRAME_MASK
        packed = _u16(data, offset + 2)
        count = packed & CLIP_FRAME_COUNT_MASK
        source_duration_100ms = (packed >> CLIP_DURATION_SHIFT) | (
            0x100 if packed_first & CLIP_DURATION_EXT_BIT else 0
        )
        if count == 0:
            raise FormatError(f"clip {index} has zero frames")
        if first >= frames or first + count > frames:
            raise FormatError(
                f"clip {index} range [{first}, {first + count}) exceeds {frames} frames"
            )
        clip_records.append(ClipRecord(first, count, source_duration_100ms))

    frame_table_offset = clips_offset + clips_bytes
    frame_table_bytes = frames * FRAME_RECORD_BYTES
    _need(data, frame_table_offset, frame_table_bytes, "frame descriptor table")

    raw_frames: list[tuple[int, int, int, int]] = []
    expected_data_offset = 0
    for index in range(frames):
        offset = frame_table_offset + index * FRAME_RECORD_BYTES
        data_offset = _u32(data, offset)
        mode = data[offset + 4]
        base_frame = _u16(data, offset + 5)
        reserved = data[offset + 7]
        if reserved != 0:
            raise FormatError(f"frame {index} reserved byte is {reserved}, expected 0")
        if mode == FRAME_MODE_FULL_I16:
            data_length = vertices * 6
            if base_frame != 0:
                raise FormatError(
                    f"full frame {index} has ignored/nonzero base reference {base_frame}"
                )
        elif mode == FRAME_MODE_BASE_I8:
            data_length = vertices * 3
            if base_frame >= frames:
                raise FormatError(
                    f"delta frame {index} base {base_frame} exceeds {frames} frames"
                )
        else:
            raise FormatError(f"frame {index} has unsupported compact mode {mode}")

        # hl-bsp writes a packed frame-data stream.  Requiring exact sequential
        # spans catches overlap, holes, stale offsets, and an inflated length.
        if data_offset != expected_data_offset:
            raise FormatError(
                f"frame {index} data starts at {data_offset}, expected packed offset "
                f"{expected_data_offset}"
            )
        data_end = data_offset + data_length
        if data_end > frame_data_bytes:
            raise FormatError(
                f"frame {index} data span [{data_offset}, {data_end}) exceeds "
                f"frame_data_len {frame_data_bytes}"
            )
        raw_frames.append((data_offset, mode, base_frame, data_length))
        expected_data_offset = data_end

    if expected_data_offset != frame_data_bytes:
        raise FormatError(
            f"frame data descriptors consume {expected_data_offset} bytes, "
            f"header declares {frame_data_bytes}"
        )
    for index, (_, mode, base_frame, _) in enumerate(raw_frames):
        if (
            mode == FRAME_MODE_BASE_I8
            and raw_frames[base_frame][1] != FRAME_MODE_FULL_I16
        ):
            raise FormatError(
                f"delta frame {index} references frame {base_frame}, which is not a full i16 base"
            )

    frame_records = tuple(FrameRecord(*record) for record in raw_frames)
    frame_data_offset = frame_table_offset + frame_table_bytes
    _need(data, frame_data_offset, frame_data_bytes, "frame data")

    # Decode every authored endpoint exactly as ModelFrame::vert does. Delta
    # additions and interpolation both wrap as i16 at runtime.
    def wrap_i16(value: int) -> int:
        return ((value + 32768) & 0xFFFF) - 32768

    decoded_frames: list[list[tuple[int, int, int]]] = []
    max_norm_sq = 0
    for record in frame_records:
        frame_offset = frame_data_offset + record.data_offset
        decoded: list[tuple[int, int, int]] = []
        if record.mode == FRAME_MODE_FULL_I16:
            for vertex in range(vertices):
                xyz = struct.unpack_from("<hhh", data, frame_offset + vertex * 6)
                decoded.append(xyz)
        else:
            # Runtime resolves the referenced full-frame descriptor directly;
            # the base may legally appear after this delta in the packed stream.
            base_record = frame_records[record.base_frame]
            base_offset = frame_data_offset + base_record.data_offset
            for vertex in range(vertices):
                base = struct.unpack_from("<hhh", data, base_offset + vertex * 6)
                delta = struct.unpack_from("<bbb", data, frame_offset + vertex * 3)
                xyz = (
                    wrap_i16(base[0] + delta[0]),
                    wrap_i16(base[1] + delta[1]),
                    wrap_i16(base[2] + delta[2]),
                )
                decoded.append(xyz)
        decoded_frames.append(decoded)
        for xyz in decoded:
            max_norm_sq = max(
                max_norm_sq,
                sum(component * component for component in xyz),
            )

    # Component-wise signed shifts can round an interpolated vertex just
    # outside both endpoint norms. Enumerate every fraction the runtime accepts
    # for each consecutive clip pair (including the loop-back pair), so the
    # radius proof covers the actual integer decoder rather than ideal lerp.
    for clip in clip_records:
        for local_frame in range(clip.frame_count):
            a = decoded_frames[clip.first_frame + local_frame]
            b = decoded_frames[
                clip.first_frame + ((local_frame + 1) % clip.frame_count)
            ]
            for frac16 in range(1, 16):
                for av, bv in zip(a, b):
                    xyz = tuple(
                        wrap_i16(
                            av[axis]
                            + (((bv[axis] - av[axis]) * frac16) >> 4)
                        )
                        for axis in range(3)
                    )
                    max_norm_sq = max(
                        max_norm_sq,
                        sum(component * component for component in xyz),
                    )

    # Mirror main.rs model_local_scale exactly. The projection path inflates
    # both vertices and translation by this reciprocal integer, so the model's
    # world-space radius is raw_radius / local_scale.
    scale_q12 = local_to_world_q12 or 4096
    local_scale = max(4096 // scale_q12, 1)
    exact_ceil = math.isqrt(max_norm_sq) // local_scale
    if (exact_ceil * local_scale) ** 2 < max_norm_sq:
        exact_ceil += 1
    minimum_render_radius = exact_ceil + 1

    triangle_bytes = TRIANGLE_BYTES[magic]
    triangles_offset = frame_data_offset + frame_data_bytes
    triangle_section_bytes = triangles * triangle_bytes
    _need(data, triangles_offset, triangle_section_bytes, "triangle section")
    triangle_end = triangles_offset + triangle_section_bytes
    if triangle_end != len(data):
        raise FormatError(
            f"geometry has {len(data) - triangle_end} trailing byte(s) after triangle section"
        )

    for index in range(triangles):
        offset = triangles_offset + index * triangle_bytes
        indices = (_u16(data, offset), _u16(data, offset + 2), _u16(data, offset + 4))
        for corner, vertex in enumerate(indices):
            if vertex >= vertices:
                raise FormatError(
                    f"triangle {index} corner {corner} references vertex {vertex} of {vertices}"
                )
        texture = _u16(data, offset + 6)
        if texture >= textures:
            raise FormatError(
                f"triangle {index} references texture {texture} of {textures}"
            )
        padding_offset = offset + (18 if magic == b"HMD6" else 14)
        if _u16(data, padding_offset) != 0:
            raise FormatError(f"triangle {index} reserved padding is nonzero")

    return (
        magic.decode("ascii"),
        vertices,
        triangles,
        textures,
        frames,
        clips,
        frame_data_bytes,
        triangle_bytes,
        local_to_world_q12,
        minimum_render_radius,
        flags,
        tuple(clip_records),
        frame_records,
    )


def parse_hmrg_bytes(data: bytes, source: str = "<memory>") -> ParsedModel:
    """Parse and validate one complete HMRG chunk."""

    _need(data, 0, 8, "HMRG wrapper")
    if data[:4] != HMRG_MAGIC:
        raise FormatError(f"wrapper magic is {data[:4]!r}, expected {HMRG_MAGIC!r}")
    geometry_bytes = _u32(data, 4)
    if geometry_bytes == 0:
        raise FormatError("HMRG geometry length is zero")
    geometry_end = 8 + geometry_bytes
    _need(data, 8, geometry_bytes, "HMRG geometry payload")
    geometry = data[8:geometry_end]
    texture_chunk = data[geometry_end:]

    (
        magic,
        vertices,
        triangles,
        textures,
        frames,
        clips,
        frame_data_bytes,
        triangle_bytes,
        local_to_world_q12,
        minimum_render_radius,
        flags,
        clip_records,
        frame_records,
    ) = _parse_geometry(geometry)
    texture_records = _parse_hltx(texture_chunk, textures)

    return ParsedModel(
        source=source,
        magic=magic,
        wrapper_bytes=len(data),
        geometry_bytes=geometry_bytes,
        texture_chunk_bytes=len(texture_chunk),
        vertices=vertices,
        triangles=triangles,
        textures=textures,
        frames=frames,
        clips=clips,
        frame_data_bytes=frame_data_bytes,
        triangle_bytes=triangle_bytes,
        local_to_world_q12=local_to_world_q12,
        minimum_render_radius=minimum_render_radius,
        flags=flags,
        clip_records=clip_records,
        frame_records=frame_records,
        texture_records=texture_records,
    )


def parse_hmrg_file(path: Path) -> ParsedModel:
    try:
        data = path.read_bytes()
    except OSError as exc:
        raise FormatError(f"cannot read file: {exc}") from exc
    return parse_hmrg_bytes(data, str(path))


_CHUNK_RE = re.compile(r"^chunk_(\d+)\.psxm$")


def _present_ids(modelpack: Path, base: int, discovery_count: int = 100) -> set[int]:
    present: set[int] = set()
    try:
        entries = modelpack.iterdir()
    except OSError:
        return present
    for path in entries:
        match = _CHUNK_RE.match(path.name)
        if not match:
            continue
        chunk = int(match.group(1))
        if base <= chunk < base + discovery_count:
            present.add(chunk - base)
    return present


def _coverage_warning(
    cohort: str,
    present: set[int],
    names: Sequence[str],
) -> str | None:
    expected = set(range(len(names)))
    missing = sorted(expected - present)
    extra = sorted(present - expected)
    if not missing and not extra:
        return None
    parts: list[str] = []
    if missing:
        parts.append("missing " + ", ".join(f"{i}:{names[i]}" for i in missing))
    if extra:
        parts.append("unexpected ids " + ", ".join(str(i) for i in extra))
    return (
        f"{cohort} roster coverage is {len(expected & present)}/{len(expected)}: "
        + "; ".join(parts)
    )


def audit_modelpack(modelpack: Path) -> AuditResult:
    errors: list[str] = []
    warnings: list[str] = []
    actors: list[Asset] = []
    viewmodels: list[Asset] = []

    actor_present = _present_ids(modelpack, ACTOR_CHUNK_BASE)
    viewmodel_present = _present_ids(modelpack, VIEWMODEL_CHUNK_BASE)
    for warning in (
        _coverage_warning("actor", actor_present, ACTOR_NAMES),
        _coverage_warning("viewmodel", viewmodel_present, VIEWMODEL_NAMES),
    ):
        if warning:
            warnings.append(warning)

    for cohort, base, names, output in (
        ("actor", ACTOR_CHUNK_BASE, ACTOR_NAMES, actors),
        ("viewmodel", VIEWMODEL_CHUNK_BASE, VIEWMODEL_NAMES, viewmodels),
    ):
        for asset_id, name in enumerate(names):
            path = modelpack / f"chunk_{base + asset_id}.psxm"
            if not path.is_file():
                continue  # Coverage warning above is intentionally non-fatal.
            try:
                model = parse_hmrg_file(path)
            except FormatError as exc:
                errors.append(f"{cohort} {asset_id}:{name} ({path.name}): {exc}")
                continue
            output.append(Asset(cohort, asset_id, name, path, model))

    for asset in actors:
        configured_radius = ACTOR_RENDER_RADII[asset.asset_id]
        if configured_radius < asset.model.minimum_render_radius:
            errors.append(
                f"{asset.label}: configured render radius {configured_radius} is below "
                f"cooked-frame minimum {asset.model.minimum_render_radius}"
            )
        if asset.asset_id not in LIVING_ACTOR_IDS:
            continue
        missing: list[str] = []
        if asset.model.clips <= PAIN_CLIP_INDEX:
            missing.append(f"pain[{PAIN_CLIP_INDEX}]")
        if asset.model.clips <= DEATH_CLIP_INDEX:
            missing.append(f"death[{DEATH_CLIP_INDEX}]")
        if missing:
            warnings.append(
                f"{asset.label}: {asset.model.clips} clips; runtime falls back for "
                + ", ".join(missing)
            )

    return AuditResult(
        actors=actors,
        viewmodels=viewmodels,
        errors=errors,
        warnings=warnings,
        actor_present=actor_present,
        viewmodel_present=viewmodel_present,
    )


def _max_line(assets: Sequence[Asset], attribute: str, label: str) -> str:
    if not assets:
        return f"    {label:<18} n/a"
    asset = max(assets, key=lambda a: getattr(a.model, attribute))
    value = getattr(asset.model, attribute)
    return f"    {label:<18} {value:>8,}  {asset.label}"


def _print_cohort(name: str, assets: Sequence[Asset], expected: int) -> None:
    formats = Counter(a.model.magic for a in assets)
    full = sum(a.model.full_frames for a in assets)
    delta = sum(a.model.delta_frames for a in assets)
    texture_records = sum(a.model.textures for a in assets)
    fmt = ", ".join(f"{key}={formats[key]}" for key in sorted(formats)) or "none"
    print(
        f"{name}: parsed {len(assets)}/{expected}; formats {fmt}; "
        f"frames full/delta={full}/{delta}; textures={texture_records}"
    )
    for attribute, label in (
        ("wrapper_bytes", "wrapper bytes"),
        ("geometry_bytes", "geometry bytes"),
        ("texture_chunk_bytes", "HLTX bytes"),
        ("vertices", "vertices"),
        ("triangles", "triangles"),
        ("textures", "textures"),
        ("frames", "frames"),
        ("clips", "clips"),
        ("frame_data_bytes", "frame data bytes"),
        ("minimum_render_radius", "minimum radius"),
        ("max_texture_width", "max texture width"),
        ("max_texture_height", "max texture height"),
        ("max_texture_area", "max texture texels"),
    ):
        print(_max_line(assets, attribute, label))


def print_report(result: AuditResult, modelpack: Path) -> None:
    actor_expected = set(range(ACTOR_COUNT))
    viewmodel_expected = set(range(VIEWMODEL_COUNT))
    print(f"model asset audit: {modelpack}")
    print(
        f"roster coverage: actors {len(actor_expected & result.actor_present)}/{ACTOR_COUNT}, "
        f"viewmodels {len(viewmodel_expected & result.viewmodel_present)}/{VIEWMODEL_COUNT}"
    )
    _print_cohort("actors", result.actors, ACTOR_COUNT)
    _print_cohort("viewmodels", result.viewmodels, VIEWMODEL_COUNT)

    if result.warnings:
        print(f"advisories ({len(result.warnings)}, non-fatal):")
        for warning in result.warnings:
            print(f"  WARN: {warning}")
    else:
        print("advisories: none")

    if result.errors:
        print(f"structural errors ({len(result.errors)}):")
        for error in result.errors:
            print(f"  ERROR: {error}")
        print("FAIL: model asset structure is invalid")
    else:
        print(
            f"PASS: {len(result.actors) + len(result.viewmodels)} HMRG chunks are structurally valid"
        )


def _build_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--modelpack",
        type=Path,
        default=DEFAULT_MODELPACK,
        help=f"directory containing chunk_*.psxm (default: {DEFAULT_MODELPACK})",
    )
    return parser


def main(argv: Iterable[str] | None = None) -> int:
    args = _build_arg_parser().parse_args(list(argv) if argv is not None else None)
    result = audit_modelpack(args.modelpack)
    print_report(result, args.modelpack)
    return 1 if result.errors else 0


if __name__ == "__main__":
    sys.exit(main())
