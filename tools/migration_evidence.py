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
MAX_SELECTOR_REPORT_BYTES = 2 * 1024 * 1024
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
    "synthetic_selector_report",
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
    unit = _object(unit, "unit")
    required = {
        "function_id", "scope_id", "comparison_family", "window_start_ns", "window_end_ns",
        "stage", "policy", "source_epochs",
    }
    allowed = required | {"m1_source_epoch_digest"}
    if required - unit.keys() or unit.keys() - allowed:
        _fail(f"unit: schema fields missing={sorted(required - unit.keys())} extra={sorted(unit.keys() - allowed)}")
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
        "m1_source_epoch_digest": (
            _hash(unit["m1_source_epoch_digest"], "unit.m1_source_epoch_digest")
            if "m1_source_epoch_digest" in unit
            else None
        ),
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
            else MAX_SELECTOR_REPORT_BYTES
            if artifact["kind"] == "synthetic_selector_report"
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
            if item["kind"] == "synthetic_selector_report":
                raw, digest, size = _read_file(item["path"], min(MAX_SELECTOR_REPORT_BYTES, remaining_total))
                value = raw.decode("ascii", errors="strict")
            elif item["kind"] in {"review_evidence", "recorded_traffic", "supporting_evidence"}:
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


def _validate_hazards_v1(doc: dict[str, Any], unit: dict[str, Any]) -> list[dict[str, Any]]:
    doc = _object(doc, "hazard register", {"schema", "version", "function_id", "scope_id", "window_start_ns", "window_end_ns", "source", "hazards"})
    _bind_doc(doc, unit, "hazard register")
    source = _object(doc["source"], "hazard register source", {"document_ref", "revision", "coverage_status", "hazard_count"})
    hazards = _list(doc["hazards"], "hazards", MAX_HAZARDS)
    declared_count = _integer(source["hazard_count"], "hazard register source.hazard_count", maximum=MAX_HAZARDS)
    if not hazards or declared_count != len(hazards):
        _fail("legacy hazard register count is empty or does not reconcile")
    ids: set[str] = set()
    clean = []
    for index, raw in enumerate(hazards):
        hazard = _object(raw, f"hazards[{index}]", {"id", "description", "mitigations"})
        hazard_id = _text(hazard["id"], f"hazards[{index}].id")
        if hazard_id in ids:
            _fail(f"duplicate hazard id {hazard_id}")
        ids.add(hazard_id)
        mitigations = _list(hazard["mitigations"], f"hazards[{index}].mitigations", 64)
        if not mitigations:
            _fail(f"hazard {hazard_id} has no listed mitigation")
        clean_mitigations = []
        requirement_ids = []
        for mitigation_index, item in enumerate(mitigations):
            mitigation = _object(item, f"hazards[{index}].mitigations[{mitigation_index}]", {"id", "description", "verification_ref"})
            requirement = _text(mitigation["id"], "mitigation.id")
            requirement_ids.append(requirement)
            clean_mitigations.append(
                {
                    "id": requirement,
                    "description": _text(mitigation["description"], "mitigation.description"),
                    "verification_ref": _text(mitigation["verification_ref"], "mitigation.verification_ref"),
                }
            )
        if len(set(requirement_ids)) != len(requirement_ids):
            _fail(f"hazard {hazard_id} has duplicate mitigation IDs")
        clean.append(
            {
                "id": hazard_id,
                "function": None,
                "cause": None,
                "effect": None,
                "severity_class": "Unassessed",
                "severity_status": "unassessed",
                "severity_rationale": "Legacy hazard register version has no function-specific severity rationale.",
                "source_refs": [],
                "mitigation_requirement_ids": sorted(requirement_ids),
                "verification_refs": [],
                "description": _text(hazard["description"], f"hazards[{index}].description"),
                "mitigations": clean_mitigations,
            }
        )
    return [
        {
            "source_document_ref": _text(source["document_ref"], "hazard source.document_ref"),
            "source_revision": _text(source["revision"], "hazard source.revision"),
            "coverage_status": _text(source["coverage_status"], "hazard source.coverage_status"),
            "declared_hazard_count": declared_count,
            "function_id": unit["function_id"],
            "function_name": None,
            "scope_id": unit["scope_id"],
            "coverage_gap": "FUNCTION_HAZARD_DETAIL_NOT_PROVIDED",
            "hazards": clean,
        }
    ]


def _validate_hazards(doc: dict[str, Any], unit: dict[str, Any]) -> list[dict[str, Any]]:
    if _text(doc.get("schema"), "hazard register.schema") != "openatca.m6-hazard-register":
        _fail("unsupported hazard register schema")
    version = _integer(doc.get("version"), "hazard register.version", maximum=65535)
    if version == 1:
        return _validate_hazards_v1(doc, unit)
    if version != 2:
        _fail("unsupported hazard register schema or version")
    doc = _object(doc, "hazard register", {"schema", "version", "function_id", "scope_id", "window_start_ns", "window_end_ns", "source", "hazards"})
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
    if not hazards:
        _fail("hazard register must contain at least one function hazard")
    ids: set[str] = set()
    result: list[dict[str, Any]] = []
    for index, raw in enumerate(hazards):
        hazard = _object(
            raw,
            f"hazards[{index}]",
            {
                "id", "function", "cause", "effect", "severity_class", "severity_status",
                "severity_rationale", "source_refs", "mitigation_requirement_ids",
                "verification_refs", "description", "mitigations",
            },
        )
        hazard_id = _text(hazard["id"], f"hazards[{index}].id")
        if hazard_id in ids:
            _fail(f"duplicate hazard id {hazard_id}")
        ids.add(hazard_id)
        mitigations = _list(hazard["mitigations"], f"hazards[{index}].mitigations", 64)
        if not mitigations:
            _fail(f"hazard {hazard_id} has no listed mitigation")
        severity_class = _text(hazard["severity_class"], f"hazards[{index}].severity_class")
        if severity_class not in {"Catastrophic", "Hazardous", "Major", "Minor", "NoEffect", "Unassessed"}:
            _fail(f"hazard {hazard_id} has an unknown severity class")
        severity_status = _text(hazard["severity_status"], f"hazards[{index}].severity_status")
        if severity_status not in {"engineering_candidate", "unassessed"}:
            _fail(f"hazard {hazard_id} has an unknown severity status")
        if (severity_class == "Unassessed") != (severity_status == "unassessed"):
            _fail(f"hazard {hazard_id} severity class and status disagree")
        requirement_ids = _list(hazard["mitigation_requirement_ids"], f"hazards[{index}].mitigation_requirement_ids", 128)
        clean_requirement_ids = sorted({_text(value, "mitigation requirement id") for value in requirement_ids})
        if len(clean_requirement_ids) != len(requirement_ids):
            _fail(f"hazard {hazard_id} has duplicate mitigation requirement IDs")
        source_refs = _list(hazard["source_refs"], f"hazards[{index}].source_refs", 128)
        clean_source_refs = [_text(value, "hazard source reference") for value in source_refs]
        verification_refs = _list(hazard["verification_refs"], f"hazards[{index}].verification_refs", 128)
        clean_verification_refs = [_text(value, "hazard verification reference") for value in verification_refs]
        if not clean_source_refs or not clean_verification_refs:
            _fail(f"hazard {hazard_id} needs source and verification references")
        mitigation_ids: set[str] = set()
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
            mitigation_ids.add(mitigation_id)
            clean_mitigations.append(
                {
                    "id": mitigation_id,
                    "description": _text(mitigation["description"], "mitigation.description"),
                    "verification_ref": _text(mitigation["verification_ref"], "mitigation.verification_ref"),
                }
            )
        if mitigation_ids != set(clean_requirement_ids):
            _fail(f"hazard {hazard_id}: requirement IDs differ from mitigation records")
        result.append(
            {
                "id": hazard_id,
                "function": _text(hazard["function"], f"hazards[{index}].function"),
                "cause": _text(hazard["cause"], f"hazards[{index}].cause"),
                "effect": _text(hazard["effect"], f"hazards[{index}].effect"),
                "severity_class": severity_class,
                "severity_status": severity_status,
                "severity_rationale": _text(hazard["severity_rationale"], f"hazards[{index}].severity_rationale"),
                "source_refs": clean_source_refs,
                "mitigation_requirement_ids": clean_requirement_ids,
                "verification_refs": clean_verification_refs,
                "description": _text(hazard["description"], f"hazards[{index}].description"),
                "mitigations": clean_mitigations,
            }
        )
    if coverage_status not in {"complete_for_function", "declared_function_scope", "unassessed"} or declared_hazard_count != len(result):
        _fail("hazard register coverage status or declared hazard count does not reconcile")
    function_names = {item["function"] for item in result}
    if len(function_names) != 1:
        _fail("hazard register must bind hazards to exactly one function for this unit")
    function_name = next(iter(function_names))
    if coverage_status != "complete_for_function":
        # A bounded list of hazards is not evidence that all function hazards were discovered.
        result_coverage_gap = "FUNCTION_HAZARD_SCOPE_NOT_ASSESSED"
    else:
        result_coverage_gap = ""
    return [
        {
            "source_document_ref": source_ref,
            "source_revision": source_revision,
            "coverage_status": coverage_status,
            "declared_hazard_count": declared_hazard_count,
            "function_id": unit["function_id"],
            "function_name": function_name,
            "scope_id": unit["scope_id"],
            "coverage_gap": result_coverage_gap,
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
    classification_counts: dict[str, int] = {}
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
        classification_counts[classification] = classification_counts.get(classification, 0) + 1
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
        "classification_counts": dict(sorted(classification_counts.items())),
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


def _validate_exposure(doc: dict[str, Any], unit: dict[str, Any]) -> tuple[list[dict[str, Any]], list[str], dict[str, dict[str, Any]]]:
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
    records = by_mode["shadow"] + by_mode["live"]
    totals: dict[str, dict[str, Any]] = {}
    for mode, mode_records in by_mode.items():
        duration = sum(item["duration_ns"] for item in mode_records)
        traffic = {
            key: sum(item["traffic"][key] for item in mode_records)
            for key in ("received", "processed", "quarantined", "refused", "unknown")
        }
        if any(value > MAX_I64 for value in traffic.values()) or duration > (1 << 64) - 1:
            _fail(f"{mode} exposure aggregate exceeds its integer bound")
        totals[mode] = {
            "interval_count": len(mode_records),
            "duration_ns": duration,
            "exposure_hours_milli": duration // 3_600_000_000,
            "traffic": traffic,
        }
    return records, gaps, totals


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
    if not clean:
        gaps.append("NO_CONTAINMENT_EVENT_RECORDS_SUBMITTED")
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


def _parse_m3_selector_report(text: str, unit: dict[str, Any], source_sha256: str, source_bytes: int) -> tuple[dict[str, Any], list[str]]:
    """Parse the bounded synthetic M3 selector receipt emitted by its existing example."""
    if unit["comparison_family"] != "alert" or unit["stage"] != "S2Window":
        _fail("synthetic conflict-alert selector report requires an alert-family S2Window unit")
    if len(text.encode("ascii", errors="strict")) > MAX_SELECTOR_REPORT_BYTES:
        _fail("synthetic selector report exceeds its byte cap")
    selected_lines = [
        line for line in text.splitlines()
        if line.startswith(("M3DRILL\t", "CYCLE_FIELDS\t", "CYCLE\t", "CYCLE_FAILED\t", "FAULT\t", "SUMMARY\t"))
    ]
    if any(len(line) > 16_384 or line.count("\t") > 128 for line in selected_lines):
        _fail("synthetic selector report line field cap exceeded")
    headers = [line for line in selected_lines if line.startswith("M3DRILL\t")]
    columns = [line for line in selected_lines if line.startswith("CYCLE_FIELDS\t")]
    fault_lines = [line for line in selected_lines if line.startswith("FAULT\t")]
    summary_lines = [line for line in selected_lines if line.startswith("SUMMARY\t")]
    if len(headers) != 1 or len(columns) != 1 or len(fault_lines) > 1 or len(summary_lines) != 1:
        _fail("synthetic selector report needs one header, columns and total summary, plus at most one fault summary")

    def key_values(line: str, label: str, expected_keys: set[str] | None = None) -> dict[str, str]:
        values: dict[str, str] = {}
        for token in line.split("\t")[1:]:
            key, separator, value = token.partition("=")
            if not separator or not key or key in values:
                _fail(f"{label}: malformed or duplicate field")
            values[key] = value
        if expected_keys is not None and values.keys() != expected_keys:
            _fail(f"{label}: field set differs from the supported report schema")
        return values

    def number(values: dict[str, str], key: str, label: str, maximum: int = MAX_I64) -> int:
        raw = values.get(key)
        if raw is None or not raw.isascii() or not raw.isdecimal():
            _fail(f"{label}.{key}: expected an unsigned decimal integer")
        return _integer(int(raw), f"{label}.{key}", maximum=maximum)

    header = key_values(
        headers[0],
        "M3DRILL",
        {
            "version", "mode", "recorded_mode", "qualification", "function_id", "function_name", "scope_id",
            "scope_name", "evidence_policy_id", "m1_policy_version", "m1_policy_digest", "m1_source_epoch_digest",
            "legacy_source_epoch", "candidate_source_epoch", "cycles", "cycle_window_ns", "cycle_step_ns", "seed",
            "tracks_per_side", "flights_per_side", "handoffs_per_side", "alerts_per_side", "terminal_interval_ns",
            "max_decision_delay_ns", "modeled_delivery_delay_ns", "delivery_timing_scope", "consumer_wire_adapter",
            "quarantine_exercised", "ca_detection_exercised", "alert_payload_preservation_exercised",
        },
    )
    if header.get("version") != "1" or header.get("mode") != "synthetic_capacity":
        _fail("unsupported M3 selector report version or mode")
    if header.get("recorded_mode") != "NOT_BUILT" or header.get("qualification") != "INCOMPLETE":
        _fail("synthetic selector report must retain its non-operational qualification status")
    if header.get("delivery_timing_scope") != "selector_api_only" or header.get("consumer_wire_adapter") != "NOT_BUILT":
        _fail("synthetic selector report has an unknown timing or consumer-interface scope")
    if number(header, "function_id", "M3DRILL", maximum=(1 << 32) - 1) != unit["function_id"]:
        _fail("synthetic selector report function_id differs from the unit")
    if number(header, "scope_id", "M3DRILL", maximum=(1 << 32) - 1) != unit["scope_id"]:
        _fail("synthetic selector report scope_id differs from the unit")
    function_name = _text(header.get("function_name"), "M3DRILL.function_name")
    scope_name = _text(header.get("scope_name"), "M3DRILL.scope_name")
    if function_name != "conflict_alert" or scope_name != "ZAU26":
        _fail("synthetic selector report is not bound to the declared conflict-alert/ZAU26 fixture")
    evidence_policy_id = _text(header.get("evidence_policy_id"), "M3DRILL.evidence_policy_id")
    m1_policy_version = number(header, "m1_policy_version", "M3DRILL", maximum=65535)
    m1_policy_digest = _hash(header.get("m1_policy_digest"), "M3DRILL.m1_policy_digest")
    if evidence_policy_id != unit["policy"]["id"] or m1_policy_version != unit["policy"]["version"] or m1_policy_digest != unit["policy"]["sha256"]:
        _fail("synthetic selector report policy identity differs from the unit")
    source_epoch_digest = _hash(header.get("m1_source_epoch_digest"), "M3DRILL.m1_source_epoch_digest")
    if unit["m1_source_epoch_digest"] is None or source_epoch_digest != unit["m1_source_epoch_digest"]:
        _fail("synthetic selector report source epoch digest is missing or differs from the unit")
    source_epoch_ids = {
        "legacy": number(header, "legacy_source_epoch", "M3DRILL", maximum=(1 << 32) - 1),
        "candidate": number(header, "candidate_source_epoch", "M3DRILL", maximum=(1 << 32) - 1),
    }
    cycles_expected = number(header, "cycles", "M3DRILL", maximum=MAX_DRILLS)
    cycle_window_ns = number(header, "cycle_window_ns", "M3DRILL")
    cycle_step_ns = number(header, "cycle_step_ns", "M3DRILL")
    payload_counts = {
        name: number(header, f"{name}_per_side", "M3DRILL")
        for name in ("tracks", "flights", "handoffs", "alerts")
    }
    modeled_delivery_delay_ns = number(header, "modeled_delivery_delay_ns", "M3DRILL")
    if cycles_expected == 0 or cycle_window_ns == 0 or cycle_step_ns < cycle_window_ns:
        _fail("synthetic selector cycle window declaration is invalid")
    if header.get("quarantine_exercised") != "0" or header.get("ca_detection_exercised") != "0" or header.get("alert_payload_preservation_exercised") != "1":
        _fail("synthetic selector report must disclose unexercised quarantine/detection and alert preservation")

    expected_columns = [
        "index", "start_ns", "end_ns", "ready_ns", "promotion_ns", "rollback_ns",
        "candidate_delivery_api_ns", "legacy_delivery_api_ns", "delivery_delay_ns", "tracks",
        "flights", "handoffs", "alerts", "differences", "output_sequences", "losses", "error",
    ]
    declared_columns = columns[0].split("\t")[1:]
    if declared_columns != expected_columns:
        _fail("synthetic selector report cycle columns are unsupported")
    cycles: list[dict[str, Any]] = []
    failed_cycles: list[dict[str, Any]] = []
    seen_indices: set[int] = set()
    ordered_indices: list[int] = []
    for line in selected_lines:
        if line.startswith("CYCLE\t"):
            values = line.split("\t")[1:]
            if len(values) != len(expected_columns):
                _fail("synthetic selector cycle has the wrong field count")
            raw = dict(zip(expected_columns, values, strict=True))
            index = number({"index": raw["index"]}, "index", "CYCLE", maximum=MAX_DRILLS)
            if index in seen_indices:
                _fail("synthetic selector report repeats a cycle index")
            seen_indices.add(index)
            ordered_indices.append(index)
            start_ns = _i64(number({"start_ns": raw["start_ns"]}, "start_ns", "CYCLE"), "CYCLE.start_ns")
            end_ns = _i64(number({"end_ns": raw["end_ns"]}, "end_ns", "CYCLE"), "CYCLE.end_ns")
            if end_ns - start_ns != cycle_window_ns or start_ns < unit["window_start_ns"] or end_ns > unit["window_end_ns"]:
                _fail("synthetic selector cycle window is outside the declared unit")
            expected_start = unit["window_start_ns"] + index * cycle_step_ns
            if start_ns != expected_start:
                _fail("synthetic selector cycle start does not match its declared step")
            sequences = raw["output_sequences"].split(",")
            if not sequences or any(not value.isascii() or not value.isdecimal() for value in sequences):
                _fail("synthetic selector output sequence list is malformed")
            if raw["error"] != "NONE":
                _fail("successful synthetic selector cycle carries an error")
            cycles.append(
                {
                    "index": index,
                    "start_ns": start_ns,
                    "end_ns": end_ns,
                    "ready_ns": number({"ready_ns": raw["ready_ns"]}, "ready_ns", "CYCLE"),
                    "promotion_ns": number({"promotion_ns": raw["promotion_ns"]}, "promotion_ns", "CYCLE"),
                    "rollback_ns": number({"rollback_ns": raw["rollback_ns"]}, "rollback_ns", "CYCLE"),
                    "candidate_delivery_api_ns": number({"candidate_delivery_api_ns": raw["candidate_delivery_api_ns"]}, "candidate_delivery_api_ns", "CYCLE"),
                    "legacy_delivery_api_ns": number({"legacy_delivery_api_ns": raw["legacy_delivery_api_ns"]}, "legacy_delivery_api_ns", "CYCLE"),
                    "delivery_delay_ns": number({"delivery_delay_ns": raw["delivery_delay_ns"]}, "delivery_delay_ns", "CYCLE"),
                    "tracks": number({"tracks": raw["tracks"]}, "tracks", "CYCLE"),
                    "flights": number({"flights": raw["flights"]}, "flights", "CYCLE"),
                    "handoffs": number({"handoffs": raw["handoffs"]}, "handoffs", "CYCLE"),
                    "alerts": number({"alerts": raw["alerts"]}, "alerts", "CYCLE"),
                    "differences": number({"differences": raw["differences"]}, "differences", "CYCLE"),
                    "output_sequences": [int(value) for value in sequences],
                    "losses": number({"losses": raw["losses"]}, "losses", "CYCLE"),
                }
            )
        elif line.startswith("CYCLE_FAILED\t"):
            parts = line.split("\t")
            if len(parts) != 4:
                _fail("failed synthetic selector cycle has malformed fields")
            index = number({"index": parts[1]}, "index", "CYCLE_FAILED", maximum=MAX_DRILLS)
            values = key_values("FAILED\t" + "\t".join(parts[2:]), "CYCLE_FAILED")
            if index in seen_indices or not values.get("error"):
                _fail("failed synthetic selector cycle duplicates an index or omits its refusal")
            seen_indices.add(index)
            ordered_indices.append(index)
            start_ns = _i64(number(values, "start_ns", "CYCLE_FAILED"), "CYCLE_FAILED.start_ns")
            end_ns = _i64(number(values, "end_ns", "CYCLE_FAILED"), "CYCLE_FAILED.end_ns")
            expected_start = unit["window_start_ns"] + index * cycle_step_ns
            if end_ns - start_ns != cycle_window_ns or start_ns != expected_start or end_ns > unit["window_end_ns"]:
                _fail("failed synthetic selector cycle window does not match its declared unit/step")
            failed_cycles.append({"index": index, "start_ns": start_ns, "end_ns": end_ns, "error": values["error"]})
    summary = key_values(
        summary_lines[0],
        "SUMMARY",
        {
            "cycles_expected", "cycles_completed", "cycles_failed", "clean_cycle_delivery_expected",
            "clean_cycle_delivery_observed", "clean_cycle_lost", "fault_cases", "sustained_same_selector_cycles",
            "sustained_trace_capacity", "sustained_next_promotion", "sustained_fallback", "clean_cycle_consumer_gaps",
            "clean_cycle_consumer_duplicates", "clean_cycle_consumer_out_of_order", "active_alerts_preserved",
            "ack_suppression_fields_compared", "handoff_rows_compared", "decision_and_delivery_timings_separate",
            "recorded_replay", "qualification",
        },
    )
    if number(summary, "cycles_expected", "SUMMARY", maximum=MAX_DRILLS) != cycles_expected:
        _fail("synthetic selector summary expected-cycle count differs from header")
    if number(summary, "cycles_completed", "SUMMARY", maximum=MAX_DRILLS) != len(cycles):
        _fail("synthetic selector summary completion count differs from cycle rows")
    if number(summary, "cycles_failed", "SUMMARY", maximum=MAX_DRILLS) != len(failed_cycles):
        _fail("synthetic selector summary failure count differs from refused rows")
    if len(seen_indices) != cycles_expected or seen_indices != set(range(cycles_expected)):
        _fail("synthetic selector report does not account for every expected cycle index")
    if ordered_indices != list(range(cycles_expected)):
        _fail("synthetic selector cycle rows are not in index order")
    fault = key_values(
        fault_lines[0],
        "FAULT",
        {
            "version", "cases", "expected_refusals", "expected_delivery_gap_refusals", "expected_delivery_duplicates",
            "expected_out_of_order", "epoch_change_after_reconcile", "duplicate_delivery", "sequence_gap",
            "stale_legacy_health", "promotion_pending_rollback", "pending_fault_delivery_offered",
            "pending_fault_delivery_delivered", "pending_fault_delivery_lost", "expired_mirror", "incomplete_family",
            "fault_injections", "losses_preserved",
        },
    ) if fault_lines else {}
    if fault_lines and (fault.get("version") != "1" or fault.get("fault_injections") != "synthetic_only"):
        _fail("synthetic selector fault summary version or provenance is unsupported")
    if not fault_lines and not failed_cycles:
        _fail("successful synthetic selector report is missing its fault suite summary")
    if not fault_lines and failed_cycles:
        gaps_for_fault = ["M3_SYNTHETIC_FAULT_SUITE_NOT_RUN_AFTER_CYCLE_FAILURE"]
    else:
        gaps_for_fault = []
    expected_deliveries = cycles_expected * 2
    observed_deliveries = sum(len(item["output_sequences"]) for item in cycles)
    if number(summary, "clean_cycle_delivery_expected", "SUMMARY") != expected_deliveries:
        _fail("synthetic selector expected-delivery count does not reconcile")
    if not failed_cycles and number(summary, "clean_cycle_delivery_observed", "SUMMARY") != observed_deliveries:
        _fail("synthetic selector observed-delivery count does not reconcile")
    if not failed_cycles and number(summary, "clean_cycle_lost", "SUMMARY") != sum(item["losses"] for item in cycles):
        _fail("synthetic selector lost-delivery count does not reconcile")
    fault_count = number(summary, "fault_cases", "SUMMARY")
    if fault and fault_count != number(fault, "cases", "FAULT"):
        _fail("synthetic selector fault case count does not reconcile")
    if not fault and fault_count != 0:
        _fail("synthetic selector fault case count does not reconcile")
    if summary.get("recorded_replay") != "NOT_BUILT" or summary.get("qualification") != "INCOMPLETE":
        _fail("synthetic selector total summary changed its qualification disclosure")
    if number(summary, "sustained_same_selector_cycles", "SUMMARY") != 64:
        _fail("synthetic selector sustained-cycle count differs from its defined fixture")
    if number(summary, "handoff_rows_compared", "SUMMARY") != payload_counts["handoffs"]:
        _fail("synthetic selector handoff comparison count does not reconcile")
    run_quality_gaps: list[str] = []
    if any(number(summary, key, "SUMMARY") > 0 for key in (
        "clean_cycle_consumer_gaps", "clean_cycle_consumer_duplicates", "clean_cycle_consumer_out_of_order"
    )):
        run_quality_gaps.append("M3_SYNTHETIC_CONSUMER_SEQUENCE_ANOMALY")
    if number(summary, "active_alerts_preserved", "SUMMARY") != 1:
        run_quality_gaps.append("M3_SYNTHETIC_ALERT_PAYLOAD_NOT_PRESERVED")
    if number(summary, "ack_suppression_fields_compared", "SUMMARY") != 1:
        run_quality_gaps.append("M3_SYNTHETIC_ACK_SUPPRESSION_NOT_COMPARED")
    if summary.get("sustained_next_promotion") != "TraceFull" or summary.get("sustained_fallback") != "Legacy":
        run_quality_gaps.append("M3_SYNTHETIC_TRACE_CAPACITY_FALLBACK_MISMATCH")
    cycle_exposure_ns = sum(item["end_ns"] - item["start_ns"] for item in cycles)
    difference_count = sum(item["differences"] for item in cycles)
    gaps = ["SYNTHETIC_SELECTOR_ONLY_NOT_OPERATIONALLY_QUALIFIED", "M3_QUARANTINE_NOT_EXERCISED", "CA_DETECTION_NOT_EXERCISED"]
    gaps.extend(gaps_for_fault)
    gaps.extend(run_quality_gaps)
    if failed_cycles or number(summary, "cycles_failed", "SUMMARY", maximum=MAX_DRILLS) > 0:
        gaps.append("M3_SYNTHETIC_SELECTOR_CYCLES_FAILED")
    for cycle in cycles:
        for name, expected in payload_counts.items():
            if cycle[name] != expected:
                _fail(f"synthetic selector cycle {name} count differs from header capacity")
        if cycle["delivery_delay_ns"] != modeled_delivery_delay_ns:
            _fail("synthetic selector cycle delivery delay differs from header")
        if cycle["output_sequences"] != [1, 2] or cycle["losses"]:
            gaps.append("M3_SYNTHETIC_SELECTOR_SEQUENCE_OR_LOSS_ANOMALY")
    if difference_count or number(summary, "clean_cycle_lost", "SUMMARY") > 0 or (failed_cycles and observed_deliveries < expected_deliveries):
        gaps.append("M3_SYNTHETIC_SELECTOR_DIFF_OR_DELIVERY_LOSS")
    return (
        {
            "schema": "openatca.m6-synthetic-selector-result",
            "version": 1,
            "source_sha256": source_sha256,
            "source_bytes": source_bytes,
            "function_id": unit["function_id"],
            "function_name": function_name,
            "scope_id": unit["scope_id"],
            "scope_name": scope_name,
            "policy": {"id": evidence_policy_id, "version": m1_policy_version, "sha256": m1_policy_digest},
            "policy_binding_note": "Evidence policy ID is a scenario label; M1 policy version and digest are the values from the selector policy.",
            "m1_source_epoch_digest": source_epoch_digest,
            "m3_side_source_epoch_ids": source_epoch_ids,
            "m6_side_source_epoch_hashes": dict(unit["source_epochs"]),
            "source_epoch_scope_note": "M1 pair digest and M3 synthetic side IDs are recorded separately from M6 unit source hashes.",
            "qualification": "SYNTHETIC_SELECTOR_ONLY",
            "timing_scope": "selector_api_only",
            "cycle_window_ns": cycle_window_ns,
            "cycle_step_ns": cycle_step_ns,
            "cycles": cycles,
            "failed_cycles": failed_cycles,
            "cycle_count": len(cycles),
            "synthetic_exposure": {
                "unit_windows": len(cycles),
                "duration_ns": cycle_exposure_ns,
                "exposure_hours_milli": cycle_exposure_ns // 3_600_000_000,
            },
            "comparison_summary": {
                "cycles_compared": len(cycles),
                "cycles_with_differences": sum(item["differences"] > 0 for item in cycles),
                "difference_count": difference_count,
            },
            "rollback_summary": {
                "cycles_with_rollback": len(cycles),
                "max_rollback_selector_ns": max((item["rollback_ns"] for item in cycles), default=0),
            },
            "fault_summary": fault,
            "run_summary": summary,
            "containment_status": "not_exercised",
            "ca_detection_status": "not_exercised",
        },
        gaps,
    )


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
    exposure_totals: dict[str, dict[str, Any]] = {}
    containment: list[dict[str, Any]] = []
    synthetic_selector: dict[str, Any] | None = None
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
                if hazard_source["coverage_gap"]:
                    gaps.append(hazard_source["coverage_gap"])
            elif kind == "shadow_comparison":
                shadow, _, artifact_gaps = _validate_shadow(doc, unit, artifacts)
                gaps.extend(artifact_gaps)
            elif kind == "exposure":
                exposure, artifact_gaps, exposure_totals = validator(doc, unit)
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
                exposure_totals = {}
            elif kind == "containment":
                containment = []
            elif kind == "drill_results":
                drills = []
    selector_artifact = _required_artifact(artifacts, "synthetic_selector_report")
    if selector_artifact is not None and selector_artifact["id"] in loaded:
        try:
            selector_status = verified_by_id.get(selector_artifact["id"], {})
            synthetic_selector, selector_gaps = _parse_m3_selector_report(
                loaded[selector_artifact["id"]],
                unit,
                selector_status.get("actual_sha256") or selector_artifact["declared_sha256"],
                selector_status.get("actual_bytes") or selector_artifact["declared_bytes"],
            )
            gaps.extend(selector_gaps)
            if hazard_source is not None and hazard_source["function_name"] is not None and hazard_source["function_name"] != synthetic_selector["function_name"]:
                gaps.append("SYNTHETIC_SELECTOR_FUNCTION_NAME_DIFFERS_FROM_HAZARD_SCOPE")
        except EvidenceError as exc:
            gaps.append(f"INVALID_ARTIFACT:synthetic_selector_report:{str(exc)[:256]}")
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
            "Containment counts are counts of submitted event rows only; no rows do not prove that no quarantine or rollback occurred.",
        ],
        "unit": unit,
        "resource_limits": {
            "manifest_bytes": MAX_MANIFEST_BYTES,
            "json_artifact_bytes_each": MAX_JSON_ARTIFACT_BYTES,
            "synthetic_selector_report_bytes": MAX_SELECTOR_REPORT_BYTES,
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
        "function_hazards": {
            "function_id": unit["function_id"],
            "function_name": None if hazard_source is None else hazard_source["function_name"],
            "scope_id": unit["scope_id"],
            "records": hazards,
            "coverage_status": None if hazard_source is None else hazard_source["coverage_status"],
        },
        "exposure": exposure,
        "exposure_totals": exposure_totals,
        "synthetic_selector_drill": synthetic_selector,
        "shadow": shadow,
        "containment_events": containment,
        "containment_event_coverage_status": "event_rows_only_completeness_unassessed",
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
