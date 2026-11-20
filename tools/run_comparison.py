#!/usr/bin/env python3
"""Build and run the offline ADS-B kinematic component comparison.

This orchestrator preserves the artifacts and provenance for a development comparison. Its
constant-velocity Python control is not a professional tracker reference, and replay-call cost is
not live latency. The supplied root caller owns any outer resource guard.
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import math
import os
import signal
import subprocess
import sys
import threading
import time
from pathlib import Path
from typing import Any

# The sealed hold-out guard (tools/sealed_guard.py); the script's own directory may not be on the path.
sys.path.insert(0, str(Path(__file__).resolve().parent))
import sealed_guard  # noqa: E402

# Rows in a sealed time window are dropped and counted; the line is printed once at exit.
DROPS = sealed_guard.DropCounter()


SCENARIOS = ("straight", "turn", "acceleration", "crossing", "dropout", "delayed")
COMPARISON_CLASS = "development_control"
MAX_LOG_BYTES = 8 * 1024 * 1024
MAX_JSON_BYTES = 16 * 1024 * 1024
BUILD_TIMEOUT_S = 1200
RUN_TIMEOUT_S = 600
PYTHON_TIMEOUT_S = 600
MAX_AIRCRAFT_COUNT = 300
MAX_DURATION_S = 1200
RUNNER_REL = Path("validation/oa-comparison-runner")
PROTOCOL_REL = Path("validation/comparison-protocol-v1.json")
# Freeze: must equal comparison_evaluator.PROTOCOL_V1_SHA256 (a test pins the two together).
PROTOCOL_V1_SHA256 = "5463f6954a5a0dcbfa8de5034242b7c4c8a8c73f4ea06c2920a80b90171c2f46"
PROTOCOL_MISMATCH_REASON = "protocol file does not match the frozen v1 hash"
STATUS_COMPLETED = "completed"
STATUS_EXPLORATORY = "exploratory (source tree not clean)"
EXIT_NOT_COMPLETED = 3
REFERENCE_TOOL_REL = Path("tools/comparison_reference.py")
BASELINE_TOOL_REL = Path("tools/comparison_baseline.py")
EVALUATOR_TOOL_REL = Path("tools/comparison_evaluator.py")
REQUIRED_OUTPUTS = (
    "truth.csv",
    "measurements.csv",
    "candidate.csv",
    "timing.csv",
    "identity_binding.csv",
    "manifest.json",
)


class ComparisonRunError(RuntimeError):
    pass


def _sha256(path: Path) -> str:
    h = hashlib.sha256()
    with sealed_guard.open_read(path, "rb") as f:
        for block in iter(lambda: f.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def _canonical_sha256(value: Any) -> str:
    data = json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
    return hashlib.sha256(data).hexdigest()


def _read_json(path: Path) -> dict[str, Any]:
    if path.stat().st_size > MAX_JSON_BYTES:
        raise ComparisonRunError(f"{path.name} exceeds the JSON artifact size limit")
    def reject_constant(value: str) -> None:
        raise ValueError(f"invalid JSON constant {value}")
    try:
        with sealed_guard.open_read(path, "r", encoding="utf-8") as f:
            value = json.loads(f.read(), parse_constant=reject_constant)
    except (OSError, UnicodeError, json.JSONDecodeError, ValueError) as exc:
        raise ComparisonRunError(f"cannot parse {path.name}: {exc}") from exc
    if not isinstance(value, dict):
        raise ComparisonRunError(f"{path.name} must contain a JSON object")
    return value


def _write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temp = path.with_name(path.name + ".tmp")
    with temp.open("w", encoding="utf-8", newline="\n") as f:
        json.dump(value, f, indent=2, sort_keys=True, allow_nan=False)
        f.write("\n")
    temp.replace(path)


def _inside(path: Path, parent: Path) -> bool:
    try:
        path.relative_to(parent)
        return True
    except ValueError:
        return False


def _external_path(path: Path, repo: Path, label: str) -> Path:
    resolved = path.expanduser().resolve()
    if _inside(resolved, repo):
        raise ComparisonRunError(f"{label} must be outside the repository checkout: {resolved}")
    return resolved


def _source_snapshot(repo: Path) -> dict[str, Any]:
    def git(*args: str) -> str:
        p = subprocess.run(
            ["git", "-C", str(repo), *args],
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            timeout=15,
        )
        return p.stdout

    head = git("rev-parse", "HEAD").strip()
    status = git("status", "--porcelain=v1", "--untracked-files=all")
    tracked = git("ls-files", "-z").split("\0")
    content = hashlib.sha256()
    for rel in sorted(x for x in tracked if x):
        p = repo / rel
        content.update(rel.encode("utf-8", "surrogateescape"))
        content.update(b"\0")
        if p.is_file():
            content.update(_sha256(p).encode("ascii"))
        content.update(b"\0")
    patch = git("diff", "--binary", "HEAD", "--")
    untracked_hashes = []
    for line in status.splitlines():
        if line.startswith("?? "):
            rel = line[3:]
            p = repo / rel
            if p.is_file():
                untracked_hashes.append((rel, _sha256(p)))
    snapshot = {
        "head": head,
        "status_porcelain": status,
        "tracked_content_sha256": content.hexdigest(),
        "diff_head_sha256": hashlib.sha256(patch.encode("utf-8", "surrogateescape")).hexdigest(),
        "untracked_file_sha256": untracked_hashes,
    }
    snapshot["snapshot_sha256"] = _canonical_sha256(snapshot)
    return snapshot


class CommandRecorder:
    def __init__(self, out: Path):
        self.out = out
        self.records: list[dict[str, Any]] = []

    def run(
        self,
        label: str,
        command: list[str],
        *,
        cwd: Path,
        env: dict[str, str] | None = None,
        timeout: int,
        expect_success: bool = True,
    ) -> dict[str, Any]:
        number = len(self.records) + 1
        stdout_path = self.out / "logs" / f"{number:03d}-{label}.stdout.txt"
        stderr_path = self.out / "logs" / f"{number:03d}-{label}.stderr.txt"
        stdout_path.parent.mkdir(parents=True, exist_ok=True)
        started = time.monotonic_ns()
        timed_out = False
        output_limit_exceeded = False
        exit_code: int | None = None
        overflow = threading.Event()
        try:
            proc = subprocess.Popen(
                command,
                cwd=cwd,
                env=env,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                start_new_session=(os.name == "posix"),
            )
        except OSError as exc:
            stdout_path.write_bytes(b"")
            stderr_path.write_text(f"could not start subprocess: {exc}\n", encoding="utf-8")
            exit_code = 127
            proc = None

        def stop_process_group() -> None:
            if proc is None:
                return
            try:
                if os.name == "posix":
                    os.killpg(proc.pid, signal.SIGKILL)
                elif proc.poll() is None:
                    proc.kill()
            except ProcessLookupError:
                pass

        def drain(pipe, path: Path) -> None:
            written = 0
            sink_failed = False
            with path.open("wb") as sink:
                while True:
                    try:
                        chunk = pipe.read(64 * 1024)
                    except OSError:
                        overflow.set()
                        break
                    if not chunk:
                        break
                    remaining = MAX_LOG_BYTES - written
                    if remaining > 0 and not sink_failed:
                        keep = chunk[:remaining]
                        try:
                            sink.write(keep)
                            written += len(keep)
                        except OSError:
                            overflow.set()
                            sink_failed = True
                    if len(chunk) > max(remaining, 0):
                        overflow.set()
                sink.flush()

        readers: list[threading.Thread] = []
        descendants_stopped = False
        if proc is not None:
            readers = [
                threading.Thread(target=drain, args=(proc.stdout, stdout_path), daemon=True),
                threading.Thread(target=drain, args=(proc.stderr, stderr_path), daemon=True),
            ]
            for reader in readers:
                reader.start()
            deadline = time.monotonic() + timeout
            while proc.poll() is None:
                if overflow.is_set():
                    output_limit_exceeded = True
                    stop_process_group()
                    break
                if time.monotonic() >= deadline:
                    timed_out = True
                    stop_process_group()
                    break
                time.sleep(0.02)
            try:
                exit_code = proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                stop_process_group()
                exit_code = proc.wait(timeout=5)
            if overflow.is_set():
                output_limit_exceeded = True
            for reader in readers:
                reader.join(timeout=5)
            if any(reader.is_alive() for reader in readers):
                descendants_stopped = True
                stop_process_group()
                for stream in (proc.stdout, proc.stderr):
                    try:
                        stream.close()
                    except OSError:
                        pass
                for reader in readers:
                    reader.join(timeout=1)
            if overflow.is_set():
                output_limit_exceeded = True
            for stream in (proc.stdout, proc.stderr):
                try:
                    stream.close()
                except OSError:
                    pass
        ended = time.monotonic_ns()
        record = {
            "label": label,
            "argv": command,
            "cwd": str(cwd),
            "timeout_s": timeout,
            "timed_out": timed_out,
            "output_limit_exceeded": output_limit_exceeded,
            "descendant_process_group_stopped": descendants_stopped,
            "exit_code": exit_code,
            "elapsed_ns": ended - started,
            "stdout": str(stdout_path.resolve()),
            "stdout_sha256": _sha256(stdout_path),
            "stderr": str(stderr_path.resolve()),
            "stderr_sha256": _sha256(stderr_path),
        }
        self.records.append(record)
        _write_json(self.out / "commands.json", self.records)
        if timed_out:
            raise ComparisonRunError(f"{label} timed out after {timeout}s; process group stopped and logs retained")
        if output_limit_exceeded:
            raise ComparisonRunError(f"{label} exceeded the live output limit; process group stopped and logs retained")
        if descendants_stopped and not timed_out:
            raise ComparisonRunError(f"{label} left descendant processes holding output pipes; process group stopped and logs retained")
        if expect_success and exit_code != 0:
            raise ComparisonRunError(f"{label} exited {exit_code}; logs retained")
        return record


def _hash_artifact(path: Path) -> dict[str, str]:
    return {"path": str(path.resolve()), "sha256": _sha256(path)}


def _csv_keys(path: Path, header: list[str]) -> list[dict[str, str]]:
    with sealed_guard.open_read(path, "r", encoding="utf-8-sig", newline="") as f:
        reader = csv.DictReader(f, strict=True)
        if reader.fieldnames != header:
            raise ComparisonRunError(f"{path.name}: unexpected CSV header")
        times = tuple(c for c in header if c in ("time_ns", "score_ns"))
        rows = []
        for row in reader:
            if sealed_guard.row_in_window(row, times, ("source_ns", "publish_ns") if "source_ns" in header else (), keep_malformed=True):
                DROPS.add()
                continue
            rows.append(row)
        return rows


def _create_truth_grid(truth_path: Path, output_path: Path) -> None:
    rows = _csv_keys(
        truth_path,
        ["scenario_id", "flight_id", "time_ns", "lat_deg", "lon_deg", "ve_mps", "vn_mps", "phase"],
    )
    output_path.parent.mkdir(parents=True, exist_ok=True)
    with output_path.open("w", encoding="utf-8", newline="") as f:
        writer = csv.DictWriter(f, fieldnames=["scenario_id", "flight_id", "time_ns"], lineterminator="\n")
        writer.writeheader()
        for row in rows:
            writer.writerow({k: row[k] for k in ("scenario_id", "flight_id", "time_ns")})


def _create_identity_binding(candidate_identity: Path, candidate: Path, baseline: Path, grid: Path, out: Path) -> None:
    fields = ["system", "scenario_id", "flight_id", "score_ns", "track_id", "truth_flight_id"]
    identity_rows = _csv_keys(candidate_identity, fields)
    candidate_rows = _csv_keys(candidate, ["scenario_id", "flight_id", "score_ns", "source_ns", "publish_ns", "lat_deg", "lon_deg", "ve_mps", "vn_mps", "status"])
    baseline_rows = _csv_keys(baseline, ["scenario_id", "flight_id", "score_ns", "source_ns", "publish_ns", "lat_deg", "lon_deg", "ve_mps", "vn_mps", "status"])
    grid_rows = _csv_keys(grid, ["scenario_id", "flight_id", "time_ns"])
    cand_keys = {(r["scenario_id"], r["flight_id"], r["score_ns"]) for r in candidate_rows}
    base_keys = {(r["scenario_id"], r["flight_id"], r["score_ns"]) for r in baseline_rows}
    expected = {(r["scenario_id"], r["flight_id"], r["time_ns"]) for r in grid_rows}
    if cand_keys != expected or base_keys != expected:
        raise ComparisonRunError("candidate or baseline output grid does not exactly match truth grid")
    normalized = []
    seen_candidate = set()
    for row in identity_rows:
        if row["system"] not in ("openatca_component", "candidate"):
            raise ComparisonRunError(f"unexpected candidate identity system {row['system']!r}")
        key = (row["scenario_id"], row["flight_id"], row["score_ns"])
        if key not in cand_keys or key in seen_candidate or row["truth_flight_id"] != row["flight_id"]:
            raise ComparisonRunError("candidate identity binding is missing, duplicate, or inconsistent")
        seen_candidate.add(key)
        normalized.append({**row, "system": "candidate"})
    if seen_candidate != cand_keys:
        raise ComparisonRunError("candidate identity binding does not cover every candidate output")
    for row in baseline_rows:
        normalized.append(
            {
                "system": "baseline",
                "scenario_id": row["scenario_id"],
                "flight_id": row["flight_id"],
                "score_ns": row["score_ns"],
                "track_id": row["flight_id"] if row["status"] != "missing" else "",
                "truth_flight_id": row["flight_id"],
            }
        )
    out.parent.mkdir(parents=True, exist_ok=True)
    with out.open("w", encoding="utf-8", newline="") as f:
        writer = csv.DictWriter(f, fieldnames=fields, lineterminator="\n")
        writer.writeheader()
        writer.writerows(normalized)


def _loss_counts(manifest: dict[str, Any]) -> dict[str, Any]:
    losses = manifest.get("loss_counters")
    if not isinstance(losses, dict) or not losses or any(
        not isinstance(v, (int, float)) or isinstance(v, bool) or not math.isfinite(v)
        for v in losses.values()
    ):
        raise ComparisonRunError("runner manifest has no explicit loss_counters object")
    return losses


def _competitive_pass(results: list[dict[str, Any]]) -> bool:
    eligible = [r for r in results if r.get("scenario_role") != "diagnostic_only"]
    return bool(eligible) and all(
        r.get("profile_gate", {}).get("status") == "nominal_profile_pass"
        and r.get("profile_gate", {}).get("usable_keysets_match") is True
        and r.get("accuracy_qualification", {}).get("pass") is True
        and r.get("accuracy_qualification", {}).get("all_quality_pass") is True
        and r.get("fault_mode_cohort_status") == "PASS"
        and r.get("speed_coverage", {}).get("pass") is True
        for r in eligible
    )


def _require_gate_exit(results: list[dict[str, Any]], require: bool) -> int:
    return 1 if require and not _competitive_pass(results) else 0


def _check_protocol_hash(recorded: str) -> None:
    if recorded != PROTOCOL_V1_SHA256:
        raise ComparisonRunError(PROTOCOL_MISMATCH_REASON)


def _final_status(clean: bool, reference_failure: bool, gate_failed: bool) -> str:
    """Run status; only a clean, reference-validated, gate-satisfying run is 'completed'."""
    if reference_failure:
        return "reference_failure"
    if gate_failed:
        return "competitive_gate_failed"
    if not clean:
        return STATUS_EXPLORATORY
    return STATUS_COMPLETED


def _exit_code(status: str) -> int:
    """Zero only for a completed run; every other status exits 3."""
    return 0 if status == STATUS_COMPLETED else EXIT_NOT_COMPLETED


def _reference_qualified(
    result: dict[str, Any], source_hash: str, truth_hash: str, measurements_hash: str, protocol_hash: str
) -> bool:
    bound_hashes = result.get("hashes", {})
    return (
        result.get("status") == "PASS"
        and result.get("validation_status") == "PASS"
        and result.get("reference_kind") == "independent_simulation"
        and result.get("scope") == "declared_model_only"
        and result.get("physical_accuracy_claim") is False
        and result.get("runner_source_hash_verified") is True
        and result.get("runner_source_sha256") == source_hash
        and isinstance(bound_hashes, dict)
        and bound_hashes.get("truth_sha256") == truth_hash
        and bound_hashes.get("measurements_sha256") == measurements_hash
        and bound_hashes.get("protocol_sha256") == protocol_hash
        and bound_hashes.get("runner_source_sha256") == source_hash
    )


def _build_command(repo: Path, target: Path) -> list[str]:
    return [
        "cargo", "+1.89.0", "build", "--locked", "--offline", "--release",
        "--manifest-path", str(repo / RUNNER_REL / "Cargo.toml"), "--target-dir", str(target),
    ]


def _build_environment(target: Path) -> dict[str, str]:
    return {**os.environ, "CARGO_BUILD_JOBS": "1", "CARGO_TARGET_DIR": str(target), "CARGO_NET_OFFLINE": "true"}


def _selected_scenarios(scenario: str) -> tuple[str, ...]:
    return SCENARIOS if scenario == "all" else (scenario,)


def _case_output_path(out: Path, scenario: str) -> Path:
    return out / "cases" / scenario


def _validate_runner_bounds(aircraft_count: int, duration_s: int) -> None:
    if aircraft_count < 1 or aircraft_count > MAX_AIRCRAFT_COUNT:
        raise ComparisonRunError(f"--aircraft-count must be in 1..{MAX_AIRCRAFT_COUNT}")
    if duration_s < 1 or duration_s > MAX_DURATION_S:
        raise ComparisonRunError(f"--duration-s must be in 1..{MAX_DURATION_S}")


def _development_cohort_id(protocol: dict[str, Any], seed: int) -> str:
    seeds = protocol.get("seeds_and_holdout", {}).get("development_seeds", {})
    declared = seeds.get("seed_ids", []) if isinstance(seeds, dict) else []
    if isinstance(seed, bool) or not isinstance(seed, int) or seed not in declared:
        raise ComparisonRunError("development wrapper accepts only frozen development seed IDs")
    return str(seeds.get("set_id"))


def _cohort_id_for_run(protocol: dict[str, Any], runner_manifest: dict[str, Any], requested_seed: int, scenario: str) -> str:
    if runner_manifest.get("seed") != requested_seed or runner_manifest.get("scenario_id") != scenario:
        raise ComparisonRunError("runner manifest seed/scenario does not match the requested development run")
    cohort_id = _development_cohort_id(protocol, requested_seed)
    if requested_seed in protocol.get("seeds_and_holdout", {}).get("qualification", {}).get("seed_ids", []):
        raise ComparisonRunError("qualification seeds may not be launched by the development wrapper")
    fault = protocol.get("fault_mode_cohort", {})
    if requested_seed in fault.get("seed_ids", []):
        raise ComparisonRunError("sealed fault-cohort seeds may not be launched by the development wrapper")
    return cohort_id


def _scenario_role(protocol: dict[str, Any], scenario: str) -> str:
    scoring = protocol.get("initialization_and_qualification", {})
    nominal = scoring.get("nominal_scenarios", {})
    diagnostic = scoring.get("diagnostic_scenarios", {})
    if scenario in nominal and nominal[scenario].get("role") == "qualification":
        return "nominal"
    if scenario in diagnostic and diagnostic[scenario].get("role") == "diagnostic_only":
        return "diagnostic_only"
    raise ComparisonRunError(f"scenario {scenario!r} has no frozen scoring role")


def _speed_claim(timing_kind: str | None, timing_ledgers: dict[str, Any] | None = None) -> str:
    if timing_ledgers:
        return "unknown: this development wrapper does not qualify external first-use timing ledgers"
    if timing_kind == "replay_call_cost":
        return "unknown: replay_call_cost is not first-use wall timing"
    return "unknown: no validated first-use wall timing ledgers were supplied"


def _run_case(
    repo: Path,
    case_out: Path,
    runner_binary: Path,
    scenario: str,
    seed: int,
    aircraft_count: int,
    duration_s: int,
    recorder: CommandRecorder,
    source_hash: str,
    protocol_path: Path,
) -> dict[str, Any]:
    case_out.mkdir(parents=True, exist_ok=False)
    runner_out = case_out / "runner"
    recorder.run(
        f"runner-{scenario}",
        [str(runner_binary), "--seed", str(seed), "--aircraft-count", str(aircraft_count), "--duration-s", str(duration_s), "--scenario", scenario, "--out", str(runner_out)],
        cwd=repo,
        timeout=RUN_TIMEOUT_S,
    )
    for name in REQUIRED_OUTPUTS:
        if not (runner_out / name).is_file():
            raise ComparisonRunError(f"runner did not produce required artifact {name}")
    truth = runner_out / "truth.csv"
    measurements = runner_out / "measurements.csv"
    candidate = runner_out / "candidate.csv"
    runner_manifest_path = runner_out / "manifest.json"
    runner_manifest = _read_json(runner_manifest_path)
    protocol_data = _read_json(protocol_path)
    cohort_id = _cohort_id_for_run(protocol_data, runner_manifest, seed, scenario)
    scenario_role = _scenario_role(protocol_data, scenario)
    loss_counters = _loss_counts(runner_manifest)
    grid = case_out / "grid.csv"
    _create_truth_grid(truth, grid)

    reference_provenance = case_out / "reference_provenance.json"
    ref_command = recorder.run(
        f"reference-{scenario}",
        [sys.executable, str(repo / REFERENCE_TOOL_REL), "--truth", str(truth), "--measurements", str(measurements), "--manifest", str(runner_manifest_path), "--protocol", str(protocol_path), "--out", str(reference_provenance), "--runner-source", str(repo / RUNNER_REL / "src" / "main.rs")],
        cwd=repo,
        env={**os.environ, "PYTHONPATH": str(repo / "tools")},
        timeout=PYTHON_TIMEOUT_S,
        expect_success=False,
    )
    reference_result = _read_json(reference_provenance)
    if reference_result.get("status") not in ("PASS", "FAIL", "UNQUALIFIED"):
        raise ComparisonRunError("reference validator did not return a recognized status")
    expected_ref_exit = 0 if reference_result["status"] == "PASS" else 1
    if ref_command["exit_code"] != expected_ref_exit:
        raise ComparisonRunError("reference validator exit code contradicts its evidence status")
    _check_protocol_hash(_sha256(protocol_path))
    reference_qualified = _reference_qualified(
        reference_result,
        source_hash,
        _sha256(truth),
        _sha256(measurements),
        _sha256(protocol_path),
    )

    baseline = case_out / "baseline.csv"
    baseline_manifest = case_out / "baseline.json"
    recorder.run(
        f"baseline-{scenario}",
        [sys.executable, str(repo / BASELINE_TOOL_REL), "--grid", str(grid), "--measurements", str(measurements), "--out", str(baseline), "--manifest", str(baseline_manifest)],
        cwd=repo,
        env={**os.environ, "PYTHONPATH": str(repo / "tools")},
        timeout=PYTHON_TIMEOUT_S,
    )
    identity_binding = case_out / "identity_binding.csv"
    _create_identity_binding(runner_out / "identity_binding.csv", candidate, baseline, grid, identity_binding)

    config = {
        "protocol_id": "openatca-comparison",
        "protocol_version": protocol_data["version"],
        "profile": "adsb_kinematic_v1",
        "runner_model_id": runner_manifest.get("model_id"),
        "scenario": scenario,
        "seed": seed,
        "aircraft_count": aircraft_count,
        "duration_s": duration_s,
        "runner_source_sha256": source_hash,
        "cohort_id": cohort_id,
        "scenario_role": scenario_role,
    }
    config_path = case_out / "configuration.json"
    _write_json(config_path, config)
    epoch_ns = runner_manifest.get("grid_start_ns")
    if (
        runner_manifest.get("scenario_id") != scenario
        or isinstance(epoch_ns, bool)
        or not isinstance(epoch_ns, int)
        or runner_manifest.get("score_origin_ns", epoch_ns) != epoch_ns
    ):
        raise ComparisonRunError("runner manifest has an invalid scenario or score epoch")
    scenario_epochs = {scenario: epoch_ns}
    declared_epochs = runner_manifest.get("scenario_epochs_ns")
    if declared_epochs is not None and declared_epochs != scenario_epochs:
        raise ComparisonRunError("runner manifest scenario_epochs_ns does not match score origin")
    comparison_manifest = {
        **runner_manifest,
        "scenario_epochs_ns": scenario_epochs,
        "initialization_ns": 2_000_000_000,
        "coast_max_age_ns": 2_000_000_000,
        "truth_sha256": _sha256(truth),
        "measurements_sha256": _sha256(measurements),
        "raw_manifest_sha256": _sha256(runner_manifest_path),
        "configuration_sha256": _sha256(config_path),
        "cohort_id": cohort_id,
        "scenario_role": scenario_role,
    }
    comparison_manifest_path = case_out / "comparison_manifest.json"
    _write_json(comparison_manifest_path, comparison_manifest)
    artifacts = {
        "truth": _hash_artifact(truth),
        "measurements": _hash_artifact(measurements),
        "candidate": _hash_artifact(candidate),
        "baseline": _hash_artifact(baseline),
        "manifest": _hash_artifact(runner_manifest_path),
        "comparison_manifest": _hash_artifact(comparison_manifest_path),
        "protocol": _hash_artifact(protocol_path),
        "reference_provenance": _hash_artifact(reference_provenance),
        "identity_binding": _hash_artifact(identity_binding),
        "grid": _hash_artifact(grid),
        "candidate_identity_raw": _hash_artifact(runner_out / "identity_binding.csv"),
        "configuration": _hash_artifact(config_path),
        "baseline_manifest": _hash_artifact(baseline_manifest),
        "runner_timing": _hash_artifact(runner_out / "timing.csv"),
        "runner_binary": _hash_artifact(runner_binary),
        "runner_source": _hash_artifact(repo / RUNNER_REL / "src" / "main.rs"),
    }
    metadata = {
        "schema": "openatca-comparison-metadata-v1",
        "comparison_class": COMPARISON_CLASS,
        "reference_qualified": reference_qualified,
        "reference_validation_status": reference_result.get("status"),
        "reference_kind": "independent_simulation",
        "scope": "declared_model_only",
        "scenario_role": scenario_role,
        "cohort_id": cohort_id,
        "initialization_ns": 2_000_000_000,
        "coast_max_age_ns": 2_000_000_000,
        "scenario_epochs_ns": scenario_epochs,
        "fault_mode_cohort_status": protocol_data.get("fault_mode_cohort", {}).get("status"),
        "input_fingerprint": artifacts["measurements"]["sha256"],
        "same_input_fingerprint": True,
        "identity_source": "scenario_manifest_not_candidate_output",
        "clock_kind": "replay_call_cost",
        "loss_counters": loss_counters,
        "artifacts": artifacts,
    }
    flat = {
        "truth": "truth_sha256",
        "measurements": "measurements_sha256",
        "candidate": "candidate_sha256",
        "baseline": "baseline_sha256",
        "manifest": "manifest_sha256",
        "comparison_manifest": "comparison_manifest_sha256",
        "protocol": "protocol_sha256",
        "reference_provenance": "reference_provenance_sha256",
        "identity_binding": "identity_binding_sha256",
    }
    for name, field in flat.items():
        metadata[field] = artifacts[name]["sha256"]
    _check_protocol_hash(metadata["protocol_sha256"])
    metadata_path = case_out / "metadata.json"
    _write_json(metadata_path, metadata)
    comparison_path = case_out / "comparison.json"
    recorder.run(
        f"evaluator-{scenario}",
        [sys.executable, str(repo / EVALUATOR_TOOL_REL), "--truth", str(truth), "--measurements", str(measurements), "--candidate", str(candidate), "--baseline", str(baseline), "--metadata", str(metadata_path), "--out", str(comparison_path)],
        cwd=repo,
        env={**os.environ, "PYTHONPATH": str(repo / "tools")},
        timeout=PYTHON_TIMEOUT_S,
    )
    result = _read_json(comparison_path)
    return {
        "scenario": scenario,
        "scenario_role": scenario_role,
        "cohort_id": cohort_id,
        "fault_mode_cohort_status": protocol_data.get("fault_mode_cohort", {}).get("status"),
        "runner_model_id": runner_manifest.get("model_id"),
        "status": "completed_reference_validated" if reference_qualified else "completed_reference_failure",
        "reference_validation_status": reference_result.get("status"),
        "comparison_class": metadata["comparison_class"],
        "accuracy_qualification": result.get("accuracy_qualification"),
        "speed_coverage": result.get("speed_coverage"),
        "profile_gate": result.get("profile_qualification"),
        "post_initialization_metrics": {
            "candidate": result.get("logical_metrics", {}).get("candidate", {}).get("post_initialization_coverage"),
            "baseline": result.get("logical_metrics", {}).get("baseline", {}).get("post_initialization_coverage"),
        },
        "accounting": result.get("accounting"),
        "loss_counters": loss_counters,
        "artifact_directory": str(case_out.resolve()),
        "comparison": str(comparison_path.resolve()),
        "comparison_sha256": _sha256(comparison_path),
        "metadata": str(metadata_path.resolve()),
        "metadata_sha256": _sha256(metadata_path),
        "artifact_hashes": {name: spec["sha256"] for name, spec in artifacts.items()},
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--out", type=Path, required=True, help="new output directory outside the checkout")
    parser.add_argument("--target-dir", type=Path)
    parser.add_argument("--seed", type=int, default=1)
    parser.add_argument("--scenario", choices=(*SCENARIOS, "all"), default="straight")
    parser.add_argument("--aircraft-count", type=int, default=8)
    parser.add_argument("--duration-s", type=int, default=60)
    parser.add_argument("--runner", type=Path, help="optional prebuilt standalone runner")
    parser.add_argument("--require-competitive-pass", action="store_true")
    return parser


def run(args: argparse.Namespace) -> int:
    repo = args.repo.expanduser().resolve()
    if not (repo / ".git").exists():
        raise ComparisonRunError("--repo must name a Git checkout")
    if not (repo / RUNNER_REL / "Cargo.toml").is_file():
        raise ComparisonRunError(f"missing runner crate: {repo / RUNNER_REL / 'Cargo.toml'}")
    for rel in (PROTOCOL_REL, REFERENCE_TOOL_REL, BASELINE_TOOL_REL, EVALUATOR_TOOL_REL):
        if not (repo / rel).is_file():
            raise ComparisonRunError(f"required comparison input is missing: {repo / rel}")
    out = _external_path(args.out, repo, "--out")
    if out.exists():
        raise ComparisonRunError(f"--out must be a fresh path: {out}")
    target_default = Path(os.environ.get("TMPDIR", "/tmp")) / "openatca-comparison" / "checkouts" / hashlib.sha256(str(repo).encode()).hexdigest()
    target = _external_path(args.target_dir or target_default, repo, "--target-dir")
    if _inside(target, out) or _inside(out, target):
        raise ComparisonRunError("--out and --target-dir must be separate paths")
    _validate_runner_bounds(args.aircraft_count, args.duration_s)
    if args.seed < 0:
        raise ComparisonRunError("--seed must be nonnegative")
    if args.runner and not args.runner.expanduser().resolve().is_file():
        raise ComparisonRunError("--runner must identify an existing prebuilt binary")
    out.parent.mkdir(parents=True, exist_ok=True)
    out.mkdir(parents=False, exist_ok=False)
    recorder = CommandRecorder(out)
    start_snapshot = None
    results: list[dict[str, Any]] = []
    clean = False
    binary: Path
    build_record: dict[str, Any] | None = None
    try:
        start_snapshot = _source_snapshot(repo)
        _write_json(out / "source-before.json", start_snapshot)
        clean = not start_snapshot["status_porcelain"]
        _check_protocol_hash(_sha256(repo / PROTOCOL_REL))
        protocol = _read_json(repo / PROTOCOL_REL)
        protocol_version = protocol.get("version")
        if protocol.get("protocol_id") != "openatca-comparison" or not isinstance(protocol_version, str):
            raise ComparisonRunError("protocol identity or version is invalid")
        _development_cohort_id(protocol, args.seed)
        if args.runner:
            binary = args.runner.expanduser().resolve()
            build_kind = "prebuilt_external_binary"
        else:
            cargo_check = recorder.run(
                "cargo-version",
                ["cargo", "+1.89.0", "--version"],
                cwd=repo,
                timeout=30,
            )
            # Version output is retained in stdout; require exact Cargo 1.89.x identity.
            version_text = Path(cargo_check["stdout"]).read_text(encoding="utf-8", errors="replace")
            if "cargo 1.89.0" not in version_text:
                raise ComparisonRunError("cargo +1.89.0 did not report Cargo 1.89.0")
            target.mkdir(parents=True, exist_ok=True)
            env = _build_environment(target)
            build_record = recorder.run(
                "build-runner",
                _build_command(repo, target),
                cwd=repo,
                env=env,
                timeout=BUILD_TIMEOUT_S,
            )
            binary = target / "release" / "oa-comparison-runner"
            if not binary.is_file():
                raise ComparisonRunError(f"release runner binary was not produced: {binary}")
            build_kind = "cargo_1_89_0_locked_offline_release"
        binary_hash_before = _sha256(binary)
        runner_source = repo / RUNNER_REL / "src" / "main.rs"
        runner_source_hash = _sha256(runner_source)
        scenarios = _selected_scenarios(args.scenario)
        for scenario in scenarios:
            current = _source_snapshot(repo)
            if current["snapshot_sha256"] != start_snapshot["snapshot_sha256"]:
                raise ComparisonRunError("repository source changed during run; artifacts retained")
            results.append(
                _run_case(
                    repo,
                    _case_output_path(out, scenario),
                    binary,
                    scenario,
                    args.seed,
                    args.aircraft_count,
                    args.duration_s,
                    recorder,
                    runner_source_hash,
                    repo / PROTOCOL_REL,
                )
            )
        binary_hash_after = _sha256(binary)
        if binary_hash_before != binary_hash_after:
            raise ComparisonRunError("runner binary changed during run")
        end_snapshot = _source_snapshot(repo)
        _write_json(out / "source-after.json", end_snapshot)
        if end_snapshot["snapshot_sha256"] != start_snapshot["snapshot_sha256"]:
            raise ComparisonRunError("repository source changed during run; artifacts retained")
        commands_path = out / "commands.json"
        summary = {
            "status": "completed",
            "protocol_version": protocol_version,
            "profile": "adsb_kinematic_v1",
            "runner_model_id_by_case": {r["scenario"]: r["runner_model_id"] for r in results},
            "build_kind": build_kind,
            "build_command": build_record,
            "cargo_target_dir": str(target),
            "source_clean_at_start": clean,
            "source_snapshot_sha256": start_snapshot["snapshot_sha256"],
            "source_before_artifact_sha256": _sha256(out / "source-before.json"),
            "source_after_artifact_sha256": _sha256(out / "source-after.json"),
            "binary_sha256": binary_hash_after,
            "configuration_sha256_by_case": {r["scenario"]: _sha256(Path(r["artifact_directory"]) / "configuration.json") for r in results},
            "scenario_results": results,
            "reference_workflow_status": "reference_failure" if any(r["status"] == "completed_reference_failure" for r in results) else "reference_validated",
            "fault_mode_cohort_status": protocol.get("fault_mode_cohort", {}).get("status"),
            "all_quality_claim_status": protocol.get("fault_mode_cohort", {}).get("all_quality_claim_status"),
            "qualification_note": "development_control only; qualification seeds are not run by this command",
            "commands": str(commands_path.resolve()),
            "commands_sha256": _sha256(commands_path),
            "speed_claim": _speed_claim("replay_call_cost"),
        }
        competitive = _competitive_pass(results)
        summary["competitive_gate"] = "pass" if competitive and clean else "fail_or_unknown"
        summary["status"] = _final_status(
            clean,
            summary["reference_workflow_status"] == "reference_failure",
            bool(_require_gate_exit(results, args.require_competitive_pass)),
        )
        _write_json(out / "summary.json", summary)
        return _exit_code(summary["status"])
    except Exception as exc:
        end_snapshot = None
        try:
            end_snapshot = _source_snapshot(repo)
            _write_json(out / "source-after.json", end_snapshot)
        except Exception:
            pass
        _write_json(
            out / "summary.json",
            {
                "status": "failed",
                "error": f"{type(exc).__name__}: {exc}",
                "source_before_sha256": start_snapshot.get("snapshot_sha256") if start_snapshot else None,
                "source_after_sha256": end_snapshot.get("snapshot_sha256") if end_snapshot else None,
                "commands": recorder.records,
                "completed_scenarios": results,
                "artifacts_retained": True,
            },
        )
        return _exit_code("failed")


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        sealed_guard.refuse(*(p and p.expanduser() for p in (args.repo, args.out, args.target_dir, args.runner)))
        return run(args)
    except sealed_guard.SealedError as exc:
        print(f"comparison run: {exc}", file=sys.stderr)
        return 2
    except ComparisonRunError as exc:
        print(f"comparison run: {exc}", file=sys.stderr)
        return 2
    except Exception as exc:
        print(f"comparison run: {type(exc).__name__}: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
