"""Synthetic analytic and negative-control tests for motion-study v1."""
import csv
import hashlib
import json
import math
import os
import tempfile
import unittest
from unittest import mock
from pathlib import Path

import motion_study as ms

TRUTH=ms.TRUTH_FIELDS
MEAS=ms.MEAS_FIELDS
DIAG=ms.DIAG_FIELDS
OUT=["scenario_id","flight_id","score_ns","source_ns","publish_ns","lat_deg","lon_deg","ve_mps","vn_mps","status"]
PARAMS={"H_Q_CV":1.0,"H_Q_CT":1.0,"H_Q_OMEGA":1.0,"H_TAU_CV_S":3.0,"H_TAU_CT_S":1.0,"CT_OMEGA_STD_FLOOR":0.001}
ZERO_LOSSES={k:0 for k in ("refused","duplicate","late_window","late_ring","time_future_dropped","time_steps_back","jump_rejected","numerical_refused","capacity_refused","work_refused","work_counter_overflow","held_unapplied","rows_unrepresentable","forecast_numerical","forecast_unrepresentable","tracker_output_overflow")}

def write_csv(path, fields, rows):
    with open(path,"w",newline="",encoding="utf-8") as f:
        w=csv.DictWriter(f,fieldnames=fields,lineterminator="\n");w.writeheader();w.writerows(rows)

def dump(path, obj):
    with open(path,"w",encoding="utf-8") as f: json.dump(obj,f,sort_keys=True,separators=(",",":"));f.write("\n")

def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()

class MotionStudyTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory(); self.root=Path(self.temp.name)
    def tearDown(self): self.temp.cleanup()

    # REQ: OA-ACC-150, OA-ACC-157
    def test_preregistration_is_valid_and_claim_scope_is_development_only(self):
        study=ms.read_json(ms.STUDY_PATH)
        self.assertEqual(study["status"],"preregistered_development_experiment")
        self.assertEqual(study["claim_scope"],"developmental_model_check_only")
        self.assertEqual([x["H_Q_CV"] for x in study["training"]["settings"]],[1.0,.1,.01])
        self.assertEqual(study["development_check"]["seed_ids"],[4,5,6])

    # REQ: OA-ACC-151, OA-ACC-157
    def test_mirrored_onset_exit_truth_at_hand_expected_times(self):
        expected={0:(0,0,150,0),60:(9000,0,150,0),105:(9000+150/math.radians(2),150/math.radians(2),0,150),120:(9000+(150/math.radians(2))*math.sin(math.radians(120)),(150/math.radians(2))*(1-math.cos(math.radians(120))),-75,150*math.sin(math.radians(120)))}
        for sign in (-1,1):
            for t,vals in expected.items():
                got=ms.onset_exit_truth(t,sign)
                self.assertAlmostEqual(got[0],vals[0],places=6);self.assertAlmostEqual(got[2],vals[2],places=6)
                self.assertAlmostEqual(got[1],sign*vals[1],places=6);self.assertAlmostEqual(got[3],sign*vals[3],places=6)

    # REQ: OA-ACC-157, OA-ACC-167
    def test_onset_exit_truth_validator_checks_both_parities_and_geographic_velocity(self):
        truth={}; epoch=0
        lat0=40.0; lon0=-75.0; f=1/298.257223563; e2=f*(2-f); a=6378137.0
        p0=math.radians(lat0); n=a/math.sqrt(1-e2*math.sin(p0)**2); m=a*(1-e2)/(1-e2*math.sin(p0)**2)**1.5
        for idx in range(20):
            flight=str(idx); x0=((idx%20)-9.5)*1000; y0=(idx//20)*2000; sign=1 if idx%2==0 else -1
            for t in range(151):
                dx,dy,vx,vy=ms.onset_exit_truth(t,sign)
                lat=lat0+(y0+dy)/(m*math.pi/180); lon=lon0+(x0+dx)/(n*math.cos(p0)*math.pi/180)
                p=math.radians(lat); q=math.sqrt(1-e2*math.sin(p)**2)
                ve=vx*(a/q*math.cos(p))/(n*math.cos(p0)); vn=vy*(a*(1-e2)/q**3)/m
                if idx==0 and t==120: self.assertGreater(abs(ve-vx),.05)
                phase="straight_before" if t<60 else ("turn" if t<=120 else "straight_after")
                truth[("onset-exit",flight,t*1_000_000_000)]={"lat_deg":lat,"lon_deg":lon,"ve_mps":ve,"vn_mps":vn,"phase":phase}
        run={"manifest":{"diagnostic_only":True,"reference_validation_allowed":False,"diagnostic_turn_sign_rule":"even_positive_odd_negative","grid_start_ns":epoch,"origin_lat_deg":lat0,"origin_lon_deg":lon0},"truth":truth}
        checked=ms._validate_onset_truth(run)
        self.assertEqual(len(checked),80)
        damaged=dict(run);damaged["truth"]=dict(truth);damaged["truth"][("onset-exit","1",105_000_000_000)]=dict(truth[("onset-exit","1",105_000_000_000)],vn_mps=0)
        with self.assertRaisesRegex(ms.StudyError,"analytic truth mismatch"):
            ms._validate_onset_truth(damaged)

    # REQ: OA-ACC-158
    def test_covariance_mahalanobis_and_invalid_matrices(self):
        self.assertAlmostEqual(ms._mahalanobis2(2,0,4,0,4),1.0)
        with self.assertRaisesRegex(ms.StudyError,"positive definite"):
            ms._mahalanobis2(1,1,1,2,1)
        with self.assertRaisesRegex(ms.StudyError,"positive definite"):
            ms._mahalanobis2(1,1,0,0,1)

    # REQ: OA-ACC-168
    def test_covariance_error_uses_candidate_local_enu_frame(self):
        truth={"lat_deg":40.0,"lon_deg":-75.0}
        estimate={"lat_deg":40.1,"lon_deg":-74.5}
        covariance=(100.0,0.0,1.0)
        east,north=ms.local_error_enu(truth,estimate)
        candidate_q2=ms._candidate_local_mahalanobis(truth,estimate,covariance)
        origin_e,origin_n=ms._enu_error_at_frame(truth,estimate,truth["lat_deg"],truth["lon_deg"])
        origin_q2=ms._mahalanobis2(origin_e,origin_n,*covariance)
        self.assertAlmostEqual(candidate_q2,ms._mahalanobis2(east,north,*covariance))
        self.assertGreater(abs(candidate_q2-origin_q2),1.0)

    # REQ: OA-ACC-168
    def test_turn_rate_rms_uses_clockwise_positive_tracker_sign(self):
        for scenario,flight,expected_deg in (("turn","0",-2.0),("onset-exit","0",-2.0),("onset-exit","1",2.0)):
            key=(scenario,flight,1)
            truth={"lat_deg":40.0,"lon_deg":-75.0,"ve_mps":0.0,"vn_mps":0.0,"phase":"turn"}
            rate=math.radians(expected_deg)
            diag={"status":"valid","turn_rate_rps":rate,"_covariance_valid":True,"pos_cov_ee_m2":100.0,"pos_cov_en_m2":0.0,"pos_cov_nn_m2":100.0}
            out={"status":"valid","lat_deg":40.0,"lon_deg":-75.0,"ve_mps":0.0,"vn_mps":0.0}
            run={"truth":{key:truth},"diag":{key:diag},"outputs":{key:out},"manifest":{"seed":1}}
            result=ms._score_rows(run,run,0,0)
            self.assertEqual(result["paired_metrics"]["candidate"]["turn_rate"]["rms"],0.0)
            wrong=ms._expected_turn_rate_deg_s(scenario,"turn",flight)*-1.0
            self.assertEqual(wrong,-expected_deg)

    # REQ: OA-ACC-158
    def test_seed_then_flight_bootstrap_is_repeatable_and_uses_whole_blocks(self):
        clusters={seed:{str(f):{"n":10,"hits50":10,"hits90":10,"hits95":10} for f in range(10)} for seed in (1,2,3)}
        first=ms._cohort_bootstrap(clusters,"hits50");second=ms._cohort_bootstrap(clusters,"hits50")
        self.assertEqual(first,second);self.assertEqual(first["hierarchy"],"seed_blocks_then_whole_flights")
        self.assertEqual(first["lower95"],1.0);self.assertEqual(first["unique_clusters"],30)
        small={1:{str(f):{"n":1,"hits50":1} for f in range(2)}}
        self.assertFalse(ms._cohort_bootstrap(small,"hits50")["pass_data_minimum"])

    # REQ: OA-ACC-157
    def test_transition_lag_requires_consecutive_sustained_samples_and_censor_is_explicit(self):
        self.assertEqual(ms._cross_lag([(60,0),(61,1),(62,1),(63,1)],60,90,.5,True),{"lag_s":1,"censored":False})
        self.assertTrue(ms._cross_lag([(60,1),(62,1),(63,1)],60,90,.5,True)["censored"])
        self.assertTrue(ms._cross_lag([(60,.5),(61,.5)],60,90,.5,False)["censored"])

    # REQ: OA-ACC-155, OA-ACC-160
    def test_loss_gate_checks_counters_input_conservation_and_pending(self):
        manifest={"loss_counters":dict(ZERO_LOSSES),"event_queue_pending_before_drain":0,"event_queue_pending_at_end":0,"measurements_offered":12,"measurements_applied":12,"measurement_refused":0,"duplicate_measurements":0,"late_window":0,"late_ring":0,"time_future_dropped":0,"time_steps_back":0,"jump_rejected":0,"numerical_refused":0,"capacity_refused":0,"work_refused":0,"rows_unrepresentable":0,"forecast_numerical_failures":0,"forecast_unrepresentable_failures":0,"tracker_output_overflow":0}
        self.assertTrue(ms._loss_gate(manifest)[0])
        manifest["measurements_applied"]=11
        self.assertFalse(ms._loss_gate(manifest)[0])
        manifest["measurements_applied"]=12;manifest["event_queue_pending_at_end"]=1
        self.assertFalse(ms._loss_gate(manifest)[0])
        manifest["event_queue_pending_at_end"]=0;manifest["late_ring"]=1
        self.assertFalse(ms._loss_gate(manifest)[0])

    # REQ: OA-ACC-152, OA-ACC-153
    def test_selection_requires_exact_preregistered_case_matrix_and_single_winner(self):
        scores=[]; scenarios=("straight","turn","acceleration","crossing","delayed","dropout","onset-exit")
        for setting,hq in (("hqcv-1",1.0),("hqcv-0_1",.1),("hqcv-0_01",.01)):
            for seed in (1,2,3):
                for scenario in scenarios:
                    role="nominal" if scenario in ("straight","turn","acceleration","crossing","delayed") else "diagnostic_only"
                    containment={scenario:{f"{seed}:{flight}":{"n":10,"hits50":10,"hits90":10,"hits95":10} for flight in range(20)}}
                    window={k:{f"{seed}:{flight}":{"n":10,"hits50":10,"hits90":10,"hits95":10} for flight in range(20)} for k in ("even:onset","even:exit","odd:onset","odd:exit")} if scenario=="onset-exit" else {}
                    scores.append({"study_id":"openatca-motion-study-v1","seed":seed,"scenario":scenario,"role":role,"setting_id":setting,"H_Q_CV":hq,"default_H_Q_CV":1.0,"selection_pass":True,"input_hashes_match":True,"candidate_native_guard_pass":True,"default_native_guard_pass":True,"hashes":{"candidate_source":"a"*64,"default_source":"a"*64,"candidate_source_head":"h"*40,"default_source_head":"h"*40,"candidate_normalized_params":"e"*64,"default_normalized_params":"e"*64,"default_params_source":"e"*64,"candidate_truth":hashlib.sha256(f"truth-{seed}-{scenario}".encode()).hexdigest(),"candidate_measurements":hashlib.sha256(f"meas-{seed}-{scenario}".encode()).hexdigest(),"default_truth":hashlib.sha256(f"truth-{seed}-{scenario}".encode()).hexdigest(),"default_measurements":hashlib.sha256(f"meas-{seed}-{scenario}".encode()).hexdigest()},"parameters_except_H_Q_CV":{"H_Q_CT":1},"paired_metrics":{"candidate":{"position":{"rms":hq,"count":100}}},"containment":{"clusters":containment,"onset_exit_window_clusters":window}})
        selected=ms.choose_training(scores)
        self.assertEqual(selected["selected_setting_id"],"hqcv-0_01")
        failed_guard_scores=json.loads(json.dumps(scores))
        failed_guard=next(row for row in failed_guard_scores if row["setting_id"]=="hqcv-0_1")
        failed_guard["candidate_native_guard_pass"]=False
        failed_guard["selection_pass"]=False
        guarded_selection=ms.choose_training(failed_guard_scores)
        self.assertEqual(guarded_selection["selected_setting_id"],"hqcv-0_01")
        failed_setting=next(row for row in guarded_selection["setting_results"] if row["setting_id"]=="hqcv-0_1")
        self.assertFalse(failed_setting["training_pass"])
        scores[0]["hashes"]["default_source"]="b"*64
        with self.assertRaisesRegex(ms.StudyError,"source lineage differs"):
            ms.choose_training(scores)
        scores[0]["hashes"]["default_source"]="a"*64
        scores_path=self.root/"selection-scores.json"; out=self.root/"selection.json"
        scores_path.write_text(json.dumps(scores),encoding="utf-8")
        self.assertEqual(ms.main(["select","--scores",str(scores_path),"--out",str(out)]),0)
        self.assertEqual(json.loads(out.read_text(encoding="utf-8"))["selected_setting_id"],"hqcv-0_01")
        with self.assertRaisesRegex(ms.StudyError,"incomplete or duplicated"):
            ms.choose_training(scores[:-1])

    # REQ: OA-ACC-153, OA-ACC-154
    def test_development_check_is_exactly_selected_candidate_and_default_pairing(self):
        selection={"status":"selected_for_single_development_check","selected_setting_id":"hqcv-0_1","setting_results":[{"setting_id":"hqcv-0_1","candidate_source_sha256":"a"*64,"source_head":"h"*40,"normalized_params_sha256":"e"*64,"parameters_except_H_Q_CV":{"H_Q_CT":1}}]}
        scores=[]
        for seed in (4,5,6):
            for scenario in ("straight","turn","acceleration","crossing","dropout","delayed","onset-exit"):
                clusters={scenario:{f"{seed}:{flight}":{"n":10,"hits50":10,"hits90":10,"hits95":10} for flight in range(20)}}
                windows={k:{f"{seed}:{flight}":{"n":10,"hits50":10,"hits90":10,"hits95":10} for flight in range(20)} for k in ("even:onset","even:exit","odd:onset","odd:exit")} if scenario=="onset-exit" else {}
                scores.append({"seed":seed,"scenario":scenario,"role":"diagnostic_only" if scenario in ("dropout","onset-exit") else "nominal","setting_id":"hqcv-0_1","H_Q_CV":.1,"default_H_Q_CV":1.0,"parameters_except_H_Q_CV":{"H_Q_CT":1},"input_hashes_match":True,"candidate_native_guard_pass":True,"default_native_guard_pass":True,"hashes":{"candidate_source":"a"*64,"default_source":"a"*64,"candidate_source_head":"h"*40,"default_source_head":"h"*40,"candidate_normalized_params":"e"*64,"default_normalized_params":"e"*64},"selection_pass":True,"containment":{"clusters":clusters,"onset_exit_window_clusters":windows}})
        result=ms.validate_check(selection,scores)
        self.assertEqual(result["case_count"],21);self.assertFalse(result["retune_permitted"])
        scores[0]["default_native_guard_pass"]=False
        scores[0]["selection_pass"]=False
        failed=ms.validate_check(selection,scores)
        self.assertFalse(failed["all_preregistered_case_gates_pass"])
        scores[0]["default_native_guard_pass"]=True
        scores[0]["selection_pass"]=True
        scores[0]["setting_id"]="hqcv-1"
        with self.assertRaisesRegex(ms.StudyError,"exactly the selected setting"):
            ms.validate_check(selection,scores)

    # REQ: OA-ACC-157
    def test_transition_gate_checks_mu_and_turn_rate_for_each_mirrored_direction(self):
        def build(candidate):
            diag={}
            for flight in ("0","1"):
                for t in range(60,64):
                    start=61 if candidate else 60
                    crossed=t>=start
                    diag[("onset-exit",flight,t*1_000_000_000)]={"status":"valid","mu_ct":.8 if crossed else .1,"turn_rate_rps":math.radians(1 if crossed else 0)}
                for t in range(120,124):
                    diag[("onset-exit",flight,t*1_000_000_000)]={"status":"valid","mu_ct":.1,"turn_rate_rps":0.0}
            return {"truth":{k:None for k in diag},"diag":diag}
        result=ms._transition_metrics(build(True),build(False),0)
        self.assertTrue(all(result["gates"].values()))
        first=result["by_flight"]["0:onset"]
        self.assertEqual(first["candidate"]["mu_ct"]["lag_s"],1.0)
        bad=build(True)
        for row in bad["diag"].values():
            if row.get("mu_ct",0)>.5: row["mu_ct"]=.1
        failed=ms._transition_metrics(bad,build(False),0)
        self.assertFalse(failed["gates"]["lag_even:onset"])
        self.assertTrue(failed["by_flight"]["0:onset"]["candidate"]["mu_ct"]["censored"])

    # REQ: OA-ACC-150, OA-ACC-160
    def test_cli_emits_json_and_rejects_malformed_score_list(self):
        bad=self.root/"scores.json"; out=self.root/"out.json"; bad.write_text("{}",encoding="utf-8")
        with self.assertRaises(ms.StudyError): ms.read_score_list(bad)
        source=self.root/"scores.json"; source.write_text("[]",encoding="utf-8")
        self.assertEqual(ms.main(["select","--scores",str(source),"--out",str(out)]),2)
        self.assertEqual(ms.main(["score","--candidate-dir",str(self.root),"--candidate-build",str(self.root/"missing"),"--default-dir",str(self.root),"--default-build",str(self.root/"missing"),"--out",str(out)]),2)

    # REQ: OA-ACC-156
    def test_diagnostic_only_dropout_does_not_enter_nominal_calibration_pool(self):
        scores=[]
        for seed in (1,2,3):
            for scenario in ("straight","turn","acceleration","crossing","delayed","dropout"):
                scores.append({"study_id":"openatca-motion-study-v1","seed":seed,"scenario":scenario,"role":"diagnostic_only" if scenario=="dropout" else "nominal","setting_id":"hqcv-1","default_H_Q_CV":1.0,"selection_pass":True,"input_hashes_match":True,"candidate_native_guard_pass":True,"default_native_guard_pass":True,"hashes":{"candidate_source":"a"*64,"default_source":"a"*64,"candidate_source_head":"h"*40,"default_source_head":"h"*40,"candidate_normalized_params":"e"*64,"default_normalized_params":"e"*64,"default_params_source":"e"*64,"candidate_truth":hashlib.sha256(f"truth-{seed}-{scenario}".encode()).hexdigest(),"candidate_measurements":hashlib.sha256(f"meas-{seed}-{scenario}".encode()).hexdigest(),"default_truth":hashlib.sha256(f"truth-{seed}-{scenario}".encode()).hexdigest(),"default_measurements":hashlib.sha256(f"meas-{seed}-{scenario}".encode()).hexdigest()},"parameters_except_H_Q_CV":{"H_Q_CT":1},"paired_metrics":{"candidate":{"position":{"rms":1.0,"count":200}}},"containment":{"clusters":{scenario:{f"{seed}:{i}":{"n":10,"hits50":10,"hits90":10,"hits95":10} for i in range(20)}}}})
        with self.assertRaisesRegex(ms.StudyError,"all three preregistered settings"):
            ms.choose_training(scores)

    def _make_run(self, name, hq, offset_m=0.0, velocity_offset=0.0, native_exit=0):
        root=self.root/name; root.mkdir()
        truth=[];diag=[];outputs=[];measurements=[];lonoff=offset_m/111319.49
        for t in range(301):
            for flight in range(20):
                lon=t*100.0/111319.49
                truth.append(dict(scenario_id="straight",flight_id=str(flight),time_ns=t*1_000_000_000,lat_deg=0,lon_deg=lon,ve_mps=100,vn_mps=0,phase="straight"))
                diag.append(dict(scenario_id="straight",flight_id=str(flight),score_ns=t*1_000_000_000,source_ns=t*1_000_000_000,track_id=str(flight),status="valid",pos_cov_ee_m2=100,pos_cov_en_m2=0,pos_cov_nn_m2=100,mu_cv=.9,mu_ct=.1,turn_rate_rps=0))
                outputs.append(dict(scenario_id="straight",flight_id=str(flight),score_ns=t*1_000_000_000,source_ns=t*1_000_000_000,publish_ns=t*1_000_000_000,lat_deg=0,lon_deg=lon+lonoff,ve_mps=100+velocity_offset,vn_mps=0,status="valid"))
        for t in range(2):
            for flight in range(20):
                event=t*20+flight+1
                measurements.append(dict(event_id=str(event),scenario_id="straight",flight_id=str(flight),sensor_id="x",sensor_ns=t*1_000_000_000,receive_ns=t*1_000_000_000,lat_deg=0,lon_deg=t*100/111319.49,ve_mps=100,vn_mps=0,sigma_pos_m=5,sigma_vel_mps=2))
        write_csv(root/"truth.csv",TRUTH,truth); write_csv(root/"measurements.csv",MEAS,measurements); write_csv(root/"diagnostics.csv",DIAG,diag); write_csv(root/"candidate.csv",OUT,outputs)
        src=root/"runner.rs";src.write_text("source",encoding="utf-8");params_src=root/"params.rs";params_src.write_text(f"pub const H_Q_CV: f64 = {hq};\npub const OTHER: f64 = 2.0;\n",encoding="utf-8");binary=root/"runner";binary.write_bytes(b"binary")
        head="a"*40; diff_path=root/"source.diff"; diff_text="" if hq==1 else "diff --git a/crates/oa-track-core/src/params.rs b/crates/oa-track-core/src/params.rs\n--- a/crates/oa-track-core/src/params.rs\n+++ b/crates/oa-track-core/src/params.rs\n@@ -1 +1 @@\n-pub const H_Q_CV: f64 = 1.0;\n+pub const H_Q_CV: f64 = 0.1;\n";diff_path.write_text(diff_text,encoding="utf-8");diff=sha(diff_path);runner_hash=sha(src);params_hash=sha(params_src)
        log=root/"guard.log";log.write_text("test result: ok. 1 passed; 0 failed; 0 ignored;",encoding="utf-8")
        result=root/"guard.json";dump(result,{"exit_code":native_exit,"head":head,"source_diff_sha256":diff,"command":["guard.sh","cargo","+1.89.0","test","-p","oa-track","--test","core","--locked","--offline","accelerating_flight_is_tracked","--","--exact","--nocapture","--test-threads=1"]})
        build={"source_head":head,"source_status":"" if hq==1 else " M crates/oa-track-core/src/params.rs\n","source_diff_sha256":diff,"source_diff_path":str(diff_path),"params_source_sha256":params_hash,"runner_source_sha256":runner_hash,"binary_sha256":sha(binary),"binary_path":str(binary),"params_source_path":str(params_src),"runner_source_path":str(src),"expected_motion_parameters":dict(PARAMS,H_Q_CV=hq),"native_guard_result":{"path":str(result),"sha256":sha(result)},"native_guard_log":{"path":str(log),"sha256":sha(log)},"native_guard_log_sha256":sha(log),"native_regression_pass":native_exit==0}
        build_path=root/"build.json";dump(build_path,build)
        manifest={"schema_version":1,"model_id":"adsb_kinematic_v2","scenario_id":"straight","seed":1,"aircraft_count":20,"scenario_duration_s":300,"grid_count":301,"grid_step_ns":1_000_000_000,"grid_start_ns":0,"origin_lat_deg":40,"origin_lon_deg":-75,"initialization_ns":2_000_000_000,"truth_rows":6020,"measurement_rows":40,"diagnostic_rows":6020,"diagnostic_missing_rows":0,"motion_parameters":dict(PARAMS,H_Q_CV=hq),"motion_params_source_sha256":params_hash,"source_implementation_sha256":runner_hash,"loss_counters":dict(ZERO_LOSSES),"event_queue_pending_before_drain":0,"event_queue_pending_at_end":0,"measurements_offered":40,"measurements_applied":40,"measurement_refused":0,"duplicate_measurements":0,"late_window":0,"late_ring":0,"time_future_dropped":0,"time_steps_back":0,"jump_rejected":0,"numerical_refused":0,"capacity_refused":0,"work_refused":0,"rows_unrepresentable":0,"forecast_numerical_failures":0,"forecast_unrepresentable_failures":0,"tracker_output_overflow":0}
        dump(root/"manifest.json",manifest)
        return root,build_path

    # REQ: OA-ACC-150, OA-ACC-155, OA-ACC-158, OA-ACC-160
    def test_score_pair_binds_inputs_build_covariance_and_native_guard(self):
        candidate,cb=self._make_run("candidate",.1,0,0)
        default,db=self._make_run("default",1,2,1)
        # Candidate and default truth/measurements are byte-identical by construction.
        report=ms.score_pair(candidate,cb,default,db)
        self.assertTrue(report["input_hashes_match"])
        self.assertTrue(report["gates"]["native_acceleration_regression_pass"])
        self.assertTrue(report["gates"]["full_candidate_postinit_coverage"])
        self.assertEqual(report["paired_metrics"]["candidate"]["position"]["count"],5980)
        # Nonzero native guard exit preserves metrics but blocks this configuration.
        candidate2,cb2=self._make_run("candidate_failed_guard",.1,0,0,native_exit=1)
        failed=ms.score_pair(candidate2,cb2,default,db)
        self.assertFalse(failed["gates"]["native_acceleration_regression_pass"])
        self.assertEqual(failed["paired_metrics"]["candidate"]["position"]["count"],5980)

    # REQ: OA-ACC-150, OA-ACC-155
    def test_score_pair_rejects_mismatched_input_bytes_and_incomplete_outputs(self):
        candidate,cb=self._make_run("candidate",.1)
        default,db=self._make_run("default",1,2,1)
        with open(candidate/"measurements.csv","a") as f: f.write("\n")
        with self.assertRaisesRegex(ms.StudyError,"byte-identical"):
            ms.score_pair(candidate,cb,default,db)

    # REQ: OA-ACC-165
    def test_paired_builds_require_same_lineage_params_only_change_and_both_guards(self):
        candidate,cb=self._make_run("candidate",.1)
        default,db=self._make_run("default",1)
        build=json.loads(cb.read_text(encoding="utf-8"));build["source_head"]="f"*40;dump(cb,build)
        with self.assertRaisesRegex(ms.StudyError,"share source head"):
            ms.score_pair(candidate,cb,default,db)

        candidate,cb=self._make_run("candidate_runner",.1)
        default,db=self._make_run("default_runner",1)
        build=json.loads(cb.read_text(encoding="utf-8"));runner=Path(build["runner_source_path"]);runner.write_text("different implementation",encoding="utf-8");build["runner_source_sha256"]=sha(runner);dump(cb,build)
        manifest=json.loads((candidate/"manifest.json").read_text(encoding="utf-8"));manifest["source_implementation_sha256"]=sha(runner);dump(candidate/"manifest.json",manifest)
        with self.assertRaisesRegex(ms.StudyError,"runner implementation"):
            ms.score_pair(candidate,cb,default,db)

        candidate,cb=self._make_run("candidate_params_body",.1)
        default,db=self._make_run("default_params_body",1)
        params=Path(json.loads(cb.read_text(encoding="utf-8"))["params_source_path"])
        params.write_text("pub const H_Q_CV: f64 = 0.1;\npub const OTHER: f64 = 3.0;\n",encoding="utf-8")
        build=json.loads(cb.read_text(encoding="utf-8"));build["params_source_sha256"]=sha(params);dump(cb,build)
        manifest=json.loads((candidate/"manifest.json").read_text(encoding="utf-8"));manifest["motion_params_source_sha256"]=sha(params);dump(candidate/"manifest.json",manifest)
        with self.assertRaisesRegex(ms.StudyError,"parameter source differs"):
            ms.score_pair(candidate,cb,default,db)

        candidate,cb=self._make_run("candidate_default_guard",.1)
        default,db=self._make_run("default_failed_guard",1,native_exit=1)
        report=ms.score_pair(candidate,cb,default,db)
        self.assertFalse(report["gates"]["native_acceleration_regression_pass"])
        self.assertEqual(report["paired_metrics"]["candidate"]["position"]["count"],5980)

    # REQ: OA-ACC-166
    def test_manifest_model_ids_match_each_preregistered_scenario_family(self):
        for scenario in ("straight","turn","acceleration","crossing","dropout","delayed"):
            self.assertEqual(ms._expected_model(scenario),"adsb_kinematic_v2")
        self.assertEqual(ms._expected_model("onset-exit"),"adsb_motion_study_v1")
        self.assertIsNone(ms._expected_model("unknown"))
        run,build=self._make_run("standard_manifest_model",.1)
        self.assertEqual(ms._load_run(run,build)["manifest"]["model_id"],"adsb_kinematic_v2")
        manifest=json.loads((run/"manifest.json").read_text(encoding="utf-8"));manifest["model_id"]="adsb_motion_study_v1";dump(run/"manifest.json",manifest)
        with self.assertRaisesRegex(ms.StudyError,"runner manifest differs"):
            ms._load_run(run,build)

    # REQ: OA-ACC-161
    def test_strict_csv_headers_numeric_ids_and_event_order(self):
        path=self.root/"truth.csv"
        rows=[dict(scenario_id="s",flight_id="9",time_ns=0,lat_deg=0,lon_deg=0,ve_mps=0,vn_mps=0,phase="p"),dict(scenario_id="s",flight_id="10",time_ns=0,lat_deg=0,lon_deg=0,ve_mps=0,vn_mps=0,phase="p")]
        write_csv(path,TRUTH,rows); self.assertEqual(len(ms._truth_map(ms._csv(path,TRUTH))),2)
        write_csv(path,list(reversed(TRUTH)),rows)
        with self.assertRaisesRegex(ms.StudyError,"exact CSV header"):
            ms._csv(path,TRUTH)
        with self.assertRaisesRegex(ms.StudyError,"non-finite"):
            ms.number({"x":"nan"},"x","fixture")
        input_rows=[dict(event_id="9",receive_ns=0,sensor_ns=0,lat_deg=0,lon_deg=0,ve_mps=0,vn_mps=0,sigma_pos_m=1,sigma_vel_mps=1),dict(event_id="10",receive_ns=0,sensor_ns=0,lat_deg=0,lon_deg=0,ve_mps=0,vn_mps=0,sigma_pos_m=1,sigma_vel_mps=1)]
        self.assertEqual(ms._validate_measurements(input_rows),2)
        with self.assertRaisesRegex(ms.StudyError,"duplicate event_id"):
            ms._validate_measurements([input_rows[0],dict(input_rows[1],event_id="9")])

    # REQ: OA-ACC-162, OA-ACC-163, OA-ACC-164
    def test_score_report_preserves_status_fractions_phase_metrics_and_guard_log_evidence(self):
        candidate,cb=self._make_run("candidate",.1,0,0)
        default,db=self._make_run("default",1,2,1)
        report=ms.score_pair(candidate,cb,default,db)
        self.assertEqual(report["status_fractions"]["candidate_valid"],1.0)
        self.assertEqual(report["status_fractions"]["candidate_missing"],0.0)
        self.assertIn("straight",report["phase_paired_metrics"])
        self.assertEqual(report["full_grid_metrics"]["candidate"]["position"]["count"],6020)
        self.assertEqual(report["cold_start_metrics"]["candidate"]["position"]["count"],40)
        self.assertEqual(report["post_initialization_metrics"]["candidate"]["position"]["count"],5980)
        self.assertTrue(report["hashes"]["candidate_native_guard"] ["one_test_pass_summary"])
        log=candidate/"guard.log";log.write_text("test result: ok. 0 passed; 0 failed; 0 ignored;",encoding="utf-8")
        build=json.loads(cb.read_text(encoding="utf-8"));build["native_guard_log"]["sha256"]=sha(log);build["native_guard_log_sha256"]=sha(log);build["native_regression_pass"]=True
        dump(cb,build)
        failed=ms.score_pair(candidate,cb,default,db)
        self.assertFalse(failed["gates"]["native_acceleration_regression_pass"])
        self.assertEqual(failed["paired_metrics"]["candidate"]["position"]["count"],5980)

    # REQ: OA-ACC-161
    def test_study_modules_import_only_standard_library(self):
        import ast
        for source in (Path(ms.__file__),Path(__file__)):
            tree=ast.parse(source.read_text(encoding="utf-8"))
            imports=[]
            for node in ast.walk(tree):
                if isinstance(node,ast.Import): imports.extend(alias.name.split(".")[0] for alias in node.names)
                elif isinstance(node,ast.ImportFrom) and node.module: imports.append(node.module.split(".")[0])
        self.assertTrue(set(imports)<=set(__import__("sys").stdlib_module_names)|{"motion_study","tools"},f"non-stdlib imports in {source}: {sorted(set(imports)-set(__import__('sys').stdlib_module_names)-{'motion_study','tools'})}")

    # REQ: OA-ACC-159
    def test_unknown_and_near_zero_ratios_are_not_fabricated(self):
        self.assertIsNone(ms._ratio(1.0,0.0));self.assertAlmostEqual(ms._ratio(.5,1.0),.5)
        study=ms.read_json(ms.STUDY_PATH)
        self.assertFalse(study["uncertainty"]["nis_claim"])
        self.assertIn("independent_physical_accuracy",study["prohibited_claims"])

    # REQ: OA-ACC-169
    def test_study_file_hash_is_pinned_and_changed_bytes_fail_closed(self):
        self.assertEqual(hashlib.sha256(ms.STUDY_PATH.read_bytes()).hexdigest(),ms.STUDY_V1_SHA256)
        self.assertEqual(ms._study()["uncertainty"]["bootstrap"],ms.read_json(ms.STUDY_PATH)["uncertainty"]["bootstrap"])
        with tempfile.TemporaryDirectory() as d:
            altered=Path(d)/"study.json"; altered.write_bytes(ms.STUDY_PATH.read_bytes()+b"\n")
            with self.assertRaisesRegex(ms.StudyError,"does not match the frozen v1 hash"): ms._study(altered)
            with mock.patch.object(ms,"STUDY_PATH",altered):
                with self.assertRaises(ms.StudyError): ms.choose_training([])

if __name__=="__main__": unittest.main()
