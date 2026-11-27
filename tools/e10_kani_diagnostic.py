"""Run one preregistered Kani proof with bounded resource telemetry."""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time
from typing import Any

from rust_proof_suite import load_registry, validate_registry


BASELINE_REF = "8c5923da2f0c7f11ca84e99fc443980d86b4ef5c"
CURRENT_REF = "64c44e714bc47bdeb956e671d7e15aa4f06edfde"
PROOF_IDS = (
    "late-append-output-projection",
    "large-move-decision-rejected-current-filter-snapshot",
)
DIAGNOSTIC_CASES = tuple(
    (revision, proof_id)
    for revision in (BASELINE_REF, CURRENT_REF)
    for proof_id in PROOF_IDS
)


def _canonical_sha256(value: Any) -> str:
    data = json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
    return hashlib.sha256(data).hexdigest()


def single_entry_registry(registry: dict[str, Any], proof_id: str) -> dict[str, Any]:
    if proof_id not in PROOF_IDS:
        raise ValueError("proof ID is outside the pinned diagnostic matrix")
    proofs = registry.get("proofs")
    if not isinstance(proofs, list):
        raise ValueError("registry proofs must be a list")
    matches = [entry for entry in proofs if isinstance(entry, dict) and entry.get("id") == proof_id]
    if len(matches) != 1 or matches[0].get("enabled") is not True:
        raise ValueError("expected exactly one enabled matching registry entry")
    subset = {
        "registry_version": registry.get("registry_version"),
        "expected_proof_count": 1,
        "proofs": [matches[0]],
    }
    errors = validate_registry(subset)
    if errors:
        raise ValueError("single-entry registry is invalid: " + "; ".join(errors))
    return subset


def _mem_available_kib() -> str:
    try:
        for line in Path("/proc/meminfo").read_text(encoding="ascii").splitlines():
            if line.startswith("MemAvailable:"):
                return line.split()[1]
    except OSError:
        pass
    return "unavailable"


def _cgroup_memory_dir() -> Path | None:
    try:
        lines = Path("/proc/self/cgroup").read_text(encoding="ascii").splitlines()
        unified = next((line[3:] for line in lines if line.startswith("0::")), None)
    except OSError:
        return None
    if unified is None:
        return None
    return Path("/sys/fs/cgroup") / unified.lstrip("/")


def _read_cgroup_value(directory: Path | None, filename: str) -> str:
    if directory is None:
        return "unavailable"
    try:
        return (directory / filename).read_text(encoding="ascii").strip().replace("\n", ";")
    except OSError:
        return "unavailable"


def _record_resource_sample(output: Any, directory: Path | None) -> None:
    values = (
        dt.datetime.now(dt.timezone.utc).isoformat(),
        _mem_available_kib(),
        _read_cgroup_value(directory, "memory.current"),
        _read_cgroup_value(directory, "memory.peak"),
        _read_cgroup_value(directory, "memory.max"),
        _read_cgroup_value(directory, "memory.events"),
    )
    output.write("\t".join(values) + "\n")
    output.flush()


def _git_head(repo: Path) -> str:
    result = subprocess.run(
        ["git", "rev-parse", "--verify", "HEAD"], cwd=repo,
        check=True, capture_output=True, text=True, timeout=30,
    )
    return result.stdout.strip()


def _require_clean_checkout(repo: Path) -> None:
    result = subprocess.run(
        ["git", "status", "--porcelain=v1", "--untracked-files=all"], cwd=repo,
        check=True, capture_output=True, text=True, timeout=30,
    )
    if result.stdout.strip():
        raise ValueError("diagnostic source checkout must be clean")


def _tool_version(command: list[str]) -> str:
    result = subprocess.run(command, check=True, capture_output=True, text=True, timeout=60)
    return result.stdout.strip()


def run_diagnostic(repo: Path, proof_id: str, output_root: Path, revision: str) -> int:
    if (revision, proof_id) not in DIAGNOSTIC_CASES:
        raise ValueError("revision/proof pair is outside the pinned diagnostic matrix")
    repo = repo.resolve(strict=True)
    output_root.mkdir(parents=True, exist_ok=False)
    registry_path = repo / "tools" / "rust_proofs.json"
    registry, errors = load_registry(registry_path)
    if errors or registry is None:
        raise ValueError("source registry failed validation: " + "; ".join(errors))
    subset = single_entry_registry(registry, proof_id)
    subset_bytes = (json.dumps(subset, indent=2) + "\n").encode("utf-8")
    subset_path = output_root / "registry.json"
    subset_path.write_bytes(subset_bytes)
    proof_entry = subset["proofs"][0]
    expected_head = revision
    source_head = _git_head(repo)
    if not source_head.startswith(expected_head):
        raise ValueError(f"checkout HEAD {source_head} does not match selected revision {expected_head}")
    _require_clean_checkout(repo)

    run_output = output_root / "suite"
    target_dir = Path(os.environ["RUNNER_TEMP"]) / f"kani-target-{proof_id}"
    command = [
        sys.executable, str(repo / "tools" / "rust_proof_suite.py"),
        "--repo", str(repo), "--registry", str(subset_path), "--shard", "0/1", "--fail-fast",
        "--out", str(run_output), "--target-dir", str(target_dir),
    ]
    manifest = {
        "source_revision": source_head,
        "selected_revision": revision,
        "proof_id": proof_id,
        "proof_entry_sha256": _canonical_sha256(proof_entry),
        "source_registry_sha256": hashlib.sha256(registry_path.read_bytes()).hexdigest(),
        "subset_registry_sha256": hashlib.sha256(subset_bytes).hexdigest(),
        "proof_entry": proof_entry,
        "command": command,
        "cargo_version": _tool_version(["cargo", "+1.89.0", "--version"]),
        "rustc_version": _tool_version(["rustc", "+1.89.0", "--version"]),
        "kani_version": _tool_version(["kani", "--version"]),
        "started_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
    }
    (output_root / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    stdout_path, stderr_path = output_root / "stdout.log", output_root / "stderr.log"
    sample_path = output_root / "resource-samples.tsv"
    cgroup_dir = _cgroup_memory_dir()
    with sample_path.open("w", encoding="utf-8") as samples:
        samples.write("utc\tmem_available_kib\tcgroup_current\tcgroup_peak\tcgroup_max\tcgroup_events\n")
        with stdout_path.open("w", encoding="utf-8") as stdout, stderr_path.open(
            "w", encoding="utf-8"
        ) as stderr:
            process = subprocess.Popen(command, cwd=repo, stdout=stdout, stderr=stderr)
            _record_resource_sample(samples, cgroup_dir)
            while process.poll() is None:
                time.sleep(5)
                _record_resource_sample(samples, cgroup_dir)
            return_code = process.wait()
    manifest["finished_utc"] = dt.datetime.now(dt.timezone.utc).isoformat()
    manifest["exit_code"] = return_code
    (output_root / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    return return_code


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--proof-id", required=True)
    parser.add_argument("--source-revision", required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        return run_diagnostic(args.repo, args.proof_id, args.out, args.source_revision)
    except (OSError, subprocess.SubprocessError, ValueError) as exc:
        print(f"E10 diagnostic refused: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
