#!/usr/bin/env python3
"""Strict, independent evaluator for paired trajectory benchmark CSVs."""
import argparse
import csv
import json
import math
import hashlib
import json
import os
import sys
from collections import defaultdict

MAX_FILE_BYTES = 128 * 1024 * 1024
MAX_ROWS = 2_000_000
MAX_FIELD = 4096
WGS84_A_M = 6378137.0
WGS84_F = 1 / 298.257223563
WGS84_E2 = WGS84_F * (2 - WGS84_F)
EXPECTED = {
    "truth": ["scenario_id", "flight_id", "time_ns", "lat_deg", "lon_deg", "ve_mps", "vn_mps", "phase"],
    "measurements": ["event_id", "scenario_id", "flight_id", "sensor_id", "sensor_ns", "receive_ns", "lat_deg", "lon_deg", "ve_mps", "vn_mps", "sigma_pos_m", "sigma_vel_mps"],
    "output": ["scenario_id", "flight_id", "score_ns", "source_ns", "publish_ns", "lat_deg", "lon_deg", "ve_mps", "vn_mps", "status"],
    "grid": ["scenario_id", "flight_id", "time_ns"],
}

class EvaluationError(ValueError):
    pass

def _csv(path, kind):
    try:
        if __import__('os').path.getsize(path) > MAX_FILE_BYTES:
            raise EvaluationError(f"{path}: exceeds byte limit {MAX_FILE_BYTES}")
        with open(path, newline="", encoding="utf-8-sig") as f:
            reader = csv.DictReader(f, strict=True)
            if reader.fieldnames is None or len(reader.fieldnames) != len(set(reader.fieldnames)):
                raise EvaluationError(f"{path}: missing or duplicate header")
            if kind == "grid" and reader.fieldnames != EXPECTED[kind]:
                raise EvaluationError(f"{path}: grid must contain only scenario_id, flight_id, time_ns")
            if set(EXPECTED[kind]) != set(reader.fieldnames):
                missing=sorted(set(EXPECTED[kind])-set(reader.fieldnames)); extra=sorted(set(reader.fieldnames)-set(EXPECTED[kind]))
                raise EvaluationError(f"{path}: schema mismatch (missing={missing}, extra={extra})")
            rows = []
            for n, row in enumerate(reader, 2):
                if n > MAX_ROWS + 1:
                    raise EvaluationError(f"{path}: exceeds row limit {MAX_ROWS}")
                if None in row or any(v is None or len(v) > MAX_FIELD for v in row.values()):
                    raise EvaluationError(f"{path}:{n}: malformed or oversized record")
                rows.append(row)
            order_fields={"truth":("scenario_id","time_ns","flight_id"),"measurements":("receive_ns","event_id")}.get(kind)
            if order_fields:
                previous=None
                for n,row in enumerate(rows,2):
                    try:
                        values=tuple(int(row[field]) if field in ("time_ns","receive_ns") else (_id_order(row[field]) if field in ("flight_id","event_id") else row[field]) for field in order_fields)
                    except ValueError as exc: raise EvaluationError(f"{path}:{n}: invalid ordering timestamp") from exc
                    if previous is not None and values<=previous:
                        raise EvaluationError(f"{path}:{n}: rows must be ordered by {', '.join(order_fields)}")
                    previous=values
            return rows
    except (OSError, UnicodeError, csv.Error) as exc:
        raise EvaluationError(f"{path}: {exc}") from exc

def _num(row, key, where, integer=False):
    try:
        value = int(row[key]) if integer else float(row[key])
    except (ValueError, TypeError, KeyError) as exc:
        raise EvaluationError(f"{where}: invalid {key}") from exc
    if integer and abs(value)>9_223_372_036_854_775_807:
        raise EvaluationError(f"{where}: out-of-range {key}")
    if not math.isfinite(value):
        raise EvaluationError(f"{where}: non-finite {key}")
    return value

def _key(row):
    return row["scenario_id"], row["flight_id"]

def _id_order(value):
    try: return 0,int(value)
    except ValueError: return 1,value

def _xyz(lat, lon):
    a, b = math.radians(lat), math.radians(lon)
    n = WGS84_A_M / math.sqrt(1-WGS84_E2*math.sin(a)**2)
    return n*math.cos(a)*math.cos(b), n*math.cos(a)*math.sin(b), n*(1-WGS84_E2)*math.sin(a)

def horizontal_error_m(a, b):
    """WGS84 ECEF to local ENU horizontal separation, including dateline wrap."""
    alat, alon = math.radians(a[0]), math.radians(a[1])
    x1, y1, z1 = _xyz(a[0], a[1]); x2, y2, z2 = _xyz(b[0], b[1])
    dx, dy, dz = x2-x1, y2-y1, z2-z1
    east = -math.sin(alon)*dx + math.cos(alon)*dy
    north = -math.sin(alat)*math.cos(alon)*dx - math.sin(alat)*math.sin(alon)*dy + math.cos(alat)*dz
    return math.hypot(east, north)

def _stats(values):
    if not values: return {"count": 0, "rms": None, "p95": None, "p99": None, "mean": None}
    ordered = sorted(values)
    def pct(p): return ordered[min(len(ordered)-1, math.ceil(p*len(ordered))-1)]
    scale=max(abs(x) for x in values)
    rms=0.0 if scale==0 else scale*math.sqrt(math.fsum((x/scale)**2 for x in values)/len(values))
    mean=0.0 if scale==0 else scale*math.fsum(x/scale for x in values)/len(values)
    return {"count": len(values), "rms": rms, "p95": pct(.95), "p99": pct(.99), "mean": mean}

def _sha256(path):
    if os.path.getsize(path)>MAX_FILE_BYTES: raise EvaluationError(f"{path}: exceeds byte limit {MAX_FILE_BYTES}")
    digest=hashlib.sha256()
    with open(path,"rb") as f:
        for block in iter(lambda:f.read(65536),b""): digest.update(block)
    return digest.hexdigest()

def _files_match(first,second):
    try: return os.path.getsize(first)==os.path.getsize(second) and _sha256(first)==_sha256(second)
    except OSError: return False

def _claim_class(value):
    return value in ("professional_tracker","legacy_export","controlled_legacy")

def _known_comparison_class(value):
    return value in ("development_control","professional_tracker","legacy_export","controlled_legacy")

def _derive_profile(protocol,manifest):
    """Bind role/cohort to frozen scenario and seed registries, never metadata labels."""
    try:
        scenario=manifest["scenario_id"]; seed=manifest["seed"]
        if isinstance(seed,bool) or not isinstance(seed,int): return {"valid":False,"reason":"invalid_seed"}
        setup=protocol["initialization_and_qualification"]
        nominal=set(setup["nominal_scenarios"]); diagnostic=set(setup["diagnostic_scenarios"])
        fault=protocol["fault_mode_cohort"]
        memberships=[]
        if seed in protocol["seeds_and_holdout"]["development_seeds"]["seed_ids"]: memberships.append(("development-set-v1",False))
        if seed in protocol["seeds_and_holdout"]["qualification"]["seed_ids"]: memberships.append(("qualification-set-v1",False))
        if seed in fault["seed_ids"]: memberships.append((fault["cohort_id"],True))
        if len(memberships)!=1: return {"valid":False,"reason":"unknown_or_ambiguous_seed"}
        cohort,is_fault=memberships[0]
        in_nominal=scenario in nominal; in_diagnostic=scenario in diagnostic
        if is_fault:
            if scenario!=fault["scenario"]: return {"valid":False,"reason":"fault_seed_scenario_mismatch"}
            role="diagnostic_only"
        else:
            if in_nominal==in_diagnostic: return {"valid":False,"reason":"unknown_or_ambiguous_scenario"}
            role="nominal" if in_nominal else "diagnostic_only"
        return {"valid":True,"scenario_id":scenario,"seed":seed,"cohort_id":cohort,"role":role}
    except (KeyError,TypeError):
        return {"valid":False,"reason":"profile_manifest_schema_mismatch"}

def _profile_binding_valid(derived,metadata,scoring_manifest):
    return (derived.get("valid") is True and metadata.get("scenario_role")==derived.get("role") and metadata.get("cohort_id")==derived.get("cohort_id") and scoring_manifest.get("scenario_role")==derived.get("role") and scoring_manifest.get("cohort_id")==derived.get("cohort_id"))

def _profile_gate(role,init_ns,coast_limit_ns,epochs_complete,coverage,minimum=.999,cohort_id=None,same_usable_keys=True):
    result={"cohort_id":cohort_id,"role":role,"minimum_usable_fraction":minimum,"candidate_usable_fraction":coverage.get("candidate",{}).get("usable_fraction"),"baseline_usable_fraction":coverage.get("baseline",{}).get("usable_fraction"),"candidate_at_least_baseline":False,"same_post_initialization_usable_keys":same_usable_keys,"status":"unqualified"}
    if role=="diagnostic_only": result["status"]="diagnostic_only"; return result
    if role!="nominal": result["status"]="unknown_scenario_role"; return result
    if init_ns!=2_000_000_000 or coast_limit_ns!=2_000_000_000 or not epochs_complete:
        result["status"]="profile_or_epoch_mismatch"; return result
    c=result["candidate_usable_fraction"]; b=result["baseline_usable_fraction"]
    if c is None or b is None: result["status"]="no_post_initialization_samples"; return result
    result["candidate_at_least_baseline"]=c>=b
    result["same_post_initialization_usable_keys"]=same_usable_keys
    if any(coverage.get(x,{}).get("coast_profile_violations",1)>0 for x in ("candidate","baseline")):
        result["status"]="coast_age_profile_violation"; return result
    if b<minimum or c<minimum: result["status"]="nominal_coverage_not_met"; return result
    if c<b: result["status"]="availability_regression"; return result
    if not same_usable_keys: result["status"]="paired_usable_key_mismatch"; return result
    result["status"]="nominal_profile_pass"; return result

def _read_json(path):
    if os.path.getsize(path)>MAX_FILE_BYTES: raise EvaluationError(f"{path}: exceeds byte limit {MAX_FILE_BYTES}")
    def unique(pairs):
        result={}
        for key,value in pairs:
            if key in result: raise ValueError(f"duplicate JSON key: {key}")
            result[key]=value
        return result
    with open(path,encoding="utf-8") as f: return json.load(f,object_pairs_hook=unique,parse_constant=lambda value: (_ for _ in ()).throw(ValueError(f"invalid JSON constant {value}")))

def _zero_losses(value):
    if isinstance(value,bool): return False
    if isinstance(value,(int,float)): return value==0
    return isinstance(value,dict) and bool(value) and all(isinstance(v,(int,float)) and not isinstance(v,bool) and math.isfinite(v) and v==0 for v in value.values())

def _load_timing_ledger(spec,input_fingerprint):
    try:
        path=spec.get("path")
        if not path or _sha256(path)!=spec.get("sha256"): return None
        data=_read_json(path)
        if not isinstance(data,dict): return None
        elapsed=data.get("elapsed_ns"); units=data.get("work_units")
        valid=(data.get("clock_kind")=="monotonic_wall" and data.get("drained") is True and data.get("input_fingerprint")==input_fingerprint and isinstance(elapsed,int) and not isinstance(elapsed,bool) and 0<elapsed<=9_223_372_036_854_775_807 and isinstance(units,int) and not isinstance(units,bool) and 0<units<=9_223_372_036_854_775_807 and _zero_losses(data.get("loss_counters")))
        return data if valid else None
    except (OSError,EvaluationError,TypeError,KeyError,ValueError,AttributeError,json.JSONDecodeError):
        return None

def _ledger_throughput(ledger):
    if not isinstance(ledger,dict): return None
    elapsed=ledger.get("elapsed_ns"); units=ledger.get("work_units")
    if isinstance(elapsed,bool) or not isinstance(elapsed,int) or elapsed<=0 or isinstance(units,bool) or not isinstance(units,int) or units<=0: return None
    return units*1e9/elapsed

def _validate_identity_binding(path, truth_keys, output_rows):
    fields=["system","scenario_id","flight_id","score_ns","track_id","truth_flight_id"]
    try:
        with open(path,newline="",encoding="utf-8-sig") as f:
            reader=csv.DictReader(f,strict=True)
            if reader.fieldnames!=fields: return False,"identity_binding_schema_mismatch"
            seen=set(); track_truth={}; bound={"candidate":set(),"baseline":set()}
            outputs={name:{(*_key(r),int(r["score_ns"])):r for r in rows} for name,rows in output_rows.items()}
            for row in reader:
                system=row["system"]
                if system not in bound: return False,"identity_binding_unknown_system"
                key=(row["scenario_id"],row["flight_id"],int(row["score_ns"]))
                skey=(system,*key)
                if skey in seen or key not in truth_keys or key not in outputs[system]: return False,"identity_binding_unexpected_or_duplicate_key"
                seen.add(skey); bound[system].add(key)
                truth_flight=row["truth_flight_id"]
                if truth_flight!=row["flight_id"]: return False,"identity_binding_mismatch"
                track=row["track_id"]
                status=outputs[system][key]["status"]
                if status in ("valid","coast") and not track: return False,"identity_binding_missing_track"
                if track:
                    tk=(system,key[0],track)
                    if tk in track_truth and track_truth[tk]!=truth_flight: return False,"identity_binding_track_reassigned"
                    track_truth[tk]=truth_flight
            if any(bound[s]!=set(outputs[s]) for s in bound): return False,"identity_binding_incomplete"
            return True,"known_identifier_key_only"
    except (OSError,csv.Error,ValueError,TypeError):
        return False,"identity_binding_unreadable"

def _bootstrap_ratio(groups, metric="rms", seed=173):
    # Deterministic hierarchical bootstrap: scenarios, then flights within scenarios.
    import random
    scenarios=defaultdict(list)
    for (scenario,flight),values in groups.items(): scenarios[scenario].append((flight,values))
    if len(scenarios) < 2 or sum(len(v) for v in scenarios.values()) < 2:
        return {"low95": None, "high95": None, "scenario_clusters":len(scenarios),"flight_clusters":len(groups)}
    rng = random.Random(seed); names=sorted(scenarios); samples=[]
    for _ in range(2000):
        chosen=[]
        for _s in names:
            s=rng.choice(names); flights=scenarios[s]
            chosen.extend(groups[(s,rng.choice(flights)[0])] for _ in flights)
        ca=[x for group in chosen for x in group[0]]; ba=[x for group in chosen for x in group[1]]
        if not ca or not ba: continue
        if metric=="rms": num=_stats(ca)["rms"]; den=_stats(ba)["rms"]
        else: num=_stats(ca)["p99"]; den=_stats(ba)["p99"]
        if den>1e-9: samples.append(num/den)
    if not samples: return {"low95":None,"high95":None,"scenario_clusters":len(scenarios),"flight_clusters":len(groups)}
    samples.sort()
    return {"low95": samples[int(.025*len(samples))], "high95": samples[min(len(samples)-1, int(.975*len(samples)))], "scenario_clusters":len(scenarios),"flight_clusters":len(groups)}

def evaluate(truth_path, measurements_path, candidate_path, baseline_path, metadata=None):
    truth = _csv(truth_path, "truth"); measurements = _csv(measurements_path, "measurements")
    candidate = _csv(candidate_path, "output"); baseline = _csv(baseline_path, "output")
    meta = metadata or {}
    canonical_protocol=os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))),"validation","comparison-protocol-v1.json")
    try: scoring_protocol=_read_json(canonical_protocol)
    except (OSError,EvaluationError,ValueError,json.JSONDecodeError): scoring_protocol={}
    early_artifacts=meta.get("artifacts",{})
    manifest_spec=(early_artifacts.get("comparison_manifest") or early_artifacts.get("manifest",{})) if isinstance(early_artifacts,dict) else {}
    manifest_path=manifest_spec.get("path") if isinstance(manifest_spec,dict) else None
    raw_spec=early_artifacts.get("manifest",{}) if isinstance(early_artifacts,dict) else {}
    raw_path=raw_spec.get("path") if isinstance(raw_spec,dict) else None
    try:
        scoring_manifest=_read_json(manifest_path) if manifest_path else {}
        if not isinstance(scoring_manifest,dict): scoring_manifest={}
    except (OSError,EvaluationError,ValueError,json.JSONDecodeError):
        scoring_manifest={}
    try:
        raw_manifest=_read_json(raw_path) if raw_path else {}
        if not isinstance(raw_manifest,dict): raw_manifest={}
    except (OSError,EvaluationError,ValueError,json.JSONDecodeError):
        raw_manifest={}
    truth_by={}; ordered_keys=[]; phases=defaultdict(list); first_grid_time={}
    for i,r in enumerate(truth,2):
        k=(*_key(r),_num(r,"time_ns",f"truth:{i}",True))
        if k in truth_by: raise EvaluationError(f"truth:{i}: duplicate grid key {k}")
        if ordered_keys and (k[0],k[2],_id_order(k[1])) <= (ordered_keys[-1][0],ordered_keys[-1][2],_id_order(ordered_keys[-1][1])): raise EvaluationError("truth rows must be strictly ordered by scenario, time, flight")
        ordered_keys.append(k)
        first_grid_time.setdefault(k[:2],k[2])
        vals={c:_num(r,c,f"truth:{i}") for c in ("lat_deg","lon_deg","ve_mps","vn_mps")}
        if abs(vals["lat_deg"])>90 or abs(vals["lon_deg"])>180: raise EvaluationError(f"truth:{i}: coordinate out of range")
        truth_by[k]=(vals,r["phase"]); phases[(k[0],k[1],r["phase"])].append(k)
    meas_events=set(); received_at={}
    for i,r in enumerate(measurements,2):
        if not r["event_id"] or r["event_id"] in meas_events: raise EvaluationError(f"measurements:{i}: duplicate/empty event_id")
        meas_events.add(r["event_id"])
        for c in ("sensor_ns","receive_ns","lat_deg","lon_deg","ve_mps","vn_mps","sigma_pos_m","sigma_vel_mps"):
            _num(r,c,f"measurements:{i}",c.endswith("_ns"))
        mk=(*_key(r),int(r["sensor_ns"]))
        received_at[mk]=max(received_at.get(mk,-1),int(r["receive_ns"]))
    def index(rows,name):
        out={}; duplicates=0
        for i,r in enumerate(rows,2):
            sk=_key(r); score=_num(r,"score_ns",f"{name}:{i}",True); k=(*sk,score)
            if k in out: duplicates+=1; continue
            if r["status"] not in ("valid","coast","missing"): raise EvaluationError(f"{name}:{i}: invalid status")
            publish=_num(r,"publish_ns",f"{name}:{i}",True)
            if r["status"]=="missing":
                if any(r[c]!="" for c in ("source_ns","lat_deg","lon_deg","ve_mps","vn_mps")):
                    raise EvaluationError(f"{name}:{i}: missing output must have blank estimate fields")
            else:
                source=_num(r,"source_ns",f"{name}:{i}",True)
                if source > publish: raise EvaluationError(f"{name}:{i}: source_ns after publish_ns")
            if score != publish: raise EvaluationError(f"{name}:{i}: accuracy score time differs from publish time")
            out[k]=r
        return out,duplicates
    cand,cand_duplicates=index(candidate,"candidate"); base,base_duplicates=index(baseline,"baseline")
    expected=set(truth_by)
    false_identity=len(set(cand)-expected)+len(set(base)-expected)
    by_system={}; rows_by_flight={}
    init_ns=scoring_manifest.get("initialization_ns",meta.get("initialization_ns",0))
    if isinstance(init_ns,bool) or not isinstance(init_ns,int) or init_ns<0: init_ns=None
    coast_limit_ns=scoring_manifest.get("coast_max_age_ns",meta.get("coast_max_age_ns"))
    if isinstance(coast_limit_ns,bool) or not isinstance(coast_limit_ns,int) or coast_limit_ns<0: coast_limit_ns=None
    scenario_epochs=scoring_manifest.get("scenario_epochs_ns",meta.get("scenario_epochs_ns",{}))
    if not isinstance(scenario_epochs,dict): scenario_epochs={}
    epoch_valid={scenario:time for scenario,time in scenario_epochs.items() if isinstance(time,int) and not isinstance(time,bool) and abs(time)<=9_223_372_036_854_775_807}
    if not epoch_valid and init_ns==0: epoch_valid={scenario:time for scenario,time in first_grid_time.items()}
    epochs_complete=set(k[0] for k in truth_by)==set(epoch_valid) and (not scoring_manifest or meta.get("scenario_epochs_ns",scenario_epochs)==scenario_epochs)
    post_totals={}; post_usable_keysets={}
    for name,out in (("candidate",cand),("baseline",base)):
        errors=[]; velocity=[]; post_errors=[]; post_velocity=[]; cold_errors=[]; cold_velocity=[]; latencies=[]; source_ages=[]; coast_ages=[]; missing=stale=valid=absent=explicit_missing=0; post_den=post_usable=post_valid=post_coast=post_missing=0; post_coast_violations=0; flights=defaultdict(list); post_flights=defaultdict(list); scenario_errors=defaultdict(list); phase_errors=defaultdict(list); flight_velocity=defaultdict(list); scenario_velocity=defaultdict(list); phase_velocity=defaultdict(list)
        usable_keys=set()
        for k,(t,phase) in truth_by.items():
            is_post=init_ns is not None and epochs_complete and k[0] in epoch_valid and k[2]>=epoch_valid[k[0]]+init_ns
            if is_post: post_den+=1
            r=out.get(k)
            if r is None:
                absent+=1; missing+=1
                if is_post: post_missing+=1
                continue
            if r["status"]=="missing":
                explicit_missing+=1; missing+=1
                if is_post: post_missing+=1
                continue
            if r["status"]=="coast": stale+=1
            else: valid+=1
            pos=[_num(r,c,f"{name}:{k}") for c in ("lat_deg","lon_deg")]
            if abs(pos[0])>90 or abs(pos[1])>180: raise EvaluationError(f"{name}: coordinate out of range")
            e=horizontal_error_m((t["lat_deg"],t["lon_deg"]),pos)
            v=math.hypot(_num(r,"ve_mps",f"{name}:{k}")-t["ve_mps"],_num(r,"vn_mps",f"{name}:{k}")-t["vn_mps"])
            errors.append(e); velocity.append(v); flights[k[:2]].append(e); scenario_errors[k[0]].append(e); phase_errors[(k[0],k[1],phase)].append(e); flight_velocity[k[:2]].append(v); scenario_velocity[k[0]].append(v); phase_velocity[(k[0],k[1],phase)].append(v)
            # delay metric is receive-to-usable based on newest source event only when source matches an input receive timestamp
            source_ns=_num(r,"source_ns",f"{name}:{k}",True); publish_ns=_num(r,"publish_ns",f"{name}:{k}",True)
            source_ages.append((publish_ns-source_ns)/1e6)
            if r["status"]=="coast": coast_ages.append((publish_ns-source_ns)/1e6)
            age_ns=publish_ns-source_ns
            if is_post and coast_limit_ns is not None and ((r["status"]=="valid" and age_ns>coast_limit_ns) or (r["status"]=="coast" and age_ns<=coast_limit_ns)):
                post_coast_violations+=1
            usable=(r["status"]=="valid" and (coast_limit_ns is None or age_ns<=coast_limit_ns)) or (r["status"]=="coast" and coast_limit_ns is not None and age_ns<=coast_limit_ns)
            if is_post:
                post_errors.append(e); post_velocity.append(v); post_flights[k[:2]].append(e)
                if r["status"]=="valid": post_valid+=1
                else: post_coast+=1
                if usable: post_usable+=1; usable_keys.add(k)
            else:
                cold_errors.append(e); cold_velocity.append(v)
            receive_ns=received_at.get((k[0],k[1],source_ns))
            if receive_ns is not None:
                latency=publish_ns-receive_ns
                if latency < 0: raise EvaluationError(f"{name}: usable output precedes its input receive time")
                latencies.append(latency/1e6)
        total=len(truth_by); coverage={"truth_rows":post_den,"valid_rows":post_valid,"coast_rows":post_coast,"missing_rows":post_missing,"usable_rows":post_usable,"usable_fraction":post_usable/post_den if post_den else None,"coast_profile_violations":post_coast_violations}
        by_system[name]={"position_error_m":_stats(errors),"velocity_error_mps":_stats(velocity),"post_initialization_position_error_m":_stats(post_errors),"post_initialization_velocity_error_mps":_stats(post_velocity),"cold_start_position_error_m":_stats(cold_errors),"cold_start_velocity_error_mps":_stats(cold_velocity),"post_initialization_coverage":coverage,"valid_count":valid,"valid_fraction":valid/total if total else None,"stale_coast":stale,"coast_fraction":stale/total if total else None,"missing":missing,"missing_fraction":missing/total if total else None,"absent_output_rows":absent,"explicit_missing_rows":explicit_missing,"source_age_ms":_stats(source_ages),"coast_source_age_ms":_stats(coast_ages),"source_timestamp_delay_proxy_ms":_stats(latencies),"receive_to_usable_ms":_stats([]),"receive_to_usable_evidence":"unavailable_without_first_use_event_ledger","source_timestamp_unmatched_rows":len(errors)-len(latencies),"per_scenario_position_error_m":{str(k):_stats(v) for k,v in scenario_errors.items()},"per_flight_position_error_m":{str(k):_stats(v) for k,v in flights.items()},"phase_position_error_m":{str(k):_stats(v) for k,v in phase_errors.items()},"per_scenario_velocity_error_mps":{str(k):_stats(v) for k,v in scenario_velocity.items()},"per_flight_velocity_error_mps":{str(k):_stats(v) for k,v in flight_velocity.items()},"phase_velocity_error_mps":{str(k):_stats(v) for k,v in phase_velocity.items()}}
        rows_by_flight[name]=post_flights; post_totals[name]=coverage; post_usable_keysets[name]=usable_keys
    same_usable_keys=post_usable_keysets["candidate"]==post_usable_keysets["baseline"]
    derived_profile=_derive_profile(scoring_protocol,raw_manifest)
    profile_binding_valid=_profile_binding_valid(derived_profile,meta,scoring_manifest)
    profile_gate=_profile_gate(derived_profile.get("role") if profile_binding_valid else None,init_ns,coast_limit_ns,epochs_complete,post_totals,cohort_id=derived_profile.get("cohort_id") if profile_binding_valid else None,same_usable_keys=same_usable_keys)
    common=post_usable_keysets["candidate"]&post_usable_keysets["baseline"]; groups=defaultdict(lambda:([],[]))
    for k in common:
        t,_phase=truth_by[k]; c=cand[k]; b=base[k]
        ce=horizontal_error_m((t["lat_deg"],t["lon_deg"]),(_num(c,"lat_deg",f"candidate:{k}"),_num(c,"lon_deg",f"candidate:{k}")))
        be=horizontal_error_m((t["lat_deg"],t["lon_deg"]),(_num(b,"lat_deg",f"baseline:{k}"),_num(b,"lon_deg",f"baseline:{k}")))
        groups[k[:2]][0].append(ce); groups[k[:2]][1].append(be)
    paired_rms=_bootstrap_ratio(groups,"rms"); paired_p99=_bootstrap_ratio(groups,"p99",seed=271)
    paired_candidate=[x for cvals,bvals in groups.values() for x in cvals]
    paired_baseline=[x for cvals,bvals in groups.values() for x in bvals]
    c_stats=_stats(paired_candidate); b_stats=_stats(paired_baseline)
    b_rms=b_stats["rms"]; c_rms=c_stats["rms"]
    b_p99=b_stats["p99"]; c_p99=c_stats["p99"]
    paired_grid={"candidate_usable_rows":len(post_usable_keysets["candidate"]),"baseline_usable_rows":len(post_usable_keysets["baseline"]),"common_usable_rows":len(common),"candidate_only_usable_rows":len(post_usable_keysets["candidate"]-common),"baseline_only_usable_rows":len(post_usable_keysets["baseline"]-common),"same_usable_keys":same_usable_keys,"candidate_excluded_from_pairing":len(post_usable_keysets["candidate"]-common),"baseline_excluded_from_pairing":len(post_usable_keysets["baseline"]-common)}
    rms_ratio=(c_rms/b_rms) if c_rms is not None and b_rms is not None and b_rms>1e-9 else None
    p99_ratio=(c_p99/b_p99) if c_p99 is not None and b_p99 is not None and b_p99>1e-9 else None
    latency_ratio=None
    missing_meta=not all(k in meta for k in ("comparison_class","reference_qualified","reference_validation_status","reference_kind","input_fingerprint","same_input_fingerprint","identity_source","loss_counters","scenario_role","cohort_id","initialization_ns","coast_max_age_ns"))
    artifacts=meta.get("artifacts",{})
    if not isinstance(artifacts,dict): artifacts={}
    artifacts={k:(v if isinstance(v,dict) else {}) for k,v in artifacts.items()}
    artifact_names=("truth","measurements","candidate","baseline","manifest","comparison_manifest","protocol","reference_provenance","identity_binding")
    artifact_checks={}
    artifact_files={"truth":truth_path,"measurements":measurements_path,"candidate":candidate_path,"baseline":baseline_path}
    try:
        for name in artifact_names:
            artifact=artifacts.get(name,{})
            path=artifact.get("path")
            claimed=artifact.get("sha256")
            actual=_sha256(artifact_files[name] if name in artifact_files else path) if (name in artifact_files or path) else None
            artifact_checks[name]=bool(claimed and actual==claimed)
    except (OSError,TypeError,AttributeError):
        artifact_checks={name:False for name in artifact_names}
    artifact_integrity=all(artifact_checks.values())
    protocol_integrity=bool(artifact_checks.get("protocol") and _files_match(artifacts.get("protocol",{}).get("path", ""),canonical_protocol))
    flat_hashes={"truth":"truth_sha256","measurements":"measurements_sha256","candidate":"candidate_sha256","baseline":"baseline_sha256","manifest":"manifest_sha256","comparison_manifest":"comparison_manifest_sha256","protocol":"protocol_sha256","reference_provenance":"reference_provenance_sha256","identity_binding":"identity_binding_sha256"}
    flat_integrity=all(meta.get(flat)==(artifacts.get(name,{}) or {}).get("sha256") and artifact_checks.get(name) for name,flat in flat_hashes.items())
    identity_valid,identity_scope=_validate_identity_binding(artifacts.get("identity_binding",{}).get("path"),expected,{"candidate":candidate,"baseline":baseline}) if artifact_checks.get("identity_binding") else (False,"identity_binding_unavailable")
    try:
        run_manifest=raw_manifest
        ref_manifest=_read_json(artifacts["reference_provenance"]["path"])
    except (OSError,KeyError,TypeError,ValueError,AttributeError,json.JSONDecodeError):
        run_manifest={}; ref_manifest={}
    if not isinstance(run_manifest,dict): run_manifest={}
    if not isinstance(ref_manifest,dict): ref_manifest={}
    ledgers=meta.get("timing_ledgers",{})
    timing_ok=False; timing_data={}
    try:
        timing_ok=(meta.get("clock_kind")=="monotonic_wall" and meta.get("same_input_fingerprint") is True and isinstance(ledgers,dict) and all(x in ledgers for x in ("candidate","baseline")) and _sha256(candidate_path)==meta.get("candidate_sha256") and _sha256(baseline_path)==meta.get("baseline_sha256"))
        for system in ("candidate","baseline"):
            data=_load_timing_ledger(ledgers[system],meta.get("input_fingerprint"))
            if data is None: timing_ok=False
            else: timing_data[system]=data
    except (OSError,EvaluationError,TypeError,KeyError,ValueError,AttributeError,json.JSONDecodeError):
        timing_ok=False
    identity_clean=(false_identity==0 and cand_duplicates==0 and base_duplicates==0)
    ref_bound=(ref_manifest.get("reference_kind")==meta.get("reference_kind") and ref_manifest.get("truth_sha256")==meta.get("truth_sha256") and ref_manifest.get("measurements_sha256")==meta.get("measurements_sha256") and ref_manifest.get("protocol_sha256")==meta.get("protocol_sha256"))
    run_bound=(run_manifest.get("truth_sha256")==meta.get("truth_sha256") and run_manifest.get("measurements_sha256")==meta.get("measurements_sha256"))
    raw_manifest_ref=next((scoring_manifest.get(k) for k in ("raw_manifest_sha256","manifest_sha256","runner_manifest_sha256") if scoring_manifest.get(k)),None)
    comparison_bound=(scoring_manifest.get("truth_sha256")==meta.get("truth_sha256") and scoring_manifest.get("measurements_sha256")==meta.get("measurements_sha256") and raw_manifest_ref==meta.get("manifest_sha256") and scoring_manifest.get("initialization_ns")==init_ns and scoring_manifest.get("coast_max_age_ns")==coast_limit_ns)
    accuracy_eligible=(not missing_meta and _claim_class(meta.get("comparison_class")) and _known_comparison_class(meta.get("comparison_class")) and meta.get("reference_qualified") is True and meta.get("reference_validation_status")=="PASS" and ref_manifest.get("validation_status")=="PASS" and ref_bound and run_bound and comparison_bound and meta.get("reference_kind") in ("independent_simulation","independent_physical") and meta.get("same_input_fingerprint") is True and meta.get("input_fingerprint")==meta.get("measurements_sha256") and artifact_integrity and flat_integrity and protocol_integrity and profile_binding_valid and meta.get("identity_source")=="scenario_manifest_not_candidate_output" and identity_valid and _zero_losses(run_manifest.get("loss_counters")) and identity_clean and profile_gate["status"]=="nominal_profile_pass")
    ledger_c=timing_data.get("candidate",{}); ledger_b=timing_data.get("baseline",{})
    throughput_c=_ledger_throughput(ledger_c) or 0
    throughput_b=_ledger_throughput(ledger_b) or 0
    speed_eligible=False
    accuracy_claim=accuracy_eligible and rms_ratio is not None and p99_ratio is not None and rms_ratio<=.70 and p99_ratio<=.80 and paired_rms["high95"] is not None and paired_rms["high95"]<=.70 and paired_p99["high95"] is not None and paired_p99["high95"]<=.80
    speed_claim=speed_eligible and latency_ratio is not None and latency_ratio<=.50 and throughput_c>=2*throughput_b
    if profile_gate["status"]!="nominal_profile_pass": accuracy_status=profile_gate["status"]
    elif by_system["candidate"]["post_initialization_position_error_m"]["count"]==0 or by_system["baseline"]["post_initialization_position_error_m"]["count"]==0: accuracy_status="no_post_initialization_samples"
    elif rms_ratio is None or p99_ratio is None: accuracy_status="unqualified_near_zero_baseline"
    elif not accuracy_eligible: accuracy_status="disabled_unqualified_comparison"
    else: accuracy_status="qualified_pass" if accuracy_claim else "qualified_not_met"
    return {
        "schema":"openatca-comparison-v1",
        "thresholds":{"rms_ratio_max":.70,"p99_error_ratio_max":.80,"p99_receive_to_usable_ratio_max":.50,"throughput_ratio_min":2.0,"zero_loss_required":True},
        "logical_metrics":by_system,
        "profile_qualification":{**profile_gate,"usable_keysets_match":same_usable_keys},
        "paired_grid":paired_grid,
        "paired_confidence":{"rms_ratio":paired_rms,"p99_error_ratio":paired_p99},
        "ratios":{"rms":rms_ratio,"p99_error":p99_ratio,"p99_receive_to_usable":latency_ratio},
        "accuracy_qualification":{"scope":"kinematic_component","status":accuracy_status,"enabled":accuracy_eligible,"pass":bool(accuracy_claim),"all_quality_pass":False,"all_quality_status":"not_qualified_v1_scope"},
        "speed_coverage":{"status":"not_qualified_missing_first_use_event_ledger","enabled":speed_eligible,"pass":bool(speed_claim),"throughput_ratio":throughput_c/throughput_b if throughput_b>0 else None,"reason":"first_use_event_timing_not_available_in_v1"},
        "qualification_evidence":{"artifacts":artifact_checks,"protocol_matches_canonical":protocol_integrity,"comparison_class_recognized":_known_comparison_class(meta.get("comparison_class")),"profile_manifest_binding_valid":profile_binding_valid,"derived_profile":derived_profile,"identity_binding_valid":identity_valid,"identity_scope":identity_scope,"uncovered_metrics":["anonymous_track_identity","first_causal_event_use_latency","full_quality_competitive_pass"]},
        "accounting":{"truth_grid_rows":len(truth_by),"measurement_events":len(meas_events),"candidate_output_rows":len(candidate),"baseline_output_rows":len(baseline),"candidate_missing_rows":by_system["candidate"]["missing"],"baseline_missing_rows":by_system["baseline"]["missing"],"candidate_duplicate_rows":cand_duplicates,"baseline_duplicate_rows":base_duplicates,"false_identity_rows":false_identity}
    }

def main(argv=None):
    p=argparse.ArgumentParser(description=__doc__)
    for arg in ("truth","measurements","candidate","baseline"): p.add_argument("--"+arg,required=True)
    p.add_argument("--metadata",required=True); p.add_argument("--out",required=True)
    a=p.parse_args(argv)
    try:
        meta=_read_json(a.metadata)
        result=evaluate(a.truth,a.measurements,a.candidate,a.baseline,meta)
        with open(a.out,"w",encoding="utf-8") as f: json.dump(result,f,indent=2,sort_keys=True); f.write("\n")
    except (EvaluationError,OSError,ValueError,json.JSONDecodeError) as exc:
        print(f"comparison evaluator: {exc}",file=sys.stderr); return 2
    return 0
if __name__=="__main__": raise SystemExit(main())
