#!/usr/bin/env python3
"""Aggregate same-pair CA/encounter episode coincidences without exporting addresses."""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import math
import os
import re
import stat
import sys
from collections import defaultdict
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable

TOOL_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(TOOL_DIR))
import sealed_guard  # noqa: E402

PROFILE = Path(__file__).with_name("a3_rate_profile_v2.json")
DIAGNOSTIC_PROFILE = Path(__file__).with_name("a3_rate_diagnostic_profile_v1.json")
PROFILE_ID = "A3-ZAU-2026-11-19-literal-coincidence-v2"
DIAGNOSTIC_PROFILE_ID = "A3-ZAU-2026-11-19-count-only-diagnostic-v1"
WARMUP_SECONDS = 120
MAX_EPISODES_PER_INPUT = 200_000
MAX_DIAGNOSTIC_ROWS_PER_INPUT = 1_000_000
MAX_PAIR_COMPARISONS = 10_000_000
MAX_CSV_BYTES = 512 * 1024 * 1024
DAY_START = 1_795_046_400  # 2026-11-19T00:00:00Z
DAY_END = 1_795_129_200  # 2026-11-19T23:00:00Z (exclusive)
HOUR = 3600
ADDRESS = re.compile(r"^[0-9A-Fa-f]{8}$")
SHA256 = re.compile(r"\bsha256 ([0-9a-f]{64})\b")
ENCOUNTER_CLASSES = frozenset(
    {"C1", "C2", "C3", "C4", "C5", "C6", "VOLUME_ONLY", "ALT_UNKNOWN"}
)


class InputError(ValueError):
    pass


@dataclass(frozen=True)
class Episode:
    pair: tuple[str, str]
    start: float
    end: float
    class_name: str
    ticks_unevaluated: int
    flags: frozenset[str] = frozenset()


def canonical_pair(a: str, b: str) -> tuple[str, str]:
    if not ADDRESS.fullmatch(a) or not ADDRESS.fullmatch(b) or a.upper() == b.upper():
        raise InputError("invalid aircraft pair identity in episode input")
    return tuple(sorted((a.upper(), b.upper())))


def _required(row: dict[str, str], key: str, source: str) -> str:
    value = row.get(key)
    if value is None or value == "":
        raise InputError(f"{source}: missing {key}")
    return value


def read_episodes(path: Path, kind: str) -> list[Episode]:
    try:
        sealed_guard.refuse(path)
        handle = sealed_guard.production().open_regular_read(path, "r", encoding="utf-8", newline="")
    except OSError as exc:
        raise InputError(f"cannot open {kind} episode CSV: {exc}") from exc
    with handle:
        if os.fstat(handle.fileno()).st_size > MAX_CSV_BYTES:
            raise InputError(f"{kind}: CSV exceeds fixed {MAX_CSV_BYTES}-byte ceiling")
        reader = csv.DictReader(handle)
        required = (
            {"address_a_hex", "address_b_hex", "start_s", "end_s", "ticks_unevaluated", "flags"}
            if kind == "alerts"
            else {"address_a_hex", "address_b_hex", "start_s", "end_s", "ticks_unevaluated", "class"}
        )
        if reader.fieldnames is None or not required.issubset(reader.fieldnames):
            raise InputError(f"{kind}: unexpected CSV schema")
        result = []
        for line, row in enumerate(reader, 2):
            if len(result) >= MAX_EPISODES_PER_INPUT:
                raise InputError(f"{kind}: fixed episode ceiling {MAX_EPISODES_PER_INPUT} exceeded")
            try:
                start = float(_required(row, "start_s", kind))
                end = float(_required(row, "end_s", kind))
                if not (math.isfinite(start) and math.isfinite(end) and start <= end):
                    raise ValueError("non-finite or reversed interval")
                if interval_intersects_sealed_window(start, end):
                    raise sealed_guard.SealedError(
                        f"{kind}:{line}: episode interval intersects a guarded hold-out window"
                    )
                pair = canonical_pair(
                    _required(row, "address_a_hex", kind),
                    _required(row, "address_b_hex", kind),
                )
                unevaluated = int(_required(row, "ticks_unevaluated", kind))
                if unevaluated < 0:
                    raise ValueError("negative unevaluated count")
                if kind == "encounters":
                    class_name = _required(row, "class", kind)
                    if class_name not in ENCOUNTER_CLASSES:
                        raise ValueError(f"unsupported encounter class {class_name!r}")
                else:
                    class_name = "ALERT"
                flags = frozenset(filter(None, row.get("flags", "").split("|")))
                result.append(Episode(pair, start, end, class_name, unevaluated, flags))
            except (ValueError, InputError) as exc:
                raise InputError(f"{kind}:{line}: {exc}") from exc
        return result


def read_summary(path: Path, kind: str) -> str:
    try:
        sealed_guard.refuse(path)
        with sealed_guard.production().open_regular_read(path, "r", encoding="utf-8") as handle:
            content = handle.read(1_048_577)
        if len(content) > 1_048_576:
            raise InputError(f"{kind} summary exceeds fixed 1 MiB ceiling")
    except OSError as exc:
        raise InputError(f"cannot open {kind} summary: {exc}") from exc
    return content


def summary_state(path: Path, kind: str) -> tuple[bool, str]:
    content = read_summary(path, kind)
    match = SHA256.search(content)
    if not match:
        raise InputError(f"{kind} summary has no clip SHA-256")
    expected = "net state: complete (no shortfall)" if kind == "CA" else "record state: complete (no shortfall)"
    return expected in content, match.group(1)


def validate_summary_thresholds(path: Path, kind: str) -> None:
    with sealed_guard.production().open_regular_read(path, "r", encoding="utf-8") as handle:
        content = handle.read(1_048_577)
    if len(content) > 1_048_576:
        raise InputError(f"{kind} summary exceeds fixed 1 MiB ceiling")
    if kind == "CA":
        found = re.findall(r"(?:default volume|volume \d+):[^\n]*horizontal ([0-9.]+) NM, vertical ([0-9.]+) ft", content)
        if not found or any(abs(float(h) - 5.0) > 0.005 or abs(float(v) - 1000.0) > 0.1 for h, v in found):
            raise InputError("CA summary thresholds do not match the frozen 5 NM / 1000 ft profile")
    else:
        match = re.search(r"record volume: horizontal < [0-9.]+ m \(([0-9.]+) NM\), vertical < ([0-9.]+) ft", content)
        if match is None or abs(float(match.group(1)) - 5.0) > 0.005 or abs(float(match.group(2)) - 1000.0) > 0.1:
            raise InputError("encounter summary thresholds do not match the frozen 5 NM / 1000 ft profile")


def overlaps(a: Episode, b: Episode) -> bool:
    return max(a.start, b.start) <= min(a.end, b.end)


def interval_intersects_sealed_window(start_s: float, end_s: float) -> bool:
    guard = sealed_guard.production()
    if guard.broken or not guard.windows:
        return True
    start_ns = round(start_s * 1_000_000_000)
    end_ns = round(end_s * 1_000_000_000)
    return any(
        start_ns < window_end and end_ns >= window_start
        for _, window_start, window_end in guard.windows
    )


def hour_index(onset: float) -> int | None:
    if onset < DAY_START or onset >= DAY_END:
        return None
    return int((onset - DAY_START) // HOUR)


def aggregate(
    alerts: Iterable[Episode],
    encounters: Iterable[Episode],
    complete: bool,
    coverage_start: float,
    coverage_end: float,
) -> dict:
    alerts = list(alerts)
    encounters = list(encounters)
    for encounter in encounters:
        if encounter.class_name not in ENCOUNTER_CLASSES:
            raise InputError(
                f"unsupported encounter class {encounter.class_name!r}; "
                "expected C1-C6, VOLUME_ONLY, or ALT_UNKNOWN"
            )
    # `complete` covers only global run shortfalls. Clip-edge coverage is classified per event.
    usable_start = coverage_start + WARMUP_SECONDS
    by_pair: dict[tuple[str, str], list[Episode]] = defaultdict(list)
    for e in encounters:
        by_pair[e.pair].append(e)
    alerts_by_pair: dict[tuple[str, str], list[Episode]] = defaultdict(list)
    for a in alerts:
        alerts_by_pair[a.pair].append(a)
    comparisons = 0

    def overlapping(left: Episode, right: Episode) -> bool:
        nonlocal comparisons
        comparisons += 1
        if comparisons > MAX_PAIR_COMPARISONS:
            raise InputError(f"fixed pair-comparison budget {MAX_PAIR_COMPARISONS} exceeded")
        return overlaps(left, right)
    hours = [
        {
            "hour_utc": f"2026-11-19T{hour:02d}:00:00Z",
            "encounter_known": 0,
            "encounter_unknown": 0,
            "encounter_hit": 0,
            "encounter_miss": 0,
            "encounter_unresolved": 0,
            "alert_matched": 0,
            "alert_nuisance": 0,
            "alert_unresolved": 0,
            "encounter_right_censored": 0,
            "alert_right_censored": 0,
            "covered_seconds": 0,
        }
        for hour in range(23)
    ]
    selected_truth = [e for e in encounters if hour_index(e.start) is not None]
    selected_alerts = [a for a in alerts if hour_index(a.start) is not None]
    for hour_num, h in enumerate(hours):
        bin_start = DAY_START + hour_num * HOUR
        bin_end = bin_start + HOUR
        h["covered_seconds"] = max(
            0.0,
            min(float(bin_end), coverage_end) - max(float(bin_start), usable_start),
        )
    for e in selected_truth:
        if e.end >= coverage_end:
            hours[hour_index(e.start)]["encounter_right_censored"] += 1  # type: ignore[index]
    for a in selected_alerts:
        if a.end >= coverage_end:
            hours[hour_index(a.start)]["alert_right_censored"] += 1  # type: ignore[index]
    for e in selected_truth:
        h = hours[hour_index(e.start)]  # type: ignore[index]
        if e.class_name == "ALT_UNKNOWN":
            h["encounter_unknown"] += 1
            h["encounter_unresolved"] += 1
            continue
        h["encounter_known"] += 1
        alert_candidates = [
            a for a in alerts_by_pair.get(e.pair, ()) if overlapping(a, e)
        ]
        has_match = any(a.ticks_unevaluated == 0 for a in alert_candidates)
        alert_incomplete = any(a.ticks_unevaluated > 0 for a in alert_candidates)
        locally_covered = e.start >= usable_start and e.end < coverage_end
        if not complete or not locally_covered or e.ticks_unevaluated > 0 or alert_incomplete:
            h["encounter_unresolved"] += 1
        elif has_match:
            h["encounter_hit"] += 1
        else:
            h["encounter_miss"] += 1

    for a in selected_alerts:
        h = hours[hour_index(a.start)]  # type: ignore[index]
        candidates = [e for e in by_pair.get(a.pair, ()) if overlapping(a, e)]
        known_match = any(
            e.class_name != "ALT_UNKNOWN" and e.ticks_unevaluated == 0
            for e in candidates
        )
        unevaluated_match = any(e.ticks_unevaluated > 0 for e in candidates)
        unknown_match = any(e.class_name == "ALT_UNKNOWN" for e in candidates)
        locally_covered = a.start >= usable_start and a.end < coverage_end
        if not complete or not locally_covered or a.ticks_unevaluated > 0:
            h["alert_unresolved"] += 1
        elif known_match:
            h["alert_matched"] += 1
        elif not unevaluated_match and not unknown_match and "alt_unknown" not in a.flags:
            h["alert_nuisance"] += 1
        else:
            h["alert_unresolved"] += 1

    for hour in hours:
        resolved = hour["encounter_hit"] + hour["encounter_miss"]
        hour["encounter_hit_fraction_of_resolved"] = (
            hour["encounter_hit"] / resolved if resolved else None
        )
        # Every row in this table is one full evaluation hour.
        covered_hours = hour["covered_seconds"] / HOUR
        hour["nuisance_alerts_per_covered_hour"] = (
            hour["alert_nuisance"] / covered_hours if complete and covered_hours > 0 else None
        )
    totals = {
        k: sum(hour[k] for hour in hours)
        for k in hours[0]
        if k not in {"hour_utc", "encounter_hit_fraction_of_resolved", "nuisance_alerts_per_covered_hour"}
    }
    resolved_total = totals["encounter_hit"] + totals["encounter_miss"]
    totals["encounter_hit_fraction_of_resolved"] = (
        totals["encounter_hit"] / resolved_total if resolved_total else None
    )
    total_covered_hours = totals["covered_seconds"] / HOUR
    totals["nuisance_alerts_per_covered_hour"] = (
        totals["alert_nuisance"] / total_covered_hours
        if complete and total_covered_hours > 0
        else None
    )
    return {
        "profile": PROFILE_ID,
        "interval_start_utc": "2026-11-19T00:00:00Z",
        "interval_end_utc_exclusive": "2026-11-19T23:00:00Z",
        "run_summaries_complete": complete,
        "all_23_hour_bins_fully_covered": all(h["covered_seconds"] == HOUR for h in hours),
        "coverage_completeness_scope": "local to event onset/end; global run summaries must also be complete",
        "clip_coverage_start_unix_s": coverage_start,
        "clip_coverage_end_unix_s": coverage_end,
        "warmup_seconds_before_evaluation": max(0.0, DAY_START - coverage_start),
        "hours": hours,
        "totals": totals,
        "claim_limit": "literal ADS-B-derived encounter-volume coincidence proxy only; not independent truth, FAA/human truth, or facility false-alert qualification",
    }


def count_only_diagnostic(
    alerts_path: Path,
    ca_summary: str,
    encounters_path: Path,
    encounter_summary: str,
    ca_complete: bool,
    encounter_complete: bool,
    clip_sha256: str,
    coverage_start: float,
    coverage_end: float,
) -> dict:
    """Address-free hourly count diagnostic; deliberately performs no alert/truth matching."""
    alert_counts, alert_censored, alert_total, alert_outside = count_episode_hours(alerts_path, "alerts", coverage_end)
    encounter_counts, encounter_censored, encounter_total, encounter_outside = count_episode_hours(
        encounters_path, "encounters", coverage_end
    )
    usable_start = coverage_start + WARMUP_SECONDS
    hours = []
    for i in range(23):
        start = DAY_START + i * HOUR
        end = start + HOUR
        covered = max(0.0, min(float(end), coverage_end) - max(float(start), usable_start))
        alerts_n = alert_counts[i]
        encounters_n = encounter_counts[i]
        hours.append({
            "hour_utc": f"2026-11-19T{i:02d}:00:00Z",
            "covered_seconds": covered,
            "alert_episode_rows": alerts_n,
            "encounter_episode_rows": encounters_n,
            "unresolved_alert_episode_rows": alerts_n,
            "unresolved_encounter_episode_rows": encounters_n,
            "alert_right_censored_rows": alert_censored[i],
            "encounter_right_censored_rows": encounter_censored[i],
            "classification": "NOT_PERFORMED_COUNT_ONLY",
        })
    return {
        "schema": "openatca-a3-count-only-diagnostic-v1",
        "profile": DIAGNOSTIC_PROFILE_ID,
        "clip_sha256": clip_sha256,
        "interval_start_utc": "2026-11-19T00:00:00Z",
        "interval_end_utc_exclusive": "2026-11-19T23:00:00Z",
        "warmup_seconds": WARMUP_SECONDS,
        "first_usable_clip_time_unix_s": coverage_start,
        "clip_coverage_end_unix_s": coverage_end,
        "ca_run_state": "COMPLETE" if ca_complete else "DEGRADED_OR_SHORTFALL",
        "encounter_run_state": "COMPLETE" if encounter_complete else "DEGRADED_OR_SHORTFALL",
        "global_classification_status": "UNRESOLVED_COUNT_ONLY_NO_MATCHING_PERFORMED",
        "unresolved_reason": "count-only diagnostic does not classify matches, misses, or nuisance alerts; global CA shortfall is preserved",
        "ca_shortfall_counters": summary_counter_map(ca_summary, "shortfalls and counts"),
        "ca_state_cycle_counts": summary_counter_map(ca_summary, "net state after each cycle"),
        "encounter_shortfall_counters": summary_counter_map(encounter_summary, "work and overflow"),
        "input_row_totals": {"alerts": alert_total, "encounters": encounter_total},
        "input_rows_outside_evaluation_window": {"alerts": alert_outside, "encounters": encounter_outside},
        "unresolved_episode_row_totals": {"alerts": alert_total - alert_outside, "encounters": encounter_total - encounter_outside},
        "all_23_hour_bins_fully_covered": all(h["covered_seconds"] == HOUR for h in hours),
        "hours": hours,
        "claim_limit": "aggregate episode-row counts only; no pair matching, hit/miss/nuisance classification, rates, or independent truth claim",
    }


def summary_counter_map(content: str, marker: str) -> dict[str, int]:
    start = content.find(marker)
    if start < 0:
        return {"summary_section_missing": 1}
    section = content[start + len(marker):]
    counters = {}
    for line in section.splitlines():
        match = re.fullmatch(r"\s+(.+): ([0-9]+)", line)
        if match:
            counters[match.group(1).strip()] = int(match.group(2))
        elif counters:
            break
    return counters


def validate_coverage(start: float, end: float) -> None:
    if not math.isfinite(start) or not math.isfinite(end):
        raise InputError("clip coverage timestamps must be finite")
    if start >= end:
        raise InputError("clip coverage interval is empty or reversed")


def count_episode_hours(path: Path, kind: str, coverage_end: float) -> tuple[list[int], list[int], int, int]:
    """Stream episode start/end columns only; row addresses are never copied into output state."""
    sealed_guard.refuse(path)
    try:
        handle = sealed_guard.production().open_regular_read(path, "r", encoding="utf-8", newline="")
    except OSError as exc:
        raise InputError(f"cannot open {kind} episode CSV: {exc}") from exc
    with handle:
        if os.fstat(handle.fileno()).st_size > MAX_CSV_BYTES:
            raise InputError(f"{kind}: CSV exceeds fixed {MAX_CSV_BYTES}-byte ceiling")
        reader = csv.DictReader(handle)
        required = {"address_a_hex", "address_b_hex", "start_s", "end_s"}
        if reader.fieldnames is None or not required.issubset(reader.fieldnames):
            raise InputError(f"{kind}: unexpected episode CSV schema")
        counts = [0] * 23
        censored = [0] * 23
        total = 0
        outside = 0
        for line, row in enumerate(reader, 2):
            total += 1
            if total > MAX_DIAGNOSTIC_ROWS_PER_INPUT:
                raise InputError(f"{kind}: fixed diagnostic row ceiling {MAX_DIAGNOSTIC_ROWS_PER_INPUT} exceeded")
            try:
                start = float(_required(row, "start_s", kind))
                end = float(_required(row, "end_s", kind))
                if not (math.isfinite(start) and math.isfinite(end) and start <= end):
                    raise ValueError("non-finite or reversed interval")
                if interval_intersects_sealed_window(start, end):
                    raise sealed_guard.SealedError(
                        f"{kind}:{line}: episode interval intersects a guarded hold-out window"
                    )
                index = hour_index(start)
                if index is None:
                    outside += 1
                    continue
                counts[index] += 1
                if end >= coverage_end:
                    censored[index] += 1
            except ValueError as exc:
                raise InputError(f"{kind}:{line}: {exc}") from exc
        return counts, censored, total, outside


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--alerts", type=Path, required=True)
    parser.add_argument("--ca-summary", type=Path, required=True)
    parser.add_argument("--encounters", type=Path, required=True)
    parser.add_argument("--encounter-summary", type=Path, required=True)
    parser.add_argument("--coverage-start-s", type=float, required=True, help="verified first usable clip time, Unix seconds")
    parser.add_argument("--coverage-end-s", type=float, required=True, help="verified clip coverage end, Unix seconds")
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument(
        "--counts-only-diagnostic", action="store_true",
        help="emit address-free episode counts only; performs no matching or rate classification",
    )
    args = parser.parse_args(argv)
    try:
        args.alerts = args.alerts.expanduser().absolute()
        args.ca_summary = args.ca_summary.expanduser().absolute()
        args.encounters = args.encounters.expanduser().absolute()
        args.encounter_summary = args.encounter_summary.expanduser().absolute()
        args.out = args.out.expanduser().absolute()
        profile_path = DIAGNOSTIC_PROFILE if args.counts_only_diagnostic else PROFILE
        for path in (args.alerts, args.ca_summary, args.encounters, args.encounter_summary, profile_path, args.out):
            sealed_guard.refuse(path)
        for path in (args.alerts, args.ca_summary, args.encounters, args.encounter_summary):
            _preflight_private_input(path)
        _preflight_private_output(args.out)
        ca_summary = read_summary(args.ca_summary, "CA")
        enc_summary = read_summary(args.encounter_summary, "encounter")
        ca_match = SHA256.search(ca_summary)
        enc_match = SHA256.search(enc_summary)
        if ca_match is None or enc_match is None:
            raise InputError("run summaries must each identify a clip SHA-256")
        ca_clip, enc_clip = ca_match.group(1), enc_match.group(1)
        ca_complete = "net state: complete (no shortfall)" in ca_summary
        enc_complete = "record state: complete (no shortfall)" in enc_summary
        validate_summary_thresholds(args.ca_summary, "CA")
        validate_summary_thresholds(args.encounter_summary, "encounter")
        if ca_clip != enc_clip:
            raise InputError("CA and encounter summaries do not identify the same clip digest")
        validate_coverage(args.coverage_start_s, args.coverage_end_s)
        with sealed_guard.production().open_regular_read(profile_path, "rb") as profile_handle:
            profile_bytes = profile_handle.read(65_537)
        if len(profile_bytes) > 65_536:
            raise InputError("frozen aggregation profile exceeds fixed 64 KiB ceiling")
        if args.counts_only_diagnostic:
            result = count_only_diagnostic(
                args.alerts, ca_summary, args.encounters, enc_summary,
                ca_complete, enc_complete, ca_clip,
                args.coverage_start_s, args.coverage_end_s,
            )
        else:
            result = aggregate(
                read_episodes(args.alerts, "alerts"),
                read_episodes(args.encounters, "encounters"),
                ca_complete and enc_complete,
                args.coverage_start_s,
                args.coverage_end_s,
            )
            result["clip_sha256"] = ca_clip
        result["profile_sha256"] = hashlib.sha256(profile_bytes).hexdigest()
        if not args.counts_only_diagnostic:
            result["ca_complete"] = ca_complete
            result["encounter_complete"] = enc_complete
        _write_private_output(args.out, result)
        return 0
    except (InputError, sealed_guard.SealedError, OSError, ValueError, UnicodeError, csv.Error) as exc:
        print(f"a3-rate-join refused: {exc}", file=sys.stderr)
        return 2


def _assert_directory_path_is_fd(path: Path, fd: int) -> None:
    sealed_guard.refuse(path)
    expected = os.fstat(fd)
    actual = os.stat(path, follow_symlinks=True)
    if (
        not stat.S_ISDIR(expected.st_mode)
        or not stat.S_ISDIR(actual.st_mode)
        or expected.st_dev != actual.st_dev
        or expected.st_ino != actual.st_ino
    ):
        raise InputError("private output directory changed during creation")


def _preflight_private_input(path: Path) -> None:
    resolved = path.resolve(strict=True)
    sealed_guard.refuse(path, resolved)
    if "openatca-private" not in resolved.parts:
        raise InputError("rate-join inputs must be inside openatca-private")
    file_stat = resolved.stat()
    parent_stat = resolved.parent.stat()
    if (
        not stat.S_ISREG(file_stat.st_mode)
        or file_stat.st_uid != os.getuid()
        or file_stat.st_nlink != 1
        or parent_stat.st_uid != os.getuid()
        or stat.S_IMODE(parent_stat.st_mode) & 0o077
    ):
        raise InputError("rate-join inputs must be owned regular single-link files in private directories")


def _preflight_private_output(path: Path) -> tuple[Path, Path, str]:
    guard = sealed_guard.production()
    if not hasattr(os, "O_NOFOLLOW") or not hasattr(os, "O_DIRECTORY"):
        raise InputError("private aggregate output requires no-follow directory support")
    given = path.expanduser().absolute()
    guard.check(given)
    if given.exists() or given.is_symlink():
        raise InputError("aggregate output directory must be new")
    parent = given.parent.resolve(strict=True)
    child_name = given.name
    if not child_name or child_name in {".", ".."} or "openatca-private" not in parent.parts:
        raise InputError("output must be a new child directory under openatca-private")
    output = parent / child_name
    guard.check(output)
    guard.check(parent)
    parent_stat = parent.stat()
    if parent_stat.st_uid != os.getuid() or stat.S_IMODE(parent_stat.st_mode) & 0o077:
        raise InputError("output parent must be owned by this user and private (no group/other permissions)")
    return output, parent, child_name


def _write_private_output(path: Path, result: dict) -> None:
    guard = sealed_guard.production()
    output, parent, child_name = _preflight_private_output(path)
    parent_fd = os.open(parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | getattr(os, "O_CLOEXEC", 0))
    try:
        guard.verify_fd(parent_fd, parent)
        parent_stat = os.fstat(parent_fd)
        if not stat.S_ISDIR(parent_stat.st_mode) or parent_stat.st_uid != os.getuid() or stat.S_IMODE(parent_stat.st_mode) & 0o077:
            raise InputError("output parent must be owned by this user and private (no group/other permissions)")
        _assert_directory_path_is_fd(parent, parent_fd)
        os.mkdir(child_name, 0o700, dir_fd=parent_fd)
        child_fd = os.open(child_name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | getattr(os, "O_CLOEXEC", 0), dir_fd=parent_fd)
        try:
            guard.verify_fd(child_fd, output)
            os.fchmod(child_fd, 0o700)
            _assert_directory_path_is_fd(output, child_fd)
            payloads = {
                "hourly.json": (json.dumps(result, indent=2, sort_keys=True) + "\n").encode("utf-8"),
                "hourly.csv": _hourly_csv(result["hours"]).encode("utf-8"),
            }
            for name, payload in payloads.items():
                destination = output / name
                guard.check(destination)
                flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | getattr(os, "O_CLOEXEC", 0)
                fd = os.open(name, flags, 0o600, dir_fd=child_fd)
                try:
                    guard.verify_fd(fd, destination)
                    if not stat.S_ISREG(os.fstat(fd).st_mode):
                        raise InputError("aggregate output must be a regular file")
                    os.fchmod(fd, 0o600)
                    with os.fdopen(fd, "wb", closefd=False) as handle:
                        handle.write(payload)
                        handle.flush()
                        os.fsync(fd)
                    guard.verify_fd(fd, destination)
                finally:
                    os.close(fd)
            _assert_directory_path_is_fd(output, child_fd)
            _assert_directory_path_is_fd(parent, parent_fd)
            os.fsync(child_fd)
            os.fsync(parent_fd)
        finally:
            os.close(child_fd)
    finally:
        os.close(parent_fd)


def _hourly_csv(hours: list[dict]) -> str:
    from io import StringIO

    handle = StringIO(newline="")
    writer = csv.DictWriter(handle, fieldnames=list(hours[0]))
    writer.writeheader()
    writer.writerows(hours)
    return handle.getvalue()


if __name__ == "__main__":
    raise SystemExit(main())
