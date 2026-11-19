"""Pinned, fail-closed runner for named Kani 0.68 proof harnesses.

Kani proves a bounded model of one named harness. This report is not a product
compiler result, an unbounded theorem, or an assessment that assumptions/stubs
are appropriate. Review the harness and its assumptions separately. Runs are
sequential, offline, single-job, memory/time limited and isolated under a
SHA-256 namespace of the resolved checkout path. RUSTFLAGS is absent unless a
registered cfg is requested. Cfgs, stub mode/summary, toolchain identity,
source snapshot, command, and raw JSON result digest are recorded per harness.

In Kani 0.68, `unsupported_construct` checks are emitted as failing assertions
when the unsupported operation is reachable. A Success status discharges that
guard, so the evaluator preserves and reports such successful guard counts;
failure or any other status fails evaluation. Requested `kani::cover!` labels
are checked from cover-category results and their Satisfied statuses. Kani's
separate code-coverage instrumentation flag is recorded but does not stand in
for a requested cover.
"""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import re
import resource
import signal
import shutil
import subprocess
import sys
import time
from typing import Any, Iterable, Mapping


PINNED_KANI_VERSION = "0.68.0"
PINNED_CARGO_TOOLCHAIN = "1.89.0"
# Closed CBMC loop-bound presets. Loop labels are tied to the pinned Kani/CBMC
# build and the source location below; never accept arbitrary CBMC arguments.
LOOP_UNWIND_PRESETS = {
    "late-replay-two-events": {
        "package": "oa-track",
        "harnesses": frozenset({
            'tracker::proofs::late_replay_restores_transaction_after_current_row_then_later_refusal',
            'tracker::proofs::rollback_paths::late_replay_restores_transaction_after_overflow_before_refusal',
            'tracker::proofs::rollback_paths::late_replay_restores_transaction_after_append_before_refusal',
            'tracker::proofs::rollback_paths::late_replay_overflow_reachability',
            'tracker::proofs::rollback_paths::late_replay_append_reachability',
            'tracker::proofs::rollback_paths::late_overflow_current_filter_projection',
            'tracker::proofs::rollback_paths::late_overflow_anchor_filter_projection',
            'tracker::proofs::rollback_paths::late_overflow_ring_projection',
            'tracker::proofs::rollback_paths::late_overflow_metadata_projection',
            'tracker::proofs::rollback_paths::late_overflow_stats_projection',
            'tracker::proofs::rollback_paths::late_append_current_filter_projection',
            'tracker::proofs::rollback_paths::late_append_anchor_filter_projection',
            'tracker::proofs::rollback_paths::late_append_ring_projection',
            'tracker::proofs::rollback_paths::late_append_metadata_projection',
            'tracker::proofs::rollback_paths::late_append_stats_projection',
            'tracker::proofs::rollback_paths::late_overflow_output_projection',
            'tracker::proofs::rollback_paths::late_append_output_projection',
        }),
        "loop_id": "_RNvNtCs1K5hzcfmox7_8oa_track7tracker11update_late.3",
        "bound": 3,
        "global_unwind": 16,
        "harness_source": {
            'tracker::proofs::late_replay_restores_transaction_after_current_row_then_later_refusal': "crates/oa-track/src/tracker_proofs.rs",
        },
        "default_harness_source": "crates/oa-track/src/tracker_rollback_paths.rs",
        "source": "crates/oa-track/src/tracker.rs",
        "line": 1191,
        "function": "update_late",
        "source_line": "while k < n_seq {",
    },
}
_late_base_preset = LOOP_UNWIND_PRESETS["late-replay-two-events"]
LOOP_UNWIND_PRESETS["late-replay-two-events-all-loops"] = {
    **_late_base_preset,
    "loops": [
        {"loop_id": "_RNvNtCs1K5hzcfmox7_8oa_track7tracker11update_late.0", "bound": 3, "line": 1104, "guard_line": 1104, "source_line": "for k in 0..len {", "log_mode": "completed_before_bound", "iterations": [1, 2]},
        {"loop_id": "_RNvNtCs1K5hzcfmox7_8oa_track7tracker11update_late.1", "bound": 2, "line": 1123, "guard_line": 1123, "source_line": "for k in (e + 1)..len {"},
        {"loop_id": "_RNvNtCs1K5hzcfmox7_8oa_track7tracker11update_late.2", "bound": 2, "line": 1161, "source_line": "for k in 0..=e {"},
        {"loop_id": "_RNvNtCs1K5hzcfmox7_8oa_track7tracker11update_late.3", "bound": 3, "line": 1191, "source_line": "while k < n_seq {"},
    ],
    "fixture_source": "crates/oa-track/src/tracker_proofs.rs",
    "fixture_snippets": [
        "const LATE_BASE_NS: i64 = PROOF_TIME - 20;",
        "const LATE_INSERT_NS: i64 = PROOF_TIME - 10;",
        "const LATE_LATER_NS: i64 = PROOF_TIME;",
        "toa_ns: LATE_BASE_NS,",
        "sensor: 1,",
        "toa_ns: LATE_INSERT_NS,",
        "sensor: 3,",
        "toa_ns: LATE_LATER_NS,",
        "sensor: 2,",
        "t.ring_len[PROOF_SLOT] = 2;",
        "t.ring_head[PROOF_SLOT] = 0;",
        "t.ring_m[PROOF_SLOT * RING_DEPTH] = base;",
        "t.ring_m[PROOF_SLOT * RING_DEPTH + 1] = later;",
        "let mut t = late_proof_tracker();",
        "let m = late_proof_measurement();",
        "update_late(&mut t, PROOF_SLOT, &m, &mut out);",
    ],
}
_late_pinned_prefix = "_RNvNtCseRq8xMCbGsa_8oa_track7tracker11update_late."
_late_local_prefix = "_RNvNtCs1K5hzcfmox7_8oa_track7tracker11update_late."
_late_id_families = {
    "local": [_late_local_prefix + str(index) for index in range(4)],
    "ci": [_late_pinned_prefix + str(index) for index in range(4)],
}
LOOP_UNWIND_PRESETS["late-replay-two-events-pinned-cargo"] = {
    **LOOP_UNWIND_PRESETS["late-replay-two-events-all-loops"],
    "loop_id": _late_pinned_prefix + "3",
    "loops": [
        {
            **loop,
            "loop_ids": [
                _late_local_prefix + loop["loop_id"].rsplit(".", 1)[1],
                _late_pinned_prefix + loop["loop_id"].rsplit(".", 1)[1],
            ],
        }
        for loop in LOOP_UNWIND_PRESETS["late-replay-two-events-all-loops"]["loops"]
    ],
    "log_families": _late_id_families,
    "cargo_toolchain": PINNED_CARGO_TOOLCHAIN,
}
LOOP_UNWIND_PRESETS["late-replay-two-events-pinned-cargo-one-loop"] = {
    **LOOP_UNWIND_PRESETS["late-replay-two-events"],
    "loop_id": _late_pinned_prefix + "3",
    "loop_ids": [_late_local_prefix + "3", _late_pinned_prefix + "3"],
    "log_families": {
        name: [ids[3]] for name, ids in _late_id_families.items()
    },
    "cargo_toolchain": PINNED_CARGO_TOOLCHAIN,
}
DEFAULT_TIMEOUT_SECONDS = 360
DEFAULT_MEMORY_MIB = 6144
ALLOWED_CFGS = {"kani_full_capacity", "kani_late_replay_covers"}
PACKAGE_RE = re.compile(r"^[A-Za-z0-9_-]+$")
HARNESS_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)*$")
COVER_STATUS = {"Satisfied", "Unsatisfiable"}
CHECK_STATUSES = {"Success", "Unreachable", "Satisfied", "Unsatisfiable"}
PROPERTY_COUNTERS = {
    "total_properties",
    "passed",
    "failed",
    "unreachable",
    "undetermined",
    "solver_error",
    "satisfied",
    "unsatisfiable",
    "covered",
    "uncovered",
}
PROPERTY_CHECK_STATUS = {
    "Success": "passed",
    "Failure": "failed",
    "Unreachable": "unreachable",
    "Undetermined": "undetermined",
    "SolverError": "solver_error",
    "Solver Error": "solver_error",
    "Satisfied": "satisfied",
    "Unsatisfiable": "unsatisfiable",
}


def _sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _canonical_sha256(value: Any) -> str:
    body = json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
    return _sha256_bytes(body.encode("utf-8"))


def source_mapping_sha256(mapping: Mapping[str, str]) -> str:
    return _canonical_sha256(dict(sorted(mapping.items())))


def _loop_unwind_configuration(preset_name: str | None) -> dict[str, Any] | None:
    if preset_name is None:
        return None
    preset = LOOP_UNWIND_PRESETS.get(preset_name)
    if preset is None:
        raise ValueError(f"unknown loop-unwind preset: {preset_name}")
    return {
        key: sorted(value) if isinstance(value, frozenset) else value
        for key, value in preset.items()
    }


def _loop_unwind_argument(preset: Mapping[str, Any]) -> str:
    loops = preset.get("loops", [preset])
    return ",".join(
        f"{loop_id}:{loop['bound']}"
        for loop in loops
        for loop_id in loop.get("loop_ids", [loop["loop_id"]])
    )


def _expected_loop_log_line_count(preset: Mapping[str, Any]) -> int:
    if not preset.get("log_families"):
        return sum(
            len(loop.get("iterations", [])) if loop.get("log_mode") == "completed_before_bound" else 1
            for loop in preset.get("loops", [preset])
        )
    return sum(
        len(loop.get("iterations", [])) if loop.get("log_mode") == "completed_before_bound" else loop["bound"]
        for loop in preset.get("loops", [preset])
    )


def _validate_loop_unwind_source(repo: Path, preset_name: str, package: str, harness: str) -> None:
    preset = LOOP_UNWIND_PRESETS[preset_name]
    if package != preset["package"] or harness not in preset["harnesses"]:
        raise ValueError(f"loop-unwind preset {preset_name!r} is not allowed for {package}:{harness}")
    path = repo / preset["source"]
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError as exc:
        raise ValueError(f"cannot read source for loop-unwind preset {preset_name!r}: {exc}") from exc
    loops = preset.get("loops", [preset])
    for loop in loops:
        line_index = loop.get("guard_line", loop["line"]) - 1
        if line_index >= len(lines) or lines[line_index].strip() != loop["source_line"]:
            raise ValueError(f"stale source location for loop-unwind preset {preset_name!r}")
        enclosing_fn = next(
            (line for line in reversed(lines[:line_index]) if re.search(r"\bfn\s+[A-Za-z_]\w*\s*\(", line)),
            "",
        )
        if not re.search(rf"\bfn\s+{re.escape(preset['function'])}\s*\(", enclosing_fn):
            raise ValueError(f"stale function location for loop-unwind preset {preset_name!r}")
    if "fixture_source" in preset:
        try:
            fixture_text = (repo / preset["fixture_source"]).read_text(encoding="utf-8")
        except OSError as exc:
            raise ValueError(f"cannot read fixture source for loop-unwind preset {preset_name!r}: {exc}") from exc
        if any(snippet not in fixture_text for snippet in preset["fixture_snippets"]):
            raise ValueError(f"stale bounded fixture for loop-unwind preset {preset_name!r}")
    harness_source = preset.get("harness_source", {}).get(harness, preset.get("default_harness_source"))
    if not isinstance(harness_source, str):
        raise ValueError(f"missing source map for loop-unwind preset {preset_name!r}")
    proof_path = repo / harness_source
    try:
        proof_lines = proof_path.read_text(encoding="utf-8").splitlines()
    except OSError as exc:
        raise ValueError(f"cannot read harness source for loop-unwind preset {preset_name!r}: {exc}") from exc
    fn_name = harness.rsplit("::", 1)[-1]
    fn_indexes = [i for i, line in enumerate(proof_lines) if re.search(rf"\bfn\s+{re.escape(fn_name)}\s*\(", line)]
    if len(fn_indexes) != 1:
        raise ValueError(f"expected one source definition for harness {harness!r}")
    fn_index = fn_indexes[0]
    previous_fn = max((i for i, line in enumerate(proof_lines[:fn_index]) if re.search(r"\bfn\s+[A-Za-z_]\w*\s*\(", line)), default=-1)
    attrs = [line.strip() for line in proof_lines[previous_fn + 1:fn_index] if line.strip().startswith("#[")]
    unwind_attrs = [attr for attr in attrs if attr.startswith("#[kani::unwind(")]
    if unwind_attrs != [f"#[kani::unwind({preset['global_unwind']})]"]:
        raise ValueError(
            f"harness {harness!r} must have exactly #[kani::unwind({preset['global_unwind']})]"
        )


def _check_loop_unwind_log(preset_name: str, stdout: str, stderr: str) -> dict[str, Any]:
    preset = LOOP_UNWIND_PRESETS[preset_name]
    families = preset.get("log_families")
    if families:
        return _check_aliased_loop_unwind_log(preset, stdout, stderr)
    source = r"[/\\]".join(re.escape(part) for part in preset["source"].split("/"))
    expected_loops = preset.get("loops", [preset])
    all_lines = (stdout + "\n" + stderr).splitlines()
    matched = []
    for loop in expected_loops:
        source_and_function = (
            rf"file (?:.*[/\\])?{source} line {loop['line']} column \d+ "
            rf"function (?:[A-Za-z_]\w*::)*{re.escape(preset['function'])} thread \d+\s*$"
        )
        if loop.get("log_mode") == "completed_before_bound":
            expected = re.compile(
                rf"^Unwinding loop {re.escape(loop['loop_id'])} iteration (\d+) {source_and_function}"
            )
            observed = []
            for candidate in all_lines:
                match = expected.search(candidate)
                if match:
                    observed.append((int(match.group(1)), candidate))
            unexpected = any(
                re.search(rf"^(?:Not )?Unwinding loop {re.escape(loop['loop_id'])} iteration (\d+)\b", candidate)
                and not expected.search(candidate)
                for candidate in all_lines
            )
            if [iteration for iteration, _ in observed] != loop["iterations"] or unexpected:
                return {
                    "passed": False,
                    "matched_line": None,
                    "matched_lines": matched + [line for _, line in observed],
                    "expected": (
                        f"completed loop {loop['loop_id']} iterations {loop['iterations']} "
                        f"before bound {loop['bound']} at {preset['source']} line {loop['line']}"
                    ),
                }
            matched.extend(line for _, line in observed)
            continue
        expected = re.compile(
            rf"^Not unwinding loop {re.escape(loop['loop_id'])} iteration {loop['bound']} {source_and_function}"
        )
        line = next((candidate for candidate in all_lines if expected.search(candidate)), None)
        if line is None:
            return {
                "passed": False,
                "matched_line": None,
                "matched_lines": matched,
                "expected": (
                    f"Not unwinding loop {loop['loop_id']} iteration {loop['bound']} "
                    f"file */{preset['source']} line {loop['line']} column <n> "
                    f"function [namespace::]*{preset['function']} thread <n>"
                ),
            }
        matched.append(line)
    if len(expected_loops) == 1:
        return {"passed": True, "matched_line": matched[0]}
    return {"passed": True, "matched_line": None, "matched_lines": matched}


def _check_aliased_loop_unwind_log(
    preset: Mapping[str, Any], stdout: str, stderr: str
) -> dict[str, Any]:
    """Accept one complete, unmixed family of explicitly registered loop IDs."""
    families = preset["log_families"]
    loops = preset.get("loops", [preset])
    all_lines = (stdout + "\n" + stderr).splitlines()
    source = r"[/\\]".join(re.escape(part) for part in preset["source"].split("/"))
    all_known_ids = {loop_id for ids in families.values() for loop_id in ids}
    requested_indexes = {loop_id.rsplit(".", 1)[-1] for loop_id in all_known_ids}
    unknown_ids = set()
    observed_ids = set()
    for candidate in all_lines:
        match = re.search(r"(?:Not unwinding|Unwinding) loop (\S+) iteration \d+", candidate)
        if match and "update_late." in match.group(1):
            loop_id = match.group(1)
            loop_index = loop_id.rsplit(".", 1)[-1]
            if loop_index not in requested_indexes:
                continue
            observed_ids.add(loop_id)
            if loop_id not in all_known_ids:
                unknown_ids.add(loop_id)
    if unknown_ids:
        return {
            "passed": False,
            "matched_line": None,
            "matched_lines": [],
            "expected": f"only registered loop IDs; unknown IDs observed: {sorted(unknown_ids)}",
        }

    for family_name, family_ids in families.items():
        if not isinstance(family_ids, list) or len(family_ids) != len(loops):
            return {"passed": False, "matched_line": None, "matched_lines": [], "expected": "valid registered loop-ID family"}
        if observed_ids != set(family_ids):
            continue
        matched = []
        valid = True
        for loop, loop_id in zip(loops, family_ids):
            source_and_function = (
                rf"file (?:.*[/\\])?{source} line {loop['line']} column \d+ "
                rf"function (?:[A-Za-z_]\w*::)*{re.escape(preset['function'])} thread \d+\s*$"
            )
            candidates = [
                line for line in all_lines
                if re.search(rf"^(?:Not unwinding|Unwinding) loop {re.escape(loop_id)} iteration \d+\b", line)
            ]
            if loop.get("log_mode") == "completed_before_bound":
                expected = re.compile(
                    rf"^Unwinding loop {re.escape(loop_id)} iteration (\d+) {source_and_function}"
                )
                observed = [(int(match.group(1)), line) for line in candidates if (match := expected.search(line))]
                if [iteration for iteration, _ in observed] != loop["iterations"] or len(observed) != len(candidates):
                    valid = False
                    break
                matched.extend(line for _, line in observed)
            else:
                expected_prefix = re.compile(
                    rf"^Unwinding loop {re.escape(loop_id)} iteration (\d+) {source_and_function}"
                )
                prelude = [
                    (int(match.group(1)), line)
                    for line in candidates
                    if (match := expected_prefix.search(line))
                ]
                expected_iterations = list(range(1, loop["bound"]))
                expected = re.compile(
                    rf"^Not unwinding loop {re.escape(loop_id)} iteration {loop['bound']} {source_and_function}"
                )
                not_lines = [line for line in candidates if line.startswith("Not unwinding loop ")]
                if (
                    [iteration for iteration, _ in prelude] != expected_iterations
                    or len(not_lines) != 1
                    or not expected.search(not_lines[0])
                    or len(prelude) + len(not_lines) != len(candidates)
                ):
                    valid = False
                    break
                matched.extend(line for _, line in prelude)
                matched.append(not_lines[0])
        if valid:
            if len(loops) == 1 and len(matched) == 1:
                return {"passed": True, "matched_line": matched[0], "matched_family": family_name}
            return {
                "passed": True,
                "matched_line": None,
                "matched_lines": matched,
                "matched_family": family_name,
            }
    return {
        "passed": False,
        "matched_line": None,
        "matched_lines": [],
        "observed_loop_ids": sorted(observed_ids),
        "expected_families": {name: ids for name, ids in families.items()},
    }


def _is_nonnegative_int(value: Any) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and value >= 0


def _reject_duplicate_keys(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def _report_object(report: str | bytes | Mapping[str, Any]) -> tuple[dict[str, Any] | None, bytes, list[str]]:
    errors: list[str] = []
    if isinstance(report, Mapping):
        raw = json.dumps(report, sort_keys=True, separators=(",", ":")).encode("utf-8")
        return dict(report), raw, errors
    if isinstance(report, str):
        raw = report.encode("utf-8")
    elif isinstance(report, bytes):
        raw = report
    else:
        return None, b"", ["result must be JSON text, bytes, or an object"]
    try:
        parsed = json.loads(raw, object_pairs_hook=_reject_duplicate_keys)
    except (UnicodeDecodeError, json.JSONDecodeError, ValueError) as exc:
        return None, raw, [f"invalid or partial Kani JSON: {exc}"]
    if not isinstance(parsed, dict):
        return None, raw, ["Kani JSON root must be an object"]
    return parsed, raw, errors


def _ids_exact(items: Any, key: str, expected: set[str], label: str, errors: list[str]) -> None:
    if not isinstance(items, list):
        errors.append(f"{label} must be a list")
        return
    seen: list[str] = []
    for item in items:
        if not isinstance(item, dict) or not isinstance(item.get(key), str):
            errors.append(f"{label} has an entry without string {key}")
            continue
        seen.append(item[key])
    if len(seen) != len(set(seen)):
        errors.append(f"{label} contains duplicate harness IDs")
    if set(seen) != expected:
        errors.append(f"{label} harness set differs: expected {sorted(expected)}, got {sorted(set(seen))}")
    if len(seen) != len(expected):
        errors.append(f"{label} count does not equal the expected nonzero harness count")


def _positive_coordinate(value: Any) -> str | None:
    if isinstance(value, int) and not isinstance(value, bool) and value > 0:
        return str(value)
    if isinstance(value, str) and re.fullmatch(r"[1-9][0-9]*", value):
        return value
    return None


def _cover_groups(
    result: Mapping[str, Any], harness: str, errors: list[str]
) -> dict[str, dict[tuple[str, str, str, str], list[str]]]:
    checks = result.get("checks")
    if not isinstance(checks, list):
        errors.append(f"{harness}: checks must be a list to verify cover sites")
        return {}
    covers: dict[str, dict[tuple[str, str, str, str], list[str]]] = {}
    seen_ids: set[int] = set()
    for check in checks:
        if not isinstance(check, dict):
            continue
        if check.get("category") != "cover":
            continue
        desc = check.get("description")
        status = check.get("status")
        check_id = check.get("id")
        if not _is_nonnegative_int(check_id):
            errors.append(f"{harness}: cover check is missing a nonnegative integer id")
        elif check_id in seen_ids:
            errors.append(f"{harness}: cover check id {check_id} is duplicated")
        else:
            seen_ids.add(check_id)
        function = check.get("function")
        location = check.get("location")
        if not isinstance(desc, str) or not desc.strip():
            errors.append(f"{harness}: cover check description is missing")
            continue
        if not isinstance(function, str) or not function:
            errors.append(f"{harness}: cover {desc!r} is missing its function identity")
            continue
        if not isinstance(location, dict):
            errors.append(f"{harness}: cover {desc!r} is missing its source location")
            continue
        source_file = location.get("file")
        line = _positive_coordinate(location.get("line"))
        column = _positive_coordinate(location.get("column"))
        if (
            not isinstance(source_file, str)
            or not source_file
            or "\\" in source_file
            or Path(source_file).is_absolute()
            or ".." in Path(source_file).parts
            or line is None
            or column is None
        ):
            errors.append(f"{harness}: cover {desc!r} has an invalid source location")
            continue
        site = (function, source_file, line, column)
        covers.setdefault(desc, {}).setdefault(site, []).append(status if isinstance(status, str) else "")
    for label, sites in covers.items():
        if len(sites) > 1:
            errors.append(f"{harness}: cover label {label!r} appears at multiple source sites")
        for site, statuses in sites.items():
            if "Satisfied" not in statuses:
                errors.append(f"{harness}: cover {label!r} at {site[1]}:{site[2]}:{site[3]} was never satisfied")
            if any(status not in COVER_STATUS for status in statuses):
                errors.append(f"{harness}: cover {label!r} has an unknown or unsuccessful result: {statuses}")
    return covers


def _check_cover_labels(
    covers: Mapping[str, Mapping[tuple[str, str, str, str], list[str]]],
    expected_labels: set[str],
    harness: str,
    errors: list[str],
) -> None:
    for label in sorted(expected_labels):
        sites = covers.get(label, {})
        if not sites:
            errors.append(f"{harness}: requested cover is missing: {label!r}")
        elif len(sites) != 1:
            errors.append(f"{harness}: requested cover {label!r} does not identify one source site")
        elif not any("Satisfied" in statuses for statuses in sites.values()):
            errors.append(f"{harness}: requested cover {label!r} was not satisfied")


def summarize_kani_report(report: str | bytes | Mapping[str, Any]) -> dict[str, Any]:
    """Return transparent status/category counts, including discharged unsupported guards."""
    obj, _, parse_errors = _report_object(report)
    if obj is None:
        return {"parse_errors": parse_errors}
    verification = obj.get("verification_results")
    if not isinstance(verification, dict) or not isinstance(verification.get("results"), list):
        return {"parse_errors": ["verification_results.results must be a list"]}
    status_counts: dict[str, int] = {}
    category_counts: dict[str, int] = {}
    unsupported_guards: list[dict[str, Any]] = []
    for result in verification["results"]:
        if not isinstance(result, dict):
            continue
        checks = result.get("checks", [])
        if not isinstance(checks, list):
            continue
        for check in checks:
            if not isinstance(check, dict):
                continue
            status = check.get("status")
            category = check.get("category")
            if isinstance(status, str):
                status_counts[status] = status_counts.get(status, 0) + 1
            if isinstance(category, str):
                category_counts[category] = category_counts.get(category, 0) + 1
                if category == "unsupported_construct":
                    unsupported_guards.append({
                        "status": status,
                        "description": check.get("description"),
                        "location": check.get("location"),
                    })
    return {
        "check_status_counts": dict(sorted(status_counts.items())),
        "check_category_counts": dict(sorted(category_counts.items())),
        "unsupported_construct_checks": unsupported_guards,
        "code_coverage_instrumentation": obj.get("coverage", {}).get("enabled")
        if isinstance(obj.get("coverage"), dict)
        else None,
    }


def evaluate_kani_report(
    report: str | bytes | Mapping[str, Any],
    expected_harnesses: Iterable[str],
    required_covers: Mapping[str, Iterable[str]] | None = None,
) -> list[str]:
    """Fail closed unless the pinned Kani JSON reports exactly the requested proofs/covers."""
    errors: list[str] = []
    obj, _, parse_errors = _report_object(report)
    errors.extend(parse_errors)
    try:
        expected_list = list(expected_harnesses)
    except TypeError:
        return errors + ["expected harness set must be an iterable of names"]
    if any(not isinstance(name, str) for name in expected_list):
        return errors + ["expected harness IDs must all be strings"]
    expected = set(expected_list)
    if not expected or len(expected_list) != len(expected):
        errors.append("expected harness set must be nonempty and unique")
    for name in expected:
        if not isinstance(name, str) or not HARNESS_RE.fullmatch(name):
            errors.append(f"invalid expected harness name: {name!r}")
    if obj is None:
        return errors

    metadata = obj.get("metadata")
    tools = obj.get("tools")
    if not isinstance(metadata, dict) or metadata.get("version") != "1.0":
        errors.append("metadata.version must be the pinned 1.0 JSON schema")
    if not isinstance(metadata, dict) or metadata.get("kani_version") != PINNED_KANI_VERSION:
        errors.append(f"metadata.kani_version must be {PINNED_KANI_VERSION}")
    if not isinstance(tools, dict) or tools.get("kani") != PINNED_KANI_VERSION:
        errors.append(f"tools.kani must be {PINNED_KANI_VERSION}")
    for field in ("rustc", "cbmc", "goto_cc", "goto_instrument"):
        if not isinstance(tools, dict) or not isinstance(tools.get(field), str) or not tools[field]:
            errors.append(f"tools.{field} must identify the Kani verification toolchain")
    if not isinstance(tools, dict) or not isinstance(tools.get("solvers"), list) or not tools["solvers"]:
        errors.append("tools.solvers must identify at least one solver")
    elif any(
        not isinstance(solver, dict)
        or not isinstance(solver.get("name"), str)
        or not solver.get("name")
        for solver in tools["solvers"]
    ):
        errors.append("tools.solvers contains an entry without a solver name")

    _ids_exact(obj.get("harness_metadata"), "pretty_name", expected, "harness_metadata", errors)
    _ids_exact(obj.get("error_details"), "harness_id", expected, "error_details", errors)
    _ids_exact(obj.get("property_details"), "harness_id", expected, "property_details", errors)
    cbmc = obj.get("cbmc")
    _ids_exact(cbmc, "harness_id", expected, "cbmc", errors)

    verification = obj.get("verification_results")
    if not isinstance(verification, dict):
        errors.append("verification_results must be an object")
        return errors
    summary = verification.get("summary")
    if not isinstance(summary, dict):
        errors.append("verification_results.summary must be an object")
        return errors
    count = len(expected)
    for field, want in (("total_harnesses", count), ("executed", count), ("successful", count), ("failed", 0)):
        value = summary.get(field)
        if not _is_nonnegative_int(value) or value != want:
            errors.append(f"summary.{field} must be {want}, got {value!r}")
    if summary.get("status") != "completed":
        errors.append(f"summary.status must be 'completed', got {summary.get('status')!r}")
    if not _is_nonnegative_int(summary.get("duration_ms")):
        errors.append("summary.duration_ms must be a nonnegative integer")

    results = verification.get("results")
    _ids_exact(results, "harness_id", expected, "verification results", errors)
    if required_covers is None:
        required_covers = {}
    if not isinstance(required_covers, Mapping):
        errors.append("required_covers must map harness IDs to label lists")
        required_covers = {}
    if set(required_covers) - expected:
        errors.append("required_covers contains a harness outside the expected set")

    result_by_id = {
        r["harness_id"]: r
        for r in results
        if isinstance(results, list) and isinstance(r, dict) and isinstance(r.get("harness_id"), str)
    } if isinstance(results, list) else {}
    cover_groups_by_harness: dict[str, dict[str, dict[tuple[str, str, str, str], list[str]]]] = {}
    check_counts_by_harness: dict[str, dict[str, int]] = {}
    for harness in sorted(expected):
        result = result_by_id.get(harness)
        if not isinstance(result, dict):
            continue
        if result.get("status") != "Success":
            errors.append(f"{harness}: result status is not Success: {result.get('status')!r}")
        if not _is_nonnegative_int(result.get("duration_ms")):
            errors.append(f"{harness}: duration_ms must be a nonnegative integer")
        checks = result.get("checks")
        if not isinstance(checks, list) or not checks:
            errors.append(f"{harness}: checks must be a nonempty list")
            continue
        cover_groups = _cover_groups(result, harness, errors)
        cover_groups_by_harness[harness] = cover_groups
        check_counts = {field: 0 for field in (
            "passed", "failed", "unreachable", "undetermined", "solver_error", "satisfied", "unsatisfiable"
        )}
        check_counts_by_harness[harness] = check_counts
        seen_check_ids: set[int] = set()
        for check in checks:
            if not isinstance(check, dict):
                errors.append(f"{harness}: malformed check result")
                continue
            check_id = check.get("id")
            if not _is_nonnegative_int(check_id):
                errors.append(f"{harness}: check id must be a nonnegative integer")
            elif check_id in seen_check_ids:
                errors.append(f"{harness}: check id {check_id} is duplicated")
            else:
                seen_check_ids.add(check_id)
            status = check.get("status")
            category = check.get("category")
            counter = PROPERTY_CHECK_STATUS.get(status) if isinstance(status, str) else None
            if counter is not None:
                check_counts[counter] += 1
            if category == "unsupported_construct" and status != "Success":
                errors.append(f"{harness}: unsupported construct was not proved unreachable")
            if isinstance(category, str) and "unwind" in category.lower() and status != "Success":
                errors.append(f"{harness}: unwinding check did not succeed")
            if not isinstance(category, str) or not category:
                errors.append(f"{harness}: check category is missing or unknown")
            if not isinstance(status, str) or status not in CHECK_STATUSES:
                errors.append(f"{harness}: unknown or unsuccessful check status {status!r}")
                continue
            if status == "Unsatisfiable" and category != "cover":
                errors.append(f"{harness}: Unsatisfiable status is only valid for a cover clone")
            if status == "Satisfied" and category != "cover":
                errors.append(f"{harness}: Satisfied status is only valid for a cover check")
            if category == "cover" and status not in COVER_STATUS:
                errors.append(f"{harness}: cover check is not satisfied: {check.get('description')!r}")
        labels = required_covers.get(harness, ())
        if isinstance(labels, str):
            errors.append(f"{harness}: required cover labels must be a collection, not a string")
            labels = ()
        try:
            label_list = list(labels)
        except TypeError:
            errors.append(f"{harness}: required cover labels are malformed")
            label_list = []
        if any(not isinstance(x, str) or not x for x in label_list):
            errors.append(f"{harness}: requested cover labels must be nonempty and unique")
            label_list = [x for x in label_list if isinstance(x, str) and x]
        elif len(label_list) != len(set(label_list)):
            errors.append(f"{harness}: requested cover labels must be nonempty and unique")
        _check_cover_labels(cover_groups, set(x for x in label_list if isinstance(x, str)), harness, errors)

    details = obj.get("error_details")
    if isinstance(details, list):
        for item in details:
            if isinstance(item, dict) and item.get("has_errors") is not False:
                errors.append(f"{item.get('harness_id')}: Kani error_details reports errors")
    properties = obj.get("property_details")
    if isinstance(properties, list):
        for item in properties:
            if not isinstance(item, dict) or not isinstance(item.get("property_details"), dict):
                errors.append("property_details entry is malformed")
                continue
            p = item["property_details"]
            if not PROPERTY_COUNTERS.issubset(p):
                errors.append(f"{item.get('harness_id')}: property counters are incomplete")
                continue
            if any(not _is_nonnegative_int(p[k]) for k in PROPERTY_COUNTERS):
                errors.append(f"{item.get('harness_id')}: property counters must be nonnegative integers")
                continue
            harness_id = item.get("harness_id")
            observed_counts = check_counts_by_harness.get(harness_id)
            if observed_counts is None:
                errors.append(f"{harness_id}: property counters have no corresponding check list")
            else:
                for field, observed in observed_counts.items():
                    if p[field] != observed:
                        errors.append(
                            f"{harness_id}: property counter {field} is {p[field]}, but check list reports {observed}"
                        )
            if p["total_properties"] == 0:
                errors.append(f"{item.get('harness_id')}: no proof properties were checked")
            if (p["passed"] + p["failed"] + p["unreachable"] + p["undetermined"]
                    + p["solver_error"] + p["satisfied"] + p["unsatisfiable"] != p["total_properties"]):
                errors.append(f"{item.get('harness_id')}: property totals do not reconcile")
            for field in ("failed", "undetermined", "solver_error", "uncovered"):
                if p[field] != 0:
                    errors.append(f"{item.get('harness_id')}: property counter {field} is {p[field]}")
            if p["unsatisfiable"]:
                groups = cover_groups_by_harness.get(item.get("harness_id"), {})
                reachable = bool(groups) and all(
                    bool(sites)
                    and all("Satisfied" in statuses and all(status in COVER_STATUS for status in statuses)
                            for statuses in sites.values())
                    for sites in groups.values()
                )
                if not reachable:
                    errors.append(
                        f"{item.get('harness_id')}: unsatisfiable property count is only allowed for cloned cover checks when every cover group has a satisfied source site"
                    )
            requested = required_covers.get(item.get("harness_id"), ())
            if isinstance(requested, (list, tuple, set)) and p["satisfied"] < len(requested):
                errors.append(f"{item.get('harness_id')}: satisfied-cover counter is below requested cover count")

    harness_metadata = obj.get("harness_metadata")
    if isinstance(harness_metadata, list):
        for item in harness_metadata:
            if not isinstance(item, dict):
                continue
            attrs = item.get("attributes")
            if not isinstance(attrs, dict) or attrs.get("kind") != "Proof" or attrs.get("should_panic") is not False:
                errors.append(f"{item.get('pretty_name')}: expected a non-panicking proof harness")
            if not isinstance(item.get("source"), dict) or not isinstance(item["source"].get("file"), str):
                errors.append(f"{item.get('pretty_name')}: source location is missing")
    coverage = obj.get("coverage")
    if not isinstance(coverage, dict) or not isinstance(coverage.get("enabled"), bool):
        errors.append("coverage.enabled must describe code-coverage instrumentation separately")

    if isinstance(cbmc, list):
        for item in cbmc:
            if not isinstance(item, dict):
                continue
            conf = item.get("configuration")
            cm = item.get("cbmc_metadata")
            if not isinstance(conf, dict) or not isinstance(conf.get("solver"), str) or not conf["solver"]:
                errors.append(f"{item.get('harness_id')}: CBMC solver is missing")
            if not isinstance(cm, dict) or not isinstance(cm.get("version"), str) or not cm["version"]:
                errors.append(f"{item.get('harness_id')}: CBMC version is missing")
    return sorted(set(errors))


def evaluate_kani_binding(report_text: str | bytes, record: Mapping[str, Any]) -> list[str]:
    """Verify hashes and source immutability binding one JSON result to one invocation."""
    errors: list[str] = []
    if not isinstance(record, Mapping):
        return ["run record must be an object"]
    raw = report_text.encode("utf-8") if isinstance(report_text, str) else report_text
    if not isinstance(raw, bytes):
        return ["result JSON must be text or bytes"]
    report_obj, _, parse_errors = _report_object(raw)
    errors.extend(parse_errors)
    if report_obj is None:
        errors.append("result JSON cannot be bound to the invocation")
    if record.get("result_sha256") != _sha256_bytes(raw):
        errors.append("result_sha256 does not bind the supplied JSON bytes")
    command = record.get("command")
    if not isinstance(command, list) or not command or any(not isinstance(x, str) for x in command):
        errors.append("command must be a nonempty argument vector")
    elif record.get("command_sha256") != _canonical_sha256(command):
        errors.append("command_sha256 does not bind the recorded argument vector")
    preset_name = record.get("loop_unwind_preset")
    scoped = record.get("loop_unwind_configuration")
    cargo_version = record.get("cargo_version")
    if cargo_version is not None:
        if not isinstance(cargo_version, str) or not re.match(r"^cargo 1\.89\.0(?:\s|$)", cargo_version):
            errors.append("recorded Cargo version is not 1.89.0")
        if not isinstance(command, list) or command[:2] != ["cargo", "+1.89.0"]:
            errors.append("command does not invoke Cargo through the pinned 1.89.0 toolchain")
    if preset_name is None:
        if scoped is not None or (
            isinstance(command, list)
            and any(arg in command for arg in ("--cbmc-args", "--unwindset", "--no-unwinding-assertions"))
        ):
            errors.append("scoped loop-unwind arguments require a registered preset")
    else:
        try:
            expected_scoped = _loop_unwind_configuration(preset_name)
        except ValueError:
            expected_scoped = None
            errors.append("loop_unwind_preset is unknown")
        if expected_scoped is not None:
            if scoped != expected_scoped:
                errors.append("loop_unwind_configuration does not match its registered preset")
            if expected_scoped.get("cargo_toolchain") == PINNED_CARGO_TOOLCHAIN and (
                cargo_version is None or not isinstance(command, list) or command[:2] != ["cargo", "+1.89.0"]
            ):
                errors.append("pinned-Cargo unwind preset requires Cargo 1.89.0 invocation and version binding")
            if record.get("package") != expected_scoped["package"] or record.get("harness") not in expected_scoped["harnesses"]:
                errors.append("loop-unwind preset is not applicable to the recorded proof")
            expected_tail = [
                "--cbmc-args", "--unwindset",
                _loop_unwind_argument(expected_scoped),
                "--unwinding-assertions",
            ]
            if not isinstance(command, list) or command[-len(expected_tail):] != expected_tail:
                errors.append("command does not contain the exact registered loop-unwind arguments")
            log_check = record.get("loop_unwind_log_check")
            if not isinstance(log_check, dict) or log_check.get("passed") is not True:
                errors.append("registered loop-unwind log check did not pass")
            else:
                if (
                    len(expected_scoped.get("loops", [expected_scoped])) == 1
                    and _expected_loop_log_line_count(expected_scoped) == 1
                ):
                    matched_lines = [log_check.get("matched_line")]
                else:
                    matched_lines = log_check.get("matched_lines")
                rechecked_log = (
                    _check_loop_unwind_log(preset_name, "\n".join(matched_lines), "")
                    if isinstance(matched_lines, list) and all(isinstance(line, str) for line in matched_lines)
                    else {"passed": False}
                )
                if (
                    not isinstance(matched_lines, list)
                    or len(matched_lines) != _expected_loop_log_line_count(expected_scoped)
                    or any(not isinstance(line, str) for line in matched_lines)
                    or rechecked_log.get("passed") is not True
                    or log_check.get("matched_family") != rechecked_log.get("matched_family")
                ):
                    errors.append("recorded loop-unwind log line does not match the preset")
    source_before = record.get("source_before")
    source_after = record.get("source_after")
    valid_source_maps = (
        isinstance(source_before, dict)
        and isinstance(source_after, dict)
        and bool(source_before)
        and all(
            isinstance(k, str) and isinstance(v, str) and re.fullmatch(r"[0-9a-f]{64}", v)
            and not k.startswith("/") and "\\" not in k and ".." not in k.split("/") and "" not in k.split("/")
            for mapping in (source_before, source_after)
            for k, v in mapping.items()
        )
    )
    if not valid_source_maps:
        errors.append("source snapshots must be nonempty path-to-SHA256 maps")
    else:
        for label, mapping in (("source_before", source_before), ("source_after", source_after)):
            digest_key = label.replace("source_", "source_sha256_")
            if record.get(digest_key) != source_mapping_sha256(mapping):
                errors.append(f"{digest_key} does not bind its source snapshot")
    if source_before != source_after:
        errors.append("source snapshots differ")
    if record.get("source_unchanged") is not True:
        errors.append("source_unchanged is not true")
    environment = record.get("environment")
    if not isinstance(environment, dict) or record.get("environment_sha256") != _canonical_sha256(environment):
        errors.append("environment_sha256 does not bind the recorded command environment")
    command = record.get("command")
    if isinstance(command, list):
        harness = record.get("harness")
        package = record.get("package")
        if not isinstance(harness, str) or not isinstance(package, str):
            errors.append("recorded package and harness names are missing")
        else:
            if "--harness" not in command or command[command.index("--harness") + 1:command.index("--harness") + 2] != [harness]:
                errors.append("command does not bind the recorded harness")
            if "-p" not in command or command[command.index("-p") + 1:command.index("-p") + 2] != [package]:
                errors.append("command does not bind the recorded package")
    if record.get("exit_code") != 0 or isinstance(record.get("exit_code"), bool):
        errors.append("command exit_code is not zero")
    if record.get("kani_version") != PINNED_KANI_VERSION:
        errors.append(f"recorded Kani version is not {PINNED_KANI_VERSION}")
    if isinstance(report_obj, dict):
        package = record.get("package")
        repo = record.get("repo")
        harness = record.get("harness")
        project = report_obj.get("project")
        crate_names = project.get("crate_name") if isinstance(project, dict) else None
        expected_crate = package.replace("-", "_") if isinstance(package, str) else None
        if not isinstance(crate_names, list) or crate_names != [expected_crate]:
            errors.append("JSON project.crate_name does not match the recorded Cargo package")
        workspace_root = project.get("workspace_root") if isinstance(project, dict) else None
        if not isinstance(repo, str) or not Path(repo).is_absolute() or not isinstance(workspace_root, str):
            errors.append("JSON project workspace root or recorded repository is missing")
        elif Path(workspace_root).resolve() != Path(repo).resolve():
            errors.append("JSON project.workspace_root does not match the recorded repository")
        metadata = report_obj.get("harness_metadata")
        if not isinstance(metadata, list) or len(metadata) != 1:
            errors.append("JSON harness_metadata must contain the one recorded harness")
        else:
            item = metadata[0]
            if not isinstance(item, dict) or item.get("pretty_name") != harness:
                errors.append("JSON pretty_name does not match the recorded harness")
            elif item.get("crate_name") != expected_crate:
                errors.append("JSON harness crate_name does not match the recorded Cargo package")
    return sorted(set(errors))


def _source_paths(repo: Path, excluded: Iterable[Path]) -> list[Path]:
    tracked = subprocess.check_output(["git", "ls-files", "-z"], cwd=repo)
    untracked = subprocess.check_output(
        ["git", "ls-files", "--others", "--exclude-standard", "-z"], cwd=repo
    )
    excluded_real = [p.resolve() for p in excluded]
    names = sorted(set(x for x in (tracked + untracked).decode().split("\0") if x))
    paths: list[Path] = []
    for name in names:
        path = repo / name
        resolved = path.resolve()
        if any(resolved == root or root in resolved.parents for root in excluded_real):
            continue
        if path.is_symlink():
            raise RuntimeError(f"refusing symlink in source snapshot: {name}")
        if not path.exists():
            raise RuntimeError(f"tracked source path is missing: {name}")
        if path.is_dir():  # e.g. a git submodule entry; hash only ordinary source files.
            continue
        if not path.is_file() or not resolved.is_relative_to(repo):
            raise RuntimeError(f"source path is not a regular in-repository file: {name}")
        paths.append(path)
    return paths


def _source_snapshot(repo: Path, excluded: Iterable[Path]) -> dict[str, str]:
    return {
        path.relative_to(repo).as_posix(): _sha256_file(path)
        for path in _source_paths(repo, excluded)
    }


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def _git(repo: Path, *args: str) -> str:
    return subprocess.check_output(["git", *args], cwd=repo, text=True).strip()


_ARCHIVE_SOURCE_SUFFIXES = {
    ".rs", ".toml", ".lock", ".py", ".md", ".yml", ".yaml", ".json",
    ".cfg", ".sh", ".txt",
}
_ARCHIVE_EXCLUDED_PARTS = {
    ".git", "target", "build-cache", "fixtures", "fixture", "seeds", "fuzz",
    "recording", "recordings", "__pycache__",
}


def changed_source_paths(repo: Path) -> tuple[list[str], list[str]]:
    """Return changed tracked and untracked source paths, preserving repo-relative names."""
    tracked = subprocess.check_output(
        ["git", "diff", "--name-only", "--no-renames", "HEAD", "--"], cwd=repo
    ).decode().splitlines()
    untracked = subprocess.check_output(
        ["git", "ls-files", "--others", "--exclude-standard", "-z"], cwd=repo
    ).decode().split("\0")

    def eligible(name: str) -> bool:
        rel = Path(name)
        if (not name or rel.is_absolute() or "\\" in name or ".." in rel.parts
                or any(part in _ARCHIVE_EXCLUDED_PARTS for part in rel.parts)
                or rel.suffix.lower() not in _ARCHIVE_SOURCE_SUFFIXES):
            return False
        path = repo / rel
        if path.is_symlink() or not path.resolve().is_relative_to(repo.resolve()):
            raise RuntimeError(f"refusing unsafe changed source path: {name}")
        return path.is_file()

    tracked_names = sorted({name for name in tracked if eligible(name)})
    untracked_names = sorted({name for name in untracked if name and eligible(name)})
    return tracked_names, untracked_names


def _copy_source_artifacts(repo: Path, check_dir: Path, names: Iterable[str]) -> dict[str, str]:
    copied: dict[str, str] = {}
    root = repo.resolve()
    for name in sorted(set(names)):
        rel = Path(name)
        source = repo / rel
        if (not name or rel.is_absolute() or "\\" in name or ".." in rel.parts
                or source.is_symlink() or not source.resolve().is_relative_to(root)
                or not source.is_file()):
            raise RuntimeError(f"refusing unsafe or missing source artifact: {name}")
        data = source.read_bytes()
        destination = check_dir / "source-files" / rel
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(data)
        copied[rel.as_posix()] = _sha256_bytes(data)
    return copied


def _limited_child(memory_bytes: int):
    def apply_limit() -> None:
        resource.setrlimit(resource.RLIMIT_AS, (memory_bytes, memory_bytes))
    return apply_limit


def run_limited_process(
    argv: list[str], cwd: Path, env: Mapping[str, str], stdout: Any, stderr: Any,
    timeout_seconds: int, memory_bytes: int, grace_seconds: float = 2.0,
) -> tuple[int, bool]:
    """Run one command and terminate its whole new process group on timeout."""
    process = subprocess.Popen(
        argv, cwd=cwd, env=dict(env), stdout=stdout, stderr=stderr,
        start_new_session=True, preexec_fn=_limited_child(memory_bytes),
    )
    try:
        process.communicate(timeout=timeout_seconds)
        return process.returncode, False
    except subprocess.TimeoutExpired:
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            process.communicate(timeout=grace_seconds)
        except subprocess.TimeoutExpired:
            pass
        # The group may outlive its leader. Escalate against the runner-created
        # process group even if communicate() has already reaped the parent.
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.wait()
        return 124, True


def _parse_cover_args(values: list[str], harnesses: set[str]) -> dict[str, list[str]]:
    result: dict[str, list[str]] = {h: [] for h in harnesses}
    for value in values:
        if "=" not in value:
            raise ValueError("--require-cover must be HARNESS=LABEL")
        harness, label = value.split("=", 1)
        if harness not in harnesses or not label.strip():
            raise ValueError(f"invalid --require-cover value: {value!r}")
        result[harness].append(label)
    for harness, labels in result.items():
        if len(labels) != len(set(labels)):
            raise ValueError(f"duplicate required cover label for {harness}")
    return result


def run_one(
    repo: Path,
    package: str,
    harness: str,
    out: Path,
    target: Path,
    required_covers: Iterable[str],
    stubbing: bool,
    stub_summaries: list[str],
    cfgs: list[str],
    harness_timeout: int,
    timeout_seconds: int,
    memory_bytes: int,
    product_rustc: str,
    host_rustc: str,
    kani_version: str,
    kani_version_output: str,
    cargo_version: str,
    loop_unwind_preset: str | None = None,
) -> dict[str, Any]:
    check_dir = out / "checks" / hashlib.sha256(harness.encode()).hexdigest()[:12]
    check_dir.mkdir(parents=True, exist_ok=False)
    result_path = check_dir / "result.json"
    argv = [
        "cargo", "+1.89.0", "--locked", "kani", "--lib", "-p", package, "--harness", harness,
        "--jobs", "1", "-Z", "unstable-options", "--export-json", str(result_path),
        "--harness-timeout", f"{harness_timeout}s",
    ]
    if stubbing:
        argv.extend(["-Z", "stubbing"])
    if loop_unwind_preset is not None:
        _validate_loop_unwind_source(repo, loop_unwind_preset, package, harness)
        preset = LOOP_UNWIND_PRESETS[loop_unwind_preset]
        argv.extend([
            "--cbmc-args", "--unwindset",
            _loop_unwind_argument(preset),
            "--unwinding-assertions",
        ])
    checkout_namespace = hashlib.sha256(str(repo).encode("utf-8")).hexdigest()
    env = os.environ.copy()
    for key in ("RUSTC_WRAPPER", "CARGO_ENCODED_RUSTFLAGS", "KANIFLAGS", "RUSTFLAGS"):
        env.pop(key, None)
    env.update({
        "CARGO_TARGET_DIR": str(target),
        "CARGO_BUILD_JOBS": "1",
        "CARGO_NET_OFFLINE": "true",
    })
    if cfgs:
        env["RUSTFLAGS"] = " ".join(f"--cfg {cfg}" for cfg in cfgs)
    recorded_env = {
        key: env[key]
        for key in (
            "PATH", "RUSTUP_TOOLCHAIN", "RUSTUP_HOME", "CARGO_HOME", "KANI_HOME",
            "CARGO_TARGET_DIR", "CARGO_BUILD_JOBS", "CARGO_NET_OFFLINE", "RUSTFLAGS",
        )
        if key in env
    }
    excluded = [check_dir, out, target]
    source_before = _source_snapshot(repo, excluded)
    record: dict[str, Any] = {
        "record_version": 1,
        "repo": str(repo),
        "repo_revision": _git(repo, "rev-parse", "HEAD"),
        "package": package,
        "harness": harness,
        "loop_unwind_preset": loop_unwind_preset,
        "loop_unwind_configuration": _loop_unwind_configuration(loop_unwind_preset),
        "requested_covers": list(required_covers),
        "command": argv,
        "command_sha256": _canonical_sha256(argv),
        "environment": recorded_env,
        "environment_sha256": _canonical_sha256(recorded_env),
        "target_dir": str(target),
        "checkout_namespace": checkout_namespace,
        "source_before": source_before,
        "source_sha256_before": source_mapping_sha256(source_before),
        "source_artifact_hashes": {},
        "source_patch_sha256": None,
        "source_unchanged": False,
        "exit_code": None,
        "elapsed_seconds": None,
        "timeout_seconds": timeout_seconds,
        "memory_limit_bytes": memory_bytes,
        "jobs": 1,
        "kani_version": kani_version,
        "cargo_version": cargo_version,
        "kani_report_toolchain": None,
        "product_rustc_version": product_rustc,
        "host_rustc_version": host_rustc,
        "kani_version_output": kani_version_output,
        "stubbing_enabled": stubbing,
        "stub_scope_descriptions": stub_summaries,
        "cfgs": cfgs,
        "model_scope": (
            "Kani/CBMC bounded model checking of this named harness; not an unbounded proof "
            "or product compiler validation. Review source assumptions, cfgs, stubs and unwind bounds."
        ),
        "verification_method": "bounded model checking (Kani 0.68.0 / CBMC)",
    }
    (check_dir / "manifest-start.json").write_text(json.dumps(record, indent=2) + "\n")
    changed_tracked, changed_untracked = changed_source_paths(repo)
    patch_cmd = ["git", "diff", "--binary", "HEAD", "--", *changed_tracked]
    patch_bytes = subprocess.check_output(patch_cmd, cwd=repo)
    (check_dir / "source.patch").write_bytes(patch_bytes)
    record["source_patch_sha256"] = _sha256_bytes(patch_bytes)
    record["source_revision"] = record["repo_revision"]
    record["changed_tracked_source_files"] = changed_tracked
    record["changed_untracked_source_files"] = changed_untracked
    started = time.monotonic()
    rc: int
    with (check_dir / "stdout.log").open("w", encoding="utf-8") as stdout, (
        check_dir / "stderr.log"
    ).open("w", encoding="utf-8") as stderr:
        try:
            rc, timed_out = run_limited_process(
                argv, repo, env, stdout, stderr, timeout_seconds, memory_bytes
            )
        except OSError as exc:
            stderr.write(f"failed to start Kani command: {exc}\n")
            rc, timed_out = 127, False
    elapsed = time.monotonic() - started
    source_after = _source_snapshot(repo, excluded)
    record.update({
        "source_after": source_after,
        "source_sha256_after": source_mapping_sha256(source_after),
        "source_unchanged": source_before == source_after,
        "exit_code": rc,
        "timed_out": timed_out,
        "elapsed_seconds": elapsed,
        "outputs": {
            name: _sha256_file(check_dir / name)
            for name in ("stdout.log", "stderr.log", "result.json")
            if (check_dir / name).is_file()
        },
    })
    report_text = result_path.read_bytes() if result_path.is_file() else b""
    record["result_sha256"] = _sha256_bytes(report_text) if report_text else None
    report_obj, _, _ = _report_object(report_text) if report_text else (None, b"", [])
    if isinstance(report_obj, dict) and isinstance(report_obj.get("tools"), dict):
        record["kani_report_toolchain"] = report_obj["tools"].get("rustc")
    if loop_unwind_preset is not None:
        stdout_text = (check_dir / "stdout.log").read_text(encoding="utf-8", errors="replace")
        stderr_text = (check_dir / "stderr.log").read_text(encoding="utf-8", errors="replace")
        record["loop_unwind_log_check"] = _check_loop_unwind_log(
            loop_unwind_preset, stdout_text, stderr_text
        )
    copied_sources: dict[str, str] = {}
    source_names = set(changed_tracked) | set(changed_untracked)
    if isinstance(report_obj, dict):
        for item in report_obj.get("harness_metadata", []):
            source = item.get("source") if isinstance(item, dict) else None
            source_file = source.get("file") if isinstance(source, dict) else None
            if isinstance(source_file, str):
                rel = Path(source_file)
                source_path = repo / rel
                if (
                    not source_file
                    or "\\" in source_file
                    or rel.is_absolute()
                    or ".." in rel.parts
                    or source_path.is_symlink()
                    or not source_path.resolve().is_relative_to(repo)
                ):
                    record.setdefault("source_copy_errors", []).append(f"unsafe harness source path: {source_file}")
                elif source_path.is_file():
                    source_names.add(rel.as_posix())
                else:
                    record.setdefault("source_copy_errors", []).append(f"harness source file is missing: {source_file}")
    try:
        copied_sources = _copy_source_artifacts(repo, check_dir, source_names)
    except (OSError, RuntimeError) as exc:
        record.setdefault("source_copy_errors", []).append(str(exc))
    record["harness_source_copies"] = {
        name: digest for name, digest in copied_sources.items()
        if isinstance(report_obj, dict) and any(
            isinstance(item, dict)
            and isinstance(item.get("source"), dict)
            and item["source"].get("file") == name
            for item in report_obj.get("harness_metadata", [])
        )
    }
    record["source_artifact_hashes"] = {
        path.relative_to(check_dir).as_posix(): _sha256_file(path)
        for path in [check_dir / "source.patch", *[check_dir / "source-files" / p for p in copied_sources]]
        if path.is_file()
    }
    errors = evaluate_kani_binding(report_text, record) if report_text else ["Kani did not produce result.json"]
    errors.extend(evaluate_kani_report(report_text, [harness], {harness: required_covers}) if report_text else [])
    if loop_unwind_preset is not None and record["loop_unwind_log_check"]["passed"] is not True:
        errors.append("CBMC log did not confirm the registered scoped unwind bound at the expected source location")
    errors.extend(record.get("source_copy_errors", []))
    for source_path, digest in copied_sources.items():
        if record.get("source_after", {}).get(source_path) != digest:
            errors.append(f"copied harness source differs from recorded source snapshot: {source_path}")
    record["evaluation_errors"] = sorted(set(errors))
    record["report_diagnostics"] = summarize_kani_report(report_text) if report_text else {}
    (check_dir / "manifest.json").write_text(json.dumps(record, indent=2) + "\n")
    record["manifest_path"] = str(check_dir / "manifest.json")
    record["manifest_sha256"] = _sha256_file(check_dir / "manifest.json")
    return record


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--package", required=True)
    parser.add_argument("--harness", action="append", required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--target-dir", type=Path)
    parser.add_argument("--require-cover", action="append", default=[], metavar="HARNESS=LABEL")
    parser.add_argument("--stubbing", action="store_true")
    parser.add_argument("--stub-summary", action="append", default=[])
    parser.add_argument("--cfg", action="append", default=[], choices=sorted(ALLOWED_CFGS))
    parser.add_argument(
        "--loop-unwind-preset", choices=sorted(LOOP_UNWIND_PRESETS),
        help="Use one closed, source-guarded CBMC per-loop bound preset",
    )
    parser.add_argument("--harness-timeout", type=int, default=DEFAULT_TIMEOUT_SECONDS)
    parser.add_argument("--timeout", type=int, help="external timeout; default harness timeout + 120s")
    parser.add_argument("--memory-mib", type=int, default=DEFAULT_MEMORY_MIB)
    args = parser.parse_args(argv)

    if not PACKAGE_RE.fullmatch(args.package):
        parser.error("--package must be a simple Cargo package name")
    if not args.harness or len(args.harness) != len(set(args.harness)):
        parser.error("at least one unique --harness is required")
    for name in args.harness:
        if not HARNESS_RE.fullmatch(name):
            parser.error(f"invalid --harness name: {name!r}")
    if args.loop_unwind_preset is not None:
        preset = LOOP_UNWIND_PRESETS[args.loop_unwind_preset]
        if args.package != preset["package"] or any(name not in preset["harnesses"] for name in args.harness):
            parser.error(f"loop-unwind preset {args.loop_unwind_preset!r} is not applicable to package/harness")
    if args.harness_timeout <= 0 or args.memory_mib < 512:
        parser.error("timeouts must be positive and memory-mib must be at least 512")
    external_timeout = args.timeout if args.timeout is not None else args.harness_timeout + 120
    if external_timeout <= args.harness_timeout:
        parser.error("external --timeout must exceed --harness-timeout")
    if args.stubbing and not args.stub_summary:
        parser.error("--stubbing requires at least one --stub-summary for the manifest")
    if not args.stubbing and args.stub_summary:
        parser.error("--stub-summary requires --stubbing")
    try:
        required_covers = _parse_cover_args(args.require_cover, set(args.harness))
    except ValueError as exc:
        parser.error(str(exc))

    repo = args.repo.resolve(strict=True)
    if not repo.is_dir() or not (repo / "Cargo.toml").is_file():
        parser.error("--repo must be a Cargo workspace directory")
    if Path(_git(repo, "rev-parse", "--show-toplevel")).resolve() != repo:
        parser.error("--repo must be the Git worktree root")
    if args.loop_unwind_preset is not None:
        try:
            _validate_loop_unwind_source(repo, args.loop_unwind_preset, args.package, args.harness[0])
        except ValueError as exc:
            parser.error(str(exc))
    if args.out.exists():
        parser.error("--out must not already exist")
    out = args.out.resolve()
    checkout_namespace = hashlib.sha256(str(repo).encode("utf-8")).hexdigest()
    cache_base = args.target_dir.resolve() if args.target_dir else Path("/tmp/openatca-rust-assurance/build-cache")
    target = cache_base / "checkouts" / checkout_namespace
    if target == repo or repo in target.parents:
        parser.error("target cache must be outside the product worktree")
    cfgs = sorted(set(args.cfg))

    cargo = shutil.which("cargo")
    kani = shutil.which("kani")
    rustc = shutil.which("rustc")
    rustup = shutil.which("rustup")
    if not cargo or not kani or not rustc or not rustup:
        parser.error("cargo, pinned Kani, rustc and rustup must already be installed; no installer is run")
    cargo_version = subprocess.check_output([cargo, "+1.89.0", "--version"], cwd=repo, text=True).strip()
    if not re.match(r"^cargo 1\.89\.0(?:\s|$)", cargo_version):
        parser.error(f"expected Cargo 1.89.0, got {cargo_version!r}")
    kani_version_output = subprocess.check_output([kani, "--version"], text=True).strip()
    if not re.search(r"^Kani Rust Verifier " + re.escape(PINNED_KANI_VERSION) + r"(?:\s|$)", kani_version_output, re.MULTILINE):
        parser.error(f"expected Kani {PINNED_KANI_VERSION}, got {kani_version_output!r}")
    kani_version = PINNED_KANI_VERSION
    installed_toolchains = subprocess.check_output([rustup, "toolchain", "list"], text=True)
    if not any(re.match(r"^1\.89\.0(?:[-\s]|$)", line) for line in installed_toolchains.splitlines()):
        parser.error("Rust 1.89.0 is not installed; this runner does not install toolchains")
    product_rustc = subprocess.check_output([rustc, "+1.89.0", "--version"], cwd=repo, text=True).strip()
    if not re.match(r"^rustc 1\.89\.0(?:\s|$)", product_rustc):
        parser.error(f"expected product rustc 1.89.0, got {product_rustc!r}")
    host_rustc = subprocess.check_output([rustc, "--version"], cwd=repo, text=True).strip()

    out.mkdir(parents=True, exist_ok=False)
    records = []
    started_utc = dt.datetime.now(dt.timezone.utc).isoformat()
    for harness in args.harness:  # Intentionally sequential: one solver job at a time.
        record = run_one(
            repo=repo,
            package=args.package,
            harness=harness,
            out=out,
            target=target,
            required_covers=required_covers[harness],
            stubbing=args.stubbing,
            stub_summaries=args.stub_summary,
            cfgs=cfgs,
            harness_timeout=args.harness_timeout,
            timeout_seconds=external_timeout,
            memory_bytes=args.memory_mib * 1024 * 1024,
            product_rustc=product_rustc,
            host_rustc=host_rustc,
            kani_version=kani_version,
            kani_version_output=kani_version_output,
            cargo_version=cargo_version,
            loop_unwind_preset=args.loop_unwind_preset,
        )
        records.append(record)
    run_status = "PASS" if len(records) == len(args.harness) and all(not r["evaluation_errors"] for r in records) else "FAIL"
    run_record = {
        "run_version": 1,
        "status": run_status,
        "started_utc": started_utc,
        "repo": str(repo),
        "repo_revision": _git(repo, "rev-parse", "HEAD"),
        "package": args.package,
        "expected_harnesses": args.harness,
        "required_covers": required_covers,
        "loop_unwind_presets": {
            record["harness"]: [record["loop_unwind_preset"]] if record["loop_unwind_preset"] else []
            for record in records
        },
        "target_dir": str(target),
        "checkout_namespace": checkout_namespace,
        "model_scope": "Bounded named-harness results only; no unbounded/product-compiler claim.",
        "product_rustc_version": product_rustc,
        "host_rustc_version": host_rustc,
        "kani_version_output": kani_version_output,
        "kani_version": kani_version,
        "cargo_version": cargo_version,
        "stubbing_enabled": args.stubbing,
        "stub_scope_descriptions": args.stub_summary,
        "cfgs": cfgs,
        "checks": records,
    }
    (out / "run.json").write_text(json.dumps(run_record, indent=2) + "\n")
    print(json.dumps({"status": run_status, "checks": len(records), "out": str(out)}, indent=2))
    return 0 if run_status == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
