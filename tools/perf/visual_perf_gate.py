#!/usr/bin/env python3
"""Authoritative visual-frame performance report and gate for hl-psx.

PSoXide's profile CSV has one row per guest ``frame_begin`` marker.  hl-psx can
emit several fixed-update markers before it renders, so averaging
``frame_cycles`` per CSV row measures simulation cadence, not visual FPS.

This tool instead finds rows that emitted ``VISUAL_FRAMES`` and measures the
elapsed bus cycles between consecutive rendered-frame endpoints.  Those
delivery intervals include any intervening catch-up simulation ticks.  It also
reports render-stage work, cadence misses/skips, and packet-arena pressure.

Examples:

  python3 tools/perf/visual_perf_gate.py report \
      --scenario c1a0=captures/hl-psx-profile.csv

  python3 tools/perf/visual_perf_gate.py stamp \
      --csv captures/perf/c1a0.csv --artifact dist/hl-psx.bin \
      --scenario c1a0 --map-index 6 --route '0x0406@1+20'

  python3 tools/perf/visual_perf_gate.py report --gate --require-same-build \
      --scenario c1a0=captures/perf/c1a0.csv \
      --scenario c2a5e=captures/perf/c2a5e.csv
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import math
import statistics
import subprocess
import sys
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Sequence


PSX_CPU_HZ = 33_868_800
DEFAULT_TARGET_FPS = 20.0
METADATA_SCHEMA = 1

REQUIRED_COLUMNS = {
    "guest_frame",
    "end_bus_cycles",
    "frame_cycles",
    "visual_render_task",
    "render",
    "sim_ticks",
    "visual_frames",
    "visual_skipped_vblanks",
    "visual_deadline_misses",
    "visual_lateness_vblanks",
    "tri_primitives",
    "tri_primitive_remaining",
    "room_submit_primitive_overflows",
}


class PerfDataError(ValueError):
    """The input cannot support an authoritative visual-frame result."""


@dataclass(frozen=True)
class ScenarioSpec:
    label: str
    csv_path: Path


@dataclass
class ScenarioResult:
    label: str
    csv_path: str
    warmup_visuals: int
    visual_samples: int
    delivery_samples: int
    sim_ticks_between_visuals: int
    elapsed_delivery_cycles: int
    actual_visual_fps: float
    actual_sim_hz: float
    delivery_cycles_p50: int
    delivery_cycles_p95: int
    visual_task_cycles_p50: int
    visual_task_cycles_p95: int
    render_cycles_p50: int
    render_cycles_p95: int
    deadline_misses: int
    deadline_miss_rate: float
    skipped_vblanks: int
    max_lateness_vblanks: int
    packet_capacity: int | None
    packet_capacity_consistent: bool
    peak_primitives: int
    min_packet_slots_free: int
    peak_packet_utilization: float | None
    packet_overflow_events: int
    metadata_path: str | None
    metadata: dict[str, Any] | None
    gate_failures: list[str]


@dataclass(frozen=True)
class GatePolicy:
    target_fps: float
    clock_hz: int
    tolerance_fraction: float
    min_visual_samples: int
    max_deadline_miss_rate: float
    max_skipped_vblanks: int
    max_packet_overflows: int
    min_packet_slots_free: int

    @property
    def frame_budget_cycles(self) -> int:
        return math.floor(self.clock_hz / self.target_fps)


def _int(row: dict[str, str], column: str, row_number: int) -> int:
    value = row.get(column, "")
    try:
        parsed = int(value)
    except (TypeError, ValueError) as exc:
        raise PerfDataError(
            f"row {row_number}: {column!r} must be an integer, got {value!r}"
        ) from exc
    if parsed < 0:
        raise PerfDataError(
            f"row {row_number}: {column!r} must be non-negative, got {parsed}"
        )
    return parsed


def nearest_rank(values: Sequence[int], percentile: float) -> int:
    """Return a conservative nearest-rank percentile for integer cycle data."""

    if not values:
        raise PerfDataError("cannot calculate a percentile from zero samples")
    if not 0.0 < percentile <= 1.0:
        raise ValueError("percentile must be in (0, 1]")
    ordered = sorted(values)
    rank = max(1, math.ceil(percentile * len(ordered)))
    return ordered[rank - 1]


def _median_int(values: Sequence[int]) -> int:
    if not values:
        raise PerfDataError("cannot calculate a median from zero samples")
    return int(round(statistics.median(values)))


def _mode_if_unambiguous(values: Sequence[int]) -> tuple[int | None, bool]:
    """Infer the packet cap only when used+free telemetry is self-consistent."""

    if not values:
        return None, False
    unique = set(values)
    if len(unique) == 1:
        return values[0], True
    counts: dict[int, int] = {}
    for value in values:
        counts[value] = counts.get(value, 0) + 1
    most_common = sorted(counts.items(), key=lambda item: (-item[1], -item[0]))
    # Expose the dominant observed cap for diagnostics, but mark it uncertain.
    return most_common[0][0], False


def _load_rows(path: Path) -> list[dict[str, int]]:
    try:
        handle = path.open(newline="", encoding="utf-8")
    except OSError as exc:
        raise PerfDataError(f"cannot open {path}: {exc}") from exc

    with handle:
        reader = csv.DictReader(handle)
        columns = set(reader.fieldnames or ())
        missing = sorted(REQUIRED_COLUMNS - columns)
        if missing:
            raise PerfDataError(
                f"{path}: missing required PSoXide columns: {', '.join(missing)}"
            )
        rows: list[dict[str, int]] = []
        previous_end: int | None = None
        for row_number, raw in enumerate(reader, start=2):
            parsed = {
                column: _int(raw, column, row_number) for column in REQUIRED_COLUMNS
            }
            end_cycles = parsed["end_bus_cycles"]
            if previous_end is not None and end_cycles < previous_end:
                raise PerfDataError(
                    f"row {row_number}: end_bus_cycles went backwards "
                    f"({end_cycles} < {previous_end})"
                )
            previous_end = end_cycles
            if parsed["visual_frames"] not in (0, 1):
                raise PerfDataError(
                    f"row {row_number}: visual_frames={parsed['visual_frames']}; "
                    "per-frame percentiles require at most one rendered frame per marker"
                )
            rows.append(parsed)
    if not rows:
        raise PerfDataError(f"{path}: profile CSV has no data rows")
    return rows


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    try:
        with path.open("rb") as handle:
            for chunk in iter(lambda: handle.read(1024 * 1024), b""):
                digest.update(chunk)
    except OSError as exc:
        raise PerfDataError(f"cannot hash {path}: {exc}") from exc
    return digest.hexdigest()


def default_metadata_path(csv_path: Path) -> Path:
    return Path(f"{csv_path}.meta.json")


def _load_metadata(
    csv_path: Path, override: Path | None
) -> tuple[Path | None, dict[str, Any] | None]:
    path = override if override is not None else default_metadata_path(csv_path)
    if not path.exists():
        return None, None
    try:
        metadata = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise PerfDataError(f"cannot read metadata {path}: {exc}") from exc
    if metadata.get("schema") != METADATA_SCHEMA:
        raise PerfDataError(
            f"{path}: unsupported metadata schema {metadata.get('schema')!r}"
        )
    expected = metadata.get("profile_sha256")
    if not isinstance(expected, str) or not expected:
        raise PerfDataError(f"{path}: profile_sha256 is missing")
    actual = _sha256(csv_path)
    if actual != expected:
        raise PerfDataError(
            f"{path}: profile CSV hash mismatch (metadata={expected}, actual={actual})"
        )
    return path, metadata


def analyze_scenario(
    spec: ScenarioSpec,
    *,
    warmup_visuals: int,
    policy: GatePolicy,
    metadata_override: Path | None = None,
) -> ScenarioResult:
    rows = _load_rows(spec.csv_path)
    visual_indices = [
        index for index, row in enumerate(rows) if row["visual_frames"] == 1
    ]
    if warmup_visuals < 0:
        raise PerfDataError("warmup visuals cannot be negative")
    if len(visual_indices) <= warmup_visuals:
        raise PerfDataError(
            f"{spec.csv_path}: only {len(visual_indices)} visual frames, "
            f"cannot discard {warmup_visuals} warmup frames"
        )

    selected_indices = visual_indices[warmup_visuals:]
    selected = [rows[index] for index in selected_indices]

    # Delivery intervals are endpoint-to-endpoint.  The first selected visual
    # has no preceding selected endpoint, so it is intentionally not a sample.
    delivery_cycles = [
        current["end_bus_cycles"] - previous["end_bus_cycles"]
        for previous, current in zip(selected, selected[1:])
    ]
    if any(cycles <= 0 for cycles in delivery_cycles):
        raise PerfDataError(
            f"{spec.csv_path}: rendered-frame endpoints are not strictly increasing"
        )
    if not delivery_cycles:
        raise PerfDataError(
            f"{spec.csv_path}: at least two post-warmup visual frames are required"
        )

    first_endpoint = selected[0]["end_bus_cycles"]
    last_endpoint = selected[-1]["end_bus_cycles"]
    elapsed_cycles = last_endpoint - first_endpoint
    # Include fixed updates after the first visual endpoint through the final
    # endpoint. This is the same closed-open interval represented by the visual
    # delivery samples above.
    sim_ticks = sum(
        row["sim_ticks"]
        for row in rows
        if first_endpoint < row["end_bus_cycles"] <= last_endpoint
    )

    render_cycles = [row["render"] for row in selected]
    visual_task_cycles = [row["visual_render_task"] for row in selected]
    # A partial first telemetry frame may have task=0. Warmup normally removes
    # it; if it remains, exclude only that invalid task sample, not the visual.
    visual_task_cycles = [cycles for cycles in visual_task_cycles if cycles > 0]
    if not visual_task_cycles:
        raise PerfDataError(f"{spec.csv_path}: no complete visual_render_task samples")

    primitives = [row["tri_primitives"] for row in selected]
    packet_free = [row["tri_primitive_remaining"] for row in selected]
    capacity_candidates = [used + free for used, free in zip(primitives, packet_free)]
    packet_capacity, capacity_consistent = _mode_if_unambiguous(capacity_candidates)
    peak_primitives = max(primitives)
    utilization = (
        peak_primitives / packet_capacity
        if packet_capacity is not None and packet_capacity > 0
        else None
    )

    misses = sum(row["visual_deadline_misses"] for row in selected)
    skips = sum(row["visual_skipped_vblanks"] for row in selected)
    overflows = sum(row["room_submit_primitive_overflows"] for row in selected)
    actual_fps = len(delivery_cycles) * policy.clock_hz / elapsed_cycles
    actual_sim_hz = sim_ticks * policy.clock_hz / elapsed_cycles

    metadata_path, metadata = _load_metadata(spec.csv_path, metadata_override)
    result = ScenarioResult(
        label=spec.label,
        csv_path=str(spec.csv_path),
        warmup_visuals=warmup_visuals,
        visual_samples=len(selected),
        delivery_samples=len(delivery_cycles),
        sim_ticks_between_visuals=sim_ticks,
        elapsed_delivery_cycles=elapsed_cycles,
        actual_visual_fps=actual_fps,
        actual_sim_hz=actual_sim_hz,
        delivery_cycles_p50=_median_int(delivery_cycles),
        delivery_cycles_p95=nearest_rank(delivery_cycles, 0.95),
        visual_task_cycles_p50=_median_int(visual_task_cycles),
        visual_task_cycles_p95=nearest_rank(visual_task_cycles, 0.95),
        render_cycles_p50=_median_int(render_cycles),
        render_cycles_p95=nearest_rank(render_cycles, 0.95),
        deadline_misses=misses,
        deadline_miss_rate=misses / len(selected),
        skipped_vblanks=skips,
        max_lateness_vblanks=max(row["visual_lateness_vblanks"] for row in selected),
        packet_capacity=packet_capacity,
        packet_capacity_consistent=capacity_consistent,
        peak_primitives=peak_primitives,
        min_packet_slots_free=min(packet_free),
        peak_packet_utilization=utilization,
        packet_overflow_events=overflows,
        metadata_path=str(metadata_path) if metadata_path is not None else None,
        metadata=metadata,
        gate_failures=[],
    )
    result.gate_failures.extend(evaluate_gate(result, policy))
    return result


def evaluate_gate(result: ScenarioResult, policy: GatePolicy) -> list[str]:
    failures: list[str] = []
    tolerance = 1.0 + policy.tolerance_fraction
    minimum_fps = policy.target_fps * (1.0 - policy.tolerance_fraction)
    budget = policy.frame_budget_cycles
    if result.visual_samples < policy.min_visual_samples:
        failures.append(
            f"only {result.visual_samples} visual samples; need {policy.min_visual_samples}"
        )
    if result.actual_visual_fps < minimum_fps:
        failures.append(
            f"actual visual FPS {result.actual_visual_fps:.2f} < {minimum_fps:.2f}"
        )
    if result.delivery_cycles_p95 > budget * tolerance:
        failures.append(
            f"delivery p95 {result.delivery_cycles_p95:,} > "
            f"{budget * tolerance:,.0f} cycles"
        )
    if result.render_cycles_p95 > budget * tolerance:
        failures.append(
            f"render p95 {result.render_cycles_p95:,} > "
            f"{budget * tolerance:,.0f} cycles"
        )
    if result.deadline_miss_rate > policy.max_deadline_miss_rate:
        failures.append(
            f"deadline miss rate {result.deadline_miss_rate:.1%} > "
            f"{policy.max_deadline_miss_rate:.1%}"
        )
    if result.skipped_vblanks > policy.max_skipped_vblanks:
        failures.append(
            f"skipped VBlanks {result.skipped_vblanks} > {policy.max_skipped_vblanks}"
        )
    if result.packet_overflow_events > policy.max_packet_overflows:
        failures.append(
            f"packet overflows {result.packet_overflow_events} > "
            f"{policy.max_packet_overflows}"
        )
    if result.min_packet_slots_free < policy.min_packet_slots_free:
        failures.append(
            f"minimum packet slots free {result.min_packet_slots_free} < "
            f"{policy.min_packet_slots_free}"
        )
    if not result.packet_capacity_consistent:
        failures.append("packet capacity could not be inferred consistently")
    return failures


def parse_scenario(value: str) -> ScenarioSpec:
    if "=" not in value:
        raise argparse.ArgumentTypeError("scenario must be LABEL=CSV")
    label, raw_path = value.split("=", 1)
    if not label.strip() or not raw_path.strip():
        raise argparse.ArgumentTypeError(
            "scenario must have a non-empty LABEL and CSV path"
        )
    return ScenarioSpec(label.strip(), Path(raw_path).expanduser())


def parse_label_path(value: str) -> tuple[str, Path]:
    if "=" not in value:
        raise argparse.ArgumentTypeError("value must be LABEL=PATH")
    label, raw_path = value.split("=", 1)
    if not label.strip() or not raw_path.strip():
        raise argparse.ArgumentTypeError("value must have a non-empty LABEL and path")
    return label.strip(), Path(raw_path).expanduser()


def _format_cycles(value: int) -> str:
    return f"{value:,}"


def _format_packet(result: ScenarioResult) -> str:
    cap = str(result.packet_capacity) if result.packet_capacity is not None else "?"
    suffix = "" if result.packet_capacity_consistent else "?"
    return (
        f"{result.peak_primitives}/{cap}{suffix}, "
        f"free {result.min_packet_slots_free}, ovf {result.packet_overflow_events}"
    )


def _identity_value(metadata: dict[str, Any] | None, *keys: str) -> Any:
    value: Any = metadata
    for key in keys:
        if not isinstance(value, dict):
            return None
        value = value.get(key)
    return value


def compare_identity(results: Sequence[ScenarioResult]) -> tuple[bool, list[str]]:
    """Verify same binary and source/emulator identities for scenario comparison."""

    reasons: list[str] = []
    if len(results) < 2:
        return True, reasons
    if any(result.metadata is None for result in results):
        reasons.append("one or more scenarios have no stamped metadata")
        return False, reasons

    checks = (
        ("artifact SHA-256", ("artifact_sha256",)),
        ("hl-psx commit", ("git", "commit")),
        ("hl-psx worktree", ("git", "worktree_fingerprint")),
        ("PSoXide commit", ("psoxide_git", "commit")),
        ("PSoXide worktree", ("psoxide_git", "worktree_fingerprint")),
    )
    for label, keys in checks:
        values = [_identity_value(result.metadata, *keys) for result in results]
        if any(value in (None, "") for value in values):
            reasons.append(f"{label} is missing")
        elif len(set(values)) != 1:
            reasons.append(f"{label} differs")
    return not reasons, reasons


def compare_visuals(results: Sequence[ScenarioResult]) -> tuple[bool, list[str]]:
    """Check deterministic final-frame equality for before/after captures."""

    if len(results) < 2:
        return True, []
    hashes = [
        _identity_value(result.metadata, "visual_artifact_sha256") for result in results
    ]
    if any(value in (None, "") for value in hashes):
        return False, ["one or more scenarios have no stamped visual artifact"]
    if len(set(hashes)) != 1:
        return False, ["final visual artifact SHA-256 differs"]
    return True, []


def _print_report(
    results: Sequence[ScenarioResult],
    policy: GatePolicy,
    same_build: tuple[bool, list[str]],
    same_visual: tuple[bool, list[str]],
) -> None:
    budget = policy.frame_budget_cycles
    print(
        f"Visual-frame performance (clock={policy.clock_hz:,} Hz, "
        f"target={policy.target_fps:g} FPS, budget={budget:,} cycles)"
    )
    print(
        "  Delivery cycles are consecutive VISUAL_FRAMES endpoints; "
        "they include intervening catch-up simulation ticks."
    )
    print()
    for result in results:
        status = "PASS" if not result.gate_failures else "FAIL"
        print(f"[{result.label}] {status}  {result.csv_path}")
        print(
            f"  visual={result.visual_samples} intervals={result.delivery_samples} "
            f"sim_ticks={result.sim_ticks_between_visuals} "
            f"visual_fps={result.actual_visual_fps:.2f} sim_hz={result.actual_sim_hz:.2f}"
        )
        print(
            "  cycles/visual: "
            f"delivery p50={_format_cycles(result.delivery_cycles_p50)} "
            f"p95={_format_cycles(result.delivery_cycles_p95)}; "
            f"visual-task p50={_format_cycles(result.visual_task_cycles_p50)} "
            f"p95={_format_cycles(result.visual_task_cycles_p95)}; "
            f"render p50={_format_cycles(result.render_cycles_p50)} "
            f"p95={_format_cycles(result.render_cycles_p95)}"
        )
        print(
            f"  cadence: misses={result.deadline_misses}/{result.visual_samples} "
            f"({result.deadline_miss_rate:.1%}) skips={result.skipped_vblanks} "
            f"max_late={result.max_lateness_vblanks} vblanks"
        )
        print(f"  packets: {_format_packet(result)}")
        if result.metadata is None:
            print("  identity: UNSTAMPED (comparison identity cannot be verified)")
        else:
            artifact = str(result.metadata.get("artifact_sha256", "?"))[:12]
            game_commit = str(_identity_value(result.metadata, "git", "commit") or "?")[
                :12
            ]
            psoxide_commit = str(
                _identity_value(result.metadata, "psoxide_git", "commit") or "?"
            )[:12]
            visual = str(result.metadata.get("visual_artifact_sha256") or "unstamped")[
                :12
            ]
            print(
                f"  identity: artifact={artifact} hl-psx={game_commit} "
                f"PSoXide={psoxide_commit} visual={visual}"
            )
        for failure in result.gate_failures:
            print(f"  gate: {failure}")
        print()

    if len(results) > 1:
        verified, reasons = same_build
        if verified:
            print(
                "Comparison identity: VERIFIED (same artifact and source/emulator state)"
            )
        else:
            print("Comparison identity: UNVERIFIED")
            for reason in reasons:
                print(f"  - {reason}")
        visual_verified, visual_reasons = same_visual
        if visual_verified:
            print("Final-frame identity: VERIFIED")
        else:
            print("Final-frame identity: DIFFERENT OR UNSTAMPED")
            for reason in visual_reasons:
                print(f"  - {reason}")
        baseline = results[0]
        print(f"Deltas vs {baseline.label}:")
        for result in results[1:]:
            fps_delta = result.actual_visual_fps - baseline.actual_visual_fps
            p95_delta = result.delivery_cycles_p95 - baseline.delivery_cycles_p95
            render_delta = result.render_cycles_p95 - baseline.render_cycles_p95
            print(
                f"  {result.label}: visual_fps={fps_delta:+.2f}, "
                f"delivery_p95={p95_delta:+,} cycles, "
                f"render_p95={render_delta:+,} cycles"
            )


def _run_git(root: Path, args: Sequence[str]) -> bytes:
    try:
        completed = subprocess.run(
            ["git", "-C", str(root), *args],
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
    except (OSError, subprocess.CalledProcessError) as exc:
        detail = ""
        if isinstance(exc, subprocess.CalledProcessError):
            detail = exc.stderr.decode("utf-8", errors="replace").strip()
        raise PerfDataError(
            f"git {' '.join(args)} failed in {root}: {detail or exc}"
        ) from exc
    return completed.stdout


def git_identity(root: Path) -> dict[str, Any]:
    top = Path(_run_git(root, ["rev-parse", "--show-toplevel"]).decode().strip())
    commit = _run_git(top, ["rev-parse", "HEAD"]).decode().strip()
    status = _run_git(top, ["status", "--porcelain=v1", "-z", "--untracked-files=all"])
    diff = _run_git(top, ["diff", "--binary", "HEAD", "--"])
    untracked = [
        Path(raw.decode("utf-8", errors="surrogateescape"))
        for raw in _run_git(
            top, ["ls-files", "--others", "--exclude-standard", "-z"]
        ).split(b"\0")
        if raw
    ]
    digest = hashlib.sha256()
    digest.update(status)
    digest.update(b"\0DIFF\0")
    digest.update(diff)
    # `git diff` does not contain untracked bytes. Hash them explicitly so two
    # captures cannot claim the same dirty source identity after an untracked
    # build input changed while retaining the same path/status entry.
    digest.update(b"\0UNTRACKED\0")
    for relative in sorted(untracked, key=lambda path: path.as_posix()):
        digest.update(relative.as_posix().encode("utf-8", errors="surrogateescape"))
        digest.update(b"\0")
        digest.update(_sha256(top / relative).encode("ascii"))
        digest.update(b"\0")
    return {
        "root": str(top),
        "commit": commit,
        "dirty": bool(status),
        "worktree_fingerprint": digest.hexdigest(),
    }


def _write_stamp(args: argparse.Namespace) -> int:
    csv_path = args.csv.expanduser().resolve()
    artifact = args.artifact.expanduser().resolve()
    visual_artifact = (
        args.visual_artifact.expanduser().resolve()
        if args.visual_artifact is not None
        else None
    )
    repo_root = args.repo_root.expanduser().resolve()
    psoxide_root = args.psoxide_root.expanduser().resolve()
    if not csv_path.is_file():
        raise PerfDataError(f"profile CSV does not exist: {csv_path}")
    if not artifact.is_file():
        raise PerfDataError(f"artifact does not exist: {artifact}")
    if visual_artifact is not None and not visual_artifact.is_file():
        raise PerfDataError(f"visual artifact does not exist: {visual_artifact}")
    # Validate the telemetry shape before blessing the capture metadata.
    _load_rows(csv_path)
    metadata = {
        "schema": METADATA_SCHEMA,
        "captured_at_utc": datetime.now(timezone.utc).isoformat(),
        "scenario": args.scenario,
        "map_index": args.map_index,
        "route": args.route,
        "command": args.command,
        "visual_hash": args.visual_hash,
        "profile_csv": str(csv_path),
        "profile_sha256": _sha256(csv_path),
        "artifact": str(artifact),
        "artifact_sha256": _sha256(artifact),
        "visual_artifact": str(visual_artifact)
        if visual_artifact is not None
        else None,
        "visual_artifact_sha256": (
            _sha256(visual_artifact) if visual_artifact is not None else None
        ),
        "git": git_identity(repo_root),
        "psoxide_git": git_identity(psoxide_root),
    }
    output = (
        args.output.expanduser().resolve()
        if args.output is not None
        else default_metadata_path(csv_path)
    )
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(
        json.dumps(metadata, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(f"STAMP -> {output}")
    print(
        f"  profile={metadata['profile_sha256'][:12]} artifact={metadata['artifact_sha256'][:12]}"
    )
    print(
        f"  hl-psx={metadata['git']['commit'][:12]} "
        f"PSoXide={metadata['psoxide_git']['commit'][:12]}"
    )
    return 0


def _report(args: argparse.Namespace) -> int:
    policy = GatePolicy(
        target_fps=args.target_fps,
        clock_hz=args.clock_hz,
        tolerance_fraction=args.tolerance_percent / 100.0,
        min_visual_samples=args.min_visual_samples,
        max_deadline_miss_rate=args.max_deadline_miss_rate,
        max_skipped_vblanks=args.max_skipped_vblanks,
        max_packet_overflows=args.max_packet_overflows,
        min_packet_slots_free=args.min_packet_slots_free,
    )
    if policy.target_fps <= 0 or policy.clock_hz <= 0:
        raise PerfDataError("target FPS and clock must be positive")
    if not 0.0 <= policy.tolerance_fraction < 1.0:
        raise PerfDataError("tolerance percent must be in [0, 100)")
    if not 0.0 <= policy.max_deadline_miss_rate <= 1.0:
        raise PerfDataError("max deadline miss rate must be in [0, 1]")

    metadata_overrides = dict(args.metadata or [])
    unknown = sorted(
        set(metadata_overrides) - {scenario.label for scenario in args.scenario}
    )
    if unknown:
        raise PerfDataError(
            f"metadata labels have no matching scenario: {', '.join(unknown)}"
        )

    results = [
        analyze_scenario(
            scenario,
            warmup_visuals=args.warmup_visuals,
            policy=policy,
            metadata_override=metadata_overrides.get(scenario.label),
        )
        for scenario in args.scenario
    ]
    same_build = compare_identity(results)
    same_visual = compare_visuals(results)
    _print_report(results, policy, same_build, same_visual)

    payload = {
        "schema": 1,
        "clock_hz": policy.clock_hz,
        "target_fps": policy.target_fps,
        "frame_budget_cycles": policy.frame_budget_cycles,
        "comparison_identity_verified": same_build[0],
        "comparison_identity_failures": same_build[1],
        "comparison_visual_verified": same_visual[0],
        "comparison_visual_failures": same_visual[1],
        "scenarios": [asdict(result) for result in results],
    }
    if args.json_out is not None:
        output = args.json_out.expanduser().resolve()
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(
            json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        print(f"JSON -> {output}")

    if not args.gate:
        return 0
    failed = any(result.gate_failures for result in results)
    if args.require_same_build and len(results) > 1 and not same_build[0]:
        failed = True
    if args.require_same_visual and len(results) > 1 and not same_visual[0]:
        failed = True
    if failed:
        print("PERF GATE: FAIL")
        return 1
    print("PERF GATE: PASS")
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command_name", required=True)

    report = subparsers.add_parser(
        "report", help="analyze one or more PSoXide guest profile CSVs"
    )
    report.add_argument(
        "--scenario",
        action="append",
        type=parse_scenario,
        required=True,
        metavar="LABEL=CSV",
        help="scenario label and PSoXide --profile-log CSV (repeatable)",
    )
    report.add_argument(
        "--metadata",
        action="append",
        type=parse_label_path,
        metavar="LABEL=JSON",
        help="override the default CSV.meta.json sidecar for a scenario",
    )
    report.add_argument(
        "--warmup-visuals",
        type=int,
        default=3,
        help="discard this many rendered frames before measuring (default: 3)",
    )
    report.add_argument("--target-fps", type=float, default=DEFAULT_TARGET_FPS)
    report.add_argument("--clock-hz", type=int, default=PSX_CPU_HZ)
    report.add_argument(
        "--tolerance-percent",
        type=float,
        default=1.0,
        help="cycle/FPS gate tolerance (default: 1%%)",
    )
    report.add_argument("--min-visual-samples", type=int, default=30)
    report.add_argument("--max-deadline-miss-rate", type=float, default=0.0)
    report.add_argument("--max-skipped-vblanks", type=int, default=0)
    report.add_argument("--max-packet-overflows", type=int, default=0)
    report.add_argument("--min-packet-slots-free", type=int, default=1)
    report.add_argument(
        "--gate",
        action="store_true",
        help="exit non-zero when the target/cadence/packet policy fails",
    )
    report.add_argument(
        "--require-same-build",
        action="store_true",
        help="with --gate, require stamped scenarios to share binary/source identity",
    )
    report.add_argument(
        "--require-same-visual",
        action="store_true",
        help="with --gate, require stamped scenarios to end on an identical frame",
    )
    report.add_argument(
        "--json-out", type=Path, help="also write machine-readable JSON"
    )
    report.set_defaults(func=_report)

    stamp = subparsers.add_parser(
        "stamp", help="bind a profile CSV to its exact disc and repository identities"
    )
    stamp.add_argument("--csv", type=Path, required=True)
    stamp.add_argument("--artifact", type=Path, required=True)
    stamp.add_argument(
        "--visual-artifact",
        type=Path,
        help="final PPM/PNG used as a deterministic visual-correctness stamp",
    )
    stamp.add_argument("--scenario", required=True)
    stamp.add_argument("--map-index", type=int)
    stamp.add_argument("--route")
    stamp.add_argument("--command")
    stamp.add_argument("--visual-hash")
    stamp.add_argument(
        "--repo-root", type=Path, default=Path(__file__).resolve().parents[2]
    )
    stamp.add_argument(
        "--psoxide-root",
        type=Path,
        default=Path(__file__).resolve().parents[3] / "PSoXide",
    )
    stamp.add_argument("--output", type=Path)
    stamp.set_defaults(func=_write_stamp)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    try:
        return int(args.func(args))
    except PerfDataError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
