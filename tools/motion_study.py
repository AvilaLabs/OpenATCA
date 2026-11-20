#!/usr/bin/env python3
"""Offline scorer and fixed-rule selector for motion-study v1 (stdlib only)."""
from __future__ import annotations

import argparse
import csv
import hashlib
import json
import math
import os
import re
import sys
from collections import defaultdict
from pathlib import Path

# The sealed hold-out guard (tools/sealed_guard.py); the script's own directory may not be on the path.
sys.path.insert(0, str(Path(__file__).resolve().parent))
import sealed_guard  # noqa: E402

# Rows in a sealed time window are dropped and counted; the line is printed once at exit.
DROPS = sealed_guard.DropCounter()
STUDY_PATH = Path(__file__).resolve().parents[1] / "validation" / "motion-study-v1.json"
# Freeze: SHA-256 of validation/motion-study-v1.json; any other bytes are refused.
STUDY_V1_SHA256 = "bd33609f2ddf189a0ea8377522f20b8d2ac7706d8e45ba90285b0c1d27a93989"
STUDY_MISMATCH_REASON = "protocol file does not match the frozen v1 hash"
MAX_BYTES = 128 * 1024 * 1024
MAX_ROWS = 2_000_000
MAX_FIELD = 4096
TRUTH_FIELDS = ["scenario_id", "flight_id", "time_ns", "lat_deg", "lon_deg", "ve_mps", "vn_mps", "phase"]
MEAS_FIELDS = ["event_id", "scenario_id", "flight_id", "sensor_id", "sensor_ns", "receive_ns", "lat_deg", "lon_deg", "ve_mps", "vn_mps", "sigma_pos_m", "sigma_vel_mps"]
DIAG_FIELDS = ["scenario_id", "flight_id", "score_ns", "source_ns", "track_id", "status", "pos_cov_ee_m2", "pos_cov_en_m2", "pos_cov_nn_m2", "mu_cv", "mu_ct", "turn_rate_rps"]
NUMERIC_TRUTH = ("lat_deg", "lon_deg", "ve_mps", "vn_mps")
NUMERIC_DIAG = ("pos_cov_ee_m2", "pos_cov_en_m2", "pos_cov_nn_m2", "mu_cv", "mu_ct", "turn_rate_rps")
ALLOWED_SETTINGS = {"hqcv-1": 1.0, "hqcv-0_1": 0.1, "hqcv-0_01": 0.01}
CHI2_2D = {50: 1.38629436112, 90: 4.60517018599, 95: 5.99146454711}

class StudyError(ValueError):
    pass

def _open(path, mode="r", **kw):
    """Every file this tool reads goes through the sealed-data guard (a refused path is an error)."""
    try:
        return sealed_guard.open_read(path, mode, **kw)
    except sealed_guard.SealedError as exc:
        raise StudyError(str(exc)) from exc

def _refuse(*paths):
    try: sealed_guard.refuse(*paths)
    except sealed_guard.SealedError as exc: raise StudyError(str(exc)) from exc

def _read_text(path: Path):
    _refuse(path)
    with _open(path,"r",encoding="utf-8",errors="strict") as f: return f.read()

def _study(path=None):
    """Load the study definition only if its bytes hash to the pinned v1 digest (fail closed)."""
    p=Path(path or STUDY_PATH); _refuse(p)
    with _open(p,"rb") as f: raw=f.read()
    if hashlib.sha256(raw).hexdigest()!=STUDY_V1_SHA256: raise StudyError(STUDY_MISMATCH_REASON)
    return json.loads(raw.decode("utf-8"))

def _parse_finite_float(value):
    result=float(value)
    if not math.isfinite(result): raise ValueError(f"non-finite JSON number {value}")
    return result

def _unique_pairs(pairs):
    out = {}
    for key, val in pairs:
        if key in out:
            raise ValueError(f"duplicate JSON key {key!r}")
        out[key] = val
    return out

def read_json(path: Path):
    _refuse(path)
    if path.stat().st_size > MAX_BYTES:
        raise StudyError(f"{path}: exceeds size limit")
    try:
        with _open(path,"r",encoding="utf-8") as f:
            value = json.load(f, object_pairs_hook=_unique_pairs, parse_constant=lambda x: (_ for _ in ()).throw(ValueError(f"invalid constant {x}")), parse_float=_parse_finite_float)
    except (OSError, UnicodeError, ValueError, json.JSONDecodeError) as exc:
        raise StudyError(f"{path}: invalid JSON: {exc}") from exc
    if not isinstance(value, dict):
        raise StudyError(f"{path}: expected JSON object")
    return value

def read_score_list(path: Path):
    _refuse(path)
    if path.stat().st_size>MAX_BYTES: raise StudyError(f"{path}: exceeds size limit")
    try:
        with _open(path,"r",encoding="utf-8") as f:
            value=json.load(f,object_pairs_hook=_unique_pairs,parse_constant=lambda x: (_ for _ in ()).throw(ValueError(f"invalid constant {x}")),parse_float=_parse_finite_float)
    except (OSError,UnicodeError,ValueError,json.JSONDecodeError) as exc:
        raise StudyError(f"{path}: invalid score JSON: {exc}") from exc
    if not isinstance(value,list) or any(not isinstance(row,dict) for row in value): raise StudyError(f"{path}: expected JSON array of score objects")
    return value

def sha256(path: Path) -> str:
    _refuse(path)
    if path.stat().st_size > MAX_BYTES:
        raise StudyError(f"{path}: exceeds size limit")
    h = hashlib.sha256()
    with _open(path,"rb") as f:
        for block in iter(lambda: f.read(65536), b""):
            h.update(block)
    return h.hexdigest()

def _id_order(value):
    try:
        return (0, int(value))
    except ValueError:
        return (1, value)

def _csv(path: Path, fields):
    _refuse(path)
    times=tuple(f for f in fields if f in ("time_ns","sensor_ns","receive_ns","score_ns")); optional=tuple(f for f in fields if f in ("source_ns","publish_ns"))
    if path.stat().st_size > MAX_BYTES:
        raise StudyError(f"{path}: exceeds byte limit")
    try:
        with _open(path,"r",newline="", encoding="utf-8-sig") as f:
            reader = csv.DictReader(f, strict=True)
            if reader.fieldnames != fields:
                raise StudyError(f"{path}: exact CSV header required")
            rows = []
            for n, row in enumerate(reader, 2):
                if n > MAX_ROWS + 1:
                    raise StudyError(f"{path}: exceeds row limit")
                if None in row or any(v is None or len(v) > MAX_FIELD for v in row.values()):
                    raise StudyError(f"{path}:{n}: malformed or oversized row")
                if sealed_guard.row_in_window(row,times,optional,keep_malformed=True,loose=True):
                    DROPS.add(); continue
                rows.append(row)
            return rows
    except (OSError, UnicodeError, csv.Error) as exc:
        raise StudyError(f"{path}: {exc}") from exc

def number(row, field, where, integer=False):
    try:
        value = int(row[field]) if integer else float(row[field])
    except (ValueError, TypeError, KeyError) as exc:
        raise StudyError(f"{where}: invalid {field}") from exc
    if integer and abs(value) > 9_223_372_036_854_775_807:
        raise StudyError(f"{where}: out-of-range {field}")
    if not math.isfinite(value):
        raise StudyError(f"{where}: non-finite {field}")
    return value

def _key(row, time_field):
    return (row["scenario_id"], row["flight_id"], number(row, time_field, "grid", True))

def _truth_map(rows):
    result = {}
    previous = None
    for i, row in enumerate(rows, 2):
        key = _key(row, "time_ns")
        order = (key[0], key[2], _id_order(key[1]))
        if previous is not None and order <= previous:
            raise StudyError(f"truth:{i}: rows not unique in scenario/time/numeric-flight order")
        previous = order
        if key in result:
            raise StudyError(f"truth:{i}: duplicate truth grid key")
        vals = {f: number(row, f, f"truth:{i}") for f in NUMERIC_TRUTH}
        if abs(vals["lat_deg"]) > 90 or abs(vals["lon_deg"]) > 180:
            raise StudyError(f"truth:{i}: coordinate out of range")
        result[key] = {**vals, "phase": row["phase"]}
    return result

def _diag_map(rows):
    result = {}
    for i, row in enumerate(rows, 2):
        key = _key(row, "score_ns")
        if key in result:
            raise StudyError(f"diagnostics:{i}: duplicate grid key")
        if row["status"] not in ("valid", "coast", "missing"):
            raise StudyError(f"diagnostics:{i}: invalid status")
        if row["status"] == "missing":
            if any(row[f] != "" for f in ("source_ns", "track_id", *NUMERIC_DIAG)):
                raise StudyError(f"diagnostics:{i}: missing row must have blank payload")
        else:
            if not row["track_id"]:
                raise StudyError(f"diagnostics:{i}: present row requires track_id")
            source = number(row, "source_ns", f"diagnostics:{i}", True)
            publish = number(row, "score_ns", f"diagnostics:{i}", True)
            if source > publish:
                raise StudyError(f"diagnostics:{i}: source after score time")
            for field in NUMERIC_DIAG:
                number(row, field, f"diagnostics:{i}")
            if not (0.0 <= float(row["mu_cv"]) <= 1.0 and 0.0 <= float(row["mu_ct"]) <= 1.0):
                raise StudyError(f"diagnostics:{i}: motion probabilities out of range")
            try:
                _mahalanobis2(0.0,0.0,*[number(row,f"pos_cov_{axis}_m2",f"diagnostics:{i}") for axis in ("ee","en","nn")])
                row["_covariance_valid"]=True
            except StudyError:
                row["_covariance_valid"]=False
        result[key] = row
    return result

def ecef(lat, lon):
    a = 6378137.0
    f = 1.0 / 298.257223563
    e2 = f * (2.0 - f)
    p, l = math.radians(lat), math.radians(lon)
    n = a / math.sqrt(1.0 - e2 * math.sin(p) ** 2)
    return (n * math.cos(p) * math.cos(l), n * math.cos(p) * math.sin(l), n * (1-e2) * math.sin(p))

def local_error_enu(truth, estimate):
    x0, y0, z0 = ecef(truth["lat_deg"], truth["lon_deg"])
    x1, y1, z1 = ecef(estimate["lat_deg"], estimate["lon_deg"])
    d = (x1-x0, y1-y0, z1-z0)
    lat, lon = math.radians(estimate["lat_deg"]), math.radians(estimate["lon_deg"])
    east = -math.sin(lon)*d[0] + math.cos(lon)*d[1]
    north = -math.sin(lat)*math.cos(lon)*d[0] - math.sin(lat)*math.sin(lon)*d[1] + math.cos(lat)*d[2]
    return east, north

def _enu_error_at_frame(truth, estimate, frame_lat, frame_lon):
    x0,y0,z0=ecef(truth["lat_deg"],truth["lon_deg"]); x1,y1,z1=ecef(estimate["lat_deg"],estimate["lon_deg"])
    dx,dy,dz=x1-x0,y1-y0,z1-z0; lat,lon=math.radians(frame_lat),math.radians(frame_lon)
    east=-math.sin(lon)*dx+math.cos(lon)*dy
    north=-math.sin(lat)*math.cos(lon)*dx-math.sin(lat)*math.sin(lon)*dy+math.cos(lat)*dz
    return east,north

def _origin_local_delta(origin, estimate, manifest):
    lat0=math.radians(float(manifest.get("origin_lat_deg",origin["lat_deg"])))
    a=6378137.0; f=1/298.257223563; e2=f*(2-f)
    den=math.sqrt(1-e2*math.sin(lat0)**2)
    meridian=a*(1-e2)/den**3
    prime=a/den
    north=(estimate["lat_deg"]-origin["lat_deg"])*(math.pi/180)*meridian
    east=(estimate["lon_deg"]-origin["lon_deg"])*(math.pi/180)*prime*math.cos(lat0)
    return east,north

def _onset_exit_geographic_velocity(east_mps, north_mps, lat_deg, origin_lat_deg):
    """Convert local tangent velocity to the runner's WGS84 geographic basis."""
    a=6378137.0; f=1/298.257223563; e2=f*(2-f)
    p0=math.radians(origin_lat_deg); p=math.radians(lat_deg)
    q0=math.sqrt(1-e2*math.sin(p0)**2); q=math.sqrt(1-e2*math.sin(p)**2)
    meridian0=a*(1-e2)/q0**3; prime0=a/q0
    ve=east_mps*(a/q*math.cos(p))/(prime0*math.cos(p0))
    vn=north_mps*(a*(1-e2)/q**3)/meridian0
    return ve,vn

def _expected_turn_rate_deg_s(scenario, phase, flight_id):
    if phase!="turn": return 0.0
    if scenario=="turn": return -2.0
    if scenario=="onset-exit": return -2.0 if int(flight_id)%2==0 else 2.0
    return 0.0

def _candidate_local_mahalanobis(truth, estimate, covariance):
    east,north=local_error_enu(truth,estimate)
    return _mahalanobis2(east,north,*covariance)

def _stats(values):
    if not values:
        return {"count": 0, "rms": None, "p95": None, "p99": None, "mean": None}
    ordered = sorted(values)
    scale = max(abs(x) for x in values)
    rms = 0.0 if scale == 0 else scale * math.sqrt(math.fsum((x/scale)**2 for x in values)/len(values))
    mean = 0.0 if scale == 0 else scale * math.fsum(x/scale for x in values)/len(values)
    return {"count":len(values), "rms":rms, "p95":ordered[math.ceil(.95*len(ordered))-1], "p99":ordered[math.ceil(.99*len(ordered))-1], "mean":mean}

def _mahalanobis2(east, north, ee, en, nn):
    det = ee*nn-en*en
    if not all(math.isfinite(x) for x in (ee,en,nn,det)) or ee <= 0 or nn <= 0 or det <= 0:
        raise StudyError("candidate position covariance is not positive definite")
    value = (nn*east*east - 2*en*east*north + ee*north*north)/det
    if not math.isfinite(value) or value < -1e-10:
        raise StudyError("invalid posterior Mahalanobis distance")
    return max(0.0, value)

def _loss_gate(manifest):
    loss = manifest.get("loss_counters")
    if not isinstance(loss, dict) or not loss:
        return False, {"reason":"missing_loss_counters"}
    bad = {k:v for k,v in loss.items() if isinstance(v,bool) or not isinstance(v,(int,float)) or not math.isfinite(v) or v != 0}
    pending = {k:manifest.get(k) for k in ("event_queue_pending_before_drain", "event_queue_pending_at_end")}
    counters={k:manifest.get(k) for k in ("measurements_offered","measurements_applied","measurement_refused","duplicate_measurements","late_window","late_ring","time_future_dropped","time_steps_back","jump_rejected","numerical_refused","capacity_refused","work_refused","rows_unrepresentable","forecast_numerical_failures","forecast_unrepresentable_failures","tracker_output_overflow")}
    counters_ok=all(isinstance(v,int) and not isinstance(v,bool) and v>=0 for v in counters.values())
    conservation=counters.get("measurements_offered")==counters.get("measurements_applied")
    all_refusal_zero=all(counters.get(k)==0 for k in ("measurement_refused","duplicate_measurements","late_window","late_ring","time_future_dropped","time_steps_back","jump_rejected","numerical_refused","capacity_refused","work_refused","rows_unrepresentable","forecast_numerical_failures","forecast_unrepresentable_failures","tracker_output_overflow"))
    pending_bad=any(v != 0 for v in pending.values())
    return not bad and counters_ok and conservation and all_refusal_zero and not pending_bad, {"nonzero_loss_counters":bad,"pending_counts":pending,"work_counters":counters,"input_conservation":conservation,"all_refusal_counters_zero":all_refusal_zero}

def _validate_measurements(rows):
    seen=set(); previous=None
    for i,row in enumerate(rows,2):
        event=row["event_id"]
        if not event or event in seen: raise StudyError(f"measurements:{i}: empty or duplicate event_id")
        seen.add(event)
        order=(number(row,"receive_ns",f"measurements:{i}",True),_id_order(event))
        if previous is not None and order<previous: raise StudyError(f"measurements:{i}: receive/event order regressed")
        previous=order
        for field in ("sensor_ns","lat_deg","lon_deg","ve_mps","vn_mps","sigma_pos_m","sigma_vel_mps"):
            number(row,field,f"measurements:{i}",field.endswith("_ns"))
    return len(seen)

def _read_outputs(directory: Path, truth):
    _refuse(directory)
    path=directory/"candidate.csv"
    if not path.is_file(): raise StudyError(f"{directory}: candidate.csv missing")
    fields=["scenario_id","flight_id","score_ns","source_ns","publish_ns","lat_deg","lon_deg","ve_mps","vn_mps","status"]
    rows=_csv(path,fields); values={}
    for i,row in enumerate(rows,2):
        key=_key(row,"score_ns")
        if key in values: raise StudyError(f"candidate.csv:{i}: duplicate key")
        if key not in truth: raise StudyError(f"candidate.csv:{i}: unknown truth key")
        if row["status"] not in ("valid","coast","missing"): raise StudyError(f"candidate.csv:{i}: invalid status")
        publish=number(row,"publish_ns",f"candidate.csv:{i}",True)
        if publish!=key[2]: raise StudyError(f"candidate.csv:{i}: publish differs from score")
        diag_status= None
        if row["status"]=="missing":
            if any(row[f]!="" for f in ("source_ns","lat_deg","lon_deg","ve_mps","vn_mps")):
                raise StudyError(f"candidate.csv:{i}: missing row must have blank payload")
        else:
            source=number(row,"source_ns",f"candidate.csv:{i}",True)
            if source>publish: raise StudyError(f"candidate.csv:{i}: source follows publish")
            vals={f:number(row,f,f"candidate.csv:{i}") for f in ("lat_deg","lon_deg","ve_mps","vn_mps")}
            if abs(vals["lat_deg"])>90 or abs(vals["lon_deg"])>180: raise StudyError(f"candidate.csv:{i}: coordinate out of range")
            values[key]={**vals,"status":row["status"],"source_ns":source}
        values.setdefault(key,{"status":row["status"],"source_ns":None})
    if set(values)!=set(truth): raise StudyError(f"{directory}: candidate output must contain every truth grid row")
    return values

def _native_guard(build, build_path):
    """Return a derived guard status from hash-bound command result and retained log."""
    result_spec=build.get("native_guard_result"); log_spec=build.get("native_guard_log")
    if not isinstance(result_spec,dict) or not isinstance(log_spec,dict): return False,None
    try:
        def resolve(spec):
            p=Path(spec.get("path",""))
            if not p.is_absolute(): p=build_path.parent/p
            if sha256(p)!=spec.get("sha256"): return None
            return p
        result_path=resolve(result_spec); log_path=resolve(log_spec)
        if not result_path or not log_path: return False,None
        if build.get("native_guard_log_sha256")!=log_spec.get("sha256"): return False,None
        result=read_json(result_path)
        command=result.get("command")
        if not isinstance(command,list) or not all(isinstance(v,str) for v in command): return False,None
        required=("cargo", "+1.89.0", "test", "-p", "oa-track", "--test", "core", "--locked", "--offline", "accelerating_flight_is_tracked", "--", "--exact", "--nocapture", "--test-threads=1")
        cursor=0
        for arg in command:
            if cursor<len(required) and arg==required[cursor]: cursor+=1
        if cursor!=len(required): return False,None
        log=_read_text(log_path)
        log_pass=bool(re.search(r"test result: ok\. 1 passed; 0 failed; 0 ignored;",log))
        valid=(result.get("exit_code")==0 and log_pass and result.get("head")==build.get("source_head") and result.get("source_diff_sha256")==build.get("source_diff_sha256"))
        if "native_regression_pass" in build and build["native_regression_pass"] is not valid: valid=False
        return bool(valid),{"result_sha256":result_spec["sha256"],"log_sha256":log_spec["sha256"],"exit_code":result.get("exit_code"),"one_test_pass_summary":log_pass,"command":command,"source_head_matches":result.get("head")==build.get("source_head"),"source_diff_matches":result.get("source_diff_sha256")==build.get("source_diff_sha256")}
    except (OSError,StudyError,ValueError,TypeError,KeyError): return False,None

def _source_lineage(build, build_path):
    """Validate that a build changes only the registered parameter constant."""
    diff_spec=build.get("source_diff_path")
    if not isinstance(diff_spec,str) or not diff_spec: raise StudyError(f"{build_path}: source_diff_path is required")
    diff_path=Path(diff_spec); diff_path=diff_path if diff_path.is_absolute() else build_path.parent/diff_path
    if sha256(diff_path)!=build.get("source_diff_sha256"): raise StudyError(f"{build_path}: source diff hash mismatch")
    diff=_read_text(diff_path)
    changed=re.findall(r"^diff --git a/(.+?) b/(.+?)$",diff,re.MULTILINE)
    allowed_path="crates/oa-track/src/params.rs"
    if any(a!=allowed_path or b!=allowed_path for a,b in changed): raise StudyError(f"{build_path}: source diff changes files outside params.rs")
    status=build.get("source_status")
    if status not in (""," M crates/oa-track/src/params.rs\n") or (status=="" and changed) or (status!="" and changed!=[(allowed_path,allowed_path)]):
        raise StudyError(f"{build_path}: source status contradicts hash-bound diff")
    modifications=[line for line in diff.splitlines() if line.startswith(("+","-")) and not line.startswith(("+++","---"))]
    params=build.get("expected_motion_parameters")
    expected_modifications=[] if status=="" else ["-pub const H_Q_CV: f64 = 1.0;",f"+pub const H_Q_CV: f64 = {params.get('H_Q_CV')};"]
    if modifications!=expected_modifications: raise StudyError(f"{build_path}: source diff must change only the H_Q_CV declaration")
    params_path=Path(build["params_source_path"]); params_path=params_path if params_path.is_absolute() else build_path.parent/params_path
    text=_read_text(params_path)
    pattern=r"(?m)^(\s*pub const H_Q_CV: f64 = )[0-9]+(?:\.[0-9]+)?(;\s*)$"
    normalized,count=re.subn(pattern,r"\g<1><STUDY_H_Q_CV>\2",text)
    if count!=1: raise StudyError(f"{build_path}: params source must declare H_Q_CV exactly once")
    return hashlib.sha256(normalized.encode("utf-8")).hexdigest()

def _expected_model(scenario):
    if scenario in ("straight","turn","acceleration","crossing","dropout","delayed"):
        return "adsb_kinematic_v2"
    if scenario=="onset-exit": return "adsb_motion_study_v1"
    return None

def _load_run(directory: Path, build_path: Path):
    _refuse(directory,build_path)
    truth_path=directory/"truth.csv"; meas_path=directory/"measurements.csv"; diag_path=directory/"diagnostics.csv"; manifest_path=directory/"manifest.json"
    for path in (truth_path,meas_path,diag_path,manifest_path,build_path):
        if not path.is_file(): raise StudyError(f"required artifact missing: {path}")
    truth_rows=_csv(truth_path,TRUTH_FIELDS); measurement_rows=_csv(meas_path,MEAS_FIELDS); diagnostic_rows=_csv(diag_path,DIAG_FIELDS)
    manifest=read_json(manifest_path); build=read_json(build_path)
    truth=_truth_map(truth_rows); diag=_diag_map(diagnostic_rows)
    _validate_measurements(measurement_rows)
    if set(truth)!=set(diag): raise StudyError(f"{directory}: diagnostics must have one row per truth key")
    scenarios={key[0] for key in truth}
    if len(scenarios)!=1 or manifest.get("scenario_id")!=next(iter(scenarios)) or manifest.get("truth_rows")!=len(truth): raise StudyError(f"{directory}: manifest contradicts truth grid")
    if manifest.get("aircraft_count")!=20 or manifest.get("scenario_duration_s")!=300 or manifest.get("grid_count")!=301 or manifest.get("grid_step_ns")!=1_000_000_000:
        raise StudyError(f"{directory}: run is outside the preregistered 20-aircraft/300-second grid")
    expected_rows=20*301
    if len(truth)!=expected_rows: raise StudyError(f"{directory}: expected exactly 6,020 truth rows")
    if manifest.get("measurement_rows")!=len(measurement_rows): raise StudyError(f"{directory}: manifest measurement count mismatch")
    if manifest.get("diagnostic_rows")!=len(diagnostic_rows) or manifest.get("diagnostic_missing_rows")!=sum(r["status"]=="missing" for r in diagnostic_rows):
        raise StudyError(f"{directory}: diagnostic manifest counts mismatch")
    required={"source_head","source_status","source_diff_sha256","source_diff_path","params_source_sha256","runner_source_sha256","binary_sha256","binary_path","params_source_path","runner_source_path","expected_motion_parameters","native_guard_result","native_guard_log","native_guard_log_sha256","native_regression_pass"}
    if not required<=build.keys() or not isinstance(build.get("source_status"),str) or not isinstance(build.get("native_regression_pass"),bool): raise StudyError(f"{build_path}: build evidence schema/status invalid")
    for name in ("source_diff_sha256","params_source_sha256","runner_source_sha256","binary_sha256","native_guard_log_sha256"):
        value=build.get(name)
        if not isinstance(value,str) or len(value)!=64 or any(ch not in "0123456789abcdef" for ch in value): raise StudyError(f"{build_path}: invalid {name}")
    params=build.get("expected_motion_parameters")
    for path_key,hash_key in (("binary_path","binary_sha256"),("params_source_path","params_source_sha256"),("runner_source_path","runner_source_sha256")):
        artifact_path=Path(build[path_key]); artifact_path=artifact_path if artifact_path.is_absolute() else build_path.parent/artifact_path
        if sha256(artifact_path)!=build[hash_key]: raise StudyError(f"{build_path}: {path_key} does not match its declared hash")
    if not isinstance(params,dict) or set(params)!={"H_Q_CV","H_Q_CT","H_Q_OMEGA","H_TAU_CV_S","H_TAU_CT_S","CT_OMEGA_STD_FLOOR"}:
        raise StudyError(f"{build_path}: exact motion parameter map required")
    if any(isinstance(v,bool) or not isinstance(v,(int,float)) or not math.isfinite(v) for v in params.values()):
        raise StudyError(f"{build_path}: motion parameters must be finite numbers")
    expected_model=_expected_model(manifest.get("scenario_id"))
    if expected_model is None or manifest.get("model_id")!=expected_model or manifest.get("motion_parameters")!=params or manifest.get("motion_params_source_sha256")!=build.get("params_source_sha256") or manifest.get("source_implementation_sha256")!=build.get("runner_source_sha256"):
        raise StudyError(f"{directory}: runner manifest differs from independent build evidence")
    normalized_params_sha256=_source_lineage(build,build_path)
    native_pass,native_evidence=_native_guard(build,build_path)
    outputs=_read_outputs(directory,truth)
    for key,row in diag.items():
        if outputs[key]["status"]!=row["status"] or (row["status"]!="missing" and outputs[key]["source_ns"]!=int(row["source_ns"])):
            raise StudyError(f"{directory}: candidate output/diagnostic status or source mismatch")
    return {"directory":directory,"truth_path":truth_path,"measurements_path":meas_path,"diagnostics_path":diag_path,"manifest_path":manifest_path,"truth":truth,"diag":diag,"manifest":manifest,"build":build,"native_regression_pass":native_pass,"native_evidence":native_evidence,"normalized_params_sha256":normalized_params_sha256,"params":params,"outputs":outputs,"hashes":{"truth":sha256(truth_path),"measurements":sha256(meas_path),"diagnostics":sha256(diag_path),"manifest":sha256(manifest_path),"build":sha256(build_path)}}

def _input_pair(candidate, default):
    for kind in ("truth","measurements"):
        if candidate["hashes"][kind]!=default["hashes"][kind]: raise StudyError("candidate and frozen default inputs must be byte-identical")
    for key in ("scenario_id","seed","aircraft_count","scenario_duration_s","model_id","grid_start_ns"):
        if candidate["manifest"].get(key)!=default["manifest"].get(key): raise StudyError(f"candidate/default manifest mismatch: {key}")
    cb,db=candidate["build"],default["build"]
    if cb.get("source_head")!=db.get("source_head") or cb.get("runner_source_sha256")!=db.get("runner_source_sha256"):
        raise StudyError("candidate and frozen default must share source head and runner implementation")
    if candidate["normalized_params_sha256"]!=default["normalized_params_sha256"]:
        raise StudyError("candidate and frozen default parameter source differs beyond H_Q_CV")
    cp,dp=candidate["params"],default["params"]
    allowed={"hqcv-1":1.0,"hqcv-0_1":0.1,"hqcv-0_01":0.01}
    if cp["H_Q_CV"] not in allowed.values(): raise StudyError("candidate H_Q_CV is not preregistered")
    if dp["H_Q_CV"]!=1.0: raise StudyError("frozen-default control must use H_Q_CV=1")
    if {k:v for k,v in cp.items() if k!="H_Q_CV"}!={k:v for k,v in dp.items() if k!="H_Q_CV"}:
        raise StudyError("other motion parameters changed between candidate and frozen default")
    setting=next(k for k,v in allowed.items() if v==cp["H_Q_CV"])
    return setting

def _cluster_counts(groups):
    return {group:{cluster:{"n":len(values),"hits50":sum(v[0] for v in values),"hits90":sum(v[1] for v in values),"hits95":sum(v[2] for v in values)} for cluster,values in sorted(clusters.items())} for group,clusters in groups.items()}

def _score_rows(run, default_run, epoch_ns, initialization_ns):
    truth=run["truth"]; cdiag=run["diag"]; ddiag=default_run["diag"]; co=run["outputs"]; do=default_run["outputs"]
    post_start=epoch_ns+initialization_ns; status=defaultdict(int)
    usable={"candidate":set(),"default":set()}; metrics={name:{"position":[],"velocity":[],"turn_position":[],"turn_rate":[]} for name in ("candidate","default")}
    full_metrics={name:{"position":[],"velocity":[]} for name in ("candidate","default")}; cold_metrics={name:{"position":[],"velocity":[]} for name in ("candidate","default")}
    scenario=defaultdict(lambda:defaultdict(lambda:{"position":[],"velocity":[],"turn_position":[],"turn_rate":[]}))
    scenario_paired=defaultdict(lambda:{name:{"position":[],"velocity":[],"turn_position":[],"turn_rate":[]} for name in ("candidate","default")})
    phase_paired=defaultdict(lambda:{name:{"position":[],"velocity":[]} for name in ("candidate","default")})
    response=defaultdict(lambda:{name:{"position":[],"velocity":[]} for name in ("candidate","default")})
    paired={name:{"position":[],"velocity":[],"turn_position":[],"turn_rate":[]} for name in ("candidate","default")}
    present_paired={name:{"position":[]} for name in ("candidate","default")}
    scenario_present_paired=defaultdict(lambda:{name:{"position":[]} for name in ("candidate","default")})
    cal_clusters=defaultdict(lambda:defaultdict(list)); window_clusters=defaultdict(lambda:defaultdict(list)); invalid_candidate_covariances=0
    cold_counts=defaultdict(int); post_counts=defaultdict(int); post_den=0
    for key,t in truth.items():
        cr,dr=cdiag[key],ddiag[key]; ce,de=co[key],do[key]
        status["candidate_"+cr["status"]]+=1; status["default_"+dr["status"]]+=1
        for name,row,out in (("candidate",cr,ce),("default",dr,de)):
            if out["status"] not in ("valid","coast"): continue
            pos=math.hypot(*local_error_enu(t,out)); vel=math.hypot(out["ve_mps"]-t["ve_mps"],out["vn_mps"]-t["vn_mps"])
            full_metrics[name]["position"].append(pos); full_metrics[name]["velocity"].append(vel)
            if key[2]<post_start: cold_metrics[name]["position"].append(pos); cold_metrics[name]["velocity"].append(vel)
        if key[2]<post_start:
            cold_counts["candidate_"+cr["status"]]+=1; cold_counts["default_"+dr["status"]]+=1; continue
        post_den+=1; post_counts["candidate_"+cr["status"]]+=1; post_counts["default_"+dr["status"]]+=1
        if cr["status"] in ("valid","coast") and not cr.get("_covariance_valid",False): invalid_candidate_covariances+=1
        if cr["status"]=="valid": usable["candidate"].add(key)
        if dr["status"]=="valid": usable["default"].add(key)
        for name,row,out in (("candidate",cr,ce),("default",dr,de)):
            if out["status"] not in ("valid","coast"): continue
            tpos=local_error_enu(t,out); pos=math.hypot(*tpos); vel=math.hypot(out["ve_mps"]-t["ve_mps"],out["vn_mps"]-t["vn_mps"])
            rate=math.degrees(number(row,"turn_rate_rps",f"diagnostics:{key}"))
            expected_rate=_expected_turn_rate_deg_s(key[0],t["phase"],key[1])
            metrics[name]["position"].append(pos); metrics[name]["velocity"].append(vel); metrics[name]["turn_rate"].append(rate-expected_rate)
            scenario[name][key[0]]["position"].append(pos); scenario[name][key[0]]["velocity"].append(vel); scenario[name][key[0]]["turn_rate"].append(rate-expected_rate)
            if t["phase"]=="turn": metrics[name]["turn_position"].append(pos); scenario[name][key[0]]["turn_position"].append(pos)
        if ce["status"] in ("valid","coast") and de["status"] in ("valid","coast"):
            for name,out in (("candidate",ce),("default",de)):
                pos=math.hypot(*local_error_enu(t,out))
                present_paired[name]["position"].append(pos)
                scenario_present_paired[key[0]][name]["position"].append(pos)
        if ce["status"]=="valid" and de["status"]=="valid":
            estimates={"candidate":ce,"default":de}
            for name,estimate in estimates.items():
                east,north=local_error_enu(t,estimate); pos=math.hypot(east,north); vel=math.hypot(estimate["ve_mps"]-t["ve_mps"],estimate["vn_mps"]-t["vn_mps"])
                rate=math.degrees(number((cr if name=="candidate" else dr),"turn_rate_rps",f"diagnostics:{key}"))
                expected_rate=_expected_turn_rate_deg_s(key[0],t["phase"],key[1])
                paired[name]["position"].append(pos); paired[name]["velocity"].append(vel); paired[name]["turn_rate"].append(rate-expected_rate)
                if t["phase"]=="turn": paired[name]["turn_position"].append(pos); scenario_paired[key[0]][name]["turn_position"].append(pos)
                scenario_paired[key[0]][name]["position"].append(pos); scenario_paired[key[0]][name]["velocity"].append(vel); scenario_paired[key[0]][name]["turn_rate"].append(rate-expected_rate)
                phase_paired[t["phase"]][name]["position"].append(pos); phase_paired[t["phase"]][name]["velocity"].append(vel)
                if key[0]=="onset-exit":
                    elapsed=(key[2]-epoch_ns)/1e9
                    for label,start_s,end_s in (("onset",60.0,90.0),("exit",120.0,150.0)):
                        if start_s<=elapsed<end_s: response[label][name]["position"].append(pos); response[label][name]["velocity"].append(vel)
            if cr.get("_covariance_valid",False):
                covariance=[number(cr,f"pos_cov_{a}_m2",f"diagnostics:{key}") for a in ("ee","en","nn")]
                q2=_candidate_local_mahalanobis(t,ce,covariance)
                hits=[int(q2<=CHI2_2D[p]) for p in (50,90,95)]
                cluster=f"{run['manifest'].get('seed')}:{key[1]}"
                cal_group=key[0]; cal_clusters[cal_group][cluster].append(hits)
                if key[0]=="onset-exit":
                    flight=int(key[1]); direction="even" if flight%2==0 else "odd"; elapsed=(key[2]-epoch_ns)/1e9
                    for label,start,end in (("onset",60,90),("exit",120,150)):
                        if start<=elapsed<end: window_clusters[f"{direction}:{label}"][cluster].append(hits)
    pairsets=usable["candidate"]==usable["default"]
    return {"post_start_ns":post_start,"postinit_truth_rows":post_den,"status_counts":dict(status),"postinit_status_counts":dict(post_counts),"cold_start_status_counts":dict(cold_counts),"usable_keys":usable,"same_usable_keys":pairsets,"metrics":{name:{m:_stats(v) for m,v in values.items()} for name,values in metrics.items()},"full_grid_metrics":{name:{m:_stats(v) for m,v in values.items()} for name,values in full_metrics.items()},"cold_start_metrics":{name:{m:_stats(v) for m,v in values.items()} for name,values in cold_metrics.items()},"scenario_metrics":{name:{sc:{m:_stats(v) for m,v in vals.items()} for sc,vals in bysc.items()} for name,bysc in scenario.items()},"paired_metrics":{name:{m:_stats(v) for m,v in vals.items()} for name,vals in paired.items()},"present_paired_metrics":{name:{m:_stats(v) for m,v in vals.items()} for name,vals in present_paired.items()},"scenario_present_paired_metrics":{sc:{name:{m:_stats(v) for m,v in vals.items()} for name,vals in systems.items()} for sc,systems in scenario_present_paired.items()},"scenario_paired_metrics":{sc:{name:{m:_stats(v) for m,v in vals.items()} for name,vals in systems.items()} for sc,systems in scenario_paired.items()},"phase_paired_metrics":{phase:{name:{m:_stats(v) for m,v in vals.items()} for name,vals in systems.items()} for phase,systems in phase_paired.items()},"response_metrics":{window:{name:{m:_stats(v) for m,v in vals.items()} for name,vals in systems.items()} for window,systems in response.items()},"containment_clusters":_cluster_counts(cal_clusters),"window_containment_clusters":_cluster_counts(window_clusters),"invalid_candidate_covariances":invalid_candidate_covariances}

def _minimum_case_containment(groups, floors, per_case=True):
    # Per-case files preserve calibration observations; minimum cluster/row floors are cohort gates.
    return bool(groups) and all(isinstance(row,dict) and row.get("n",0)>0 for clusters in groups.values() for row in clusters.values())

def _cross_lag(times_values, start_s, end_s, threshold, above):
    selected=[]
    for t_s,value in times_values:
        if t_s<start_s or t_s>=end_s: continue
        condition=value>=threshold if above else value<threshold
        if selected and t_s!=selected[-1][0]+1.0:
            selected=[]
        selected.append((t_s,condition))
        if len(selected)>=3 and all(x[1] for x in selected[-3:]):
            return {"lag_s":selected[-3][0]-start_s,"censored":False}
    return {"lag_s":None,"censored":True}

def _transition_metrics(candidate, default, epoch_ns):
    flights=sorted({key[1] for key in candidate["truth"]},key=_id_order)
    reports={}; gates={}; window_results={}
    for flight in flights:
        try: parity=int(flight)%2
        except ValueError as exc: raise StudyError("onset-exit flight IDs must be numeric for mirrored parity") from exc
        direction="odd" if parity else "even"
        for transition,event_s,lo,hi,enter in (("onset",60.0,60.0,90.0,True),("exit",120.0,120.0,150.0,False)):
            one={}
            for name,run in (("candidate",candidate),("default",default)):
                mu=[]; rate=[]
                for key,row in run["diag"].items():
                    if key[1]!=flight or row["status"] not in ("valid","coast"): continue
                    elapsed=(key[2]-epoch_ns)/1e9
                    mu_value=number(row,"mu_ct",f"diagnostics:{key}")
                    rate_deg=abs(math.degrees(number(row,"turn_rate_rps",f"diagnostics:{key}")))
                    mu.append((elapsed,mu_value)); rate.append((elapsed,rate_deg))
                mu_lag=_cross_lag(mu,lo,hi,.5,enter)
                rate_lag=_cross_lag(rate,lo,hi,.5,enter)
                one[name]={"mu_ct":mu_lag,"turn_rate_deg_s":rate_lag}
            okay=True
            for field in ("mu_ct","turn_rate_deg_s"):
                c=one["candidate"][field]; d=one["default"][field]
                if c["censored"] or d["censored"] or c["lag_s"]>d["lag_s"]+1.0: okay=False
            key=f"{direction}:{transition}"
            reports[f"{flight}:{transition}"]={"flight_id":flight,"direction":direction,"transition":transition,"onset_epoch_s":event_s,"candidate":one["candidate"],"default":one["default"],"pass":okay}
            gates[f"lag_{key}"]=all(reports[x]["pass"] for x in reports if reports[x]["direction"]==direction and reports[x]["transition"]==transition)
    return {"gates":gates,"by_flight":reports}

def _validate_onset_truth(run):
    manifest=run["manifest"]
    if manifest.get("diagnostic_only") is not True or manifest.get("reference_validation_allowed") is not False or manifest.get("diagnostic_turn_sign_rule")!="even_positive_odd_negative":
        raise StudyError("onset-exit diagnostic role/sign rule does not match preregistration")
    epoch=manifest["grid_start_ns"]; flights=sorted({k[1] for k in run["truth"]},key=_id_order)
    checked={}
    for flight in flights:
        try: sign=1 if int(flight)%2==0 else -1
        except ValueError as exc: raise StudyError("onset-exit flight IDs must be numeric") from exc
        origin_key=next((k for k in run["truth"] if k[1]==flight and k[2]==epoch),None)
        if origin_key is None: raise StudyError(f"onset-exit lacks grid origin for flight {flight}")
        origin=run["truth"][origin_key]
        for t_s in (0,60,105,120):
            key=(origin_key[0],flight,epoch+int(t_s*1e9))
            if key not in run["truth"]: raise StudyError(f"onset-exit lacks fixed analytic time {t_s}s for flight {flight}")
            truth=run["truth"][key]; expected=onset_exit_truth(t_s,sign)
            east,north=_origin_local_delta(origin,truth,manifest)
            expected_ve,expected_vn=_onset_exit_geographic_velocity(expected[2],expected[3],truth["lat_deg"],manifest["origin_lat_deg"])
            if abs(east-expected[0])>1.0 or abs(north-expected[1])>1.0 or abs(truth["ve_mps"]-expected_ve)>.05 or abs(truth["vn_mps"]-expected_vn)>.05:
                raise StudyError(f"onset-exit analytic truth mismatch for flight {flight} at {t_s}s")
            checked[f"{flight}:{t_s}"]={"east_m":east,"north_m":north,"ve_mps":truth["ve_mps"],"vn_mps":truth["vn_mps"]}
    return checked

def score_pair(candidate_dir, candidate_build, default_dir, default_build):
    c=_load_run(Path(candidate_dir),Path(candidate_build)); d=_load_run(Path(default_dir),Path(default_build)); setting=_input_pair(c,d)
    scenario=c["manifest"]["scenario_id"]; seed=c["manifest"].get("seed")
    if isinstance(seed,bool) or not isinstance(seed,int) or seed not in (1,2,3,4,5,6): raise StudyError("only preregistered development seeds 1-6 are permitted")
    known={"straight","turn","acceleration","crossing","dropout","delayed","onset-exit"}
    if scenario not in known: raise StudyError("scenario is outside the preregistered study")
    epoch=c["manifest"].get("grid_start_ns"); init=c["manifest"].get("initialization_ns")
    if isinstance(epoch,bool) or not isinstance(epoch,int) or init!=2_000_000_000: raise StudyError("invalid epoch or post-initialization window")
    if scenario=="onset-exit": analytic_truth=_validate_onset_truth(c); _validate_onset_truth(d)
    else: analytic_truth=None
    result=_score_rows(c,d,epoch,init); c_loss,c_loss_data=_loss_gate(c["manifest"]); d_loss,d_loss_data=_loss_gate(d["manifest"])
    post_n=result["postinit_truth_rows"]; c_n=len(result["usable_keys"]["candidate"]); d_n=len(result["usable_keys"]["default"])
    cov_c=c_n/post_n if post_n else None; cov_d=d_n/post_n if post_n else None; same=result["same_usable_keys"]
    paired=result["paired_metrics"]
    metric_fields={"position_p99":("position","p99"),"position_rms":("position","rms"),"velocity_rms":("velocity","rms"),"turn_phase_position_rms":("turn_position","rms"),"turn_rate_rms":("turn_rate","rms")}
    ratios={name:_ratio(paired["candidate"][field][stat],paired["default"][field][stat]) for name,(field,stat) in metric_fields.items()}
    scenario_ratios={}
    for sc,systems in result["scenario_paired_metrics"].items():
        scenario_ratios[sc]={name:_ratio(systems["candidate"][field][stat],systems["default"][field][stat]) for name,(field,stat) in metric_fields.items()}
    study=_study(); floors=study["uncertainty"]["coverage_floors_by_nominal_scenario"]
    nominal=scenario in study["main_profile_gates"]["nominal_scenarios"]
    coverage_gate=(cov_c==1.0 and cov_c is not None and cov_d is not None and cov_c>=cov_d and same)
    losses_ok=c_loss and d_loss
    sr=scenario_ratios.get(scenario,{})
    scenario_gate=all(_within_ratio(result["scenario_paired_metrics"].get(scenario,{}).get("candidate",{}).get(field,{}).get(stat),result["scenario_paired_metrics"].get(scenario,{}).get("default",{}).get(field,{}).get(stat)) for field,stat in (("position","p99"),("position","rms"),("velocity","rms"),("turn_rate","rms")))
    if scenario=="turn": scenario_gate &= _within_ratio(result["scenario_paired_metrics"].get("turn",{}).get("candidate",{}).get("turn_position",{}).get("rms"),result["scenario_paired_metrics"].get("turn",{}).get("default",{}).get("turn_position",{}).get("rms"))
    native_pass=c["native_regression_pass"] and d["native_regression_pass"]
    containment_case=bool(result["containment_clusters"].get(scenario))
    gates={"zero_lost_pending_refused":bool(losses_ok),"native_acceleration_regression_pass":native_pass,"candidate_covariances_positive_definite":result["invalid_candidate_covariances"]==0,"full_candidate_postinit_coverage":bool(coverage_gate),"same_valid_keyset":bool(same),"per_scenario_regression_bounds":bool(scenario_gate)}
    role="nominal" if nominal else "diagnostic_only"
    transition_metrics=None
    if scenario=="dropout":
        gates["dropout_position_p99_no_regression"]=_within_ratio(result["scenario_present_paired_metrics"].get(scenario,{}).get("candidate",{}).get("position",{}).get("p99"),result["scenario_present_paired_metrics"].get(scenario,{}).get("default",{}).get("position",{}).get("p99"))
        gates["dropout_same_coverage"]=cov_c==cov_d
        passed=all(gates[k] for k in ("zero_lost_pending_refused","native_acceleration_regression_pass","same_valid_keyset","dropout_position_p99_no_regression","dropout_same_coverage"))
    elif scenario=="onset-exit":
        transition=_transition_metrics(c,d,epoch); transition_metrics=transition["by_flight"]; gates.update(transition["gates"])
        gates["onset_exit_analytic_truth"]=analytic_truth is not None
        gates["onset_exit_response_position_bounds"]=all(_within_ratio(result["response_metrics"].get(w,{}).get("candidate",{}).get("position",{}).get(stat),result["response_metrics"].get(w,{}).get("default",{}).get("position",{}).get(stat)) for w in ("onset","exit") for stat in ("rms","p99"))
        gates["onset_exit_coverage_match"]=cov_c==cov_d and same
        gates["onset_exit_covariance_observations"]=all(bool(result["window_containment_clusters"].get(k)) for k in ("even:onset","odd:onset","even:exit","odd:exit"))
        passed=all(gates.values())
    else:
        if nominal: gates["per_case_containment_observed"]=containment_case
        passed=all(gates.values()) if nominal else False
    return {"schema":"openatca-motion-study-score-v1","study_id":"openatca-motion-study-v1","scenario":scenario,"seed":seed,"role":role,"setting_id":setting,"H_Q_CV":c["params"]["H_Q_CV"],"default_H_Q_CV":d["params"]["H_Q_CV"],"parameters_except_H_Q_CV":{k:v for k,v in c["params"].items() if k!="H_Q_CV"},"hashes":{"candidate_truth":c["hashes"]["truth"],"default_truth":d["hashes"]["truth"],"candidate_measurements":c["hashes"]["measurements"],"default_measurements":d["hashes"]["measurements"],"candidate_diagnostics":c["hashes"]["diagnostics"],"default_diagnostics":d["hashes"]["diagnostics"],"candidate_build":c["hashes"]["build"],"default_build":d["hashes"]["build"],"candidate_source":c["build"]["runner_source_sha256"],"default_source":d["build"]["runner_source_sha256"],"candidate_source_head":c["build"]["source_head"],"default_source_head":d["build"]["source_head"],"candidate_params_source":c["build"]["params_source_sha256"],"default_params_source":d["build"]["params_source_sha256"],"candidate_normalized_params":c["normalized_params_sha256"],"default_normalized_params":d["normalized_params_sha256"],"candidate_native_guard_log":c["build"]["native_guard_log_sha256"],"candidate_native_guard":c["native_evidence"],"default_native_guard":d["native_evidence"]},"candidate_native_guard_pass":c["native_regression_pass"],"default_native_guard_pass":d["native_regression_pass"],"input_hashes_match":True,"post_initialization_metrics":result["metrics"],"full_grid_metrics":result["full_grid_metrics"],"cold_start_metrics":result["cold_start_metrics"],"scenario_metrics":result["scenario_metrics"],"phase_paired_metrics":result["phase_paired_metrics"],"paired_metrics":paired,"present_paired_metrics":result["present_paired_metrics"],"response_metrics":result["response_metrics"],"per_scenario_ratios":scenario_ratios,"ratios_to_frozen_default":ratios,"coverage":{"postinit_truth_rows":post_n,"candidate_valid_rows":c_n,"default_valid_rows":d_n,"candidate_fraction":cov_c,"default_fraction":cov_d,"same_usable_keys":same,"candidate_only_keys":len(result["usable_keys"]["candidate"]-result["usable_keys"]["default"]),"default_only_keys":len(result["usable_keys"]["default"]-result["usable_keys"]["candidate"])},"status_counts":result["status_counts"],"status_fractions":{f"{system}_{status}":result["status_counts"].get(f"{system}_{status}",0)/len(c["truth"]) for system in ("candidate","default") for status in ("valid","coast","missing")},"postinit_status_counts":result["postinit_status_counts"],"postinit_status_fractions":{f"{system}_{status}":result["postinit_status_counts"].get(f"{system}_{status}",0)/post_n for system in ("candidate","default") for status in ("valid","coast","missing")} if post_n else {},"cold_start_status_counts":result["cold_start_status_counts"],"cold_start_status_fractions":{f"{system}_{status}":result["cold_start_status_counts"].get(f"{system}_{status}",0)/(len(c["truth"])-post_n) for system in ("candidate","default") for status in ("valid","coast","missing")} if len(c["truth"])>post_n else {},"loss_evidence":{"candidate":c_loss_data,"default":d_loss_data},"containment":{"clusters":result["containment_clusters"],"onset_exit_window_clusters":result["window_containment_clusters"],"floors":floors,"invalid_candidate_covariances":result["invalid_candidate_covariances"]},"analytic_truth_points":analytic_truth,"transition_metrics":transition_metrics,"gates":gates,"selection_pass":bool(passed)}


def _ratio(a,b):
    return a/b if a is not None and b is not None and b>1e-9 else None

def _within_ratio(candidate, reference, maximum=1.02):
    if candidate is None or reference is None: return False
    if reference<=1e-9: return candidate<=1e-9
    return candidate/reference<=maximum

def _cohort_bootstrap(clusters_by_seed, success_key, bootstrap_seed=173):
    import random
    unique=sum(len(v) for v in clusters_by_seed.values())
    rows=sum(c["n"] for block in clusters_by_seed.values() for c in block.values())
    seed_ids=tuple(sorted(clusters_by_seed))
    if unique<20 or rows<200 or len(seed_ids)!=3:
        return {"pass_data_minimum":False,"unique_clusters":unique,"scored_rows":rows,"lower95":None,"upper95":None,"empirical_fraction":None}
    study=_study(); bootstrap=study["uncertainty"]["bootstrap"]
    if bootstrap_seed!=bootstrap["seed"]: raise StudyError("bootstrap seed differs from preregistration")
    replicates=bootstrap["replicates"]; rng=random.Random(bootstrap_seed); reps=[]
    for _ in range(replicates):
        n=hit=0
        for _seed_draw in range(3):
            selected_seed=rng.choice(seed_ids); block=clusters_by_seed[selected_seed]; flight_ids=list(block)
            for _flight_draw in range(len(flight_ids)):
                cluster=block[rng.choice(flight_ids)]; n+=cluster["n"]; hit+=cluster[success_key]
        reps.append(hit/n if n else 0.0)
    reps.sort()
    lo_index=math.ceil(.05*len(reps))-1
    hi_index=math.ceil(.95*len(reps))-1
    all_n=rows; all_hit=sum(c[success_key] for block in clusters_by_seed.values() for c in block.values())
    return {"pass_data_minimum":True,"unique_clusters":unique,"scored_rows":rows,"empirical_fraction":all_hit/all_n,"lower95":reps[lo_index],"upper95":reps[hi_index],"bootstrap_seed":bootstrap_seed,"replicates":len(reps),"hierarchy":"seed_blocks_then_whole_flights"}

def _merge_cluster_group(scores, container, group):
    merged={}
    for score in scores:
        clusters=score.get("containment",{}).get(container,{}).get(group,{})
        for cluster,row in clusters.items():
            seed,flight=cluster.split(":",1)
            seed=int(seed)
            target=merged.setdefault(seed,{})
            if flight in target: raise StudyError("duplicate seed/flight containment cluster across cases")
            target[flight]=row
    return merged

def _cohort_calibration(scores, groups, container="clusters"):
    study=_study(); floors={int(k):v for k,v in study["uncertainty"]["coverage_floors_by_nominal_scenario"].items()}; report={}; passed=True
    for group in groups:
        clusters=_merge_cluster_group(scores,container,group)
        group_result={}
        for p in (50,90,95):
            boot=_cohort_bootstrap(clusters,f"hits{p}")
            boot["floor"]=floors[p]
            boot["pass"]=bool(boot["pass_data_minimum"] and boot["lower95"]>=floors[p])
            group_result[str(p)]=boot
            passed &= boot["pass"]
        report[group]=group_result
    return passed,report

def choose_training(scores):
    study=_study(); expected_seeds={1,2,3}; scenario_list=study["training"]["scenario_set"] if "scenario_set" in study["training"] else [*study["main_profile_gates"]["nominal_scenarios"],"dropout","onset-exit"]
    expected={(seed,sc) for seed in expected_seeds for sc in scenario_list}; by_setting=defaultdict(list)
    for score in scores:
        if score.get("study_id")!="openatca-motion-study-v1" or score.get("seed") not in expected_seeds or score.get("scenario") not in scenario_list:
            raise StudyError("training score has wrong study, seed, or scenario")
        expected_role="nominal" if score["scenario"] in study["main_profile_gates"]["nominal_scenarios"] else "diagnostic_only"
        if score.get("role")!=expected_role: raise StudyError("training scenario role differs from preregistration")
        by_setting[score.get("setting_id")].append(score)
    if set(by_setting)!=set(ALLOWED_SETTINGS): raise StudyError("training requires all three preregistered settings")
    settings=[]
    for setting in sorted(ALLOWED_SETTINGS):
        rows=by_setting[setting]
        if len(rows)!=len(expected) or {(r.get("seed"),r.get("scenario")) for r in rows}!=expected:
            raise StudyError(f"training cases incomplete or duplicated for {setting}")
        if any(r.get("default_H_Q_CV")!=1.0 or r.get("H_Q_CV")!=ALLOWED_SETTINGS[setting] or r.get("input_hashes_match") is not True or not isinstance(r.get("candidate_native_guard_pass"),bool) or not isinstance(r.get("default_native_guard_pass"),bool) for r in rows): raise StudyError("training candidate/default configuration, native guard evidence, or input pairing differs from preregistration")
        if any(r.get("hashes",{}).get("candidate_source_head")!=r.get("hashes",{}).get("default_source_head") or r.get("hashes",{}).get("candidate_source")!=r.get("hashes",{}).get("default_source") or r.get("hashes",{}).get("candidate_normalized_params")!=r.get("hashes",{}).get("default_normalized_params") for r in rows): raise StudyError("training candidate/default source lineage differs")
        src={r.get("hashes",{}).get("candidate_source") for r in rows}
        default_src={r.get("hashes",{}).get("default_source") for r in rows}
        default_params_src={r.get("hashes",{}).get("default_params_source") for r in rows}
        param_sets={json.dumps(r.get("parameters_except_H_Q_CV"),sort_keys=True) for r in rows}
        if len(src)!=1 or len(param_sets)!=1 or len(default_src)!=1 or len(default_params_src)!=1: raise StudyError("candidate/default source or unchanged parameter provenance differs within setting")
        all_case_gates=all(r.get("selection_pass") is True and r["candidate_native_guard_pass"] and r["default_native_guard_pass"] for r in rows)
        calibration_pass, calibration=_cohort_calibration([r for r in rows if r["role"]=="nominal"],study["main_profile_gates"]["nominal_scenarios"])
        window_pass, windows=_cohort_calibration([r for r in rows if r["scenario"]=="onset-exit"],("even:onset","even:exit","odd:onset","odd:exit"),"onset_exit_window_clusters")
        pooled_num=pooled_den=0.0
        for r in rows:
            if r["role"]!="nominal": continue
            stat=r["paired_metrics"]["candidate"]["position"]
            if stat["rms"] is not None and stat["count"]:
                pooled_num += stat["rms"]**2*stat["count"]; pooled_den += stat["count"]
        pooled=math.sqrt(pooled_num/pooled_den) if pooled_den else None
        passed=all_case_gates and calibration_pass and window_pass
        heads={r.get("hashes",{}).get("candidate_source_head") for r in rows}; normalized={r.get("hashes",{}).get("candidate_normalized_params") for r in rows}; default_normalized={r.get("hashes",{}).get("default_normalized_params") for r in rows}
        if len(heads)!=1 or len(normalized)!=1 or len(default_normalized)!=1: raise StudyError("source head or normalized parameters vary within setting")
        settings.append({"setting_id":setting,"H_Q_CV":ALLOWED_SETTINGS[setting],"parameters_except_H_Q_CV":json.loads(next(iter(param_sets))),"source_head":next(iter(heads)),"normalized_params_sha256":next(iter(normalized)),"default_normalized_params_sha256":next(iter(default_normalized)),"case_gates_pass":all_case_gates,"nominal_undercoverage_pass":calibration_pass,"nominal_containment":calibration,"onset_exit_undercoverage_pass":window_pass,"onset_exit_containment":windows,"training_pass":passed,"pooled_nominal_position_rms_m":pooled if passed else None,"case_count":len(rows),"candidate_source_sha256":next(iter(src)),"default_source_sha256":next(iter(default_src)),"default_params_source_sha256":next(iter(default_params_src))})
    if len({x["candidate_source_sha256"] for x in settings})!=1 or len({x["source_head"] for x in settings})!=1 or len({x["normalized_params_sha256"] for x in settings})!=1 or len({x["default_normalized_params_sha256"] for x in settings})!=1 or len({json.dumps(x["parameters_except_H_Q_CV"],sort_keys=True) for x in settings})!=1:
        raise StudyError("three candidate settings must share one implementation and differ only in H_Q_CV")
    if len({x["default_source_sha256"] for x in settings})!=1 or len({x["default_params_source_sha256"] for x in settings})!=1:
        raise StudyError("frozen-default implementation must be identical across all training cases")
    by_case={}
    for setting,rows in by_setting.items():
        for row in rows:
            key=(row["seed"],row["scenario"])
            hashes=row.get("hashes",{})
            fingerprint=(hashes.get("candidate_truth"),hashes.get("candidate_measurements"),hashes.get("default_truth"),hashes.get("default_measurements"))
            if row.get("input_hashes_match") is not True or not all(isinstance(x,str) and re.fullmatch(r"[0-9a-f]{64}",x) for x in fingerprint): raise StudyError("training score lacks paired input hashes")
            if fingerprint[0]!=fingerprint[2] or fingerprint[1]!=fingerprint[3]: raise StudyError("training score candidate/default input hashes differ")
            if key in by_case and by_case[key]!=fingerprint: raise StudyError("candidate settings were not run on byte-identical inputs")
            by_case[key]=fingerprint
    eligible=[x for x in settings if x["training_pass"] and x["pooled_nominal_position_rms_m"] is not None]
    winner=min(eligible,key=lambda x:(x["pooled_nominal_position_rms_m"],x["H_Q_CV"],x["setting_id"])) if eligible else None
    return {"schema":"openatca-motion-study-selection-v1","study_id":"openatca-motion-study-v1","seed_ids":sorted(expected_seeds),"scenario_set":scenario_list,"setting_results":settings,"selected_setting_id":winner["setting_id"] if winner else None,"selected_H_Q_CV":winner["H_Q_CV"] if winner else None,"status":"selected_for_single_development_check" if winner else "no_candidate_passed","cross_scenario_confidence_intervals":None,"bootstrap_protocol":_study()["uncertainty"]["bootstrap"],"claim_scope":"developmental_model_check_only"}

def validate_check(selection, scores):
    winner=selection.get("selected_setting_id")
    if selection.get("status")!="selected_for_single_development_check" or winner not in ALLOWED_SETTINGS:
        raise StudyError("development check requires a previously selected training winner")
    expected={(seed,scenario) for seed in (4,5,6) for scenario in ("straight","turn","acceleration","crossing","dropout","delayed","onset-exit")}
    actual={(s.get("seed"),s.get("scenario")) for s in scores}
    winner_hq=ALLOWED_SETTINGS[winner]
    if len(scores)!=len(expected) or actual!=expected or any(s.get("setting_id")!=winner or s.get("H_Q_CV")!=winner_hq or s.get("default_H_Q_CV")!=1.0 or s.get("input_hashes_match") is not True for s in scores):
        raise StudyError("development check requires exactly the selected setting and frozen default across seeds 4-6 and seven scenarios")
    selected=next((row for row in selection.get("setting_results",[]) if row.get("setting_id")==winner),None)
    if not selected: raise StudyError("selection record lacks the chosen candidate source/configuration")
    for score in scores:
        hashes=score.get("hashes",{})
        if score.get("input_hashes_match") is not True or not isinstance(score.get("candidate_native_guard_pass"),bool) or not isinstance(score.get("default_native_guard_pass"),bool) or hashes.get("candidate_source")!=hashes.get("default_source") or hashes.get("candidate_source_head")!=hashes.get("default_source_head") or hashes.get("candidate_normalized_params")!=hashes.get("default_normalized_params") or hashes.get("candidate_source")!=selected.get("candidate_source_sha256") or hashes.get("candidate_source_head")!=selected.get("source_head") or hashes.get("candidate_normalized_params")!=selected.get("normalized_params_sha256") or score.get("parameters_except_H_Q_CV")!=selected.get("parameters_except_H_Q_CV"):
            raise StudyError("development check candidate source/parameters differ from the selected training winner")
    checks=all(s.get("selection_pass") is True and s["candidate_native_guard_pass"] and s["default_native_guard_pass"] for s in scores)
    study=_study()
    nominal_pass,nominal_report=_cohort_calibration([s for s in scores if s["role"]=="nominal"],study["main_profile_gates"]["nominal_scenarios"])
    windows_pass,windows_report=_cohort_calibration([s for s in scores if s["scenario"]=="onset-exit"],("even:onset","even:exit","odd:onset","odd:exit"),"onset_exit_window_clusters")
    return {"schema":"openatca-motion-study-check-v1","study_id":"openatca-motion-study-v1","selected_setting_id":winner,"case_count":len(scores),"all_preregistered_case_gates_pass":checks,"check_nominal_undercoverage_pass":nominal_pass,"check_nominal_containment":nominal_report,"check_onset_exit_undercoverage_pass":windows_pass,"check_onset_exit_containment":windows_report,"retune_permitted":False,"claim_scope":"developmental_model_check_only"}


def onset_exit_truth(time_s, sign=1):
    """Mirrored analytic ENU displacement and velocity from the existing per-flight origin."""
    if sign not in (-1,1): raise StudyError("turn sign must be -1 or 1")
    w=math.radians(2.0); speed=150.0
    if time_s<60.0: return speed*time_s,0.0,speed,0.0
    q=sign*w*(time_s-60.0); radius=speed/(sign*w)
    if time_s<=120.0:
        return 9000.0+radius*math.sin(q),radius*(1-math.cos(q)),speed*math.cos(q),speed*math.sin(q)
    q=sign*w*60.0; xe=9000.0+radius*math.sin(q); ye=radius*(1-math.cos(q))
    ve=speed*math.cos(q); vn=speed*math.sin(q); dt=time_s-120.0
    return xe+ve*dt,ye+vn*dt,ve,vn



def main(argv=None):
    parser=argparse.ArgumentParser(description=__doc__)
    sub=parser.add_subparsers(dest="command",required=True)
    pair=sub.add_parser("score"); pair.add_argument("--candidate-dir",required=True); pair.add_argument("--candidate-build",required=True); pair.add_argument("--default-dir",required=True); pair.add_argument("--default-build",required=True); pair.add_argument("--out",required=True)
    select=sub.add_parser("select"); select.add_argument("--scores",required=True); select.add_argument("--out",required=True)
    check=sub.add_parser("check"); check.add_argument("--selection",required=True); check.add_argument("--scores",required=True); check.add_argument("--out",required=True)
    args=parser.parse_args(argv)
    try:
        _refuse(*(getattr(args,k) for k in ("candidate_dir","candidate_build","default_dir","default_build","scores","selection","out") if getattr(args,k,None) is not None))
        if args.command=="score": result=score_pair(args.candidate_dir,args.candidate_build,args.default_dir,args.default_build)
        elif args.command=="select":
            raw=read_score_list(Path(args.scores)); result=choose_training(raw)
        elif args.command=="check":
            selection=read_json(Path(args.selection)); raw=read_score_list(Path(args.scores)); result=validate_check(selection,raw)
        out=Path(args.out); out.parent.mkdir(parents=True,exist_ok=True)
        with out.open("w",encoding="utf-8") as f: json.dump(result,f,indent=2,sort_keys=True,allow_nan=False); f.write("\n")
    except (StudyError,OSError,ValueError,TypeError,KeyError,json.JSONDecodeError) as exc:
        print(f"motion study: {exc}",file=sys.stderr); return 2
    return 0

if __name__=="__main__": raise SystemExit(main())
