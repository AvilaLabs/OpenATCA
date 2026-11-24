#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use oa_regional_session::cli::synthetic_model;
use oa_regional_session::runtime::{
    regional_new, regional_step, regional_step_with_performance, MAX_PERFORMANCE_ENVELOPES,
};
use oa_regional_session::surveillance::SurveillanceConfig;
use oa_regional_session::{parse_event_line, ReceiptBatch};
use oa_legacy_traj::core::trajectory::{
    FlightKey, PerformanceEnvelope, PerformanceSource, PerformanceStatus,
};

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
    regional_step(engine, &confirmation_batch()).unwrap()
}

fn confirmation_batch() -> ReceiptBatch {
    ReceiptBatch {
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
    }
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

fn first_flight_key() -> FlightKey {
    FlightKey {
        gufi: 1u128.to_be_bytes(),
        aircraft_address: 0xabc101,
        centre: *b"ZAU1",
        plan_id: *b"PLAN00000001",
    }
}

fn performance_for(key: FlightKey) -> PerformanceEnvelope {
    PerformanceEnvelope {
        key,
        reserved_key_align: [0; 4],
        source_ms: 1_000,
        available_ms: 2_000,
        message_id: 9001,
        max_age_ms: 5_000,
        max_climb_fpm: 600,
        max_descent_fpm: 600,
        max_groundspeed_mmps: 1_000_000,
        reserved_speed_cap: 0,
        source: PerformanceSource::Measured,
        status: PerformanceStatus::Valid,
        reserved: [0; 6],
    }
}

fn confirmed_with_performance(
    performance: &[PerformanceEnvelope],
) -> oa_regional_session::runtime::RegionalFrame {
    let mut engine = runtime();
    regional_step(&mut engine, &start()).unwrap();
    regional_step_with_performance(&mut engine, &confirmation_batch(), performance).unwrap()
}

// REQ: OA-REG-014
#[test]
fn opt_in_uses_full_key_performance_and_preserves_horizontal_only_results() {
    let baseline = confirmed(&mut runtime());
    for flight in &baseline.flights {
        assert!(flight.vertical_profile.is_none());
        assert_eq!(flight.vertical_refusal, "vertical profile not requested");
    }

    let envelope = performance_for(first_flight_key());
    let opt_in = confirmed_with_performance(&[envelope]);
    let baseline_first = baseline.flights.iter().find(|f| f.address == 0xabc101).unwrap();
    let opt_in_first = opt_in.flights.iter().find(|f| f.address == 0xabc101).unwrap();
    assert_eq!(baseline_first.trajectory, opt_in_first.trajectory);
    let profile = opt_in_first.vertical_profile.unwrap();
    assert_eq!(profile.performance, envelope);
    assert_eq!(profile.key, envelope.key);
    assert_eq!(profile.availability, oa_legacy_traj::core::trajectory::Availability::Available);
    assert_eq!(profile.points[0].availability, oa_legacy_traj::core::trajectory::Availability::Available);
    assert_eq!(profile.points[0].reachable_min_ft, 10_000);
    assert!(profile.points[0].reachable_max_ft >= 10_000);
    assert!(opt_in_first.vertical_refusal.is_empty());

    let other_flight = opt_in.flights.iter().find(|f| f.address == 0xabc102).unwrap();
    assert!(other_flight.vertical_profile.is_none());
    assert!(other_flight.vertical_refusal.contains("full flight key"));

    let disabled = confirmed_with_performance(&[]);
    for expected in &baseline.flights {
        let actual = disabled.flights.iter().find(|f| f.address == expected.address).unwrap();
        assert_eq!(expected.trajectory, actual.trajectory);
        assert!(actual.vertical_profile.is_none());
    }
}

// REQ: OA-REG-014
#[test]
fn only_one_eligible_exact_full_key_envelope_is_selected() {
    let mut mismatched_key = first_flight_key();
    mismatched_key.plan_id[0] ^= 1;
    let mismatch_frame = confirmed_with_performance(&[performance_for(mismatched_key)]);
    let mismatch = mismatch_frame.flights.iter().find(|f| f.address == 0xabc101).unwrap();
    assert!(mismatch.trajectory.is_some());
    assert!(mismatch.vertical_profile.is_none());
    assert!(mismatch.vertical_refusal.contains("full flight key"));

    let envelope = performance_for(first_flight_key());
    let duplicate_frame = confirmed_with_performance(&[envelope, envelope]);
    let duplicate = duplicate_frame.flights.iter().find(|f| f.address == 0xabc101).unwrap();
    assert!(duplicate.trajectory.is_some());
    assert!(duplicate.vertical_profile.is_none());
    assert!(duplicate.vertical_refusal.contains("duplicate eligible"));
}

// REQ: OA-REG-014
#[test]
fn future_performance_payload_does_not_change_the_eligible_profile_or_baseline() {
    let eligible = performance_for(first_flight_key());
    let expected = confirmed_with_performance(&[eligible]);

    let mut future = eligible;
    future.source_ms = 2_001;
    future.available_ms = 2_002;
    future.key.plan_id = [b'X'; 12];
    future.max_climb_fpm = u32::MAX;
    future.max_descent_fpm = u32::MAX;
    future.max_groundspeed_mmps = u32::MAX;
    future.reserved_key_align = [1; 4];
    future.reserved = [1; 6];
    future.status = PerformanceStatus::Contradictory;
    let actual = confirmed_with_performance(&[eligible, future]);
    assert_eq!(actual, expected);
}

// REQ: OA-REG-014
#[test]
fn over_capacity_performance_slice_refuses_before_runtime_mutation() {
    let mut engine = runtime();
    let envelope = performance_for(first_flight_key());
    let rows = vec![envelope; MAX_PERFORMANCE_ENVELOPES + 1];
    let error = regional_step_with_performance(&mut engine, &start(), &rows).unwrap_err();
    assert!(error.contains("performance envelope slice exceeds"));

    let frame = regional_step(&mut engine, &start()).unwrap();
    assert_eq!(frame.sequence, 1);
    assert!(frame.flights.is_empty());
}
