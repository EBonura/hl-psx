#!/usr/bin/env python3
"""Find animation type hints for actors supplied by a level transition.

GoldSrc carries eligible named monsters through a landmark. hl-psx streams a
fresh per-map actor/model set, so a destination script can name an actor absent
from its own BSP. This pass propagates actor identity/type across the transition
graph to a fixed point (needed by multi-hop BigMomma), then writes type hints for
script clip lookup. It never creates a prop; runtime carry owns actor state.
"""

from __future__ import annotations

import argparse
import re
import struct
from dataclasses import dataclass
from pathlib import Path

PAIR_RE = re.compile(r'"([^"]*)"\s*"([^"]*)"')
BLOCK_RE = re.compile(r"\{([^}]*)\}", re.DOTALL)

LUMP_PLANES = 1
LUMP_VISIBILITY = 4
LUMP_NODES = 5
LUMP_LEAVES = 10
LUMP_MODELS = 14

ACTOR_TYPES = {
    "monster_scientist": 0,
    "monster_barney": 1,
    "monster_headcrab": 2,
    "monster_zombie": 5,
    "monster_houndeye": 6,
    "monster_bullchicken": 7,
    "monster_human_grunt": 8,
    "monster_alien_slave": 9,
    "monster_alien_grunt": 10,
    "monster_alien_controller": 11,
    "monster_barnacle": 12,
    "monster_leech": 13,
    "monster_cockroach": 14,
    "monster_gman": 15,
    "monster_gargantua": 16,
    "monster_nihilanth": 17,
    "monster_bigmomma": 18,
    "monster_ichthyosaur": 19,
    "monster_sentry": 20,
    "monster_turret": 21,
    "monster_miniturret": 22,
    "monster_apache": 23,
    "monster_flyer_flock": 24,
    "monster_sitting_scientist": 25,
    "monster_tentacle": 50,
    "monster_human_assassin": 51,
}


def actor_type(entity: dict[str, str]) -> int | None:
    """Resolve classname actors plus allowlisted model-selected generics."""
    actor = ACTOR_TYPES.get(entity.get("classname", ""))
    if actor is not None:
        return actor
    if entity.get("classname") != "monster_generic":
        return None
    model = entity.get("model", "").replace("\\", "/").rsplit("/", 1)[-1].lower()
    return 52 if model == "loader.mdl" else None

# GoldSrc collision hulls in native HL axes (x, y, z-up). These are transition
# boxes, not render radii; the distinction is required for BigMomma/sentries.
ACTOR_HULLS = {
    2: ((-12, -12, 0), (12, 12, 24)),
    6: ((-16, -16, 0), (16, 16, 36)),
    7: ((-32, -32, 0), (32, 32, 64)),
    10: ((-32, -32, 0), (32, 32, 64)),
    11: ((-32, -32, 0), (32, 32, 64)),
    12: ((-16, -16, -32), (16, 16, 0)),
    13: ((-1, -1, 0), (1, 1, 2)),
    14: ((-1, -1, 0), (1, 1, 2)),
    17: ((-32, -32, 0), (32, 32, 64)),
    18: ((-32, -32, 0), (32, 32, 64)),
    19: ((-32, -32, -32), (32, 32, 32)),
    20: ((-16, -16, -64), (16, 16, 64)),
    21: ((-32, -32, -16), (32, 32, 16)),
    22: ((-16, -16, -16), (16, 16, 16)),
    23: ((-32, -32, -64), (32, 32, 0)),
    24: ((-5, -5, 0), (5, 5, 2)),
    25: ((-14, -14, 0), (14, 14, 36)),
}
DEFAULT_ACTOR_HULL = ((-16, -16, 0), (16, 16, 72))


@dataclass(frozen=True)
class TransitionProp:
    destination: str
    actor_type: int
    targetname: str
    origin: tuple[float, float, float]
    yaw: float
    source: str

    def line(self) -> str:
        x, y, z = self.origin
        return (
            f"{self.destination}|{self.actor_type}|{self.targetname}|"
            f"{x:g}|{y:g}|{z:g}|{self.yaw:g}|{self.source}"
        )


def decompress_vis(data: bytes, offset: int, row_bytes: int) -> bytes:
    if offset < 0:
        return bytes([0xFF]) * row_bytes
    output = bytearray()
    cursor = offset
    while len(output) < row_bytes and cursor < len(data):
        value = data[cursor]
        cursor += 1
        if value:
            output.append(value)
            continue
        if cursor >= len(data):
            break
        count = data[cursor]
        cursor += 1
        output.extend(b"\0" * min(count, row_bytes - len(output)))
    output.extend(b"\0" * (row_bytes - len(output)))
    return bytes(output)


class BspVisibility:
    """The BSP30 point-leaf/PVS subset used by EntitiesInPVS."""

    def __init__(self, path: Path):
        self.data = path.read_bytes()
        if len(self.data) < 124 or struct.unpack_from("<i", self.data, 0)[0] != 30:
            raise ValueError(f"{path}: not a GoldSrc BSP30 map")
        self.lumps = [struct.unpack_from("<ii", self.data, 4 + index * 8) for index in range(15)]
        models = self.lump(LUMP_MODELS)
        self.vis_leaf_count = struct.unpack_from("<i", models, 52)[0] if len(models) >= 56 else 0

    def lump(self, index: int) -> bytes:
        offset, length = self.lumps[index]
        if offset < 0 or length < 0 or offset + length > len(self.data):
            return b""
        return self.data[offset : offset + length]

    def point_leaf(self, point: tuple[float, float, float]) -> int:
        nodes = self.lump(LUMP_NODES)
        planes = self.lump(LUMP_PLANES)
        node = 0
        for _ in range(512):
            if node < 0:
                return -node - 1
            offset = node * 24
            if offset + 8 > len(nodes):
                return 0
            plane_index = max(struct.unpack_from("<i", nodes, offset)[0], 0)
            plane_offset = plane_index * 20
            if plane_offset + 16 > len(planes):
                return 0
            nx, ny, nz, distance = struct.unpack_from("<ffff", planes, plane_offset)
            side = point[0] * nx + point[1] * ny + point[2] * nz - distance
            child = struct.unpack_from("<h", nodes, offset + (4 if side >= 0 else 6))[0]
            node = child
        return 0

    def point_visible(
        self,
        viewpoint: tuple[float, float, float],
        target: tuple[float, float, float],
    ) -> bool:
        view_leaf = self.point_leaf(viewpoint)
        target_leaf = self.point_leaf(target)
        view_cluster = view_leaf - 1
        target_cluster = target_leaf - 1
        if view_cluster < 0 or target_cluster < 0 or target_cluster >= self.vis_leaf_count:
            return False
        leaves = self.lump(LUMP_LEAVES)
        leaf_offset = view_leaf * 28
        if leaf_offset + 8 > len(leaves):
            return False
        vis_offset = struct.unpack_from("<i", leaves, leaf_offset + 4)[0]
        row = decompress_vis(
            self.lump(LUMP_VISIBILITY),
            vis_offset,
            (self.vis_leaf_count + 7) // 8,
        )
        return bool(row[target_cluster >> 3] & (1 << (target_cluster & 7)))

    def box_visible(
        self,
        viewpoint: tuple[float, float, float],
        mins: tuple[float, float, float],
        maxs: tuple[float, float, float],
    ) -> bool:
        """GoldSrc Mod_BoxVisible semantics, including solid-view full PVS."""
        view_leaf = self.point_leaf(viewpoint)
        if view_leaf <= 0:
            return True
        leaves = self.lump(LUMP_LEAVES)
        leaf_offset = view_leaf * 28
        if leaf_offset + 8 > len(leaves):
            return True
        vis_offset = struct.unpack_from("<i", leaves, leaf_offset + 4)[0]
        row = decompress_vis(
            self.lump(LUMP_VISIBILITY),
            vis_offset,
            (self.vis_leaf_count + 7) // 8,
        )
        nodes = self.lump(LUMP_NODES)
        planes = self.lump(LUMP_PLANES)

        def visit(node: int, depth: int = 0) -> bool:
            if node < 0:
                cluster = -node - 2  # leaf id - 1
                return 0 <= cluster < self.vis_leaf_count and bool(
                    row[cluster >> 3] & (1 << (cluster & 7))
                )
            if depth > 512:
                return True
            offset = node * 24
            if offset + 8 > len(nodes):
                return True
            plane_index = struct.unpack_from("<i", nodes, offset)[0]
            po = plane_index * 20
            if plane_index < 0 or po + 16 > len(planes):
                return True
            nx, ny, nz, distance = struct.unpack_from("<ffff", planes, po)
            normal = (nx, ny, nz)
            near = tuple(
                mins[axis] if normal[axis] >= 0 else maxs[axis]
                for axis in range(3)
            )
            far = tuple(
                maxs[axis] if normal[axis] >= 0 else mins[axis]
                for axis in range(3)
            )
            min_side = sum(normal[i] * near[i] for i in range(3)) - distance
            max_side = sum(normal[i] * far[i] for i in range(3)) - distance
            c0, c1 = struct.unpack_from("<hh", nodes, offset + 4)
            if min_side >= 0:
                return visit(c0, depth + 1)
            if max_side < 0:
                return visit(c1, depth + 1)
            return visit(c0, depth + 1) or visit(c1, depth + 1)

        return visit(0)


def actor_bounds(
    entity: dict[str, str], actor_type: int
) -> tuple[tuple[float, float, float], tuple[float, float, float]] | None:
    origin = parse_vec3(entity.get("origin", ""))
    if origin is None:
        return None
    mins, maxs = ACTOR_HULLS.get(actor_type, DEFAULT_ACTOR_HULL)
    return (
        tuple(origin[i] + mins[i] for i in range(3)),
        tuple(origin[i] + maxs[i] for i in range(3)),
    )


def brush_entity_bounds(
    visibility: BspVisibility, entity: dict[str, str]
) -> tuple[tuple[float, float, float], tuple[float, float, float]] | None:
    model = entity.get("model", "")
    if not model.startswith("*"):
        return None
    try:
        index = int(model[1:])
    except ValueError:
        return None
    models = visibility.lump(LUMP_MODELS)
    offset = index * 64
    if index <= 0 or offset + 24 > len(models):
        return None
    mins = struct.unpack_from("<fff", models, offset)
    maxs = struct.unpack_from("<fff", models, offset + 12)
    origin = parse_vec3(entity.get("origin", "")) or (0.0, 0.0, 0.0)
    return (
        tuple(mins[i] + origin[i] for i in range(3)),
        tuple(maxs[i] + origin[i] for i in range(3)),
    )


def bounds_overlap(
    a: tuple[tuple[float, float, float], tuple[float, float, float]],
    b: tuple[tuple[float, float, float], tuple[float, float, float]],
) -> bool:
    return all(a[0][axis] <= b[1][axis] and a[1][axis] >= b[0][axis] for axis in range(3))


def parse_vec3(value: str) -> tuple[float, float, float] | None:
    try:
        parts = tuple(float(part) for part in value.split())
    except ValueError:
        return None
    return parts if len(parts) == 3 else None


def entity_lump(path: Path) -> str:
    data = path.read_bytes()
    if len(data) < 124 or struct.unpack_from("<i", data, 0)[0] != 30:
        raise ValueError(f"{path}: not a GoldSrc BSP30 map")
    offset, length = struct.unpack_from("<ii", data, 4)
    if offset < 0 or length < 0 or offset + length > len(data):
        raise ValueError(f"{path}: invalid entity lump")
    return data[offset : offset + length].decode("latin1", errors="replace")


def parse_entities(path: Path) -> list[dict[str, str]]:
    return [dict(PAIR_RE.findall(block)) for block in BLOCK_RE.findall(entity_lump(path))]


def yaw_degrees(entity: dict[str, str]) -> float:
    angles = parse_vec3(entity.get("angles", ""))
    if angles is not None:
        return angles[1]
    try:
        return float(entity.get("angle", "0"))
    except ValueError:
        return 0.0


def landmark_transform(
    origin: tuple[float, float, float],
    source_landmark: tuple[float, float, float],
    destination_landmark: tuple[float, float, float],
) -> tuple[float, float, float]:
    return tuple(
        destination_landmark[axis] + origin[axis] - source_landmark[axis]
        for axis in range(3)
    )


def named_landmarks(entities: list[dict[str, str]]) -> dict[str, tuple[float, float, float]]:
    out: dict[str, tuple[float, float, float]] = {}
    for entity in entities:
        if entity.get("classname") != "info_landmark":
            continue
        name = entity.get("targetname", "")
        origin = parse_vec3(entity.get("origin", ""))
        if name and origin is not None:
            out[name] = origin
    return out


def generate(
    maps_dir: Path, map_names: list[str]
) -> list[TransitionProp]:
    entities = {
        name: parse_entities(maps_dir / f"{name}.bsp")
        for name in map_names
        if (maps_dir / f"{name}.bsp").is_file()
    }
    incoming: dict[str, list[tuple[str, str]]] = {}
    for source, source_entities in entities.items():
        for entity in source_entities:
            if entity.get("classname") != "trigger_changelevel":
                continue
            destination = entity.get("map", "")
            landmark = entity.get("landmark", "")
            if destination in entities and landmark:
                incoming.setdefault(destination, []).append((source, landmark))

    # Map-local authored identities seed a fixed-point propagation over every
    # landmark edge. Geometry is deliberately not applied here: these are safe
    # clip/type overapproximations, while runtime bbox-PVS + transition volumes
    # decide whether an actor actually crosses on a particular playthrough.
    # value = possible (type, original source map) pairs. Ambiguity is harmless
    # until a destination script actually needs that identity; only then is it
    # a cooker error rather than a last-write-wins guess.
    available: dict[str, dict[str, set[tuple[int, str]]]] = {
        name: {} for name in entities
    }
    for map_name, map_entities in entities.items():
        for entity in map_entities:
            resolved_type = actor_type(entity)
            targetname = entity.get("targetname", "")
            if not targetname or resolved_type is None or resolved_type in {16, 50}:
                continue  # Gargantua/tentacle do not have ACROSS_TRANSITION.
            available[map_name].setdefault(targetname, set()).add((resolved_type, map_name))

    changed = True
    while changed:
        changed = False
        for destination in sorted(entities):
            for source, _landmark in sorted(incoming.get(destination, [])):
                for targetname, values in sorted(available[source].items()):
                    destination_values = available[destination].setdefault(targetname, set())
                    before = len(destination_values)
                    destination_values.update(values)
                    changed |= len(destination_values) != before

    result: list[TransitionProp] = []
    for destination in map_names:
        dest_entities = entities.get(destination)
        if dest_entities is None:
            continue
        scripted_names = {
            entity.get("m_iszEntity", "")
            for entity in dest_entities
            if entity.get("classname") in {"scripted_sequence", "aiscripted_sequence"}
        }
        scripted_names.discard("")
        existing = {
            entity.get("targetname", "")
            for entity in dest_entities
            if actor_type(entity) is not None
        }
        for targetname in sorted(scripted_names - existing):
            hints = available[destination].get(targetname)
            if not hints:
                continue
            types = {actor_type for actor_type, _source in hints}
            if len(types) != 1:
                raise ValueError(
                    f"{destination}: scripted incoming identity {targetname!r} "
                    f"has conflicting types {sorted(types)}"
                )
            chosen_type = next(iter(types))
            source = min(source for ty, source in hints if ty == chosen_type)
            # Legacy eight-field shape retained for Makefile/cooker stability;
            # position/yaw are intentionally ignored by the cooker now.
            result.append(
                TransitionProp(
                    destination=destination,
                    actor_type=chosen_type,
                    targetname=targetname,
                    origin=(0.0, 0.0, 0.0),
                    yaw=0.0,
                    source=source,
                )
            )
    return result


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("maps_dir", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("maps", nargs="+")
    args = parser.parse_args()

    props = generate(args.maps_dir, args.maps)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    text = "\n".join(prop.line() for prop in props)
    args.output.write_text(text + ("\n" if text else ""), encoding="utf-8")
    print(f"transition type hints -> {args.output} ({len(props)} actors)")
    for prop in props:
        print(f"  {prop.destination}: {prop.targetname} from {prop.source}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
