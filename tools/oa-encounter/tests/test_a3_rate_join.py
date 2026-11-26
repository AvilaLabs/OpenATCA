import sys
import json
import os
import stat
import tempfile
import unittest
from unittest import mock
from pathlib import Path


sys.path.insert(0, str(Path(__file__).parents[1]))
import a3_rate_join as rate  # noqa: E402


def episode(a, b, start, end, cls="C4", ticks=0, flags=frozenset()):
    return rate.Episode(
        rate.canonical_pair(a, b),
        rate.DAY_START + start,
        rate.DAY_START + end,
        cls,
        ticks,
        flags,
    )


def aggregate(alerts, encounters, complete):
    return rate.aggregate(
        alerts,
        encounters,
        complete,
        rate.DAY_START - 600,
        rate.DAY_END + 2640,
    )


class RateJoinTests(unittest.TestCase):
    # REQ: OA-ENC-022
    def test_exact_encounter_class_vocabulary_is_accepted(self):
        for class_name in ("C1", "C2", "C3", "C4", "C5", "C6", "VOLUME_ONLY", "ALT_UNKNOWN"):
            with self.subTest(class_name=class_name):
                result = aggregate(
                    [], [episode("00000001", "00000002", 30, 40, class_name)], True
                )
                hour = result["hours"][0]
                if class_name == "ALT_UNKNOWN":
                    self.assertEqual(hour["encounter_unresolved"], 1)
                    self.assertEqual(hour["encounter_miss"], 0)
                else:
                    self.assertEqual(hour["encounter_miss"], 1)

    # REQ: OA-ENC-022
    def test_unknown_missing_and_typo_encounter_classes_refuse_input(self):
        rows = (
            ("class,ticks_unevaluated\nUNKNOWN,0\n", "unsupported encounter class"),
            ("class,ticks_unevaluated\n,0\n", "missing class"),
            ("class,ticks_unevaluated\nC7,0\n", "unsupported encounter class"),
            ("ticks_unevaluated\n0\n", "unexpected CSV schema"),
        )
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "encounters.csv"
            for csv_text, expected_error in rows:
                with self.subTest(csv_text=csv_text):
                    path.write_text(
                        "address_a_hex,address_b_hex,start_s,end_s," + csv_text.split("\n", 1)[0]
                        + "\n00000001,00000002,0,10,"
                        + (csv_text.split("\n", 1)[1] if "\n" in csv_text else ""),
                        encoding="utf-8",
                    )
                    with self.assertRaisesRegex(rate.InputError, expected_error):
                        rate.read_episodes(path, "encounters")

    # REQ: OA-ENC-022
    def test_direct_aggregate_refuses_invalid_encounter_class_before_labeling(self):
        malformed = episode("00000001", "00000002", 30, 40, "C4")
        malformed = rate.Episode(
            malformed.pair,
            malformed.start,
            malformed.end,
            "C4 typo",
            malformed.ticks_unevaluated,
            malformed.flags,
        )
        with self.assertRaisesRegex(rate.InputError, "unsupported encounter class"):
            aggregate([], [malformed], True)

    # REQ: OA-ENC-022
    def test_summary_counter_map_stops_before_later_numeric_sections(self):
        summary = (
            "shortfalls and counts\n"
            "  rows dropped: 70\n"
            "\nnet state after the end of the run\n"
            "  alerts: 263819\n"
        )
        self.assertEqual(rate.summary_counter_map(summary, "shortfalls and counts"), {"rows dropped": 70})

    # REQ: OA-ENC-022
    def test_coverage_validation_rejects_nonfinite_and_reversed_timestamps(self):
        for start, end in ((float("nan"), 100.0), (0.0, float("inf")), (10.0, 10.0)):
            with self.subTest(start=start, end=end), self.assertRaises(rate.InputError):
                rate.validate_coverage(start, end)

    # REQ: OA-ENC-022
    def test_count_only_diagnostic_reports_bounded_unresolved_counts_without_rates(self):
        digest = "c" * 64
        with tempfile.TemporaryDirectory() as temp:
            private = Path(temp) / "openatca-private"
            private.mkdir(mode=0o700)
            alerts, encounters = private / "alerts.csv", private / "encounters.csv"
            ca, enc = private / "ca.txt", private / "enc.txt"
            out = private / "diagnostic"
            alerts.write_text(
                "address_a_hex,address_b_hex,start_s,end_s,flags,ticks_unevaluated\n"
                f"00000001,00000002,{rate.DAY_START + 120},{rate.DAY_START + 125},,0\n",
                encoding="utf-8",
            )
            encounters.write_text(
                "address_a_hex,address_b_hex,start_s,end_s,class,ticks_unevaluated\n"
                f"00000002,00000001,{rate.DAY_START + 120},{rate.DAY_START + 125},C4,0\n",
                encoding="utf-8",
            )
            ca.write_text(
                f"clip: input.oaf sha256 {digest}\n"
                "default volume: horizontal 5.00 NM, vertical 1000 ft\n"
                "net state: DEGRADED\nshortfalls and counts:\n  inputmissing: 70\n",
                encoding="utf-8",
            )
            enc.write_text(
                f"clip: input.oaf sha256 {digest}\n"
                "record volume: horizontal < 9260.0 m (5.00 NM), vertical < 1000 ft\n"
                "record state: complete (no shortfall)\nwork and overflow:\n  dropped: 0\n",
                encoding="utf-8",
            )
            result = rate.main([
                "--alerts", str(alerts), "--ca-summary", str(ca),
                "--encounters", str(encounters), "--encounter-summary", str(enc),
                "--coverage-start-s", str(rate.DAY_START), "--coverage-end-s", str(rate.DAY_END),
                "--counts-only-diagnostic", "--out", str(out),
            ])
            self.assertEqual(result, 0)
            payload = json.loads((out / "hourly.json").read_text(encoding="utf-8"))
            self.assertEqual(payload["schema"], "openatca-a3-count-only-diagnostic-v1")
            self.assertEqual(payload["ca_run_state"], "DEGRADED_OR_SHORTFALL")
            self.assertEqual(payload["ca_shortfall_counters"], {"inputmissing": 70})
            self.assertEqual(payload["hours"][0]["alert_episode_rows"], 1)
            self.assertEqual(payload["hours"][0]["encounter_episode_rows"], 1)
            self.assertEqual(payload["hours"][0]["unresolved_alert_episode_rows"], 1)
            self.assertEqual(payload["global_classification_status"], "UNRESOLVED_COUNT_ONLY_NO_MATCHING_PERFORMED")
            self.assertNotIn("alert_nuisance", payload["hours"][0])
            self.assertNotIn("rate", " ".join(payload["hours"][0]))
            self.assertNotIn("00000001", (out / "hourly.json").read_text(encoding="utf-8"))

    # REQ: OA-ENC-022
    def test_count_only_diagnostic_refuses_more_than_fixed_row_limit(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "episodes.csv"
            path.write_text(
                "address_a_hex,address_b_hex,start_s,end_s\n"
                f"00000001,00000002,{rate.DAY_START + 120},{rate.DAY_START + 125}\n"
                f"00000001,00000002,{rate.DAY_START + 130},{rate.DAY_START + 135}\n",
                encoding="utf-8",
            )
            with mock.patch.object(rate, "MAX_DIAGNOSTIC_ROWS_PER_INPUT", 1):
                with self.assertRaisesRegex(rate.InputError, "fixed diagnostic row ceiling"):
                    rate.count_episode_hours(path, "alerts", rate.DAY_END)

    # REQ: OA-ENC-022
    def test_closed_interval_endpoint_overlap_is_a_match_and_unordered_pair_matches(self):
        alert = episode("00000002", "00000001", 3600, 3610)
        truth = episode("00000001", "00000002", 3610, 3620, "C4")
        result = aggregate([alert], [truth], True)
        hour = result["hours"][1]
        self.assertEqual(hour["encounter_hit"], 1)
        self.assertEqual(hour["alert_matched"], 1)
        self.assertEqual(hour["alert_nuisance"], 0)

    # REQ: OA-ENC-022
    def test_wrong_pair_does_not_match(self):
        alert = episode("00000001", "00000002", 30, 40)
        truth = episode("00000003", "00000004", 35, 45, "C4")
        result = aggregate([alert], [truth], True)
        self.assertEqual(result["hours"][0]["encounter_miss"], 1)
        self.assertEqual(result["hours"][0]["alert_nuisance"], 1)

    # REQ: OA-ENC-022
    def test_altitude_unknown_truth_and_alert_stay_unresolved(self):
        alert = episode("00000001", "00000002", 30, 40, flags=frozenset({"alt_unknown"}))
        truth = episode("00000001", "00000002", 35, 45, "ALT_UNKNOWN")
        result = aggregate([alert], [truth], True)
        hour = result["hours"][0]
        self.assertEqual(hour["encounter_unknown"], 1)
        self.assertEqual(hour["encounter_known"], 0)
        self.assertEqual(hour["alert_unresolved"], 1)
        self.assertEqual(hour["alert_nuisance"], 0)

    # REQ: OA-ENC-022
    def test_incomplete_run_or_unevaluated_episode_never_creates_negative_claim(self):
        truth = episode("00000001", "00000002", 30, 40, "C4", ticks=1)
        alert = episode("00000001", "00000002", 30, 40, ticks=1)
        result = aggregate([alert], [truth], False)
        hour = result["hours"][0]
        self.assertEqual(hour["encounter_miss"], 0)
        self.assertEqual(hour["encounter_unresolved"], 1)
        self.assertEqual(hour["alert_nuisance"], 0)
        self.assertEqual(hour["alert_unresolved"], 1)
        self.assertIsNone(hour["nuisance_alerts_per_covered_hour"])

        complete = aggregate(
            [episode("00000001", "00000002", 30, 40, ticks=1)],
            [episode("00000001", "00000002", 30, 40, "C4")],
            True,
        )["hours"][0]
        self.assertEqual(complete["encounter_hit"], 0)
        self.assertEqual(complete["encounter_unresolved"], 1)
        self.assertEqual(complete["alert_matched"], 0)
        self.assertEqual(complete["alert_unresolved"], 1)

    # REQ: OA-ENC-022
    def test_episode_onset_is_counted_once_in_utc_half_open_hour(self):
        truth = episode("00000001", "00000002", 3599, 3601, "C4")
        alert = episode("00000001", "00000002", 3599, 3601)
        result = aggregate([alert], [truth], True)
        self.assertEqual(result["hours"][0]["encounter_hit"], 1)
        self.assertEqual(result["hours"][1]["encounter_hit"], 0)
        self.assertEqual(rate.hour_index(rate.DAY_END), None)
        self.assertEqual(rate.hour_index(rate.DAY_START), 0)

    # REQ: OA-ENC-022
    def test_unmatched_alert_is_nuisance_only_with_complete_coverage(self):
        alert = episode("00000001", "00000002", 30, 40)
        complete = aggregate([alert], [], True)["hours"][0]
        incomplete = aggregate([alert], [], False)["hours"][0]
        self.assertEqual(complete["alert_nuisance"], 1)
        self.assertEqual(incomplete["alert_unresolved"], 1)

    # REQ: OA-ENC-022
    def test_clip_without_full_window_and_warmup_makes_negative_claim_unresolved(self):
        alert = episode("00000001", "00000002", 30, 40)
        too_short = rate.aggregate([alert], [], True, rate.DAY_START, rate.DAY_END)
        self.assertFalse(too_short["all_23_hour_bins_fully_covered"])
        self.assertEqual(too_short["hours"][0]["alert_unresolved"], 1)
        self.assertEqual(too_short["hours"][0]["alert_nuisance"], 0)

    # REQ: OA-ENC-022
    def test_missing_preroll_only_affects_near_start_episodes(self):
        near_truth = episode("00000001", "00000002", 30, 40, "C4")
        later_truth = episode("00000003", "00000004", 3600 + 30, 3600 + 40, "C4")
        later_alert = episode("00000003", "00000004", 3600 + 40, 3600 + 50)
        result = rate.aggregate(
            [later_alert],
            [near_truth, later_truth],
            True,
            rate.DAY_START,
            rate.DAY_END + 2640,
        )
        self.assertEqual(result["hours"][0]["encounter_unresolved"], 1)
        self.assertEqual(result["hours"][0]["encounter_miss"], 0)
        self.assertEqual(result["hours"][1]["encounter_hit"], 1)
        self.assertEqual(result["hours"][1]["encounter_unresolved"], 0)
        self.assertEqual(result["hours"][1]["covered_seconds"], 3600)

    # REQ: OA-ENC-022
    def test_right_censoring_is_local_to_the_event_time(self):
        end = rate.DAY_START + 5 * 3600 + 100
        alert = episode("00000001", "00000002", 5 * 3600 + 10, 5 * 3600 + 100)
        result = rate.aggregate([alert], [], True, rate.DAY_START - 300, end)
        hour = result["hours"][5]
        self.assertEqual(hour["alert_right_censored"], 1)
        self.assertEqual(hour["alert_unresolved"], 1)
        self.assertEqual(hour["alert_nuisance"], 0)

    # REQ: OA-ENC-022
    def test_cli_requires_matching_clip_profiles_and_emits_only_aggregate_data(self):
        digest = "a" * 64
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            private = root / "openatca-private"
            private.mkdir(mode=0o700)
            alerts = private / "alerts.csv"
            encounters = private / "encounters.csv"
            ca_summary = private / "ca-summary.txt"
            encounter_summary = private / "encounter-summary.txt"
            out = private / "aggregate"
            alerts.write_text(
                "address_a_hex,address_b_hex,start_s,end_s,flags,ticks_unevaluated\n"
                f"00000001,00000002,{rate.DAY_START + 200},{rate.DAY_START + 210},,0\n",
                encoding="utf-8",
            )
            encounters.write_text(
                "address_a_hex,address_b_hex,start_s,end_s,class,ticks_unevaluated\n"
                f"00000002,00000001,{rate.DAY_START + 200},{rate.DAY_START + 210},C4,0\n",
                encoding="utf-8",
            )
            ca_summary.write_text(
                f"clip: input.oaf sha256 {digest}\n"
                "configuration: cfg sha256 " + "b" * 64 + "\n"
                "default volume: horizontal 5.00 NM, vertical 1000 ft\n"
                "net state: complete (no shortfall)\n",
                encoding="utf-8",
            )
            encounter_summary.write_text(
                f"clip: input.oaf sha256 {digest}\n"
                "record volume: horizontal < 9260.0 m (5.00 NM), vertical < 1000 ft\n"
                "record state: complete (no shortfall)\n",
                encoding="utf-8",
            )
            result = rate.main(
                [
                    "--alerts", str(alerts),
                    "--ca-summary", str(ca_summary),
                    "--encounters", str(encounters),
                    "--encounter-summary", str(encounter_summary),
                    "--coverage-start-s", str(rate.DAY_START - 600),
                    "--coverage-end-s", str(rate.DAY_END + 2640),
                    "--out", str(out),
                ]
            )
            self.assertEqual(result, 0)
            published = (out / "hourly.json").read_text(encoding="utf-8")
            self.assertNotIn("00000001", published)
            self.assertNotIn("00000002", published)
            parsed = json.loads(published)
            self.assertEqual(parsed["hours"][0]["encounter_hit"], 1)
            self.assertEqual(stat.S_IMODE((out / "hourly.json").stat().st_mode), 0o600)
            self.assertEqual(stat.S_IMODE(out.stat().st_mode), 0o700)
            self.assertEqual(result, 0)
            with self.assertRaises(rate.InputError):
                rate._preflight_private_output(out)

    # REQ: OA-ENC-022
    def test_episode_csv_guard_requires_regular_file(self):
        with tempfile.TemporaryDirectory() as temp:
            fifo = Path(temp) / "alerts.csv"
            os.mkfifo(fifo, 0o600)
            with self.assertRaises(rate.InputError):
                rate.read_episodes(fifo, "alerts")

    # REQ: OA-ENC-022
    def test_episode_interval_guard_rejects_holdout_overlap_including_closed_boundary(self):
        guard = rate.sealed_guard.production()
        self.assertTrue(guard.windows)
        _, start, end = guard.windows[0]
        start_s = start / 1_000_000_000
        self.assertTrue(rate.interval_intersects_sealed_window(start_s - 1, start_s))
        self.assertTrue(rate.interval_intersects_sealed_window(start_s, start_s + 1))
        self.assertFalse(rate.interval_intersects_sealed_window(start_s - 2, start_s - 1))

    # REQ: OA-ENC-022
    def test_degraded_summary_keeps_clip_digest_and_mixed_runs_stay_unresolved(self):
        digest = "a" * 64
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            ca = root / "ca-summary.txt"
            encounter = root / "encounter-summary.txt"
            ca.write_text(
                f"clip: same.oaf sha256 {digest}\nnet state: DEGRADED at some cycle\n",
                encoding="utf-8",
            )
            encounter.write_text(
                f"clip: same.oaf sha256 {digest}\nrecord state: complete (no shortfall)\n",
                encoding="utf-8",
            )
            ca_complete, ca_clip = rate.summary_state(ca, "CA")
            encounter_complete, encounter_clip = rate.summary_state(encounter, "encounter")
        self.assertFalse(ca_complete)
        self.assertTrue(encounter_complete)
        self.assertEqual(ca_clip, encounter_clip)
        alert = episode("00000001", "00000002", 200, 210)
        truth = episode("00000001", "00000002", 200, 210, "C4")
        result = aggregate([alert], [truth], ca_complete and encounter_complete)
        hour = result["hours"][0]
        self.assertEqual(hour["encounter_hit"], 0)
        self.assertEqual(hour["encounter_unresolved"], 1)
        self.assertEqual(hour["alert_unresolved"], 1)

    # REQ: OA-ENC-022
    def test_different_degraded_clip_digests_never_alias(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            ca = root / "ca-summary.txt"
            encounter = root / "encounter-summary.txt"
            ca.write_text(f"clip: one.oaf sha256 {'a' * 64}\nnet state: DEGRADED\n", encoding="utf-8")
            encounter.write_text(f"clip: two.oaf sha256 {'b' * 64}\nrecord state: DEGRADED\n", encoding="utf-8")
            ca_complete, ca_clip = rate.summary_state(ca, "CA")
            encounter_complete, encounter_clip = rate.summary_state(encounter, "encounter")
        self.assertFalse(ca_complete)
        self.assertFalse(encounter_complete)
        self.assertNotEqual(ca_clip, encounter_clip)


if __name__ == "__main__":
    unittest.main()
