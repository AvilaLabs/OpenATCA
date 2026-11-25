#!/usr/bin/env python3
"""Build a private, deterministic M6/H3 evidence bundle from bounded JSON artifacts.

This is an internal version-1 adapter format, not an FAA format, ICD, approval, or authority
record. It only verifies the bytes and claims supplied to it. It does not produce operational
readiness or FAA acceptance. All input files are opened through ``tools.sealed_guard`` and all
absolute timestamps are checked against its production sealed windows.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import stat
import sys
from pathlib import PurePosixPath
from typing import Any

import sealed_guard

SCHEMA = "openatca.m6-input"
SCHEMA_VERSION = 1
PACKAGE_SCHEMA = "openatca.m6-evidence"
PACKAGE_VERSION = 1
MAX_MANIFEST_BYTES = 2 * 1024 * 1024
MAX_JSON_ARTIFACT_BYTES = 16 * 1024 * 1024
MAX_OPAQUE_ARTIFACT_BYTES = 64 * 1024 * 1024
MAX_TOTAL_ARTIFACT_BYTES = 256 * 1024 * 1024
MAX_ARTIFACTS = 32
MAX_JSON_DEPTH = 32
MAX_JSON_NODES = 1_000_000
MAX_HAZARDS = 10_000
MAX_DIFF_ROWS = 50_000
MAX_DRILLS = 10_000
MAX_CONTAINMENT_EVENTS = 100_000
MAX_OUTPUT_BYTES = 64 * 1024 * 1024
MAX_STRING_CHARS = 4096
MAX_I64 = (1 << 63) - 1
MIN_I64 = -(1 << 63)
HASH_RE = re.compile(r"[0-9a-f]{64}", re.ASCII)
FAMILIES = ("handoff", "point_out", "sector_assignment", "flight_amendment", "alert")
ARTIFACT_KINDS = {
    "hazard_register",
    "shadow_comparison",
    "exposure",
    "containment",
    "drill_results",
    "review_evidence",
    "recorded_traffic",
    "supporting_evidence",
}
ABSOLUTE_TIME_KEYS = {
    "window_start_ns",
    "window_end_ns",
    "window_from_ns",
    "window_to_ns",
    "start_ns",
    "end_ns",
    "observation_start_ns",
    "observation_end_ns",
    "run_start_ns",
    "run_end_ns",
    "source_time_ns",
    "observed_time_ns",
    "event_time_ns",
    "decision_time_ns",
    "started_at_ns",
    "ended_at_ns",
    "observed_at_ns",
    "timestamp_ns",
    "at_ns",
    "evaluated_at_ns",
    "transition_ns",
}
REQUIRED_ARTIFACT_KINDS = {
    "hazard_register",
    "shadow_comparison",
    "exposure",
    "containment",
    "drill_results",
}
OUTCOME_TO_COUNTER = {
    "Equivalent": "exact_pairs",
    "Different": "different_pairs",
    "LegacyOnly": "legacy_only",
    "CandidateOnly": "candidate_only",
    "Unobservable": "unobservable_groups",
    "OutsideWindow": "outside_only_groups",
}
SIDE_PARTITIONS = (
    "eligible",
    "missing_key",
    "invalid_key",
    "ambiguous_key",
    "missing_time",
    "outside_window",
    "unknown",
    "unsupported",
    "invalid",
)
M4_FAMILY_COUNTERS = (
    "legacy_records", "candidate_records",
    "legacy_eligible", "candidate_eligible",
    "legacy_missing_key", "candidate_missing_key",
    "legacy_invalid_key", "candidate_invalid_key",
    "legacy_ambiguous_key", "candidate_ambiguous_key",
    "legacy_missing_time", "candidate_missing_time",
    "legacy_outside_window", "candidate_outside_window",
    "legacy_unknown", "candidate_unknown",
    "legacy_unsupported", "candidate_unsupported",
    "legacy_invalid", "candidate_invalid",
    "legacy_out_of_order", "candidate_out_of_order",
    "key_groups", "exact_pairs", "different_pairs", "legacy_only", "candidate_only",
    "duplicate_groups", "duplicate_legacy_records", "duplicate_candidate_records",
    "unobservable_groups", "outside_only_groups", "field_differences", "field_unobservable",
    "source_time_differences", "observed_time_differences",
    "legacy_observed_time_missing", "candidate_observed_time_missing",
    "legacy_fields_present", "candidate_fields_present",
    "legacy_fields_absent", "candidate_fields_absent",
    "legacy_fields_unknown", "candidate_fields_unknown",
    "legacy_fields_unsupported", "candidate_fields_unsupported",
    "legacy_fields_invalid", "candidate_fields_invalid",
)
ADAPTER_FAMILY_COUNTERS = (
    "legacy_in_window_records", "candidate_in_window_records",
    "legacy_missing_source_time_records", "candidate_missing_source_time_records",
)
EXPECTED_FAMILY_COUNTERS = set(M4_FAMILY_COUNTERS + ADAPTER_FAMILY_COUNTERS)


class EvidenceError(Exception):
    """The input contract cannot support a trustworthy package."""


class FatalInput(EvidenceError):
    """The manifest itself is malformed or a sealed guard refuses access."""


def _fail(message: str) -> None:
    raise EvidenceError(message)


def _object(value: Any, where: str, keys: set[str] | None = None) -> dict[str, Any]:
    if not isinstance(value, dict) or any(not isinstance(key, str) for key in value):
        _fail(f"{where}: expected a JSON object")
    if keys is not None:
        missing = keys - value.keys()
        extra = value.keys() - keys
        if missing or extra:
            _fail(f"{where}: schema fields missing={sorted(missing)} extra={sorted(extra)}")
    return value


def _list(value: Any, where: str, cap: int) -> list[Any]:
    if not isinstance(value, list):
        _fail(f"{where}: expected an array")
    if len(value) > cap:
        _fail(f"{where}: cap exceeded ({len(value)} > {cap})")
    return value


def _text(value: Any, where: str, *, allow_empty: bool = False) -> str:
    if not isinstance(value, str) or len(value) > MAX_STRING_CHARS:
        _fail(f"{where}: expected bounded text")
    if not allow_empty and not value.strip():
        _fail(f"{where}: empty text is not permitted")
    if any(ord(char) < 0x20 and char not in "\t\n\r" for char in value):
        _fail(f"{where}: control characters are not permitted")
    return value


def _integer(value: Any, where: str, *, minimum: int = 0, maximum: int = MAX_I64) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        _fail(f"{where}: expected an integer")
    if value < minimum or value > maximum:
        _fail(f"{where}: integer outside [{minimum}, {maximum}]")
    return value


def _i64(value: Any, where: str) -> int:
    return _integer(value, where, minimum=MIN_I64, maximum=MAX_I64)


def _hash(value: Any, where: str) -> str:
    if not isinstance(value, str) or HASH_RE.fullmatch(value) is None:
        _fail(f"{where}: expected lowercase SHA-256 hex")
    return value


def _unique_pairs(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise EvidenceError(f"duplicate JSON object key: {key}")
        result[key] = value
    return result


def _reject_float(value: str) -> Any:
    raise EvidenceError(f"floating-point JSON number is not accepted: {value[:32]}")


def _reject_constant(value: str) -> Any:
    raise EvidenceError(f"non-finite JSON constant is not accepted: {value}")


def _walk_bounded(value: Any) -> None:
    stack: list[tuple[Any, int]] = [(value, 1)]
    nodes = 0
    guard = sealed_guard.production()
    while stack:
        current, depth = stack.pop()
        nodes += 1
        if nodes > MAX_JSON_NODES or depth > MAX_JSON_DEPTH:
            _fail("JSON structural cap exceeded")
        if isinstance(current, dict):
            for key, child in current.items():
                if key in ABSOLUTE_TIME_KEYS:
                    if isinstance(child, bool) or not isinstance(child, int):
                        _fail(f"{key}: absolute timestamps must be integer nanoseconds")
                    if guard.in_window(child):
                        raise sealed_guard.SealedError(
                            f"{key}: refused by OA-SEAL-010: timestamp falls in sealed window"
                        )
                stack.append((child, depth + 1))
        elif isinstance(current, list):
            for child in current:
                stack.append((child, depth + 1))
        elif isinstance(current, str) and len(current) > MAX_STRING_CHARS:
            _fail("JSON string cap exceeded")


def _parse_json(raw: bytes, where: str) -> Any:
    try:
        text = raw.decode("utf-8", errors="strict")
        value = json.loads(
            text,
            object_pairs_hook=_unique_pairs,
            parse_float=_reject_float,
            parse_constant=_reject_constant,
        )
    except (UnicodeError, json.JSONDecodeError, RecursionError, ValueError) as exc:
        if isinstance(exc, EvidenceError):
            raise
        raise EvidenceError(f"{where}: invalid bounded JSON: {exc}") from exc
    _walk_bounded(value)
    return value


def _stable_stat(file_obj: Any, where: str) -> os.stat_result:
    before = os.fstat(file_obj.fileno())
    if not stat.S_ISREG(before.st_mode):
        _fail(f"{where}: input is not a regular file")
    return before


def _same_file(before: os.stat_result, after: os.stat_result) -> bool:
    return (
        before.st_dev == after.st_dev
        and before.st_ino == after.st_ino
        and before.st_size == after.st_size
        and before.st_mtime_ns == after.st_mtime_ns
        and before.st_ctime_ns == after.st_ctime_ns
    )


def _read_file(path: str, cap: int) -> tuple[bytes, str, int]:
    guard = sealed_guard.production()
    with guard.open_regular_read(path, "rb") as file_obj:
        before = _stable_stat(file_obj, path)
        if before.st_size > cap:
            _fail(f"{path}: byte cap exceeded ({before.st_size} > {cap})")
        data = file_obj.read(cap + 1)
        after = os.fstat(file_obj.fileno())
        if len(data) > cap:
            _fail(f"{path}: byte cap exceeded while reading")
        if not _same_file(before, after) or len(data) != before.st_size:
            _fail(f"{path}: input changed or was truncated while read")
    return data, hashlib.sha256(data).hexdigest(), len(data)


def _hash_opaque(path: str, cap: int) -> tuple[str, int]:
    raw, digest, size = _read_file(path, cap)
    if sealed_guard.file_has_sealed_time(raw, sealed_guard.production()):
        raise sealed_guard.SealedError(f"{path}: refused by OA-SEAL-010: opaque artifact contains a sealed time token")
    return digest, size


def _relative_artifact_path(manifest_path: str, value: Any, where: str) -> str:
    rel = _text(value, where)
    pure = PurePosixPath(rel)
    if "\\" in rel or pure.is_absolute() or any(part in ("", ".", "..") for part in pure.parts):
        _fail(f"{where}: artifact path must be a clean relative path")
    parent = os.path.dirname(os.path.abspath(manifest_path))
    return os.path.join(parent, *pure.parts)


def _validate_unit(unit: Any) -> dict[str, Any]:
    unit = _object(
        unit,
        "unit",
        {
            "function_id",
            "scope_id",
            "comparison_family",
            "window_start_ns",
            "window_end_ns",
            "stage",
            "policy",
            "source_epochs",
        },
    )
    function_id = _integer(unit["function_id"], "unit.function_id", maximum=(1 << 32) - 1)
    scope_id = _integer(unit["scope_id"], "unit.scope_id", maximum=(1 << 32) - 1)
    if function_id == 0 or scope_id == 0:
        _fail("unit function_id and scope_id must be nonzero")
    family = _text(unit["comparison_family"], "unit.comparison_family")
    if family not in FAMILIES:
        _fail("unit.comparison_family is not a supported M4 family")
    start = _i64(unit["window_start_ns"], "unit.window_start_ns")
    end = _i64(unit["window_end_ns"], "unit.window_end_ns")
    if start >= end:
        _fail("unit window must be a nonempty half-open interval")
    stage = _text(unit["stage"], "unit.stage")
    if stage not in {"S0Shadow", "S1Advisory", "S2Window", "S3Extended", "S4Continuous"}:
        _fail("unit.stage is not an active M1 rung")
    policy = _object(unit["policy"], "unit.policy", {"id", "version", "sha256"})
    policy_id = _text(policy["id"], "unit.policy.id")
    policy_version = _integer(policy["version"], "unit.policy.version", maximum=65535)
    policy_sha = _hash(policy["sha256"], "unit.policy.sha256")
    epochs = _object(unit["source_epochs"], "unit.source_epochs", {"legacy", "candidate"})
    return {
        "function_id": function_id,
        "scope_id": scope_id,
        "comparison_family": family,
        "window_start_ns": start,
        "window_end_ns": end,
        "stage": stage,
        "policy": {"id": policy_id, "version": policy_version, "sha256": policy_sha},
        "source_epochs": {
            "legacy": _hash(epochs["legacy"], "unit.source_epochs.legacy"),
            "candidate": _hash(epochs["candidate"], "unit.source_epochs.candidate"),
        },
    }


def _validate_manifest(value: Any, path: str) -> tuple[dict[str, Any], list[dict[str, Any]]]:
    value = _object(value, "manifest", {"schema", "version", "unit", "artifacts"})
    if _text(value["schema"], "manifest.schema") != SCHEMA or _integer(value["version"], "manifest.version", maximum=65535) != SCHEMA_VERSION:
        _fail("unsupported M6 input schema or version")
    unit = _validate_unit(value["unit"])
    artifacts_raw = _list(value["artifacts"], "manifest.artifacts", MAX_ARTIFACTS)
    artifacts: list[dict[str, Any]] = []
    seen: set[str] = set()
    for index, item in enumerate(artifacts_raw):
        item = _object(
            item,
            f"manifest.artifacts[{index}]",
            {"id", "kind", "path", "sha256", "bytes"},
        )
        artifact_id = _text(item["id"], f"artifact[{index}].id")
        kind = _text(item["kind"], f"artifact[{index}].kind")
        if artifact_id in seen:
            _fail(f"duplicate artifact id: {artifact_id}")
        if kind not in ARTIFACT_KINDS:
            _fail(f"artifact[{index}]: unknown artifact kind {kind!r}")
        seen.add(artifact_id)
        path_value = item["path"]
        digest_value = item["sha256"]
        size_value = item["bytes"]
        if path_value is None:
            if digest_value is not None or size_value is not None:
                _fail(f"artifact {artifact_id}: a missing path must have null hash and size")
            artifact_path = None
            declared_hash = None
            declared_bytes = None
        else:
            artifact_path = _relative_artifact_path(path, path_value, f"artifact {artifact_id}.path")
            declared_hash = _hash(digest_value, f"artifact {artifact_id}.sha256")
            declared_bytes = _integer(size_value, f"artifact {artifact_id}.bytes", maximum=MAX_TOTAL_ARTIFACT_BYTES)
        artifacts.append(
            {
                "id": artifact_id,
                "kind": kind,
                "path_ref": path_value,
                "path": artifact_path,
                "declared_sha256": declared_hash,
                "declared_bytes": declared_bytes,
            }
        )
    declared_total = 0
    for artifact in artifacts:
        if artifact["declared_bytes"] is None:
            continue
        cap = (
            MAX_OPAQUE_ARTIFACT_BYTES
            if artifact["kind"] in {"review_evidence", "recorded_traffic", "supporting_evidence"}
            else MAX_JSON_ARTIFACT_BYTES
        )
        if artifact["declared_bytes"] > cap:
            _fail(f"artifact {artifact['id']}: declared bytes exceed per-kind cap")
        declared_total += artifact["declared_bytes"]
        if declared_total > MAX_TOTAL_ARTIFACT_BYTES:
            _fail("declared aggregate artifact bytes exceed cap")
    return unit, artifacts


def _artifact_index(artifacts: list[dict[str, Any]]) -> dict[str, dict[str, Any]]:
    return {artifact["id"]: artifact for artifact in artifacts}


def _load_artifacts(artifacts: list[dict[str, Any]]) -> tuple[dict[str, Any], list[dict[str, Any]], list[str]]:
    loaded: dict[str, Any] = {}
    statuses: list[dict[str, Any]] = []
    gaps: list[str] = []
    total = 0
    for item in artifacts:
        artifact_status = {
            "id": item["id"],
            "kind": item["kind"],
            "path_ref": item["path_ref"],
            "declared_sha256": item["declared_sha256"],
            "declared_bytes": item["declared_bytes"],
            "actual_sha256": None,
            "actual_bytes": None,
            "status": "missing",
            "error": None,
        }
        if item["path"] is None:
            artifact_status["error"] = "not supplied"
            gaps.append(f"MISSING_ARTIFACT:{item['id']}")
            statuses.append(artifact_status)
            continue
        try:
            remaining_total = MAX_TOTAL_ARTIFACT_BYTES - total
            if remaining_total <= 0:
                raise EvidenceError("total input artifact byte cap exceeded")
            if item["kind"] in {"review_evidence", "recorded_traffic", "supporting_evidence"}:
                digest, size = _hash_opaque(item["path"], min(MAX_OPAQUE_ARTIFACT_BYTES, remaining_total))
                value = None
            else:
                raw, digest, size = _read_file(item["path"], min(MAX_JSON_ARTIFACT_BYTES, remaining_total))
                value = _object(_parse_json(raw, item["path"]), item["path"])
            total = total + size
            if total > MAX_TOTAL_ARTIFACT_BYTES:
                raise EvidenceError("total input artifact byte cap exceeded")
            artifact_status["actual_sha256"] = digest
            artifact_status["actual_bytes"] = size
            if digest != item["declared_sha256"] or size != item["declared_bytes"]:
                artifact_status["status"] = "hash_or_size_mismatch"
                artifact_status["error"] = "actual bytes do not match manifest identity"
                gaps.append(f"ARTIFACT_IDENTITY_MISMATCH:{item['id']}")
            else:
                artifact_status["status"] = "verified"
                if value is not None:
                    loaded[item["id"]] = value
        except sealed_guard.SealedError:
            raise
        except (EvidenceError, OSError, ValueError) as exc:
            artifact_status["status"] = "unavailable_or_invalid"
            artifact_status["error"] = str(exc)[:512]
            gaps.append(f"ARTIFACT_UNAVAILABLE_OR_INVALID:{item['id']}")
        statuses.append(artifact_status)
    return loaded, statuses, gaps


def _check_doc_header(doc: dict[str, Any], schema: str, label: str) -> None:
    if _text(doc.get("schema"), f"{label}.schema") != schema or _integer(doc.get("version"), f"{label}.version", maximum=65535) != 1:
        _fail(f"{label}: unsupported schema or version")


def _bind_doc(doc: dict[str, Any], unit: dict[str, Any], label: str) -> None:
    for key in ("function_id", "scope_id"):
        value = _integer(doc.get(key), f"{label}.{key}", maximum=(1 << 32) - 1)
        if value != unit[key]:
            _fail(f"{label}: {key} differs from the swap unit")
    for key in ("window_start_ns", "window_end_ns"):
        value = _i64(doc.get(key), f"{label}.{key}")
        if value != unit[key]:
            _fail(f"{label}: {key} differs from the swap unit")


def _validate_hazards(doc: dict[str, Any], unit: dict[str, Any]) -> list[dict[str, Any]]:
    doc = _object(doc, "hazard register", {"schema", "version", "function_id", "scope_id", "window_start_ns", "window_end_ns", "source", "hazards"})
    _check_doc_header(doc, "openatca.m6-hazard-register", "hazard register")
    _bind_doc(doc, unit, "hazard register")
    source = _object(
        doc.get("source"),
        "hazard register source",
        {"document_ref", "revision", "coverage_status", "hazard_count"},
    )
    source_ref = _text(source["document_ref"], "hazard register source.document_ref")
    source_revision = _text(source["revision"], "hazard register source.revision")
    coverage_status = _text(source["coverage_status"], "hazard register source.coverage_status")
    declared_hazard_count = _integer(source["hazard_count"], "hazard register source.hazard_count", maximum=MAX_HAZARDS)
    hazards = _list(doc.get("hazards"), "hazards", MAX_HAZARDS)
    ids: set[str] = set()
    result: list[dict[str, Any]] = []
    for index, raw in enumerate(hazards):
        hazard = _object(raw, f"hazards[{index}]", {"id", "description", "mitigations"})
        hazard_id = _text(hazard["id"], f"hazards[{index}].id")
        if hazard_id in ids:
            _fail(f"duplicate hazard id {hazard_id}")
        ids.add(hazard_id)
        mitigations = _list(hazard["mitigations"], f"hazards[{index}].mitigations", 64)
        if not mitigations:
            _fail(f"hazard {hazard_id} has no listed mitigation")
        seen_mitigations: set[str] = set()
        clean_mitigations = []
        for j, raw_mitigation in enumerate(mitigations):
            mitigation = _object(
                raw_mitigation,
                f"hazards[{index}].mitigations[{j}]",
                {"id", "description", "verification_ref"},
            )
            mitigation_id = _text(mitigation["id"], "mitigation.id")
            if mitigation_id in seen_mitigations:
                _fail(f"hazard {hazard_id}: duplicate mitigation id {mitigation_id}")
            seen_mitigations.add(mitigation_id)
            clean_mitigations.append(
                {
                    "id": mitigation_id,
                    "description": _text(mitigation["description"], "mitigation.description"),
                    "verification_ref": _text(mitigation["verification_ref"], "mitigation.verification_ref"),
                }
            )
        result.append(
            {
                "id": hazard_id,
                "description": _text(hazard["description"], f"hazards[{index}].description"),
                "mitigations": clean_mitigations,
            }
        )
    if coverage_status != "complete_for_function" or declared_hazard_count != len(result):
        _fail("hazard register does not declare complete function coverage with a matching count")
    return [
        {
            "source_document_ref": source_ref,
            "source_revision": source_revision,
            "coverage_status": coverage_status,
            "declared_hazard_count": declared_hazard_count,
            "hazards": result,
        }
    ]


def _validate_shadow(
    doc: dict[str, Any],
    unit: dict[str, Any],
    artifacts: dict[str, dict[str, Any]],
) -> tuple[dict[str, Any], list[dict[str, Any]], list[str]]:
    doc = _object(
        doc,
        "shadow artifact",
        {"schema", "version", "function_id", "scope_id", "window_start_ns", "window_end_ns", "family", "policy", "source_epochs", "coverage", "summary", "comparisons"},
    )
    _check_doc_header(doc, "openatca.m6-shadow-adapter", "shadow artifact")
    _bind_doc(doc, unit, "shadow artifact")
    family = unit["comparison_family"]
    if doc.get("family") != family:
        _fail("shadow artifact family differs from unit comparison_family")
    policy = _object(doc.get("policy"), "shadow policy", {"id", "version", "sha256"})
    normalized_policy = {
        "id": _text(policy["id"], "shadow.policy.id"),
        "version": _integer(policy["version"], "shadow.policy.version", maximum=65535),
        "sha256": _hash(policy["sha256"], "shadow.policy.sha256"),
    }
    if normalized_policy != unit["policy"]:
        _fail("shadow policy identity differs from unit policy")
    epochs = _object(doc.get("source_epochs"), "shadow source_epochs", {"legacy", "candidate"})
    clean_epochs = {side: _hash(epochs[side], f"shadow source_epochs.{side}") for side in epochs}
    if clean_epochs != unit["source_epochs"]:
        _fail("shadow source epoch hashes differ from unit")
    coverage = _object(doc.get("coverage"), "shadow coverage", {"legacy", "candidate"})
    allowed_coverage = {"CompleteWithEvents", "CompleteEmpty", "Partial", "Missing", "Unsupported"}
    if coverage["legacy"] not in allowed_coverage or coverage["candidate"] not in allowed_coverage:
        _fail("shadow coverage status is unknown")
    summary = _object(doc.get("summary"), "shadow summary", {"comparison_rows", "family_counts"})
    comparisons = _list(doc.get("comparisons"), "shadow comparisons", MAX_DIFF_ROWS)
    expected_rows = _integer(summary.get("comparison_rows"), "shadow.summary.comparison_rows", maximum=MAX_DIFF_ROWS)
    if expected_rows != len(comparisons):
        _fail("shadow comparison row count does not reconcile")
    family_counts = _object(summary.get("family_counts"), "shadow.summary.family_counts", EXPECTED_FAMILY_COUNTERS)
    counters = {
        key: _integer(value, f"shadow.summary.family_counts.{key}")
        for key, value in family_counts.items()
    }
    for side in ("legacy", "candidate"):
        partition = sum(counters[f"{side}_{part}"] for part in SIDE_PARTITIONS)
        if partition != counters[f"{side}_records"]:
            _fail(f"shadow {side} record denominator does not reconcile")
        status = coverage[side]
        in_window = counters[f"{side}_in_window_records"]
        missing_source_time = counters[f"{side}_missing_source_time_records"]
        if in_window > counters[f"{side}_records"] - missing_source_time:
            _fail(f"shadow {side} in-window records exceed timestamped source records")
        if (status == "CompleteEmpty" and (in_window != 0 or missing_source_time != 0)) or (status == "CompleteWithEvents" and (in_window == 0 or missing_source_time != 0)):
            _fail(f"shadow {side} coverage status contradicts its source-row counts")
        if status in {"Missing", "Unsupported"} and in_window != 0:
            _fail(f"shadow {side} missing/unsupported coverage has in-window rows")
    row_counts = {name: 0 for name in (
        "exact_pairs", "different_pairs", "legacy_only", "candidate_only",
        "unobservable_groups", "outside_only_groups", "missing_key_rows",
        "invalid_key_rows", "ambiguous_key_rows", "duplicate_rows",
    )}
    differences: list[dict[str, Any]] = []
    field_issue_counts = {"Different": 0, "Unobservable": 0, "MissingRequired": 0}
    seen_comparison_ids: set[str] = set()
    review_gaps: list[str] = []
    resolved_evidence = {
        item["declared_sha256"]
        for item in artifacts.values()
        if item["kind"] == "review_evidence" and item["status"] == "verified"
    }
    for index, raw in enumerate(comparisons):
        row = _object(
            raw,
            f"comparisons[{index}]",
            {"comparison_id", "outcome", "classification", "review", "legacy_source", "candidate_source", "field_issues", "duplicate_group_id"},
        )
        comparison_id = _text(row["comparison_id"], f"comparisons[{index}].comparison_id")
        if comparison_id in seen_comparison_ids:
            _fail(f"duplicate comparison id {comparison_id}")
        seen_comparison_ids.add(comparison_id)
        outcome = _text(row["outcome"], f"comparisons[{index}].outcome")
        legacy_identity = _validate_source_identity(row["legacy_source"], comparison_id, "legacy")
        candidate_identity = _validate_source_identity(row["candidate_source"], comparison_id, "candidate")
        if legacy_identity is None and candidate_identity is None:
            _fail(f"comparison {comparison_id}: row has no side-local source identity")
        if outcome in OUTCOME_TO_COUNTER:
            row_counts[OUTCOME_TO_COUNTER[outcome]] += 1
        elif outcome == "DuplicateAmbiguous":
            row_counts["duplicate_rows"] += 1
        elif outcome in {"MissingKey", "InvalidKey", "AmbiguousKey"}:
            row_counts[{"MissingKey": "missing_key_rows", "InvalidKey": "invalid_key_rows", "AmbiguousKey": "ambiguous_key_rows"}[outcome]] += 1
        else:
            _fail(f"comparison {comparison_id}: unknown outcome {outcome!r}")
        classification = _text(row["classification"], f"comparison {comparison_id}.classification")
        review = row["review"]
        evidence_hash = None
        reviewer_role = None
        is_exact = outcome == "Equivalent"
        if is_exact:
            if classification != "equivalent" or review is not None:
                _fail(f"comparison {comparison_id}: exact outcome has a contradictory classification")
            if legacy_identity is None or candidate_identity is None:
                _fail(f"comparison {comparison_id}: exact pair lacks a source identity")
        elif outcome == "OutsideWindow":
            if classification != "outside_window" or review is not None:
                _fail(f"comparison {comparison_id}: outside-window row has a contradictory classification")
        elif classification not in {"better", "worse", "unresolved", "pending"}:
            _fail(f"comparison {comparison_id}: classification is unknown")
        if classification in {"better", "worse"}:
            if outcome not in {"Different", "LegacyOnly", "CandidateOnly"}:
                _fail(f"comparison {comparison_id}: this outcome is not reviewable")
            review = _object(
                review,
                f"comparison {comparison_id}.review",
                {"comparison_id", "judgment", "policy", "function_id", "scope_id", "window_start_ns", "window_end_ns", "source_epochs", "legacy_source", "candidate_source", "reviewer_role", "evidence_sha256"},
            )
            if review["comparison_id"] != comparison_id or review["judgment"] != classification:
                _fail(f"comparison {comparison_id}: review does not bind this row and classification")
            review_policy = _object(review["policy"], "review policy", {"id", "version", "sha256"})
            normalized_review_policy = {
                "id": _text(review_policy["id"], "review.policy.id"),
                "version": _integer(review_policy["version"], "review.policy.version", maximum=65535),
                "sha256": _hash(review_policy["sha256"], "review.policy.sha256"),
            }
            if normalized_review_policy != unit["policy"]:
                _fail(f"comparison {comparison_id}: review policy does not match unit")
            for key in ("function_id", "scope_id"):
                value = _integer(review[key], f"review.{key}", maximum=(1 << 32) - 1)
                if value != unit[key]:
                    _fail(f"comparison {comparison_id}: review {key} does not match unit")
            for key in ("window_start_ns", "window_end_ns"):
                value = _i64(review[key], f"review.{key}")
                if value != unit[key]:
                    _fail(f"comparison {comparison_id}: review {key} does not match unit")
            review_epochs = _object(review["source_epochs"], "review source_epochs", {"legacy", "candidate"})
            if {side: _hash(review_epochs[side], f"review.source_epochs.{side}") for side in review_epochs} != unit["source_epochs"]:
                _fail(f"comparison {comparison_id}: review source epochs do not match unit")
            if _validate_source_identity(review["legacy_source"], comparison_id, "review legacy") != legacy_identity or _validate_source_identity(review["candidate_source"], comparison_id, "review candidate") != candidate_identity:
                _fail(f"comparison {comparison_id}: review does not bind exact source pair")
            if outcome == "Different" and (legacy_identity is None or candidate_identity is None):
                _fail(f"comparison {comparison_id}: different pair lacks one source identity")
            if outcome == "LegacyOnly" and (legacy_identity is None or candidate_identity is not None):
                _fail(f"comparison {comparison_id}: legacy-only row has inconsistent source identities")
            if outcome == "CandidateOnly" and (legacy_identity is not None or candidate_identity is None):
                _fail(f"comparison {comparison_id}: candidate-only row has inconsistent source identities")
            digest = _hash(review["evidence_sha256"], f"comparison {comparison_id}.review.evidence_sha256")
            role = _text(review["reviewer_role"], f"comparison {comparison_id}.review.reviewer_role")
            if role not in {"OperationsEvaluator", "SafetyReviewer", "IndependentAssessor"}:
                _fail(f"comparison {comparison_id}: unrecognized reviewer role")
            if digest not in resolved_evidence:
                review_gaps.append(f"UNRESOLVED_REVIEW_EVIDENCE:{comparison_id}")
            evidence_hash = digest
            reviewer_role = role
        elif not is_exact:
            if review is not None:
                _fail(f"comparison {comparison_id}: review object on a pending/nonreviewable row")
            if classification in {"unresolved", "pending"}:
                review_gaps.append(f"REVIEW_PENDING:{comparison_id}")
        field_issues = _list(row["field_issues"], f"comparison {comparison_id}.field_issues", 32)
        clean_issues = []
        prior_field = -1
        for issue in field_issues:
            issue = _object(issue, "field issue", {"id", "kind"})
            field_id = _integer(issue["id"], "field issue.id", maximum=65535)
            kind = _text(issue["kind"], "field issue.kind")
            if field_id <= prior_field or kind not in {"Different", "Unobservable", "MissingRequired"}:
                _fail(f"comparison {comparison_id}: field issues are unordered or invalid")
            prior_field = field_id
            field_issue_counts[kind] += 1
            clean_issues.append({"id": field_id, "kind": kind})
        if outcome in {"Equivalent", "OutsideWindow"} and clean_issues:
            _fail(f"comparison {comparison_id}: non-comparison row contains field issues")
        if outcome == "Equivalent" and classification != "equivalent":
            _fail(f"comparison {comparison_id}: exact row classification is not equivalent")
        if outcome in {"Different", "LegacyOnly", "CandidateOnly"} and classification == "outside_window":
            _fail(f"comparison {comparison_id}: comparable row has outside-window classification")
        if outcome == "Different" and classification == "equivalent":
            _fail(f"comparison {comparison_id}: a different row cannot be classified equivalent")
        if outcome in {"DuplicateAmbiguous", "Unobservable", "MissingKey", "InvalidKey", "AmbiguousKey"} and classification != "unresolved":
            review_gaps.append(f"NONREVIEWABLE_OUTCOME_NOT_UNRESOLVED:{comparison_id}")
        if outcome not in {"Equivalent", "OutsideWindow"}:
            differences.append(
                {
                    "comparison_id": comparison_id,
                    "outcome": outcome,
                    "classification": classification,
                    "reviewer_role": reviewer_role,
                    "evidence_sha256": evidence_hash,
                    "legacy_source": legacy_identity,
                    "candidate_source": candidate_identity,
                    "field_issues": clean_issues,
                }
            )
    expected_outcomes = {
        "exact_pairs": counters["exact_pairs"],
        "different_pairs": counters["different_pairs"],
        "legacy_only": counters["legacy_only"],
        "candidate_only": counters["candidate_only"],
        "unobservable_groups": counters["unobservable_groups"],
        "outside_only_groups": counters["outside_only_groups"],
        "missing_key_rows": counters["legacy_missing_key"] + counters["candidate_missing_key"],
        "invalid_key_rows": counters["legacy_invalid_key"] + counters["candidate_invalid_key"],
        "ambiguous_key_rows": counters["legacy_ambiguous_key"] + counters["candidate_ambiguous_key"],
        "duplicate_rows": counters["duplicate_legacy_records"] + counters["duplicate_candidate_records"],
    }
    if row_counts != expected_outcomes:
        _fail("shadow row outcomes do not reconcile to the published per-family counters")
    if field_issue_counts["Different"] != counters["field_differences"] or field_issue_counts["Unobservable"] + field_issue_counts["MissingRequired"] != counters["field_unobservable"]:
        _fail("field issue rows do not reconcile to the published per-family counters")
    expected_duplicate_groups = _integer(counters["duplicate_groups"], "duplicate_groups")
    duplicate_group_ids = {
        _text(row.get("duplicate_group_id"), "duplicate_group_id")
        for row in comparisons
        if row.get("outcome") == "DuplicateAmbiguous"
    }
    if len(duplicate_group_ids) != expected_duplicate_groups:
        _fail("duplicate group identities do not reconcile")
    gaps: list[str] = []
    if coverage["legacy"] in {"Missing", "Partial", "Unsupported"} or coverage["candidate"] in {"Missing", "Partial", "Unsupported"}:
        gaps.append("SHADOW_COVERAGE_INCOMPLETE")
    if coverage["legacy"] == "CompleteEmpty" and coverage["candidate"] == "CompleteEmpty":
        gaps.append("SHADOW_FAMILY_EMPTY")
    if counters["legacy_out_of_order"] or counters["candidate_out_of_order"]:
        gaps.append("SHADOW_SOURCE_OUT_OF_ORDER")
    incomplete_record_counters = (
        "legacy_missing_key", "candidate_missing_key", "legacy_invalid_key", "candidate_invalid_key",
        "legacy_ambiguous_key", "candidate_ambiguous_key", "legacy_missing_time", "candidate_missing_time",
        "legacy_unknown", "candidate_unknown", "legacy_unsupported", "candidate_unsupported",
        "legacy_invalid", "candidate_invalid", "legacy_fields_unknown", "candidate_fields_unknown",
        "legacy_fields_unsupported", "candidate_fields_unsupported", "legacy_fields_invalid", "candidate_fields_invalid",
    )
    if any(counters[name] > 0 for name in incomplete_record_counters) or counters["field_unobservable"] > 0:
        gaps.append("SHADOW_RECORD_OR_FIELD_OBSERVABILITY_INCOMPLETE")
    if review_gaps:
        gaps.extend(review_gaps)
    if any(row["classification"] == "worse" for row in comparisons):
        gaps.append("SHADOW_WORSE_CLASSIFICATION")
    if any(row["classification"] in {"unresolved", "pending"} for row in comparisons):
        gaps.append("SHADOW_UNRESOLVED_DIFFERENCE")
    summary_out = {
        "comparison_rows": expected_rows,
        "coverage": coverage,
        "family_counts": counters,
        "outcome_counts": row_counts,
        "differences": differences,
        "diff_count": len(differences),
    }
    return summary_out, differences, gaps


def _validate_source_identity(value: Any, comparison_id: str, side: str) -> Any:
    if value is None:
        return None
    identity = _object(value, f"comparison {comparison_id}.{side}_source", {"stream", "decoded_offset", "row", "operation_index"})
    return {
        "stream": _integer(identity["stream"], "source stream", maximum=(1 << 32) - 1),
        "decoded_offset": _integer(identity["decoded_offset"], "source decoded_offset", maximum=(1 << 64) - 1),
        "row": _integer(identity["row"], "source row", maximum=(1 << 32) - 1),
        "operation_index": _integer(identity["operation_index"], "source operation_index", maximum=65535),
    }


def _validate_exposure(doc: dict[str, Any], unit: dict[str, Any]) -> tuple[list[dict[str, Any]], list[str]]:
    doc = _object(doc, "exposure", {"schema", "version", "function_id", "scope_id", "window_start_ns", "window_end_ns", "records"})
    _check_doc_header(doc, "openatca.m6-exposure", "exposure")
    _bind_doc(doc, unit, "exposure")
    rows = _list(doc.get("records"), "exposure.records", 1000)
    stage_order = {"S0Shadow": 0, "S1Advisory": 1, "S2Window": 2, "S3Extended": 3, "S4Continuous": 4}
    unit_stage = stage_order[unit["stage"]]
    by_mode: dict[str, list[dict[str, Any]]] = {"shadow": [], "live": []}
    intervals_by_mode: dict[str, list[tuple[int, int]]] = {"shadow": [], "live": []}
    seen: set[tuple[str, str, int, int]] = set()
    for index, raw in enumerate(rows):
        row = _object(
            raw,
            f"exposure.records[{index}]",
            {"mode", "stage", "start_ns", "end_ns", "duration_ns", "source_epochs", "traffic"},
        )
        mode = _text(row["mode"], "exposure mode")
        if mode not in by_mode:
            _fail("exposure mode must be shadow or live")
        stage = _text(row["stage"], "exposure stage")
        if stage not in {"S0Shadow", "S1Advisory", "S2Window", "S3Extended", "S4Continuous"}:
            _fail("exposure stage is unknown")
        stage_rank = stage_order[stage]
        if stage_rank > unit_stage or (mode == "shadow" and stage_rank >= 2) or (mode == "live" and stage_rank < 2):
            _fail("exposure record stage is inconsistent with unit stage or exposure mode")
        start = _i64(row["start_ns"], "exposure.start_ns")
        end = _i64(row["end_ns"], "exposure.end_ns")
        duration = _integer(row["duration_ns"], "exposure.duration_ns", maximum=(1 << 64) - 1)
        if start >= end or end - start != duration:
            _fail("exposure interval and duration do not reconcile")
        if start < unit["window_start_ns"] or end > unit["window_end_ns"]:
            _fail("exposure interval is outside the declared swap-unit window")
        epochs = _object(row["source_epochs"], "exposure.source_epochs", {"legacy", "candidate"})
        clean_epochs = {side: _hash(epochs[side], f"exposure.source_epochs.{side}") for side in epochs}
        if clean_epochs != unit["source_epochs"]:
            _fail("exposure source epochs differ from unit")
        if (mode == "shadow" and stage not in {"S0Shadow", "S1Advisory"}) or (mode == "live" and stage == "S0Shadow"):
            _fail("exposure mode and M1 stage disagree")
        traffic = _object(row["traffic"], "exposure.traffic", {"received", "processed", "quarantined", "refused", "unknown"})
        counts = {key: _integer(value, f"exposure.traffic.{key}") for key, value in traffic.items()}
        if counts["received"] != counts["processed"] + counts["quarantined"] + counts["refused"] + counts["unknown"]:
            _fail("exposure input denominator does not reconcile")
        key = (mode, stage, start, end)
        if key in seen:
            _fail("duplicate exposure record")
        seen.add(key)
        intervals_by_mode[mode].append((start, end))
        by_mode[mode].append(
            {
                "mode": mode,
                "stage": stage,
                "start_ns": start,
                "end_ns": end,
                "duration_ns": duration,
                "exposure_seconds": duration // 1_000_000_000,
                "exposure_hours_milli": duration // 3_600_000_000,
                "traffic": counts,
                "source_epochs": clean_epochs,
            }
        )
    gaps: list[str] = []
    for mode, values in by_mode.items():
        intervals = sorted(intervals_by_mode[mode])
        previous_end: int | None = None
        for start, end in intervals:
            if previous_end is not None and start < previous_end:
                _fail(f"{mode} exposure intervals overlap")
            previous_end = end
        if not values:
            gaps.append(f"MISSING_{mode.upper()}_EXPOSURE")
        if values and sum(item["traffic"]["received"] for item in values) == 0:
            gaps.append(f"ZERO_{mode.upper()}_TRAFFIC_DENOMINATOR")
        if values and sum(item["traffic"]["unknown"] for item in values) > 0:
            gaps.append(f"UNKNOWN_{mode.upper()}_INPUT_ACCOUNTING")
    return by_mode["shadow"] + by_mode["live"], gaps


def _validate_containment(doc: dict[str, Any], unit: dict[str, Any]) -> tuple[list[dict[str, Any]], list[str]]:
    doc = _object(doc, "containment", {"schema", "version", "function_id", "scope_id", "window_start_ns", "window_end_ns", "events", "counts"})
    _check_doc_header(doc, "openatca.m6-containment", "containment")
    _bind_doc(doc, unit, "containment")
    records = _list(doc.get("events"), "containment.events", MAX_CONTAINMENT_EVENTS)
    seen: set[str] = set()
    kinds = {"quarantine": 0, "rollback": 0}
    clean = []
    for index, raw in enumerate(records):
        event = _object(raw, f"containment.events[{index}]", {"event_id", "kind", "at_ns", "reason_id", "status", "evidence_sha256"})
        event_id = _text(event["event_id"], "containment.event_id")
        if event_id in seen:
            _fail(f"duplicate containment event {event_id}")
        seen.add(event_id)
        kind = _text(event["kind"], "containment.kind")
        if kind not in kinds:
            _fail(f"unknown containment event kind {kind!r}")
        at_ns = _i64(event["at_ns"], "containment.at_ns")
        if not unit["window_start_ns"] <= at_ns < unit["window_end_ns"]:
            _fail("containment event is outside unit window")
        evidence_hash = event["evidence_sha256"]
        if evidence_hash is not None:
            evidence_hash = _hash(evidence_hash, "containment.evidence_sha256")
        status = _text(event["status"], "containment.status")
        if status not in {"completed", "failed", "unknown", "pending"}:
            _fail("containment status is unknown")
        kinds[kind] += 1
        clean.append(
            {
                "event_id": event_id,
                "kind": kind,
                "at_ns": at_ns,
                "reason_id": _text(event["reason_id"], "containment.reason_id"),
                "status": status,
                "evidence_sha256": evidence_hash,
            }
        )
    declared = _object(doc.get("counts"), "containment.counts", {"quarantine", "rollback"})
    if _integer(declared["quarantine"], "containment.counts.quarantine") != kinds["quarantine"]:
        _fail("quarantine event count does not reconcile")
    if _integer(declared["rollback"], "containment.counts.rollback") != kinds["rollback"]:
        _fail("rollback event count does not reconcile")
    gaps = []
    if any(item["status"] != "completed" for item in clean):
        gaps.append("CONTAINMENT_EVENT_PENDING_OR_FAILED")
    return clean, gaps


def _validate_drills(
    doc: dict[str, Any],
    unit: dict[str, Any],
    artifacts: dict[str, dict[str, Any]],
) -> tuple[list[dict[str, Any]], dict[str, Any], list[str]]:
    doc = _object(doc, "drills", {"schema", "version", "function_id", "scope_id", "window_start_ns", "window_end_ns", "drills"})
    _check_doc_header(doc, "openatca.m6-drills", "drills")
    _bind_doc(doc, unit, "drills")
    rows = _list(doc.get("drills"), "drills", MAX_DRILLS)
    evidence_by_digest = {
        item["declared_sha256"]: item["kind"]
        for item in artifacts.values()
        if item["declared_sha256"] is not None and item["status"] == "verified"
    }
    seen: set[str] = set()
    clean = []
    gaps: list[str] = []
    actual_reported = 0
    synthetic_count = 0
    failing_drills = 0
    max_flip_latency = 0
    max_rollback_latency = 0
    max_flip_duration = 0
    max_rollback_duration = 0
    continuity_counts = {
        "missed_updates": 0,
        "tracks_dropped": 0,
        "data_blocks_lost": 0,
        "active_alerts_lost": 0,
        "alerts_reraised": 0,
        "handoff_states_changed": 0,
        "interface_gaps": 0,
        "interface_duplicates": 0,
        "interface_out_of_order": 0,
    }
    for index, raw in enumerate(rows):
        drill = _object(raw, f"drills[{index}]", {"drill_id", "traffic_kind", "traffic_sha256", "scope_class", "update_interval_ns", "flip", "rollback", "interface"})
        drill_id = _text(drill["drill_id"], "drill_id")
        if drill_id in seen:
            _fail(f"duplicate drill id {drill_id}")
        seen.add(drill_id)
        traffic_kind = _text(drill["traffic_kind"], "drill.traffic_kind")
        if traffic_kind not in {"recorded_peak", "synthetic_fixture", "unknown"}:
            _fail("unknown drill traffic kind")
        traffic_hash = _hash(drill["traffic_sha256"], "drill.traffic_sha256")
        if evidence_by_digest.get(traffic_hash) != "recorded_traffic":
            gaps.append(f"DRILL_TRAFFIC_EVIDENCE_UNRESOLVED:{drill_id}")
        if traffic_kind == "recorded_peak" and evidence_by_digest.get(traffic_hash) == "recorded_traffic":
            actual_reported += 1
        else:
            synthetic_count += int(traffic_kind == "synthetic_fixture")
            gaps.append(f"DRILL_NOT_VERIFIED_RECORDED_TRAFFIC:{drill_id}")
        scope_class = _text(drill["scope_class"], "drill.scope_class")
        if scope_class not in {"terminal", "enroute"}:
            _fail("drill scope_class must be terminal or enroute")
        update_interval = _integer(drill["update_interval_ns"], "drill.update_interval_ns", minimum=1, maximum=MAX_I64)
        if update_interval > (12_000_000_000 if scope_class == "enroute" else 4_800_000_000):
            _fail(f"drill {drill_id}: update interval exceeds the declared H5 class limit")
        phases = {}
        drill_failed = False
        for phase_name in ("flip", "rollback"):
            phase = _object(drill[phase_name], f"drill.{phase_name}", {"decision_to_action_ns", "action_duration_ns", "missed_updates", "tracks_dropped", "data_blocks_lost", "active_alerts_lost", "alerts_reraised", "handoff_states_changed"})
            measured = {key: _integer(value, f"drill.{phase_name}.{key}", maximum=(1 << 64) - 1) for key, value in phase.items()}
            if measured["decision_to_action_ns"] > 1_000_000_000 or measured["action_duration_ns"] > update_interval:
                drill_failed = True
            for key in continuity_counts:
                if key in measured:
                    continuity_counts[key] += measured[key]
            for key in ("missed_updates", "tracks_dropped", "data_blocks_lost", "active_alerts_lost", "alerts_reraised", "handoff_states_changed"):
                if measured[key] != 0:
                    drill_failed = True
            phases[phase_name] = measured
        interface = _object(drill["interface"], f"drill.interface", {"gaps", "duplicates", "out_of_order", "limits_artifact_sha256"})
        iface = {key: _integer(interface[key], f"drill.interface.{key}") for key in ("gaps", "duplicates", "out_of_order")}
        for source_key, target_key in (("gaps", "interface_gaps"), ("duplicates", "interface_duplicates"), ("out_of_order", "interface_out_of_order")):
            continuity_counts[target_key] += iface[source_key]
        limit_hash = interface["limits_artifact_sha256"]
        if limit_hash is None:
            gaps.append(f"H5_INTERFACE_LIMITS_UNAVAILABLE:{drill_id}")
        else:
            limit_hash = _hash(limit_hash, "drill.interface.limits_artifact_sha256")
            if evidence_by_digest.get(limit_hash) not in {"supporting_evidence", "review_evidence"}:
                gaps.append(f"H5_INTERFACE_LIMITS_UNRESOLVED:{drill_id}")
            else:
                gaps.append(f"H5_INTERFACE_LIMITS_NOT_INTERPRETED:{drill_id}")
        if any(iface.values()):
            if limit_hash is None:
                gaps.append(f"H5_INTERFACE_DEVIATIONS_UNASSESSED:{drill_id}")
        if drill_failed:
            failing_drills += 1
            gaps.append(f"H5_DRILL_TIMING_OR_STATE_FAILURE:{drill_id}")
        max_flip_latency = max(max_flip_latency, phases["flip"]["decision_to_action_ns"])
        max_rollback_latency = max(max_rollback_latency, phases["rollback"]["decision_to_action_ns"])
        max_flip_duration = max(max_flip_duration, phases["flip"]["action_duration_ns"])
        max_rollback_duration = max(max_rollback_duration, phases["rollback"]["action_duration_ns"])
        clean.append(
            {
                "drill_id": drill_id,
                "traffic_kind": traffic_kind,
                "traffic_sha256": traffic_hash,
                "traffic_hash_verified": traffic_hash in evidence_by_digest,
                "scope_class": scope_class,
                "update_interval_ns": update_interval,
                "flip": phases["flip"],
                "rollback": phases["rollback"],
                "interface": iface,
                "interface_limits_sha256": limit_hash,
                "h5_timing_and_state_thresholds_met": not drill_failed,
            }
        )
    if actual_reported < 100:
        gaps.append(f"INSUFFICIENT_REPORTED_RECORDED_DRILLS_WITH_HASH_VERIFIED_ARTIFACT:{actual_reported}/100")
    summary = {
        "reported_drills": len(clean),
        "reported_recorded_peak_drills_with_hash_verified_artifact": actual_reported,
        "synthetic_fixture_drills": synthetic_count,
        "failing_drills": failing_drills,
        "max_flip_decision_to_action_ns": max_flip_latency,
        "max_rollback_decision_to_action_ns": max_rollback_latency,
        "max_flip_action_duration_ns": max_flip_duration,
        "max_rollback_action_duration_ns": max_rollback_duration,
        "continuity_totals": continuity_counts,
        "interface_limit_status": "unavailable_or_not_interpreted",
    }
    return clean, summary, gaps


def _required_artifact(artifacts: dict[str, dict[str, Any]], kind: str) -> dict[str, Any] | None:
    matches = [item for item in artifacts.values() if item["kind"] == kind]
    if len(matches) > 1:
        _fail(f"manifest contains multiple {kind} artifacts")
    return matches[0] if matches else None


def _build_package(manifest_path: str) -> dict[str, Any]:
    raw, manifest_hash, manifest_size = _read_file(manifest_path, MAX_MANIFEST_BYTES)
    manifest = _parse_json(raw, manifest_path)
    unit, artifact_list = _validate_manifest(manifest, manifest_path)
    guard = sealed_guard.production()
    if guard.broken or not guard.windows or any(
        unit["window_start_ns"] < window_end and window_start < unit["window_end_ns"]
        for _, window_start, window_end in guard.windows
    ):
        raise sealed_guard.SealedError("swap-unit window intersects a sealed hold-out window")
    artifacts = _artifact_index(artifact_list)
    loaded, artifact_statuses, gaps = _load_artifacts(artifact_list)
    for required_kind in sorted(REQUIRED_ARTIFACT_KINDS):
        if _required_artifact(artifacts, required_kind) is None:
            gaps.append(f"MISSING_ARTIFACT_KIND:{required_kind}")
    for required_kind in REQUIRED_ARTIFACT_KINDS:
        item = _required_artifact(artifacts, required_kind)
        if item is not None and item["id"] not in loaded:
            gaps.append(f"UNAVAILABLE_REQUIRED_ARTIFACT:{required_kind}")
    verified_by_id = {
        status["id"]: status
        for status in artifact_statuses
        if status["status"] == "verified"
    }
    hazards: list[dict[str, Any]] = []
    hazard_source: dict[str, Any] | None = None
    shadow: dict[str, Any] | None = None
    exposure: list[dict[str, Any]] = []
    containment: list[dict[str, Any]] = []
    drills: list[dict[str, Any]] = []
    drill_summary: dict[str, Any] = {
        "reported_drills": 0,
        "reported_recorded_peak_drills_with_hash_verified_artifact": 0,
        "synthetic_fixture_drills": 0,
        "failing_drills": 0,
        "continuity_totals": {},
        "interface_limit_status": "unavailable_or_not_interpreted",
    }
    for kind, validator in (
        ("hazard_register", _validate_hazards),
        ("shadow_comparison", None),
        ("exposure", _validate_exposure),
        ("containment", _validate_containment),
        ("drill_results", None),
    ):
        artifact = _required_artifact(artifacts, kind)
        if artifact is None or artifact["id"] not in loaded:
            continue
        try:
            doc = loaded[artifact["id"]]
            if kind == "hazard_register":
                hazard_list = validator(doc, unit)
                hazards = hazard_list[0]["hazards"]
                if not hazards:
                    gaps.append("HAZARD_REGISTER_EMPTY")
                hazard_source = {key: value for key, value in hazard_list[0].items() if key != "hazards"}
            elif kind == "shadow_comparison":
                shadow, _, artifact_gaps = _validate_shadow(doc, unit, artifacts)
                gaps.extend(artifact_gaps)
            elif kind == "exposure":
                exposure, artifact_gaps = validator(doc, unit)
                gaps.extend(artifact_gaps)
            elif kind == "containment":
                containment, artifact_gaps = validator(doc, unit)
                gaps.extend(artifact_gaps)
            elif kind == "drill_results":
                drills, drill_summary, artifact_gaps = _validate_drills(doc, unit, verified_by_id)
                gaps.extend(artifact_gaps)
        except EvidenceError as exc:
            gaps.append(f"INVALID_ARTIFACT:{kind}:{str(exc)[:256]}")
            if kind == "shadow_comparison":
                shadow = None
            elif kind == "hazard_register":
                hazards = []
            elif kind == "exposure":
                exposure = []
            elif kind == "containment":
                containment = []
            elif kind == "drill_results":
                drills = []
    if any(event["status"] != "completed" for event in containment):
        gaps.append("QUARANTINE_OR_ROLLBACK_STATUS_INCOMPLETE")
    if shadow and shadow["diff_count"]:
        for row in shadow["differences"]:
            if row["classification"] in {"worse", "unresolved", "pending"}:
                gaps.append(f"BLOCKING_SHADOW_DIFF:{row['comparison_id']}:{row['classification']}")
    gaps = sorted(set(gaps))
    package_status = "INCOMPLETE" if gaps else "COMPLETE_FOR_HUMAN_REVIEW"
    return {
        "schema": PACKAGE_SCHEMA,
        "version": PACKAGE_VERSION,
        "assessment_status": package_status,
        "promotion_authority": "NOT_IMPLEMENTED",
        "approval_status": "NOT_APPROVED",
        "promotion_eligible": False,
        "limitations": [
            "This generated package is evidence organization, not an FAA approval, safety case acceptance, or controller authority record.",
            "Input provenance strings and reviewer identities are claims from source artifacts; this tool does not authenticate them.",
            "A recorded-traffic drill classification is a source claim; only the referenced bounded artifact bytes and hash are verified, not that the bytes are genuine traffic or a performed drill.",
            "No unavailable ERAM/SFDPS observations or controller actions are inferred.",
            "Interface allowances are not interpreted; absence of an approved ICD limit source leaves H5 interface continuity unassessed.",
        ],
        "unit": unit,
        "resource_limits": {
            "manifest_bytes": MAX_MANIFEST_BYTES,
            "json_artifact_bytes_each": MAX_JSON_ARTIFACT_BYTES,
            "opaque_artifact_bytes_each": MAX_OPAQUE_ARTIFACT_BYTES,
            "artifact_bytes_total": MAX_TOTAL_ARTIFACT_BYTES,
            "artifact_count": MAX_ARTIFACTS,
            "json_depth": MAX_JSON_DEPTH,
            "json_nodes": MAX_JSON_NODES,
            "hazard_records": MAX_HAZARDS,
            "comparison_rows": MAX_DIFF_ROWS,
            "containment_events": MAX_CONTAINMENT_EVENTS,
            "drill_records": MAX_DRILLS,
            "package_bytes": MAX_OUTPUT_BYTES,
        },
        "h5_rule_limits": {
            "decision_to_action_max_ns": 1_000_000_000,
            "terminal_update_interval_max_ns": 4_800_000_000,
            "enroute_update_interval_max_ns": 12_000_000_000,
            "action_duration_max": "declared update interval",
            "continuity_counts_max": 0,
            "interface_limit_source": "required but not interpreted by this generator",
        },
        "manifest": {"sha256": manifest_hash, "bytes": manifest_size},
        "artifacts": sorted(artifact_statuses, key=lambda item: item["id"]),
        "hazard_register_source": hazard_source,
        "hazards": hazards,
        "exposure": exposure,
        "shadow": shadow,
        "containment_events": containment,
        "containment_counts": {
            "quarantine": sum(event["kind"] == "quarantine" for event in containment),
            "rollback": sum(event["kind"] == "rollback" for event in containment),
        },
        "drills": drills,
        "h5_metrics": drill_summary,
        "gaps": gaps,
    }


def _write_private_output(out_dir: str, package: dict[str, Any]) -> str:
    guard = sealed_guard.production()
    encoder = json.JSONEncoder(sort_keys=True, separators=(",", ":"), ensure_ascii=True, allow_nan=False)
    chunks: list[bytes] = []
    output_bytes = 1
    for encoded in encoder.iterencode(package):
        chunk = encoded.encode("ascii")
        output_bytes = output_bytes + len(chunk)
        if output_bytes > MAX_OUTPUT_BYTES:
            _fail(f"output cap exceeded ({output_bytes} > {MAX_OUTPUT_BYTES})")
        chunks.append(chunk)
    chunks.append(b"\n")
    raw = b"".join(chunks)
    given_out_dir = os.fspath(out_dir)
    absolute_out_dir = os.path.abspath(given_out_dir)
    guard.check(given_out_dir)
    guard.check(absolute_out_dir)
    parent = os.path.dirname(absolute_out_dir) or os.path.sep
    child_name = os.path.basename(absolute_out_dir)
    if not child_name or child_name in {".", ".."}:
        _fail("output directory must name a new child directory")
    if not hasattr(os, "O_NOFOLLOW"):
        _fail("private output requires no-follow open support")
    guard.check(parent)
    directory_flags = os.O_RDONLY | getattr(os, "O_DIRECTORY", 0) | getattr(os, "O_CLOEXEC", 0)
    if not hasattr(os, "O_DIRECTORY"):
        _fail("private output requires directory-file-descriptor support")
    parent_fd = os.open(parent, directory_flags)
    try:
        guard.verify_fd(parent_fd, parent)
        if not stat.S_ISDIR(os.fstat(parent_fd).st_mode):
            _fail("output parent must be a directory")
        _assert_directory_path_is_fd(guard, parent, parent_fd)
        try:
            os.mkdir(child_name, 0o700, dir_fd=parent_fd)
        except FileExistsError as exc:
            raise EvidenceError("output directory already exists; refusing overwrite") from exc
        child_flags = directory_flags | getattr(os, "O_NOFOLLOW", 0)
        child_fd = os.open(child_name, child_flags, dir_fd=parent_fd)
        try:
            guard.verify_fd(child_fd, absolute_out_dir)
            if not stat.S_ISDIR(os.fstat(child_fd).st_mode):
                _fail("created output path is not a directory")
            os.fchmod(child_fd, 0o700)
            _assert_directory_path_is_fd(guard, absolute_out_dir, child_fd)
            out_path = os.path.join(absolute_out_dir, "m6-evidence.json")
            guard.check(out_path)
            file_flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_CLOEXEC", 0)
            file_flags |= getattr(os, "O_NOFOLLOW", 0)
            fd = os.open("m6-evidence.json", file_flags, 0o600, dir_fd=child_fd)
            try:
                guard.verify_fd(fd, out_path)
                if not stat.S_ISREG(os.fstat(fd).st_mode):
                    _fail("created evidence output is not a regular file")
                os.fchmod(fd, 0o600)
                with os.fdopen(fd, "wb", closefd=False) as file_obj:
                    file_obj.write(raw)
                    file_obj.flush()
                    os.fsync(fd)
                guard.verify_fd(fd, out_path)
                _assert_directory_path_is_fd(guard, absolute_out_dir, child_fd)
                _assert_directory_path_is_fd(guard, parent, parent_fd)
                os.fsync(child_fd)
                os.fsync(parent_fd)
            finally:
                os.close(fd)
        finally:
            os.close(child_fd)
    finally:
        os.close(parent_fd)
    return out_path


def _assert_directory_path_is_fd(guard: Any, path: str, fd: int) -> None:
    """Refuse a pathname replaced after the output directory descriptor was opened."""
    guard.check(path)
    expected = os.fstat(fd)
    actual = os.stat(path, follow_symlinks=True)
    if (
        not stat.S_ISDIR(expected.st_mode)
        or not stat.S_ISDIR(actual.st_mode)
        or expected.st_dev != actual.st_dev
        or expected.st_ino != actual.st_ino
    ):
        _fail(f"output directory path changed during creation: {path}")


def generate(manifest_path: str, out_dir: str) -> tuple[str, dict[str, Any]]:
    package = _build_package(manifest_path)
    output = _write_private_output(out_dir, package)
    return output, package


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", required=True, help="bounded internal M6 v1 manifest")
    parser.add_argument("--out-dir", required=True, help="new private directory; must not already exist")
    args = parser.parse_args(argv)
    try:
        output, package = generate(args.manifest, args.out_dir)
    except sealed_guard.SealedError as exc:
        print(f"REFUSED: {exc}", file=sys.stderr)
        return 2
    except (EvidenceError, OSError, ValueError, RecursionError) as exc:
        print(f"INCOMPLETE: {exc}", file=sys.stderr)
        return 1
    print(f"{package['assessment_status']} {output}")
    return 0 if package["assessment_status"] == "COMPLETE_FOR_HUMAN_REVIEW" else 3


if __name__ == "__main__":
    raise SystemExit(main())
