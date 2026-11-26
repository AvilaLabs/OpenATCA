#!/usr/bin/env python3
"""Validate the pinned HIGH OSCAL inventory and write the deterministic status summary."""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import sys
from collections import Counter
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
SOURCE_PATH = Path("docs/security/oscal/NIST_SP-800-53_rev5_HIGH-baseline-resolved-profile_catalog.json")
MAP_PATH = Path("docs/security/control-map.json")
MARKDOWN_PATH = Path("docs/security/generated/control-status.md")
CSV_PATH = Path("docs/security/generated/control-status.csv")
SOURCE_SHA256 = "e2c1a3dbc1d003ce2aea7e70f64527b17fea3a032b972a5fcb8645d1a75f9ed0"
SOURCE_BYTES = 4_637_935
SOURCE_COMMIT = "78650f02ad9321bb7b817846f8fbd4f2bcd620de"
SOURCE_URL = (
    "https://raw.githubusercontent.com/usnistgov/oscal-content/"
    f"{SOURCE_COMMIT}/nist.gov/SP800-53/rev5/json/"
    "NIST_SP-800-53_rev5_HIGH-baseline-resolved-profile_catalog.json"
)
MAX_SOURCE_BYTES = 8 * 1024 * 1024
MAX_MAP_BYTES = 2 * 1024 * 1024
STATUSES = ("implemented", "partial", "planned", "inherited")


def strict_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, item in pairs:
        if key in value:
            raise ValueError(f"duplicate JSON key: {key}")
        value[key] = item
    return value


def read_bounded(path: Path, maximum: int) -> bytes:
    if path.is_symlink():
        raise ValueError(f"refusing symlink input: {path}")
    size = path.stat().st_size
    if size < 0 or size > maximum:
        raise ValueError(f"input size outside 0..{maximum} bytes: {path}")
    data = path.read_bytes()
    if len(data) != size:
        raise ValueError(f"input length changed while reading: {path}")
    return data


def source_inventory(data: dict[str, Any]) -> list[dict[str, str]]:
    catalog = data.get("catalog")
    if not isinstance(catalog, dict):
        raise ValueError("OSCAL source is missing catalog")
    metadata = catalog.get("metadata")
    if not isinstance(metadata, dict) or metadata.get("version") != "5.2.0":
        raise ValueError("unexpected NIST catalog metadata version")
    if metadata.get("oscal-version") != "1.2.2":
        raise ValueError("unexpected OSCAL metadata version")
    if not isinstance(metadata.get("title"), str) or "HIGH IMPACT BASELINE" not in metadata["title"]:
        raise ValueError("source is not the recorded HIGH resolved profile")
    groups = catalog.get("groups")
    if not isinstance(groups, list) or not groups:
        raise ValueError("resolved catalog has no control groups")
    inventory: list[dict[str, str]] = []
    seen_oscal_ids: set[str] = set()

    def visit(control: Any, family: str) -> None:
        if not isinstance(control, dict):
            raise ValueError("control entry is not an object")
        oscal_id = control.get("id")
        title = control.get("title")
        if not isinstance(oscal_id, str) or not isinstance(title, str) or not title:
            raise ValueError("control entry is missing id or title")
        if oscal_id in seen_oscal_ids:
            raise ValueError(f"duplicate OSCAL control id in source: {oscal_id!r}")
        seen_oscal_ids.add(oscal_id)
        if not oscal_id.startswith(f"{family}-"):
            raise ValueError(f"control {oscal_id!r} does not match family {family!r}")
        number = oscal_id[len(family) + 1 :]
        control_number, dot, enhancement = number.partition(".")
        reference = f"{family.upper()}-{control_number}"
        if dot:
            if not enhancement.isascii() or not enhancement.isdigit():
                raise ValueError(f"invalid enhancement id: {oscal_id}")
            reference += f"({enhancement})"
        params = control.get("params", [])
        if not isinstance(params, list) or any(
            not isinstance(param, dict) or not isinstance(param.get("id"), str)
            for param in params
        ):
            raise ValueError(f"invalid parameters for {oscal_id}")
        inventory.append(
            {
                "control_id": reference,
                "oscal_id": oscal_id,
                "family": family.upper(),
                "title": title,
                "parameter_ids": [param["id"] for param in params],
            }
        )
        children = control.get("controls", [])
        if not isinstance(children, list):
            raise ValueError(f"invalid nested controls for {oscal_id}")
        for child in children:
            visit(child, family)

    seen_families: set[str] = set()
    for group in groups:
        if not isinstance(group, dict):
            raise ValueError("control group is not an object")
        family = group.get("id")
        controls = group.get("controls")
        if not isinstance(family, str) or not family.isascii() or not family.isalpha():
            raise ValueError("invalid control family id")
        if family in seen_families:
            raise ValueError(f"duplicate family in resolved catalog: {family}")
        seen_families.add(family)
        if not isinstance(controls, list):
            raise ValueError(f"invalid controls in family {family}")
        for control in controls:
            visit(control, family)
    return inventory


def validate_map(source_rows: list[dict[str, str]], mapping: dict[str, Any]) -> list[dict[str, Any]]:
    if mapping.get("schema") != "openatca.oscal-control-map" or mapping.get("version") != 1:
        raise ValueError("control map schema/version is not supported")
    source = mapping.get("source")
    if not isinstance(source, dict):
        raise ValueError("control map has no source provenance")
    if source.get("sha256") != SOURCE_SHA256 or source.get("bytes") != SOURCE_BYTES:
        raise ValueError("control map source identity does not match pinned OSCAL bytes")
    rows = mapping.get("controls")
    if not isinstance(rows, list):
        raise ValueError("control map controls must be an array")
    by_id: dict[str, dict[str, Any]] = {}
    for row in rows:
        if not isinstance(row, dict):
            raise ValueError("control map row must be an object")
        oscal_id = row.get("oscal_id")
        if not isinstance(oscal_id, str) or oscal_id in by_id:
            raise ValueError(f"missing or duplicate control-map id: {oscal_id!r}")
        by_id[oscal_id] = row
    expected = {row["oscal_id"]: row for row in source_rows}
    missing = sorted(expected.keys() - by_id.keys())
    extra = sorted(by_id.keys() - expected.keys())
    if missing or extra:
        raise ValueError(f"control inventory mismatch: missing={missing[:8]} extra={extra[:8]}")
    validated: list[dict[str, Any]] = []
    for expected_id, expected_row in expected.items():
        row = by_id[expected_id]
        for field in ("control_id", "family", "title", "parameter_ids"):
            if row.get(field) != expected_row[field]:
                raise ValueError(f"{expected_id}: {field} differs from the pinned resolved profile")
        status = row.get("status")
        if status not in STATUSES:
            raise ValueError(f"{expected_id}: unknown status {status!r}")
        missing_facts = row.get("missing_facts")
        evidence = row.get("evidence_refs")
        implemented = row.get("implemented_refs")
        if not isinstance(missing_facts, list) or any(not isinstance(x, str) or not x for x in missing_facts):
            raise ValueError(f"{expected_id}: missing_facts must contain nonempty strings")
        if not isinstance(evidence, list):
            raise ValueError(f"{expected_id}: evidence_refs must be an array")
        if not isinstance(implemented, list):
            raise ValueError(f"{expected_id}: implemented_refs must be an array")
        if status == "implemented" and not implemented:
            raise ValueError(f"{expected_id}: implemented status requires an exact test or CI reference")
        if status != "implemented" and implemented:
            raise ValueError(f"{expected_id}: only implemented rows may have implemented_refs")
        for ref in [*evidence, *implemented]:
            if not isinstance(ref, dict) or ref.get("kind") not in ("rust_test", "ci_job"):
                raise ValueError(f"{expected_id}: invalid evidence reference")
            required = ("kind", "path", "test_name") if ref["kind"] == "rust_test" else ("kind", "path", "job", "step")
            if any(not isinstance(ref.get(field), str) or not ref[field] for field in required):
                raise ValueError(f"{expected_id}: incomplete evidence reference")
        if status in ("partial", "planned", "inherited") and not missing_facts:
            raise ValueError(f"{expected_id}: {status} status requires explicit missing facts")
        validated.append(row)
    return validated


def summaries(rows: list[dict[str, Any]]) -> dict[str, Counter[str]]:
    result: dict[str, Counter[str]] = {}
    for row in rows:
        result.setdefault(row["family"], Counter())[row["status"]] += 1
    return result


def render_markdown(counts: dict[str, Counter[str]], rows: list[dict[str, Any]]) -> str:
    totals = Counter(row["status"] for row in rows)
    lines = [
        "# NIST SP 800-53 Rev. 5 HIGH engineering evidence status",
        "",
        "Source: `docs/security/oscal/SOURCES.md`; all rows are checked against the pinned resolved OSCAL profile.",
        "",
        "These are engineering evidence dispositions, not compliance findings, an ATO, or an FAA authorization.",
        "`Implemented` requires a direct test or CI reference; `partial` and `inherited` list missing facts.",
        "",
        "| Family | Total | Implemented | Partial | Planned | Inherited |",
        "|---|---:|---:|---:|---:|---:|",
    ]
    for family in sorted(counts):
        group = counts[family]
        lines.append(
            f"| {family} | {sum(group.values())} | {group['implemented']} | {group['partial']} | {group['planned']} | {group['inherited']} |"
        )
    lines.append(
        f"| **Total** | **{len(rows)}** | **{totals['implemented']}** | **{totals['partial']}** | **{totals['planned']}** | **{totals['inherited']}** |"
    )
    lines.extend(["", "Counts are generated from `docs/security/control-map.json`; they do not measure compliance.", ""])
    return "\n".join(lines)


def render_csv(counts: dict[str, Counter[str]]) -> str:
    from io import StringIO

    stream = StringIO(newline="")
    writer = csv.writer(stream, lineterminator="\n")
    writer.writerow(["family", "total", *STATUSES])
    for family in sorted(counts):
        group = counts[family]
        writer.writerow([family, sum(group.values()), *(group[status] for status in STATUSES)])
    totals = Counter()
    for group in counts.values():
        totals.update(group)
    writer.writerow(["TOTAL", sum(totals.values()), *(totals[status] for status in STATUSES)])
    return stream.getvalue()


def generate(root: Path, check: bool) -> None:
    source_bytes = read_bounded(root / SOURCE_PATH, MAX_SOURCE_BYTES)
    if len(source_bytes) != SOURCE_BYTES or hashlib.sha256(source_bytes).hexdigest() != SOURCE_SHA256:
        raise ValueError("pinned NIST OSCAL source bytes do not match recorded identity")
    source_data = json.loads(source_bytes, object_pairs_hook=strict_object)
    source_rows = source_inventory(source_data)
    map_bytes = read_bounded(root / MAP_PATH, MAX_MAP_BYTES)
    mapping = json.loads(map_bytes, object_pairs_hook=strict_object)
    catalog_metadata = source_data["catalog"]["metadata"]
    source_record = mapping.get("source") if isinstance(mapping, dict) else None
    expected_source = {
        "repo": "usnistgov/oscal-content",
        "release": "v1.5.0",
        "commit": SOURCE_COMMIT,
        "path": "nist.gov/SP800-53/rev5/json/NIST_SP-800-53_rev5_HIGH-baseline-resolved-profile_catalog.json",
        "url": SOURCE_URL,
        "sha256": SOURCE_SHA256,
        "bytes": SOURCE_BYTES,
        "catalog_version": catalog_metadata["version"],
        "oscal_version": catalog_metadata["oscal-version"],
        "profile_title": catalog_metadata["title"],
    }
    if not isinstance(source_record, dict) or any(
        source_record.get(key) != value for key, value in expected_source.items()
    ):
        raise ValueError("control map source provenance does not match the pinned artifact")
    rows = validate_map(source_rows, mapping)
    counts = summaries(rows)
    outputs = {
        MARKDOWN_PATH: render_markdown(counts, rows),
        CSV_PATH: render_csv(counts),
    }
    for relative, expected in outputs.items():
        path = root / relative
        if check:
            if not path.is_file() or path.read_text(encoding="utf-8") != expected:
                raise ValueError(f"generated output is stale or missing: {relative}")
        else:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(expected, encoding="utf-8", newline="")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true", help="fail if generated Markdown/CSV differ")
    args = parser.parse_args()
    try:
        generate(ROOT, args.check)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"control map generation failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
