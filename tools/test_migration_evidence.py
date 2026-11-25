"""Synthetic-only contract tests for the M6/H3 evidence generator."""
from __future__ import annotations

import hashlib
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import migration_evidence as m6  # noqa: E402
import sealed_guard  # noqa: E402


class MigrationEvidenceTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.manifest = self.root / "manifest.json"
        self.output = self.root / "private-output"
        self.unit = {
            "function_id": 7,
            "scope_id": 26,
            "comparison_family": "handoff",
            "window_start_ns": 1_800_000_000,
            "window_end_ns": 2_000_000_000,
            "stage": "S2Window",
            "policy": {"id": "M4-H2-EXACT-1", "version": 1, "sha256": "a" * 64},
            "source_epochs": {"legacy": "b" * 64, "candidate": "c" * 64},
        }
        self.documents = self._documents()
        self._write_bundle()

    def tearDown(self) -> None:
        self.temp.cleanup()

    def _source(self, stream: int) -> dict[str, int]:
        return {"stream": stream, "decoded_offset": 12, "row": 1, "operation_index": 0}

    def _family_counts(self) -> dict[str, int]:
        counters = {name: 0 for name in m6.EXPECTED_FAMILY_COUNTERS}
        counters.update(
            {
                "legacy_records": 1,
                "candidate_records": 1,
                "legacy_in_window_records": 1,
                "candidate_in_window_records": 1,
                "exact_pairs": 1,
            }
        )
        for side in ("legacy", "candidate"):
            counters.update(
                {
                    f"{side}_eligible": 1,
                    f"{side}_missing_key": 0,
                    f"{side}_invalid_key": 0,
                    f"{side}_ambiguous_key": 0,
                    f"{side}_missing_time": 0,
                    f"{side}_outside_window": 0,
                    f"{side}_unknown": 0,
                    f"{side}_unsupported": 0,
                    f"{side}_invalid": 0,
                }
            )
        return counters

    def _documents(self) -> dict[str, dict]:
        unit = self.unit
        header = {
            "function_id": unit["function_id"],
            "scope_id": unit["scope_id"],
            "window_start_ns": unit["window_start_ns"],
            "window_end_ns": unit["window_end_ns"],
        }
        return {
            "hazards": {
                "schema": "openatca.m6-hazard-register",
                "version": 1,
                **header,
                "source": {
                    "document_ref": "synthetic-fixture-only",
                    "revision": "test",
                    "coverage_status": "complete_for_function",
                    "hazard_count": 1,
                },
                "hazards": [
                    {
                        "id": "SYN-H-1",
                        "description": "synthetic test hazard",
                        "mitigations": [
                            {"id": "SYN-M-1", "description": "synthetic mitigation", "verification_ref": "synthetic-test"}
                        ],
                    }
                ],
            },
            "shadow": {
                "schema": "openatca.m6-shadow-adapter",
                "version": 1,
                **header,
                "family": "handoff",
                "policy": dict(unit["policy"]),
                "source_epochs": dict(unit["source_epochs"]),
                "coverage": {"legacy": "CompleteWithEvents", "candidate": "CompleteWithEvents"},
                "summary": {"comparison_rows": 1, "family_counts": self._family_counts()},
                "comparisons": [
                    {
                        "comparison_id": "synthetic-comparison-1",
                        "outcome": "Equivalent",
                        "classification": "equivalent",
                        "review": None,
                        "legacy_source": self._source(1),
                        "candidate_source": self._source(2),
                        "field_issues": [],
                        "duplicate_group_id": None,
                    }
                ],
            },
            "exposure": {
                "schema": "openatca.m6-exposure",
                "version": 1,
                **header,
                "records": [
                    {
                        "mode": "shadow",
                        "stage": "S1Advisory",
                        "start_ns": 1_800_000_000,
                        "end_ns": 1_900_000_000,
                        "duration_ns": 100_000_000,
                        "source_epochs": dict(unit["source_epochs"]),
                        "traffic": {"received": 1, "processed": 1, "quarantined": 0, "refused": 0, "unknown": 0},
                    },
                    {
                        "mode": "live",
                        "stage": "S2Window",
                        "start_ns": 1_900_000_000,
                        "end_ns": 2_000_000_000,
                        "duration_ns": 100_000_000,
                        "source_epochs": dict(unit["source_epochs"]),
                        "traffic": {"received": 1, "processed": 1, "quarantined": 0, "refused": 0, "unknown": 0},
                    },
                ],
            },
            "containment": {
                "schema": "openatca.m6-containment",
                "version": 1,
                **header,
                "events": [],
                "counts": {"quarantine": 0, "rollback": 0},
            },
            "drills": {
                "schema": "openatca.m6-drills",
                "version": 1,
                **header,
                "drills": [],
            },
        }

    def _write_bundle(self) -> None:
        artifact_rows = []
        for artifact_id, doc in (
            ("hazards-v1", self.documents["hazards"]),
            ("shadow-v1", self.documents["shadow"]),
            ("exposure-v1", self.documents["exposure"]),
            ("containment-v1", self.documents["containment"]),
            ("drills-v1", self.documents["drills"]),
        ):
            raw = (json.dumps(doc, sort_keys=True, separators=(",", ":")) + "\n").encode()
            path = self.root / f"{artifact_id}.json"
            path.write_bytes(raw)
            artifact_rows.append(
                {
                    "id": artifact_id,
                    "kind": {
                        "hazards-v1": "hazard_register",
                        "shadow-v1": "shadow_comparison",
                        "exposure-v1": "exposure",
                        "containment-v1": "containment",
                        "drills-v1": "drill_results",
                    }[artifact_id],
                    "path": path.name,
                    "sha256": hashlib.sha256(raw).hexdigest(),
                    "bytes": len(raw),
                }
            )
        manifest = {"schema": m6.SCHEMA, "version": 1, "unit": self.unit, "artifacts": artifact_rows}
        self.manifest.write_text(json.dumps(manifest, sort_keys=True, separators=(",", ":")) + "\n")
        self.manifest_doc = manifest

    def _rewrite_artifact(self, artifact_id: str, doc: dict) -> None:
        row = next(item for item in self.manifest_doc["artifacts"] if item["id"] == artifact_id)
        path = self.root / row["path"]
        raw = (json.dumps(doc, sort_keys=True, separators=(",", ":")) + "\n").encode()
        path.write_bytes(raw)
        row["sha256"] = hashlib.sha256(raw).hexdigest()
        row["bytes"] = len(raw)
        self.manifest.write_text(json.dumps(self.manifest_doc, sort_keys=True, separators=(",", ":")) + "\n")

    def _add_recorded_traffic_reference(self, raw: bytes = b"synthetic recorded-traffic identity fixture; not operational data") -> str:
        path = self.root / "recorded-traffic.fixture"
        path.write_bytes(raw)
        digest = hashlib.sha256(raw).hexdigest()
        self.manifest_doc["artifacts"].append(
            {
                "id": "synthetic-recorded-traffic",
                "kind": "recorded_traffic",
                "path": path.name,
                "sha256": digest,
                "bytes": len(raw),
            }
        )
        self.manifest.write_text(json.dumps(self.manifest_doc, sort_keys=True, separators=(",", ":")) + "\n")
        return digest

    # REQ: OA-MIG-601, OA-MIG-602
    def test_private_output_is_bounded_and_missing_actual_drills_remain_incomplete(self) -> None:
        output_path, package = m6.generate(str(self.manifest), str(self.output))
        self.assertEqual(package["assessment_status"], "INCOMPLETE")
        self.assertFalse(package["promotion_eligible"])
        self.assertEqual(package["approval_status"], "NOT_APPROVED")
        self.assertIn("INSUFFICIENT_REPORTED_RECORDED_DRILLS_WITH_HASH_VERIFIED_ARTIFACT:0/100", package["gaps"])
        self.assertEqual(os.stat(self.output).st_mode & 0o777, 0o700)
        self.assertEqual(os.stat(output_path).st_mode & 0o777, 0o600)
        with self.assertRaises(m6.EvidenceError):
            m6.generate(str(self.manifest), str(self.output))

    # REQ: OA-MIG-601
    def test_wrong_artifact_hash_is_retained_as_incomplete_not_accepted(self) -> None:
        row = next(item for item in self.manifest_doc["artifacts"] if item["id"] == "hazards-v1")
        row["sha256"] = "0" * 64
        self.manifest.write_text(json.dumps(self.manifest_doc, sort_keys=True, separators=(",", ":")) + "\n")
        _, package = m6.generate(str(self.manifest), str(self.output))
        status = next(item for item in package["artifacts"] if item["id"] == "hazards-v1")
        self.assertEqual(status["status"], "hash_or_size_mismatch")
        self.assertIn("UNAVAILABLE_REQUIRED_ARTIFACT:hazard_register", package["gaps"])
        self.assertEqual(package["assessment_status"], "INCOMPLETE")

    # REQ: OA-MIG-601
    def test_shadow_scope_mismatch_does_not_produce_a_complete_package(self) -> None:
        shadow = self.documents["shadow"]
        shadow["scope_id"] += 1
        self._rewrite_artifact("shadow-v1", shadow)
        _, package = m6.generate(str(self.manifest), str(self.output))
        self.assertIsNone(package["shadow"])
        self.assertTrue(any(gap.startswith("INVALID_ARTIFACT:shadow_comparison:") for gap in package["gaps"]))

    # REQ: OA-MIG-601, OA-MIG-602
    def test_unresolved_shadow_difference_is_reported_and_blocks(self) -> None:
        shadow = self.documents["shadow"]
        counts = shadow["summary"]["family_counts"]
        counts["exact_pairs"] = 0
        counts["different_pairs"] = 1
        counts["field_differences"] = 1
        shadow["comparisons"][0].update(
            {
                "outcome": "Different",
                "classification": "pending",
                "field_issues": [{"id": 3, "kind": "Different"}],
            }
        )
        self._rewrite_artifact("shadow-v1", shadow)
        _, package = m6.generate(str(self.manifest), str(self.output))
        self.assertEqual(package["assessment_status"], "INCOMPLETE")
        self.assertIn("REVIEW_PENDING:synthetic-comparison-1", package["gaps"])
        self.assertEqual(package["shadow"]["diff_count"], 1)
        self.assertEqual(package["shadow"]["differences"][0]["field_issues"][0]["id"], 3)

    # REQ: OA-MIG-601
    def test_denominator_mismatch_is_not_silently_reconciled(self) -> None:
        shadow = self.documents["shadow"]
        shadow["summary"]["family_counts"]["legacy_records"] = 2
        self._rewrite_artifact("shadow-v1", shadow)
        _, package = m6.generate(str(self.manifest), str(self.output))
        self.assertIsNone(package["shadow"])
        self.assertTrue(any("record denominator does not reconcile" in gap for gap in package["gaps"]))

    # REQ: OA-MIG-601
    def test_duplicate_artifact_ids_are_refused_before_any_package_is_written(self) -> None:
        self.manifest_doc["artifacts"].append(dict(self.manifest_doc["artifacts"][0]))
        self.manifest.write_text(json.dumps(self.manifest_doc, sort_keys=True, separators=(",", ":")) + "\n")
        with self.assertRaises(m6.EvidenceError):
            m6.generate(str(self.manifest), str(self.output))
        self.assertFalse(self.output.exists())

    # REQ: OA-MIG-601
    def test_fifo_input_is_refused_without_blocking(self) -> None:
        fifo = self.root / "synthetic-input.fifo"
        os.mkfifo(fifo, 0o600)
        with self.assertRaises(OSError):
            m6._read_file(str(fifo), 1024)

    # REQ: OA-MIG-601
    def test_overlapping_same_mode_exposure_intervals_are_refused(self) -> None:
        exposure = self.documents["exposure"]
        row = dict(exposure["records"][0])
        row.update(start_ns=1_850_000_000, end_ns=1_950_000_000)
        exposure["records"].append(row)
        self._rewrite_artifact("exposure-v1", exposure)
        _, package = m6.generate(str(self.manifest), str(self.output))
        self.assertEqual(package["assessment_status"], "INCOMPLETE")
        self.assertTrue(any("exposure intervals overlap" in gap for gap in package["gaps"]))

    # REQ: OA-MIG-601
    def test_adjacent_half_open_exposures_of_same_mode_are_accepted(self) -> None:
        exposure = self.documents["exposure"]
        row = dict(exposure["records"][0])
        row.update(
            stage="S0Shadow",
            start_ns=1_900_000_000,
            end_ns=2_000_000_000,
        )
        exposure["records"].append(row)
        self._rewrite_artifact("exposure-v1", exposure)
        _, package = m6.generate(str(self.manifest), str(self.output))
        self.assertEqual(package["assessment_status"], "INCOMPLETE")
        self.assertFalse(any("exposure intervals overlap" in gap for gap in package["gaps"]))

    # REQ: OA-MIG-601
    def test_output_creation_stays_bound_to_checked_directory_descriptor(self) -> None:
        moved = self.root / "renamed-private-output"
        real_open = os.open
        raced = False

        def replace_directory_before_file_open(path, flags, mode=0o777, *, dir_fd=None):
            nonlocal raced
            if path == "m6-evidence.json" and dir_fd is not None and not raced:
                os.rename(self.output, moved)
                os.mkdir(self.output, 0o700)
                raced = True
            if dir_fd is None:
                return real_open(path, flags, mode)
            return real_open(path, flags, mode, dir_fd=dir_fd)

        with mock.patch.object(m6.os, "open", side_effect=replace_directory_before_file_open):
            with self.assertRaises(m6.EvidenceError):
                m6._write_private_output(str(self.output), {"synthetic": True})
        self.assertTrue(raced)
        self.assertTrue((moved / "m6-evidence.json").is_file())
        self.assertFalse((self.output / "m6-evidence.json").exists())

    # REQ: OA-MIG-601, OA-SEAL-010
    def test_sealed_artifact_path_is_refused_before_open(self) -> None:
        hazard = next(item for item in self.manifest_doc["artifacts"] if item["id"] == "hazards-v1")
        hazard["path"] = "raw/adsblol/2026.11.21/not-present.json"
        self.manifest.write_text(json.dumps(self.manifest_doc, sort_keys=True, separators=(",", ":")) + "\n")
        with self.assertRaises(sealed_guard.SealedError):
            m6.generate(str(self.manifest), str(self.output))
        self.assertFalse(self.output.exists())

    # REQ: OA-MIG-601, OA-SEAL-010
    def test_sealed_unit_timestamp_is_refused_from_manifest_labels(self) -> None:
        seal_start = sealed_guard.production().windows[0][1]
        self.manifest_doc["unit"]["window_start_ns"] = seal_start
        self.manifest_doc["unit"]["window_end_ns"] = seal_start + 2_000_000_000
        self.manifest.write_text(json.dumps(self.manifest_doc, sort_keys=True, separators=(",", ":")) + "\n")
        with self.assertRaises(sealed_guard.SealedError):
            m6.generate(str(self.manifest), str(self.output))
        self.assertFalse(self.output.exists())

    # REQ: OA-MIG-601, OA-SEAL-010
    def test_sealed_timestamp_in_opaque_evidence_is_refused(self) -> None:
        self._add_recorded_traffic_reference(b"synthetic marker 2026-11-21T12:00:00Z only")
        with self.assertRaises(sealed_guard.SealedError):
            m6.generate(str(self.manifest), str(self.output))
        self.assertFalse(self.output.exists())

    # REQ: OA-MIG-602
    def test_unknown_classification_is_invalid_and_remains_incomplete(self) -> None:
        shadow = self.documents["shadow"]
        shadow["comparisons"][0]["outcome"] = "Different"
        shadow["comparisons"][0]["classification"] = "looks_good"
        shadow["summary"]["family_counts"]["exact_pairs"] = 0
        shadow["summary"]["family_counts"]["different_pairs"] = 1
        self._rewrite_artifact("shadow-v1", shadow)
        _, package = m6.generate(str(self.manifest), str(self.output))
        self.assertEqual(package["assessment_status"], "INCOMPLETE")
        self.assertTrue(any(gap.startswith("INVALID_ARTIFACT:shadow_comparison:") for gap in package["gaps"]))

    # REQ: OA-MIG-602
    def test_h5_drill_metrics_reconcile_but_never_grant_promotion(self) -> None:
        traffic_hash = self._add_recorded_traffic_reference(b"synthetic fixture traffic marker")
        phase = {
            "decision_to_action_ns": 100_000_000,
            "action_duration_ns": 200_000_000,
            "missed_updates": 0,
            "tracks_dropped": 0,
            "data_blocks_lost": 0,
            "active_alerts_lost": 0,
            "alerts_reraised": 0,
            "handoff_states_changed": 0,
        }
        self.documents["drills"]["drills"] = [
            {
                "drill_id": f"synthetic-drill-{index:03}",
                "traffic_kind": "synthetic_fixture",
                "traffic_sha256": traffic_hash,
                "scope_class": "terminal",
                "update_interval_ns": 4_800_000_000,
                "flip": dict(phase),
                "rollback": dict(phase),
                "interface": {"gaps": 0, "duplicates": 0, "out_of_order": 0, "limits_artifact_sha256": None},
            }
            for index in range(100)
        ]
        self._rewrite_artifact("drills-v1", self.documents["drills"])
        _, package = m6.generate(str(self.manifest), str(self.output))
        self.assertEqual(package["h5_metrics"]["reported_recorded_peak_drills_with_hash_verified_artifact"], 0)
        self.assertEqual(package["h5_metrics"]["synthetic_fixture_drills"], 100)
        self.assertEqual(package["h5_metrics"]["failing_drills"], 0)
        self.assertEqual(package["h5_metrics"]["max_flip_action_duration_ns"], 200_000_000)
        self.assertEqual(package["assessment_status"], "INCOMPLETE")
        self.assertFalse(package["promotion_eligible"])
        self.assertEqual(package["approval_status"], "NOT_APPROVED")

    # REQ: OA-MIG-602
    def test_failed_h5_drill_threshold_is_a_blocking_gap(self) -> None:
        traffic_hash = self._add_recorded_traffic_reference(b"synthetic fixture traffic marker")
        phase = {
            "decision_to_action_ns": 1_000_000_001,
            "action_duration_ns": 200_000_000,
            "missed_updates": 1,
            "tracks_dropped": 0,
            "data_blocks_lost": 0,
            "active_alerts_lost": 0,
            "alerts_reraised": 0,
            "handoff_states_changed": 0,
        }
        self.documents["drills"]["drills"] = [
            {
                "drill_id": "synthetic-failed-drill",
                "traffic_kind": "synthetic_fixture",
                "traffic_sha256": traffic_hash,
                "scope_class": "terminal",
                "update_interval_ns": 4_800_000_000,
                "flip": dict(phase),
                "rollback": dict(phase),
                "interface": {"gaps": 0, "duplicates": 0, "out_of_order": 0, "limits_artifact_sha256": None},
            }
        ]
        self._rewrite_artifact("drills-v1", self.documents["drills"])
        _, package = m6.generate(str(self.manifest), str(self.output))
        self.assertEqual(package["assessment_status"], "INCOMPLETE")
        self.assertIn("H5_DRILL_TIMING_OR_STATE_FAILURE:synthetic-failed-drill", package["gaps"])
        self.assertEqual(package["h5_metrics"]["failing_drills"], 1)


if __name__ == "__main__":
    unittest.main()
