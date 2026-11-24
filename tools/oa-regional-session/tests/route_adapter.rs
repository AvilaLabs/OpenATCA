#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_airspace::core::route::{empty_converted_route, RouteSource};
use oa_legacy_traj::core::trajectory::{
    self as t, AltitudeConstraintKind, PathGeometry, SpeedConstraintKind, SpeedReference,
};
use oa_regional_session::flight::{EffectiveClearance, SharedAltitude, SharedFlightView};
use oa_regional_session::route::{
    canonical_route_digest, clearance_evidence, route_event, route_issue, route_store_new,
    trajectory_points, RouteStore, MAX_ROUTE_REVISIONS,
};
use oa_regional_session::surveillance::ObservedTrack;
use oa_regional_session::{EventKind, Identity, NormalizedEvent, SourceKind};

const CENTRE: [u8; 4] = *b"ZAU1";
const ADDRESS: u32 = 0x00ab_cdef;
const PLAN: [u8; 12] = *b"PLAN00000001";
const ROUTE_TEXT: &str = "FIX:FIXA:K1:1:- FIX:FIXB:K1:1:-";

fn event(
    gufi: u128,
    _plan_id: [u8; 12],
    sequence: u64,
    source_ms: i64,
    receipt_ms: i64,
    kind: EventKind,
) -> NormalizedEvent {
    NormalizedEvent {
        source_ns: source_ms * 1_000_000,
        receipt_ns: receipt_ms * 1_000_000,
        sequence,
        source: SourceKind::Route,
        identity: Identity {
            flight_gufi: gufi,
            aircraft_address: ADDRESS,
        },
        centre: CENTRE,
        kind,
    }
}

fn file_route(
    gufi: u128,
    plan_id: [u8; 12],
    sequence: u64,
    source_ms: i64,
    receipt_ms: i64,
    text: &str,
) -> NormalizedEvent {
    let mut bytes = [0; oa_regional_session::input::MAX_FILED_ROUTE_BYTES];
    bytes
        .get_mut(..text.len())
        .expect("test route fits input buffer")
        .copy_from_slice(text.as_bytes());
    event(
        gufi,
        plan_id,
        sequence,
        source_ms,
        receipt_ms,
        EventKind::FiledRoute {
            plan_id,
            meter_fix: [0; 8],
            text_len: u16::try_from(text.len()).expect("test route length fits u16"),
            text: bytes,
        },
    )
}

fn cursor(
    gufi: u128,
    plan_id: [u8; 12],
    sequence: u64,
    source_ms: i64,
    receipt_ms: i64,
    digest: [u8; 32],
    fix_text: &str,
) -> NormalizedEvent {
    let mut bytes = [0; oa_regional_session::input::MAX_FILED_ROUTE_BYTES];
    bytes
        .get_mut(..fix_text.len())
        .expect("test fix fits input buffer")
        .copy_from_slice(fix_text.as_bytes());
    event(
        gufi,
        plan_id,
        sequence,
        source_ms,
        receipt_ms,
        EventKind::ActiveCursor {
            plan_id,
            route_payload_sha256: digest,
            fix_len: u16::try_from(fix_text.len()).expect("test fix length fits u16"),
            fix: bytes,
        },
    )
}

fn view(gufi: u128, plan_id: [u8; 12]) -> SharedFlightView {
    SharedFlightView {
        gufi,
        address: ADDRESS,
        centre: CENTRE,
        plan_id,
        plan_state: oa_flight::ST_ACTIVE,
        reserved_after_plan_state: [0; 3],
        plan_stamp: oa_flight::Stamp::default(),
        assigned: SharedAltitude {
            form: oa_flight::ALT_NONE,
            reserved_form: [0; 3],
            a_ft: 0,
            b_ft: 0,
        },
        reserved_after_assigned: [0; 4],
        assigned_stamp: oa_flight::Stamp::default(),
        interim: SharedAltitude {
            form: oa_flight::ALT_NONE,
            reserved_form: [0; 3],
            a_ft: 0,
            b_ft: 0,
        },
        reserved_after_interim: [0; 4],
        interim_stamp: oa_flight::Stamp::default(),
        effective_clearance: EffectiveClearance {
            kind: oa_flight::ALT_NONE,
            reserved_before_altitude: [0; 3],
            altitude_ft: 0,
            supported_as_single_level: false,
            reserved_after_supported: [0; 3],
        },
        callsign: [0; 8],
        departure: [0; 12],
        arrival: [0; 12],
        reserved_before_identity_stamp: [0; 4],
        identity_stamp: oa_flight::Stamp::default(),
        address_stamp: oa_flight::Stamp::default(),
        owner_unit: [0; 8],
        owner_stamp: oa_flight::Stamp::default(),
        handoff_state: oa_flight::HO_NONE,
        handoff_from: [0; 8],
        handoff_to: [0; 8],
        handoff_accepted_by: [0; 8],
        reserved_before_handoff_stamp: [0; 7],
        handoff_stamp: oa_flight::Stamp::default(),
        reserved_tail: [0; 8],
    }
}

fn observed() -> ObservedTrack {
    ObservedTrack {
        state_ns: 10_000_000_000,
        newest_source_ns: 10_000_000_000,
        address: ADDRESS,
        lat_e7: 410_000_000,
        lon_e7: -870_000_000,
        altitude_ft: 25_000,
        vertical_rate_fpm: 0,
        east_mmps: 100_000,
        north_mmps: 0,
        r95_mm: 1_000,
    }
}

fn store_with_route(gufi: u128, plan_id: [u8; 12]) -> RouteStore {
    let model = oa_regional_session::cli::synthetic_model().expect("synthetic model is valid");
    let mut store = route_store_new([0xA5; 32]);
    route_event(
        &mut store,
        &model,
        &file_route(gufi, plan_id, 1, 1_000, 1_000, ROUTE_TEXT),
    )
    .expect("synthetic route converts");
    store
}

#[test]
fn canonical_digest_binds_snapshot_text_and_complete_converted_provenance() {
    let gufi = 0x1234;
    let model = oa_regional_session::cli::synthetic_model().expect("synthetic model is valid");
    let mut first = route_store_new([1; 32]);
    let mut second = route_store_new([2; 32]);
    route_event(
        &mut first,
        &model,
        &file_route(gufi, PLAN, 1, 1_000, 1_000, ROUTE_TEXT),
    )
    .unwrap();
    route_event(
        &mut second,
        &model,
        &file_route(gufi, PLAN, 1, 1_000, 1_000, ROUTE_TEXT),
    )
    .unwrap();
    let first_revision = first.revisions.first().unwrap();
    let second_revision = second.revisions.first().unwrap();
    assert_ne!(
        first_revision.evidence.payload_sha256,
        second_revision.evidence.payload_sha256
    );
    assert_eq!(first_revision.evidence.key.gufi, gufi.to_be_bytes());
    assert_eq!(
        first_revision.text_sha256,
        oa_sha256::sha256(ROUTE_TEXT.as_bytes())
    );
    assert_eq!(
        first_revision.converted.source.first().copied(),
        Some(RouteSource::DirectFix as u8)
    );

    let mut changed_provenance = first_revision.converted.clone();
    *changed_provenance.procedure_ident.get_mut(0).unwrap() = oa_airspace::core::route::Label {
        bytes: *b"PROC________",
        len: 4,
    };
    *changed_provenance.gps_mea_dir.get_mut(0).unwrap() = oa_airspace::core::model::compass::NORTH;
    let digest_with_changed_provenance = canonical_route_digest(
        [1; 32],
        first_revision.text_sha256,
        first_revision.report,
        &changed_provenance,
    )
    .unwrap();
    assert_ne!(
        first_revision.evidence.payload_sha256,
        digest_with_changed_provenance
    );

    let mut changed_model = model.clone();
    *changed_model.points.lat.get_mut(0).unwrap() += 1;
    let mut changed_model_store = route_store_new([1; 32]);
    route_event(
        &mut changed_model_store,
        &changed_model,
        &file_route(gufi, PLAN, 1, 1_000, 1_000, ROUTE_TEXT),
    )
    .unwrap();
    assert_ne!(
        first_revision.evidence.payload_sha256,
        changed_model_store
            .revisions
            .first()
            .unwrap()
            .evidence
            .payload_sha256
    );

    let changed_text = "FIX:FIXB:K1:1:- FIX:FIXA:K1:1:-";
    let mut third = route_store_new([1; 32]);
    route_event(
        &mut third,
        &model,
        &file_route(gufi, PLAN, 1, 1_000, 1_000, changed_text),
    )
    .unwrap();
    assert_ne!(
        first_revision.evidence.payload_sha256,
        third.revisions.first().unwrap().evidence.payload_sha256
    );
}

#[test]
fn altitude_speed_mea_and_procedure_channels_are_mapped_independently() {
    let mut route = empty_converted_route();
    route.len = 2;
    *route.source.get_mut(0).unwrap() = RouteSource::DirectFix as u8;
    *route.source.get_mut(1).unwrap() = RouteSource::Procedure as u8;
    *route.path_term.get_mut(1).unwrap() = 18; // TF
    *route.connected.get_mut(1).unwrap() = 1;
    *route.alt_desc.get_mut(1).unwrap() = oa_airspace::core::model::alt_desc::BETWEEN;
    *route.alt1_ft.get_mut(1).unwrap() = 17_000; // BETWEEN: high
    *route.alt2_ft.get_mut(1).unwrap() = 10_000; // BETWEEN: low
    *route.mea_ft.get_mut(1).unwrap() = 12_000;
    *route.mea_dir.get_mut(1).unwrap() = oa_airspace::core::model::compass::NORTH;
    *route.speed_kt.get_mut(1).unwrap() = 450;
    *route.procedure_airport_ident.get_mut(1).unwrap() = oa_airspace::core::route::Label {
        bytes: *b"KORD________",
        len: 4,
    };
    *route.procedure_airport_region.get_mut(1).unwrap() = oa_airspace::core::route::Label {
        bytes: *b"K1__",
        len: 2,
    };
    *route.procedure_ident.get_mut(1).unwrap() = oa_airspace::core::route::Label {
        bytes: *b"STAR________",
        len: 4,
    };
    *route.procedure_transition.get_mut(1).unwrap() = oa_airspace::core::route::Label {
        bytes: *b"TRANS_______",
        len: 5,
    };
    *route.procedure_kind.get_mut(1).unwrap() = 1;
    *route.procedure_route_type.get_mut(1).unwrap() = b'A';
    let points = trajectory_points(&route).expect("supported route point channels map");
    let tf = points.get(1).unwrap();
    assert_eq!(tf.min_altitude_ft, 12_000);
    assert_eq!(tf.max_altitude_ft, 17_000);
    assert_eq!(tf.altitude_constraint_kind, AltitudeConstraintKind::Window);
    assert_eq!(tf.min_speed_mmps, t::NO_SPEED_MMPS);
    assert_eq!(tf.max_speed_mmps, 231_500);
    assert_eq!(tf.speed_constraint_kind, SpeedConstraintKind::Maximum);
    assert_eq!(tf.speed_reference, SpeedReference::IndicatedAirspeed);
    assert_eq!(tf.geometry, PathGeometry::GreatCircleLeg);

    *route.gps_mea_ft.get_mut(1).unwrap() = 14_000;
    assert!(
        trajectory_points(&route).is_err(),
        "GPS MEA is not inferred without equipment evidence"
    );
    *route.gps_mea_ft.get_mut(1).unwrap() = oa_airspace::core::model::ALT_NONE;
    *route.turn.get_mut(1).unwrap() = oa_airspace::core::model::turn::LEFT;
    assert!(
        trajectory_points(&route).is_err(),
        "turn constraints are explicitly unsupported"
    );
}

#[test]
fn path_term_and_source_pairs_have_explicit_geometry() {
    let mut route = empty_converted_route();
    route.len = 2;
    *route.source.get_mut(0).unwrap() = RouteSource::Procedure as u8;
    *route.path_term.get_mut(0).unwrap() = 15; // initial fix
    *route.source.get_mut(1).unwrap() = RouteSource::Procedure as u8;
    *route.path_term.get_mut(1).unwrap() = 7; // DF
    *route.connected.get_mut(1).unwrap() = 1;
    let points = trajectory_points(&route).unwrap();
    assert_eq!(
        points.first().unwrap().geometry,
        PathGeometry::GreatCircleLeg
    ); // no preceding route edge
    assert_eq!(points.get(1).unwrap().geometry, PathGeometry::DirectTo);

    *route.path_term.get_mut(1).unwrap() = 18; // TF
    assert_eq!(
        trajectory_points(&route).unwrap().get(1).unwrap().geometry,
        PathGeometry::GreatCircleLeg
    );
    *route.path_term.get_mut(1).unwrap() = 17; // RF is not in the accepted IF/DF/TF set
    assert!(trajectory_points(&route).is_err());

    *route.source.get_mut(1).unwrap() = RouteSource::DirectConnector as u8;
    *route.path_term.get_mut(1).unwrap() = oa_airspace::core::model::path_term::NONE;
    assert!(
        trajectory_points(&route).is_err(),
        "the converter explicitly marks a DCT edge DF"
    );
    *route.path_term.get_mut(1).unwrap() = 7;
    assert_eq!(
        trajectory_points(&route).unwrap().get(1).unwrap().geometry,
        PathGeometry::DirectTo
    );
}

#[test]
fn newer_cursor_replaces_old_unknown_fix_and_new_plan_blocks_old_cursor() {
    let gufi = 0x9876;
    let mut store = store_with_route(gufi, PLAN);
    let route_hash = store.revisions.first().unwrap().text_sha256;
    route_event(
        &mut store,
        &oa_regional_session::cli::synthetic_model().unwrap(),
        &cursor(
            gufi,
            PLAN,
            2,
            2_000,
            2_000,
            route_hash,
            "FIX:MISSING:K1:1:-",
        ),
    )
    .unwrap();
    route_event(
        &mut store,
        &oa_regional_session::cli::synthetic_model().unwrap(),
        &cursor(gufi, PLAN, 3, 3_000, 3_000, route_hash, "FIX:FIXA:K1:1:-"),
    )
    .unwrap();
    let output = route_issue(&store, &view(gufi, PLAN), &observed(), 10_000_000_000, 1)
        .expect("latest cursor is selected");
    assert_eq!(output.prior_route_points_ignored, 0);

    let next_plan = *b"PLAN00000002";
    let mut changed = store_with_route(gufi, next_plan);
    route_event(
        &mut changed,
        &oa_regional_session::cli::synthetic_model().unwrap(),
        &cursor(gufi, PLAN, 4, 4_000, 4_000, route_hash, "FIX:FIXA:K1:1:-"),
    )
    .unwrap();
    assert!(route_issue(
        &changed,
        &view(gufi, next_plan),
        &observed(),
        10_000_000_000,
        2
    )
    .is_err());
}

#[test]
fn older_same_source_conflict_is_not_hidden_by_newer_cursor() {
    let gufi = 0x4321;
    let mut store = store_with_route(gufi, PLAN);
    let route_hash = store.revisions.first().unwrap().text_sha256;
    let model = oa_regional_session::cli::synthetic_model().unwrap();
    route_event(
        &mut store,
        &model,
        &cursor(gufi, PLAN, 2, 2_000, 2_000, route_hash, "FIX:FIXA:K1:1:-"),
    )
    .unwrap();
    route_event(
        &mut store,
        &model,
        &cursor(gufi, PLAN, 3, 3_000, 3_000, route_hash, "FIX:FIXA:K1:1:-"),
    )
    .unwrap();
    route_event(
        &mut store,
        &model,
        &cursor(gufi, PLAN, 4, 2_000, 2_000, route_hash, "FIX:FIXB:K1:1:-"),
    )
    .unwrap();
    assert!(route_issue(&store, &view(gufi, PLAN), &observed(), 10_000_000_000, 3).is_err());
}

#[test]
fn cursor_timestamp_and_store_budgets_are_checked_before_storage_or_scan() {
    let gufi = 0x1111;
    let model = oa_regional_session::cli::synthetic_model().unwrap();
    let mut store = route_store_new([0; 32]);
    let bad = cursor(gufi, PLAN, 1, 2_000, 1_000, [0; 32], "FIX:FIXA:K1:1:-");
    assert!(route_event(&mut store, &model, &bad).is_err());
    assert!(store.cursors.is_empty() && store.cursor_text_hashes.is_empty());

    let fractional = event(
        gufi,
        PLAN,
        1,
        1_000,
        1_000,
        EventKind::ActiveCursor {
            plan_id: PLAN,
            route_payload_sha256: [0; 32],
            fix_len: 0,
            fix: [0; oa_regional_session::input::MAX_FILED_ROUTE_BYTES],
        },
    );
    let mut fractional = fractional;
    fractional.source_ns += 1;
    assert!(route_event(&mut store, &model, &fractional).is_err());
    assert!(store.cursors.is_empty());

    let mut over = route_store_new([0; 32]);
    let empty_cursor = t::ActiveRouteCursor {
        source_ms: 1,
        available_ms: 1,
        message_id: 1,
        route_payload_sha256: [0; 32],
        key: t::FlightKey {
            gufi: gufi.to_be_bytes(),
            aircraft_address: ADDRESS,
            centre: CENTRE,
            plan_id: PLAN,
        },
        next_fix: t::FixIdentity {
            ident: t::RouteIdentifier {
                len: 0,
                bytes: [0; 12],
            },
            region: [0; 4],
            scope: t::RouteIdentifier {
                len: 0,
                bytes: [0; 12],
            },
            class: 0,
        },
        reserved: [0; 5],
    };
    over.cursors = vec![empty_cursor; MAX_ROUTE_REVISIONS + 1];
    over.cursor_text_hashes = vec![[0; 32]; MAX_ROUTE_REVISIONS + 1];
    assert!(route_issue(&over, &view(gufi, PLAN), &observed(), 10_000_000_000, 4).is_err());

    let mut oversized = empty_converted_route();
    oversized.len = u16::MAX;
    assert!(trajectory_points(&oversized).is_err());
}

#[test]
fn explicit_interim_clear_stamp_remains_the_effective_state_provenance() {
    let gufi = 0x2222;
    let mut store = store_with_route(gufi, PLAN);
    let route_hash = store.revisions.first().unwrap().text_sha256;
    let model = oa_regional_session::cli::synthetic_model().unwrap();
    route_event(
        &mut store,
        &model,
        &cursor(gufi, PLAN, 2, 2_000, 2_000, route_hash, "FIX:FIXA:K1:1:-"),
    )
    .unwrap();
    let mut flight = view(gufi, PLAN);
    flight.assigned.form = oa_flight::ALT_SIMPLE;
    flight.assigned.a_ft = 30_000;
    flight.assigned_stamp = oa_flight::Stamp {
        ts_ms: 1_000,
        avail_ms: 1_000,
        msg: 1,
    };
    flight.interim.form = oa_flight::ALT_NIL;
    flight.interim_stamp = oa_flight::Stamp {
        ts_ms: 5_000,
        avail_ms: 9_000,
        msg: 2,
    };
    let clearance = clearance_evidence(&flight);
    assert_eq!(clearance.form, t::ClearanceForm::Exact);
    assert_eq!(clearance.min_altitude_ft, 30_000);
    assert_eq!(
        (
            clearance.source_ms,
            clearance.available_ms,
            clearance.message_id
        ),
        (5_000, 9_000, 2)
    );
    assert!(route_issue(&store, &flight, &observed(), 10_000_000_000, 5).is_ok());

    flight.plan_stamp = oa_flight::Stamp {
        ts_ms: 11_000,
        avail_ms: 11_000,
        msg: 3,
    };
    assert!(
        route_issue(&store, &flight, &observed(), 10_000_000_000, 6).is_err(),
        "a future flight snapshot is refused at the requested cutoff"
    );
}
