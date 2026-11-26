#![allow(clippy::expect_used)]

use oa_flight::{ALT_NIL, HO_ACCEPTED, HO_NONE, R_NO_HANDOFF, R_STALE, ST_ACTIVE};
use oa_regional_session::flight::{
    flight_apply_batch, flight_apply_event, flight_driver_capacity, flight_driver_new,
    flight_driver_new_bounded, flight_driver_report, flight_view_for_gufi,
    flight_views_for_address, AdapterRefusal, FlightDriverConfig, FlightDriverError,
    FlightViewStatus, SharedFlightView, MAX_DRIVER_FLIGHTS, MAX_OFFLINE_DRIVER_FLIGHTS,
};
use oa_regional_session::{
    EventKind, FlightOperation, HandoffOperation, Identity, NormalizedEvent, SourceKind,
};

const CENTRE: [u8; 4] = *b"ZAU1";
const ADDRESS: u32 = 0x00ab_cdef;

fn driver(
    capacity: usize,
    refusal_log_capacity: usize,
    conflict_log_capacity: usize,
) -> oa_regional_session::flight::FlightDriver {
    flight_driver_new(FlightDriverConfig {
        flight_capacity: capacity,
        refusal_log_capacity,
        conflict_log_capacity,
    })
    .expect("bounded driver config is valid")
}

fn flight_event(
    gufi: u128,
    address: u32,
    time_ms: i64,
    sequence: u64,
    operation: FlightOperation,
) -> NormalizedEvent {
    NormalizedEvent {
        receipt_ns: time_ms * 1_000_000,
        source_ns: time_ms * 1_000_000,
        sequence,
        source: SourceKind::Flight,
        identity: Identity {
            flight_gufi: gufi,
            aircraft_address: address,
        },
        centre: CENTRE,
        kind: EventKind::Flight(operation),
    }
}

fn apply(
    driver: &mut oa_regional_session::flight::FlightDriver,
    event: NormalizedEvent,
) -> oa_regional_session::flight::FlightApplyOutcome {
    flight_apply_event(driver, &event).expect("source and receipt stamps are whole milliseconds")
}

fn active_plan(gufi: u128, address: u32, time_ms: i64, sequence: u64) -> [NormalizedEvent; 2] {
    [
        flight_event(
            gufi,
            address,
            time_ms,
            sequence,
            FlightOperation::SetPlan {
                plan_id: *b"PLAN00000001",
                status: 2,
            },
        ),
        flight_event(
            gufi,
            address,
            time_ms + 1,
            sequence + 1,
            FlightOperation::SetAddress,
        ),
    ]
}

// REQ: OA-REG-001
#[test]
fn actual_flight_operations_amend_clear_and_transfer_only_on_take() {
    let gufi = 0xfedc_ba98_7654_3210_0123_4567_89ab_cdef;
    let mut table = driver(4, 16, 16);
    let events = [
        flight_event(
            gufi,
            ADDRESS,
            100,
            1,
            FlightOperation::SetPlan {
                plan_id: *b"PLAN00000001",
                status: 2,
            },
        ),
        flight_event(gufi, ADDRESS, 101, 2, FlightOperation::SetAddress),
        flight_event(
            gufi,
            ADDRESS,
            102,
            3,
            FlightOperation::AssignAltitude {
                altitude_ft: 36_000,
            },
        ),
        flight_event(
            gufi,
            ADDRESS,
            103,
            4,
            FlightOperation::SetInterimAltitude {
                altitude_ft: Some(24_000),
            },
        ),
        flight_event(
            gufi,
            ADDRESS,
            104,
            5,
            FlightOperation::SetOwner {
                unit: *b"ZAU12\0\0\0",
            },
        ),
        flight_event(
            gufi,
            ADDRESS,
            105,
            6,
            FlightOperation::Handoff {
                operation: HandoffOperation::Offer,
                unit: *b"ZMP34\0\0\0",
            },
        ),
        flight_event(
            gufi,
            ADDRESS,
            106,
            7,
            FlightOperation::Handoff {
                operation: HandoffOperation::Accept,
                unit: *b"ZMP34\0\0\0",
            },
        ),
    ];
    for event in events {
        assert!(apply(&mut table, event).adapter_refusal.is_none());
    }

    let before_take =
        flight_view_for_gufi(&mut table, gufi, CENTRE, 106).expect("view is available");
    assert_eq!(before_take.gufi, gufi);
    assert_eq!(before_take.plan_state, ST_ACTIVE);
    assert_eq!(before_take.plan_id, *b"PLAN00000001");
    assert_eq!(before_take.address, ADDRESS);
    assert_eq!(before_take.assigned.a_ft, 36_000);
    assert_eq!(before_take.interim.a_ft, 24_000);
    assert_eq!(before_take.effective_clearance.altitude_ft, 24_000);
    assert!(before_take.effective_clearance.supported_as_single_level);
    assert_eq!(before_take.reserved_after_plan_state, [0; 3]);
    assert_eq!(before_take.reserved_after_assigned, [0; 4]);
    assert_eq!(before_take.reserved_after_interim, [0; 4]);
    assert_eq!(before_take.reserved_before_identity_stamp, [0; 4]);
    assert_eq!(before_take.reserved_before_handoff_stamp, [0; 7]);
    assert_eq!(before_take.reserved_tail, [0; 8]);
    assert_eq!(
        before_take.effective_clearance.reserved_before_altitude,
        [0; 3]
    );
    assert_eq!(
        before_take.effective_clearance.reserved_after_supported,
        [0; 3]
    );
    assert_eq!(before_take.owner_unit, *b"ZAU12\0\0\0");
    assert_eq!(before_take.handoff_state, HO_ACCEPTED);
    assert_eq!(before_take.handoff_accepted_by, *b"ZMP34\0\0\0");

    let duplicate_accept = apply(
        &mut table,
        flight_event(
            gufi,
            ADDRESS,
            107,
            8,
            FlightOperation::Handoff {
                operation: HandoffOperation::Accept,
                unit: *b"ZMP34\0\0\0",
            },
        ),
    );
    assert_eq!(
        duplicate_accept.operation,
        Some(oa_flight::Outcome::Unchanged)
    );
    let wrong_accept = apply(
        &mut table,
        flight_event(
            gufi,
            ADDRESS,
            108,
            9,
            FlightOperation::Handoff {
                operation: HandoffOperation::Accept,
                unit: *b"ZAU12\0\0\0",
            },
        ),
    );
    assert_eq!(
        wrong_accept.adapter_refusal,
        Some(AdapterRefusal::AcceptingUnitIsNotOfferedTarget)
    );

    let amendment = apply(
        &mut table,
        flight_event(
            gufi,
            ADDRESS,
            109,
            10,
            FlightOperation::SetPlan {
                plan_id: *b"PLAN00000002",
                status: 2,
            },
        ),
    );
    assert_eq!(amendment.operation, Some(oa_flight::Outcome::Applied));
    assert_eq!(
        apply(
            &mut table,
            flight_event(
                gufi,
                ADDRESS,
                110,
                11,
                FlightOperation::SetInterimAltitude { altitude_ft: None }
            )
        )
        .operation,
        Some(oa_flight::Outcome::Applied)
    );
    assert_eq!(
        apply(
            &mut table,
            flight_event(
                gufi,
                ADDRESS,
                111,
                12,
                FlightOperation::ClearAssignedAltitude
            )
        )
        .operation,
        Some(oa_flight::Outcome::Applied)
    );
    let after_clear =
        flight_view_for_gufi(&mut table, gufi, CENTRE, 111).expect("current view exists");
    assert_eq!(after_clear.interim.form, ALT_NIL);
    assert_eq!(after_clear.assigned.form, ALT_NIL);
    assert!(!after_clear.effective_clearance.supported_as_single_level);

    let stale = apply(
        &mut table,
        flight_event(
            gufi,
            ADDRESS,
            112,
            13,
            FlightOperation::AssignAltitude {
                altitude_ft: 35_000,
            },
        ),
    );
    assert_eq!(stale.operation, Some(oa_flight::Outcome::Applied));
    let old_altitude = NormalizedEvent {
        source_ns: 110_000_000,
        receipt_ns: 113_000_000,
        ..flight_event(
            gufi,
            ADDRESS,
            113,
            14,
            FlightOperation::AssignAltitude {
                altitude_ft: 34_000,
            },
        )
    };
    assert_eq!(
        apply(&mut table, old_altitude).operation,
        Some(oa_flight::Outcome::Refused(R_STALE))
    );

    let take = apply(
        &mut table,
        flight_event(
            gufi,
            ADDRESS,
            114,
            15,
            FlightOperation::Handoff {
                operation: HandoffOperation::Take,
                unit: *b"ZMP34\0\0\0",
            },
        ),
    );
    assert_eq!(take.operation, Some(oa_flight::Outcome::Applied));
    let repeated_take = apply(
        &mut table,
        flight_event(
            gufi,
            ADDRESS,
            115,
            16,
            FlightOperation::Handoff {
                operation: HandoffOperation::Take,
                unit: *b"ZMP34\0\0\0",
            },
        ),
    );
    assert_eq!(
        repeated_take.operation,
        Some(oa_flight::Outcome::Refused(R_NO_HANDOFF))
    );
    let after_take =
        flight_view_for_gufi(&mut table, gufi, CENTRE, 115).expect("taken view exists");
    assert_eq!(after_take.owner_unit, *b"ZMP34\0\0\0");
    assert_eq!(after_take.handoff_state, HO_NONE);
    let old_offer = NormalizedEvent {
        source_ns: 110_000_000,
        receipt_ns: 116_000_000,
        ..flight_event(
            gufi,
            ADDRESS,
            116,
            17,
            FlightOperation::Handoff {
                operation: HandoffOperation::Offer,
                unit: *b"ZAU12\0\0\0",
            },
        )
    };
    assert_eq!(
        apply(&mut table, old_offer).operation,
        Some(oa_flight::Outcome::Refused(R_STALE))
    );
}

#[test]
fn malformed_later_batch_event_refuses_before_any_table_mutation() {
    let gufi = 0x1000_0000_0000_0000_0000_0000_0000_0007;
    let mut table = driver(4, 8, 8);
    let first = flight_event(
        gufi,
        ADDRESS,
        100,
        1,
        FlightOperation::SetPlan {
            plan_id: *b"PLAN00000001",
            status: 2,
        },
    );
    let mut later = flight_event(gufi, ADDRESS, 100, 2, FlightOperation::SetAddress);
    later.source_ns -= 1;
    assert_eq!(
        flight_apply_batch(&mut table, &[first, later]),
        Err(FlightDriverError::NonMillisecondAdapterTimestamp)
    );
    let report = flight_driver_report(&table);
    assert_eq!(report.flights, 0);
    assert_eq!(report.operations, 0);
    assert_eq!(report.adapter.timestamp_precision_refusals, 1);

    assert_eq!(
        apply(&mut table, first).operation,
        Some(oa_flight::Outcome::Applied)
    );
}

// REQ: OA-REG-001
#[test]
fn timeout_and_late_acceptance_are_visible_in_actual_core_outcomes() {
    let gufi = 0x8000_0000_0000_0000_0000_0000_0000_0002;
    let mut table = driver(4, 8, 8);
    for event in active_plan(gufi, ADDRESS, 1, 1) {
        apply(&mut table, event);
    }
    apply(
        &mut table,
        flight_event(
            gufi,
            ADDRESS,
            3,
            3,
            FlightOperation::SetOwner {
                unit: *b"ZAU12\0\0\0",
            },
        ),
    );
    apply(
        &mut table,
        flight_event(
            gufi,
            ADDRESS,
            4,
            4,
            FlightOperation::Handoff {
                operation: HandoffOperation::Offer,
                unit: *b"ZMP34\0\0\0",
            },
        ),
    );
    let failed = apply(
        &mut table,
        flight_event(
            gufi,
            ADDRESS,
            5,
            5,
            FlightOperation::Handoff {
                operation: HandoffOperation::Fail,
                unit: *b"ZAU12\0\0\0",
            },
        ),
    );
    assert_eq!(failed.operation, Some(oa_flight::Outcome::Applied));
    assert_eq!(
        apply(
            &mut table,
            flight_event(
                gufi,
                ADDRESS,
                6,
                6,
                FlightOperation::Handoff {
                    operation: HandoffOperation::Accept,
                    unit: *b"ZMP34\0\0\0"
                }
            )
        )
        .operation,
        Some(oa_flight::Outcome::Refused(R_NO_HANDOFF))
    );
    let view =
        flight_view_for_gufi(&mut table, gufi, CENTRE, 6).expect("view exists after timeout");
    assert_eq!(view.owner_unit, *b"ZAU12\0\0\0");
    assert_eq!(view.handoff_state, HO_NONE);
}

// REQ: OA-REG-002
#[test]
fn address_join_keeps_ambiguity_full_gufi_and_cutoff_refusal_visible() {
    let first = 0xffff_0000_0000_0000_0000_0000_0000_0001;
    let second = 0x0000_ffff_0000_0000_0000_0000_0000_0002;
    let mut table = driver(4, 8, 8);
    for event in active_plan(first, ADDRESS, 10, 1) {
        apply(&mut table, event);
    }
    for event in active_plan(second, ADDRESS, 20, 3) {
        apply(&mut table, event);
    }

    let mut views: Vec<SharedFlightView> = Vec::with_capacity(MAX_DRIVER_FLIGHTS);
    let too_early = flight_views_for_address(&mut table, ADDRESS, CENTRE, 20, &mut views);
    assert_eq!(too_early.status, FlightViewStatus::FutureSnapshot);
    assert!(views.is_empty());
    let ambiguous = flight_views_for_address(&mut table, ADDRESS, CENTRE, 21, &mut views);
    assert_eq!(ambiguous.status, FlightViewStatus::AmbiguousAddress);
    assert_eq!(ambiguous.claimants, 2);
    assert_eq!(views.len(), 2);
    assert!(views.iter().any(|view| view.gufi == first));
    assert!(views.iter().any(|view| view.gufi == second));
    assert_eq!(
        flight_view_for_gufi(&mut table, first, *b"ZMP1", 21),
        Err(FlightViewStatus::UnknownCentre)
    );
}

// REQ: OA-REG-004
#[test]
fn driver_and_core_capacity_and_log_overflow_counters_are_explicit() {
    assert!(flight_driver_new(FlightDriverConfig {
        flight_capacity: MAX_DRIVER_FLIGHTS + 1,
        refusal_log_capacity: 1,
        conflict_log_capacity: 1
    })
    .is_err());
    let mut table = driver(3, 1, 1);
    for (gufi, time, seq) in [(1, 1, 1), (2, 3, 3), (3, 5, 5)] {
        for event in active_plan(gufi, ADDRESS, time, seq) {
            apply(&mut table, event);
        }
    }
    for event in active_plan(4, ADDRESS, 7, 7) {
        apply(&mut table, event);
    }
    let report = flight_driver_report(&table);
    assert_eq!(report.flights, 3);
    assert!(report.refusals_logged <= 1);
    assert!(report.refusals_unlisted > 0);
    assert!(report.conflicts_logged <= 1);
    assert!(report.conflicts_unlisted > 0);
}

// REQ: OA-REG-004
#[test]
fn explicit_offline_profile_can_raise_table_cap_without_changing_default() {
    let config = FlightDriverConfig {
        flight_capacity: 64,
        refusal_log_capacity: 1,
        conflict_log_capacity: 1,
    };
    assert!(flight_driver_new(config).is_err());
    let bounded = flight_driver_new_bounded(config, 64).expect("explicit cap is admitted");
    assert_eq!(flight_driver_capacity(&bounded), 64);
    assert!(flight_driver_new_bounded(
        FlightDriverConfig {
            flight_capacity: MAX_OFFLINE_DRIVER_FLIGHTS + 1,
            ..config
        },
        MAX_OFFLINE_DRIVER_FLIGHTS + 1,
    )
    .is_err());
}

// REQ: OA-REG-003
#[test]
fn identity_binding_requires_an_explicit_flight_operation_and_source_precision_is_not_truncated() {
    let gufi = 0x1000_0000_0000_0000_0000_0000_0000_0001;
    let mut table = driver(2, 4, 4);
    apply(
        &mut table,
        flight_event(
            gufi,
            ADDRESS,
            1,
            1,
            FlightOperation::SetPlan {
                plan_id: *b"PLAN00000001",
                status: 2,
            },
        ),
    );
    let position = NormalizedEvent {
        receipt_ns: 2_000_000,
        source_ns: 1_000_000,
        sequence: 1,
        source: SourceKind::Surveillance,
        identity: Identity {
            flight_gufi: gufi,
            aircraft_address: ADDRESS,
        },
        centre: CENTRE,
        kind: EventKind::Position {
            latitude_e7: 410_000_000,
            longitude_e7: -870_000_000,
            altitude_ft: Some(35_000),
            east_mmps: 0,
            north_mmps: 0,
            vertical_rate_fpm: None,
        },
    };
    assert!(
        flight_apply_event(&mut table, &position)
            .expect("position event is ignored")
            .ignored_non_flight_event
    );
    assert_eq!(
        flight_view_for_gufi(&mut table, gufi, CENTRE, 1)
            .expect("plan view exists")
            .address,
        0
    );

    let fractional = NormalizedEvent {
        source_ns: 2_500_000,
        receipt_ns: 3_000_000,
        ..flight_event(gufi, ADDRESS, 3, 2, FlightOperation::SetAddress)
    };
    assert_eq!(
        flight_apply_event(&mut table, &fractional),
        Err(oa_regional_session::flight::FlightDriverError::NonMillisecondAdapterTimestamp)
    );
    assert_eq!(
        flight_view_for_gufi(&mut table, gufi, CENTRE, 1)
            .expect("view remains unchanged")
            .address,
        0
    );
}
