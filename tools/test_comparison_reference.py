"""Focused unit and negative-control tests for OA-CMP-150..179."""

from __future__ import annotations

import csv
import hashlib
import json
import math
import tempfile
import unittest
from pathlib import Path

from tools import comparison_reference as ref


START_NS = 1_800_000_000_000_000_000
SEED = 91


def make_case(root: Path, scenario: str = "straight", aircraft: int = 3, duration: int = 8) -> tuple[Path, Path, Path, Path]:
    truth_path = root / "truth.csv"
    measurement_path = root / "measurements.csv"
    manifest_path = root / "manifest.json"
    protocol_path = root / "protocol.json"
    runner_source_path = root / "runner.rs"
    runner_source_path.write_bytes(b"synthetic fixture runner source\n")
    manifest = {
        "schema_version": 1,
        "model_id": "adsb_kinematic_v2",
        "source_implementation_sha256": hashlib.sha256(runner_source_path.read_bytes()).hexdigest(),
        "scenario_id": scenario,
        "seed": SEED,
        "aircraft_count": aircraft,
        "origin_lat_deg": 40.0,
        "origin_lon_deg": -75.0,
        "origin_alt_m": 0.0,
        "wgs84_semi_major_axis_m": ref.WGS84_A_M,
        "wgs84_flattening": ref.WGS84_F,
        "grid_start_ns": START_NS,
        "grid_step_ns": 1_000_000_000,
        "grid_count": duration + 1,
        "noise_seed": SEED,
        "noise_algorithm": "Box-Muller standard normal; independent east, north, east-velocity, north-velocity axes",
        "noise_keying": "LCG seeded by seed XOR event_id*0x9E3779B97F4A7C15 XOR axis*0xD1B54A32D192ED03; two LCG draws per normal",
        "noise_lcg_multiplier": 6_364_136_223_846_793_005,
        "noise_lcg_increment": 1_442_695_040_888_963_407,
        "noise_axis_keys": [1, 2, 3, 4],
        "event_id_keying": "truth-grid ordinal: time_index*aircraft_count+flight_index, including absent IDs on dropouts",
        "sigma_pos_m": float(f"{ref.SIGMA_POSITION_M:.8f}"),
        "sigma_vel_mps": float(f"{ref.SIGMA_VELOCITY_MPS:.8f}"),
        "scenario_duration_s": duration,
        "turn_rate_deg_s": 2.0,
        "turn_speed_mps": 150.0,
        "turn_sign_rule": "all_positive",
        "crossing_sign_rule": "even_positive_odd_negative",
        "crossing_pair_columns": 10,
        "crossing_pair_spacing_m": 5000.0,
        "measurement_delay_ns": 1_400_000_000 if scenario == "delayed" else 0,
        "delay_event_count": aircraft * sum(1 for sec in range(duration + 1) if scenario == "delayed" and sec % 10 == 5),
        "dropout_rule": "sec_mod_12_in_4_through_7",
        "dropout_event_count": aircraft * sum(1 for sec in range(duration + 1) if scenario == "dropout" and 4 <= sec % 12 <= 7),
        "delay_rule": "sec_mod_10_eq_5",
        "delay_event_ids": [sec * aircraft + flight for sec in range(duration + 1) if scenario == "delayed" and sec % 10 == 5 for flight in range(aircraft)],
        "dropout_event_ids": [sec * aircraft + flight for sec in range(duration + 1) if scenario == "dropout" and 4 <= sec % 12 <= 7 for flight in range(aircraft)],
        "local_frame": "WGS84 meridional and prime-vertical radii at origin; coordinates and ground velocities transformed analytically",
    }
    protocol = {
        "protocol_id": "openatca-comparison",
        "version": "1.0.3",
        "status": "draft_prequalification_clarification",
        "profile": {"id": "adsb_kinematic_v1"},
        "claim_scope": {"initial_cohort": "adsb_kinematic_v1", "not_established": ["physical_accuracy"]},
        "initialization_and_qualification": {
            "score_origin": "scenario_manifest.grid_start_ns",
            "candidate_initialization": {"scheme": "M_of_N_distinct_input_reports", "m": 2, "n": 3, "distinct_report_key": "event_id"},
            "development_control_initialization": {"minimum_distinct_input_reports_before_state_is_usable": 2, "distinct_report_key": "event_id"},
            "coast_when_actual_source_age_ns_greater_than": 2_000_000_000,
            "post_initialization_window": {"start_ns": "scenario_manifest.grid_start_ns + 2000000000", "exclude_before_start_from_postinit_metrics_only": True, "retain_cold_start_metrics": True, "retain_all_grid_metrics": True},
            "usable_statuses": ["valid"],
            "nominal_scenarios": {name: {"role": "qualification", "postinit_usable_ratio_min": 0.999, "candidate_ratio_at_least_baseline": True} for name in ("straight", "turn", "acceleration", "crossing", "delayed")},
            "diagnostic_scenarios": {"dropout": {"role": "diagnostic_only", "qualification_gate_in_v1": False, "never_reclassify_after_observing_candidate": True}},
            "paired_postinit_scoring": {
                "key": ["scenario_id", "flight_id", "score_ns"],
                "candidate_and_baseline_usable_keysets_must_match_for_accuracy_qualification": True,
                "mismatch_blocks_qualified_accuracy": True,
                "retain_raw_per_system_metrics_on_mismatch": True,
                "report_keyset_intersection_and_difference_counts": True,
            },
        },
        "csv": {
            "truth": list(ref.TRUTH_FIELDS),
            "measurements": list(ref.MEASUREMENT_FIELDS),
            "headers_exact": True,
            "timestamp_unit": "integer_nanoseconds",
        },
        "references": {"simulation_truth": "latent scenario state with independent keyed measurement noise"},
        "causality_and_time": {"future_measurements_or_smoothed_truth_in_candidate": False},
        "seeds_and_holdout": {"qualification": {"seed_ids": list(range(1_000_001, 1_000_101))}},
        "fault_mode_cohort": {
            "cohort_id": "dropout_fault_v1",
            "status": "BLOCKED_NOT_IMPLEMENTED_OR_REVIEWED",
            "scenario": "dropout",
            "required_for_all_quality_pass": True,
            "seed_ids": list(range(1_000_101, 1_000_121)),
            "seed_count": 20,
            "allocation": {"aircraft_count_per_seed": 300, "duration_s_per_aircraft": 1200, "simulated_aircraft_hours_total": 2000},
            "separate_from_nominal_performance_seed_set": True,
            "no_payload_generated_or_opened": True,
            "replicates_are_independent_references": False,
            "required_gates_pending_implementation_and_independent_review": [
                "offered_event_dispositions_match_the_frozen_dropout_schedule_with_zero_unexpected_loss",
                "every_required_score_grid_point_has_one_explicit_status_row",
                "no_valid_output_when_actual_supporting_source_age_exceeds_two_seconds",
                "recovery_is_valid_at_the_first_score_grid_after_a_fresh_measurement_is_received",
                "known_flight_identity_is_preserved_through_dropout_and_recovery",
            ],
            "all_quality_claim_status": "BLOCKED_UNTIL_ALL_REQUIRED_GATES_PASS_ON_SEPARATE_COHORT",
        },
        "limits": {"max_rows_per_csv": 200_000},
        "analytic_position_tolerance_m": 0.03,
        "analytic_velocity_tolerance_mps": 0.003,
        "noise_family_alpha": 0.001,
    }
    truth_rows: list[list[str]] = []
    measurements: list[list[str]] = []
    for sec in range(duration + 1):
        for flight in range(aircraft):
            lat, lon, ve, vn, phase = ref._analytic_state(
                scenario, flight, float(sec), duration, aircraft, 40.0, -75.0,
                ref.WGS84_A_M, ref.WGS84_F,
            )
            sensor_ns = START_NS + sec * 1_000_000_000
            truth_rows.append([
                scenario, str(flight), str(sensor_ns), f"{lat:.8f}", f"{lon:.8f}",
                f"{ve:.4f}", f"{vn:.4f}", phase,
            ])
            if scenario == "dropout" and sec % 12 in (4, 5, 6, 7):
                continue
            event_id = sec * aircraft + flight
            axes = [
                ref._keyed_normal(SEED, event_id, axis, manifest) * ref.SIGMA_POSITION_M
                for axis in (1, 2)
            ] + [
                ref._keyed_normal(SEED, event_id, axis, manifest) * ref.SIGMA_VELOCITY_MPS
                for axis in (3, 4)
            ]
            local_m, local_n = ref._wgs84_radii(lat, ref.WGS84_A_M, ref.WGS84_F)
            noisy_lat = lat + axes[1] / (local_m * math.pi / 180.0)
            noisy_lon = lon + axes[0] / (local_n * math.cos(math.radians(lat)) * math.pi / 180.0)
            delay = 1_400_000_000 if scenario == "delayed" and sec % 10 == 5 else 0
            measurements.append([
                str(event_id), scenario, str(flight), "1", str(sensor_ns), str(sensor_ns + delay),
                f"{noisy_lat:.8f}", f"{noisy_lon:.8f}", f"{ve + axes[2]:.4f}",
                f"{vn + axes[3]:.4f}", f"{ref.SIGMA_POSITION_M:.4f}",
                f"{ref.SIGMA_VELOCITY_MPS:.4f}",
            ])
    measurements.sort(key=lambda row: (int(row[5]), int(row[0])))
    with truth_path.open("w", newline="", encoding="utf-8") as f:
        writer = csv.writer(f, lineterminator="\n")
        writer.writerow(ref.TRUTH_FIELDS)
        writer.writerows(truth_rows)
    with measurement_path.open("w", newline="", encoding="utf-8") as f:
        writer = csv.writer(f, lineterminator="\n")
        writer.writerow(ref.MEASUREMENT_FIELDS)
        writer.writerows(measurements)
    manifest["truth_rows"] = len(truth_rows)
    manifest["expected_measurements"] = len(measurements)
    manifest["measurement_rows"] = len(measurements)
    manifest["measurements_offered"] = len(measurements)
    manifest["measurements_applied"] = len(measurements)
    for field in ("measurement_refused", "duplicate_measurements", "late_measurements", "capacity_refused", "work_refused", "tracker_output_overflow", "queue_pending_at_end"):
        manifest[field] = 0
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    protocol_path.write_text(json.dumps(protocol, indent=2) + "\n", encoding="utf-8")
    return truth_path, measurement_path, manifest_path, protocol_path


def independent_turn_row(flight_id: int, elapsed_s: float, direction: float = 1.0) -> tuple[float, float, float, float]:
    """Test-side closed-form for turn rows; intentionally does not call validator oracle."""
    a, f = 6_378_137.0, 1.0 / 298.257223563
    e2 = f * (2.0 - f)
    lat0, lon0 = math.radians(40.0), -75.0
    q0 = math.sqrt(1.0 - e2 * math.sin(lat0) ** 2)
    n0, m0 = a / q0, a * (1.0 - e2) / q0**3
    east0 = (flight_id % 20 - 9.5) * 1000.0
    north0 = (flight_id // 20) * 2000.0
    omega = 2.0 * math.pi / 180.0
    angle = omega * elapsed_s
    radius = 150.0 / omega
    east = east0 + direction * radius * math.sin(angle)
    north = north0 + direction * radius * (1.0 - math.cos(angle))
    lat = math.degrees(lat0) + north / (m0 * math.pi / 180.0)
    lon = lon0 + east / (n0 * math.cos(lat0) * math.pi / 180.0)
    phi = math.radians(lat)
    q = math.sqrt(1.0 - e2 * math.sin(phi) ** 2)
    meridian, prime = a * (1.0 - e2) / q**3, a / q
    vx, vy = direction * 150.0 * math.cos(angle), direction * 150.0 * math.sin(angle)
    ve = vx * prime * math.cos(phi) / (n0 * math.cos(lat0))
    vn = vy * meridian / m0
    return lat, lon, ve, vn


class ComparisonReferenceTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def run_case(self, scenario: str = "straight", aircraft: int = 3, duration: int = 8) -> dict:
        files = make_case(self.root, scenario, aircraft, duration)
        return ref.validate(*files, runner_source_path=files[0].parent / "runner.rs")

    def rewrite_csv(self, path: Path, rows: list[list[str]], fields: tuple[str, ...]) -> None:
        with path.open("w", newline="", encoding="utf-8") as f:
            writer = csv.writer(f, lineterminator="\n")
            writer.writerow(fields)
            writer.writerows(rows)

    def read_csv(self, path: Path) -> list[list[str]]:
        with path.open(newline="", encoding="utf-8") as f:
            rows = list(csv.reader(f))
        return rows[1:]

    def read_manifest(self) -> dict:
        return json.loads((self.root / "manifest.json").read_text(encoding="utf-8"))

    def write_manifest(self, manifest: dict) -> None:
        (self.root / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")

    # REQ: OA-CMP-151, OA-CMP-152, OA-CMP-153, OA-CMP-155
    def test_all_declared_scenarios_pass_closed_form_and_noise_checks(self) -> None:
        for scenario, duration in (("straight", 8), ("turn", 8), ("acceleration", 8),
                                   ("crossing", 8), ("dropout", 13), ("delayed", 8)):
            with self.subTest(scenario=scenario):
                result = self.run_case(scenario, aircraft=3, duration=duration)
                self.assertEqual(result["validation_status"], "PASS", result["reason"])
                self.assertEqual(result["scope"], "declared_model_only")
                self.assertFalse(result["physical_accuracy_claim"])
                self.assertEqual(result["reference_kind"], "independent_simulation")

    # REQ: OA-CMP-151
    def test_even_and_odd_turn_directions_match_independent_vectors_and_reject_wrong_sign(self) -> None:
        truth, measurements, manifest, protocol = make_case(self.root, "turn", aircraft=2, duration=60)
        truth_rows = self.read_csv(truth)
        measurement_rows = self.read_csv(measurements)
        by_truth = {(int(row[1]), int(row[2])): row for row in truth_rows}
        for flight in (0, 1):
            for sec in (0, 45):
                row = by_truth[(flight, START_NS + sec * 1_000_000_000)]
                expected = independent_turn_row(flight, float(sec), direction=1.0)
                self.assertEqual(row[3], f"{expected[0]:.8f}")
                self.assertEqual(row[4], f"{expected[1]:.8f}")
                self.assertEqual(row[5], f"{expected[2]:.4f}")
                self.assertEqual(row[6], f"{expected[3]:.4f}")
        good = ref.validate(truth, measurements, manifest, protocol, truth.parent / "runner.rs")
        self.assertEqual(good["validation_status"], "PASS", good["reason"])

        # A wrong odd-flight sign preserves the t=0 position but reverses its velocity;
        # update its paired measurement by the same delta so deterministic noise still agrees.
        sec, flight = 0, 1
        event_time = START_NS + sec * 1_000_000_000
        old = by_truth[(flight, event_time)]
        wrong = independent_turn_row(flight, 0.0, direction=-1.0)
        old_ve, old_vn = float(old[5]), float(old[6])
        old[5], old[6] = f"{wrong[2]:.4f}", f"{wrong[3]:.4f}"
        event_id = sec * 2 + flight
        measurement = next(row for row in measurement_rows if int(row[0]) == event_id)
        measurement[8] = f"{float(measurement[8]) + wrong[2] - old_ve:.4f}"
        measurement[9] = f"{float(measurement[9]) + wrong[3] - old_vn:.4f}"
        self.rewrite_csv(truth, truth_rows, ref.TRUTH_FIELDS)
        self.rewrite_csv(measurements, measurement_rows, ref.MEASUREMENT_FIELDS)
        bad = ref.validate(truth, measurements, manifest, protocol, truth.parent / "runner.rs")
        self.assertEqual(bad["status"], "FAIL")
        self.assertIn("analytic WGS84 velocity mismatch", bad["reason"])

    # REQ: OA-CMP-151
    def test_100m_truth_shift_fails(self) -> None:
        truth, measurements, manifest, protocol = make_case(self.root)
        rows = self.read_csv(truth)
        rows[0][3] = f"{float(rows[0][3]) + 100.0 / 111_000.0:.8f}"
        self.rewrite_csv(truth, rows, ref.TRUTH_FIELDS)
        result = ref.validate(truth, measurements, manifest, protocol, truth.parent / "runner.rs")
        self.assertEqual(result["status"], "FAIL")
        self.assertIn("analytic trajectory position mismatch", result["reason"])

    # REQ: OA-CMP-151
    def test_velocity_inconsistency_fails(self) -> None:
        truth, measurements, manifest, protocol = make_case(self.root)
        rows = self.read_csv(truth)
        rows[2][5] = f"{float(rows[2][5]) + 20.0:.4f}"
        self.rewrite_csv(truth, rows, ref.TRUTH_FIELDS)
        result = ref.validate(truth, measurements, manifest, protocol, truth.parent / "runner.rs")
        self.assertEqual(result["status"], "FAIL")
        self.assertIn("analytic WGS84 velocity mismatch", result["reason"])

    # REQ: OA-CMP-152
    def test_negative_and_wrong_future_receive_times_fail(self) -> None:
        truth, measurements, manifest, protocol = make_case(self.root)
        rows = self.read_csv(measurements)
        rows[0][4] = "-1"
        rows[0][5] = "-1"
        self.rewrite_csv(measurements, rows, ref.MEASUREMENT_FIELDS)
        result = ref.validate(truth, measurements, manifest, protocol, truth.parent / "runner.rs")
        self.assertEqual(result["status"], "FAIL")
        self.assertIn("nonnegative", result["reason"])

        truth, measurements, manifest, protocol = make_case(self.root)
        rows = self.read_csv(measurements)
        rows[-1][5] = str(int(rows[-1][5]) + 1_000_000_000)
        rows.sort(key=lambda row: (int(row[5]), int(row[0])))
        self.rewrite_csv(measurements, rows, ref.MEASUREMENT_FIELDS)
        result = ref.validate(truth, measurements, manifest, protocol, truth.parent / "runner.rs")
        self.assertEqual(result["status"], "FAIL")
        self.assertIn("causal delay schedule", result["reason"])

    # REQ: OA-CMP-150, OA-CMP-152
    def test_measurement_reorder_duplicate_and_missing_event_fail(self) -> None:
        truth, measurements, manifest, protocol = make_case(self.root, "delayed", duration=8)
        rows = self.read_csv(measurements)
        rows[0], rows[1] = rows[1], rows[0]
        self.rewrite_csv(measurements, rows, ref.MEASUREMENT_FIELDS)
        result = ref.validate(truth, measurements, manifest, protocol, truth.parent / "runner.rs")
        self.assertEqual(result["status"], "FAIL")
        self.assertIn("ordered by", result["reason"])

        truth, measurements, manifest, protocol = make_case(self.root)
        rows = self.read_csv(measurements)
        rows.append(rows[-1].copy())
        self.rewrite_csv(measurements, rows, ref.MEASUREMENT_FIELDS)
        result = ref.validate(truth, measurements, manifest, protocol, truth.parent / "runner.rs")
        self.assertEqual(result["status"], "FAIL")
        self.assertIn("duplicate event_id", result["reason"])

        truth, measurements, manifest, protocol = make_case(self.root)
        rows = self.read_csv(measurements)
        rows.pop()
        self.rewrite_csv(measurements, rows, ref.MEASUREMENT_FIELDS)
        result = ref.validate(truth, measurements, manifest, protocol, truth.parent / "runner.rs")
        self.assertEqual(result["status"], "FAIL")
        self.assertIn("row count", result["reason"])

    # REQ: OA-CMP-153
    def test_independent_noise_metadata_rejects_aliased_axis(self) -> None:
        truth, measurements, manifest, protocol = make_case(self.root)
        data = self.read_manifest()
        data["noise_axis_keys"] = [1, 1, 3, 4]
        self.write_manifest(data)
        result = ref.validate(truth, measurements, manifest, protocol, truth.parent / "runner.rs")
        self.assertEqual(result["status"], "FAIL")
        self.assertIn("four distinct axis", result["reason"])

    # REQ: OA-CMP-150
    def test_truth_schema_and_nonfinite_values_are_rejected(self) -> None:
        truth, measurements, manifest, protocol = make_case(self.root)
        rows = self.read_csv(truth)
        self.rewrite_csv(truth, rows, tuple(["wrong"] + list(ref.TRUTH_FIELDS[1:])))
        result = ref.validate(truth, measurements, manifest, protocol, truth.parent / "runner.rs")
        self.assertEqual(result["status"], "FAIL")
        self.assertIn("header must exactly equal", result["reason"])

        truth, measurements, manifest, protocol = make_case(self.root)
        rows = self.read_csv(truth)
        rows[0][3] = "nan"
        self.rewrite_csv(truth, rows, ref.TRUTH_FIELDS)
        result = ref.validate(truth, measurements, manifest, protocol, truth.parent / "runner.rs")
        self.assertEqual(result["status"], "FAIL")
        self.assertIn("is not numeric", result["reason"])

        truth, measurements, manifest, protocol = make_case(self.root)
        protocol_data = json.loads(protocol.read_text(encoding="utf-8"))
        protocol_data["limits"]["max_rows_per_csv"] = 1
        protocol.write_text(json.dumps(protocol_data), encoding="utf-8")
        result = ref.validate(truth, measurements, manifest, protocol, truth.parent / "runner.rs")
        self.assertEqual(result["status"], "FAIL")
        self.assertIn("protocol row limit", result["reason"])

    # REQ: OA-CMP-156
    def test_unknown_world_model_is_unqualified(self) -> None:
        truth, measurements, manifest, protocol = make_case(self.root)
        data = self.read_manifest()
        data["model_id"] = "unknown-world"
        self.write_manifest(data)
        result = ref.validate(truth, measurements, manifest, protocol, truth.parent / "runner.rs")
        self.assertEqual(result["validation_status"], "UNQUALIFIED")

    # REQ: OA-CMP-154, OA-CMP-155
    def test_output_hash_bindings_are_present(self) -> None:
        truth, measurements, manifest, protocol = make_case(self.root)
        result = ref.validate(truth, measurements, manifest, protocol, truth.parent / "runner.rs")
        self.assertEqual(result["validation_status"], "PASS")
        self.assertEqual(result["truth_input_sha256"], ref._sha256(truth))
        self.assertEqual(result["measurement_input_sha256"], ref._sha256(measurements))
        self.assertEqual(result["protocol_sha256"], ref._sha256(protocol))
        self.assertEqual(result["runner_source_sha256"], hashlib.sha256((self.root / "runner.rs").read_bytes()).hexdigest())
        self.assertEqual(result["protocol_version"], "1.0.3")
        self.assertEqual(result["protocol_status"], "draft_prequalification_clarification")
        self.assertEqual(result["fault_mode_cohort_status"], "BLOCKED_NOT_IMPLEMENTED_OR_REVIEWED")
        self.assertEqual(result["all_quality_claim_status"], "BLOCKED_UNTIL_ALL_REQUIRED_GATES_PASS_ON_SEPARATE_COHORT")
        self.assertTrue(result["runner_source_hash_verified"])

    # REQ: OA-CMP-154, OA-CMP-156
    def test_pass_requires_matching_runner_source_hash(self) -> None:
        files = make_case(self.root)
        unbound = ref.validate(*files)
        self.assertEqual(unbound["validation_status"], "UNQUALIFIED")
        self.assertIn("runner source bytes are required", unbound["reason"])

        wrong_source = self.root / "wrong-runner.rs"
        wrong_source.write_bytes(b"different runner source\n")
        mismatch = ref.validate(*files, runner_source_path=wrong_source)
        self.assertEqual(mismatch["validation_status"], "FAIL")
        self.assertIn("does not match manifest", mismatch["reason"])

    # REQ: OA-CMP-157
    def test_protocol_fault_cohort_ids_are_frozen_and_blocked(self) -> None:
        files = make_case(self.root)
        protocol_data = json.loads(files[3].read_text(encoding="utf-8"))
        cohort = protocol_data["fault_mode_cohort"]
        self.assertEqual(cohort["seed_ids"], list(range(1_000_101, 1_000_121)))
        self.assertEqual(cohort["status"], "BLOCKED_NOT_IMPLEMENTED_OR_REVIEWED")
        self.assertEqual(cohort["allocation"], {
            "aircraft_count_per_seed": 300,
            "duration_s_per_aircraft": 1200,
            "simulated_aircraft_hours_total": 2000,
        })
        self.assertTrue(cohort["separate_from_nominal_performance_seed_set"])
        self.assertTrue(cohort["no_payload_generated_or_opened"])
        self.assertEqual(len(cohort["required_gates_pending_implementation_and_independent_review"]), 5)
        paired = protocol_data["initialization_and_qualification"]["paired_postinit_scoring"]
        self.assertEqual(paired["key"], ["scenario_id", "flight_id", "score_ns"])
        self.assertTrue(paired["mismatch_blocks_qualified_accuracy"])
        cohort["seed_ids"][0] += 1
        files[3].write_text(json.dumps(protocol_data), encoding="utf-8")
        result = ref.validate(*files, runner_source_path=files[0].parent / "runner.rs")
        self.assertEqual(result["validation_status"], "UNQUALIFIED")
        self.assertIn("fault cohort or blocked gate", result["reason"])


if __name__ == "__main__":
    unittest.main()
