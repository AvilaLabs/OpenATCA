#!/usr/bin/env python3
"""Reject proof-resource limit increases relative to an immutable registry revision."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
from typing import Any

from rust_proof_suite import validate_registry


REGISTRY_RELATIVE = Path("tools/rust_proofs.json")
MAX_REGISTRY_BYTES = 256 * 1024
CAP_FIELDS = ("memory_mib", "harness_timeout_seconds", "external_timeout_seconds")


class CapCheckError(ValueError):
    """A registry cannot be compared safely."""


def _git(repo: Path, *args: str) -> bytes:
    result = subprocess.run(
        ["git", "-C", str(repo), *args],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if result.returncode:
        raise CapCheckError("git could not read the requested revision")
    return result.stdout


def _read_git_registry(repo: Path, revision: str) -> bytes:
    if len(revision) not in (40, 64) or any(c not in "0123456789abcdef" for c in revision):
        raise CapCheckError("base must be an immutable commit hash")
    commit = _git(repo, "rev-parse", "--verify", f"{revision}^{{commit}}").decode("ascii").strip()
    if not commit or len(commit) not in (40, 64) or any(c not in "0123456789abcdef" for c in commit):
        raise CapCheckError("base revision did not resolve to a commit")
    blob = _git(repo, "rev-parse", "--verify", f"{commit}:{REGISTRY_RELATIVE.as_posix()}").decode("ascii").strip()
    if not blob or len(blob) not in (40, 64) or any(c not in "0123456789abcdef" for c in blob):
        raise CapCheckError("base registry did not resolve to a blob")
    size_text = _git(repo, "cat-file", "-s", blob).decode("ascii").strip()
    if not size_text.isdecimal():
        raise CapCheckError("base registry has an invalid blob size")
    size = int(size_text)
    if size > MAX_REGISTRY_BYTES:
        raise CapCheckError("base registry exceeds the read limit")
    raw = _git(repo, "cat-file", "blob", blob)
    if len(raw) != size:
        raise CapCheckError("base registry changed during bounded read")
    return raw


def _read_worktree_registry(repo: Path) -> bytes:
    path = repo / REGISTRY_RELATIVE
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(path, flags)
    except OSError as exc:
        raise CapCheckError("candidate registry could not be opened as a regular file") from exc
    try:
        before = os.fstat(descriptor)
        if not stat.S_ISREG(before.st_mode) or before.st_size > MAX_REGISTRY_BYTES:
            raise CapCheckError("candidate registry is not regular or exceeds the read limit")
        chunks: list[bytes] = []
        remaining = MAX_REGISTRY_BYTES + 1
        while remaining:
            chunk = os.read(descriptor, min(remaining, 16 * 1024))
            if not chunk:
                break
            chunks.append(chunk)
            remaining -= len(chunk)
        raw = b"".join(chunks)
        after = os.fstat(descriptor)
        if len(raw) > MAX_REGISTRY_BYTES:
            raise CapCheckError("candidate registry exceeds the read limit")
        if (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns) != (
            after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns
        ) or len(raw) != after.st_size:
            raise CapCheckError("candidate registry changed during bounded read")
        return raw
    finally:
        os.close(descriptor)


def _registry(raw: bytes, label: str) -> list[dict[str, Any]]:
    try:
        document = json.loads(raw)
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise CapCheckError(f"{label} registry is not valid JSON") from exc
    errors = validate_registry(document)
    if errors:
        raise CapCheckError(f"{label} registry failed validation: {errors[0]}")
    return document["proofs"]


def _identity(proof: dict[str, Any]) -> tuple[str, str]:
    return proof["package"], proof["harness"]


def compare_registries(base_proofs: list[dict[str, Any]], current_proofs: list[dict[str, Any]]) -> dict[str, Any]:
    """Compare every old obligation against current rows matching either stable key."""
    current_by_id: dict[str, dict[str, Any]] = {proof["id"]: proof for proof in current_proofs}
    current_by_identity: dict[tuple[str, str], list[dict[str, Any]]] = {}
    for proof in current_proofs:
        current_by_identity.setdefault(_identity(proof), []).append(proof)

    violations: list[dict[str, str]] = []
    removed: list[dict[str, str]] = []
    for old in base_proofs:
        candidates: dict[str, dict[str, Any]] = {}
        by_id = current_by_id.get(old["id"])
        if by_id is not None:
            candidates[by_id["id"]] = by_id
        for proof in current_by_identity.get(_identity(old), []):
            candidates[proof["id"]] = proof
        if not current_by_identity.get(_identity(old)):
            removed.append({"id": old["id"], "package": old["package"], "harness": old["harness"]})
        if not candidates:
            continue
        for candidate in candidates.values():
            for field in CAP_FIELDS:
                if candidate[field] > old[field]:
                    violations.append({
                        "base_id": old["id"],
                        "current_id": candidate["id"],
                        "field": field,
                        "base": str(old[field]),
                        "current": str(candidate[field]),
                    })

    base_ids = {proof["id"] for proof in base_proofs}
    current_ids = {proof["id"] for proof in current_proofs}
    added = [
        {"id": proof["id"], "package": proof["package"], "harness": proof["harness"]}
        for proof in current_proofs
        if proof["id"] not in base_ids
    ]
    return {
        "violations": violations,
        "removed": removed,
        "added": added,
        "removed_ids": sorted(base_ids - current_ids),
        "added_ids": sorted(current_ids - base_ids),
    }


def check(repo: Path, base_revision: str) -> dict[str, Any]:
    repo = repo.resolve(strict=True)
    base = _registry(_read_git_registry(repo, base_revision), "base")
    current = _registry(_read_worktree_registry(repo), "candidate")
    return compare_registries(base, current)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--base", required=True, help="immutable main merge-base commit")
    args = parser.parse_args(argv)
    try:
        report = check(args.repo, args.base)
    except (CapCheckError, OSError) as exc:
        print(f"proof-cap check refused: {exc}", file=sys.stderr)
        return 2
    print(json.dumps(report, sort_keys=True))
    if report["violations"] or report["removed"]:
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
