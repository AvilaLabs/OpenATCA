#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use oa_legacy_traj::core::trajectory::{
    FlightKey, PerformanceEnvelope, PerformanceSource, PerformanceStatus,
};
use oa_regional_session::cli::synthetic_model;
use oa_regional_session::runtime::{
    regional_new, regional_select_automation_mode, regional_step, regional_step_with_performance,
    AutomationDegradation, AutomationModeError, EffectiveAutomationMode, RegionalAutomationMode,
    MAX_PERFORMANCE_ENVELOPES,
};
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

#[test]
fn outage_mode_preserves_flight_data_and_recovers_on_a_fresh_surveillance_measurement() {
    let mut engine = runtime();
    let first = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: SECOND,
            source_clock_regressions: 0,
            events: vec![
                row(
                    SECOND,
                    1,
                    "SURVEILLANCE",
                    0,
                    0xabc101,
                    "POS\t410000000\t-870000000\t10000\t0\t0\t0",
                ),
                row(SECOND, 1, "FLIGHT", 1, 0xabc101, "PLAN\tPLAN00000001\t2"),
                row(SECOND, 2, "FLIGHT", 1, 0xabc101, "OWNER\tSECTOR1"),
            ],
        },
    )
    .unwrap();
    assert_eq!(
        first.automation.effective,
        EffectiveAutomationMode::Surveillance
    );
    assert!(first.automation.fusion_off);
    assert_eq!(first.flight_report.flights, 1);

    let current = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 2 * SECOND,
            source_clock_regressions: 0,
            events: vec![row(
                2 * SECOND,
                2,
                "SURVEILLANCE",
                0,
                0xabc101,
                "POS\t410000000\t-870000000\t10000\t0\t0\t0",
            )],
        },
    )
    .unwrap();
    assert_eq!(
        current.automation.effective,
        EffectiveAutomationMode::Surveillance
    );
    assert!(!current.surveillance.tracks.is_empty());

    let outage = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 3 * SECOND,
            source_clock_regressions: 0,
            events: vec![
                row(3 * SECOND, 3, "SURVEILLANCE", 0, 0, "FEED\tDOWN\t0"),
                row(3 * SECOND, 3, "FLIGHT", 1, 0xabc101, "HO_OFFER\tSECTOR2"),
            ],
        },
    )
    .unwrap();
    assert_eq!(
        outage.automation.effective,
        EffectiveAutomationMode::FlightDataOnly
    );
    assert_eq!(
        outage.automation.degradation,
        AutomationDegradation::SurveillanceUnavailable
    );
    assert!(outage.surveillance.surveillance_withheld);
    assert!(outage.surveillance.tracks.is_empty());
    assert!(outage.surveillance.withheld_tracks > 0);
    assert_eq!(outage.flight_report.flights, 1);
    assert!(outage.flight_operations[1].operation.is_some());
    assert_eq!(outage.flight_data.len(), 1);
    assert_eq!(outage.flight_data_unavailable, 0);
    assert_eq!(outage.flight_data[0].handoff_to, *b"SECTOR2\0");

    let recovered = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 4 * SECOND,
            source_clock_regressions: 0,
            events: vec![row(
                4 * SECOND,
                4,
                "SURVEILLANCE",
                0,
                0xabc101,
                "POS\t410000000\t-870000000\t10000\t0\t0\t0",
            )],
        },
    )
    .unwrap();
    assert_eq!(
        recovered.automation.effective,
        EffectiveAutomationMode::Surveillance
    );
    assert_eq!(
        recovered.automation.degradation,
        AutomationDegradation::None
    );
    assert!(!recovered.surveillance.surveillance_withheld);
    assert_eq!(recovered.flight_report.flights, 1);
}

#[test]
fn ca_alert_history_is_retained_with_an_explicit_unavailable_product_status() {
    let mut engine = runtime();
    let active = confirmed(&mut engine);
    assert_eq!(
        active.surveillance.ca_product,
        oa_regional_session::surveillance::ProductAvailability::Available
    );
    let start_ns = active.surveillance.ca_open[0].start_ns;

    let outage = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 3 * SECOND,
            source_clock_regressions: 0,
            events: vec![row(3 * SECOND, 5, "SURVEILLANCE", 0, 0, "FEED\tDOWN\t0")],
        },
    )
    .unwrap();
    assert_eq!(
        outage.surveillance.ca_product,
        oa_regional_session::surveillance::ProductAvailability::Withheld(
            oa_regional_session::surveillance::ProductUnavailableReason::SurveillanceFeedUnavailable
        )
    );
    assert!(outage.surveillance.withheld_ca_records > 0);
    assert!(outage
        .surveillance
        .ca_open
        .iter()
        .chain(&outage.surveillance.ca_closed)
        .any(|alert| alert.start_ns == start_ns));

    let recovered = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 4 * SECOND,
            source_clock_regressions: 0,
            events: vec![
                row(
                    4 * SECOND,
                    6,
                    "SURVEILLANCE",
                    1,
                    0xabc101,
                    "POS\t410000000\t-869988000\t10000\t100000\t0\t0",
                ),
                row(
                    4 * SECOND,
                    7,
                    "SURVEILLANCE",
                    2,
                    0xabc102,
                    "POS\t410000000\t-869912000\t10000\t-100000\t0\t0",
                ),
            ],
        },
    )
    .unwrap();
    assert_eq!(
        recovered.surveillance.ca_product,
        oa_regional_session::surveillance::ProductAvailability::Available
    );
}

#[test]
fn unsupported_fusion_selection_is_refused_and_reported_off() {
    let mut engine = runtime();
    assert_eq!(
        regional_select_automation_mode(&mut engine, RegionalAutomationMode::Fusion),
        Err(AutomationModeError::FusionUnavailable)
    );
    let frame = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: SECOND,
            source_clock_regressions: 0,
            events: Vec::new(),
        },
    )
    .unwrap();
    assert_eq!(
        frame.automation.selected,
        RegionalAutomationMode::Surveillance
    );
    assert!(frame.automation.fusion_off);
}

#[test]
fn non_surveillance_feed_state_does_not_claim_aggregate_track_health() {
    let mut engine = runtime();
    let live = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: SECOND,
            source_clock_regressions: 0,
            events: vec![row(
                SECOND,
                1,
                "SURVEILLANCE",
                0,
                0xabc101,
                "POS\t410000000\t-870000000\t10000\t0\t0\t0",
            )],
        },
    )
    .unwrap();
    assert_eq!(
        live.automation.effective,
        EffectiveAutomationMode::Surveillance
    );

    let unrelated_feed_loss = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 2 * SECOND,
            source_clock_regressions: 0,
            events: vec![row(2 * SECOND, 2, "FLIGHT", 0, 0, "FEED\tDOWN\t0")],
        },
    )
    .unwrap();
    assert_eq!(
        unrelated_feed_loss.automation.effective,
        EffectiveAutomationMode::Surveillance
    );
}

#[test]
fn explicit_flight_data_only_selection_remains_selected_without_sensor_health() {
    let mut engine = runtime();
    regional_select_automation_mode(&mut engine, RegionalAutomationMode::FlightDataOnly).unwrap();
    let first = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: SECOND,
            source_clock_regressions: 0,
            events: vec![row(
                SECOND,
                1,
                "SURVEILLANCE",
                0,
                0xabc101,
                "POS\t410000000\t-870000000\t10000\t0\t0\t0",
            )],
        },
    )
    .unwrap();
    assert_eq!(
        first.automation.selected,
        RegionalAutomationMode::FlightDataOnly
    );
    assert_eq!(
        first.automation.effective,
        EffectiveAutomationMode::FlightDataOnly
    );
    assert_eq!(
        first.automation.degradation,
        AutomationDegradation::OperatorSelectedFlightDataOnly
    );
    let frame = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 2 * SECOND,
            source_clock_regressions: 0,
            events: vec![row(
                2 * SECOND,
                2,
                "SURVEILLANCE",
                0,
                0xabc101,
                "POS\t410000000\t-870000000\t10000\t0\t0\t0",
            )],
        },
    )
    .unwrap();
    assert!(frame.surveillance.surveillance_withheld);
    assert!(frame.surveillance.tracks.is_empty());
    assert!(frame.surveillance.withheld_tracks > 0);
    regional_select_automation_mode(&mut engine, RegionalAutomationMode::Surveillance).unwrap();
    let surveillance_unavailable = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 8 * SECOND,
            source_clock_regressions: 0,
            events: Vec::new(),
        },
    )
    .unwrap();
    assert_eq!(
        surveillance_unavailable.automation.selected,
        RegionalAutomationMode::Surveillance
    );
    assert_eq!(
        surveillance_unavailable.automation.effective,
        EffectiveAutomationMode::FlightDataOnly
    );
    assert_eq!(
        surveillance_unavailable.automation.degradation,
        AutomationDegradation::SurveillanceUnavailable
    );
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
    runtime_with_model(synthetic_model().unwrap())
}

fn runtime_with_model(
    model: oa_airspace::core::model::Model,
) -> oa_regional_session::runtime::RegionalRuntime {
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

fn intern(
    model: &mut oa_airspace::core::model::Model,
    text: &str,
) -> oa_airspace::core::model::StrIx {
    let raw = model.strings.offsets.len().checked_sub(1).unwrap();
    let index = oa_airspace::core::model::StrIx::new(u32::try_from(raw).unwrap());
    model.strings.text.push_str(text);
    model
        .strings
        .offsets
        .push(u32::try_from(model.strings.text.len()).unwrap());
    index
}

fn insert_bit(bits: &mut oa_airspace::core::model::BitSet, index: usize, value: bool) {
    let old_len = usize::try_from(bits.len).unwrap();
    let old_values: Vec<_> = (0..old_len)
        .map(|row| {
            bits.words
                .get(row / 64)
                .is_some_and(|word| (word >> (row % 64)) & 1 != 0)
        })
        .collect();
    let mut rebuilt = oa_airspace::core::model::BitSet::default();
    for row in 0..=old_len {
        oa_airspace::core::model::bit_push(
            &mut rebuilt,
            if row == index {
                value
            } else {
                old_values[if row < index { row } else { row - 1 }]
            },
        );
    }
    *bits = rebuilt;
}

fn repeated_name_model() -> oa_airspace::core::model::Model {
    let mut model = synthetic_model().unwrap();
    let ident = intern(&mut model, "FIXA");
    let region = intern(&mut model, "K2");
    let points = &mut model.points;
    points
        .kind
        .insert(1, oa_airspace::core::model::point_kind::FIX);
    points.lat.insert(1, 420_000_000);
    points.lon.insert(1, -860_000_000);
    points
        .elev_ft
        .insert(1, oa_airspace::core::model::ELEV_NONE);
    points.ident.insert(1, ident);
    points
        .alt_ident
        .insert(1, oa_airspace::core::model::StrIx::NONE);
    points.region.insert(1, region);
    points
        .scope
        .insert(1, oa_airspace::core::model::StrIx::NONE);
    points.name.insert(1, oa_airspace::core::model::StrIx::NONE);
    for (column, value) in [
        (&mut points.src_nasr, false),
        (&mut points.src_cifp, false),
        (&mut points.in_area, true),
        (&mut points.cls_b, false),
        (&mut points.cls_c, false),
        (&mut points.cls_d, false),
        (&mut points.cls_e, false),
    ] {
        insert_bit(column, 1, value);
    }
    model
}

fn ambiguous_airway_model() -> oa_airspace::core::model::Model {
    use oa_airspace::core::model as m;

    let mut model = synthetic_model().unwrap();
    let fix_c = intern(&mut model, "FIXC");
    let fix_d = intern(&mut model, "FIXD");
    let region = intern(&mut model, "K1");
    let airway = intern(&mut model, "V1");
    for (lat, lon, ident) in [
        (415_000_000, -865_000_000, fix_c),
        (405_000_000, -855_000_000, fix_d),
    ] {
        let points = &mut model.points;
        points.kind.push(m::point_kind::FIX);
        points.lat.push(lat);
        points.lon.push(lon);
        points.elev_ft.push(m::ELEV_NONE);
        points.ident.push(ident);
        points.alt_ident.push(m::StrIx::NONE);
        points.region.push(region);
        points.scope.push(m::StrIx::NONE);
        points.name.push(m::StrIx::NONE);
        for (column, value) in [
            (&mut points.src_nasr, false),
            (&mut points.src_cifp, false),
            (&mut points.in_area, true),
            (&mut points.cls_b, false),
            (&mut points.cls_c, false),
            (&mut points.cls_d, false),
            (&mut points.cls_e, false),
        ] {
            m::bit_push(column, value);
        }
    }
    model.airways.designator.push(airway);
    model.airways.location.push(b'C');
    model.airways.legs.push(m::Span {
        start: m::AirwayLegIx::new(0),
        len: 4,
    });
    for (from, to) in [(0, 2), (2, 1), (0, 3), (3, 1)] {
        let legs = &mut model.airway_legs;
        legs.from.push(m::PointIx::new(from));
        legs.to.push(m::PointIx::new(to));
        legs.mea_ft.push(m::ALT_NONE);
        legs.mea_opp_ft.push(m::ALT_NONE);
        legs.gps_mea_ft.push(m::ALT_NONE);
        legs.mea_dir.push(m::compass::NONE);
        legs.gps_mea_dir.push(m::compass::NONE);
        m::bit_push(&mut legs.gap, false);
        m::bit_push(&mut legs.signal_gap, false);
        m::bit_push(&mut legs.dogleg, false);
    }
    model
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

fn set_position_altitude(batch: &mut ReceiptBatch, altitude_ft: i32) {
    for event in &mut batch.events {
        if let oa_regional_session::EventKind::Position {
            altitude_ft: field, ..
        } = &mut event.kind
        {
            *field = Some(altitude_ft);
        }
    }
}

// REQ: OA-REG-015
#[test]
fn selection_and_console_replace_only_controls_and_keep_the_observation_epoch() {
    use oa_regional_session::runtime::{
        regional_console_overlay, regional_pixels, regional_select,
    };
    let mut engine = runtime();
    let frame = confirmed(&mut engine);
    let view = frame
        .controller_views
        .iter()
        .find(|view| view.address == 0xabc101)
        .unwrap();
    let selected = regional_select(
        &mut engine,
        view.address,
        view.generation,
        Some(view.identity),
    )
    .unwrap();
    for _ in 0..100 {
        let edited = regional_console_overlay(&mut engine, "INPUT ASSIGNED 12000").unwrap();
        assert_ne!(edited, selected);
        assert_eq!(regional_console_overlay(&mut engine, "").unwrap(), selected);
    }
    let before = regional_pixels(&engine).unwrap().to_vec();
    assert!(regional_console_overlay(&mut engine, "bad\ninput").is_err());
    assert_eq!(regional_pixels(&engine).unwrap(), before);
    let next = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 3 * SECOND,
            source_clock_regressions: 0,
            events: Vec::new(),
        },
    )
    .unwrap();
    assert_eq!(next.controller_selected_address, Some(view.address));
    assert_eq!(next.controller_selection, Some(view.identity));
    assert_eq!(next.surveillance.time_ns, 3 * SECOND);
    assert_eq!(next.surveillance.ca_stats.tracks, 2);
    assert!(regional_select(
        &mut engine,
        view.address,
        view.generation + 1,
        Some(view.identity)
    )
    .is_err());
    assert!(
        regional_pixels(&engine).is_ok(),
        "ordinary selection refusal remains visible"
    );
    let next = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 4 * SECOND,
            source_clock_regressions: 0,
            events: Vec::new(),
        },
    )
    .unwrap();
    assert_eq!(next.controller_selected_address, None);
    assert_eq!(next.controller_selection, None);
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

// REQ: OA-REG-018
#[test]
fn la2014_extreme_altitude_survives_malformed_route_quarantine() {
    let mut engine = runtime();
    let mut initial = start();
    set_position_altitude(&mut initial, 100_000);
    let tentative = regional_step(&mut engine, &initial).unwrap();
    assert!(tentative.surveillance.tracks.is_empty());

    let mut confirmation = confirmation_batch();
    set_position_altitude(&mut confirmation, 100_000);
    let retained = regional_step(&mut engine, &confirmation).unwrap();
    assert!(retained.surveillance.displayed > 0);
    let high_track = retained
        .surveillance
        .tracks
        .iter()
        .find(|track| track.address == 0xabc101)
        .unwrap();
    assert_eq!(high_track.altitude_ft, 100_000);

    let frame = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 3 * SECOND,
            source_clock_regressions: 0,
            events: vec![
                row(
                    3 * SECOND,
                    5,
                    "ROUTE",
                    1,
                    0xabc101,
                    "FILED_ROUTE\tPLAN00000001\tFIXB\tTHIS IS NOT A ROUTE",
                ),
                row(
                    3 * SECOND,
                    5,
                    "SURVEILLANCE",
                    1,
                    0xabc101,
                    "POS\t410000000\t-869988000\t100000\t100000\t0\t0",
                ),
            ],
        },
    )
    .expect("a known malformed route item must not discard its healthy neighbor");

    assert_eq!(frame.quarantine.records.len(), 1);
    let refused = &frame.quarantine.records[0];
    assert_eq!(
        refused.identity.source,
        oa_quarantine::InputSource::FlightData
    );
    assert_eq!(refused.identity.source_instance, 1);
    assert_eq!(refused.identity.item_id, 3_u64 << 8);
    assert_eq!(
        refused.reasons.0,
        oa_quarantine::REASON_ALTITUDE_ROUTE_INVALID
    );
    assert_eq!(frame.input.input_events, 2);
    assert_eq!(frame.input.source_events[0], 1);
    assert_eq!(frame.quarantine.source_stats[0].queued, 1);
    assert_eq!(frame.quarantine.source_stats[1].accepted, 5);
    assert_eq!(frame.quarantine.source_stats[1].queued, 0);
    let later = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 4 * SECOND,
            source_clock_regressions: 0,
            events: vec![row(
                4 * SECOND,
                6,
                "ROUTE",
                1,
                0xabc101,
                &format!("FILED_ROUTE\tPLAN00000001\tFIXB\t{ROUTE}"),
            )],
        },
    )
    .expect("later valid route revision should restore availability for its cutoff");
    assert_eq!(later.quarantine.records.len(), 0);
    assert_eq!(later.quarantine.source_stats[0].queued, 1);
    assert_eq!(later.quarantine.source_stats[0].accepted, 13);
}

// REQ: OA-REG-018
#[test]
fn nats2023_repeated_local_name_resolves_with_full_region_identity_at_runtime() {
    let mut engine = runtime_with_model(repeated_name_model());
    confirmed(&mut engine);
    let frame = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 3 * SECOND,
            source_clock_regressions: 0,
            events: vec![
                row(
                    3 * SECOND,
                    5,
                    "SURVEILLANCE",
                    1,
                    0xabc101,
                    "POS\t410000000\t-869988000\t65000\t100000\t0\t0",
                ),
                row(
                    3 * SECOND,
                    5,
                    "ROUTE",
                    1,
                    0xabc101,
                    "FILED_ROUTE\tPLAN00000001\tFIXB\tFIX:FIXA:K2:1:- FIX:FIXB:K1:1:-",
                ),
            ],
        },
    )
    .expect("full route identity and high but supported altitude remain admissible");

    assert_eq!(frame.quarantine.records.len(), 0);
    assert_eq!(frame.input.source_events, [1, 0, 1, 0, 0]);
    assert_eq!(frame.quarantine.source_stats[0].accepted, 13);
    assert_eq!(frame.quarantine.source_stats[0].queued, 0);
    assert_eq!(frame.quarantine.source_stats[1].accepted, 5);
    assert_eq!(frame.quarantine.source_stats[1].queued, 0);
}

// REQ: OA-REG-018
#[test]
fn ambiguous_route_is_quarantined_and_following_healthy_item_commits() {
    let mut engine = runtime_with_model(ambiguous_airway_model());
    confirmed(&mut engine);
    let frame = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 3 * SECOND,
            source_clock_regressions: 0,
            events: vec![
                row(
                    3 * SECOND,
                    5,
                    "ROUTE",
                    1,
                    0xabc101,
                    "FILED_ROUTE\tPLAN00000001\tFIXB\tAWY:V1:C:FIXA:K1:1:-:FIXB:K1:1:-:12000",
                ),
                row(
                    3 * SECOND,
                    5,
                    "SURVEILLANCE",
                    1,
                    0xabc101,
                    "POS\t410000000\t-869988000\t10000\t100000\t0\t0",
                ),
            ],
        },
    )
    .expect("ambiguous route is isolated and the next normalized item is handled");

    assert_eq!(frame.quarantine.records.len(), 1);
    assert_eq!(
        frame.quarantine.records[0].reasons.0,
        oa_quarantine::REASON_WAYPOINT_NAME_AMBIGUOUS
    );
    assert_eq!(frame.quarantine.records[0].identity.item_id, 3_u64 << 8);
    assert_eq!(frame.quarantine.source_stats[0].queued, 1);
    assert_eq!(frame.quarantine.source_stats[1].accepted, 5);
    assert_eq!(frame.quarantine.source_stats[1].queued, 0);

    let later = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 4 * SECOND,
            source_clock_regressions: 0,
            events: vec![row(
                4 * SECOND,
                6,
                "ROUTE",
                1,
                0xabc101,
                &format!("FILED_ROUTE\tPLAN00000001\tFIXB\t{ROUTE}"),
            )],
        },
    )
    .expect("a later valid amendment remains acceptable after the fenced refusal");
    assert_eq!(later.quarantine.records.len(), 0);
    assert_eq!(later.quarantine.source_stats[0].queued, 1);
    assert_eq!(later.quarantine.source_stats[0].accepted, 13);
}

// REQ: OA-REG-018
#[test]
fn staged_flight_adapter_refusal_does_not_advance_or_block_following_operation() {
    let mut engine = runtime();
    confirmed(&mut engine);
    let frame = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 3 * SECOND,
            source_clock_regressions: 0,
            events: vec![
                row(
                    3 * SECOND,
                    9,
                    "FLIGHT",
                    1,
                    0xabc101,
                    "FLIGHT\tOWNER\tSECTOR2",
                ),
                row(
                    3 * SECOND,
                    10,
                    "FLIGHT",
                    1,
                    0xabc101,
                    "FLIGHT\tASSIGNED\t13000",
                ),
            ],
        },
    )
    .expect("a staged adapter refusal must permit a following valid operation");

    assert_eq!(frame.quarantine.records.len(), 1);
    assert_eq!(frame.quarantine.records[0].identity.item_id, 3_u64 << 8);
    assert_eq!(
        frame.quarantine.records[0].reasons.0,
        oa_quarantine::REASON_INPUT_INVALID
    );
    assert_eq!(frame.quarantine.source_stats[0].queued, 1);
    assert_eq!(frame.quarantine.source_stats[0].accepted, 13);
    assert_eq!(frame.flight_report.adapter.owner_change_requires_take, 1);
    let flight = frame
        .flights
        .iter()
        .find(|flight| flight.address == 0xabc101)
        .unwrap();
    let view = flight.view.unwrap();
    assert_eq!(view.owner_unit, *b"SECTOR1\0");
    assert_eq!(view.assigned.form, oa_flight::ALT_SIMPLE);
    assert_eq!(view.assigned.a_ft, 13_000);
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
    let baseline_first = baseline
        .flights
        .iter()
        .find(|f| f.address == 0xabc101)
        .unwrap();
    let opt_in_first = opt_in
        .flights
        .iter()
        .find(|f| f.address == 0xabc101)
        .unwrap();
    assert_eq!(baseline_first.trajectory, opt_in_first.trajectory);
    let profile = opt_in_first.vertical_profile.unwrap();
    assert_eq!(profile.performance, envelope);
    assert_eq!(profile.key, envelope.key);
    assert_eq!(
        profile.availability,
        oa_legacy_traj::core::trajectory::Availability::Available
    );
    assert_eq!(
        profile.points[0].availability,
        oa_legacy_traj::core::trajectory::Availability::Available
    );
    assert_eq!(profile.points[0].reachable_min_ft, 10_000);
    assert!(profile.points[0].reachable_max_ft >= 10_000);
    assert!(opt_in_first.vertical_refusal.is_empty());

    let other_flight = opt_in
        .flights
        .iter()
        .find(|f| f.address == 0xabc102)
        .unwrap();
    assert!(other_flight.vertical_profile.is_none());
    assert!(other_flight.vertical_refusal.contains("full flight key"));

    let disabled = confirmed_with_performance(&[]);
    for expected in &baseline.flights {
        let actual = disabled
            .flights
            .iter()
            .find(|f| f.address == expected.address)
            .unwrap();
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
    let mismatch = mismatch_frame
        .flights
        .iter()
        .find(|f| f.address == 0xabc101)
        .unwrap();
    assert!(mismatch.trajectory.is_some());
    assert!(mismatch.vertical_profile.is_none());
    assert!(mismatch.vertical_refusal.contains("full flight key"));

    let envelope = performance_for(first_flight_key());
    let duplicate_frame = confirmed_with_performance(&[envelope, envelope]);
    let duplicate = duplicate_frame
        .flights
        .iter()
        .find(|f| f.address == 0xabc101)
        .unwrap();
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
