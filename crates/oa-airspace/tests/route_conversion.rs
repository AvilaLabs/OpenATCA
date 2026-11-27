#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use oa_airspace::core::model::{self, AirwayLegIx, LegIx, PointIx, RouteIx, Span, StrIx};
use oa_airspace::core::model::{Meta, Model};
use oa_airspace::core::route::{
    convert_route as core_convert_route, empty_converted_route, ConvertedRoute, FixIdentity, Label,
    RouteFailure, RoutePlan, RouteReport, RouteStep, IDENT_BYTES, MAX_ROUTE_POINTS,
    MAX_ROUTE_STEPS, MAX_ROUTE_WORK, REGION_BYTES, SCOPE_BYTES,
};

fn model() -> Model {
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
        preferred_route_state: model::preferred_route_state::UNAVAILABLE,
        preferred_routes: model::PreferredRoutes::default(),
        preferred_route_segments: model::PreferredRouteSegments::default(),
        mora: model::Mora::default(),
        unresolved: model::Unresolved::default(),
        disagreements: model::Disagreements::default(),
        cycle_override: None,
    }
}

fn intern(m: &mut Model, value: &str) -> StrIx {
    let raw = u32::try_from(m.strings.offsets.len() - 1).unwrap();
    m.strings.text.push_str(value);
    m.strings
        .offsets
        .push(u32::try_from(m.strings.text.len()).unwrap());
    StrIx::new(raw)
}

fn point(
    m: &mut Model,
    ident: &str,
    region: &str,
    scope: &str,
    kind: u8,
    lat: i32,
    lon: i32,
) -> PointIx {
    let index = u32::try_from(m.points.kind.len()).unwrap();
    let ident_ix = intern(m, ident);
    let region_ix = if region.is_empty() {
        StrIx::NONE
    } else {
        intern(m, region)
    };
    let scope_ix = if scope.is_empty() {
        StrIx::NONE
    } else {
        intern(m, scope)
    };
    let name_ix = intern(m, "");
    m.points.kind.push(kind);
    m.points.lat.push(lat);
    m.points.lon.push(lon);
    m.points.elev_ft.push(model::ELEV_NONE);
    m.points.ident.push(ident_ix);
    m.points.alt_ident.push(StrIx::NONE);
    m.points.region.push(region_ix);
    m.points.scope.push(scope_ix);
    m.points.name.push(name_ix);
    PointIx::new(index)
}

fn label<const N: usize>(s: &str) -> Label<N> {
    assert!(s.len() <= N);
    let mut bytes = [0; N];
    bytes[..s.len()].copy_from_slice(s.as_bytes());
    Label {
        bytes,
        len: s.len() as u8,
    }
}

fn identity(ident: &str, region: &str, scope: &str) -> FixIdentity {
    FixIdentity {
        ident: label::<IDENT_BYTES>(ident),
        region: label::<REGION_BYTES>(region),
        scope: label::<SCOPE_BYTES>(scope),
        class: model::point_kind::FIX,
    }
}

fn plan(step: RouteStep) -> RoutePlan {
    RoutePlan {
        steps: [step; MAX_ROUTE_STEPS],
        len: 1,
    }
}

fn add_bit(bits: &mut model::BitSet, value: bool) {
    model::bit_push(bits, value);
}

struct Run {
    route: ConvertedRoute,
    report: RouteReport,
}
fn convert(m: &Model, plan: &RoutePlan) -> Run {
    let mut route = empty_converted_route();
    let report = core_convert_route(m, plan, &mut route);
    Run { route, report }
}

// REQ: OA-RTE-001
#[test]
fn direct_fix_is_full_identity_and_dct_is_explicit() {
    let mut m = model();
    let a = point(&mut m, "FIXA", "K1", "", model::point_kind::FIX, 100, 200);
    let _same_elsewhere = point(&mut m, "FIXA", "K2", "", model::point_kind::FIX, 100, 200);
    let _b = point(&mut m, "FIXB", "K1", "", model::point_kind::FIX, 300, 400);
    let direct = convert(&m, &plan(RouteStep::Fix(identity("FIXA", "K1", ""))));
    assert_eq!(direct.report.failure, RouteFailure::None);
    assert_eq!(direct.route.len, 1);
    assert_eq!(direct.route.region[0], label("K1"));
    assert_eq!(direct.route.connected[0], 0);
    assert_eq!(a.get(), 0);

    let mut steps = [RouteStep::Fix(identity("FIXA", "K1", "")); MAX_ROUTE_STEPS];
    steps[1] = RouteStep::Direct(identity("FIXB", "K1", ""));
    let dct = convert(&m, &RoutePlan { steps, len: 2 });
    assert_eq!(dct.report.failure, RouteFailure::None);
    assert_eq!(dct.route.len, 2);
    assert_eq!(dct.route.connected[1], 1);
    assert_eq!(dct.route.path_term[1], 7);
}

// REQ: OA-RTE-002
#[test]
fn duplicate_full_identities_are_ambiguous() {
    let mut m = model();
    point(&mut m, "DUP", "K1", "", model::point_kind::FIX, 0, 0);
    point(&mut m, "DUP", "K1", "", model::point_kind::FIX, 1, 1);
    let result = convert(&m, &plan(RouteStep::Fix(identity("DUP", "K1", ""))));
    assert_eq!(result.report.failure, RouteFailure::Ambiguous);
    assert_eq!(result.route.len, 0);
    assert_eq!(result.report.refused, 1);
}

// REQ: OA-RTE-003
#[test]
fn airway_expands_only_contiguous_legs_and_keeps_mea() {
    let mut m = model();
    let a = point(&mut m, "A", "K1", "", model::point_kind::FIX, 0, 0);
    let b = point(&mut m, "B", "K1", "", model::point_kind::FIX, 100, 100);
    let c = point(&mut m, "C", "K1", "", model::point_kind::FIX, 200, 200);
    let designator = intern(&mut m, "J12");
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
        add_bit(&mut m.airway_legs.gap, false);
        add_bit(&mut m.airway_legs.signal_gap, false);
        add_bit(&mut m.airway_legs.dogleg, false);
    }
    let step = RouteStep::Airway {
        designator: label("J12"),
        location: b'H',
        entry: identity("A", "K1", ""),
        exit: identity("C", "K1", ""),
        planned_altitude_ft: 7000,
    };
    let result = convert(&m, &plan(step));
    assert_eq!(result.report.failure, RouteFailure::None);
    assert_eq!(result.route.len, 3);
    assert_eq!(result.route.ident[2], label("C"));
    assert_eq!(result.route.connected[1], 1);
    assert_eq!(result.route.mea_ft[1], 6000);
    assert_eq!(result.route.gps_mea_ft[1], 5000);
    assert_eq!(result.route.airway_designator[1], label("J12"));

    m.airway_legs.gps_mea_ft.fill(model::ALT_NONE);
    m.airway_legs.gps_mea_dir.fill(model::compass::NONE);
    let reverse_step = RouteStep::Airway {
        designator: label("J12"),
        location: b'H',
        entry: identity("C", "K1", ""),
        exit: identity("A", "K1", ""),
        planned_altitude_ft: 9000,
    };
    let reverse = convert(&m, &plan(reverse_step));
    assert_eq!(reverse.report.failure, RouteFailure::UnsupportedConstraint);
    assert_eq!(reverse.route.len, 0);

    model::bit_set(&mut m.airway_legs.gap, 0, true);
    let refused = convert(&m, &plan(step));
    assert_eq!(refused.report.failure, RouteFailure::AirwayGap);
    assert_eq!(refused.route.len, 0);
}

// REQ: OA-RTE-004
#[test]
fn cifp_if_tf_transition_preserves_constraints_and_identity() {
    let mut m = model();
    let apt = point(&mut m, "KAAA", "K1", "", model::point_kind::AIRPORT, 10, 20);
    let ifix = point(
        &mut m,
        "IFIX",
        "K1",
        "",
        model::point_kind::TERMINAL_WAYPOINT,
        30,
        40,
    );
    let tfix = point(
        &mut m,
        "TFIX",
        "K1",
        "",
        model::point_kind::TERMINAL_WAYPOINT,
        50,
        60,
    );
    let proc_ident = intern(&mut m, "TEST1");
    let transition = intern(&mut m, "TRANS");
    m.procs.kind.push(model::proc_kind::SID);
    m.procs.airport.push(apt);
    m.procs.ident.push(proc_ident);
    m.procs.name.push(StrIx::NONE);
    m.procs.routes.push(Span {
        start: RouteIx::new(0),
        len: 1,
    });
    add_bit(&mut m.procs.src_nasr, false);
    add_bit(&mut m.procs.src_cifp, true);
    add_bit(&mut m.procs.coded, true);
    m.routes.route_type.push(b'4');
    m.routes.transition.push(transition);
    m.routes.legs.push(Span {
        start: LegIx::new(0),
        len: 2,
    });
    m.legs.fix.extend([ifix, tfix]);
    m.legs.center.extend([PointIx::NONE; 2]);
    m.legs.path_term.extend([15, 18]);
    m.legs.turn.extend([model::turn::NONE, model::turn::RIGHT]);
    m.legs.course_mag_cdeg.extend([model::NONE_U16; 2]);
    m.legs.dist_or_time.extend([u32::MAX; 2]);
    m.legs
        .alt_desc
        .extend([model::alt_desc::AT_OR_ABOVE, model::alt_desc::NONE]);
    m.legs.alt1_ft.extend([5000, model::ALT_NONE]);
    m.legs.alt2_ft.extend([model::ALT_NONE; 2]);
    m.legs.speed_kt.extend([250, model::NONE_U16]);
    for _ in 0..2 {
        add_bit(&mut m.legs.dist_is_time, false);
    }
    let step = RouteStep::Procedure {
        airport_ident: label("KAAA"),
        airport_region: label("K1"),
        kind: model::proc_kind::SID,
        ident: label("TEST1"),
        route_type: b'4',
        transition: label("TRANS"),
    };
    let result = convert(&m, &plan(step));
    assert_eq!(result.report.failure, RouteFailure::None);
    assert_eq!(result.route.len, 2);
    assert_eq!(result.route.connected[0], 0);
    assert_eq!(result.route.connected[1], 1);
    assert_eq!(result.route.alt_desc[0], model::alt_desc::AT_OR_ABOVE);
    assert_eq!(result.route.alt1_ft[0], 5000);
    assert_eq!(result.route.turn[1], model::turn::RIGHT);
    assert_eq!(result.route.procedure_transition[1], label("TRANS"));
    assert_eq!(result.route.procedure_route_type[1], b'4');

    m.points.kind[apt.row()] = model::point_kind::FIX;
    assert_eq!(
        convert(&m, &plan(step)).report.failure,
        RouteFailure::MalformedModel
    );
    m.points.kind[apt.row()] = model::point_kind::AIRPORT;
    m.legs.alt1_ft[0] = model::ALT_NONE;
    assert_eq!(
        convert(&m, &plan(step)).report.failure,
        RouteFailure::MalformedModel
    );
    m.legs.alt1_ft[0] = 5_000;
    m.legs.alt_desc[0] = model::alt_desc::BETWEEN;
    m.legs.alt2_ft[0] = 6_000;
    assert_eq!(
        convert(&m, &plan(step)).report.failure,
        RouteFailure::MalformedModel
    );
    m.legs.alt_desc[0] = model::alt_desc::NONE;
    m.legs.alt2_ft[0] = model::ALT_NONE;
    assert_eq!(
        convert(&m, &plan(step)).report.failure,
        RouteFailure::UnsupportedConstraint
    );
    m.legs.alt_desc[0] = model::alt_desc::AT_OR_ABOVE;

    m.legs.path_term[1] = 4;
    let unsupported = convert(&m, &plan(step));
    assert_eq!(unsupported.report.failure, RouteFailure::UnsupportedPath);
    assert_eq!(unsupported.route.len, 0);
    assert_eq!(unsupported.report.unsupported, 1);
}

// REQ: OA-RTE-005
#[test]
fn malformed_columns_work_and_step_limits_refuse_atomically() {
    let mut m = model();
    point(&mut m, "A", "K1", "", model::point_kind::FIX, 0, 0);
    let step = RouteStep::Fix(identity("A", "K1", ""));
    m.points.lon.clear();
    let malformed = convert(&m, &plan(step));
    assert_eq!(malformed.report.failure, RouteFailure::MalformedModel);
    assert_eq!(malformed.route.len, 0);

    let mut m = model();
    m.airways
        .designator
        .resize(MAX_ROUTE_WORK as usize + 1, StrIx::NONE);
    m.airways.location.resize(MAX_ROUTE_WORK as usize + 1, b'H');
    m.airways.legs.resize(
        MAX_ROUTE_WORK as usize + 1,
        Span {
            start: AirwayLegIx::new(0),
            len: 0,
        },
    );
    let airway = RouteStep::Airway {
        designator: label("J12"),
        location: b'H',
        entry: identity("A", "K1", ""),
        exit: identity("B", "K1", ""),
        planned_altitude_ft: 9000,
    };
    let overwork = convert(&m, &plan(airway));
    assert_eq!(overwork.report.failure, RouteFailure::WorkLimit);
    assert_eq!(overwork.report.work_used, MAX_ROUTE_WORK);
    assert_eq!(overwork.route.len, 0);

    let too_many = RoutePlan {
        steps: [RouteStep::Fix(identity("A", "K1", "")); MAX_ROUTE_STEPS],
        len: MAX_ROUTE_STEPS as u8 + 1,
    };
    let limited = convert(&m, &too_many);
    assert_eq!(limited.report.failure, RouteFailure::InvalidInput);
    assert!(limited.route.len as usize <= MAX_ROUTE_POINTS);
    assert_eq!(limited.report.unexamined, too_many.len);
    assert_eq!(limited.report.refused, 0);
}

// REQ: OA-RTE-002
#[test]
fn unsorted_duplicate_keys_are_never_hidden_by_lookup_order() {
    let mut m = model();
    point(&mut m, "A", "K1", "", model::point_kind::FIX, 0, 0);
    point(&mut m, "B", "K1", "", model::point_kind::FIX, 1, 1);
    point(&mut m, "A", "K1", "", model::point_kind::FIX, 2, 2);
    let result = convert(&m, &plan(RouteStep::Fix(identity("A", "K1", ""))));
    assert_eq!(result.report.failure, RouteFailure::Ambiguous);
    assert_eq!(result.route.len, 0);
}
