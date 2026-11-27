"""Synthetic checks for changed-line mutation evidence validation."""

from __future__ import annotations

import unittest
from pathlib import Path
from unittest.mock import patch

import check_safety_mutations as checker


def report_for(*summaries: str) -> dict[str, object]:
    outcomes: list[dict[str, object]] = [
        {
            "scenario": "Baseline",
            "summary": "Success",
            "phase_results": [{"phase": "Test", "process_status": "Success"}],
        }
    ]
    for index, summary in enumerate(summaries):
        outcomes.append(
            {
                "scenario": {"Mutant": {"name": f"src/example.rs:{index + 1}:1"}},
                "summary": summary,
            }
        )
    counts = {
        "CaughtMutant": "caught",
        "MissedMutant": "missed",
        "Timeout": "timeout",
        "Unviable": "unviable",
        "Success": "success",
    }
    totals = {field: summaries.count(name) for name, field in counts.items()}
    return {
        "cargo_mutants_version": checker.REQUIRED_CARGO_MUTANTS_VERSION,
        "end_time": "synthetic-finished",
        "outcomes": outcomes,
        "total_mutants": len(summaries),
        **totals,
    }


class MutationEvidenceTests(unittest.TestCase):
    def test_caught_and_exactly_allowlisted_miss_reconcile(self) -> None:
        report = report_for("CaughtMutant", "MissedMutant")
        identity = "src/example.rs:2:1"
        result = checker.assess_outcomes(
            2,
            report,
            {identity: "Equivalent because a second checked guard rejects this boundary."},
        )
        self.assertEqual((result.total, result.caught, result.allowed_missed), (2, 1, 1))

    def test_unviable_is_not_misreported_as_killed(self) -> None:
        """Kills naive-exit-code mutation: cargo-mutants exit 0 with unviable=1."""
        report = report_for("Unviable")
        report["unviable"] = 1
        with self.assertRaisesRegex(checker.MutationCheckError, "cannot count as caught"):
            checker.assess_outcomes(0, report, {})

    def test_timeout_and_success_are_inconclusive(self) -> None:
        for summary, field in (("Timeout", "timeout"), ("Success", "success")):
            with self.subTest(summary=summary):
                report = report_for(summary)
                report[field] = 1
                with self.assertRaises(checker.MutationCheckError):
                    checker.assess_outcomes(0, report, {})

    def test_stale_or_weak_allowlist_fails(self) -> None:
        with self.assertRaisesRegex(checker.MutationCheckError, "stale identities"):
            checker.assess_outcomes(
                0,
                report_for("CaughtMutant"),
                {"src/old.rs:8:2": "An old mutation has a reason that is long enough."},
            )
        for reason in ("todo", "known equivalent, allowed"):
            with self.subTest(reason=reason), self.assertRaises(checker.MutationCheckError):
                checker.parse_reason("src/example.rs:2:1", reason)

    def test_summary_totals_must_reconcile(self) -> None:
        report = report_for("CaughtMutant")
        report["caught"] = 0
        with self.assertRaisesRegex(checker.MutationCheckError, "account for every"):
            checker.assess_outcomes(0, report, {})

    def test_empty_safety_diff_has_no_mutation_credit(self) -> None:
        selected, packages = checker.changed_safety_sources(
            ["docs/guide.md", "crates/oa-display/tests/colors.rs", "tools/demo.rs"]
        )
        self.assertEqual((selected, packages), ([], []))
        command = checker.cargo_mutants_command([], Path("d"), Path("o"))
        self.assertIn("--in-diff", command)
        self.assertNotIn("--in-place", command)

    def test_empty_range_is_reported_as_skip_without_credit(self) -> None:
        commit = "a" * 40

        def git_result(_repo: Path, *args: str) -> bytes:
            if args[0] == "rev-parse":
                return (commit + "\n").encode("ascii")
            if args[0] == "diff":
                return b""
            raise AssertionError(f"unexpected git operation: {args[0]}")

        with patch.object(checker, "git_bytes", side_effect=git_result):
            result = checker.run_check(Path("synthetic-repo"), "base", "head")
        self.assertEqual(result["status"], "skip-no-safety-source-diff")
        self.assertIs(result["pass_credit"], False)

    def test_scope_selects_only_safety_implementation_source(self) -> None:
        selected, packages = checker.changed_safety_sources(
            [
                "crates/oa-safety/src/inhibit.rs",
                "crates/oa-safety/tests/safety.rs",
                "crates/oa-track/src/tracker.rs",
                "tools/oa-regional-session/src/runtime.rs",
            ]
        )
        self.assertEqual(selected, ["crates/oa-safety/src/inhibit.rs"])
        self.assertEqual(packages, ["oa-safety"])


if __name__ == "__main__":
    unittest.main()
