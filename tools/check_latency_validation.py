#!/usr/bin/env python3
"""Validate the checked-in latency aggregate and its predeclared target claims."""

import json
import math
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_REPORT = ROOT / "validation" / "latency-v1.json"
SCHEMA = "openatca-latency-v1"
METRICS = {
    "converging_pair_alert_scan_to_frame_packet": "alert",
    "scan_input_to_frame_packet": "display",
    "flight_plan_amendment": "amendment",
    "handoff_initiate_accept_take": "handoff",
}
REQUIRED_CHECKS = (
    "alert_in_creating_scan",
    "all_pass",
    "amendment_p99_under_10ms",
    "display_p99_under_100ms",
    "handoff_p99_under_10ms",
)
REQUIRED_TARGETS = ("alert_same_scan", "display_p99_pass")
LIMIT_KEYS = {
    "display": "display_p99_limit_ms",
    "amendment": "amendment_p99_limit_ms",
    "handoff": "handoff_p99_limit_ms",
}
EXPECTED_LIMITS = {"display": 100, "amendment": 10, "handoff": 10}


class ValidationError(ValueError):
    pass


def _reject_constant(value):
    raise ValidationError(f"non-finite JSON number {value}")


def _mapping(parent, name):
    value = parent.get(name)
    if not isinstance(value, dict):
        raise ValidationError(f"{name} must be an object")
    return value


def _boolean(mapping, key, label):
    value = mapping.get(key)
    if type(value) is not bool:
        raise ValidationError(f"{label}.{key} must be a boolean")
    return value


def _count(counts, key):
    value = counts.get(key)
    if type(value) is not int or value <= 0:
        raise ValidationError(f"counts.{key} must be a positive integer")
    return value


def _finite_nonnegative(value, label):
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ValidationError(f"{label} must be a finite nonnegative number")
    if not math.isfinite(value) or value < 0:
        raise ValidationError(f"{label} must be a finite nonnegative number")
    return value


def validate_report(report):
    if not isinstance(report, dict) or report.get("schema") != SCHEMA:
        raise ValidationError(f"schema must be {SCHEMA!r}")
    checks = _mapping(report, "checks")
    targets = _mapping(report, "targets")
    counts = _mapping(report, "counts")
    metrics = _mapping(report, "metrics")

    check_values = {key: _boolean(checks, key, "checks") for key in REQUIRED_CHECKS}
    target_values = {key: _boolean(targets, key, "targets") for key in REQUIRED_TARGETS}
    for label, key in LIMIT_KEYS.items():
        value = targets.get(key)
        if type(value) not in (int, float) or not math.isfinite(value) or value != EXPECTED_LIMITS[label]:
            raise ValidationError(f"targets.{key} must remain {EXPECTED_LIMITS[label]} ms")

    expected_samples = {
        "alert": _count(counts, "synthetic_alert_trials"),
        "display": _count(counts, "normal_push_frames") - 1,
        "amendment": _count(counts, "expected_plan_change_conflicts"),
        "handoff": _count(counts, "expected_plan_change_conflicts"),
    }
    if expected_samples["display"] <= 0:
        raise ValidationError("counts.normal_push_frames must include a warmup plus measured frames")
    for key in ("alerts_rendered_in_creating_scan", "normal_push_reports_processed", "normal_push_reports_supplied",
                "flight_operations", "flight_operations_applied", "refused_operations"):
        value = counts.get(key)
        if type(value) is not int or value < 0:
            raise ValidationError(f"counts.{key} must be a nonnegative integer")
    if counts["alerts_rendered_in_creating_scan"] != expected_samples["alert"]:
        raise ValidationError("alert trial and rendered-in-creating-scan counts disagree")
    if counts["normal_push_reports_processed"] != counts["normal_push_reports_supplied"]:
        raise ValidationError("normal-push supplied and processed report counts disagree")
    if counts["flight_operations_applied"] != counts["flight_operations"] or counts["refused_operations"] != 0:
        raise ValidationError("flight operation counts show missing application or refusals")

    p99_pass = {}
    for metric_name, kind in METRICS.items():
        metric = metrics.get(metric_name)
        if not isinstance(metric, dict):
            raise ValidationError(f"metrics.{metric_name} must be an object")
        if metric.get("unit") != "ns":
            raise ValidationError(f"metrics.{metric_name}.unit must be 'ns'")
        n = metric.get("n")
        if type(n) is not int or n <= 0 or n != expected_samples[kind]:
            raise ValidationError(f"metrics.{metric_name}.n must equal {expected_samples[kind]}")
        values = {field: _finite_nonnegative(metric.get(field), f"metrics.{metric_name}.{field}")
                  for field in ("p50", "p95", "p99", "max")}
        if not values["p50"] <= values["p95"] <= values["p99"] <= values["max"]:
            raise ValidationError(f"metrics.{metric_name} percentile values are out of order")
        if kind in LIMIT_KEYS:
            limit_ns = targets[LIMIT_KEYS[kind]] * 1_000_000
            p99_pass[kind] = values["p99"] < limit_ns

    if not target_values["alert_same_scan"] or not check_values["alert_in_creating_scan"]:
        raise ValidationError("same-scan alert target is false")
    if not target_values["display_p99_pass"]:
        raise ValidationError("display P99 target is false")
    expected_checks = {
        "alert_in_creating_scan": target_values["alert_same_scan"],
        "display_p99_under_100ms": p99_pass["display"],
        "amendment_p99_under_10ms": p99_pass["amendment"],
        "handoff_p99_under_10ms": p99_pass["handoff"],
    }
    for key, expected in expected_checks.items():
        if check_values[key] != expected:
            raise ValidationError(f"checks.{key} disagrees with measured counts/metrics")
        if not check_values[key]:
            raise ValidationError(f"checks.{key} is false")
    if check_values["all_pass"] != all(expected_checks.values()) or not check_values["all_pass"]:
        raise ValidationError("checks.all_pass is false or inconsistent")
    return True


def load_report(path):
    try:
        with Path(path).open(encoding="utf-8") as stream:
            return json.load(stream, parse_constant=_reject_constant)
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise ValidationError(f"cannot read JSON report: {exc}") from exc


def main(argv=None):
    args = sys.argv[1:] if argv is None else argv
    if len(args) > 1:
        print("usage: check_latency_validation.py [report.json]", file=sys.stderr)
        return 2
    path = args[0] if args else DEFAULT_REPORT
    try:
        validate_report(load_report(path))
    except ValidationError as exc:
        print(f"latency report invalid: {exc}", file=sys.stderr)
        return 1
    print("latency-v1 report valid")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
