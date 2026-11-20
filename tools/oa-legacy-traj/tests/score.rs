//! The three systems scored end to end over a made-up recording and a made-up clip: an aircraft
//! that passes a point at a known time and one that lands at a known time.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::BTreeMap;

use oa_adsb::{reports_push, Reports, F_GROUND, ROW_EMPTY};
use oa_bench::adsb_truth;
use oa_bench::legacy::{
    flight_ix, new_legacy, pad, push_route, push_tbfm, push_tfms, set_addr, tbfm_kind, tfms_kind,
    Legacy, RoutePointIn, TfmsIn, NO_COORD, NO_TIME,
};
use oa_legacy_traj::core::join::OUTCOME_NAMES;
use oa_legacy_traj::core::report::{render_counts_csv, render_text};
use oa_legacy_traj::core::types::{exclusion_index, Config, Exclusion, Place};
use oa_legacy_traj::shell::run::{score_all, Inputs, Scored};

const M_PER_DEG: f64 = 111_194.926_644_558_74;
/// A time in 2023, far from every sealed window.
const T0_S: i64 = 1_700_000_000;
const ADDR1: u32 = 0xa00001;
const ADDR2: u32 = 0xa00002;
const ADDR_NO_TRUTH: u32 = 0xb00000;
/// The point aircraft 1 passes, and the airport aircraft 2 lands at.
const P: Place = Place {
    lat_deg: 41.0,
    lon_deg: -88.0,
};
const AIRPORT: Place = Place {
    lat_deg: 41.5,
    lon_deg: -88.0,
};
/// Aircraft 1 is at the point at this second; aircraft 2's last airborne report is at this one.
const TC_S: i64 = T0_S + 1000;
const TL_S: i64 = T0_S + 5000;

fn e7(d: f64) -> i32 {
    (d * 1e7).round() as i32
}

fn row(addr: u32, t_s: i64, lat: f64, cs: &[u8], ground: bool) -> oa_adsb::Row {
    let mut r = ROW_EMPTY;
    r.icao = addr;
    r.time_ns = t_s * 1_000_000_000;
    r.lat_e7 = e7(lat);
    r.lon_e7 = e7(-88.0);
    r.nic = 8;
    r.nac_p = 10;
    r.sil = 3;
    r.version = 2;
    r.source = 0;
    r.callsign[..cs.len()].copy_from_slice(cs);
    if ground {
        r.flags = F_GROUND;
    } else {
        r.baro_alt_ft = 5000;
    }
    r
}

/// Aircraft 1 flies north at 100 m/s through the point at `TC_S`; aircraft 2 flies north at
/// 100 m/s and touches down at the airport at `TL_S`, then sits on the ground there.
fn clip() -> Reports {
    let mut r = Reports::default();
    for t in TC_S - 600..=TC_S + 600 {
        let lat = P.lat_deg + 100.0 * (t - TC_S) as f64 / M_PER_DEG;
        reports_push(&mut r, &row(ADDR1, t, lat, b"TST1", false));
    }
    for t in TL_S - 600..=TL_S + 300 {
        let lat = AIRPORT.lat_deg + 100.0 * (t.min(TL_S) - TL_S) as f64 / M_PER_DEG;
        reports_push(&mut r, &row(ADDR2, t, lat, b"TST2", t > TL_S));
    }
    r
}

fn pt(name: &[u8], at: Option<Place>, est_s: Option<i64>) -> RoutePointIn {
    RoutePointIn {
        fix: pad(name),
        est_ms: est_s.map_or(NO_TIME, |s| s * 1000),
        lat_e7: at.map_or(NO_COORD, |p| e7(p.lat_deg)),
        lon_e7: at.map_or(NO_COORD, |p| e7(p.lon_deg)),
    }
}

fn flight(
    leg: &mut Legacy,
    gufi: u128,
    addr: Option<u32>,
    cs: &[u8],
    plan: &[u8],
    arr: &[u8],
) -> u32 {
    let f = flight_ix(leg, gufi).unwrap();
    if let Some(a) = addr {
        set_addr(leg, f, a);
    }
    let i = f as usize;
    leg.flights.callsign[i] = pad(cs);
    leg.flights.plan_id[i] = pad(plan);
    leg.flights.arr[i] = pad(arr);
    f
}

fn legacy() -> Legacy {
    let mut leg = new_legacy();
    // Flight 1 (address): a route message 7 minutes before the crossing.
    let f1 = flight(&mut leg, 1, Some(ADDR1), b"TST1", b"PLAN1", b"");
    let far = Place {
        lat_deg: 45.0,
        lon_deg: -80.0,
    };
    let apart = Place {
        lat_deg: 42.0,
        lon_deg: -88.0,
    };
    push_route(
        &mut leg,
        f1,
        (TC_S - 420) * 1000,
        &[
            pt(b"WPTP", Some(P), Some(TC_S + 30)),
            pt(b"MFXA", Some(P), Some(TC_S + 45)),
            pt(b"FARAWAY", Some(far), Some(TC_S + 900)),
            pt(b"AMBIG", Some(P), Some(TC_S + 60)),
            pt(b"AMBIG", Some(apart), Some(TC_S + 60)),
            pt(b"AMBIG", None, Some(TC_S + 60)),
            pt(b"NOCOORD", None, Some(TC_S + 60)),
            pt(b"NOTIME", Some(P), None),
        ],
    );
    // Flight 3 (no address, the callsign of aircraft 1): two minutes before.
    let f3 = flight(&mut leg, 3, None, b"TST1", b"PLAN3", b"");
    push_route(
        &mut leg,
        f3,
        (TC_S - 120) * 1000,
        &[pt(b"WPTP", Some(P), Some(TC_S + 10))],
    );
    // Flight 2 (an address no aircraft of the clip has): its two points are counted, not dropped.
    let f2 = flight(&mut leg, 2, Some(ADDR_NO_TRUTH), b"NOP9", b"PLAN9", b"");
    push_route(
        &mut leg,
        f2,
        (TC_S - 300) * 1000,
        &[
            pt(b"WPTP", Some(P), Some(TC_S)),
            pt(b"WPTQ", Some(P), Some(TC_S)),
        ],
    );
    // TBFM: flight 1's meter fix 15 minutes ahead, 90 s late.
    push_tbfm(
        &mut leg,
        pad(b"PLAN1"),
        pad(b"TST1"),
        pad(b"MFXA"),
        tbfm_kind::ETA_MFX,
        (TC_S + 90) * 1000,
        (TC_S - 900) * 1000,
    );
    // Aircraft 2: its flight lands at KORD; TBFM runway estimate 25 minutes ahead, 50 s early.
    let f4 = flight(&mut leg, 4, Some(ADDR2), b"TST2", b"PLAN2", b"KORD");
    let _ = f4;
    push_tbfm(
        &mut leg,
        pad(b"PLAN2"),
        pad(b"TST2"),
        pad(b"MFXB"),
        tbfm_kind::ETA_RWY,
        (TL_S - 50) * 1000,
        (TL_S - 25 * 60) * 1000,
    );
    // TFMS: arrival 65 minutes ahead, 120 s late; and a departure estimate, which is not scored.
    let base = TfmsIn {
        plan_id: pad(b"PLAN2"),
        acid: pad(b"TST2"),
        dep: pad(b"KMSP"),
        arr: pad(b"KORD"),
        kind: tfms_kind::ETA,
        time_ms: (TL_S + 120) * 1000,
        issue_ms: (TL_S - 65 * 60) * 1000,
    };
    push_tfms(&mut leg, &base);
    push_tfms(
        &mut leg,
        &TfmsIn {
            kind: tfms_kind::ETD,
            ..base
        },
    );
    // A TFMS flight no SFDPS flight, address or callsign explains.
    push_tfms(
        &mut leg,
        &TfmsIn {
            plan_id: pad(b"PLAN77"),
            acid: pad(b"ZZZ1"),
            ..base
        },
    );
    leg
}

fn cfg() -> Config {
    Config {
        cut_m: 2.0 * 1852.0,
        runway_radius_m: 5.0 * 1852.0,
        landing_gap_max_ms: 120_000,
        max_look_ahead_ms: 120 * 60_000,
    }
}

fn scored() -> Scored {
    let reports = clip();
    let mut tcfg = adsb_truth::config_with(None, None);
    tcfg.loo = false;
    let built = adsb_truth::build(&reports, &tcfg).unwrap();
    let mut arps = BTreeMap::new();
    arps.insert("ORD".to_string(), AIRPORT);
    let leg = legacy();
    score_all(
        &Inputs {
            legacy: &leg,
            built: &built,
            cfg: cfg(),
            step_ms: 5000,
            arps: Some(arps),
        },
        reports,
    )
}

fn near(v: f64, want: f64) {
    assert!((v - want).abs() <= 2.0, "{v} against {want}");
}

// REQ: OA-LTJ-012
#[test]
fn every_flight_is_counted_by_outcome_and_a_flight_without_truth_is_counted_not_dropped() {
    let s = scored();
    let eram = &s.systems[0];
    // Three flights with routes: address, no truth, callsign.
    assert_eq!(eram.outcomes, [1, 1, 0, 1, 0]);
    assert_eq!(s.sfdps_flights, (4, 3));
    assert_eq!(OUTCOME_NAMES.len(), eram.outcomes.len());
    let k = &eram.kinds[0];
    // 8 + 1 + 2 route points offered; the two of the flight with no truth are excluded for it.
    assert_eq!(k.estimates, 11);
    assert_eq!(k.exclusions[exclusion_index(Exclusion::NotJoined)], 2);
    let scored: usize = k.errors.iter().map(Vec::len).sum();
    let excluded: u64 = k.exclusions.iter().sum();
    assert_eq!(scored as u64 + excluded, k.estimates);
    // TBFM and TFMS: the TFMS flight nobody explains is a flight with no truth.
    let tfms = &s.systems[2];
    assert_eq!(tfms.outcomes.iter().sum::<u64>(), 2);
    assert_eq!(tfms.outcomes[3], 1);
    assert_eq!(
        tfms.kinds[0].exclusions[exclusion_index(Exclusion::NotJoined)],
        1
    );
    let counts = render_counts_csv(eram);
    assert!(
        counts.contains(
            "ERAM,join,no truth (an identity, but no ADS-B leg of it in the interval),1\n"
        ),
        "{counts}"
    );
}

// REQ: OA-LTJ-014
#[test]
fn eram_is_scored_along_its_route_by_look_ahead_with_the_sign_estimate_minus_actual() {
    let s = scored();
    let k = &s.systems[0].kinds[0];
    // Flight 1 at 7 minutes (bin 5 to 10): WPTP 30 s late, MFXA 45 s late, the first AMBIG point
    // 60 s late. Flight 3 at 2 minutes (bin 0 to 5): 10 s late.
    assert_eq!(k.errors[0].len(), 1);
    near(k.errors[0][0], 10.0);
    assert_eq!(k.errors[1].len(), 3);
    let mut e = k.errors[1].clone();
    e.sort_by(|a, b| a.partial_cmp(b).unwrap());
    near(e[0], 30.0);
    near(e[1], 45.0);
    near(e[2], 60.0);
    assert!(k.errors[2..].iter().all(Vec::is_empty));
    let x = |r: Exclusion| k.exclusions[exclusion_index(r)];
    assert_eq!(x(Exclusion::BeyondCut), 2);
    assert_eq!(x(Exclusion::PlaceAmbiguous), 1);
    assert_eq!(x(Exclusion::PlaceUnknown), 1);
    assert_eq!(x(Exclusion::NoEstimate), 1);
    // Beyond the cut: the far point and the second AMBIG point with coordinates (60 NM away). The
    // AMBIG point with no coordinates has two places in the book: ambiguous.
    assert_eq!(x(Exclusion::NotJoined), 2);
}

// REQ: OA-LTJ-015
#[test]
fn tbfm_is_scored_against_the_meter_fix_crossing_and_the_landing() {
    let s = scored();
    let t = &s.systems[1];
    // The meter fix MFXA has the place its route point gave: 15 minutes ahead, 90 s late.
    let mfx = &t.kinds[0];
    assert_eq!(mfx.errors[2].len(), 1);
    near(mfx.errors[2][0], 90.0);
    // The runway estimate: 25 minutes ahead, 50 s early, against the last airborne report.
    let rwy = &t.kinds[2];
    assert_eq!(rwy.errors[3].len(), 1);
    assert_eq!(rwy.errors[3][0], -50.0);
    assert_eq!(t.outcomes[0], 2);
    assert_eq!(rwy.estimates, 1);
}

// REQ: OA-LTJ-016
#[test]
fn tfms_arrival_estimates_are_scored_against_the_landing_and_departures_are_not() {
    let s = scored();
    let k = &s.systems[2].kinds[0];
    // 65 minutes ahead, 120 s late.
    assert_eq!(k.errors[5], vec![120.0]);
    assert_eq!(k.estimates, 2);
    assert_eq!(s.etd_rows, 1);
    let text = render_text(&[], &cfg(), &s.systems);
    assert!(
        text.contains("60 to 120 min: n=1 flights=1 median=120.0"),
        "{text}"
    );
}

// REQ: OA-LTJ-020
#[test]
fn a_reference_point_is_derived_only_where_enough_landings_agree() {
    use oa_legacy_traj::core::types::AirState;
    use oa_legacy_traj::shell::places::derive_airports;
    let landing = |lat: f64, t_s: i64| {
        vec![
            AirState {
                t_ms: t_s * 1000,
                lat_deg: lat,
                lon_deg: -88.0,
                ground: false,
            },
            AirState {
                t_ms: (t_s + 10) * 1000,
                lat_deg: lat,
                lon_deg: -88.0,
                ground: true,
            },
        ]
    };
    let mut states = BTreeMap::new();
    // Four aircraft land within 0.2 NM of one spot; one lands 100 NM away (a later leg of the
    // same day); a fifth airport has two landings only.
    for (k, lat) in [41.50, 41.501, 41.499, 41.5005, 43.2]
        .into_iter()
        .enumerate()
    {
        states.insert(k as u32, landing(lat, 1000));
    }
    states.insert(10, landing(40.0, 1000));
    states.insert(11, landing(40.0, 1000));
    let mut flights: Vec<(String, u32, i64, i64)> =
        (0..5).map(|k| ("ORD".to_string(), k, 0, 5_000_000)).collect();
    flights.push(("XYZ".to_string(), 10, 0, 5_000_000));
    flights.push(("XYZ".to_string(), 11, 0, 5_000_000));
    // A landing outside the flight's interval is not a candidate.
    flights.push(("ORD".to_string(), 10, 0, 500_000));
    let arps = derive_airports(&flights, &states);
    assert_eq!(arps.len(), 1);
    let p = arps["ORD"];
    assert!((p.lat_deg - 41.5).abs() < 0.01, "{}", p.lat_deg);
    assert!((p.lon_deg + 88.0).abs() < 1e-9);
}
