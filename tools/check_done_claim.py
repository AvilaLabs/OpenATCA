#!/usr/bin/env python3
"""Validate clause-level evidence for a claim against the pinned requirements register."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import hashlib
import json
import os
import re
import stat
import subprocess
import sys
from pathlib import Path
from typing import Any


REQUIREMENTS_SHA256 = "f0d298003b6e1ee495c96928b15010962e5432ced645f363abb43691e89986e7"
REQ_RE = re.compile(r"^- \*\*((OA-[A-Z0-9]+-[0-9]+))\*\*\s+(.*)$")
SYMBOL_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_:.!<>-]*$")
CLAIM_SCHEMA = "openatca.done-claim.v1"
MANIFEST_SCHEMA = "openatca.required-clause-manifest.v1"
INVENTORY_SCHEMA = "openatca.validated-test-inventory.v1"
MAX_SOURCE_BYTES = 4 * 1024 * 1024
MAX_JSON_BYTES = 2 * 1024 * 1024
MAX_EVIDENCE_BYTES = 4 * 1024 * 1024
MAX_TRACE_TOOL_BYTES = 128 * 1024 * 1024
MAX_REQUIREMENT_ROWS = 10_000
MAX_CLAUSES_PER_ROW = 256
MAX_TEST_ENTRIES = 10_000
MAX_EVIDENCE_PER_CLAUSE = 128


class ClaimError(ValueError):
    pass


@dataclass(frozen=True)
class RowClaimInputs:
    root: Path
    source_bytes: bytes
    rows_by_id: dict[str, str]
    manifest: dict[str, Any]
    manifest_bytes: bytes
    claim: dict[str, Any]
    inventory: dict[str, Any]
    inventory_bytes: bytes
    row_id: str
    source_commit: str


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def normalized(text: str) -> str:
    return " ".join(text.split())


def parse_requirements(text: str) -> dict[str, str]:
    """Read the register's stable-ID bullets and preserve their complete normalized wording."""
    lines = text.splitlines()
    found: dict[str, list[str]] = {}
    active: str | None = None
    for line in lines:
        match = REQ_RE.match(line)
        if match:
            req_id, first = match.group(2), match.group(3)
            if req_id in found:
                raise ClaimError(f"duplicate requirement ID {req_id}")
            if len(found) >= MAX_REQUIREMENT_ROWS:
                raise ClaimError("requirements row count exceeds its limit")
            found[req_id] = [first]
            active = req_id
        elif active is not None:
            if line.startswith("- **") or line.startswith("### ") or line.startswith("## "):
                active = None
            elif line.startswith("  "):
                found[active].append(line.strip())
            elif not line.strip():
                continue
            else:
                active = None
    if not found:
        raise ClaimError("requirements register contains no stable-ID bullets")
    return {req_id: normalized(" ".join(parts)) for req_id, parts in found.items()}


def _table_cells(line: str) -> list[str]:
    body = line.strip().strip("|")
    cells: list[str] = []
    current: list[str] = []
    escaped = False
    for char in body:
        if char == "|" and not escaped:
            cells.append("".join(current).replace(r"\|", "|").strip())
            current.clear()
        else:
            current.append(char)
        escaped = char == "\\" and not escaped
        if char != "\\":
            escaped = False
    cells.append("".join(current).replace(r"\|", "|").strip())
    return cells


def parse_faa_acceptance(text: str) -> dict[str, str]:
    """Extract row IDs and full Required cells from the repeated acceptance-list tables."""
    rows: dict[str, str] = {}
    columns: dict[str, int] | None = None
    for line in text.splitlines():
        if not line.lstrip().startswith("|"):
            columns = None
            continue
        cells = _table_cells(line)
        folded = [cell.casefold() for cell in cells]
        if "id" in folded and "required" in folded:
            columns = {name: folded.index(name) for name in ("id", "required")}
            continue
        if columns is None or all(re.fullmatch(r":?-{3,}:?", cell.replace(" ", "")) for cell in cells):
            continue
        if len(cells) <= max(columns.values()):
            raise ClaimError("acceptance source has a malformed requirement table row")
        req_id = cells[columns["id"]].strip(" `")
        required = normalized(cells[columns["required"]])
        if not req_id or not required:
            raise ClaimError("acceptance source contains a row without an ID or Required text")
        if req_id in rows:
            raise ClaimError("acceptance source has duplicate row IDs")
        if len(rows) >= MAX_REQUIREMENT_ROWS:
            raise ClaimError("acceptance row count exceeds its limit")
        rows[req_id] = required
    if not rows:
        raise ClaimError("acceptance source has no ID/Required table")
    return rows


def _bounded_read(path: Path, limit: int, description: str) -> bytes:
    """Read only a regular, no-follow file and stop after the configured cap."""
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(path, flags)
    except OSError as exc:
        raise ClaimError(f"cannot open {description}") from exc
    try:
        info = os.fstat(descriptor)
        if not stat.S_ISREG(info.st_mode) or info.st_size > limit:
            raise ClaimError(f"{description} is not a regular file within its byte cap")
        with os.fdopen(descriptor, "rb", closefd=False) as stream:
            data = stream.read(limit + 1)
        if len(data) > limit:
            raise ClaimError(f"{description} exceeds its byte cap")
        return data
    finally:
        os.close(descriptor)


def _evidence(root: Path, item: Any, clause_index: int) -> None:
    if not isinstance(item, dict):
        raise ClaimError(f"clause {clause_index}: evidence entry must be an object")
    kind, rel, symbol = item.get("kind"), item.get("path"), item.get("symbol")
    start, end, digest = item.get("start_line"), item.get("end_line"), item.get("sha256")
    if kind != "source" or set(item) != {"kind", "path", "start_line", "end_line", "symbol", "sha256"}:
        raise ClaimError(f"clause {clause_index}: source evidence fields are invalid")
    if not isinstance(rel, str) or not rel or Path(rel).is_absolute() or ".." in Path(rel).parts:
        raise ClaimError(f"clause {clause_index}: evidence path must be repository-relative")
    if not isinstance(symbol, str) or not SYMBOL_RE.fullmatch(symbol):
        raise ClaimError(f"clause {clause_index}: evidence symbol is missing or malformed")
    if not isinstance(start, int) or isinstance(start, bool) or start < 1:
        raise ClaimError(f"clause {clause_index}: invalid evidence start_line")
    if not isinstance(end, int) or isinstance(end, bool) or end < start:
        raise ClaimError(f"clause {clause_index}: invalid evidence end_line")
    if not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest):
        raise ClaimError(f"clause {clause_index}: evidence sha256 is malformed")
    path = (root / rel).resolve()
    if root.resolve() not in path.parents:
        raise ClaimError(f"clause {clause_index}: evidence escapes repository root")
    try:
        data = _bounded_read(path, MAX_EVIDENCE_BYTES, "evidence file")
    except (OSError, ClaimError) as exc:
        raise ClaimError(f"clause {clause_index}: evidence file is missing") from exc
    if sha256(data) != digest:
        raise ClaimError(f"clause {clause_index}: evidence digest does not match file")
    lines = data.decode("utf-8").splitlines()
    if end > len(lines):
        raise ClaimError(f"clause {clause_index}: evidence line range exceeds file")
    excerpt = "\n".join(lines[start - 1 : end])
    if not re.search(rf"\b{re.escape(symbol)}\b", excerpt):
        raise ClaimError(f"clause {clause_index}: evidence symbol not found in cited lines")


def _scope_digest(source_digest: str, manifest_digest: str, row_id: str, clauses: list[dict[str, int]]) -> str:
    scope = {"source_sha256": source_digest, "manifest_sha256": manifest_digest, "row_id": row_id, "clauses": clauses}
    encoded = json.dumps(scope, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return sha256(encoded)


def _current_commit(root: Path) -> str:
    result = subprocess.run(
        ["git", "-C", str(root), "rev-parse", "--verify", "HEAD"],
        capture_output=True,
        text=True,
        check=False,
    )
    commit = result.stdout.strip()
    if result.returncode != 0 or not re.fullmatch(r"[0-9a-f]{40}", commit):
        raise ClaimError("repository commit cannot be verified")
    status = subprocess.run(
        ["git", "-C", str(root), "status", "--porcelain", "--untracked-files=all"],
        capture_output=True,
        text=True,
        check=False,
    )
    if status.returncode != 0 or status.stdout:
        raise ClaimError("repository source is not a clean committed snapshot")
    return commit


def _trace_has_test(trace_text: str, requirement_id: str, marker: str) -> bool:
    for line in trace_text.splitlines():
        if not line.lstrip().startswith("|"):
            continue
        cells = _table_cells(line)
        if len(cells) >= 3 and cells[0].strip(" `") == requirement_id and marker in cells[2]:
            return True
    return False


def _validate_manifest(rows_by_id: dict[str, str], manifest: dict[str, Any]) -> dict[str, Any]:
    if manifest.get("schema") != MANIFEST_SCHEMA or set(manifest) != {"schema", "source_sha256", "requirements"}:
        raise ClaimError("canonical manifest schema or fields are invalid")
    rows = manifest.get("requirements")
    if not isinstance(rows, list):
        raise ClaimError("canonical manifest requirements must be a list")
    indexed: dict[str, Any] = {}
    for row in rows:
        if not isinstance(row, dict) or set(row) != {"id", "text_sha256", "clauses"}:
            raise ClaimError("canonical manifest row is malformed")
        req_id = row.get("id")
        if not isinstance(req_id, str) or req_id in indexed:
            raise ClaimError("canonical manifest contains duplicate or invalid row IDs")
        indexed[req_id] = row
    if set(indexed) != set(rows_by_id):
        raise ClaimError("canonical manifest omits or invents an authoritative row")
    for req_id, required_text in rows_by_id.items():
        row = indexed[req_id]
        if row.get("text_sha256") != sha256(required_text.encode("utf-8")):
            raise ClaimError(f"{req_id}: manifest text binding mismatch")
        spans = row.get("clauses")
        if not isinstance(spans, list) or not spans or len(spans) > MAX_CLAUSES_PER_ROW:
            raise ClaimError(f"{req_id}: canonical manifest has no clauses")
        cursor = 0
        for span in spans:
            if not isinstance(span, dict) or set(span) != {"start", "end"}:
                raise ClaimError(f"{req_id}: canonical clause span is malformed")
            start, end = span.get("start"), span.get("end")
            if (
                not isinstance(start, int) or isinstance(start, bool)
                or not isinstance(end, int) or isinstance(end, bool)
                or start != cursor or end <= start or end > len(required_text)
            ):
                raise ClaimError(f"{req_id}: manifest clauses have a gap, overlap, or invalid span")
            cursor = end
        if cursor != len(required_text):
            raise ClaimError(f"{req_id}: manifest omits authoritative requirement text")
    return indexed


def _validate_inventory(
    root: Path,
    inventory: dict[str, Any],
    inventory_digest: str,
    source_commit: str,
    manifest_digest: str,
    scope_digest: str,
    clause_digests: set[str],
) -> set[tuple[str, str, int, str, str]]:
    if inventory.get("schema") != INVENTORY_SCHEMA or inventory.get("producer") != "oa-trace-clause-review":
        raise ClaimError("test inventory must be an oa-trace clause review inventory")
    expected_keys = {
        "schema", "producer", "source_commit", "manifest_sha256", "scope_sha256",
        "trace_report_path", "trace_report_sha256", "validator_sha256", "tests",
    }
    if set(inventory) != expected_keys:
        raise ClaimError("test inventory contains unsupported fields")
    if inventory.get("source_commit") != source_commit or not re.fullmatch(r"[0-9a-f]{40}", source_commit):
        raise ClaimError("test inventory is not bound to the current source commit")
    if inventory.get("manifest_sha256") != manifest_digest or inventory.get("scope_sha256") != scope_digest:
        raise ClaimError("test inventory is not bound to the requested row and clause scope")
    validator_digest = inventory.get("validator_sha256")
    if not isinstance(validator_digest, str) or not re.fullmatch(r"[0-9a-f]{64}", validator_digest):
        raise ClaimError("test inventory has no pinned oa-trace validator binary")
    trace_digest, trace_rel = inventory.get("trace_report_sha256"), inventory.get("trace_report_path")
    if not isinstance(trace_digest, str) or not re.fullmatch(r"[0-9a-f]{64}", trace_digest):
        raise ClaimError("test inventory has no valid trace report digest")
    if not isinstance(trace_rel, str) or Path(trace_rel).is_absolute() or ".." in Path(trace_rel).parts:
        raise ClaimError("trace report path must be repository-relative")
    trace_path = (root / trace_rel).resolve()
    if root.resolve() not in trace_path.parents:
        raise ClaimError("trace report escapes repository root")
    trace_text = _bounded_read(trace_path, MAX_JSON_BYTES, "trace report").decode("utf-8")
    if sha256(trace_text.encode("utf-8")) != trace_digest:
        raise ClaimError("test inventory trace report digest mismatch")
    entries = inventory.get("tests")
    if not isinstance(entries, list) or len(entries) > MAX_TEST_ENTRIES:
        raise ClaimError("test inventory tests must be a list")
    refs: set[tuple[str, str, int, str, str]] = set()
    for item in entries:
        if not isinstance(item, dict) or set(item) != {
            "requirement_id", "path", "symbol", "line", "source_sha256", "clause_sha256"
        }:
            raise ClaimError("test inventory contains a malformed entry")
        req_id, rel, symbol, line = item["requirement_id"], item["path"], item["symbol"], item["line"]
        clause_digest = item["clause_sha256"]
        if not isinstance(clause_digest, str) or clause_digest not in clause_digests:
            raise ClaimError("test inventory clause binding is outside requested scope")
        if not isinstance(req_id, str) or not re.fullmatch(r"OA-[A-Z0-9]+-[0-9]{3}", req_id):
            raise ClaimError("test inventory has an invalid requirement link")
        if not isinstance(rel, str) or Path(rel).is_absolute() or ".." in Path(rel).parts:
            raise ClaimError("test inventory path must be repository-relative")
        if not isinstance(symbol, str) or not SYMBOL_RE.fullmatch(symbol):
            raise ClaimError("test inventory symbol is malformed")
        if not isinstance(line, int) or isinstance(line, bool) or line < 1:
            raise ClaimError("test inventory line is invalid")
        if not _trace_has_test(trace_text, req_id, f"`{symbol}` ({rel}:{line})"):
            raise ClaimError("test inventory reference is absent from the bound oa-trace report")
        source_path = (root / rel).resolve()
        if root.resolve() not in source_path.parents:
            raise ClaimError("test inventory source escapes repository root")
        source_bytes = _bounded_read(source_path, MAX_EVIDENCE_BYTES, "test source")
        if item["source_sha256"] != sha256(source_bytes):
            raise ClaimError("test inventory source digest mismatch")
        refs.add((req_id, rel, line, symbol, clause_digest))
    return refs


def _verify_test_refs(root: Path, inventory: dict[str, Any], validator_path: Path) -> None:
    validator = validator_path.resolve()
    if not os.access(validator, os.X_OK):
        raise ClaimError("trusted oa-trace validator is not executable")
    validator_bytes = _bounded_read(validator, MAX_TRACE_TOOL_BYTES, "oa-trace validator")
    if sha256(validator_bytes) != inventory.get("validator_sha256"):
        raise ClaimError("oa-trace validator digest differs from validated inventory")
    references = sorted({f"{item['path']}::{item['symbol']}" for item in inventory["tests"]})
    for offset in range(0, len(references), 64):
        args = [str(validator), "--root", str(root)]
        for reference in references[offset : offset + 64]:
            args.extend(("--verify-test", reference))
        try:
            validator_env = {"PATH": os.defpath}
            data_root = os.environ.get("OPENATCA_DATA")
            if data_root is not None:
                validator_env["OPENATCA_DATA"] = data_root
            result = subprocess.run(
                args,
                capture_output=True,
                text=True,
                timeout=30,
                check=False,
                env=validator_env,
            )
        except (OSError, subprocess.TimeoutExpired) as exc:
            raise ClaimError("oa-trace validator could not verify test inventory") from exc
        if result.returncode != 0:
            raise ClaimError("oa-trace rejected a cited test reference")


def _validate_claim_clauses(
    root: Path, row_id: str, text: str, spans: list[dict[str, int]], claim: dict[str, Any],
    tests: set[tuple[str, str, int, str, str]],
) -> None:
    clauses = claim.get("clauses")
    if not isinstance(clauses, list) or len(clauses) != len(spans):
        raise ClaimError(f"{row_id}: claim omits or duplicates canonical clauses")
    for index, (span, clause) in enumerate(zip(spans, clauses)):
        if not isinstance(clause, dict) or set(clause) != {"start", "end", "status", "evidence"}:
            raise ClaimError(f"{row_id}: clause {index} is malformed")
        if clause.get("start") != span["start"] or clause.get("end") != span["end"]:
            raise ClaimError(f"{row_id}: claim clause ranges differ from canonical manifest")
        if clause.get("status") != "covered":
            raise ClaimError(f"{row_id}: clause {index} is not fully covered; the claim is refused")
        evidence = clause.get("evidence")
        if not isinstance(evidence, list) or not evidence or len(evidence) > MAX_EVIDENCE_PER_CLAUSE:
            raise ClaimError(f"{row_id}: clause {index} has no evidence")
        test_seen = False
        for item in evidence:
            if not isinstance(item, dict) or not isinstance(item.get("kind"), str):
                raise ClaimError(f"{row_id}: clause {index} evidence is malformed")
            if item["kind"] == "test":
                if set(item) != {"kind", "requirement_id", "path", "line", "symbol", "clause_sha256"}:
                    raise ClaimError(f"{row_id}: clause {index} test evidence is malformed")
                if (
                    not isinstance(item["requirement_id"], str)
                    or not isinstance(item["path"], str)
                    or not isinstance(item["line"], int)
                    or isinstance(item["line"], bool)
                    or not isinstance(item["symbol"], str)
                ):
                    raise ClaimError(f"{row_id}: clause {index} test evidence fields are invalid")
                expected_clause_digest = sha256(text[span["start"] : span["end"]].encode("utf-8"))
                if item["clause_sha256"] != expected_clause_digest:
                    raise ClaimError(f"{row_id}: clause {index} test evidence binds different wording")
                ref = (item["requirement_id"], item["path"], item["line"], item["symbol"], expected_clause_digest)
                if ref not in tests:
                    raise ClaimError(f"{row_id}: clause {index} test is not in the validated inventory")
                test_seen = True
            elif item["kind"] == "source":
                if set(item) != {"kind", "path", "start_line", "end_line", "symbol", "sha256"}:
                    raise ClaimError(f"{row_id}: clause {index} source evidence is malformed")
                _evidence(root, item, index)
            else:
                raise ClaimError(f"{row_id}: clause {index} evidence kind is invalid")
        if not test_seen:
            raise ClaimError(f"{row_id}: clause {index} lacks per-clause oa-trace test evidence")


def validate_row_claim(inputs: RowClaimInputs) -> None:
    """Check one requested row against source, all-row manifest, claim, and trace inventory."""
    root = inputs.root
    source_bytes = inputs.source_bytes
    rows_by_id = inputs.rows_by_id
    manifest = inputs.manifest
    manifest_bytes = inputs.manifest_bytes
    claim = inputs.claim
    inventory = inputs.inventory
    inventory_bytes = inputs.inventory_bytes
    row_id = inputs.row_id
    source_commit = inputs.source_commit
    source_digest, manifest_digest = sha256(source_bytes), sha256(manifest_bytes)
    inventory_digest = sha256(inventory_bytes)
    if manifest.get("source_sha256") != source_digest:
        raise ClaimError("manifest is not bound to the current authoritative source")
    indexed = _validate_manifest(rows_by_id, manifest)
    if claim.get("schema") != CLAIM_SCHEMA or set(claim) != {
        "schema", "source_sha256", "manifest_sha256", "test_inventory_sha256", "row_id", "clauses"
    }:
        raise ClaimError("status claim schema or fields are invalid")
    if claim.get("source_sha256") != source_digest or claim.get("manifest_sha256") != manifest_digest:
        raise ClaimError("status claim is not bound to the authoritative source and canonical manifest")
    if claim.get("test_inventory_sha256") != inventory_digest:
        raise ClaimError("status claim is not bound to the validated test inventory")
    if row_id not in rows_by_id or claim.get("row_id") != row_id:
        raise ClaimError("requested row is absent or status claim row does not match")
    spans = indexed[row_id]["clauses"]
    scope_digest = _scope_digest(source_digest, manifest_digest, row_id, spans)
    text = rows_by_id[row_id]
    clause_digests = {sha256(text[span["start"] : span["end"]].encode("utf-8")) for span in spans}
    tests = _validate_inventory(root, inventory, inventory_digest, source_commit, manifest_digest, scope_digest, clause_digests)
    _validate_claim_clauses(root, row_id, text, spans, claim, tests)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path.cwd())
    parser.add_argument("--requirements", type=Path, default=Path("REQUIREMENTS.md"))
    parser.add_argument("--source-kind", choices=("public", "faa"), default="public")
    parser.add_argument("--manifest", required=True, type=Path)
    parser.add_argument("--claim", required=True, type=Path)
    parser.add_argument("--test-inventory", required=True, type=Path)
    parser.add_argument("--row", required=True)
    parser.add_argument("--source-commit", required=True)
    parser.add_argument("--expected-source-sha256")
    parser.add_argument("--oa-trace-bin", required=True, type=Path)
    args = parser.parse_args()
    root = args.repo.resolve()
    if args.source_kind == "faa":
        private_root = os.environ.get("OPENATCA_PRIVATE")
        if not private_root:
            print("claim rejected: OPENATCA_PRIVATE is required for FAA acceptance source", file=sys.stderr)
            return 1
        private_root_path = Path(private_root).resolve()
        if private_root_path == Path("/") or not private_root_path.is_dir():
            print("claim rejected: OPENATCA_PRIVATE does not identify an allowed private data root", file=sys.stderr)
            return 1
        req_path = private_root_path / "FAA_ACCEPTANCE_LIST.md"
    else:
        req_path = args.requirements if args.requirements.is_absolute() else root / args.requirements
    manifest_path = args.manifest if args.manifest.is_absolute() else root / args.manifest
    claim_path = args.claim if args.claim.is_absolute() else root / args.claim
    inventory_path = args.test_inventory if args.test_inventory.is_absolute() else root / args.test_inventory
    try:
        source_bytes = _bounded_read(req_path, MAX_SOURCE_BYTES, "requirements source")
        source_text = source_bytes.decode("utf-8")
        if args.source_kind == "faa":
            if not isinstance(args.expected_source_sha256, str) or not re.fullmatch(
                r"[0-9a-f]{64}", args.expected_source_sha256
            ):
                raise ClaimError("FAA mode requires an independently supplied source digest")
            if sha256(source_bytes) != args.expected_source_sha256:
                raise ClaimError("FAA source differs from the independently supplied digest")
            rows_by_id = parse_faa_acceptance(source_text)
        else:
            if sha256(source_bytes) != REQUIREMENTS_SHA256:
                raise ClaimError("public requirements source differs from the reviewed pin")
            rows_by_id = parse_requirements(source_text)
        manifest_bytes = _bounded_read(manifest_path, MAX_JSON_BYTES, "canonical manifest")
        claim_bytes = _bounded_read(claim_path, MAX_JSON_BYTES, "status claim")
        inventory_bytes = _bounded_read(inventory_path, MAX_JSON_BYTES, "validated test inventory")
        manifest = json.loads(manifest_bytes.decode("utf-8"))
        claim = json.loads(claim_bytes.decode("utf-8"))
        inventory = json.loads(inventory_bytes.decode("utf-8"))
        if not all(isinstance(value, dict) for value in (manifest, claim, inventory)):
            raise ClaimError("manifest, claim, and inventory roots must be objects")
        if _current_commit(root) != args.source_commit:
            raise ClaimError("requested source commit differs from the clean repository snapshot")
        validate_row_claim(RowClaimInputs(
            root=root,
            source_bytes=source_bytes,
            rows_by_id=rows_by_id,
            manifest=manifest,
            manifest_bytes=manifest_bytes,
            claim=claim,
            inventory=inventory,
            inventory_bytes=inventory_bytes,
            row_id=args.row,
            source_commit=args.source_commit,
        ))
        _verify_test_refs(root, inventory, args.oa_trace_bin)
    except (OSError, UnicodeError, json.JSONDecodeError, ClaimError) as exc:
        print(f"claim rejected: {exc}", file=sys.stderr)
        return 1
    print("row claim eligible for independent cold review")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
