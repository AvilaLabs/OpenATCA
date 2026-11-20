#![forbid(unsafe_code)]
//! Offline selected-address trace. Tracker inputs are set-A measurements only; truth queries are
//! made after each output row has been produced and are diagnostic labels, never tracker inputs.
//! Shell (B3): files, arguments and CSV output. All aircraft drive the unchanged tracker.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use oa_adsb::{decode_payload_lenient, reports_len, reports_row, Reports};
use oa_bench::adsb_truth;
use oa_bench::geo::{enu, heading_deg, wrap180, KT_MPS};
use oa_bench::input::{for_each_adsb_frame, load_adsb};
use oa_track::adsb::{adsb_measurements, AdsbConfig};
use oa_track::score::{
    interval_stats, scoring_config, split_reports, IntervalStats, DRAIN_ROWS, PER_FLIGHT_MIN_N,
    R4_PER_FLIGHT_M,
};
use oa_track::{
    output_clear, output_len, output_new, tracker_advance, tracker_flush, tracker_new,
    tracker_stats, tracker_update, TrackOutput, Tracker, DEFAULT_CONFIG,
};
use oa_truth::DEFAULT_CONFIG as TRUTH_DEFAULT;

// A7.2: cold command-line configuration, size guard waived.
struct Args {
    clip: PathBuf,
    out: PathBuf,
    addresses: BTreeSet<u32>,
    start_ns: Option<i64>,
    end_ns: Option<i64>,
}

fn parse_address(s: &str) -> Result<u32, String> {
    let value = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X"));
    match value {
        Some(v) => u32::from_str_radix(v, 16).map_err(|e| format!("bad address {s}: {e}")),
        None => s
            .parse::<u32>()
            .map_err(|e| format!("bad address {s}: {e}")),
    }
}

fn parse_args() -> Result<Args, String> {
    let mut clip = None;
    let mut out = None;
    let mut addresses = BTreeSet::new();
    let mut start_ns = None;
    let mut end_ns = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let value = |args: &mut std::iter::Skip<std::env::Args>, key: &str| {
            args.next().ok_or_else(|| format!("{key} requires a value"))
        };
        match arg.as_str() {
            "--adsb" => clip = Some(PathBuf::from(value(&mut args, "--adsb")?)),
            "--out" => out = Some(PathBuf::from(value(&mut args, "--out")?)),
            "--addr" => {
                addresses.insert(parse_address(&value(&mut args, "--addr")?)?);
            }
            "--start-ns" => {
                start_ns = Some(
                    value(&mut args, "--start-ns")?
                        .parse::<i64>()
                        .map_err(|e| format!("bad --start-ns: {e}"))?,
                );
            }
            "--end-ns" => {
                end_ns = Some(
                    value(&mut args, "--end-ns")?
                        .parse::<i64>()
                        .map_err(|e| format!("bad --end-ns: {e}"))?,
                );
            }
            _ => return Err(format!("unknown argument {arg}")),
        }
    }
    let (Some(clip), Some(out)) = (clip, out) else {
        return Err("usage: prediction_trace --adsb <clip.oaf> --out <dir> --addr <u32> [--addr <u32> ...] [--start-ns <ns> --end-ns <ns>]".into());
    };
    if addresses.is_empty() {
        return Err("at least one --addr is required".into());
    }
    if start_ns.is_some() != end_ns.is_some() {
        return Err("--start-ns and --end-ns must be supplied together".into());
    }
    if start_ns.zip(end_ns).is_some_and(|(a, b)| a > b) {
        return Err("--start-ns must be <= --end-ns".into());
    }
    Ok(Args {
        clip,
        out,
        addresses,
        start_ns,
        end_ns,
    })
}

fn f32_cell(x: f32) -> String {
    if x.is_finite() {
        format!("{x:.7}")
    } else {
        String::new()
    }
}

fn f64_cell(x: f64) -> String {
    if x.is_finite() {
        format!("{x:.7}")
    } else {
        String::new()
    }
}

fn vector3(v: &[f32; 3]) -> [String; 3] {
    let [a, b, c] = v;
    [f32_cell(*a), f32_cell(*b), f32_cell(*c)]
}

fn in_window(t_ns: i64, start: Option<i64>, end: Option<i64>) -> bool {
    start.is_none_or(|v| t_ns >= v) && end.is_none_or(|v| t_ns <= v)
}

type ReceiveKey = (u32, i64, [i64; 11]);
type GroupKey = (u32, i64);

fn receive_key(row: &oa_adsb::Row) -> ReceiveKey {
    (
        row.icao,
        row.time_ns,
        [
            i64::from(row.lat_e7),
            i64::from(row.lon_e7),
            i64::from(row.baro_alt_ft),
            i64::from(row.geom_alt_ft),
            i64::from(row.vrate_fpm),
            i64::from(row.gs_dkt),
            i64::from(row.track_ddeg),
            i64::from(row.squawk),
            i64::from(row.flags),
            i64::from(row.source),
            i64::from(row.category),
        ],
    )
}

/// The receive times of the frames that carry the selected addresses' rows. The map is only ever
/// looked up by the keys of rows already read (and filtered of the sealed window) by the clip
/// reader, so a row it holds that the reader dropped is never used.
fn receive_times(
    path: &Path,
    selected: &BTreeSet<u32>,
) -> Result<BTreeMap<ReceiveKey, Vec<i64>>, String> {
    let mut map: BTreeMap<ReceiveKey, Vec<i64>> = BTreeMap::new();
    let mut error = None;
    for_each_adsb_frame(path, &mut |recv_ns, payload| {
        if error.is_some() {
            return;
        }
        let mut reports = Reports::default();
        if decode_payload_lenient(payload, &mut reports).is_err() {
            return;
        }
        for i in 0..reports_len(&reports) {
            let Some(row) = reports_row(&reports, i) else {
                error = Some(format!("decoded frame report {i} has incomplete columns"));
                return;
            };
            if !selected.contains(&row.icao) {
                continue;
            }
            map.entry(receive_key(&row)).or_default().push(recv_ns);
        }
    })
    .map_err(|e| e.to_string())?;
    if let Some(e) = error {
        return Err(e);
    }
    Ok(map)
}

fn output_columns_len(o: &TrackOutput) -> Result<usize, String> {
    let n = output_len(o);
    for (name, len) in [
        ("addr", o.addr.len()),
        ("lat_e7", o.lat_e7.len()),
        ("lon_e7", o.lon_e7.len()),
        ("vel_east_mps", o.vel_east_mps.len()),
        ("vel_north_mps", o.vel_north_mps.len()),
        ("pos_cov_m2", o.pos_cov_m2.len()),
        ("vel_cov_m2s2", o.vel_cov_m2s2.len()),
        ("turn_rate_rps", o.turn_rate_rps.len()),
        ("alt_ft", o.alt_ft.len()),
        ("alt_var_ft2", o.alt_var_ft2.len()),
        ("vrate_fpm", o.vrate_fpm.len()),
        ("mu_h", o.mu_h.len()),
        ("mu_v", o.mu_v.len()),
        ("flags", o.flags.len()),
    ] {
        if len != n {
            return Err(format!(
                "TrackOutput column {name} has {len} rows; expected {n}"
            ));
        }
    }
    Ok(n)
}

fn output_get<T: Copy>(name: &str, column: &[T], i: usize) -> Result<T, String> {
    column
        .get(i)
        .copied()
        .ok_or_else(|| format!("TrackOutput {name} missing at row {i}"))
}

fn csv_record(
    writer: &mut BufWriter<File>,
    fields: Vec<String>,
    expected: usize,
    label: &str,
) -> Result<(), String> {
    if fields.len() != expected {
        return Err(format!(
            "{label} CSV row has {} fields; expected {expected}",
            fields.len()
        ));
    }
    writeln!(writer, "{}", fields.join(",")).map_err(|e| e.to_string())
}

fn write_truth_and_error(
    fields: &mut Vec<String>,
    truth: &oa_truth::Truth,
    addr: u32,
    t_ns: i64,
    position_e7: [i32; 2],
    velocity_mps: [f32; 2],
) {
    let [track_lat_e7, track_lon_e7] = position_e7;
    let [track_east_mps, track_north_mps] = velocity_mps;
    let t_ms = t_ns.div_euclid(1_000_000);
    match adsb_truth::query(truth, addr, t_ms) {
        Ok(s) => {
            fields.extend([
                "ok".into(),
                s.leg.to_string(),
                f64_cell(s.sigma95_h_m),
                f64_cell(s.lat),
                f64_cell(s.lon),
                f64_cell(s.vel_east_mps),
                f64_cell(s.vel_north_mps),
                f64_cell(heading_deg(s.vel_east_mps, s.vel_north_mps)),
                String::new(),
            ]);
            let (e, n) = enu(
                s.lat,
                s.lon,
                f64::from(track_lat_e7) * 1e-7,
                f64::from(track_lon_e7) * 1e-7,
            );
            let speed = s.vel_east_mps.hypot(s.vel_north_mps);
            if e.is_finite() && n.is_finite() && speed >= 10.0 {
                let (ue, un) = (s.vel_east_mps / speed, s.vel_north_mps / speed);
                let along = (e * ue + n * un) as f32;
                let cross = (e * un - n * ue) as f32;
                fields.extend([
                    f64_cell(f64::from(cross)),
                    f64_cell(f64::from(along)),
                    f64_cell(f64::from(cross).hypot(f64::from(along))),
                ]);
            } else {
                fields.extend([String::new(), String::new(), String::new()]);
            }
            let output_speed = f64::from(track_east_mps).hypot(f64::from(track_north_mps));
            let truth_speed = s.vel_east_mps.hypot(s.vel_north_mps);
            let speed_error = output_speed - truth_speed;
            let heading_error = if output_speed >= 30.0 * KT_MPS && truth_speed >= 30.0 * KT_MPS {
                f64_cell(wrap180(
                    heading_deg(f64::from(track_east_mps), f64::from(track_north_mps))
                        - heading_deg(s.vel_east_mps, s.vel_north_mps),
                ))
            } else {
                String::new()
            };
            fields.extend([f64_cell(speed_error), heading_error]);
        }
        Err(e) => fields.extend([
            "missing".into(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            format!("{:?}", e),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
        ]),
    }
}

pub(crate) fn write_output_rows(
    o: &TrackOutput,
    writer: &mut BufWriter<File>,
    truth: &oa_truth::Truth,
    selected: &BTreeSet<u32>,
    start: Option<i64>,
    end: Option<i64>,
) -> Result<(), String> {
    let n = output_columns_len(o)?;
    for i in 0..n {
        let t = output_get("time", &o.t_ns, i)?;
        let addr = output_get("address", &o.addr, i)?;
        if !selected.contains(&addr) || !in_window(t, start, end) {
            continue;
        }
        let lat = output_get("latitude", &o.lat_e7, i)?;
        let lon = output_get("longitude", &o.lon_e7, i)?;
        let ve = output_get("east velocity", &o.vel_east_mps, i)?;
        let vn = output_get("north velocity", &o.vel_north_mps, i)?;
        let pc = output_get("position covariance", &o.pos_cov_m2, i)?;
        let vc = output_get("velocity covariance", &o.vel_cov_m2s2, i)?;
        let turn = output_get("turn rate", &o.turn_rate_rps, i)?;
        let mu_h = output_get("horizontal mode probabilities", &o.mu_h, i)?;
        let mu_v = output_get("vertical mode probabilities", &o.mu_v, i)?;
        let [mu_h0, mu_h1] = mu_h;
        let [mu_v0, mu_v1] = mu_v;
        let flags = output_get("flags", &o.flags, i)?;
        let mut fields = vec![
            if flags & oa_track::O_TICK != 0 {
                "tick"
            } else {
                "update"
            }
            .to_string(),
            t.to_string(),
            format!("0x{addr:08X}"),
            lat.to_string(),
            lon.to_string(),
            f32_cell(ve),
            f32_cell(vn),
            f32_cell(turn),
        ];
        fields.extend(vector3(&pc));
        fields.extend(vector3(&vc));
        fields.extend([
            f32_cell(mu_h0),
            f32_cell(mu_h1),
            f32_cell(mu_v0),
            f32_cell(mu_v1),
            flags.to_string(),
        ]);
        write_truth_and_error(&mut fields, truth, addr, t, [lat, lon], [ve, vn]);
        fields.extend(std::iter::once("context_unknown".to_string()));
        fields.extend(vec![String::new(); 16]);
        csv_record(writer, fields, OUTPUT_HEADER.split(',').count(), "outputs")?;
    }
    Ok(())
}

pub(crate) fn stats_csv(t: &Tracker) -> String {
    let s = tracker_stats(t);
    let mut o = String::from("name,value\n");
    macro_rules! field {
        ($n:literal, $v:expr) => {
            let _ = writeln!(o, "{},{}", $n, $v);
        };
    }
    field!("offered", s.offered);
    field!("applied", s.applied);
    for (name, value) in oa_track::REFUSE_NAMES.iter().zip(s.refused.iter()) {
        let _ = writeln!(o, "refused_{name},{}", value);
    }
    field!("duplicates", s.duplicates);
    field!("late_window", s.late_window);
    field!("late_ring", s.late_ring);
    field!("refiltered", s.refiltered);
    field!("replayed", s.replayed);
    field!("jump_rejected", s.jump_rejected);
    field!("numerical_refused", s.numerical_refused);
    field!("resyncs", s.resyncs);
    field!("alt_rejected", s.alt_rejected);
    field!("alt_geometric_skipped", s.alt_geometric_skipped);
    field!("capacity_refused", s.capacity_refused);
    field!("started", s.started);
    field!("confirmed", s.confirmed);
    for (name, value) in oa_track::END_NAMES.iter().zip(s.ended.iter()) {
        let _ = writeln!(o, "ended_{name},{}", value);
    }
    field!("init_implausible", s.init_implausible);
    field!("init_window_expired", s.init_window_expired);
    field!("ticks", s.ticks);
    field!("ticks_skipped", s.ticks_skipped);
    field!("rows_computed", s.rows_computed);
    field!("rows_unrepresentable", s.rows_unrepresentable);
    field!("time_held", s.time_held);
    field!("time_steps", s.time_steps);
    field!("time_held_applied", s.time_held_applied);
    field!("time_steps_back", s.time_steps_back);
    field!("time_future_dropped", s.time_future_dropped);
    field!("time_sensor_evicted", s.time_sensor_evicted);
    field!("nis_sum", s.nis_sum);
    field!("nis_n", s.nis_n);
    o
}

const INPUT_HEADER: &str = "address_hex,toa_ns,receive_ns,receive_status,source_match_status,tracker_status,applied_delta,held_delta,held_applied_delta,refiltered_delta,refusal_delta,jump_rejected_delta,duplicate_delta,late_window_delta,late_ring_delta,lat_e7,lon_e7,gs_dkt,track_ddeg,vel_east_mps,vel_north_mps,pos_cov_ee,pos_cov_en,pos_cov_nn,vel_cov_ee,vel_cov_en,vel_cov_nn,sensor";
const RAW_HEADER: &str = "address_hex,toa_ns,receive_ns,receive_status,adapter_status,source_group_count,measurement_group_count,lat_e7,lon_e7,baro_alt_ft,geom_alt_ft,gs_dkt,track_ddeg,nac_p,nic,sil,version,flags,source,roll_cdeg,track_rate_cdps";
pub(crate) const OUTPUT_HEADER: &str = "kind,t_ns,address_hex,lat_e7,lon_e7,vel_east_mps,vel_north_mps,turn_rate_rps,pos_cov_ee,pos_cov_en,pos_cov_nn,vel_cov_ee,vel_cov_en,vel_cov_nn,mu_h_straight,mu_h_turn,mu_v_level,mu_v_vertical,flags,truth_status,truth_leg,sigma95_h_m,truth_lat,truth_lon,truth_vel_east,truth_vel_north,truth_heading_deg,truth_reason,error_cross_m,error_along_m,error_horizontal_m,diagnostic_speed_delta_mps,diagnostic_heading_delta_deg,latest_input_status,latest_input_toa_ns,latest_input_receive_ns,data_age_ms,receive_minus_toa_ms,sensor,input_address_hex,input_lat_e7,input_lon_e7,input_vel_east,input_vel_north,input_pos_cov_ee,input_pos_cov_en,input_pos_cov_nn,input_vel_cov_ee,input_vel_cov_en,input_vel_cov_nn";

fn run(args: Args) -> Result<(), String> {
    if args.out.exists() {
        // seal-allow: the output directory; only its emptiness is read
        let mut entries = fs::read_dir(&args.out).map_err(|e| e.to_string())?;
        if entries.next().is_some() {
            return Err(format!(
                "output directory {} must be empty",
                args.out.display()
            ));
        }
    } else {
        fs::create_dir_all(&args.out).map_err(|e| format!("{}: {e}", args.out.display()))?;
    }
    let clip = load_adsb(&args.clip).map_err(|e| e.to_string())?;
    let clip_sealed = clip.sealed_window_dropped;
    let full_reports = reports_len(&clip.reports);
    let clip_hash = clip.hash.sha256.clone();
    let (set_a, set_b, split) = split_reports(&clip.reports);
    drop(clip.reports);
    // Match the full scorer population and exact input ordering before diagnostics are filtered.
    let (all_measurements, adapter_stats) = adsb_measurements(&set_a, &AdsbConfig::default());
    let interval = interval_stats(&all_measurements).unwrap_or(IntervalStats {
        n: 0,
        p50_ns: 1_000_000_000,
        p90_ns: 1_000_000_000,
        p99_ns: 1_000_000_000,
    });
    let config = scoring_config(&DEFAULT_CONFIG, &interval);
    let built = adsb_truth::build(&set_b, &TRUTH_DEFAULT).map_err(|e| e.to_string())?;
    let truth_n = oa_truth::leg_count(&built.truth);
    let received = receive_times(&args.clip, &args.addresses)?;
    // After the last read of the clip (receive_times reads it again and drops the same frames,
    // which are not counted twice).
    eprintln!("{}", oa_sealed::drop_line(clip_sealed));
    let mut raw_counts: BTreeMap<GroupKey, u64> = BTreeMap::new();
    let mut measurement_counts: BTreeMap<GroupKey, u64> = BTreeMap::new();
    let mut raw_rows = Vec::new();
    for i in 0..reports_len(&set_a) {
        let row = reports_row(&set_a, i)
            .ok_or_else(|| format!("set-A report {i} has incomplete columns"))?;
        if !args.addresses.contains(&row.icao) {
            continue;
        }
        *raw_counts.entry((row.icao, row.time_ns)).or_default() += 1;
        raw_rows.push(row);
    }
    drop(set_a);
    drop(set_b);
    let mut unique_raw: BTreeMap<GroupKey, oa_adsb::Row> = BTreeMap::new();
    for row in &raw_rows {
        let key = (row.icao, row.time_ns);
        if raw_counts.get(&key) == Some(&1) {
            unique_raw.insert(key, *row);
        }
    }
    for m in &all_measurements {
        if args.addresses.contains(&m.addr) {
            *measurement_counts.entry((m.addr, m.toa_ns)).or_default() += 1;
        }
    }
    let selected_count = measurement_counts.values().sum::<u64>();
    let mut tracker = tracker_new(&config).map_err(|e| e.to_string())?;
    let mut out = output_new(config.out_capacity);
    let inputs_file = File::create(args.out.join("inputs.csv")).map_err(|e| e.to_string())?;
    let mut inputs = BufWriter::new(inputs_file);
    writeln!(inputs, "{INPUT_HEADER}").map_err(|e| e.to_string())?;
    let raw_file = File::create(args.out.join("raw-inputs.csv")).map_err(|e| e.to_string())?;
    let mut raw_out = BufWriter::new(raw_file);
    writeln!(raw_out, "{RAW_HEADER}").map_err(|e| e.to_string())?;
    let outputs_file = File::create(args.out.join("outputs.csv")).map_err(|e| e.to_string())?;
    let mut outputs = BufWriter::new(outputs_file);
    writeln!(outputs, "{OUTPUT_HEADER}").map_err(|e| e.to_string())?;
    for row in &raw_rows {
        let key = (row.icao, row.time_ns);
        let raw_n = raw_counts.get(&key).copied().ok_or_else(|| {
            format!(
                "raw report key missing for {:08X} at {}",
                row.icao, row.time_ns
            )
        })?;
        let meas_n = measurement_counts.get(&key).copied().unwrap_or(0);
        let recv = received.get(&receive_key(row));
        let (recv_ns, recv_status) = match recv {
            Some(v) if v.len() == 1 => (
                v.first()
                    .copied()
                    .map(|x| x.to_string())
                    .unwrap_or_default(),
                "unique_frame_stamp",
            ),
            Some(v) if v.len() > 1 => (String::new(), "ambiguous_frame_stamp"),
            _ => (String::new(), "frame_stamp_unmatched"),
        };
        let adapter = if raw_n == 1 && meas_n == 1 {
            "passed_unique_match"
        } else if raw_n == 1 && meas_n == 0 {
            "adapter_rejected"
        } else {
            "ambiguous_conflict_group"
        };
        if in_window(row.time_ns, args.start_ns, args.end_ns) {
            let fields = vec![
                format!("0x{:08X}", row.icao),
                row.time_ns.to_string(),
                recv_ns,
                recv_status.into(),
                adapter.into(),
                raw_n.to_string(),
                meas_n.to_string(),
                row.lat_e7.to_string(),
                row.lon_e7.to_string(),
                row.baro_alt_ft.to_string(),
                row.geom_alt_ft.to_string(),
                row.gs_dkt.to_string(),
                row.track_ddeg.to_string(),
                row.nac_p.to_string(),
                row.nic.to_string(),
                row.sil.to_string(),
                row.version.to_string(),
                row.flags.to_string(),
                row.source.to_string(),
                row.roll_cdeg.to_string(),
                row.track_rate_cdps.to_string(),
            ];
            csv_record(
                &mut raw_out,
                fields,
                RAW_HEADER.split(',').count(),
                "raw-inputs",
            )?;
        }
    }
    for m in &all_measurements {
        let before = *tracker_stats(&tracker);
        tracker_update(&mut tracker, m, &mut out);
        let after = *tracker_stats(&tracker);
        if args.addresses.contains(&m.addr) && in_window(m.toa_ns, args.start_ns, args.end_ns) {
            let refusal_delta: u64 = after
                .refused
                .iter()
                .zip(before.refused.iter())
                .map(|(a, b)| a.saturating_sub(*b))
                .sum();
            let tracker_status = if refusal_delta > 0 {
                "validation_refused"
            } else if after.time_held > before.time_held {
                "held_pending"
            } else {
                "outcome_unattributed"
            };
            let source_key = (m.addr, m.toa_ns);
            let (receive_ns, receive_status, source_status) =
                match raw_counts.get(&source_key).copied() {
                    Some(1) if measurement_counts.get(&source_key).copied() == Some(1) => {
                        let row = unique_raw.get(&source_key);
                        let found = row
                            .and_then(|r| received.get(&receive_key(r)))
                            .filter(|v| v.len() == 1)
                            .and_then(|v| v.first())
                            .copied();
                        (
                            found.map_or_else(String::new, |v| v.to_string()),
                            if found.is_some() {
                                "unique_frame_stamp"
                            } else {
                                "frame_stamp_unmatched"
                            },
                            "unique_source_match",
                        )
                    }
                    _ => (
                        String::new(),
                        "ambiguous_or_missing_source",
                        "ambiguous_source_match",
                    ),
                };
            let [pc0, pc1, pc2] = vector3(&m.pos_cov_m2);
            let [vc0, vc1, vc2] = vector3(&m.vel_cov_m2s2);
            let raw = unique_raw.get(&source_key);
            let gs = raw.map_or_else(String::new, |r| r.gs_dkt.to_string());
            let track = raw.map_or_else(String::new, |r| r.track_ddeg.to_string());
            let fields = vec![
                format!("0x{:08X}", m.addr),
                m.toa_ns.to_string(),
                receive_ns,
                receive_status.into(),
                source_status.into(),
                tracker_status.into(),
                after.applied.saturating_sub(before.applied).to_string(),
                after.time_held.saturating_sub(before.time_held).to_string(),
                after
                    .time_held_applied
                    .saturating_sub(before.time_held_applied)
                    .to_string(),
                after
                    .refiltered
                    .saturating_sub(before.refiltered)
                    .to_string(),
                refusal_delta.to_string(),
                after
                    .jump_rejected
                    .saturating_sub(before.jump_rejected)
                    .to_string(),
                after
                    .duplicates
                    .saturating_sub(before.duplicates)
                    .to_string(),
                after
                    .late_window
                    .saturating_sub(before.late_window)
                    .to_string(),
                after.late_ring.saturating_sub(before.late_ring).to_string(),
                m.lat_e7.to_string(),
                m.lon_e7.to_string(),
                gs,
                track,
                f32_cell(m.vel_east_mps),
                f32_cell(m.vel_north_mps),
                pc0,
                pc1,
                pc2,
                vc0,
                vc1,
                vc2,
                m.sensor.to_string(),
            ];
            csv_record(
                &mut inputs,
                fields,
                INPUT_HEADER.split(',').count(),
                "inputs",
            )?;
        }
        if output_len(&out) > DRAIN_ROWS.min(config.out_capacity / 2) {
            write_output_rows(
                &out,
                &mut outputs,
                &built.truth,
                &args.addresses,
                args.start_ns,
                args.end_ns,
            )?;
            output_clear(&mut out);
        }
    }
    let end = all_measurements.last().map_or(0, |m| m.toa_ns);
    tracker_advance(&mut tracker, end, &mut out);
    tracker_flush(&mut tracker, end, &mut out);
    write_output_rows(
        &out,
        &mut outputs,
        &built.truth,
        &args.addresses,
        args.start_ns,
        args.end_ns,
    )?;
    outputs.flush().map_err(|e| e.to_string())?;
    inputs.flush().map_err(|e| e.to_string())?;
    raw_out.flush().map_err(|e| e.to_string())?;
    let mut stats = stats_csv(&tracker);
    let _ = writeln!(stats, "output_overflow,{}", out.overflow);
    fs::write(args.out.join("stats.csv"), stats).map_err(|e| e.to_string())?;
    let metadata = format!("clip_sha256={},reports={},set_a={},set_b={},truth_legs={},selected_addresses={},selected_measurements={},adapter_removed={:?},interval_p50_ns={},interval_p90_ns={},interval_p99_ns={},window_start_ns={},window_end_ns={},r4_min_samples={},r4_threshold_m={}\n",
        clip_hash, full_reports, split.set_a, split.set_b, truth_n,
        args.addresses.len(), selected_count, adapter_stats.removed, interval.p50_ns,
        interval.p90_ns, interval.p99_ns, args.start_ns.map_or_else(String::new, |v| v.to_string()),
        args.end_ns.map_or_else(String::new, |v| v.to_string()), PER_FLIGHT_MIN_N, R4_PER_FLIGHT_M);
    fs::write(args.out.join("metadata.txt"), metadata).map_err(|e| e.to_string())?;
    Ok(())
}

fn main() -> ExitCode {
    // Every path is checked, as given and resolved, before anything is opened or created.
    let sealed = |a: Args| -> Result<Args, String> {
        for p in [&a.clip, &a.out] {
            oa_sealed::refuse(p)?;
        }
        Ok(a)
    };
    match parse_args().and_then(sealed).and_then(run) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("prediction_trace: {e}");
            ExitCode::from(1)
        }
    }
}
