"""Tests for cross-revision Kani proof resource limits."""

from __future__ import annotations

import copy
from contextlib import contextmanager
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock

from check_proof_caps import (
    CAP_FIELDS, CapCheckError, _git, _read_worktree_registry, _registry, check,
    compare_registries,
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
    @contextmanager
    def candidate_registry_workspace(self):
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory)
            tools = repo / "tools"
            tools.mkdir()
            yield repo, tools / "rust_proofs.json"

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

    # Kills an unbounded Git subprocess or conversion of timeout/launch failure into empty data.
    def test_git_read_has_timeout_and_refuses_process_failures(self):
        for failure in (
            OSError("git unavailable"),
            subprocess.TimeoutExpired("git", timeout=30),
            subprocess.CalledProcessError(1, "git"),
        ):
            with self.subTest(failure=type(failure).__name__):
                with mock.patch("check_proof_caps.subprocess.check_output", side_effect=failure) as call:
                    with self.assertRaises(CapCheckError):
                        _git(Path("."), "rev-parse", "HEAD")
                    self.assertLessEqual(call.call_args.kwargs["timeout"], 30)

    def test_candidate_registry_read_refuses_missing_malformed_and_root_configuration(self):
        with self.candidate_registry_workspace() as (repo, registry):
            with self.assertRaises(CapCheckError):
                _read_worktree_registry(repo)
            registry.write_bytes(b"not-json")
            with self.assertRaises(CapCheckError):
                _registry(_read_worktree_registry(repo), "candidate")

        with self.assertRaises(CapCheckError):
            check(Path("/"), "0" * 40)

    def test_candidate_registry_read_refuses_symlink_and_nonregular_file(self):
        with self.candidate_registry_workspace() as (repo, registry):
            tools = registry.parent
            target = tools / "target.json"
            target.write_bytes(b"{}")
            registry.symlink_to(target)
            with self.assertRaises(CapCheckError):
                _read_worktree_registry(repo)

            registry.unlink()
            registry.mkdir()
            with self.assertRaises(CapCheckError):
                _read_worktree_registry(repo)

    def test_candidate_registry_read_refuses_file_changed_during_read(self):
        with self.candidate_registry_workspace() as (repo, registry):
            registry.write_bytes(b"old")
            original_open = Path.open

            def replace_before_open(path, *args, **kwargs):
                if path == registry:
                    with open(path, "wb") as stream:
                        stream.write(b"replacement is longer")
                return original_open(path, *args, **kwargs)

            with mock.patch.object(Path, "open", replace_before_open):
                with self.assertRaises(CapCheckError):
                    _read_worktree_registry(repo)


if __name__ == "__main__":
    unittest.main()
