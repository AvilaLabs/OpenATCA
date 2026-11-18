"""Source-only fixtures for the negative-control provisioner."""

import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from io import StringIO
from unittest import mock

from prepare_rust_proof_controls import main


class PrepareControlsTests(unittest.TestCase):
    def make_clean_source(self, root: Path) -> Path:
        source = root / "source"
        source.mkdir()
        files = {
            "crates/oa-track/src/tracker.rs": (
                "fn update_in_order() {\n"
                "        Step::Resynced => {\n"
                "            t.stats.applied += 1;\n"
                "            emit_if_confirmed(t, slot, out);\n"
                "        }\n"
                "}\n"
                "fn caller_refusal() {\n"
                "        Step::NumericalRefused => {\n"
                "            restore_track_txn(t, slot, snapshot);\n"
                "            restore_output_txn(out, output);\n"
                "        }\n"
                "}\n"
                "fn update_late() {\n"
                "            Step::NumericalRefused => {\n"
                "                restore_track_txn(t, slot, transaction);\n"
                "                restore_output_txn(out, output);\n"
                "                return;\n"
                "            }\n"
                "}\n"
            ),
            "crates/oa-track/src/types.rs": "    o.flags.push(r.flags);\n",
            "crates/oa-track/src/time_gate.rs": "                if ns < oldest_ns {\n                    oldest_ns = ns;\n                }\n",
            "crates/oa-replay/src/mono_split.rs": (
                "    wall_ns\n"
                "        .checked_sub(mono_ns)\n"
                "        .map(|step_ns| (wall_ns, mono_ns, step_ns))\n"
            ),
            "crates/oa-track/src/tracker_rollback_paths.rs": "// unchanged rollback harness\n",
            "crates/oa-track/src/time_gate_proofs_full.rs": "// unchanged full sensor harness\n",
        }
        for rel, content in files.items():
            path = source / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content, encoding="utf-8")
        subprocess.run(["git", "init", "-q", str(source)], check=True)
        subprocess.run(["git", "-C", str(source), "config", "user.email", "proof@example.invalid"], check=True)
        subprocess.run(["git", "-C", str(source), "config", "user.name", "Proof Fixture"], check=True)
        subprocess.run(["git", "-C", str(source), "add", "."], check=True)
        subprocess.run(["git", "-C", str(source), "commit", "-qm", "clean fixture"], check=True)
        return source

    def test_clean_head_freezes_and_controls_mutate_one_product_file_only(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            source = self.make_clean_source(root)
            out = root / "controls"
            stdout = StringIO()
            with mock.patch("sys.argv", ["prepare_rust_proof_controls", "--source", str(source), "--out", str(out)]):
                with redirect_stdout(stdout), redirect_stderr(StringIO()):
                    self.assertEqual(main(), 0)

            overview = json.loads((out / "overview.json").read_text())
            self.assertEqual(overview["changed_source_paths"], [])
            self.assertEqual(overview["deleted_source_paths"], [])
            self.assertEqual(overview["frozen_source_hashes_sha256"], {})
            self.assertTrue((out / "source-snapshot" / "frozen").is_dir())

            caller = json.loads((out / "manifests" / "caller-track-restore.json").read_text())
            expected_caller = caller["expected_proof"]
            self.assertEqual(expected_caller["package"], "oa-track")
            self.assertEqual(expected_caller["harness"], "tracker::proofs::rollback_paths::in_order_refusal_restores_track_metadata_projection")
            self.assertEqual(expected_caller["configuration"], {
                "cfgs": [],
                "stubbing": True,
                "stub_summaries": [
                    "decide independently dirties the selected transaction projection with symbolic values and returns NumericalRefused; production update/capture/restore remain real."
                ],
                "required_covers": ["metadata projection dirtied before refusal"],
                "loop_unwind_overrides": [],
                "harness_timeout_seconds": 300,
                "external_timeout_seconds": 420,
                "memory_mib": 8192,
            })
            caller_checkout = Path(caller["checkout"])
            self.assertEqual(caller["patch"]["path"], "crates/oa-track/src/tracker.rs")
            caller_patch = (out / "patches" / "caller-track-restore.patch").read_text()
            self.assertEqual(
                [line for line in caller_patch.splitlines() if line.startswith("-") and not line.startswith("---")],
                ["-            restore_track_txn(t, slot, snapshot);"],
            )
            self.assertFalse(any(line.startswith("+") and not line.startswith("+++") for line in caller_patch.splitlines()))
            self.assertEqual(
                subprocess.run(["git", "-C", str(caller_checkout), "diff", "--name-only", "HEAD"], check=True, capture_output=True, text=True).stdout.splitlines(),
                ["crates/oa-track/src/tracker.rs"],
            )
            caller_proof_rel = "crates/oa-track/src/tracker_rollback_paths.rs"
            self.assertEqual((caller_checkout / caller_proof_rel).read_bytes(), (source / caller_proof_rel).read_bytes())
            caller_source_text = (caller_checkout / "crates/oa-track/src/tracker.rs").read_text()
            self.assertIn("restore_output_txn(out, output);", caller_source_text)
            self.assertNotIn("restore_track_txn(t, slot, snapshot);", caller_source_text)

            rollback = json.loads((out / "manifests" / "late-rollback.json").read_text())
            expected = rollback["expected_proof"]
            self.assertEqual(expected["package"], "oa-track")
            self.assertEqual(expected["harness"], "tracker::proofs::rollback_paths::late_overflow_output_projection")
            self.assertEqual(expected["configuration"], {
                "cfgs": [],
                "stubbing": True,
                "stub_summaries": [
                    "filter_apply changes the inserted replay event then returns NumericalRefused for the later event; filter_output returns the late row; unexpected replay/restart paths panic."
                ],
                "required_covers": [],
                "loop_unwind_overrides": ["late-replay-two-events"],
                "harness_timeout_seconds": 600,
                "external_timeout_seconds": 720,
                "memory_mib": 12288,
            })
            rollback_checkout = Path(rollback["checkout"])
            self.assertEqual(rollback["patch"]["path"], "crates/oa-track/src/tracker.rs")
            rollback_patch = (out / "patches" / "late-rollback.patch").read_text()
            self.assertEqual(
                [line for line in rollback_patch.splitlines() if line.startswith("-") and not line.startswith("---")],
                ["-                restore_output_txn(out, output);"],
            )
            self.assertFalse(any(line.startswith("+") and not line.startswith("+++") for line in rollback_patch.splitlines()))
            self.assertEqual(
                subprocess.run(["git", "-C", str(rollback_checkout), "diff", "--name-only", "HEAD"], check=True, capture_output=True, text=True).stdout.splitlines(),
                ["crates/oa-track/src/tracker.rs"],
            )
            proof_rel = "crates/oa-track/src/tracker_rollback_paths.rs"
            self.assertEqual((rollback_checkout / proof_rel).read_bytes(), (source / proof_rel).read_bytes())
            rollback_source_text = (rollback_checkout / "crates/oa-track/src/tracker.rs").read_text()
            self.assertIn("restore_track_txn(t, slot, transaction);", rollback_source_text)
            self.assertNotIn("restore_output_txn(out, output);", rollback_source_text.split("fn update_late() {", 1)[1])

            sensor = json.loads((out / "manifests" / "sensor-oldest-tie.json").read_text())
            expected_sensor = sensor["expected_proof"]
            self.assertEqual(expected_sensor["package"], "oa-track")
            self.assertEqual(expected_sensor["harness"], "time_gate::proofs_full::full_sensor_table_note_new_id_evicts_first_argmin")
            self.assertEqual(expected_sensor["configuration"], {
                "cfgs": ["kani_full_capacity"],
                "stubbing": False,
                "stub_summaries": [],
                "required_covers": [
                    "eviction counter saturates",
                    "minimum report-time input",
                    "maximum report-time input",
                    "tied minimum keeps earliest slot",
                ],
                "loop_unwind_overrides": [],
                "harness_timeout_seconds": 600,
                "external_timeout_seconds": 720,
                "memory_mib": 12288,
            })
            sensor_checkout = Path(sensor["checkout"])
            self.assertEqual(sensor["patch"]["path"], "crates/oa-track/src/time_gate.rs")
            sensor_patch = (out / "patches" / "sensor-oldest-tie.patch").read_text()
            self.assertEqual(
                [line for line in sensor_patch.splitlines() if line.startswith("-") and not line.startswith("---")],
                ["-                if ns < oldest_ns {"],
            )
            self.assertEqual(
                [line for line in sensor_patch.splitlines() if line.startswith("+") and not line.startswith("+++")],
                ["+                if ns <= oldest_ns {"],
            )
            self.assertEqual(
                subprocess.run(["git", "-C", str(sensor_checkout), "diff", "--name-only", "HEAD"], check=True, capture_output=True, text=True).stdout.splitlines(),
                ["crates/oa-track/src/time_gate.rs"],
            )
            sensor_proof_rel = "crates/oa-track/src/time_gate_proofs_full.rs"
            self.assertEqual((sensor_checkout / sensor_proof_rel).read_bytes(), (source / sensor_proof_rel).read_bytes())
            self.assertIn("if ns <= oldest_ns {", (sensor_checkout / "crates/oa-track/src/time_gate.rs").read_text())
            self.assertNotIn("if ns < oldest_ns {", (sensor_checkout / "crates/oa-track/src/time_gate.rs").read_text())


if __name__ == "__main__":
    unittest.main()
