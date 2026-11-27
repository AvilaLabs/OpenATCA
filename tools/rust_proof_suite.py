"""Run the repository's registered Kani proof suite sequentially."""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import math
import os
from pathlib import Path
import re
import signal
import shutil
import subprocess
import sys
import time
import tomllib
from typing import Any

from kani_validation import LOOP_UNWIND_PRESETS, _source_snapshot, source_mapping_sha256


REGISTRY_VERSION = 1
MAX_PROOFS = 128
ALLOWED_PACKAGES = {
    "oa-track", "oa-track-core", "oa-replay", "oa-safety", "oa-ca", "oa-math", "oa-runway", "oa-asterix", "oa-adsb",
    "oa-feed", "oa-truth", "oa-msaw",
}
ALLOWED_CFGS = {"kani_full_capacity", "kani_late_replay_covers"}
ID_RE = re.compile(r"^[a-z0-9][a-z0-9-]*$")
HARNESS_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)*$")


# Paths whose change selects every proof: the registry, the runner, the workflow, the
# workspace build inputs and what feeds the proof controls.
RUN_ALL_PATHS = frozenset({
    "tools/rust_proofs.json", "tools/rust_proof_suite.py", "tools/kani_validation.py",
    "tools/prepare_rust_proof_controls.py", ".github/workflows/rust-proofs.yml",
    "Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "mutants.toml",
})
RUN_ALL_PREFIXES = ("fuzz/", "validation/", "adapters/", ".cargo/", "tools/testdata/")
TIERS = ("push", "nightly")
# A proof recorded over this many seconds is nightly, one at or under it is push.
NIGHTLY_OVER_SECONDS = 120
CI_SECONDS_SOURCES = ("ci", "local", "none")
GNU_TIME_COMMAND = "time"
MAX_RESOURCE_OUTPUT_BYTES = 4096
RESOURCE_FORMAT = "OPENATCA_PROOF_RESOURCE_V1 %M %e"
_PACKAGE_ROOTS = ("crates", "tools")
_DEPENDENCY_TABLES = ("dependencies", "build-dependencies", "dev-dependencies")


class SelectionError(ValueError):
    """The change cannot be mapped to packages; the run must not guess."""


def repo_argument_parser(description: str) -> argparse.ArgumentParser:
    """Create a command parser with the common required repository-root option."""
    parser = argparse.ArgumentParser(description=description)
    parser.add_argument("--repo", type=Path, required=True)
    return parser


def _path_dependencies(data: dict[str, Any]) -> set[str]:
    """Path dependencies of one manifest in every table, target-specific ones and dev ones
    included: counting more dependencies only ever runs more proofs."""
    tables = [data]
    targets = data.get("target", {})
    if isinstance(targets, dict):
        tables.extend(t for t in targets.values() if isinstance(t, dict))
    deps: set[str] = set()
    for holder in tables:
        for table in _DEPENDENCY_TABLES:
            entries = holder.get(table, {})
            if not isinstance(entries, dict):
                continue
            for dep, spec in entries.items():
                if isinstance(spec, dict) and "path" in spec:
                    deps.add(spec.get("package", dep))
    return deps


def package_graph(repo: Path) -> dict[str, dict[str, Any]]:
    """Every workspace package under crates/ and tools/: its directory and path dependencies.
    A manifest that cannot be read or parsed raises: a silent skip would shrink the selection."""
    graph: dict[str, dict[str, Any]] = {}
    for root in _PACKAGE_ROOTS:
        for manifest in sorted((repo / root).glob("*/Cargo.toml")):
            try:
                data = tomllib.loads(manifest.read_text(encoding="utf-8"))
            except (OSError, UnicodeDecodeError, tomllib.TOMLDecodeError) as exc:
                raise SelectionError(f"cannot read {manifest}: {exc}") from exc
            name = data.get("package", {}).get("name")
            if not isinstance(name, str):
                continue
            graph[name] = {"dir": f"{root}/{manifest.parent.name}/", "deps": _path_dependencies(data)}
    return graph


def registry_packages_missing(proofs: list[dict[str, Any]], graph: dict[str, dict[str, Any]]) -> list[str]:
    """Registered packages that are not in the workspace graph (their proofs could never be
    selected by a change)."""
    return sorted({p["package"] for p in proofs} - set(graph))


def unresolved_path_dependencies(graph: dict[str, dict[str, Any]]) -> list[str]:
    """Names referenced by path edges but absent from the current package graph."""
    return sorted({dep for info in graph.values() for dep in info["deps"] if dep not in graph})


def package_directories_at(repo: Path, revision: str) -> set[str]:
    """Cargo package directories recorded by a revision, including packages deleted at HEAD."""
    try:
        result = subprocess.run(
            ["git", "-C", str(repo), "ls-tree", "-r", "-z", "--name-only", revision,
             "--", *_PACKAGE_ROOTS],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False, timeout=30,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise SelectionError("cannot inspect Cargo package manifests at the comparison base") from exc
    if result.returncode != 0 or len(result.stdout) > 32 * 1024 * 1024:
        raise SelectionError("cannot inspect Cargo package manifests at the comparison base")
    try:
        return {
            (path.rpartition(b"/")[0] + b"/").decode("utf-8", "strict")
            for path in result.stdout.split(b"\0")
            if path.endswith(b"/Cargo.toml")
        }
    except UnicodeDecodeError as exc:
        raise SelectionError("Cargo package path at comparison base is not UTF-8") from exc


def _package_root_for_path(path: str) -> str | None:
    parts = path.split("/")
    if len(parts) >= 3 and parts[0] in _PACKAGE_ROOTS:
        return f"{parts[0]}/{parts[1]}/"
    return None


def unmapped_package_paths(
    graph: dict[str, dict[str, Any]], changed: list[str], base_directories: set[str],
) -> list[str]:
    """Deleted package directories from the base must not silently disappear from selection."""
    directories = {info["dir"] for info in graph.values()}
    unmapped = set()
    for path in changed:
        directory = _package_root_for_path(path)
        if directory is not None and directory not in directories and directory in base_directories:
            unmapped.add(directory)
    return sorted(unmapped)


def has_unowned_package_root_changes(
    graph: dict[str, dict[str, Any]], changed: list[str], base_directories: set[str],
) -> bool:
    """Unknown loose source/support under crates/ or tools/ selects every proof conservatively."""
    directories = {info["dir"] for info in graph.values()}
    for path in changed:
        directory = _package_root_for_path(path)
        if directory is not None and directory not in directories and directory not in base_directories:
            return True
    return False


def affected_packages(graph: dict[str, dict[str, Any]], changed: list[str]) -> set[str] | None:
    """Packages whose proofs a change can affect: those with a changed file and every package
    that depends on one of them, directly or not. `None` means everything."""
    if any(path in RUN_ALL_PATHS or path.startswith(RUN_ALL_PREFIXES) for path in changed):
        return None
    touched = {
        name for name, info in graph.items()
        if any(path.startswith(info["dir"]) for path in changed)
    }
    affected = set(touched)
    grew = True
    while grew:
        grew = False
        for name, info in graph.items():
            if name not in affected and info["deps"] & affected:
                affected.add(name)
                grew = True
    return affected


def select_proofs(
    proofs: list[dict[str, Any]], affected: set[str] | None,
) -> list[dict[str, Any]]:
    """All enabled proofs reached by the change, regardless of recorded runtime tier."""
    chosen = [p for p in proofs if p["enabled"]]
    if affected is not None:
        chosen = [p for p in chosen if p["package"] in affected]
    return chosen


def proof_selection_report(
    proofs: list[dict[str, Any]], chosen: list[dict[str, Any]], selected: list[dict[str, Any]],
    affected: set[str] | None, changed_from: str | None, changed: list[str] | None,
) -> dict[str, Any]:
    chosen_ids = {proof["id"] for proof in chosen}
    selected_ids = {proof["id"] for proof in selected}
    excluded = []
    skipped = []
    for proof in proofs:
        if proof["id"] in selected_ids:
            continue
        if proof["id"] in chosen_ids:
            reason = "assigned to another shard"
            skipped.append({"id": proof["id"], "status": "SKIPPED", "reason": reason})
        elif not proof["enabled"]:
            reason = "disabled in registry"
            excluded.append({"id": proof["id"], "status": "EXCLUDED", "reason": reason})
        elif changed_from is not None and affected is not None and proof["package"] not in affected:
            reason = "package not reached by changed files"
            excluded.append({"id": proof["id"], "status": "EXCLUDED", "reason": reason})
        else:
            reason = "not selected"
            excluded.append({"id": proof["id"], "status": "EXCLUDED", "reason": reason})
    return {
        "mode": "changed_packages" if changed_from is not None else "all_enabled",
        "base": changed_from,
        "changed_files": changed if changed is not None else ("unknown, all packages assumed" if changed_from else None),
        "affected_packages": sorted(affected) if affected is not None else ("all" if changed_from else None),
        "selected_proof_ids": [proof["id"] for proof in chosen],
        "shard_proof_ids": [proof["id"] for proof in selected],
        "skipped_proofs": skipped,
        "excluded_proofs": excluded,
    }


def changed_files(repo: Path, base: str) -> list[str] | None:
    """Files changed since `base`, or None when the comparison cannot be made. Renames are
    reported as a delete and an add, so both the old and the new path count as changed. NUL
    framing preserves Git paths containing whitespace, quotes, newlines or non-ASCII bytes."""
    if base.startswith("-") or not re.fullmatch(r"[0-9a-fA-F]{7,64}|[A-Za-z0-9_./-]+", base):
        return None
    if set(base) == {"0"}:
        return None
    try:
        out = subprocess.run(
            ["git", "diff", "--name-only", "-z", "--no-renames", base, "HEAD", "--"], cwd=repo,
            check=True, capture_output=True, timeout=60,
        ).stdout
    except (OSError, subprocess.SubprocessError):
        return None
    if not out:
        return []
    if not out.endswith(b"\0"):
        return None
    try:
        return [path.decode("utf-8", "strict") for path in out[:-1].split(b"\0")]
    except UnicodeDecodeError:
        # An unrepresentable path makes the affected set unknowable. The caller treats None as
        # all packages, so this cannot turn a changed package into an empty selection.
        return None


def balance_shards(proofs: list[dict[str, Any]], count: int) -> list[list[dict[str, Any]]]:
    """Balance measured runtimes, using external timeout only for unmeasured scheduling weight."""
    shards: list[list[dict[str, Any]]] = [[] for _ in range(count)]
    load = [0] * count
    order = {id(p): i for i, p in enumerate(proofs)}
    def weight(proof: dict[str, Any]) -> int:
        if proof.get("ci_seconds_source") == "none":
            return proof["external_timeout_seconds"]
        return proof["ci_seconds"]

    for proof in sorted(proofs, key=lambda p: (-weight(p), order[id(p)])):
        target = min(range(count), key=lambda i: (load[i], i))
        shards[target].append(proof)
        load[target] += weight(proof)
    for shard in shards:
        shard.sort(key=lambda p: order[id(p)])
    return shards


def _canonical_sha256(value: Any) -> str:
    data = json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
    return hashlib.sha256(data).hexdigest()


def _sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def proof_target_context_sha256(proof: dict[str, Any], source_sha256: str) -> str:
    """Bind one build-cache namespace to its proof configuration and source snapshot."""
    return _canonical_sha256({"proof": proof, "source_sha256": source_sha256})


def proof_target_path(target_root: Path, proof: dict[str, Any], source_sha256: str) -> Path:
    """Give each proof/config/source context a disjoint Cargo target-cache subtree."""
    context = proof_target_context_sha256(proof, source_sha256)
    return target_root / "proofs" / proof["id"] / context


def validate_registry(data: Any) -> list[str]:
    """Validate the complete proof matrix before any subprocess is started."""
    errors: list[str] = []
    if not isinstance(data, dict):
        return ["registry must be a JSON object"]
    if data.get("registry_version") != REGISTRY_VERSION:
        errors.append(f"registry_version must be {REGISTRY_VERSION}")
    entries = data.get("proofs")
    if not isinstance(entries, list) or not entries:
        return errors + ["proofs must be a nonempty list"]
    expected_count = data.get("expected_proof_count")
    if not isinstance(expected_count, int) or isinstance(expected_count, bool) or expected_count != len(entries):
        errors.append("expected_proof_count must match the complete registry entry count")
    if len(entries) > MAX_PROOFS:
        errors.append(f"registry exceeds the {MAX_PROOFS}-proof limit")

    ids: set[str] = set()
    harnesses: set[tuple[str, str]] = set()
    for index, entry in enumerate(entries):
        label = f"proofs[{index}]"
        if not isinstance(entry, dict):
            errors.append(f"{label} must be an object")
            continue
        required_keys = {
            "id", "requirement", "package", "harness", "enabled", "experimental",
            "required_covers", "stubbing", "stub_summaries", "cfgs",
            "harness_timeout_seconds", "external_timeout_seconds", "memory_mib", "tier",
            "ci_seconds", "ci_seconds_source",
        }
        allowed_keys = required_keys | {"loop_unwind_overrides"}
        if not required_keys.issubset(entry) or not set(entry).issubset(allowed_keys):
            errors.append(f"{label} keys must exactly match the registered proof schema")
        proof_id = entry.get("id")
        if not isinstance(proof_id, str) or not ID_RE.fullmatch(proof_id):
            errors.append(f"{label}.id is invalid")
        elif proof_id in ids:
            errors.append(f"duplicate proof id: {proof_id}")
        else:
            ids.add(proof_id)
        if not isinstance(entry.get("requirement"), str) or not entry["requirement"].strip():
            errors.append(f"{label}.requirement must explain the proof scope")
        package = entry.get("package")
        harness = entry.get("harness")
        if not isinstance(package, str) or package not in ALLOWED_PACKAGES:
            errors.append(f"{label}.package is not an allowed workspace package")
        if not isinstance(harness, str) or not HARNESS_RE.fullmatch(harness):
            errors.append(f"{label}.harness is invalid")
        elif isinstance(package, str) and (package, harness) in harnesses:
            errors.append(f"duplicate package/harness pair: {package} {harness}")
        elif isinstance(package, str) and isinstance(harness, str):
            harnesses.add((package, harness))
        if entry.get("tier") not in TIERS:
            errors.append(f"{label}.tier must be one of {', '.join(TIERS)}")
        ci_seconds = entry.get("ci_seconds")
        if not isinstance(ci_seconds, int) or isinstance(ci_seconds, bool) or ci_seconds < 0:
            errors.append(f"{label}.ci_seconds must be a nonnegative integer")
        elif entry.get("tier") in TIERS and (ci_seconds > NIGHTLY_OVER_SECONDS) != (
            entry.get("tier") == "nightly"
        ):
            errors.append(
                f"{label}.tier must be nightly exactly when ci_seconds is over {NIGHTLY_OVER_SECONDS}"
            )
        if entry.get("ci_seconds_source") not in CI_SECONDS_SOURCES:
            errors.append(f"{label}.ci_seconds_source must be one of {', '.join(CI_SECONDS_SOURCES)}")
        if not isinstance(entry.get("enabled"), bool) or not isinstance(entry.get("experimental"), bool):
            errors.append(f"{label}.enabled and experimental must be booleans")
        covers = entry.get("required_covers")
        if not isinstance(covers, list) or any(not isinstance(x, str) or not x.strip() for x in covers):
            errors.append(f"{label}.required_covers must be a list of nonempty labels")
        elif len(covers) != len(set(covers)):
            errors.append(f"{label}.required_covers contains duplicate labels")
        stubbing = entry.get("stubbing")
        summaries = entry.get("stub_summaries")
        if not isinstance(stubbing, bool):
            errors.append(f"{label}.stubbing must be a boolean")
        if not isinstance(summaries, list) or any(not isinstance(x, str) or not x.strip() for x in summaries):
            errors.append(f"{label}.stub_summaries must be a list of descriptions")
        elif isinstance(stubbing, bool) and stubbing != bool(summaries):
            errors.append(f"{label}: stubbing mode and stub summaries disagree")
        cfgs = entry.get("cfgs")
        if not isinstance(cfgs, list) or any(not isinstance(x, str) or x not in ALLOWED_CFGS for x in cfgs):
            errors.append(f"{label}.cfgs contains an unregistered cfg")
        elif len(cfgs) != len(set(cfgs)):
            errors.append(f"{label}.cfgs contains duplicates")
        for field, minimum, maximum in (
            ("harness_timeout_seconds", 30, 7200),
            ("external_timeout_seconds", 60, 7500),
            ("memory_mib", 512, 32768),
        ):
            value = entry.get(field)
            if not isinstance(value, int) or isinstance(value, bool) or not minimum <= value <= maximum:
                errors.append(f"{label}.{field} must be an integer in [{minimum}, {maximum}]")
        harness_timeout = entry.get("harness_timeout_seconds")
        external_timeout = entry.get("external_timeout_seconds")
        if (isinstance(harness_timeout, int) and not isinstance(harness_timeout, bool)
                and isinstance(external_timeout, int) and not isinstance(external_timeout, bool)
                and external_timeout <= harness_timeout):
            errors.append(f"{label}.external_timeout_seconds must exceed harness timeout")
        if entry.get("experimental") is True and entry.get("enabled") is True:
            errors.append(f"{label}: experimental proofs must remain disabled until promoted")
        overrides = entry.get("loop_unwind_overrides", [])
        if not isinstance(overrides, list) or any(not isinstance(x, str) for x in overrides):
            errors.append(f"{label}.loop_unwind_overrides must be a list of preset names")
        else:
            if len(overrides) > 1:
                errors.append(f"{label}.loop_unwind_overrides supports at most one preset")
            if len(overrides) != len(set(overrides)):
                errors.append(f"{label}.loop_unwind_overrides contains duplicates")
            for preset_name in overrides:
                preset = LOOP_UNWIND_PRESETS.get(preset_name)
                if preset is None:
                    errors.append(f"{label}.loop_unwind_overrides contains an unknown preset")
                elif package != preset["package"] or harness not in preset["harnesses"]:
                    errors.append(f"{label}: loop-unwind preset is not applicable to package/harness")
    # Kani's --harness matches a substring of the full name, so a harness whose name is
    # contained in another's selects both and the run fails its one-harness check.
    names = sorted({(package, harness) for package, harness in harnesses})
    for package, harness in names:
        for other_package, other in names:
            if package == other_package and harness != other and harness in other:
                errors.append(f"harness name {harness} is contained in {other}; Kani would select both")
    return sorted(set(errors))


def _parse_registry_bytes(raw: bytes) -> tuple[dict[str, Any] | None, list[str]]:
    try:
        data = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        return None, [f"cannot read registry JSON: {exc}"]
    errors = validate_registry(data)
    return (data if not errors else None), errors


def load_registry(path: Path) -> tuple[dict[str, Any] | None, list[str]]:
    try:
        return _parse_registry_bytes(path.read_bytes())
    except OSError as exc:
        return None, [f"cannot read registry JSON: {exc}"]


def _terminate_group(process: subprocess.Popen[Any], grace: float = 3.0) -> None:
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        process.communicate(timeout=grace)
    except subprocess.TimeoutExpired:
        pass
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    process.wait()


def _failure_diagnostics(
    run_record: dict[str, Any] | None, rc: int, timed_out: bool, run_record_path: Path,
) -> list[str]:
    diagnostics: list[str] = []
    if timed_out:
        diagnostics.append("proof runner timed out")
    if rc != 0:
        diagnostics.append(f"proof runner exited with status {rc}")
    if not isinstance(run_record, dict):
        diagnostics.append("runner produced no readable run.json record")
    else:
        checks = run_record.get("checks")
        if isinstance(checks, list):
            for check in checks:
                if not isinstance(check, dict):
                    continue
                errors = check.get("evaluation_errors")
                if not isinstance(errors, list):
                    continue
                for error in errors:
                    if isinstance(error, str) and error:
                        compact = " ".join(error.split())
                        diagnostics.append(compact[:240])
        if run_record.get("status") != "PASS" and not diagnostics:
            diagnostics.append(f"child run status is {str(run_record.get('status'))[:80]}")
    if not diagnostics and not run_record_path.is_file():
        diagnostics.append("runner output record is missing")
    if not diagnostics:
        diagnostics.append("run did not match the registered proof binding")
    return list(dict.fromkeys(diagnostics))[:4]


def _resource_tool_identity() -> dict[str, Any]:
    """Identify the bounded Linux measurement tool used around one proof runner."""
    try:
        located = shutil.which(GNU_TIME_COMMAND)
        if located is None:
            raise OSError("GNU time executable is not available on PATH")
        tool = Path(located).resolve(strict=True)
        info = tool.stat()
        if not tool.is_file() or info.st_size <= 0 or info.st_size > 4 * 1024 * 1024:
            raise OSError("GNU time executable is not a bounded regular file")
        version = subprocess.run(
            [str(tool), "--version"], capture_output=True, text=True, timeout=5, check=False,
        )
        first_line = version.stdout.splitlines()[0] if version.stdout.splitlines() else ""
        if version.returncode != 0 or "GNU Time" not in first_line:
            raise OSError("measurement executable is not GNU time")
        return {
            "status": "available",
            "configured_command": GNU_TIME_COMMAND,
            "resolved_path": str(tool),
            "version": first_line[:160],
            "binary_sha256": hashlib.sha256(tool.read_bytes()).hexdigest(),
        }
    except (OSError, subprocess.SubprocessError, IndexError) as error:
        return {
            "status": "unavailable",
            "configured_command": GNU_TIME_COMMAND,
            "reason": str(error)[:240],
        }


def _read_resource_output(path: Path) -> tuple[dict[str, Any] | None, str | None, str | None]:
    """Read one GNU-time record: largest waited-process RSS and wall elapsed time."""
    try:
        if path.is_symlink() or not path.is_file() or path.stat().st_size > MAX_RESOURCE_OUTPUT_BYTES:
            return None, None, "resource output is missing, unsafe, or oversized"
        raw = path.read_bytes()
    except OSError as error:
        return None, None, f"resource output could not be read: {error}"
    try:
        text = raw.decode("ascii")
    except UnicodeDecodeError:
        return None, hashlib.sha256(raw).hexdigest(), "resource output is not ASCII"
    match = re.fullmatch(r"OPENATCA_PROOF_RESOURCE_V1 ([0-9]+) ([0-9]+(?:\.[0-9]+)?)\n?", text)
    if match is None:
        return None, hashlib.sha256(raw).hexdigest(), "resource output does not match the exact GNU-time record"
    maximum_rss_kib = int(match.group(1))
    elapsed_seconds = float(match.group(2))
    if maximum_rss_kib <= 0 or not math.isfinite(elapsed_seconds) or elapsed_seconds < 0:
        return None, hashlib.sha256(raw).hexdigest(), "resource values are outside their valid ranges"
    return {
        "maximum_rss_kib": maximum_rss_kib,
        "elapsed_seconds": elapsed_seconds,
        "measurement_scope": (
            "GNU time %M maximum RSS reported for the timed validation runner and its waited "
            "descendants; Linux reports the largest individual waited child, not a simultaneous "
            "process-tree sum. This is not virtual memory or the configured RLIMIT_AS bound."
        ),
    }, hashlib.sha256(raw).hexdigest(), None


def _write_resource_record(
    proof_dir: Path, tool: dict[str, Any], raw_path: Path,
) -> tuple[dict[str, Any], str]:
    measurement, raw_sha256, error = _read_resource_output(raw_path)
    status = "available" if tool.get("status") == "available" and measurement is not None else "unavailable"
    record = {
        "resource_record_version": 1,
        "status": status,
        "tool": tool,
        "measurement": measurement,
        "raw_output_sha256": raw_sha256,
        "error": error if error is not None else (None if status == "available" else "measurement tool unavailable"),
    }
    path = proof_dir / "resource.json"
    encoded = json.dumps(record, sort_keys=True, separators=(",", ":")).encode("utf-8") + b"\n"
    path.write_bytes(encoded)
    return record, hashlib.sha256(encoded).hexdigest()


def _run_one_proof(
    proof: dict[str, Any], repo: Path, out: Path, target_root: Path, runner: Path,
    source_sha256: str,
) -> dict[str, Any]:
    proof_dir = out / "proofs" / proof["id"]
    proof_dir.mkdir(parents=True, exist_ok=False)
    run_dir = proof_dir / "run"
    target_dir = proof_target_path(target_root, proof, source_sha256)
    cmd = [
        sys.executable, str(runner), "--repo", str(repo), "--package", proof["package"],
        "--harness", proof["harness"], "--out", str(run_dir),
        "--target-dir", str(target_dir), "--harness-timeout",
        str(proof["harness_timeout_seconds"]), "--timeout",
        str(proof["external_timeout_seconds"]), "--memory-mib", str(proof["memory_mib"]),
    ]
    for label in proof["required_covers"]:
        cmd.extend(["--require-cover", f"{proof['harness']}={label}"])
    for preset_name in proof.get("loop_unwind_overrides", []):
        cmd.extend(["--loop-unwind-preset", preset_name])
    if proof["stubbing"]:
        cmd.append("--stubbing")
        for summary in proof["stub_summaries"]:
            cmd.extend(["--stub-summary", summary])
    for cfg in proof["cfgs"]:
        cmd.extend(["--cfg", cfg])
    started = time.monotonic()
    stdout_path, stderr_path = proof_dir / "stdout.log", proof_dir / "stderr.log"
    raw_resource_path = proof_dir / "gnu-time.txt"
    resource_tool = _resource_tool_identity()
    measured_cmd = cmd
    if resource_tool.get("status") == "available":
        measured_cmd = [
            resource_tool["resolved_path"], "-q", "-f", RESOURCE_FORMAT,
            "-o", str(raw_resource_path), "--", *cmd,
        ]
    timed_out = False
    try:
        with stdout_path.open("w", encoding="utf-8") as stdout, stderr_path.open(
            "w", encoding="utf-8"
        ) as stderr:
            process = subprocess.Popen(
                measured_cmd, cwd=repo, env=os.environ.copy(), stdout=stdout, stderr=stderr,
                start_new_session=True,
            )
            try:
                rc = process.wait(timeout=proof["external_timeout_seconds"] + 30)
            except subprocess.TimeoutExpired:
                timed_out = True
                _terminate_group(process)
                rc = 124
    except OSError as exc:
        rc = 127
        stderr_path.write_text(f"failed to start runner: {exc}\n", encoding="utf-8")
    run_record_path = run_dir / "run.json"
    run_record: dict[str, Any] | None = None
    if run_record_path.is_file():
        try:
            run_record = json.loads(run_record_path.read_text(encoding="utf-8"))
        except (OSError, UnicodeDecodeError, json.JSONDecodeError):
            pass
    resource_record, resource_sha256 = _write_resource_record(proof_dir, resource_tool, raw_resource_path)
    ok = (
        rc == 0 and not timed_out and isinstance(run_record, dict)
        and run_record.get("status") == "PASS"
        and run_record.get("expected_harnesses") == [proof["harness"]]
        and run_record.get("package") == proof["package"]
        and run_record.get("required_covers") == {proof["harness"]: proof["required_covers"]}
        and run_record.get("loop_unwind_presets", {}).get(proof["harness"], [])
        == proof.get("loop_unwind_overrides", [])
        and resource_record.get("status") == "available"
    )
    diagnostics = [] if ok else _failure_diagnostics(run_record, rc, timed_out, run_record_path)
    if resource_record.get("status") != "available":
        diagnostics.append("per-proof peak/time measurement unavailable; proof cannot receive PASS")
    return {
        "id": proof["id"],
        "target_context_sha256": proof_target_context_sha256(proof, source_sha256),
        "package": proof["package"],
        "harness": proof["harness"],
        "loop_unwind_overrides": proof.get("loop_unwind_overrides", []),
        "experimental": proof["experimental"],
        "status": "PASS" if ok else "FAIL",
        "diagnostics": diagnostics,
        "exit_code": rc,
        "timed_out": timed_out,
        "elapsed_seconds": time.monotonic() - started,
        "command": cmd,
        "command_sha256": _canonical_sha256(cmd),
        "measurement_command": measured_cmd,
        "resource_file": f"proofs/{proof['id']}/resource.json",
        "resource_file_sha256": resource_sha256,
        "resource_measurement": resource_record,
        "run_record_sha256": hashlib.sha256(run_record_path.read_bytes()).hexdigest()
        if run_record_path.is_file() else None,
        "run_dir": str(run_dir),
    }


def run_suite(
    repo: Path, out: Path, target_dir: Path, registry_path: Path,
    shard: tuple[int, int] = (0, 1), changed_from: str | None = None,
) -> int:
    try:
        registry_path = registry_path.resolve(strict=True)
        registry_bytes = registry_path.read_bytes()
        registry, errors = _parse_registry_bytes(registry_bytes)
    except OSError as exc:
        registry, errors, registry_bytes = None, [f"cannot read registry JSON: {exc}"], b""
    if errors or registry is None:
        print("invalid Kani proof registry:", *errors, sep="\n  ", file=sys.stderr)
        return 2
    repo = repo.resolve(strict=True)
    out = out.resolve()
    target_dir = target_dir.resolve()
    if not (repo / "Cargo.toml").is_file() or not (repo / ".git").exists():
        print("--repo must be the root of a Git Cargo workspace", file=sys.stderr)
        return 2
    runner = repo / "tools" / "kani_validation.py"
    if not runner.is_file():
        print("Kani validation runner is missing", file=sys.stderr)
        return 2
    suite_script = Path(__file__).resolve()
    try:
        pipeline_source_bytes = {
            "tools/rust_proof_suite.py": suite_script.read_bytes(),
            "tools/kani_validation.py": runner.read_bytes(),
            "tools/rust_proofs.json": registry_bytes,
        }
    except OSError as exc:
        print(f"cannot snapshot proof pipeline sources: {exc}", file=sys.stderr)
        return 2
    if out.exists():
        print("--out must not already exist", file=sys.stderr)
        return 2
    # Shard `i` of `n` is the i-th of `n` groups of about equal recorded time: the same registry
    # split across jobs, so that each job finishes within the CI time limit.
    shard_index, shard_count = shard
    # A changed-package run includes every enabled tier for affected packages.
    affected: set[str] | None = None
    changed: list[str] | None = None
    if changed_from is not None:
        try:
            graph = package_graph(repo)
        except SelectionError as exc:
            print(f"cannot select proofs: {exc}", file=sys.stderr)
            return 2
        missing = registry_packages_missing(registry["proofs"], graph)
        if missing:
            print(f"registry packages not in the workspace: {', '.join(missing)}", file=sys.stderr)
            return 2
        changed = changed_files(repo, changed_from)
        unresolved = unresolved_path_dependencies(graph)
        if unresolved:
            print(f"cannot select proofs: unresolved workspace path dependencies: {', '.join(unresolved)}", file=sys.stderr)
            return 2
        base_directories = package_directories_at(repo, changed_from)
        unmapped = unmapped_package_paths(graph, changed or [], base_directories)
        if unmapped:
            print(f"cannot select proofs: changed package paths are absent from HEAD: {', '.join(unmapped)}", file=sys.stderr)
            return 2
        affected = None if changed is None or has_unowned_package_root_changes(
            graph, changed, base_directories,
        ) else affected_packages(graph, changed)
    chosen = select_proofs(registry["proofs"], affected)
    selected = balance_shards(chosen, shard_count)[shard_index]
    selection_report = proof_selection_report(
        registry["proofs"], chosen, selected, affected, changed_from, changed,
    )
    if not selected:
        report = {
            "suite_status": "SKIPPED",
            "reason": "no proof selected for this shard",
            "selection": selection_report,
            "shard": f"{shard_index}/{shard_count}",
        }
        print(json.dumps(report))
        return 0 if (chosen or changed_from is not None) else 2
    try:
        source_snapshot = _source_snapshot(repo, [out, target_dir])
        source_snapshot_sha256 = source_mapping_sha256(source_snapshot)
    except (OSError, RuntimeError, subprocess.CalledProcessError, UnicodeError) as exc:
        print(f"cannot bind proof target namespaces to checkout sources: {exc}", file=sys.stderr)
        return 2
    out.parent.mkdir(parents=True, exist_ok=True)
    out.mkdir()
    pipeline_source_hashes: dict[str, str] = {}
    for logical_path, content in pipeline_source_bytes.items():
        destination = out / "pipeline-source" / logical_path
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(content)
        pipeline_source_hashes[logical_path] = hashlib.sha256(content).hexdigest()

    started_utc = dt.datetime.now(dt.timezone.utc).isoformat()
    outcomes = []
    for proof in selected:
        outcome = _run_one_proof(proof, repo, out, target_dir, runner, source_snapshot_sha256)
        outcomes.append(outcome)
        summary = {"proof": proof["id"], "status": outcome["status"]}
        if outcome["diagnostics"]:
            summary["diagnostics"] = outcome["diagnostics"]
        print(json.dumps(summary), flush=True)

    def unchanged(path: Path, key: str) -> bool:
        try:
            return _sha256_bytes(path.read_bytes()) == pipeline_source_hashes[key]
        except OSError:
            return False

    pipeline_source_unchanged = {
        "tools/rust_proof_suite.py": unchanged(suite_script, "tools/rust_proof_suite.py"),
        "tools/kani_validation.py": unchanged(runner, "tools/kani_validation.py"),
        "tools/rust_proofs.json": unchanged(registry_path, "tools/rust_proofs.json"),
    }
    suite = {
        "suite_version": 1,
        "registry_sha256": hashlib.sha256(registry_bytes).hexdigest(),
        "source_snapshot_sha256": source_snapshot_sha256,
        "pipeline_source_hashes": pipeline_source_hashes,
        "pipeline_source_unchanged": pipeline_source_unchanged,
        "started_utc": started_utc,
        "repo": str(repo),
        "selected_count": len(selected),
        "configured_count": len(registry["proofs"]),
        "shard": f"{shard_index}/{shard_count}",
        "selection": selection_report,
        "loop_unwind_presets": {
            proof["harness"]: proof.get("loop_unwind_overrides", []) for proof in selected
        },
        "status": "PASS" if all(x["status"] == "PASS" for x in outcomes)
        and all(pipeline_source_unchanged.values()) else "FAIL",
        "checks": outcomes,
    }
    (out / "suite.json").write_text(json.dumps(suite, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"suite_status": suite["status"], "selected": len(selected), "out": str(out)}))
    return 0 if suite["status"] == "PASS" else 1


def main(argv: list[str] | None = None) -> int:
    parser = repo_argument_parser(__doc__)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--target-dir", type=Path, required=True)
    parser.add_argument("--registry", type=Path)
    parser.add_argument(
        "--fail-fast", action="store_true",
        help="deprecated and ignored; every selected proof runs so failures are fully reported",
    )
    parser.add_argument(
        "--shard", default="0/1",
        help="run shard I of N (the I-th of N groups balanced by ci_seconds), for example 1/4",
    )
    parser.add_argument(
        "--changed-from", metavar="REV",
        help="run every enabled proof for packages changed since REV or depending on a changed package",
    )
    args = parser.parse_args(argv)
    if args.fail_fast:
        print("--fail-fast is deprecated and ignored; all selected proofs will run", file=sys.stderr)
    try:
        shard_index, shard_count = (int(x) for x in args.shard.split("/"))
    except ValueError:
        parser.error("--shard must look like I/N")
    if not 0 <= shard_index < shard_count:
        parser.error("--shard I/N needs 0 <= I < N")
    repo = args.repo.resolve(strict=True)
    registry_path = args.registry or repo / "tools" / "rust_proofs.json"
    return run_suite(
        repo, args.out, args.target_dir, registry_path,
        shard=(shard_index, shard_count), changed_from=args.changed_from,
    )


if __name__ == "__main__":
    raise SystemExit(main())
