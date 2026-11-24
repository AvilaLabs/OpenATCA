//! Offline route/flight/trajectory bridge (shell, B1/B3/C6). Converted geometry and every
//! restriction remain bound to the snapshot, full flight key and exact admitted route bytes.
//! This adapter never guesses an active leg, identity, airspeed or altitude crossing solution.

use crate::flight::SharedFlightView;
use crate::input::{EventKind, NormalizedEvent, SourceKind, MAX_FILED_ROUTE_BYTES};
use crate::surveillance::ObservedTrack;
use oa_airspace::core::model::{self, Model};
use oa_airspace::core::route::{
    convert_route, empty_converted_route, ConvertedRoute, RouteFailure, RouteReport, RouteSource,
    RouteStep,
};
use oa_airspace::shell::route_cli::parse_route_steps;
use oa_legacy_traj::core::trajectory::{
    self as t, AltitudeConstraintKind as A, SpeedConstraintKind as S,
};

/// C6/C7: one session retains at most 256 route/cursor revisions across all flights, matching
/// the shared selector's bound. Exhaustion stops the run before conversion or storage.
pub const MAX_ROUTE_REVISIONS: usize = 256;
/// Maximum bytes in the versioned canonical route payload encoding. The checked maximum is
/// 91-byte header + 256 active points × 125 bytes per point = 32,091 bytes.
const MAX_ROUTE_HASH_BYTES: usize = 512 * 1024;
const ROUTE_HASH_HEADER_BYTES: usize = 91;
const ROUTE_HASH_POINT_BYTES: usize = 125;
const MAX_CURSOR_COMPARISONS: usize = MAX_ROUTE_REVISIONS * (MAX_ROUTE_REVISIONS - 1) / 2;
const PATH_DF: u8 = 7;
const PATH_IF: u8 = 15;
const PATH_TF: u8 = 18;

// A3/C6: cold immutable offline revisions, capped above. Complete converter output retains
// airway/procedure/MEA/turn provenance even where the baseline predictor cannot use it.
pub struct RouteRevision {
    pub evidence: t::RevisionEvidence,
    pub text_sha256: [u8; 32],
    pub report: RouteReport,
    pub converted: ConvertedRoute,
}
// A7.2: cold shell container, size guard waived.
pub struct RouteStore {
    pub revisions: Vec<RouteRevision>,
    pub cursors: Vec<t::ActiveRouteCursor>,
    pub cursor_text_hashes: Vec<[u8; 32]>,
    pub rejected_legacy_routes: u64,
    pub snapshot_sha256: [u8; 32],
}

pub fn route_store_new(snapshot_sha256: [u8; 32]) -> RouteStore {
    RouteStore {
        revisions: Vec::with_capacity(MAX_ROUTE_REVISIONS),
        cursors: Vec::with_capacity(MAX_ROUTE_REVISIONS),
        cursor_text_hashes: Vec::with_capacity(MAX_ROUTE_REVISIONS),
        rejected_legacy_routes: 0,
        snapshot_sha256,
    }
}

pub fn flight_key(view: &SharedFlightView) -> t::FlightKey {
    t::FlightKey {
        gufi: view.gufi.to_be_bytes(),
        aircraft_address: view.address,
        centre: view.centre,
        plan_id: view.plan_id,
    }
}

fn event_key(event: &NormalizedEvent, plan_id: [u8; 12]) -> t::FlightKey {
    t::FlightKey {
        gufi: event.identity.flight_gufi.to_be_bytes(),
        aircraft_address: event.identity.aircraft_address,
        centre: event.centre,
        plan_id,
    }
}

fn ms(ns: i64) -> Result<i64, String> {
    if ns < 0 || ns % 1_000_000 != 0 {
        return Err("trajectory evidence requires exact whole milliseconds".into());
    }
    if oa_sealed::in_sealed_window(ns) {
        return Err("sealed timestamp refused".into());
    }
    Ok(ns / 1_000_000)
}

fn bounded_text(bytes: &[u8], len: u16) -> Result<&str, String> {
    std::str::from_utf8(
        bytes
            .get(..usize::from(len))
            .ok_or("text length exceeds stored payload")?,
    )
    .map_err(|_| "invalid route text".into())
}

fn hash_size(point_count: usize) -> Result<usize, String> {
    if point_count > t::MAX_ROUTE_POINTS {
        return Err("converted route exceeds trajectory capacity".into());
    }
    let points_bytes = point_count
        .checked_mul(ROUTE_HASH_POINT_BYTES)
        .ok_or("route hash size overflow")?;
    let total = ROUTE_HASH_HEADER_BYTES
        .checked_add(points_bytes)
        .ok_or("route hash size overflow")?;
    if total > MAX_ROUTE_HASH_BYTES {
        return Err("route hash encoding budget exceeded".into());
    }
    Ok(total)
}

fn encode_label<const N: usize>(bytes: &mut Vec<u8>, label: oa_airspace::core::route::Label<N>) {
    bytes.extend_from_slice(&label.bytes);
    bytes.push(label.len);
}

/// Canonical V1 digest input. Integer fields are little-endian; only active point rows are
/// encoded. Every converted source/procedure/MEA/turn field participates in the digest.
pub fn canonical_route_digest(
    snapshot: [u8; 32],
    text_sha256: [u8; 32],
    report: RouteReport,
    route: &ConvertedRoute,
) -> Result<[u8; 32], String> {
    let n = usize::from(route.len);
    let expected = hash_size(n)?;
    let mut bytes = Vec::with_capacity(expected);
    bytes.extend_from_slice(b"OA-ROUTE\x01");
    bytes.extend_from_slice(&snapshot);
    bytes.extend_from_slice(&text_sha256);
    bytes.push(report.failure as u8);
    bytes.extend_from_slice(&[
        report.offered,
        report.resolved,
        report.unsupported,
        report.refused,
        report.unexamined,
    ]);
    bytes.extend_from_slice(&report.work_used.to_le_bytes());
    bytes.extend_from_slice(&report.work_limit.to_le_bytes());
    bytes.extend_from_slice(&route.len.to_le_bytes());
    bytes.extend_from_slice(&route.reserved);
    for i in 0..n {
        bytes.extend_from_slice(&value(&route.lat_e7, i)?.to_le_bytes());
        bytes.extend_from_slice(&value(&route.lon_e7, i)?.to_le_bytes());
        encode_label(&mut bytes, value(&route.ident, i)?);
        encode_label(&mut bytes, value(&route.region, i)?);
        encode_label(&mut bytes, value(&route.scope, i)?);
        bytes.extend_from_slice(&[
            value(&route.class, i)?,
            value(&route.source, i)?,
            value(&route.path_term, i)?,
            value(&route.alt_desc, i)?,
        ]);
        bytes.extend_from_slice(&value(&route.alt1_ft, i)?.to_le_bytes());
        bytes.extend_from_slice(&value(&route.alt2_ft, i)?.to_le_bytes());
        bytes.extend_from_slice(&value(&route.speed_kt, i)?.to_le_bytes());
        bytes.push(value(&route.turn, i)?);
        bytes.extend_from_slice(&value(&route.mea_ft, i)?.to_le_bytes());
        bytes.extend_from_slice(&value(&route.gps_mea_ft, i)?.to_le_bytes());
        bytes.push(value(&route.mea_dir, i)?);
        bytes.push(value(&route.gps_mea_dir, i)?);
        encode_label(&mut bytes, value(&route.airway_designator, i)?);
        bytes.push(value(&route.airway_location, i)?);
        encode_label(&mut bytes, value(&route.procedure_airport_ident, i)?);
        encode_label(&mut bytes, value(&route.procedure_airport_region, i)?);
        encode_label(&mut bytes, value(&route.procedure_ident, i)?);
        encode_label(&mut bytes, value(&route.procedure_transition, i)?);
        bytes.push(value(&route.procedure_kind, i)?);
        bytes.push(value(&route.procedure_route_type, i)?);
        bytes.push(value(&route.connected, i)?);
    }
    if bytes.len() != expected {
        return Err("canonical route encoding size mismatch".into());
    }
    Ok(oa_sha256::sha256(&bytes))
}

fn fix(key: oa_airspace::core::route::FixIdentity) -> t::FixIdentity {
    t::FixIdentity {
        ident: t::RouteIdentifier {
            len: key.ident.len,
            bytes: key.ident.bytes,
        },
        region: key.region.bytes,
        scope: t::RouteIdentifier {
            len: key.scope.len,
            bytes: key.scope.bytes,
        },
        class: key.class,
    }
}

/// Admit route intent only after its receipt batch becomes visible. Bad conversion is retained
/// as a failed revision; it never silently falls back to an older successful route.
pub fn route_event(
    store: &mut RouteStore,
    model: &Model,
    event: &NormalizedEvent,
) -> Result<(), String> {
    if event.source_ns > event.receipt_ns {
        return Err("route source after availability".into());
    }
    match event.kind {
        EventKind::FiledRoute {
            plan_id,
            text_len,
            text,
            ..
        } => {
            if event.source != SourceKind::Route {
                return Err("filed route has wrong source kind".into());
            }
            if store.revisions.len() >= MAX_ROUTE_REVISIONS {
                return Err("route revision capacity exhausted".into());
            }
            let source_ms = ms(event.source_ns)?;
            let available_ms = ms(event.receipt_ns)?;
            if usize::from(text_len) > MAX_FILED_ROUTE_BYTES {
                return Err("filed route text exceeds input limit".into());
            }
            let _maximum_encoding_size = hash_size(t::MAX_ROUTE_POINTS)?;
            let text = bounded_text(&text, text_len)?;
            let plan =
                parse_route_steps(text).map_err(|e| format!("normalized route parse: {e:?}"))?;
            let mut converted = empty_converted_route();
            let report = convert_route(model, &plan, &mut converted);
            let text_sha256 = oa_sha256::sha256(text.as_bytes());
            let payload_sha256 =
                canonical_route_digest(store.snapshot_sha256, text_sha256, report, &converted)?;
            let evidence = t::RevisionEvidence {
                source_ms,
                available_ms,
                message_id: event.sequence,
                payload_sha256,
                key: event_key(event, plan_id),
                reserved: [0; 4],
            };
            store.revisions.push(RouteRevision {
                evidence,
                text_sha256,
                report,
                converted,
            });
        }
        EventKind::ActiveCursor {
            plan_id,
            route_payload_sha256,
            fix_len,
            fix: text,
        } => {
            if event.source != SourceKind::Route {
                return Err("active route cursor has wrong source kind".into());
            }
            if store.cursors.len() >= MAX_ROUTE_REVISIONS {
                return Err("route cursor capacity exhausted".into());
            }
            let source_ms = ms(event.source_ns)?;
            let available_ms = ms(event.receipt_ns)?;
            if usize::from(fix_len) > MAX_FILED_ROUTE_BYTES {
                return Err("cursor identity text exceeds input limit".into());
            }
            let plan = parse_route_steps(bounded_text(&text, fix_len)?)
                .map_err(|e| format!("cursor identity parse: {e:?}"))?;
            if plan.len != 1 {
                return Err("cursor requires one full FIX identity".into());
            }
            let next = match plan.steps.first() {
                Some(RouteStep::Fix(key)) => fix(*key),
                _ => {
                    return Err(
                        "cursor requires a FIX identity, not a connector or procedure".into(),
                    )
                }
            };
            store.cursors.push(t::ActiveRouteCursor {
                source_ms,
                available_ms,
                message_id: event.sequence,
                route_payload_sha256: [0; 32],
                key: event_key(event, plan_id),
                next_fix: next,
                reserved: [0; 5],
            });
            // The external digest identifies exact filed text. On issue it is resolved to the
            // complete converted payload digest (snapshot + text + all converted fields).
            store.cursor_text_hashes.push(route_payload_sha256);
        }
        EventKind::Route { .. } => {
            store.rejected_legacy_routes = store.rejected_legacy_routes.saturating_add(1);
        }
        _ => {}
    }
    Ok(())
}

fn value<T: Copy>(column: &[T], i: usize) -> Result<T, String> {
    column
        .get(i)
        .copied()
        .ok_or_else(|| "inconsistent converted route columns".into())
}

fn same_flight(a: t::FlightKey, b: t::FlightKey) -> bool {
    a.gufi == b.gufi && a.aircraft_address == b.aircraft_address && a.centre == b.centre
}

fn stamp_in_cutoff(stamp: oa_flight::Stamp, cutoff_ms: i64) -> bool {
    stamp.ts_ms >= 0
        && stamp.avail_ms >= 0
        && stamp.ts_ms <= stamp.avail_ms
        && stamp.avail_ms <= cutoff_ms
}

fn flight_view_in_cutoff(view: &SharedFlightView, cutoff_ms: i64) -> bool {
    [
        view.plan_stamp,
        view.assigned_stamp,
        view.interim_stamp,
        view.identity_stamp,
        view.address_stamp,
        view.owner_stamp,
        view.handoff_stamp,
    ]
    .iter()
    .all(|stamp| stamp_in_cutoff(*stamp, cutoff_ms))
}

pub fn trajectory_points(route: &ConvertedRoute) -> Result<Vec<t::RoutePoint>, String> {
    let n = usize::from(route.len);
    if n > t::MAX_ROUTE_POINTS {
        return Err("converted route exceeds trajectory capacity".into());
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let source = value(&route.source, i)?;
        let path_term = value(&route.path_term, i)?;
        let connected = value(&route.connected, i)?;
        let turn = value(&route.turn, i)?;
        if turn != model::turn::NONE {
            return Err("route turn constraint is unsupported by the trajectory model".into());
        }
        let source_is_direct_fix = source == RouteSource::DirectFix as u8;
        let source_is_direct_connector = source == RouteSource::DirectConnector as u8;
        let source_is_airway = source == RouteSource::Airway as u8;
        let source_is_procedure = source == RouteSource::Procedure as u8;
        if !(source_is_direct_fix
            || source_is_direct_connector
            || source_is_airway
            || source_is_procedure)
        {
            return Err("converted route has an unknown source code".into());
        }
        if connected > 1 {
            return Err("converted route has invalid connected flag".into());
        }
        if (source_is_direct_fix || source_is_airway) && path_term != model::path_term::NONE {
            return Err("non-procedure route point has a procedure path term".into());
        }
        if source_is_direct_connector && path_term != PATH_DF {
            return Err("explicit direct connector requires its converted DF path term".into());
        }
        if source_is_procedure && !matches!(path_term, PATH_IF | PATH_DF | PATH_TF) {
            return Err("procedure path term is unsupported by trajectory geometry".into());
        }
        let gps_mea = value(&route.gps_mea_ft, i)?;
        let gps_mea_dir = value(&route.gps_mea_dir, i)?;
        if gps_mea != model::ALT_NONE || gps_mea_dir != model::compass::NONE {
            return Err("GPS MEA cannot be applied without aircraft equipment evidence".into());
        }
        let mut low = t::NO_ALTITUDE_FT;
        let mut high = t::NO_ALTITUDE_FT;
        let mut altitude_kind = match value(&route.alt_desc, i)? {
            model::alt_desc::NONE => A::None,
            model::alt_desc::AT_OR_ABOVE => {
                low = value(&route.alt1_ft, i)?;
                A::Minimum
            }
            model::alt_desc::BETWEEN => {
                low = value(&route.alt2_ft, i)?;
                high = value(&route.alt1_ft, i)?;
                A::Window
            }
            _ => A::Unsupported,
        };
        let mea = value(&route.mea_ft, i)?;
        // A direction-scoped conventional MEA is conservatively enforced in every direction.
        // This can refuse a route that would not need the MEA in its actual direction, but never
        // weakens the bound. The exact direction remains in the canonical route evidence.
        if mea != model::ALT_NONE {
            low = if low == t::NO_ALTITUDE_FT {
                mea
            } else {
                low.max(mea)
            };
            altitude_kind = if altitude_kind == A::Unsupported {
                A::Unsupported
            } else if high != t::NO_ALTITUDE_FT {
                A::Window
            } else {
                A::Minimum
            };
        }
        let speed = value(&route.speed_kt, i)?;
        let has_speed = speed != model::NONE_U16;
        // Exact rational kt -> mm/s, rounded to nearest integer before checked narrowing.
        let speed_mmps = if has_speed {
            u32::from(speed)
                .checked_mul(463_000)
                .and_then(|v| v.checked_add(450))
                .map(|v| v / 900)
                .ok_or("speed conversion overflow")?
        } else {
            t::NO_SPEED_MMPS
        };
        // `connected` records a supported coded edge. Convert only path/source pairs whose
        // geometry is represented by the trajectory model. IF is a fix initializer, not an
        // incoming segment; DF and explicit direct connectors are direct-to, TF and airway legs
        // are great-circle legs. A first route point has no preceding route edge.
        let geometry = if i == 0 {
            t::PathGeometry::GreatCircleLeg
        } else if connected == 0 {
            t::PathGeometry::VectorsGap
        } else if source_is_direct_connector {
            t::PathGeometry::DirectTo
        } else if source_is_airway || (source_is_procedure && path_term == PATH_TF) {
            t::PathGeometry::GreatCircleLeg
        } else if source_is_procedure && path_term == PATH_DF {
            t::PathGeometry::DirectTo
        } else if source_is_procedure && path_term == PATH_IF {
            return Err("IF procedure leg cannot claim a connected incoming path".into());
        } else {
            return Err("connected route source/path-term has unsupported geometry".into());
        };
        out.push(t::RoutePoint {
            lat_e7: value(&route.lat_e7, i)?,
            lon_e7: value(&route.lon_e7, i)?,
            min_altitude_ft: low,
            max_altitude_ft: high,
            min_speed_mmps: t::NO_SPEED_MMPS,
            max_speed_mmps: speed_mmps,
            fix: fix(oa_airspace::core::route::FixIdentity {
                ident: value(&route.ident, i)?,
                region: value(&route.region, i)?,
                scope: value(&route.scope, i)?,
                class: value(&route.class, i)?,
            }),
            altitude_constraint_kind: altitude_kind,
            speed_constraint_kind: if has_speed { S::Maximum } else { S::None },
            speed_reference: if has_speed {
                t::SpeedReference::IndicatedAirspeed
            } else {
                t::SpeedReference::Unknown
            },
            geometry,
            reserved: [0; 1],
        });
    }
    Ok(out)
}

fn later_stamp(a: oa_flight::Stamp, b: oa_flight::Stamp) -> oa_flight::Stamp {
    if (a.avail_ms, a.ts_ms, a.msg) >= (b.avail_ms, b.ts_ms, b.msg) {
        a
    } else {
        b
    }
}

/// Build the predictor's effective-clearance evidence from the separately stamped assigned and
/// interim fields. Explicit interim NIL uses assigned value with the later field stamp.
pub fn clearance_evidence(view: &SharedFlightView) -> t::Clearance {
    // SharedFlightView retains assigned and interim values and stamps independently. Its
    // effective_clearance is a convenience summary; derive the supported form here from those
    // source fields so unsupported non-NIL interim forms cannot fall back to assigned. For an
    // explicit interim NIL, the effective value is assigned while the later assigned/clear stamp
    // preserves when that effective state became current.
    let (alt, stamp) = match view.interim.form {
        oa_flight::ALT_NONE => (view.assigned, view.assigned_stamp),
        oa_flight::ALT_NIL => (
            view.assigned,
            later_stamp(view.assigned_stamp, view.interim_stamp),
        ),
        _ => (view.interim, view.interim_stamp),
    };
    let (form, low, high, at_fix) = match alt.form {
        oa_flight::ALT_SIMPLE => (
            t::ClearanceForm::Exact,
            alt.a_ft,
            alt.a_ft,
            t::NO_ALTITUDE_FT,
        ),
        oa_flight::ALT_BLOCK => (
            t::ClearanceForm::Block,
            alt.a_ft,
            alt.b_ft,
            t::NO_ALTITUDE_FT,
        ),
        oa_flight::ALT_AT_FIX => (
            t::ClearanceForm::AltitudeAtFix,
            t::NO_ALTITUDE_FT,
            t::NO_ALTITUDE_FT,
            alt.a_ft,
        ),
        oa_flight::ALT_NONE | oa_flight::ALT_NIL => (
            t::ClearanceForm::Absent,
            t::NO_ALTITUDE_FT,
            t::NO_ALTITUDE_FT,
            t::NO_ALTITUDE_FT,
        ),
        oa_flight::ALT_VFR => (
            t::ClearanceForm::Vfr,
            t::NO_ALTITUDE_FT,
            t::NO_ALTITUDE_FT,
            t::NO_ALTITUDE_FT,
        ),
        _ => (
            t::ClearanceForm::Unsupported,
            t::NO_ALTITUDE_FT,
            t::NO_ALTITUDE_FT,
            t::NO_ALTITUDE_FT,
        ),
    };
    t::Clearance {
        source_ms: stamp.ts_ms,
        available_ms: stamp.avail_ms,
        message_id: stamp.msg,
        min_altitude_ft: low,
        max_altitude_ft: high,
        altitude_at_fix_ft: at_fix,
        form,
        reserved: [0; 3],
    }
}

struct PreparedRouteIssue {
    key: t::FlightKey,
    route: t::RouteProvenance,
    cutoff_ms: i64,
    max_track_age_ms: i64,
    owner: t::OwnerProvenance,
    route_cursor: Option<t::ActiveRouteCursor>,
    track: t::CurrentTrack,
    clearance: t::Clearance,
    eta_target: t::EtaTarget,
    points: Vec<t::RoutePoint>,
}

impl PreparedRouteIssue {
    fn input(&self) -> t::TrajectoryIssueInput<'_> {
        t::TrajectoryIssueInput {
            key: self.key,
            route: self.route,
            cutoff_ms: self.cutoff_ms,
            max_track_age_ms: self.max_track_age_ms,
            owner: self.owner,
            route_cursor: self.route_cursor,
            track: self.track,
            clearance: self.clearance,
            eta_target: self.eta_target,
            points: &self.points,
        }
    }
}

/// Converts one receipt into the exact issue-time values shared by horizontal and optional
/// vertical forecasting. Both modes use this selector, cursor, track, clearance and route slice.
fn prepare_route_issue(
    store: &RouteStore,
    view: &SharedFlightView,
    observed: &ObservedTrack,
    cutoff_ns: i64,
    frame_sequence: u64,
) -> Result<PreparedRouteIssue, String> {
    let cutoff_ms = ms(cutoff_ns)?;
    if !flight_view_in_cutoff(view, cutoff_ms) {
        return Err("flight view has invalid or future-stamped state at cutoff".into());
    }
    let key = flight_key(view);
    if store.revisions.len() > MAX_ROUTE_REVISIONS
        || store.cursors.len() > MAX_ROUTE_REVISIONS
        || store.cursors.len() != store.cursor_text_hashes.len()
    {
        return Err("route store budget exceeded or cursor columns inconsistent".into());
    }
    let evidence: Vec<_> = store.revisions.iter().map(|r| r.evidence).collect();
    let selected = t::select_revision_evidence(key, cutoff_ms, &evidence);
    if selected.available == t::Availability::Unavailable {
        return Err(format!(
            "route evidence: {:?}; counts={:?}",
            selected.refusal, selected.counts
        ));
    }
    if selected.selected.key != key {
        return Err("newest route revision does not match frozen flight plan".into());
    }
    let revision = store
        .revisions
        .iter()
        .find(|r| r.evidence == selected.selected)
        .ok_or("selected revision unavailable")?;
    if revision.report.failure != RouteFailure::None {
        return Err(format!("route conversion: {:?}", revision.report));
    }
    let pairs = store
        .cursors
        .len()
        .checked_mul(store.cursors.len().saturating_sub(1))
        .map(|n| n / 2)
        .ok_or("route cursor comparison count overflow")?;
    if pairs > MAX_CURSOR_COMPARISONS {
        return Err("route cursor comparison budget exceeded".into());
    }
    // Detect same-source conflicts across the entire eligible history before selecting its newest
    // row. Comparing only against the current winner can hide an older conflict behind a newer
    // cursor, which would make selection depend on storage order.
    for (i, (left, left_hash)) in store
        .cursors
        .iter()
        .zip(&store.cursor_text_hashes)
        .enumerate()
    {
        if left.available_ms > cutoff_ms || !same_flight(left.key, key) {
            continue;
        }
        if left.source_ms > left.available_ms
            || left.available_ms < 0
            || left.source_ms < 0
            || left.reserved != [0; 5]
        {
            return Err("invalid active cursor evidence".into());
        }
        for (right, right_hash) in store
            .cursors
            .iter()
            .zip(&store.cursor_text_hashes)
            .skip(i + 1)
        {
            if right.available_ms > cutoff_ms
                || !same_flight(right.key, key)
                || right.source_ms != left.source_ms
            {
                continue;
            }
            if right.source_ms > right.available_ms
                || right.available_ms < 0
                || right.source_ms < 0
                || right.reserved != [0; 5]
            {
                return Err("invalid active cursor evidence".into());
            }
            if right.key != left.key || right.next_fix != left.next_fix || right_hash != left_hash {
                return Err("same-source cursor conflict".into());
            }
        }
    }
    let mut selected_cursor: Option<(t::ActiveRouteCursor, [u8; 32])> = None;
    for (candidate, text_hash) in store.cursors.iter().zip(&store.cursor_text_hashes) {
        if candidate.available_ms > cutoff_ms || !same_flight(candidate.key, key) {
            continue;
        }
        if candidate.source_ms > candidate.available_ms
            || candidate.available_ms < 0
            || candidate.source_ms < 0
            || candidate.reserved != [0; 5]
        {
            return Err("invalid active cursor evidence".into());
        }
        if selected_cursor.is_some_and(|(prior, _)| {
            (candidate.source_ms, candidate.message_id) <= (prior.source_ms, prior.message_id)
        }) {
            continue;
        }
        selected_cursor = Some((*candidate, *text_hash));
    }
    let cursor = match selected_cursor {
        Some((mut chosen, text_hash)) => {
            if chosen.key != key {
                return Err("newest active cursor belongs to a different plan revision".into());
            }
            if text_hash != revision.text_sha256 {
                return Err("active cursor binds a different filed-route payload".into());
            }
            chosen.route_payload_sha256 = revision.evidence.payload_sha256;
            Some(chosen)
        }
        None => None,
    };
    let points = trajectory_points(&revision.converted)?;
    let east = f64::from(observed.east_mmps);
    let north = f64::from(observed.north_mmps);
    let speed = oa_ieee::round(oa_ieee::sqrt(east * east + north * north));
    if !speed.is_finite() || speed < 0.0 || speed >= f64::from(u32::MAX) {
        return Err("observed groundspeed unrepresentable".into());
    }
    let state_ms = ms(observed.state_ns)?;
    let newest_source_ms = ms(observed.newest_source_ns)?;
    if state_ms > cutoff_ms || newest_source_ms > cutoff_ms || newest_source_ms > state_ms {
        return Err("observed track is future-stamped or has source after state".into());
    }
    let track = t::CurrentTrack {
        state_ms,
        newest_source_ms,
        available_ms: cutoff_ms,
        message_id: frame_sequence,
        aircraft_address: observed.address,
        lat_e7: observed.lat_e7,
        lon_e7: observed.lon_e7,
        altitude_ft: observed.altitude_ft,
        vertical_rate_fpm: observed.vertical_rate_fpm,
        groundspeed_mmps: speed as u32,
    };
    let owner = t::OwnerProvenance {
        source_ms: view.owner_stamp.ts_ms,
        available_ms: view.owner_stamp.avail_ms,
        message_id: view.owner_stamp.msg,
        owner_unit: view.owner_unit,
        owner_state: if view.owner_unit == [0; 8] {
            t::OwnerState::Unknown
        } else {
            t::OwnerState::Known
        },
        reserved: [0; 7],
    };
    let r = revision.evidence;
    let route = t::RouteProvenance {
        key: r.key,
        source_ms: r.source_ms,
        available_ms: r.available_ms,
        message_id: r.message_id,
        payload_sha256: r.payload_sha256,
        reserved: [0; 4],
    };
    let target = points.last().ok_or("converted route is empty")?.fix;
    Ok(PreparedRouteIssue {
        key,
        route,
        cutoff_ms,
        max_track_age_ms: 5000,
        owner,
        route_cursor: cursor,
        track,
        clearance: clearance_evidence(view),
        eta_target: t::EtaTarget {
            present: true,
            fix: target,
        },
        points,
    })
}

/// Returns one issue-time horizontal result using the same prepared input as optional vertical mode.
pub fn route_issue(
    store: &RouteStore,
    view: &SharedFlightView,
    observed: &ObservedTrack,
    cutoff_ns: i64,
    frame_sequence: u64,
) -> Result<t::TrajectoryIssueOutput, String> {
    let prepared = prepare_route_issue(store, view, observed, cutoff_ns, frame_sequence)?;
    Ok(t::issue_trajectory(&prepared.input()))
}

/// Returns the horizontal baseline and bounded vertical profile from one shared prepared input.
pub fn route_issue_with_performance(
    store: &RouteStore,
    view: &SharedFlightView,
    observed: &ObservedTrack,
    cutoff_ns: i64,
    frame_sequence: u64,
    performance: t::PerformanceEnvelope,
) -> Result<(t::TrajectoryIssueOutput, t::VerticalProfileOutput), String> {
    let prepared = prepare_route_issue(store, view, observed, cutoff_ns, frame_sequence)?;
    let input = prepared.input();
    Ok((
        t::issue_trajectory(&input),
        t::issue_vertical_profile(&input, performance),
    ))
}

/// Converts a receipt nanosecond timestamp through the shared exact-millisecond gate.
pub(crate) fn receipt_cutoff_ms(cutoff_ns: i64) -> Result<i64, String> {
    ms(cutoff_ns)
}
