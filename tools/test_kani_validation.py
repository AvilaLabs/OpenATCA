"""Fail-closed tests for the pinned Kani JSON result contract."""

import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from contextlib import redirect_stderr, redirect_stdout
from unittest import mock

from kani_validation import (
    _copy_source_artifacts,
    changed_source_paths,
    evaluate_kani_binding,
    evaluate_kani_report,
    main as kani_runner_main,
    run_limited_process,
)


HARNESS = "tracker::proofs::bounded_dispatch"
COVERS = ["row stored", "applied dispatch"]


def fixture_report():
    """Small Kani 0.68-shaped report with explicit successful cover checks."""
    checks = [
        {"id": 1, "function": HARNESS, "status": "Success", "description": "assertions", "category": "assertion"},
        {
            "id": 2,
            "function": "kani::unsupported_guard",
            "status": "Success",
            "description": "unsupported path is not reached",
            "category": "unsupported_construct",
        },
        {"id": 3, "function": HARNESS, "status": "Unreachable", "description": "dead library branch", "category": "unreachable"},
        {
            "id": 4, "function": HARNESS, "status": "Satisfied", "description": COVERS[0],
            "category": "cover", "location": {"file": "crates/oa-track/src/tracker_proofs.rs", "line": "513", "column": "9"},
        },
        {
            "id": 5, "function": HARNESS, "status": "Satisfied", "description": COVERS[1],
            "category": "cover", "location": {"file": "crates/oa-track/src/tracker_proofs.rs", "line": "514", "column": "9"},
        },
    ]
    return {
        "metadata": {"version": "1.0", "kani_version": "0.68.0", "target": "x86_64-unknown-linux-gnu", "build_mode": "release"},
        "project": {"crate_name": ["oa_track"], "workspace_root": "/tmp/repo", "output_dir": "/out"},
        "tools": {
            "kani": "0.68.0",
            "rustc": "rustc nightly for Kani",
            "cbmc": "6.11.0",
            "goto_cc": "goto-cc 6.11.0",
            "goto_instrument": "6.11.0",
            "solvers": [{"name": "cadical", "version": None}],
        },
        "harness_metadata": [{
            "pretty_name": HARNESS,
            "crate_name": "oa_track",
            "source": {"file": "crates/oa-track/src/tracker_proofs.rs", "start_line": 1, "end_line": 10},
            "attributes": {"kind": "Proof", "should_panic": False},
            "is_bounded": False,
        }],
        "error_details": [{"harness_id": HARNESS, "has_errors": False}],
        "property_details": [{
            "harness_id": HARNESS,
            "property_details": {
                "total_properties": 5,
                "passed": 2,
                "failed": 0,
                "unreachable": 1,
                "undetermined": 0,
                "solver_error": 0,
                "satisfied": 2,
                "unsatisfiable": 0,
                "covered": 0,
                "uncovered": 0,
            },
        }],
        "cbmc": [{
            "harness_id": HARNESS,
            "cbmc_metadata": {"version": "6.11.0"},
            "configuration": {"solver": "cadical"},
        }],
        "verification_results": {
            "summary": {
                "total_harnesses": 1,
                "executed": 1,
                "status": "completed",
                "successful": 1,
                "failed": 0,
                "duration_ms": 10,
            },
            "results": [{"harness_id": HARNESS, "status": "Success", "duration_ms": 10, "checks": checks}],
        },
        # Code coverage instrumentation is distinct from requested kani::cover! goals.
        "coverage": {"enabled": False},
    }


FULL_COVER_LABELS = [
    "absent monotonic timestamp",
    "invalid monotonic reference",
    "different monotonic stream",
    "monotonic regression",
    "monotonic delta exceeds i64",
    "wall delta underflow",
    "wall delta overflow",
    "clock step underflow",
    "representable timestamp split",
]


def report_with_cloned_cover_checks():
    report = fixture_report()
    result = report["verification_results"]["results"][0]
    checks = [
        {"id": i, "function": HARNESS, "status": "Success", "description": "assertion", "category": "assertion"}
        for i in range(1, 69)
    ]
    next_id = 69
    for index, label in enumerate(FULL_COVER_LABELS):
        location = {
            "file": "crates/oa-replay/src/mono_split.rs",
            "line": str(166 + index),
            "column": "9",
        }
        check = {
            "id": next_id,
            "function": HARNESS,
            "status": "Satisfied",
            "description": label,
            "category": "cover",
            "location": location,
        }
        next_id += 1
        checks.append(check)
        clone_count = 2 if index == 2 else 1 if index == 1 else 0
        for _ in range(clone_count):
            clone = dict(check)
            clone["id"] = next_id
            clone["status"] = "Unsatisfiable"
            clone["location"] = dict(location)
            next_id += 1
            checks.append(clone)
    result["checks"] = checks
    properties = report["property_details"][0]["property_details"]
    properties.update({
        "total_properties": 80,
        "passed": 68,
        "failed": 0,
        "unreachable": 0,
        "undetermined": 0,
        "solver_error": 0,
        "satisfied": 9,
        "unsatisfiable": 3,
        "covered": 0,
        "uncovered": 0,
    })
    return report


def _sha(value):
    return hashlib.sha256(value).hexdigest()


def _canonical_sha(value):
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
    return _sha(encoded)


def fixture_binding(report_text):
    source = {"crates/oa-track/src/tracker_proofs.rs": "a" * 64}
    command = ["cargo", "kani", "--lib", "-p", "oa-track", "--harness", HARNESS]
    environment = {"CARGO_NET_OFFLINE": "true", "CARGO_BUILD_JOBS": "1"}
    return {
        "result_sha256": _sha(report_text.encode()),
        "command": command,
        "command_sha256": _canonical_sha(command),
        "environment": environment,
        "environment_sha256": _canonical_sha(environment),
        "source_before": source,
        "source_after": dict(source),
        "source_sha256_before": _canonical_sha(source),
        "source_sha256_after": _canonical_sha(source),
        "source_unchanged": True,
        "exit_code": 0,
        "kani_version": "0.68.0",
        "package": "oa-track",
        "harness": HARNESS,
        "repo": "/tmp/repo",
    }


class KaniReportValidationTests(unittest.TestCase):
    def test_satisfied_named_covers_and_successful_unsupported_guards_pass(self):
        report = fixture_report()
        self.assertEqual(
            evaluate_kani_report(json.dumps(report), [HARNESS], {HARNESS: COVERS}),
            [],
        )

    def test_zero_matched_harnesses_fail(self):
        report = fixture_report()
        report["harness_metadata"] = []
        report["error_details"] = []
        report["property_details"] = []
        report["cbmc"] = []
        report["verification_results"] = {
            "summary": {"total_harnesses": 0, "executed": 0, "status": "completed", "successful": 0, "failed": 0, "duration_ms": 0},
            "results": [],
        }
        errors = evaluate_kani_report(report, [HARNESS])
        self.assertTrue(any("expected nonzero" in e or "harness set differs" in e for e in errors))

    def test_wrong_harness_name_fails_even_when_counts_succeed(self):
        report = fixture_report()
        wrong = "tracker::proofs::different"
        report["harness_metadata"][0]["pretty_name"] = wrong
        for name in ("error_details", "property_details", "cbmc"):
            report[name][0]["harness_id"] = wrong
        report["verification_results"]["results"][0]["harness_id"] = wrong
        self.assertTrue(evaluate_kani_report(report, [HARNESS]))

    def test_solver_incomplete_and_unwind_failure_are_rejected(self):
        report = fixture_report()
        result = report["verification_results"]["results"][0]
        result["checks"].append({"id": 6, "function": HARNESS, "status": "Failure", "description": "unwinding assertion failed", "category": "unwinding_assertion"})
        result["checks"].append({"id": 7, "function": HARNESS, "status": "Undetermined", "description": "solver did not complete", "category": "assertion"})
        result["status"] = "Undetermined"
        report["verification_results"]["summary"].update({"status": "incomplete", "successful": 0, "failed": 1})
        prop = report["property_details"][0]["property_details"]
        prop.update({"total_properties": 7, "failed": 1, "undetermined": 1})
        errors = evaluate_kani_report(report, [HARNESS], {HARNESS: COVERS})
        self.assertTrue(any("unwinding check did not succeed" in e for e in errors))
        self.assertTrue(any("undetermined" in e or "status" in e for e in errors))

    def test_missing_solver_identity_fails_closed(self):
        report = fixture_report()
        report["cbmc"][0]["configuration"].pop("solver")
        errors = evaluate_kani_report(report, [HARNESS])
        self.assertTrue(any("CBMC solver is missing" in e for e in errors))

    def test_reachable_unsupported_guard_is_rejected(self):
        report = fixture_report()
        report["verification_results"]["results"][0]["checks"][1]["status"] = "Failure"
        report["verification_results"]["summary"].update({"successful": 0, "failed": 1})
        prop = report["property_details"][0]["property_details"]
        prop.update({"failed": 1, "total_properties": 6})
        errors = evaluate_kani_report(report, [HARNESS])
        self.assertTrue(any("unsupported construct was not proved unreachable" in e for e in errors))

    def test_missing_requested_cover_fails(self):
        report = fixture_report()
        report["verification_results"]["results"][0]["checks"] = [
            c for c in report["verification_results"]["results"][0]["checks"]
            if c.get("description") != COVERS[1]
        ]
        report["property_details"][0]["property_details"].update({"total_properties": 4, "satisfied": 1})
        errors = evaluate_kani_report(report, [HARNESS], {HARNESS: COVERS})
        self.assertTrue(any("requested cover is missing: 'applied dispatch'" in e for e in errors))

    def test_unreached_cover_fails(self):
        report = fixture_report()
        report["verification_results"]["results"][0]["checks"][4]["status"] = "Unreachable"
        report["property_details"][0]["property_details"].update({
            "total_properties": 5,
            "unreachable": 2,
            "satisfied": 1,
        })
        errors = evaluate_kani_report(report, [HARNESS], {HARNESS: COVERS})
        self.assertTrue(any("cover check is not satisfied" in e for e in errors))

    def test_unknown_check_status_and_kani_error_fail(self):
        report = fixture_report()
        report["verification_results"]["results"][0]["checks"][0]["status"] = "Unknown"
        report["error_details"][0]["has_errors"] = True
        errors = evaluate_kani_report(report, [HARNESS])
        self.assertTrue(any("unknown or unsuccessful check status" in e for e in errors))
        self.assertTrue(any("Kani error_details reports errors" in e for e in errors))

    def test_same_source_cover_clones_accept_one_satisfied_goal_and_unsatisfiable_duplicates(self):
        report = report_with_cloned_cover_checks()
        self.assertEqual(evaluate_kani_report(report, [HARNESS], {HARNESS: FULL_COVER_LABELS}), [])

    def test_identically_named_cover_at_a_distinct_source_site_is_rejected(self):
        report = report_with_cloned_cover_checks()
        checks = report["verification_results"]["results"][0]["checks"]
        extra = dict(next(c for c in checks if c.get("description") == FULL_COVER_LABELS[1]))
        extra["id"] = 100
        extra["location"] = dict(extra["location"], line="220")
        checks.append(extra)
        properties = report["property_details"][0]["property_details"]
        properties["total_properties"] += 1
        properties["satisfied"] += 1
        errors = evaluate_kani_report(report, [HARNESS], {HARNESS: FULL_COVER_LABELS})
        self.assertTrue(any("appears at multiple source sites" in error for error in errors))

    def test_all_unsatisfiable_cover_clone_group_is_rejected(self):
        report = report_with_cloned_cover_checks()
        checks = report["verification_results"]["results"][0]["checks"]
        first = FULL_COVER_LABELS[0]
        for check in checks:
            if check.get("category") == "cover" and check.get("description") == first:
                check["status"] = "Unsatisfiable"
        properties = report["property_details"][0]["property_details"]
        properties["satisfied"] -= 1
        properties["unsatisfiable"] += 1
        errors = evaluate_kani_report(report, [HARNESS], {HARNESS: FULL_COVER_LABELS})
        self.assertTrue(any("was never satisfied" in error for error in errors))

    def test_unknown_cover_and_duplicate_cover_id_are_rejected(self):
        report = report_with_cloned_cover_checks()
        checks = report["verification_results"]["results"][0]["checks"]
        cover_checks = [c for c in checks if c.get("category") == "cover"]
        cover_checks[0]["status"] = "Unknown"
        cover_checks[1]["id"] = cover_checks[0]["id"]
        errors = evaluate_kani_report(report, [HARNESS], {HARNESS: FULL_COVER_LABELS})
        self.assertTrue(any("unknown or unsuccessful check status" in error for error in errors))
        self.assertTrue(any("cover check id" in error and "duplicated" in error for error in errors))

    def test_cover_without_exact_source_location_is_rejected(self):
        report = fixture_report()
        report["verification_results"]["results"][0]["checks"][3].pop("location")
        errors = evaluate_kani_report(report, [HARNESS], {HARNESS: COVERS})
        self.assertTrue(any("missing its source location" in error for error in errors))

    def test_unsatisfiable_status_on_non_cover_check_is_rejected(self):
        report = report_with_cloned_cover_checks()
        report["verification_results"]["results"][0]["checks"][0]["status"] = "Unsatisfiable"
        errors = evaluate_kani_report(report, [HARNESS])
        self.assertTrue(any("only valid for a cover clone" in error for error in errors))

    def test_check_ids_are_unique_across_cover_and_noncover_checks(self):
        report = fixture_report()
        checks = report["verification_results"]["results"][0]["checks"]
        checks[1]["id"] = checks[0]["id"]
        checks[3]["id"] = checks[0]["id"]
        errors = evaluate_kani_report(report, [HARNESS], {HARNESS: COVERS})
        self.assertTrue(any("check id 1 is duplicated" in error for error in errors))

    def test_check_ids_must_be_nonnegative_integers(self):
        report = fixture_report()
        report["verification_results"]["results"][0]["checks"][0]["id"] = -1
        self.assertTrue(any(
            "check id must be a nonnegative integer" in error
            for error in evaluate_kani_report(report, [HARNESS])
        ))

    def test_truncated_check_list_fails_against_full_property_counters(self):
        report = report_with_cloned_cover_checks()
        checks = report["verification_results"]["results"][0]["checks"]
        del checks[0]
        errors = evaluate_kani_report(report, [HARNESS], {HARNESS: FULL_COVER_LABELS})
        self.assertTrue(any("property counter passed is 68, but check list reports 67" in error for error in errors))

    def test_tampered_property_counter_fails_against_check_statuses(self):
        report = report_with_cloned_cover_checks()
        report["property_details"][0]["property_details"]["passed"] += 1
        errors = evaluate_kani_report(report, [HARNESS], {HARNESS: FULL_COVER_LABELS})
        self.assertTrue(any("property counter passed is 69, but check list reports 68" in error for error in errors))

    def test_partial_json_fails_closed(self):
        self.assertTrue(evaluate_kani_report('{"verification_results":', [HARNESS]))


class KaniBindingTests(unittest.TestCase):
    def test_result_source_and_command_hashes_bind(self):
        text = json.dumps(fixture_report(), separators=(",", ":"))
        record = fixture_binding(text)
        self.assertEqual(evaluate_kani_binding(text, record), [])

    def test_changed_source_and_stale_source_snapshot_fail(self):
        text = json.dumps(fixture_report(), separators=(",", ":"))
        record = fixture_binding(text)
        record["source_after"]["crates/oa-track/src/tracker_proofs.rs"] = "b" * 64
        record["source_sha256_after"] = _canonical_sha(record["source_after"])
        record["source_unchanged"] = False
        errors = evaluate_kani_binding(text, record)
        self.assertTrue(any("source snapshots differ" in e for e in errors))
        self.assertTrue(any("source_unchanged" in e for e in errors))

    def test_json_or_command_hash_tampering_fails(self):
        text = json.dumps(fixture_report(), separators=(",", ":"))
        record = fixture_binding(text)
        self.assertTrue(evaluate_kani_binding(text + " ", record))
        record = fixture_binding(text)
        record["command"].append("--unwind")
        self.assertTrue(evaluate_kani_binding(text, record))

    def test_json_crate_workspace_and_harness_identity_are_bound(self):
        report = fixture_report()
        text = json.dumps(report, separators=(",", ":"))
        record = fixture_binding(text)
        self.assertEqual(evaluate_kani_binding(text, record), [])

        report["project"]["crate_name"] = ["oa_replay"]
        text_wrong_package = json.dumps(report, separators=(",", ":"))
        record_wrong_package = fixture_binding(text_wrong_package)
        self.assertTrue(any("crate_name" in e for e in evaluate_kani_binding(text_wrong_package, record_wrong_package)))

        report = fixture_report()
        report["project"]["workspace_root"] = "/tmp/other-checkout"
        text_wrong_root = json.dumps(report, separators=(",", ":"))
        self.assertTrue(any("workspace_root" in e for e in evaluate_kani_binding(text_wrong_root, fixture_binding(text_wrong_root))))


class RunnerIsolationTests(unittest.TestCase):
    def test_changed_source_archive_includes_untracked_proof_modules_and_excludes_fixtures(self):
        with tempfile.TemporaryDirectory() as temp:
            repo = Path(temp) / "repo"
            repo.mkdir()
            subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
            subprocess.run(["git", "config", "user.email", "proof@example.invalid"], cwd=repo, check=True)
            subprocess.run(["git", "config", "user.name", "Proof Fixture"], cwd=repo, check=True)
            tracked = repo / "crates" / "oa-track" / "src" / "lib.rs"
            tracked.parent.mkdir(parents=True)
            tracked.write_text("pub fn base() {}\n")
            subprocess.run(["git", "add", "."], cwd=repo, check=True)
            subprocess.run(["git", "commit", "-qm", "base"], cwd=repo, check=True)
            tracked.write_text("pub fn changed() {}\n")
            added = repo / "crates" / "oa-replay" / "src"
            added.mkdir(parents=True)
            (added / "mono_split.rs").write_text("pub fn split_proof() {}\n")
            (added / "fullmode.rs").write_text("pub fn fullmode_proof() {}\n")
            fixtures = repo / "tests" / "fixtures"
            fixtures.mkdir(parents=True)
            (fixtures / "large-generated.json").write_text('{"fixture":true}\n')

            changed, untracked = changed_source_paths(repo)
            self.assertIn("crates/oa-track/src/lib.rs", changed)
            self.assertIn("crates/oa-replay/src/mono_split.rs", untracked)
            self.assertIn("crates/oa-replay/src/fullmode.rs", untracked)
            self.assertNotIn("tests/fixtures/large-generated.json", untracked)

            out = Path(temp) / "out"
            out.mkdir()
            copies = _copy_source_artifacts(repo, out, changed + untracked)
            self.assertEqual(set(copies), set(changed + untracked))
            self.assertEqual(
                (out / "source-files/crates/oa-replay/src/mono_split.rs").read_text(),
                "pub fn split_proof() {}\n",
            )

    def test_timeout_terminates_descendant_process_group(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            marker = root / "late-marker"
            child_code = (
                "import time,pathlib; time.sleep(1); "
                f"pathlib.Path({str(marker)!r}).write_text('survived')"
            )
            parent_code = (
                "import subprocess,sys,time; "
                f"subprocess.Popen([sys.executable,'-c',{child_code!r}]); "
                "time.sleep(30)"
            )
            with (root / "stdout").open("w") as stdout, (root / "stderr").open("w") as stderr:
                rc, timed_out = run_limited_process(
                    [sys.executable, "-c", parent_code], root, os.environ,
                    stdout, stderr, 0.2, 512 * 1024 * 1024, grace_seconds=0.2,
                )
            self.assertEqual(rc, 124)
            self.assertTrue(timed_out)
            time.sleep(1.1)
            self.assertFalse(marker.exists(), "a descendant survived the runner timeout")


class RunnerCliBindingTests(unittest.TestCase):
    def _fake_toolchain(self, root, kani_output):
        bin_dir = root / "bin"
        bin_dir.mkdir()
        repo = root / "repo"
        repo.mkdir()
        (repo / "Cargo.toml").write_text("[workspace]\nmembers = []\n")
        source = repo / "src" / "harness.rs"
        source.parent.mkdir()
        source.write_text("// fake Kani harness source\n")
        subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
        subprocess.run(["git", "config", "user.email", "kani@example.invalid"], cwd=repo, check=True)
        subprocess.run(["git", "config", "user.name", "Kani Fixture"], cwd=repo, check=True)
        subprocess.run(["git", "add", "Cargo.toml", "src/harness.rs"], cwd=repo, check=True)
        subprocess.run(["git", "commit", "-qm", "fixture"], cwd=repo, check=True)

        def executable(name, body):
            path = bin_dir / name
            path.write_text("#!/usr/bin/env python3\n" + body)
            path.chmod(0o755)

        executable("kani", f"import sys\nsys.stdout.write({kani_output!r})\n")
        executable(
            "rustup",
            "print('1.89.0-x86_64-unknown-linux-gnu (default)')\n",
        )
        executable(
            "rustc",
            "import sys\nprint('rustc 1.89.0 (fake product compiler)' if '+1.89.0' in sys.argv else 'rustc 1.98.0 (fake host compiler)')\n",
        )
        report_path = root / "kani-result-fixture.json"
        report = fixture_report()
        report["project"]["workspace_root"] = str(repo)
        report["harness_metadata"][0]["source"]["file"] = "src/harness.rs"
        report_path.write_text(json.dumps(report))
        executable(
            "cargo",
            "import json,os,pathlib,sys\n"
            "args=sys.argv[1:]\n"
            "destination=pathlib.Path(args[args.index('--export-json')+1])\n"
            "destination.write_text(pathlib.Path(os.environ['FAKE_KANI_RESULT']).read_text())\n",
        )
        return bin_dir, repo, report_path

    def _invoke_main(self, root, kani_output):
        bin_dir, repo, report_path = self._fake_toolchain(root, kani_output)
        out = root / "run-evidence"
        cache = root / "cache"
        old_path = os.environ.get("PATH", "")
        env = {
            "PATH": str(bin_dir) + os.pathsep + old_path,
            "FAKE_KANI_RESULT": str(report_path),
        }
        stdout, stderr = io.StringIO(), io.StringIO()
        with mock.patch.dict(os.environ, env), redirect_stdout(stdout), redirect_stderr(stderr):
            try:
                rc = kani_runner_main([
                    "--repo", str(repo),
                    "--package", "oa-track",
                    "--harness", HARNESS,
                    "--out", str(out),
                    "--target-dir", str(cache),
                    "--harness-timeout", "60",
                    "--timeout", "120",
                    "--memory-mib", "512",
                ])
            except SystemExit as exc:
                rc = exc.code
        return rc, out, stderr.getvalue()

    def test_multiline_pinned_kani_version_passes_full_cli_binding(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            version = "Kani Rust Verifier 0.68.0 (standalone)\nCommit: pinned-test-build\n"
            rc, out, stderr = self._invoke_main(root, version)
            self.assertEqual(rc, 0, stderr)
            run_record = json.loads((out / "run.json").read_text())
            self.assertEqual(run_record["status"], "PASS")
            self.assertEqual(run_record["kani_version"], "0.68.0")
            self.assertEqual(run_record["kani_version_output"], version.strip())
            check = run_record["checks"][0]
            self.assertEqual(check["evaluation_errors"], [])
            self.assertTrue(check["source_unchanged"])
            self.assertEqual(check["package"], "oa-track")
            self.assertEqual(check["harness"], HARNESS)
            self.assertIn("src/harness.rs", check["harness_source_copies"])

    def test_wrong_kani_cli_version_is_rejected_before_proof_command(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            rc, out, stderr = self._invoke_main(
                root, "Kani Rust Verifier 0.69.0 (standalone)\nCommit: other-build\n"
            )
            self.assertEqual(rc, 2)
            self.assertFalse(out.exists())
            self.assertIn("expected Kani 0.68.0", stderr)


if __name__ == "__main__":
    unittest.main()
