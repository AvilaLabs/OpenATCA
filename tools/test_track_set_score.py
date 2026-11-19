"""Independent hand cases and brute-force checks for GOSPA set scoring."""

from __future__ import annotations

import csv
import hashlib
import json
import math
import random
import tempfile
import unittest
from pathlib import Path

import track_set_score as scorer


TRUTH_FIELDS = scorer.TRUTH_FIELDS
OUTPUT_FIELDS = scorer.OUTPUT_FIELDS
STEP_NS = 1_000_000_000


def _row(fields: tuple[str, ...], values: dict[str, object]) -> dict[str, str]:
    return {field: str(values[field]) for field in fields}


def write_case(
    root: Path,
    truths: list[dict[str, object]],
    outputs: list[dict[str, object]],
    *,
    ticks: int = 1,
    expected_per_tick: int | None = None,
    trace_valid: bool = True,
) -> tuple[Path, Path, Path]:
    scenario = "mixed-radar1k"
    truth_path, output_path, manifest_path = root / "truth.csv", root / "outputs.csv", root / "manifest.json"
    with truth_path.open("w", newline="", encoding="utf-8") as stream:
        writer = csv.DictWriter(stream, fieldnames=TRUTH_FIELDS, lineterminator="\n")
        writer.writeheader()
        writer.writerows(_row(TRUTH_FIELDS, row) for row in truths)
    with output_path.open("w", newline="", encoding="utf-8") as stream:
        writer = csv.DictWriter(stream, fieldnames=OUTPUT_FIELDS, lineterminator="\n")
        writer.writeheader()
        writer.writerows(_row(OUTPUT_FIELDS, row) for row in outputs)
    if expected_per_tick is None:
        expected_per_tick = len(truths) // ticks
    manifest = {
        "schema_version": 1,
        "scenario_id": scenario,
        "config": {"seed": 3, "n": expected_per_tick, "layout": "test", "radars": 1,
                   "adsb_frac": 0.0, "warmup_s": 0, "duration_s": ticks - 1,
                   "horizon_s": 0, "tracker_capacity": 2000, "bucket_ns": STEP_NS},
        "sources": {"repository_commit": "e" * 40, "generator": {"path": "gen.py", "sha256": "a" * 64},
                    "fusion": {"path": "fusion.rs", "sha256": "b" * 64},
                    "trace_runner": {"path": "trace.rs", "sha256": "c" * 64},
                    "input_hasher": {"path": "sha256.rs", "sha256": "d" * 64},
                    "truth_implementation": "analytic latent truth"},
        "fingerprint_schema": "oa-fusion-input-fields-le-v1",
        "input_fingerprint": "1" * 64,
        "input_count": 0,
        "measured_input_fingerprint": "2" * 64,
        "measured_input_count": 0,
        "grid": {"start_ns": 0, "end_ns": (ticks - 1) * STEP_NS,
                 "step_ns": STEP_NS, "tick_count": ticks},
        "output_snapshots": [
            {"time_ns": tick * STEP_NS, "count": sum(1 for row in outputs if int(row["time_ns"]) == tick * STEP_NS)}
            for tick in range(ticks)
        ],
        "expected_truth_rows_per_tick": expected_per_tick,
        "expected_truth_total": expected_per_tick * ticks,
        "truth_snapshots": [
            {"time_ns": tick * STEP_NS, "count": sum(1 for row in truths if int(row["time_ns"]) == tick * STEP_NS)}
            for tick in range(ticks)
        ],
        "truth_file": {"name": truth_path.name, "sha256": hashlib.sha256(truth_path.read_bytes()).hexdigest(), "rows": len(truths)},
        "output_file": {"name": output_path.name, "sha256": hashlib.sha256(output_path.read_bytes()).hexdigest(), "rows": len(outputs)},
        "accounting": {
            "trace_valid": trace_valid,
            "strict_processing_pass": trace_valid,
            "export_valid": True,
            "emitted_output_rows": len(outputs),
            "drain_output_rows": 0, "drain_output_overflow": 0,
            "pending_adsb_before_finish": 0, "pending_plots_before_finish": 0,
            "empty_ticks": sum(1 for tick in range(ticks) if not any(int(row["time_ns"]) == tick * STEP_NS for row in outputs)),
            "pending_overflow": 0, "work_refused": 0, "work_execution_fault": 0,
            "work_faulted_inputs": 0, "work_faults": 0, "output_overflow": 0,
            "prediction_failures": 0, "prediction_refused": 0, "invalid_truth": 0,
            "input_reconciled": True, "work_counter_overflowed": False, "work_faulted": False,
            "tracker_work_counter_overflowed": False, "adsb_in": 0, "plots_in": 0,
            "processed_events": 0, "offered_adsb": 0, "offered_plots": 0,
            "measured_offered_adsb": 0, "measured_offered_plots": 0, "applied_adsb": None,
            "applied_hard": 0, "applied_assoc": 0, "applied_plots": 0, "held": 0, "unassigned": 0,
            "late": 0, "plots_refused": 0, "adsb_address_refused": 0,
            "bucket_plot_overflow": 0, "bucket_work_capped": 0, "assoc_refused": 0,
            "init_plots_refused": 0, "scratch_full": 0, "wide_full": 0, "init_wide_full": 0,
            "track_refused": 0, "track_rec_overflow": 0, "keyinfo_overflow": 0,
            "assign_log_overflow": 0, "plot_scan_full": 0, "work_bound_overflow": 0,
            "work_ingest_refused_inputs": 0, "tracker_work_refused": 0,
            "tracker_refused": 0, "unrepresentable_rows": 0, "tracker_work_counter_overflow": 0,
            "tracker_rejected": 0, "tracker_late_window": 0, "tracker_late_ring": 0,
            "tracker_jump_rejected": 0, "tracker_numerical_refused": 0, "tracker_capacity_refused": 0,
            "tracker_duplicates": 0, "tracker_time_held": 0, "tracker_time_held_applied": 0,
            "tracker_time_future_dropped": 0,
            "time_held": 0, "time_held_applied": 0, "time_future_dropped": 0,
            "tracker_offered": 0, "tracker_applied": 0,
            "tracker_work_refused": 0,
            "pending_adsb_end": 0, "pending_plots_end": 0,
        },
    }
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    return truth_path, output_path, manifest_path


def truth(time_ns: int, truth_id: str, lat: float, lon: float) -> dict[str, object]:
    return {"scenario_id": "mixed-radar1k", "time_ns": time_ns, "truth_id": truth_id, "lat_deg": lat, "lon_deg": lon}


def output(time_ns: int, track_id: str, lat: float, lon: float, status: str = "valid") -> dict[str, object]:
    return {"scenario_id": "mixed-radar1k", "time_ns": time_ns, "track_id": track_id,
            "source_ns": time_ns, "lat_deg": lat, "lon_deg": lon, "status": status}


def independent_chord_squared(lat1: float, lon1: float, lat2: float, lon2: float) -> float:
    """Test-only WGS84 ECEF calculation, separate from the scorer implementation."""
    a, f = 6_378_137.0, 1 / 298.257223563
    e2 = f * (2.0 - f)

    def xyz(lat: float, lon: float) -> tuple[float, float, float]:
        phi, lam = math.radians(lat), math.radians(lon)
        n = a / math.sqrt(1.0 - e2 * math.sin(phi) ** 2)
        return (n * math.cos(phi) * math.cos(lam), n * math.cos(phi) * math.sin(lam),
                n * (1.0 - e2) * math.sin(phi))

    first, second = xyz(lat1, lon1), xyz(lat2, lon2)
    return sum((first[i] - second[i]) ** 2 for i in range(3))


def brute_gospa(truth_positions: list[float], output_positions: list[float]) -> float:
    """Enumerate every partial one-to-one map; no Hungarian/components are used."""
    n, m = len(truth_positions), len(output_positions)
    best = math.inf

    def visit(i: int, used: set[int], matched_cost: float, match_count: int) -> None:
        nonlocal best
        if i == n:
            missed = n - match_count
            false = m - match_count
            cost = matched_cost + scorer.CUTOFF_SQUARED_M2 / 2.0 * (missed + false)
            best = min(best, cost)
            return
        visit(i + 1, used, matched_cost, match_count)
        for j in range(m):
            if j in used:
                continue
            distance2 = independent_chord_squared(0.0, truth_positions[i], 0.0, output_positions[j])
            if distance2 < scorer.CUTOFF_SQUARED_M2:
                used.add(j)
                visit(i + 1, used, matched_cost + distance2, match_count + 1)
                used.remove(j)

    visit(0, set(), 0.0, 0)
    return best


class TrackSetScoreTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)

    def tearDown(self) -> None:
        self.temp.cleanup()

    def run_case(self, truths: list[dict[str, object]], outputs: list[dict[str, object]], **kwargs: object) -> dict:
        return scorer.score(*write_case(self.root, truths, outputs, **kwargs))

    # REQ: OA-ACC-050
    def test_cli_exposes_standalone_artifact_arguments(self) -> None:
        import subprocess
        import sys

        result = subprocess.run([sys.executable, str(Path(scorer.__file__)), "--help"],
                                check=False, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0)
        for argument in ("--truth", "--outputs", "--manifest", "--out"):
            self.assertIn(argument, result.stdout)

    # REQ: OA-ACC-062, OA-ACC-070, OA-ACC-075, OA-ACC-076, OA-ACC-077, OA-ACC-078
    def test_hand_derived_empty_miss_and_far_costs(self) -> None:
        empty = self.run_case([], [], ticks=2, expected_per_tick=0)
        self.assertEqual(empty["status"], "PASS")
        self.assertEqual(empty["aggregate"]["gospa_cost_m2"], 0.0)

        one_truth = [truth(0, "T", 0.0, 0.0)]
        missed = self.run_case(one_truth, [])
        self.assertEqual(missed["aggregate"]["missed_truth_count"], 1)
        self.assertEqual(missed["aggregate"]["missed_truth_cost_m2"], 500_000.0)
        self.assertAlmostEqual(missed["aggregate"]["gospa_m_total_cost_root"], math.sqrt(500_000.0))

        far = self.run_case(one_truth, [output(0, "X", 0.0, 0.02)])
        self.assertEqual(far["aggregate"]["match_count"], 0)
        self.assertEqual(far["aggregate"]["missed_truth_count"], 1)
        self.assertEqual(far["aggregate"]["false_or_extra_track_count"], 1)
        self.assertEqual(far["aggregate"]["gospa_cost_m2"], 1_000_000.0)

    # REQ: OA-ACC-050, OA-ACC-054, OA-ACC-055
    def test_file_hashes_and_manifest_schema_are_bound(self) -> None:
        paths = write_case(self.root, [truth(0, "T", 0.0, 0.0)], [output(0, "X", 0.0, 0.0)])
        passed = scorer.score(*paths)
        self.assertEqual(passed["status"], "PASS")
        self.assertEqual(passed["hashes"]["truth_sha256"], hashlib.sha256(paths[0].read_bytes()).hexdigest())
        self.assertEqual(passed["hashes"]["outputs_sha256"], hashlib.sha256(paths[1].read_bytes()).hexdigest())
        manifest = json.loads(paths[2].read_text())
        manifest["output_file"]["sha256"] = "0" * 64
        paths[2].write_text(json.dumps(manifest))
        bad_hash = scorer.score(*paths)
        self.assertEqual(bad_hash["status"], "FAIL")
        self.assertIn("SHA-256", bad_hash["reason"])

        manifest["output_file"]["sha256"] = hashlib.sha256(paths[1].read_bytes()).hexdigest()
        manifest["input_fingerprint"] = "bad"
        paths[2].write_text(json.dumps(manifest))
        bad_fingerprint = scorer.score(*paths)
        self.assertEqual(bad_fingerprint["status"], "UNQUALIFIED")
        self.assertNotIn("aggregate", bad_fingerprint)

        manifest["input_fingerprint"] = "1" * 64
        manifest["schema_version"] = 2
        paths[2].write_text(json.dumps(manifest))
        unknown = scorer.score(*paths)
        self.assertEqual(unknown["status"], "UNQUALIFIED")

    # REQ: OA-ACC-053, OA-ACC-063, OA-ACC-067, OA-ACC-091
    def test_duplicate_truth_and_future_source_are_excluded_and_uncertified(self) -> None:
        duplicate_truths = [truth(0, "T", 0.0, 0.0), truth(0, "T", 0.0, 0.0)]
        paths = write_case(self.root, duplicate_truths, [], expected_per_tick=1)
        manifest = json.loads(paths[2].read_text())
        manifest["truth_snapshots"][0]["count"] = 1
        manifest["truth_file"]["rows"] = 2
        manifest["expected_truth_total"] = 1
        paths[2].write_text(json.dumps(manifest))
        duplicate = scorer.score(*paths)
        self.assertEqual(duplicate["status"], "UNQUALIFIED")
        self.assertEqual(duplicate["counts"]["duplicate_truth_rows"], 1)

        future_row = output(0, "X", 0.0, 0.0)
        future_row["source_ns"] = STEP_NS
        paths = write_case(self.root, [truth(0, "T", 0.0, 0.0)], [future_row])
        with paths[1].open("w", newline="", encoding="utf-8") as stream:
            writer = csv.DictWriter(stream, fieldnames=OUTPUT_FIELDS, lineterminator="\n")
            writer.writeheader()
            writer.writerow(_row(OUTPUT_FIELDS, future_row))
        manifest = json.loads(paths[2].read_text())
        manifest["output_file"]["sha256"] = hashlib.sha256(paths[1].read_bytes()).hexdigest()
        paths[2].write_text(json.dumps(manifest))
        future = scorer.score(*paths)
        self.assertEqual(future["status"], "UNQUALIFIED")
        self.assertEqual(future["counts"]["malformed_output_rows"], 1)

    # REQ: OA-ACC-057, OA-ACC-058, OA-ACC-060, OA-ACC-061, OA-ACC-063, OA-ACC-068, OA-ACC-087, OA-ACC-089
    def test_duplicate_and_empty_output_tick_are_counted(self) -> None:
        truths = [truth(tick * STEP_NS, f"T{tick}", 0.0, tick * 0.0001) for tick in range(3)]
        outputs = [output(0, "X", 0.0, 0.0), output(2 * STEP_NS, "Y", 0.0, 0.0002)]
        outputs.append(output(2 * STEP_NS, "Y", 0.0, 0.0002))
        result = self.run_case(truths, outputs, ticks=3, expected_per_tick=1)
        self.assertEqual(result["status"], "UNQUALIFIED")
        self.assertEqual(result["counts"]["duplicate_output_rows"], 1)
        self.assertEqual(result["counts"]["output_ticks_absent"], 1)
        self.assertEqual(result["counts"]["intentional_empty_output_ticks"], 1)
        self.assertNotIn("aggregate", result)

    # REQ: OA-ACC-058, OA-ACC-059, OA-ACC-061, OA-ACC-089, OA-ACC-090
    def test_unmarked_absent_output_tick_is_not_scored_as_misses(self) -> None:
        truths = [truth(tick * STEP_NS, f"T{tick}", 0.0, tick * 0.0001) for tick in range(2)]
        outputs = [output(0, "X", 0.0, 0.0)]
        paths = write_case(self.root, truths, outputs, ticks=2, expected_per_tick=1)
        manifest = json.loads(paths[2].read_text())
        manifest["output_snapshots"][1]["count"] = 1
        manifest["accounting"]["emitted_output_rows"] = 2
        paths[2].write_text(json.dumps(manifest))
        result = scorer.score(*paths)
        self.assertEqual(result["status"], "UNQUALIFIED")
        self.assertEqual(result["counts"]["output_inventory_mismatched_ticks"], 1)
        self.assertNotIn("aggregate", result)

    # REQ: OA-ACC-062, OA-ACC-066, OA-ACC-076
    def test_zero_truth_with_extra_track_and_coast_rows_are_scored_as_outputs(self) -> None:
        result = self.run_case([], [output(0, "coasted", 10.0, 20.0, "coast")], expected_per_tick=0)
        self.assertEqual(result["status"], "PASS")
        self.assertEqual(result["aggregate"]["truth_count"], 0)
        self.assertEqual(result["aggregate"]["false_or_extra_track_count"], 1)
        self.assertEqual(result["snapshots"][0]["false_or_extra_track_cost_m2"], 500_000.0)

    # REQ: OA-ACC-074, OA-ACC-084, OA-ACC-085, OA-ACC-086, OA-ACC-088, OA-ACC-099
    def test_swapped_set_is_localized_by_minimum_assignment_and_reports_changes(self) -> None:
        truths = [truth(0, "A", 0.0, 0.0), truth(0, "B", 0.0, 0.001),
                  truth(STEP_NS, "A", 0.0, 0.001), truth(STEP_NS, "B", 0.0, 0.0)]
        outputs = [output(0, "track-1", 0.0, 0.0), output(0, "track-2", 0.0, 0.001),
                   output(STEP_NS, "track-1", 0.0, 0.0), output(STEP_NS, "track-2", 0.0, 0.001)]
        result = self.run_case(truths, outputs, ticks=2, expected_per_tick=2)
        self.assertEqual(result["status"], "PASS")
        self.assertEqual(result["aggregate"]["missed_truth_count"], 0)
        self.assertEqual(result["aggregate"]["false_or_extra_track_count"], 0)
        self.assertEqual(result["association_changes"]["count"], 4)
        self.assertTrue(all(change["certification"] == "descriptive_not_certified_unique" for change in result["association_changes"]["events"]))
        self.assertFalse(result["identity_claim"])
        self.assertEqual(result["snapshots"][0]["global_assignment_ambiguity"], "NOT_ASSESSED; deterministic Hungarian representative selected when optima tie")

    # REQ: OA-ACC-081, OA-ACC-082, OA-ACC-083
    def test_tied_minimum_is_deterministic_and_counted(self) -> None:
        truths = [truth(0, "T", 0.0, 0.0)]
        outputs = [output(0, "a", 0.0, 0.0001), output(0, "b", 0.0, -0.0001)]
        first = self.run_case(truths, outputs)
        second = self.run_case(truths, outputs)
        self.assertEqual(first["snapshots"][0]["matches"][0]["track_id"], "a")
        self.assertEqual(first["snapshots"][0]["ambiguous_local_nearest_tie_node_count"], 1)
        self.assertEqual(first["snapshots"][0]["matches"], second["snapshots"][0]["matches"])

    # REQ: OA-ACC-071, OA-ACC-072, OA-ACC-073
    def test_cutoff_nextafter_and_negative_cell_boundary(self) -> None:
        delta_rad = 2.0 * math.asin(scorer.CUTOFF_M / (2.0 * scorer.WGS84_A_M))
        cutoff_lon = math.degrees(delta_rad)
        just_inside = math.nextafter(cutoff_lon, 0.0)
        just_outside = math.nextafter(cutoff_lon, math.inf)
        inside = self.run_case([truth(0, "T", 0.0, 0.0)], [output(0, "O", 0.0, just_inside)])
        outside = self.run_case([truth(0, "T", 0.0, 0.0)], [output(0, "O", 0.0, just_outside)])
        self.assertEqual(inside["aggregate"]["match_count"], 1)
        self.assertEqual(outside["aggregate"]["match_count"], 0)

        cell_edge_y = -scorer.SPATIAL_CELL_WIDTH_M
        lon1 = math.degrees(math.asin((cell_edge_y - 0.001) / scorer.WGS84_A_M))
        lon2 = math.degrees(math.asin((cell_edge_y + 999.998) / scorer.WGS84_A_M))
        across_negative_cells = self.run_case([truth(0, "T", 0.0, lon1)], [output(0, "O", 0.0, lon2)])
        self.assertEqual(across_negative_cells["aggregate"]["match_count"], 1)

        dateline_near = self.run_case([truth(0, "T", 0.0, 179.999)], [output(0, "O", 0.0, -179.999)])
        self.assertEqual(dateline_near["aggregate"]["match_count"], 1)
        antipodal = self.run_case([truth(0, "T", 0.0, 0.0)], [output(0, "O", 0.0, 180.0)])
        self.assertEqual(antipodal["aggregate"]["match_count"], 0)

    # REQ: OA-ACC-051, OA-ACC-053, OA-ACC-064, OA-ACC-065, OA-ACC-068, OA-ACC-069
    def test_malformed_duplicate_identity_and_bad_timestamp_are_excluded_and_counted(self) -> None:
        truth_rows = [truth(0, "T", 0.0, 0.0)]
        output_rows = [output(0, "X", 0.0, 0.0), output(0, "X", 0.0, 0.0)]
        paths = write_case(self.root, truth_rows, output_rows)
        with paths[1].open("a", encoding="utf-8") as stream:
            stream.write("mixed-radar1k,0,bad,0,not-a-lat,0,valid\n")
        manifest = json.loads(paths[2].read_text())
        manifest["output_file"]["sha256"] = hashlib.sha256(paths[1].read_bytes()).hexdigest()
        manifest["output_file"]["rows"] += 1
        manifest["accounting"]["emitted_output_rows"] += 1
        manifest["output_snapshots"][0]["count"] += 1
        paths[2].write_text(json.dumps(manifest))
        result = scorer.score(*paths)
        self.assertEqual(result["status"], "UNQUALIFIED")
        self.assertEqual(result["counts"]["duplicate_output_rows"], 1)
        self.assertEqual(result["counts"]["malformed_output_rows"], 1)
        self.assertEqual(result["counts"]["excluded_output_rows"], 2)

        # Non-finite and out-of-range coordinates are also excluded.
        paths = write_case(self.root, truth_rows, [output(0, "X", float("nan"), 0.0)])
        bad = scorer.score(*paths)
        self.assertEqual(bad["status"], "UNQUALIFIED")
        self.assertEqual(bad["counts"]["malformed_output_rows"], 1)

    # REQ: OA-ACC-074, OA-ACC-098
    def test_optimal_assignment_beats_greedy_nearest_neighbour(self) -> None:
        # Truth at 0 m and 600 m, outputs at 400 m and 1200 m on one parallel. Greedy takes the
        # globally nearest pair (600 m truth with 400 m output, 200 m apart) and strands both others.
        to_lon = lambda metres: math.degrees(metres / scorer.WGS84_A_M)
        truth_lons = [to_lon(0.0), to_lon(600.0)]
        output_lons = [to_lon(400.0), to_lon(1200.0)]
        left = [scorer.Point("s", 0, f"T{i}", 0.0, lon) for i, lon in enumerate(truth_lons)]
        right = [scorer.Point("s", 0, f"O{j}", 0.0, lon) for j, lon in enumerate(output_lons)]
        observed = scorer._assign_snapshot(left, right, [0], [0], [0])["gospa_cost_m2"]
        optimum = 400.0 ** 2 + 600.0 ** 2
        greedy = 200.0 ** 2 + scorer.CUTOFF_SQUARED_M2
        self.assertAlmostEqual(observed, optimum, delta=1.0)
        self.assertLess(observed, greedy)
        self.assertAlmostEqual(observed, brute_gospa(truth_lons, output_lons), delta=1e-5)

    # REQ: OA-ACC-074, OA-ACC-098
    def test_assignment_matches_bruteforce_on_seeded_random_instances_up_to_five_by_five(self) -> None:
        rng = random.Random(7)
        for case in range(50):
            n, m = rng.randint(0, 5), rng.randint(0, 5)
            truth_lons = [math.degrees(rng.uniform(0.0, 2500.0) / scorer.WGS84_A_M) for _ in range(n)]
            output_lons = [math.degrees(rng.uniform(0.0, 2500.0) / scorer.WGS84_A_M) for _ in range(m)]
            left = [scorer.Point("s", 0, f"T{i}", 0.0, lon) for i, lon in enumerate(truth_lons)]
            right = [scorer.Point("s", 0, f"O{j}", 0.0, lon) for j, lon in enumerate(output_lons)]
            observed = scorer._assign_snapshot(left, right, [0], [0], [0])["gospa_cost_m2"]
            self.assertAlmostEqual(observed, brute_gospa(truth_lons, output_lons), delta=1e-4, msg=f"case {case} n={n} m={m}")

    # REQ: OA-ACC-074, OA-ACC-078, OA-ACC-079, OA-ACC-080, OA-ACC-098
    def test_assignment_matches_bruteforce_for_all_sizes_through_four(self) -> None:
        longitude_choices = (-0.02, -0.004, 0.0, 0.003, 0.02)
        for n in range(5):
            for m in range(5):
                truth_lons = list(longitude_choices[:n])
                output_lons = list(longitude_choices[4 - m:]) if m else []
                left = [scorer.Point("s", 0, f"T{i}", 0.0, lon) for i, lon in enumerate(truth_lons)]
                right = [scorer.Point("s", 0, f"O{j}", 0.0, lon) for j, lon in enumerate(output_lons)]
                observed = scorer._assign_snapshot(left, right, [0], [0], [0])["gospa_cost_m2"]
                expected = brute_gospa(truth_lons, output_lons)
                self.assertAlmostEqual(observed, expected, delta=1e-5, msg=f"n={n},m={m}")

    # REQ: OA-ACC-052, OA-ACC-094, OA-ACC-095, OA-ACC-096, OA-ACC-097
    def test_component_and_work_caps_return_unqualified_without_approximation(self) -> None:
        truths = [truth(0, "A", 0.0, 0.0), truth(0, "B", 0.0, 0.0001)]
        outputs = [output(0, "x", 0.0, 0.0), output(0, "y", 0.0, 0.0001)]
        original = scorer.MAX_COMPONENT_SIDE
        scorer.MAX_COMPONENT_SIDE = 1
        try:
            result = self.run_case(truths, outputs, expected_per_tick=2)
        finally:
            scorer.MAX_COMPONENT_SIDE = original
        self.assertEqual(result["status"], "UNQUALIFIED")
        self.assertIn("exceeds", result["reason"])

        original_edges = scorer.MAX_CANDIDATE_EDGES_PER_TICK
        scorer.MAX_CANDIDATE_EDGES_PER_TICK = 1
        try:
            edge_cap = self.run_case(truths, outputs, expected_per_tick=2)
        finally:
            scorer.MAX_CANDIDATE_EDGES_PER_TICK = original_edges
        self.assertEqual(edge_cap["status"], "UNQUALIFIED")
        self.assertIn("candidate edge count", edge_cap["reason"])

        original_work = scorer.MAX_TOTAL_HUNGARIAN_CUBE_WORK
        scorer.MAX_TOTAL_HUNGARIAN_CUBE_WORK = 0
        try:
            work_cap = self.run_case(truths[:1], outputs[:1])
        finally:
            scorer.MAX_TOTAL_HUNGARIAN_CUBE_WORK = original_work
        self.assertEqual(work_cap["status"], "UNQUALIFIED")
        self.assertIn("assignment work", work_cap["reason"])

        original_checks = scorer.MAX_NEIGHBOR_CHECKS
        scorer.MAX_NEIGHBOR_CHECKS = 0
        try:
            search_cap = self.run_case(truths[:1], outputs[:1])
        finally:
            scorer.MAX_NEIGHBOR_CHECKS = original_checks
        self.assertEqual(search_cap["status"], "UNQUALIFIED")
        self.assertIn("spatial candidate checks", search_cap["reason"])

        original_total_edges = scorer.MAX_CANDIDATE_EDGES_TOTAL
        scorer.MAX_CANDIDATE_EDGES_TOTAL = 1
        two_ticks_truth = [truth(0, "A", 0.0, 0.0), truth(STEP_NS, "B", 0.0, 0.0)]
        two_ticks_outputs = [output(0, "x", 0.0, 0.0), output(STEP_NS, "y", 0.0, 0.0)]
        try:
            run_edge_cap = self.run_case(two_ticks_truth, two_ticks_outputs, ticks=2, expected_per_tick=1)
        finally:
            scorer.MAX_CANDIDATE_EDGES_TOTAL = original_total_edges
        self.assertEqual(run_edge_cap["status"], "UNQUALIFIED")
        self.assertIn("run candidate edge count", run_edge_cap["reason"])

    # REQ: OA-ACC-056, OA-ACC-092, OA-ACC-093
    def test_input_reconciliation_and_loss_counters_gate_trace(self) -> None:
        paths = write_case(self.root, [truth(0, "T", 0.0, 0.0)], [output(0, "X", 0.0, 0.0)])
        manifest = json.loads(paths[2].read_text())
        manifest["accounting"]["offered_plots"] = 1
        manifest["accounting"]["measured_offered_plots"] = 1
        manifest["accounting"]["plots_in"] = 0
        manifest["input_count"] = 1
        manifest["measured_input_count"] = 1
        paths[2].write_text(json.dumps(manifest))
        inconsistent = scorer.score(*paths)
        self.assertEqual(inconsistent["status"], "UNQUALIFIED_PROCESSING")
        self.assertFalse(inconsistent["processing_quality_pass"])
        self.assertIn("aggregate", inconsistent)
        self.assertTrue(any("reconcile" in reason for reason in inconsistent["processing_failure_reasons"]))

        paths = write_case(self.root, [truth(0, "T", 0.0, 0.0)], [output(0, "X", 0.0, 0.0)])
        manifest = json.loads(paths[2].read_text())
        manifest["accounting"]["late"] = 1
        paths[2].write_text(json.dumps(manifest))
        late = scorer.score(*paths)
        self.assertEqual(late["status"], "UNQUALIFIED_PROCESSING")
        self.assertEqual(late["trace_failure_counters"]["late"], 1)
        self.assertIn("aggregate", late)

        paths = write_case(self.root, [truth(0, "T", 0.0, 0.0)], [output(0, "X", 0.0, 0.0)], trace_valid=False)
        diagnostic = scorer.score(*paths)
        self.assertEqual(diagnostic["status"], "UNQUALIFIED_PROCESSING")
        self.assertFalse(diagnostic["processing_quality_pass"])
        self.assertIn("aggregate", diagnostic)

        paths = write_case(self.root, [truth(0, "T", 0.0, 0.0)], [output(0, "X", 0.0, 0.0)])
        manifest = json.loads(paths[2].read_text())
        manifest["accounting"]["prediction_failures"] = 1
        paths[2].write_text(json.dumps(manifest))
        invalid_export = scorer.score(*paths)
        self.assertEqual(invalid_export["status"], "UNQUALIFIED")
        self.assertIn("export_failure_counters", invalid_export)
        self.assertNotIn("aggregate", invalid_export)

        paths = write_case(self.root, [truth(0, "T", 0.0, 0.0)], [output(0, "X", 0.0, 0.0)])
        manifest = json.loads(paths[2].read_text())
        manifest["accounting"]["output_overflow"] = 1
        manifest["accounting"]["drain_output_overflow"] = 1
        paths[2].write_text(json.dumps(manifest))
        processing_loss = scorer.score(*paths)
        self.assertEqual(processing_loss["status"], "UNQUALIFIED_PROCESSING")
        self.assertEqual(processing_loss["trace_failure_counters"]["output_overflow"], 1)
        self.assertEqual(processing_loss["trace_failure_counters"]["drain_output_overflow"], 1)
        self.assertIn("aggregate", processing_loss)


if __name__ == "__main__":
    unittest.main()
