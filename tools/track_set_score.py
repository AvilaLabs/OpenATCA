#!/usr/bin/env python3
"""Bounded exact set-quality scoring for truth and emitted track snapshots.

This offline shell tool implements the preregistered GOSPA(p=2, alpha=2, c=1000m)
metric. Position distance is the zero-height WGS84 ECEF chord, a global metric that
agrees with horizontal distance locally. It never claims that proximity assignment
proves real track identity.
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import math
import re
import sys
from collections import defaultdict, deque
from dataclasses import dataclass
from pathlib import Path
from typing import Any

# The sealed hold-out guard (tools/sealed_guard.py); the script's own directory may not be on the path.
sys.path.insert(0, str(Path(__file__).resolve().parent))
import sealed_guard  # noqa: E402

# Rows in a sealed time window are dropped and counted; the line is printed once at exit.
DROPS = sealed_guard.DropCounter()


TRUTH_FIELDS = ("scenario_id", "time_ns", "truth_id", "lat_deg", "lon_deg")
OUTPUT_FIELDS = ("scenario_id", "time_ns", "track_id", "source_ns", "lat_deg", "lon_deg", "status")
MAX_FILE_BYTES = 128 * 1024 * 1024
MAX_ROWS = 150_000
MAX_FIELD_CHARS = 4096
MAX_TICKS = 10_000
MAX_POINTS_PER_TICK = 5_000
MAX_NEIGHBOR_CHECKS = 20_000_000
MAX_CANDIDATE_EDGES_PER_TICK = 200_000
MAX_CANDIDATE_EDGES_TOTAL = 2_000_000
MAX_COMPONENT_SIDE = 256
MAX_TOTAL_HUNGARIAN_CUBE_WORK = 100_000_000
CUTOFF_M = 1_000.0
CUTOFF_SQUARED_M2 = CUTOFF_M * CUTOFF_M
AMBIGUOUS_TIE_EPSILON_M2 = 1e-9
SPATIAL_CELL_WIDTH_M = 1_000.01
WGS84_A_M = 6_378_137.0
WGS84_F = 1.0 / 298.257223563
WGS84_E2 = WGS84_F * (2.0 - WGS84_F)


class ScoreError(ValueError):
    """Input or score contract is invalid."""


class UnqualifiedError(ValueError):
    """Exact score cannot be computed within the declared safe work bounds."""


@dataclass(frozen=True, slots=True)
class Point:
    scenario_id: str
    time_ns: int
    object_id: str
    lat_deg: float
    lon_deg: float
    status: str = "valid"


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with sealed_guard.open_read(path, "rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def _bounded_csv(path: Path, expected_fields: tuple[str, ...], kind: str) -> tuple[list[dict[str, str]], int]:
    sealed_guard.refuse(path)
    try:
        if path.stat().st_size > MAX_FILE_BYTES:
            raise ScoreError(f"{kind} exceeds byte limit {MAX_FILE_BYTES}")
        csv.field_size_limit(MAX_FIELD_CHARS)
        with sealed_guard.open_read(path, "r", encoding="utf-8-sig", newline="") as handle:
            reader = csv.DictReader(handle, strict=True)
            if reader.fieldnames is None or tuple(reader.fieldnames) != expected_fields:
                raise ScoreError(f"{kind} header must exactly equal {','.join(expected_fields)}")
            rows: list[dict[str, str]] = []
            malformed = 0
            for row_num, row in enumerate(reader, start=2):
                if row_num > MAX_ROWS + 1:
                    raise ScoreError(f"{kind} exceeds row limit {MAX_ROWS}")
                if (None in row or any(value is None or len(value) > MAX_FIELD_CHARS for value in row.values())):
                    malformed += 1
                    continue
                if sealed_guard.row_in_window(row, ("time_ns",), ("source_ns",)):
                    DROPS.add(); continue
                rows.append(row)
            return rows, malformed
    except (OSError, UnicodeError, csv.Error) as exc:
        raise ScoreError(f"cannot read {kind}: {exc}") from exc


def _integer(raw: str, label: str) -> int:
    if not raw or raw.strip() != raw or any(c in raw for c in ".eE+") or re.fullmatch(r"-?[0-9]+", raw) is None:
        raise ScoreError(f"{label} must be a canonical base-10 integer")
    try:
        value = int(raw, 10)
    except ValueError as exc:
        raise ScoreError(f"{label} must be a canonical base-10 integer") from exc
    if value < 0 or value > 9_223_372_036_854_775_807:
        raise ScoreError(f"{label} is outside signed nanosecond range")
    return value


def _finite(raw: str, label: str) -> float:
    try:
        if re.fullmatch(r"-?(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)(?:[eE][-+]?[0-9]+)?", raw) is None:
            raise ValueError("not strict numeric text")
        value = float(raw)
    except (TypeError, ValueError) as exc:
        raise ScoreError(f"{label} must be numeric") from exc
    if not math.isfinite(value):
        raise ScoreError(f"{label} must be finite")
    return value


def _point(row: dict[str, str], kind: str) -> Point:
    if kind == "truth":
        object_id = row["truth_id"]
        status = "truth"
    else:
        object_id = row["track_id"]
        status = row["status"]
        if status not in ("valid", "coast"):
            raise ScoreError("output status must be valid or coast")
        source_ns = _integer(row["source_ns"], "output.source_ns")
        if source_ns > _integer(row["time_ns"], "output.time_ns"):
            raise ScoreError("output.source_ns cannot be after output.time_ns")
    scenario_id = row["scenario_id"]
    if not scenario_id or not object_id:
        raise ScoreError(f"{kind} scenario and object IDs must be nonempty")
    time_ns = _integer(row["time_ns"], f"{kind}.time_ns")
    lat = _finite(row["lat_deg"], f"{kind}.lat_deg")
    lon = _finite(row["lon_deg"], f"{kind}.lon_deg")
    if not -90.0 <= lat <= 90.0 or not -180.0 <= lon <= 180.0:
        raise ScoreError(f"{kind} latitude/longitude outside geodetic range")
    return Point(scenario_id, time_ns, object_id, lat, lon, status)


def _ecef(point: Point) -> tuple[float, float, float]:
    phi = math.radians(point.lat_deg)
    lam = math.radians(point.lon_deg)
    normal = WGS84_A_M / math.sqrt(1.0 - WGS84_E2 * math.sin(phi) ** 2)
    return (
        normal * math.cos(phi) * math.cos(lam),
        normal * math.cos(phi) * math.sin(lam),
        normal * (1.0 - WGS84_E2) * math.sin(phi),
    )


def _ecef_dist2(first: tuple[float, float, float], second: tuple[float, float, float]) -> float:
    return math.fsum((first[i] - second[i]) ** 2 for i in range(3))


def _hungarian(cost: list[list[float]]) -> list[int]:
    """Minimum square assignment; row/column order is the deterministic tie rule."""
    n = len(cost)
    if n == 0:
        return []
    u = [0.0] * (n + 1)
    v = [0.0] * (n + 1)
    p = [0] * (n + 1)
    way = [0] * (n + 1)
    for i in range(1, n + 1):
        p[0] = i
        minv = [math.inf] * (n + 1)
        used = [False] * (n + 1)
        j0 = 0
        while True:
            used[j0] = True
            i0 = p[j0]
            delta = math.inf
            j1 = 0
            for j in range(1, n + 1):
                if used[j]:
                    continue
                cur = cost[i0 - 1][j - 1] - u[i0] - v[j]
                if cur < minv[j]:
                    minv[j] = cur
                    way[j] = j0
                if minv[j] < delta:
                    delta = minv[j]
                    j1 = j
            if not math.isfinite(delta):
                raise UnqualifiedError("assignment solver reached a non-finite augmenting step")
            for j in range(n + 1):
                if used[j]:
                    u[p[j]] += delta
                    v[j] -= delta
                else:
                    minv[j] -= delta
            j0 = j1
            if p[j0] == 0:
                break
        while True:
            j1 = way[j0]
            p[j0] = p[j1]
            j0 = j1
            if j0 == 0:
                break
    assignment = [-1] * n
    for j in range(1, n + 1):
        if p[j] != 0:
            assignment[p[j] - 1] = j - 1
    return assignment


def _components(left_count: int, right_count: int, edges: dict[tuple[int, int], float]) -> list[tuple[list[int], list[int]]]:
    left_adj: list[list[int]] = [[] for _ in range(left_count)]
    right_adj: list[list[int]] = [[] for _ in range(right_count)]
    for i, j in edges:
        left_adj[i].append(j)
        right_adj[j].append(i)
    for neighbors in left_adj:
        neighbors.sort()
    for neighbors in right_adj:
        neighbors.sort()
    seen_left: set[int] = set()
    seen_right: set[int] = set()
    result: list[tuple[list[int], list[int]]] = []
    for start in range(left_count):
        if start in seen_left or not left_adj[start]:
            continue
        todo: deque[tuple[bool, int]] = deque([(True, start)])
        component_left: set[int] = set()
        component_right: set[int] = set()
        while todo:
            is_left, index = todo.popleft()
            if is_left:
                if index in seen_left:
                    continue
                seen_left.add(index)
                component_left.add(index)
                todo.extend((False, neighbor) for neighbor in left_adj[index])
            else:
                if index in seen_right:
                    continue
                seen_right.add(index)
                component_right.add(index)
                todo.extend((True, neighbor) for neighbor in right_adj[index])
        result.append((sorted(component_left), sorted(component_right)))
    return result


def _assign_snapshot(
    truth: list[Point],
    outputs: list[Point],
    check_budget: list[int],
    work_budget: list[int],
    edge_budget: list[int],
) -> dict[str, Any]:
    truth = sorted(truth, key=lambda point: point.object_id)
    outputs = sorted(outputs, key=lambda point: point.object_id)
    if len(truth) > MAX_POINTS_PER_TICK or len(outputs) > MAX_POINTS_PER_TICK:
        raise UnqualifiedError(f"snapshot cardinality exceeds {MAX_POINTS_PER_TICK}")
    truth_xyz = [_ecef(point) for point in truth]
    output_xyz = [_ecef(point) for point in outputs]
    cells: dict[tuple[int, int, int], list[int]] = defaultdict(list)
    for index, xyz in enumerate(truth_xyz):
        cells[tuple(math.floor(value / SPATIAL_CELL_WIDTH_M) for value in xyz)].append(index)
    edges: dict[tuple[int, int], float] = {}
    incident_truth: list[list[float]] = [[] for _ in truth]
    incident_output: list[list[float]] = [[] for _ in outputs]
    for output_index, xyz in enumerate(output_xyz):
        cell = tuple(math.floor(value / SPATIAL_CELL_WIDTH_M) for value in xyz)
        for dx in (-1, 0, 1):
            for dy in (-1, 0, 1):
                for dz in (-1, 0, 1):
                    for truth_index in cells.get((cell[0] + dx, cell[1] + dy, cell[2] + dz), ()):
                        check_budget[0] += 1
                        if check_budget[0] > MAX_NEIGHBOR_CHECKS:
                            raise UnqualifiedError(f"spatial candidate checks exceed {MAX_NEIGHBOR_CHECKS}")
                        distance2 = _ecef_dist2(truth_xyz[truth_index], xyz)
                        if distance2 < CUTOFF_SQUARED_M2:
                            if len(edges) >= MAX_CANDIDATE_EDGES_PER_TICK:
                                raise UnqualifiedError(f"snapshot candidate edge count exceeds {MAX_CANDIDATE_EDGES_PER_TICK}")
                            edge_budget[0] += 1
                            if edge_budget[0] > MAX_CANDIDATE_EDGES_TOTAL:
                                raise UnqualifiedError(f"run candidate edge count exceeds {MAX_CANDIDATE_EDGES_TOTAL}")
                            edges[(truth_index, output_index)] = distance2
                            incident_truth[truth_index].append(distance2)
                            incident_output[output_index].append(distance2)

    ambiguous_nodes = 0
    for values in incident_truth + incident_output:
        if len(values) >= 2:
            least = min(values)
            if sum(abs(value - least) <= AMBIGUOUS_TIE_EPSILON_M2 for value in values) > 1:
                ambiguous_nodes += 1

    matches: list[tuple[int, int, float]] = []
    for left, right in _components(len(truth), len(outputs), edges):
        dimension = max(len(left), len(right))
        if dimension > MAX_COMPONENT_SIDE:
            raise UnqualifiedError(f"assignment component side {dimension} exceeds {MAX_COMPONENT_SIDE}")
        work_budget[0] += dimension**3
        if work_budget[0] > MAX_TOTAL_HUNGARIAN_CUBE_WORK:
            raise UnqualifiedError(f"assignment work exceeds {MAX_TOTAL_HUNGARIAN_CUBE_WORK} cubic units")
        matrix = [[CUTOFF_SQUARED_M2] * dimension for _ in range(dimension)]
        # Padded rows/columns encode one unmatched object's c^2/2 penalty.
        for i in range(len(left)):
            for j in range(len(right)):
                value = edges.get((left[i], right[j]))
                matrix[i][j] = value if value is not None else CUTOFF_SQUARED_M2
        for i in range(len(left)):
            for j in range(len(right), dimension):
                matrix[i][j] = CUTOFF_SQUARED_M2 / 2.0
        for i in range(len(left), dimension):
            for j in range(len(right)):
                matrix[i][j] = CUTOFF_SQUARED_M2 / 2.0
        assignment = _hungarian(matrix)
        for i in range(len(left)):
            j = assignment[i]
            if j < len(right):
                distance2 = edges.get((left[i], right[j]))
                if distance2 is not None:
                    matches.append((left[i], right[j], distance2))

    matches.sort(key=lambda item: (truth[item[0]].object_id, outputs[item[1]].object_id))
    matched_truth = {i for i, _, _ in matches}
    matched_outputs = {j for _, j, _ in matches}
    missed = len(truth) - len(matched_truth)
    false = len(outputs) - len(matched_outputs)
    localization_cost = math.fsum(distance2 for _, _, distance2 in matches)
    missed_cost = missed * CUTOFF_SQUARED_M2 / 2.0
    false_cost = false * CUTOFF_SQUARED_M2 / 2.0
    total_cost = math.fsum((localization_cost, missed_cost, false_cost))
    return {
        "truth_count": len(truth),
        "output_count": len(outputs),
        "match_count": len(matches),
        "missed_truth_count": missed,
        "missed_truth_cost_m2": missed_cost,
        "false_or_extra_track_count": false,
        "false_or_extra_track_cost_m2": false_cost,
        "localization_cost_m2": localization_cost,
        "gospa_cost_m2": total_cost,
        "gospa_m": math.sqrt(total_cost),
        "ambiguous_local_nearest_tie_node_count": ambiguous_nodes,
        "local_tie_definition": "number of truth/output nodes with multiple candidate edges tied for their minimum squared ECEF chord distance within 1e-9 m^2",
        "global_assignment_ambiguity": "NOT_ASSESSED; deterministic Hungarian representative selected when optima tie",
        "matches": [
            {"truth_id": truth[i].object_id, "track_id": outputs[j].object_id, "distance_m": math.sqrt(distance2)}
            for i, j, distance2 in matches
        ],
    }


def _parse_points(path: Path, fields: tuple[str, ...], kind: str) -> tuple[list[Point], dict[str, int]]:
    rows, malformed = _bounded_csv(path, fields, kind)
    points: list[Point] = []
    excluded = 0
    errors = 0
    for row in rows:
        try:
            points.append(_point(row, kind))
        except (ScoreError, KeyError):
            malformed += 1
            excluded += 1
            errors += 1
    return points, {f"malformed_{kind}_rows": malformed, f"excluded_{kind}_rows": excluded, f"{kind}_validation_errors": errors}


def _load_manifest(path: Path) -> dict[str, Any]:
    sealed_guard.refuse(path)
    try:
        if path.stat().st_size > 1024 * 1024:
            raise ScoreError("manifest exceeds 1 MiB limit")
        with sealed_guard.open_read(path, "r", encoding="utf-8") as handle:
            data = json.loads(handle.read())
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise ScoreError(f"cannot read manifest: {exc}") from exc
    if not isinstance(data, dict) or data.get("schema_version") != 1:
        raise UnqualifiedError("manifest schema_version must be 1")
    return data


def _manifest_contract(manifest: dict[str, Any]) -> tuple[str, list[int], int, dict[int, int], dict[int, int]]:
    scenario = manifest.get("scenario_id")
    grid = manifest.get("grid")
    accounting = manifest.get("accounting")
    sources = manifest.get("sources")
    config = manifest.get("config")
    if not isinstance(scenario, str) or not scenario or not isinstance(grid, dict):
        raise UnqualifiedError("manifest requires scenario_id and grid object")
    def source_ids_valid(value: Any) -> bool:
        if isinstance(value, str):
            return bool(value)
        if isinstance(value, dict):
            return bool(value) and all(isinstance(k, str) and k and source_ids_valid(v) for k, v in value.items())
        if isinstance(value, list):
            return bool(value) and all(source_ids_valid(item) for item in value)
        return False

    if not isinstance(sources, dict) or not sources or not source_ids_valid(sources):
        raise UnqualifiedError("manifest requires nonempty source IDs")
    if not isinstance(config, dict) or isinstance(config.get("n"), bool) or not isinstance(config.get("n"), int) or config["n"] < 0:
        raise UnqualifiedError("manifest config.n must be a nonnegative integer")
    try:
        start = grid["start_ns"]
        end = grid["end_ns"]
        step = grid["step_ns"]
        count = grid["tick_count"]
        expected_per_tick = manifest["expected_truth_rows_per_tick"]
        expected_total = manifest["expected_truth_total"]
    except (KeyError, TypeError) as exc:
        raise UnqualifiedError("manifest grid or expected truth count fields are incomplete") from exc
    if any(isinstance(v, bool) or not isinstance(v, int) for v in (start, end, step, count, expected_per_tick, expected_total)):
        raise UnqualifiedError("manifest grid and expected truth count values must be integers")
    if start < 0 or end < start or end > 9_223_372_036_854_775_807 or step != 1_000_000_000:
        raise UnqualifiedError("manifest score grid must be a valid inclusive one-second nanosecond grid")
    if (end - start) % step != 0 or count != (end - start) // step + 1 or not 1 <= count <= MAX_TICKS:
        raise UnqualifiedError("manifest grid bounds, step and tick_count disagree or exceed limit")
    if expected_per_tick < 0 or expected_per_tick != config["n"] or expected_total != expected_per_tick * count:
        raise UnqualifiedError("manifest expected truth rows per tick and total disagree")
    if not isinstance(accounting, dict) or accounting.get("export_valid") is not True:
        raise ScoreError("manifest accounting.export_valid is false or missing; no descriptive score is emitted")
    if not isinstance(accounting.get("trace_valid"), bool) or not isinstance(accounting.get("strict_processing_pass"), bool):
        raise UnqualifiedError("manifest trace_valid and strict_processing_pass must explicitly report processing status")
    fingerprint_schema = manifest.get("fingerprint_schema")
    if fingerprint_schema != "oa-fusion-input-fields-le-v1":
        raise UnqualifiedError("manifest fingerprint_schema is missing or unsupported")
    for key in ("input_fingerprint", "measured_input_fingerprint"):
        value = manifest.get(key)
        if not isinstance(value, str) or len(value) != 64 or any(char not in "0123456789abcdef" for char in value):
            raise UnqualifiedError(f"manifest {key} must be a lowercase SHA-256 digest")
    for key in ("input_count", "measured_input_count"):
        value = manifest.get(key)
        if isinstance(value, bool) or not isinstance(value, int) or value < 0:
            raise UnqualifiedError(f"manifest {key} must be a nonnegative integer")
    counters = (
        "emitted_output_rows", "empty_ticks", "pending_overflow", "work_refused",
        "work_execution_fault", "work_faulted_inputs", "work_faults", "output_overflow",
        "prediction_failures", "prediction_refused", "invalid_truth", "offered_adsb", "offered_plots", "adsb_in", "plots_in",
        "measured_offered_adsb", "measured_offered_plots", "processed_events", "applied_hard", "applied_assoc", "applied_plots", "held", "unassigned",
        "late", "plots_refused",
        "adsb_address_refused", "pending_adsb_end", "pending_plots_end", "bucket_plot_overflow",
        "bucket_work_capped", "assoc_refused", "init_plots_refused", "scratch_full", "wide_full",
        "init_wide_full", "track_refused", "track_rec_overflow", "keyinfo_overflow", "assign_log_overflow",
        "plot_scan_full", "work_bound_overflow", "work_ingest_refused_inputs", "tracker_refused",
        "unrepresentable_rows", "tracker_offered", "tracker_applied", "tracker_work_refused", "tracker_work_counter_overflow", "tracker_rejected",
        "tracker_late_window", "tracker_late_ring", "tracker_jump_rejected", "tracker_numerical_refused",
        "tracker_capacity_refused", "tracker_duplicates", "tracker_time_held", "tracker_time_held_applied",
        "tracker_time_future_dropped", "time_held", "time_held_applied", "time_future_dropped", "drain_output_rows", "drain_output_overflow",
        "pending_adsb_before_finish", "pending_plots_before_finish",
    )
    for key in counters:
        value = accounting.get(key)
        if isinstance(value, bool) or not isinstance(value, int) or value < 0:
            raise UnqualifiedError(f"manifest accounting.{key} must be a nonnegative integer")
    if accounting.get("applied_adsb") is not None and (
        isinstance(accounting.get("applied_adsb"), bool)
        or not isinstance(accounting.get("applied_adsb"), int)
        or accounting["applied_adsb"] < 0
    ):
        raise UnqualifiedError("manifest accounting.applied_adsb must be null or a nonnegative integer")
    if not isinstance(accounting.get("input_reconciled"), bool):
        raise UnqualifiedError("manifest accounting.input_reconciled must be an explicit boolean")
    for key in ("work_counter_overflowed", "work_faulted", "tracker_work_counter_overflowed"):
        if not isinstance(accounting.get(key), bool):
            raise UnqualifiedError(f"manifest accounting.{key} must be an explicit boolean")
    # Counter reconciliation is strict-processing evidence. Preserve complete set
    # exports for descriptive scoring when these counters expose tracker losses.
    def read_inventory(name: str) -> dict[int, int]:
        items = manifest.get(name)
        if not isinstance(items, list) or len(items) != count:
            raise UnqualifiedError(f"manifest {name} must enumerate every grid tick")
        inventory: dict[int, int] = {}
        for i, item in enumerate(items):
            if not isinstance(item, dict) or item.get("time_ns") != start + i * step:
                raise UnqualifiedError(f"manifest {name} must be ordered over the complete score grid")
            rows = item.get("count")
            if isinstance(rows, bool) or not isinstance(rows, int) or rows < 0:
                raise UnqualifiedError(f"manifest {name} counts must be nonnegative integers")
            inventory[item["time_ns"]] = rows
        return inventory

    truth_inventory = read_inventory("truth_snapshots")
    output_inventory = read_inventory("output_snapshots")
    if any(truth_inventory[tick] != expected_per_tick for tick in truth_inventory):
        raise UnqualifiedError("manifest truth_snapshots do not match expected_truth_rows_per_tick")
    if sum(output_inventory.values()) != accounting["emitted_output_rows"]:
        raise UnqualifiedError("manifest output_snapshots counts do not sum to emitted_output_rows")
    if (manifest["input_count"] != accounting["offered_adsb"] + accounting["offered_plots"]
            or manifest["measured_input_count"] != accounting["measured_offered_adsb"] + accounting["measured_offered_plots"]):
        raise UnqualifiedError("manifest whole/measured input fingerprint counts do not reconcile to offered inputs")
    ticks = [start + i * step for i in range(count)]
    return scenario, ticks, expected_per_tick, truth_inventory, output_inventory


def score(truth_path: Path, outputs_path: Path, manifest_path: Path) -> dict[str, Any]:
    sealed_guard.refuse(truth_path, outputs_path, manifest_path)
    hashes: dict[str, str | None] = {}
    for name, path in (("truth_sha256", truth_path), ("outputs_sha256", outputs_path), ("manifest_sha256", manifest_path)):
        try:
            hashes[name] = _sha256(path)
        except OSError:
            hashes[name] = None
    evidence: dict[str, Any] = {
        "scorer": "openatca-track-set-gospa-v1",
        "status": "FAIL",
        "reason": "not scored",
        "scope": "set_level_synthetic_accuracy_metric_only",
        "quality_claim": False,
        "identity_claim": False,
        "processing_quality_pass": False,
        "distance_metric": "zero-height WGS84 ECEF chord distance",
        "metric": {"name": "GOSPA", "p": 2, "alpha": 2, "cutoff_m": CUTOFF_M},
        "tie_break": "sort truth_id and track_id ascending; Hungarian scans sorted columns and retains the first equal minimum",
        "hashes": hashes,
        "counts": {
            "malformed_truth_rows": 0,
            "malformed_output_rows": 0,
            "duplicate_truth_rows": 0,
            "duplicate_output_rows": 0,
            "excluded_truth_rows": 0,
            "excluded_output_rows": 0,
            "truth_ticks_missing_or_short": 0,
            "truth_inventory_mismatched_ticks": 0,
            "output_ticks_absent": 0,
            "intentional_empty_output_ticks": 0,
            "output_inventory_mismatched_ticks": 0,
            "output_ticks_missing_from_inventory": 0,
            "truth_validation_errors": 0,
            "output_validation_errors": 0,
        },
        "snapshots": [],
    }
    try:
        if any(value is None for value in hashes.values()):
            raise UnqualifiedError("required truth, output, or manifest file is unavailable")
        manifest = _load_manifest(manifest_path)
        scenario, ticks, expected_truth_per_tick, declared_truth_counts, declared_output_counts = _manifest_contract(manifest)
        accounting = manifest["accounting"]
        export_failure_counters = {
            key: accounting.get(key, 0)
            for key in ("prediction_failures", "invalid_truth")
            if accounting.get(key, 0) != 0
        }
        if export_failure_counters:
            evidence["status"] = "UNQUALIFIED"
            evidence["reason"] = "prediction or truth-integrity counters invalidate the exported sets"
            evidence["export_failure_counters"] = export_failure_counters
            return evidence
        evidence["scenario_id"] = scenario
        evidence["sources"] = manifest["sources"]
        evidence["input_fingerprints"] = {
            "schema": manifest["fingerprint_schema"],
            "input_fingerprint": manifest["input_fingerprint"],
            "input_count": manifest["input_count"],
            "measured_input_fingerprint": manifest["measured_input_fingerprint"],
            "measured_input_count": manifest["measured_input_count"],
        }
        for key, path in (("truth_file", truth_path), ("output_file", outputs_path)):
            declaration = manifest.get(key)
            if not isinstance(declaration, dict):
                raise UnqualifiedError(f"manifest {key} declaration is missing")
            if declaration.get("name") != path.name or declaration.get("sha256") != _sha256(path):
                raise ScoreError(f"{key} name or SHA-256 does not match supplied file")

        truth_points, truth_counts = _parse_points(truth_path, TRUTH_FIELDS, "truth")
        output_points, output_counts = _parse_points(outputs_path, OUTPUT_FIELDS, "output")
        evidence["counts"].update(truth_counts)
        evidence["counts"].update(output_counts)
        truth_decl = manifest["truth_file"]
        output_decl = manifest["output_file"]
        if truth_decl.get("rows") != len(truth_points) + truth_counts["malformed_truth_rows"]:
            evidence["counts"]["truth_validation_errors"] += 1
        if manifest.get("expected_truth_total") != len(truth_points):
            evidence["counts"]["truth_validation_errors"] += 1
        if output_decl.get("rows") != len(output_points) + output_counts["malformed_output_rows"]:
            evidence["counts"]["output_validation_errors"] += 1

        truth_by_tick: dict[int, list[Point]] = defaultdict(list)
        output_by_tick: dict[int, list[Point]] = defaultdict(list)
        tick_set = set(ticks)
        actual_output_counts: dict[int, int] = defaultdict(int)
        seen_truth: set[tuple[int, str]] = set()
        seen_output: set[tuple[int, str]] = set()
        actual_truth_counts: dict[int, int] = defaultdict(int)
        for point in truth_points:
            if point.scenario_id != scenario or point.time_ns not in tick_set:
                evidence["counts"]["excluded_truth_rows"] += 1
                evidence["counts"]["truth_validation_errors"] += 1
                continue
            actual_truth_counts[point.time_ns] += 1
            key = point.time_ns, point.object_id
            if key in seen_truth:
                evidence["counts"]["duplicate_truth_rows"] += 1
                evidence["counts"]["excluded_truth_rows"] += 1
                evidence["counts"]["truth_validation_errors"] += 1
                continue
            seen_truth.add(key)
            truth_by_tick[point.time_ns].append(point)
        for point in output_points:
            if point.scenario_id != scenario or point.time_ns not in tick_set:
                evidence["counts"]["excluded_output_rows"] += 1
                evidence["counts"]["output_validation_errors"] += 1
                continue
            actual_output_counts[point.time_ns] += 1
            key = point.time_ns, point.object_id
            if key in seen_output:
                evidence["counts"]["duplicate_output_rows"] += 1
                evidence["counts"]["excluded_output_rows"] += 1
                evidence["counts"]["output_validation_errors"] += 1
                continue
            seen_output.add(key)
            output_by_tick[point.time_ns].append(point)

        if len(truth_by_tick) > MAX_TICKS or len(output_by_tick) > MAX_TICKS:
            raise UnqualifiedError(f"tick count exceeds {MAX_TICKS}")
        for time_ns in ticks:
            ntruth = len(truth_by_tick.get(time_ns, ()))
            if ntruth != expected_truth_per_tick:
                evidence["counts"]["truth_ticks_missing_or_short"] += 1
                evidence["counts"]["truth_validation_errors"] += 1
            if actual_truth_counts.get(time_ns, 0) != declared_truth_counts[time_ns]:
                evidence["counts"]["truth_inventory_mismatched_ticks"] += 1
        if (evidence["counts"]["truth_ticks_missing_or_short"]
                or evidence["counts"]["truth_inventory_mismatched_ticks"]
                or evidence["counts"]["malformed_truth_rows"]):
            evidence["status"] = "UNQUALIFIED"
            evidence["reason"] = "truth snapshot inventory is missing or disagrees with unique truth rows"
            return evidence
        absent_output_ticks = [time_ns for time_ns in ticks if not actual_output_counts.get(time_ns)]
        evidence["counts"]["output_ticks_absent"] = len(absent_output_ticks)
        mismatched_inventory = [time_ns for time_ns in ticks if actual_output_counts.get(time_ns, 0) != declared_output_counts[time_ns]]
        evidence["counts"]["output_inventory_mismatched_ticks"] = len(mismatched_inventory)
        evidence["counts"]["output_ticks_missing_from_inventory"] = 0
        evidence["counts"]["intentional_empty_output_ticks"] = sum(1 for time_ns in ticks if declared_output_counts[time_ns] == 0)
        evidence["output_snapshot_inventory"] = [
            {"time_ns": time_ns, "declared_rows": declared_output_counts[time_ns], "observed_rows": actual_output_counts.get(time_ns, 0)}
            for time_ns in ticks
        ]
        if mismatched_inventory:
            evidence["status"] = "UNQUALIFIED"
            evidence["reason"] = "output snapshot inventory is missing or disagrees with emitted rows"
            return evidence
        structural_counts = (
            "malformed_truth_rows", "malformed_output_rows", "duplicate_truth_rows", "duplicate_output_rows",
            "excluded_truth_rows", "excluded_output_rows", "truth_validation_errors", "output_validation_errors",
        )
        if any(evidence["counts"][key] for key in structural_counts):
            evidence["status"] = "UNQUALIFIED"
            evidence["reason"] = "CSV rows or declared file contents fail structural validation; no descriptive score is emitted"
            return evidence
        accounting = manifest["accounting"]
        observed_empty_ticks = sum(1 for time_ns in ticks if actual_output_counts.get(time_ns, 0) == 0)
        if (accounting.get("emitted_output_rows") != len(output_points)
                or accounting.get("empty_ticks") != observed_empty_ticks):
            evidence["status"] = "UNQUALIFIED"
            evidence["reason"] = "export row/tick counters disagree with the structurally valid output CSV"
            return evidence
        check_budget = [0]
        work_budget = [0]
        edge_budget = [0]
        prior_matches: dict[str, Any] = {}
        association_changes: list[dict[str, Any]] = []
        association_change_count = 0
        totals = {key: 0 for key in ("truth_count", "output_count", "match_count", "missed_truth_count", "false_or_extra_track_count")}
        cost_totals = {key: 0.0 for key in ("missed_truth_cost_m2", "false_or_extra_track_cost_m2", "localization_cost_m2", "gospa_cost_m2")}
        for time_ns in ticks:
            snapshot = _assign_snapshot(truth_by_tick.get(time_ns, []), output_by_tick.get(time_ns, []), check_budget, work_budget, edge_budget)
            snapshot["time_ns"] = time_ns
            snapshot["declared_output_rows"] = declared_output_counts[time_ns]
            snapshot["output_tick_present"] = bool(actual_output_counts.get(time_ns, 0))
            snapshot["intentional_empty_output_tick"] = declared_output_counts[time_ns] == 0
            current_track_to_truth: dict[str, str] = {}
            current_truth_to_track: dict[str, str] = {}
            current_local_tie = snapshot["ambiguous_local_nearest_tie_node_count"] > 0
            for pair in snapshot["matches"]:
                track_id, truth_id = pair["track_id"], pair["truth_id"]
                current_track_to_truth[track_id] = truth_id
                current_truth_to_track[truth_id] = track_id
            for track_id, truth_id in current_track_to_truth.items():
                previous_truth = prior_matches.get("tracks", {}).get(track_id)
                if previous_truth is not None and previous_truth != truth_id:
                    association_change_count += 1
                    if len(association_changes) < 100_000:
                        association_changes.append({"time_ns": time_ns, "axis": "track_id", "track_id": track_id, "previous_truth_id": previous_truth, "current_truth_id": truth_id,
                                                    "local_tied_nearest_at_previous_tick": prior_matches.get("local_tied_nearest", False), "local_tied_nearest_at_current_tick": current_local_tie,
                                                    "global_assignment_ambiguity": "NOT_ASSESSED", "certification": "descriptive_not_certified_unique"})
            for truth_id, track_id in current_truth_to_track.items():
                previous_track = prior_matches.get("truths", {}).get(truth_id)
                if previous_track is not None and previous_track != track_id:
                    association_change_count += 1
                    if len(association_changes) < 100_000:
                        association_changes.append({"time_ns": time_ns, "axis": "truth_id", "truth_id": truth_id, "previous_track_id": previous_track, "current_track_id": track_id,
                                                    "local_tied_nearest_at_previous_tick": prior_matches.get("local_tied_nearest", False), "local_tied_nearest_at_current_tick": current_local_tie,
                                                    "global_assignment_ambiguity": "NOT_ASSESSED", "certification": "descriptive_not_certified_unique"})
            prior_matches = {"tracks": current_track_to_truth, "truths": current_truth_to_track, "local_tied_nearest": current_local_tie}
            evidence["snapshots"].append(snapshot)
            for key in totals:
                totals[key] += snapshot[key]
            for key in cost_totals:
                cost_totals[key] += snapshot[key]

        accounting = manifest["accounting"]
        failure_counters = [
            "pending_overflow", "work_refused", "work_execution_fault", "work_faulted_inputs",
            "work_faults", "output_overflow", "prediction_failures", "prediction_refused", "invalid_truth",
            "pending_adsb_end", "pending_plots_end",
            "late", "plots_refused", "adsb_address_refused", "output_overflow", "drain_output_overflow",
            "bucket_plot_overflow", "bucket_work_capped",
            "assoc_refused", "init_plots_refused", "scratch_full", "wide_full", "init_wide_full",
            "track_refused", "track_rec_overflow", "keyinfo_overflow", "assign_log_overflow",
            "plot_scan_full", "work_bound_overflow", "work_ingest_refused_inputs", "tracker_refused",
            "unrepresentable_rows", "tracker_work_refused", "tracker_work_counter_overflow", "tracker_rejected", "tracker_late_window",
            "tracker_late_ring", "tracker_jump_rejected", "tracker_numerical_refused", "tracker_capacity_refused",
            "tracker_duplicates", "tracker_time_held", "tracker_time_held_applied", "tracker_time_future_dropped",
        ]
        nonzero_failures = {key: accounting.get(key) for key in failure_counters if accounting.get(key, 0) != 0}
        processing_failure_reasons: list[str] = []
        if nonzero_failures:
            evidence["trace_failure_counters"] = nonzero_failures
            processing_failure_reasons.append("one or more processing/loss counters are nonzero")
        reconciliation_mismatches: list[str] = []
        if accounting.get("input_reconciled") is not True:
            reconciliation_mismatches.append("input_reconciled is false")
        for flag in ("work_counter_overflowed", "work_faulted", "tracker_work_counter_overflowed"):
            if accounting[flag] is not False:
                reconciliation_mismatches.append(f"{flag} is true")
        if accounting["adsb_in"] != accounting["offered_adsb"] or accounting["plots_in"] != accounting["offered_plots"]:
            reconciliation_mismatches.append("fusion input counters do not reconcile to offered ADS-B/plot counts")
        if accounting["processed_events"] + accounting["pending_adsb_end"] + accounting["pending_plots_end"] != accounting["offered_adsb"] + accounting["offered_plots"]:
            reconciliation_mismatches.append("processed and pending counts do not reconcile to offered inputs")
        if accounting["applied_plots"] != accounting["applied_hard"] + accounting["applied_assoc"]:
            reconciliation_mismatches.append("applied_plots does not equal applied_hard plus applied_assoc")
        if reconciliation_mismatches:
            evidence["processing_reconciliation_mismatches"] = reconciliation_mismatches
            processing_failure_reasons.extend(reconciliation_mismatches)
        if accounting.get("trace_valid") is not True or accounting.get("strict_processing_pass") is not True:
            processing_failure_reasons.append("strict trace processing pass is false")

        evidence["aggregate"] = {
            **totals,
            **cost_totals,
            "gospa_m_total_cost_root": math.sqrt(cost_totals["gospa_cost_m2"]),
            "gospa_m_mean_per_grid_tick": math.fsum(s["gospa_m"] for s in evidence["snapshots"]) / len(ticks),
        }
        evidence["association_changes"] = {
            "count": association_change_count,
            "events": association_changes,
            "events_truncated": association_change_count > len(association_changes),
            "interpretation": "changes in proximity-metric assignment by emitted IDs; not proof of physical identity continuity",
        }
        evidence["work"] = {
            "spatial_neighbor_pair_checks": check_budget[0],
            "candidate_edges": edge_budget[0],
            "hungarian_cubic_units": work_budget[0],
            "max_component_side": MAX_COMPONENT_SIDE,
            "max_total_hungarian_cubic_units": MAX_TOTAL_HUNGARIAN_CUBE_WORK,
            "max_candidate_edges_per_tick": MAX_CANDIDATE_EDGES_PER_TICK,
            "max_candidate_edges_total": MAX_CANDIDATE_EDGES_TOTAL,
            "edge_storage_memory_budget_note": "at most 200000 retained graph edges in one snapshot; cap refusal is UNQUALIFIED",
            "input_rows_per_file_max": MAX_ROWS,
            "input_bytes_per_file_max": MAX_FILE_BYTES,
            "ticks_max": MAX_TICKS,
            "conservative_peak_python_working_set_estimate_bytes": 512 * 1024 * 1024,
            "memory_estimate_note": "planning estimate from bounded rows, points, matches and candidate edges; not a measured RSS guarantee",
        }
        evidence["accounting"] = accounting
        errors = evidence["counts"]["truth_validation_errors"] + evidence["counts"]["output_validation_errors"]
        errors += evidence["counts"]["malformed_truth_rows"] + evidence["counts"]["malformed_output_rows"]
        if errors:
            evidence["reason"] = "scored with input or trace-integrity errors; inspect named counts"
        elif processing_failure_reasons:
            evidence["status"] = "UNQUALIFIED_PROCESSING"
            evidence["processing_quality_pass"] = False
            evidence["processing_failure_reasons"] = processing_failure_reasons
            evidence["reason"] = "descriptive set metrics computed from a structurally valid export; strict processing quality did not pass"
        else:
            evidence["status"] = "PASS"
            evidence["processing_quality_pass"] = True
            evidence["reason"] = "exact bounded set score computed; no quality claim is made"
    except UnqualifiedError as exc:
        evidence["status"] = "UNQUALIFIED"
        evidence["reason"] = str(exc)
    except (ScoreError, OSError, ValueError, TypeError, KeyError, OverflowError) as exc:
        evidence["status"] = "FAIL"
        evidence["reason"] = str(exc)
    return evidence


def _cli() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--truth", required=True, type=Path)
    parser.add_argument("--outputs", required=True, type=Path)
    parser.add_argument("--manifest", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    try:
        sealed_guard.refuse(args.truth, args.outputs, args.manifest, args.out)
        result = score(args.truth, args.outputs, args.manifest)
    except sealed_guard.SealedError as exc:
        print(f"track_set_score: {exc}", file=sys.stderr)
        return 2
    try:
        args.out.parent.mkdir(parents=True, exist_ok=True)
        with args.out.open("w", encoding="utf-8", newline="\n") as handle:
            json.dump(result, handle, indent=2, sort_keys=True, allow_nan=False)
            handle.write("\n")
    except OSError as exc:
        print(f"cannot write score evidence: {exc}", file=sys.stderr)
        return 2
    print(json.dumps({"status": result["status"], "reason": result["reason"], "out": str(args.out)}))
    return 0 if result["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(_cli())
