"""Static contract tests for comparison protocol requirements OA-CMP-200..209.

These tests intentionally assert the frozen protocol values rather than run a
comparison. They are standard-library only so the coordinator can execute them
without a Cargo build or external data.
"""

import json
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
MANIFEST_PATH = ROOT / "validation" / "comparison-protocol-v1.json"


def read_manifest():
    with MANIFEST_PATH.open(encoding="utf-8") as source:
        return json.load(source)


class ComparisonProtocolContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.manifest = read_manifest()

    # REQ: OA-CMP-200
    def test_oa_cmp_200_development_and_qualification_seeds_are_disjoint(self):
        seeds = self.manifest["seeds_and_holdout"]
        dev = set(seeds["development_seeds"]["seed_ids"])
        dev.add(seeds["development_seeds"]["default_seed"])
        qualification = set(seeds["qualification"]["seed_ids"])
        self.assertTrue(dev.isdisjoint(qualification), "development seed overlaps qualification")

    # REQ: OA-CMP-201
    def test_oa_cmp_201_qualification_seed_ids_are_unique_and_counted(self):
        qualification = self.manifest["seeds_and_holdout"]["qualification"]
        seed_ids = qualification["seed_ids"]
        self.assertEqual(len(seed_ids), len(set(seed_ids)), "qualification seed IDs must be unique")
        self.assertEqual(len(seed_ids), qualification["seed_count"])

    # REQ: OA-CMP-202
    def test_oa_cmp_202_competitive_thresholds_are_frozen(self):
        goals = self.manifest["scorecard"]["competitive_relative_goals"]
        self.assertEqual(goals["position_rms_ratio_max"], 0.70)
        self.assertEqual(goals["p99_position_error_ratio_max"], 0.80)
        self.assertEqual(goals["p99_receive_to_usable_latency_ratio_max"], 0.50)
        self.assertEqual(goals["throughput_ratio_min"], 2.0)
        self.assertIs(goals["same_quality_required"], True)
        self.assertIs(goals["same_resource_envelope_required"], True)

    # REQ: OA-CMP-203
    def test_oa_cmp_203_csv_headers_and_claim_stages_are_supported_contract(self):
        self.assertEqual(
            self.manifest["csv"]["truth"],
            ["scenario_id", "flight_id", "time_ns", "lat_deg", "lon_deg", "ve_mps", "vn_mps", "phase"],
        )
        self.assertEqual(
            self.manifest["csv"]["measurements"],
            ["event_id", "scenario_id", "flight_id", "sensor_id", "sensor_ns", "receive_ns", "lat_deg", "lon_deg", "ve_mps", "vn_mps", "sigma_pos_m", "sigma_vel_mps"],
        )
        self.assertEqual(
            self.manifest["csv"]["outputs"],
            ["scenario_id", "flight_id", "score_ns", "source_ns", "publish_ns", "lat_deg", "lon_deg", "ve_mps", "vn_mps", "status"],
        )
        stages = self.manifest["claim_scope"]["stage_claims"]
        self.assertEqual(
            set(stages),
            {"development_control", "professional_tracker", "legacy_export", "controlled_legacy"},
        )
        self.assertEqual(self.manifest["profile"]["external_reference_stage"], "unknown_until_a_qualified_artifact_and_approved_run_are_recorded")

    # REQ: OA-CMP-204
    def test_oa_cmp_204_qualification_aircraft_hours_meet_exact_allocation(self):
        qualification = self.manifest["seeds_and_holdout"]["qualification"]
        allocation = qualification["allocation"]
        hours = (
            len(qualification["seed_ids"])
            * allocation["aircraft_count"]
            * allocation["duration_s_per_aircraft"]
            / 3600
        )
        self.assertEqual(qualification["minimum_simulated_aircraft_hours"], 10000)
        self.assertEqual(hours, qualification["simulated_aircraft_hours_total"])
        self.assertGreaterEqual(hours, qualification["minimum_simulated_aircraft_hours"])
        scenario_allocation = allocation["seed_allocation_by_scenario"]
        self.assertEqual(sum(scenario_allocation.values()), len(qualification["seed_ids"]))
        self.assertEqual(scenario_allocation["dropout"], 0)

    # REQ: OA-CMP-205
    def test_oa_cmp_205_qualification_uses_frozen_nonoverlapping_seed_ids(self):
        qualification = self.manifest["seeds_and_holdout"]["qualification"]
        self.assertEqual(qualification["seed_ids"], list(range(1_000_001, 1_000_101)))
        self.assertIs(qualification["generator_distributions_and_allocation_frozen_before_candidate_freeze"], True)

    # REQ: OA-CMP-206
    def test_oa_cmp_206_input_output_ledgers_require_zero_unaccounted_work(self):
        ledger = self.manifest["run_ledger"]
        gates = self.manifest["strict_gates"]
        self.assertEqual(ledger["unprocessed_or_unaccounted_allowed"], 0)
        self.assertIs(gates["every_measurement_exactly_one_terminal_disposition"], True)
        self.assertIs(gates["every_required_truth_grid_point_exactly_one_output_status"], True)
        self.assertEqual(gates["unaccounted_input_or_output_loss_allowed"], 0)
        self.assertIs(gates["missing_rows_scored_not_omitted"], True)

    # REQ: OA-CMP-207
    def test_oa_cmp_207_holdout_is_single_use_and_reclassified_after_tuning(self):
        holdout = self.manifest["seeds_and_holdout"]["actual_holdout"]
        self.assertIs(holdout["consume_once"], True)
        self.assertIs(holdout["inspect_for_tuning"], False)
        self.assertIn("reclassify", holdout["if_outcome_informs_change"])
        self.assertIs(holdout["publish_consumption_and_reclassification_state"], True)

    # REQ: OA-CMP-208
    def test_oa_cmp_208_no_unqualified_or_synthetic_professional_baseline_is_claimed(self):
        baseline = self.manifest["baseline_status"]
        self.assertEqual(baseline["professional_tracker"], "unknown_no_qualified_artifact_or_approved_run")
        self.assertEqual(baseline["legacy_export"], "unknown_no_authenticated_permitted_export")
        self.assertIs(baseline["python_or_controlled_legacy_may_be_labeled_as_professional"], False)

    # REQ: OA-CMP-209
    def test_oa_cmp_209_seed_replicates_are_not_independent_references(self):
        qualification = self.manifest["seeds_and_holdout"]["qualification"]
        statistics = self.manifest["statistics_and_exclusions"]
        self.assertIs(qualification["seed_replicates_are_independent_references"], False)
        self.assertIs(qualification["must_not_claim_100_independent_references"], True)
        self.assertIs(statistics["seed_as_independent_aircraft_or_physical_reference"], False)
        self.assertEqual(
            qualification["shared_world_reference_model_note"],
            "scenario seeds do not create independent physical or world-reference models",
        )

    # REQ: OA-CMP-210
    def test_oa_cmp_210_missing_output_has_explicit_canonical_payload(self):
        missing = self.manifest["csv"]["missing_output_row"]
        self.assertIs(missing["score_ns_equals_publish_ns"], True)
        self.assertEqual(missing["source_ns"], "empty")
        self.assertEqual(missing["state_fields_empty"], ["lat_deg", "lon_deg", "ve_mps", "vn_mps"])
        self.assertIs(missing["row_required_for_each_missing_grid_point"], True)

    # REQ: OA-CMP-211, OA-CMP-212, OA-CMP-213, OA-CMP-214, OA-CMP-215, OA-CMP-216, OA-CMP-218
    def test_oa_cmp_211_to_216_freeze_initialization_windows_and_scenario_roles(self):
        self.assertEqual(self.manifest["version"], "1.0.3")
        init = self.manifest["initialization_and_qualification"]
        self.assertEqual(init["score_origin"], "scenario_manifest.grid_start_ns")
        self.assertEqual(init["candidate_initialization"]["m"], 2)
        self.assertEqual(init["candidate_initialization"]["n"], 3)
        self.assertEqual(init["development_control_initialization"]["minimum_distinct_input_reports_before_state_is_usable"], 2)
        self.assertIs(init["post_initialization_window"]["retain_cold_start_metrics"], True)
        self.assertIs(init["post_initialization_window"]["retain_all_grid_metrics"], True)
        self.assertIn("+ 2000000000", init["post_initialization_window"]["start_ns"])
        self.assertEqual(init["coast_when_actual_source_age_ns_greater_than"], 2000000000)
        self.assertEqual(init["usable_statuses"], ["valid"])
        self.assertEqual(set(init["nominal_scenarios"]), {"straight", "turn", "acceleration", "crossing", "delayed"})
        for scenario in init["nominal_scenarios"].values():
            self.assertEqual(scenario["postinit_usable_ratio_min"], 0.999)
            self.assertIs(scenario["candidate_ratio_at_least_baseline"], True)
        dropout = init["diagnostic_scenarios"]["dropout"]
        self.assertEqual(dropout["role"], "diagnostic_only")
        self.assertIs(dropout["qualification_gate_in_v1"], False)
        self.assertIs(dropout["never_reclassify_after_observing_candidate"], True)
        paired = init["paired_postinit_scoring"]
        self.assertEqual(paired["key"], ["scenario_id", "flight_id", "score_ns"])
        self.assertIs(paired["candidate_and_baseline_usable_keysets_must_match_for_accuracy_qualification"], True)
        self.assertIs(paired["mismatch_blocks_qualified_accuracy"], True)
        self.assertIs(paired["retain_raw_per_system_metrics_on_mismatch"], True)
        self.assertIs(paired["report_keyset_intersection_and_difference_counts"], True)

    # REQ: OA-CMP-217
    def test_oa_cmp_217_separate_dropout_fault_cohort_blocks_all_quality(self):
        cohort = self.manifest["fault_mode_cohort"]
        self.assertEqual(cohort["status"], "BLOCKED_NOT_IMPLEMENTED_OR_REVIEWED")
        self.assertEqual(cohort["seed_ids"], list(range(1_000_101, 1_000_121)))
        self.assertEqual(cohort["allocation"], {
            "aircraft_count_per_seed": 300,
            "duration_s_per_aircraft": 1200,
            "simulated_aircraft_hours_total": 2000,
        })
        self.assertIs(cohort["separate_from_nominal_performance_seed_set"], True)
        self.assertIs(cohort["no_payload_generated_or_opened"], True)
        self.assertIn("BLOCKED", cohort["all_quality_claim_status"])
        self.assertGreaterEqual(len(cohort["required_gates_pending_implementation_and_independent_review"]), 5)


if __name__ == "__main__":
    unittest.main()
