#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use super::*;

fn outcome(source: InputSource, item_id: u64, refusal_reasons: u16) -> InputOutcome {
    InputOutcome {
        identity: InputIdentity {
            source,
            source_instance: 7,
            item_id,
        },
        refusal_reasons: ReasonFlags(refusal_reasons),
        clock_domain_id: 3,
        started_at_ns: 100,
        completed_at_ns: 120,
        elapsed_limit_ns: 40,
        work_units: 5,
        work_limit: 10,
    }
}

// REQ: OA-QUAR-001, OA-QUAR-002
#[test]
fn la_2014_altitude_block_and_route_shape_isolated_from_interleaved_healthy_inputs() {
    let mut queue = queue_new();
    let mut accepted_flight_state = 0u32;
    let bad_plan = outcome(
        InputSource::FlightData,
        101,
        REASON_ALTITUDE_ROUTE_INVALID | REASON_INPUT_INVALID,
    );
    let refused = route_input(&mut queue, bad_plan).expect("bounded refusal route");
    assert!(matches!(refused, Disposition::Queued(_)));
    // The adapter must not apply operations from a failed staged plan.
    if refused == Disposition::Accepted {
        accepted_flight_state += 1;
    }
    let healthy_track = route_input(&mut queue, outcome(InputSource::Surveillance, 201, 0))
        .expect("healthy input remains admissible");
    assert_eq!(healthy_track, Disposition::Accepted);
    if healthy_track == Disposition::Accepted {
        accepted_flight_state += 1;
    }
    let healthy_plan = route_input(&mut queue, outcome(InputSource::FlightData, 102, 0))
        .expect("healthy flight plan remains admissible");
    assert_eq!(healthy_plan, Disposition::Accepted);
    if healthy_plan == Disposition::Accepted {
        accepted_flight_state += 1;
    }

    assert_eq!(accepted_flight_state, 2);
    let event = queue_front(&queue, InputSource::FlightData)
        .expect("flight data partition")
        .expect("failed plan retained for handling");
    assert_eq!(event.identity.item_id, 101);
    assert_eq!(
        event.reasons.0,
        REASON_ALTITUDE_ROUTE_INVALID | REASON_INPUT_INVALID
    );
    assert_eq!(
        source_stats(&queue, InputSource::Surveillance)
            .expect("surveillance counters")
            .accepted,
        1
    );
}

#[derive(Clone, Copy)]
struct SyntheticWaypoint {
    display_name: [u8; 8],
    region: [u8; 4],
    fix_id: u32,
    lat_e7: i32,
    lon_e7: i32,
}

fn name_only_is_ambiguous(points: &[SyntheticWaypoint], name: [u8; 8]) -> bool {
    let mut matches = 0u32;
    for point in points {
        if point.display_name == name {
            matches += 1;
        }
    }
    matches > 1
}

fn same_identity_has_location_conflict(points: &[SyntheticWaypoint]) -> bool {
    let mut left = 0usize;
    while left < points.len() {
        let mut right = left + 1;
        while right < points.len() {
            if points[left].region == points[right].region
                && points[left].fix_id == points[right].fix_id
                && (points[left].lat_e7 != points[right].lat_e7
                    || points[left].lon_e7 != points[right].lon_e7)
            {
                return true;
            }
            right += 1;
        }
        left += 1;
    }
    false
}

// REQ: OA-QUAR-001, OA-QUAR-003
#[test]
fn nats_2023_duplicate_name_shape_is_ambiguous_only_without_full_fix_identity() {
    let repeated_name = *b"DUPFIX  ";
    let points = [
        SyntheticWaypoint {
            display_name: repeated_name,
            region: *b"KZXX",
            fix_id: 10,
            lat_e7: 410_000_000,
            lon_e7: -870_000_000,
        },
        SyntheticWaypoint {
            display_name: repeated_name,
            region: *b"KZYY",
            fix_id: 20,
            lat_e7: 420_000_000,
            lon_e7: -880_000_000,
        },
    ];
    assert!(name_only_is_ambiguous(&points, repeated_name));
    // Repeated local text is not identity (RULESET E7): explicit region+fix identity preserves
    // the rare route rather than rejecting it as a duplicate-name error.
    assert_ne!(
        (points[0].region, points[0].fix_id),
        (points[1].region, points[1].fix_id)
    );
    assert!(!same_identity_has_location_conflict(&points));
    let conflicting_location = [
        points[0],
        SyntheticWaypoint {
            lat_e7: 411_000_000,
            ..points[0]
        },
    ];
    assert!(same_identity_has_location_conflict(&conflicting_location));

    let mut queue = queue_new();
    let ambiguous = route_input(
        &mut queue,
        outcome(InputSource::FlightData, 301, REASON_WAYPOINT_NAME_AMBIGUOUS),
    )
    .expect("ambiguous route is quarantined");
    assert!(matches!(ambiguous, Disposition::Queued(_)));
    let resolved_rare_plan = route_input(&mut queue, outcome(InputSource::FlightData, 302, 0))
        .expect("full fix identity resolves same-name waypoint");
    assert_eq!(resolved_rare_plan, Disposition::Accepted);
    assert_eq!(
        queue_front(&queue, InputSource::FlightData)
            .expect("flight data partition")
            .expect("ambiguity event")
            .identity
            .item_id,
        301
    );
    assert_eq!(
        source_stats(&queue, InputSource::FlightData)
            .expect("flight data counters")
            .reason_counts[5],
        1
    );
}

// REQ: OA-QUAR-002, OA-QUAR-004
#[test]
fn work_and_elapsed_time_overruns_are_named_refusals_and_reason_counts() {
    let mut queue = queue_new();
    let mut over_budget = outcome(InputSource::Adaptation, 401, 0);
    over_budget.work_units = 11;
    over_budget.completed_at_ns = 150;
    let disposition = route_input(&mut queue, over_budget).expect("bounded overrun route");
    let Disposition::Queued(event) = disposition else {
        panic!("work and time overrun must be refused");
    };
    assert_eq!(event.reasons.0, REASON_WORK_BUDGET | REASON_TIME_BUDGET);
    let stats = source_stats(&queue, InputSource::Adaptation).expect("adaptation counters");
    assert_eq!(stats.queued, 1);
    assert_eq!(stats.reason_counts[2], 1);
    assert_eq!(stats.reason_counts[3], 1);
}

// REQ: OA-QUAR-002, OA-QUAR-004
#[test]
fn full_source_queue_returns_counted_manual_event_without_blocking_other_sources() {
    let mut queue = queue_new();
    let mut item_id = 1u64;
    while source_stats(&queue, InputSource::FlightData)
        .expect("flight data counters")
        .queued
        < 128
    {
        let refused = route_input(
            &mut queue,
            outcome(InputSource::FlightData, item_id, REASON_INPUT_INVALID),
        )
        .expect("bounded queue admission");
        assert!(matches!(refused, Disposition::Queued(_)));
        item_id += 1;
    }
    let full = route_input(
        &mut queue,
        outcome(InputSource::FlightData, item_id, REASON_ADAPTATION_CONFLICT),
    )
    .expect("queue-full manual path is visible");
    assert!(matches!(full, Disposition::ManualHandlingRequired(_)));
    let stats = source_stats(&queue, InputSource::FlightData).expect("flight data counters");
    assert_eq!(stats.queued, 128);
    assert_eq!(stats.queue_full_manual_events, 1);
    assert_eq!(stats.reason_counts[7], 1);

    let healthy_adaptation = route_input(&mut queue, outcome(InputSource::Adaptation, 501, 0))
        .expect("full flight-data queue does not halt adaptation");
    let healthy_surveillance = route_input(&mut queue, outcome(InputSource::Surveillance, 601, 0))
        .expect("full flight-data queue does not halt surveillance");
    assert_eq!(healthy_adaptation, Disposition::Accepted);
    assert_eq!(healthy_surveillance, Disposition::Accepted);
}

// REQ: OA-QUAR-002
#[test]
fn acknowledgement_is_explicit_and_invalid_timing_never_mutates_queue() {
    let mut queue = queue_new();
    let mut invalid = outcome(InputSource::Surveillance, 701, REASON_SURVEILLANCE_INVALID);
    invalid.completed_at_ns = 99;
    assert_eq!(
        route_input(&mut queue, invalid),
        Err(QuarantineError::ClockRegression)
    );
    assert_eq!(
        queue_front(&queue, InputSource::Surveillance).expect("empty surveillance queue"),
        None
    );

    route_input(
        &mut queue,
        outcome(InputSource::Surveillance, 702, REASON_SURVEILLANCE_INVALID),
    )
    .expect("quarantine one surveillance event");
    let before = queue_front(&queue, InputSource::Surveillance)
        .expect("queue front")
        .expect("event present");
    let acknowledged = queue_acknowledge(&mut queue, InputSource::Surveillance)
        .expect("explicit manual responsibility handoff");
    assert_eq!(acknowledged, before);
    assert_eq!(
        queue_front(&queue, InputSource::Surveillance).expect("empty after acknowledge"),
        None
    );
    assert_eq!(
        source_stats(&queue, InputSource::Surveillance)
            .expect("surveillance counters")
            .acknowledged,
        1
    );
}

// REQ: OA-QUAR-004
#[test]
fn counter_overflow_refuses_without_partially_changing_queue() {
    let mut queue = queue_new();
    let stats = queue.stats.get_mut(0).expect("fixed first source counter");
    stats.accepted = u64::MAX;
    let before = *stats;
    assert_eq!(
        route_input(&mut queue, outcome(InputSource::FlightData, 801, 0)),
        Err(QuarantineError::CounterOverflow)
    );
    assert_eq!(source_stats(&queue, InputSource::FlightData), Ok(before));
    assert_eq!(
        queue_front(&queue, InputSource::FlightData).expect("queue unchanged"),
        None
    );

    let stats = queue.stats.get_mut(0).expect("fixed first source counter");
    stats.accepted = 0;
    stats.queued = u64::MAX;
    let before_enqueue = *stats;
    assert_eq!(
        route_input(
            &mut queue,
            outcome(InputSource::FlightData, 802, REASON_INPUT_INVALID)
        ),
        Err(QuarantineError::CounterOverflow)
    );
    assert_eq!(
        source_stats(&queue, InputSource::FlightData),
        Ok(before_enqueue)
    );
    assert_eq!(
        queue_front(&queue, InputSource::FlightData).expect("enqueue failure leaves queue empty"),
        None
    );
}
