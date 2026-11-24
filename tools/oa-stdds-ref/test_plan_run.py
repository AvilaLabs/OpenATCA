"""Source-level tests for metadata-only run planning; all fixtures are synthetic."""

import unittest

import plan_run


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
        "output_directory": "/private/output/run-001",
    }


class PlanTests(unittest.TestCase):
    def test_freezes_modes_limits_hashes_and_exact_current_argv(self):
        plan = plan_run.build_plan(fixture())
        self.assertEqual(plan["modes"], ["raw_published_mrtTime", "fitted_in_sample_agreement"])
        self.assertIn("prospective accuracy", plan["fit_policy"]["label"])
        self.assertEqual(plan["argv"], ["/private/bin/oa-stdds-ref", "--adsb", "/private/example/adsb.oaf", "--out", "/private/output/run-001", "/private/example/stdds.oaf.gz"])
        self.assertEqual(plan["limits"]["stdds_files"], 32)
        self.assertEqual(len(plan["plan_sha256"]), 64)
        self.assertIn("does not clip", plan["interval_utc"]["selection"])

    def test_invalid_interval_and_hash_are_rejected(self):
        bad = fixture()
        bad["interval_utc"] = {"start": "2026-11-25T00:00:00Z", "end": "2026-11-24T00:00:00Z"}
        with self.assertRaises(plan_run.PlanError):
            plan_run.build_plan(bad)
        bad = fixture()
        bad["inputs"]["adsb"]["sha256"] = "not-a-digest"
        with self.assertRaises(plan_run.PlanError):
            plan_run.build_plan(bad)

    def test_metadata_paths_are_never_opened_or_inspected(self):
        # Deliberately nonexistent input paths are accepted: this stage consumes saved metadata only.
        plan = plan_run.build_plan(fixture())
        self.assertEqual(plan["inputs"]["adsb"]["path"], "/private/example/adsb.oaf")


if __name__ == "__main__":
    unittest.main()
