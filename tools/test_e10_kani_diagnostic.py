"""Tests the focused E-10 diagnostic matrix and exact registry-entry binding."""

import copy
from pathlib import Path
import unittest

from e10_kani_diagnostic import (
    BASELINE_REF,
    CURRENT_REF,
    DIAGNOSTIC_CASES,
    PROOF_IDS,
    single_entry_registry,
)
from rust_proof_suite import load_registry


REPO = Path(__file__).resolve().parents[1]


class DiagnosticRegistryTests(unittest.TestCase):
    def test_matrix_is_exactly_two_proofs_on_each_pinned_revision(self):
        self.assertEqual(
            DIAGNOSTIC_CASES,
            tuple((revision, proof_id) for revision in (BASELINE_REF, CURRENT_REF) for proof_id in PROOF_IDS),
        )
        self.assertEqual(len(DIAGNOSTIC_CASES), 4)
        self.assertEqual(set(PROOF_IDS), {
            "late-append-output-projection",
            "large-move-decision-rejected-current-filter-snapshot",
        })

    def test_subset_preserves_the_complete_original_entry(self):
        registry, errors = load_registry(REPO / "tools" / "rust_proofs.json")
        self.assertEqual(errors, [])
        self.assertIsNotNone(registry)
        for proof_id in PROOF_IDS:
            with self.subTest(proof_id=proof_id):
                original = next(entry for entry in registry["proofs"] if entry["id"] == proof_id)
                before = copy.deepcopy(original)
                subset = single_entry_registry(registry, proof_id)
                self.assertEqual(subset["expected_proof_count"], 1)
                self.assertEqual(subset["registry_version"], registry["registry_version"])
                self.assertEqual(subset["proofs"], [before])
                self.assertEqual(subset["proofs"][0]["memory_mib"], before["memory_mib"])
                self.assertEqual(subset["proofs"][0].get("loop_unwind_overrides", []), before.get("loop_unwind_overrides", []))
                self.assertEqual(subset["proofs"][0]["stub_summaries"], before["stub_summaries"])
                self.assertEqual(original, before)

    def test_duplicate_or_disabled_registry_entry_is_refused(self):
        registry, errors = load_registry(REPO / "tools" / "rust_proofs.json")
        self.assertEqual(errors, [])
        self.assertIsNotNone(registry)
        proof_id = PROOF_IDS[0]
        entry = next(entry for entry in registry["proofs"] if entry["id"] == proof_id)
        duplicated = {**registry, "proofs": [entry, entry]}
        with self.assertRaisesRegex(ValueError, "exactly one enabled"):
            single_entry_registry(duplicated, proof_id)
        disabled = {**registry, "proofs": [{**entry, "enabled": False}]}
        with self.assertRaisesRegex(ValueError, "exactly one enabled"):
            single_entry_registry(disabled, proof_id)

    def test_unlisted_id_is_refused(self):
        registry, errors = load_registry(REPO / "tools" / "rust_proofs.json")
        self.assertEqual(errors, [])
        self.assertIsNotNone(registry)
        with self.assertRaisesRegex(ValueError, "outside the pinned"):
            single_entry_registry(registry, "other-proof")


if __name__ == "__main__":
    unittest.main()
