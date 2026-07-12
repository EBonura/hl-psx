#!/usr/bin/env python3
"""Map-segmented 20 Hz semantic input tapes for GoldSrc/hl-psx comparison.

``HLINPUT1`` is deliberately separate from PSoXide's vblank-clocked
``PXITAPE1``.  A route is split at map boundaries and every expanded sample is
indexed by the local fixed-physics tick, so loading time cannot consume input.
"""

from __future__ import annotations

import argparse
import re
import struct
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable


MAGIC = b"HLINPUT1"
TICK_HZ = 20
HEADER = struct.Struct("<8sHHI")
SEGMENT_HEADER = struct.Struct("<16s16sIIII")
RUN = struct.Struct("<HbbbbH")

ACTION_ATTACK = 1 << 0
ACTION_JUMP = 1 << 1
ACTION_DUCK = 1 << 2
ACTION_USE = 1 << 3
ACTION_ATTACK2 = 1 << 4
ACTION_RELOAD = 1 << 5
ACTION_NEXT_WEAPON = 1 << 6
ACTION_PREV_WEAPON = 1 << 7
ACTION_FLASHLIGHT = 1 << 8
KNOWN_ACTIONS = (1 << 9) - 1

NEUTRAL = None  # assigned after InputSample is defined
MAP_NAME = re.compile(r"^[A-Za-z0-9_-]{1,15}$")
TRACE_MARKER = "HLPSX|input|"


class TapeError(ValueError):
    """A semantic tape is malformed or was consumed out of sequence."""


@dataclass(frozen=True)
class InputSample:
    forward: int = 0
    strafe: int = 0
    turn: int = 0
    look: int = 0
    actions: int = 0

    def validate(self) -> None:
        for name in ("forward", "strafe", "turn", "look"):
            value = getattr(self, name)
            if not -128 <= value <= 127:
                raise TapeError(f"{name}={value} does not fit signed 8-bit input")
        unknown = self.actions & ~KNOWN_ACTIONS
        if unknown:
            raise TapeError(f"unknown semantic action bits 0x{unknown:04x}")
        if not 0 <= self.actions <= 0xFFFF:
            raise TapeError(f"actions={self.actions} does not fit u16")


NEUTRAL = InputSample()


@dataclass(frozen=True)
class InputRun:
    ticks: int
    sample: InputSample

    def validate(self) -> None:
        if not 1 <= self.ticks <= 0xFFFF:
            raise TapeError(f"run duration {self.ticks} is outside 1..65535")
        self.sample.validate()


@dataclass(frozen=True)
class Segment:
    map: str
    next_map: str
    runs: tuple[InputRun, ...]
    neutral_tail_ticks: int = 200
    flags: int = 0

    @property
    def total_ticks(self) -> int:
        return sum(run.ticks for run in self.runs)

    def validate(self) -> None:
        _validate_map_name(self.map, allow_empty=False)
        _validate_map_name(self.next_map, allow_empty=True)
        if not self.runs:
            raise TapeError(f"segment {self.map} has no input runs")
        if self.total_ticks > 0xFFFFFFFF:
            raise TapeError(f"segment {self.map} exceeds u32 tick count")
        if not 0 <= self.neutral_tail_ticks <= 0xFFFFFFFF:
            raise TapeError(f"segment {self.map} has invalid neutral tail")
        if self.flags != 0:
            raise TapeError(f"segment {self.map} has unknown flags 0x{self.flags:08x}")
        for run in self.runs:
            run.validate()

    def expand(self) -> tuple[InputSample, ...]:
        self.validate()
        return tuple(sample for run in self.runs for sample in (run.sample,) * run.ticks)

    def sample_at(self, tick: int) -> InputSample:
        if tick < 0:
            raise TapeError(f"negative local input tick {tick}")
        cursor = 0
        for run in self.runs:
            if tick < cursor + run.ticks:
                return run.sample
            cursor += run.ticks
        if tick < self.total_ticks + self.neutral_tail_ticks:
            return NEUTRAL
        raise TapeError(
            f"segment {self.map} exhausted at local tick {tick} "
            f"(route={self.total_ticks}, neutral_tail={self.neutral_tail_ticks})"
        )


@dataclass(frozen=True)
class Route:
    segments: tuple[Segment, ...]
    flags: int = 0

    def validate(self) -> None:
        if not self.segments:
            raise TapeError("semantic route has no map segments")
        if len(self.segments) > 0xFFFF:
            raise TapeError("semantic route has too many segments")
        if self.flags != 0:
            raise TapeError(f"route has unknown flags 0x{self.flags:08x}")
        for index, segment in enumerate(self.segments):
            segment.validate()
            expected = self.segments[index + 1].map if index + 1 < len(self.segments) else ""
            if segment.next_map != expected:
                raise TapeError(
                    f"segment {index} ({segment.map}) expects {segment.next_map!r}; "
                    f"ordered next segment is {expected!r}"
                )


class RouteCursor:
    """Strict map/tick consumer used by tests and host replay adapters."""

    def __init__(self, route: Route):
        route.validate()
        self.route = route
        self.segment_index = -1
        self.next_tick = 0

    @property
    def segment(self) -> Segment:
        if self.segment_index < 0:
            raise TapeError("route has not received its first map_start")
        return self.route.segments[self.segment_index]

    def begin_map(self, map_name: str) -> None:
        wanted = self.segment_index + 1
        if wanted >= len(self.route.segments):
            raise TapeError(f"unexpected map {map_name!r} after final route segment")
        expected = self.route.segments[wanted].map
        if map_name != expected:
            raise TapeError(f"unexpected map {map_name!r}; route requires {expected!r}")
        self.segment_index = wanted
        self.next_tick = 0

    def consume(self, map_name: str, local_tick: int) -> InputSample:
        if map_name != self.segment.map:
            raise TapeError(
                f"input requested for map {map_name!r} while segment {self.segment.map!r} is active"
            )
        if local_tick != self.next_tick:
            raise TapeError(
                f"map {map_name} requested input tick {local_tick}; expected {self.next_tick}"
            )
        sample = self.segment.sample_at(local_tick)
        self.next_tick += 1
        return sample


def _validate_map_name(name: str, *, allow_empty: bool) -> None:
    if allow_empty and not name:
        return
    if not MAP_NAME.fullmatch(name):
        raise TapeError(f"invalid map name {name!r}; expected 1..15 ASCII map characters")


def _encode_name(name: str, *, allow_empty: bool) -> bytes:
    _validate_map_name(name, allow_empty=allow_empty)
    raw = name.encode("ascii")
    return raw + bytes(16 - len(raw))


def _decode_name(raw: bytes, *, allow_empty: bool) -> str:
    nul = raw.find(b"\0")
    if nul < 0:
        nul = len(raw)
    if any(raw[nul:]):
        raise TapeError("map-name field contains nonzero bytes after its terminator")
    try:
        name = raw[:nul].decode("ascii")
    except UnicodeDecodeError as error:
        raise TapeError("map-name field is not ASCII") from error
    _validate_map_name(name, allow_empty=allow_empty)
    return name


def encode(route: Route) -> bytes:
    route.validate()
    data = bytearray(HEADER.pack(MAGIC, TICK_HZ, len(route.segments), route.flags))
    for segment in route.segments:
        data += SEGMENT_HEADER.pack(
            _encode_name(segment.map, allow_empty=False),
            _encode_name(segment.next_map, allow_empty=True),
            segment.total_ticks,
            len(segment.runs),
            segment.neutral_tail_ticks,
            segment.flags,
        )
        for run in segment.runs:
            run.validate()
            sample = run.sample
            data += RUN.pack(
                run.ticks,
                sample.forward,
                sample.strafe,
                sample.turn,
                sample.look,
                sample.actions,
            )
    return bytes(data)


def decode(data: bytes) -> Route:
    if len(data) < HEADER.size:
        raise TapeError("semantic input is shorter than its header")
    magic, tick_hz, segment_count, flags = HEADER.unpack_from(data)
    if magic != MAGIC:
        raise TapeError("unknown semantic input magic")
    if tick_hz != TICK_HZ:
        raise TapeError(f"semantic input tick rate is {tick_hz}, expected {TICK_HZ}")
    if segment_count == 0:
        raise TapeError("semantic input has zero segments")

    offset = HEADER.size
    segments: list[Segment] = []
    for segment_index in range(segment_count):
        if offset + SEGMENT_HEADER.size > len(data):
            raise TapeError(f"truncated segment header {segment_index}")
        raw_map, raw_next, total_ticks, run_count, tail, segment_flags = SEGMENT_HEADER.unpack_from(
            data, offset
        )
        offset += SEGMENT_HEADER.size
        map_name = _decode_name(raw_map, allow_empty=False)
        next_map = _decode_name(raw_next, allow_empty=True)
        if run_count == 0:
            raise TapeError(f"segment {map_name} has zero runs")
        runs: list[InputRun] = []
        for run_index in range(run_count):
            if offset + RUN.size > len(data):
                raise TapeError(f"truncated run {run_index} in segment {map_name}")
            ticks, forward, strafe, turn, look, actions = RUN.unpack_from(data, offset)
            offset += RUN.size
            run = InputRun(ticks, InputSample(forward, strafe, turn, look, actions))
            run.validate()
            runs.append(run)
        segment = Segment(map_name, next_map, tuple(runs), tail, segment_flags)
        if segment.total_ticks != total_ticks:
            raise TapeError(
                f"segment {map_name} declares {total_ticks} ticks but runs expand to "
                f"{segment.total_ticks}"
            )
        segments.append(segment)
    if offset != len(data):
        raise TapeError(f"semantic input has {len(data) - offset} trailing bytes")
    route = Route(tuple(segments), flags)
    route.validate()
    return route


def read_route(path: Path) -> Route:
    try:
        return decode(path.read_bytes())
    except OSError as error:
        raise TapeError(f"read {path}: {error}") from error


def write_route(path: Path, route: Route) -> None:
    if path.parent != Path(""):
        path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(encode(route))


def rle(samples: Iterable[InputSample]) -> tuple[InputRun, ...]:
    runs: list[InputRun] = []
    for sample in samples:
        sample.validate()
        if runs and runs[-1].sample == sample and runs[-1].ticks < 0xFFFF:
            previous = runs[-1]
            runs[-1] = InputRun(previous.ticks + 1, sample)
        else:
            runs.append(InputRun(1, sample))
    return tuple(runs)


def _parse_int(value: str, field: str) -> int:
    try:
        return int(value, 0)
    except ValueError as error:
        raise TapeError(f"invalid {field} value {value!r}") from error


def extract_trace(lines: Iterable[str], neutral_tail_ticks: int = 200) -> Route:
    if not 0 <= neutral_tail_ticks <= 0xFFFFFFFF:
        raise TapeError("neutral tail must fit u32")
    grouped: list[tuple[str, list[InputSample]]] = []
    current_map = ""
    expected_tick = 0

    for line_number, line in enumerate(lines, 1):
        marker = line.find(TRACE_MARKER)
        if marker < 0:
            continue
        fields: dict[str, str] = {}
        for item in line[marker:].strip().split("|")[2:]:
            if "=" in item:
                key, value = item.split("=", 1)
                fields[key] = value
        missing = [
            key
            for key in ("map", "tick", "forward", "strafe", "turn", "look", "actions")
            if key not in fields
        ]
        if missing:
            raise TapeError(f"line {line_number}: input row lacks {', '.join(missing)}")
        map_name = fields["map"]
        tick = _parse_int(fields["tick"], "tick")
        if map_name != current_map:
            if tick != 0:
                raise TapeError(f"line {line_number}: first input tick for {map_name} is {tick}, not 0")
            grouped.append((map_name, []))
            current_map = map_name
            expected_tick = 0
        if tick != expected_tick:
            raise TapeError(
                f"line {line_number}: map {map_name} input tick {tick}; expected {expected_tick}"
            )
        sample = InputSample(
            _parse_int(fields["forward"], "forward"),
            _parse_int(fields["strafe"], "strafe"),
            _parse_int(fields["turn"], "turn"),
            _parse_int(fields["look"], "look"),
            _parse_int(fields["actions"], "actions"),
        )
        sample.validate()
        grouped[-1][1].append(sample)
        expected_tick += 1

    if not grouped:
        raise TapeError("trace contains no HLPSX|input rows")
    segments = []
    for index, (map_name, samples) in enumerate(grouped):
        next_map = grouped[index + 1][0] if index + 1 < len(grouped) else ""
        segments.append(Segment(map_name, next_map, rle(samples), neutral_tail_ticks))
    route = Route(tuple(segments))
    route.validate()
    return route


def _sample_line(map_name: str, tick: int, sample: InputSample) -> str:
    return (
        f"HLPSX|input|map={map_name}|tick={tick}|forward={sample.forward}"
        f"|strafe={sample.strafe}|turn={sample.turn}|look={sample.look}"
        f"|actions=0x{sample.actions:04x}"
    )


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    validate = sub.add_parser("validate", help="validate and summarize an HLINPUT1 tape")
    validate.add_argument("tape", type=Path)

    expand = sub.add_parser("expand", help="expand one map segment to HLPSX input rows")
    expand.add_argument("tape", type=Path)
    expand.add_argument("--map", required=True)
    expand.add_argument("--occurrence", type=int, default=1)

    extract = sub.add_parser("extract", help="extract HLINPUT1 from HLPSX|input trace rows")
    extract.add_argument("trace", type=Path)
    extract.add_argument("output", type=Path)
    extract.add_argument("--neutral-tail-ticks", type=int, default=200)
    return parser


def main() -> int:
    args = _parser().parse_args()
    try:
        if args.command == "validate":
            route = read_route(args.tape)
            ticks = sum(segment.total_ticks for segment in route.segments)
            runs = sum(len(segment.runs) for segment in route.segments)
            print(f"valid HLINPUT1: {len(route.segments)} maps, {ticks} ticks, {runs} runs")
            print("maps: " + ", ".join(segment.map for segment in route.segments))
        elif args.command == "expand":
            route = read_route(args.tape)
            matches = [segment for segment in route.segments if segment.map == args.map]
            if args.occurrence <= 0 or args.occurrence > len(matches):
                raise TapeError(
                    f"map {args.map!r} occurrence {args.occurrence} not present in route"
                )
            segment = matches[args.occurrence - 1]
            for tick, sample in enumerate(segment.expand()):
                print(_sample_line(segment.map, tick, sample))
        else:
            route = extract_trace(
                args.trace.read_text(encoding="utf-8", errors="replace").splitlines(),
                args.neutral_tail_ticks,
            )
            write_route(args.output, route)
            print(
                f"wrote {len(route.segments)} maps / "
                f"{sum(segment.total_ticks for segment in route.segments)} ticks to {args.output}"
            )
    except (OSError, TapeError) as error:
        raise SystemExit(f"hlinput: {error}") from error
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
