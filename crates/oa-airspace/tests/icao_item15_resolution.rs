#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use oa_airspace::core::model::{self, AirwayLegIx, LegIx, Meta, RouteIx, Span, StrIx};
use oa_airspace::core::route::{FixIdentity, Label};
use oa_airspace::shell::route_parse::{resolve_icao_item15, Item15Reject, MagneticVariationSource};

fn fixture() -> Model {
    Model {
        meta: Meta {
            nasr_effective: 20261101,
            cifp_cycle: 2611,
            artcc: "TEST".into(),
            buffer_m: 0,
            nasr_sha256: [0; 32],
            cifp_sha256: [0; 32],
        },
        strings: model::StrTable::default(),
        points: model::Points::default(),
        vertices: Vec::new(),
        boundaries: model::Boundaries::default(),
        segments: model::Segments::default(),
        airways: model::Airways::default(),
        airway_legs: model::AirwayLegs::default(),
        procs: model::Procs::default(),
        routes: model::Routes::default(),
        legs: model::Legs::default(),
        mora: model::Mora::default(),
        unresolved: model::Unresolved::default(),
        disagreements: model::Disagreements::default(),
        cycle_override: None,
    }
}
use model::Model;

fn intern(m: &mut Model, value: &str) -> StrIx {
    let raw = u32::try_from(m.strings.offsets.len() - 1).unwrap();
    m.strings.text.push_str(value);
    m.strings
        .offsets
        .push(u32::try_from(m.strings.text.len()).unwrap());
    StrIx::new(raw)
}
fn point(m: &mut Model, ident: &str, region: &str, kind: u8, lat: i32, lon: i32) -> model::PointIx {
    let ix = u32::try_from(m.points.kind.len()).unwrap();
    m.points.kind.push(kind);
    m.points.lat.push(lat);
    m.points.lon.push(lon);
    m.points.elev_ft.push(model::ELEV_NONE);
    let ident_ix = intern(m, ident);
    let region_ix = intern(m, region);
    m.points.ident.push(ident_ix);
    m.points.alt_ident.push(StrIx::NONE);
    m.points.region.push(region_ix);
    m.points.scope.push(StrIx::NONE);
    m.points.name.push(StrIx::NONE);
    model::PointIx::new(ix)
}
fn label<const N: usize>(value: &str) -> Label<N> {
    let mut bytes = [0; N];
    bytes[..value.len()].copy_from_slice(value.as_bytes());
    Label {
        bytes,
        len: u8::try_from(value.len()).unwrap(),
    }
}
fn identity(ident: &str, region: &str, kind: u8) -> FixIdentity {
    FixIdentity {
        ident: label(ident),
        region: label(region),
        scope: label(""),
        class: model::point_kind::class(kind),
    }
}
fn airway(m: &mut Model, a: model::PointIx, b: model::PointIx, c: model::PointIx) {
    let designator = intern(m, "J12");
    m.airways.designator.push(designator);
    m.airways.location.push(b'H');
    m.airways.legs.push(Span {
        start: AirwayLegIx::new(0),
        len: 2,
    });
    m.airway_legs.from.extend([a, b]);
    m.airway_legs.to.extend([b, c]);
    m.airway_legs.mea_ft.extend([6000, 7000]);
    m.airway_legs.mea_opp_ft.extend([8000, 9000]);
    m.airway_legs.gps_mea_ft.extend([5000, 6000]);
    m.airway_legs.mea_dir.extend([model::compass::EAST; 2]);
    m.airway_legs.gps_mea_dir.extend([model::compass::EAST; 2]);
    for _ in 0..2 {
        model::bit_push(&mut m.airway_legs.gap, false);
        model::bit_push(&mut m.airway_legs.signal_gap, false);
        model::bit_push(&mut m.airway_legs.dogleg, false);
    }
}
fn sid(m: &mut Model, airport: model::PointIx, first: model::PointIx, last: model::PointIx) {
    let ident = intern(m, "TEST1");
    m.procs.kind.push(model::proc_kind::SID);
    m.procs.airport.push(airport);
    m.procs.ident.push(ident);
    m.procs.name.push(StrIx::NONE);
    m.procs.routes.push(Span {
        start: RouteIx::new(0),
        len: 1,
    });
    model::bit_push(&mut m.procs.src_nasr, false);
    model::bit_push(&mut m.procs.src_cifp, true);
    model::bit_push(&mut m.procs.coded, true);
    let transition = intern(m, "TRANS");
    m.routes.route_type.push(b'4');
    m.routes.transition.push(transition);
    m.routes.legs.push(Span {
        start: LegIx::new(0),
        len: 2,
    });
    m.legs.fix.extend([first, last]);
    m.legs.center.extend([model::PointIx::NONE; 2]);
    m.legs.path_term.extend([15, 18]);
    m.legs.turn.extend([model::turn::NONE; 2]);
    m.legs.course_mag_cdeg.extend([model::NONE_U16; 2]);
    m.legs.dist_or_time.extend([u32::MAX; 2]);
    m.legs.alt_desc.extend([model::alt_desc::NONE; 2]);
    m.legs.alt1_ft.extend([model::ALT_NONE; 2]);
    m.legs.alt2_ft.extend([model::ALT_NONE; 2]);
    m.legs.speed_kt.extend([model::NONE_U16; 2]);
    model::bit_push(&mut m.legs.dist_is_time, false);
    model::bit_push(&mut m.legs.dist_is_time, false);
}

// REQ: OA-NAM-038
#[test]
fn item15_airway_enumeration_expands_source_legs_and_refuses_unknown_endpoints() {
    let mut m = fixture();
    let a = point(&mut m, "A", "K1", model::point_kind::FIX, 100, 200);
    let b = point(&mut m, "B", "K1", model::point_kind::FIX, 200, 300);
    let c = point(&mut m, "C", "K1", model::point_kind::FIX, 300, 400);
    airway(&mut m, a, b, c);
    let elements = ["A".to_owned(), "J12".to_owned(), "C".to_owned()];
    let result = resolve_icao_item15(&m, &elements, Some(9000), "KAAA", "KBBB", &[]).unwrap();
    assert_eq!(result.route.len, 3);
    assert_eq!(result.route.ident.get(1), Some(&label("B")));
    let bad = ["A".to_owned(), "J12".to_owned(), "NOPE".to_owned()];
    assert_eq!(
        resolve_icao_item15(&m, &bad, Some(9000), "KAAA", "KBBB", &[]),
        Err(Item15Reject::UnknownPoint)
    );
    assert_eq!(
        resolve_icao_item15(&m, &elements, Some(6500), "KAAA", "KBBB", &[]),
        Err(Item15Reject::AltitudeBelowMea)
    );
    assert_eq!(
        resolve_icao_item15(
            &m,
            &["A/A070".into(), "J12".into(), "C".into()],
            None,
            "KAAA",
            "KBBB",
            &[]
        )
        .unwrap()
        .route
        .len,
        3
    );
    assert_eq!(
        resolve_icao_item15(
            &m,
            &["A/A050".into(), "J12".into(), "C".into()],
            None,
            "KAAA",
            "KBBB",
            &[]
        ),
        Err(Item15Reject::AltitudeBelowMea)
    );
    let mut duplicate_context = fixture();
    let selected = point(
        &mut duplicate_context,
        "A",
        "K1",
        model::point_kind::FIX,
        100,
        200,
    );
    point(
        &mut duplicate_context,
        "A",
        "K2",
        model::point_kind::FIX,
        101,
        201,
    );
    let middle = point(
        &mut duplicate_context,
        "B",
        "K1",
        model::point_kind::FIX,
        200,
        300,
    );
    let end = point(
        &mut duplicate_context,
        "C",
        "K1",
        model::point_kind::FIX,
        300,
        400,
    );
    airway(&mut duplicate_context, selected, middle, end);
    let selected_route = resolve_icao_item15(
        &duplicate_context,
        &elements,
        Some(9000),
        "KAAA",
        "KBBB",
        &[],
    )
    .unwrap();
    assert_eq!(selected_route.route.region.first(), Some(&label("K1")));
}

// REQ: OA-NAM-038
#[test]
fn item15_preserves_full_identity_and_refuses_direct_ambiguity() {
    let mut m = fixture();
    point(&mut m, "DUP", "K1", model::point_kind::FIX, 100, 200);
    point(&mut m, "DUP", "K1", model::point_kind::FIX, 101, 201);
    let elements = ["DUP".to_owned()];
    assert_eq!(
        resolve_icao_item15(&m, &elements, Some(10000), "KAAA", "KBBB", &[]),
        Err(Item15Reject::AmbiguousPoint)
    );
}

// REQ: OA-NAM-038
#[test]
fn item15_selects_cifp_sid_by_departure_airport_and_expands_its_route() {
    let mut m = fixture();
    let airport = point(&mut m, "KAAA", "K1", model::point_kind::AIRPORT, 0, 0);
    let first = point(
        &mut m,
        "SIDIF",
        "K1",
        model::point_kind::TERMINAL_WAYPOINT,
        100,
        200,
    );
    let last = point(
        &mut m,
        "SIDTF",
        "K1",
        model::point_kind::TERMINAL_WAYPOINT,
        300,
        400,
    );
    sid(&mut m, airport, first, last);
    let elements = ["SIDTF".to_owned()];
    let result = resolve_icao_item15(&m, &elements, Some(9000), "KAAA", "KBBB", &[]).unwrap();
    assert_eq!(result.route.len, 2);
    assert_eq!(result.route.ident.first(), Some(&label("SIDIF")));
    assert_eq!(
        result.route.procedure_kind.first(),
        Some(&model::proc_kind::SID)
    );
}

// REQ: OA-NAM-038
#[test]
fn bearing_distance_needs_explicit_variation_and_retains_source_provenance() {
    let mut m = fixture();
    let reference_row = point(&mut m, "REF", "K1", model::point_kind::FIX, 0, 0);
    let reference = identity("REF", "K1", model::point_kind::FIX);
    let origin = oa_geo::LatLon::new(0.0, 0.0).unwrap();
    let dest = oa_geo::wgs84()
        .direct(origin, 91.0, 18520.0)
        .unwrap()
        .position;
    let target = point(
        &mut m,
        "TARGET",
        "K1",
        model::point_kind::FIX,
        oa_airspace::core::geom::deg_to_e7(dest.lat()).unwrap(),
        oa_airspace::core::geom::deg_to_e7(dest.lon()).unwrap(),
    );
    let _ = (reference_row, target);
    let elements = ["REF090010".to_owned()];
    assert_eq!(
        resolve_icao_item15(&m, &elements, Some(10000), "KAAA", "KBBB", &[]),
        Err(Item15Reject::MissingVariation)
    );
    let variation = MagneticVariationSource {
        reference,
        variation_cdeg: 100,
        epoch: Some("202611".to_owned()),
        document: "synthetic source row".to_owned(),
        locator: "row-1".to_owned(),
    };
    let result =
        resolve_icao_item15(&m, &elements, Some(10000), "KAAA", "KBBB", &[variation]).unwrap();
    assert_eq!(result.route.len, 1);
    assert_eq!(
        result
            .bearing_distance
            .first()
            .map(|e| e.bearing_magnetic_deg),
        Some(90)
    );
    assert_eq!(
        result.bearing_distance.first().map(|e| e.variation_cdeg),
        Some(100)
    );
    assert_eq!(
        result
            .bearing_distance
            .first()
            .and_then(|e| e.epoch.as_deref()),
        Some("202611")
    );
}

// REQ: OA-NAM-038
#[test]
fn item15_parses_high_levels_metric_and_cruise_climb_without_saturation() {
    let mut m = fixture();
    point(&mut m, "FIX", "K1", model::point_kind::FIX, 100, 200);
    for (token, expected) in [
        ("FIX/F600", Some(60_000)),
        ("FIX/S0840", Some(27_560)),
        ("FIX/M0840", Some(27_560)),
        ("FIX/M090F350", Some(35_000)),
        ("C/FIX/N0120F350PLUS", Some(35_000)),
        ("C/FIX/N0120PLUS", None),
    ] {
        let result =
            resolve_icao_item15(&m, &[token.to_owned()], Some(10000), "KAAA", "KBBB", &[]).unwrap();
        assert_eq!(result.route.len, 1);
        assert_eq!(result.initial_level_ft, Some(10000));
        assert_eq!(
            result
                .level_changes
                .first()
                .map(|change| change.altitude_ft),
            expected
        );
    }
    assert_eq!(
        resolve_icao_item15(
            &m,
            &["FIX/A999".to_owned()],
            Some(10000),
            "KAAA",
            "KBBB",
            &[]
        ),
        Err(Item15Reject::MalformedGroup)
    );
}

// REQ: OA-RTE-003
#[test]
fn item15_unknown_airway_and_airway_gap_refuse_without_partial_route() {
    let mut m = fixture();
    let a = point(&mut m, "A", "K1", model::point_kind::FIX, 100, 200);
    let b = point(&mut m, "B", "K1", model::point_kind::FIX, 200, 300);
    let c = point(&mut m, "C", "K1", model::point_kind::FIX, 300, 400);
    airway(&mut m, a, b, c);
    assert_eq!(
        resolve_icao_item15(
            &m,
            &["A".into(), "J99".into(), "C".into()],
            Some(9000),
            "KAAA",
            "KBBB",
            &[]
        ),
        Err(Item15Reject::UnknownAirway)
    );
    model::bit_set(&mut m.airway_legs.gap, 0, true);
    assert_eq!(
        resolve_icao_item15(
            &m,
            &["A".into(), "J12".into(), "C".into()],
            Some(9000),
            "KAAA",
            "KBBB",
            &[]
        ),
        Err(Item15Reject::Discontinuity)
    );
}
