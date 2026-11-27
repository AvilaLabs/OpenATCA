"""Tests for cross-revision Kani proof resource limits."""

from __future__ import annotations

import copy
from pathlib import Path
import tempfile
import unittest

from check_proof_caps import (
    CAP_FIELDS, CapCheckError, _read_worktree_registry, _registry, check, compare_registries,
)

RESOURCE_FIELDS = ("memory_mib", "harness_timeout_seconds", "external_timeout_seconds")


def proof(proof_id: str = "sample-proof", harness: str = "track::proofs::sample") -> dict:
    return {
        "id": proof_id,
        "package": "oa-track-core",
        "harness": harness,
        "memory_mib": 4096,
        "harness_timeout_seconds": 120,
        "external_timeout_seconds": 180,
    }


class ProofCapComparisonTests(unittest.TestCase):
    def test_guard_covers_all_three_resource_limits(self):
        self.assertEqual(set(CAP_FIELDS), set(RESOURCE_FIELDS))

    def test_each_existing_limit_is_non_increasing(self):
        base = proof()
        for field in RESOURCE_FIELDS:
            with self.subTest(field=field):
                changed = copy.deepcopy(base)
                changed[field] += 1
                report = compare_registries([base], [changed])
                self.assertTrue(report["violations"])
                self.assertEqual(report["violations"][0]["field"], field)

    def test_renamed_proof_id_cannot_raise_same_harness_limit(self):
        base = proof(proof_id="old-proof")
        renamed = proof(proof_id="new-proof")
        renamed["memory_mib"] += 1

        report = compare_registries([base], [renamed])

        self.assertTrue(report["violations"])
        self.assertEqual(report["violations"][0]["base_id"], base["id"])
        self.assertEqual(report["violations"][0]["current_id"], renamed["id"])
        self.assertEqual(report["added_ids"], [renamed["id"]])
        self.assertEqual(report["removed_ids"], [base["id"]])
        self.assertFalse(report["removed"])

    def test_same_harness_rename_preserves_caps_and_reports_id_change(self):
        base = proof(proof_id="old-proof")
        renamed = proof(proof_id="new-proof")

        report = compare_registries([base], [renamed])

        self.assertFalse(report["violations"])
        self.assertFalse(report["removed"])
        self.assertEqual(report["added_ids"], [renamed["id"]])
        self.assertEqual(report["removed_ids"], [base["id"]])

    def test_missing_old_harness_is_reported_as_removal(self):
        base = proof()

        report = compare_registries([base], [])

        self.assertTrue(report["removed"])
        self.assertFalse(report["added"])

    def test_new_unrelated_proof_is_reported_without_becoming_old_harness_match(self):
        base = proof()
        new = proof(proof_id="new-proof", harness="track::proofs::different")
        new["memory_mib"] += 1

        report = compare_registries([base], [base, new])

        self.assertFalse(report["violations"])
        self.assertEqual(report["added"], [{
            "id": new["id"], "package": new["package"], "harness": new["harness"]
        }])

    def test_candidate_registry_read_refuses_missing_malformed_and_root_configuration(self):
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory)
            (repo / "tools").mkdir()
            with self.assertRaises(CapCheckError):
                _read_worktree_registry(repo)
            registry = repo / "tools" / "rust_proofs.json"
            registry.write_bytes(b"not-json")
            with self.assertRaises(CapCheckError):
                _registry(_read_worktree_registry(repo), "candidate")

        with self.assertRaises(CapCheckError):
            check(Path("/"), "0" * 40)


if __name__ == "__main__":
    unittest.main()
