"""Tests for the hosted fixture's expected-block verification mode."""

from __future__ import annotations

import contextlib
import io
import json
import unittest
from unittest.mock import patch

import check_safety_mutations as checker
import run_safety_mutants_fixture as fixture
from test_check_safety_mutations import report_for


def evidence() -> fixture.FixtureEvidence:
    report = report_for("CaughtMutant", "MissedMutant")
    for index, outcome in enumerate(report["outcomes"][1:], 1):
        mutant = outcome["scenario"]["Mutant"]
        mutant["package"] = "e04-mutants-fixture"
        mutant["file"] = "src/lib.rs"
        mutant["name"] = f"src/lib.rs:{index}:1: replace unobserved_diagnostic -> bool"
        mutant["function"]["function_name"] = "unobserved_diagnostic"
    packages = {"e04-mutants-fixture"}
    allowlist = fixture.expected_fixture_survivors(report["outcomes"])
    summary = checker.assess_outcomes(2, report, allowlist, packages)
    return fixture.FixtureEvidence(report, 2, packages, set(summary.identities), summary)


def evidence_without_survivor() -> fixture.FixtureEvidence:
    report = report_for("CaughtMutant")
    mutant = report["outcomes"][1]["scenario"]["Mutant"]
    mutant["package"] = "e04-mutants-fixture"
    mutant["file"] = "src/lib.rs"
    mutant["name"] = "src/lib.rs:1:1: replace unobserved_diagnostic -> bool"
    mutant["function"]["function_name"] = "unobserved_diagnostic"
    packages = {"e04-mutants-fixture"}
    summary = checker.assess_outcomes(0, report, {}, packages)
    return fixture.FixtureEvidence(report, 0, packages, set(summary.identities), summary)


class NegativeProofTests(unittest.TestCase):
    def test_negative_mode_runs_positive_control_then_reports_expected_status_one(self) -> None:
        item = evidence()
        events: list[str] = []
        original_assess = checker.assess_outcomes

        def positive_control() -> fixture.FixtureEvidence:
            events.append("positive")
            return item

        def assessed(*args, **kwargs):
            events.append("negative")
            return original_assess(*args, **kwargs)

        output = io.StringIO()
        with patch.object(fixture, "run_fixture", side_effect=positive_control):
            with patch.object(checker, "assess_outcomes", side_effect=assessed):
                with contextlib.redirect_stdout(output):
                    status = fixture.main(["--negative-proof"])
        receipt = json.loads(output.getvalue())
        self.assertEqual(events, ["positive", "negative"])
        self.assertEqual(status, 1)
        self.assertEqual(receipt["status"], "negative-proof-verified")
        self.assertEqual(receipt["expected_block_status"], 1)
        self.assertTrue(receipt["verified_refusal"])

    def test_unexpected_checker_schema_failure_returns_two_not_one(self) -> None:
        item = evidence()
        item.report["cargo_mutants_version"] = "unexpected"
        stderr = io.StringIO()
        with patch.object(fixture, "run_fixture", return_value=item):
            with contextlib.redirect_stderr(stderr):
                status = fixture.main(["--negative-proof"])
        receipt = json.loads(stderr.getvalue())
        self.assertEqual(status, 2)
        self.assertFalse(receipt["verified_refusal"])

    def test_absent_known_survivor_never_returns_expected_block_status(self) -> None:
        stderr = io.StringIO()
        with patch.object(fixture, "run_fixture", return_value=evidence_without_survivor()):
            with contextlib.redirect_stderr(stderr):
                status = fixture.main(["--negative-proof"])
        receipt = json.loads(stderr.getvalue())
        self.assertEqual(status, 2)
        self.assertFalse(receipt["verified_refusal"])

    def test_normal_mode_retains_passing_fixture_exit(self) -> None:
        item = evidence()
        output = io.StringIO()
        with patch.object(fixture, "run_fixture", return_value=item):
            with contextlib.redirect_stdout(output):
                status = fixture.main([])
        self.assertEqual(status, 0)
        self.assertEqual(json.loads(output.getvalue())["status"], "fixture-validated")


if __name__ == "__main__":
    unittest.main()
