#!/usr/bin/env python3
"""Normalize and compare deterministic GoldSrc and hl-psx trace logs."""

from __future__ import annotations

import argparse
import difflib
import json
import math
import re
import sys
from collections import defaultdict
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable

TRACE_MARKER = re.compile(r"\b(HLREF|HLPSX)\|")
MAP_OCCURRENCE = re.compile(r"^([A-Za-z0-9_-]+)(?:@([1-9][0-9]*))?$")
IGNORED_EXACT_FIELDS = frozenset(("run_id", "host_cycle", "host_frame"))
INPUT_FIELDS = ("forward", "strafe", "turn", "look", "actions")
Occurrence = tuple[str, int]


@dataclass(frozen=True)
class Record:
    source: str
    kind: str
    fields: dict[str, str]

    @property
    def map(self) -> str:
        return self.fields.get("map", "")

    @property
    def tick(self) -> int:
        return number(self.fields.get("tick"), 0)


def number(value: str | None, default: int = 0) -> int:
    if value is None:
        return default
    value = value.strip()
    try:
        # Trace actions are normally hexadecimal while positions/times can be
        # decimal floats. Accept both without silently turning 0x0008 into 0.
        return int(value, 0)
    except ValueError:
        pass
    try:
        return int(float(value))
    except ValueError:
        return default


def parse_map_occurrence(value: str) -> tuple[str, int | None]:
    """Parse ``MAP`` or the occurrence-specific ``MAP@N`` requirement form."""

    match = MAP_OCCURRENCE.fullmatch(value)
    if match is None:
        raise ValueError(
            f"invalid map occurrence {value!r}; expected MAP or MAP@N with N >= 1"
        )
    return match.group(1), int(match.group(2)) if match.group(2) else None


def occurrence_matches(spec: tuple[str, int | None], occurrence: Occurrence) -> bool:
    return spec[0] == occurrence[0] and (spec[1] is None or spec[1] == occurrence[1])


def parse_pipe_line(line: str) -> Record | None:
    marker = TRACE_MARKER.search(line)
    if not marker:
        return None
    payload = line[marker.start() :].strip()
    parts = payload.split("|")
    if len(parts) < 2:
        return None
    fields: dict[str, str] = {}
    for item in parts[2:]:
        if "=" in item:
            key, value = item.split("=", 1)
            fields[key] = value
    return Record(parts[0], parts[1], fields)


def parse_json_line(line: str) -> Record | None:
    line = line.strip()
    if not line.startswith("{"):
        return None
    try:
        value = json.loads(line)
    except json.JSONDecodeError:
        return None
    source = str(value.pop("source", ""))
    kind = str(value.pop("kind", ""))
    if source not in ("HLREF", "HLPSX", "goldsrc-xash", "hl-psx-psoxide"):
        return None
    source = "HLREF" if source in ("HLREF", "goldsrc-xash") else "HLPSX"
    return Record(source, kind, {str(k): str(v) for k, v in value.items()})


def read_records(path: Path) -> list[Record]:
    records: list[Record] = []
    with path.open("r", encoding="utf-8", errors="replace") as stream:
        for line in stream:
            record = parse_pipe_line(line) or parse_json_line(line)
            if record is not None:
                records.append(record)
    if not records:
        raise ValueError(f"{path}: no HLREF/HLPSX trace records")
    return records


def normalized(record: Record, neutral_source: bool = False) -> dict[str, object]:
    fields = {
        key: value
        for key, value in sorted(record.fields.items())
        if key not in IGNORED_EXACT_FIELDS
    }
    return {
        "source": "TRACE" if neutral_source else record.source,
        "kind": record.kind,
        **fields,
    }


def canonical_json(records: Iterable[Record], neutral_source: bool = False) -> str:
    return (
        "\n".join(
            json.dumps(
                normalized(record, neutral_source),
                sort_keys=True,
                separators=(",", ":"),
            )
            for record in records
        )
        + "\n"
    )


def event_key(record: Record) -> tuple[str, str, str]:
    event = record.fields.get("event", "")
    detail = record.fields.get("target", "")
    if event == "changelevel":
        detail = record.fields.get("next_map", "")
    return record.map, event, detail


def annotate_occurrences(records: Iterable[Record]) -> list[tuple[Occurrence, Record]]:
    """Assign each record to an ordered visit of its map.

    Both engines emit a hard map marker (`HLREF|map` or the PSX map_start
    event). Falling back to a map-name change also keeps small hand-written
    fixtures useful. Explicit markers are essential for routes that return to
    the same BSP, such as c1a1c -> c1a1d -> c1a1c.

    Xash publishes a new ``HLREF|map`` while the client command path can still
    flush one final ``HLREF|input`` row tagged with the preceding map. Once an
    explicit marker has established the active visit, attribute such late rows
    to that map's most recent occurrence without changing the active visit.
    Otherwise the stale row invents two one-record visits around every real
    changelevel and corrupts input parity.
    """
    counts: dict[str, int] = defaultdict(int)
    latest: dict[str, Occurrence] = {}
    current: Occurrence | None = None
    saw_explicit_start = False
    result: list[tuple[Occurrence, Record]] = []
    for record in records:
        explicit_start = record.kind == "map" or (
            record.kind == "event" and record.fields.get("event") == "map_start"
        )
        occurrence = current
        if explicit_start and record.map:
            saw_explicit_start = True
            counts[record.map] += 1
            current = (record.map, counts[record.map])
            latest[record.map] = current
            occurrence = current
        elif record.map and current is None:
            counts[record.map] += 1
            current = (record.map, counts[record.map])
            latest[record.map] = current
            occurrence = current
        elif record.map and current is not None and record.map != current[0]:
            if saw_explicit_start and record.map in latest:
                occurrence = latest[record.map]
            else:
                counts[record.map] += 1
                current = (record.map, counts[record.map])
                latest[record.map] = current
                occurrence = current
        if occurrence is not None:
            result.append((occurrence, record))
    return result


def semantic_events_with_occurrence(
    records: Iterable[Record],
) -> list[tuple[Occurrence, Record]]:
    annotated = annotate_occurrences(records)
    explicit_starts = {
        occurrence
        for occurrence, record in annotated
        if record.kind == "event" and record.fields.get("event") == "map_start"
    }
    first_ticks: dict[Occurrence, int] = {}
    for occurrence, record in annotated:
        if "tick" in record.fields:
            first_ticks.setdefault(occurrence, record.tick)

    result: list[tuple[Occurrence, Record]] = []
    synthesized: set[Occurrence] = set()
    for occurrence, record in annotated:
        if occurrence not in explicit_starts and occurrence not in synthesized:
            result.append(
                (
                    occurrence,
                    Record(
                        record.source,
                        "event",
                        {
                            "map": occurrence[0],
                            "tick": str(first_ticks.get(occurrence, 0)),
                            "event": "map_start",
                        },
                    ),
                )
            )
            synthesized.add(occurrence)
        if record.kind == "event" and record.fields.get("event") in (
            "map_start",
            "target_fire",
            "changelevel",
        ):
            result.append((occurrence, record))
    return result


def semantic_events(records: Iterable[Record]) -> list[Record]:
    return [record for _, record in semantic_events_with_occurrence(records)]


def map_start_ticks(records: Iterable[Record]) -> dict[str, int]:
    """Return each map's first traced tick in its source clock.

    HLREF uses one global server tick across changelevels, while HLPSX's
    `tick` field restarts at zero for every room. Comparisons must subtract the
    local map origin before reporting drift or every map after c0a0 appears
    hundreds/thousands of ticks early.
    """
    starts: dict[str, int] = {}
    for record in semantic_events(records):
        if record.fields.get("event") == "map_start":
            starts.setdefault(record.map, record.tick)
    return starts


def input_snapshot_ticks(records: Iterable[Record]) -> dict[Occurrence, int]:
    """Return the first post-input state tick for each map.

    GoldSrc can publish server state before its local client becomes active.
    The first ``HLREF|input`` therefore precedes (rather than shares a number
    with) the state snapshot it controls. hl-psx uses a map-local clock, but
    obeys the same record order. Anchoring at the first subsequent tick aligns
    the two post-physics states without assuming either engine's tick offset.
    """
    waiting: set[Occurrence] = set()
    starts: dict[Occurrence, int] = {}
    for occurrence, record in annotate_occurrences(records):
        if record.kind == "input" and occurrence not in starts:
            waiting.add(occurrence)
        elif record.kind == "tick" and occurrence in waiting:
            starts[occurrence] = record.tick
            waiting.remove(occurrence)
    return starts


def semantic_input_rows(
    records: Iterable[Record],
) -> dict[Occurrence, list[tuple[int, ...]]]:
    """Normalize replayed input rows for exact cross-engine tape parity."""
    rows: dict[Occurrence, list[tuple[int, ...]]] = defaultdict(list)
    for occurrence, record in annotate_occurrences(records):
        if record.kind != "input" or not record.map:
            continue
        input_tick = number(
            record.fields.get("input_tick", record.fields.get("tick")), -1
        )
        rows[occurrence].append(
            (
                input_tick,
                *(number(record.fields.get(field), 0) for field in INPUT_FIELDS),
            )
        )
    return rows


def compare_inputs(
    reference: Iterable[Record], psx: Iterable[Record]
) -> list[dict[str, object]]:
    ref_rows = semantic_input_rows(reference)
    psx_rows = semantic_input_rows(psx)
    map_order = list(dict.fromkeys((*ref_rows.keys(), *psx_rows.keys())))
    result: list[dict[str, object]] = []
    for occurrence in map_order:
        left = ref_rows.get(occurrence, [])
        right = psx_rows.get(occurrence, [])
        row: dict[str, object] = {
            "map": occurrence[0],
            "occurrence": occurrence[1],
            "reference_samples": len(left),
            "psx_samples": len(right),
            "identical": left == right,
        }
        if left != right:
            common = min(len(left), len(right))
            mismatch = next((i for i in range(common) if left[i] != right[i]), common)
            row["first_mismatch_index"] = mismatch
            row["reference"] = list(left[mismatch]) if mismatch < len(left) else None
            row["psx"] = list(right[mismatch]) if mismatch < len(right) else None
        result.append(row)
    return result


def canonical_xyz(record: Record, prefix: str) -> tuple[float, float, float] | None:
    keys = (f"{prefix}x", f"{prefix}y", f"{prefix}z")
    if any(key not in record.fields for key in keys):
        return None
    raw = tuple(float(record.fields[key]) for key in keys)
    if record.source == "HLPSX":
        # Cooker: HL [x,y,z] -> PSX [x,z,y]. Intro maps use scale 1.
        return raw[0], raw[2], raw[1]
    return raw


def entity_brush(record: Record) -> int:
    """Return the BSP submodel number used as a cross-engine identity."""
    if "brush" in record.fields:
        return number(record.fields["brush"], -1)
    model = record.fields.get("model", "")
    if model.startswith("*"):
        return number(model[1:], -1)
    return -1


def entity_key(record: Record) -> str | None:
    """Stable identity shared by GoldSrc edicts and cooked PSX records.

    Brush submodels are unique within a BSP and survive cooking exactly. Named
    point actors use their targetname. Unnamed point actors are intentionally
    left to the existing prop trace until they gain a transition-stable id.
    """
    brush = entity_brush(record)
    if brush >= 0:
        return f"brush:*{brush}"
    targetname = record.fields.get("targetname", "")
    if targetname:
        return f"named:{targetname}"
    globalname = record.fields.get("globalname", "")
    if globalname:
        return f"global:{globalname}"
    return None


def canonical_entity_center(record: Record) -> tuple[float, float, float] | None:
    if record.fields.get("class") == "func_rotating":
        # GoldSrc's cx/cy/cz is the center of the live rotated abs bounds;
        # hl-psx reports the rotated geometric center. The authored x/y/z is
        # the invariant brush pivot in both engines and is the comparable
        # position; fan orientation has its own circular angle gate below.
        return canonical_xyz(record, "")
    center = canonical_xyz(record, "c")
    return center if center is not None else canonical_xyz(record, "")


def entity_snapshots(
    records: Iterable[Record], origins: dict[Occurrence, int]
) -> dict[Occurrence, dict[int, dict[str, Record]]]:
    """Index checkpoint rows by visit, map-local tick, and authored identity."""
    result: dict[Occurrence, dict[int, dict[str, Record]]] = defaultdict(
        lambda: defaultdict(dict)
    )
    for occurrence, record in annotate_occurrences(records):
        if record.kind != "entity":
            continue
        key = entity_key(record)
        if key is None:
            continue
        local_tick = (
            number(record.fields["map_tick"], 0)
            if "map_tick" in record.fields
            else record.tick - origins.get(occurrence, 0)
        )
        result[occurrence][local_tick][key] = record
    return result


def entity_is_active(record: Record) -> bool | None:
    if "active" in record.fields:
        return number(record.fields["active"], 0) != 0
    if record.source == "HLREF":
        entity_class = record.fields.get("class", "")
        # PSX `active` is the live draw/collision state for toggled walls and
        # breakables, not merely allocation of their fixed entity slot. GoldSrc
        # keeps both as edicts after TurnOff()/Die(), but makes them SOLID_NOT
        # before the edict is eventually freed. Normalize that short lifecycle
        # window while leaving intentionally nonsolid live entities (passable
        # trains, fans and open doors) present.
        breakable_pushable = (
            entity_class == "func_pushable"
            and number(record.fields.get("spawnflags"), 0) & 128 != 0
        )
        if entity_class in ("func_wall_toggle", "func_breakable") or breakable_pushable:
            solid = record.fields.get("solid")
            return number(solid, 0) != 0 if solid is not None else True
        # A present edict is active even when intentionally non-solid. Freed
        # edicts emit no row at all.
        return True
    return None


def canonical_entity_angle(record: Record) -> float | None:
    """Return a func_rotating phase in GoldSrc axial degrees.

    The cooker swaps HL Y/Z, which is a reflection. Its PSX Q12 fan phase is
    therefore the negative of the corresponding GoldSrc axial angle. Keeping
    the comparison modulo one turn also handles long-running fans whose SDK
    checkpoint angles grow beyond +/-360 degrees.
    """

    if record.fields.get("class") != "func_rotating":
        return None
    if record.source == "HLREF":
        spawnflags = number(record.fields.get("spawnflags"), 0)
        field = "roll" if spawnflags & 4 else "pitch" if spawnflags & 8 else "yaw"
        if field not in record.fields:
            return None
        return float(record.fields[field])
    raw = record.fields.get("yaw_q12", record.fields.get("phase"))
    if raw is None:
        return None
    return -(number(raw, 0) & 0x0FFF) * 360.0 / 4096.0


def circular_angle_error(left: float, right: float) -> float:
    return abs((left - right + 180.0) % 360.0 - 180.0)


ENTITY_SPAWNFLAG_MASKS: dict[str, int] = {
    # CFuncTrain mutates bit 0 (SF_TRAIN_WAIT_RETRIGGER) at runtime. Bit 3 is
    # the stable PASSABLE behavior shared with the PSX cooked record.
    "func_train": 0x0008,
    # path_track may mutate NOCONTROL (bit 1); the other tracktrain behaviors
    # remain authored and directly comparable.
    "func_tracktrain": 0x000D,
    "func_wall_toggle": 0x0001,
    "func_breakable": 0x0107,
    "func_pushable": 0x0187,
    "func_rotating": 0x03FF,
    "func_plat": 0x0001,
    "func_platrot": 0x0001,
    # GoldSrc adds the high SF_DOOR_SILENT bit to water doors during Spawn;
    # compare only authored low behavior bits retained by the PSX format.
    "func_door": 0x0339,
    "func_door_rotating": 0x03FB,
}


def comparable_entity_health(record: Record, entity_class: str) -> int | None:
    """Return canonical live HP only for classes where HP drives gameplay."""

    if "health" not in record.fields:
        return None
    health = number(record.fields["health"])
    if entity_class in ("monster_scientist", "monster_barney"):
        return health
    spawnflags = number(record.fields.get("spawnflags"), 0)
    is_breakable = entity_class == "func_breakable" or (
        entity_class == "func_pushable" and spawnflags & 128 != 0
    )
    if not is_breakable:
        # Ordinary CPushable retains the authored pev->health number but its
        # TakeDamage override never consumes it. PSX correctly spends no state
        # on that inert metadata.
        return None
    if spawnflags & 1 != 0:
        # Trigger-only breakables ignore weapon damage and Die() directly on
        # Use. PSX stores one as an allocation-free "alive" sentinel even when
        # the BSP authored health zero, so the numeric value is not semantic.
        return None
    # A non-trigger-only zero-health GoldSrc breakable and PSX's one-HP sentinel
    # both die to the first positive hit.
    return max(1, health)


def entity_field_differences(
    left: Record, right: Record
) -> list[tuple[str, object, object]]:
    """Return class-aware semantic differences, excluding raw engine layout."""

    differences: list[tuple[str, object, object]] = []
    left_class = left.fields.get("class")
    right_class = right.fields.get("class")
    if left_class and right_class and left_class != right_class:
        differences.append(("class", left_class, right_class))
        return differences

    entity_class = left_class or right_class or ""
    spawnflag_mask = ENTITY_SPAWNFLAG_MASKS.get(entity_class)
    if (
        spawnflag_mask is not None
        and "spawnflags" in left.fields
        and "spawnflags" in right.fields
    ):
        left_flags = number(left.fields["spawnflags"]) & spawnflag_mask
        right_flags = number(right.fields["spawnflags"]) & spawnflag_mask
        if left_flags != right_flags:
            differences.append(("spawnflags", left_flags, right_flags))

    left_health = comparable_entity_health(left, entity_class)
    right_health = comparable_entity_health(right, entity_class)
    if (
        left_health is not None
        and right_health is not None
        and left_health != right_health
    ):
        differences.append(("health", left_health, right_health))
    return differences


def compare_entities(
    reference: Iterable[Record],
    psx: Iterable[Record],
    reference_origins: dict[Occurrence, int],
    psx_origins: dict[Occurrence, int],
) -> list[dict[str, object]]:
    """Compare once-per-second post-physics entity checkpoints.

    Entity traces use a map-local tick published by both engines, avoiding the
    startup-frame offset between Xash's global server clock and PSX map time.
    """
    left = entity_snapshots(reference, reference_origins)
    right = entity_snapshots(psx, psx_origins)
    map_order = list(dict.fromkeys((*left.keys(), *right.keys())))
    result: list[dict[str, object]] = []
    for occurrence in map_order:
        left_ticks = left.get(occurrence, {})
        right_ticks = right.get(occurrence, {})
        common_ticks = sorted(set(left_ticks) & set(right_ticks))
        errors: list[float] = []
        angle_errors: list[float] = []
        errors_by_entity: dict[
            str,
            list[
                tuple[
                    float,
                    int,
                    tuple[float, float, float],
                    tuple[float, float, float],
                ]
            ],
        ] = defaultdict(list)
        angle_errors_by_entity: dict[str, list[tuple[float, int, float, float]]] = (
            defaultdict(list)
        )
        worst: dict[str, object] | None = None
        worst_angle: dict[str, object] | None = None
        left_all = {key for snapshot in left_ticks.values() for key in snapshot}
        right_all = {key for snapshot in right_ticks.values() for key in snapshot}
        missing: set[str] = left_all - right_all
        extra: set[str] = right_all - left_all
        active_mismatches: list[dict[str, object]] = []
        field_mismatches: list[dict[str, object]] = []
        matched_samples = 0
        for tick in common_ticks:
            left_entities = left_ticks[tick]
            right_entities = right_ticks[tick]
            left_keys = set(left_entities)
            right_keys = set(right_entities)
            missing.update(left_keys - right_keys)
            extra.update(right_keys - left_keys)
            for key in sorted(left_keys & right_keys):
                left_record = left_entities[key]
                right_record = right_entities[key]
                left_pos = canonical_entity_center(left_record)
                right_pos = canonical_entity_center(right_record)
                if left_pos is not None and right_pos is not None:
                    error = distance(left_pos, right_pos)
                    errors.append(error)
                    errors_by_entity[key].append((error, tick, left_pos, right_pos))
                    matched_samples += 1
                    if worst is None or error >= float(worst["error"]):
                        worst = {
                            "tick": tick,
                            "entity": key,
                            "error": round(error, 3),
                            "reference": list(left_pos),
                            "psx": list(right_pos),
                        }
                left_active = entity_is_active(left_record)
                right_active = entity_is_active(right_record)
                if (
                    left_active is not None
                    and right_active is not None
                    and left_active != right_active
                ):
                    active_mismatches.append(
                        {
                            "tick": tick,
                            "entity": key,
                            "reference": left_active,
                            "psx": right_active,
                        }
                    )
                for field, reference_value, psx_value in entity_field_differences(
                    left_record, right_record
                ):
                    field_mismatches.append(
                        {
                            "tick": tick,
                            "entity": key,
                            "field": field,
                            "reference": reference_value,
                            "psx": psx_value,
                        }
                    )
                left_angle = canonical_entity_angle(left_record)
                right_angle = canonical_entity_angle(right_record)
                if left_angle is not None and right_angle is not None:
                    error = circular_angle_error(left_angle, right_angle)
                    angle_errors.append(error)
                    angle_errors_by_entity[key].append(
                        (error, tick, left_angle, right_angle)
                    )
                    if worst_angle is None or error >= float(worst_angle["error"]):
                        worst_angle = {
                            "tick": tick,
                            "entity": key,
                            "error": round(error, 3),
                            "reference_degrees": round(left_angle, 3),
                            "psx_degrees": round(right_angle, 3),
                        }
        row: dict[str, object] = {
            "map": occurrence[0],
            "occurrence": occurrence[1],
            "reference_snapshots": len(left_ticks),
            "psx_snapshots": len(right_ticks),
            "shared_snapshots": len(common_ticks),
            "matched_samples": matched_samples,
            "missing_entities": sorted(missing),
            "extra_entities": sorted(extra),
            "active_mismatches": active_mismatches,
            "field_mismatches": field_mismatches,
        }
        if errors:
            row["position"] = {
                "rms": round(
                    math.sqrt(sum(value * value for value in errors) / len(errors)), 3
                ),
                "max": round(max(errors), 3),
                "worst": worst,
            }
            # Aggregate RMS can hide one broken cinematic actor among dozens of
            # exact static brushes. Keep a deterministic per-entity ranking so
            # a playthrough trace points straight at the next irregularity.
            ranked: list[dict[str, object]] = []
            for key, samples in errors_by_entity.items():
                worst_sample = max(samples, key=lambda item: (item[0], item[1]))
                ranked.append(
                    {
                        "entity": key,
                        "samples": len(samples),
                        "rms": round(
                            math.sqrt(
                                sum(item[0] * item[0] for item in samples)
                                / len(samples)
                            ),
                            3,
                        ),
                        "max": round(worst_sample[0], 3),
                        "worst": {
                            "tick": worst_sample[1],
                            "reference": list(worst_sample[2]),
                            "psx": list(worst_sample[3]),
                        },
                    }
                )
            ranked.sort(key=lambda item: (-float(item["max"]), str(item["entity"])))
            row["position_by_entity"] = ranked
        if angle_errors:
            row["angle"] = {
                "rms_degrees": round(
                    math.sqrt(
                        sum(value * value for value in angle_errors) / len(angle_errors)
                    ),
                    3,
                ),
                "max_degrees": round(max(angle_errors), 3),
                "worst": worst_angle,
            }
            angle_ranked: list[dict[str, object]] = []
            for key, samples in angle_errors_by_entity.items():
                worst_sample = max(samples, key=lambda item: (item[0], item[1]))
                angle_ranked.append(
                    {
                        "entity": key,
                        "samples": len(samples),
                        "rms_degrees": round(
                            math.sqrt(
                                sum(item[0] * item[0] for item in samples)
                                / len(samples)
                            ),
                            3,
                        ),
                        "max_degrees": round(worst_sample[0], 3),
                        "worst": {
                            "tick": worst_sample[1],
                            "reference_degrees": round(worst_sample[2], 3),
                            "psx_degrees": round(worst_sample[3], 3),
                        },
                    }
                )
            angle_ranked.sort(
                key=lambda item: (-float(item["max_degrees"]), str(item["entity"]))
            )
            row["angle_by_entity"] = angle_ranked
        result.append(row)
    return result


def map_ticks(records: Iterable[Record], occurrence: Occurrence) -> dict[int, Record]:
    return {
        record.tick: record
        for key, record in annotate_occurrences(records)
        if record.kind == "tick" and key == occurrence
    }


def anchor_tick(records: Iterable[Record], occurrence: Occurrence) -> int:
    events = [
        record
        for key, record in semantic_events_with_occurrence(records)
        if key == occurrence
    ]
    for record in events:
        if (
            record.fields.get("event") == "target_fire"
            and record.fields.get("target") == "train"
        ):
            return record.tick
    for record in events:
        if record.fields.get("event") == "map_start":
            return record.tick
    return 0


def distance(a: tuple[float, float, float], b: tuple[float, float, float]) -> float:
    return math.sqrt(sum((left - right) ** 2 for left, right in zip(a, b)))


def compare_positions(
    reference: list[Record],
    psx: list[Record],
    occurrence: Occurrence,
    reference_anchor: int | None = None,
    psx_anchor: int | None = None,
) -> dict[str, object]:
    ref_ticks = map_ticks(reference, occurrence)
    psx_ticks = map_ticks(psx, occurrence)
    ref_anchor = (
        anchor_tick(reference, occurrence)
        if reference_anchor is None
        else reference_anchor
    )
    psx_anchor = anchor_tick(psx, occurrence) if psx_anchor is None else psx_anchor
    common_relative = sorted(
        {tick - ref_anchor for tick in ref_ticks}
        & {tick - psx_anchor for tick in psx_ticks}
    )
    result: dict[str, object] = {
        "map": occurrence[0],
        "occurrence": occurrence[1],
        "reference_anchor": ref_anchor,
        "psx_anchor": psx_anchor,
        "samples": len(common_relative),
    }
    for prefix, label in (("p", "player"), ("train_", "train")):
        errors: list[float] = []
        worst_relative = 0
        for relative in common_relative:
            left = canonical_xyz(ref_ticks[ref_anchor + relative], prefix)
            right = canonical_xyz(psx_ticks[psx_anchor + relative], prefix)
            if left is None or right is None:
                continue
            error = distance(left, right)
            errors.append(error)
            if error >= max(errors):
                worst_relative = relative
        if errors:
            result[label] = {
                "samples": len(errors),
                "rms": round(
                    math.sqrt(sum(value * value for value in errors) / len(errors)), 3
                ),
                "max": round(max(errors), 3),
                "worst_relative_tick": worst_relative,
            }
    return result


def comparison_report(reference: list[Record], psx: list[Record]) -> dict[str, object]:
    ref_events = semantic_events_with_occurrence(reference)
    psx_events = semantic_events_with_occurrence(psx)
    ref_starts = {
        occurrence: record.tick
        for occurrence, record in ref_events
        if record.fields.get("event") == "map_start"
    }
    psx_starts = {
        occurrence: record.tick
        for occurrence, record in psx_events
        if record.fields.get("event") == "map_start"
    }
    ref_input_starts = input_snapshot_ticks(reference)
    psx_input_starts = input_snapshot_ticks(psx)
    # A semantic-input anchor is valid only when both engines published one;
    # otherwise retain the established map-start/train comparison behavior.
    shared_input_maps = set(ref_input_starts) & set(psx_input_starts)
    ref_origins = dict(ref_starts)
    psx_origins = dict(psx_starts)
    for occurrence in shared_input_maps:
        ref_origins[occurrence] = ref_input_starts[occurrence]
        psx_origins[occurrence] = psx_input_starts[occurrence]
    ref_keys = [
        (*occurrence, *event_key(record)[1:]) for occurrence, record in ref_events
    ]
    psx_keys = [
        (*occurrence, *event_key(record)[1:]) for occurrence, record in psx_events
    ]
    matcher = difflib.SequenceMatcher(a=ref_keys, b=psx_keys, autojunk=False)
    missing: list[dict[str, object]] = []
    extra: list[dict[str, object]] = []
    timing: list[dict[str, object]] = []
    for tag, i1, i2, j1, j2 in matcher.get_opcodes():
        if tag in ("delete", "replace"):
            missing.extend(
                {**normalized(record), "occurrence": occurrence[1]}
                for occurrence, record in ref_events[i1:i2]
            )
        if tag in ("insert", "replace"):
            extra.extend(
                {**normalized(record), "occurrence": occurrence[1]}
                for occurrence, record in psx_events[j1:j2]
            )
        if tag == "equal":
            for (left_occurrence, left), (right_occurrence, right) in zip(
                ref_events[i1:i2], psx_events[j1:j2]
            ):
                ref_tick = left.tick - ref_origins.get(left_occurrence, 0)
                psx_tick = right.tick - psx_origins.get(right_occurrence, 0)
                timing.append(
                    {
                        "map": left.map,
                        "occurrence": left_occurrence[1],
                        "event": left.fields.get("event", ""),
                        "detail": event_key(left)[2],
                        "reference_tick": ref_tick,
                        "psx_tick": psx_tick,
                        "drift_ticks": psx_tick - ref_tick,
                    }
                )

    ref_map_occurrences = [
        occurrence
        for occurrence, record in ref_events
        if record.fields.get("event") == "map_start"
    ]
    psx_map_occurrences = [
        occurrence
        for occurrence, record in psx_events
        if record.fields.get("event") == "map_start"
    ]
    ref_maps = [occurrence[0] for occurrence in ref_map_occurrences]
    psx_maps = [occurrence[0] for occurrence in psx_map_occurrences]
    psx_occurrence_set = set(psx_map_occurrences)
    shared_maps = [
        occurrence
        for occurrence in ref_map_occurrences
        if occurrence in psx_occurrence_set
    ]
    input_comparison = compare_inputs(reference, psx)
    entity_comparison = compare_entities(reference, psx, ref_origins, psx_origins)
    return {
        "reference_records": len(reference),
        "psx_records": len(psx),
        "reference_maps": ref_maps,
        "psx_maps": psx_maps,
        "missing_events": missing,
        "extra_events": extra,
        "event_timing": timing,
        "input_comparison": input_comparison,
        "entity_comparison": entity_comparison,
        "input_alignment": [
            {
                "map": occurrence[0],
                "occurrence": occurrence[1],
                "reference_snapshot_tick": ref_input_starts[occurrence],
                "psx_snapshot_tick": psx_input_starts[occurrence],
            }
            for occurrence in shared_maps
            if occurrence in shared_input_maps
        ],
        "positions": [
            compare_positions(
                reference,
                psx,
                occurrence,
                ref_input_starts.get(occurrence)
                if occurrence in shared_input_maps
                else None,
                psx_input_starts.get(occurrence)
                if occurrence in shared_input_maps
                else None,
            )
            for occurrence in shared_maps
        ],
    }


def command_normalize(args: argparse.Namespace) -> int:
    records = read_records(args.input)
    output = canonical_json(records, args.neutral_source)
    if args.output:
        args.output.write_text(output, encoding="utf-8")
    else:
        sys.stdout.write(output)
    return 0


def command_self_check(args: argparse.Namespace) -> int:
    left = canonical_json(read_records(args.left))
    right = canonical_json(read_records(args.right))
    if left == right:
        print("deterministic: normalized traces are byte-identical")
        return 0
    left_lines = left.splitlines()
    right_lines = right.splitlines()
    diff = list(
        difflib.unified_diff(
            left_lines, right_lines, str(args.left), str(args.right), n=2
        )
    )
    print("nondeterministic: normalized traces differ", file=sys.stderr)
    for line in diff[: args.max_diff_lines]:
        print(line, file=sys.stderr)
    return 1


def command_compare(args: argparse.Namespace) -> int:
    reference = read_records(args.reference)
    psx = read_records(args.psx)
    report = comparison_report(reference, psx)
    psx_annotated = annotate_occurrences(psx)
    psx_map_occurrences = [
        occurrence
        for occurrence, record in semantic_events_with_occurrence(psx)
        if record.fields.get("event") == "map_start"
    ]
    missing_required_maps: list[str] = []
    for requirement in args.require_map:
        wanted = parse_map_occurrence(requirement)
        if not any(occurrence_matches(wanted, visit) for visit in psx_map_occurrences):
            missing_required_maps.append(requirement)

    psx_events = [
        (occurrence, record)
        for occurrence, record in semantic_events_with_occurrence(psx)
        if record.kind == "event"
    ]
    missing_required_events: list[str] = []
    for requirement in args.require_event:
        parts = requirement.split(":", 2)
        if len(parts) != 3:
            raise ValueError(
                f"invalid --require-event {requirement!r}; "
                "expected MAP[@N]:EVENT:DETAIL"
            )
        wanted = parse_map_occurrence(parts[0])
        if not any(
            occurrence_matches(wanted, occurrence)
            and record.fields.get("event", "") == parts[1]
            and event_key(record)[2] == parts[2]
            for occurrence, record in psx_events
        ):
            missing_required_events.append(requirement)

    psx_carries = {
        (
            occurrence,
            record.fields.get("direction", ""),
            record.fields.get("id", ""),
        )
        for occurrence, record in psx_annotated
        if record.kind == "event" and record.fields.get("event") == "actor_carry"
    }
    missing_required_carries: list[str] = []
    carry_directions = ("in", "out", "train-in", "train-out")
    for requirement in args.require_carry:
        parts = requirement.split(":", 2)
        if len(parts) != 3 or parts[1] not in carry_directions:
            raise ValueError(
                f"invalid --require-carry {requirement!r}; "
                "expected MAP[@N]:in|out|train-in|train-out:ID"
            )
        wanted = parse_map_occurrence(parts[0])
        if not any(
            occurrence_matches(wanted, occurrence)
            and direction == parts[1]
            and carry_id == parts[2]
            for occurrence, direction, carry_id in psx_carries
        ):
            missing_required_carries.append(requirement)

    detached_exits: list[str] = []
    ticks_by_occurrence: dict[Occurrence, list[Record]] = defaultdict(list)
    for occurrence, record in psx_annotated:
        if record.kind == "tick":
            ticks_by_occurrence[occurrence].append(record)
    for requirement in args.require_attached_exit:
        wanted = parse_map_occurrence(requirement)
        visits = sorted(
            occurrence
            for occurrence in ticks_by_occurrence
            if occurrence_matches(wanted, occurrence)
        )
        # Unqualified MAP preserves the old behavior: check its final visit.
        ticks = ticks_by_occurrence.get(visits[-1], []) if visits else []
        if not ticks or number(ticks[-1].fields.get("train_attached"), 0) != 1:
            detached_exits.append(requirement)
    report["missing_required_maps"] = missing_required_maps
    report["missing_required_events"] = missing_required_events
    report["missing_required_carries"] = missing_required_carries
    report["detached_required_exits"] = detached_exits
    rendered = json.dumps(report, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.write_text(rendered, encoding="utf-8")
    else:
        sys.stdout.write(rendered)
    failed = bool(
        missing_required_maps
        or missing_required_events
        or missing_required_carries
        or detached_exits
    )
    if getattr(args, "require_input_parity", False):
        failed |= not report["input_comparison"] or any(
            not item["identical"] for item in report["input_comparison"]
        )
    if getattr(args, "require_entity_parity", False):
        failed |= not report["entity_comparison"] or any(
            item["missing_entities"]
            or item["extra_entities"]
            or item["active_mismatches"]
            or item["field_mismatches"]
            or item["reference_snapshots"] != item["psx_snapshots"]
            for item in report["entity_comparison"]
        )
    max_entity_error = getattr(args, "max_entity_error", None)
    if max_entity_error is not None:
        for item in report["entity_comparison"]:
            position = item.get("position")
            failed |= bool(position and position["max"] > max_entity_error)
    max_entity_angle_error = getattr(args, "max_entity_angle_error", None)
    if max_entity_angle_error is not None:
        for item in report["entity_comparison"]:
            angle = item.get("angle")
            failed |= bool(angle and angle["max_degrees"] > max_entity_angle_error)
    if args.max_train_error is not None:
        for position in report["positions"]:
            train = position.get("train")
            failed |= bool(train and train["max"] > args.max_train_error)
    if args.strict:
        failed |= bool(report["missing_events"])
    return 1 if failed else 0


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser()
    commands = root.add_subparsers(dest="command", required=True)

    normalize = commands.add_parser("normalize")
    normalize.add_argument("input", type=Path)
    normalize.add_argument("--output", type=Path)
    normalize.add_argument("--neutral-source", action="store_true")
    normalize.set_defaults(func=command_normalize)

    self_check = commands.add_parser("self-check")
    self_check.add_argument("left", type=Path)
    self_check.add_argument("right", type=Path)
    self_check.add_argument("--max-diff-lines", type=int, default=40)
    self_check.set_defaults(func=command_self_check)

    compare = commands.add_parser("compare")
    compare.add_argument("reference", type=Path)
    compare.add_argument("psx", type=Path)
    compare.add_argument("--output", type=Path)
    compare.add_argument(
        "--require-map",
        action="append",
        default=[],
        metavar="MAP[@N]",
        help="require a map visit; @N selects an exact repeated occurrence",
    )
    compare.add_argument(
        "--require-event",
        action="append",
        default=[],
        metavar="MAP[@N]:EVENT:DETAIL",
    )
    compare.add_argument(
        "--require-carry",
        action="append",
        default=[],
        metavar="MAP[@N]:in|out|train-in|train-out:ID",
    )
    compare.add_argument(
        "--require-attached-exit",
        action="append",
        default=[],
        metavar="MAP[@N]",
    )
    compare.add_argument("--max-train-error", type=float)
    compare.add_argument(
        "--max-entity-error",
        type=float,
        help="fail when a matched brush/actor checkpoint exceeds this distance",
    )
    compare.add_argument(
        "--max-entity-angle-error",
        type=float,
        help=(
            "fail when a matched func_rotating checkpoint exceeds this "
            "circular phase error in degrees"
        ),
    )
    compare.add_argument(
        "--require-input-parity",
        action="store_true",
        help="fail unless both traces contain byte-equivalent semantic input rows",
    )
    compare.add_argument(
        "--require-entity-parity",
        action="store_true",
        help="fail on missing/extra/inactive or mismatched entity state fields",
    )
    compare.add_argument("--strict", action="store_true")
    compare.set_defaults(func=command_compare)
    return root


def main() -> int:
    args = parser().parse_args()
    try:
        return args.func(args)
    except (OSError, ValueError) as error:
        print(error, file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
