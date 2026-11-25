#!/usr/bin/env python3
"""Run a pinned M3 synthetic selector drill; recorded-traffic mode is not implemented."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import selectors
import signal
import stat
import subprocess
import tempfile
import time

MAX_BINARY_BYTES = 128 * 1024 * 1024
MAX_BINDING_BYTES = 1024 * 1024
MAX_CAPTURE_BYTES = 256 * 1024
MAX_RUNTIME_S = 60
MAX_PHASE_NS = 60_000_000_000
HEX40 = re.compile(r"^[0-9a-f]{40}$")
HEX64 = re.compile(r"^[0-9a-f]{64}$")


class ChildRunFailure(RuntimeError):
    def __init__(self, cause: BaseException, stdout: bytes, stderr: bytes, elapsed_s: float):
        super().__init__(str(cause))
        self.stdout = stdout
        self.stderr = stderr
        self.elapsed_s = elapsed_s


def sha256_file(path: Path, cap: int = MAX_BINARY_BYTES) -> str:
    digest = hashlib.sha256()
    total = 0
    with path.open("rb") as stream:
        while True:
            chunk = stream.read(1024 * 1024)
            if not chunk:
                break
            total += len(chunk)
            if total > cap:
                raise ValueError(f"file exceeds bounded size: {path}")
            digest.update(chunk)
    return digest.hexdigest()


def source_identity(root: Path, expected: str) -> dict[str, str]:
    commit = subprocess.run(["git", "-C", str(root), "rev-parse", "HEAD"],
                            check=True, capture_output=True, text=True, timeout=5).stdout.strip()
    status = subprocess.run(["git", "-C", str(root), "status", "--porcelain", "--untracked-files=all"],
                            check=True, capture_output=True, text=True, timeout=5).stdout
    if commit != expected or status:
        raise ValueError("source checkout must match the expected commit and be clean")
    return {"commit": commit, "status": "clean"}


def load_build_binding(path: Path, expected_sha256: str, source_commit: str,
                       binary: Path, binary_sha256: str) -> dict:
    if path.is_symlink() or not path.is_file():
        raise ValueError("build binding must be a regular non-symlink file")
    binding_hash = sha256_file(path, MAX_BINDING_BYTES)
    if binding_hash != expected_sha256:
        raise ValueError("build binding hash differs from its frozen expected SHA-256")
    with path.open("rb") as stream:
        binding_bytes = stream.read(MAX_BINDING_BYTES + 1)
    if len(binding_bytes) > MAX_BINDING_BYTES:
        raise ValueError("build binding exceeds bounded size")
    if hashlib.sha256(binding_bytes).hexdigest() != binding_hash:
        raise ValueError("build binding changed while it was being read")
    binding = json.loads(binding_bytes.decode("utf-8"))
    if (not isinstance(binding, dict) or binding.get("schema") != "oa-migration-build-binding/1"
            or set(binding) != {"schema", "source", "build", "artifact"}):
        raise ValueError("build binding has an unknown schema or fields")
    source = binding["source"]
    build = binding["build"]
    artifact = binding["artifact"]
    if not isinstance(source, dict) or source != {"commit": source_commit, "status": "clean"}:
        raise ValueError("build binding is not tied to the clean expected source commit")
    if not isinstance(build, dict):
        raise ValueError("build binding build record is malformed")
    argv = build.get("argv")
    package_arg_matches = isinstance(argv, list) and any(
        part in ("--package=oa-migration-switch", "-p=oa-migration-switch")
        or (part in ("-p", "--package") and argv_index + 1 < len(argv)
            and argv[argv_index + 1] == "oa-migration-switch")
        for argv_index, part in enumerate(argv)
    )
    example_arg_matches = isinstance(argv, list) and any(
        part == "--example=m3_synthetic_drill"
        or (part == "--example" and argv_index + 1 < len(argv)
            and argv[argv_index + 1] == "m3_synthetic_drill")
        for argv_index, part in enumerate(argv)
    )
    if (not isinstance(argv, list) or not all(isinstance(part, str) for part in argv)
            or "build" not in argv
            or not package_arg_matches
            or not example_arg_matches
            or build.get("exit") != 0 or build.get("timed_out") is not False
            or not isinstance(build.get("cargo_json_sha256"), str)
            or not HEX64.fullmatch(build["cargo_json_sha256"])):
        raise ValueError("build binding does not describe the successful pinned synthetic-drill build")
    if not isinstance(artifact, dict):
        raise ValueError("build binding artifact record is malformed")
    manifest_path = artifact.get("manifest_path")
    if (not isinstance(manifest_path, str)
            or not manifest_path.replace("\\", "/").endswith("crates/oa-migration-switch/Cargo.toml")
            or artifact.get("target_name") != "m3_synthetic_drill"
            or artifact.get("profile_test") is not False
            or artifact.get("sha256") != binary_sha256
            or artifact.get("retained_path") != str(binary)
            or not isinstance(artifact.get("bytes"), int)
            or artifact["bytes"] <= 0 or artifact["bytes"] > MAX_BINARY_BYTES):
        raise ValueError("build binding artifact does not identify the retained executable")
    return {"sha256": binding_hash, "record": binding}


def parse_pairs(fields: list[str]) -> dict[str, str]:
    pairs: dict[str, str] = {}
    for field in fields:
        key, separator, value = field.partition("=")
        if not separator or not key or key in pairs:
            raise ValueError("malformed or duplicate key in drill output")
        pairs[key] = value
    return pairs


def parse_output(raw: bytes) -> dict:
    text = raw.decode("ascii", errors="strict")
    lines = text.splitlines()
    if not lines or not lines[0].startswith("M3DRILL\tversion=1\t"):
        raise ValueError("missing M3DRILL v1 header")
    header = parse_pairs(lines[0].split("\t")[1:])
    cycles = []
    fault_records = []
    sustained_records = []
    summaries = []
    for line in lines[1:]:
        fields = line.split("\t")
        if fields[0] == "CYCLE_FIELDS":
            continue
        if fields[0] == "CYCLE":
            if len(fields) != 15:
                raise ValueError("malformed CYCLE record")
            cycles.append({
                "index": int(fields[1]), "ready_ns": int(fields[2]),
                "promotion_ns": int(fields[3]), "rollback_ns": int(fields[4]),
                "candidate_delivery_api_ns": int(fields[5]),
                "legacy_delivery_api_ns": int(fields[6]),
                "delivery_delay_ns": int(fields[7]), "tracks": int(fields[8]),
                "flights": int(fields[9]), "handoffs": int(fields[10]),
                "alerts": int(fields[11]), "output_sequences": fields[12],
                "losses": int(fields[13]), "error": fields[14],
            })
        elif fields[0] == "FAULT":
            fault_records.append(parse_pairs(fields[1:]))
        elif fields[0] == "SUSTAINED":
            sustained_records.append(parse_pairs(fields[1:]))
        elif fields[0] == "SUMMARY":
            summaries.append(parse_pairs(fields[1:]))
        else:
            raise ValueError("unknown M3DRILL output record")
    if len(cycles) != 100 or len(fault_records) != 1 or len(sustained_records) != 1 or len(summaries) != 1:
        raise ValueError("drill output is incomplete")
    return {"header": header, "cycles": cycles, "faults": fault_records[0],
            "sustained": sustained_records[0], "summary": summaries[0]}


def validate_output(parsed: dict) -> None:
    expected_header = {
        "version": "1",
        "mode": "synthetic_capacity", "recorded_mode": "NOT_BUILT", "qualification": "INCOMPLETE",
        "cycles": "100", "seed": "0x4d335f53594e5448", "tracks_per_side": "64",
        "flights_per_side": "64", "handoffs_per_side": "64", "alerts_per_side": "64",
        "terminal_interval_ns": "4800000000", "max_decision_delay_ns": "1000000000",
        "modeled_delivery_delay_ns": "100000000", "delivery_timing_scope": "selector_api_only",
        "consumer_wire_adapter": "NOT_BUILT",
    }
    if parsed["header"] != expected_header:
        raise ValueError("drill parameters differ from the frozen synthetic input contract")
    for expected_index, cycle in enumerate(parsed["cycles"]):
        if (cycle["index"] != expected_index or cycle["tracks"] != 64
                or cycle["flights"] != 64 or cycle["handoffs"] != 64 or cycle["alerts"] != 64
                or cycle["output_sequences"] != "1,2" or cycle["losses"] != 0
                or cycle["delivery_delay_ns"] != 100_000_000 or cycle["error"] != "NONE"
                or any(cycle[name] < 0 or cycle[name] > MAX_PHASE_NS for name in
                       ("ready_ns", "promotion_ns", "rollback_ns", "candidate_delivery_api_ns",
                        "legacy_delivery_api_ns"))):
            raise ValueError(f"clean synthetic cycle {expected_index} is incomplete or lost work")
    expected_faults = {
        "version": "1", "cases": "8", "expected_refusals": "8",
        "expected_delivery_gap_refusals": "2", "expected_delivery_duplicates": "1",
        "pending_fault_delivery_offered": "1", "pending_fault_delivery_delivered": "0",
        "pending_fault_delivery_lost": "1",
        "expected_out_of_order": "Stale",
        "epoch_change_after_reconcile": "WrongUnit",
        "duplicate_delivery": "OutputDuplicate", "sequence_gap": "OutputGap",
        "stale_legacy_health": "Stale", "promotion_pending_rollback": "accepted_then_delivery_gap",
        "expired_mirror": "Stale", "incomplete_family": "Incomplete",
        "fault_injections": "synthetic_only", "losses_preserved": "1",
    }
    if parsed["faults"] != expected_faults:
        raise ValueError("fault-injection outcomes differ from the declared synthetic suite")
    expected_sustained = {
        "version": "1", "completed_flip_rollback_cycles": "64", "consumer_deliveries": "128",
        "trace_records": "128", "trace_capacity": "128", "final_rollback_slot_preserved": "1",
        "final_record_sequence": "128", "final_record_from": "Candidate", "final_record_to": "Legacy",
        "next_promotion": "TraceFull",
        "selected_after_refusal": "Legacy", "fallback_not_blocked": "1", "recorded": "0",
    }
    if parsed["sustained"] != expected_sustained:
        raise ValueError("sustained same-selector trace-capacity case changed")
    expected_summary = {
        "cycles_expected": "100", "cycles_completed": "100", "cycles_failed": "0",
        "clean_cycle_delivery_expected": "200", "clean_cycle_delivery_observed": "200",
        "clean_cycle_lost": "0",
        "fault_cases": "8", "sustained_same_selector_cycles": "64",
        "sustained_trace_capacity": "128", "sustained_next_promotion": "TraceFull",
        "sustained_fallback": "Legacy", "clean_cycle_consumer_gaps": "0",
        "clean_cycle_consumer_duplicates": "0", "clean_cycle_consumer_out_of_order": "0",
        "active_alerts_preserved": "1", "ack_suppression_fields_compared": "1",
        "handoff_rows_compared": "64", "decision_and_delivery_timings_separate": "1",
        "recorded_replay": "NOT_BUILT", "qualification": "INCOMPLETE",
    }
    if parsed["summary"] != expected_summary:
        raise ValueError("summary denominators, losses, or incomplete status changed")


def run_child(binary: Path) -> tuple[int, bytes, bytes, float]:
    started = time.monotonic()
    child = subprocess.Popen([str(binary)], stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE, start_new_session=True, close_fds=True)
    selector = selectors.DefaultSelector()
    if child.stdout is None or child.stderr is None:
        raise RuntimeError("failed to capture child streams")
    selector.register(child.stdout, selectors.EVENT_READ, "stdout")
    selector.register(child.stderr, selectors.EVENT_READ, "stderr")
    captured = {"stdout": bytearray(), "stderr": bytearray()}
    deadline = started + MAX_RUNTIME_S
    try:
        while selector.get_map() or child.poll() is None:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError("synthetic drill exceeded wall bound")
            for key, _ in selector.select(min(remaining, 0.25)):
                chunk = os.read(key.fileobj.fileno(), 8192)
                if not chunk:
                    selector.unregister(key.fileobj)
                    continue
                target = captured[key.data]
                if len(target) + len(chunk) > MAX_CAPTURE_BYTES:
                    target.extend(chunk[:max(0, MAX_CAPTURE_BYTES - len(target))])
                    raise ValueError(f"{key.data} exceeded capture bound")
                target.extend(chunk)
        return_code = child.wait(timeout=max(0.1, deadline - time.monotonic()))
        return return_code, bytes(captured["stdout"]), bytes(captured["stderr"]), time.monotonic() - started
    except BaseException as exc:
        if child.poll() is None:
            try:
                os.killpg(child.pid, signal.SIGTERM)
                child.wait(timeout=1)
            except (ProcessLookupError, subprocess.TimeoutExpired):
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                child.wait()
        raise ChildRunFailure(exc, bytes(captured["stdout"]), bytes(captured["stderr"]),
                              time.monotonic() - started) from exc
    finally:
        selector.close()
        child.stdout.close()
        child.stderr.close()


def write_temp_bytes(fd: int, encoded: bytes) -> None:
    with os.fdopen(fd, "wb") as stream:
        stream.write(encoded)
        stream.flush()
        os.fsync(stream.fileno())


def write_report(path: Path, payload: dict) -> None:
    path = path.absolute()
    parent = path.parent
    if parent.is_symlink():
        raise ValueError("output parent may not be a symlink")
    if not parent.exists():
        if parent.parent.is_symlink() or not parent.parent.is_dir():
            raise ValueError("new output parent requires an existing non-symlink parent directory")
        if parent.parent.resolve(strict=True) != parent.parent:
            raise ValueError("output path may not traverse a symlinked ancestor")
        parent.mkdir(mode=0o700)
        if stat.S_IMODE(parent.stat().st_mode) & 0o077:
            raise ValueError("new output parent is not private")
    if not parent.is_dir():
        raise ValueError("output parent must be a directory")
    if parent.resolve(strict=True) != parent:
        raise ValueError("output path may not traverse a symlinked ancestor")
    if path.exists() or path.is_symlink():
        raise ValueError("output path must be fresh")
    encoded = (json.dumps(payload, sort_keys=True, separators=(",", ":")) + "\n").encode("utf-8")
    fd, temp_name = tempfile.mkstemp(prefix=f".{path.name}.partial-", dir=parent)
    temp = Path(temp_name)
    try:
        os.fchmod(fd, stat.S_IRUSR | stat.S_IWUSR)
        # Failed serialization leaves a named partial artifact, never a truncated final report.
        write_temp_bytes(fd, encoded)
    except BaseException:
        try:
            os.close(fd)
        except OSError:
            pass
        raise
    # Atomic hard-link publication refuses to replace a concurrent writer's file.
    os.link(temp, path, follow_symlinks=False)
    temp.unlink()
    directory_fd = os.open(parent, os.O_RDONLY)
    try:
        os.fsync(directory_fd)
    finally:
        os.close(directory_fd)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--source-root", required=True, type=Path)
    parser.add_argument("--expected-source-commit", required=True)
    parser.add_argument("--expected-binary-sha256", required=True)
    parser.add_argument("--build-binding", required=True, type=Path)
    parser.add_argument("--expected-build-binding-sha256", required=True)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    if (not HEX40.fullmatch(args.expected_source_commit)
            or not HEX64.fullmatch(args.expected_binary_sha256)
            or not HEX64.fullmatch(args.expected_build_binding_sha256)):
        parser.error("source commit and SHA-256 values must be lowercase hex digests")
    if args.binary.is_symlink():
        parser.error("binary path may not be a symlink")
    binary = args.binary.resolve(strict=True)
    mode = binary.stat().st_mode
    if not stat.S_ISREG(mode) or not os.access(binary, os.X_OK):
        parser.error("binary must be a regular executable file")
    source = source_identity(args.source_root.resolve(strict=True), args.expected_source_commit)
    binary_hash_before = sha256_file(binary)
    if binary_hash_before != args.expected_binary_sha256:
        parser.error("binary hash does not match the pinned expected SHA-256")
    try:
        if args.build_binding.is_symlink():
            raise ValueError("build binding path may not be a symlink")
        build_binding = load_build_binding(args.build_binding.resolve(strict=True),
                                           args.expected_build_binding_sha256,
                                           args.expected_source_commit, binary, binary_hash_before)
    except (OSError, ValueError, json.JSONDecodeError) as exc:
        parser.error(f"invalid build binding: {exc}")
    input_spec = {
        "schema": "m3-synthetic-capacity-input-v1", "seed": "0x4d335f53594e5448",
        "cycles": 100, "tracks_per_side": 64, "flights_per_side": 64,
        "handoffs_per_side": 64, "alerts_per_side": 64,
        "variation": "cycle_index_mod_8; same consumer projection on both sides",
        "track_fields": "lat=i+v;lon=-i;alt_ft=20000+i;gs_dkt=3000+(v%20);track_ddeg=(i+v)%3600",
        "handoff_lifecycle": "1+(v%4)",
        "alert_fields": "active=1;acknowledged=v%2;suppressed=(v>>1)&1",
        "recorded_traffic": "NOT_BUILT",
    }
    input_digest = hashlib.sha256(json.dumps(input_spec, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
    result = {"schema": "openatca-m3-drill-report-v1", "execution": "started",
              "source": source, "binary": {"path": str(binary), "sha256_before": binary_hash_before},
              "build_binding": build_binding,
              "input": input_spec, "input_sha256": input_digest,
              "qualification": "INCOMPLETE", "recorded_mode": "NOT_BUILT"}
    try:
        code, stdout, stderr, elapsed_s = run_child(binary)
        result["execution"] = "finished"
        result["return_code"] = code
        result["elapsed_wall_s"] = elapsed_s
        result["stdout_sha256"] = hashlib.sha256(stdout).hexdigest()
        result["stderr_sha256"] = hashlib.sha256(stderr).hexdigest()
        result["stdout"] = stdout.decode("ascii", errors="replace")
        result["stderr"] = stderr.decode("utf-8", errors="replace")
        result["binary"]["sha256_after"] = sha256_file(binary)
        if result["binary"]["sha256_after"] != binary_hash_before:
            raise ValueError("binary changed during the drill")
        result["parsed"] = parse_output(stdout)
        validate_output(result["parsed"])
        clean_cycles = result["parsed"]["cycles"]
        decision_limit = int(result["parsed"]["header"]["max_decision_delay_ns"])
        phase_samples = {
            "readiness_ns": [row["ready_ns"] for row in clean_cycles],
            "selection_total_ns": [row["ready_ns"] + row["promotion_ns"] for row in clean_cycles],
            "promotion_ns": [row["promotion_ns"] for row in clean_cycles],
            "rollback_ns": [row["rollback_ns"] for row in clean_cycles],
            "candidate_delivery_api_ns": [row["candidate_delivery_api_ns"] for row in clean_cycles],
            "legacy_delivery_api_ns": [row["legacy_delivery_api_ns"] for row in clean_cycles],
        }
        result["timing"] = {
            "decision_limit_ns": decision_limit,
            "modeled_delivery_delay_ns": 100_000_000,
            "delivery_measurement_scope": "selector_api_only; no consumer wire adapter",
            "samples_per_phase": {name: len(values) for name, values in phase_samples.items()},
            "phase_max_ns": {name: max(values) for name, values in phase_samples.items()},
            "phase_over_decision_limit": {
                name: sum(value > decision_limit for value in values)
                for name, values in phase_samples.items()
                if name not in ("candidate_delivery_api_ns", "legacy_delivery_api_ns")
            },
            "modeled_delivery_within_terminal_update_interval":
                100_000_000 <= int(result["parsed"]["header"]["terminal_interval_ns"]),
            "qualification_claim": "NONE; selector API synthetic timing only",
        }
        summary = result["parsed"]["summary"]
        if code != 0 or summary.get("cycles_completed") != "100" or summary.get("cycles_failed") != "0":
            result["execution"] = "failed_or_incomplete"
    except BaseException as exc:
        result["execution"] = "failed"
        result["error"] = f"{type(exc).__name__}: {exc}"
        if isinstance(exc, ChildRunFailure):
            result["elapsed_wall_s"] = exc.elapsed_s
            result["stdout"] = exc.stdout.decode("ascii", errors="replace")
            result["stderr"] = exc.stderr.decode("utf-8", errors="replace")
            result["stdout_sha256"] = hashlib.sha256(exc.stdout).hexdigest()
            result["stderr_sha256"] = hashlib.sha256(exc.stderr).hexdigest()
    write_report(args.output, result)
    print(json.dumps({"schema": result["schema"], "execution": result["execution"],
                      "qualification": result["qualification"], "output": str(args.output),
                      "binary_sha256": binary_hash_before}, sort_keys=True))
    return 0 if result["execution"] == "finished" else 1


if __name__ == "__main__":
    raise SystemExit(main())
