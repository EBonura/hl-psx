#!/usr/bin/env python3
"""Run and report a reproducible visual-performance audit across all hl-psx maps.

Each map is booted directly through the ``debug-map-boot`` pad protocol, then
measured with PSoXide's real rendered-frame telemetry.  The report deliberately
uses visual endpoint-to-endpoint delivery time rather than the fixed simulation
tick, which can remain near 20 Hz while rendering falls behind.

The authoritative workflow is::

    python3 tools/perf/fleet_audit.py run --build --build-emulator \
        --out captures/perf-fleet

The run is resumable.  Raw profiles, counter logs, visual hashes, final display
images, console logs, exact build identities, CSV/JSON summaries, and a Markdown
report are retained under the output directory.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import csv
import hashlib
import json
import os
import platform
import re
import shlex
import statistics
import subprocess
import sys
import time
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterable, Sequence


SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parents[1]
TOOLS_DIR = REPO_ROOT / "tools"
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))
if str(TOOLS_DIR) not in sys.path:
    sys.path.insert(0, str(TOOLS_DIR))

from memory_report import LOAD_ADDR, STATIC_LIMIT, parse_map  # noqa: E402
from visual_perf_gate import (  # noqa: E402
    GatePolicy,
    PerfDataError,
    PSX_CPU_HZ,
    ScenarioSpec,
    analyze_scenario,
    git_identity,
    nearest_rank,
)


SCHEMA = 1
DEFAULT_VISUAL_FRAMES = 64
DEFAULT_WARMUP_VISUALS = 4
DEFAULT_GUEST_FRAMES = 1200
DEFAULT_STEPS = 800_000_000
DEFAULT_TIMEOUT_SECONDS = 600
DEFAULT_FEATURES = "emulator-telemetry,debug-map-boot"
MAP_BOOT_HOLD_TICKS = 180

EXTRA_PROFILE_COLUMNS = {
    "visual_frames",
    "tri_primitives",
    "room_surf_whole_quads",
}

LOAD_ERROR_MARKERS = (
    "WORLD.PAK texture stream failed",
    "WORLD.PAK texture chunk invalid",
    "WORLD.PAK viewmodel stream failed",
    "WORLD.PAK world stream failed",
    "texture/world count mismatch",
)


class FleetError(RuntimeError):
    """A fleet configuration, capture, or report is not trustworthy."""


@dataclass(frozen=True)
class StaticRam:
    linker_map: str
    linker_map_sha256: str
    used_bytes: int
    headroom_bytes: int
    sections: dict[str, int]


@dataclass(frozen=True)
class ExtraMetrics:
    packet_p50: int
    packet_p95: int
    packet_peak: int
    triangle_equiv_p50: int
    triangle_equiv_p95: int
    triangle_equiv_peak: int


@dataclass
class MapResult:
    map_index: int
    map_name: str
    capture_status: str
    gate_pass: bool | None
    error: str | None
    boot_verified: bool
    load_anomalies: list[str]
    host_seconds: float | None
    profile_csv: str
    counter_csv: str
    visual_hash_csv: str
    final_display: str | None
    console_log: str
    visual_samples: int | None = None
    delivery_samples: int | None = None
    actual_visual_fps: float | None = None
    delivery_cycles_p50: int | None = None
    delivery_cycles_p95: int | None = None
    visual_task_cycles_p50: int | None = None
    visual_task_cycles_p95: int | None = None
    render_cycles_p50: int | None = None
    render_cycles_p95: int | None = None
    deadline_misses: int | None = None
    deadline_miss_rate: float | None = None
    skipped_vblanks: int | None = None
    max_lateness_vblanks: int | None = None
    packet_capacity: int | None = None
    peak_primitives: int | None = None
    min_packet_slots_free: int | None = None
    peak_packet_utilization: float | None = None
    packet_overflow_events: int | None = None
    packet_p50: int | None = None
    packet_p95: int | None = None
    packet_peak: int | None = None
    triangle_equiv_p50: int | None = None
    triangle_equiv_p95: int | None = None
    triangle_equiv_peak: int | None = None
    gate_failures: list[str] | None = None

    @property
    def status(self) -> str:
        if self.capture_status != "ok":
            return "ERROR"
        return "PASS" if self.gate_pass else "FAIL"


@dataclass(frozen=True)
class CaptureConfig:
    disc: Path
    frontend: Path
    psoxide_root: Path
    visual_frames: int
    guest_frames: int
    steps: int
    timeout_seconds: int
    warmup_visuals: int
    route: str
    screenshot: bool


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def canonical_hash(value: Any) -> str:
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(encoded).hexdigest()


def _makefile_maps(path: Path) -> list[str]:
    text = path.read_text(encoding="utf-8")
    match = re.search(r"^MAPLIST := \\\n((?:\t.*(?:\\)?\n)+)", text, re.MULTILINE)
    if not match:
        raise FleetError(f"MAPLIST not found in {path}")
    return re.findall(r"\bc\d[a-z0-9]*\b", match.group(1))


def _menu_maps(path: Path) -> list[str]:
    text = path.read_text(encoding="utf-8")
    match = re.search(
        r"pub const MAPS:\s*\[&str;\s*(\d+)\]\s*=\s*\[(.*?)\];",
        text,
        re.DOTALL,
    )
    if not match:
        raise FleetError(f"MAPS registry not found in {path}")
    declared = int(match.group(1))
    maps = re.findall(r'"([a-z0-9]+)"', match.group(2))
    if len(maps) != declared:
        raise FleetError(
            f"{path}: MAPS declares {declared} entries but contains {len(maps)}"
        )
    return maps


def load_map_registry(repo_root: Path = REPO_ROOT) -> list[str]:
    menu_maps = _menu_maps(repo_root / "game/src/menu.rs")
    make_maps = _makefile_maps(repo_root / "Makefile")
    if menu_maps != make_maps:
        raise FleetError("game/src/menu.rs MAPS and Makefile MAPLIST differ")
    if len(set(menu_maps)) != len(menu_maps):
        raise FleetError("map registry contains duplicate names")
    return menu_maps


def parse_map_selection(expression: str, maps: Sequence[str]) -> list[int]:
    value = expression.strip()
    if value.lower() == "all":
        return list(range(len(maps)))
    by_name = {name: index for index, name in enumerate(maps)}
    selected: list[int] = []
    for token in (part.strip() for part in value.split(",")):
        if not token:
            continue
        if token in by_name:
            candidates: Iterable[int] = (by_name[token],)
        elif re.fullmatch(r"\d+", token):
            candidates = (int(token),)
        elif re.fullmatch(r"\d+-\d+", token):
            first, last = (int(part) for part in token.split("-", 1))
            if first > last:
                raise FleetError(f"descending map range is not allowed: {token}")
            candidates = range(first, last + 1)
        else:
            raise FleetError(f"unknown map selector: {token!r}")
        for index in candidates:
            if not 0 <= index < len(maps):
                raise FleetError(f"map index out of range: {index}")
            if index not in selected:
                selected.append(index)
    if not selected:
        raise FleetError("map selection is empty")
    return selected


def map_boot_route(index: int) -> str:
    if not 0 <= index <= 0xFF:
        raise FleetError(f"debug-map-boot only carries an 8-bit map index: {index}")
    return f"0x{0x0400 | index:04x}@1+{MAP_BOOT_HOLD_TICKS}"


def policy_from_args(args: argparse.Namespace) -> GatePolicy:
    return GatePolicy(
        target_fps=args.target_fps,
        clock_hz=args.clock_hz,
        tolerance_fraction=args.tolerance_percent / 100.0,
        min_visual_samples=args.min_visual_samples,
        max_deadline_miss_rate=args.max_deadline_miss_rate,
        max_skipped_vblanks=args.max_skipped_vblanks,
        max_packet_overflows=args.max_packet_overflows,
        min_packet_slots_free=args.min_packet_slots_free,
    )


def validate_policy(policy: GatePolicy) -> None:
    if policy.target_fps <= 0 or policy.clock_hz <= 0:
        raise FleetError("target FPS and clock must be positive")
    if not 0 <= policy.tolerance_fraction < 1:
        raise FleetError("tolerance percent must be in [0, 100)")
    if not 0 <= policy.max_deadline_miss_rate <= 1:
        raise FleetError("max deadline miss rate must be in [0, 1]")


def parse_static_ram(path: Path) -> StaticRam:
    symbols, sections, _entries = parse_map(path)
    bss_end = symbols.get("__bss_end")
    if bss_end is None:
        raise FleetError(f"{path}: __bss_end not found")
    return StaticRam(
        linker_map=str(path.resolve()),
        linker_map_sha256=sha256_file(path),
        used_bytes=bss_end - LOAD_ADDR,
        headroom_bytes=STATIC_LIMIT - bss_end,
        sections=sections,
    )


def visual_rows(path: Path, warmup_visuals: int) -> list[dict[str, int]]:
    with path.open(newline="", encoding="utf-8") as handle:
        reader = csv.DictReader(handle)
        columns = set(reader.fieldnames or ())
        missing = EXTRA_PROFILE_COLUMNS - columns
        if missing:
            raise FleetError(
                f"{path}: missing triangle/packet columns: {', '.join(sorted(missing))}"
            )
        rows: list[dict[str, int]] = []
        for row_number, raw in enumerate(reader, start=2):
            try:
                visual = int(raw["visual_frames"])
                packets = int(raw["tri_primitives"])
                quads = int(raw["room_surf_whole_quads"])
            except (TypeError, ValueError) as exc:
                raise FleetError(f"{path}: invalid integer on row {row_number}") from exc
            if min(visual, packets, quads) < 0:
                raise FleetError(f"{path}: negative telemetry on row {row_number}")
            if visual == 1:
                rows.append({"packets": packets, "quads": quads})
            elif visual != 0:
                raise FleetError(
                    f"{path}: visual_frames={visual} on row {row_number}; expected 0 or 1"
                )
    if len(rows) <= warmup_visuals:
        raise FleetError(
            f"{path}: only {len(rows)} visual rows; cannot discard {warmup_visuals}"
        )
    return rows[warmup_visuals:]


def _median_int(values: Sequence[int]) -> int:
    return int(round(statistics.median(values)))


def extra_metrics(path: Path, warmup_visuals: int) -> ExtraMetrics:
    rows = visual_rows(path, warmup_visuals)
    packets = [row["packets"] for row in rows]
    # A transient quad consumes one arena packet but represents two GPU
    # triangles. Persistent tram/viewmodel packet caches are not reflected in
    # TRI_PRIMITIVES, so this remains an explicitly documented lower bound.
    triangle_equiv = [row["packets"] + row["quads"] for row in rows]
    return ExtraMetrics(
        packet_p50=_median_int(packets),
        packet_p95=nearest_rank(packets, 0.95),
        packet_peak=max(packets),
        triangle_equiv_p50=_median_int(triangle_equiv),
        triangle_equiv_p95=nearest_rank(triangle_equiv, 0.95),
        triangle_equiv_peak=max(triangle_equiv),
    )


def verify_boot(console: str, map_name: str) -> bool:
    pattern = re.compile(rf"\]\s+{re.escape(map_name)}\s*$", re.MULTILINE)
    return bool(pattern.search(console))


def find_load_anomalies(console: str) -> list[str]:
    return [marker for marker in LOAD_ERROR_MARKERS if marker in console]


def _empty_result(
    index: int,
    name: str,
    map_dir: Path,
    *,
    error: str,
    host_seconds: float | None = None,
    console: str = "",
) -> MapResult:
    return MapResult(
        map_index=index,
        map_name=name,
        capture_status="error",
        gate_pass=None,
        error=error,
        boot_verified=verify_boot(console, name),
        load_anomalies=find_load_anomalies(console),
        host_seconds=host_seconds,
        profile_csv=str(map_dir / "profile.csv"),
        counter_csv=str(map_dir / "counter.csv"),
        visual_hash_csv=str(map_dir / "visual-hash.csv"),
        final_display=str(map_dir / "final.ppm") if (map_dir / "final.ppm").exists() else None,
        console_log=str(map_dir / "console.log"),
    )


def analyze_map_capture(
    index: int,
    name: str,
    map_dir: Path,
    policy: GatePolicy,
    warmup_visuals: int,
    *,
    host_seconds: float | None = None,
) -> MapResult:
    profile = map_dir / "profile.csv"
    console_path = map_dir / "console.log"
    try:
        console = console_path.read_text(encoding="utf-8", errors="replace")
    except OSError as exc:
        return _empty_result(index, name, map_dir, error=str(exc))
    boot_verified = verify_boot(console, name)
    anomalies = find_load_anomalies(console)
    if not boot_verified:
        return _empty_result(
            index,
            name,
            map_dir,
            error=f"console does not prove direct boot of {name}",
            host_seconds=host_seconds,
            console=console,
        )
    if anomalies:
        return _empty_result(
            index,
            name,
            map_dir,
            error="; ".join(anomalies),
            host_seconds=host_seconds,
            console=console,
        )
    try:
        perf = analyze_scenario(
            ScenarioSpec(name, profile),
            warmup_visuals=warmup_visuals,
            policy=policy,
        )
        extra = extra_metrics(profile, warmup_visuals)
    except (PerfDataError, FleetError, OSError) as exc:
        return _empty_result(
            index,
            name,
            map_dir,
            error=str(exc),
            host_seconds=host_seconds,
            console=console,
        )
    return MapResult(
        map_index=index,
        map_name=name,
        capture_status="ok",
        gate_pass=not perf.gate_failures,
        error=None,
        boot_verified=True,
        load_anomalies=[],
        host_seconds=host_seconds,
        profile_csv=str(profile),
        counter_csv=str(map_dir / "counter.csv"),
        visual_hash_csv=str(map_dir / "visual-hash.csv"),
        final_display=str(map_dir / "final.ppm") if (map_dir / "final.ppm").exists() else None,
        console_log=str(console_path),
        visual_samples=perf.visual_samples,
        delivery_samples=perf.delivery_samples,
        actual_visual_fps=perf.actual_visual_fps,
        delivery_cycles_p50=perf.delivery_cycles_p50,
        delivery_cycles_p95=perf.delivery_cycles_p95,
        visual_task_cycles_p50=perf.visual_task_cycles_p50,
        visual_task_cycles_p95=perf.visual_task_cycles_p95,
        render_cycles_p50=perf.render_cycles_p50,
        render_cycles_p95=perf.render_cycles_p95,
        deadline_misses=perf.deadline_misses,
        deadline_miss_rate=perf.deadline_miss_rate,
        skipped_vblanks=perf.skipped_vblanks,
        max_lateness_vblanks=perf.max_lateness_vblanks,
        packet_capacity=perf.packet_capacity,
        peak_primitives=perf.peak_primitives,
        min_packet_slots_free=perf.min_packet_slots_free,
        peak_packet_utilization=perf.peak_packet_utilization,
        packet_overflow_events=perf.packet_overflow_events,
        packet_p50=extra.packet_p50,
        packet_p95=extra.packet_p95,
        packet_peak=extra.packet_peak,
        triangle_equiv_p50=extra.triangle_equiv_p50,
        triangle_equiv_p95=extra.triangle_equiv_p95,
        triangle_equiv_peak=extra.triangle_equiv_peak,
        gate_failures=perf.gate_failures,
    )


def resolve_frontend(psoxide_root: Path, override: Path | None) -> Path:
    if override is not None:
        expanded = override.expanduser().resolve()
        if expanded.is_file() and os.access(expanded, os.X_OK):
            return expanded
        raise FleetError(f"PSoXide frontend is not executable: {expanded}")
    candidates = [
        (psoxide_root / "emu/target/release/frontend").resolve(),
        (psoxide_root / "target/release/frontend").resolve(),
    ]
    usable = [path for path in candidates if path.is_file() and os.access(path, os.X_OK)]
    if usable:
        # Workspaces may have both a historical emu-local target and the active
        # shared target. Selecting the newest avoids silently running the stale
        # copy after `cargo build -p frontend --release`.
        return max(usable, key=lambda path: path.stat().st_mtime_ns)
    raise FleetError(
        "PSoXide frontend not found; pass --frontend or run with --build-emulator"
    )


def build_game(repo_root: Path, psoxide_root: Path, out_dir: Path) -> Path:
    linker_map = (out_dir / "build/hl-psx.map").resolve()
    linker_map.parent.mkdir(parents=True, exist_ok=True)
    if " " in str(linker_map):
        raise FleetError("--build output path cannot contain spaces (RUSTFLAGS limitation)")
    env = os.environ.copy()
    prefix = env.get("RUSTFLAGS", "").strip()
    map_flags = f"-Clink-arg=-Map -Clink-arg={linker_map}"
    env["RUSTFLAGS"] = f"{prefix} {map_flags}".strip()
    # Keep the PSX-only linker flags out of mkisopsx's native Apple/Linux link.
    # The Makefile's `disc` target inherits RUSTFLAGS into both cargo commands,
    # so perform its two phases explicitly here.
    command = ["cargo", "build", "--release", "--features", DEFAULT_FEATURES]
    env["PSOXIDE"] = str(psoxide_root)
    print("BUILD GAME:", shlex.join(command), flush=True)
    completed = subprocess.run(command, cwd=repo_root / "game", env=env, check=False)
    if completed.returncode != 0:
        raise FleetError(f"game build failed with exit code {completed.returncode}")
    if not linker_map.is_file():
        raise FleetError(f"build did not produce linker map: {linker_map}")
    exe = repo_root / "game/target/mipsel-sony-psx/release/hl-psx.exe"
    disc_bin = repo_root / "dist/hl-psx.bin"
    disc_bin.parent.mkdir(parents=True, exist_ok=True)
    pack = [
        "cargo",
        "run",
        "--release",
        "--",
        "--exe",
        str(exe),
        "--out",
        str(disc_bin),
        "--volume",
        "HLPSX",
        "--world-pack-rooms-dir",
        str(repo_root / "data/rooms"),
        "--world-pack-compress-rooms",
    ]
    for directory in ("modelpack", "sfx", "voices", "sprites"):
        pack.extend(["--world-pack-extra-dir", str(repo_root / "data" / directory)])
    track_list = repo_root / "data/music/tracks.txt"
    if track_list.is_file():
        pack.extend(["--cdda-track-list", str(track_list)])
    print("PACK DISC:", shlex.join(pack), flush=True)
    completed = subprocess.run(
        pack, cwd=psoxide_root / "tools/mkisopsx", check=False
    )
    if completed.returncode != 0:
        raise FleetError(f"disc pack failed with exit code {completed.returncode}")
    return linker_map


def build_emulator(psoxide_root: Path) -> None:
    command = ["cargo", "build", "-p", "frontend", "--release"]
    print("BUILD EMULATOR:", shlex.join(command), flush=True)
    completed = subprocess.run(
        command, cwd=psoxide_root / "emu", check=False
    )
    if completed.returncode != 0:
        raise FleetError(f"PSoXide build failed with exit code {completed.returncode}")


def cue_files(cue_path: Path) -> list[Path]:
    paths = [cue_path.resolve()]
    if cue_path.suffix.lower() != ".cue":
        return paths
    text = cue_path.read_text(encoding="utf-8", errors="replace")
    for raw in re.findall(r'^\s*FILE\s+"([^"]+)"', text, re.MULTILINE | re.IGNORECASE):
        path = (cue_path.parent / raw).resolve()
        if path not in paths:
            paths.append(path)
    return paths


def artifact_identity(disc: Path) -> list[dict[str, Any]]:
    result = []
    for path in cue_files(disc):
        if not path.is_file():
            raise FleetError(f"disc artifact does not exist: {path}")
        result.append(
            {"path": str(path), "size": path.stat().st_size, "sha256": sha256_file(path)}
        )
    return result


def capture_signature(
    config: CaptureConfig,
    artifacts: list[dict[str, Any]],
    frontend_sha: str,
    game_git: dict[str, Any],
    psoxide_git: dict[str, Any],
    static_ram: StaticRam | None,
) -> dict[str, Any]:
    return {
        "disc_artifacts": artifacts,
        "frontend": str(config.frontend),
        "frontend_sha256": frontend_sha,
        "game_git": game_git,
        "psoxide_git": psoxide_git,
        "static_ram": asdict(static_ram) if static_ram else None,
        "visual_frames": config.visual_frames,
        "guest_frames": config.guest_frames,
        "steps": config.steps,
        "timeout_seconds": config.timeout_seconds,
        "warmup_visuals": config.warmup_visuals,
        "route": config.route,
        "screenshot": config.screenshot,
    }


def build_launch_command(
    config: CaptureConfig, index: int, map_dir: Path
) -> list[str]:
    command = [
        str(config.frontend),
        "launch",
        "--path",
        str(config.disc),
        "--embedded-playtest",
        "--steps",
        str(config.steps),
        "--guest-visual-frames",
        str(config.visual_frames),
        "--guest-frames",
        str(config.guest_frames),
        "--pad-pulses",
        map_boot_route(index),
        "--profile-log",
        str(map_dir / "profile.csv"),
        "--counter-log",
        str(map_dir / "counter.csv"),
        "--visual-hash-log",
        str(map_dir / "visual-hash.csv"),
        "--visual-hash-interval",
        "1",
        "--dump-guest-profile",
        "--guest-debug-log",
        "--dump-hash",
    ]
    if config.screenshot:
        command.extend(["--dump-display", str(map_dir / "final.ppm")])
    if config.route in {"forward", "forward-run"}:
        command.append("--hold-forward")
    if config.route == "forward-run":
        command.append("--hold-run")
    return command


def write_map_capture_json(
    map_dir: Path,
    result: MapResult,
    command: Sequence[str],
    returncode: int | None,
) -> None:
    files: dict[str, dict[str, Any]] = {}
    for name in ("profile.csv", "counter.csv", "visual-hash.csv", "final.ppm", "console.log"):
        path = map_dir / name
        if path.is_file():
            files[name] = {"size": path.stat().st_size, "sha256": sha256_file(path)}
    payload = {
        "schema": SCHEMA,
        "captured_at_utc": utc_now(),
        "command": list(command),
        "returncode": returncode,
        "files": files,
        "result": asdict(result),
    }
    (map_dir / "capture.json").write_text(
        json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )


def run_one_map(
    index: int,
    name: str,
    out_dir: Path,
    config: CaptureConfig,
    policy: GatePolicy,
) -> MapResult:
    map_dir = out_dir / "maps" / f"{index:03d}-{name}"
    map_dir.mkdir(parents=True, exist_ok=True)
    command = build_launch_command(config, index, map_dir)
    start = time.monotonic()
    returncode: int | None = None
    try:
        completed = subprocess.run(
            command,
            cwd=config.psoxide_root / "emu",
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=config.timeout_seconds,
            check=False,
        )
        returncode = completed.returncode
        console = completed.stdout
    except subprocess.TimeoutExpired as exc:
        output = exc.stdout or ""
        console = output.decode(errors="replace") if isinstance(output, bytes) else output
        console += f"\nFLEET ERROR: timed out after {config.timeout_seconds}s\n"
        elapsed = time.monotonic() - start
        (map_dir / "console.log").write_text(console, encoding="utf-8")
        result = _empty_result(
            index,
            name,
            map_dir,
            error=f"emulator timeout after {config.timeout_seconds}s",
            host_seconds=elapsed,
            console=console,
        )
        write_map_capture_json(map_dir, result, command, returncode)
        return result
    elapsed = time.monotonic() - start
    (map_dir / "console.log").write_text(console, encoding="utf-8")
    if returncode != 0:
        result = _empty_result(
            index,
            name,
            map_dir,
            error=f"frontend exited with code {returncode}",
            host_seconds=elapsed,
            console=console,
        )
    else:
        result = analyze_map_capture(
            index,
            name,
            map_dir,
            policy,
            config.warmup_visuals,
            host_seconds=elapsed,
        )
    write_map_capture_json(map_dir, result, command, returncode)
    return result


def load_completed_map(
    index: int,
    name: str,
    out_dir: Path,
    policy: GatePolicy,
    warmup_visuals: int,
) -> MapResult | None:
    map_dir = out_dir / "maps" / f"{index:03d}-{name}"
    capture_path = map_dir / "capture.json"
    if not capture_path.is_file():
        return None
    try:
        payload = json.loads(capture_path.read_text(encoding="utf-8"))
        if payload.get("schema") != SCHEMA:
            return None
        saved = payload.get("result", {})
        host_seconds = saved.get("host_seconds")
        result = analyze_map_capture(
            index,
            name,
            map_dir,
            policy,
            warmup_visuals,
            host_seconds=host_seconds,
        )
        return result if result.capture_status == "ok" else None
    except (OSError, json.JSONDecodeError, TypeError):
        return None


def _csv_value(value: Any) -> Any:
    if isinstance(value, list):
        return " | ".join(str(item) for item in value)
    return value


def write_summary_csv(path: Path, results: Sequence[MapResult], static: StaticRam | None) -> None:
    rows = []
    for result in sorted(results, key=lambda item: item.map_index):
        row = asdict(result)
        row["status"] = result.status
        row["static_ram_used_bytes"] = static.used_bytes if static else None
        row["static_ram_headroom_bytes"] = static.headroom_bytes if static else None
        rows.append({key: _csv_value(value) for key, value in row.items()})
    fields = list(rows[0]) if rows else ["map_index", "map_name", "status"]
    with path.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=fields)
        writer.writeheader()
        writer.writerows(rows)


def _rank(
    results: Sequence[MapResult], field: str, *, reverse: bool
) -> list[MapResult]:
    valid = [result for result in results if getattr(result, field) is not None]
    return sorted(valid, key=lambda result: getattr(result, field), reverse=reverse)


def aggregate_results(results: Sequence[MapResult]) -> dict[str, Any]:
    measured = [result for result in results if result.capture_status == "ok"]
    total_visuals = sum(result.visual_samples or 0 for result in measured)
    total_misses = sum(result.deadline_misses or 0 for result in measured)
    rankings = {
        "lowest_visual_fps": [item.map_name for item in _rank(measured, "actual_visual_fps", reverse=False)[:10]],
        "highest_delivery_p95": [item.map_name for item in _rank(measured, "delivery_cycles_p95", reverse=True)[:10]],
        "highest_render_p95": [item.map_name for item in _rank(measured, "render_cycles_p95", reverse=True)[:10]],
        "highest_packet_peak": [item.map_name for item in _rank(measured, "packet_peak", reverse=True)[:10]],
        "highest_triangle_equiv_peak": [item.map_name for item in _rank(measured, "triangle_equiv_peak", reverse=True)[:10]],
    }
    fps_values = [item.actual_visual_fps for item in measured if item.actual_visual_fps is not None]
    return {
        "maps_total": len(results),
        "maps_measured": len(measured),
        "maps_capture_error": len(results) - len(measured),
        "maps_gate_pass": sum(item.gate_pass is True for item in measured),
        "maps_gate_fail": sum(item.gate_pass is False for item in measured),
        "fleet_visual_fps_p50": statistics.median(fps_values) if fps_values else None,
        "total_post_warmup_visuals": total_visuals,
        "total_deadline_misses": total_misses,
        "aggregate_deadline_miss_rate": total_misses / total_visuals if total_visuals else None,
        "rankings": rankings,
    }


def _fmt_int(value: int | None) -> str:
    return "-" if value is None else f"{value:,}"


def _fmt_fps(value: float | None) -> str:
    return "-" if value is None else f"{value:.2f}"


def ranking_table(results: Sequence[MapResult], limit: int = 12) -> str:
    worst = _rank(results, "delivery_cycles_p95", reverse=True)[:limit]
    lines = [
        "| map | FPS | delivery p95 | render p95 | miss | packets peak | tri-eq peak |",
        "|---|---:|---:|---:|---:|---:|---:|",
    ]
    for item in worst:
        lines.append(
            f"| {item.map_name} ({item.map_index}) | {_fmt_fps(item.actual_visual_fps)} | "
            f"{_fmt_int(item.delivery_cycles_p95)} | {_fmt_int(item.render_cycles_p95)} | "
            f"{(item.deadline_miss_rate or 0):.1%} | {_fmt_int(item.packet_peak)} | "
            f"{_fmt_int(item.triangle_equiv_peak)} |"
        )
    return "\n".join(lines)


def write_report(
    path: Path,
    results: Sequence[MapResult],
    aggregate: dict[str, Any],
    policy: GatePolicy,
    static: StaticRam | None,
    manifest_path: Path | None,
) -> None:
    static_text = "unavailable (supply --linker-map or use run --build)"
    if static:
        static_text = (
            f"{static.used_bytes:,} bytes used; {static.headroom_bytes:,} bytes "
            f"({static.headroom_bytes / 1024:.1f} KiB) headroom"
        )
    errors = [result for result in results if result.capture_status != "ok"]
    fails = [result for result in results if result.capture_status == "ok" and not result.gate_pass]
    text = [
        f"# hl-psx {len(results)}-map visual performance fleet",
        "",
        f"Generated: {utc_now()}",
        "",
        f"- Maps measured: {aggregate['maps_measured']}/{aggregate['maps_total']}",
        f"- Strict {policy.target_fps:g} FPS gate: {aggregate['maps_gate_pass']} pass, "
        f"{aggregate['maps_gate_fail']} fail, {aggregate['maps_capture_error']} capture error",
        f"- Fleet median visual FPS: {_fmt_fps(aggregate['fleet_visual_fps_p50'])}",
        f"- Post-warmup deadline misses: {aggregate['total_deadline_misses']}/"
        f"{aggregate['total_post_warmup_visuals']} "
        f"({(aggregate['aggregate_deadline_miss_rate'] or 0):.1%})",
        f"- Static RAM: {static_text}",
        f"- Frame budget: {policy.frame_budget_cycles:,} cycles at {policy.clock_hz:,} Hz",
    ]
    if manifest_path:
        text.append(f"- Provenance manifest: `{manifest_path}`")
    text.extend(
        [
            "",
            "## Worst spawn views by visual delivery p95",
            "",
            ranking_table(results),
            "",
            "## Measurement semantics",
            "",
            "Delivery cycles are measured between consecutive `VISUAL_FRAMES` endpoints and "
            "include intervening catch-up simulation work. Warmup is discarded by rendered "
            "visual count, not by CSV row. `packets` is transient mixed packet-arena pressure. "
            "`tri-eq` is `tri_primitives + room_surf_whole_quads`, because a quad packet emits "
            "two triangles.",
            "",
            "Both packet and triangle-equivalent counts are lower bounds: the separately resident "
            "tram and viewmodel packet caches are linked directly and are intentionally absent "
            "from `TRI_PRIMITIVES`. Timing and cadence metrics remain authoritative.",
            "",
            "Each map is sampled at its deterministic spawn view. This is a fleet triage pass, "
            "not proof of every viewpoint. Use the reported worst maps for longer route captures.",
        ]
    )
    if errors:
        text.extend(["", "## Capture errors", ""])
        text.extend(f"- {item.map_name} ({item.map_index}): {item.error}" for item in errors)
    if fails:
        text.extend(["", "## Gate failures", ""])
        for item in fails:
            text.append(
                f"- {item.map_name} ({item.map_index}): "
                + "; ".join(item.gate_failures or ["unspecified"])
            )
    path.write_text("\n".join(text) + "\n", encoding="utf-8")


def write_reports(
    out_dir: Path,
    results: Sequence[MapResult],
    policy: GatePolicy,
    static: StaticRam | None,
) -> dict[str, Any]:
    aggregate = aggregate_results(results)
    manifest_path = out_dir / "manifest.json"
    write_summary_csv(out_dir / "summary.csv", results, static)
    payload = {
        "schema": SCHEMA,
        "generated_at_utc": utc_now(),
        "policy": asdict(policy),
        "static_ram": asdict(static) if static else None,
        "aggregate": aggregate,
        "maps": [asdict(item) | {"status": item.status} for item in sorted(results, key=lambda item: item.map_index)],
    }
    (out_dir / "summary.json").write_text(
        json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    write_report(
        out_dir / "REPORT.md",
        results,
        aggregate,
        policy,
        static,
        manifest_path if manifest_path.is_file() else None,
    )
    return aggregate


def load_static_from_manifest(out_dir: Path) -> StaticRam | None:
    manifest = out_dir / "manifest.json"
    if not manifest.is_file():
        return None
    payload = json.loads(manifest.read_text(encoding="utf-8"))
    raw = payload.get("signature", {}).get("static_ram")
    return StaticRam(**raw) if raw else None


def collect_existing_results(
    out_dir: Path,
    maps: Sequence[str],
    selected: Sequence[int],
    policy: GatePolicy,
    warmup_visuals: int,
) -> list[MapResult]:
    results = []
    for index in selected:
        name = maps[index]
        map_dir = out_dir / "maps" / f"{index:03d}-{name}"
        results.append(
            analyze_map_capture(index, name, map_dir, policy, warmup_visuals)
        )
    return results


def ensure_output_dir(out_dir: Path, resume: bool) -> None:
    if out_dir.exists() and any(out_dir.iterdir()) and not resume:
        raise FleetError(f"output directory is not empty (use --resume): {out_dir}")
    out_dir.mkdir(parents=True, exist_ok=True)


def default_out_dir(repo_root: Path) -> Path:
    stamp = datetime.now().strftime("%Y%m%d-%H%M%S")
    return repo_root / "captures" / f"perf-fleet-{stamp}"


def run_command(args: argparse.Namespace) -> int:
    repo_root = args.repo_root.expanduser().resolve()
    psoxide_root = args.psoxide_root.expanduser().resolve()
    maps = load_map_registry(repo_root)
    selected = parse_map_selection(args.maps, maps)
    out_dir = (args.out or default_out_dir(repo_root)).expanduser().resolve()
    ensure_output_dir(out_dir, args.resume)
    if args.build_emulator:
        build_emulator(psoxide_root)
    linker_map = None
    if args.build:
        linker_map = build_game(repo_root, psoxide_root, out_dir)
    elif args.linker_map:
        linker_map = args.linker_map.expanduser().resolve()
    disc = args.disc.expanduser().resolve()
    if not disc.is_file():
        raise FleetError(f"disc does not exist: {disc}; use --build or --disc")
    frontend = resolve_frontend(psoxide_root, args.frontend)
    static = parse_static_ram(linker_map) if linker_map else None
    policy = policy_from_args(args)
    validate_policy(policy)
    config = CaptureConfig(
        disc=disc,
        frontend=frontend,
        psoxide_root=psoxide_root,
        visual_frames=args.visual_frames,
        # The guest-frame cap is only a dead-render fallback. Scale it for long
        # captures so update-heavy maps cannot hit the fallback before reaching
        # the requested visual count (the worst fleet maps need 4-9 sim ticks
        # per delivered visual).
        guest_frames=args.guest_frames
        if args.guest_frames is not None
        else max(DEFAULT_GUEST_FRAMES, args.visual_frames * 16),
        steps=args.steps,
        timeout_seconds=args.timeout_seconds,
        warmup_visuals=args.warmup_visuals,
        route=args.route,
        screenshot=not args.no_screenshots,
    )
    artifacts = artifact_identity(disc)
    frontend_sha = sha256_file(frontend)
    inherited_manifest: dict[str, Any] | None = None
    if args.base_manifest:
        base_path = args.base_manifest.expanduser().resolve()
        inherited_manifest = json.loads(base_path.read_text(encoding="utf-8"))
        base_signature = inherited_manifest.get("signature", {})
        if base_signature.get("disc_artifacts") != artifacts:
            raise FleetError("--base-manifest disc artifacts differ from the current disc")
        if base_signature.get("frontend_sha256") != frontend_sha:
            raise FleetError("--base-manifest frontend differs from the current frontend")
        if static and base_signature.get("static_ram", {}).get(
            "linker_map_sha256"
        ) != static.linker_map_sha256:
            raise FleetError("--base-manifest linker map differs from --linker-map")
        game_git = base_signature.get("game_git")
        psoxide_git = base_signature.get("psoxide_git")
        if not isinstance(game_git, dict) or not isinstance(psoxide_git, dict):
            raise FleetError("--base-manifest has no source identities")
    else:
        game_git = git_identity(repo_root)
        psoxide_git = git_identity(psoxide_root)
    signature = capture_signature(
        config, artifacts, frontend_sha, game_git, psoxide_git, static
    )
    if args.base_manifest:
        base_path = args.base_manifest.expanduser().resolve()
        signature["base_manifest"] = {
            "path": str(base_path),
            "sha256": sha256_file(base_path),
        }
    signature_hash = canonical_hash(signature)
    manifest_path = out_dir / "manifest.json"
    if args.resume and manifest_path.is_file():
        old = json.loads(manifest_path.read_text(encoding="utf-8"))
        if old.get("signature_hash") != signature_hash:
            raise FleetError(
                "resume identity/configuration differs from manifest; use a new output directory"
            )
    else:
        manifest = {
            "schema": SCHEMA,
            "created_at_utc": utc_now(),
            "python": sys.version,
            "platform": platform.platform(),
            "map_registry": maps,
            "signature": signature,
            "signature_hash": signature_hash,
        }
        manifest_path.write_text(
            json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )

    completed_results: dict[int, MapResult] = {}
    pending: list[int] = []
    for index in selected:
        prior = (
            load_completed_map(
                index, maps[index], out_dir, policy, args.warmup_visuals
            )
            if args.resume
            else None
        )
        if prior:
            completed_results[index] = prior
            print(f"[{index:03d}/{maps[index]}] resume: valid capture", flush=True)
        else:
            pending.append(index)

    def capture(index: int) -> MapResult:
        return run_one_map(index, maps[index], out_dir, config, policy)

    if args.jobs == 1:
        for ordinal, index in enumerate(pending, start=1):
            print(
                f"[{ordinal}/{len(pending)}] capture {index:03d}/{maps[index]}",
                flush=True,
            )
            result = capture(index)
            completed_results[index] = result
            print(
                f"  -> {result.status} fps={_fmt_fps(result.actual_visual_fps)} "
                f"delivery_p95={_fmt_int(result.delivery_cycles_p95)} "
                f"host={result.host_seconds or 0:.1f}s",
                flush=True,
            )
    else:
        with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as executor:
            futures = {executor.submit(capture, index): index for index in pending}
            for future in concurrent.futures.as_completed(futures):
                index = futures[future]
                result = future.result()
                completed_results[index] = result
                print(
                    f"[{index:03d}/{maps[index]}] {result.status} "
                    f"fps={_fmt_fps(result.actual_visual_fps)} "
                    f"delivery_p95={_fmt_int(result.delivery_cycles_p95)} "
                    f"host={result.host_seconds or 0:.1f}s",
                    flush=True,
                )

    results = [completed_results[index] for index in selected]
    aggregate = write_reports(out_dir, results, policy, static)
    print(f"REPORT -> {out_dir / 'REPORT.md'}")
    print(f"SUMMARY -> {out_dir / 'summary.csv'}")
    print(
        f"fleet: {aggregate['maps_measured']}/{aggregate['maps_total']} measured, "
        f"{aggregate['maps_gate_pass']} pass, {aggregate['maps_gate_fail']} fail, "
        f"{aggregate['maps_capture_error']} errors"
    )
    if aggregate["maps_capture_error"]:
        return 2
    if args.gate and aggregate["maps_gate_fail"]:
        return 1
    return 0


def report_command(args: argparse.Namespace) -> int:
    repo_root = args.repo_root.expanduser().resolve()
    out_dir = args.out.expanduser().resolve()
    maps = load_map_registry(repo_root)
    selected = parse_map_selection(args.maps, maps)
    policy = policy_from_args(args)
    validate_policy(policy)
    static = (
        parse_static_ram(args.linker_map.expanduser().resolve())
        if args.linker_map
        else load_static_from_manifest(out_dir)
    )
    results = collect_existing_results(
        out_dir, maps, selected, policy, args.warmup_visuals
    )
    aggregate = write_reports(out_dir, results, policy, static)
    print(f"REPORT -> {out_dir / 'REPORT.md'}")
    if aggregate["maps_capture_error"]:
        return 2
    if args.gate and aggregate["maps_gate_fail"]:
        return 1
    return 0


def list_command(args: argparse.Namespace) -> int:
    for index, name in enumerate(load_map_registry(args.repo_root.expanduser().resolve())):
        print(f"{index:03d} {name}")
    return 0


def add_policy_arguments(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--warmup-visuals", type=int, default=DEFAULT_WARMUP_VISUALS)
    parser.add_argument("--target-fps", type=float, default=20.0)
    parser.add_argument("--clock-hz", type=int, default=PSX_CPU_HZ)
    parser.add_argument("--tolerance-percent", type=float, default=1.0)
    parser.add_argument("--min-visual-samples", type=int, default=60)
    parser.add_argument("--max-deadline-miss-rate", type=float, default=0.0)
    parser.add_argument("--max-skipped-vblanks", type=int, default=0)
    parser.add_argument("--max-packet-overflows", type=int, default=0)
    parser.add_argument("--min-packet-slots-free", type=int, default=1)
    parser.add_argument("--gate", action="store_true", help="exit 1 if any measured map fails")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", type=Path, default=REPO_ROOT)
    subparsers = parser.add_subparsers(dest="command", required=True)

    run = subparsers.add_parser("run", help="build/capture/report a map fleet")
    run.add_argument("--out", type=Path)
    run.add_argument("--maps", default="all", help="all, names, indexes, or ranges")
    run.add_argument("--resume", action="store_true")
    run.add_argument("--build", action="store_true", help="build exact telemetry/debug-map disc")
    run.add_argument("--build-emulator", action="store_true")
    run.add_argument("--psoxide-root", type=Path, default=REPO_ROOT.parent / "PSoXide")
    run.add_argument("--frontend", type=Path)
    run.add_argument("--disc", type=Path, default=REPO_ROOT / "dist/hl-psx.cue")
    run.add_argument("--linker-map", type=Path)
    run.add_argument(
        "--base-manifest",
        type=Path,
        help="inherit frozen build/source identity after verifying disc/frontend hashes",
    )
    run.add_argument("--visual-frames", type=int, default=DEFAULT_VISUAL_FRAMES)
    run.add_argument(
        "--guest-frames",
        type=int,
        help="dead-render fallback (default: max(1200, 16 * visual frames))",
    )
    run.add_argument("--steps", type=int, default=DEFAULT_STEPS)
    run.add_argument("--timeout-seconds", type=int, default=DEFAULT_TIMEOUT_SECONDS)
    run.add_argument("--jobs", type=int, default=1)
    run.add_argument(
        "--route",
        choices=("static", "forward", "forward-run"),
        default="static",
        help="motion after direct boot; static is the deterministic baseline",
    )
    run.add_argument("--no-screenshots", action="store_true")
    add_policy_arguments(run)
    run.set_defaults(func=run_command)

    report = subparsers.add_parser("report", help="re-analyze an existing fleet")
    report.add_argument("--out", type=Path, required=True)
    report.add_argument("--maps", default="all")
    report.add_argument("--linker-map", type=Path)
    add_policy_arguments(report)
    report.set_defaults(func=report_command)

    listing = subparsers.add_parser("list", help="print canonical map indexes")
    listing.set_defaults(func=list_command)
    return parser


def validate_run_args(args: argparse.Namespace) -> None:
    for name in (
        "visual_frames",
        "steps",
        "timeout_seconds",
        "jobs",
        "min_visual_samples",
    ):
        if getattr(args, name) <= 0:
            raise FleetError(f"--{name.replace('_', '-')} must be positive")
    if args.guest_frames is not None and args.guest_frames <= 0:
        raise FleetError("--guest-frames must be positive")
    if args.warmup_visuals < 0:
        raise FleetError("--warmup-visuals cannot be negative")
    if args.visual_frames <= args.warmup_visuals + 1:
        raise FleetError("visual frame limit must leave at least two post-warmup frames")
    if args.min_visual_samples > args.visual_frames - args.warmup_visuals:
        raise FleetError("minimum visual samples exceed the capture's post-warmup frames")


def main(argv: Sequence[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    try:
        if args.command == "run":
            validate_run_args(args)
        return int(args.func(args))
    except (FleetError, PerfDataError, OSError, subprocess.SubprocessError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
