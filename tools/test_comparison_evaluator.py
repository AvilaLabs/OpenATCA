"""Small analytic and negative-control tests for the comparison evaluator."""
import ast
import contextlib
import csv
import io
import sys
import json
import math
import os
import tempfile
import unittest

import comparison_evaluator as ce
import comparison_baseline as cb

TRUTH = ["scenario_id","flight_id","time_ns","lat_deg","lon_deg","ve_mps","vn_mps","phase"]
MEAS = ["event_id","scenario_id","flight_id","sensor_id","sensor_ns","receive_ns","lat_deg","lon_deg","ve_mps","vn_mps","sigma_pos_m","sigma_vel_mps"]
OUT = ce.EXPECTED["output"]

def write(path, header, rows):
    with open(path,"w",newline="",encoding="utf-8") as f:
        w=csv.DictWriter(f,fieldnames=header); w.writeheader(); w.writerows(rows)

def truth_rows():
    return [dict(scenario_id="s",flight_id="f",time_ns=i*1_000_000_000,lat_deg=0,lon_deg=i*100/111195,ve_mps=100,vn_mps=0,phase="cruise") for i in range(3)]

def output_rows(offset=0, delay=0, flight="f"):
    return [dict(scenario_id="s",flight_id=flight,score_ns=i*1_000_000_000,source_ns=i*1_000_000_000-delay,publish_ns=i*1_000_000_000,lat_deg=0,lon_deg=(i*100+offset)/111195,ve_mps=100,vn_mps=0,status="valid") for i in range(3)]

class ComparisonTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory(); self.d=self.tmp.name
        self.t=os.path.join(self.d,"truth.csv"); self.m=os.path.join(self.d,"measurements.csv")
        self.c=os.path.join(self.d,"candidate.csv"); self.b=os.path.join(self.d,"baseline.csv")
        write(self.t,TRUTH,truth_rows()); write(self.m,MEAS,[])
        write(self.c,OUT,output_rows()); write(self.b,OUT,output_rows(offset=100))
    def tearDown(self): self.tmp.cleanup()
    # REQ: OA-CMP-001, OA-CMP-010
    def test_analytic_wgs84_offset_is_approximately_one_hundred_metres(self):
        error=ce.horizontal_error_m((0,0),(0,100/111195))
        self.assertAlmostEqual(error,100,delta=.2)
    # REQ: OA-CMP-003
    def test_interleaved_truth_grid_is_ordered_by_scenario_time_then_flight(self):
        keys=[("s",str(f),t) for t in (0,1_000_000_000) for f in range(12)]
        truth=[dict(scenario_id=s,flight_id=f,time_ns=t,lat_deg=0,lon_deg=0,ve_mps=0,vn_mps=0,phase="cruise") for s,f,t in keys]
        outputs=[dict(scenario_id=s,flight_id=f,score_ns=t,source_ns="",publish_ns=t,lat_deg="",lon_deg="",ve_mps="",vn_mps="",status="missing") for s,f,t in keys]
        write(self.t,TRUTH,truth); write(self.m,MEAS,[]); write(self.c,OUT,outputs); write(self.b,OUT,outputs)
        result=ce.evaluate(self.t,self.m,self.c,self.b,{})
        self.assertEqual(result["accounting"]["truth_grid_rows"],24)
    # REQ: OA-CMP-010
    def test_baseline_geodesic_inverse_direct_roundtrip(self):
        for origin,distance,azimuth in (((45.0,-90.0),100_000,0.8),((10.0,179.8),50_000,1.4),((-30.0,20.0),250_000,2.1)):
            lat,lon,final=cb._vincenty_direct(*origin,azimuth,distance)
            recovered,first,last=cb._vincenty_inverse(*origin,lat,lon)
            self.assertAlmostEqual(recovered,distance,delta=1e-5)
            self.assertAlmostEqual(math.sin(first-azimuth),0,delta=1e-9)
            self.assertAlmostEqual(math.cos(first-azimuth),1,delta=1e-9)
            self.assertAlmostEqual(math.sin(last-final),0,delta=1e-9)
    # REQ: OA-CMP-010
    def test_geodesic_velocity_basis_rotation_matches_finite_difference(self):
        origin=(60.0,-100.0); lat2,lon2,final=cb._vincenty_direct(*origin,1.1,120_000)
        lat3,lon3,_=cb._vincenty_direct(lat2,lon2,final,1.0)
        distance,first_after,_=cb._vincenty_inverse(*origin,lat3,lon3)
        self.assertAlmostEqual(distance,120_001,delta=1e-5)
        self.assertAlmostEqual(math.sin(first_after-1.1),0,delta=1e-8)
        self.assertAlmostEqual(math.cos(first_after-1.1),1,delta=1e-8)
    # REQ: OA-CMP-002, OA-CMP-011, OA-CMP-013, OA-CMP-015, OA-CMP-016, OA-CMP-017, OA-CMP-019, OA-CMP-020, OA-CMP-028, OA-CMP-042
    def test_analytic_velocity_and_delay_metrics(self):
        write(self.c,OUT,output_rows(delay=500_000_000))
        rows=[dict(event_id=f"e{i}",scenario_id="s",flight_id="f",sensor_id="x",sensor_ns=i*1_000_000_000-500_000_000,receive_ns=i*1_000_000_000-500_000_000,lat_deg=0,lon_deg=0,ve_mps=100,vn_mps=0,sigma_pos_m=5,sigma_vel_mps=2) for i in range(3)]
        write(self.m,MEAS,rows); result=ce.evaluate(self.t,self.m,self.c,self.b,{})
        self.assertEqual(result["logical_metrics"]["candidate"]["source_timestamp_delay_proxy_ms"]["p99"],500)
        self.assertIsNone(result["logical_metrics"]["candidate"]["receive_to_usable_ms"]["p99"])
        self.assertFalse(result["accuracy_qualification"]["enabled"])
        rows[0]["receive_ns"]=1; write(self.m,MEAS,rows)
        with self.assertRaisesRegex(ce.EvaluationError,"precedes its input receive time"):
            ce.evaluate(self.t,self.m,self.c,self.b,{})
    # REQ: OA-CMP-004, OA-CMP-043
    def test_output_flight_swap_is_counted_as_false_identity(self):
        write(self.c,OUT,output_rows(flight="other"))
        result=ce.evaluate(self.t,self.m,self.c,self.b,{})
        self.assertEqual(result["accounting"]["false_identity_rows"],3)
    # REQ: OA-CMP-005
    def test_duplicate_output_grid_key_is_counted(self):
        rows=output_rows(); write(self.c,OUT,rows+[rows[0]])
        result=ce.evaluate(self.t,self.m,self.c,self.b,{})
        self.assertEqual(result["accounting"]["candidate_duplicate_rows"],1)
    # REQ: OA-CMP-008
    def test_mismatched_accuracy_and_publish_times_are_rejected(self):
        rows=output_rows(); rows[0]["score_ns"]="99"; write(self.c,OUT,rows)
        with self.assertRaisesRegex(ce.EvaluationError,"score time differs from publish time"): ce.evaluate(self.t,self.m,self.c,self.b,{})
    # REQ: OA-CMP-009
    def test_bad_source_publish_order_is_rejected(self):
        rows=output_rows(); rows[0]["source_ns"]="2"; write(self.c,OUT,rows)
        with self.assertRaisesRegex(ce.EvaluationError,"source_ns after publish_ns"): ce.evaluate(self.t,self.m,self.c,self.b,{})
    # REQ: OA-CMP-032, OA-CMP-036, OA-CMP-048
    def test_baseline_is_development_control_and_writes_grid(self):
        measurements=[dict(event_id="e",scenario_id="s",flight_id="f",sensor_id="x",sensor_ns=0,receive_ns=0,lat_deg=0,lon_deg=0,ve_mps=100,vn_mps=0,sigma_pos_m=5,sigma_vel_mps=2)]
        write(self.m,MEAS,measurements); manifest=cb.run(self.t,self.m,self.b)
        rows=ce._csv(self.b,"output")
        self.assertEqual(len(rows),3); self.assertEqual(manifest["comparison_class"],"development_control")
    # REQ: OA-CMP-032, OA-CMP-033, OA-CMP-049
    def test_baseline_accepts_key_only_grid_manifest(self):
        grid=os.path.join(self.d,"grid.csv")
        write(grid,["scenario_id","flight_id","time_ns"],[{k:r[k] for k in ("scenario_id","flight_id","time_ns")} for r in truth_rows()])
        manifest=cb.run(None,self.m,self.b,grid_path=grid)
        self.assertEqual(manifest["output_rows"],3)
        with open(grid,"w") as f: f.write("scenario_id,flight_id,time_ns,phase\ns,f,0,cruise\n")
        with self.assertRaisesRegex(ce.EvaluationError,"grid must contain only"): cb.run(None,self.m,self.b,grid_path=grid)
    # REQ: OA-CMP-034
    def test_baseline_does_not_use_measurement_with_future_sensor_time(self):
        rows=[dict(event_id="future",scenario_id="s",flight_id="f",sensor_id="x",sensor_ns=5_000_000_000,receive_ns=0,lat_deg=0,lon_deg=0,ve_mps=100,vn_mps=0,sigma_pos_m=5,sigma_vel_mps=2)]
        write(self.m,MEAS,rows); cb.run(self.t,self.m,self.b)
        outputs=ce._csv(self.b,"output")
        self.assertTrue(all(row["status"]=="missing" for row in outputs))
        self.assertTrue(all(row["source_ns"]=="" and row["lat_deg"]=="" for row in outputs))
    # REQ: OA-CMP-035, OA-CMP-048
    def test_baseline_uses_mofn2_and_coasts_after_source_age_limit(self):
        times=[0,1_000_000_000,2_000_000_000,3_000_000_000,4_000_000_000]
        write(self.t,TRUTH,[dict(scenario_id="s",flight_id="f",time_ns=t,lat_deg=0,lon_deg=0,ve_mps=100,vn_mps=0,phase="cruise") for t in times])
        rows=[dict(event_id=f"e{i}",scenario_id="s",flight_id="f",sensor_id="x",sensor_ns=t,receive_ns=t,lat_deg=0,lon_deg=0,ve_mps=100,vn_mps=0,sigma_pos_m=5,sigma_vel_mps=2) for i,t in enumerate(times[:2])]
        write(self.m,MEAS,rows); manifest=cb.run(self.t,self.m,self.b)
        outputs=ce._csv(self.b,"output")
        self.assertEqual([row["status"] for row in outputs],["missing","valid","valid","valid","coast"])
        self.assertEqual(manifest["confirmation_profile"],{"hits":2,"window_reports":3})
    # REQ: OA-CMP-032
    def test_baseline_refuses_empty_or_duplicate_event_ids_globally(self):
        duplicate=[dict(event_id="same",scenario_id="s",flight_id="f",sensor_id="x",sensor_ns=i,receive_ns=i,lat_deg=0,lon_deg=0,ve_mps=0,vn_mps=0,sigma_pos_m=1,sigma_vel_mps=1) for i in (0,1)]
        write(self.m,MEAS,duplicate)
        os.unlink(self.b)
        with self.assertRaisesRegex(ce.EvaluationError,"duplicate/empty event_id"): cb.run(self.t,self.m,self.b)
        self.assertFalse(os.path.exists(self.b))
        duplicate[0]["event_id"]=""
        write(self.m,MEAS,duplicate)
        with self.assertRaisesRegex(ce.EvaluationError,"duplicate/empty event_id"): cb.run(self.t,self.m,self.b)
    # REQ: OA-CMP-035
    def test_two_distinct_event_ids_at_same_sensor_time_confirm_mofn2(self):
        rows=[dict(event_id=f"same-time-{i}",scenario_id="s",flight_id="f",sensor_id="x",sensor_ns=0,receive_ns=i*100_000_000,lat_deg=0,lon_deg=0,ve_mps=100,vn_mps=0,sigma_pos_m=5,sigma_vel_mps=2) for i in range(2)]
        write(self.m,MEAS,rows); cb.run(self.t,self.m,self.b)
        outputs=ce._csv(self.b,"output")
        self.assertEqual(outputs[0]["status"],"missing")
        self.assertEqual(outputs[1]["status"],"valid")

    # REQ: OA-CMP-003, OA-CMP-032, OA-CMP-035
    def test_numeric_event_ids_sort_numerically_across_digit_boundary(self):
        events=[dict(event_id=str(i),scenario_id="s",flight_id="f",sensor_id="x",sensor_ns=0,receive_ns=0,lat_deg=0,lon_deg=0,ve_mps=100,vn_mps=0,sigma_pos_m=5,sigma_vel_mps=2) for i in range(1,13)]
        write(self.m,MEAS,events); cb.run(self.t,self.m,self.b)
        outputs=ce._csv(self.b,"output")
        self.assertEqual(outputs[0]["status"],"valid")
    # REQ: OA-CMP-043
    def test_identity_binding_requires_track_to_truth_flight_match(self):
        rows=output_rows(); keys={("s","f",i*1_000_000_000) for i in range(3)}
        outputs={"candidate":rows,"baseline":rows}
        path=os.path.join(self.d,"identity.csv"); header=["system","scenario_id","flight_id","score_ns","track_id","truth_flight_id"]
        bindings=[dict(system=system,scenario_id="s",flight_id="f",score_ns=i*1_000_000_000,track_id=f"track-{system}",truth_flight_id="f") for system in ("candidate","baseline") for i in range(3)]
        write(path,header,bindings)
        self.assertTrue(ce._validate_identity_binding(path,keys,outputs)[0])
        bindings[0]["truth_flight_id"]="other"; write(path,header,bindings)
        self.assertFalse(ce._validate_identity_binding(path,keys,outputs)[0])
    # REQ: OA-CMP-008, OA-CMP-037
    def test_malformed_inputs_fail_closed(self):
        with open(self.t,"w") as f: f.write("scenario_id,flight_id\ns,f\n")
        with self.assertRaisesRegex(ce.EvaluationError,"schema mismatch"): ce.evaluate(self.t,self.m,self.c,self.b,{})
    # REQ: OA-CMP-006, OA-CMP-014, OA-CMP-047
    def test_missing_track_rows_are_counted_for_each_system(self):
        blank=dict(scenario_id="s",flight_id="f",score_ns=1_000_000_000,source_ns="",publish_ns=1_000_000_000,lat_deg="",lon_deg="",ve_mps="",vn_mps="",status="missing")
        write(self.c,OUT,output_rows()[:1]+[blank]); result=ce.evaluate(self.t,self.m,self.c,self.b,{})
        self.assertEqual(result["accounting"]["candidate_missing_rows"],2)
    # REQ: OA-CMP-006
    def test_missing_status_rejects_numeric_estimate_payload(self):
        row=dict(scenario_id="s",flight_id="f",score_ns=0,source_ns="",publish_ns=0,lat_deg=0,lon_deg="",ve_mps="",vn_mps="",status="missing")
        write(self.c,OUT,[row])
        with self.assertRaisesRegex(ce.EvaluationError,"missing output must have blank estimate fields"): ce.evaluate(self.t,self.m,self.c,self.b,{})
    # REQ: OA-CMP-006, OA-CMP-014, OA-CMP-018, OA-CMP-047
    def test_cold_start_is_reported_separately_and_all_missing_fails_coverage(self):
        times=[0,1_000_000_000,2_000_000_000,3_000_000_000,4_000_000_000]
        tr=[dict(scenario_id="s",flight_id="f",time_ns=t,lat_deg=0,lon_deg=0,ve_mps=0,vn_mps=0,phase="cruise") for t in times]
        write(self.t,TRUTH,tr); write(self.m,MEAS,[])
        def rows(statuses):
            return [dict(scenario_id="s",flight_id="f",score_ns=t,source_ns=(t if status!="missing" else ""),publish_ns=t,lat_deg=(0 if status!="missing" else ""),lon_deg=(0 if status!="missing" else ""),ve_mps=(0 if status!="missing" else ""),vn_mps=(0 if status!="missing" else ""),status=status) for t,status in zip(times,statuses)]
        write(self.c,OUT,rows(["valid","valid","missing","missing","missing"])); write(self.b,OUT,rows(["valid"]*5))
        manifest=os.path.join(self.d,"manifest.json")
        with open(manifest,"w") as f: json.dump({"initialization_ns":2_000_000_000,"coast_max_age_ns":2_000_000_000,"scenario_epochs_ns":{"s":0}},f)
        meta={"artifacts":{"manifest":{"path":manifest}},"initialization_ns":2_000_000_000,"coast_max_age_ns":2_000_000_000,"scenario_epochs_ns":{"s":0},"scenario_role":"nominal"}
        result=ce.evaluate(self.t,self.m,self.c,self.b,meta)
        candidate=result["logical_metrics"]["candidate"]
        self.assertEqual(candidate["cold_start_position_error_m"]["count"],2)
        self.assertEqual(candidate["post_initialization_coverage"]["usable_fraction"],0)
        self.assertEqual(result["profile_qualification"]["status"],"unknown_scenario_role")
        self.assertFalse(result["accuracy_qualification"]["pass"])
        write(self.c,OUT,rows(["missing"]*5)); result=ce.evaluate(self.t,self.m,self.c,self.b,meta)
        self.assertEqual(result["logical_metrics"]["candidate"]["cold_start_position_error_m"]["count"],0)
        self.assertEqual(result["profile_qualification"]["status"],"unknown_scenario_role")
    # REQ: OA-CMP-007
    def test_equal_coverage_with_different_missing_postinit_keys_fails_pairing(self):
        times=[0]+list(range(2_000_000_000,1_002_000_000_000,1_000_000_000))
        truth=[dict(scenario_id="s",flight_id="f",time_ns=t,lat_deg=0,lon_deg=0,ve_mps=0,vn_mps=0,phase="cruise") for t in times]
        write(self.t,TRUTH,truth); write(self.m,MEAS,[])
        def system_rows(missing_time):
            return [dict(scenario_id="s",flight_id="f",score_ns=t,source_ns=("" if t==missing_time else t),publish_ns=t,lat_deg=("" if t==missing_time else 0),lon_deg=("" if t==missing_time else 0),ve_mps=("" if t==missing_time else 0),vn_mps=("" if t==missing_time else 0),status=("missing" if t==missing_time else "valid")) for t in times]
        write(self.c,OUT,system_rows(2_000_000_000)); write(self.b,OUT,system_rows(times[-1]))
        manifest=os.path.join(self.d,"manifest.json")
        with open(manifest,"w") as f: json.dump({"initialization_ns":2_000_000_000,"coast_max_age_ns":2_000_000_000,"scenario_epochs_ns":{"s":0}},f)
        meta={"artifacts":{"manifest":{"path":manifest}},"initialization_ns":2_000_000_000,"coast_max_age_ns":2_000_000_000,"scenario_epochs_ns":{"s":0},"scenario_role":"nominal","cohort_id":"nominal-v1"}
        result=ce.evaluate(self.t,self.m,self.c,self.b,meta)
        self.assertEqual(result["profile_qualification"]["candidate_usable_fraction"],.999)
        self.assertEqual(result["profile_qualification"]["baseline_usable_fraction"],.999)
        self.assertFalse(result["profile_qualification"]["usable_keysets_match"])
        self.assertEqual(result["paired_grid"]["common_usable_rows"],998)
        self.assertEqual(result["paired_grid"]["common_usable_rows"],998)
        self.assertEqual(result["paired_grid"]["candidate_excluded_from_pairing"],1)
        self.assertEqual(result["paired_grid"]["baseline_excluded_from_pairing"],1)
        self.assertFalse(result["accuracy_qualification"]["pass"])
    # REQ: OA-CMP-021, OA-CMP-046
    def test_zero_reference_error_has_no_fabricated_ratio(self):
        exact=[dict(scenario_id="s",flight_id="f",score_ns=i*1_000_000_000,source_ns=i*1_000_000_000,publish_ns=i*1_000_000_000,lat_deg=0,lon_deg=i*100/111195,ve_mps=100,vn_mps=0,status="valid") for i in range(3)]
        write(self.c,OUT,exact); write(self.b,OUT,exact); result=ce.evaluate(self.t,self.m,self.c,self.b,{})
        self.assertIsNone(result["ratios"]["rms"])
        self.assertNotEqual(result["accuracy_qualification"]["status"],"qualified_pass")
    # REQ: OA-CMP-031
    def test_one_flight_has_no_cluster_confidence_bounds(self):
        result=ce.evaluate(self.t,self.m,self.c,self.b,{})
        self.assertIsNone(result["paired_confidence"]["rms_ratio"]["low95"])
    # REQ: OA-CMP-029, OA-CMP-030
    def test_hierarchical_bootstrap_clusters_scenarios_and_skips_zero_denominators(self):
        groups={("s1","f1"):([1.0],[2.0]),("s2","f2"):([1.0],[2.0])}
        result=ce._bootstrap_ratio(groups,"rms")
        self.assertEqual(result["scenario_clusters"],2)
        self.assertEqual(result["low95"],0.5)
        zeros={("s1","f1"):([1.0],[0.0]),("s2","f2"):([1.0],[0.0])}
        self.assertIsNone(ce._bootstrap_ratio(zeros,"rms")["high95"])
    # REQ: OA-CMP-018
    def test_profile_gate_separates_nominal_and_diagnostic_dropout(self):
        coverage={"candidate":{"usable_fraction":1.0,"coast_profile_violations":0},"baseline":{"usable_fraction":.999,"coast_profile_violations":0}}
        nominal=ce._profile_gate("nominal",2_000_000_000,2_000_000_000,True,coverage,cohort_id="nominal-v1")
        self.assertEqual(nominal["status"],"nominal_profile_pass")
        dropout=ce._profile_gate("diagnostic_only",2_000_000_000,2_000_000_000,True,coverage,cohort_id="dropout-v1.0.3")
        self.assertEqual(dropout["status"],"diagnostic_only")
        self.assertEqual(dropout["cohort_id"],"dropout-v1.0.3")
        low={"candidate":{"usable_fraction":.95,"coast_profile_violations":0},"baseline":{"usable_fraction":1.0,"coast_profile_violations":0}}
        self.assertEqual(ce._profile_gate("nominal",2_000_000_000,2_000_000_000,True,low)["status"],"nominal_coverage_not_met")
        regression={"candidate":{"usable_fraction":.999,"coast_profile_violations":0},"baseline":{"usable_fraction":1.0,"coast_profile_violations":0}}
        self.assertEqual(ce._profile_gate("nominal",2_000_000_000,2_000_000_000,True,regression)["status"],"availability_regression")
        equal_low={"candidate":{"usable_fraction":.999,"coast_profile_violations":0},"baseline":{"usable_fraction":.999,"coast_profile_violations":0}}
        same_coverage_different_keys=ce._profile_gate("nominal",2_000_000_000,2_000_000_000,True,equal_low,cohort_id="nominal-v1",same_usable_keys=False)
        self.assertEqual(same_coverage_different_keys["candidate_usable_fraction"],same_coverage_different_keys["baseline_usable_fraction"])
        self.assertEqual(same_coverage_different_keys["status"],"paired_usable_key_mismatch")
        result=ce.evaluate(self.t,self.m,self.c,self.b,{"scenario_role":"diagnostic_only","cohort_id":"dropout-v1.0.3"})
        self.assertEqual(result["profile_qualification"]["status"],"unknown_scenario_role")
        self.assertFalse(result["accuracy_qualification"]["pass"])
        self.assertFalse(result["accuracy_qualification"]["all_quality_pass"])
    # REQ: OA-CMP-022, OA-CMP-023, OA-CMP-024, OA-CMP-039
    def test_unknown_comparison_class_is_not_a_qualified_class(self):
        self.assertFalse(ce._known_comparison_class("unregistered_reference"))
        self.assertFalse(ce._claim_class("development_control"))
        self.assertTrue(ce._claim_class("professional_tracker"))
    # REQ: OA-CMP-022, OA-CMP-024, OA-CMP-039
    def test_profile_role_and_cohort_derive_from_frozen_protocol_and_manifest(self):
        protocol={"initialization_and_qualification":{"nominal_scenarios":{"straight":{}},"diagnostic_scenarios":{"dropout":{}}},"seeds_and_holdout":{"development_seeds":{"set_id":"development-set-v1","seed_ids":list(range(32))},"qualification":{"set_id":"qualification-set-v1","seed_ids":list(range(1_000_001,1_000_101))}},"fault_mode_cohort":{"cohort_id":"dropout_fault_v1","scenario":"dropout","seed_ids":list(range(1_000_101,1_000_121))}}
        nominal=ce._derive_profile(protocol,{"scenario_id":"straight","seed":12})
        self.assertEqual((nominal["role"],nominal["cohort_id"]),("nominal","development-set-v1"))
        diagnostic=ce._derive_profile(protocol,{"scenario_id":"dropout","seed":12})
        self.assertEqual(diagnostic["role"],"diagnostic_only")
        fault=ce._derive_profile(protocol,{"scenario_id":"dropout","seed":1_000_101})
        self.assertEqual((fault["role"],fault["cohort_id"]),("diagnostic_only","dropout_fault_v1"))
        self.assertFalse(ce._derive_profile(protocol,{"scenario_id":"straight","seed":1_000_101})["valid"])
        self.assertFalse(ce._derive_profile(protocol,{"scenario_id":"straight","seed":700})["valid"])
        self.assertFalse(ce._derive_profile(protocol,{"scenario_id":"unknown","seed":12})["valid"])

    # REQ: OA-CMP-022, OA-CMP-039
    def test_mutated_role_or_cohort_does_not_match_hashed_scoring_manifest(self):
        protocol={"initialization_and_qualification":{"nominal_scenarios":{"straight":{}},"diagnostic_scenarios":{"dropout":{}}},"seeds_and_holdout":{"development_seeds":{"set_id":"development-set-v1","seed_ids":list(range(32))},"qualification":{"set_id":"qualification-set-v1","seed_ids":list(range(1_000_001,1_000_101))}},"fault_mode_cohort":{"cohort_id":"dropout_fault_v1","scenario":"dropout","seed_ids":list(range(1_000_101,1_000_121))}}
        derived=ce._derive_profile(protocol,{"scenario_id":"straight","seed":1})
        metadata={"scenario_role":"nominal","cohort_id":"development-set-v1"}
        scoring={"scenario_role":"nominal","cohort_id":"development-set-v1"}
        self.assertTrue(ce._profile_binding_valid(derived,metadata,scoring))
        metadata["scenario_role"]="diagnostic_only"
        self.assertFalse(ce._profile_binding_valid(derived,metadata,scoring))
        metadata["scenario_role"]="nominal"; metadata["cohort_id"]="dropout_fault_v1"
        self.assertFalse(ce._profile_binding_valid(derived,metadata,scoring))
        metadata["cohort_id"]="development-set-v1"; scoring["cohort_id"]="tampered"
        self.assertFalse(ce._profile_binding_valid(derived,metadata,scoring))

    # REQ: OA-CMP-012, OA-CMP-044, OA-CMP-045
    def test_metric_statistics_use_nearest_rank_and_empty_is_null(self):
        stats=ce._stats(list(range(1,21)))
        self.assertEqual(stats["count"],20); self.assertEqual(stats["p95"],19); self.assertEqual(stats["p99"],20)
        self.assertEqual(ce._stats([]),{"count":0,"rms":None,"p95":None,"p99":None,"mean":None})

    # REQ: OA-CMP-026, OA-CMP-027, OA-CMP-040, OA-CMP-041
    def test_timing_ledger_is_hash_bound_and_drained(self):
        path=os.path.join(self.d,"timing.json")
        ledger={"clock_kind":"monotonic_wall","elapsed_ns":2_000_000_000,"work_units":4,"drained":True,"input_fingerprint":"abc","loss_counters":{"missing":0}}
        with open(path,"w") as f: json.dump(ledger,f)
        spec={"path":path,"sha256":ce._sha256(path)}
        parsed=ce._load_timing_ledger(spec,"abc")
        self.assertEqual(ce._ledger_throughput(parsed),2.0)
        ledger["drained"]=False
        with open(path,"w") as f: json.dump(ledger,f)
        self.assertIsNone(ce._load_timing_ledger({"path":path,"sha256":ce._sha256(path)},"abc"))
        ledger["drained"]=True; ledger["loss_counters"]={"missing":1}
        with open(path,"w") as f: json.dump(ledger,f)
        self.assertIsNone(ce._load_timing_ledger({"path":path,"sha256":ce._sha256(path)},"abc"))
        self.assertIsNone(ce._load_timing_ledger(spec,"abc"))

    # REQ: OA-CMP-037, OA-CMP-038
    def test_cli_returns_error_for_bad_metadata_and_writes_json_on_success(self):
        bad=os.path.join(self.d,"bad.json"); out=os.path.join(self.d,"report.json")
        with open(bad,"w") as f: f.write("{")
        with contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(ce.main(["--truth",self.t,"--measurements",self.m,"--candidate",self.c,"--baseline",self.b,"--metadata",bad,"--out",out]),2)
        good=os.path.join(self.d,"meta.json")
        with open(good,"w") as f: json.dump({},f)
        self.assertEqual(ce.main(["--truth",self.t,"--measurements",self.m,"--candidate",self.c,"--baseline",self.b,"--metadata",good,"--out",out]),0)
        with open(out) as f: report=json.load(f)
        self.assertIn("logical_metrics",report); self.assertIn("accuracy_qualification",report); self.assertIn("accounting",report)

    # REQ: OA-CMP-025
    def test_protocol_artifact_must_match_canonical_bytes(self):
        canonical=os.path.join(self.d,"canonical.json"); artifact=os.path.join(self.d,"protocol.json")
        with open(canonical,"w") as f: f.write('{"protocol":"v1"}\n')
        with open(artifact,"w") as f: f.write('{"protocol":"v1"}\n')
        self.assertTrue(ce._files_match(canonical,artifact))
        with open(artifact,"w") as f: f.write('{"protocol":"other"}\n')
        self.assertFalse(ce._files_match(canonical,artifact))

if __name__=="__main__": unittest.main()
