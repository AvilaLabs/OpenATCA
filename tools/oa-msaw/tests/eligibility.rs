//! Who is alerted: the basis of a track from the flight join's answer and the adaptation's
//! requests, and the run with no flight data. Every value is made up.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_encounter_run::join::{AddrResult, Answer, CallsignObs, CsReject, CsResult, Key};
use oa_flight::{Alt, ALT_SIMPLE, ALT_VFR, ALT_VFR_ON_TOP_PLUS};
use oa_msaw::{Alert, FloorKind, Grade};
use oa_msaw_run::config::{parse_config, RequestSpec, Requester};
use oa_msaw_run::eligibility::{basis_of, judge, requested, with_beacon, Basis, Inputs};

const DEV_CFG: &str = include_str!("../testdata/dev.cfg");

fn flight(key: Key, form: u8, active: bool) -> Answer {
    Answer {
        flight: true,
        key,
        active,
        assigned: Alt {
            form,
            a_ft: 0,
            b_ft: 0,
        },
        ..Answer::default()
    }
}

// REQ: OA-MSAW-056
#[test]
fn only_a_track_with_an_active_ifr_flight_or_a_request_is_alerted() {
    assert_eq!(
        basis_of(&flight(Key::Address, ALT_SIMPLE, true), false),
        Basis::Address
    );
    assert_eq!(
        basis_of(&flight(Key::CallsignChecked, ALT_SIMPLE, true), false),
        Basis::Callsign
    );
    assert_eq!(
        basis_of(&flight(Key::CallsignOverAddress, ALT_SIMPLE, true), false),
        Basis::Callsign
    );
    // VFR on top is an IFR clearance; a VFR flight is not alerted, nor one not yet active.
    assert_eq!(
        basis_of(&flight(Key::Address, ALT_VFR_ON_TOP_PLUS, true), false),
        Basis::Address
    );
    assert_eq!(
        basis_of(&flight(Key::Address, ALT_VFR, true), false),
        Basis::VfrPlan
    );
    assert_eq!(
        basis_of(&flight(Key::Address, ALT_SIMPLE, false), false),
        Basis::NotActive
    );
    // A request is alerted whatever the flight record says.
    assert_eq!(
        basis_of(&flight(Key::Address, ALT_VFR, true), true),
        Basis::Request
    );
    assert_eq!(basis_of(&Answer::default(), true), Basis::Request);
    // No flight, and several that cannot be told apart (by address or by callsign).
    assert_eq!(basis_of(&Answer::default(), false), Basis::NoFlight);
    let several_addr = Answer {
        addr: AddrResult::SeveralUnresolved,
        ..Answer::default()
    };
    assert_eq!(basis_of(&several_addr, false), Basis::Ambiguous);
    let several_cs = Answer {
        cs: CsResult::Rejected(CsReject::SeveralLive),
        ..Answer::default()
    };
    assert_eq!(basis_of(&several_cs, false), Basis::Ambiguous);
    for b in [
        Basis::Address,
        Basis::Callsign,
        Basis::Request,
        Basis::NotActive,
        Basis::Ambiguous,
        Basis::NoFlight,
    ] {
        assert!(b.alerted());
    }
    for b in [Basis::VfrPlan, Basis::VfrBeacon] {
        assert!(!b.alerted());
    }
}

// REQ: OA-MSAW-056
#[test]
fn a_track_with_no_flight_is_alerted_unless_its_beacon_code_is_a_vfr_code() {
    for b in [Basis::NoFlight, Basis::Ambiguous, Basis::NotActive] {
        assert_eq!(with_beacon(b, false), b);
        assert!(with_beacon(b, false).alerted());
        assert_eq!(with_beacon(b, true), Basis::VfrBeacon);
        assert!(!with_beacon(b, true).alerted());
    }
    // A flight that is known keeps its basis whatever the code.
    for b in [
        Basis::Address,
        Basis::Callsign,
        Basis::Request,
        Basis::VfrPlan,
    ] {
        assert_eq!(with_beacon(b, true), b);
    }
    // The adaptation's VFR codes: 1200 and the lines of `vfr_beacon_code`.
    let s = parse_config(&format!("{DEV_CFG}\nvfr_beacon_code = 1202\n")).unwrap();
    assert_eq!(s.vfr_codes, vec![1200, 1202]);
    assert_eq!(parse_config(DEV_CFG).unwrap().vfr_codes, vec![1200]);
    assert!(parse_config(&format!("{DEV_CFG}\nvfr_beacon_code = 1280\n")).is_err());
}

#[test]
fn a_request_names_an_address_or_a_callsign_for_its_span_only() {
    let text = format!(
        "{DEV_CFG}\nmsaw_request = addr:a1b2c3 10 100\nmsaw_request = call:ual123 200 300\n"
    );
    let set = parse_config(&text).unwrap();
    assert_eq!(set.requests.len(), 2);
    assert_eq!(set.requests[0].who, Requester::Address(0x00A1_B2C3));
    assert_eq!(set.requests[1].who, Requester::Callsign("UAL123".into()));
    let r = &set.requests;
    let cs = CallsignObs::Stable(*b"UAL123\0\0");
    assert!(requested(r, 0xA1B2C3, CallsignObs::Absent, 50.0));
    assert!(!requested(r, 0xA1B2C3, CallsignObs::Absent, 100.0));
    assert!(!requested(r, 0xA1B2C4, CallsignObs::Absent, 50.0));
    assert!(requested(r, 1, cs, 250.0));
    assert!(!requested(r, 1, cs, 150.0));
    assert!(!requested(r, 1, CallsignObs::Unstable, 250.0));
    assert!(!requested(
        r,
        1,
        CallsignObs::Stable(*b"DAL9\0\0\0\0"),
        250.0
    ));
    for bad in [
        "msaw_request = a1b2c3 10 100",
        "msaw_request = addr:a1b2 10 100",
        "msaw_request = call:TOOLONGCALL 10 100",
        "msaw_request = addr:a1b2c3 100 10",
        "msaw_request = addr:a1b2c3 10",
    ] {
        assert!(
            parse_config(&format!("{DEV_CFG}\n{bad}\n")).is_err(),
            "{bad}"
        );
    }
    // The departure clearance is a required key.
    let without: String = DEV_CFG
        .lines()
        .filter(|l| !l.starts_with("departure_min_clearance_ft"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(parse_config(&without)
        .unwrap_err()
        .contains("departure_min_clearance_ft"));
}

fn alert(addr: u32, start_s: i64) -> Alert {
    Alert {
        addr,
        start_ns: start_s * 1_000_000_000,
        end_ns: start_s * 1_000_000_000,
        caution_ns: None,
        warning_ns: None,
        peak: Grade::Caution,
        lat_e7: 0,
        lon_e7: 0,
        alt_ft: 0.0,
        vrate_fpm: 0.0,
        speed_mps: 0.0,
        worst_margin_ft: -10.0,
        worst_floor_ft: 0,
        worst_kind: FloorKind::Mva,
        worst_lat_e7: 0,
        worst_lon_e7: 0,
        worst_radius_m: 0.0,
        worst_k: 0,
        worst_ns: 0,
        worst_flags: 0,
        r95_m: 0.0,
        vel_east_mps: 0.0,
        vel_north_mps: 0.0,
        ttv_min_s: 0.0,
        track_deg: -1,
        worst_kinds: 0,
        cycles: 1,
        flags: 0,
        end_reason: 0,
        protect_ns: 0,
    }
}

#[test]
fn with_no_flight_data_every_alert_stands_and_is_said_not_to_be_judged() {
    let alerts = [alert(1, 10), alert(2, 20)];
    let tl = Default::default();
    let reqs = [RequestSpec {
        who: Requester::Address(2),
        start_s: 0.0,
        end_s: 1000.0,
    }];
    let mut sealed = 0;
    let r = judge(
        &Inputs {
            alerts: &alerts,
            tracks: &[(1, 0, 0, 0), (2, 0, 0, 0)],
            callsigns: &tl,
            requests: &reqs,
            idents: &Default::default(),
            vfr_codes: &[1200],
            t0_ns: 0,
            sfdps: None,
        },
        &mut sealed,
    )
    .unwrap();
    assert!(!r.judged);
    assert!(r.advisories.is_empty());
    assert_eq!(r.basis.len(), 2);
    assert_eq!(r.alerts_by[Basis::NotJudged as usize], 1);
    assert_eq!(r.alerts_by[Basis::Request as usize], 1);
    assert_eq!(r.tracks, 2);
}
