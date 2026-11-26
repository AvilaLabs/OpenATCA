"""Synthetic-only unit checks for P-010 manifest/output gates; never read operational inputs."""
from __future__ import annotations

import contextlib
import importlib.util
import io
import json
import tempfile
import unittest
import sys
from unittest import mock
from pathlib import Path

SCRIPT = Path(__file__).with_name("reproduce_p010.py")
SPEC = importlib.util.spec_from_file_location("reproduce_p010_under_test", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("cannot load P-010 wrapper source")
p010 = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(p010)


class P010SyntheticGateTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.old_open = p010._open_guarded
        self.old_read_guarded = p010._read_guarded
        p010._open_guarded = lambda path: Path(path).open("rb")
        def read_guarded(path, cap):
            data = Path(path).read_bytes()
            if len(data) > cap:
                raise p010.Refused("test size cap")
            return data
        p010._read_guarded = read_guarded

    def tearDown(self) -> None:
        p010._open_guarded = self.old_open
        p010._read_guarded = self.old_read_guarded
        self.tmp.cleanup()

    def write(self, name: str, data: bytes) -> Path:
        path = self.root / name
        path.write_bytes(data)
        return path

    def cross_capture_summary(self, manifest_sha: str, scale: float = 1.0, binary_digest: str = "e" * 64) -> dict:
        profiles = [
            {"name": "candidate-raw", "tracker_role": "candidate", "timing_profile": "fixed-10-30-120-v1", "publication_grid": None},
            {"name": "replication-raw", "tracker_role": "replication", "timing_profile": "legacy-day-intervals", "publication_grid": None},
        ]
        public_output = {
            "name": "fixed", "timing_profile": "fixed-10-30-120-v1", "rows": 10, "seconds": 2,
            "digest": {"bytes": 1, "sha256": "1" * 64},
            "canonical_rows": {"bytes": 2, "sha256": "2" * 64},
            "profile_summary_sha256": "3" * 64,
        }
        claim = lambda metric, better, diff, lo, hi: (
            f"CLAIM metric={metric} better={better} diff={diff * scale} lo={lo * scale} hi={hi * scale} "
            f"verdict=none floor=0 oa3=1 pt3=1"
        )
        arms = [
            {"name": "candidate-raw", "tracker_role": "candidate", "timing_profile": "fixed-10-30-120-v1", "publication_grid": None, "accounting": self.cross_capture_accounting(), "claims": [
                claim("M1_median", "lower", 0.2, -0.2, 0.4),
                claim("M5_speed_median", "lower", 0.02, -0.02, 0.04),
                claim("M4t_300", "higher", 0.003, -0.003, 0.006),
            ]},
            {"name": "replication-raw", "tracker_role": "replication", "timing_profile": "legacy-day-intervals", "publication_grid": None, "accounting": self.cross_capture_accounting(), "claims": [
                claim("M1_median", "lower", 0.1, -0.2, 0.3),
                claim("M5_speed_median", "lower", 0.01, -0.02, 0.03),
                claim("M4t_300", "higher", 0.002, -0.003, 0.005),
            ]},
        ]
        return {
            "schema": p010.AGGREGATE_SCHEMA, "day": p010.DAY, "measurement_status": "complete",
            "qualification": "not-assessed; frozen-coverage-and-input-health-policy-required",
            "window": {"warmup_from": p010.WARMUP_FROM, "score_from": p010.SCORE_FROM, "score_to": p010.SCORE_TO, "interval": "half-open"},
            "arms": arms, "cohort_comparability": [], "availability_interpretation": "synthetic", "interpretation": "synthetic",
            "bindings": {
                "driver_sha256": p010._sha(p010._read_guarded(SCRIPT, 2 * 1024 * 1024)), "manifest_sha256": manifest_sha,
                "candidate_commit": "c" * 40, "replication_commit": p010.REPLICATION_TRACKER_COMMIT,
                "replication_scorer_commit": "a" * 40, "replication_scorer_patch_sha256": "b" * 64,
                "build_binary_sha256": {role: binary_digest for role in p010.BUILD_ROLES},
                "public_archive_inventory_sha256": "7" * 64, "public_clip_sha256": "4" * 64, "public_oa_outputs": [public_output], "compare_profiles": profiles,
                "health_eligibility": "unknown-not-assessed", "disclosure_eligibility": "unknown-not-assessed",
            },
        }

    @staticmethod
    def cross_capture_accounting() -> dict:
        return {
            "qualification": "not-assessed; frozen-coverage-and-input-health-policy-required",
            "address_minute_opportunities": 100, "no_active_leg_address_minutes": 1,
            "eligible_address_minutes": 90, "ambiguous_address_minute_candidates": 2,
            "unusable_truth_address_minutes": 3, "no_truth_address_minutes": 4,
            "paired_position_time_rows": 80, "available_address_minutes": 75,
            "availability_denominator": 90, "availability_percent": "83.333333",
            "tracker_measurements_before_window_filter": 1000,
            "tracker_measurements_after_warmup_filter": 800,
            "adsb_bad_frames": 0, "adsb_dropped_records": 0,
            "adsb_sealed_drops": 0, "sfdps_sealed_drops": 0,
        }

    def write_cross_capture(self, name: str, summary: dict) -> Path:
        return self.write(name, json.dumps(summary, ensure_ascii=True, sort_keys=True).encode("ascii"))

    def manifest(self) -> dict:
        tag = "2026-11-30"
        url = f"https://github.com/adsblol/globe_history_2026/releases/tag/{tag}"
        parts = [{"name": "part00", "bytes": 1, "sha256": "a" * 64}]
        inventory = {"tag": tag, "inventory_url": url, "parts": parts}
        candidate = "c" * 40
        replication_scorer = "a" * 40
        return {
            "schema": p010.SCHEMA,
            "day": p010.DAY,
            "driver_sha256": p010._sha(SCRIPT.read_bytes()),
            "release": {"tag": tag, "inventory_url": url, "inventory_sha256": p010._sha(json.dumps(inventory, ensure_ascii=True, sort_keys=True, separators=(",", ":")).encode("ascii")), "parts": parts},
            "clip": {"box": p010.ZAU_BOX, "frame_ms": 1000},
            "window": {"archive_from": p010.ARCHIVE_FROM, "archive_to": p010.ARCHIVE_TO, "warmup_from": p010.WARMUP_FROM, "score_from": p010.SCORE_FROM, "score_to": p010.SCORE_TO},
            "candidate": {"tree": "/tmp/candidate", "commit": candidate},
            "replication": {"tree": "/tmp/replication", "commit": p010.REPLICATION_TRACKER_COMMIT},
            "replication_scorer": {"tree": "/tmp/replication-scorer", "commit": replication_scorer},
            "replication_scorer_relation": {"base_commit": p010.REPLICATION_TRACKER_COMMIT, "scorer_commit": replication_scorer, "changed_paths": list(p010.SCORER_PATCH_PATHS), "delta_sha256": "b" * 64, "review_artifact": "/tmp/c-scorer-review.md", "review_sha256": "b" * 64},
            "builds": {role: {"metadata": f"/tmp/{role}.json", "metadata_sha256": "d" * 64, "binary": f"/tmp/{role}", "binary_sha256": "e" * 64, "artifact_name": p010.ARTIFACT_NAMES[role], "target_dir": f"/tmp/p010-build-target-{role}"} for role in p010.BUILD_ROLES},
            "public_arms": [{"name": "fixed", "timing_profile": "fixed-10-30-120-v1"}],
            "compare_arms": [
                {"name": "candidate-raw", "tracker_role": "candidate", "timing_profile": "fixed-10-30-120-v1", "publication_grid": None},
                {"name": "replication-raw", "tracker_role": "replication", "timing_profile": "legacy-day-intervals", "publication_grid": None},
            ],
            "limits": dict(p010.LIMITS),
        }

    # REQ: OA-TRK-412
    def test_directory_budgets_cover_empty_children_and_depth(self) -> None:
        directory = self.root / "tree"
        directory.mkdir()
        for i in range(3):
            (directory / str(i)).mkdir()
        with mock.patch.dict(p010.LIMITS, {"max_output_directories": 3}):
            with self.assertRaises(p010.Incomplete):
                p010._measure_output_tree(directory)
        with mock.patch.dict(p010.LIMITS, {"max_sfdps_directories": 3}):
            with self.assertRaises(p010.Refused):
                p010._measure_dir(directory, 10, 100)
        with mock.patch.dict(p010.LIMITS, {"max_output_depth": 0}):
            with self.assertRaises(p010.Incomplete):
                p010._measure_output_tree(directory)

    # REQ: OA-TRK-412
    def test_archive_recheck_refuses_mutated_part(self) -> None:
        directory = self.root / "archive"
        directory.mkdir()
        part = directory / "part00"
        part.write_bytes(b"a")
        doc = {"release": {"parts": [{"name": "part00", "bytes": 1, "sha256": p010._sha(b"a")}]}}
        self.assertEqual(p010._verify_inventory(directory, doc), [part.resolve()])
        part.write_bytes(b"b")
        with self.assertRaises(p010.Refused):
            p010._verify_inventory(directory, doc)

    # REQ: OA-TRK-412
    def test_source_command_capture_is_bounded_while_child_runs(self) -> None:
        result = p010._capture_source_command([sys.executable, "-c", "print('ok')"], 1024)
        self.assertEqual(result, b"ok\n")
        with self.assertRaises(p010.Refused):
            p010._capture_source_command([sys.executable, "-c", "import os; os.write(1,b'x'*8192)"], 1024)
        with self.assertRaises(p010.Refused):
            p010._capture_source_command([sys.executable, "-c", "import os; os.write(2,b'x'*131072)"], 1024)

    # REQ: OA-TRK-412
    def test_manifest_schema_and_limits_are_exact(self) -> None:
        doc = self.manifest()
        path = self.write("manifest.json", json.dumps(doc).encode())
        parsed, digest = p010._validate_manifest(path)
        self.assertEqual(parsed, doc)
        self.assertEqual(len(digest), 64)
        doc["limits"]["max_output_rows"] += 1
        path = self.write("manifest-limit.json", json.dumps(doc).encode())
        with self.assertRaises(p010.Refused):
            p010._validate_manifest(path)
        doc = self.manifest()
        doc["driver_sha256"] = "f" * 64
        path = self.write("manifest-driver.json", json.dumps(doc).encode())
        with self.assertRaises(p010.Refused):
            p010._validate_manifest(path)
        path = self.write("manifest-duplicate.json", b'{"schema":"a","schema":"b"}')
        with self.assertRaises(p010.Refused):
            p010._validate_manifest(path)

    def test_duplicate_json_keys_and_day_window_are_rejected(self) -> None:
        with self.assertRaises(ValueError):
            p010._object_no_duplicates([("day", "2026-11-30"), ("day", "2026-12-01")])

    def test_cross_capture_comparison_is_aggregate_only_and_not_a_qualification(self) -> None:
        project = self.cross_capture_summary("5" * 64)
        outsider = self.cross_capture_summary("6" * 64, scale=1.1)
        project_path = self.write_cross_capture("project-summary.json", project)
        outsider_path = self.write_cross_capture("outsider-summary.json", outsider)
        parsed_project = p010._read_cross_capture_summary(project_path)
        parsed_outsider = p010._read_cross_capture_summary(outsider_path)
        result = p010._compare_capture_summaries(parsed_project, parsed_outsider, "candidate-raw")
        self.assertEqual(result["status"], "COMPARISON_COMPLETE")
        self.assertEqual(result["metrics"]["M1_median"]["outsider_minus_project"], 0.020000000000000018)
        self.assertEqual(result["metrics"]["M1_median"]["proposed_bound"], 0.25)
        self.assertEqual(result["prediction_hit_miss"], "not-assessed-separately")
        self.assertEqual(result["capture_close_match"], "not-assessed-pending-approved-statistic-health-and-disclosure-policy")
        self.assertNotIn("private_cohorts", result)

    def test_cross_capture_cli_preserves_neutral_agreement_as_not_a_prediction_hit(self) -> None:
        project_path = self.write_cross_capture("neutral-project.json", self.cross_capture_summary("5" * 64, scale=0.0))
        outsider_path = self.write_cross_capture("neutral-outsider.json", self.cross_capture_summary("6" * 64, scale=0.0))
        out = self.root / "cross-capture-output"
        stdout = io.StringIO()
        with mock.patch.object(p010, "_new_private_dir", side_effect=lambda path: path.mkdir(mode=0o700)):
            with contextlib.redirect_stdout(stdout):
                code = p010.main([
                    "compare-captures", "--project-summary", str(project_path),
                    "--outsider-summary", str(outsider_path), "--arm", "candidate-raw", "--out", str(out),
                ])
        self.assertEqual(code, 0)
        self.assertEqual(json.loads(stdout.getvalue())["status"], "COMPARISON_COMPLETE")
        result = json.loads((out / "cross-capture-summary.json").read_text(encoding="ascii"))
        self.assertEqual(result["numeric_bounds"], "within-proposed-bounds")
        self.assertEqual(result["descriptive_alignment"], "aligned")
        self.assertEqual(result["prediction_hit_miss"], "not-assessed-separately")
        self.assertEqual(result["qualification"], "not-assessed")
        self.assertEqual(result["capture_close_match"], "not-assessed-pending-approved-statistic-health-and-disclosure-policy")
        self.assertEqual(result["matched_cohort_checked"], "not-assessed")
        self.assertFalse(result["private_cohort_bindings_disclosed"])

    def test_cross_capture_cli_allows_distinct_valid_local_build_hashes(self) -> None:
        project_path = self.write_cross_capture(
            "build-project.json", self.cross_capture_summary("5" * 64, scale=0.0, binary_digest="e" * 64)
        )
        outsider_path = self.write_cross_capture(
            "build-outsider.json", self.cross_capture_summary("6" * 64, scale=0.0, binary_digest="f" * 64)
        )
        out = self.root / "different-local-builds"
        stdout = io.StringIO()
        with mock.patch.object(p010, "_new_private_dir", side_effect=lambda path: path.mkdir(mode=0o700)):
            with contextlib.redirect_stdout(stdout):
                code = p010.main([
                    "compare-captures", "--project-summary", str(project_path),
                    "--outsider-summary", str(outsider_path), "--arm", "candidate-raw", "--out", str(out),
                ])
        self.assertEqual(code, 0)
        result = json.loads((out / "cross-capture-summary.json").read_text(encoding="ascii"))
        self.assertEqual(result["status"], "COMPARISON_COMPLETE")

    def test_cross_capture_reader_still_refuses_bad_local_build_mapping(self) -> None:
        malformed = self.cross_capture_summary("5" * 64)
        malformed["bindings"]["build_binary_sha256"].pop(p010.BUILD_ROLES[0])
        with self.assertRaises(p010.Refused):
            p010._read_cross_capture_summary(self.write_cross_capture("bad-local-build.json", malformed))

    def test_cross_capture_reader_refuses_private_duplicate_and_bad_accounting_fields(self) -> None:
        cases = []
        private_field = self.cross_capture_summary("5" * 64)
        private_field["arms"][0]["private_flight_ids"] = ["synthetic-only"]
        cases.append(("private", private_field))
        private_accounting = self.cross_capture_summary("5" * 64)
        private_accounting["arms"][0]["accounting"]["addresses"] = []
        cases.append(("private-accounting", private_accounting))
        duplicate_arm = self.cross_capture_summary("5" * 64)
        duplicate_arm["arms"].append(dict(duplicate_arm["arms"][0]))
        cases.append(("duplicate-arm", duplicate_arm))
        for name, summary in cases:
            with self.subTest(name=name):
                path = self.write_cross_capture(f"{name}.json", summary)
                with self.assertRaises(p010.Refused):
                    p010._read_cross_capture_summary(path)

    def test_cross_capture_metric_reader_refuses_reversed_and_nonfinite_intervals(self) -> None:
        reversed_interval = self.cross_capture_summary("5" * 64)
        reversed_interval["arms"][0]["claims"][0] = reversed_interval["arms"][0]["claims"][0].replace(
            "lo=-0.2 hi=0.4", "lo=0.4 hi=-0.2"
        )
        reversed_path = self.write_cross_capture("reversed.json", reversed_interval)
        with self.assertRaises(p010.Refused):
            p010._read_cross_capture_summary(reversed_path)

        nonfinite = self.cross_capture_summary("5" * 64)
        nonfinite["arms"][0]["claims"][0] = (
            "CLAIM metric=M1_median better=lower diff=NaN lo=NaN hi=NaN verdict=none floor=0 oa3=1 pt3=1"
        )
        nonfinite_path = self.write_cross_capture("nonfinite.json", nonfinite)
        parsed = p010._read_cross_capture_summary(nonfinite_path)
        outsider = p010._read_cross_capture_summary(
            self.write_cross_capture("nonfinite-outsider.json", self.cross_capture_summary("6" * 64))
        )
        with self.assertRaises(p010.Refused):
            p010._compare_capture_summaries(parsed, outsider, "candidate-raw")

    def test_cross_capture_comparison_refuses_shared_source_mismatch(self) -> None:
        project = self.cross_capture_summary("5" * 64)
        outsider = self.cross_capture_summary("6" * 64)
        outsider["bindings"]["candidate_commit"] = "d" * 40
        project = p010._read_cross_capture_summary(self.write_cross_capture("source-project.json", project))
        outsider = p010._read_cross_capture_summary(self.write_cross_capture("source-outsider.json", outsider))
        with self.assertRaises(p010.Refused):
            p010._compare_capture_summaries(project, outsider, "candidate-raw")
        self.assertEqual(p010._utc("2026-11-30T05:00:00Z", "warmup"), 1_796_014_800)
        with self.assertRaises(p010.Refused):
            p010._utc("2026-11-30T05:00:00.001Z", "warmup")

    def test_manifest_requires_exact_candidate_and_frozen_c_source_roles(self) -> None:
        doc = self.manifest()
        self.assertEqual(p010._build_source(doc, "candidate_scorer"), doc["candidate"])
        self.assertEqual(p010._build_source(doc, "replication_scorer"), doc["replication_scorer"])
        self.assertEqual(p010._validate_compare_pairs(doc["compare_arms"]), None)
        doc = self.manifest()
        doc["replication"]["commit"] = "f" * 40
        path = self.write("manifest-wrong-c.json", json.dumps(doc).encode())
        with self.assertRaises(p010.Refused):
            p010._validate_manifest(path)

        doc = self.manifest()
        doc["compare_arms"][0]["timing_profile"] = "legacy-day-intervals"
        path = self.write("manifest-profile-role-confusion.json", json.dumps(doc).encode())
        with self.assertRaises(p010.Refused):
            p010._validate_manifest(path)

        doc = self.manifest()
        doc["compare_arms"].pop()
        path = self.write("manifest-unpaired-source-role.json", json.dumps(doc).encode())
        with self.assertRaises(p010.Refused):
            p010._validate_manifest(path)

        doc = self.manifest()
        doc["replication_scorer_relation"]["changed_paths"] = ["crates/oa-track/src/fusion.rs"]
        path = self.write("manifest-tracker-delta-as-scorer.json", json.dumps(doc).encode())
        with self.assertRaises(p010.Refused):
            p010._validate_manifest(path)

    # REQ: OA-TRK-412
    def test_build_commands_have_isolated_target_and_disable_wrappers(self) -> None:
        target = Path("/tmp/p010-exclusive-target-candidate")
        argv = p010._build_argv("candidate", target)
        self.assertIn("--target-dir", argv)
        self.assertEqual(argv[argv.index("--target-dir") + 1], str(target))
        self.assertIn("build.incremental=false", argv)
        self.assertIn('build.rustc-wrapper=""', argv)
        self.assertIn('build.rustc-workspace-wrapper=""', argv)
        self.assertEqual(p010.GUARD_PROBE_SHA256, p010._sha(p010.GUARD_PROBE_SOURCE.encode("utf-8")))

    # REQ: OA-TRK-412
    def test_manifest_refuses_shared_or_nested_target_dirs(self) -> None:
        doc = self.manifest()
        doc["builds"]["candidate_scorer"]["target_dir"] = doc["builds"]["candidate"]["target_dir"]
        path = self.write("manifest-shared-target.json", json.dumps(doc).encode())
        with self.assertRaises(p010.Refused):
            p010._validate_manifest(path)
        doc = self.manifest()
        doc["builds"]["candidate_scorer"]["target_dir"] = doc["builds"]["candidate"]["target_dir"] + "/nested"
        path = self.write("manifest-nested-target.json", json.dumps(doc).encode())
        with self.assertRaises(p010.Refused):
            p010._validate_manifest(path)

    def test_private_cohort_reconciliation_requires_same_truth_membership(self) -> None:
        def arm(role, eligible_sha="a" * 64, available_sha="c" * 64, available_count=2):
            return {
                "tracker_role": role, "publication_grid": None,
                "clip_sha256_private": "d" * 64,
                "sfdps_aggregate_sha256_private": "e" * 64,
                "sfdps_input_bytes_private": 99,
                "private_cohorts": {
                    "eligible_count": 5, "eligible_sha256": eligible_sha,
                    "available_count": available_count, "available_sha256": available_sha,
                },
            }

        equal = p010._reconcile_source_cohorts([arm("candidate"), arm("replication", available_sha="f" * 64, available_count=3)])
        self.assertTrue(equal[0]["public_eligibility_identity_equal"])
        self.assertFalse(equal[0]["eram_availability_identity_equal"])
        self.assertFalse(equal[0]["common_available_intersection_computed"])
        with self.assertRaises(p010.Incomplete):
            p010._reconcile_source_cohorts([arm("candidate"), arm("replication", eligible_sha="b" * 64)])

    def test_canonical_binary_rows_reconcile_exact_length(self) -> None:
        good = b"OATKROWS" + (1).to_bytes(4, "little") + (85).to_bytes(4, "little") + (2).to_bytes(8, "little") + bytes(170)
        path = self.write("rows.bin", good)
        self.assertEqual(p010._validate_canon_rows(path), 2)
        bad = self.write("bad.bin", good[:-1])
        with self.assertRaises(p010.Incomplete):
            p010._validate_canon_rows(bad)

    def test_child_output_tree_has_directory_and_depth_bounds(self) -> None:
        class OpenGuard:
            @staticmethod
            def check(_path):
                return None

        old_guard = p010._guard
        old_max_dirs = p010.LIMITS["max_output_directories"]
        try:
            p010._guard = lambda: OpenGuard()
            root = self.root / "out"
            root.mkdir()
            (root / "one.bin").write_bytes(b"a")
            self.assertEqual(p010._measure_output_tree(root), (1, 1))
            p010.LIMITS["max_output_directories"] = 1
            (root / "sub").mkdir()
            with self.assertRaises(p010.Incomplete):
                p010._measure_output_tree(root)
        finally:
            p010._guard = old_guard
            p010.LIMITS["max_output_directories"] = old_max_dirs

    def test_digest_checks_score_window_and_per_second_totals(self) -> None:
        header = "# oa-track digest v2: second chain_hash second_hash rows; last line: end seconds rows chain"
        z = "0" * 64
        content = f"{header}\n1796018400 {z} {z} 2\n1796083199 {z} {z} 1\nend 2 3 {z}\n".encode()
        path = self.write("digest.txt", content)
        self.assertEqual(p010._validate_canon_digest(path, 3), 2)
        outside = self.write("outside.txt", f"{header}\n1796018399 {z} {z} 1\nend 1 1 {z}\n".encode())
        with self.assertRaises(p010.Incomplete):
            p010._validate_canon_digest(outside, 1)

    def test_p010_report_accepts_red_claim_outcome_but_rejects_wrong_contract(self) -> None:
        arm = {"publication_grid": None, "timing_profile": "fixed-10-30-120-v1"}
        values = {
            "P010_MODE": "local-window-v1", "P010_STATUS": "measurement-complete",
            "P010_QUALIFICATION": "not-assessed; frozen-coverage-and-input-health-policy-required",
            "P010_WINDOW_PROFILE": "p010-zau-window-v1", "P010_ARM": "raw",
            "P010_TIMING_PROFILE": arm["timing_profile"],
            "P010_TIMING_CALIBRATION_INPUTS": "versioned-constants-no-cadence-statistics",
            "P010_FULL_CLIP_SPLIT": "once-before-set-A-filter", "P010_CLIP_SHA256": "a" * 64,
            "P010_COHORT_IDENTITY_FORMAT": "domain-separated-sorted-addr-u32be-minute-i64be-truth-leg-u32be-v1",
            "P010_WARMUP_FROM": p010.WARMUP_FROM, "P010_SCORE_FROM": p010.SCORE_FROM,
            "P010_SCORE_TO": p010.SCORE_TO, "P010_SCORE_INTERVAL": "half-open",
            "P010_ERAM_AVAILABILITY_SCOPE": "post-P006-pairing-and-flight-selection; not-all-local-SFDPS-rows",
            "P010_PUBLIC_ELIGIBILITY": "one-unambiguous-set-B-truth-leg-per-address-at-whole-UTC-minute; valid-finite-position-and-velocity; speed-at-least-10-mps; actual-time-truth-leg-must-match-eligible-minute-leg; fixed-ZAU-box",
            "P010_ELIGIBILITY_MINUTE_STEP_MS": "60000", "P010_ELIGIBILITY_MINUTE_ANCHOR": "score-from",
            "P010_ZAU_BOX_PROFILE": "zau-v1", "P010_ZAU_BOX_LAT_MIN_E7": "385000000",
            "P010_ZAU_BOX_LAT_MAX_E7": "465000000", "P010_ZAU_BOX_LON_MIN_E7": "-935000000",
            "P010_ZAU_BOX_LON_MAX_E7": "-835000000", "P010_CAP_STATUS": "within-limits",
            "P010_INTERPRETATION": "local-P010-comparison-only; not-P006-holdout; no-causal-or-safety-claim",
            "P010_PRIVATE_OUTPUTS": "p002.txt,samples.csv; do-not-publish-samples.csv",
            "P010_WARMUP_FROM_NS": "1796014800000000000", "P010_SCORE_FROM_NS": "1796018400000000000",
            "P010_SCORE_TO_NS": "1796083200000000000", "P010_SFDPS_INPUT_BYTES": "10",
            "P010_SFDPS_PREFLIGHT_INPUT_BYTES": "10", "P010_SFDPS_AGGREGATE_SHA256": "b" * 64,
            "P010_ADDRESS_MINUTE_OPPORTUNITIES": "5", "P010_NO_ACTIVE_LEG_ADDRESS_MINUTES": "1",
            "P010_PUBLIC_ELIGIBLE_ADDRESS_MINUTES": "3", "P010_PUBLIC_ELIGIBLE_COHORT_SHA256": "c" * 64,
            "P010_AMBIGUOUS_ADDRESS_MINUTE_CANDIDATES": "1",
            "P010_UNUSABLE_TRUTH_ADDRESS_MINUTES": "0", "P010_NO_TRUTH_ADDRESS_MINUTES": "0",
            "P010_ERAM_FINITE_PAIRED_POSITION_TIME_ROWS": "2", "P010_ERAM_AVAILABLE_ADDRESS_MINUTES": "2",
            "P010_ERAM_AVAILABLE_COHORT_SHA256": "e" * 64,
            "P010_ERAM_AVAILABILITY_DENOMINATOR": "3", "P010_TRACKER_MEASUREMENTS_BEFORE_WINDOW_FILTER": "8",
            "P010_TRACKER_MEASUREMENTS_AFTER_WARMUP_FILTER": "4", "P010_CAP_MAX_ADDRESSES": "100000",
            "P010_ERAM_AVAILABILITY_PERCENT": "66.666667", "P010_ADSB_BAD_FRAMES": "0",
            "P010_ADSB_DROPPED_RECORDS": "0", "P010_ADSB_SEALED_DROPS": "0", "P010_SFDPS_SEALED_DROPS": "0",
            "P010_CAP_MAX_ELIGIBLE_ADDRESS_MINUTES": "10000000", "P010_CAP_MAX_SFDPS_INPUT_BYTES": "64000000000",
        }
        text = "\n".join(f"{k}={v}" for k, v in values.items())
        self.assertEqual(p010._parse_p010_report(text, arm, "a" * 64)["P010_STATUS"], "measurement-complete")
        c_arm = {"tracker_role": "replication", "publication_grid": None, "timing_profile": "legacy-day-intervals"}
        c_text = text.replace("P010_TIMING_PROFILE=fixed-10-30-120-v1", "P010_TIMING_PROFILE=legacy-day-intervals")
        c_text = c_text.replace("P010_TIMING_CALIBRATION_INPUTS=versioned-constants-no-cadence-statistics", "P010_TIMING_CALIBRATION_INPUTS=full-set-A-before-warmup-filter-noncausal-C-replication-arm")
        self.assertEqual(p010._parse_p010_report(c_text, c_arm, "a" * 64)["P010_TIMING_PROFILE"], "legacy-day-intervals")
        with self.assertRaises(p010.Incomplete):
            p010._parse_p010_report(text.replace("P010_FULL_CLIP_SPLIT=once-before-set-A-filter", "P010_FULL_CLIP_SPLIT=resplit"), arm, "a" * 64)
        with self.assertRaises(p010.Incomplete):
            p010._parse_p010_report(text.replace("P010_ADDRESS_MINUTE_OPPORTUNITIES=5", "P010_ADDRESS_MINUTE_OPPORTUNITIES=6"), arm, "a" * 64)
        with self.assertRaises(p010.Incomplete):
            p010._parse_p010_report(text.replace("P010_SFDPS_INPUT_BYTES=10", "P010_SFDPS_INPUT_BYTES=9"), arm, "a" * 64)

    # REQ: OA-TRK-412
    def test_claim_aggregate_is_exact_metric_allowlist(self) -> None:
        metrics = "\n".join([
            "CLAIM metric=M1_median better=lower diff=1.0 lo=0.5 hi=1.5 verdict=eram floor=0 oa3=4.0 pt3=5.0",
            "CLAIM metric=M5_speed_median better=lower diff=NaN lo=NaN hi=NaN verdict=none floor=0 oa3=NaN pt3=NaN",
            "CLAIM metric=M4t_300 better=higher diff=-0.1 lo=-0.2 hi=0.0 verdict=none floor=1 oa3=0.5 pt3=0.6",
        ])
        self.assertEqual(len(p010._approved_aggregate(metrics)), 3)
        report = metrics + "\nCLAIM run holdout=0 frozen=none clip=" + ("a" * 64) + " sfdps=" + ("b" * 64) + " samples=" + ("c" * 64)
        approved = p010._approved_aggregate(report)
        self.assertEqual(approved, metrics.splitlines())
        self.assertFalse(any(token in line for line in approved for token in ("CLAIM run", "clip=", "sfdps=", "samples=")))
        with self.assertRaises(p010.Incomplete):
            p010._approved_aggregate(metrics.replace("lo=0.5 hi=1.5 verdict=eram", "lo=-1.5 hi=-0.5 verdict=eram"))
        with self.assertRaises(p010.Incomplete):
            p010._approved_aggregate(metrics + "\nCLAIM metric=M6_unknown better=lower diff=0 lo=0 hi=0 verdict=none floor=0 oa3=0 pt3=0")
        with self.assertRaises(p010.Incomplete):
            p010._approved_aggregate(metrics + "\n" + metrics.splitlines()[0])
        with self.assertRaises(p010.Incomplete):
            p010._approved_aggregate(metrics.replace("lo=-0.2 hi=0.0 verdict=none", "lo=0.1 hi=0.2 verdict=none"))
        with self.assertRaises(p010.Incomplete):
            p010._approved_aggregate(metrics.replace("lo=-0.2 hi=0.0 verdict=none", "lo=0.2 hi=0.1 verdict=none"))

    def test_csv_comments_are_profile_bound_and_rows_stay_local(self) -> None:
        arm = {"publication_grid": "sfdps-v1", "timing_profile": "legacy-day-intervals"}
        comments = [
            "# DEVELOPMENT RUN — not a hold-out score",
            "# P010_MODE=local-window-v1", "# P010_ARM=sfdps-v1",
            "# P010_TIMING_PROFILE=legacy-day-intervals", "# P010_WINDOW_PROFILE=p010-zau-window-v1",
            "# P010_WARMUP_FROM=2026-11-30T05:00:00Z", "# P010_SCORE_FROM=2026-11-30T06:00:00Z",
            "# P010_SCORE_TO=2026-12-01T00:00:00Z", "# P010_ZAU_BOX_PROFILE=zau-v1",
            "# OA_PUBLICATION_GRID=sfdps-v1; OA tick positions rounded to 1 arcsecond then six decimal degrees and velocity components to 1 kt before pairing",
        ]
        header_fields = ["addr", "t_ms", "phase", "band", "common", "common_two_way"]
        for system in ("oa", "eram", "pt"):
            header_fields.extend(f"{system}_{field}" for field in ("miss", "cross_m", "along_raw_m", "along_fit_m", "h_fit_m", "along_tol_m", "h_tol_m", "speed_err_mps", "track_err_deg", "row_dt_ms"))
        header_fields.extend(("oa_coast", "oa_uncertain", "oa_reacquired"))
        header = ",".join(header_fields)
        sample = ["000001", "1796018400000", "A", "low", "1", "1"] + [""] * 33
        path = self.write("samples.csv", ("\n".join(comments + [header, ",".join(sample)]) + "\n").encode())
        self.assertEqual(p010._validate_samples_csv(path, arm), 1)
        with self.assertRaises(p010.Incomplete):
            p010._validate_samples_csv(path, {"publication_grid": None, "timing_profile": arm["timing_profile"]})


if __name__ == "__main__":
    unittest.main()
