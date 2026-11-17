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
    def test_checked_in_registry_has_twenty_five_unique_configured_harnesses(self):
        registry, errors = load_registry(REGISTRY_PATH)
        self.assertEqual(errors, [])
        self.assertIsNotNone(registry)
        self.assertEqual(registry["expected_proof_count"], 25)
        self.assertEqual(len({p["id"] for p in registry["proofs"]}), 25)
        self.assertEqual(
            sum(p["enabled"] for p in registry["proofs"]),
            sum(not p["experimental"] for p in registry["proofs"]),
        )
        self.assertEqual(sum(p["enabled"] for p in registry["proofs"]), 22)
        experimental = {p["id"] for p in registry["proofs"] if p["experimental"]}
        self.assertEqual(experimental, {
            "tracker-numerical-refusal-rollback",
            "tracker-late-replay-rollback",
            "full-sensor-update",
        })

    def test_minimal_valid_matrix_is_accepted(self):
        self.assertEqual(validate_registry(registry_with(proof_entry())), [])

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
record = {
    "status": "PASS",
    "package": package,
    "expected_harnesses": [harness],
    "required_covers": {harness: covers},
    "checks": [{"harness": harness, "status": "PASS", "source_unchanged": True}],
}
mode = os.environ.get("FAKE_RUN_RECORD_MODE", "pass")
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

    def invoke(self, root, mode="pass", proof_count=2):
        repo, registry, out, target, mode = self.fake_workspace(root, mode, proof_count)
        stdout, stderr = io.StringIO(), io.StringIO()
        with mock.patch.dict(os.environ, {"FAKE_RUN_RECORD_MODE": mode}), redirect_stdout(stdout), redirect_stderr(stderr):
            result = run_suite(repo, out, target, registry)
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


if __name__ == "__main__":
    unittest.main()
