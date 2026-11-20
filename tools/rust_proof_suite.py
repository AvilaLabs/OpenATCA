"""Run the repository's registered Kani proof suite sequentially."""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time
import tomllib
from typing import Any

from kani_validation import LOOP_UNWIND_PRESETS


REGISTRY_VERSION = 1
MAX_PROOFS = 128
ALLOWED_PACKAGES = {
    "oa-track", "oa-replay", "oa-safety", "oa-ca", "oa-math", "oa-runway", "oa-asterix", "oa-adsb",
    "oa-feed", "oa-truth",
}
ALLOWED_CFGS = {"kani_full_capacity", "kani_late_replay_covers"}
ID_RE = re.compile(r"^[a-z0-9][a-z0-9-]*$")
HARNESS_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)*$")


# Paths whose change reruns every push-tier proof: the registry, the runner, the workflow, the
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
_PACKAGE_ROOTS = ("crates", "tools")
_DEPENDENCY_TABLES = ("dependencies", "build-dependencies", "dev-dependencies")


class SelectionError(ValueError):
    """The change cannot be mapped to packages; the run must not guess."""


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
    proofs: list[dict[str, Any]], affected: set[str] | None, push: bool,
) -> list[dict[str, Any]]:
    """The enabled proofs to run. A push run takes only the push tier and, unless a change to
    the registry, runner or workflow selects everything, only the affected packages."""
    chosen = [p for p in proofs if p["enabled"]]
    if push:
        chosen = [p for p in chosen if p["tier"] == "push"]
        if affected is not None:
            chosen = [p for p in chosen if p["package"] in affected]
    return chosen


def changed_files(repo: Path, base: str) -> list[str] | None:
    """Files changed since `base`, or None when the comparison cannot be made. Renames are
    reported as a delete and an add, so both the old and the new path count as changed."""
    if base.startswith("-") or not re.fullmatch(r"[0-9a-fA-F]{7,64}|[A-Za-z0-9_./-]+", base):
        return None
    if set(base) == {"0"}:
        return None
    try:
        out = subprocess.run(
            ["git", "diff", "--name-only", "--no-renames", base, "HEAD", "--"], cwd=repo,
            check=True, capture_output=True, text=True, timeout=60,
        ).stdout
    except (OSError, subprocess.SubprocessError):
        return None
    return [line for line in out.splitlines() if line]


def balance_shards(proofs: list[dict[str, Any]], count: int) -> list[list[dict[str, Any]]]:
    """Split `proofs` into `count` shards of about equal `ci_seconds`: longest first onto the
    least loaded shard, ties to the lowest shard, each shard in registry order."""
    shards: list[list[dict[str, Any]]] = [[] for _ in range(count)]
    load = [0] * count
    order = {id(p): i for i, p in enumerate(proofs)}
    for proof in sorted(proofs, key=lambda p: (-p["ci_seconds"], order[id(p)])):
        target = min(range(count), key=lambda i: (load[i], i))
        shards[target].append(proof)
        load[target] += proof["ci_seconds"]
    for shard in shards:
        shard.sort(key=lambda p: order[id(p)])
    return shards


def _canonical_sha256(value: Any) -> str:
    data = json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
    return hashlib.sha256(data).hexdigest()


def _sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


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


def _not_run_outcome(proof: dict[str, Any], failed_id: str) -> dict[str, Any]:
    return {
        "id": proof["id"],
        "package": proof["package"],
        "harness": proof["harness"],
        "loop_unwind_overrides": proof.get("loop_unwind_overrides", []),
        "experimental": proof["experimental"],
        "status": "NOT_RUN",
        "exit_code": None,
        "timed_out": None,
        "elapsed_seconds": None,
        "command": None,
        "command_sha256": None,
        "run_record_sha256": None,
        "run_dir": None,
        "reason": f"fail-fast after proof {failed_id} did not pass",
        "diagnostics": [],
    }


def run_suite(
    repo: Path, out: Path, target_dir: Path, registry_path: Path, fail_fast: bool = False,
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
    # A push run (--changed-from given) takes only the push tier of the packages the change reaches;
    # without it the whole enabled registry runs, nightly tier included. The shards are balanced by
    # recorded seconds over what was chosen.
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
        affected = None if changed is None else affected_packages(graph, changed)
    chosen = select_proofs(registry["proofs"], affected, changed_from is not None)
    selected = balance_shards(chosen, shard_count)[shard_index]
    if changed_from is not None and not selected:
        print(json.dumps({
            "suite_status": "SKIPPED", "reason": "no proof of this shard is affected",
            "base": changed_from,
            "changed_files": changed if changed is not None else "unknown, all packages assumed",
            "affected_packages": sorted(affected) if affected is not None else "all",
        }))
        return 0
    if not selected:
        print("registry has no enabled proof entries", file=sys.stderr)
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
    failed_proof_id: str | None = None
    for proof in selected:
        if fail_fast and failed_proof_id is not None:
            outcome = _not_run_outcome(proof, failed_proof_id)
            outcomes.append(outcome)
            print(json.dumps({
                "proof": proof["id"], "status": "NOT_RUN", "reason": outcome["reason"],
            }), flush=True)
            continue
        proof_dir = out / "proofs" / proof["id"]
        proof_dir.mkdir(parents=True, exist_ok=False)
        run_dir = proof_dir / "run"
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
        process: subprocess.Popen[Any] | None = None
        timed_out = False
        try:
            with stdout_path.open("w", encoding="utf-8") as stdout, stderr_path.open(
                "w", encoding="utf-8"
            ) as stderr:
                process = subprocess.Popen(
                    cmd, cwd=repo, env=os.environ.copy(), stdout=stdout, stderr=stderr,
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
        ok = (
            rc == 0 and not timed_out and isinstance(run_record, dict)
            and run_record.get("status") == "PASS"
            and run_record.get("expected_harnesses") == [proof["harness"]]
            and run_record.get("package") == proof["package"]
            and run_record.get("required_covers") == {proof["harness"]: proof["required_covers"]}
            and run_record.get("loop_unwind_presets", {}).get(proof["harness"], [])
            == proof.get("loop_unwind_overrides", [])
        )
        diagnostics = [] if ok else _failure_diagnostics(run_record, rc, timed_out, run_record_path)
        outcomes.append({
            "id": proof["id"],
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
            "run_record_sha256": hashlib.sha256(run_record_path.read_bytes()).hexdigest()
            if run_record_path.is_file() else None,
            "run_dir": str(run_dir),
        })
        summary = {"proof": proof["id"], "status": outcomes[-1]["status"]}
        if diagnostics:
            summary["diagnostics"] = diagnostics
        print(json.dumps(summary), flush=True)
        if fail_fast and not ok:
            failed_proof_id = proof["id"]

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
        "pipeline_source_hashes": pipeline_source_hashes,
        "pipeline_source_unchanged": pipeline_source_unchanged,
        "started_utc": started_utc,
        "repo": str(repo),
        "selected_count": len(selected),
        "configured_count": len(registry["proofs"]),
        "shard": f"{shard_index}/{shard_count}",
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
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--target-dir", type=Path, required=True)
    parser.add_argument("--registry", type=Path)
    parser.add_argument(
        "--fail-fast", action="store_true",
        help="stop starting proofs after the first non-PASS result",
    )
    parser.add_argument(
        "--shard", default="0/1",
        help="run shard I of N (the I-th of N groups balanced by ci_seconds), for example 1/4",
    )
    parser.add_argument(
        "--changed-from", metavar="REV",
        help="push run: only the push-tier proofs of packages changed since REV or depending on "
        "a changed package; without it the full suite runs",
    )
    args = parser.parse_args(argv)
    try:
        shard_index, shard_count = (int(x) for x in args.shard.split("/"))
    except ValueError:
        parser.error("--shard must look like I/N")
    if not 0 <= shard_index < shard_count:
        parser.error("--shard I/N needs 0 <= I < N")
    repo = args.repo.resolve(strict=True)
    registry_path = args.registry or repo / "tools" / "rust_proofs.json"
    return run_suite(
        repo, args.out, args.target_dir, registry_path, fail_fast=args.fail_fast,
        shard=(shard_index, shard_count), changed_from=args.changed_from,
    )


if __name__ == "__main__":
    raise SystemExit(main())
