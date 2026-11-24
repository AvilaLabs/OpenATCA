#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use oa_regional_session::cli::synthetic_model;
use oa_regional_session::runtime::{regional_new, regional_step};
use oa_regional_session::surveillance::SurveillanceConfig;
use oa_regional_session::{parse_event_line, ReceiptBatch};

const SECOND: i64 = 1_000_000_000;
const ROUTE: &str = "FIX:FIXA:K1:1:- DCT:FIXB:K1:1:-";
const ROUTE_HASH: &str = "2f6fb15ca1faf3d7d3f4f082096b670efc91dbe6f4d44c0ac4a57c713e181840";

// REQ: OA-REG-002, OA-REG-003, OA-UI-006
#[test]
fn raw_surveillance_needs_no_invented_flight_identity() {
    let mut engine = runtime();
    let mut latest = None;
    for second in [1, 2] {
        latest = Some(
            regional_step(
                &mut engine,
                &ReceiptBatch {
                    receipt_ns: second * SECOND,
                    source_clock_regressions: 0,
                    events: vec![row(
                        second * SECOND,
                        second as u64,
                        "SURVEILLANCE",
                        0,
                        0xabc101,
                        "POS\t410000000\t-870000000\t10000\t0\t0\t0",
                    )],
                },
            )
            .unwrap(),
        );
    }
    let frame = latest.unwrap();
    assert_eq!(frame.surveillance.tracks.len(), 1);
    assert_eq!(frame.surveillance.displayed, 1);
    assert_eq!(frame.controller_views[0].identity_present, 0);
    assert_eq!(frame.controller_views[0].identity_claims, 0);
    assert!(frame.flights[0].trajectory.is_none());
    assert!(frame.flights[0].refusal.contains("UnknownFlight"));
    assert_eq!(frame.flight_report.flights, 0);
}

fn row(
    t: i64,
    seq: u64,
    source: &str,
    flight: u128,
    address: u32,
    body: &str,
) -> oa_regional_session::NormalizedEvent {
    parse_event_line(&format!(
        "v1\t{t}\t{t}\t{seq}\t{source}\t{flight:032X}\t{address:06X}\tZAU1\t{body}"
    ))
    .expect("valid synthetic row")
}
fn start() -> ReceiptBatch {
    let mut events = vec![
        row(
            SECOND,
            1,
            "SURVEILLANCE",
            1,
            0xabc101,
            "POS\t410000000\t-870000000\t10000\t100000\t0\t0",
        ),
        row(
            SECOND,
            2,
            "SURVEILLANCE",
            2,
            0xabc102,
            "POS\t410000000\t-869900000\t10000\t-100000\t0\t0",
        ),
    ];
    let mut sequence = 1;
    for (flight, address, plan) in [(1, 0xabc101, "PLAN00000001"), (2, 0xabc102, "PLAN00000002")] {
        for body in [
            format!("FLIGHT\tPLAN\t{plan}\t2"),
            "FLIGHT\tADDRESS".into(),
            "FLIGHT\tASSIGNED\t12000".into(),
            "FLIGHT\tOWNER\tSECTOR1".into(),
        ] {
            events.push(row(SECOND, sequence, "FLIGHT", flight, address, &body));
            sequence += 1;
        }
    }
    sequence = 1;
    for (flight, address, plan) in [(1, 0xabc101, "PLAN00000001"), (2, 0xabc102, "PLAN00000002")] {
        events.push(row(
            SECOND,
            sequence,
            "ROUTE",
            flight,
            address,
            &format!("FILED_ROUTE\t{plan}\tFIXB\t{ROUTE}"),
        ));
        sequence += 1;
        events.push(row(
            SECOND,
            sequence,
            "ROUTE",
            flight,
            address,
            &format!("CURSOR\t{plan}\t{ROUTE_HASH}\tFIX:FIXB:K1:1:-"),
        ));
        sequence += 1;
    }
    ReceiptBatch {
        receipt_ns: SECOND,
        source_clock_regressions: 0,
        events,
    }
}
fn runtime() -> oa_regional_session::runtime::RegionalRuntime {
    let model = synthetic_model().unwrap();
    let bytes = oa_airspace::core::snapshot::encode(&model).unwrap();
    regional_new(
        model,
        oa_sha256::sha256(&bytes),
        *b"ZAU1",
        SurveillanceConfig {
            centre_lat_e7: 410000000,
            centre_lon_e7: -870000000,
            range_nm: 180.0,
        },
    )
    .unwrap()
}

fn confirmed(
    engine: &mut oa_regional_session::runtime::RegionalRuntime,
) -> oa_regional_session::runtime::RegionalFrame {
    let tentative = regional_step(engine, &start()).unwrap();
    assert!(
        tentative.surveillance.tracks.is_empty(),
        "one report is intentionally tentative"
    );
    let batch = ReceiptBatch {
        receipt_ns: 2 * SECOND,
        source_clock_regressions: 0,
        events: vec![
            row(
                2 * SECOND,
                3,
                "SURVEILLANCE",
                1,
                0xabc101,
                "POS\t410000000\t-869988000\t10000\t100000\t0\t0",
            ),
            row(
                2 * SECOND,
                4,
                "SURVEILLANCE",
                2,
                0xabc102,
                "POS\t410000000\t-869912000\t10000\t-100000\t0\t0",
            ),
        ],
    };
    regional_step(engine, &batch).unwrap()
}

// REQ: OA-REG-004
#[test]
fn actual_tracker_flight_route_ca_and_scope_share_one_cutoff() {
    let frame = confirmed(&mut runtime());
    assert_eq!(frame.surveillance.tracks.len(), 2);
    assert_eq!(frame.surveillance.displayed, 2);
    assert_eq!(frame.surveillance.time_ns, 2 * SECOND);
    assert_eq!(frame.flights.len(), 2);
    for flight in &frame.flights {
        assert!(flight.refusal.is_empty(), "{}", flight.refusal);
        let view = flight.view.unwrap();
        assert_eq!(view.address, flight.address);
        let forecast = flight.trajectory.unwrap();
        assert_eq!(forecast.key.aircraft_address, flight.address);
        assert_eq!(forecast.cutoff_ms, 2000);
        assert_eq!(
            forecast.horizons[0].altitude_ft, 10000,
            "a level observation must not teleport to its clearance"
        );
        assert_ne!(forecast.horizons[0].lat_e7, i32::MIN);
    }
    assert!(
        !frame.surveillance.ca_open.is_empty(),
        "two closing aircraft are already inside development minima"
    );
}

// REQ: OA-REG-005
#[test]
fn replayed_prefix_is_identical_and_future_amendments_do_not_change_it() {
    let mut first = runtime();
    let before = confirmed(&mut first);
    let amendment = ReceiptBatch {
        receipt_ns: 3 * SECOND,
        source_clock_regressions: 0,
        events: vec![row(
            3 * SECOND,
            9,
            "FLIGHT",
            1,
            0xabc101,
            "FLIGHT\tUNASSIGNED",
        )],
    };
    let later = regional_step(&mut first, &amendment).unwrap();
    assert_eq!(
        later.flights[0].view.unwrap().assigned.form,
        oa_flight::ALT_NIL
    );
    let independently_reconstructed = confirmed(&mut runtime());
    assert_eq!(before, independently_reconstructed);
}

// REQ: OA-REG-006
#[test]
fn route_plan_mismatch_blocks_old_route_and_failed_frame_requires_reconstruction() {
    let mut engine = runtime();
    confirmed(&mut engine);
    let changed = ReceiptBatch {
        receipt_ns: 3 * SECOND,
        source_clock_regressions: 0,
        events: vec![row(
            3 * SECOND,
            9,
            "FLIGHT",
            1,
            0xabc101,
            "FLIGHT\tPLAN\tPLAN00000003\t2",
        )],
    };
    let frame = regional_step(&mut engine, &changed).unwrap();
    assert!(frame.flights[0]
        .refusal
        .contains("does not match frozen flight plan"));
    let invalid = ReceiptBatch {
        receipt_ns: 3 * SECOND,
        source_clock_regressions: 0,
        events: Vec::new(),
    };
    assert!(regional_step(&mut engine, &invalid).is_err());
    let next = ReceiptBatch {
        receipt_ns: 4 * SECOND,
        ..invalid
    };
    assert!(regional_step(&mut engine, &next)
        .unwrap_err()
        .contains("reconstruct"));
}

// REQ: OA-REG-007
#[test]
fn feed_loss_is_visible_without_erasing_or_reidentifying_traffic() {
    let mut engine = runtime();
    confirmed(&mut engine);
    let loss = ReceiptBatch {
        receipt_ns: 6 * SECOND,
        source_clock_regressions: 0,
        events: vec![row(
            6 * SECOND,
            5,
            "SURVEILLANCE",
            1,
            0xabc101,
            "FEED\tDOWN\t1",
        )],
    };
    let frame = regional_step(&mut engine, &loss).unwrap();
    assert_eq!(frame.surveillance.tracks.len(), 2);
    assert!(
        frame
            .surveillance
            .warnings
            .iter()
            .any(|text| text.contains("TRACK") && text.contains("LOST")),
        "{:?}",
        frame.surveillance.warnings
    );
}
