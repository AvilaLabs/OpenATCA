#!/usr/bin/env python3
"""Synthetic regression and refusal tests for development sample tail attribution."""

import csv
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import tracker_tail
import sealed_guard


FIXTURE_DIR = Path(__file__).parent / "testdata"
FIXTURE = FIXTURE_DIR / "tracker_tail_synthetic.csv"
REPORT = FIXTURE_DIR / "p002.txt"
SYNTHETIC_RULES = "root /tmp\nseal held.csv\nwindow 1970-01-01T00:00:00Z 1970-01-01T00:00:02Z\n"


def write_paired_files(root: Path, *, held_value: str = "80.0") -> tuple[Path, Path]:
    report = root / "p002.txt"
    report.write_text(REPORT.read_text(encoding="utf-8"), encoding="utf-8")
    samples = root / "samples.csv"
    with FIXTURE.open(encoding="utf-8", newline="") as stream:
        rows = list(csv.reader(stream))
    # Corrupt the first data row's score field so a sealed-time test proves time is checked first.
    rows[2][5] = held_value
    with samples.open("w", newline="", encoding="utf-8") as stream:
        writer = csv.writer(stream, lineterminator="\n")
        writer.writerows(rows)
    return samples, report


class TrackerTailTests(unittest.TestCase):
    def test_synthetic_turn_gap_reacquisition_failure_is_attributed(self) -> None:
        loaded = tracker_tail.load_samples(FIXTURE, REPORT, allow_synthetic=True)
        report = tracker_tail.summarize(loaded)
        self.assertEqual(report["denominators"], {
            "sample_rows_total": 6,
            "non_two_way_rows_excluded": 0,
            "common_two_way_rows": 6,
            "common_two_way_rows_missing_oa_cross_excluded": 0,
            "common_two_way_finite_oa_cross_scored": 6,
            "procedure_report_grid_samples_removed_all_systems": 0,
        })
        self.assertEqual(report["overall"]["median_abs_cross_m"], 3.0)
        self.assertEqual(report["overall"]["p95_abs_cross_m"], 80.0)
        self.assertEqual(report["overall"]["overall_p95_tail_n_using_greater_or_equal_cutoff"], 1)
        cohorts = report["cohorts"]
        high_heading = next(x for x in cohorts["absolute_heading_error"] if x["cohort"] == ">=5 deg")
        self.assertEqual((high_heading["n"], high_heading["overall_p95_tail_n"]), (1, 1))
        long_interval = next(x for x in cohorts["preceding_scored_sample_interval"] if x["cohort"] == ">60 s")
        self.assertEqual((long_interval["n"], long_interval["overall_p95_tail_n"]), (1, 1))
        reacquired = next(x for x in cohorts["reacquired"] if x["cohort"] == "yes")
        self.assertEqual((reacquired["n"], reacquired["overall_p95_tail_n"]), (1, 1))
        joint = next(x for x in cohorts["heading_interval_reacquisition_joint"]
                     if x["cohort"] == "heading=>=5 deg; interval=>60 s; reacquired=yes")
        self.assertEqual((joint["n"], joint["overall_p95_tail_n"]), (1, 1))
        serialized = str(report)
        self.assertNotIn("SYNTHETIC-A", serialized)
        self.assertNotIn("SYNTHETIC-B", serialized)

    def test_nearest_rank_matches_p002_definition(self) -> None:
        self.assertEqual(tracker_tail.nearest_rank([4.0, 1.0, 3.0, 2.0], 0.50), 2.0)
        self.assertEqual(tracker_tail.nearest_rank([4.0, 1.0, 3.0, 2.0], 0.95), 4.0)

    def test_sealed_symlink_is_refused_before_open(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            samples, report = write_paired_files(root)
            held = root / "held.csv"
            held.write_text("HELD DATA MUST NOT BE OPENED\n", encoding="utf-8")
            samples.unlink()
            samples.symlink_to(held)
            guard = sealed_guard.Guard(SYNTHETIC_RULES, root=root)
            with mock.patch.object(sealed_guard, "refuse", side_effect=lambda *paths: [guard.check(p) for p in paths if p]), \
                    mock.patch.object(sealed_guard, "open_read", wraps=guard.open) as open_read:
                with self.assertRaises(sealed_guard.SealedError):
                    tracker_tail.load_samples(samples, report, allow_synthetic=True)
                open_read.assert_not_called()

    def test_sealed_output_symlink_is_refused_and_target_is_untouched(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            held = root / "held.csv"
            held.write_text("HELD OUTPUT TARGET\n", encoding="utf-8")
            output = root / "report.json"
            output.symlink_to(held)
            guard = sealed_guard.Guard(SYNTHETIC_RULES, root=root)
            with mock.patch.object(sealed_guard, "refuse", side_effect=lambda *paths: [guard.check(p) for p in paths if p]):
                with self.assertRaises(sealed_guard.SealedError):
                    tracker_tail._write_output(output, "{}\n")
            self.assertEqual(held.read_text(encoding="utf-8"), "HELD OUTPUT TARGET\n")

    def test_sealed_timestamp_refuses_before_interpreting_measurement(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            samples, report = write_paired_files(root, held_value="HELD METRIC MUST NOT BE INTERPRETED")
            guard = sealed_guard.Guard(SYNTHETIC_RULES, root=root)
            with mock.patch.object(sealed_guard, "production", return_value=guard), \
                    mock.patch.object(sealed_guard, "refuse", side_effect=lambda *paths: [guard.check(p) for p in paths if p]), \
                    mock.patch.object(sealed_guard, "open_read", wraps=guard.open):
                with self.assertRaisesRegex(tracker_tail.TailError, "sealed hold-out window"):
                    tracker_tail.load_samples(samples, report, allow_synthetic=True)

    def test_report_pair_and_fixed_categories_are_required(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            samples, report = write_paired_files(root)
            data = samples.read_text(encoding="utf-8").replace(",cruise,", ",user-controlled-label,")
            samples.write_text(data, encoding="utf-8")
            with self.assertRaisesRegex(tracker_tail.TailError, "noncanonical phase"):
                tracker_tail.load_samples(samples, report, allow_synthetic=True)

    def test_nov19_development_gate_includes_rehearsal_end_but_not_after(self) -> None:
        run = tracker_tail.DevelopmentRun("P-006 section 10 strict tune rehearsal 2026-11-19",
                                          tracker_tail._epoch_ms(2026, 11, 19),
                                          tracker_tail._epoch_ms(2026, 11, 19, 23, 44, 0) + 1,
                                          110)
        unsealed = mock.Mock()
        unsealed.in_window.return_value = False
        accepted = tracker_tail._epoch_ms(2026, 11, 19, 23, 44, 0)
        self.assertEqual(tracker_tail._parse_time_ms(str(accepted), run, unsealed), accepted)
        with self.assertRaisesRegex(tracker_tail.TailError, "outside the paired development run"):
            tracker_tail._parse_time_ms(str(accepted + 1), run, unsealed)


if __name__ == "__main__":
    unittest.main()
