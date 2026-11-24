"""Synthetic tests for metadata-only, sealed-aware comparison planning."""

import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent / "oa-stdds-ref"))
import plan_run  # noqa: E402
import sealed_guard  # noqa: E402


def fixture():
    digest = "a" * 64
    return {
        "schema": "oa-stdds-ref-plan-input-v1",
        "cohort_id": "synthetic-development-example",
        "interval_utc": {"start": "2026-11-24T00:00:00Z", "end": "2026-11-25T00:00:00Z"},
        "inputs": {
            "adsb": {"path": "/private/example/adsb.oaf", "sha256": digest, "source_id": "synthetic-adsb-manifest"},
            "stdds": [{"path": "/private/example/stdds.oaf.gz", "sha256": digest, "source_id": "synthetic-stdds-manifest"}],
        },
        "executable": {"path": "/private/bin/oa-stdds-ref", "sha256": digest, "source_id": "synthetic-binary-record"},
        "source": {"commit": "synthetic-commit", "tree_sha256": digest},
        "output_directory": "/openatca-private/output/run-001",
    }


def synthetic_guard(*rules):
    lines = ["root /", *rules]
    if not any(line.startswith("window ") for line in rules):
        lines.append("window 2000-01-01T00:00:00Z 2000-01-02T00:00:00Z")
    if not any(line.startswith("seal ") for line in rules):
        lines.append("seal unused-seal-rule")
    return sealed_guard.Guard(text="\n".join(lines))


class PlanTests(unittest.TestCase):
    def test_freezes_modes_limits_hashes_and_exact_current_argv(self):
        plan = plan_run.build_plan(fixture())
        self.assertEqual(plan["modes"], ["raw_published_mrtTime", "fitted_in_sample_agreement"])
        self.assertIn("prospective accuracy", plan["fit_policy"]["label"])
        self.assertEqual(plan["argv"], ["/private/bin/oa-stdds-ref", "--adsb", "/private/example/adsb.oaf", "--out", "/openatca-private/output/run-001", "--cohort-id", "synthetic-development-example", "--from-ms", "1795478400000", "--to-ms", "1795564800000", "/private/example/stdds.oaf.gz"])
        self.assertEqual(plan["limits"]["stdds_files"], 32)
        self.assertEqual(len(plan["plan_sha256"]), 64)
        self.assertIn("enforced before offset fitting", plan["interval_utc"]["selection"])

    def test_rejects_invalid_interval_hash_and_sealed_interval_before_payload_metadata(self):
        bad = fixture()
        bad["interval_utc"] = {"start": "2026-11-25T00:00:00Z", "end": "2026-11-24T00:00:00Z"}
        with self.assertRaises(plan_run.PlanError):
            plan_run.build_plan(bad)
        bad = fixture()
        bad["interval_utc"]["start"] = "2026-11-24T00:00:00.0011Z"
        with self.assertRaises(plan_run.PlanError):
            plan_run.build_plan(bad)
        bad = fixture()
        bad["inputs"]["adsb"]["sha256"] = "not-a-digest"
        with self.assertRaises(plan_run.PlanError):
            plan_run.build_plan(bad)
        bad = fixture()
        bad.pop("inputs")
        guard = synthetic_guard("window 2026-11-24T12:00:00Z 2026-11-24T13:00:00Z")
        with self.assertRaisesRegex(plan_run.PlanError, "sealed time window"):
            plan_run.build_plan(bad, guard)

    def test_rejects_sealed_input_and_output_paths_without_opening_them(self):
        bad = fixture()
        bad["inputs"]["stdds"][0]["path"] = "/private/sealed-recording.oaf"
        guard = synthetic_guard("seal private/sealed-recording.oaf")
        with self.assertRaisesRegex(plan_run.PlanError, "sealed-data guard"):
            plan_run.build_plan(bad, guard)
        bad = fixture()
        bad["output_directory"] = "/openatca-private/blocked/run-001"
        guard = synthetic_guard("seal openatca-private/blocked")
        with self.assertRaisesRegex(plan_run.PlanError, "sealed-data guard"):
            plan_run.build_plan(bad, guard)

    def test_nonexistent_recording_paths_are_metadata_only(self):
        plan = plan_run.build_plan(fixture())
        self.assertEqual(plan["inputs"]["adsb"]["path"], "/private/example/adsb.oaf")

    def test_inventory_size_bound_guarded_open_and_no_overwrite(self):
        with tempfile.TemporaryDirectory() as temporary:
            private = Path(temporary) / "openatca-private"
            private.mkdir()
            inventory = private / "inventory.json"
            output = private / "plan.json"
            inventory.write_text(json.dumps(fixture()), encoding="utf-8")
            self.assertEqual(plan_run.main(["--inventory", str(inventory), "--out", str(output)]), 0)
            original = output.read_bytes()
            self.assertEqual(plan_run.main(["--inventory", str(inventory), "--out", str(output)]), 2)
            self.assertEqual(output.read_bytes(), original)

            oversized = private / "oversized.json"
            oversized.write_bytes(b" " * (plan_run.MAX_INVENTORY_BYTES + 1))
            refused_output = private / "should-not-exist.json"
            self.assertEqual(plan_run.main(["--inventory", str(oversized), "--out", str(refused_output)]), 2)
            self.assertFalse(refused_output.exists())

    def test_main_refuses_guarded_inventory_and_output_paths(self):
        with tempfile.TemporaryDirectory() as temporary:
            private = Path(temporary) / "openatca-private"
            private.mkdir()
            inventory = private / "inventory.json"
            inventory.write_text(json.dumps(fixture()), encoding="utf-8")
            output = private / "blocked.json"
            guard = synthetic_guard(f"seal {private.name}/{output.name}")
            self.assertEqual(plan_run.main(["--inventory", str(inventory), "--out", str(output)], guard), 2)
            self.assertFalse(output.exists())


if __name__ == "__main__":
    unittest.main()
