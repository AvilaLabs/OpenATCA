"""Tests the focused E-10 diagnostic matrix and exact registry-entry binding."""

import copy
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

from e10_kani_diagnostic import (
    DIAGNOSTIC_CASES,
    PROOF_IDS,
    RESOURCE_SAMPLE_HEADER,
    TARGET_CONTEXT_CASES,
    TARGET_CONTEXT_ENTRY_SHA256,
    TARGET_CONTEXT_PIPELINE_SHA256,
    TARGET_CONTEXT_PROOF_IDS,
    TARGET_CONTEXT_REF,
    TARGET_CONTEXTS,
    _single_entry_registry,
    _process_tree_memory,
    single_entry_registry,
    target_directories_for_context,
)
from rust_proof_suite import load_registry


REPO = Path(__file__).resolve().parents[1]


class DiagnosticRegistryTests(unittest.TestCase):
    def test_matrix_is_exactly_two_proofs_on_each_pinned_revision(self):
        expected = (
            ("8c5923da2f0c7f11ca84e99fc443980d86b4ef5c", "late-append-output-projection"),
            ("8c5923da2f0c7f11ca84e99fc443980d86b4ef5c", "large-move-decision-rejected-current-filter-snapshot"),
            ("64c44e714bc47bdeb956e671d7e15aa4f06edfde", "late-append-output-projection"),
            ("64c44e714bc47bdeb956e671d7e15aa4f06edfde", "large-move-decision-rejected-current-filter-snapshot"),
        )
        self.assertEqual(
            DIAGNOSTIC_CASES,
            expected,
        )
        self.assertEqual(len(DIAGNOSTIC_CASES), len(set(DIAGNOSTIC_CASES)))
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

    def test_target_context_matrix_is_independent_exact_and_ordered(self):
        expected_ids = (
            "large-move-decision-rejected-current-filter-horizontal",
            "late-append-output-projection",
            "large-move-decision-rejected-current-filter-snapshot",
            "large-move-decision-rejected-anchor-filter-snapshot",
            "large-move-decision-rejected-last-time-and-updates",
        )
        expected = (
            ("shared", "82009cde336ab3f2cf5db3bd8897df90b2c2bdcb", expected_ids),
            ("fresh", "82009cde336ab3f2cf5db3bd8897df90b2c2bdcb", expected_ids),
        )
        self.assertEqual(TARGET_CONTEXT_CASES, expected)
        self.assertEqual(TARGET_CONTEXT_PROOF_IDS, expected_ids)
        self.assertEqual(TARGET_CONTEXT_REF, expected[0][1])
        self.assertEqual(TARGET_CONTEXTS, ("shared", "fresh"))
        self.assertEqual(set(TARGET_CONTEXT_ENTRY_SHA256), set(expected_ids))
        self.assertEqual(len(set(TARGET_CONTEXT_ENTRY_SHA256.values())), len(expected_ids))

    def test_context_selects_one_shared_or_five_fresh_target_paths(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            shared = target_directories_for_context("shared", root)
            fresh = target_directories_for_context("fresh", root)
        self.assertEqual(set(shared), set(TARGET_CONTEXT_PROOF_IDS))
        self.assertEqual(set(fresh), set(TARGET_CONTEXT_PROOF_IDS))
        self.assertEqual(len(set(shared.values())), 1)
        self.assertEqual(len(set(fresh.values())), len(TARGET_CONTEXT_PROOF_IDS))
        with self.assertRaisesRegex(ValueError, "shared or fresh"):
            target_directories_for_context("unknown", Path("."))

    def test_target_context_entries_match_full_registry_without_setting_drift(self):
        expected_ids = (
            "large-move-decision-rejected-current-filter-horizontal",
            "late-append-output-projection",
            "large-move-decision-rejected-current-filter-snapshot",
            "large-move-decision-rejected-anchor-filter-snapshot",
            "large-move-decision-rejected-last-time-and-updates",
        )
        source = subprocess.run(
            ["git", "show", f"{TARGET_CONTEXT_REF}:tools/rust_proofs.json"],
            cwd=REPO, check=True, capture_output=True, text=True,
        ).stdout
        registry = json.loads(source)
        for proof_id in TARGET_CONTEXT_PROOF_IDS:
            with self.subTest(proof_id=proof_id):
                original = next(row for row in registry["proofs"] if row["id"] == proof_id)
                digest = hashlib.sha256(
                    json.dumps(original, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
                ).hexdigest()
                self.assertEqual(digest, TARGET_CONTEXT_ENTRY_SHA256[proof_id])
                subset = _single_entry_registry(registry, proof_id, expected_ids)
                self.assertEqual(subset["proofs"], [original])
                self.assertTrue(original["enabled"])
                self.assertIn("ci_seconds", original)
                self.assertIn("harness_timeout_seconds", original)
                self.assertIn("external_timeout_seconds", original)
                self.assertIn("memory_mib", original)
                self.assertIn("stub_summaries", original)
                if proof_id == expected_ids[0]:
                    self.assertEqual(original["memory_mib"], 16384)

    def test_target_context_runner_and_registry_source_hashes_are_pinned(self):
        expected = {
            "tools/rust_proof_suite.py": "c599d6f90acdcedd7155af9f78ccc0f90c6908b9feaf1b5bfa9a5802059d6e94",
            "tools/kani_validation.py": "1914868906ff98957945796627676faf8d360b938780a6ffcce25e4e2a56d8cf",
            "tools/rust_proofs.json": "6463add26059e74ad8e84fe5e5ce6152f8bfbeae1e55f02cbbdb0a7d7cb0922d",
        }
        self.assertEqual(TARGET_CONTEXT_PIPELINE_SHA256, expected)
        for relative, digest in expected.items():
            result = subprocess.run(
                ["git", "show", f"{TARGET_CONTEXT_REF}:{relative}"],
                cwd=REPO, check=True, capture_output=True,
            )
            self.assertEqual(hashlib.sha256(result.stdout).hexdigest(), digest)

    def test_process_tree_sampler_captures_descendant_peak_and_rss(self):
        with tempfile.TemporaryDirectory() as temp:
            proc = Path(temp)
            for pid, parent, peak, rss in (
                (100, 1, 111, 22), (101, 100, 333, 44), (102, 101, 222, 555),
            ):
                directory = proc / str(pid)
                directory.mkdir()
                (directory / "stat").write_text(f"{pid} (proof process) S {parent} 0\n")
                (directory / "status").write_text(f"Name:\ttest\nVmPeak:\t{peak} kB\nVmRSS:\t{rss} kB\n")
            self.assertEqual(_process_tree_memory(100, proc), ("333", "555", "3"))
        self.assertIn("max_process_vmpeak_kib", RESOURCE_SAMPLE_HEADER)
        self.assertIn("max_process_vmrss_kib", RESOURCE_SAMPLE_HEADER)

    def test_target_context_runner_refuses_unpinned_context_or_revision(self):
        from e10_kani_diagnostic import run_target_context_comparison

        with tempfile.TemporaryDirectory() as temp:
            output = Path(temp) / "output"
            with self.assertRaisesRegex(ValueError, "outside the pinned matrix"):
                run_target_context_comparison(REPO, "shared", output, "not-the-pinned-ref")
            self.assertFalse(output.exists())
            with self.assertRaisesRegex(ValueError, "outside the pinned matrix"):
                run_target_context_comparison(REPO, "guess", output, TARGET_CONTEXT_REF)
            self.assertFalse(output.exists())

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
