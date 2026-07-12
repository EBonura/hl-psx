#!/usr/bin/env python3
"""Run the patched Xash/HLSDK deterministic Half-Life reference capture."""

from __future__ import annotations

import argparse
import hashlib
import math
import os
import subprocess
import sys
from pathlib import Path

from hlinput import TapeError, read_route


INTRO_MAPS = ("c0a0", "c0a0a", "c0a0b", "c0a0c", "c0a0d", "c0a0e")


def entity_interval(value: str) -> int:
    try:
        interval = int(value)
    except ValueError as error:
        raise argparse.ArgumentTypeError("must be an integer") from error
    if not 1 <= interval <= 1000:
        raise argparse.ArgumentTypeError("must be between 1 and 1000 ticks")
    return interval


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(
        description="Capture a deterministic 20 Hz GoldSrc reference trace."
    )
    result.add_argument(
        "--runtime-dir",
        type=Path,
        required=True,
        help="staged Xash runtime containing xash3d and patched HLSDK libraries",
    )
    result.add_argument(
        "--half-life-dir",
        type=Path,
        required=True,
        help="read-only Half-Life install containing valve/maps",
    )
    result.add_argument("--output", type=Path, required=True, help="trace output path")
    result.add_argument("--map", default="c0a0", help="initial map (default: c0a0)")
    result.add_argument("--seed", type=int, default=1337)
    result.add_argument("--max-ticks", type=int, default=6500)
    result.add_argument(
        "--entity-interval",
        type=entity_interval,
        default=20,
        metavar="TICKS",
        help=(
            "emit entity snapshots every N fixed ticks (default: 20; use 1 "
            "for short lifecycle probes)"
        ),
    )
    result.add_argument(
        "--semantic-input",
        type=Path,
        help="optional validated HLINPUT1 route (otherwise input is neutral)",
    )
    result.add_argument(
        "--initial-origin",
        type=float,
        nargs=3,
        metavar=("X", "Y", "Z"),
        help=(
            "reference-only GoldSrc checkpoint origin in Half-Life coordinates; "
            "applied once before the first player physics frame"
        ),
    )
    result.add_argument(
        "--initial-angles",
        type=float,
        nargs=3,
        metavar=("PITCH", "YAW", "ROLL"),
        help=(
            "reference-only initial GoldSrc view angles; use a neutral first "
            "input tick so the forced client view is acknowledged"
        ),
    )
    result.add_argument(
        "--framework-dir",
        type=Path,
        help="macOS directory containing SDL2.framework",
    )
    return result


def require_file(path: Path, description: str) -> None:
    if not path.is_file():
        raise SystemExit(f"missing {description}: {path}")


def format_vec3(values: list[float] | tuple[float, ...], option: str) -> str:
    """Validate and serialize an exact float32-friendly harness vector."""

    if len(values) != 3 or any(not math.isfinite(value) for value in values):
        raise SystemExit(f"{option} requires exactly three finite numbers")
    if any(abs(value) > 1_000_000.0 for value in values):
        raise SystemExit(f"{option} value is outside the harness safety range")
    return " ".join(format(value, ".9g") for value in values)


def capture(args: argparse.Namespace) -> tuple[int, tuple[str, ...], str]:
    runtime = args.runtime_dir.expanduser().resolve()
    half_life = args.half_life_dir.expanduser().resolve()
    output = args.output.expanduser().resolve()
    executable = runtime / "xash3d"

    if args.seed == 0:
        raise SystemExit("--seed must be nonzero (zero disables deterministic seeding)")
    if args.max_ticks <= 0:
        raise SystemExit("--max-ticks must be positive")
    require_file(executable, "Xash executable")
    require_file(half_life / "valve" / "maps" / f"{args.map}.bsp", "initial BSP")
    if not (runtime / "valve" / "dlls").is_dir():
        raise SystemExit(f"missing staged server game library directory: {runtime / 'valve/dlls'}")

    semantic_input = None
    if args.semantic_input is not None:
        semantic_input = args.semantic_input.expanduser().resolve()
        require_file(semantic_input, "HLINPUT1 semantic route")
        try:
            route = read_route(semantic_input)
        except TapeError as error:
            raise SystemExit(f"invalid --semantic-input: {error}") from error
        if route.segments[0].map != args.map:
            raise SystemExit(
                f"semantic route starts at {route.segments[0].map}, not requested map {args.map}"
            )

    environment = os.environ.copy()
    environment.update(
        {
            "HLREF_TRACE": "1",
            "HLREF_SEED": str(args.seed),
            "HLREF_MAX_TICKS": str(args.max_ticks),
            "HLREF_ENTITY_INTERVAL": str(args.entity_interval),
            "SDL_VIDEODRIVER": "dummy",
            "SDL_AUDIODRIVER": "dummy",
            "XASH3D_BASEDIR": str(runtime),
            "XASH3D_RODIR": str(half_life),
        }
    )
    environment.pop("HLREF_INITIAL_ORIGIN", None)
    environment.pop("HLREF_INITIAL_ANGLES", None)
    if args.initial_origin is not None:
        environment["HLREF_INITIAL_ORIGIN"] = format_vec3(
            args.initial_origin, "--initial-origin"
        )
    if args.initial_angles is not None:
        environment["HLREF_INITIAL_ANGLES"] = format_vec3(
            args.initial_angles, "--initial-angles"
        )
    if semantic_input is None:
        environment["HLREF_NEUTRAL_INPUT"] = "1"
        environment.pop("HLREF_SEMANTIC_INPUT", None)
    else:
        environment["HLREF_NEUTRAL_INPUT"] = "0"
        environment["HLREF_SEMANTIC_INPUT"] = str(semantic_input)
    if args.framework_dir is not None:
        framework_dir = str(args.framework_dir.expanduser().resolve())
        previous = environment.get("DYLD_FRAMEWORK_PATH")
        environment["DYLD_FRAMEWORK_PATH"] = (
            f"{framework_dir}{os.pathsep}{previous}" if previous else framework_dir
        )

    command = [
        str(executable),
        "-ref",
        "null",
        "-dev",
        "1",
        "-log",
        "-nowriteconfig",
        "-noip",
        "-noip6",
        "+map",
        args.map,
        "+host_framerate",
        "0.05",
        "+fps_max",
        "0",
        "+volume",
        "0",
    ]

    output.parent.mkdir(parents=True, exist_ok=True)
    temporary = output.with_name(f".{output.name}.tmp")
    record_count = 0
    maps: list[str] = []
    digest = hashlib.sha256()

    try:
        with temporary.open("wb") as trace:
            process = subprocess.Popen(
                command,
                cwd=runtime,
                env=environment,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
            )
            assert process.stdout is not None
            for raw_line in process.stdout:
                marker = raw_line.find(b"HLREF|")
                if marker < 0:
                    continue
                line = raw_line[marker:].rstrip(b"\r\n") + b"\n"
                trace.write(line)
                digest.update(line)
                record_count += 1
                if line.startswith(b"HLREF|map|"):
                    fields = dict(
                        item.split("=", 1)
                        for item in line.decode("utf-8", "replace").strip().split("|")[2:]
                        if "=" in item
                    )
                    maps.append(fields.get("map", ""))
            return_code = process.wait()
        if return_code != 0:
            raise SystemExit(f"Xash exited with status {return_code}")
        if record_count == 0:
            raise SystemExit("capture completed without any HLREF records")

        lines = temporary.read_bytes().splitlines()
        expected_stop = f"HLREF|stop|map=".encode()
        if not lines[-1].startswith(expected_stop) or (
            f"|tick={args.max_ticks}|reason=max_ticks".encode() not in lines[-1]
        ):
            raise SystemExit(
                f"capture did not reach deterministic stop tick {args.max_ticks}: "
                f"last record was {lines[-1].decode('utf-8', 'replace')}"
            )
        if args.map == "c0a0" and tuple(maps[: len(INTRO_MAPS)]) != INTRO_MAPS:
            raise SystemExit(
                "opening ride did not traverse the expected map sequence: " + ", ".join(maps)
            )
        temporary.replace(output)
    finally:
        if temporary.exists():
            temporary.unlink()

    return record_count, tuple(maps), digest.hexdigest()


def main() -> int:
    args = parser().parse_args()
    records, maps, checksum = capture(args)
    print(f"wrote {records} records to {args.output}")
    print(f"maps: {', '.join(maps)}")
    print(f"sha256: {checksum}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
