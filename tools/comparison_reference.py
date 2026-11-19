#!/usr/bin/env python3
"""Independent validator for the synthetic comparison world (offline shell tool).

This module validates the runner's *synthetic world inputs*, not tracker quality. It contains
no tracker filter, production Rust bindings, external I/O, or third-party dependencies. The world
equations and bounds are stated here and in the comparison protocol; the CLI performs bounded
file reads and writes a hash-bound JSON evidence record.
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import math
import re
import os
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable


TRUTH_FIELDS = (
    "scenario_id",
    "flight_id",
    "time_ns",
    "lat_deg",
    "lon_deg",
    "ve_mps",
    "vn_mps",
    "phase",
)
MEASUREMENT_FIELDS = (
    "event_id",
    "scenario_id",
    "flight_id",
    "sensor_id",
    "sensor_ns",
    "receive_ns",
    "lat_deg",
    "lon_deg",
    "ve_mps",
    "vn_mps",
    "sigma_pos_m",
    "sigma_vel_mps",
)
MAX_FILE_BYTES = 512 * 1024 * 1024
MAX_ROWS = 400_000
MAX_CSV_FIELD_BYTES = 4096
WGS84_A_M = 6_378_137.0
WGS84_F = 1.0 / 298.257223563
SIGMA_POSITION_M = 30.0 / 2.448
SIGMA_VELOCITY_MPS = 3.0 / 2.448
DEFAULT_POSITION_TOLERANCE_M = 0.03
DEFAULT_VELOCITY_TOLERANCE_MPS = 0.003
NOISE_FAMILY_ALPHA = 0.001


class ValidationError(ValueError):
    """Input was readable but contradicted the declared synthetic-world contract."""


class UnqualifiedError(ValueError):
    """A required qualification artifact or supported reference model is unavailable."""


@dataclass(frozen=True)
class TruthRow:
    scenario_id: str
    flight_id: int
    time_ns: int
    lat_deg: float
    lon_deg: float
    ve_mps: float
    vn_mps: float
    phase: str


@dataclass(frozen=True)
class MeasurementRow:
    event_id: int
    scenario_id: str
    flight_id: int
    sensor_id: str
    sensor_ns: int
    receive_ns: int
    lat_deg: float
    lon_deg: float
    ve_mps: float
    vn_mps: float
    sigma_pos_m: float
    sigma_vel_mps: float


def _sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def _read_json(path: Path, max_bytes: int) -> dict[str, Any]:
    if path.stat().st_size > max_bytes:
        raise ValidationError(f"{path.name} exceeds the protocol JSON byte limit")
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as e:
        raise ValidationError(f"cannot parse {path.name}: {e}") from e
    if not isinstance(data, dict):
        raise ValidationError(f"{path.name} must contain a JSON object")
    return data


def _finite_float(value: str, label: str) -> float:
    try:
        if re.fullmatch(r"-?(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)(?:[eE][-+]?[0-9]+)?", value) is None:
            raise ValueError("not strict numeric text")
        out = float(value)
    except (TypeError, ValueError) as e:
        raise ValidationError(f"{label} is not numeric") from e
    if not math.isfinite(out):
        raise ValidationError(f"{label} must be finite")
    return out


def _int(value: str, label: str) -> int:
    try:
        # Reject integral-looking floats and whitespace-only strings.
        if not value or value.strip() != value or any(ch in value for ch in ".eE+") or re.fullmatch(r"-?[0-9]+", value) is None:
            raise ValueError("not a canonical integer")
        return int(value, 10)
    except (TypeError, ValueError) as e:
        raise ValidationError(f"{label} is not a canonical base-10 integer") from e


def _bounded_rows(path: Path, expected_fields: tuple[str, ...], max_rows: int) -> Iterable[dict[str, str]]:
    try:
        size = path.stat().st_size
    except OSError as e:
        raise ValidationError(f"cannot stat {path}: {e}") from e
    if size > MAX_FILE_BYTES:
        raise ValidationError(f"{path.name} exceeds the hard {MAX_FILE_BYTES}-byte input limit")
    csv.field_size_limit(MAX_CSV_FIELD_BYTES)
    try:
        f = path.open("r", encoding="utf-8-sig", newline="")
    except OSError as e:
        raise ValidationError(f"cannot open {path}: {e}") from e
    with f:
        reader = csv.DictReader(f, strict=True)
        if reader.fieldnames is None or tuple(reader.fieldnames) != expected_fields:
            raise ValidationError(
                f"{path.name} header must exactly equal {','.join(expected_fields)}"
            )
        count = 0
        try:
            for row in reader:
                count += 1
                if count > max_rows:
                    raise ValidationError(f"{path.name} exceeds the {max_rows}-row protocol limit")
                if None in row or any(value is None for value in row.values()):
                    raise ValidationError(f"{path.name} row {count + 1} has the wrong field count")
                yield row
        except csv.Error as e:
            raise ValidationError(f"malformed CSV in {path.name}: {e}") from e


def _parse_truth(path: Path, max_rows: int) -> list[TruthRow]:
    rows: list[TruthRow] = []
    previous: tuple[str, int, int] | None = None
    for r in _bounded_rows(path, TRUTH_FIELDS, max_rows):
        scenario = r["scenario_id"]
        phase = r["phase"]
        if not scenario or not phase:
            raise ValidationError("truth scenario_id and phase must be nonempty")
        row = TruthRow(
            scenario,
            _int(r["flight_id"], "truth.flight_id"),
            _int(r["time_ns"], "truth.time_ns"),
            _finite_float(r["lat_deg"], "truth.lat_deg"),
            _finite_float(r["lon_deg"], "truth.lon_deg"),
            _finite_float(r["ve_mps"], "truth.ve_mps"),
            _finite_float(r["vn_mps"], "truth.vn_mps"),
            phase,
        )
        if not (-90.0 <= row.lat_deg <= 90.0 and -180.0 <= row.lon_deg <= 180.0):
            raise ValidationError("truth latitude/longitude is outside geodetic bounds")
        key = (row.scenario_id, row.time_ns, row.flight_id)
        if previous is not None and key <= previous:
            raise ValidationError("truth rows must be strictly ordered by scenario, time, flight")
        previous = key
        rows.append(row)
    if not rows:
        raise ValidationError("truth CSV is empty")
    return rows


def _parse_measurements(path: Path, max_rows: int) -> list[MeasurementRow]:
    rows: list[MeasurementRow] = []
    previous: tuple[int, int] | None = None
    event_ids: set[int] = set()
    for r in _bounded_rows(path, MEASUREMENT_FIELDS, max_rows):
        scenario = r["scenario_id"]
        sensor = r["sensor_id"]
        if not scenario or not sensor:
            raise ValidationError("measurement scenario_id and sensor_id must be nonempty")
        row = MeasurementRow(
            _int(r["event_id"], "measurement.event_id"),
            scenario,
            _int(r["flight_id"], "measurement.flight_id"),
            sensor,
            _int(r["sensor_ns"], "measurement.sensor_ns"),
            _int(r["receive_ns"], "measurement.receive_ns"),
            _finite_float(r["lat_deg"], "measurement.lat_deg"),
            _finite_float(r["lon_deg"], "measurement.lon_deg"),
            _finite_float(r["ve_mps"], "measurement.ve_mps"),
            _finite_float(r["vn_mps"], "measurement.vn_mps"),
            _finite_float(r["sigma_pos_m"], "measurement.sigma_pos_m"),
            _finite_float(r["sigma_vel_mps"], "measurement.sigma_vel_mps"),
        )
        if row.event_id < 0 or row.flight_id < 0 or row.sensor_ns < 0 or row.receive_ns < 0:
            raise ValidationError("measurement identifiers and timestamps must be nonnegative")
        if not (-90.0 <= row.lat_deg <= 90.0 and -180.0 <= row.lon_deg <= 180.0):
            raise ValidationError("measurement latitude/longitude is outside geodetic bounds")
        if row.sigma_pos_m <= 0.0 or row.sigma_vel_mps <= 0.0:
            raise ValidationError("reported measurement sigmas must be positive")
        if row.event_id in event_ids:
            raise ValidationError(f"duplicate event_id {row.event_id}")
        event_ids.add(row.event_id)
        key = (row.receive_ns, row.event_id)
        if previous is not None and key < previous:
            raise ValidationError("measurement rows must be ordered by (receive_ns,event_id)")
        previous = key
        rows.append(row)
    if not rows:
        raise ValidationError("measurement CSV is empty")
    return rows


def _wgs84_radii(lat_deg: float, a_m: float, flattening: float) -> tuple[float, float]:
    e2 = flattening * (2.0 - flattening)
    phi = math.radians(lat_deg)
    q = math.sqrt(1.0 - e2 * math.sin(phi) ** 2)
    prime_vertical = a_m / q
    meridional = a_m * (1.0 - e2) / q**3
    return meridional, prime_vertical


def _analytic_state(
    scenario: str,
    flight_id: int,
    elapsed_s: float,
    duration_s: int,
    aircraft_count: int,
    origin_lat_deg: float,
    origin_lon_deg: float,
    a_m: float,
    flattening: float,
) -> tuple[float, float, float, float, str]:
    """Independent closed-form ENU world equations and local WGS84 mapping."""
    m0, n0 = _wgs84_radii(origin_lat_deg, a_m, flattening)
    phi0 = math.radians(origin_lat_deg)
    x0 = ((flight_id % 20) - 9.5) * 1_000.0
    y0 = (flight_id // 20) * 2_000.0
    # The runner counter-rotates only paired crossing traffic. All other scenarios,
    # including turns, share the positive turn direction across flight IDs.
    sign = -1.0 if scenario == "crossing" and flight_id % 2 else 1.0
    if scenario == "crossing":
        pair_index = flight_id // 2
        pair_count = (aircraft_count + 1) // 2
        pair_columns = 10
        pair_rows = (pair_count + pair_columns - 1) // pair_columns
        x_offset = ((pair_index % pair_columns) - (pair_columns - 1) / 2.0) * 5_000.0
        y_center = (pair_index // pair_columns - (pair_rows - 1) / 2.0) * 5_000.0
        x = x_offset + sign * (100.0 * elapsed_s - 50.0 * duration_s)
        y = y_center
        vx, vy, phase = sign * 100.0, 0.0, "crossing"
    elif scenario == "turn":
        omega = math.radians(2.0)
        speed = 150.0
        radius = speed / omega
        angle = omega * elapsed_s
        x = x0 + sign * radius * math.sin(angle)
        y = y0 + sign * radius * (1.0 - math.cos(angle))
        vx, vy, phase = sign * speed * math.cos(angle), sign * speed * math.sin(angle), "turn"
    elif scenario == "acceleration":
        x = x0 + 55.0 * elapsed_s + 0.01 * elapsed_s**2
        y = y0
        vx, vy, phase = 55.0 + 0.02 * elapsed_s, 0.0, "acceleration"
    elif scenario in ("straight", "dropout", "delayed"):
        x = x0 + 70.0 * elapsed_s
        y = y0
        vx, vy, phase = 70.0, 0.0, "straight"
    else:
        raise ValidationError(f"unsupported manifest scenario_id {scenario!r}")

    # This is the declared local tangent-plane geographic mapping, not a tracker projection.
    lat = origin_lat_deg + y / (m0 * math.pi / 180.0)
    lon = origin_lon_deg + x / (n0 * math.cos(phi0) * math.pi / 180.0)
    m, n = _wgs84_radii(lat, a_m, flattening)
    ve = vx * (n * math.cos(math.radians(lat))) / (n0 * math.cos(phi0))
    vn = vy * m / m0
    return lat, lon, ve, vn, phase


def _local_error_m(
    lat_a: float,
    lon_a: float,
    lat_b: float,
    lon_b: float,
    origin_lat_deg: float,
    a_m: float,
    flattening: float,
) -> tuple[float, float]:
    m, n = _wgs84_radii(origin_lat_deg, a_m, flattening)
    east = math.radians(lon_a - lon_b) * n * math.cos(math.radians(origin_lat_deg))
    north = math.radians(lat_a - lat_b) * m
    return east, north


def _geodetic_ecef(lat_deg: float, lon_deg: float, alt_m: float, a_m: float, flattening: float) -> tuple[float, float, float]:
    e2 = flattening * (2.0 - flattening)
    phi = math.radians(lat_deg)
    lam = math.radians(lon_deg)
    q = math.sqrt(1.0 - e2 * math.sin(phi) ** 2)
    n = a_m / q
    return (
        (n + alt_m) * math.cos(phi) * math.cos(lam),
        (n + alt_m) * math.cos(phi) * math.sin(lam),
        (n * (1.0 - e2) + alt_m) * math.sin(phi),
    )


def _ecef_central_difference_velocity(
    scenario: str,
    flight_id: int,
    time_s: float,
    duration_s: int,
    aircraft_count: int,
    origin_lat_deg: float,
    origin_lon_deg: float,
    a_m: float,
    flattening: float,
    step_s: float = 0.05,
) -> tuple[float, float]:
    before = _analytic_state(scenario, flight_id, time_s - step_s, duration_s, aircraft_count,
                             origin_lat_deg, origin_lon_deg, a_m, flattening)
    after = _analytic_state(scenario, flight_id, time_s + step_s, duration_s, aircraft_count,
                            origin_lat_deg, origin_lon_deg, a_m, flattening)
    center = _analytic_state(scenario, flight_id, time_s, duration_s, aircraft_count,
                             origin_lat_deg, origin_lon_deg, a_m, flattening)
    ecef_before = _geodetic_ecef(before[0], before[1], 0.0, a_m, flattening)
    ecef_after = _geodetic_ecef(after[0], after[1], 0.0, a_m, flattening)
    d = tuple((ecef_after[i] - ecef_before[i]) / (2.0 * step_s) for i in range(3))
    phi, lam = math.radians(center[0]), math.radians(center[1])
    east = -math.sin(lam) * d[0] + math.cos(lam) * d[1]
    north = -math.sin(phi) * math.cos(lam) * d[0] - math.sin(phi) * math.sin(lam) * d[1] + math.cos(phi) * d[2]
    return east, north


def _rayleigh_max_bound(n: int, alpha: float) -> float:
    """Exact family-wise upper bound for max radius of N independent N(0,I2) draws."""
    if n <= 0 or not (0.0 < alpha < 1.0):
        raise ValidationError("invalid finite-sample noise bound parameters")
    one_minus_per_draw = (1.0 - alpha) ** (1.0 / n)
    # For chi-square with 2 degrees of freedom, F(x)=1-exp(-x/2).
    tail = 1.0 - one_minus_per_draw
    if tail <= 0.0:
        return math.inf
    return math.sqrt(-2.0 * math.log(tail))


def _keyed_normal(seed: int, event_id: int, axis_key: int, noise_cfg: dict[str, Any]) -> float:
    """Rebuild one declared Box-Muller latent from the versioned manifest key schedule."""
    mask = (1 << 64) - 1
    multiplier = int(noise_cfg.get("lcg_multiplier", 6_364_136_223_846_793_005))
    increment = int(noise_cfg.get("lcg_increment", 1_442_695_040_888_963_407))
    event_mul = int(noise_cfg.get("event_id_multiplier", 0x9E3779B97F4A7C15))
    axis_mul = int(noise_cfg.get("axis_multiplier", 0xD1B54A32D192ED03))
    state = (seed ^ ((event_id * event_mul) & mask) ^ ((axis_key * axis_mul) & mask)) & mask
    state = (state * multiplier + increment) & mask
    u1 = ((state >> 11) + 1.0) / ((1 << 53) + 1.0)
    state = (state * multiplier + increment) & mask
    u2 = (state >> 11) / float(1 << 53)
    return math.sqrt(-2.0 * math.log(u1)) * math.cos(math.tau * u2)


def _load_policy(protocol: dict[str, Any]) -> tuple[int, float, float, float]:
    limits = protocol.get("limits", {})
    if not isinstance(limits, dict):
        raise ValidationError("protocol.limits must be an object")
    max_rows = limits.get("max_rows_per_csv", MAX_ROWS)
    if not isinstance(max_rows, int) or max_rows <= 0 or max_rows > MAX_ROWS:
        raise ValidationError(f"protocol.max_rows_per_csv must be in 1..{MAX_ROWS}")
    analytic_tol = float(protocol.get("analytic_position_tolerance_m", DEFAULT_POSITION_TOLERANCE_M))
    velocity_tol = float(protocol.get("analytic_velocity_tolerance_mps", DEFAULT_VELOCITY_TOLERANCE_MPS))
    alpha = float(protocol.get("noise_family_alpha", NOISE_FAMILY_ALPHA))
    if not all(math.isfinite(x) and x > 0.0 for x in (analytic_tol, velocity_tol)):
        raise ValidationError("protocol analytic tolerances must be finite and positive")
    if not (0.0 < alpha < 0.05):
        raise ValidationError("protocol noise_family_alpha must be between 0 and 0.05")
    return max_rows, analytic_tol, velocity_tol, alpha


def _validate_protocol_schema(protocol: dict[str, Any]) -> None:
    """Accept only the reviewed comparison contract schema and its exact CSV wire fields."""
    if protocol.get("protocol_id") != "openatca-comparison" or protocol.get("version") != "1.0.3":
        raise UnqualifiedError("protocol ID/version is not a recognized comparison contract")
    profile = protocol.get("profile")
    claim_scope = protocol.get("claim_scope")
    csv_contract = protocol.get("csv")
    references = protocol.get("references")
    if not all(isinstance(value, dict) for value in (profile, claim_scope, csv_contract, references)):
        raise UnqualifiedError("protocol is missing the recognized profile, claim_scope, csv or references section")
    if not isinstance(profile.get("id"), str) or not profile["id"]:
        raise ValidationError("protocol.profile.id must be a nonempty string")
    if not isinstance(claim_scope.get("initial_cohort"), str) or not isinstance(claim_scope.get("not_established"), list):
        raise UnqualifiedError("protocol claim_scope does not match comparison contract schema 1.0.1")
    if csv_contract.get("truth") != list(TRUTH_FIELDS) or csv_contract.get("measurements") != list(MEASUREMENT_FIELDS):
        raise UnqualifiedError("protocol CSV fields do not match the supported comparison input schemas")
    if csv_contract.get("headers_exact") is not True or csv_contract.get("timestamp_unit") != "integer_nanoseconds":
        raise UnqualifiedError("protocol CSV encoding/order contract is unsupported")
    simulation_truth = references.get("simulation_truth")
    if not isinstance(simulation_truth, str) or not simulation_truth.strip():
        raise UnqualifiedError("protocol does not define its simulation-truth source")
    causality = protocol.get("causality_and_time")
    if not isinstance(causality, dict) or causality.get("future_measurements_or_smoothed_truth_in_candidate") is not False:
        raise UnqualifiedError("protocol does not explicitly prohibit future measurements or smoothed truth")
    qualification = protocol.get("initialization_and_qualification")
    if not isinstance(qualification, dict):
        raise UnqualifiedError("protocol v1.0.2 is missing initialization_and_qualification")
    if qualification.get("score_origin") != "scenario_manifest.grid_start_ns":
        raise UnqualifiedError("protocol score origin is unsupported")
    candidate_init = qualification.get("candidate_initialization", {})
    control_init = qualification.get("development_control_initialization", {})
    if candidate_init != {"scheme": "M_of_N_distinct_input_reports", "m": 2, "n": 3, "distinct_report_key": "event_id"}:
        raise UnqualifiedError("protocol candidate initialization contract is unsupported")
    if control_init != {"minimum_distinct_input_reports_before_state_is_usable": 2, "distinct_report_key": "event_id"}:
        raise UnqualifiedError("protocol development-control initialization contract is unsupported")
    if qualification.get("coast_when_actual_source_age_ns_greater_than") != 2_000_000_000:
        raise UnqualifiedError("protocol coast-age threshold is unsupported")
    postinit = qualification.get("post_initialization_window", {})
    if postinit != {
        "start_ns": "scenario_manifest.grid_start_ns + 2000000000",
        "exclude_before_start_from_postinit_metrics_only": True,
        "retain_cold_start_metrics": True,
        "retain_all_grid_metrics": True,
    }:
        raise UnqualifiedError("protocol post-initialization metric window is unsupported")
    if qualification.get("usable_statuses") != ["valid"]:
        raise UnqualifiedError("protocol usable-status definition is unsupported")
    nominal = qualification.get("nominal_scenarios", {})
    expected_nominal = {"straight", "turn", "acceleration", "crossing", "delayed"}
    if not isinstance(nominal, dict) or set(nominal) != expected_nominal:
        raise UnqualifiedError("protocol nominal qualification scenario set is unsupported")
    for scenario, disposition in nominal.items():
        if (not isinstance(disposition, dict) or disposition.get("role") != "qualification"
                or disposition.get("postinit_usable_ratio_min") != 0.999
                or disposition.get("candidate_ratio_at_least_baseline") is not True):
            raise UnqualifiedError(f"protocol nominal qualification role is unsupported for {scenario}")
    diagnostic = qualification.get("diagnostic_scenarios", {})
    dropout = diagnostic.get("dropout") if isinstance(diagnostic, dict) else None
    if (not isinstance(dropout, dict) or dropout.get("role") != "diagnostic_only"
            or dropout.get("qualification_gate_in_v1") is not False
            or dropout.get("never_reclassify_after_observing_candidate") is not True):
        raise UnqualifiedError("protocol dropout diagnostic role is unsupported")
    paired = qualification.get("paired_postinit_scoring")
    expected_paired = {
        "key": ["scenario_id", "flight_id", "score_ns"],
        "candidate_and_baseline_usable_keysets_must_match_for_accuracy_qualification": True,
        "mismatch_blocks_qualified_accuracy": True,
        "retain_raw_per_system_metrics_on_mismatch": True,
        "report_keyset_intersection_and_difference_counts": True,
    }
    if paired != expected_paired:
        raise UnqualifiedError("protocol paired post-initialization scoring contract is unsupported")
    cohort = protocol.get("fault_mode_cohort")
    if not isinstance(cohort, dict):
        raise UnqualifiedError("protocol v1.0.3 is missing its sealed fault-mode cohort")
    expected_fault_ids = list(range(1_000_101, 1_000_121))
    expected_fault_gates = [
        "offered_event_dispositions_match_the_frozen_dropout_schedule_with_zero_unexpected_loss",
        "every_required_score_grid_point_has_one_explicit_status_row",
        "no_valid_output_when_actual_supporting_source_age_exceeds_two_seconds",
        "recovery_is_valid_at_the_first_score_grid_after_a_fresh_measurement_is_received",
        "known_flight_identity_is_preserved_through_dropout_and_recovery",
    ]
    if (cohort.get("cohort_id") != "dropout_fault_v1"
            or cohort.get("status") != "BLOCKED_NOT_IMPLEMENTED_OR_REVIEWED"
            or cohort.get("scenario") != "dropout"
            or cohort.get("required_for_all_quality_pass") is not True
            or cohort.get("seed_ids") != expected_fault_ids
            or cohort.get("seed_count") != 20
            or cohort.get("allocation") != {
                "aircraft_count_per_seed": 300,
                "duration_s_per_aircraft": 1200,
                "simulated_aircraft_hours_total": 2000,
            }
            or cohort.get("separate_from_nominal_performance_seed_set") is not True
            or cohort.get("no_payload_generated_or_opened") is not True
            or cohort.get("replicates_are_independent_references") is not False
            or cohort.get("required_gates_pending_implementation_and_independent_review") != expected_fault_gates
            or cohort.get("all_quality_claim_status") != "BLOCKED_UNTIL_ALL_REQUIRED_GATES_PASS_ON_SEPARATE_COHORT"):
        raise UnqualifiedError("protocol dropout fault cohort or blocked gate contract is unsupported")
    seed_contract = protocol.get("seeds_and_holdout")
    nominal_contract = seed_contract.get("qualification") if isinstance(seed_contract, dict) else None
    qualification_seeds = nominal_contract.get("seed_ids") if isinstance(nominal_contract, dict) else None
    if qualification_seeds != list(range(1_000_001, 1_000_101)):
        raise UnqualifiedError("protocol nominal qualification seed allocation is unsupported")
    if set(expected_fault_ids) & set(qualification_seeds):
        raise ValidationError("sealed dropout fault seed IDs overlap the nominal performance seed set")


def _manifest_values(manifest: dict[str, Any]) -> tuple[str, int, int, int, int, float, float, float, float]:
    scenario = manifest.get("scenario_id", manifest.get("scenario"))
    if not isinstance(scenario, str) or not scenario:
        raise ValidationError("manifest must declare nonempty scenario_id")
    try:
        aircraft = int(manifest.get("aircraft_count", manifest.get("aircraft")))
        duration = int(manifest.get("duration_s", manifest.get("scenario_duration_s", manifest.get("duration_seconds"))))
        grid_start = int(manifest.get("grid_start_ns", manifest.get("epoch_ns")))
        grid_step = int(manifest.get("grid_step_ns", manifest.get("interval_ns", 1_000_000_000)))
        origin_lat = float(manifest.get("origin_lat_deg", 40.0))
        origin_lon = float(manifest.get("origin_lon_deg", -75.0))
        a_m = float(manifest.get("wgs84_a_m", manifest.get("wgs84_semi_major_axis_m", WGS84_A_M)))
        flattening = float(manifest.get("wgs84_f", manifest.get("wgs84_flattening", WGS84_F)))
    except (TypeError, ValueError, OverflowError) as e:
        raise ValidationError(f"manifest has missing or invalid numeric world fields: {e}") from e
    if not (1 <= aircraft <= 300 and 1 <= duration <= 1_200):
        raise ValidationError("manifest aircraft_count/duration_s is outside supported bounds")
    if grid_step != 1_000_000_000:
        raise ValidationError("current analytic world requires a one-second truth grid")
    if grid_start < 0 or grid_start + duration * grid_step > 2**63 - 1:
        raise ValidationError("manifest time range overflows supported signed nanoseconds")
    if not (-80.0 <= origin_lat <= 80.0 and -180.0 <= origin_lon <= 180.0):
        raise ValidationError("manifest origin is outside supported local WGS84 bounds")
        if not math.isclose(float(manifest.get("origin_alt_m", 0.0)), 0.0, abs_tol=1e-12):
            raise ValidationError("analytic v2 local frame requires zero origin altitude")
    if not (6_000_000.0 < a_m < 7_000_000.0 and 0.0 < flattening < 0.01):
        raise ValidationError("manifest WGS84 ellipsoid parameters are invalid")
    return scenario, aircraft, duration, grid_start, grid_step, origin_lat, origin_lon, a_m, flattening


def validate(
    truth_path: Path,
    measurement_path: Path,
    manifest_path: Path,
    protocol_path: Path,
    runner_source_path: Path | None = None,
) -> dict[str, Any]:
    """Validate a synthetic world and return a status-bearing, hash-bound evidence object."""
    paths = {
        "truth_sha256": truth_path,
        "measurements_sha256": measurement_path,
        "manifest_sha256": manifest_path,
        "protocol_sha256": protocol_path,
    }
    hashes: dict[str, str | None] = {}
    for label, path in paths.items():
        try:
            hashes[label] = _sha256(path)
        except OSError:
            hashes[label] = None
    if runner_source_path is not None:
        try:
            hashes["runner_source_sha256"] = _sha256(runner_source_path)
        except OSError:
            hashes["runner_source_sha256"] = None
    missing_hashes = [name for name, value in hashes.items() if name != "runner_source_sha256" and value is None]

    report: dict[str, Any] = {
        "validator": "openatca-comparison-reference-v1",
        "status": "FAIL",
        "validation_status": "FAIL",
        "reference_kind": "independent_simulation",
        "scope": "declared_model_only",
        "reason": "not validated",
        "physical_accuracy_claim": False,
        "interpretation": "synthetic world and generated-input consistency only; never physical accuracy",
        "hashes": hashes,
        "truth_input_sha256": hashes.get("truth_sha256"),
        "measurement_input_sha256": hashes.get("measurements_sha256"),
        "runner_source_sha256": hashes.get("runner_source_sha256"),
        "runner_source_hash_verified": hashes.get("runner_source_sha256") is not None,
        "protocol_sha256": hashes.get("protocol_sha256"),
        "checks": {},
    }
    if missing_hashes:
        report["status"] = "UNQUALIFIED"
        report["validation_status"] = "UNQUALIFIED"
        report["reason"] = "required input artifact unavailable: " + ", ".join(missing_hashes)
        return report
    if runner_source_path is None or hashes.get("runner_source_sha256") is None:
        report["status"] = "UNQUALIFIED"
        report["validation_status"] = "UNQUALIFIED"
        report["reason"] = "runner source bytes are required to bind the declared implementation hash"
        return report
    try:
        protocol = _read_json(protocol_path, 1024 * 1024)
        manifest = _read_json(manifest_path, 1024 * 1024)
        _validate_protocol_schema(protocol)
        max_rows, analytic_tol, velocity_tol, alpha = _load_policy(protocol)
        if manifest.get("model_id", manifest.get("world_model_id")) not in (
            "adsb_kinematic_v2",
        ):
            raise UnqualifiedError("manifest declares an unsupported analytic model_id")
        declared_runner_hash = manifest.get("runner_source_sha256", manifest.get("source_implementation_sha256"))
        if not isinstance(declared_runner_hash, str) or len(declared_runner_hash) != 64 or any(
            ch not in "0123456789abcdef" for ch in declared_runner_hash
        ):
            raise UnqualifiedError("manifest has no valid declared runner source SHA-256")
        if hashes["runner_source_sha256"] != declared_runner_hash:
            raise ValidationError("runner source hash does not match manifest declaration")
        (scenario, aircraft, duration, grid_start, grid_step, origin_lat, origin_lon,
         a_m, flattening) = _manifest_values(manifest)
        if not math.isclose(float(manifest.get("turn_rate_deg_s", 2.0)), 2.0, abs_tol=1e-12):
            raise ValidationError("manifest turn_rate_deg_s differs from analytic v2 model")
        if not math.isclose(float(manifest.get("turn_speed_mps", 150.0)), 150.0, abs_tol=1e-12):
            raise ValidationError("manifest turn_speed_mps differs from analytic v2 model")
        if manifest.get("turn_sign_rule") != "all_positive":
            raise UnqualifiedError("manifest must declare the supported all-positive turn direction rule")
        if manifest.get("crossing_sign_rule") != "even_positive_odd_negative":
            raise UnqualifiedError("manifest must declare the supported alternating crossing direction rule")
        if int(manifest.get("crossing_pair_columns", 10)) != 10:
            raise ValidationError("manifest crossing_pair_columns differs from analytic v2 model")
        if not math.isclose(float(manifest.get("crossing_pair_spacing_m", 5000.0)), 5000.0, abs_tol=1e-12):
            raise ValidationError("manifest crossing_pair_spacing_m differs from analytic v2 model")
        expected_truth_rows = aircraft * (duration + 1)
        declared_grid_count = manifest.get("grid_count")
        if declared_grid_count is not None and int(declared_grid_count) != duration + 1:
            raise ValidationError("manifest grid_count must equal duration_s + 1")
        declared_truth_rows = manifest.get("truth_rows")
        if declared_truth_rows is not None and int(declared_truth_rows) != expected_truth_rows:
            raise ValidationError("manifest truth_rows differs from the analytic grid size")
        if expected_truth_rows > max_rows:
            raise ValidationError("expected truth grid exceeds the protocol row limit")
        truth = _parse_truth(truth_path, max_rows)
        measurements = _parse_measurements(measurement_path, max_rows)
        if len(truth) != expected_truth_rows:
            raise ValidationError(
                f"truth row count {len(truth)} != expected {expected_truth_rows}"
            )
        if any(r.scenario_id != scenario for r in truth) or any(
            r.scenario_id != scenario for r in measurements
        ):
            raise ValidationError("scenario_id is inconsistent with manifest")
        truth_by_key: dict[tuple[int, int], TruthRow] = {}
        truth_max_position_error = 0.0
        truth_max_velocity_error = 0.0
        ecef_velocity_checks: list[float] = []
        phase_counts: dict[str, int] = {}
        expected_phases = {"straight": "straight", "dropout": "straight", "delayed": "straight",
                           "turn": "turn", "acceleration": "acceleration", "crossing": "crossing"}
        expected_phase = expected_phases.get(scenario)
        if expected_phase is None:
            raise ValidationError(f"unsupported scenario_id {scenario!r}")
        for idx, row in enumerate(truth):
            expected_sec = idx // aircraft
            expected_flight = idx % aircraft
            expected_time = grid_start + expected_sec * grid_step
            if row.flight_id != expected_flight or row.time_ns != expected_time:
                raise ValidationError("truth rows do not cover the complete unique one-second grid")
            if row.phase != expected_phase:
                raise ValidationError("truth phase does not match the declared scenario")
            expected = _analytic_state(scenario, row.flight_id, float(expected_sec), duration, aircraft,
                                       origin_lat, origin_lon, a_m, flattening)
            d_e, d_n = _local_error_m(row.lat_deg, row.lon_deg, expected[0], expected[1],
                                      origin_lat, a_m, flattening)
            pos_err = math.hypot(d_e, d_n)
            east_from_origin, north_from_origin = _local_error_m(
                row.lat_deg, row.lon_deg, origin_lat, origin_lon, origin_lat, a_m, flattening
            )
            if math.hypot(east_from_origin, north_from_origin) > 100_000.0:
                raise ValidationError("analytic truth exceeds the 100 km regional extent bound")
            vel_err = math.hypot(row.ve_mps - expected[2], row.vn_mps - expected[3])
            truth_max_position_error = max(truth_max_position_error, pos_err)
            truth_max_velocity_error = max(truth_max_velocity_error, vel_err)
            phase_counts[row.phase] = phase_counts.get(row.phase, 0) + 1
            if pos_err > analytic_tol:
                raise ValidationError(f"analytic trajectory position mismatch at flight {row.flight_id}, t={row.time_ns}")
            if vel_err > velocity_tol:
                raise ValidationError(f"analytic WGS84 velocity mismatch at flight {row.flight_id}, t={row.time_ns}")
            if expected_sec in (0, duration // 2, duration):
                fd = _ecef_central_difference_velocity(scenario, row.flight_id, float(expected_sec),
                                                       duration, aircraft, origin_lat, origin_lon, a_m, flattening)
                ecef_error = math.hypot(fd[0] - row.ve_mps, fd[1] - row.vn_mps)
                ecef_velocity_checks.append(ecef_error)
                if ecef_error > 0.03:
                    raise ValidationError("ECEF central-difference velocity disagrees with analytic WGS84 velocity")
            truth_by_key[(row.flight_id, row.time_ns)] = row

        schedule = manifest.get("schedule", {})
        if not isinstance(schedule, dict):
            raise ValidationError("manifest.schedule must be an object")
        delay_period = int(schedule.get("delay_period_s", 10))
        delay_residue = int(schedule.get("delay_residue_s", 5))
        delay_ns = int(schedule.get("delay_ns", manifest.get("measurement_delay_ns", 400_000_000)))
        dropout_period = int(schedule.get("dropout_period_s", 12))
        dropout_start = int(schedule.get("dropout_start_s", 4))
        dropout_end = int(schedule.get("dropout_end_s", 7))
        delay_rule = manifest.get("delay_rule")
        if delay_rule != "sec_mod_10_eq_5":
            raise ValidationError("manifest delay_rule differs from the supported v2 schedule")
        delay_period, delay_residue = 10, 5
        if scenario == "delayed" and delay_ns <= 1_000_000_000:
            raise ValidationError("delayed scenario must create receive-order inversion with >1 s delay")
        if scenario == "delayed" and delay_ns != 1_400_000_000:
            raise ValidationError("analytic v2 delayed scenario requires the frozen 1.4 s receive delay")
        dropout_rule = manifest.get("dropout_rule")
        if dropout_rule != "sec_mod_12_in_4_through_7":
            raise ValidationError("manifest dropout_rule differs from the supported v2 schedule")
        dropout_period, dropout_start, dropout_end = 12, 4, 7
        if delay_period <= 0 or not (0 <= delay_residue < delay_period):
            raise ValidationError("manifest delay schedule is invalid")
        if dropout_period <= 0 or not (0 <= dropout_start <= dropout_end < dropout_period):
            raise ValidationError("manifest dropout schedule is invalid")
        expected_event_rows = 0
        expected_pairs: set[tuple[int, int]] = set()
        expected_delay_ids: list[int] = []
        expected_dropout_ids: list[int] = []
        for sec in range(duration + 1):
            for flight in range(aircraft):
                dropped = scenario == "dropout" and dropout_start <= sec % dropout_period <= dropout_end
                if not dropped:
                    expected_event_rows += 1
                    expected_pairs.add((flight, grid_start + sec * grid_step))
                    if scenario == "delayed" and sec % delay_period == delay_residue:
                        expected_delay_ids.append(sec * aircraft + flight)
                else:
                    expected_dropout_ids.append(sec * aircraft + flight)
        if len(measurements) != expected_event_rows:
            raise ValidationError(f"measurement row count {len(measurements)} != expected {expected_event_rows}")
        for field in ("expected_measurements", "measurement_rows"):
            declared = manifest.get(field)
            if declared is not None and int(declared) != expected_event_rows:
                raise ValidationError(f"manifest {field} differs from the schedule")
        if expected_event_rows > max_rows:
            raise ValidationError("expected measurement rows exceed the protocol row limit")

        sigma_pos = float(manifest.get("sigma_pos_m", manifest.get("noise", {}).get("sigma_pos_m", SIGMA_POSITION_M)))
        sigma_vel = float(manifest.get("sigma_vel_mps", manifest.get("noise", {}).get("sigma_vel_mps", SIGMA_VELOCITY_MPS)))
        noise_cfg = manifest.get("noise", {})
        if not isinstance(noise_cfg, dict):
            raise ValidationError("manifest.noise must be an object")
        declared_noise_algorithm = manifest.get("noise_algorithm", noise_cfg.get("algorithm"))
        independent_noise = noise_cfg.get("independent_axes", manifest.get("independent_noise_axes"))
        if independent_noise is not True and not (
            isinstance(declared_noise_algorithm, str)
            and "independent" in declared_noise_algorithm.lower()
        ):
            raise ValidationError("manifest must declare independent Gaussian noise axes for this validator")
        if not (math.isclose(sigma_pos, SIGMA_POSITION_M, rel_tol=0.0, abs_tol=1e-7)
                and math.isclose(sigma_vel, SIGMA_VELOCITY_MPS, rel_tol=0.0, abs_tol=1e-7)):
            raise ValidationError("manifest sigma values differ from the declared comparison protocol")
        if not isinstance(declared_noise_algorithm, str) or "box-muller" not in declared_noise_algorithm.lower():
            raise ValidationError("manifest must declare the supported Box-Muller noise algorithm")
        noise_keying = manifest.get("noise_keying")
        if not isinstance(noise_keying, str) or "0x9E3779B97F4A7C15" not in noise_keying or "0xD1B54A32D192ED03" not in noise_keying:
            raise ValidationError("manifest does not declare the supported event/axis noise key derivation")
        event_keying = manifest.get("event_id_keying")
        if event_keying != "truth-grid ordinal: time_index*aircraft_count+flight_index, including absent IDs on dropouts":
            raise ValidationError("manifest event_id_keying does not match the declared generation order")
        if int(manifest.get("noise_seed", -1)) != int(manifest.get("seed", -2)):
            raise ValidationError("noise_seed must equal the run seed in this protocol")

        seen_pairs: set[tuple[int, int]] = set()
        event_id_by_pair = {(r.flight_id, r.sensor_ns): r.event_id for r in measurements}
        expected_event_id = 0
        expected_event_ids: dict[tuple[int, int], int] = {}
        for sec in range(duration + 1):
            for flight in range(aircraft):
                dropped = scenario == "dropout" and dropout_start <= sec % dropout_period <= dropout_end
                if not dropped:
                    expected_event_ids[(flight, grid_start + sec * grid_step)] = sec * aircraft + flight
        if event_id_by_pair != expected_event_ids:
            raise ValidationError("event_id does not match deterministic scenario-time/flight ordering")
        pos_radii: list[float] = []
        vel_radii: list[float] = []
        position_components: list[tuple[float, float]] = []
        velocity_components: list[tuple[float, float]] = []
        max_latency = 0
        delayed_count = 0
        sensors: set[str] = set()
        for row in measurements:
            if row.flight_id >= aircraft:
                raise ValidationError("measurement flight_id is outside the manifest flight range")
            key = (row.flight_id, row.sensor_ns)
            if key not in truth_by_key:
                raise ValidationError("measurement has no matching truth event by flight/time identifier")
            if key in seen_pairs:
                raise ValidationError("more than one measurement exists for a flight/time event")
            seen_pairs.add(key)
            if row.receive_ns < row.sensor_ns:
                raise ValidationError("measurement arrives before its sensor time")
            sec = (row.sensor_ns - grid_start) // grid_step
            if row.sensor_ns != grid_start + sec * grid_step or not (0 <= sec <= duration):
                raise ValidationError("measurement sensor_ns is not on the declared truth grid")
            expected_latency = delay_ns if scenario == "delayed" and sec % delay_period == delay_residue else 0
            latency = row.receive_ns - row.sensor_ns
            if latency != expected_latency:
                raise ValidationError("measurement receive_ns violates the declared causal delay schedule")
            max_latency = max(max_latency, latency)
            if latency:
                delayed_count += 1
            if not math.isclose(row.sigma_pos_m, sigma_pos, rel_tol=0.0, abs_tol=0.000051):
                raise ValidationError("row sigma_pos_m differs from manifest covariance")
            if not math.isclose(row.sigma_vel_mps, sigma_vel, rel_tol=0.0, abs_tol=0.000051):
                raise ValidationError("row sigma_vel_mps differs from manifest covariance")
            ref = truth_by_key[key]
            # The runner applies Gaussian ENU offsets using radii at the nominal truth
            # latitude for that event; use the same local basis to recover the latent.
            east, north = _local_error_m(row.lat_deg, row.lon_deg, ref.lat_deg, ref.lon_deg,
                                         ref.lat_deg, a_m, flattening)
            ve_noise = row.ve_mps - ref.ve_mps
            vn_noise = row.vn_mps - ref.vn_mps
            p = (east / sigma_pos, north / sigma_pos)
            v = (ve_noise / sigma_vel, vn_noise / sigma_vel)
            # Reconstruct the input generator's independent per-event/per-axis latent from its
            # manifest key schedule. Coordinate and CSV decimal formatting introduce small
            # quantization error, so compare against explicit tolerances rather than bit equality.
            noise_seed = int(manifest.get("noise_seed", manifest.get("seed", 0)))
            key_noise_cfg = {
                **noise_cfg,
                "lcg_multiplier": manifest.get("noise_lcg_multiplier", noise_cfg.get("lcg_multiplier", 6_364_136_223_846_793_005)),
                "lcg_increment": manifest.get("noise_lcg_increment", noise_cfg.get("lcg_increment", 1_442_695_040_888_963_407)),
            }
            axis_keys = manifest.get("noise_axis_keys", noise_cfg.get("axis_keys", [1, 2, 3, 4]))
            if not isinstance(axis_keys, list) or len(axis_keys) != 4 or len(set(axis_keys)) != 4:
                raise ValidationError("manifest noise_axis_keys must contain four distinct axis identifiers")
            e_expected = _keyed_normal(noise_seed, row.event_id, int(axis_keys[0]), key_noise_cfg) * sigma_pos
            n_expected = _keyed_normal(noise_seed, row.event_id, int(axis_keys[1]), key_noise_cfg) * sigma_pos
            ve_expected = _keyed_normal(noise_seed, row.event_id, int(axis_keys[2]), key_noise_cfg) * sigma_vel
            vn_expected = _keyed_normal(noise_seed, row.event_id, int(axis_keys[3]), key_noise_cfg) * sigma_vel
            if abs(east - e_expected) > 0.003 or abs(north - n_expected) > 0.003:
                raise ValidationError("position residual disagrees with declared seed/event/axis latent")
            if abs(ve_noise - ve_expected) > 0.0002 or abs(vn_noise - vn_expected) > 0.0002:
                raise ValidationError("velocity residual disagrees with declared seed/event/axis latent")
            position_components.append(p)
            velocity_components.append(v)
            pos_radii.append(math.hypot(*p))
            vel_radii.append(math.hypot(*v))
            sensors.add(row.sensor_id)
        if seen_pairs != expected_pairs:
            missing = len(expected_pairs - seen_pairs)
            unexpected = len(seen_pairs - expected_pairs)
            raise ValidationError(f"measurement schedule mismatch: missing={missing}, unexpected={unexpected}")
        if scenario == "delayed" and not any(
            measurements[i].sensor_ns < measurements[i - 1].sensor_ns
            for i in range(1, len(measurements))
        ):
            raise ValidationError("delayed scenario did not create the declared receive-order inversion")
        expected_delay_count = delayed_count
        declared_delay_count = manifest.get("delay_event_count", manifest.get("delayed_event_count"))
        if declared_delay_count is not None and int(declared_delay_count) != expected_delay_count:
            raise ValidationError("manifest delayed_event_count differs from measurement schedule")
        expected_dropout_count = aircraft * sum(
            1 for sec in range(duration + 1)
            if scenario == "dropout" and dropout_start <= sec % dropout_period <= dropout_end
        )
        declared_dropout_count = manifest.get("dropout_event_count")
        if declared_dropout_count is not None and int(declared_dropout_count) != expected_dropout_count:
            raise ValidationError("manifest dropout_event_count differs from analytic schedule")
        for field, expected_ids in (("delay_event_ids", expected_delay_ids),
                                    ("dropout_event_ids", expected_dropout_ids)):
            if manifest.get(field) != expected_ids:
                raise ValidationError(f"manifest {field} differs from deterministic schedule event IDs")
        for field in ("measurements_offered", "measurements_applied"):
            declared = manifest.get(field)
            if declared is not None and int(declared) != expected_event_rows:
                raise ValidationError(f"manifest {field} differs from the complete offered input set")
        for field in (
            "measurement_refused", "duplicate_measurements", "late_measurements",
            "late_window", "late_ring", "jump_rejected", "numerical_refused",
            "capacity_refused", "work_refused", "work_counter_overflow",
            "work_counter_overflowed", "tracker_output_overflow", "queue_pending_at_end",
            "event_queue_pending_at_end", "held_unapplied", "rows_unrepresentable",
            "forecast_numerical", "forecast_unrepresentable",
        ):
            declared = manifest.get(field)
            if declared is not None and int(declared) != 0:
                raise ValidationError(f"manifest loss ledger {field} is nonzero")

        # A finite-N, family-wise tail check on 2-D standardized residuals. This is a synthetic
        # noise-generation sanity check, not an estimate of physical sensor accuracy.
        p_bound = _rayleigh_max_bound(len(pos_radii), alpha)
        v_bound = _rayleigh_max_bound(len(vel_radii), alpha)
        observed_p = max(pos_radii)
        observed_v = max(vel_radii)
        if observed_p > p_bound or observed_v > v_bound:
            raise ValidationError("observed measurement residual exceeds finite-sample Gaussian envelope")

        def moments(values: list[tuple[float, float]]) -> dict[str, Any]:
            n = len(values)
            means = [sum(v[i] for v in values) / n for i in (0, 1)]
            variances = [sum((v[i] - means[i]) ** 2 for v in values) / max(1, n - 1) for i in (0, 1)]
            # 4.0 standard errors is deliberately broad and finite-sample-scaled; the stricter
            # max-radius check above is the formal probability bound.
            mean_bound = 4.0 / math.sqrt(n)
            if any(abs(m) > mean_bound for m in means):
                raise ValidationError("standardized residual mean exceeds the finite-N sanity bound")
            return {
                "n": n,
                "standardized_component_mean": means,
                "standardized_component_sample_variance": variances,
                "mean_component_abs_bound_4se": mean_bound,
                "interpretation": "finite-sample synthetic-noise diagnostic, not physical accuracy",
            }

        report["protocol_id"] = protocol.get("protocol_id")
        report["protocol_version"] = protocol.get("version")
        report["protocol_status"] = protocol.get("status")
        report["protocol_profile_id"] = protocol.get("profile", {}).get("id")
        report["fault_mode_cohort_status"] = protocol["fault_mode_cohort"]["status"]
        report["all_quality_claim_status"] = protocol["fault_mode_cohort"]["all_quality_claim_status"]
        report["model_id"] = manifest.get("model_id", manifest.get("world_model_id"))
        report["runner_source_sha256"] = hashes.get("runner_source_sha256") or declared_runner_hash
        report["runner_source_hash_verified"] = hashes.get("runner_source_sha256") == declared_runner_hash
        report["scenario_id"] = scenario
        report["checks"] = {
            "bounded_csv_parse": "PASS",
            "truth_grid_order_unique_and_complete": "PASS",
            "closed_form_wgs84_trajectory_and_velocity": "PASS",
            "ecef_central_difference_velocity_sanity": "PASS",
            "measurement_ids_unique_dense_and_receive_sorted": "PASS",
            "causal_sensor_receive_schedule": "PASS",
            "dropout_schedule_and_measurement_denominator": "PASS",
            "reported_covariance_matches_manifest": "PASS",
            "synthetic_gaussian_noise_finite_n_envelope": "PASS",
        }
        report["counts"] = {
            "truth_rows": len(truth),
            "measurement_rows": len(measurements),
            "aircraft": aircraft,
            "duration_s": duration,
            "expected_grid_rows": expected_truth_rows,
            "expected_measurement_events": expected_event_rows,
            "observed_sensors": sorted(sensors),
            "delay_events": delayed_count,
            "max_receive_latency_ns": max_latency,
            "phase_rows": phase_counts,
        }
        report["analytic_oracle"] = {
            "max_truth_horizontal_error_m": truth_max_position_error,
            "max_truth_velocity_vector_error_mps": truth_max_velocity_error,
            "max_ecef_central_difference_velocity_error_mps": max(ecef_velocity_checks, default=0.0),
            "ecef_velocity_check_samples": len(ecef_velocity_checks),
            "position_tolerance_m": analytic_tol,
            "velocity_tolerance_mps": velocity_tol,
            "geodetic_method": "independent WGS84 radii of curvature and local tangent mapping",
        }
        report["noise_diagnostics"] = {
            "declared_sigma_pos_m": sigma_pos,
            "declared_sigma_vel_mps": sigma_vel,
            "position_standardized_max_radius": observed_p,
            "position_finite_sample_familywise_max_bound": p_bound,
            "velocity_standardized_max_radius": observed_v,
            "velocity_finite_sample_familywise_max_bound": v_bound,
            "family_alpha_per_domain": alpha,
            "position_components": moments(position_components),
            "velocity_components": moments(velocity_components),
            "physical_accuracy_claim": False,
        }
        report["status"] = "PASS"
        report["validation_status"] = "PASS"
        report["reason"] = "synthetic world and generated input consistency checks passed"
    except UnqualifiedError as e:
        report["status"] = "UNQUALIFIED"
        report["validation_status"] = "UNQUALIFIED"
        report["reason"] = str(e)
    except (ValidationError, OSError, ValueError, TypeError, OverflowError, KeyError) as e:
        report["status"] = "FAIL"
        report["validation_status"] = "FAIL"
        report["reason"] = str(e)
    return report


def _cli() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--truth", required=True, type=Path)
    parser.add_argument("--measurements", required=True, type=Path)
    parser.add_argument("--manifest", required=True, type=Path)
    parser.add_argument("--protocol", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--runner-source", type=Path)
    args = parser.parse_args()
    result = validate(args.truth, args.measurements, args.manifest, args.protocol, args.runner_source)
    try:
        args.out.parent.mkdir(parents=True, exist_ok=True)
        with args.out.open("w", encoding="utf-8", newline="\n") as f:
            json.dump(result, f, indent=2, sort_keys=True, allow_nan=False)
            f.write("\n")
    except OSError as e:
        print(f"could not write evidence JSON: {e}", file=sys.stderr)
        return 2
    print(json.dumps({"status": result["status"], "reason": result["reason"], "out": str(args.out)}))
    return 0 if result["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(_cli())
