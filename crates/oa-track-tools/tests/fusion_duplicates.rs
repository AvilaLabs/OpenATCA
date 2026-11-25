//! Synthetic checks for anonymous radar hypotheses across sensors.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_geo::aeqd::{frame_new, unproject};
use oa_track::fusion::{
    fusion_advance, fusion_assign_log, fusion_config, fusion_finish, fusion_new, fusion_plots,
    fusion_radar_tracks, fusion_stats, fusion_tracker_config,
};
use oa_track::radar::{polar_of, radar_geom, RadarPlot, RadarSite, NONE_CODE};
use oa_track::{output_new, DEFAULT_CONFIG, NO_ADDR};
use oa_track_tools::fusion_score::identify_tracks;

const SITE: RadarSite = RadarSite {
    lat_deg: 41.976_940_27,
    lon_deg: -87.908_149_72,
    height_m: 207.0,
    sigma_range_m: 70.0,
    sigma_az_deg: 0.08,
    range_quantum_m: 0.0,
    alt_quantum_ft: 100,
    period_ns: 4_800_000_000,
    epoch_ns: 0,
    sensor: 1,
};

fn sites() -> [RadarSite; 2] {
    [
        SITE,
        RadarSite {
            lon_deg: SITE.lon_deg + 0.48,
            sensor: 2,
            ..SITE
        },
    ]
}

#[derive(Clone, Copy)]
struct Line {
    e0: f64,
    n0: f64,
    ve: f64,
    vn: f64,
    alt_ft: i32,
    label: u32,
}

fn plots(site: &RadarSite, lines: &[Line]) -> Vec<RadarPlot> {
    let geom = radar_geom(site);
    let frame = frame_new(SITE.lat_deg, SITE.lon_deg);
    let mut out = Vec::new();
    for scan in 0..24 {
        let t = scan as f64 * 4.8;
        for line in lines {
            let (lat, lon) =
                unproject(&frame, line.e0 + line.ve * t, line.n0 + line.vn * t).unwrap();
            let (range_m, az_deg) = polar_of(&geom, lat, lon, f64::from(line.alt_ft) * 0.3048);
            out.push(RadarPlot {
                time_ns: scan * site.period_ns,
                range_m,
                az_deg,
                alt_ft: line.alt_ft / 100 * 100,
                addr: NO_ADDR,
                label: line.label,
                mode_a: NONE_CODE,
            });
        }
    }
    out
}

fn check_hypotheses(lines: &[Line], expected_hypotheses: usize) {
    let sites = sites();
    let mut cfg = fusion_config(fusion_tracker_config(&DEFAULT_CONFIG, &sites));
    cfg.log_assign = true;
    let mut fusion = fusion_new(&cfg, &sites).unwrap();
    for (i, site) in sites.iter().enumerate() {
        let p = plots(site, lines);
        fusion_plots(&mut fusion, i, &p);
    }
    let end = 24 * SITE.period_ns;
    let mut output = output_new(1 << 16);
    fusion_advance(&mut fusion, end, &mut output);
    fusion_finish(&mut fusion, end + SITE.period_ns, &mut output);
    let stats = fusion_stats(&fusion);
    assert_eq!(stats.cand_confirmed, expected_hypotheses as u64);
    assert_eq!(stats.cand_merged_on_confirm, 0);
    let keys: Vec<_> = fusion_radar_tracks(&fusion)
        .iter()
        .map(|track| track.key)
        .collect();
    assert_eq!(keys.len(), expected_hypotheses);
    let score = identify_tracks(fusion_assign_log(&fusion), &keys);
    let mut matched: Vec<_> = score.truth_of.values().copied().collect();
    matched.sort_unstable();
    let mut expected: Vec<_> = lines.iter().map(|line| line.label).collect();
    expected.sort_unstable();
    assert_eq!(matched, expected);
    assert_eq!(
        score.duplicates as usize,
        expected_hypotheses - expected.len()
    );
    // Duplicate source-local keys are a scorer count, not evidence that plots changed identity.
    assert_eq!(score.swaps, 0);
    assert_eq!(score.wrong_plots, 0);
    assert_eq!(score.false_tracks, 0);
}

fn check_single_radar(lines: &[Line]) {
    let site = SITE;
    let mut cfg = fusion_config(fusion_tracker_config(&DEFAULT_CONFIG, &[site]));
    cfg.log_assign = true;
    let mut fusion = fusion_new(&cfg, &[site]).unwrap();
    let p = plots(&site, lines);
    fusion_plots(&mut fusion, 0, &p);
    let end = 24 * site.period_ns;
    let mut output = output_new(1 << 16);
    fusion_advance(&mut fusion, end, &mut output);
    fusion_finish(&mut fusion, end + site.period_ns, &mut output);

    let keys: Vec<_> = fusion_radar_tracks(&fusion)
        .iter()
        .map(|track| track.key)
        .collect();
    let score = identify_tracks(fusion_assign_log(&fusion), &keys);
    assert_eq!(keys.len(), lines.len());
    assert_eq!(score.duplicates, 0);
    assert_eq!(score.swaps, 0);
    assert_eq!(score.wrong_plots, 0);
    assert_eq!(score.false_tracks, 0);
}

#[test]
fn one_anonymous_aircraft_confirmed_independently_by_two_radars_reproduces_duplicate() {
    let aircraft = [Line {
        e0: 20_000.0,
        n0: 25_000.0,
        ve: -140.0,
        vn: 30.0,
        alt_ft: 22_000,
        label: 0xA00001,
    }];
    // Each radar's candidate confirms independently; current contract keeps both anonymous keys.
    // The paired one-radar control has one key and no duplicate; these two keys add a scorer
    // duplicate while keeping swaps and wrong plots at zero, isolating the E7 source-local case.
    check_single_radar(&aircraft);
    check_hypotheses(&aircraft, 2);
}

#[test]
fn close_parallel_real_aircraft_remain_four_source_local_hypotheses() {
    let aircraft = [
        Line {
            e0: 20_000.0,
            n0: 25_000.0,
            ve: -140.0,
            vn: 30.0,
            alt_ft: 22_000,
            label: 0xA00001,
        },
        Line {
            e0: 20_000.0,
            n0: 25_100.0,
            ve: -140.0,
            vn: 30.0,
            alt_ft: 22_000,
            label: 0xA00002,
        },
    ];
    check_hypotheses(&aircraft, 4);
}

#[test]
fn crossing_real_aircraft_remain_separate_when_their_altitudes_differ() {
    let aircraft = [
        Line {
            e0: 15_000.0,
            n0: 25_000.0,
            ve: 120.0,
            vn: 0.0,
            alt_ft: 18_000,
            label: 0xA00001,
        },
        Line {
            e0: 25_000.0,
            n0: 15_000.0,
            ve: 0.0,
            vn: 120.0,
            alt_ft: 20_000,
            label: 0xA00002,
        },
    ];
    check_hypotheses(&aircraft, 4);
}
