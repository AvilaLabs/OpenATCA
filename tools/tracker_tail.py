#!/usr/bin/env python3
"""Bounded, aggregate-only tail attribution for verified P-002 development exports.

This offline shell tool does not replay or rescore tracker output. It groups common two-way
P-002 rows by fixed phase, altitude, tracker flags, heading disagreement, and interval between
successive scored samples for the same aircraft address. That interval is not an ADS-B report
gap. Use turn_diag for report-age and reference-turn diagnostics when its inputs are authorized.

Input records are time-checked against the production sealed-window guard before their other
fields are interpreted. Output contains no aircraft addresses, sample timestamps, or source labels.
"""

from __future__ import annotations

import argparse
import csv
import datetime as dt
import itertools
import json
import math
import os
import re
import stat
import sys
from collections import defaultdict
from dataclasses import dataclass
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import sealed_guard  # noqa: E402


# C2/C7: every in-memory row-derived collection is bounded by MAX_ROWS; address groups and
# quantile buffers are partitions/copies of that same bounded input. Output categories are fixed.
MAX_FILE_BYTES = 128 * 1024 * 1024
MAX_ROWS = 150_000
MAX_FIELD_CHARS = 4096
MAX_OUTPUT_BYTES = 1024 * 1024
MAX_ADDRS = 150_000
MAX_REPORT_LINES = 20_000

SAMPLE_FIELDS = (
    "addr", "t_ms", "phase", "band", "common", "common_two_way", "oa_miss",
    "oa_cross_m", "oa_along_raw_m", "oa_along_fit_m", "oa_h_fit_m", "oa_along_tol_m",
    "oa_h_tol_m", "oa_speed_err_mps", "oa_track_err_deg", "oa_row_dt_ms", "eram_miss",
    "eram_cross_m", "eram_along_raw_m", "eram_along_fit_m", "eram_h_fit_m",
    "eram_along_tol_m", "eram_h_tol_m", "eram_speed_err_mps", "eram_track_err_deg",
    "eram_row_dt_ms", "pt_miss", "pt_cross_m", "pt_along_raw_m", "pt_along_fit_m",
    "pt_h_fit_m", "pt_along_tol_m", "pt_h_tol_m", "pt_speed_err_mps", "pt_track_err_deg",
    "pt_row_dt_ms", "oa_coast", "oa_uncertain", "oa_reacquired",
)
PHASES = ("climb", "cruise", "descent", "unknown")
BANDS = (
    "below 10000 ft", "10000 to 17999 ft", "18000 to 28999 ft",
    "29000 to 40999 ft", "41000 ft and above", "no altitude",
)
_INT = re.compile(r"(?:0|[1-9][0-9]*)\Z", re.ASCII)
_SIGNED_INT = re.compile(r"-?(?:0|[1-9][0-9]*)\Z", re.ASCII)
_FLOAT = re.compile(r"-?(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)(?:[eE][-+]?[0-9]+)?\Z", re.ASCII)
_ADDR = re.compile(r"[0-9A-Fa-f]{6}\Z", re.ASCII)
_MAX_I64 = (1 << 63) - 1
_MAX_MS = _MAX_I64 // 1_000_000
MAX_JOINT_COHORTS = 4 * 4 * 2
MAX_COHORTS_PER_DIMENSION = MAX_JOINT_COHORTS


class TailError(ValueError):
    """Invalid or refused development input."""


@dataclass(frozen=True, slots=True)
class DevelopmentRun:
    label: str
    first_ms: int
    last_ms_exclusive: int
    excluded_grid_samples: int
    synthetic: bool = False


@dataclass(slots=True)
class Sample:
    # Address is retained only in memory to compute the previous sample interval.
    addr: str
    t_ms: int
    phase: str
    band: str
    cross_m: float
    heading_err_deg: float | None
    coast: bool
    uncertain: bool
    reacquired: bool
    gap_s: float | None = None


def nearest_rank(values: list[float], quantile: float) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    return ordered[max(0, math.ceil(quantile * len(ordered)) - 1)]


def _epoch_ms(year: int, month: int, day: int, hour: int = 0, minute: int = 0, second: int = 0) -> int:
    stamp = dt.datetime(year, month, day, hour, minute, second, tzinfo=dt.timezone.utc)
    return int(stamp.timestamp()) * 1000


def _read_bounded(path: Path, limit: int, kind: str) -> list[str]:
    """Read UTF-8 lines through OA-SEAL-002's descriptor-checked production opener."""
    try:
        with sealed_guard.open_read(path, "rb") as stream:
            info = os.fstat(stream.fileno())
            if not stat.S_ISREG(info.st_mode):
                raise TailError(f"{kind} must be a regular file")
            if info.st_size > limit:
                raise TailError(f"{kind} exceeds byte limit {limit}")
            raw = stream.read(limit + 1)
            if len(raw) > limit:
                raise TailError(f"{kind} exceeds byte limit {limit}")
            lines = raw.decode("utf-8-sig").splitlines(keepends=True)
            line_limit = MAX_ROWS + 3 if kind == "samples" else MAX_REPORT_LINES
            if len(lines) > line_limit:
                raise TailError(f"{kind} exceeds line limit {line_limit}")
            return lines
    except (OSError, UnicodeError, csv.Error, sealed_guard.SealedError) as exc:
        raise TailError(f"cannot safely read {kind}: {exc}") from exc


def verify_development_report(report_path: Path, samples_path: Path, *, allow_synthetic: bool = False) -> DevelopmentRun:
    """Require a sibling P-002 report with one of the explicitly permitted development labels."""
    sample_name_ok = samples_path.name == "samples.csv" or (
        allow_synthetic and samples_path.name == "tracker_tail_synthetic.csv"
    )
    if report_path.name != "p002.txt" or not sample_name_ok:
        raise TailError("expected sibling p002.txt and samples.csv files")
    try:
        if report_path.parent.resolve(strict=True) != samples_path.parent.resolve(strict=True):
            raise TailError("p002 report and samples export must be siblings")
    except OSError as exc:
        raise TailError(f"cannot resolve paired development artifacts: {exc}") from exc

    lines = _read_bounded(report_path, 2 * 1024 * 1024, "procedure report")
    if not lines:
        raise TailError("empty P-002 procedure report")
    first_line = lines[0].rstrip("\r\n")
    joined = "".join(lines)
    required_header = "P-002 scoring with the P-003 to P-006 amendments: OpenATCA, ERAM, passthrough baseline"
    if not any(line.rstrip("\r\n") == required_header for line in lines[:3]):
        raise TailError("procedure report is not the required P-002 scoring format")

    labels = [line[len("  label: "):].rstrip("\r\n") for line in lines if line.startswith("  label: ")]
    if len(labels) != 1:
        raise TailError("procedure report must contain one label")
    label = labels[0]
    if label == "P-006 section 10 development replay 2026-11-15":
        if first_line != "DEVELOPMENT RUN — not a hold-out score":
            raise TailError("Nov15 report is missing its development-run marker")
        if "2026.11.15/" not in joined or "excluded windows: 0; grid samples removed in all: 0" not in joined:
            raise TailError("Nov15 report metadata does not match the authorized development run")
        run = DevelopmentRun(label, _epoch_ms(2026, 11, 15), _epoch_ms(2026, 11, 16), 0)
    elif label == "P-006 section 10 strict tune rehearsal 2026-11-19":
        if "2026.11.19/" not in joined and "2026-11-19" not in joined:
            raise TailError("Nov19 report metadata does not match the authorized rehearsal")
        if "strict tune rehearsal" not in label:
            raise TailError("Nov19 metadata is not a strict tune rehearsal")
        # This is the explicitly permitted pre-holdout rehearsal interval. The production sealed
        # guard independently refuses its sealed-window margin beginning at 23:45Z.
        run = DevelopmentRun(label, _epoch_ms(2026, 11, 19), _epoch_ms(2026, 11, 19, 23, 44, 0) + 1, 0)
    elif allow_synthetic and label == "SYNTHETIC tracker-tail test fixture":
        if first_line != "SYNTHETIC fixture; no operational or real-aircraft data":
            raise TailError("synthetic report marker is invalid")
        run = DevelopmentRun(label, 0, _epoch_ms(1970, 1, 2), 0, synthetic=True)
    else:
        raise TailError("procedure report is not an allowed development run")

    removed = re.search(r"excluded windows: ([0-9]+); grid samples removed in all: ([0-9]+)", joined)
    if not removed:
        raise TailError("procedure report lacks exclusion accounting")
    removed_count = int(removed.group(2))
    if label.endswith("2026-11-15") and removed_count != 0:
        raise TailError("Nov15 procedure report unexpectedly excludes grid samples")
    if label.endswith("2026-11-19") and removed_count != 110:
        raise TailError("Nov19 procedure report does not match the authorized rehearsal exclusions")
    return DevelopmentRun(run.label, run.first_ms, run.last_ms_exclusive, removed_count, run.synthetic)


def _parse_time_ms(raw: str, run: DevelopmentRun, guard: sealed_guard.Guard | None = None) -> int:
    if not _INT.fullmatch(raw):
        raise TailError("sample t_ms must be a canonical nonnegative integer")
    value = int(raw)
    if value > _MAX_MS:
        raise TailError("sample t_ms cannot be converted to signed nanoseconds")
    ns = value * 1_000_000  # checked above before multiplication, per E8
    if (guard or sealed_guard.production()).in_window(ns):
        raise TailError("sample timestamp falls in the sealed hold-out window; refusing the entire export")
    if value < run.first_ms or value >= run.last_ms_exclusive:
        raise TailError("sample timestamp is outside the paired development run window")
    return value


def _parse_float(raw: str, column: str, *, optional: bool = False) -> float | None:
    if raw == "" and optional:
        return None
    if not _FLOAT.fullmatch(raw):
        raise TailError(f"{column} must be a finite numeric value" if raw else f"{column} is required")
    value = float(raw)
    if not math.isfinite(value):
        raise TailError(f"{column} must be finite")
    return value


def _parse_i64(raw: str, column: str) -> int:
    if not _SIGNED_INT.fullmatch(raw):
        raise TailError(f"{column} must be a canonical signed integer")
    value = int(raw)
    if not -(1 << 63) <= value <= _MAX_I64:
        raise TailError(f"{column} is outside signed 64-bit range")
    return value


def load_samples(samples_path: Path, report_path: Path, *, allow_synthetic: bool = False) -> dict[str, object]:
    """Validate the paired export, every time, schema, flags and category labels before scoring."""
    sealed_guard.refuse(samples_path, report_path)
    run = verify_development_report(report_path, samples_path, allow_synthetic=allow_synthetic)
    lines = _read_bounded(samples_path, MAX_FILE_BYTES, "samples")
    if not lines:
        raise TailError("empty samples export")
    if len(lines) > MAX_ROWS + 3:
        raise TailError(f"samples export exceeds row limit {MAX_ROWS}")
    # DictReader would silently accept extra columns under a None key; strict rows make corruption visible.
    content = iter(lines)
    first = next(content).rstrip("\r\n")
    if first.startswith("#"):
        try:
            header_line = next(content).rstrip("\r\n")
        except StopIteration as exc:
            raise TailError("sample export has no header") from exc
    else:
        header_line = first
        content = iter(lines[1:])
    reader = csv.reader(itertools.chain((header_line,), content), strict=True)
    try:
        header = next(reader)
    except (StopIteration, csv.Error) as exc:
        raise TailError("sample export has no valid CSV header") from exc
    if tuple(header) != SAMPLE_FIELDS:
        raise TailError("sample export header does not match the required P-002 schema")
    if not (header_line == ",".join(SAMPLE_FIELDS) and tuple(header) == SAMPLE_FIELDS):
        raise TailError("sample export metadata/header is corrupt")

    rows: list[Sample] = []
    total = common = noncommon = missing_cross = 0
    csv.field_size_limit(MAX_FIELD_CHARS)
    for record_number, fields in enumerate(reader, start=1):
        if record_number > MAX_ROWS:
            raise TailError(f"samples export exceeds row limit {MAX_ROWS}")
        total += 1
        if len(fields) != len(SAMPLE_FIELDS) or any(len(field) > MAX_FIELD_CHARS for field in fields):
            raise TailError(f"sample row {record_number} has invalid width or an oversized field")
        row = dict(zip(SAMPLE_FIELDS, fields, strict=True))

        # Check the timestamp before interpreting any measurements, labels, or identity fields.
        t_ms = _parse_time_ms(row["t_ms"], run)
        if not _ADDR.fullmatch(row["addr"]):
            raise TailError(f"sample row {record_number} has an invalid aircraft address")
        if row["phase"] not in PHASES or row["band"] not in BANDS:
            raise TailError(f"sample row {record_number} has a noncanonical phase or altitude band")
        for flag in ("common", "common_two_way", "oa_miss", "eram_miss", "pt_miss"):
            if row[flag] not in ("0", "1"):
                raise TailError(f"sample row {record_number} has an invalid {flag} flag")
        for flag in ("oa_coast", "oa_uncertain", "oa_reacquired"):
            allowed = ("0", "1") if row["common_two_way"] == "1" else ("", "0", "1")
            if row[flag] not in allowed:
                raise TailError(f"sample row {record_number} has an invalid {flag} flag")

        # Validate all exported numeric fields. Empty values are legitimate where a system missed
        # a row or a heading/error is unavailable; malformed text and NaN/inf are never normalized.
        for column in SAMPLE_FIELDS:
            if column.endswith("_m") or column.endswith("_mps") or column.endswith("_deg"):
                _parse_float(row[column], column, optional=True)
        for column in ("oa_row_dt_ms", "eram_row_dt_ms", "pt_row_dt_ms"):
            if row[column]:
                _parse_i64(row[column], column)

        if row["common_two_way"] != "1":
            noncommon += 1
            continue
        if row["oa_miss"] != "0" or row["eram_miss"] != "0":
            raise TailError(f"sample row {record_number} marks a miss inside the common two-way set")
        common += 1
        cross = _parse_float(row["oa_cross_m"], "oa_cross_m", optional=True)
        if cross is None:
            missing_cross += 1
            continue
        heading = _parse_float(row["oa_track_err_deg"], "oa_track_err_deg", optional=True)
        rows.append(Sample(
            addr=row["addr"].upper(),
            t_ms=t_ms,
            phase=row["phase"],
            band=row["band"],
            cross_m=cross,
            heading_err_deg=heading,
            coast=row["oa_coast"] == "1",
            uncertain=row["oa_uncertain"] == "1",
            reacquired=row["oa_reacquired"] == "1",
        ))
    if total == 0:
        raise TailError("samples export has no data rows")
    return {
        "run": run,
        "rows": rows,
        "denominators": {
            "sample_rows_total": total,
            "non_two_way_rows_excluded": noncommon,
            "common_two_way_rows": common,
            "common_two_way_rows_missing_oa_cross_excluded": missing_cross,
            "common_two_way_finite_oa_cross_scored": len(rows),
            "procedure_report_grid_samples_removed_all_systems": run.excluded_grid_samples,
        },
    }


def _heading_bin(value: float | None) -> str:
    if value is None:
        return "missing"
    magnitude = abs(value)
    if magnitude < 1.0:
        return "<1 deg"
    if magnitude < 5.0:
        return "1 to <5 deg"
    return ">=5 deg"


def _gap_bin(seconds: float | None) -> str:
    if seconds is None:
        return "first sample"
    if seconds <= 10.0:
        return "<=10 s"
    if seconds <= 60.0:
        return ">10 to 60 s"
    return ">60 s"


def _flag(value: bool) -> str:
    return "yes" if value else "no"


def summarize(loaded: dict[str, object]) -> dict[str, object]:
    rows = loaded["rows"]
    if not isinstance(rows, list):
        raise TailError("validated sample rows are unavailable")
    if not rows:
        raise TailError("no finite common two-way OpenATCA cross-track rows to score")
    by_addr: dict[str, list[Sample]] = defaultdict(list)
    for row in rows:
        by_addr[row.addr].append(row)
    if len(by_addr) > MAX_ADDRS:
        raise TailError(f"address group count exceeds limit {MAX_ADDRS}")
    for track_rows in by_addr.values():
        track_rows.sort(key=lambda row: (row.t_ms, row.phase, row.band, row.cross_m))
        previous: int | None = None
        for row in track_rows:
            row.gap_s = None if previous is None else (row.t_ms - previous) / 1000.0
            previous = row.t_ms

    all_errors = [abs(row.cross_m) for row in rows]
    p95 = nearest_rank(all_errors, 0.95)
    if p95 is None:
        raise TailError("p95 is unavailable for an empty score set")
    tail_count = sum(error >= p95 for error in all_errors)
    dimensions = {
        "phase": lambda row: row.phase,
        "altitude_band": lambda row: row.band,
        "coast": lambda row: _flag(row.coast),
        "uncertain": lambda row: _flag(row.uncertain),
        "reacquired": lambda row: _flag(row.reacquired),
        "absolute_heading_error": lambda row: _heading_bin(row.heading_err_deg),
        "preceding_scored_sample_interval": lambda row: _gap_bin(row.gap_s),
    }
    cohorts: dict[str, list[dict[str, object]]] = {}
    for name, classify in dimensions.items():
        groups: dict[str, list[float]] = defaultdict(list)
        for row in rows:
            groups[classify(row)].append(abs(row.cross_m))
        results = []
        for label in sorted(groups):
            errors = groups[label]
            count_tail = sum(error >= p95 for error in errors)
            results.append({
                "cohort": label,
                "n": len(errors),
                "median_abs_cross_m": nearest_rank(errors, 0.50),
                "p95_abs_cross_m": nearest_rank(errors, 0.95),
                "overall_p95_tail_n": count_tail,
                "share_of_overall_tail_pct": round(100.0 * count_tail / tail_count, 2),
            })
        if len(results) > MAX_COHORTS_PER_DIMENSION:
            raise TailError("cohort count exceeds fixed output bound")
        cohorts[name] = results

    joint: dict[tuple[str, str, str], list[float]] = defaultdict(list)
    for row in rows:
        key = (_heading_bin(row.heading_err_deg), _gap_bin(row.gap_s), _flag(row.reacquired))
        joint[key].append(abs(row.cross_m))
    if len(joint) > MAX_JOINT_COHORTS:
        raise TailError("joint cohort count exceeds fixed output bound")
    cohorts["heading_interval_reacquisition_joint"] = [
        {
            "cohort": f"heading={h}; interval={gap}; reacquired={reacquired}",
            "n": len(errors),
            "median_abs_cross_m": nearest_rank(errors, 0.50),
            "p95_abs_cross_m": nearest_rank(errors, 0.95),
            "overall_p95_tail_n": sum(error >= p95 for error in errors),
            "share_of_overall_tail_pct": round(100.0 * sum(error >= p95 for error in errors) / tail_count, 2),
        }
        for (h, gap, reacquired), errors in sorted(joint.items())
    ]
    return {
        "schema_version": 1,
        "denominators": loaded["denominators"],
        "overall": {
            "median_abs_cross_m": nearest_rank(all_errors, 0.50),
            "p95_abs_cross_m": p95,
            "overall_p95_tail_n_using_greater_or_equal_cutoff": tail_count,
        },
        "cohorts": cohorts,
        "definitions": {
            "score_set": "common_two_way=1 and finite oa_cross_m",
            "error": "absolute oa_cross_m in metres",
            "quantile": "nearest rank",
            "tail": "rows with absolute cross-track error >= overall nearest-rank p95 cutoff",
            "heading_error": "absolute oa_track_err_deg; descriptive disagreement proxy, not a turn-rate classifier",
            "interval": "difference from preceding scored sample for the same address; not an ADS-B report gap",
            "procedure_excluded_samples": "reported separately from rows in the sample export",
            "privacy": "aggregate output only; no addresses, timestamps, paths, or run labels are emitted",
        },
    }


def _write_output(path: Path, rendered: str) -> None:
    """Refuse sealed destinations and verify the opened output descriptor before writing."""
    payload = rendered.encode("utf-8")
    if len(payload) > MAX_OUTPUT_BYTES:
        raise TailError(f"output exceeds byte limit {MAX_OUTPUT_BYTES}")
    sealed_guard.refuse(path)
    flags = os.O_WRONLY | os.O_CREAT
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    if hasattr(os, "O_NONBLOCK"):
        flags |= os.O_NONBLOCK
    fd = os.open(path, flags, 0o600)
    try:
        sealed_guard.production().verify_fd(fd, path)
        if not stat.S_ISREG(os.fstat(fd).st_mode):
            raise TailError("output must be a regular file")
        os.ftruncate(fd, 0)
        with os.fdopen(fd, "wb", closefd=False) as stream:
            stream.write(payload)
            stream.flush()
    finally:
        os.close(fd)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--samples", required=True, type=Path, help="paired P-002 development samples.csv")
    parser.add_argument("--report", required=True, type=Path, help="sibling P-002 development p002.txt")
    parser.add_argument("--out", required=True, type=Path, help="write aggregate JSON to this guarded path")
    args = parser.parse_args(argv)
    try:
        sealed_guard.refuse(args.samples, args.report, args.out)
        loaded = load_samples(args.samples, args.report)
        rendered = json.dumps(summarize(loaded), indent=2, sort_keys=True) + "\n"
        _write_output(args.out, rendered)
    except (OSError, UnicodeError, csv.Error, TailError, sealed_guard.SealedError, ValueError) as exc:
        print(f"tracker_tail: {exc}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
