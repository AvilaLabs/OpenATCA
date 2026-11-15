//! A terminal and an en-route radar over a real ZAU clip. Skipped (with a note) when the clip is
//! absent: set `OPENATCA_SENSORSIM_CLIP` to an OAF1 clip of ADS-B payload frames.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::Instant;

use oa_asterix::{decode_stream, Block, SourceId};
use oa_sensorsim::clip::read_clip;
use oa_sensorsim::geom::{FT_M, NM_M};
use oa_sensorsim::plots::{plots_len, to_cat048};
use oa_sensorsim::profile::*;
use oa_sensorsim::radar::{simulate_radar, RadarSite};
use oa_truth::{build_truth, DEFAULT_CONFIG};

// REQ: OA-SIM-018
#[test]
fn radars_over_a_real_clip() {
    let Ok(path) = std::env::var("OPENATCA_SENSORSIM_CLIP") else {
        eprintln!("OPENATCA_SENSORSIM_CLIP not set: skipping the real-data run");
        return;
    };
    if !std::path::Path::new(&path).exists() {
        eprintln!("{path} absent: skipping the real-data run");
        return;
    }
    let (reports, bad, dropped) = read_clip(&path).unwrap();
    assert_eq!((bad, dropped), (0, 0));
    let (truth, ts) = build_truth(&reports, &DEFAULT_CONFIG).unwrap();
    let first = *reports.time_ns.iter().min().unwrap();
    let last = *reports.time_ns.iter().max().unwrap();
    // O'Hare ARP and Peoria ARP (NASR 2026-10-29), standing in for antenna positions.
    let mk = |lat, lon, h_ft: f64, period, cov_nm: f64| RadarSite {
        lat_deg: lat,
        lon_deg: lon,
        height_m: h_ft * FT_M,
        rotation_period_s: period,
        initial_az_deg: 0.0,
        epoch_ns: first,
        coverage_range_m: cov_nm * NM_M,
        earth_k: EARTH_K_STANDARD,
        profile: MSSR_EUROCONTROL_1997,
        bias_range_m: 0.0,
        bias_az_deg: 0.0,
        split_dr_m: 300.0,
        split_daz_deg: 0.3,
        sac: 0,
        sic: 1,
    };
    let sites = [
        (
            "terminal",
            mk(41.976_940_27, -87.908_149_72, 680.0, SCAN_TERMINAL_S, 60.0),
        ),
        (
            "enroute",
            mk(40.664_194_44, -89.693_25, 660.7, SCAN_EN_ROUTE_S, 200.0),
        ),
    ];
    for (name, s) in sites {
        let t = Instant::now();
        let (plots, st) = simulate_radar(&truth, &s, 1, first, last + 1).unwrap();
        let secs = t.elapsed().as_secs_f64();
        let bytes = to_cat048(&plots, SourceId { sac: 0, sic: 1 }).unwrap();
        let n_dec: usize = decode_stream(&bytes)
            .unwrap()
            .iter()
            .map(|b| match b {
                Block::Cat048(r) => r.len(),
                _ => 0,
            })
            .sum();
        assert_eq!(n_dec, plots_len(&plots));
        let with_truth = st.in_coverage + st.out_of_range + st.below_horizon;
        eprintln!(
            "{name}: {} aircraft ({} kept by truth), {} plots, in coverage {} of {} with truth, detected {} ({:.2}%), false {}, split {}, no truth {}, unsolved {}, {} bytes, {secs:.1} s",
            ts.aircraft_out,
            st.aircraft,
            plots_len(&plots),
            st.in_coverage,
            with_truth,
            st.detected,
            100.0 * st.detected as f64 / st.in_coverage.max(1) as f64,
            st.false_plots,
            st.split_plots,
            st.no_truth,
            st.unsolved,
            bytes.len()
        );
        assert!(st.in_coverage > 0);
        let rate = st.detected as f64 / st.in_coverage as f64;
        assert!((rate - 0.97).abs() < 0.01, "{rate}");
        assert!(st.unsolved * 1000 <= st.crossings, "{st:?}");
    }
}
