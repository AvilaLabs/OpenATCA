//! One aircraft's timeline around a time, for reading a turn by eye: the set-A measurements the
//! tracker was given, the reference's heading, and the tracker's rows (heading, turn rate, turn
//! probability, position against the reference).
//!
//! ```text
//! cargo run --release -p oa-track-tools --example turn_trace -- --adsb <clip.oaf> --addr <hex> --t-ms <unix ms> [--span-s 40]
//! ```
//!
//! The split, tracker run and reference are those of `turn_diag`. A diagnostic: prints to standard
//! output only.

use std::path::PathBuf;
use std::process::ExitCode;

use oa_bench::adsb_truth;
use oa_bench::input::{file_opener, load_adsb_counted};
use oa_track::adsb::{adsb_measurements, AdsbConfig};
use oa_track::{
    output_clear, output_len, output_new, tracker_advance, tracker_flush, tracker_new,
    tracker_update, TrackOutput, DEFAULT_CONFIG, M_HAS_VEL,
};
use oa_track_tools::score::{
    interval_stats, scoring_config, split_reports, IntervalStats, DRAIN_ROWS,
};
use oa_truth::DEFAULT_CONFIG as TRUTH_DEFAULT;

const M_PER_DEG: f64 = 111_320.0;

fn compass(ve: f64, vn: f64) -> f64 {
    ve.atan2(vn).to_degrees().rem_euclid(360.0)
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (mut adsb, mut addr, mut t_ms, mut span) = (None, None, None, 40i64);
    while let Some(a) = args.next() {
        let v = args.next();
        match a.as_str() {
            "--adsb" => adsb = v.map(PathBuf::from),
            "--addr" => addr = v.and_then(|s| u32::from_str_radix(&s, 16).ok()),
            "--t-ms" => t_ms = v.and_then(|s| s.parse::<i64>().ok()),
            "--span-s" => span = v.and_then(|s| s.parse().ok()).unwrap_or(span),
            _ => return ExitCode::from(2),
        }
    }
    let (Some(adsb), Some(addr), Some(t_ms)) = (adsb, addr, t_ms) else {
        eprintln!(
            "usage: turn_trace --adsb <clip.oaf> --addr <hex> --t-ms <unix ms> [--span-s 40]"
        );
        return ExitCode::from(2);
    };
    if let Err(e) = oa_sealed::refuse(&adsb) {
        eprintln!("turn_trace: {e}");
        return ExitCode::from(2);
    }
    let (lo, hi) = (
        (t_ms - span * 1000) * 1_000_000,
        (t_ms + span * 1000) * 1_000_000,
    );
    let mut sealed = 0u64;
    let Ok(clip) = load_adsb_counted(&adsb, &file_opener(), &mut sealed) else {
        return ExitCode::from(1);
    };
    let (set_a, set_b, _) = split_reports(&clip.reports);
    drop(clip);
    let (meas, _) = adsb_measurements(&set_a, &AdsbConfig::default());
    drop(set_a);
    let iv = interval_stats(&meas).unwrap_or(IntervalStats {
        n: 0,
        p50_ns: 1_000_000_000,
        p90_ns: 1_000_000_000,
        p99_ns: 1_000_000_000,
    });
    let cfg = scoring_config(&DEFAULT_CONFIG, &iv);
    let built = match adsb_truth::build(&set_b, &TRUTH_DEFAULT) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("truth: {e}");
            return ExitCode::from(1);
        }
    };
    let truth_at =
        |t_ns: i64| adsb_truth::query(&built.truth, addr, t_ns.div_euclid(1_000_000)).ok();
    let rel = |t_ns: i64| (t_ns - t_ms * 1_000_000) as f64 / 1e9;
    println!("measurements (set A): t_rel  heading  speed  | truth heading");
    for m in &meas {
        if m.addr != addr || m.toa_ns < lo || m.toa_ns > hi {
            continue;
        }
        let v = if m.flags & M_HAS_VEL != 0 {
            let (e, n) = (f64::from(m.vel_east_mps), f64::from(m.vel_north_mps));
            format!("{:7.2} {:6.1}", compass(e, n), e.hypot(n))
        } else {
            "   none".to_string()
        };
        let tr = truth_at(m.toa_ns).map_or("none".to_string(), |s| {
            format!("{:7.2}", compass(s.vel_east_mps, s.vel_north_mps))
        });
        println!("  {:7.2} {v} | {tr}", rel(m.toa_ns));
    }
    let mut t = match tracker_new(&cfg) {
        Ok(t) => t,
        Err(_) => return ExitCode::from(1),
    };
    let mut scratch = output_new(cfg.out_capacity);
    println!("tracker rows: t_rel kind heading omega_cw_dps mu_turn | truth heading, error east north (m, tracker minus truth)");
    let dump = |o: &TrackOutput| {
        for i in 0..o.t_ns.len() {
            let (Some(&a), Some(&tn), Some(&f)) = (o.addr.get(i), o.t_ns.get(i), o.flags.get(i))
            else {
                continue;
            };
            if a != addr || tn < lo || tn > hi {
                continue;
            }
            let g = |v: &[f32]| v.get(i).copied().map_or(f64::NAN, f64::from);
            let (ve, vn) = (g(&o.vel_east_mps), g(&o.vel_north_mps));
            let mu = o.mu_h.get(i).map_or(f64::NAN, |m| f64::from(m[1]));
            let (la, lo_) = (
                f64::from(o.lat_e7.get(i).copied().unwrap_or(0)) * 1e-7,
                f64::from(o.lon_e7.get(i).copied().unwrap_or(0)) * 1e-7,
            );
            let (th, err) = truth_at(tn).map_or(("   none".to_string(), String::new()), |s| {
                let n = (la - s.lat) * M_PER_DEG;
                let e = (lo_ - s.lon) * M_PER_DEG * s.lat.to_radians().cos();
                (
                    format!("{:7.2}", compass(s.vel_east_mps, s.vel_north_mps)),
                    format!("{e:7.1} {n:7.1}"),
                )
            });
            println!(
                "  {:7.2} {} {:7.2} {:6.2} {:5.3} | {th} {err}",
                rel(tn),
                if f & oa_track::O_TICK != 0 {
                    "tick"
                } else {
                    "upd "
                },
                compass(ve, vn),
                g(&o.turn_rate_rps).to_degrees(),
                mu
            );
        }
    };
    for m in &meas {
        tracker_update(&mut t, m, &mut scratch);
        if output_len(&scratch) > DRAIN_ROWS.min(cfg.out_capacity / 2) {
            dump(&scratch);
            output_clear(&mut scratch);
        }
        if m.toa_ns > hi + 20_000_000_000 {
            break;
        }
    }
    tracker_advance(&mut t, hi, &mut scratch);
    tracker_flush(&mut t, hi, &mut scratch);
    dump(&scratch);
    ExitCode::SUCCESS
}
