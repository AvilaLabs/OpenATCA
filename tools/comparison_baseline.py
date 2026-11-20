#!/usr/bin/env python3
"""Independent constant-velocity Kalman baseline for shared CSV v1.

Development accuracy control only. This Python implementation cannot qualify Rust speed.
"""
import argparse
import csv
import json
import math

from comparison_evaluator import EvaluationError, _csv, _num, _key, _id_order, _sha256, WGS84_A_M, WGS84_F, EXPECTED

def _vincenty_inverse(lat1,lon1,lat2,lon2):
    """WGS84 geodesic inverse returning distance and forward azimuths."""
    a=WGS84_A_M; f=WGS84_F; b=a*(1-f)
    phi1,phi2=math.radians(lat1),math.radians(lat2)
    u1=math.atan((1-f)*math.tan(phi1)); u2=math.atan((1-f)*math.tan(phi2))
    L=(math.radians(lon2-lon1)+math.pi)%(2*math.pi)-math.pi; lam=L
    for _ in range(100):
        sl,cl=math.sin(lam),math.cos(lam)
        ss=math.hypot(math.cos(u2)*sl,math.cos(u1)*math.sin(u2)-math.sin(u1)*math.cos(u2)*cl)
        if ss==0: return 0.0,0.0,0.0
        cs=math.sin(u1)*math.sin(u2)+math.cos(u1)*math.cos(u2)*cl; sigma=math.atan2(ss,cs)
        sa=math.cos(u1)*math.cos(u2)*sl/ss; c2a=1-sa*sa
        c2sm=0.0 if c2a<1e-15 else cs-2*math.sin(u1)*math.sin(u2)/c2a
        C=f/16*c2a*(4+f*(4-3*c2a))
        nxt=L+(1-C)*f*sa*(sigma+C*ss*(c2sm+C*cs*(-1+2*c2sm*c2sm)))
        if abs(nxt-lam)<1e-13: lam=nxt; break
        lam=nxt
    else: raise EvaluationError("WGS84 geodesic inverse did not converge")
    u2sq=c2a*(a*a-b*b)/(b*b); A=1+u2sq/16384*(4096+u2sq*(-768+u2sq*(320-175*u2sq)))
    B=u2sq/1024*(256+u2sq*(-128+u2sq*(74-47*u2sq)))
    ds=B*ss*(c2sm+B/4*(cs*(-1+2*c2sm*c2sm)-B/6*c2sm*(-3+4*ss*ss)*(-3+4*c2sm*c2sm)))
    dist=b*A*(sigma-ds)
    az1=math.atan2(math.cos(u2)*math.sin(lam),math.cos(u1)*math.sin(u2)-math.sin(u1)*math.cos(u2)*math.cos(lam))
    az2=math.atan2(math.cos(u1)*math.sin(lam),-math.sin(u1)*math.cos(u2)+math.cos(u1)*math.sin(u2)*math.cos(lam))
    return dist,az1,az2

def _vincenty_direct(lat1,lon1,az1,distance):
    """WGS84 geodesic direct returning endpoint and final forward azimuth."""
    a=WGS84_A_M; f=WGS84_F; b=a*(1-f); phi1=math.radians(lat1)
    sa1,ca1=math.sin(az1),math.cos(az1); tu1=(1-f)*math.tan(phi1)
    cu1=1/math.sqrt(1+tu1*tu1); su1=tu1*cu1; sigma1=math.atan2(tu1,ca1)
    salpha=cu1*sa1; c2a=1-salpha*salpha; u2sq=c2a*(a*a-b*b)/(b*b)
    A=1+u2sq/16384*(4096+u2sq*(-768+u2sq*(320-175*u2sq)))
    B=u2sq/1024*(256+u2sq*(-128+u2sq*(74-47*u2sq)))
    sigma=distance/(b*A)
    for _ in range(100):
        c2sm=math.cos(2*sigma1+sigma); ss=math.sin(sigma); cs=math.cos(sigma)
        ds=B*ss*(c2sm+B/4*(cs*(-1+2*c2sm*c2sm)-B/6*c2sm*(-3+4*ss*ss)*(-3+4*c2sm*c2sm)))
        nxt=distance/(b*A)+ds
        if abs(nxt-sigma)<1e-13: sigma=nxt; break
        sigma=nxt
    ss,cs=math.sin(sigma),math.cos(sigma); c2sm=math.cos(2*sigma1+sigma)
    tmp=su1*ss-cu1*cs*ca1
    phi2=math.atan2(su1*cs+cu1*ss*ca1,(1-f)*math.hypot(salpha,tmp))
    lam=math.atan2(ss*sa1,cu1*cs-su1*ss*ca1)
    C=f/16*c2a*(4+f*(4-3*c2a))
    L=lam-(1-C)*f*salpha*(sigma+C*ss*(c2sm+C*cs*(-1+2*c2sm*c2sm)))
    lon2=math.radians(lon1)+L; az2=math.atan2(salpha,-tmp)
    return math.degrees(phi2),((math.degrees(lon2)+180)%360)-180,az2

def _inverse2(a,b,c):
    det=a*c-b*b
    if det <= 0 or not math.isfinite(det): raise EvaluationError("baseline covariance is not positive definite")
    return c/det,-b/det,a/det

def run(truth_path, measurements_path, output_path, process_noise=1.0, grid_path=None):
    truth=_csv(grid_path,"grid") if grid_path else _csv(truth_path,"truth")
    measurements=_csv(measurements_path,"measurements")
    event_ids=set()
    for i,row in enumerate(measurements,2):
        event_id=row["event_id"]
        if not event_id.strip() or event_id in event_ids:
            raise EvaluationError(f"measurements:{i}: duplicate/empty event_id")
        event_ids.add(event_id)
    if process_noise < 0 or not math.isfinite(process_noise): raise EvaluationError("process_noise must be finite and nonnegative")
    grid={}
    for i,r in enumerate(truth,2):
        k=(*_key(r),_num(r,"time_ns",f"truth:{i}",True))
        if k in grid: raise EvaluationError(f"duplicate truth grid key {k}")
        grid[k]=r
    byflight={}
    for i,r in enumerate(measurements,2):
        byflight.setdefault((r["scenario_id"],r["flight_id"]),[]).append(r)
    outputs=[]; counters={"measurements_used":0,"late_sensor_time_refused":0,"invalid_coordinate_refused":0,"not_received_by_grid_end":0,"future_source_pending_at_grid_end":0}
    for flight in sorted({k[:2] for k in grid}):
        events=sorted(byflight.get(flight,[]),key=lambda r:(_num(r,"receive_ns","measurement",True),_id_order(r["event_id"])))
        # State [east,north,ve,vn], diagonal covariance. Measurement covariance from declared sigma.
        state=None; cov=[[1e8,0.0,1e6],[1e8,0.0,1e6]]; last_ns=None; cursor=0; pending=[]; applied=[]; confirmation_event_ids=[]
        for key in sorted(k for k in grid if k[:2]==flight):
            scenario,flight_id,t_ns=key
            while cursor<len(events) and _num(events[cursor],"receive_ns","measurement",True)<=t_ns:
                pending.append(events[cursor]); cursor+=1
            ready=[m for m in pending if _num(m,"sensor_ns","measurement",True)<=t_ns]
            pending=[m for m in pending if _num(m,"sensor_ns","measurement",True)>t_ns]
            ready.sort(key=lambda r:(_num(r,"sensor_ns","measurement",True),_num(r,"receive_ns","measurement",True),_id_order(r["event_id"])))
            for m in ready:
                m_ns=_num(m,"sensor_ns","measurement",True)
                if last_ns is not None and m_ns<last_ns:
                    counters["late_sensor_time_refused"]+=1; continue
                lat=_num(m,"lat_deg","measurement"); lon=_num(m,"lon_deg","measurement")
                if abs(lat)>90 or abs(lon)>180:
                    counters["invalid_coordinate_refused"]+=1; continue
                applied.append(m); counters["measurements_used"]+=1
                confirmation_event_ids.append(m["event_id"])
                confirmation_event_ids=confirmation_event_ids[-3:]
                # Local tangent origin is the first accepted measurement; no truth enters estimator.
                if state is None:
                    state=[0.0,0.0,_num(m,"ve_mps","measurement"),_num(m,"vn_mps","measurement")]
                    origin=(lat,lon); state[0]=state[1]=0.0; last_ns=m_ns
                dt=max(0.0,(m_ns-last_ns)/1e9); last_ns=max(last_ns,m_ns)
                state[0]+=state[2]*dt; state[1]+=state[3]*dt
                for axis,(p00,p01,p11) in enumerate(cov):
                    cov[axis]=[p00+2*dt*p01+dt*dt*p11+process_noise*dt**3/3,
                               p01+dt*p11+process_noise*dt**2/2,
                               p11+process_noise*dt]
                # Measurement conversion to local ENU.
                distance,az1,az2=_vincenty_inverse(origin[0],origin[1],lat,lon)
                ve=_num(m,"ve_mps","measurement"); vn=_num(m,"vn_mps","measurement")
                speed=math.hypot(ve,vn); local_az=math.atan2(ve,vn); rotation=az2-az1; origin_az=local_az-rotation
                z=[distance*math.sin(az1),distance*math.cos(az1),speed*math.sin(origin_az),speed*math.cos(origin_az)]
                sp=max(0.01,_num(m,"sigma_pos_m","measurement")); sv=max(0.01,_num(m,"sigma_vel_mps","measurement"))
                for axis,(pos_i,vel_i,zpos,zvel) in enumerate(((0,2,z[0],z[2]),(1,3,z[1],z[3]))):
                    p00,p01,p11=cov[axis]
                    innovation=zpos-state[pos_i]; s=p00+sp*sp
                    k0=p00/s; k1=p01/s
                    state[pos_i]+=k0*innovation; state[vel_i]+=k1*innovation
                    p00,p01,p11=p00-p00*k0,p01-p00*k1,p11-p01*k1
                    innovation=zvel-state[vel_i]; s=p11+sv*sv
                    k0=p01/s; k1=p11/s
                    state[pos_i]+=k0*innovation; state[vel_i]+=k1*innovation
                    cov[axis]=[p00-k0*p01,p01-k0*p11,p11-k1*p11]
            tr=grid[key]
            if state is None or len(confirmation_event_ids)<2:
                outputs.append(dict(scenario_id=scenario,flight_id=flight_id,score_ns=t_ns,source_ns="",publish_ns=t_ns,lat_deg="",lon_deg="",ve_mps="",vn_mps="",status="missing")); continue
            dt=max(0.0,(t_ns-last_ns)/1e9); pred=[state[0]+state[2]*dt,state[1]+state[3]*dt,state[2],state[3]]
            distance=math.hypot(pred[0],pred[1]); az1=math.atan2(pred[0],pred[1])
            lat,lon,az2=_vincenty_direct(origin[0],origin[1],az1,distance)
            rotation=az2-az1; speed=math.hypot(pred[2],pred[3]); origin_az=math.atan2(pred[2],pred[3]); local_az=origin_az+rotation
            ve=speed*math.sin(local_az); vn=speed*math.cos(local_az)
            # Input/phase source timestamps are measurement-derived, never truth-derived.
            source=max((_num(m,"sensor_ns","measurement",True) for m in applied),default=t_ns)
            status="coast" if t_ns-source>2_000_000_000 else "valid"
            outputs.append(dict(scenario_id=scenario,flight_id=flight_id,score_ns=t_ns,source_ns=source,publish_ns=t_ns,lat_deg=lat,lon_deg=lon,ve_mps=ve,vn_mps=vn,status=status))
        counters["not_received_by_grid_end"]+=len(events)-cursor
        counters["future_source_pending_at_grid_end"]+=len(pending)
    with open(output_path,"w",newline="",encoding="utf-8") as f:
        w=csv.DictWriter(f,fieldnames=EXPECTED["output"]); w.writeheader(); w.writerows(outputs)
    grid_flights={k[:2] for k in grid}
    counters["unknown_flight_measurements"] = sum(len(v) for f,v in byflight.items() if f not in grid_flights)
    return {"baseline":"cv-kalman-v1","comparison_class":"development_control","reference_qualified":False,"grid_source_sha256":_sha256(grid_path or truth_path),"measurements_sha256":_sha256(measurements_path),"state_order":["east_m","north_m","ve_mps","vn_mps"],"initial_position_variance_m2":1e8,"initial_velocity_variance_m2ps2":1e6,"process_noise_m2ps3":process_noise,"measurement_sigma_floor":0.01,"confirmation_profile":{"hits":2,"window_reports":3},"measurement_rows":len(measurements),"output_rows":len(outputs),"accounting":counters}

def main():
    p=argparse.ArgumentParser(description=__doc__); source=p.add_mutually_exclusive_group(required=True); source.add_argument("--truth"); source.add_argument("--grid"); p.add_argument("--measurements",required=True); p.add_argument("--out",required=True); p.add_argument("--manifest"); p.add_argument("--process-noise",type=float,default=1.0)
    a=p.parse_args(); manifest=run(a.truth,a.measurements,a.out,process_noise=a.process_noise,grid_path=a.grid)
    if a.manifest:
        with open(a.manifest,"w",encoding="utf-8") as f: json.dump(manifest,f,indent=2,sort_keys=True); f.write("\n")
if __name__=="__main__": main()
