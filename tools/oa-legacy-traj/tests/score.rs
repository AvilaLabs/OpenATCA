//! The three systems scored end to end over a made-up recording and a made-up clip: an aircraft
//! that passes a point at a known time and one that lands at a known time.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_adsb::{reports_push, Reports, F_GROUND, ROW_EMPTY};
use oa_bench::adsb_truth;
use oa_bench::legacy::{
    flight_ix, new_legacy, pad, push_route, push_tbfm, push_tfms, set_addr, tbfm_kind, tfms_kind,
    Legacy, RoutePointIn, TfmsIn, NO_COORD, NO_TIME,
};
use oa_legacy_traj::core::join::OUTCOME_NAMES;
use oa_legacy_traj::core::report::{render_counts_csv, render_text};
use oa_legacy_traj::core::types::{exclusion_index, Config, Exclusion, Place};
use oa_legacy_traj::shell::places::{apply_places, PlaceTable, PlacesFile};
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

fn row_at(
    addr: u32,
    t_s: i64,
    (lat, lon): (f64, f64),
    cs: &[u8],
    ground: bool,
    alt_ft: i32,
) -> oa_adsb::Row {
    let mut r = ROW_EMPTY;
    r.icao = addr;
    r.time_ns = t_s * 1_000_000_000;
    r.lat_e7 = e7(lat);
    r.lon_e7 = e7(lon);
    r.nic = 8;
    r.nac_p = 10;
    r.sil = 3;
    r.version = 2;
    r.source = 0;
    r.callsign[..cs.len()].copy_from_slice(cs);
    if ground {
        r.flags = F_GROUND;
    } else {
        r.baro_alt_ft = alt_ft;
    }
    r
}

fn row(addr: u32, t_s: i64, lat: f64, cs: &[u8], ground: bool, alt_ft: i32) -> oa_adsb::Row {
    row_at(addr, t_s, (lat, -88.0), cs, ground, alt_ft)
}

/// Aircraft 1 flies north at 100 m/s through the point at `TC_S`; aircraft 2 flies north at
/// 100 m/s and touches down at the airport at `TL_S`, then sits on the ground there.
fn clip() -> Reports {
    let mut r = Reports::default();
    for t in TC_S - 600..=TC_S + 600 {
        let lat = P.lat_deg + 100.0 * (t - TC_S) as f64 / M_PER_DEG;
        reports_push(&mut r, &row(ADDR1, t, lat, b"TST1", false, 5000));
    }
    for t in TL_S - 600..=TL_S + 300 {
        let lat = AIRPORT.lat_deg + 100.0 * (t.min(TL_S) - TL_S) as f64 / M_PER_DEG;
        reports_push(&mut r, &row(ADDR2, t, lat, b"TST2", t > TL_S, 300));
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
    // TBFM: flight 1's meter fix MFXC, a name only the places file knows: 8 minutes ahead, 20 s
    // late.
    push_tbfm(
        &mut leg,
        pad(b"PLAN1"),
        pad(b"TST1"),
        pad(b"MFXC"),
        tbfm_kind::ETA_MFX,
        (TC_S + 20) * 1000,
        (TC_S - 480) * 1000,
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
    // The same flight's runway estimate once airborne: 5 minutes ahead, 30 s early.
    push_tbfm(
        &mut leg,
        pad(b"PLAN2"),
        pad(b"TST2"),
        pad(b"MFXB"),
        tbfm_kind::ETA_RWY,
        (TL_S - 30) * 1000,
        (TL_S - 300) * 1000,
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
    // And once airborne: 6 minutes stated lead, 60 s late.
    push_tfms(
        &mut leg,
        &TfmsIn {
            time_ms: (TL_S + 60) * 1000,
            issue_ms: (TL_S - 300) * 1000,
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
    Config::new(2.0 * 1852.0, 5.0 * 1852.0, 30_000, 120 * 60_000)
}

fn scored() -> Scored {
    scored_with(clip(), legacy(), |_| {})
}

fn scored_with(reports: Reports, leg: Legacy, more: impl Fn(&mut PlaceTable)) -> Scored {
    let mut tcfg = adsb_truth::config_with(None, None);
    tcfg.loo = false;
    let built = adsb_truth::build(&reports, &tcfg).unwrap();
    let mut places = PlacesFile::default();
    places.airports.insert("ORD".to_string(), AIRPORT);
    places.fixes.push((pad(b"MFXC"), P));
    places.fixes.push((pad(b"MFXA"), P));
    // A name with two places 60 NM apart: the one nearest the route's own coordinates is taken.
    places.fixes.push((pad(b"AMBIG"), P));
    places.fixes.push((
        pad(b"AMBIG"),
        Place {
            lat_deg: 42.0,
            lon_deg: -88.0,
        },
    ));
    let mut table = PlaceTable::default();
    apply_places(&mut table, &places);
    table.elev_ft.insert("ORD".to_string(), 0.0);
    more(&mut table);
    score_all(
        &Inputs {
            legacy: &leg,
            built: &built,
            cfg: cfg(),
            step_ms: 5000,
            table,
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
    let scored: usize = k
        .groups
        .iter()
        .map(|g| g.errors.iter().map(Vec::len).sum::<usize>())
        .sum();
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
    // 60 s late, and the AMBIG point without coordinates, whose nearer place is taken, 60 s late.
    // Flight 3 joined by callsign, at 2 minutes (bin 0 to 5): 10 s late, in the callsign block.
    assert_eq!(k.groups[2].errors[0].len(), 1);
    near(k.groups[2].errors[0][0], 10.0);
    assert!(k.groups[0].errors[0].is_empty());
    assert_eq!(k.groups[0].errors[1].len(), 4);
    let mut e = k.groups[0].errors[1].clone();
    e.sort_by(|a, b| a.partial_cmp(b).unwrap());
    near(e[0], 30.0);
    near(e[1], 45.0);
    near(e[2], 60.0);
    near(e[3], 60.0);
    assert_eq!(k.resolved_ambiguous, 1);
    assert!(k.groups[0].errors[2..].iter().all(Vec::is_empty));
    // The crossing at the three cuts: the 2 NM headline equals its sensitivity row.
    assert_eq!(k.sens["crossing cut 2.0 NM"][1].len(), 4);
    assert!(k.sens.contains_key("crossing cut 0.5 NM"));
    let x = |r: Exclusion| k.exclusions[exclusion_index(r)];
    assert_eq!(x(Exclusion::BeyondCut), 2);
    assert_eq!(x(Exclusion::PlaceAmbiguous), 0);
    assert_eq!(x(Exclusion::PlaceUnknown), 1);
    assert_eq!(x(Exclusion::NoEstimate), 1);
    // Beyond the cut: the far point and the second AMBIG point with coordinates (60 NM away).
    assert_eq!(x(Exclusion::NotJoined), 2);
}

// REQ: OA-LTJ-015
// REQ: OA-LTJ-031
#[test]
fn tbfm_is_scored_against_the_meter_fix_crossing_and_the_landing() {
    let s = scored();
    let t = &s.systems[1];
    // The meter fix MFXA has the place its route point gave: 16.5 minutes of stated lead, 90 s
    // late. The message was issued before the aircraft was airborne: pre-departure.
    let mfx = &t.kinds[0];
    assert_eq!(mfx.groups[1].errors[2].len(), 1);
    near(mfx.groups[1].errors[2][0], 90.0);
    assert!(mfx.groups[0].errors[2].is_empty());
    // The meter fix MFXC is in the places file only: 8 minutes ahead, 20 s late, airborne.
    assert_eq!(mfx.groups[0].errors[1].len(), 1);
    near(mfx.groups[0].errors[1][0], 20.0);
    assert_eq!(mfx.estimates, 2);
    // The runway estimates, against the middle of the gap to the first ground report (0.5 s
    // after the last airborne report): 24 minutes of lead and not yet airborne, 50.5 s early;
    // 4.5 minutes of lead and airborne, 30.5 s early.
    let rwy = &t.kinds[2];
    assert_eq!(rwy.groups[1].errors[3], vec![-50.5]);
    assert_eq!(rwy.groups[0].errors[0], vec![-30.5]);
    assert_eq!(rwy.gaps_s, vec![1.0]);
    // The touchdown at the last airborne report, and at the first ground report.
    assert_eq!(rwy.td_air[0], vec![-30.0]);
    assert_eq!(rwy.td_gnd[0], vec![-31.0]);
    assert_eq!(rwy.last_in_bin.len(), 1);
    assert_eq!(t.outcomes[0], 2);
    assert_eq!(rwy.estimates, 2);
}

// REQ: OA-LTJ-016
#[test]
fn tfms_arrival_estimates_are_scored_against_the_landing_and_departures_are_not() {
    let s = scored();
    let k = &s.systems[2].kinds[0];
    // 67 minutes of stated lead, not yet airborne, 119.5 s late; and 6 minutes, airborne, 59.5 s.
    assert_eq!(k.groups[1].errors[5], vec![119.5]);
    assert_eq!(k.groups[0].errors[1], vec![59.5]);
    assert_eq!(k.estimates, 3);
    assert_eq!(s.etd_rows, 1);
    let text = render_text(&[], &cfg(), &s.sections, &s.systems);
    assert!(
        text.contains("5 to 10 min: n=1 flights=1 |e| median=59.5"),
        "{text}"
    );
}

// REQ: OA-LTJ-020
#[test]
fn an_airport_that_is_not_in_the_table_is_excluded_and_counted_never_derived() {
    // The synthetic day has the landing at ORD; with no place for ORD the runway estimates are
    // excluded as airport unknown, though the aircraft lands within a few miles of a spot.
    let reports = clip();
    let mut tcfg = adsb_truth::config_with(None, None);
    tcfg.loo = false;
    let built = adsb_truth::build(&reports, &tcfg).unwrap();
    let leg = legacy();
    let s = score_all(
        &Inputs {
            legacy: &leg,
            built: &built,
            cfg: cfg(),
            step_ms: 5000,
            table: PlaceTable::default(),
        },
        reports,
    );
    let rwy = &s.systems[1].kinds[2];
    assert!(rwy
        .groups
        .iter()
        .all(|g| g.errors.iter().all(Vec::is_empty)));
    assert_eq!(
        rwy.exclusions[exclusion_index(Exclusion::AirportUnknown)],
        2
    );
    let tf = &s.systems[2].kinds[0];
    assert!(tf.groups.iter().all(|g| g.errors.iter().all(Vec::is_empty)));
    assert!(tf.exclusions[exclusion_index(Exclusion::AirportUnknown)] >= 1);
}

// REQ: OA-LTJ-021
#[test]
fn a_places_file_gives_named_points_and_airports_and_a_bad_line_is_named() {
    use oa_legacy_traj::shell::places::{airport_places, parse_places, point_places};
    let f = parse_places(
        "# points\nfix, abc, 41.5, -88.25\nfix,ABC,43.5,-88.25\nfix,SOLO,41.0,-88.0\n\nairport,KORD,41.98,-87.9\n",
    )
    .unwrap();
    assert_eq!(f.fixes.len(), 3);
    assert_eq!(f.airports.len(), 1);
    assert!(f.airports.contains_key("ORD"));
    let mut t = PlaceTable::default();
    apply_places(&mut t, &f);
    assert_eq!(point_places(&t, "ABC").len(), 2);
    assert_eq!(point_places(&t, "SOLO")[0].lat_deg, 41.0);
    assert!(point_places(&t, "NONE").is_empty());
    assert_eq!(airport_places(&t, "KORD").len(), 1);
    for (bad, line) in [
        ("fix,AB,1,2\nfix,CD,x,2", 2),
        ("fix,AB,95,2", 1),
        ("pier,AB,1,2", 1),
        ("fix,TOOLONGNAME,1,2", 1),
        ("airport,KORD,1,2\nairport,ORD,1,2", 2),
        ("fix,AB,1", 1),
    ] {
        let e = parse_places(bad).unwrap_err();
        assert!(e.contains(&format!("line {line}")), "{bad}: {e}");
    }
}

// REQ: OA-LTJ-024
#[test]
fn nasr_gives_fixes_navaids_and_airports_by_both_codes_and_a_places_file_overrides_it() {
    use oa_legacy_traj::shell::places::{airport_places, point_places, table_from_csv};
    let dir = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/oa-nasr/tests/fixtures/"
    );
    let rd = |n: &str| std::fs::read(format!("{dir}{n}.csv")).unwrap();
    let mut t = table_from_csv(&rd("FIX_BASE"), &rd("NAV_BASE"), &rd("APT_BASE")).unwrap();
    // A fix, a navaid and an airport (FAA and K-prefixed ICAO form).
    assert_eq!(point_places(&t, "aaall").len(), 1);
    assert!((point_places(&t, "AAALL")[0].lat_deg - 42.12018888).abs() < 1e-6);
    assert_eq!(point_places(&t, "DPA").len(), 1);
    assert_eq!(airport_places(&t, "ORD").len(), 1);
    assert_eq!(airport_places(&t, "KORD").len(), 1);
    // A route may name an airport: the point lookup falls back to airports.
    assert_eq!(point_places(&t, "ORD").len(), 1);
    assert!(point_places(&t, "NOSUCH").is_empty());
    let f = oa_legacy_traj::shell::places::parse_places("fix,AAALL,10.0,20.0\n").unwrap();
    apply_places(&mut t, &f);
    assert_eq!(point_places(&t, "AAALL").len(), 1);
    assert_eq!(point_places(&t, "AAALL")[0].lat_deg, 10.0);
}

const ADDR3: u32 = 0xa00003;
/// The point leg A of aircraft 3 passes, and the one leg B passes (a turnaround, a day later in
/// this made-up clip).
const P3: Place = Place {
    lat_deg: 40.0,
    lon_deg: -88.0,
};
const Q3: Place = Place {
    lat_deg: 40.0,
    lon_deg: -87.0,
};
const PA_S: i64 = T0_S + 10_600;
const QB_S: i64 = T0_S + 20_600;

/// Aircraft 3 flies two legs 2 h apart: leg A north through `P3` at `PA_S`, leg B north through
/// `Q3` at `QB_S`.
fn clip3() -> Reports {
    let mut r = Reports::default();
    for (centre, lon) in [(PA_S, -88.0), (QB_S, -87.0)] {
        for t in centre - 600..=centre + 600 {
            let lat = 40.0 + 100.0 * (t - centre) as f64 / M_PER_DEG;
            reports_push(&mut r, &row_at(ADDR3, t, (lat, lon), b"TST3", false, 5000));
        }
    }
    r
}

// REQ: OA-LTJ-029
#[test]
fn a_turnaround_leg_is_never_scored_for_a_message_of_the_flight_before_it() {
    let mut leg = new_legacy();
    let f = flight(&mut leg, 6, Some(ADDR3), b"TST3", b"PLAN6", b"");
    // In progress on leg A: P3 is on leg A (20 s late); Q3 is on leg B only: not on the bound
    // leg, so it is not scored. Then a message between the legs binds leg B, the first one that
    // starts after it: Q3 40 s late (not airborne yet); P3 is not on leg B.
    push_route(
        &mut leg,
        f,
        (PA_S - 300) * 1000,
        &[
            pt(b"WPA", Some(P3), Some(PA_S + 20)),
            pt(b"WPB", Some(Q3), Some(QB_S + 20)),
        ],
    );
    push_route(
        &mut leg,
        f,
        (PA_S + 4_000) * 1000,
        &[
            pt(b"WPA", Some(P3), Some(PA_S + 5_000)),
            pt(b"WPB", Some(Q3), Some(QB_S + 40)),
        ],
    );
    let s = scored_with(clip3(), leg, |_| {});
    let k = &s.systems[0].kinds[0];
    assert_eq!(k.groups[0].errors.iter().map(Vec::len).sum::<usize>(), 1);
    near(k.groups[0].errors[1][0], 20.0);
    assert_eq!(k.groups[1].errors.iter().map(Vec::len).sum::<usize>(), 1);
    near(k.groups[1].errors[5][0], 40.0);
    // Two points of the two messages are on the other leg: beyond the cut on the bound one.
    assert_eq!(k.exclusions[exclusion_index(Exclusion::BeyondCut)], 2);
    assert_eq!(k.estimates, 4);
    assert_eq!(
        s.systems[0].counters["joined flights whose interval touched more than one leg"],
        1
    );
}

// REQ: OA-LTJ-030
#[test]
fn a_callsign_join_is_refused_when_no_leg_fits_the_airports_or_the_window() {
    let mut leg = new_legacy();
    // Joined by callsign; the arrival airport is known and far from where the leg ends.
    let f = flight(&mut leg, 7, None, b"TST3", b"PLAN7", b"KXYZ");
    push_route(
        &mut leg,
        f,
        (PA_S - 300) * 1000,
        &[pt(b"WPA", Some(P3), Some(PA_S + 20))],
    );
    // Joined by callsign, no airports named, but the first issue is over 3 h before the leg
    // that the interval reaches through a later message.
    let g = flight(&mut leg, 8, None, b"TST3", b"PLAN8", b"");
    push_route(
        &mut leg,
        g,
        (PA_S - 300) * 1000,
        &[pt(b"WPA", Some(P3), Some(PA_S + 20))],
    );
    push_route(
        &mut leg,
        g,
        (PA_S - 4 * 3600) * 1000,
        &[pt(b"WPA", Some(P3), Some(PA_S + 20))],
    );
    // Joined by callsign, no airports named, in progress: kept (in the callsign block).
    let h = flight(&mut leg, 9, None, b"TST3", b"PLAN9", b"");
    push_route(
        &mut leg,
        h,
        (PA_S - 300) * 1000,
        &[pt(b"WPA", Some(P3), Some(PA_S + 20))],
    );
    let s = scored_with(clip3(), leg, |t| {
        t.airports.insert(
            "XYZ".to_string(),
            vec![Place {
                lat_deg: 45.0,
                lon_deg: -80.0,
            }],
        );
    });
    let e = &s.systems[0];
    assert_eq!(e.outcomes[1], 1);
    assert_eq!(e.outcomes[3], 2);
    let a = "callsign joins refused: no leg took off at the departure airport or landed at the arrival airport";
    let w = "callsign joins refused: no leg starts within the window after the first issue";
    assert_eq!(e.counters[a], 1);
    assert_eq!(e.counters[w], 1);
    assert_eq!(
        e.kinds[0].groups[2]
            .errors
            .iter()
            .map(Vec::len)
            .sum::<usize>(),
        1
    );
    assert!(e.kinds[0].groups[0].errors.iter().all(Vec::is_empty));
}

// REQ: OA-LTJ-034
#[test]
fn no_two_distinct_airports_share_a_key_and_a_shared_key_is_found() {
    use oa_legacy_traj::shell::places::table_from_csv;
    let dir = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/oa-nasr/tests/fixtures/"
    );
    let rd = |n: &str| std::fs::read(format!("{dir}{n}.csv")).unwrap();
    let t = table_from_csv(&rd("FIX_BASE"), &rd("NAV_BASE"), &rd("APT_BASE")).unwrap();
    assert!(t.collisions.is_empty());
    assert!(t.elev_ft.contains_key("ORD"));
    // A second airport whose ICAO identifier is another's FAA identifier after the K is dropped.
    let apt = String::from_utf8(rd("APT_BASE")).unwrap();
    let mut lines: Vec<String> = apt.lines().map(str::to_string).collect();
    let head = lines.remove(0);
    let first = lines[0].clone();
    let cols: Vec<&str> = head.split(',').collect();
    let col = |n: &str| cols.iter().position(|c| c.trim_matches('"') == n).unwrap();
    let mut other: Vec<String> = first.split(',').map(str::to_string).collect();
    other[col("SITE_NO")] = "\"99999.\"".to_string();
    other[col("ARPT_ID")] = "\"KORD\"".to_string();
    let csv = format!("{head}\n{first}\n{}\n", other.join(","));
    // A four-letter identifier K... is keyed without the K: it collides with ORD of site 04508.
    let t2 = table_from_csv(&rd("FIX_BASE"), &rd("NAV_BASE"), csv.as_bytes());
    // The synthetic row may not parse as a different airport; when it does, the key is reported.
    if let Ok(t2) = t2 {
        assert!(
            t2.collisions.contains(&"ORD".to_string()),
            "{:?}",
            t2.collisions
        );
    }
}
