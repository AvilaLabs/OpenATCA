"""Synthetic tests for the cargo-mutants 27.1.0 report contract."""

from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import check_safety_mutations as checker


def phase(name: str, status: object) -> dict[str, object]:
    return {"phase": name, "duration": 0.001, "process_status": status, "argv": ["cargo", "test"]}


def span() -> dict[str, dict[str, int]]:
    return {"start": {"line": 1, "column": 1}, "end": {"line": 1, "column": 2}}


def report_for(*summaries: str) -> dict[str, object]:
    outcomes: list[dict[str, object]] = [
        {
            "scenario": "Baseline",
            "summary": "Success",
            "log_path": "baseline.log",
            "diff_path": None,
            "phase_results": [phase("Build", "Success"), phase("Test", "Success")],
        }
    ]
    for index, summary in enumerate(summaries):
        name = f"src/example.rs:{index + 1}:1: replace sample -> bool"
        status: object = {"Failure": 101} if summary == "CaughtMutant" else "Success"
        outcomes.append(
            {
                "scenario": {
                    "Mutant": {
                        "name": name,
                        "package": "oa-safety",
                        "file": "src/example.rs",
                        "function": {"function_name": "sample", "return_type": "bool", "span": span()},
                        "span": span(),
                        "replacement": "false",
                        "genre": "FnValue",
                    }
                },
                "summary": summary,
                "log_path": f"mutant-{index}.log",
                "diff_path": f"mutant-{index}.diff",
                "phase_results": [phase("Build", "Success"), phase("Test", status)],
            }
        )
    names = {
        "CaughtMutant": "caught",
        "MissedMutant": "missed",
        "Timeout": "timeout",
        "Unviable": "unviable",
        "Success": "success",
    }
    return {
        "outcomes": outcomes,
        "total_mutants": len(summaries),
        **{field: summaries.count(summary) for summary, field in names.items()},
        "start_time": "2026-11-27T00:00:00Z",
        "end_time": "2026-11-27T00:00:01Z",
        "cargo_mutants_version": checker.REQUIRED_CARGO_MUTANTS_VERSION,
    }


class MutationEvidenceTests(unittest.TestCase):
    def test_real_schema_caught_and_allowlisted_survivor(self) -> None:
        report = report_for("CaughtMutant", "MissedMutant")
        identity = "oa-safety::src/example.rs:2:1: replace sample -> bool"
        result = checker.assess_outcomes(
            2,
            report,
            {identity: "Equivalent because a second checked guard rejects this boundary."},
            {"oa-safety"},
        )
        self.assertEqual((result.total, result.caught, result.allowed_missed), (2, 1, 1))

    def test_exit_zero_with_unviable_mutant_is_not_credit(self) -> None:
        """Kills naive-exit-code mutation: treat exit zero as proof all mutants were caught."""
        report = report_for("Unviable")
        with self.assertRaisesRegex(checker.MutationCheckError, "cannot count as caught"):
            checker.assess_outcomes(0, report, {}, {"oa-safety"})

    def test_caught_summary_requires_executed_failing_test(self) -> None:
        """Kills acceptance of a CaughtMutant without a failed Test phase."""
        report = report_for("CaughtMutant")
        report["outcomes"][1]["phase_results"][-1]["process_status"] = "Success"
        with self.assertRaisesRegex(checker.MutationCheckError, "executed failing test"):
            checker.assess_outcomes(0, report, {}, {"oa-safety"})

    def test_duplicate_identity_in_any_outcome_is_rejected(self) -> None:
        """Kills duplicate checks applied only to missed mutant rows."""
        report = report_for("CaughtMutant", "MissedMutant")
        report["outcomes"][2]["scenario"] = report["outcomes"][1]["scenario"]
        identity = "oa-safety::src/example.rs:1:1: replace sample -> bool"
        with self.assertRaisesRegex(checker.MutationCheckError, "duplicate mutant identity"):
            checker.assess_outcomes(2, report, {identity: "A mutant has an exact review reason."}, {"oa-safety"})

    def test_allowlist_staleness_is_limited_to_changed_file_scope(self) -> None:
        """Kills stale-entry rejection applied globally instead of to changed files."""
        allowlist = {
            "oa-safety::src/other.rs:8:2: replace old -> bool": "A separate file carries this valid exception.",
            "oa-track::src/track.rs:8:2: replace old -> bool": "Another package carries this valid exception.",
        }
        selected = checker.allowlist_in_scope(allowlist, {("oa-safety", "src/example.rs")})
        self.assertEqual(selected, {})

    def test_stale_allowlist_entry_in_changed_file_fails(self) -> None:
        """Kills stale survivor exceptions retained after the changed mutant disappears."""
        source_scope = {("oa-safety", "src/example.rs")}
        global_entries = {
            "oa-safety::src/example.rs:8:2: replace removed -> bool": "This previously missed mutant has review rationale."
        }
        scoped_entries = checker.allowlist_in_scope(global_entries, source_scope)
        with self.assertRaisesRegex(checker.MutationCheckError, "stale identities"):
            checker.assess_outcomes(0, report_for("CaughtMutant"), scoped_entries, {"oa-safety"})

    def test_missing_wrong_and_root_allowlist_paths_refuse(self) -> None:
        """Kills fail-open behavior when allowlist configuration is missing or unsafe."""
        with tempfile.TemporaryDirectory() as temp_dir:
            for path in (Path(temp_dir) / "missing.txt", Path(temp_dir), Path("/")):
                with self.subTest(path=str(path)), self.assertRaises(checker.MutationCheckError):
                    checker.read_allowlist(path)

    def test_allowlist_requires_exact_scoped_mutant_identity(self) -> None:
        with self.assertRaisesRegex(checker.MutationCheckError, "exact scoped mutant name"):
            checker.parse_allowlist_text(
                "oa-safety::src/example.rs:line:1: replace old -> bool\t"
                "Reason is long enough and specific.\n"
            )

    def test_timeout_and_unclassified_success_are_inconclusive(self) -> None:
        for summary, field in (("Timeout", "timeout"), ("Success", "success")):
            with self.subTest(summary=summary):
                with self.assertRaises(checker.MutationCheckError):
                    checker.assess_outcomes(0, report_for(summary), {}, {"oa-safety"})

    def test_zero_mutants_is_not_mutation_credit(self) -> None:
        result = checker.assess_outcomes(0, report_for(), {}, {"oa-safety"})
        self.assertEqual(result.total, 0)
        with self.assertRaisesRegex(checker.MutationCheckError, "empty mutant selections"):
            checker.require_complete_selection(set(), result)
        stale = {"oa-safety::src/example.rs:9:1: removed mutant": "This obsolete exception has rationale text."}
        with self.assertRaisesRegex(checker.MutationCheckError, "stale identities"):
            checker.assess_outcomes(0, report_for(), stale, {"oa-safety"})

    def test_selected_mutant_list_must_reconcile_with_outcomes(self) -> None:
        """Kills acceptance when selected mutants are absent from final outcomes."""
        selected = checker.parse_mutant_list(
            json.dumps(
                [
                    {
                        **report_for("CaughtMutant")["outcomes"][1]["scenario"]["Mutant"],
                        "diff": "synthetic diff",
                    }
                ]
            ),
            {"oa-safety"},
        )
        summary = checker.assess_outcomes(0, report_for("CaughtMutant"), {}, {"oa-safety"})
        checker.require_complete_selection(selected, summary)
        with self.assertRaisesRegex(checker.MutationCheckError, "complete selected mutant list"):
            checker.require_complete_selection(selected | {"oa-safety::src/missing.rs:9:1: absent"}, summary)

    def test_mutant_totals_must_reconcile_with_scenario_rows(self) -> None:
        report = report_for("CaughtMutant")
        report["caught"] = 0
        with self.assertRaisesRegex(checker.MutationCheckError, "account for every"):
            checker.assess_outcomes(0, report, {}, {"oa-safety"})

    def test_changed_scope_includes_all_eleven_safety_packages(self) -> None:
        """Kills source-scope omissions, including tracker and adapter packages."""
        paths = [f"{root}/src/lib.rs" for root in checker.SAFETY_CRATE_ROOTS]
        selected, packages = checker.changed_safety_sources(paths)
        self.assertEqual(len(selected), 11)
        self.assertEqual(set(packages), set(checker.SAFETY_CRATE_ROOTS.values()))

    def test_scope_excludes_test_and_non_safety_sources(self) -> None:
        paths = [
            "crates/oa-safety/tests/safety.rs",
            "tools/oa-regional-session/src/runtime.rs",
            "crates/oa-display/src/colors.rs",
        ]
        selected, packages = checker.changed_safety_sources(paths)
        self.assertEqual(selected, ["crates/oa-display/src/colors.rs"])
        self.assertEqual(packages, ["oa-display"])

    def test_empty_safety_diff_is_an_explicit_no_credit_skip(self) -> None:
        commit = "a" * 40

        def git_result(_repo: Path, *args: str) -> bytes:
            if args[0] == "rev-parse":
                return (commit + "\n").encode("ascii")
            if args[0] == "diff":
                return b""
            if args[0] == "show":
                return b"# no configured survivors\n"
            raise AssertionError(f"unexpected git operation: {args[0]}")

        with patch.object(checker, "git_bytes", side_effect=git_result):
            result = checker.run_check(Path("synthetic-repo"), "base", "head")
        self.assertEqual(result["status"], "skip-no-safety-source-diff")
        self.assertIs(result["pass_credit"], False)

    def test_mutant_command_uses_scratch_safe_changed_lines_and_workspace_tests(self) -> None:
        command = checker.cargo_mutants_command(["oa-safety"], Path("diff"), Path("out"))
        self.assertIn("--in-diff", command)
        self.assertEqual(command[command.index("--test-workspace") + 1], "true")
        self.assertNotIn("--in-place", command)


if __name__ == "__main__":
    unittest.main()
