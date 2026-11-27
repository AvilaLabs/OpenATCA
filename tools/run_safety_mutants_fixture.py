"""Run a tiny Rust fixture through cargo-mutants and validate the real JSON output."""

from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import check_safety_mutations as checker


@dataclass(frozen=True)
class FixtureEvidence:
    report: dict[str, Any]
    cargo_exit: int
    packages: set[str]
    selected: set[str]
    positive_summary: checker.MutationSummary


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


def run_fixture() -> FixtureEvidence:
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
        result = subprocess.run(command, cwd=repo, check=False, timeout=600)
        report = checker.bounded_json(output / "mutants.out" / "outcomes.json")
        allowlist = expected_fixture_survivors(report.get("outcomes", []))
        summary = checker.assess_outcomes(
            result.returncode, report, allowlist, package_set
        )
        checker.require_complete_selection(listed, summary)
        return FixtureEvidence(report, result.returncode, package_set, listed, summary)


def verify_negative_proof(evidence: FixtureEvidence) -> dict[str, Any]:
    survivors = expected_fixture_survivors(evidence.report.get("outcomes", []))
    if len(survivors) != 1:
        raise checker.MutationCheckError("negative proof requires exactly one known surviving mutant")
    if evidence.positive_summary.caught < 1 or evidence.positive_summary.allowed_missed < 1:
        raise checker.MutationCheckError("positive control did not validate caught and allowed-missed cases")
    try:
        checker.assess_outcomes(evidence.cargo_exit, evidence.report, {}, evidence.packages)
    except checker.MutationCheckError as error:
        expected = "changed-line safety mutants survived without an exact reason"
        if str(error) != expected:
            raise checker.MutationCheckError("negative proof was blocked for an unexpected reason") from error
        return {
            "status": "negative-proof-verified",
            "expected_block_status": 1,
            "verified_refusal": expected,
            "cargo_mutants_version": checker.REQUIRED_CARGO_MUTANTS_VERSION,
            "positive_control": "passed",
            "caught": evidence.positive_summary.caught,
            "known_missed_mutant": next(iter(survivors))[:256],
            "selected": len(evidence.selected),
        }
    raise checker.MutationCheckError("negative proof unexpectedly accepted the unallowlisted survivor")


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--negative-proof", action="store_true",
        help="verify that an intentionally missed fixture mutant receives the expected block",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    negative = False
    try:
        negative = parse_args(argv).negative_proof
        evidence = run_fixture()
        if negative:
            receipt = verify_negative_proof(evidence)
            checker.emit(receipt, sys.stdout)
            return 1
        checker.emit(
            {
                "status": "fixture-validated",
                "cargo_mutants_version": checker.REQUIRED_CARGO_MUTANTS_VERSION,
                "caught": evidence.positive_summary.caught,
                "intentional_survivors": evidence.positive_summary.allowed_missed,
                "total": evidence.positive_summary.total,
            },
            sys.stdout,
        )
        return 0
    except Exception as error:
        if negative:
            checker.emit(
                {"status": "negative-proof-invalid", "verified_refusal": False, "error": str(error)[:300]},
                sys.stderr,
            )
            return 2
        if isinstance(error, checker.MutationCheckError):
            checker.emit({"status": "fixture-failed", "error": str(error)}, sys.stderr)
            return 1
        raise


if __name__ == "__main__":
    raise SystemExit(main())
