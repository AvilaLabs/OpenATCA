"""Standard-library contract tests for the comparison command wrapper."""

from __future__ import annotations

import csv
import hashlib
import json
import os
import subprocess
import tempfile
import time
import unittest
from unittest import mock
from pathlib import Path

from tools import run_comparison as wrapper


class ComparisonWrapperTests(unittest.TestCase):
    # REQ: OA-CMP-050
    def test_cmp_050_output_must_be_fresh_and_outside_checkout(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            checkout = root / "checkout"
            checkout.mkdir()
            with self.assertRaises(wrapper.ComparisonRunError):
                wrapper._external_path(checkout / "results", checkout, "--out")
            external = root / "results"
            wrapper._external_path(external, checkout, "--out")
            external.mkdir()
            self.assertTrue(external.exists())

    # REQ: OA-CMP-051
    def test_cmp_051_snapshot_changes_when_tracked_source_changes(self):
        with tempfile.TemporaryDirectory() as temp:
            repo = Path(temp)
            subprocess.run(["git", "init", str(repo)], check=True, stdout=subprocess.DEVNULL)
            subprocess.run(["git", "-C", str(repo), "config", "user.email", "test@example.invalid"], check=True)
            subprocess.run(["git", "-C", str(repo), "config", "user.name", "Protocol Test"], check=True)
            source = repo / "source.txt"
            source.write_text("before\n", encoding="utf-8")
            subprocess.run(["git", "-C", str(repo), "add", "source.txt"], check=True)
            subprocess.run(["git", "-C", str(repo), "commit", "-m", "fixture"], check=True, stdout=subprocess.DEVNULL)
            before = wrapper._source_snapshot(repo)
            source.write_text("after\n", encoding="utf-8")
            after = wrapper._source_snapshot(repo)
            self.assertNotEqual(before["snapshot_sha256"], after["snapshot_sha256"])

    # REQ: OA-CMP-062
    def test_cmp_052_nonzero_stage_keeps_bounded_command_logs(self):
        with tempfile.TemporaryDirectory() as temp:
            out = Path(temp) / "run"
            out.mkdir()
            recorder = wrapper.CommandRecorder(out)
            with self.assertRaises(wrapper.ComparisonRunError):
                recorder.run("failure", ["python3", "-c", "import sys; print('kept'); sys.exit(7)"], cwd=out, timeout=10)
            self.assertTrue((out / "logs" / "001-failure.stdout.txt").is_file())
            self.assertTrue((out / "logs" / "001-failure.stderr.txt").is_file())
            self.assertEqual(recorder.records[0]["exit_code"], 7)

    # REQ: OA-CMP-053
    def test_cmp_053_artifact_and_captured_streams_have_known_sha256_digests(self):
        with tempfile.TemporaryDirectory() as temp:
            out = Path(temp) / "run"
            out.mkdir()
            artifact = out / "truth.csv"
            artifact.write_bytes(b"scenario_id,flight_id,time_ns\nstraight,1,2\n")
            artifact_hash = hashlib.sha256(artifact.read_bytes()).hexdigest()
            self.assertEqual(wrapper._hash_artifact(artifact)["sha256"], artifact_hash)

            recorder = wrapper.CommandRecorder(out)
            record = recorder.run(
                "hash-streams",
                ["python3", "-c", "import sys; print('out'); print('err', file=sys.stderr)"],
                cwd=out,
                timeout=10,
            )
            self.assertEqual(record["stdout_sha256"], hashlib.sha256(b"out\n").hexdigest())
            self.assertEqual(record["stderr_sha256"], hashlib.sha256(b"err\n").hexdigest())

    # REQ: OA-CMP-065
    def test_cmp_064_live_output_cap_stops_child_and_bounds_log_size(self):
        with tempfile.TemporaryDirectory() as temp:
            out = Path(temp) / "run"
            out.mkdir()
            recorder = wrapper.CommandRecorder(out)
            with mock.patch.object(wrapper, "MAX_LOG_BYTES", 1024):
                with self.assertRaises(wrapper.ComparisonRunError):
                    recorder.run(
                        "flood",
                        ["python3", "-c", "import sys\nwhile True:\n sys.stdout.write('x'*65536); sys.stdout.flush()"],
                        cwd=out,
                        timeout=10,
                    )
            self.assertTrue(recorder.records[0]["output_limit_exceeded"])
            self.assertLessEqual(Path(recorder.records[0]["stdout"]).stat().st_size, 1024)

    @unittest.skipUnless(os.name == "posix", "process group cleanup test requires POSIX")
    # REQ: OA-CMP-065
    def test_cmp_065_timeout_kills_descendant_process_group(self):
        with tempfile.TemporaryDirectory() as temp:
            out = Path(temp) / "run"
            out.mkdir()
            marker = out / "child-survived"
            child = f"import pathlib,time; time.sleep(0.6); pathlib.Path({str(marker)!r}).write_text('alive')"
            parent = "import subprocess,sys,time; subprocess.Popen([sys.executable, '-c', sys.argv[1]]); time.sleep(5)"
            recorder = wrapper.CommandRecorder(out)
            with self.assertRaises(wrapper.ComparisonRunError):
                recorder.run("timeout-tree", ["python3", "-c", parent, child], cwd=out, timeout=0.15)
            time.sleep(0.8)
            self.assertFalse(marker.exists())

    # REQ: OA-CMP-059
    def test_cmp_053_loss_counters_must_be_explicit(self):
        with self.assertRaises(wrapper.ComparisonRunError):
            wrapper._loss_counts({"measurement_refused": 0})
        self.assertEqual(wrapper._loss_counts({"loss_counters": {"unprocessed": 0}}), {"unprocessed": 0})

    # REQ: OA-CMP-063
    def test_cmp_063_json_artifacts_reject_nonfinite_and_nonobject_values(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "artifact.json"
            path.write_text('{"value":NaN}\n', encoding="utf-8")
            with self.assertRaises(wrapper.ComparisonRunError):
                wrapper._read_json(path)
            path.write_text('[1,2]\n', encoding="utf-8")
            with self.assertRaises(wrapper.ComparisonRunError):
                wrapper._read_json(path)

    # REQ: OA-CMP-054
    def test_cmp_054_all_scenarios_have_isolated_case_directories(self):
        self.assertEqual(wrapper._selected_scenarios("all"), wrapper.SCENARIOS)
        with tempfile.TemporaryDirectory() as temp:
            out = Path(temp)
            case_paths = [wrapper._case_output_path(out, scenario) for scenario in wrapper._selected_scenarios("all")]
            self.assertEqual(len(set(case_paths)), 6)
            self.assertTrue(all(path.parent == out / "cases" for path in case_paths))
            self.assertEqual(wrapper._selected_scenarios("turn"), ("turn",))

    # REQ: OA-CMP-055
    def test_cmp_055_grid_exposes_only_scenario_flight_and_time(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            truth = root / "truth.csv"
            truth.write_text(
                "scenario_id,flight_id,time_ns,lat_deg,lon_deg,ve_mps,vn_mps,phase\n"
                "straight,4,100,1,2,3,4,cruise\n",
                encoding="utf-8",
            )
            grid = root / "grid.csv"
            wrapper._create_truth_grid(truth, grid)
            with grid.open(encoding="utf-8", newline="") as source:
                reader = csv.DictReader(source)
                self.assertEqual(reader.fieldnames, ["scenario_id", "flight_id", "time_ns"])
                self.assertEqual(next(reader), {"scenario_id": "straight", "flight_id": "4", "time_ns": "100"})

    # REQ: OA-CMP-056
    def test_cmp_055_identity_binding_rejects_missing_or_extra_rows(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            fields = ["scenario_id", "flight_id", "score_ns", "source_ns", "publish_ns", "lat_deg", "lon_deg", "ve_mps", "vn_mps", "status"]
            identity_fields = ["system", "scenario_id", "flight_id", "score_ns", "track_id", "truth_flight_id"]
            grid = root / "grid.csv"
            with grid.open("w", encoding="utf-8", newline="") as f:
                w = csv.DictWriter(f, fieldnames=["scenario_id", "flight_id", "time_ns"])
                w.writeheader(); w.writerow({"scenario_id": "straight", "flight_id": 1, "time_ns": 5})
            def output(name):
                path = root / name
                with path.open("w", encoding="utf-8", newline="") as f:
                    w = csv.DictWriter(f, fieldnames=fields); w.writeheader()
                    w.writerow(dict(scenario_id="straight", flight_id=1, score_ns=5, source_ns=4, publish_ns=5, lat_deg=1, lon_deg=2, ve_mps=3, vn_mps=4, status="valid"))
                return path
            candidate = output("candidate.csv")
            baseline = output("baseline.csv")
            identity = root / "identity.csv"
            with identity.open("w", encoding="utf-8", newline="") as f:
                w = csv.DictWriter(f, fieldnames=identity_fields); w.writeheader()
            with self.assertRaises(wrapper.ComparisonRunError):
                wrapper._create_identity_binding(identity, candidate, baseline, grid, root / "binding.csv")

    # REQ: OA-CMP-061
    def test_cmp_056_competitive_gate_requires_every_quality_and_speed_bar(self):
        passing = {"scenario_role": "nominal", "profile_gate": {"status": "nominal_profile_pass", "usable_keysets_match": True}, "accuracy_qualification": {"pass": True, "all_quality_pass": True}, "fault_mode_cohort_status": "PASS", "speed_coverage": {"pass": True}}
        self.assertTrue(wrapper._competitive_pass([passing]))
        self.assertFalse(wrapper._competitive_pass([passing, {"scenario_role": "nominal", "profile_gate": {"status": "nominal_profile_pass", "usable_keysets_match": True}, "accuracy_qualification": {"pass": True}, "fault_mode_cohort_status": "PASS", "speed_coverage": {"pass": False}}]))
        diagnostic = {"scenario_role": "diagnostic_only", "profile_gate": {"status": "diagnostic_only"}}
        self.assertTrue(wrapper._competitive_pass([passing, diagnostic]))
        self.assertFalse(wrapper._competitive_pass([diagnostic]))
        self.assertFalse(wrapper._competitive_pass([]))
        self.assertEqual(wrapper._require_gate_exit([], require=True), 1)
        self.assertEqual(wrapper._require_gate_exit([passing], require=True), 0)

    # REQ: OA-CMP-057
    def test_cmp_057_python_control_is_never_classified_as_professional(self):
        # The wrapper stamps every run development_control, regardless of baseline metrics.
        self.assertEqual(wrapper.COMPARISON_CLASS, "development_control")
        self.assertFalse(wrapper._competitive_pass([{"accuracy_qualification": {"pass": True, "all_quality_pass": False}, "speed_coverage": {"pass": True}}]))

    # REQ: OA-CMP-059
    def test_cmp_059_model_reference_requires_hash_bound_pass(self):
        valid = {
            "status": "PASS", "validation_status": "PASS", "reference_kind": "independent_simulation",
            "scope": "declared_model_only", "physical_accuracy_claim": False,
            "runner_source_hash_verified": True, "runner_source_sha256": "abc",
            "hashes": {"truth_sha256": "t", "measurements_sha256": "m", "protocol_sha256": "p", "runner_source_sha256": "abc"},
        }
        self.assertTrue(wrapper._reference_qualified(valid, "abc", "t", "m", "p"))
        self.assertFalse(wrapper._reference_qualified(valid, "abc", "other", "m", "p"))
        self.assertFalse(wrapper._reference_qualified({**valid, "runner_source_sha256": "other"}, "abc", "t", "m", "p"))
        self.assertFalse(wrapper._reference_qualified({**valid, "status": "FAIL"}, "abc", "t", "m", "p"))

    # REQ: OA-CMP-058
    def test_cmp_058_replay_call_cost_never_becomes_first_use_latency_claim(self):
        self.assertIn("not first-use wall timing", wrapper._speed_claim("replay_call_cost"))
        self.assertIn("no validated", wrapper._speed_claim(None))
        self.assertIn("does not qualify", wrapper._speed_claim("wall_clock", {"elapsed_ns": [10]}))

    # REQ: OA-CMP-051
    def test_cmp_051_changed_source_snapshot_stops_pipeline_and_keeps_record(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            repo = root / "repo"
            (repo / ".git").mkdir(parents=True)
            (repo / wrapper.RUNNER_REL).mkdir(parents=True)
            (repo / wrapper.RUNNER_REL / "Cargo.toml").write_text("[package]\n", encoding="utf-8")
            source = repo / wrapper.RUNNER_REL / "src"
            source.mkdir()
            (source / "main.rs").write_text("fn main() {}\n", encoding="utf-8")
            for rel in (wrapper.PROTOCOL_REL, wrapper.REFERENCE_TOOL_REL, wrapper.BASELINE_TOOL_REL, wrapper.EVALUATOR_TOOL_REL):
                path = repo / rel; path.parent.mkdir(parents=True, exist_ok=True)
                content = json.dumps({
                    "protocol_id": "openatca-comparison",
                    "version": "1.0.3",
                    "seeds_and_holdout": {"development_seeds": {"set_id": "development-set-v1", "seed_ids": [0, 1]}},
                }) + "\n" if rel == wrapper.PROTOCOL_REL else "{}\n"
                path.write_text(content, encoding="utf-8")
            binary = root / "runner"
            binary.write_bytes(b"runner")
            output = root / "output"
            before = {"snapshot_sha256": "a", "status_porcelain": " M file"}
            after = {"snapshot_sha256": "b", "status_porcelain": " M changed"}
            args = wrapper.build_parser().parse_args(["--repo", str(repo), "--out", str(output), "--runner", str(binary)])
            with mock.patch.object(wrapper, "_source_snapshot", side_effect=[before, after, after]):
                code = wrapper.run(args)
            self.assertEqual(code, 1)
            summary = json.loads((output / "summary.json").read_text(encoding="utf-8"))
            self.assertIn("source changed", summary["error"])
            self.assertTrue(summary["artifacts_retained"])

    # REQ: OA-CMP-060
    def test_cmp_060_default_cli_is_development_only_seed_one(self):
        args = wrapper.build_parser().parse_args(["--out", "/tmp/fresh-comparison"])
        self.assertEqual(args.seed, 1)
        self.assertEqual(args.scenario, "straight")
        self.assertEqual(args.aircraft_count, 8)
        self.assertEqual(args.duration_s, 60)
        self.assertEqual(wrapper.MAX_AIRCRAFT_COUNT, 300)
        self.assertEqual(wrapper.MAX_DURATION_S, 1200)
        self.assertEqual(wrapper.SCENARIOS, ("straight", "turn", "acceleration", "crossing", "dropout", "delayed"))

    # REQ: OA-CMP-052
    def test_cmp_052_build_is_pinned_locked_offline_and_single_job(self):
        target = Path("/external/target")
        command = wrapper._build_command(Path("/checkout"), target)
        self.assertEqual(command[:6], ["cargo", "+1.89.0", "build", "--locked", "--offline", "--release"])
        self.assertIn("--manifest-path", command)
        self.assertIn("--target-dir", command)
        env = wrapper._build_environment(target)
        self.assertEqual(env["CARGO_BUILD_JOBS"], "1")
        self.assertEqual(env["CARGO_NET_OFFLINE"], "true")
        self.assertEqual(env["CARGO_TARGET_DIR"], str(target))

    # REQ: OA-CMP-064
    def test_cmp_064_runner_bounds_match_frozen_runner(self):
        wrapper._validate_runner_bounds(300, 1200)
        for aircraft_count, duration_s in ((301, 1200), (300, 1201), (0, 60)):
            with self.assertRaises(wrapper.ComparisonRunError):
                wrapper._validate_runner_bounds(aircraft_count, duration_s)

    # REQ: OA-CMP-066
    def test_cmp_066_cohort_and_scenario_role_are_bound_to_protocol_and_raw_manifest(self):
        protocol = wrapper._read_json(Path(__file__).resolve().parents[1] / wrapper.PROTOCOL_REL)
        manifest = {"seed": 1, "scenario_id": "straight"}
        self.assertEqual(wrapper._cohort_id_for_run(protocol, manifest, 1, "straight"), "development-set-v1")
        self.assertEqual(wrapper._scenario_role(protocol, "straight"), "nominal")
        self.assertEqual(wrapper._scenario_role(protocol, "dropout"), "diagnostic_only")
        with self.assertRaises(wrapper.ComparisonRunError):
            wrapper._cohort_id_for_run(protocol, {**manifest, "seed": 1000001}, 1, "straight")
        with self.assertRaises(wrapper.ComparisonRunError):
            wrapper._cohort_id_for_run(protocol, {**manifest, "scenario_id": "turn"}, 1, "straight")


if __name__ == "__main__":
    unittest.main()
