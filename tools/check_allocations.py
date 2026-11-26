#!/usr/bin/env python3
"""Build and run the isolated M3 allocator fixtures and their negative control."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time

sys.dont_write_bytecode = True
from allocator_validation import (allocation_layout_errors, allocation_route_ledger_errors,
                                  evaluate_allocator_result)


def sha(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def source_state(repo):
    head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=repo, text=True).strip()
    status = subprocess.check_output(["git", "status", "--porcelain"], cwd=repo, text=True)
    tracked = subprocess.check_output(["git", "ls-files", "-z"], cwd=repo).split(b"\0")
    paths = {Path(p.decode()) for p in tracked if p}
    paths.update(Path(name) for name in ("tools/check_allocations.py", "tools/allocator_validation.py",
                                         "tools/test_allocator_validation.py") if (repo / name).is_file())
    paths.update(p.relative_to(repo) for p in (repo / "validation/oa-alloc-check").rglob("*")
                 if p.is_file() and "target" not in p.parts)
    return {"revision": head, "status": status,
            "files": {str(p): sha(repo / p) for p in sorted(paths)}}


def run(args):
    repo, target, out = args.repo.resolve(), args.target_dir.resolve(), args.out.resolve()
    if target == repo or repo in target.parents or out == repo or repo in out.parents:
        raise ValueError("target and evidence directories must be outside the repository")
    out.mkdir(parents=True, exist_ok=False)
    env = dict(os.environ, CARGO_BUILD_JOBS="1", CARGO_NET_OFFLINE="true", RUSTFLAGS="")
    report = {"scope": "M3 exercised routes plus the H5 4096-row selector profile; not all inputs or all real-time crates",
              "commands": [], "errors": [], "layout_errors": [], "binary_sha256": None}
    manifest = repo / "validation/oa-alloc-check/Cargo.toml"
    binary = target / "release/oa-alloc-check"

    def command(name, argv, timeout=1800):
        started = time.monotonic()
        try:
            process = subprocess.run(argv, cwd=repo, env=env, capture_output=True,
                                     text=True, timeout=timeout)
            code, stdout, stderr = process.returncode, process.stdout, process.stderr
        except subprocess.TimeoutExpired:
            code, stdout, stderr = 124, "", "TIMEOUT"
        (out / f"{name}.stdout.txt").write_text(stdout)
        (out / f"{name}.stderr.txt").write_text(stderr)
        report["commands"].append({"name": name, "argv": list(map(str, argv)),
                                   "exit_code": code, "elapsed_s": time.monotonic() - started,
                                   "status": "PASS" if code == 0 else "FAIL"})
        return code, stdout

    try:
        report["source_before"] = source_state(repo)
        report["layout_errors"] = allocation_layout_errors(repo)
        report["errors"].extend(report["layout_errors"])
        report["route_ledger_errors"] = allocation_route_ledger_errors(repo)
        report["errors"].extend(report["route_ledger_errors"])
        lock = manifest.with_name("Cargo.lock")
        if not lock.is_file() or 'source = ' in lock.read_text():
            report["errors"].append("standalone lockfile missing or contains an external dependency")
        if not report["errors"]:
            operations = [
                ("tracker-capacity-tests", [
                    "test", "--manifest-path", str(repo / "Cargo.toml"), "-p", "oa-track-core",
                    "--locked", "--offline", "--target-dir",
                    str(target / "tracker-capacity-tests"), "--", "tracker::tests::",
                ]),
                ("format", ["fmt", "--manifest-path", str(manifest), "--", "--check"]),
                ("lint", ["clippy", "--manifest-path", str(manifest), "--locked", "--all-targets",
                          "--target-dir", str(target), "--", "-D", "warnings"]),
                ("build", ["build", "--release", "--manifest-path", str(manifest), "--locked",
                           "--target-dir", str(target)]),
                ("tests", ["test", "--release", "--manifest-path", str(manifest), "--locked",
                           "--target-dir", str(target), "--", "--test-threads=1"]),
            ]
            build_ok = False
            for name, flags in operations:
                code, _ = command(name, ["cargo", "+1.89.0", *flags])
                if code:
                    report["errors"].append(f"{name}: exit {code}")
                if name == "build":
                    build_ok = code == 0 and binary.is_file()
            if build_ok:
                report["binary_sha256"] = sha(binary)
                for injected, name in ((False, "normal"), (True, "injected")):
                    argv = [str(binary)] + (["--inject-allocation"] if injected else [])
                    code, stdout = command(name, argv, 120)
                    errors = evaluate_allocator_result(stdout, expect_injected=injected)
                    expected_code = 1 if injected else 0
                    if code != expected_code:
                        errors.append(f"{name}: expected exit {expected_code}, got {code}")
                    report["commands"][-1]["status"] = "FAIL" if errors else "PASS"
                    report["errors"].extend(errors)
                    try:
                        report[name + "_result"] = json.loads(stdout)
                    except (ValueError, TypeError):
                        report[name + "_result"] = None
                report["binary_unchanged"] = sha(binary) == report["binary_sha256"]
                if not report["binary_unchanged"]:
                    report["errors"].append("allocator binary changed during execution")
            else:
                report["errors"].append("allocator binary did not build; runtime checks not run")
        report["source_after"] = source_state(repo)
        report["source_unchanged"] = report["source_after"] == report["source_before"]
        if not report["source_unchanged"]:
            report["errors"].append("source changed during allocator check")
    except (OSError, ValueError, TypeError, subprocess.SubprocessError) as error:
        report["errors"].append(f"allocator check exception: {error}")
    required = {"tracker-capacity-tests", "format", "lint", "build", "tests", "normal", "injected"}
    commands = {item["name"]: item["status"] for item in report["commands"]}
    if any(commands.get(name) != "PASS" for name in required):
        report["errors"].append("required allocator checks did not all pass")
    report["status"] = "FAIL" if report["errors"] else "PASS"
    report["evidence_hashes"] = {p.name: sha(p) for p in out.iterdir() if p.is_file()}
    (out / "result.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"status": report["status"], "errors": report["errors"]}))
    return 1 if report["errors"] else 0


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parent.parent)
    parser.add_argument("--target-dir", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    try:
        return run(parser.parse_args())
    except (OSError, ValueError) as error:
        print(f"allocator check: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
