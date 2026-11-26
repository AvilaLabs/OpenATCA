#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use oa_legacy_traj::core::trajectory::*;

fn key() -> FlightKey {
    FlightKey {
        gufi: 0x1122_u128.to_be_bytes(),
        aircraft_address: 0xabc123,
        centre: *b"ZAU\0",
        plan_id: *b"PLAN-1234567",
    }
}

fn ident() -> FixIdentity {
    FixIdentity {
        ident: RouteIdentifier {
            len: 3,
            bytes: *b"ABC\0\0\0\0\0\0\0\0\0",
        },
        region: *b"US\0\0",
        scope: RouteIdentifier {
            len: 3,
            bytes: *b"ZAU\0\0\0\0\0\0\0\0\0",
        },
        class: 2,
    }
}

fn input<'a>(points: &'a [RoutePoint]) -> TrajectoryIssueInput<'a> {
    TrajectoryIssueInput {
        key: key(),
        route: RouteProvenance {
            source_ms: 1_000,
            available_ms: 1_100,
            message_id: 7,
            payload_sha256: [4; 32],
            key: key(),
            reserved: [0; 4],
        },
        cutoff_ms: 1_200,
        max_track_age_ms: 30_000,
        owner: OwnerProvenance {
            source_ms: 1_000,
            available_ms: 1_100,
            message_id: 3,
            owner_unit: *b"ZAU12\0\0\0",
            owner_state: OwnerState::Known,
            reserved: [0; 7],
        },
        route_cursor: Some(ActiveRouteCursor {
            source_ms: 1_000,
            available_ms: 1_100,
            message_id: 8,
            route_payload_sha256: [4; 32],
            key: key(),
            next_fix: ident(),
            reserved: [0; 5],
        }),
        track: CurrentTrack {
            state_ms: 1_200,
            newest_source_ms: 1_000,
            available_ms: 1_200,
            message_id: 9,
            aircraft_address: key().aircraft_address,
            lat_e7: 0,
            lon_e7: 0,
            altitude_ft: 10_000,
            vertical_rate_fpm: 0,
            groundspeed_mmps: 100_000,
        },
        clearance: Clearance {
            source_ms: 1_000,
            available_ms: 1_100,
            message_id: 4,
            min_altitude_ft: 10_000,
            max_altitude_ft: 10_000,
            altitude_at_fix_ft: NO_ALTITUDE_FT,
            form: ClearanceForm::Exact,
            reserved: [0; 3],
        },
        eta_target: EtaTarget {
            present: true,
            fix: ident(),
        },
        points,
    }
}

fn performance() -> PerformanceEnvelope {
    PerformanceEnvelope {
        key: key(),
        reserved_key_align: [0; 4],
        source_ms: 1_000,
        available_ms: 1_100,
        message_id: 12,
        max_age_ms: 30_000,
        max_climb_fpm: 600,
        max_descent_fpm: 600,
        max_groundspeed_mmps: 200_000,
        reserved_speed_cap: 0,
        source: PerformanceSource::Measured,
        status: PerformanceStatus::Valid,
        reserved: [0; 6],
    }
}

fn input_without_clearance<'a>(points: &'a [RoutePoint]) -> TrajectoryIssueInput<'a> {
    let mut request = input(points);
    request.clearance.source_ms = 0;
    request.clearance.available_ms = 0;
    request.clearance.message_id = 0;
    request.clearance.min_altitude_ft = NO_ALTITUDE_FT;
    request.clearance.max_altitude_ft = NO_ALTITUDE_FT;
    request.clearance.altitude_at_fix_ft = NO_ALTITUDE_FT;
    request.clearance.form = ClearanceForm::Absent;
    request
}

fn route_point(lat_e7: i32, lon_e7: i32, fix: FixIdentity) -> RoutePoint {
    RoutePoint {
        lat_e7,
        lon_e7,
        min_altitude_ft: NO_ALTITUDE_FT,
        max_altitude_ft: NO_ALTITUDE_FT,
        min_speed_mmps: NO_SPEED_MMPS,
        max_speed_mmps: NO_SPEED_MMPS,
        fix,
        altitude_constraint_kind: AltitudeConstraintKind::None,
        speed_constraint_kind: SpeedConstraintKind::None,
        geometry: PathGeometry::GreatCircleLeg,
        speed_reference: SpeedReference::GroundSpeed,
        reserved: [0; 1],
    }
}

#[test]
fn sequential_vertical_envelope_checks_at_above_below_and_window_restrictions() {
    let mut point = route_point(0, 1_000_000, ident()); // about 111 seconds at 100 m/s
    point.altitude_constraint_kind = AltitudeConstraintKind::AtAltitude;
    point.min_altitude_ft = 10_500;
    let points = [point];
    let request = input_without_clearance(&points);
    let result = issue_vertical_profile(&request, performance());
    assert_eq!(result.availability, Availability::Available);
    assert_eq!(result.points[0].reachable_min_ft, 10_500);
    assert_eq!(result.points[0].reachable_max_ft, 10_500);

    let mut climb = point;
    climb.min_altitude_ft = 11_000;
    let climb = [climb];
    assert_eq!(
        issue_vertical_profile(&input_without_clearance(&climb), performance()).availability,
        Availability::Available
    );
    let mut descent = point;
    descent.min_altitude_ft = 9_000;
    let descent = [descent];
    assert_eq!(
        issue_vertical_profile(&input_without_clearance(&descent), performance()).availability,
        Availability::Available
    );

    let mut above = point;
    above.min_altitude_ft = 12_000;
    let above = [above];
    assert_eq!(
        issue_vertical_profile(&input_without_clearance(&above), performance()).refusal,
        Refusal::InfeasibleVerticalProfile
    );

    let mut below = point;
    below.min_altitude_ft = 8_000;
    let below = [below];
    assert_eq!(
        issue_vertical_profile(&input_without_clearance(&below), performance()).refusal,
        Refusal::InfeasibleVerticalProfile
    );

    let mut window = route_point(0, 1_000_000, ident());
    window.altitude_constraint_kind = AltitudeConstraintKind::Window;
    window.min_altitude_ft = 10_200;
    window.max_altitude_ft = 10_800;
    let window = [window];
    let window_result = issue_vertical_profile(&input_without_clearance(&window), performance());
    assert_eq!(window_result.availability, Availability::Available);
    assert_eq!(window_result.points[0].reachable_min_ft, 10_200);
    assert_eq!(window_result.points[0].reachable_max_ft, 10_800);
}

#[test]
fn sequential_descent_clearance_leveloff_and_separate_speed_channel() {
    let mut point = route_point(0, 1_000_000, ident());
    point.altitude_constraint_kind = AltitudeConstraintKind::AtAltitude;
    point.min_altitude_ft = 9_500;
    point.speed_constraint_kind = SpeedConstraintKind::Window;
    point.min_speed_mmps = 90_000;
    point.max_speed_mmps = 110_000;
    let points = [point];
    let mut request = input(&points);
    request.track.vertical_rate_fpm = -300;
    request.clearance.min_altitude_ft = 10_500;
    request.clearance.max_altitude_ft = 10_500;
    request.clearance.form = ClearanceForm::Exact;
    assert_eq!(
        issue_vertical_profile(&request, performance()).refusal,
        Refusal::InfeasibleVerticalProfile
    );

    request.clearance.min_altitude_ft = NO_ALTITUDE_FT;
    request.clearance.max_altitude_ft = NO_ALTITUDE_FT;
    request.clearance.altitude_at_fix_ft = NO_ALTITUDE_FT;
    request.clearance.form = ClearanceForm::Absent;
    request.clearance.source_ms = 0;
    request.clearance.available_ms = 0;
    request.clearance.message_id = 0;
    let feasible = issue_vertical_profile(&request, performance());
    assert_eq!(feasible.availability, Availability::Available);
    assert_eq!(
        feasible.points[0].speed_mmps,
        request.track.groundspeed_mmps
    );

    let first = route_point(0, 1_000_000, ident());
    let mut second_fix = ident();
    second_fix.ident.bytes[0] = b'D';
    let second = route_point(0, 2_000_000, second_fix);
    let level_points = [first, second];
    let mut level_request = input(&level_points);
    level_request.clearance.min_altitude_ft = 10_500;
    level_request.clearance.max_altitude_ft = 10_500;
    let level = issue_vertical_profile(&level_request, performance());
    assert_eq!(level.availability, Availability::Available);
    // A clearance is a level-off target; it does not force attainment at the first fix.
    assert_eq!(level.points[0].reachable_min_ft, 10_000);
    assert_eq!(level.points[0].reachable_max_ft, 10_500);
    assert_eq!(level.points[1].reachable_min_ft, 10_000);
    assert_eq!(level.points[1].reachable_max_ft, 10_500);

    let mut bad_speed = point;
    bad_speed.min_speed_mmps = 150_000;
    let bad_speed = [bad_speed];
    assert_eq!(
        issue_vertical_profile(&input(&bad_speed), performance()).refusal,
        Refusal::InfeasibleSpeedWindow
    );
    let mut over_cap = point;
    over_cap.min_speed_mmps = 210_000;
    over_cap.max_speed_mmps = 220_000;
    let over_cap = [over_cap];
    assert_eq!(
        issue_vertical_profile(&input(&over_cap), performance()).refusal,
        Refusal::InfeasibleSpeedWindow
    );
}

// REQ: OA-TRAJ-025
#[test]
fn performance_evidence_is_flight_bound_and_zero_rate_is_a_valid_level_bound() {
    let points = [route_point(0, 1_000, ident())];
    let request = input_without_clearance(&points);
    let mut evidence = performance();
    evidence.max_climb_fpm = 0;
    evidence.max_descent_fpm = 0;
    let level = issue_vertical_profile(&request, evidence);
    assert_eq!(level.availability, Availability::Available);
    assert_eq!(level.points[0].reachable_min_ft, 10_000);
    assert_eq!(level.points[0].reachable_max_ft, 10_000);
    assert_eq!(level.performance, evidence);

    evidence.key.gufi[0] ^= 1;
    let mismatched = issue_vertical_profile(&request, evidence);
    assert_eq!(mismatched.refusal, Refusal::PerformanceIdentityMismatch);
    assert_eq!(mismatched.performance, evidence);

    let mut missing_message = performance();
    missing_message.message_id = 0;
    assert_eq!(
        issue_vertical_profile(&request, missing_message).refusal,
        Refusal::InvalidPerformance
    );
    let mut excessive_rate = performance();
    excessive_rate.max_climb_fpm = 60_001;
    assert_eq!(
        issue_vertical_profile(&request, excessive_rate).refusal,
        Refusal::InvalidPerformance
    );
    let mut bad_key_padding = performance();
    bad_key_padding.reserved_key_align[0] = 1;
    assert_eq!(
        issue_vertical_profile(&request, bad_key_padding).refusal,
        Refusal::InvalidReservedFields
    );

    let mut future_evidence = performance();
    future_evidence.source_ms = request.cutoff_ms + 1;
    future_evidence.available_ms = request.cutoff_ms + 2;
    let future_output = issue_vertical_profile(&request, future_evidence);
    let mut changed_future_evidence = future_evidence;
    changed_future_evidence.max_climb_fpm = u32::MAX;
    changed_future_evidence.max_groundspeed_mmps = u32::MAX;
    changed_future_evidence.status = PerformanceStatus::Contradictory;
    changed_future_evidence.reserved[0] = 9;
    let changed_future_output = issue_vertical_profile(&request, changed_future_evidence);
    assert_eq!(future_output, changed_future_output);
    assert_eq!(
        future_output.refusal,
        Refusal::PerformanceUnavailableAtCutoff
    );
    assert_eq!(
        future_output.performance.source,
        PerformanceSource::Unavailable
    );
}

// REQ: OA-TRAJ-025
#[test]
fn current_unsupported_and_future_clearances_never_produce_a_feasible_profile() {
    let mut point = route_point(0, 1_000, ident());
    point.altitude_constraint_kind = AltitudeConstraintKind::None;
    let points = [point];
    let mut request = input(&points);
    request.clearance.form = ClearanceForm::AltitudeAtFix;
    request.clearance.min_altitude_ft = NO_ALTITUDE_FT;
    request.clearance.max_altitude_ft = NO_ALTITUDE_FT;
    request.clearance.altitude_at_fix_ft = 20_000;
    request.clearance.message_id = 14;
    let unsupported = issue_vertical_profile(&request, performance());
    assert_eq!(unsupported.availability, Availability::Unavailable);
    assert_eq!(unsupported.refusal, Refusal::UnsupportedClearanceForm);
    assert_eq!(
        unsupported.clearance_assumption,
        ClearanceAssumption::Unsupported
    );
    for form in [
        ClearanceForm::Block,
        ClearanceForm::Vfr,
        ClearanceForm::Unsupported,
    ] {
        request.clearance.form = form;
        request.clearance.altitude_at_fix_ft = NO_ALTITUDE_FT;
        let output = issue_vertical_profile(&request, performance());
        assert_eq!(output.refusal, Refusal::UnsupportedClearanceForm);
        assert_eq!(output.availability, Availability::Unavailable);
    }

    request.clearance.form = ClearanceForm::Exact;
    request.clearance.altitude_at_fix_ft = NO_ALTITUDE_FT;
    request.clearance.min_altitude_ft = 12_000;
    request.clearance.max_altitude_ft = 12_000;
    request.clearance.source_ms = request.cutoff_ms + 1;
    request.clearance.available_ms = request.cutoff_ms + 2;
    let future = issue_vertical_profile(&request, performance());
    assert_eq!(future.availability, Availability::Unavailable);
    assert_eq!(future.refusal, Refusal::ClearanceUnavailableAtCutoff);
    assert_eq!(
        future.clearance_assumption,
        ClearanceAssumption::Unavailable
    );

    request.clearance.source_ms = 1_000;
    request.clearance.available_ms = 1_100;
    request.clearance.min_altitude_ft = NO_ALTITUDE_FT;
    request.clearance.max_altitude_ft = NO_ALTITUDE_FT;
    assert_eq!(
        issue_vertical_profile(&request, performance()).refusal,
        Refusal::InfeasibleAltitudeWindow
    );

    request.clearance = Clearance {
        source_ms: 0,
        available_ms: 0,
        message_id: 0,
        min_altitude_ft: NO_ALTITUDE_FT,
        max_altitude_ft: NO_ALTITUDE_FT,
        altitude_at_fix_ft: 0,
        form: ClearanceForm::Absent,
        reserved: [0; 3],
    };
    let malformed_absent = issue_vertical_profile(&request, performance());
    assert_eq!(
        malformed_absent.refusal,
        Refusal::ClearanceUnavailableAtCutoff
    );
}

// REQ: OA-TRAJ-026
#[test]
fn assigned_exact_target_is_a_leveloff_target_and_route_constraints_remain_sequential() {
    let near = [route_point(0, 1_000, ident())];
    let mut request = input(&near);
    request.clearance.min_altitude_ft = 18_000;
    request.clearance.max_altitude_ft = 18_000;
    request.clearance.form = ClearanceForm::Exact;
    let target = issue_vertical_profile(&request, performance());
    assert_eq!(target.availability, Availability::Available);
    assert!(target.points[0].reachable_min_ft < 18_000);
    assert!(target.points[0].reachable_max_ft < 18_000);

    let mut below = route_point(0, 1_000, ident());
    below.altitude_constraint_kind = AltitudeConstraintKind::Maximum;
    below.max_altitude_ft = 9_000;
    let below_points = [below];
    let mut below_request = input(&below_points);
    below_request.clearance.min_altitude_ft = 18_000;
    below_request.clearance.max_altitude_ft = 18_000;
    let below_result = issue_vertical_profile(&below_request, performance());
    assert_eq!(below_result.refusal, Refusal::InfeasibleVerticalProfile);

    let mut at_target = route_point(0, 10_000_000, ident());
    at_target.altitude_constraint_kind = AltitudeConstraintKind::AtAltitude;
    at_target.min_altitude_ft = 18_000;
    let mut later_fix = ident();
    later_fix.ident.bytes[0] = b'D';
    let later = route_point(0, 10_000_100, later_fix);
    let points = [at_target, later];
    let mut request = input(&points);
    request.clearance.min_altitude_ft = 18_000;
    request.clearance.max_altitude_ft = 18_000;
    let result = issue_vertical_profile(&request, performance());
    assert_eq!(result.availability, Availability::Available);
    assert_eq!(result.points[0].reachable_min_ft, 18_000);
    assert_eq!(result.points[1].reachable_min_ft, 18_000);
    assert_eq!(result.points[1].reachable_max_ft, 18_000);

    let mut contradictory_point = later;
    contradictory_point.altitude_constraint_kind = AltitudeConstraintKind::AtAltitude;
    contradictory_point.min_altitude_ft = 10_000;
    let contradictory = [at_target, contradictory_point];
    let mut contradiction = input(&contradictory);
    contradiction.clearance.min_altitude_ft = 18_000;
    contradiction.clearance.max_altitude_ft = 18_000;
    let result = issue_vertical_profile(&contradiction, performance());
    assert_eq!(result.refusal, Refusal::InfeasibleVerticalProfile);
    assert_eq!(result.examined_points, 2);
    assert_eq!(result.points[0].availability, Availability::Available);
    assert_eq!(result.points[1].availability, Availability::Unavailable);
}

// REQ: OA-TRAJ-026
#[test]
fn window_clearance_allows_entry_without_a_deadline_and_holds_inside_bounds() {
    let mut too_soon = route_point(0, 1_000, ident());
    too_soon.altitude_constraint_kind = AltitudeConstraintKind::AtAltitude;
    too_soon.min_altitude_ft = 15_000;
    let too_soon_points = [too_soon];
    let mut early = input(&too_soon_points);
    early.clearance.form = ClearanceForm::Window;
    early.clearance.min_altitude_ft = 12_000;
    early.clearance.max_altitude_ft = 16_000;
    let early_result = issue_vertical_profile(&early, performance());
    assert_eq!(early_result.refusal, Refusal::InfeasibleVerticalProfile);

    let mut first_distant = route_point(0, 10_000_000, ident());
    first_distant.altitude_constraint_kind = AltitudeConstraintKind::None;
    let mut reached_fix = ident();
    reached_fix.ident.bytes[0] = b'D';
    let mut reached = route_point(0, 10_000_000, reached_fix);
    reached.altitude_constraint_kind = AltitudeConstraintKind::AtAltitude;
    reached.min_altitude_ft = 15_000;
    let mut final_fix = ident();
    final_fix.ident.bytes[0] = b'E';
    let final_point = route_point(0, 10_000_100, final_fix);
    let points = [first_distant, reached, final_point];
    let mut request = input(&points);
    request.clearance.form = ClearanceForm::Window;
    request.clearance.min_altitude_ft = 12_000;
    request.clearance.max_altitude_ft = 16_000;
    let result = issue_vertical_profile(&request, performance());
    assert_eq!(result.availability, Availability::Available);
    assert!(result.points[0].reachable_min_ft < 12_000);
    assert_eq!(result.points[1].reachable_min_ft, 15_000);
    assert_eq!(result.points[2].reachable_min_ft, 15_000);
    assert_eq!(result.points[2].reachable_max_ft, 15_000);

    request.clearance.min_altitude_ft = 9_000;
    request.clearance.max_altitude_ft = NO_ALTITUDE_FT;
    let open_upper = issue_vertical_profile(&request, performance());
    assert_eq!(open_upper.availability, Availability::Available);
    assert!(open_upper.points[0].reachable_min_ft >= 9_000);
    assert_ne!(open_upper.points[0].reachable_max_ft, NO_ALTITUDE_FT);
}

// REQ: OA-TRAJ-026
#[test]
fn microfoot_propagation_handles_zero_tiny_legs_and_reports_checked_overflow() {
    let mut second_fix = ident();
    second_fix.ident.bytes[0] = b'D';
    let points = [
        route_point(0, 0, ident()),
        route_point(0, 0, second_fix),
        route_point(0, 1, {
            let mut fix = ident();
            fix.ident.bytes[0] = b'E';
            fix
        }),
    ];
    let request = input_without_clearance(&points);
    let result = issue_vertical_profile(&request, performance());
    assert_eq!(result.availability, Availability::Available);
    assert_eq!(result.points[0].crossing_ms, request.track.state_ms);
    assert_eq!(result.points[1].crossing_ms, request.track.state_ms);
    assert!(result.points[2].crossing_ms >= request.track.state_ms);

    let long_points = [route_point(0, 900_000_000, ident())];
    let long_request = input_without_clearance(&long_points);
    let mut slow = performance();
    slow.max_groundspeed_mmps = 1;
    slow.max_climb_fpm = 60_000;
    let mut slow_request = long_request;
    slow_request.track.groundspeed_mmps = 1;
    assert_eq!(
        issue_vertical_profile(&slow_request, slow).refusal,
        Refusal::AltitudeOverflow
    );

    let time_points = [route_point(0, 1_000_000, ident())];
    let mut time_overflow = input_without_clearance(&time_points);
    time_overflow.track.state_ms = i64::MAX;
    time_overflow.track.available_ms = i64::MAX;
    time_overflow.track.newest_source_ms = i64::MAX;
    time_overflow.cutoff_ms = i64::MAX;
    let mut fresh = performance();
    fresh.source_ms = i64::MAX - 10;
    fresh.available_ms = i64::MAX - 5;
    fresh.max_age_ms = 20;
    assert_eq!(
        issue_vertical_profile(&time_overflow, fresh).refusal,
        Refusal::TimeOverflow
    );
}

#[test]
fn vertical_profile_requires_fresh_noncontradictory_performance_and_checks_overflow() {
    let points = [route_point(0, 1_000_000, ident())];
    let request = input(&points);
    let mut stale = performance();
    stale.max_age_ms = 1;
    assert_eq!(
        issue_vertical_profile(&request, stale).refusal,
        Refusal::PerformanceStale
    );
    let mut future = performance();
    future.source_ms = 1_201;
    future.available_ms = 1_202;
    assert_eq!(
        issue_vertical_profile(&request, future).refusal,
        Refusal::PerformanceUnavailableAtCutoff
    );
    let mut contradictory = performance();
    contradictory.status = PerformanceStatus::Contradictory;
    assert_eq!(
        issue_vertical_profile(&request, contradictory).refusal,
        Refusal::InvalidPerformance
    );

    let mut overflow_request = input(&points);
    overflow_request.track.state_ms = i64::MAX;
    overflow_request.track.available_ms = i64::MAX;
    overflow_request.track.newest_source_ms = i64::MAX;
    overflow_request.cutoff_ms = i64::MAX;
    let mut fresh = performance();
    fresh.source_ms = i64::MAX - 10;
    fresh.available_ms = i64::MAX - 5;
    fresh.max_age_ms = 20;
    assert_eq!(
        issue_vertical_profile(&overflow_request, fresh).refusal,
        Refusal::TimeOverflow
    );

    let too_many = [route_point(0, 1_000_000, ident()); MAX_ROUTE_POINTS + 1];
    assert_eq!(
        issue_vertical_profile(&input(&too_many), performance()).refusal,
        Refusal::RouteTooLong
    );
}

// REQ: OA-TRAJ-017
#[test]
fn unavailable_clearance_payload_cannot_withhold_horizontal_forecasts() {
    let mut points = [route_point(0, 100_000_000, ident())];
    points[0].altitude_constraint_kind = AltitudeConstraintKind::Minimum;
    points[0].min_altitude_ft = 8_000;
    let mut request = input(&points);
    request.clearance.source_ms = request.cutoff_ms + 1;
    request.clearance.available_ms = request.cutoff_ms + 2;
    let baseline = issue_trajectory(&request);
    for (min, max) in [(15_000, 5_000), (5_000, 5_000), (12_000, 12_000)] {
        request.clearance.min_altitude_ft = min;
        request.clearance.max_altitude_ft = max;
        let output = issue_trajectory(&request);
        assert_eq!(output, baseline);
        assert_eq!(output.horizons[0].available, Availability::Degraded);
        assert_eq!(output.horizons[0].altitude_ft, NO_ALTITUDE_FT);
        assert_eq!(output.horizons[0].flags & OUTPUT_CAPTURED_ALTITUDE, 0);
    }
}

// REQ: OA-TRAJ-018
#[test]
fn repeated_eta_fix_is_ambiguous_and_crossed_vector_gap_remains_visible() {
    let mut a = ident();
    a.ident.bytes[0] = b'X';
    let mut b = ident();
    b.ident.bytes[0] = b'B';
    let points = [
        route_point(0, 1_000_000, ident()),
        route_point(0, 2_000_000, a),
        route_point(0, 3_000_000, b),
        route_point(0, 4_000_000, a),
    ];
    let mut request = input(&points);
    request.eta_target.fix = a;
    assert_eq!(
        issue_trajectory(&request).eta.refusal,
        Refusal::AmbiguousEtaTarget
    );

    let mut gap_points = [
        route_point(0, 1_000_000, ident()),
        route_point(0, 100_000_000, b),
    ];
    gap_points[0].geometry = PathGeometry::VectorsGap;
    let mut gap_request = input(&gap_points);
    gap_request.eta_target.fix = b;
    let output = issue_trajectory(&gap_request);
    for horizon in output.horizons {
        assert_eq!(horizon.available, Availability::Degraded);
        assert_ne!(horizon.flags & OUTPUT_VECTORS_GAP, 0);
    }
    assert_eq!(output.eta.refusal, Refusal::UnsupportedGeometry);
}

// REQ: OA-TRAJ-014
#[test]
fn unscoped_fix_is_valid_and_foreign_observed_address_is_refused() {
    let mut fix = ident();
    fix.scope = RouteIdentifier {
        len: 0,
        bytes: [0; 12],
    };
    let points = [route_point(0, 100_000_000, fix)];
    let mut request = input(&points);
    request.route_cursor.as_mut().unwrap().next_fix = fix;
    request.eta_target.fix = fix;
    let issued = issue_trajectory(&request);
    assert_eq!(issued.horizons[0].available, Availability::Available);
    assert_eq!(issued.eta.available, Availability::Available);
    request.track.aircraft_address += 1;
    assert_eq!(
        issue_trajectory(&request).horizons[0].refusal,
        Refusal::TrackIdentityMismatch
    );
}

// REQ: OA-TRAJ-015
#[test]
fn clearance_capture_never_teleports_level_or_away_moving_traffic() {
    let points = [route_point(0, 100_000_000, ident())];
    for (altitude, rate, expected, captured) in [
        (10_000, 0, 10_000, false),
        (10_000, 600, 12_000, true),
        (14_000, -600, 12_000, true),
        (10_000, -600, 7_000, false),
        (14_000, 600, 17_000, false),
    ] {
        let mut request = input(&points);
        request.track.altitude_ft = altitude;
        request.track.vertical_rate_fpm = rate;
        request.clearance.min_altitude_ft = 12_000;
        request.clearance.max_altitude_ft = 12_000;
        let result = issue_trajectory(&request);
        assert_eq!(
            result.horizons[0].altitude_ft, expected,
            "altitude {altitude}, rate {rate}"
        );
        assert_eq!(
            result.horizons[0].flags & OUTPUT_CAPTURED_ALTITUDE != 0,
            captured
        );
    }
    let mut malformed = input(&points);
    malformed.clearance.min_altitude_ft = 9_000;
    malformed.clearance.max_altitude_ft = 11_000;
    assert_eq!(
        issue_trajectory(&malformed).horizons[0].refusal,
        Refusal::UnsupportedClearanceForm
    );
}

// REQ: OA-TRAJ-016
#[test]
fn unsupported_constraint_beyond_all_horizons_is_still_declared_for_the_route() {
    let mut points = [route_point(0, 100_000_000, ident())];
    points[0].altitude_constraint_kind = AltitudeConstraintKind::Minimum;
    points[0].min_altitude_ft = 5_000;
    let result = issue_trajectory(&input(&points));
    assert_ne!(result.model_flags & MODEL_ROUTE_CONSTRAINT_UNSUPPORTED, 0);
    points[0].speed_constraint_kind = SpeedConstraintKind::Window;
    points[0].speed_reference = SpeedReference::Unknown;
    points[0].min_speed_mmps = NO_SPEED_MMPS;
    points[0].max_speed_mmps = 120_000;
    let result = issue_trajectory(&input(&points));
    assert_eq!(result.horizons[0].refusal, Refusal::UnsupportedConstraint);
    assert_eq!(
        result.horizons[0].flags & OUTPUT_SPEED_CONSTRAINT_VIOLATED,
        0
    );
}

// REQ: OA-TRAJ-001
#[test]
fn constant_groundspeed_horizons_and_exact_fix_eta_are_deterministic() {
    let points = [route_point(0, 100_000_000, ident())];
    let result = issue_trajectory(&input(&points));
    assert_eq!(result.key, key());
    assert_eq!(result.horizons[0].horizon_ms, 300_000);
    assert_eq!(result.horizons[1].horizon_ms, 600_000);
    assert_eq!(result.horizons[2].horizon_ms, 1_200_000);
    assert!(result.horizons[0].lon_e7 > 2_690_000 && result.horizons[0].lon_e7 < 2_705_000);
    assert_eq!(result.horizons[0].available, Availability::Available);
    assert_eq!(result.horizons[0].altitude_ft, 10_000);
    assert_eq!(result.eta.available, Availability::Available);
    assert_eq!(result.eta.fix.region, *b"US\0\0");
}

// REQ: OA-TRAJ-002
#[test]
fn altitude_capture_and_infeasible_route_window_are_explicit() {
    let mut points = [route_point(0, 100_000_000, ident())];
    points[0].altitude_constraint_kind = AltitudeConstraintKind::Window;
    points[0].min_altitude_ft = 12_000;
    points[0].max_altitude_ft = 10_000;
    let result = issue_trajectory(&input(&points));
    assert_eq!(
        result.horizons[0].refusal,
        Refusal::InfeasibleAltitudeWindow
    );
    points[0].altitude_constraint_kind = AltitudeConstraintKind::None;
    let mut climb = input(&points);
    climb.track.vertical_rate_fpm = 1_200;
    let captured = issue_trajectory(&climb);
    assert_eq!(captured.horizons[0].altitude_ft, 10_000);
    assert_ne!(captured.horizons[0].flags & OUTPUT_CAPTURED_ALTITUDE, 0);
}

// REQ: OA-TRAJ-003
#[test]
fn published_speed_window_is_reported_without_coercing_groundspeed() {
    let mut points = [route_point(0, 100_000_000, ident())];
    points[0].speed_constraint_kind = SpeedConstraintKind::Window;
    points[0].min_speed_mmps = 110_000;
    points[0].max_speed_mmps = 120_000;
    let result = issue_trajectory(&input(&points));
    assert_eq!(result.horizons[0].available, Availability::Degraded);
    assert_eq!(result.horizons[0].refusal, Refusal::InfeasibleSpeedWindow);
    assert_eq!(result.horizons[0].groundspeed_mmps, 100_000);
    points[0].speed_reference = SpeedReference::IndicatedAirspeed;
    let airspeed = issue_trajectory(&input(&points));
    assert_eq!(airspeed.horizons[0].available, Availability::Degraded);
    assert_eq!(airspeed.horizons[0].refusal, Refusal::UnsupportedConstraint);
    assert_eq!(airspeed.horizons[0].groundspeed_mmps, 100_000);
}

// REQ: OA-TRAJ-021
#[test]
fn altitude_and_speed_constraints_are_retained_and_evaluated_independently() {
    let mut points = [route_point(0, 100_000_000, ident())];
    points[0].altitude_constraint_kind = AltitudeConstraintKind::Minimum;
    points[0].min_altitude_ft = 8_000;
    points[0].speed_constraint_kind = SpeedConstraintKind::Minimum;
    points[0].min_speed_mmps = 90_000;
    let result = issue_trajectory(&input(&points));

    assert_ne!(result.model_flags & MODEL_ROUTE_CONSTRAINT_UNSUPPORTED, 0);
    assert_eq!(result.horizons[0].available, Availability::Degraded);
    assert_eq!(result.horizons[0].refusal, Refusal::UnsupportedConstraint);
    assert_ne!(result.horizons[0].flags & OUTPUT_CONSTRAINT_CHECKED, 0);
    assert_eq!(
        result.horizons[0].flags & OUTPUT_SPEED_CONSTRAINT_VIOLATED,
        0
    );
    assert_eq!(result.horizons[0].groundspeed_mmps, 100_000);
    assert_eq!(result.horizons[0].altitude_ft, NO_ALTITUDE_FT);
}

// REQ: OA-TRAJ-022
#[test]
fn indicated_airspeed_constraint_is_never_compared_with_groundspeed() {
    let mut points = [route_point(0, 100_000_000, ident())];
    points[0].speed_constraint_kind = SpeedConstraintKind::Window;
    points[0].speed_reference = SpeedReference::IndicatedAirspeed;
    points[0].min_speed_mmps = 110_000;
    points[0].max_speed_mmps = 120_000;
    let result = issue_trajectory(&input(&points));

    assert_eq!(result.horizons[0].refusal, Refusal::UnsupportedConstraint);
    assert_eq!(result.horizons[0].groundspeed_mmps, 100_000);
    assert_eq!(
        result.horizons[0].flags & OUTPUT_SPEED_CONSTRAINT_VIOLATED,
        0
    );
    assert_eq!(result.horizons[0].flags & OUTPUT_CONSTRAINT_CHECKED, 0);
}

// REQ: OA-TRAJ-023
#[test]
fn future_route_payload_is_isolated_before_constraint_validation() {
    let valid_points = [route_point(0, 100_000_000, ident())];
    let mut baseline_request = input(&valid_points);
    baseline_request.route.available_ms = baseline_request.cutoff_ms + 1;
    let baseline = issue_trajectory(&baseline_request);

    let mut malformed_points = [route_point(0, 100_000_000, ident())];
    malformed_points[0].lat_e7 = i32::MIN;
    malformed_points[0].altitude_constraint_kind = AltitudeConstraintKind::Window;
    malformed_points[0].min_altitude_ft = 15_000;
    malformed_points[0].max_altitude_ft = 5_000;
    malformed_points[0].speed_constraint_kind = SpeedConstraintKind::Window;
    malformed_points[0].min_speed_mmps = 130_000;
    malformed_points[0].max_speed_mmps = 90_000;
    let mut future_request = input(&malformed_points);
    future_request.route.available_ms = future_request.cutoff_ms + 1;
    let future_payload = issue_trajectory(&future_request);

    assert_eq!(future_payload, baseline);
    assert_eq!(
        future_payload.horizons[0].refusal,
        Refusal::RouteUnavailableAtCutoff
    );
}

// REQ: OA-TRAJ-024
#[test]
fn malformed_constraint_in_either_channel_is_refused_even_with_other_channel_present() {
    let mut altitude_bad = [route_point(0, 100_000_000, ident())];
    altitude_bad[0].altitude_constraint_kind = AltitudeConstraintKind::Window;
    altitude_bad[0].min_altitude_ft = 15_000;
    altitude_bad[0].max_altitude_ft = 5_000;
    altitude_bad[0].speed_constraint_kind = SpeedConstraintKind::Minimum;
    altitude_bad[0].min_speed_mmps = 90_000;
    assert_eq!(
        issue_trajectory(&input(&altitude_bad)).horizons[0].refusal,
        Refusal::InfeasibleAltitudeWindow
    );

    let mut speed_bad = [route_point(0, 100_000_000, ident())];
    speed_bad[0].altitude_constraint_kind = AltitudeConstraintKind::Minimum;
    speed_bad[0].min_altitude_ft = 8_000;
    speed_bad[0].speed_constraint_kind = SpeedConstraintKind::Window;
    speed_bad[0].min_speed_mmps = 130_000;
    speed_bad[0].max_speed_mmps = 90_000;
    assert_eq!(
        issue_trajectory(&input(&speed_bad)).horizons[0].refusal,
        Refusal::InfeasibleSpeedWindow
    );
}

// REQ: OA-TRAJ-004
#[test]
fn track_freshness_and_route_cutoff_are_checked() {
    let points = [route_point(0, 100_000_000, ident())];
    let mut stale = input(&points);
    stale.max_track_age_ms = 10;
    assert_eq!(
        issue_trajectory(&stale).horizons[0].refusal,
        Refusal::StaleTrack
    );
    let mut future = input(&points);
    future.route.available_ms = 1_201;
    assert_eq!(
        issue_trajectory(&future).horizons[0].refusal,
        Refusal::RouteUnavailableAtCutoff
    );
    let mut future_track = input(&points);
    future_track.track.state_ms = 1_201;
    future_track.track.available_ms = 1_201;
    assert_eq!(
        issue_trajectory(&future_track).horizons[0].refusal,
        Refusal::FutureTrack
    );
    let mut no_speed = input(&points);
    no_speed.track.groundspeed_mmps = 0;
    assert_eq!(
        issue_trajectory(&no_speed).horizons[0].refusal,
        Refusal::NoGroundspeed
    );
    let mut future_clearance = input(&points);
    future_clearance.clearance.available_ms = 1_201;
    let clearance_result = issue_trajectory(&future_clearance);
    assert_eq!(
        clearance_result.horizons[0].available,
        Availability::Degraded
    );
    assert_eq!(
        clearance_result.horizons[0].refusal,
        Refusal::ClearanceUnavailableAtCutoff
    );
    let mut foreign_route = input(&points);
    foreign_route.route.key.gufi[0] ^= 1;
    assert_eq!(
        issue_trajectory(&foreign_route).horizons[0].refusal,
        Refusal::RouteIdentityMismatch
    );
}

// REQ: OA-TRAJ-005
#[test]
fn exact_fix_identity_includes_region_and_route_end_is_not_extrapolated() {
    let points = [route_point(0, 1_000_000, ident())];
    let mut wrong_region = input(&points);
    wrong_region.eta_target.fix.region = *b"CA\0\0";
    assert_eq!(
        issue_trajectory(&wrong_region).eta.refusal,
        Refusal::EtaTargetNotFound
    );
    let result = issue_trajectory(&input(&points));
    assert_eq!(result.horizons[0].refusal, Refusal::RouteHasNoEnd);
}

// REQ: OA-TRAJ-006
#[test]
fn active_cursor_uses_only_the_filed_route_suffix() {
    let first_id = ident();
    let mut next_id = ident();
    next_id.ident.bytes = *b"NEXT\0\0\0\0\0\0\0\0";
    next_id.ident.len = 4;
    let points = [
        route_point(0, 100_000_000, first_id),
        route_point(0, 200_000_000, next_id),
    ];
    let mut request = input(&points);
    request.route_cursor = Some(ActiveRouteCursor {
        key: key(),
        next_fix: next_id,
        source_ms: 1_000,
        available_ms: 1_100,
        message_id: 10,
        route_payload_sha256: [4; 32],
        reserved: [0; 5],
    });
    let result = issue_trajectory(&request);
    assert_eq!(result.prior_route_points_ignored, 1);
    assert!(result.horizons[0].lon_e7 > 2_690_000 && result.horizons[0].lon_e7 < 2_705_000);
}

// REQ: OA-TRAJ-007
#[test]
fn route_capacity_refusal_counts_every_unexamined_point() {
    let point = route_point(0, 100_000_000, ident());
    let points = [point; MAX_ROUTE_POINTS + 1];
    let result = issue_trajectory(&input(&points));
    assert_eq!(result.horizons[0].refusal, Refusal::RouteTooLong);
    assert_eq!(result.examined_route_points, 0);
    assert_eq!(
        result.over_capacity_unexamined,
        (MAX_ROUTE_POINTS + 1) as u64
    );
}

// REQ: OA-TRAJ-008
#[test]
fn missing_ambiguous_and_future_active_cursors_are_refused() {
    let id = ident();
    let points = [
        route_point(0, 100_000_000, id),
        route_point(0, 200_000_000, id),
    ];
    let mut ambiguous = input(&points);
    assert_eq!(
        issue_trajectory(&ambiguous).horizons[0].refusal,
        Refusal::AmbiguousActiveLeg
    );
    ambiguous.route_cursor = None;
    assert_eq!(
        issue_trajectory(&ambiguous).horizons[0].refusal,
        Refusal::ActiveLegUnknown
    );
    ambiguous.route_cursor = Some(ActiveRouteCursor {
        key: key(),
        next_fix: id,
        source_ms: 1_000,
        available_ms: 1_201,
        message_id: 11,
        route_payload_sha256: [4; 32],
        reserved: [0; 5],
    });
    assert_eq!(
        issue_trajectory(&ambiguous).horizons[0].refusal,
        Refusal::CursorUnavailableAtCutoff
    );
    ambiguous.route_cursor = Some(ActiveRouteCursor {
        key: key(),
        next_fix: id,
        source_ms: 1_000,
        available_ms: 1_100,
        message_id: 11,
        route_payload_sha256: [5; 32],
        reserved: [0; 5],
    });
    assert_eq!(
        issue_trajectory(&ambiguous).horizons[0].refusal,
        Refusal::CursorIdentityMismatch
    );
    let mut foreign_key = key();
    foreign_key.gufi[0] ^= 1;
    ambiguous.route_cursor = Some(ActiveRouteCursor {
        key: foreign_key,
        next_fix: id,
        source_ms: 1_000,
        available_ms: 1_100,
        message_id: 11,
        route_payload_sha256: [4; 32],
        reserved: [0; 5],
    });
    assert_eq!(
        issue_trajectory(&ambiguous).horizons[0].refusal,
        Refusal::CursorIdentityMismatch
    );
}

// REQ: OA-TRAJ-009
#[test]
fn timestamp_epoch_overflow_is_reported_per_horizon() {
    let points = [route_point(0, 100_000_000, ident())];
    let mut near_max = input(&points);
    near_max.cutoff_ms = i64::MAX - 1;
    near_max.track.state_ms = i64::MAX - 1;
    near_max.track.newest_source_ms = i64::MAX - 1;
    near_max.track.available_ms = i64::MAX - 1;
    near_max.track.message_id = 10;
    assert_eq!(
        issue_trajectory(&near_max).horizons[0].refusal,
        Refusal::TimeOverflow
    );
}

// REQ: OA-TRAJ-010
#[test]
fn revision_selector_withholds_invalid_amendments_and_counts_the_full_cutoff_set() {
    let k = key();
    let good = RevisionEvidence {
        key: k,
        source_ms: 100,
        available_ms: 120,
        message_id: 1,
        payload_sha256: [1; 32],
        reserved: [0; 4],
    };
    let mut invalid = good;
    invalid.source_ms = 200;
    invalid.available_ms = 150;
    invalid.message_id = 2;
    let withheld = select_revision_evidence(k, 300, &[good, invalid]);
    assert_eq!(withheld.refusal, EvidenceRefusal::InvalidEvidence);
    assert_eq!(withheld.selected.source_ms, 0);
    assert_eq!(withheld.counts.invalid_time_order, 1);

    let mut future = invalid;
    future.available_ms = 301;
    future.key.gufi = [0; 16];
    let gated = select_revision_evidence(k, 300, &[good, future]);
    assert_eq!(gated.available, Availability::Available);
    assert_eq!(gated.selected.message_id, good.message_id);
    assert_eq!(gated.counts.future_availability, 1);
    assert_eq!(gated.counts.invalid_identity, 0);
}

// REQ: OA-TRAJ-019
#[test]
fn reserved_alignment_bytes_are_zero_checked_only_for_cutoff_eligible_inputs() {
    let mut point = route_point(0, 100_000_000, ident());
    point.reserved[0] = 1;
    let bad_points = [point];
    let mut request = input(&bad_points);
    assert_eq!(
        issue_trajectory(&request).horizons[0].refusal,
        Refusal::InvalidReservedFields
    );

    let eligible = RevisionEvidence {
        key: key(),
        source_ms: 100,
        available_ms: 120,
        message_id: 1,
        payload_sha256: [1; 32],
        reserved: [1, 0, 0, 0],
    };
    let rejected = select_revision_evidence(key(), 300, &[eligible]);
    assert_eq!(rejected.refusal, EvidenceRefusal::InvalidEvidence);
    assert_eq!(rejected.counts.invalid_identity, 1);

    let mut future = eligible;
    future.available_ms = 301;
    let gated = select_revision_evidence(key(), 300, &[future]);
    assert_eq!(gated.refusal, EvidenceRefusal::NoAvailableEvidence);
    assert_eq!(gated.counts.future_availability, 1);
    assert_eq!(gated.counts.invalid_identity, 0);

    let clean_points = [route_point(0, 100_000_000, ident())];
    request = input(&clean_points);
    request.owner.owner_state = OwnerState::Unknown;
    request.owner.source_ms = request.cutoff_ms + 1;
    request.owner.available_ms = request.cutoff_ms + 2;
    let baseline = issue_trajectory(&request);
    request.owner.reserved[0] = 1;
    let with_future_owner_padding = issue_trajectory(&request);
    assert_eq!(with_future_owner_padding, baseline);
}

// REQ: OA-TRAJ-020
#[test]
fn antipodal_geometry_is_refused_without_misreporting_route_exhaustion() {
    let points = [route_point(0, 1_800_000_000, ident())];
    let output = issue_trajectory(&input(&points));
    for horizon in output.horizons {
        assert_eq!(horizon.available, Availability::Unavailable);
        assert_eq!(horizon.refusal, Refusal::UnsupportedGeometry);
        assert_eq!(horizon.lat_e7, NO_POSITION_E7);
    }
}

// REQ: OA-TRAJ-011
#[test]
fn over_capacity_evidence_counts_all_unexamined_rows() {
    let k = key();
    let row = RevisionEvidence {
        key: k,
        source_ms: 100,
        available_ms: 120,
        message_id: 1,
        payload_sha256: [1; 32],
        reserved: [0; 4],
    };
    let rows = [row; MAX_REVISION_EVIDENCE + 1];
    let selected = select_revision_evidence(k, 300, &rows);
    assert_eq!(selected.refusal, EvidenceRefusal::TooManyCandidates);
    assert_eq!(selected.counts.examined, 0);
    assert_eq!(
        selected.counts.over_capacity_unexamined,
        (MAX_REVISION_EVIDENCE + 1) as u64
    );
}

// REQ: OA-TRAJ-012
#[test]
fn foreign_evidence_reasons_are_a_disjoint_partition() {
    let k = key();
    let mut different_gufi = k;
    different_gufi.gufi[0] ^= 1;
    different_gufi.aircraft_address += 1;
    different_gufi.centre = *b"ZMP ";
    let mut different_address = k;
    different_address.aircraft_address += 1;
    different_address.centre = *b"ZMP ";
    let mut different_centre = k;
    different_centre.centre = *b"ZMP ";
    let row = |key, msg| RevisionEvidence {
        key,
        source_ms: 100,
        available_ms: 120,
        message_id: msg,
        payload_sha256: [msg as u8; 32],
        reserved: [0; 4],
    };
    let selected = select_revision_evidence(
        k,
        300,
        &[
            row(different_gufi, 1),
            row(different_address, 2),
            row(different_centre, 3),
        ],
    );
    assert_eq!(selected.refusal, EvidenceRefusal::NoAvailableEvidence);
    assert_eq!(selected.counts.foreign_gufi, 1);
    assert_eq!(selected.counts.foreign_aircraft_address, 1);
    assert_eq!(selected.counts.foreign_reporting_centre, 1);
}

// REQ: OA-TRAJ-013
#[test]
fn route_revision_selection_preserves_full_flight_identity_and_detects_conflict() {
    let k = key();
    let older = RevisionEvidence {
        key: k,
        source_ms: 100,
        available_ms: 200,
        message_id: 2,
        payload_sha256: [1; 32],
        reserved: [0; 4],
    };
    let newer = RevisionEvidence {
        key: k,
        source_ms: 150,
        available_ms: 250,
        message_id: 3,
        payload_sha256: [2; 32],
        reserved: [0; 4],
    };
    let chosen = select_revision_evidence(k, 300, &[newer, older]);
    assert_eq!(chosen.available, Availability::Available);
    assert_eq!(chosen.selected.payload_sha256, [2; 32]);
    let conflict = RevisionEvidence {
        key: k,
        source_ms: 150,
        available_ms: 240,
        message_id: 4,
        payload_sha256: [3; 32],
        reserved: [0; 4],
    };
    assert_eq!(
        select_revision_evidence(k, 300, &[newer, conflict]).refusal,
        EvidenceRefusal::SameSourceConflict
    );
    let mut changed_plan_key = k;
    changed_plan_key.plan_id = *b"PLAN-7654321";
    let changed_plan = RevisionEvidence {
        key: changed_plan_key,
        source_ms: 200,
        available_ms: 260,
        message_id: 5,
        payload_sha256: [5; 32],
        reserved: [0; 4],
    };
    let plan_result = select_revision_evidence(k, 300, &[older, changed_plan]);
    assert_eq!(plan_result.selected.key.plan_id, changed_plan_key.plan_id);
    assert_eq!(plan_result.selected.source_ms, 200);
    let same_time_plan_conflict = RevisionEvidence {
        key: k,
        source_ms: 200,
        available_ms: 250,
        message_id: 6,
        payload_sha256: [5; 32],
        reserved: [0; 4],
    };
    assert_eq!(
        select_revision_evidence(k, 300, &[changed_plan, same_time_plan_conflict]).refusal,
        EvidenceRefusal::SameSourceConflict
    );
}

// REQ: OA-ENC-023
#[test]
fn sampled_shared_route_preserves_fixed_horizons_and_includes_route_turns() {
    let mut second_fix = ident();
    second_fix.ident.bytes = *b"DEF\0\0\0\0\0\0\0\0\0";
    let points = [
        route_point(0, 1_000_000, ident()),
        route_point(10_000_000, 1_000_000, second_fix),
    ];
    let request = input(&points);
    let fixed = issue_trajectory(&request);

    let mut offsets = [0_i64; MAX_PROFILE_SAMPLES];
    let (profile, count) = trajectory_profile_offsets(&request, &mut offsets);
    assert_eq!(profile.eta.refusal, Refusal::None);
    assert!(count > PROFILE_UNIFORM_SAMPLES);
    let first = HorizonOutput {
        horizon_ms: 0,
        predicted_time_ms: 0,
        lat_e7: 0,
        lon_e7: 0,
        altitude_ft: 0,
        groundspeed_mmps: 0,
        flags: 0,
        available: Availability::Unavailable,
        refusal: Refusal::None,
        reserved: [0; 2],
    };
    let mut samples = [first; MAX_PROFILE_SAMPLES];
    let sampled = issue_trajectory_samples(
        &request,
        offsets.get(..count).unwrap(),
        samples.get_mut(..count).unwrap(),
    );
    assert_eq!(sampled.eta.refusal, Refusal::None);
    for (index, expected) in fixed.horizons.iter().enumerate() {
        let actual = samples
            .iter()
            .take(count)
            .find(|sample| sample.horizon_ms == expected.horizon_ms)
            .unwrap();
        assert_eq!(actual, expected, "fixed horizon slot {index}");
    }
    let corner = samples.iter().take(count).find(|sample| {
        (sample.horizon_ms - 111_000).abs() <= 500
            && sample.lat_e7 == 0
            && (sample.lon_e7 - 1_000_000).abs() <= 100
    });
    assert!(
        corner.is_some(),
        "profile includes the reached route vertex"
    );
    let after_turn = samples
        .iter()
        .take(count)
        .find(|sample| sample.horizon_ms == 150_000)
        .unwrap();
    let coarse_chord_lon = fixed.horizons[0].lon_e7 / 2;
    assert!((after_turn.lon_e7 - coarse_chord_lon).abs() > 200_000);
}

// REQ: OA-ENC-023
#[test]
fn sampled_route_profile_and_horizon_capacity_refuse_without_truncation() {
    let points = [route_point(0, 1_000_000, ident())];
    let request = input(&points);
    let mut short_profile = [0_i64; MAX_PROFILE_SAMPLES - 1];
    let (profile, count) = trajectory_profile_offsets(&request, &mut short_profile);
    assert_eq!(profile.eta.refusal, Refusal::SampleCapacity);
    assert_eq!(count, 0);

    let offsets = [HORIZON_5_MIN_MS, 0];
    let sentinel = HorizonOutput {
        horizon_ms: 17,
        predicted_time_ms: 19,
        lat_e7: 23,
        lon_e7: 29,
        altitude_ft: 31,
        groundspeed_mmps: 37,
        flags: 41,
        available: Availability::Degraded,
        refusal: Refusal::UnsupportedGeometry,
        reserved: [43; 2],
    };
    let mut outputs = [sentinel; 2];
    let result = issue_trajectory_samples(&request, &offsets, &mut outputs);
    assert_eq!(result.eta.refusal, Refusal::InvalidSampleHorizon);
    assert_eq!(outputs, [sentinel; 2]);
}
