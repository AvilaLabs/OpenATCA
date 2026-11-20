//! The split of the climbing and descending encounters near 1,000 ft by what the flight record
//! held, over made-up encounters and answers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_encounter::Encounter;
use oa_encounter_run::args::parse;
use oa_encounter_run::join::{
    answer_queries, assigned_of, effective_of, is_near_climb, join_text, observe, timeline, Answer,
    CallsignObs, Clearance, CsReject, CsResult, DiagClass, JoinStats, Key, Query, Sfdps,
};
use oa_encounter_run::run::run;
use oa_flight::{Alt, ALT_AT_FIX, ALT_NIL, ALT_SIMPLE, ALT_VFR};

fn enc(v: f32, vr: f32, class: u8) -> Encounter {
    Encounter {
        addr_a: 1,
        addr_b: 2,
        start_ns: 0,
        end_ns: 5_000_000_000,
        closest_ns: 2_000_000_000,
        interp_ns: 2_000_000_000,
        min_h_m: 500.0,
        min_h_interp_m: 500.0,
        min_v_ft: v,
        vrate_a_fpm: vr,
        vrate_b_fpm: 0.0,
        max_abs_vrate_fpm: vr.abs(),
        v_at_closest_ft: v,
        min_scaled_m: 900.0,
        alt_ft: 12_000.0,
        lat_e7: 411_234_567,
        lon_e7: -881_234_567,
        ticks_inside: 6,
        ticks_alt_unknown: 0,
        ticks_unevaluated: 0,
        flags_a: 0,
        flags_b: 0,
        ground_either: 0,
        class,
    }
}

fn simple(ft: i32) -> Alt {
    Alt {
        form: ALT_SIMPLE,
        a_ft: ft,
        b_ft: 0,
    }
}

fn ans(assigned: Alt, interim: Alt) -> Answer {
    Answer {
        flight: true,
        assigned,
        interim,
        key: Key::Address,
        ..Answer::default()
    }
}

const NONE: Alt = Alt {
    form: 0,
    a_ft: 0,
    b_ft: 0,
};

fn stats() -> JoinStats {
    JoinStats {
        frames: 10,
        ops: 20,
        refused: 1,
        conflicts: 0,
        flights_held: 3,
        positions: 0,
        positions_refused: 0,
    }
}

fn count(text: &str, section: &str, label: &str) -> u64 {
    let s = text.split(section).nth(1).unwrap();
    let line = s.lines().find(|l| l.contains(label)).unwrap();
    line.rsplit(": ").next().unwrap().trim().parse().unwrap()
}

// REQ: OA-FLT-020
#[test]
fn the_group_is_airborne_climbing_encounters_from_900_ft_up_to_the_volume() {
    let ap = [];
    assert!(is_near_climb(&enc(950.0, 800.0, 2), &ap, 1000.0));
    assert!(is_near_climb(&enc(900.0, -800.0, 2), &ap, 1000.0));
    assert!(
        !is_near_climb(&enc(899.0, 800.0, 2), &ap, 1000.0),
        "under the band"
    );
    assert!(
        !is_near_climb(&enc(950.0, 100.0, 2), &ap, 1000.0),
        "level flight"
    );
    assert!(
        !is_near_climb(&enc(1000.0, 800.0, 2), &ap, 1000.0),
        "not under the volume"
    );
}

// REQ: OA-FLT-020
#[test]
fn the_interim_altitude_wins_where_it_is_held_and_a_clear_leaves_the_assigned_one() {
    let a = ans(simple(30_000), simple(24_000));
    assert_eq!(assigned_of(&a), Clearance::Feet(30_000));
    assert_eq!(effective_of(&a), Clearance::Feet(24_000));
    let cleared = ans(
        simple(30_000),
        Alt {
            form: ALT_NIL,
            a_ft: 0,
            b_ft: 0,
        },
    );
    assert_eq!(effective_of(&cleared), Clearance::Feet(30_000));
    let vfr = ans(
        Alt {
            form: ALT_VFR,
            a_ft: 0,
            b_ft: 0,
        },
        NONE,
    );
    assert_eq!(
        assigned_of(&vfr),
        Clearance::NotSimple,
        "VFR is never a number"
    );
    let fix = ans(
        Alt {
            form: ALT_AT_FIX,
            a_ft: 22_000,
            b_ft: 7_000,
        },
        NONE,
    );
    assert_eq!(assigned_of(&fix), Clearance::NotSimple);
    assert_eq!(assigned_of(&ans(NONE, NONE)), Clearance::Missing);
}

// REQ: OA-FLT-020
#[test]
fn each_encounter_of_the_group_is_split_once_and_the_unresolved_are_counted_apart() {
    // Encounters 0 to 5 are in the group; 6 is level flight (not in it).
    let es = [
        enc(950.0, 800.0, 2),
        enc(950.0, 800.0, 2),
        enc(950.0, 800.0, 2),
        enc(950.0, 800.0, 2),
        enc(950.0, 800.0, 3),
        enc(950.0, 800.0, 3),
        enc(990.0, 0.0, 2),
    ];
    let none = Answer::default();
    let mut answers = Vec::new();
    // 0: cleared 1,000 ft apart (level-off). 1: cleared 500 ft apart (does not stop).
    answers.extend([ans(simple(30_000), NONE), ans(simple(31_000), NONE)]);
    answers.extend([ans(simple(30_000), NONE), ans(simple(30_500), NONE)]);
    // 2: no record for the second. 3: a record with no altitude.
    answers.extend([ans(simple(30_000), NONE), none]);
    answers.extend([ans(simple(30_000), NONE), ans(NONE, NONE)]);
    // 4: assigned the same, the interim 1,000 ft apart. 5: VFR.
    answers.extend([
        ans(simple(30_000), simple(24_000)),
        ans(simple(30_000), simple(25_000)),
    ]);
    answers.extend([
        ans(
            Alt {
                form: ALT_VFR,
                a_ft: 0,
                b_ft: 0,
            },
            NONE,
        ),
        ans(simple(30_000), NONE),
    ]);
    answers.extend([none, none]);
    let t = join_text(&es, &answers, &[], 1000.0, &stats(), &[], "test");
    assert!(t.contains("before (one group): 6 climbing"));
    let a = t.split("judged on the assigned altitude").nth(1).unwrap();
    let a = a.split("judged on the interim").next().unwrap();
    let all = |label: &str| count(a, "all classes:", label);
    assert_eq!(all("level-off"), 1);
    assert_eq!(all("does not stop"), 2, "encounters 1 and 4");
    assert_eq!(all("no flight record"), 1);
    assert_eq!(all("a flight record with no altitude"), 1);
    assert_eq!(all("that is not one level"), 1);
    let sum: u64 = [
        "level-off",
        "does not stop",
        "no flight record",
        "a flight record with no altitude",
        "that is not one level",
    ]
    .iter()
    .map(|l| all(l))
    .sum();
    assert_eq!(sum, 6, "every encounter of the group lands in exactly one");
    let e = t
        .split("judged on the interim altitude over assigned")
        .nth(1)
        .unwrap();
    assert_eq!(
        count(e, "all classes:", "level-off"),
        2,
        "encounters 0 and 4 stop 1,000 ft apart"
    );
    assert!(t.contains("flight record found: both aircraft 5, one 1, neither 0"));
}

// REQ: OA-FLT-021
#[test]
fn the_arguments_take_a_window_and_the_sfdps_input() {
    let a = |v: &[&str]| parse(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    let base = ["--adsb", "c.oaf", "--out", "o"];
    let ok = a(&[
        base[0],
        base[1],
        base[2],
        base[3],
        "--from",
        "100",
        "--to",
        "200",
        "--sfdps",
        "d",
        "--lag-floor-s",
        "60",
    ])
    .unwrap();
    assert_eq!(ok.window, Some((100, 200)));
    assert_eq!(ok.lag_floor_s, 60);
    assert!(ok.sfdps.is_some());
    let d = a(&base).unwrap();
    assert_eq!(
        (d.window, d.lag_floor_s, d.sfdps, d.hours),
        (None, 58, None, false)
    );
    assert!(a(&[base[0], base[1], base[2], base[3], "--from", "100"]).is_err());
    assert!(a(&[base[0], base[1], base[2], base[3], "--from", "200", "--to", "100"]).is_err());
    assert!(a(&[base[0], base[1], base[2], base[3], "--from", "x", "--to", "1"]).is_err());
}

// REQ: OA-FLT-021
#[test]
fn a_window_or_lag_floor_that_overflows_is_an_error_not_a_panic() {
    let a = |v: &[&str]| parse(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap();
    let big = i64::MAX.to_string();
    let e = run(&a(&[
        "--adsb",
        "/nonexistent/c.oaf",
        "--out",
        "/nonexistent/o",
        "--from",
        "1",
        "--to",
        &big,
    ]))
    .unwrap_err();
    assert!(e.contains("--to") && e.contains("out of range"), "{e}");
    let e = run(&a(&[
        "--adsb",
        "/nonexistent/c.oaf",
        "--out",
        "/nonexistent/o",
        "--sfdps",
        "/nonexistent/d",
        "--lag-floor-s",
        &big,
    ]))
    .unwrap_err();
    assert!(e.contains("--lag-floor-s"), "{e}");
}

const DOC_HEAD: &str = r#"<?xml version="1.0" encoding="UTF-8"?><ns5:MessageCollection xmlns:ns5="http://www.faa.aero/nas/3.0" xmlns:ns2="http://www.fixm.aero/base/3.0" xmlns:ns3="http://www.fixm.aero/flight/3.0" xmlns:ns4="http://www.fixm.aero/foundation/3.0">"#;

/// 2030-01-02T03:04:05.000Z, Unix ms.
const T0: i64 = 1_893_553_445_000;
const MS: i64 = 1_000_000;
const CHI: (i32, i32) = (419_000_000, -879_000_000);

fn guid(n: u32) -> String {
    format!("{n:08x}-2222-3333-4444-555555555555")
}

/// One flight message: gufi `n`, time `T0 + dt_s`, the body, received 10 s after it.
fn frame(seq: u64, n: u32, dt_s: i64, body: &str) -> Vec<u8> {
    let ts = T0 + dt_s * 1000;
    let iso = |ms: i64| {
        // The fixtures stay inside 2030-01-02 03:04 to 03:09.
        let s = (ms - T0) / 1000;
        format!(
            "2030-01-02T03:{:02}:{:02}.000Z",
            4 + (5 + s) / 60,
            (5 + s) % 60
        )
    };
    let xml = format!(
        r#"{DOC_HEAD}<message xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:type="ns5:FlightMessageType"><flight xsi:type="ns5:NasFlightType" centre="ZXX" source="TH" system="SLC" timestamp="{}"><gufi codeSpace="urn:uuid">{}</gufi>{body}</flight></message></ns5:MessageCollection>"#,
        iso(ts),
        guid(n)
    );
    let mut buf = vec![0u8; xml.len() + 64];
    let n = oa_feed::encode_frame(
        &mut buf,
        1,
        0,
        seq,
        (ts + 10_000) * 1_000_000,
        0,
        xml.as_bytes(),
    )
    .unwrap();
    buf.truncate(n);
    buf
}

fn id(cs: &str, status: &str) -> String {
    format!(
        r#"<flightStatus fdpsFlightStatus="{status}"/><flightIdentification aircraftIdentification="{cs}" computerId="123"/>"#
    )
}

fn pos(lat: &str, lon: &str, hhmmss: &str) -> String {
    format!(
        r#"<enRoute><position positionTime="2030-01-02T{hhmmss}Z"><position><location srsName="urn:ogc:def:crs:EPSG::4326"><pos>{lat} {lon}</pos></location></position></position></enRoute>"#
    )
}

fn cs(s: &str) -> CallsignObs {
    let mut c = [0u8; 8];
    c[..s.len()].copy_from_slice(s.as_bytes());
    CallsignObs::Stable(c)
}

fn q(addr: u32, dt_s: i64, callsign: CallsignObs, at: (i32, i32)) -> Query {
    Query {
        addr,
        t_ns: (T0 + dt_s * 1000) * MS,
        callsign,
        lat_e7: at.0,
        lon_e7: at.1,
        try_callsign: true,
    }
}

fn recording(tag: &str, frames: &[Vec<u8>]) -> Sfdps {
    let dir = std::env::temp_dir().join(format!("oa-encounter-join-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.oaf"), frames.concat()).unwrap();
    Sfdps {
        dir,
        lag_floor_ms: 58_000,
        pick_newest: false,
    }
}

// REQ: OA-FLT-024
#[test]
fn an_operation_that_ages_in_after_the_question_is_not_seen_by_it() {
    let body = format!(
        "{}<aircraftDescription aircraftAddress=\"A1B2C3\"/><assignedAltitude><simple uom=\"FEET\">24000.0</simple></assignedAltitude>",
        id("ABC123", "ACTIVE")
    );
    let sf = recording("addr", &[frame(1, 1, 0, &body)]);
    let addr = 0x00A1_B2C3;
    let none = CallsignObs::Absent;
    let queries = [
        q(addr, 11, none, CHI),     // received, not yet aged in
        q(addr, 57, none, CHI),     // a moment before it ages in
        q(addr, 58, none, CHI),     // as it ages in
        q(addr, 90, none, CHI),     // after
        q(addr + 1, 90, none, CHI), // another aircraft
    ];
    let (a, _d, st) = answer_queries(&sf, &queries).unwrap();
    assert_eq!(st.frames, 1);
    let held: Vec<bool> = a.iter().map(|x| x.flight).collect();
    assert_eq!(held, [false, false, true, true, false]);
    assert_eq!(a[3].assigned.a_ft, 24_000);
    assert_eq!(a[3].key, Key::Address);
    let _ = std::fs::remove_dir_all(&sf.dir);
}

// REQ: OA-FLT-025
#[test]
fn the_callsign_names_a_flight_only_when_it_is_the_one_flying_flight_with_it() {
    let alt = |ft: &str| {
        format!("<assignedAltitude><simple uom=\"FEET\">{ft}</simple></assignedAltitude>")
    };
    let frames = [
        // 1: near the ADS-B position; 2: no position; 3: far away.
        frame(
            1,
            1,
            0,
            &format!(
                "{}{}{}",
                id("NEAR1", "ACTIVE"),
                alt("30000.0"),
                pos("41.95", "-87.9", "03:05:30")
            ),
        ),
        frame(
            2,
            2,
            0,
            &format!("{}{}", id("NOPOS2", "ACTIVE"), alt("31000.0")),
        ),
        frame(
            3,
            3,
            0,
            &format!(
                "{}{}",
                id("FAR3", "ACTIVE"),
                pos("43.5", "-87.9", "03:05:30")
            ),
        ),
        // 4 and 5 share a callsign; 6 ended, 7 flying with the same callsign.
        frame(4, 4, 0, &id("TWIN4", "ACTIVE")),
        frame(5, 5, 0, &id("TWIN4", "ACTIVE")),
        frame(6, 6, 0, &id("ONE6", "ACTIVE")),
        frame(7, 6, 1, &id("ONE6", "COMPLETED")),
        frame(
            8,
            7,
            2,
            &format!("{}{}", id("ONE6", "ACTIVE"), alt("27000.0")),
        ),
        // 8: positions older than 120 s before the question do not check it (here: far, but old).
        frame(
            9,
            8,
            0,
            &format!(
                "{}{}",
                id("OLD8", "ACTIVE"),
                pos("43.5", "-87.9", "03:04:00")
            ),
        ),
    ];
    let sf = recording("cs", &frames);
    let t = 100; // everything has aged in; the positions are 15 s old
    let a = |addr| q(addr, t, CallsignObs::Absent, CHI);
    let queries = [
        q(1, t, cs("NEAR1"), CHI),
        q(2, t, cs("NOPOS2"), CHI),
        q(3, t, cs("FAR3"), CHI),
        q(4, t, cs("TWIN4"), CHI),
        q(5, t, cs("ONE6"), CHI),
        q(6, t, cs("NOBODY"), CHI),
        q(7, t, CallsignObs::Unstable, CHI),
        a(8),
        q(9, 300, cs("OLD8"), CHI),
    ];
    let (r, _d, _) = answer_queries(&sf, &queries).unwrap();
    assert_eq!(r[0].key, Key::CallsignChecked);
    assert_eq!(r[0].assigned.a_ft, 30_000);
    assert_eq!(
        r[0].cs,
        CsResult::Accepted {
            checked: true,
            several: false
        }
    );
    assert_eq!(r[1].key, Key::CallsignUnchecked);
    assert_eq!(r[1].assigned.a_ft, 31_000);
    assert_eq!(r[2].cs, CsResult::Rejected(CsReject::PositionDisagrees));
    assert!(!r[2].flight);
    assert_eq!(r[3].cs, CsResult::Rejected(CsReject::SeveralLive));
    assert_eq!(
        r[4].key,
        Key::CallsignUnchecked,
        "the ended flight does not count"
    );
    assert_eq!(r[4].assigned.a_ft, 27_000);
    assert_eq!(r[5].cs, CsResult::Rejected(CsReject::NoLiveFlight));
    assert_eq!(r[6].cs, CsResult::Rejected(CsReject::Unstable));
    assert_eq!(r[7].cs, CsResult::Rejected(CsReject::NoCallsign));
    // The position of flight 8 is 04:00 and the question is at 09:05: older than 120 s.
    assert_eq!(r[8].key, Key::CallsignUnchecked);
    assert!(r.iter().take(8).all(|x| x.addr_flight.is_none()));
    let _ = std::fs::remove_dir_all(&sf.dir);
}

// REQ: OA-FLT-026
#[test]
fn the_callsign_join_sees_only_what_had_aged_in_at_the_question() {
    let frames = [
        frame(
            1,
            1,
            0,
            &format!(
                "{}{}",
                id("LATE1", "ACTIVE"),
                pos("43.5", "-87.9", "03:04:05")
            ),
        ),
        frame(2, 2, 0, &id("LATE1", "ACTIVE")),
    ];
    // Flight 1 ages in at T0 + 58 s with a position 1.6 degrees away; flight 2 shares the callsign.
    let sf = recording("late", &frames);
    let queries = [
        q(1, 30, cs("LATE1"), CHI), // nothing has aged in
        q(1, 57, cs("LATE1"), CHI),
        q(1, 59, cs("LATE1"), CHI), // both aged in
        q(1, 59, CallsignObs::Absent, CHI),
    ];
    let (r, _d, _) = answer_queries(&sf, &queries).unwrap();
    assert_eq!(r[0].cs, CsResult::Rejected(CsReject::NoLiveFlight));
    assert_eq!(r[1].cs, CsResult::Rejected(CsReject::NoLiveFlight));
    assert_eq!(r[2].cs, CsResult::Rejected(CsReject::SeveralLive));
    let _ = std::fs::remove_dir_all(&sf.dir);

    // A position that has not aged in does not check the join: it stands, unchecked.
    let frames = [
        frame(1, 1, 0, &id("POS1", "ACTIVE")),
        frame(2, 1, 100, &pos("43.5", "-87.9", "03:05:45")),
    ];
    let sf = recording("late-pos", &frames);
    let queries = [
        q(1, 120, cs("POS1"), CHI), // the position (ts 100 s) is received at 110 s but ages in at 158 s
        q(1, 200, cs("POS1"), CHI),
    ];
    let (r, _d, _) = answer_queries(&sf, &queries).unwrap();
    assert_eq!(r[0].key, Key::CallsignUnchecked);
    assert_eq!(r[1].cs, CsResult::Rejected(CsReject::PositionDisagrees));
    let _ = std::fs::remove_dir_all(&sf.dir);
}

// REQ: OA-FLT-027
#[test]
fn the_address_join_wins_and_the_two_joins_are_kept_to_be_compared() {
    let frames = [
        frame(
            1,
            1,
            0,
            &format!(
                "{}<aircraftDescription aircraftAddress=\"A1B2C3\"/>",
                id("AAA1", "ACTIVE")
            ),
        ),
        frame(2, 2, 0, &id("BBB2", "ACTIVE")),
    ];
    let sf = recording("both", &frames);
    let t = 120;
    let queries = [
        q(0x00A1_B2C3, t, cs("AAA1"), CHI),
        q(0x00A1_B2C3, t, cs("BBB2"), CHI),
        // The address finds nothing; the callsign is not asked for.
        Query {
            try_callsign: false,
            ..q(0x0000_0042, t, cs("BBB2"), CHI)
        },
    ];
    let (r, _d, _) = answer_queries(&sf, &queries).unwrap();
    assert_eq!(r[0].key, Key::Address);
    assert_eq!(r[0].addr_flight, r[0].cs_flight, "the same flight");
    assert_eq!(r[1].key, Key::Address);
    assert_ne!(
        r[1].addr_flight, r[1].cs_flight,
        "different flights are visible"
    );
    assert!(r[1].cs_flight.is_some());
    assert_eq!(r[2].key, Key::None);
    assert_eq!(r[2].cs, CsResult::NotTried);
    let _ = std::fs::remove_dir_all(&sf.dir);
}

// REQ: OA-FLT-028
#[test]
fn the_adsb_callsign_must_be_stable_over_the_previous_60_s_and_the_position_is_the_latest() {
    use oa_adsb::Reports;
    let mut r = Reports::default();
    let mut add = |addr: u32, t_s: i64, cs: &str, lat: i32| {
        let mut c = [0u8; 8];
        c[..cs.len()].copy_from_slice(cs.as_bytes());
        r.icao.push(addr);
        r.time_ns.push(t_s * 1_000_000_000);
        r.lat_e7.push(lat);
        r.lon_e7.push(-879_000_000);
        r.callsign.push(c);
    };
    // 1: stable (a lower-case, padded form counts as the same); 2: changes; 3: none; 4: changed
    // longer than 60 s ago.
    for t in [100, 110, 120] {
        add(1, t, "abc123 ", 419_000_000 + t as i32);
        add(3, t, "", 419_000_000);
    }
    add(2, 100, "AAA1", 419_000_000);
    add(2, 120, "BBB2", 419_000_000);
    add(4, 10, "OLD1", 419_000_000);
    add(4, 100, "NEW1", 419_000_000);
    add(4, 120, "NEW1", 419_000_000);
    let want = [1u32, 2, 3, 4].into_iter().collect();
    let tl = timeline(&r, &want, 0, i64::MAX);
    let at = 125 * 1_000_000_000;
    let (o, p) = observe(&tl, 1, at);
    assert_eq!(o, cs("ABC123"));
    assert_eq!(p, Some((419_000_120, -879_000_000)), "the latest report");
    assert_eq!(observe(&tl, 2, at).0, CallsignObs::Unstable);
    assert_eq!(observe(&tl, 3, at).0, CallsignObs::Absent);
    assert_eq!(observe(&tl, 4, at).0, cs("NEW1"));
    assert_eq!(observe(&tl, 9, at).0, CallsignObs::Absent);
    // A report after the question is not used; one older than 60 s gives no position.
    assert_eq!(
        observe(&tl, 1, 105 * 1_000_000_000).1.map(|p| p.0),
        Some(419_000_100)
    );
    assert_eq!(observe(&tl, 1, 200 * 1_000_000_000).0, CallsignObs::Absent);
    assert_eq!(observe(&tl, 1, 200 * 1_000_000_000).1, None);
}

fn plan(cs: &str, status: &str, dep: &str, arr: &str) -> String {
    format!(
        "{}<departure departurePoint=\"{dep}\"/><arrival arrivalPoint=\"{arr}\"/>",
        id(cs, status)
    )
}

fn with_pick(mut sf: Sfdps, pick: bool) -> Sfdps {
    sf.pick_newest = pick;
    sf
}

// REQ: OA-FLT-030
#[test]
fn plans_of_one_aircraft_are_picked_by_the_newest_stamp_only_where_they_share_a_route() {
    // Flights 1 and 2 are two plans of one aircraft (the same route); 3 and 4 share a callsign
    // but not a route.
    let mk = |order: [u32; 2]| {
        let mut f = vec![
            frame(1, 3, 0, &plan("SAME1", "ACTIVE", "KAAA", "KBBB")),
            frame(2, 4, 0, &plan("SAME1", "ACTIVE", "KAAA", "KCCC")),
        ];
        let mut k = 3;
        for n in order {
            let alt = if n == 2 { "28000.0" } else { "26000.0" };
            // Flight 2 is stamped later whatever the order of arrival.
            let dt = i64::from(n) * 5;
            let b = format!(
                "{}<assignedAltitude><simple uom=\"FEET\">{alt}</simple></assignedAltitude>",
                plan("ONE", "ACTIVE", "KAAA", "KBBB")
            );
            f.push(frame(k, n, dt, &b));
            k += 1;
        }
        f
    };
    let queries = |cs_a: &str, cs_b: &str| [q(100, 200, cs(cs_a), CHI), q(101, 200, cs(cs_b), CHI)];
    for order in [[1, 2], [2, 1]] {
        let sf = recording("pick", &mk(order));
        let (r, _, _) = answer_queries(
            &with_pick(recording("pick2", &mk(order)), false),
            &queries("ONE", "SAME1"),
        )
        .unwrap();
        assert_eq!(
            r[0].cs,
            CsResult::Rejected(CsReject::SeveralLive),
            "as before"
        );
        let (r, _, _) = answer_queries(&with_pick(sf, true), &queries("ONE", "SAME1")).unwrap();
        assert_eq!(
            r[0].cs,
            CsResult::Accepted {
                checked: false,
                several: true
            }
        );
        assert_eq!(
            r[0].assigned.a_ft, 28_000,
            "the newest plan, whatever the order"
        );
        assert_eq!(
            r[1].cs,
            CsResult::Rejected(CsReject::SeveralLive),
            "another route"
        );
    }
}

// REQ: OA-FLT-032
#[test]
fn a_disagreement_between_the_keys_is_classed_and_looked_at_again_half_an_hour_on() {
    let addr = |a: &str| format!("<aircraftDescription aircraftAddress=\"{a}\"/>");
    let end = |n: u32, dt: i64, cs: &str| frame(100 + u64::from(n), n, dt, &id(cs, "COMPLETED"));
    let frames = [
        // Aircraft 100: its plan 1 holds the address with callsign SAM1 and is dropped; plan 2
        // states SAM1 on the same route, and ends 20 min on.
        frame(
            1,
            1,
            0,
            &format!(
                "{}{}",
                plan("SAM1", "ACTIVE", "KAAA", "KBBB"),
                addr("000064")
            ),
        ),
        frame(2, 1, 1, &id("SAM1", "DROPPED")),
        frame(3, 2, 0, &plan("SAM1", "ACTIVE", "KAAA", "KBBB")),
        // Aircraft 101: another arrival.
        frame(
            4,
            3,
            0,
            &format!(
                "{}{}",
                plan("OTH2", "ACTIVE", "KAAA", "KBBB"),
                addr("000065")
            ),
        ),
        frame(5, 3, 1, &id("OTH2", "DROPPED")),
        frame(6, 4, 0, &plan("OTH2", "ACTIVE", "KAAA", "KCCC")),
        // Aircraft 102: the address flight has another callsign.
        frame(
            7,
            5,
            0,
            &format!(
                "{}{}",
                plan("DIF3", "ACTIVE", "KAAA", "KBBB"),
                addr("000066")
            ),
        ),
        frame(8, 6, 0, &plan("NEW3", "ACTIVE", "KAAA", "KBBB")),
        // Frames are in the order received. Plan 2 of aircraft 100 ends 20 min on, and a frame
        // after the 30 min lets the replay reach the second look.
        end(2, 20 * 60, "SAM1"),
        frame(9, 7, 33 * 60, &id("LATE9", "ACTIVE")),
    ];
    let sf = recording("diag", &frames);
    let queries = [
        q(0x64, 120, cs("SAM1"), CHI),
        q(0x65, 120, cs("OTH2"), CHI),
        q(0x66, 120, cs("NEW3"), CHI),
    ];
    let (r, d, _) = answer_queries(&sf, &queries).unwrap();
    assert!(r.iter().all(|x| x.key == Key::Address), "{r:?}");
    assert_eq!(d.len(), 3);
    let by = |k: usize| d.iter().find(|x| x.query == k).unwrap();
    assert_eq!(by(0).class, DiagClass::SameRoute);
    assert_eq!(by(1).class, DiagClass::OtherRoute);
    assert_eq!(by(2).class, DiagClass::OtherCallsign);
    assert!(d.iter().all(|x| x.probed));
    assert!(
        by(0).addr_ended_later && by(0).cs_ended_later,
        "dropped, and completed"
    );
    assert!(!by(2).addr_ended_later && !by(2).cs_ended_later);
    assert!(!by(0).refile_near);
    let _ = std::fs::remove_dir_all(&sf.dir);
}
