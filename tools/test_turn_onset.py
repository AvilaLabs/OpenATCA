"""Synthetic regression source for the preregistered turn-onset metric; coordinator runs it."""

import unittest
from unittest.mock import patch
import calendar
import tempfile
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import turn_onset
import sealed_guard


def turn_rows(direction=1):
    rows = []
    for second in range(50):
        if second <= 10:
            course = 359.0
        elif second <= 15:
            course = (359.0 + direction * 0.6 * (second - 10)) % 360.0
        else:
            course = (359.0 + direction * 3.0) % 360.0
        rows.append((second * 1000, course, 120.0))
    return rows


class TurnOnsetTests(unittest.TestCase):
    def test_signed_rate_truth_label_backdates_but_detector_waits_for_three_updates(self):
        truth = turn_rows()
        estimates = [(t, c) for t, c, _ in truth]
        estimate_rows = [(t, c, None) for t, c in estimates]
        events = turn_onset.truth_events(truth)
        alarms = turn_onset.detect(estimates)
        self.assertEqual(events, [(11_000, 1)])
        self.assertEqual(alarms, [(13_000, 1)])
        result = turn_onset.score(truth, events, estimate_rows, alarms)
        self.assertEqual(result["delays"], [2.0])

    def test_rate_uses_real_output_elapsed_time_and_direction(self):
        updates = [(t * 1000, 0.0) for t in range(11)]
        updates.extend([(11_000, 0.0), (21_000, 6.0), (31_000, 12.0), (41_000, 18.0)])
        self.assertEqual(turn_onset.detect(updates), [(41_000, 1)])

    def test_wrong_direction_cannot_match_and_remains_false_alarm(self):
        truth = turn_rows(direction=1)
        result = turn_onset.score(truth, [(11_000, 1)], [], [(13_000, -1)])
        self.assertEqual(result["matched"], 0)
        self.assertEqual(result["opposite_direction_misses"], 1)
        self.assertEqual(result["false_turns"], 1)
        self.assertEqual(result["never_detected"], 1)

    def test_no_output_is_an_availability_diagnostic_and_a_miss(self):
        truth = turn_rows()
        events = turn_onset.truth_events(truth)
        result = turn_onset.score(truth, events, [], [])
        self.assertEqual(result["missing_output_events"], 1)
        self.assertEqual(result["never_detected"], 1)

    def test_truth_support_uses_the_same_30_knot_gate_as_event_labels(self):
        below_boundary = turn_rows()
        below_boundary[20] = (20_000, below_boundary[20][1], 29.9)
        events = turn_onset.truth_events(below_boundary)
        self.assertEqual(events, [(11_000, 1)])
        below_result = turn_onset.score(below_boundary, events, [], [])
        self.assertEqual(below_result["truth_gaps"], 1)

        at_boundary = turn_rows()
        at_boundary[20] = (20_000, at_boundary[20][1], 30.0)
        events = turn_onset.truth_events(at_boundary)
        self.assertEqual(events, [(11_000, 1)])
        at_result = turn_onset.score(at_boundary, events, [], [])
        self.assertEqual(at_result["truth_gaps"], 0)

    def test_gap_window_censors_alarms_regardless_of_direction(self):
        truth = turn_rows()
        truth[20] = (20_000, truth[20][1], 29.9)
        events = turn_onset.truth_events(truth)
        alarms = [(13_000, 1), (14_000, -1), (42_000, -1)]
        result = turn_onset.score(truth, events, [], alarms)
        self.assertEqual(result["truth_gaps"], 1)
        self.assertEqual(result["gap_associated_alarms"], 2)
        self.assertEqual(result["false_turns"], 1)

    def test_matching_refuses_dense_alarm_work_before_admitting_a_result(self):
        truth = turn_rows()
        alarms = [(13_000 + offset, 1) for offset in range(20)]
        with patch.object(turn_onset, "MAX_MATCH_WORK", 100):
            with self.assertRaisesRegex(ValueError, "bounded work limit"):
                turn_onset.score(truth, [(11_000, 1)], [], alarms)

    def test_course_difference_wraps_across_north(self):
        self.assertEqual(turn_onset.wrap180(1.0 - 359.0), 2.0)

    def test_production_sealed_guard_refuses_synthetic_sealed_row_time(self):
        sealed_ms = calendar.timegm((2026, 11, 21, 12, 0, 0)) * 1000
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "synthetic.csv"
            path.write_text("leg,state_time_ms,system,course_deg,availability_ms\n"
                            f"leg-1,{sealed_ms},oa,90,\n", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "sealed window"):
                turn_onset.load_estimates(path)

    def test_production_sealed_guard_accepts_synthetic_development_time(self):
        development_ms = calendar.timegm((2026, 11, 10, 12, 0, 0)) * 1000
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "synthetic.csv"
            path.write_text("leg,state_time_ms,system,course_deg,availability_ms\n"
                            f"leg-1,{development_ms},oa,90,\n", encoding="utf-8")
            self.assertFalse(sealed_guard.in_sealed_window(development_ms * 1_000_000))
            self.assertEqual(turn_onset.load_estimates(path)[("leg-1", "oa")][0][0], development_ms)

    def test_production_sealed_guard_checks_optional_availability_clock(self):
        development_ms = calendar.timegm((2026, 11, 10, 12, 0, 0)) * 1000
        sealed_ms = calendar.timegm((2026, 11, 21, 12, 0, 0)) * 1000
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "synthetic.csv"
            path.write_text("leg,state_time_ms,system,course_deg,availability_ms\n"
                            f"leg-1,{development_ms},sfdps,90,{sealed_ms}\n", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "sealed window"):
                turn_onset.load_estimates(path)

    def test_production_sealed_guard_uses_half_open_ns_window_for_millisecond_csv(self):
        _, from_ns, to_ns = sealed_guard.production().windows[0]
        first_sealed_ms = (from_ns + 999_999) // 1_000_000
        first_open_ms = to_ns // 1_000_000
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "synthetic.csv"
            path.write_text("leg,state_time_ms,system,course_deg,availability_ms\n"
                            f"leg-1,{first_sealed_ms},oa,90,\n", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "sealed window"):
                turn_onset.load_estimates(path)
            path.write_text("leg,state_time_ms,system,course_deg,availability_ms\n"
                            f"leg-1,{first_open_ms},oa,90,\n", encoding="utf-8")
            self.assertEqual(turn_onset.load_estimates(path)[("leg-1", "oa")][0][0], first_open_ms)

    def test_invalid_required_and_optional_time_cells_remain_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "synthetic.csv"
            for bad in ("not-a-time", "1.5", str(2**63)):
                path.write_text("leg,time_ms,truth_course_deg,truth_speed_kt\n"
                                f"leg-1,{bad},90,120\n", encoding="utf-8")
                with self.assertRaises(ValueError):
                    turn_onset.load_truth(path)
                path.write_text("leg,state_time_ms,system,course_deg,availability_ms\n"
                                f"leg-1,1000,sfdps,90,{bad}\n", encoding="utf-8")
                with self.assertRaises(ValueError):
                    turn_onset.load_estimates(path)

    def test_production_sealed_guard_checks_availability_at_half_open_boundary(self):
        development_ms = calendar.timegm((2026, 11, 10, 12, 0, 0)) * 1000
        _, from_ns, to_ns = sealed_guard.production().windows[0]
        first_sealed_ms = (from_ns + 999_999) // 1_000_000
        first_open_ms = to_ns // 1_000_000
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "synthetic.csv"
            path.write_text("leg,state_time_ms,system,course_deg,availability_ms\n"
                            f"leg-1,{development_ms},sfdps,90,{first_sealed_ms}\n", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "sealed window"):
                turn_onset.load_estimates(path)
            path.write_text("leg,state_time_ms,system,course_deg,availability_ms\n"
                            f"leg-1,{development_ms},sfdps,90,{first_open_ms}\n", encoding="utf-8")
            self.assertEqual(turn_onset.load_estimates(path)[("leg-1", "sfdps")][0][2], first_open_ms)


if __name__ == "__main__":
    unittest.main()
