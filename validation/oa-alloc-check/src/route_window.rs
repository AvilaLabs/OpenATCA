#![forbid(unsafe_code)]
//! C5 allocation fixture for the bounded oa-airspace route converter. Shell (B3): hand-built
//! Model, plan, and output storage are prepared before the allocator window is armed.

use crate::allocator::{self, Counts};
use oa_airspace::core::model::{self, AirwayLegIx, LegIx, PointIx, RouteIx, Span, StrIx};
use oa_airspace::core::model::{Meta, Model};
use oa_airspace::core::route::{
    convert_route, empty_converted_route, ConvertedRoute, FixIdentity, Label, RouteFailure,
    RoutePlan, RouteReport, RouteSource, RouteStep, IDENT_BYTES, REGION_BYTES, SCOPE_BYTES,
};

const RUNS: u64 = 8;

// A7.2: cold report for the named deterministic allocation fixture.
pub struct RouteEvidence {
    pub counts: Counts,
    pub iterations: u64,
    pub direct_points: u64,
    pub airway_points: u64,
    pub procedure_points: u64,
    pub connected_links: u64,
    pub refusals: u64,
    pub atomic_empty: bool,
    pub recoveries: u64,
    pub work_used: u64,
    pub work_limit: u64,
    /// The requested allocation-injection check completed successfully. This is true
    /// in normal mode because no injection was requested, and in positive-control
    /// mode because the injected allocation was observed.
    pub injection_check_passed: bool,
}

fn label<const N: usize>(text: &str) -> Result<Label<N>, &'static str> {
    if text.len() > N || text.len() > usize::from(u8::MAX) || !text.is_ascii() {
        return Err("route fixture label exceeds its fixed capacity");
    }
    let mut bytes = [0; N];
    bytes
        .get_mut(..text.len())
        .ok_or("route fixture label slice is invalid")?
        .copy_from_slice(text.as_bytes());
    Ok(Label {
        bytes,
        len: text.len() as u8,
    })
}

fn intern(model: &mut Model, text: &str) -> Result<StrIx, &'static str> {
    let index = model
        .strings
        .offsets
        .len()
        .checked_sub(1)
        .ok_or("route fixture string table is invalid")?;
    let index = u32::try_from(index).map_err(|_| "route fixture string index exceeds u32")?;
    let end = model
        .strings
        .text
        .len()
        .checked_add(text.len())
        .ok_or("route fixture string bytes overflow")?;
    let end = u32::try_from(end).map_err(|_| "route fixture string bytes exceed u32")?;
    model.strings.text.push_str(text);
    model.strings.offsets.push(end);
    Ok(StrIx::new(index))
}

fn add_point(
    model: &mut Model,
    ident: &str,
    kind: u8,
    lat: i32,
    lon: i32,
) -> Result<PointIx, &'static str> {
    let raw =
        u32::try_from(model.points.kind.len()).map_err(|_| "route fixture has too many points")?;
    let ident_ix = intern(model, ident)?;
    let region_ix = intern(model, "K1")?;
    let name_ix = intern(model, "")?;
    model.points.kind.push(kind);
    model.points.lat.push(lat);
    model.points.lon.push(lon);
    model.points.elev_ft.push(model::ELEV_NONE);
    model.points.ident.push(ident_ix);
    model.points.alt_ident.push(StrIx::NONE);
    model.points.region.push(region_ix);
    model.points.scope.push(StrIx::NONE);
    model.points.name.push(name_ix);
    Ok(PointIx::new(raw))
}

fn bit(bits: &mut model::BitSet, value: bool) {
    model::bit_push(bits, value);
}

fn fixture_model() -> Result<Model, &'static str> {
    let mut model = Model {
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
        preferred_routes: model::PreferredRoutes::default(),
        cycle_override: None,
    };

    // Point order follows the model's stable (ident, region, scope, class) key.
    add_point(&mut model, "A", model::point_kind::FIX, 100, 200)?;
    let b = add_point(&mut model, "B", model::point_kind::FIX, 300, 400)?;
    let c = add_point(&mut model, "C", model::point_kind::FIX, 500, 600)?;
    let d = add_point(&mut model, "D", model::point_kind::FIX, 700, 800)?;
    let ifix = add_point(
        &mut model,
        "IFX",
        model::point_kind::TERMINAL_WAYPOINT,
        900,
        1000,
    )?;
    let airport = add_point(&mut model, "KAAA", model::point_kind::AIRPORT, 1100, 1200)?;
    let tfix = add_point(
        &mut model,
        "TFX",
        model::point_kind::TERMINAL_WAYPOINT,
        1300,
        1400,
    )?;

    let airway_name = intern(&mut model, "J12")?;
    model.airways.designator.push(airway_name);
    model.airways.location.push(b'H');
    model.airways.legs.push(Span {
        start: AirwayLegIx::new(0),
        len: 2,
    });
    model.airway_legs.from.extend([b, c]);
    model.airway_legs.to.extend([c, d]);
    model.airway_legs.mea_ft.extend([7000, 8000]);
    model.airway_legs.mea_opp_ft.extend([9000, 10000]);
    model.airway_legs.gps_mea_ft.extend([6000, 7000]);
    model.airway_legs.mea_dir.extend([model::compass::EAST; 2]);
    model
        .airway_legs
        .gps_mea_dir
        .extend([model::compass::EAST; 2]);
    for _ in 0..2 {
        bit(&mut model.airway_legs.gap, false);
        bit(&mut model.airway_legs.signal_gap, false);
        bit(&mut model.airway_legs.dogleg, false);
    }

    let procedure_name = intern(&mut model, "SID1")?;
    let transition = intern(&mut model, "TRN1")?;
    model.procs.kind.push(model::proc_kind::SID);
    model.procs.airport.push(airport);
    model.procs.ident.push(procedure_name);
    model.procs.name.push(StrIx::NONE);
    model.procs.routes.push(Span {
        start: RouteIx::new(0),
        len: 1,
    });
    bit(&mut model.procs.src_nasr, false);
    bit(&mut model.procs.src_cifp, true);
    bit(&mut model.procs.coded, true);
    model.routes.route_type.push(b'4');
    model.routes.transition.push(transition);
    model.routes.legs.push(Span {
        start: LegIx::new(0),
        len: 2,
    });
    model.legs.fix.extend([ifix, tfix]);
    model.legs.center.extend([PointIx::NONE; 2]);
    model.legs.path_term.extend([15, 18]);
    model
        .legs
        .turn
        .extend([model::turn::NONE, model::turn::RIGHT]);
    model.legs.course_mag_cdeg.extend([model::NONE_U16; 2]);
    model.legs.dist_or_time.extend([model::NONE_U32; 2]);
    model
        .legs
        .alt_desc
        .extend([model::alt_desc::AT_OR_ABOVE, model::alt_desc::NONE]);
    model.legs.alt1_ft.extend([6000, model::ALT_NONE]);
    model.legs.alt2_ft.extend([model::ALT_NONE; 2]);
    model.legs.speed_kt.extend([model::NONE_U16, 240]);
    for _ in 0..2 {
        bit(&mut model.legs.dist_is_time, false);
    }
    Ok(model)
}

fn fixture_plan() -> Result<RoutePlan, &'static str> {
    let fix_a = FixIdentity {
        ident: label::<IDENT_BYTES>("A")?,
        region: label::<REGION_BYTES>("K1")?,
        scope: label::<SCOPE_BYTES>("")?,
        class: model::point_kind::FIX,
    };
    let fix_b = FixIdentity {
        ident: label::<IDENT_BYTES>("B")?,
        region: label::<REGION_BYTES>("K1")?,
        scope: label::<SCOPE_BYTES>("")?,
        class: model::point_kind::FIX,
    };
    let fix_d = FixIdentity {
        ident: label::<IDENT_BYTES>("D")?,
        region: label::<REGION_BYTES>("K1")?,
        scope: label::<SCOPE_BYTES>("")?,
        class: model::point_kind::FIX,
    };
    let steps = [
        RouteStep::Fix(fix_a),
        RouteStep::Direct(fix_b),
        RouteStep::Airway {
            designator: label("J12")?,
            location: b'H',
            entry: fix_b,
            exit: fix_d,
            planned_altitude_ft: 9000,
        },
        RouteStep::Procedure {
            airport_ident: label("KAAA")?,
            airport_region: label("K1")?,
            kind: model::proc_kind::SID,
            ident: label("SID1")?,
            route_type: b'4',
            transition: label("TRN1")?,
        },
        RouteStep::Fix(fix_a),
        RouteStep::Fix(fix_a),
        RouteStep::Fix(fix_a),
        RouteStep::Fix(fix_a),
        RouteStep::Fix(fix_a),
        RouteStep::Fix(fix_a),
        RouteStep::Fix(fix_a),
        RouteStep::Fix(fix_a),
        RouteStep::Fix(fix_a),
        RouteStep::Fix(fix_a),
        RouteStep::Fix(fix_a),
        RouteStep::Fix(fix_a),
        RouteStep::Fix(fix_a),
        RouteStep::Fix(fix_a),
        RouteStep::Fix(fix_a),
        RouteStep::Fix(fix_a),
        RouteStep::Fix(fix_a),
        RouteStep::Fix(fix_a),
        RouteStep::Fix(fix_a),
        RouteStep::Fix(fix_a),
        RouteStep::Fix(fix_a),
        RouteStep::Fix(fix_a),
        RouteStep::Fix(fix_a),
        RouteStep::Fix(fix_a),
        RouteStep::Fix(fix_a),
        RouteStep::Fix(fix_a),
        RouteStep::Fix(fix_a),
        RouteStep::Fix(fix_a),
    ];
    Ok(RoutePlan { steps, len: 4 })
}

fn fixture_labels_match(route: &ConvertedRoute) -> Result<bool, &'static str> {
    let expected = [
        ("A", 100, 200, model::point_kind::FIX),
        ("B", 300, 400, model::point_kind::FIX),
        ("C", 500, 600, model::point_kind::FIX),
        ("D", 700, 800, model::point_kind::FIX),
        // NASR fixes and CIFP terminal waypoints share the stable FIX identity class.
        ("IFX", 900, 1000, model::point_kind::FIX),
        ("TFX", 1300, 1400, model::point_kind::FIX),
    ];
    if route.len != 6 {
        return Ok(false);
    }
    let region = label::<REGION_BYTES>("K1")?;
    let scope = label::<SCOPE_BYTES>("")?;
    let mut i = 0usize;
    for (name, lat, lon, class) in expected {
        let name = label::<IDENT_BYTES>(name)?;
        if route.ident.get(i) != Some(&name)
            || route.region.get(i) != Some(&region)
            || route.scope.get(i) != Some(&scope)
            || route.lat_e7.get(i) != Some(&lat)
            || route.lon_e7.get(i) != Some(&lon)
            || route.class.get(i) != Some(&class)
        {
            return Ok(false);
        }
        i = i
            .checked_add(1)
            .ok_or("route fixture output index overflow")?;
    }
    Ok(true)
}

pub fn route_converter_window(inject: bool) -> Result<RouteEvidence, &'static str> {
    let model = fixture_model()?;
    let plan = fixture_plan()?;
    let mut unsupported_model = fixture_model()?;
    *unsupported_model
        .legs
        .path_term
        .get_mut(1)
        .ok_or("route fixture path terminator missing")? = 4;
    let unsupported_plan = fixture_plan()?;
    let mut output = empty_converted_route();

    if !allocator::start() {
        return Err("route converter allocation window did not arm");
    }
    let mut iterations = 0u64;
    let mut successes = 0u64;
    let mut last_report = RouteReport {
        failure: RouteFailure::InvalidInput,
        offered: 0,
        resolved: 0,
        unsupported: 0,
        refused: 0,
        unexamined: 0,
        work_used: 0,
        work_limit: 0,
    };
    for _ in 0..RUNS {
        last_report = convert_route(&model, &plan, &mut output);
        iterations = iterations.saturating_add(1);
        if last_report.failure == RouteFailure::None {
            successes = successes.saturating_add(1);
        }
    }
    let refusal_report = convert_route(&unsupported_model, &unsupported_plan, &mut output);
    iterations = iterations.saturating_add(1);
    let atomic_empty = refusal_report.failure == RouteFailure::UnsupportedPath && output.len == 0;
    let refusals = u64::from(refusal_report.refused);
    let recovery_report = convert_route(&model, &plan, &mut output);
    iterations = iterations.saturating_add(1);
    let recoveries = if recovery_report.failure == RouteFailure::None {
        1
    } else {
        0
    };
    let injection_observed = !inject || allocator::inject_core_allocation();
    if !injection_observed {
        let _ = allocator::stop();
        return Err("route converter allocation injection failed");
    }
    let counts = allocator::stop().ok_or("route converter allocation window did not disarm")?;

    if successes != RUNS
        || refusals != 1
        || !atomic_empty
        || recoveries != 1
        || !fixture_labels_match(&output)?
        || last_report.resolved != 4
        || recovery_report.resolved != 4
        || last_report.work_used == 0
        || last_report.work_used > last_report.work_limit
    {
        return Err("route converter allocation fixture did not produce its required route/refusal evidence");
    }
    let expected_sources = [
        RouteSource::DirectFix as u8,
        RouteSource::DirectConnector as u8,
        RouteSource::Airway as u8,
        RouteSource::Airway as u8,
        RouteSource::Procedure as u8,
        RouteSource::Procedure as u8,
    ];
    let expected_connected: [u8; 6] = [0, 1, 1, 1, 0, 1];
    let expected_terms: [u8; 6] = [0, 7, 0, 0, 15, 18];
    let mut direct_points = 0u64;
    let mut airway_points = 0u64;
    let mut procedure_points = 0u64;
    let mut connected_links = 0u64;
    for i in 0..usize::from(output.len) {
        let source = *output.source.get(i).ok_or("route fixture source missing")?;
        if Some(&source) != expected_sources.get(i)
            || output.connected.get(i) != expected_connected.get(i)
            || output.path_term.get(i) != expected_terms.get(i)
        {
            return Err("route converter source/path provenance changed");
        }
        match source {
            value
                if value == RouteSource::DirectFix as u8
                    || value == RouteSource::DirectConnector as u8 =>
            {
                direct_points += 1
            }
            value if value == RouteSource::Airway as u8 => airway_points += 1,
            value if value == RouteSource::Procedure as u8 => procedure_points += 1,
            _ => return Err("route converter emitted an unknown source value"),
        }
        if output.connected.get(i) == Some(&1) {
            connected_links += 1;
        }
    }
    let j12 = label::<IDENT_BYTES>("J12")?;
    let sid1 = label::<IDENT_BYTES>("SID1")?;
    let k_aaa = label::<IDENT_BYTES>("KAAA")?;
    let trn1 = label::<SCOPE_BYTES>("TRN1")?;
    let k1 = label::<REGION_BYTES>("K1")?;
    if direct_points != 2
        || airway_points != 2
        || procedure_points != 2
        || connected_links != 4
        || output.airway_designator.get(2) != Some(&j12)
        || output.airway_designator.get(3) != Some(&j12)
        || output.procedure_ident.get(4) != Some(&sid1)
        || output.procedure_ident.get(5) != Some(&sid1)
        || output.procedure_airport_ident.get(4) != Some(&k_aaa)
        || output.procedure_airport_ident.get(5) != Some(&k_aaa)
        || output.procedure_transition.get(4) != Some(&trn1)
        || output.procedure_transition.get(5) != Some(&trn1)
        || output.procedure_kind.get(4) != Some(&model::proc_kind::SID)
        || output.procedure_kind.get(5) != Some(&model::proc_kind::SID)
        || output.procedure_route_type.get(4) != Some(&b'4')
        || output.procedure_route_type.get(5) != Some(&b'4')
        || output.procedure_airport_region.get(4) != Some(&k1)
        || output.procedure_airport_region.get(5) != Some(&k1)
        || output.airway_location.get(2) != Some(&b'H')
        || output.airway_location.get(3) != Some(&b'H')
        || output.alt_desc.get(4) != Some(&model::alt_desc::AT_OR_ABOVE)
        || output.alt1_ft.get(4) != Some(&6000)
        || output.speed_kt.get(5) != Some(&240)
        || output.turn.get(5) != Some(&model::turn::RIGHT)
        || output.mea_ft.get(2) != Some(&7000)
        || output.gps_mea_ft.get(2) != Some(&6000)
    {
        return Err("route converter failed exact identity or constraint assertions");
    }
    Ok(RouteEvidence {
        counts,
        iterations,
        direct_points,
        airway_points,
        procedure_points,
        connected_links,
        refusals,
        atomic_empty,
        recoveries,
        work_used: u64::from(recovery_report.work_used),
        work_limit: u64::from(recovery_report.work_limit),
        injection_check_passed: injection_observed,
    })
}
