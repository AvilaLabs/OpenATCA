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
from dataclasses import dataclass
from typing import Any

from rust_proof_suite import load_registry, validate_registry


BASELINE_REF = "8c5923da2f0c7f11ca84e99fc443980d86b4ef5c"
CURRENT_REF = "64c44e714bc47bdeb956e671d7e15aa4f06edfde"
TARGET_CONTEXT_REF = "82009cde336ab3f2cf5db3bd8897df90b2c2bdcb"
TARGET_CONTEXT_REGISTRY_SHA256 = "6463add26059e74ad8e84fe5e5ce6152f8bfbeae1e55f02cbbdb0a7d7cb0922d"
TARGET_CONTEXT_PIPELINE_SHA256 = {
    "tools/rust_proof_suite.py": "c599d6f90acdcedd7155af9f78ccc0f90c6908b9feaf1b5bfa9a5802059d6e94",
    "tools/kani_validation.py": "1914868906ff98957945796627676faf8d360b938780a6ffcce25e4e2a56d8cf",
    "tools/rust_proofs.json": TARGET_CONTEXT_REGISTRY_SHA256,
}
PROOF_IDS = (
    "late-append-output-projection",
    "large-move-decision-rejected-current-filter-snapshot",
)
DIAGNOSTIC_CASES = tuple(
    (revision, proof_id)
    for revision in (BASELINE_REF, CURRENT_REF)
    for proof_id in PROOF_IDS
)
TARGET_CONTEXT_PROOF_IDS = (
    "large-move-decision-rejected-current-filter-horizontal",
    "late-append-output-projection",
    "large-move-decision-rejected-current-filter-snapshot",
    "large-move-decision-rejected-anchor-filter-snapshot",
    "large-move-decision-rejected-last-time-and-updates",
)
TARGET_CONTEXT_ENTRY_SHA256 = {
    "large-move-decision-rejected-current-filter-horizontal": "35563458d41e3a124082c4dacbde2d8e7c6e326d5f45a52f7347a7cbdbf8aea1",
    "late-append-output-projection": "567a1db19ccb6a3d86745668835ff44a92cdea21281227ac1117956e55dd6698",
    "large-move-decision-rejected-current-filter-snapshot": "4a23863425dbdb12e24b5c785562d32af07a956f2e083270cd4af8cc9e9ccbec",
    "large-move-decision-rejected-anchor-filter-snapshot": "0dce24e6f043d2eee18d912a61d310abe878113d091ac070e98e305a1fbd66b8",
    "large-move-decision-rejected-last-time-and-updates": "30cdd8c748c3c5baa9a415b5a43044cf56286617eb4c1a7d28a717fefc2d9571",
}
TARGET_CONTEXTS = ("shared", "fresh")
TARGET_CONTEXT_CASES = tuple(
    (context, TARGET_CONTEXT_REF, TARGET_CONTEXT_PROOF_IDS)
    for context in TARGET_CONTEXTS
)
RESOURCE_SAMPLE_HEADER = (
    "utc\tmem_available_kib\tcgroup_current\tcgroup_peak\tcgroup_max\tcgroup_events"
    "\tmax_process_vmpeak_kib\tmax_process_vmrss_kib\tobserved_process_count"
)


def _canonical_sha256(value: Any) -> str:
    data = json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
    return hashlib.sha256(data).hexdigest()


def single_entry_registry(registry: dict[str, Any], proof_id: str) -> dict[str, Any]:
    return _single_entry_registry(registry, proof_id, PROOF_IDS)


def _single_entry_registry(
    registry: dict[str, Any], proof_id: str, allowed_ids: tuple[str, ...],
) -> dict[str, Any]:
    if proof_id not in allowed_ids:
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


def _process_tree_memory(root_pid: int, proc_root: Path = Path("/proc")) -> tuple[str, str, str]:
    children: dict[int, list[int]] = {}
    try:
        entries = list(proc_root.iterdir())
    except OSError:
        return "unavailable", "unavailable", "unavailable"
    for entry in entries:
        if not entry.name.isdigit():
            continue
        try:
            stat = (entry / "stat").read_text(encoding="ascii")
            close = stat.rfind(")")
            if close < 0:
                continue
            fields = stat[close + 1 :].split()
            if len(fields) < 2:
                continue
            parent = int(fields[1])
            children.setdefault(parent, []).append(int(entry.name))
        except (OSError, ValueError, IndexError):
            continue
    pending = [root_pid]
    pids: set[int] = set()
    while pending:
        pid = pending.pop()
        if pid in pids:
            continue
        pids.add(pid)
        pending.extend(children.get(pid, []))
    vmpeak: list[int] = []
    vmrss: list[int] = []
    for pid in pids:
        try:
            lines = (proc_root / str(pid) / "status").read_text(encoding="ascii").splitlines()
        except OSError:
            continue
        for line in lines:
            if line.startswith("VmPeak:"):
                try:
                    vmpeak.append(int(line.split()[1]))
                except (ValueError, IndexError):
                    pass
            elif line.startswith("VmRSS:"):
                try:
                    vmrss.append(int(line.split()[1]))
                except (ValueError, IndexError):
                    pass
    return (
        str(max(vmpeak)) if vmpeak else "unavailable",
        str(max(vmrss)) if vmrss else "unavailable",
        str(len(pids)),
    )


def _record_resource_sample(output: Any, directory: Path | None, root_pid: int) -> None:
    vmpeak, vmrss, process_count = _process_tree_memory(root_pid)
    values = (
        dt.datetime.now(dt.timezone.utc).isoformat(),
        _mem_available_kib(),
        _read_cgroup_value(directory, "memory.current"),
        _read_cgroup_value(directory, "memory.peak"),
        _read_cgroup_value(directory, "memory.max"),
        _read_cgroup_value(directory, "memory.events"),
        vmpeak,
        vmrss,
        process_count,
    )
    output.write("\t".join(values) + "\n")
    output.flush()


def _resource_summary(path: Path) -> dict[str, Any]:
    lines = path.read_text(encoding="utf-8").splitlines()
    if not lines or lines[0] != RESOURCE_SAMPLE_HEADER:
        return {"complete": False, "sample_count": 0}
    columns = lines[0].split("\t")
    indices = {name: columns.index(name) for name in columns}
    values = {name: [] for name in (
        "cgroup_current", "cgroup_peak", "max_process_vmpeak_kib", "max_process_vmrss_kib",
    )}
    unavailable = False
    for line in lines[1:]:
        fields = line.split("\t")
        if len(fields) != len(columns):
            unavailable = True
            continue
        for name in values:
            item = fields[indices[name]]
            if item == "unavailable":
                unavailable = True
            else:
                try:
                    values[name].append(int(item))
                except ValueError:
                    unavailable = True
        try:
            int(fields[indices["observed_process_count"]])
        except (ValueError, IndexError):
            unavailable = True
    return {
        "complete": bool(lines[1:]) and not unavailable,
        "sample_count": len(lines) - 1,
        "max_sampled_cgroup_current_bytes": max(values["cgroup_current"], default=None),
        "cgroup_peak_highwater_first_sample_bytes": values["cgroup_peak"][0] if values["cgroup_peak"] else None,
        "cgroup_peak_highwater_last_sample_bytes": values["cgroup_peak"][-1] if values["cgroup_peak"] else None,
        "max_sampled_process_vmpeak_kib": max(values["max_process_vmpeak_kib"], default=None),
        "max_sampled_process_vmrss_kib": max(values["max_process_vmrss_kib"], default=None),
    }


def target_directories_for_context(context: str, runner_temp: Path) -> dict[str, Path]:
    if context not in TARGET_CONTEXTS:
        raise ValueError("target context must be shared or fresh")
    runner_temp = runner_temp.resolve(strict=True)
    if context == "shared":
        target = runner_temp / "kani-target-context-shared"
        return {proof_id: target for proof_id in TARGET_CONTEXT_PROOF_IDS}
    return {
        proof_id: runner_temp / f"kani-target-context-fresh-{proof_id}"
        for proof_id in TARGET_CONTEXT_PROOF_IDS
    }


def _write_manifest(path: Path, value: dict[str, Any]) -> None:
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def _context_case_command(
    repo: Path, registry_path: Path, run_output: Path, target_dir: Path,
) -> list[str]:
    return [
        sys.executable, str(repo / "tools" / "rust_proof_suite.py"),
        "--repo", str(repo), "--registry", str(registry_path), "--shard", "0/1",
        "--out", str(run_output), "--target-dir", str(target_dir),
    ]


def _run_context_case(
    case: "ContextCase",
) -> dict[str, Any]:
    command = _context_case_command(case.repo, case.registry_path, case.run_output, case.target_dir)
    stdout_path = case.output_root / f"{case.proof_id}.stdout.log"
    stderr_path = case.output_root / f"{case.proof_id}.stderr.log"
    sample_path = case.output_root / f"{case.proof_id}.resource-samples.tsv"
    started = time.monotonic()
    print(json.dumps({"proof_id": case.proof_id, "event": "started"}), flush=True)
    with sample_path.open("w", encoding="utf-8") as samples:
        samples.write(RESOURCE_SAMPLE_HEADER + "\n")
        with stdout_path.open("w", encoding="utf-8") as stdout, stderr_path.open(
            "w", encoding="utf-8"
        ) as stderr:
            try:
                process = subprocess.Popen(command, cwd=case.repo, stdout=stdout, stderr=stderr)
            except OSError as exc:
                stderr.write(f"runner launch failed: {exc}\n")
                return_code = 127
            else:
                _record_resource_sample(samples, case.cgroup_dir, process.pid)
                while process.poll() is None:
                    time.sleep(case.sample_seconds)
                    _record_resource_sample(samples, case.cgroup_dir, process.pid)
                return_code = process.wait()
    suite_path = case.run_output / "suite.json"
    suite: dict[str, Any] | None = None
    if suite_path.is_file():
        try:
            suite = json.loads(suite_path.read_text(encoding="utf-8"))
        except (OSError, UnicodeDecodeError, json.JSONDecodeError):
            pass
    checks = suite.get("checks", []) if isinstance(suite, dict) else []
    check = checks[0] if len(checks) == 1 and isinstance(checks[0], dict) else {}
    expected_status = check.get("status") if check.get("id") == case.proof_id else "NO_RESULT"
    result = {
        "proof_id": case.proof_id,
        "proof_entry_sha256": _canonical_sha256(case.entry),
        "subset_registry_sha256": hashlib.sha256(case.registry_path.read_bytes()).hexdigest(),
        "status": expected_status,
        "suite_status": suite.get("status") if isinstance(suite, dict) else "NO_RESULT",
        "runner_exit_code": return_code,
        "elapsed_seconds": time.monotonic() - started,
        "target_directory": str(case.target_dir),
        "runner_command": command,
        "suite_sha256": hashlib.sha256(suite_path.read_bytes()).hexdigest() if suite_path.is_file() else None,
        "resource_samples": sample_path.name,
        "resource_summary": _resource_summary(sample_path),
    }
    print(json.dumps({"proof_id": case.proof_id, "event": "finished", "status": expected_status,
                      "runner_exit_code": return_code}), flush=True)
    return result


@dataclass(frozen=True)
class ContextCase:
    repo: Path
    proof_id: str
    entry: dict[str, Any]
    registry_path: Path
    run_output: Path
    target_dir: Path
    output_root: Path
    cgroup_dir: Path | None
    sample_seconds: int = 1


def run_target_context_comparison(
    repo: Path, context: str, output_root: Path, revision: str,
) -> int:
    if (context, revision, TARGET_CONTEXT_PROOF_IDS) not in TARGET_CONTEXT_CASES:
        raise ValueError("target-context run is outside the pinned matrix")
    repo = repo.resolve(strict=True)
    source_head = _git_head(repo)
    if source_head != TARGET_CONTEXT_REF or revision != TARGET_CONTEXT_REF:
        raise ValueError("target-context source must be the exact pinned full-registry revision")
    _require_clean_checkout(repo)
    registry_path = repo / "tools" / "rust_proofs.json"
    registry_bytes = registry_path.read_bytes()
    registry_hash = hashlib.sha256(registry_bytes).hexdigest()
    if registry_hash != TARGET_CONTEXT_REGISTRY_SHA256:
        raise ValueError("pinned target-context source registry digest changed")
    registry, errors = load_registry(registry_path)
    if errors or registry is None:
        raise ValueError("source registry failed validation: " + "; ".join(errors))
    pipeline_hashes = {
        f"tools/{name}": hashlib.sha256((repo / "tools" / name).read_bytes()).hexdigest()
        for name in ("rust_proof_suite.py", "kani_validation.py")
    }
    pipeline_hashes["tools/rust_proofs.json"] = registry_hash
    if pipeline_hashes != TARGET_CONTEXT_PIPELINE_SHA256:
        raise ValueError("pinned proof runner source digests changed")
    runner_temp = Path(os.environ["RUNNER_TEMP"]).resolve(strict=True)
    resolved_output = output_root.resolve()
    if resolved_output == runner_temp or runner_temp not in resolved_output.parents:
        raise ValueError("target-context evidence output must be a new child of RUNNER_TEMP")
    output_root.mkdir(parents=True, exist_ok=False)
    registry_dir = output_root / "registries"
    runs_dir = output_root / "suite-runs"
    registry_dir.mkdir()
    runs_dir.mkdir()
    targets = target_directories_for_context(context, runner_temp)
    for target in dict.fromkeys(targets.values()):
        if target.exists() or target.is_symlink():
            raise ValueError("target-context directory must be fresh and unused")
        target.mkdir()
    entries = {}
    for proof_id in TARGET_CONTEXT_PROOF_IDS:
        entry_registry = _single_entry_registry(registry, proof_id, TARGET_CONTEXT_PROOF_IDS)
        entry = entry_registry["proofs"][0]
        entry_hash = _canonical_sha256(entry)
        if entry_hash != TARGET_CONTEXT_ENTRY_SHA256[proof_id]:
            raise ValueError(f"pinned proof entry changed for {proof_id}")
        path = registry_dir / f"{proof_id}.json"
        path.write_text(json.dumps(entry_registry, indent=2) + "\n", encoding="utf-8")
        entries[proof_id] = (entry, path)
    manifest = {
        "diagnostic_version": 2,
        "mode": "target-context-comparison",
        "target_context": context,
        "source_revision": source_head,
        "selected_revision": revision,
        "full_registry_sha256": registry_hash,
        "pipeline_source_hashes": pipeline_hashes,
        "controller_source_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        "workflow_source_sha256": hashlib.sha256(
            (Path(__file__).resolve().parents[1] / ".github" / "workflows" / "rust-proofs.yml").read_bytes()
        ).hexdigest(),
        "cargo_version": _tool_version(["cargo", "+1.89.0", "--version"]),
        "rustc_version": _tool_version(["rustc", "+1.89.0", "--version"]),
        "kani_version": _tool_version(["kani", "--version"]),
        "ordered_proof_ids": list(TARGET_CONTEXT_PROOF_IDS),
        "preceding_16_gib_control_proof_id": TARGET_CONTEXT_PROOF_IDS[0],
        "preceding_16_gib_control_memory_mib": entries[TARGET_CONTEXT_PROOF_IDS[0]][0]["memory_mib"],
        "proof_entry_sha256": {pid: TARGET_CONTEXT_ENTRY_SHA256[pid] for pid in TARGET_CONTEXT_PROOF_IDS},
        "target_directory_by_proof": {pid: str(targets[pid]) for pid in TARGET_CONTEXT_PROOF_IDS},
        "target_context_semantics": (
            "one target directory reused sequentially by all five proof entries"
            if context == "shared" else "one new target directory per proof entry"
        ),
        "proof_runs": [],
        "started_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
    }
    manifest_path = output_root / "context-manifest.json"
    _write_manifest(manifest_path, manifest)
    cgroup_dir = _cgroup_memory_dir()
    for proof_id in TARGET_CONTEXT_PROOF_IDS:
        entry, registry_subset = entries[proof_id]
        run_output = runs_dir / proof_id
        case = _run_context_case(
            ContextCase(
                repo, proof_id, entry, registry_subset, run_output, targets[proof_id],
                output_root, cgroup_dir,
            )
        )
        manifest["proof_runs"].append(case)
        _write_manifest(manifest_path, manifest)
    manifest["finished_utc"] = dt.datetime.now(dt.timezone.utc).isoformat()
    manifest["exit_code"] = 0 if all(
        row["status"] == "PASS" and row["runner_exit_code"] == 0
        and row["resource_summary"]["complete"]
        for row in manifest["proof_runs"]
    ) else 1
    _write_manifest(manifest_path, manifest)
    return manifest["exit_code"]


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
        samples.write(RESOURCE_SAMPLE_HEADER + "\n")
        with stdout_path.open("w", encoding="utf-8") as stdout, stderr_path.open(
            "w", encoding="utf-8"
        ) as stderr:
            process = subprocess.Popen(command, cwd=repo, stdout=stdout, stderr=stderr)
            _record_resource_sample(samples, cgroup_dir, process.pid)
            while process.poll() is None:
                time.sleep(5)
                _record_resource_sample(samples, cgroup_dir, process.pid)
            return_code = process.wait()
    manifest["finished_utc"] = dt.datetime.now(dt.timezone.utc).isoformat()
    manifest["exit_code"] = return_code
    (output_root / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    return return_code


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--proof-id")
    parser.add_argument("--target-context", choices=TARGET_CONTEXTS)
    parser.add_argument("--source-revision", required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        if (args.proof_id is None) == (args.target_context is None):
            raise ValueError("select exactly one of --proof-id or --target-context")
        if args.target_context is not None:
            return run_target_context_comparison(
                args.repo, args.target_context, args.out, args.source_revision
            )
        return run_diagnostic(args.repo, args.proof_id, args.out, args.source_revision)
    except (OSError, subprocess.SubprocessError, ValueError) as exc:
        print(f"E10 diagnostic refused: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
