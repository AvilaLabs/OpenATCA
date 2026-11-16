"""Pure acceptance-oracle tests for validate_candidate.evaluate_perf."""
import importlib.util
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
SPEC = importlib.util.spec_from_file_location("validate_candidate", Path(__file__).with_name("validate_candidate.py"))
gate = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gate)


def evidence():
    kv = {"keeps_up": "1", "lost_work": "0", "buffers_stable": "1",
          "tracks_confirmed": "1200", "measure_s_asked": "2",
          "measure_s_done": "2", "truncated": "0",
          "offered_aircraft_unique": "1200", "offered_aircraft_expected": "1200",
          "unknown_input_labels": "0"}
    kv.update({k: "0" for k in gate.LOSS_KEYS})
    rows = [
        {"second": "60", "wall_ms": "3.1", "adsb_offered": "4", "plots_offered": "5", "rows_emitted": "10"},
        {"second": "61", "wall_ms": "4.2", "adsb_offered": "4", "plots_offered": "5", "rows_emitted": "10"},
    ]
    return kv, rows


def fusion_csv(turn_speed=7.9, truncate=False):
    header = "section,label,n,posall,posstraight,posturn,speedstraight,speedturn,headingstraight,headingturn"
    row = lambda section, label, speed: f'"{section}","{label}",100,0,0,0,0,{speed},0,0'
    lines = [header]
    if truncate:
        lines.append(row("R|all", "b  ours, no reg", turn_speed))
        return "\n".join(lines) + "\n"
    lines.extend(row("R|all", "b  ours, no reg", turn_speed) for _ in range(2))
    lines.extend(row("M|all", "b  ours, no reg", turn_speed) for _ in range(3))
    lines.extend(row("M|all", "c  ours + reg", turn_speed) for _ in range(3))
    return "\n".join(lines) + "\n"


def safeview_fixture(root):
    safeview = root / "safeview"
    actual = root / "allowed_data"
    safeview.mkdir()
    actual.mkdir()
    for name in ("raw", "derived", "tools", "fuzz"):
        target = actual / name
        target.mkdir()
        (safeview / name).symlink_to(target, target_is_directory=True)
    return safeview, actual


class PerfEvidenceTests(unittest.TestCase):
    def test_positive_full_window_with_zero_losses_passes(self):
        kv, rows = evidence()
        self.assertEqual(gate.evaluate_perf(kv, rows, 2, 1200), [])

    def test_input_coverage_cannot_be_replaced_by_hypothesis_count(self):
        kv, rows = evidence()
        kv["tracks_confirmed"] = "2400"
        self.assertEqual(gate.evaluate_perf(kv, rows, 2, 1200), [])
        kv["offered_aircraft_unique"] = "1199"
        self.assertTrue(any("input aircraft coverage" in e for e in gate.evaluate_perf(kv, rows, 2, 1200)))
        kv["offered_aircraft_unique"] = "1200"
        kv["unknown_input_labels"] = "1"
        self.assertTrue(any("unknown truth labels" in e for e in gate.evaluate_perf(kv, rows, 2, 1200)))
        kv.pop("offered_aircraft_expected")
        self.assertTrue(any("missing required keys" in e for e in gate.evaluate_perf(kv, rows, 2, 1200)))

    def test_missing_keys_and_empty_rows_are_not_assumed_clean(self):
        errors = gate.evaluate_perf({"keeps_up": "1"}, [], 2, 1200)
        self.assertTrue(any("missing required keys" in e for e in errors))
        self.assertTrue(any("missing loss counters" in e for e in errors))
        self.assertTrue(any("missing per-second CSV rows" in e for e in errors))

    def test_zero_samples_and_nonfinite_measurement_fail(self):
        kv, rows = evidence()
        self.assertTrue(any("missing per-second CSV rows" in e for e in gate.evaluate_perf(kv, [], 2, 1200)))
        rows[0]["wall_ms"] = "nan"
        self.assertTrue(any("non-finite" in e for e in gate.evaluate_perf(kv, rows, 2, 1200)))

    def test_positive_verdict_cannot_hide_loss_or_contradictory_flag(self):
        kv, rows = evidence()
        kv["loss.fusion.late"] = "1"
        kv["lost_work"] = "1"
        errors = gate.evaluate_perf(kv, rows, 2, 1200)
        self.assertTrue(any("contradicts" in e for e in errors))
        self.assertTrue(any("has loss" in e for e in errors))

    def test_partial_window_is_not_a_pass_even_with_fast_rows(self):
        kv, rows = evidence()
        kv["measure_s_done"] = "1"
        self.assertTrue(any("partial" in e for e in gate.evaluate_perf(kv, rows, 2, 1200)))

    def test_intentional_overload_control_requires_real_counted_loss(self):
        kv, rows = evidence()
        kv.update(keeps_up="0", lost_work="1", **{"loss.tracker.capacity_refused": "12"})
        self.assertEqual(gate.evaluate_perf(kv, rows, 2, 1200, overload=True), [])
        kv["loss.tracker.capacity_refused"] = "0"
        self.assertTrue(any("no nonzero loss" in e for e in gate.evaluate_perf(kv, rows, 2, 1200, overload=True)))
        kv["loss.tracker.capacity_refused"] = "-2"
        self.assertTrue(any("nonnegative integers" in e for e in gate.evaluate_perf(kv, rows, 2, 1200, overload=True)))

    def test_one_missing_expected_loss_counter_fails_closed(self):
        kv, rows = evidence()
        kv.pop(gate.LOSS_KEYS[0])
        self.assertTrue(any("missing loss counters" in e for e in gate.evaluate_perf(kv, rows, 2, 1200)))

    def test_duplicate_seconds_and_zero_activity_fail(self):
        kv, rows = evidence()
        rows[1]["second"] = rows[0]["second"]
        self.assertTrue(any("duplicate or nonconsecutive" in e for e in gate.evaluate_perf(kv, rows, 2, 1200)))
        rows[1]["second"] = "1"
        for r in rows:
            r["plots_offered"] = "0"
            r["rows_emitted"] = "0"
        errors = gate.evaluate_perf(kv, rows, 2, 1200)
        self.assertTrue(any("zero plot activity" in e for e in errors))
        self.assertTrue(any("zero output-row activity" in e for e in errors))

    def test_negative_wall_or_input_count_fails(self):
        kv, rows = evidence()
        rows[0]["wall_ms"] = "-0.1"
        rows[1]["adsb_offered"] = "-1"
        errors = gate.evaluate_perf(kv, rows, 2, 1200)
        self.assertTrue(any("negative per-second wall_ms" in e for e in errors))
        self.assertTrue(any("nonnegative" in e for e in errors))

    def test_overload_requires_stable_buffers_and_complete_window(self):
        kv, rows = evidence()
        kv.update(keeps_up="0", lost_work="1", **{"loss.tracker.capacity_refused": "12", "buffers_stable": "0"})
        self.assertTrue(any("buffers are not confirmed stable" in e for e in gate.evaluate_perf(kv, rows, 2, 1200, overload=True)))
        kv.update(buffers_stable="1", measure_s_done="1")
        self.assertTrue(any("partial" in e for e in gate.evaluate_perf(kv, rows, 2, 1200, overload=True)))

    def test_fusion_base_targets_pass_and_turn_speed_miss_is_fail(self):
        self.assertEqual(gate.evaluate_fusion_csv(fusion_csv())[0], [])
        errors, rows = gate.evaluate_fusion_csv(fusion_csv(turn_speed=9.946))
        self.assertTrue(any("speed_turn" in e for e in errors))
        self.assertIn("speed_turn", rows[0]["missed_targets"])

    def test_fusion_base_requires_full_expected_row_multiplicity(self):
        errors, _ = gate.evaluate_fusion_csv(fusion_csv(truncate=True))
        self.assertTrue(any("expected 2, found 1" in e for e in errors))
        self.assertTrue(any("expected 3, found 0" in e for e in errors))

    def test_source_integrity_detects_head_status_or_hash_change(self):
        args = ("a", "a", "", "", {"x": "1"}, {"x": "1"})
        self.assertFalse(gate.source_integrity_changed(*args))
        self.assertTrue(gate.source_integrity_changed("a", "b", "", "", {"x": "1"}, {"x": "1"}))
        self.assertTrue(gate.source_integrity_changed("a", "a", "", "M file", {"x": "1"}, {"x": "1"}))
        self.assertTrue(gate.source_integrity_changed("a", "a", "", "", {"x": "1"}, {"x": "2"}))

    def test_clip_evidence_rejects_skips_and_zero_samples(self):
        good = "clip_s=600\naircraft=2\nadsb_reports=100\nplots_radar0=20\npass0.inputs=50\npass0.calls=60\n"
        with tempfile.TemporaryDirectory() as tmp:
            d = Path(tmp)
            for name in ("clip.kv", "clip.txt", "summary.txt", "results.kv"):
                (d / name).write_text(good if name == "clip.kv" else "result\n")
            self.assertEqual(gate.evaluate_clip_artifacts(d, "clip run completed"), [])
            self.assertTrue(any("skipped" in e for e in gate.evaluate_clip_artifacts(d, "clip latency skipped")))
            (d / "clip.kv").write_text(good.replace("adsb_reports=100", "adsb_reports=0"))
            self.assertTrue(any("no measured" in e for e in gate.evaluate_clip_artifacts(d, "clip run completed")))

    def test_reference_paths_are_typed_and_safe_data_parent_is_bounded(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            data, actual = safeview_fixture(root)
            resolved = gate.resolve_reference_paths(
                {"OPENATCA_DATA": str(data), "OPENATCA_RAW": str(actual / "raw")}, root)
            self.assertEqual(set(resolved), {"OPENATCA_DATA", "OPENATCA_RAW"})
            source = actual / "raw" / "example.bin"
            source.write_bytes(b"first")
            first = gate.input_snapshot(resolved)
            source.write_bytes(b"second")
            second = gate.input_snapshot(resolved)
            self.assertNotEqual(first, second)
            renamed = source.with_name("renamed.bin")
            source.rename(renamed)
            third = gate.input_snapshot(resolved)
            self.assertNotEqual(second, third)
            (data / "scds").mkdir()
            with self.assertRaises(ValueError):
                gate.resolve_reference_paths({"OPENATCA_DATA": str(data)}, root)

    def test_directory_manifest_rejects_forbidden_symlink_target_and_cycle(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            data, actual = safeview_fixture(root)
            (root / "scds").mkdir()
            resolved = gate.resolve_reference_paths({"OPENATCA_DATA": str(data)}, root)
            (actual / "raw" / "linked").symlink_to(root / "scds", target_is_directory=True)
            with self.assertRaises(ValueError):
                gate.input_snapshot(resolved)
            (actual / "raw" / "linked").unlink()
            (actual / "raw" / "back").symlink_to(actual / "raw", target_is_directory=True)
            with self.assertRaises(ValueError):
                gate.input_snapshot(resolved)

    def test_safeview_raw_target_to_forbidden_tree_fails_before_traversal(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            safeview, _actual = safeview_fixture(root)
            forbidden = root / "scds" / "raw"
            forbidden.mkdir(parents=True)
            (safeview / "raw").unlink()
            (safeview / "raw").symlink_to(forbidden, target_is_directory=True)
            original_iterdir = Path.iterdir

            def guarded_iterdir(path):
                if path.resolve() == forbidden.resolve():
                    raise AssertionError("forbidden child was traversed")
                return original_iterdir(path)

            with patch.object(Path, "iterdir", guarded_iterdir):
                with self.assertRaises(ValueError):
                    gate.resolve_reference_paths({"OPENATCA_DATA": str(safeview)}, root)

    def test_reference_symlink_into_forbidden_tree_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            forbidden = root / "scds"
            forbidden.mkdir()
            link = root / "snapshot"
            link.symlink_to(forbidden, target_is_directory=True)
            with self.assertRaises(ValueError):
                gate.resolve_reference_paths({"OPENATCA_SNAPSHOT": str(link)}, root)


if __name__ == "__main__":
    unittest.main()
