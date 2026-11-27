"""Run a tiny Rust fixture through cargo-mutants and validate the real JSON output."""

from __future__ import annotations

import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

import check_safety_mutations as checker


def copy_fixture(destination: Path) -> None:
    source = Path(__file__).resolve().parents[1] / "validation" / "fixtures" / "safety-mutation-smoke"
    shutil.copytree(source, destination)


def git_fixture_baseline(repo: Path) -> None:
    subprocess.run(["git", "init", "--quiet"], cwd=repo, check=True, timeout=30)
    baseline = repo / "baseline-lib.rs"
    source = repo / "src" / "lib.rs"
    final_source = source.read_bytes()
    source.write_bytes(baseline.read_bytes())
    subprocess.run(
        ["git", "add", "Cargo.toml", "src/lib.rs", "tests/decision.rs"],
        cwd=repo,
        check=True,
        timeout=30,
    )
    source.write_bytes(final_source)


def changed_lines(repo: Path, destination: Path) -> None:
    result = subprocess.run(
        ["git", "diff", "--no-ext-diff", "--no-color", "--unified=0", "--", "src/lib.rs"],
        cwd=repo,
        check=False,
        capture_output=True,
        timeout=30,
    )
    if result.returncode not in (0, 1):
        raise checker.MutationCheckError("git could not create the fixture source diff")
    diff = result.stdout
    if not diff:
        raise checker.MutationCheckError("fixture diff is empty; no mutation schema evidence")
    destination.write_bytes(diff)


def expected_fixture_survivors(outcomes: list[dict[str, Any]]) -> dict[str, str]:
    allowed: dict[str, str] = {}
    caught = 0
    for outcome in outcomes:
        scenario = outcome.get("scenario")
        if scenario == "Baseline":
            continue
        package, _file, name = checker.mutant_identity(outcome)
        if package != "e04-mutants-fixture":
            raise checker.MutationCheckError("fixture emitted a mutant from another package")
        if outcome.get("summary") == "CaughtMutant":
            caught += 1
        elif outcome.get("summary") == "MissedMutant":
            mutant = scenario["Mutant"]
            function = mutant.get("function")
            function_name = function.get("function_name") if isinstance(function, dict) else None
            if function_name != "unobserved_diagnostic":
                raise checker.MutationCheckError("fixture survivor escaped its intentionally untested function")
            allowed[f"{package}::{name}"] = (
                "Synthetic fixture intentionally leaves this diagnostic untested."
            )
    if caught == 0 or not allowed:
        raise checker.MutationCheckError("fixture must produce both caught and intentionally missed mutants")
    return allowed


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="openatca-mutants-fixture-") as temporary:
        root = Path(temporary)
        repo = root / "fixture"
        copy_fixture(repo)
        git_fixture_baseline(repo)
        diff = root / "fixture.diff"
        changed_lines(repo, diff)
        package_set = {"e04-mutants-fixture"}
        listed_run = subprocess.run(
            checker.cargo_mutants_list_command(sorted(package_set), diff),
            cwd=repo,
            check=False,
            capture_output=True,
            text=True,
            timeout=120,
        )
        if listed_run.returncode != 0:
            raise checker.MutationCheckError("cargo-mutants fixture listing failed")
        listed = checker.parse_mutant_list(listed_run.stdout, package_set)
        if not listed:
            raise checker.MutationCheckError("fixture did not produce any changed-line mutants")
        output = root / "mutants-output"
        command = [
            "cargo", "mutants", "--package", "e04-mutants-fixture", "--test-workspace", "false",
            "--in-diff", str(diff), "--jobs", "1", "--no-shuffle", "--output", str(output),
        ]
        result = subprocess.run(command, cwd=repo, check=False)
        report = checker.bounded_json(output / "mutants.out" / "outcomes.json")
        allowlist = expected_fixture_survivors(report.get("outcomes", []))
        summary = checker.assess_outcomes(
            result.returncode, report, allowlist, package_set
        )
        checker.require_complete_selection(listed, summary)
        checker.emit(
            {
                "status": "fixture-validated",
                "cargo_mutants_version": checker.REQUIRED_CARGO_MUTANTS_VERSION,
                "caught": summary.caught,
                "intentional_survivors": summary.allowed_missed,
                "total": summary.total,
            }
        )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except checker.MutationCheckError as error:
        checker.emit({"status": "fixture-failed", "error": str(error)}, sys.stderr)
        raise SystemExit(1)
