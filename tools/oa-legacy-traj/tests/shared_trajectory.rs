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
