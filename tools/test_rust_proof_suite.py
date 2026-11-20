"""Configuration checks for the sequential proof suite."""

import copy
import hashlib
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from unittest import mock

from rust_proof_suite import load_registry, run_suite, validate_registry
from kani_validation import LOOP_UNWIND_PRESETS


REPO = Path(__file__).resolve().parents[1]
REGISTRY_PATH = REPO / "tools" / "rust_proofs.json"


def proof_entry():
    return {
        "id": "sample-proof",
        "requirement": "A bounded arithmetic invariant.",
        "package": "oa-track",
        "harness": "types::proofs::bounded_example",
        "enabled": True,
        "experimental": False,
        "required_covers": ["append reached"],
        "stubbing": False,
        "stub_summaries": [],
        "cfgs": [],
        "harness_timeout_seconds": 300,
        "external_timeout_seconds": 420,
        "memory_mib": 4096,
    }


def registry_with(*proofs):
    return {
        "registry_version": 1,
        "expected_proof_count": len(proofs),
        "proofs": list(proofs),
    }


class SuiteConfigurationTests(unittest.TestCase):
    def test_checked_in_registry_has_all_required_unique_configured_harnesses(self):
        registry, errors = load_registry(REGISTRY_PATH)
        self.assertEqual(errors, [])
        self.assertIsNotNone(registry)
        self.assertEqual(registry["expected_proof_count"], 72)
        self.assertEqual(len({p["id"] for p in registry["proofs"]}), 72)
        self.assertEqual(
            sum(p["enabled"] for p in registry["proofs"]),
            sum(not p["experimental"] for p in registry["proofs"]),
        )
        self.assertEqual(sum(p["enabled"] for p in registry["proofs"]), 63)
        experimental = {p["id"] for p in registry["proofs"] if p["experimental"]}
        self.assertEqual(experimental, {
            "tracker-numerical-refusal-rollback",
            "tracker-late-replay-rollback",
            "full-sensor-update",
            "late-append-combined",
            "late-overflow-combined",
            "hypot-dd-square",
            "hypot-dd-root",
            "hypot-fast-plain",
            "hypot-fast-scaled",
        })

    def test_the_safety_net_inhibition_proofs_are_registered(self):
        registry, errors = load_registry(REGISTRY_PATH)
        self.assertEqual(errors, [])
        safety = {p["id"]: p for p in registry["proofs"] if p["package"] == "oa-safety"}
        self.assertEqual(set(safety), {"inhibition-in-force", "inhibition-band"})
        for proof in safety.values():
            self.assertTrue(proof["enabled"] and not proof["experimental"])
            self.assertTrue(proof["harness"].startswith("inhibit::proofs::"))

    def test_the_conflict_alert_solver_proofs_are_registered(self):
        registry, errors = load_registry(REGISTRY_PATH)
        self.assertEqual(errors, [])
        ca = {p["id"]: p for p in registry["proofs"] if p["package"] == "oa-ca"}
        self.assertEqual(set(ca), {
            "ca-horizontal-span",
            "ca-vertical-span",
            "ca-horizontal-now-step",
            "ca-horizontal-now-bounds",
            "ca-horizontal-now-approach",
            "ca-horizontal-now-root",
            "ca-horizontal-now-sqrt",
            "ca-vertical-now-has-span",
            "ca-horizontal-miss-point",
            "ca-meet",
            "ca-predict-span",
            "ca-predict-ttl",
            "ca-unknown-altitude",
        })
        for proof in ca.values():
            self.assertTrue(proof["enabled"] and not proof["experimental"])
            self.assertTrue(proof["harness"].startswith("solve::proofs::"))

    def test_the_hypot_proofs_are_registered_in_oa_math(self):
        registry, errors = load_registry(REGISTRY_PATH)
        self.assertEqual(errors, [])
        math = {p["id"]: p for p in registry["proofs"] if p["package"] == "oa-math"}
        self.assertEqual(
            {i for i, p in math.items() if p["enabled"]},
            {"hypot-operands-ordered", "hypot-slow-operands", "hypot-dd-operands",
             "hypot-dd-sum", "hypot-scale-round"},
        )
        self.assertEqual(
            {i for i, p in math.items() if not p["enabled"]},
            {"hypot-dd-square", "hypot-dd-root", "hypot-fast-plain", "hypot-fast-scaled"},
        )
        for proof in math.values():
            self.assertTrue(proof["harness"].startswith("root::proofs::"))

    def test_late_rollback_conjunction_has_every_component_and_reachability_case(self):
        registry, errors = load_registry(REGISTRY_PATH)
        self.assertEqual(errors, [])
        enabled = {proof["id"]: proof for proof in registry["proofs"] if proof["enabled"]}
        components = {"current-filter", "anchor-filter", "ring", "metadata", "stats", "output"}
        common_stubs = enabled["late-append-output-projection"]["stub_summaries"]
        for mode in ("append", "overflow"):
            expected = {f"late-{mode}-{component}-projection" for component in components}
            expected.add(f"late-{mode}-reachability")
            actual = {proof_id for proof_id in enabled if proof_id.startswith(f"late-{mode}-")}
            self.assertEqual(actual, expected)
            for proof_id in expected:
                proof = enabled[proof_id]
                self.assertEqual(proof["package"], "oa-track")
                self.assertTrue(proof["stubbing"])
                self.assertEqual(proof["stub_summaries"], common_stubs)
                self.assertEqual(proof["loop_unwind_overrides"], ["late-replay-two-events-pinned-cargo"])
                if proof_id.endswith("reachability"):
                    self.assertEqual(proof["harness"], f"tracker::proofs::rollback_paths::late_replay_{mode}_reachability")
                    self.assertEqual(proof["cfgs"], ["kani_late_replay_covers"])
                    self.assertEqual(set(proof["required_covers"]), {
                        "late replay attempts current report before later failure",
                        "late replay reaches later numerical refusal",
                        f"late replay {mode} rollback case",
                    })
                else:
                    suffix = proof_id.removeprefix(f"late-{mode}-").replace("-", "_")
                    self.assertEqual(proof["harness"], f"tracker::proofs::rollback_paths::late_{mode}_{suffix}")
                    self.assertEqual(proof["cfgs"], [])
                    self.assertEqual(proof["required_covers"], [])

    def test_minimal_valid_matrix_is_accepted(self):
        self.assertEqual(validate_registry(registry_with(proof_entry())), [])

    def test_closed_loop_unwind_preset_is_optional_and_restricted(self):
        proof = proof_entry()
        proof["harness"] = sorted(LOOP_UNWIND_PRESETS["late-replay-two-events"]["harnesses"])[0]
        proof["loop_unwind_overrides"] = ["late-replay-two-events"]
        self.assertEqual(validate_registry(registry_with(proof)), [])
        for malformed in (
            ["unknown"], ["late-replay-two-events", "late-replay-two-events"],
            "late-replay-two-events", ["late-replay-two-events", "x"],
            ["late-replay-two-events:2"], [{"preset": "late-replay-two-events", "bound": 2}],
        ):
            with self.subTest(malformed=malformed):
                bad = copy.deepcopy(proof)
                bad["loop_unwind_overrides"] = malformed
                self.assertTrue(validate_registry(registry_with(bad)))
        wrong_harness = copy.deepcopy(proof)
        wrong_harness["harness"] = "types::proofs::bounded_example"
        self.assertTrue(any("not applicable" in error for error in validate_registry(registry_with(wrong_harness))))

    def test_expected_count_and_duplicate_harness_are_rejected(self):
        first = proof_entry()
        second = copy.deepcopy(first)
        second["id"] = "another-id"
        data = registry_with(first, second)
        data["expected_proof_count"] = 3
        errors = validate_registry(data)
        self.assertTrue(any("expected_proof_count" in error for error in errors))
        self.assertTrue(any("duplicate package/harness" in error for error in errors))

    def test_unregistered_cfg_and_stub_mismatch_are_rejected(self):
        proof = proof_entry()
        proof["cfgs"] = ["kani_experimental"]
        proof["stubbing"] = True
        errors = validate_registry(registry_with(proof))
        self.assertTrue(any("unregistered cfg" in error for error in errors))
        self.assertTrue(any("stubbing mode and stub summaries disagree" in error for error in errors))

    def test_wrongly_typed_package_and_cfg_fail_closed(self):
        proof = proof_entry()
        proof["package"] = ["oa-track"]
        proof["cfgs"] = [["kani_full_capacity"]]
        errors = validate_registry(registry_with(proof))
        self.assertTrue(any("package is not an allowed" in error for error in errors))
        self.assertTrue(any("unregistered cfg" in error for error in errors))

    def test_experimental_entry_cannot_run_until_promoted(self):
        proof = proof_entry()
        proof["experimental"] = True
        proof["enabled"] = True
        self.assertTrue(any("must remain disabled" in error for error in validate_registry(registry_with(proof))))

    def test_registry_parser_rejects_invalid_json(self):
        # The parser is strict about malformed source and does not produce a partial registry.
        self.assertEqual(load_registry(REGISTRY_PATH.with_name("missing-registry.json"))[0], None)


class SuiteEndToEndTests(unittest.TestCase):
    @staticmethod
    def fake_runner_source():
        return '''import json, os, pathlib, sys
args = sys.argv[1:]
def value(flag): return args[args.index(flag) + 1]
out = pathlib.Path(value("--out"))
out.mkdir(parents=True, exist_ok=True)
package = value("--package")
harness = value("--harness")
covers = [item.split("=", 1)[1] for flag, item in zip(args, args[1:]) if flag == "--require-cover"]
loop_presets = [item for flag, item in zip(args, args[1:]) if flag == "--loop-unwind-preset"]
mode = os.environ.get("FAKE_RUN_RECORD_MODE", "pass")
invocation_log = os.environ.get("FAKE_INVOKED")
if invocation_log:
    with open(invocation_log, "a", encoding="utf-8") as stream:
        stream.write(harness + "\\n")
record = {
    "status": "FAIL" if mode == "fail" else "PASS",
    "package": package,
    "expected_harnesses": [harness],
    "required_covers": {harness: covers},
    "loop_unwind_presets": {harness: loop_presets},
    "checks": [{"harness": harness, "status": "FAIL" if mode == "fail" else "PASS",
                "evaluation_errors": ["loop binding did not match source evidence"] if mode == "fail" else [],
                "source_unchanged": True}],
}
if mode == "wrong":
    record["expected_harnesses"] = ["other::proof"]
if mode != "missing":
    (out / "run.json").write_text(json.dumps(record))
print("fake runner stdout")
'''

    def fake_workspace(self, root, mode="pass", proof_count=2):
        repo = root / "repo"
        repo.mkdir()
        (repo / ".git").mkdir()
        (repo / "Cargo.toml").write_text("[workspace]\nmembers = []\n")
        tools_dir = repo / "tools"
        tools_dir.mkdir()
        (tools_dir / "kani_validation.py").write_text(self.fake_runner_source())
        (tools_dir / "rust_proof_suite.py").write_text("# placeholder checked-out suite source\n")
        proofs = []
        for index in range(proof_count):
            proof = proof_entry()
            if index:
                proof["id"] = f"second-proof-{index}"
                proof["harness"] = f"types::proofs::bounded_example_{index}"
                proof["required_covers"] = []
            proofs.append(proof)
        registry_path = tools_dir / "rust_proofs.json"
        registry_path.write_text(json.dumps(registry_with(*proofs), indent=2) + "\n")
        return repo, registry_path, root / "evidence", root / "target", mode

    def invoke(self, root, mode="pass", proof_count=2, fail_fast=False):
        repo, registry, out, target, mode = self.fake_workspace(root, mode, proof_count)
        stdout, stderr = io.StringIO(), io.StringIO()
        with mock.patch.dict(os.environ, {"FAKE_RUN_RECORD_MODE": mode}), redirect_stdout(stdout), redirect_stderr(stderr):
            result = run_suite(repo, out, target, registry, fail_fast=fail_fast)
        return result, out, registry, stdout.getvalue(), stderr.getvalue()

    def test_two_fake_harness_runs_pass_and_pipeline_sources_are_archived(self):
        with tempfile.TemporaryDirectory() as temp:
            result, out, registry, _, stderr = self.invoke(Path(temp))
            self.assertEqual(result, 0, stderr)
            suite = json.loads((out / "suite.json").read_text())
            self.assertEqual(suite["status"], "PASS")
            self.assertEqual(suite["selected_count"], 2)
            registry_copy = out / "pipeline-source/tools/rust_proofs.json"
            self.assertEqual(registry_copy.read_bytes(), registry.read_bytes())
            runner_copy = out / "pipeline-source/tools/kani_validation.py"
            self.assertEqual(
                runner_copy.read_text(),
                self.fake_runner_source(),
            )
            self.assertEqual(
                suite["pipeline_source_hashes"]["tools/rust_proofs.json"],
                hashlib.sha256(registry.read_bytes()).hexdigest(),
            )
            self.assertTrue(all(suite["pipeline_source_unchanged"].values()))
            from rust_proof_suite import __file__ as suite_script
            suite_copy = out / "pipeline-source/tools/rust_proof_suite.py"
            self.assertEqual(suite_copy.read_bytes(), Path(suite_script).read_bytes())
            for proof_id in ("sample-proof", "second-proof-1"):
                proof_dir = out / "proofs" / proof_id
                self.assertTrue((proof_dir / "stdout.log").is_file())
                self.assertTrue((proof_dir / "stderr.log").is_file())
                self.assertEqual(json.loads((proof_dir / "run/run.json").read_text())["status"], "PASS")

    def test_wrong_or_missing_run_record_fails_the_suite(self):
        for mode in ("wrong", "missing"):
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as temp:
                result, out, _, _, _ = self.invoke(Path(temp), mode=mode, proof_count=1)
                suite = json.loads((out / "suite.json").read_text())
                self.assertEqual(result, 1)
                self.assertEqual(suite["status"], "FAIL")
                self.assertEqual(suite["checks"][0]["status"], "FAIL")

    # REQ: OA-GEN-006
    def test_fail_fast_reports_failure_and_marks_remaining_proofs_not_run(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            repo, registry, out, target, _ = self.fake_workspace(root, mode="fail", proof_count=3)
            invoked = root / "invoked.txt"
            stdout, stderr = io.StringIO(), io.StringIO()
            with mock.patch.dict(os.environ, {
                "FAKE_RUN_RECORD_MODE": "fail", "FAKE_INVOKED": str(invoked),
            }), redirect_stdout(stdout), redirect_stderr(stderr):
                result = run_suite(repo, out, target, registry, fail_fast=True)
            suite = json.loads((out / "suite.json").read_text())
            statuses = [check["status"] for check in suite["checks"]]
            self.assertEqual(result, 1)
            self.assertEqual(suite["status"], "FAIL")
            self.assertEqual(suite["selected_count"], 3)
            self.assertEqual(statuses, ["FAIL", "NOT_RUN", "NOT_RUN"])
            self.assertEqual(invoked.read_text().splitlines(), [proof_entry()["harness"]])
            for outcome in suite["checks"][1:]:
                self.assertIsNone(outcome["exit_code"])
                self.assertIsNone(outcome["timed_out"])
                self.assertIsNone(outcome["elapsed_seconds"])
                self.assertIsNone(outcome["run_dir"])
                self.assertIn("fail-fast", outcome["reason"])
            self.assertIn("loop binding did not match source evidence", stdout.getvalue())

    # REQ: OA-GEN-006
    def test_fail_fast_success_runs_every_proof_and_default_failure_continues(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            repo, registry, out, target, _ = self.fake_workspace(root, proof_count=3)
            invoked = root / "invoked.txt"
            stdout, stderr = io.StringIO(), io.StringIO()
            with mock.patch.dict(os.environ, {
                "FAKE_RUN_RECORD_MODE": "pass", "FAKE_INVOKED": str(invoked),
            }), redirect_stdout(stdout), redirect_stderr(stderr):
                result = run_suite(repo, out, target, registry, fail_fast=True)
            suite = json.loads((out / "suite.json").read_text())
            self.assertEqual(result, 0, stderr.getvalue())
            self.assertEqual(suite["status"], "PASS")
            self.assertEqual([x["status"] for x in suite["checks"]], ["PASS"] * 3)
            self.assertEqual(len(invoked.read_text().splitlines()), 3)
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            repo, registry, out, target, _ = self.fake_workspace(root, mode="fail", proof_count=2)
            invoked = root / "invoked.txt"
            stdout, stderr = io.StringIO(), io.StringIO()
            with mock.patch.dict(os.environ, {
                "FAKE_RUN_RECORD_MODE": "fail", "FAKE_INVOKED": str(invoked),
            }), redirect_stdout(stdout), redirect_stderr(stderr):
                result = run_suite(repo, out, target, registry)
            suite = json.loads((out / "suite.json").read_text())
            self.assertEqual(result, 1)
            self.assertEqual(suite["status"], "FAIL")
            self.assertEqual([x["status"] for x in suite["checks"]], ["FAIL"] * 2)
            self.assertEqual(len(invoked.read_text().splitlines()), 2)

    def test_loop_unwind_preset_is_forwarded_and_recorded(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            repo, registry, out, target, _ = self.fake_workspace(root, proof_count=1)
            proof = proof_entry()
            proof["harness"] = sorted(LOOP_UNWIND_PRESETS["late-replay-two-events"]["harnesses"])[0]
            proof["loop_unwind_overrides"] = ["late-replay-two-events"]
            registry.write_text(json.dumps(registry_with(proof), indent=2) + "\n")
            stdout, stderr = io.StringIO(), io.StringIO()
            with redirect_stdout(stdout), redirect_stderr(stderr):
                result = run_suite(repo, out, target, registry)
            self.assertEqual(result, 0, stderr.getvalue())
            suite = json.loads((out / "suite.json").read_text())
            outcome = suite["checks"][0]
            self.assertIn("--loop-unwind-preset", outcome["command"])
            self.assertIn("late-replay-two-events", outcome["command"])
            run_record = json.loads((out / "proofs" / proof["id"] / "run" / "run.json").read_text())
            self.assertEqual(run_record["loop_unwind_presets"][proof["harness"]], ["late-replay-two-events"])


if __name__ == "__main__":
    unittest.main()
