#!/usr/bin/env python3
"""Run cargo-mutants only on changed Rust lines in the safety implementation crates.

The tool resolves a caller-supplied base/head range, creates a detached scratch worktree at
head, and leaves the writer checkout untouched. cargo-mutants uses its default copied-source
mode; this tool never supplies --in-place. Empty safety-source diffs report a skip with no
mutation credit. Unviable builds, timeouts, incomplete reports, and unexplained survivors fail.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any

SAFETY_CRATE_DIRS = frozenset(
    {"oa-ca", "oa-display", "oa-migration-switch", "oa-runway", "oa-safety"}
)
REQUIRED_CARGO_MUTANTS_VERSION = "27.1.0"
ALLOWLIST_RELATIVE_PATH = Path("validation/mutants-allowed.txt")
MAX_REPORT_BYTES = 16 * 1024 * 1024
MIN_ALLOW_REASON_CHARS = 24
MIN_ALLOW_REASON_WORDS = 4
PLACEHOLDER_WORDS = frozenset({"allow", "allowed", "because", "known", "n/a", "reason", "todo", "tbd"})


class MutationCheckError(RuntimeError):
    """The changed-line mutation evidence is incomplete or unsafe to accept."""


@dataclass(frozen=True)
class GitRange:
    base: str
    head: str


@dataclass(frozen=True)
class MutationSummary:
    total: int
    caught: int
    allowed_missed: int
    missed: int


def git_bytes(repo: Path, *args: str) -> bytes:
    try:
        result = subprocess.run(
            ["git", "-C", str(repo), *args],
            check=True,
            capture_output=True,
            timeout=30,
        )
    except (OSError, subprocess.SubprocessError) as error:
        raise MutationCheckError(f"git command failed: {args[0]}") from error
    return result.stdout


def resolve_commit(repo: Path, ref: str) -> str:
    if not ref or ref.startswith("-") or "\x00" in ref:
        raise MutationCheckError("base and head must be non-empty Git references")
    resolved = git_bytes(repo, "rev-parse", "--verify", "--end-of-options", f"{ref}^{{commit}}")
    commit = resolved.decode("ascii", errors="strict").strip()
    if not re.fullmatch(r"[0-9a-f]{40}", commit) or set(commit) == {"0"}:
        raise MutationCheckError("base/head did not resolve to a non-zero commit")
    return commit


def resolve_range(repo: Path, base_ref: str, head_ref: str) -> GitRange:
    return GitRange(resolve_commit(repo, base_ref), resolve_commit(repo, head_ref))


def safety_package_for_source(path: str) -> str | None:
    parts = PurePosixPath(path).parts
    if (
        len(parts) >= 4
        and parts[0] == "crates"
        and parts[1] in SAFETY_CRATE_DIRS
        and parts[2] == "src"
        and path.endswith(".rs")
    ):
        return parts[1]
    return None


def changed_safety_sources(paths: list[str]) -> tuple[list[str], list[str]]:
    selected: list[str] = []
    packages: set[str] = set()
    for path in paths:
        package = safety_package_for_source(path)
        if package is not None:
            selected.append(path)
            packages.add(package)
    return sorted(selected), sorted(packages)


def read_package_name(worktree: Path, package_dir: str) -> str:
    manifest = worktree / "crates" / package_dir / "Cargo.toml"
    if manifest.is_symlink() or not manifest.is_file() or manifest.stat().st_size > 256 * 1024:
        raise MutationCheckError(f"safety package manifest is missing or invalid: {package_dir}")
    try:
        package = tomllib.loads(manifest.read_text(encoding="utf-8")).get("package", {})
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise MutationCheckError(f"cannot parse safety package manifest: {package_dir}") from error
    name = package.get("name")
    if name != package_dir:
        raise MutationCheckError(f"safety crate directory/package mismatch: {package_dir}")
    return name


def range_diff(repo: Path, git_range: GitRange, paths: list[str]) -> bytes:
    if not paths:
        return b""
    return git_bytes(
        repo,
        "diff",
        "--no-ext-diff",
        "--no-color",
        "--no-renames",
        "--unified=0",
        f"{git_range.base}..{git_range.head}",
        "--",
        *paths,
    )


def parse_reason(identity: str, reason: str) -> str:
    text = reason.strip()
    words = re.findall(r"[A-Za-z0-9][A-Za-z0-9'-]*", text)
    if len(text) < MIN_ALLOW_REASON_CHARS or len(words) < MIN_ALLOW_REASON_WORDS:
        raise MutationCheckError(f"allowlisted mutant needs a substantive reason: {identity}")
    if {word.lower() for word in words} <= PLACEHOLDER_WORDS:
        raise MutationCheckError(f"allowlisted mutant reason is a placeholder: {identity}")
    return text


def read_allowlist(path: Path) -> dict[str, str]:
    if path.is_symlink() or not path.is_file() or path.stat().st_size > 1024 * 1024:
        raise MutationCheckError("mutant allowlist is not a bounded regular file")
    entries: dict[str, str] = {}
    for line_number, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if not raw.strip() or raw.lstrip().startswith("#"):
            continue
        fields = raw.split("\t")
        if len(fields) != 2:
            raise MutationCheckError(f"allowlist line {line_number} must be identity<TAB>reason")
        identity = fields[0].strip()
        if not identity or identity in entries or any(ord(char) < 32 for char in identity):
            raise MutationCheckError(f"allowlist line {line_number} has an invalid/duplicate identity")
        entries[identity] = parse_reason(identity, fields[1])
    return entries


def bounded_json(path: Path) -> dict[str, Any]:
    if path.is_symlink() or not path.is_file() or path.stat().st_size > MAX_REPORT_BYTES:
        raise MutationCheckError("cargo-mutants outcomes report is missing or exceeds its byte cap")
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise MutationCheckError("cargo-mutants outcomes report is unreadable") from error
    if not isinstance(value, dict):
        raise MutationCheckError("cargo-mutants outcomes report must be a JSON object")
    return value


def mutant_identity(outcome: dict[str, Any]) -> str:
    scenario = outcome.get("scenario")
    if not isinstance(scenario, dict) or set(scenario) != {"Mutant"}:
        raise MutationCheckError("cargo-mutants report contains an unexpected scenario")
    mutant = scenario["Mutant"]
    identity = mutant.get("name") if isinstance(mutant, dict) else None
    if not isinstance(identity, str) or not identity.strip():
        raise MutationCheckError("cargo-mutants report has a mutant without an exact identity")
    return identity


def check_baseline(outcomes: list[dict[str, Any]]) -> None:
    baselines = [row for row in outcomes if row.get("scenario") == "Baseline"]
    if len(baselines) != 1 or baselines[0].get("summary") != "Success":
        raise MutationCheckError("cargo-mutants did not record one successful baseline")
    phases = baselines[0].get("phase_results")
    if not isinstance(phases, list) or not any(
        isinstance(phase, dict)
        and phase.get("phase") == "Test"
        and phase.get("process_status") == "Success"
        for phase in phases
    ):
        raise MutationCheckError("cargo-mutants baseline did not complete its tests")


def checked_counts(report: dict[str, Any]) -> dict[str, int]:
    names = ("total_mutants", "caught", "missed", "timeout", "unviable", "success")
    counts: dict[str, int] = {}
    for name in names:
        value = report.get(name)
        if type(value) is not int or value < 0:
            raise MutationCheckError(f"cargo-mutants report has invalid {name} count")
        counts[name] = value
    accounted = sum(counts[name] for name in ("caught", "missed", "timeout", "unviable", "success"))
    if accounted != counts["total_mutants"] or counts["total_mutants"] == 0:
        raise MutationCheckError("cargo-mutants did not account for every changed-line mutant")
    return counts


def assess_outcomes(
    exit_code: int,
    report: dict[str, Any],
    allowlist: dict[str, str],
) -> MutationSummary:
    if report.get("cargo_mutants_version") != REQUIRED_CARGO_MUTANTS_VERSION:
        raise MutationCheckError("cargo-mutants outcomes came from an unexpected version")
    if not report.get("end_time"):
        raise MutationCheckError("cargo-mutants report is incomplete")
    outcomes = report.get("outcomes")
    if not isinstance(outcomes, list) or any(not isinstance(row, dict) for row in outcomes):
        raise MutationCheckError("cargo-mutants report has invalid scenario outcomes")
    check_baseline(outcomes)
    counts = checked_counts(report)
    mutant_outcomes = [row for row in outcomes if row.get("scenario") != "Baseline"]
    if len(mutant_outcomes) != counts["total_mutants"]:
        raise MutationCheckError("scenario count differs from cargo-mutants totals")

    summary_fields = {
        "CaughtMutant": "caught",
        "MissedMutant": "missed",
        "Timeout": "timeout",
        "Unviable": "unviable",
        "Success": "success",
    }
    by_summary = dict.fromkeys(summary_fields, 0)
    missed_ids: set[str] = set()
    for row in mutant_outcomes:
        summary = row.get("summary")
        if summary not in by_summary:
            raise MutationCheckError("cargo-mutants reported an unclassified mutant outcome")
        by_summary[summary] += 1
        if summary == "MissedMutant":
            identity = mutant_identity(row)
            if identity in missed_ids:
                raise MutationCheckError("cargo-mutants reported a duplicate missed identity")
            missed_ids.add(identity)
    if any(by_summary[key] != counts[field] for key, field in summary_fields.items()):
        raise MutationCheckError("scenario outcomes do not reconcile with cargo-mutants totals")
    if counts["timeout"] or counts["unviable"] or counts["success"]:
        raise MutationCheckError("timeout, unviable, or unclassified mutants cannot count as caught")

    stale = set(allowlist) - missed_ids
    if stale:
        raise MutationCheckError("mutant allowlist contains stale identities")
    unallowed = missed_ids - set(allowlist)
    if unallowed:
        raise MutationCheckError("changed-line safety mutants survived without an exact reason")
    expected_exit = 2 if counts["missed"] else 0
    if exit_code != expected_exit:
        raise MutationCheckError("cargo-mutants exit code does not match the complete outcome report")
    return MutationSummary(counts["total_mutants"], counts["caught"], counts["missed"], len(missed_ids))


def cargo_mutants_version(worktree: Path) -> str:
    try:
        result = subprocess.run(
            ["cargo", "mutants", "--version"],
            cwd=worktree,
            check=True,
            capture_output=True,
            text=True,
            timeout=30,
        )
    except (OSError, subprocess.SubprocessError) as error:
        raise MutationCheckError("cargo-mutants 27.1.0 is required") from error
    match = re.fullmatch(r"cargo-mutants ([0-9]+\.[0-9]+\.[0-9]+)\s*", result.stdout)
    if match is None or match.group(1) != REQUIRED_CARGO_MUTANTS_VERSION:
        raise MutationCheckError("cargo-mutants version does not match the recorded output format")
    return match.group(1)


def cargo_mutants_command(
    packages: list[str], diff_path: Path, output_parent: Path
) -> list[str]:
    command = [
        "cargo",
        "mutants",
        "--in-diff",
        str(diff_path),
        "--jobs",
        "1",
        "--no-shuffle",
        "--test-workspace",
        "false",
        "--output",
        str(output_parent),
    ]
    for package in packages:
        command.extend(["--package", package])
    return command


def run_cargo_mutants(command: list[str], worktree: Path) -> int:
    try:
        result = subprocess.run(command, cwd=worktree, check=False)
    except OSError as error:
        raise MutationCheckError("cargo-mutants could not be started") from error
    return result.returncode


def add_scratch_worktree(repo: Path, commit: str, scratch: Path) -> Path:
    worktree = scratch / "checkout"
    try:
        subprocess.run(
            ["git", "-C", str(repo), "worktree", "add", "--detach", str(worktree), commit],
            check=True,
            capture_output=True,
            timeout=60,
        )
    except (OSError, subprocess.SubprocessError) as error:
        raise MutationCheckError("cannot create detached scratch worktree") from error
    return worktree


def emit(result: dict[str, Any], stream: Any = sys.stdout) -> None:
    print(json.dumps(result, sort_keys=True, separators=(",", ":")), file=stream)


def run_check(repo: Path, base_ref: str, head_ref: str) -> dict[str, Any]:
    git_range = resolve_range(repo, base_ref, head_ref)
    name_bytes = git_bytes(
        repo,
        "diff",
        "--name-only",
        "-z",
        "--no-renames",
        f"{git_range.base}..{git_range.head}",
        "--",
        *(f"crates/{name}/src" for name in sorted(SAFETY_CRATE_DIRS)),
    )
    changed_paths = [path.decode("utf-8") for path in name_bytes.split(b"\x00") if path]
    source_files, package_dirs = changed_safety_sources(changed_paths)
    base_result = {"base": git_range.base, "head": git_range.head}
    if not source_files:
        return dict(
            base_result,
            status="skip-no-safety-source-diff",
            pass_credit=False,
            changed_source_files=[],
            message="No changed Rust source lines in the safety crate scope.",
        )

    with tempfile.TemporaryDirectory(prefix="openatca-safety-mutants-") as scratch_name:
        scratch = Path(scratch_name).resolve(strict=True)
        worktree = add_scratch_worktree(repo, git_range.head, scratch)
        worktree_added = True
        try:
            packages = [read_package_name(worktree, package) for package in package_dirs]
            diff = range_diff(repo, git_range, source_files)
            if not diff:
                return dict(
                    base_result,
                    status="skip-empty-safety-diff",
                    pass_credit=False,
                    changed_source_files=source_files,
                    message="The selected safety source diff is empty; no mutation credit.",
                )
            diff_path = scratch / "safety-source.diff"
            diff_path.write_bytes(diff)
            version = cargo_mutants_version(worktree)
            output_parent = scratch / "cargo-mutants-output"
            command = cargo_mutants_command(packages, diff_path, output_parent)
            exit_code = run_cargo_mutants(command, worktree)
            report = bounded_json(output_parent / "mutants.out" / "outcomes.json")
            allowlist = read_allowlist(worktree / ALLOWLIST_RELATIVE_PATH)
            summary = assess_outcomes(exit_code, report, allowlist)
            return dict(
                base_result,
                status="pass",
                pass_credit=True,
                cargo_mutants_version=version,
                changed_source_files=source_files,
                packages=packages,
                total_mutants=summary.total,
                caught=summary.caught,
                missed_allowed=summary.allowed_missed,
                missed_total=summary.missed,
            )
        finally:
            if worktree_added:
                try:
                    subprocess.run(
                        ["git", "-C", str(repo), "worktree", "remove", "--force", str(worktree)],
                        check=True,
                        capture_output=True,
                        timeout=60,
                    )
                except (OSError, subprocess.SubprocessError) as error:
                    raise MutationCheckError("cannot remove detached scratch worktree") from error


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", required=True, help="diff base commit/ref (PR base, push before, or origin/main)")
    parser.add_argument("--head", required=True, help="diff head commit/ref (PR head, push after, or HEAD)")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    try:
        repo = Path(__file__).resolve().parents[1]
        args = parse_args(argv)
        result = run_check(repo, args.base, args.head)
        emit(result)
        return 0 if result["status"].startswith("skip-") or result["status"] == "pass" else 1
    except MutationCheckError as error:
        emit({"status": "fail", "pass_credit": False, "error": str(error)}, sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
