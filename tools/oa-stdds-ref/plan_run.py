#!/usr/bin/env python3
"""Freeze a private oa-stdds-ref invocation plan from a metadata-only input inventory.

The inventory is an operator-curated record, not a data reader. This helper never opens,
hashes, stats, or enumerates the FAA input paths named in it.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from datetime import datetime
from pathlib import Path
from typing import Any


MAX_STDDS_FILES = 32
MAX_STDDS_STORED_BYTES = 384 * 1024 * 1024
MAX_STDDS_DECODED_BYTES = 512 * 1024 * 1024
MAX_ADSB_BYTES = 128 * 1024 * 1024
MAX_ROWS = 100_000
MAX_REPORT_BYTES = 256 * 1024 * 1024
MAX_ANALYSIS_WORK = 80_000_000
OUTPUTS = ["stdds_ref.txt", "stdds_ref.csv", "stdds_ref.json", "stdds_ref.complete"]
HEX = re.compile(r"^[0-9a-f]{64}$")


class PlanError(ValueError):
    pass


def _utc(value: Any, label: str) -> datetime:
    if not isinstance(value, str) or not value.endswith("Z"):
        raise PlanError(f"{label} must be an RFC3339 UTC timestamp ending in Z")
    try:
        parsed = datetime.fromisoformat(value[:-1] + "+00:00")
    except ValueError as exc:
        raise PlanError(f"{label} is invalid: {value}") from exc
    return parsed


def _unix_ms(value: datetime, label: str) -> int:
    if value.microsecond % 1000:
        raise PlanError(f"{label} must have millisecond precision")
    delta = value - datetime(1970, 1, 1, tzinfo=value.tzinfo)
    return delta.days * 86_400_000 + delta.seconds * 1000 + delta.microseconds // 1000


def _input(item: Any, kind: str, index: int) -> dict[str, str]:
    if not isinstance(item, dict):
        raise PlanError(f"{kind}[{index}] must be an object")
    path, digest, source_id = item.get("path"), item.get("sha256"), item.get("source_id")
    if not isinstance(path, str) or not path or "\0" in path:
        raise PlanError(f"{kind}[{index}].path must be a nonempty path string")
    if not isinstance(source_id, str) or not source_id.strip():
        raise PlanError(f"{kind}[{index}].source_id must identify its saved provenance record")
    if not isinstance(digest, str) or not HEX.fullmatch(digest):
        raise PlanError(f"{kind}[{index}].sha256 must be a lowercase SHA-256 from saved metadata")
    return {"path": path, "sha256": digest, "source_id": source_id}


def build_plan(spec: dict[str, Any]) -> dict[str, Any]:
    if spec.get("schema") != "oa-stdds-ref-plan-input-v1":
        raise PlanError("schema must be oa-stdds-ref-plan-input-v1")
    cohort = spec.get("cohort_id")
    if not isinstance(cohort, str) or not 1 <= len(cohort) <= 128 or not all(
        char.isascii() and (char.isalnum() or char in "-_.:") for char in cohort
    ):
        raise PlanError("cohort_id must use 1..128 ASCII letters, digits, dot, dash, underscore, or colon")
    interval = spec.get("interval_utc")
    if not isinstance(interval, dict):
        raise PlanError("interval_utc must contain start and end")
    start, end = _utc(interval.get("start"), "interval_utc.start"), _utc(interval.get("end"), "interval_utc.end")
    if start >= end:
        raise PlanError("interval_utc.start must precede interval_utc.end")
    from_ms = _unix_ms(start, "interval_utc.start")
    to_ms = _unix_ms(end, "interval_utc.end")
    inventory = spec.get("inputs")
    if not isinstance(inventory, dict) or not isinstance(inventory.get("stdds"), list):
        raise PlanError("inputs.stdds must be an array")
    if not 1 <= len(inventory["stdds"]) <= MAX_STDDS_FILES:
        raise PlanError(f"STDDS input count must be 1..{MAX_STDDS_FILES}")
    stdds = [_input(x, "inputs.stdds", i) for i, x in enumerate(inventory["stdds"])]
    adsb = _input(inventory.get("adsb"), "inputs.adsb", 0)
    executable = spec.get("executable")
    if not isinstance(executable, dict):
        raise PlanError("executable metadata is required")
    binary = _input(executable, "executable", 0)
    output = spec.get("output_directory")
    if not isinstance(output, str) or not output.strip():
        raise PlanError("output_directory is required")
    source = spec.get("source")
    if not isinstance(source, dict) or not isinstance(source.get("commit"), str) or not HEX.fullmatch(source.get("tree_sha256", "")):
        raise PlanError("source.commit and source.tree_sha256 are required")
    plan = {
        "schema": "oa-stdds-ref-run-plan-v1",
        "cohort_id": cohort,
        "interval_utc": {"start": interval["start"], "end": interval["end"], "from_ms_inclusive": from_ms, "to_ms_exclusive": to_ms, "selection": "STDDS mrtTime rows in the half-open interval; enforced before offset fitting; ADS-B truth retains full clip context"},
        "modes": ["raw_published_mrtTime", "fitted_in_sample_agreement"],
        "fit_policy": {"offset_search_ms": [-15000, 15000], "step_ms": 100, "minimum_samples": 8, "label": "fitted metrics are in-sample calibrated agreement, not prospective accuracy"},
        "inputs": {"adsb": adsb, "stdds": stdds},
        "source": source,
        "executable": binary,
        "output_directory": output,
        "expected_outputs": OUTPUTS,
        "output_manifest": {"required_files": OUTPUTS, "digest_policy": "record SHA-256 for each exact output; accept the bundle only when stdds_ref.complete is present and its embedded digests match the summary, CSV, JSON, and provenance header"},
        "limits": {"stdds_files": MAX_STDDS_FILES, "stdds_stored_bytes": MAX_STDDS_STORED_BYTES, "stdds_decoded_bytes": MAX_STDDS_DECODED_BYTES, "adsb_bytes": MAX_ADSB_BYTES, "retained_rows_each": MAX_ROWS, "report_bytes": MAX_REPORT_BYTES, "analysis_work_units": MAX_ANALYSIS_WORK},
        "argv": [binary["path"], "--adsb", adsb["path"], "--out", output, "--cohort-id", cohort, "--from-ms", str(from_ms), "--to-ms", str(to_ms), *[x["path"] for x in stdds]],
        "provenance_boundary": "metadata-only plan; the referenced source records and hashes must be independently verified by the operator before execution",
    }
    digest_input = json.dumps(plan, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
    plan["plan_sha256"] = hashlib.sha256(digest_input).hexdigest()
    return plan


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--inventory", type=Path, required=True, help="metadata-only JSON inventory")
    parser.add_argument("--out", type=Path, required=True, help="new private plan JSON path")
    args = parser.parse_args(argv)
    try:
        spec = json.loads(args.inventory.read_text(encoding="utf-8"))
        if not isinstance(spec, dict):
            raise PlanError("inventory must be a JSON object")
        result = build_plan(spec)
        args.out.parent.mkdir(parents=True, exist_ok=True)
        with args.out.open("x", encoding="utf-8", newline="\n") as stream:
            json.dump(result, stream, indent=2, sort_keys=True, allow_nan=False)
            stream.write("\n")
    except (OSError, UnicodeError, json.JSONDecodeError, PlanError) as exc:
        print(f"oa-stdds-ref plan: {exc}", file=sys.stderr)
        return 2
    print(f"plan SHA-256: {result['plan_sha256']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
