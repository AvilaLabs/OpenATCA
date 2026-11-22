"""Configuration checks for the sequential proof suite."""

import copy
import hashlib
import io
import json
import os
from pathlib import Path
import tempfile
import re
import subprocess
import unittest
from contextlib import redirect_stderr, redirect_stdout
from unittest import mock

from rust_proof_suite import (
    NIGHTLY_OVER_SECONDS, SelectionError, affected_packages, balance_shards, changed_files,
    load_registry, package_graph, registry_packages_missing, run_suite, select_proofs,
    validate_registry,
)
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
        "tier": "push",
        "ci_seconds": 10,
        "ci_seconds_source": "ci",
    }


def registry_with(*proofs):
    return {
        "registry_version": 1,
        "expected_proof_count": len(proofs),
        "proofs": list(proofs),
    }


class SuiteConfigurationTests(unittest.TestCase):
    def test_every_registered_harness_exists_as_a_kani_proof_in_its_package(self):
        # A registry entry whose harness was renamed or removed fails in CI only after the
        # proofs before it have run (a harness registered under a name that did not exist, 2026-11-20); this finds it locally in a second.
        registry, errors = load_registry(REGISTRY_PATH)
        self.assertEqual(errors, [])
        root = REGISTRY_PATH.parent.parent
        sources = {}
        for proof in registry["proofs"]:
            pkg = proof["package"]
            if pkg not in sources:
                text = []
                for base in (root / "crates" / pkg, root / "tools" / pkg):
                    for path in sorted((base / "src").rglob("*.rs")) if (base / "src").is_dir() else []:
                        text.append(path.read_text(encoding="utf-8"))
                sources[pkg] = "\n".join(text)
            name = proof["harness"].rsplit("::", 1)[-1]
            pattern = re.compile(r"#\[kani::proof\][^{]*?\bfn\s+" + re.escape(name) + r"\s*\(", re.S)
            self.assertIsNotNone(pattern.search(sources[pkg]), f"{proof['id']}: no #[kani::proof] fn {name} in {pkg}")

    def test_checked_in_registry_has_all_required_unique_configured_harnesses(self):
        registry, errors = load_registry(REGISTRY_PATH)
        self.assertEqual(errors, [])
        self.assertIsNotNone(registry)
        self.assertEqual(registry["expected_proof_count"], 111)
        self.assertEqual(len({p["id"] for p in registry["proofs"]}), 111)
        self.assertEqual(
            sum(p["enabled"] for p in registry["proofs"]),
            sum(not p["experimental"] for p in registry["proofs"]),
        )
        self.assertEqual(sum(p["enabled"] for p in registry["proofs"]), 102)
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

    def test_the_msaw_proofs_are_registered(self):
        registry, errors = load_registry(REGISTRY_PATH)
        self.assertEqual(errors, [])
        msaw = {p["id"]: p for p in registry["proofs"] if p["package"] == "oa-msaw"}
        self.assertEqual(set(msaw), {
            "msaw-predicted-altitude-finite",
            "msaw-floor-monotone",
            "msaw-violation-time-in-span",
            "msaw-corridor-floor-above-terrain",
            "msaw-airport-area-floor-above-terrain-and-obstacles",
            "msaw-stale-altitude-classed-lost",
        })
        for proof in msaw.values():
            self.assertTrue(proof["enabled"] and not proof["experimental"])
            self.assertTrue(proof["harness"].startswith("calc::proofs::"))
            self.assertEqual(proof["tier"], "push")

    def test_the_runway_occupancy_proof_is_registered_with_its_covers(self):
        registry, errors = load_registry(REGISTRY_PATH)
        self.assertEqual(errors, [])
        runway = {p["id"]: p for p in registry["proofs"] if p["package"] == "oa-runway"}
        self.assertEqual(set(runway), {"runway-occupies-total"})
        for proof in runway.values():
            self.assertTrue(proof["enabled"] and not proof["experimental"])
            self.assertTrue(proof["harness"].startswith("occupy::proofs::"))
            self.assertTrue(proof["required_covers"])
            self.assertTrue(proof["requirement"].startswith("OA-RWY-"))

    def test_the_conflict_alert_solver_proofs_are_registered(self):
        registry, errors = load_registry(REGISTRY_PATH)
        self.assertEqual(errors, [])
        ca = {p["id"]: p for p in registry["proofs"] if p["package"] == "oa-ca"}
        self.assertEqual(set(ca), {
            "ca-horizontal-span",
            "ca-vertical-span",
            "ca-horizontal-now-bounds",
            "ca-horizontal-now-approach",
            "ca-horizontal-now-root",
            "ca-horizontal-now-sqrt",
            "ca-vertical-now-has-span",
            "ca-horizontal-miss-point",
            "ca-horizontal-miss",
            "ca-predict-span",
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

    def test_the_parser_and_decoder_proofs_are_registered(self):
        registry, errors = load_registry(REGISTRY_PATH)
        self.assertEqual(errors, [])
        by_package = {}
        for p in registry["proofs"]:
            by_package.setdefault(p["package"], set()).add(p["id"])
        self.assertEqual(by_package["oa-asterix"], {
            "asterix-primitives", "asterix-time-round-trip", "asterix-ident-round-trip",
            "asterix-block-framing", "asterix-position-range", "asterix-walk-cat048",
            "asterix-fspec-cat021", "asterix-fspec-cat048", "asterix-fspec-cat062",
        })
        self.assertEqual(by_package["oa-adsb"], {
            "adsb-decode-row-ads2", "adsb-decode-row-ads1", "adsb-header-two-records",
            "adsb-payload-ads3-4", "adsb-payload-ads1-4",
        })
        stubbed = {p["id"] for p in registry["proofs"] if p["package"] == "oa-adsb" and p["stubbing"]}
        self.assertEqual(stubbed, {"adsb-payload-ads3-4", "adsb-payload-ads1-4"})

    def test_the_nightly_tier_is_exactly_the_proofs_recorded_over_the_threshold(self):
        registry, errors = load_registry(REGISTRY_PATH)
        self.assertEqual(errors, [])
        self.assertEqual(NIGHTLY_OVER_SECONDS, 120)
        for p in registry["proofs"]:
            over = p["ci_seconds"] > NIGHTLY_OVER_SECONDS
            self.assertEqual(p["tier"], "nightly" if over else "push", p["id"])
            self.assertEqual(p["ci_seconds_source"] == "none", p["ci_seconds"] == 0, p["id"])
            if p["enabled"]:
                self.assertGreater(p["ci_seconds"], 0, p["id"])
        # A few known ones: slow tracker proofs and the long FSPEC walk are nightly.
        tiers = {p["id"]: p["tier"] for p in registry["proofs"]}
        self.assertEqual(tiers["large-move-decision-rejected"], "nightly")
        self.assertEqual(tiers["asterix-fspec-cat021"], "nightly")
        self.assertEqual(tiers["output-append"], "push")
        local = {p["id"] for p in registry["proofs"] if p["ci_seconds_source"] == "local"}
        self.assertEqual(len(local), 20)
        self.assertTrue(all(i.startswith(("asterix-", "adsb-", "msaw-")) for i in local))

    def test_the_tier_must_follow_the_ci_seconds_rule(self):
        for seconds, tier, ok in (
            (120, "push", True), (121, "nightly", True), (121, "push", False),
            (120, "nightly", False), (0, "push", True),
        ):
            proof = proof_entry()
            proof.update(ci_seconds=seconds, tier=tier)
            self.assertEqual(validate_registry(registry_with(proof)) == [], ok, (seconds, tier))

    def test_ci_seconds_and_its_source_are_validated(self):
        for field, bad in (
            ("ci_seconds", -1), ("ci_seconds", 1.5), ("ci_seconds", True), ("ci_seconds", None),
            ("ci_seconds_source", "guess"), ("ci_seconds_source", None),
        ):
            proof = proof_entry()
            proof[field] = bad
            self.assertTrue(validate_registry(registry_with(proof)), (field, bad))
        for field in ("ci_seconds", "ci_seconds_source"):
            proof = proof_entry()
            del proof[field]
            self.assertTrue(validate_registry(registry_with(proof)), field)

    def test_an_entry_needs_a_known_tier(self):
        for bad in ("weekly", None):
            proof = proof_entry()
            proof["tier"] = bad
            self.assertTrue(validate_registry(registry_with(proof)))
        proof = proof_entry()
        del proof["tier"]
        self.assertTrue(validate_registry(registry_with(proof)))

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

    def test_a_harness_name_contained_in_another_is_rejected(self):
        first = proof_entry()
        second = copy.deepcopy(first)
        second["id"] = "another-id"
        second["harness"] = first["harness"] + "_more"
        data = registry_with(first, second)
        errors = validate_registry(data)
        self.assertTrue(any("is contained in" in error for error in errors), errors)

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
                proof["harness"] = f"types::proofs::example_{index}_bounded"
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

    def test_shards_split_the_enabled_proofs_between_jobs_without_overlap(self):
        seen = []
        for index in range(2):
            with tempfile.TemporaryDirectory() as temp:
                repo, registry, out, target, _ = self.fake_workspace(Path(temp), proof_count=5)
                stdout, stderr = io.StringIO(), io.StringIO()
                with redirect_stdout(stdout), redirect_stderr(stderr):
                    result = run_suite(repo, out, target, registry, shard=(index, 2))
                self.assertEqual(result, 0, stderr.getvalue())
                suite = json.loads((out / "suite.json").read_text())
                self.assertEqual(suite["shard"], f"{index}/2")
                seen.append([x["proof"] if "proof" in x else x["id"] for x in suite["checks"]])
        self.assertEqual([len(x) for x in seen], [3, 2])
        self.assertFalse(set(seen[0]) & set(seen[1]))

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


def sample_graph():
    # c depends on b, b on a; d stands alone.
    return {
        "a": {"dir": "crates/a/", "deps": set()},
        "b": {"dir": "crates/b/", "deps": {"a"}},
        "c": {"dir": "crates/c/", "deps": {"b"}},
        "d": {"dir": "crates/d/", "deps": set()},
    }


def sample_proofs():
    proofs = []
    for package in "abcd":
        for tier in ("push", "nightly"):
            proof = proof_entry()
            proof.update(id=f"{package}-{tier}", package=package, tier=tier)
            proofs.append(proof)
    return proofs


def selected_ids(changed, push=True):
    affected = affected_packages(sample_graph(), changed)
    return {p["id"] for p in select_proofs(sample_proofs(), affected, push)}


class PushSelectionTests(unittest.TestCase):
    def test_a_change_in_one_package_selects_only_its_proofs(self):
        self.assertEqual(selected_ids(["crates/d/src/lib.rs"]), {"d-push"})
        self.assertEqual(selected_ids(["crates/c/tests/t.rs", "crates/c/Cargo.toml"]), {"c-push"})

    def test_a_change_in_a_dependency_selects_its_dependents(self):
        self.assertEqual(selected_ids(["crates/a/src/lib.rs"]), {"a-push", "b-push", "c-push"})
        self.assertEqual(selected_ids(["crates/b/src/x.rs"]), {"b-push", "c-push"})

    def test_a_change_outside_every_package_selects_nothing(self):
        self.assertEqual(selected_ids(["docs/VERIFICATION.md", "REQUIREMENTS.md"]), set())

    def test_a_change_to_the_registry_runner_or_workflow_selects_everything_in_the_push_tier(self):
        for path in (
            "tools/rust_proofs.json", "tools/rust_proof_suite.py", "tools/kani_validation.py",
            ".github/workflows/rust-proofs.yml", "Cargo.toml", "Cargo.lock",
        ):
            self.assertEqual(
                selected_ids([path]), {"a-push", "b-push", "c-push", "d-push"}, path,
            )

    def test_the_nightly_tier_never_runs_on_a_push_and_always_in_the_full_run(self):
        every = [p for p in sample_proofs()]
        for changed in (["crates/a/src/lib.rs"], ["tools/rust_proofs.json"]):
            self.assertFalse(any(i.endswith("nightly") for i in selected_ids(changed)))
        full = {p["id"] for p in select_proofs(every, None, False)}
        self.assertEqual(full, {p["id"] for p in every})

    def test_a_disabled_proof_is_never_selected(self):
        proofs = sample_proofs()
        for proof in proofs:
            proof["enabled"] = False
        self.assertEqual(select_proofs(proofs, None, True), [])
        self.assertEqual(select_proofs(proofs, None, False), [])

    def test_the_real_workspace_graph_ties_the_tracker_to_its_dependencies(self):
        graph = package_graph(REPO)
        self.assertIn("oa-math", graph["oa-track"]["deps"])
        registry, errors = load_registry(REGISTRY_PATH)
        self.assertEqual(errors, [])
        self.assertEqual(registry_packages_missing(registry["proofs"], graph), [])
        self.assertEqual(graph["oa-track"]["dir"], "crates/oa-track/")
        registry, _ = load_registry(REGISTRY_PATH)
        proofs = registry["proofs"]
        math_change = {p["package"] for p in select_proofs(
            proofs, affected_packages(graph, ["crates/oa-math/src/trig.rs"]), True)}
        self.assertIn("oa-math", math_change)
        self.assertIn("oa-track", math_change)
        tracker_change = {p["package"] for p in select_proofs(
            proofs, affected_packages(graph, ["crates/oa-track/src/tracker.rs"]), True)}
        self.assertIn("oa-track", tracker_change)
        self.assertNotIn("oa-asterix", tracker_change)


def manifest(name, deps="", dev="", target=""):
    text = f'[package]\nname = "{name}"\nversion = "0.0.1"\n'
    if deps:
        text += "[dependencies]\n" + deps
    if dev:
        text += "[dev-dependencies]\n" + dev
    if target:
        text += target
    return text


def git(repo, *args):
    done = subprocess.run(
        ["git", "-c", "user.name=t", "-c", "user.email=t@example.com", *args],
        cwd=repo, check=True, capture_output=True, text=True,
    )
    return done.stdout.strip()


def make_workspace(root, files):
    """A real git repository holding `files` (path -> text), committed; returns its path."""
    repo = Path(root) / "ws"
    repo.mkdir()
    git(repo, "init", "-q")
    for path, text in files.items():
        (repo / path).parent.mkdir(parents=True, exist_ok=True)
        (repo / path).write_text(text)
    git(repo, "add", "-A")
    git(repo, "commit", "-q", "-m", "base")
    return repo


class SelectionHardeningTests(unittest.TestCase):
    BASE = {
        "Cargo.toml": "[workspace]\nmembers = []\n",
        "crates/a/Cargo.toml": manifest("oa-math"),
        "crates/b/Cargo.toml": manifest("oa-track", 'oa-math = { path = "../a" }\n'),
        "crates/c/Cargo.toml": manifest("oa-ca"),
        "crates/a/src/lib.rs": "",
    }

    def test_a_rename_counts_as_a_change_to_the_old_and_the_new_path(self):
        with tempfile.TemporaryDirectory() as temp:
            repo = make_workspace(temp, self.BASE)
            base = git(repo, "rev-parse", "HEAD")
            git(repo, "mv", "crates/a/src/lib.rs", "crates/c/lib.rs")
            git(repo, "commit", "-q", "-m", "move")
            changed = changed_files(repo, base)
            self.assertEqual(sorted(changed), ["crates/a/src/lib.rs", "crates/c/lib.rs"])
            affected = affected_packages(package_graph(repo), changed)
            self.assertEqual(affected, {"oa-math", "oa-track", "oa-ca"})

    def test_a_base_starting_with_a_dash_is_refused(self):
        with tempfile.TemporaryDirectory() as temp:
            repo = make_workspace(temp, self.BASE)
            for base in ("--output=/tmp/x", "-p", "--help"):
                self.assertIsNone(changed_files(repo, base))
            self.assertIsNone(changed_files(repo, "0" * 40))
            self.assertIsNotNone(changed_files(repo, "HEAD"))

    def test_dev_and_target_dependencies_count_as_dependencies(self):
        with tempfile.TemporaryDirectory() as temp:
            files = dict(self.BASE)
            files["crates/c/Cargo.toml"] = manifest(
                "oa-ca", dev='oa-math = { path = "../a" }\n',
                target='[target.\'cfg(unix)\'.dependencies]\noa-track = { path = "../b" }\n',
            )
            repo = make_workspace(temp, files)
            graph = package_graph(repo)
            self.assertEqual(graph["oa-ca"]["deps"], {"oa-math", "oa-track"})
            self.assertEqual(
                affected_packages(graph, ["crates/a/src/lib.rs"]), {"oa-math", "oa-track", "oa-ca"}
            )
            only_target = dict(self.BASE)
            only_target["crates/c/Cargo.toml"] = manifest(
                "oa-ca",
                target='[target.\'cfg(unix)\'.build-dependencies]\noa-math = { path = "../a" }\n',
            )
        with tempfile.TemporaryDirectory() as temp:
            repo = make_workspace(temp, only_target)
            self.assertEqual(package_graph(repo)["oa-ca"]["deps"], {"oa-math"})

    def test_a_manifest_that_does_not_parse_is_an_error_not_a_skip(self):
        with tempfile.TemporaryDirectory() as temp:
            files = dict(self.BASE)
            files["crates/c/Cargo.toml"] = '[package\nname = "c"\n'
            repo = make_workspace(temp, files)
            with self.assertRaises(SelectionError):
                package_graph(repo)

    def test_a_registry_package_missing_from_the_workspace_fails_the_run(self):
        with tempfile.TemporaryDirectory() as temp:
            repo = make_workspace(temp, self.BASE)
            graph = package_graph(repo)
            ghost = proof_entry()
            ghost["package"] = "oa-runway"
            self.assertEqual(registry_packages_missing([ghost], graph), ["oa-runway"])
            known = proof_entry()
            known["package"] = "oa-math"
            self.assertEqual(registry_packages_missing([known], graph), [])
            registry = Path(temp) / "reg.json"
            registry.write_text(json.dumps(registry_with(ghost)) + "\n")
            (repo / "tools").mkdir()
            (repo / "tools" / "kani_validation.py").write_text("")
            stdout, stderr = io.StringIO(), io.StringIO()
            with redirect_stdout(stdout), redirect_stderr(stderr):
                result = run_suite(
                    repo, Path(temp) / "out", Path(temp) / "t", registry, changed_from="HEAD",
                )
            self.assertEqual(result, 2)
            self.assertIn("oa-runway", stderr.getvalue())

    def test_a_change_to_the_proof_inputs_selects_everything_in_the_push_tier(self):
        graph = {"a": {"dir": "crates/a/", "deps": set()}}
        for path in (
            "fuzz/Cargo.toml", "fuzz/fuzz_targets/x.rs", "validation/rust-proof-status.json",
            "adapters/scds-bridge/src/lib.rs", ".cargo/mutants.toml",
            "tools/prepare_rust_proof_controls.py", "tools/testdata/goto_show_loops_update_late.txt",
            "mutants.toml", "tools/rust_proofs.json", "tools/rust_proof_suite.py",
            "tools/kani_validation.py", ".github/workflows/rust-proofs.yml",
        ):
            self.assertIsNone(affected_packages(graph, [path]), path)
        self.assertEqual(affected_packages(graph, ["docs/x.md", "crates/b/src/lib.rs"]), set())

    def test_an_empty_selection_says_what_it_compared_before_it_skips(self):
        with tempfile.TemporaryDirectory() as temp:
            files = dict(self.BASE)
            files["docs/readme.md"] = "x"
            repo = make_workspace(temp, files)
            base = git(repo, "rev-parse", "HEAD")
            (repo / "docs/readme.md").write_text("y")
            git(repo, "commit", "-qam", "docs")
            (repo / "tools").mkdir()
            (repo / "tools" / "kani_validation.py").write_text("")
            known = proof_entry()
            known["package"] = "oa-ca"
            registry = Path(temp) / "reg.json"
            registry.write_text(json.dumps(registry_with(known)) + "\n")
            stdout, stderr = io.StringIO(), io.StringIO()
            with redirect_stdout(stdout), redirect_stderr(stderr):
                result = run_suite(
                    repo, Path(temp) / "out", Path(temp) / "t", registry, changed_from=base,
                )
            self.assertEqual(result, 0, stderr.getvalue())
            report = json.loads(stdout.getvalue().splitlines()[-1])
            self.assertEqual(report["suite_status"], "SKIPPED")
            self.assertEqual(report["base"], base)
            self.assertEqual(report["changed_files"], ["docs/readme.md"])
            self.assertEqual(report["affected_packages"], [])


class ShardBalanceTests(unittest.TestCase):
    def proofs(self, seconds):
        out = []
        for i, sec in enumerate(seconds):
            proof = proof_entry()
            proof.update(id=f"p{i}", ci_seconds=sec)
            out.append(proof)
        return out

    def test_every_proof_lands_in_exactly_one_shard_in_registry_order(self):
        proofs = self.proofs([300, 5, 5, 200, 40, 40, 1, 90, 0, 7])
        shards = balance_shards(proofs, 4)
        flat = [p["id"] for shard in shards for p in shard]
        self.assertEqual(sorted(flat), sorted(p["id"] for p in proofs))
        for shard in shards:
            ids = [int(p["id"][1:]) for p in shard]
            self.assertEqual(ids, sorted(ids))

    def test_the_shards_are_balanced_by_ci_seconds(self):
        proofs = self.proofs([100, 100, 100, 100, 50, 50, 50, 50, 25, 25, 25, 25])
        loads = [sum(p["ci_seconds"] for p in shard) for shard in balance_shards(proofs, 4)]
        self.assertEqual(loads, [175] * 4)
        skewed = self.proofs([400, 10, 10, 10, 10, 10, 10, 10])
        loads = sorted(sum(p["ci_seconds"] for p in s) for s in balance_shards(skewed, 3))
        self.assertEqual(loads[-1], 400)
        self.assertLessEqual(loads[0], 40)

    def test_the_real_registry_full_run_is_balanced_over_four_shards(self):
        registry, errors = load_registry(REGISTRY_PATH)
        self.assertEqual(errors, [])
        chosen = select_proofs(registry["proofs"], None, False)
        loads = [sum(p["ci_seconds"] for p in s) for s in balance_shards(chosen, 4)]
        self.assertLessEqual(max(loads) - min(loads), max(p["ci_seconds"] for p in chosen))

    def test_the_workflow_runs_four_shards(self):
        text = (REPO / ".github" / "workflows" / "rust-proofs.yml").read_text()
        self.assertIn('shard: ["0/4", "1/4", "2/4", "3/4"]', text)


if __name__ == "__main__":
    unittest.main()
