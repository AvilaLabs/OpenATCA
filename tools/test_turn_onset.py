"""Synthetic v2 and experimental v3 turn-onset regressions; coordinator runs them."""

import unittest
from unittest.mock import patch
import calendar
import io
import tempfile
import sys
from contextlib import redirect_stderr
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import turn_onset
import sealed_guard


class CountingRaw:
    def __init__(self, data):
        self._stream = io.BytesIO(data)
        self.bytes_read = 0

    def __enter__(self):
        return self

    def __exit__(self, *_exc):
        return False

    def read(self, size=-1):
        block = self._stream.read(size)
        self.bytes_read += len(block)
        return block


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


def v3_ramp_rows(direction=1, initial_course=359.8, end_second=49):
    rows = [(second * 1000, initial_course, 120.0) for second in range(11)]
    course = initial_course
    ramp_rates = (0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7)
    for second in range(11, end_second + 1):
        rate = direction * (ramp_rates[second - 11] if second <= 17 else 0.7)
        course = (course + rate) % 360.0
        rows.append((second * 1000, course, 120.0))
    return rows


class TurnOnsetTests(unittest.TestCase):
    def test_capacity_profiles_preserve_standard_and_bound_extended(self):
        self.assertEqual(turn_onset.budget_limits("v2", False),
                         (250_000, 32 * 1024 * 1024))
        self.assertEqual(turn_onset.budget_limits("v3", False),
                         (250_000, 32 * 1024 * 1024))
        self.assertEqual(turn_onset.budget_limits("v3", True),
                         (1_000_000, 128 * 1024 * 1024))
        with self.assertRaisesRegex(ValueError, "requires --protocol v3"):
            turn_onset.budget_limits("v2", True)

    def test_extended_budget_cli_refuses_before_opening_input(self):
        stderr = io.StringIO()
        with redirect_stderr(stderr):
            status = turn_onset.main([
                "--truth", "missing-truth.csv", "--estimates", "missing-estimates.csv",
                "--extended-budget",
            ])
        self.assertEqual(status, 2)
        self.assertIn("requires --protocol v3", stderr.getvalue())

    def test_extended_limits_load_small_synthetic_truth_without_large_allocation(self):
        development_ms = calendar.timegm((2026, 11, 10, 12, 0, 0)) * 1000
        rows = "leg,time_ms,truth_course_deg,truth_speed_kt\n" + "".join(
            f"leg-1,{development_ms + second * 1000},90,120\n" for second in range(3)
        )
        estimates = "leg,state_time_ms,system,course_deg,availability_ms\n" + "".join(
            f"leg-1,{development_ms + second * 1000},sfdps,90,\n" for second in range(3)
        )
        max_rows, max_bytes = turn_onset.budget_limits("v3", True)
        with tempfile.TemporaryDirectory() as tmp:
            truth_path = Path(tmp) / "small-extended-truth.csv"
            estimates_path = Path(tmp) / "small-extended-estimates.csv"
            truth_path.write_text(rows, encoding="utf-8")
            estimates_path.write_text(estimates, encoding="utf-8")
            loaded_truth = turn_onset.load_truth(
                truth_path, max_rows=max_rows, max_bytes=max_bytes)
            loaded_estimates = turn_onset.load_estimates(
                estimates_path, max_rows=max_rows, max_bytes=max_bytes)
        self.assertEqual(len(loaded_truth["leg-1"]), 3)
        self.assertEqual(len(loaded_estimates[("leg-1", "sfdps")]), 3)

    def test_injected_row_cap_refuses_before_appending_excess_row(self):
        development_ms = calendar.timegm((2026, 11, 10, 12, 0, 0)) * 1000
        rows = "leg,time_ms,truth_course_deg,truth_speed_kt\n" + "".join(
            f"leg-1,{development_ms + second * 1000},90,120\n" for second in range(3)
        )
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "row-cap.csv"
            path.write_text(rows, encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "exceeds 2 rows"):
                turn_onset.load_truth(path, max_rows=2, max_bytes=4096)

    def test_python_parser_bounds_csv_columns_physical_records_and_quoted_newlines(self):
        header = "leg,time_ms,truth_course_deg,truth_speed_kt"
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "many-columns.csv"
            path.write_text(header + "," + ",".join(f"extra{i}" for i in range(13)) + "\n",
                            encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "exceeds 16 columns"):
                turn_onset.load_truth(path, max_rows=10, max_bytes=4096)

            path.write_text(header + "\n" + "leg-1," + "x" * 1030 + ",120\n",
                            encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "exceeds 1024 UTF-8 bytes"):
                turn_onset.load_truth(path, max_rows=10, max_bytes=4096)

            path.write_text(header + '\nleg-1,"90\n120",120\n', encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "malformed single-line record"):
                turn_onset.load_truth(path, max_rows=10, max_bytes=4096)

    def test_guarded_descriptor_reader_probes_at_most_one_byte_past_limit(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "small-stat-but-growing.csv"
            path.write_bytes(b"x")
            source = CountingRaw(b"abcdef")
            with patch.object(sealed_guard, "open_read", return_value=source) as opener:
                with self.assertRaisesRegex(ValueError, "descriptor exceeds 2 byte limit"):
                    list(turn_onset.iter_csv(
                        path, ("a",), (), max_bytes=2, max_rows=1
                    ))
            self.assertEqual(opener.call_args.kwargs, {"buffering": 0})
            self.assertEqual(source.bytes_read, 3)

    def test_caller_cannot_raise_csv_caps_above_extended_hard_maximum(self):
        with self.assertRaisesRegex(ValueError, "row limit"):
            turn_onset.load_truth(Path("unused"), max_rows=turn_onset.EXTENDED_MAX_ROWS + 1)
        with self.assertRaisesRegex(ValueError, "byte limit"):
            turn_onset.load_truth(Path("unused"), max_bytes=turn_onset.EXTENDED_MAX_BYTES + 1)

    def test_v3_gradual_ramp_labels_first_departure_and_alarms_on_third_confirmation(self):
        truth = v3_ramp_rows()
        truth_diag = turn_onset._new_v3_diagnostics()
        events = turn_onset.truth_events(truth, "v3", truth_diag)
        estimates = [(t, c) for t, c, _ in truth]
        detector_diag = turn_onset._new_v3_diagnostics()
        alarms = turn_onset.detect(estimates, "v3", detector_diag)
        self.assertEqual(events, [(13_000, 1)])
        self.assertEqual(alarms, [(17_000, 1)])
        self.assertEqual(truth_diag["transition_confirmed"], 1)
        self.assertEqual(detector_diag["transition_confirmed"], 1)

    def test_v2_default_is_unchanged_on_gradual_ramp(self):
        rows = v3_ramp_rows()
        updates = [(t, c) for t, c, _ in rows]
        self.assertEqual(turn_onset.truth_events(rows), turn_onset.truth_events(rows, "v2"))
        self.assertEqual(turn_onset.truth_events(rows), [])
        self.assertEqual(turn_onset.detect(updates), turn_onset.detect(updates, "v2"))
        self.assertEqual(turn_onset.detect(updates), [])

    def test_v3_straight_baseline_boundary_excludes_departure_interval(self):
        def ramp_after(quiet_seconds):
            rows = [(second * 1000, 0.0, 120.0) for second in range(quiet_seconds + 1)]
            course = 0.0
            for second, rate in enumerate((0.3, 0.5, 0.5, 0.5), quiet_seconds + 1):
                course = (course + rate) % 360.0
                rows.append((second * 1000, course, 120.0))
            return rows

        nine_seconds = ramp_after(9)
        ten_seconds = ramp_after(10)
        self.assertEqual(turn_onset.truth_events(nine_seconds, "v3"), [])
        self.assertEqual(turn_onset.detect([(t, c) for t, c, _ in nine_seconds], "v3"), [])
        self.assertEqual(turn_onset.truth_events(ten_seconds, "v3"), [(11_000, 1)])
        self.assertEqual(turn_onset.detect([(t, c) for t, c, _ in ten_seconds], "v3"),
                         [(14_000, 1)])

    def test_v3_twelve_second_cadence_confirms_three_updates_over_24_seconds(self):
        updates = [(0, 0.0), (12_000, 0.0), (24_000, 0.0),
                   (36_000, 6.0), (48_000, 12.0), (60_000, 18.0)]
        diagnostics = turn_onset._new_v3_diagnostics()
        self.assertEqual(turn_onset.detect(updates, "v3", diagnostics), [(60_000, 1)])
        self.assertEqual(diagnostics["transition_started"], 1)
        self.assertEqual(diagnostics["transition_confirmed"], 1)

    def test_v3_transition_timeout_reverse_quiet_and_invalid_cancel(self):
        timeout_rates = [0.3] * 32
        timeout_rows = [(second * 1000, 0.0, 120.0) for second in range(11)]
        course = 0.0
        for second, rate in enumerate(timeout_rates, 11):
            course = (course + rate) % 360.0
            timeout_rows.append((second * 1000, course, 120.0))
        diag = turn_onset._new_v3_diagnostics()
        self.assertEqual(turn_onset.truth_events(timeout_rows, "v3", diag), [])
        self.assertEqual(diag["transition_cancelled_timeout"], 1)

        reverse_rows = [(second * 1000, 0.0, 120.0) for second in range(11)]
        course = 0.0
        for second, rate in enumerate((0.3, 0.4, -0.3, *([0.0] * 12)), 11):
            course = (course + rate) % 360.0
            reverse_rows.append((second * 1000, course, 120.0))
        diag = turn_onset._new_v3_diagnostics()
        self.assertEqual(turn_onset.truth_events(reverse_rows, "v3", diag), [])
        self.assertEqual(diag["transition_cancelled_reverse"], 1)

        quiet_rows = [(second * 1000, 0.0, 120.0) for second in range(11)]
        course = 0.0
        for second, rate in enumerate((0.3, 0.4, 0.2, *([0.0] * 12)), 11):
            course = (course + rate) % 360.0
            quiet_rows.append((second * 1000, course, 120.0))
        diag = turn_onset._new_v3_diagnostics()
        self.assertEqual(turn_onset.truth_events(quiet_rows, "v3", diag), [])
        self.assertEqual(diag["transition_cancelled_quiet"], 1)

        invalid_rows = v3_ramp_rows(end_second=14)
        invalid_rows[-1] = (invalid_rows[-1][0], invalid_rows[-1][1], 29.9)
        diag = turn_onset._new_v3_diagnostics()
        self.assertEqual(turn_onset.truth_events(invalid_rows, "v3", diag), [])
        self.assertEqual(diag["transition_cancelled_invalid"], 1)

    def test_v3_truth_gap_cancels_unconfirmed_transition(self):
        rows = v3_ramp_rows(end_second=13)
        rows.append((15_000, rows[-1][1] + 0.6, 120.0))
        diag = turn_onset._new_v3_diagnostics()
        self.assertEqual(turn_onset.truth_events(rows, "v3", diag), [])
        self.assertEqual(diag["transition_cancelled_gap"], 1)

    def test_v3_irregular_sfdps_updates_use_elapsed_rates_and_transition_time(self):
        updates = [
            (0, 359.8), (4_000, 359.8), (10_000, 359.8),
            (11_000, 359.9), (12_000, 0.1), (14_000, 0.7),
            (19_000, 2.7), (20_000, 3.2), (22_000, 4.4), (23_000, 5.1),
        ]
        diag = turn_onset._new_v3_diagnostics()
        self.assertEqual(turn_onset.detect(updates, "v3", diag), [(23_000, 1)])
        self.assertEqual(diag["transition_started"], 1)

    def test_v3_active_latch_rearms_after_ten_seconds_straight(self):
        rows = [(second * 1000, 359.8, 120.0) for second in range(11)]
        course = 359.8
        rates = {11: 0.1, 12: 0.2, 13: 0.3, 14: 0.4,
                 15: 0.5, 16: 0.6, 17: 0.7, 18: 0.7, 19: 0.7, 20: 0.7}
        rates.update({second: 0.0 for second in range(21, 31)})
        rates.update({31: 0.3, 32: 0.4, 33: 0.5, 34: 0.6, 35: 0.7})
        for second in sorted(rates):
            course = (course + rates[second]) % 360.0
            rows.append((second * 1000, course, 120.0))
        self.assertEqual(turn_onset.truth_events(rows, "v3"),
                         [(13_000, 1), (31_000, 1)])

    def test_v3_wraps_both_course_directions(self):
        clockwise = v3_ramp_rows(direction=1, initial_course=359.8)
        counterclockwise = v3_ramp_rows(direction=-1, initial_course=0.2)
        self.assertEqual(turn_onset.truth_events(clockwise, "v3"), [(13_000, 1)])
        self.assertEqual(turn_onset.truth_events(counterclockwise, "v3"), [(13_000, -1)])
        self.assertEqual(turn_onset.detect([(t, c) for t, c, _ in clockwise], "v3"),
                         [(17_000, 1)])
        self.assertEqual(turn_onset.detect([(t, c) for t, c, _ in counterclockwise], "v3"),
                         [(17_000, -1)])

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
