//! Bounded route conversion over the immutable NASR/CIFP model. **Core** (B1, B7): no I/O,
//! allocation, clock, or model indexes cross this module boundary. Unsupported geometry is a
//! visible refusal; this module never invents a straight segment.

use crate::core::model::{self, Model, PointIx};
use core::cmp::Ordering;

pub const MAX_ROUTE_STEPS: usize = 32;
pub const MAX_ROUTE_POINTS: usize = 256;
/// Maximum model rows/legs inspected by one conversion. The reported ZAU adaptation has
/// 11,640 points: 64 full endpoint scans (two per 32 airway steps) cost 744,960 visits, leaving
/// room for airway/procedure selection and expansion. This offline converter checks every
/// matching identity, including in a caller-built unsorted model; it never trusts sort order.
/// Every visit is charged before reading. Larger adaptations may explicitly exhaust this cap.
pub const MAX_ROUTE_WORK: u32 = 1_048_576;
pub const IDENT_BYTES: usize = 12;
pub const REGION_BYTES: usize = 4;
pub const SCOPE_BYTES: usize = 12;
const PATH_IF: u8 = 15;
const PATH_DF: u8 = 7;
const PATH_TF: u8 = 18;

/// Fixed label component. Bytes after `len` must be zero; labels are UTF-8, exact and never
/// truncated. A7.2: cold input value, size guard waived.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Label<const N: usize> {
    pub bytes: [u8; N],
    pub len: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct FixIdentity {
    pub ident: Label<IDENT_BYTES>,
    pub region: Label<REGION_BYTES>,
    pub scope: Label<SCOPE_BYTES>,
    pub class: u8,
}

/// A caller-normalized route token. Airway altitude is the planned altitude used for MEA
/// eligibility. Procedure route type and transition are exact CIFP identities.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C, u8)]
pub enum RouteStep {
    Fix(FixIdentity),
    /// Explicit filed direct connector to this fix from the preceding point.
    Direct(FixIdentity),
    Airway {
        designator: Label<IDENT_BYTES>,
        location: u8,
        entry: FixIdentity,
        exit: FixIdentity,
        planned_altitude_ft: i32,
    },
    Procedure {
        airport_ident: Label<IDENT_BYTES>,
        airport_region: Label<REGION_BYTES>,
        kind: u8,
        ident: Label<IDENT_BYTES>,
        route_type: u8,
        transition: Label<SCOPE_BYTES>,
    },
}
// A7: each offered route token is examined in a bounded loop; repr(C, u8) makes the 88-byte
// variant envelope explicit (largest payload is the airway token).
const _: () = assert!(core::mem::size_of::<RouteStep>() == 88);
const _: () = assert!(core::mem::size_of::<Label<IDENT_BYTES>>() == 13);
const _: () = assert!(core::mem::size_of::<Label<REGION_BYTES>>() == 5);
const _: () = assert!(core::mem::size_of::<Label<SCOPE_BYTES>>() == 13);
const _: () = assert!(core::mem::size_of::<FixIdentity>() == 32);

/// Caller-owned fixed input buffer. A7.2: cold request, size guard waived.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoutePlan {
    pub steps: [RouteStep; MAX_ROUTE_STEPS],
    pub len: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum RouteSource {
    DirectFix = 1,
    Airway = 2,
    Procedure = 3,
    DirectConnector = 4,
}

/// Successful output is struct-of-arrays, caller-owned, and fixed capacity. `connected[i]`
/// means the previous point is joined by an explicitly supported coded path.
#[derive(Clone, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct ConvertedRoute {
    pub lat_e7: [i32; MAX_ROUTE_POINTS],
    pub lon_e7: [i32; MAX_ROUTE_POINTS],
    pub ident: [Label<IDENT_BYTES>; MAX_ROUTE_POINTS],
    pub region: [Label<REGION_BYTES>; MAX_ROUTE_POINTS],
    pub scope: [Label<SCOPE_BYTES>; MAX_ROUTE_POINTS],
    pub class: [u8; MAX_ROUTE_POINTS],
    pub source: [u8; MAX_ROUTE_POINTS],
    pub path_term: [u8; MAX_ROUTE_POINTS],
    pub alt_desc: [u8; MAX_ROUTE_POINTS],
    pub alt1_ft: [i32; MAX_ROUTE_POINTS],
    pub alt2_ft: [i32; MAX_ROUTE_POINTS],
    pub speed_kt: [u16; MAX_ROUTE_POINTS],
    pub turn: [u8; MAX_ROUTE_POINTS],
    pub mea_ft: [i32; MAX_ROUTE_POINTS],
    pub gps_mea_ft: [i32; MAX_ROUTE_POINTS],
    pub mea_dir: [u8; MAX_ROUTE_POINTS],
    pub gps_mea_dir: [u8; MAX_ROUTE_POINTS],
    pub airway_designator: [Label<IDENT_BYTES>; MAX_ROUTE_POINTS],
    pub airway_location: [u8; MAX_ROUTE_POINTS],
    pub procedure_airport_ident: [Label<IDENT_BYTES>; MAX_ROUTE_POINTS],
    pub procedure_airport_region: [Label<REGION_BYTES>; MAX_ROUTE_POINTS],
    pub procedure_ident: [Label<IDENT_BYTES>; MAX_ROUTE_POINTS],
    pub procedure_transition: [Label<SCOPE_BYTES>; MAX_ROUTE_POINTS],
    pub procedure_kind: [u8; MAX_ROUTE_POINTS],
    pub procedure_route_type: [u8; MAX_ROUTE_POINTS],
    pub connected: [u8; MAX_ROUTE_POINTS],
    pub len: u16,
    /// A7: explicit zero alignment bytes; no implicit tail padding is budgeted.
    pub reserved: [u8; 2],
}
// A7.1: fixed SoA output is retained for the flight; the 32,004-byte budget covers 256 points
// with full identity, procedure/airway provenance, and all supported constraints.
const _: () = assert!(core::mem::size_of::<ConvertedRoute>() == 32_004);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum RouteFailure {
    None = 0,
    InvalidInput = 1,
    MalformedModel = 2,
    WorkLimit = 3,
    PointLimit = 4,
    NotFound = 5,
    Ambiguous = 6,
    AirwayGap = 7,
    BorderCrossing = 8,
    MeaNotSatisfied = 9,
    ProcedureConflict = 10,
    UnsupportedPath = 11,
    UnsupportedConstraint = 12,
    UnsupportedUnresolved = 13,
}

/// Counts distinguish offered, resolved, refused and unexamined steps. `work_limit` is always
/// the named cap, including failures before the first lookup.
/// Per-call outcome, including atomic replacement rule. A7.2: cold report value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouteReport {
    pub failure: RouteFailure,
    pub offered: u8,
    pub resolved: u8,
    pub unsupported: u8,
    pub refused: u8,
    pub unexamined: u8,
    pub work_used: u32,
    pub work_limit: u32,
}

const EMPTY_LABEL_ID: Label<IDENT_BYTES> = Label {
    bytes: [0; IDENT_BYTES],
    len: 0,
};
const EMPTY_LABEL_REGION: Label<REGION_BYTES> = Label {
    bytes: [0; REGION_BYTES],
    len: 0,
};
const EMPTY_LABEL_SCOPE: Label<SCOPE_BYTES> = Label {
    bytes: [0; SCOPE_BYTES],
    len: 0,
};

/// Initializes caller-owned output storage before use.
pub fn empty_converted_route() -> ConvertedRoute {
    ConvertedRoute {
        lat_e7: [model::POS_NONE; MAX_ROUTE_POINTS],
        lon_e7: [model::POS_NONE; MAX_ROUTE_POINTS],
        ident: [EMPTY_LABEL_ID; MAX_ROUTE_POINTS],
        region: [EMPTY_LABEL_REGION; MAX_ROUTE_POINTS],
        scope: [EMPTY_LABEL_SCOPE; MAX_ROUTE_POINTS],
        class: [0; MAX_ROUTE_POINTS],
        source: [0; MAX_ROUTE_POINTS],
        path_term: [model::path_term::NONE; MAX_ROUTE_POINTS],
        alt_desc: [model::alt_desc::NONE; MAX_ROUTE_POINTS],
        alt1_ft: [model::ALT_NONE; MAX_ROUTE_POINTS],
        alt2_ft: [model::ALT_NONE; MAX_ROUTE_POINTS],
        speed_kt: [model::NONE_U16; MAX_ROUTE_POINTS],
        turn: [model::turn::NONE; MAX_ROUTE_POINTS],
        mea_ft: [model::ALT_NONE; MAX_ROUTE_POINTS],
        gps_mea_ft: [model::ALT_NONE; MAX_ROUTE_POINTS],
        mea_dir: [model::compass::NONE; MAX_ROUTE_POINTS],
        gps_mea_dir: [model::compass::NONE; MAX_ROUTE_POINTS],
        airway_designator: [EMPTY_LABEL_ID; MAX_ROUTE_POINTS],
        airway_location: [0; MAX_ROUTE_POINTS],
        procedure_airport_ident: [EMPTY_LABEL_ID; MAX_ROUTE_POINTS],
        procedure_airport_region: [EMPTY_LABEL_REGION; MAX_ROUTE_POINTS],
        procedure_ident: [EMPTY_LABEL_ID; MAX_ROUTE_POINTS],
        procedure_transition: [EMPTY_LABEL_SCOPE; MAX_ROUTE_POINTS],
        procedure_kind: [0; MAX_ROUTE_POINTS],
        procedure_route_type: [0; MAX_ROUTE_POINTS],
        connected: [0; MAX_ROUTE_POINTS],
        len: 0,
        reserved: [0; 2],
    }
}

fn valid_label<const N: usize>(label: Label<N>, empty_ok: bool) -> bool {
    let n = usize::from(label.len);
    let Some(tail) = label.bytes.get(n..) else {
        return false;
    };
    if (!empty_ok && n == 0) || tail.iter().any(|b| *b != 0) {
        return false;
    }
    label
        .bytes
        .get(..n)
        .and_then(|b| core::str::from_utf8(b).ok())
        .is_some_and(|s| s.bytes().all(|byte| byte.is_ascii_graphic()))
}

fn text_or_empty(table: &model::StrTable, ix: model::StrIx) -> Result<&str, RouteFailure> {
    if ix == model::StrIx::NONE {
        return Ok("");
    }
    model::str_at(table, ix).ok_or(RouteFailure::MalformedModel)
}

fn label_eq<const N: usize>(
    table: &model::StrTable,
    ix: model::StrIx,
    label: Label<N>,
) -> Result<bool, RouteFailure> {
    let n = usize::from(label.len);
    let bytes = label.bytes.get(..n).ok_or(RouteFailure::InvalidInput)?;
    Ok(text_or_empty(table, ix)?.as_bytes() == bytes)
}

fn value<T: Copy>(column: &[T], row: usize) -> Result<T, RouteFailure> {
    column.get(row).copied().ok_or(RouteFailure::MalformedModel)
}

fn point_identity_order(m: &Model, row: usize, key: FixIdentity) -> Result<Ordering, RouteFailure> {
    let kind = value(&m.points.kind, row)?;
    let ident = value(&m.points.ident, row)?;
    let region = value(&m.points.region, row)?;
    let scope = value(&m.points.scope, row)?;
    let ident_cmp = text_or_empty(&m.strings, ident)?.as_bytes().cmp(
        key.ident
            .bytes
            .get(..usize::from(key.ident.len))
            .ok_or(RouteFailure::InvalidInput)?,
    );
    if ident_cmp != Ordering::Equal {
        return Ok(ident_cmp);
    }
    let region_cmp = text_or_empty(&m.strings, region)?.as_bytes().cmp(
        key.region
            .bytes
            .get(..usize::from(key.region.len))
            .ok_or(RouteFailure::InvalidInput)?,
    );
    if region_cmp != Ordering::Equal {
        return Ok(region_cmp);
    }
    let scope_cmp = text_or_empty(&m.strings, scope)?.as_bytes().cmp(
        key.scope
            .bytes
            .get(..usize::from(key.scope.len))
            .ok_or(RouteFailure::InvalidInput)?,
    );
    if scope_cmp != Ordering::Equal {
        return Ok(scope_cmp);
    }
    Ok(model::point_kind::class(kind).cmp(&key.class))
}

// D4: this private boundary copies one model point and its complete leg metadata together.
// Keeping the coupled source fields here prevents layout/index knowledge escaping the module.
#[allow(clippy::too_many_arguments)]
fn append_point(
    out: &mut ConvertedRoute,
    m: &Model,
    point: PointIx,
    source: RouteSource,
    term: u8,
    alt_desc: u8,
    alt1: i32,
    alt2: i32,
    speed: u16,
    turn: u8,
    mea: i32,
    gps_mea: i32,
    mea_dir: u8,
    gps_mea_dir: u8,
    connected: bool,
) -> Result<(), RouteFailure> {
    let i = usize::from(out.len);
    if i >= MAX_ROUTE_POINTS {
        return Err(RouteFailure::PointLimit);
    }
    let p = point.row();
    let Some((&lat, &lon, &kind, &ident, &region, &scope)) = m
        .points
        .lat
        .get(p)
        .zip(m.points.lon.get(p))
        .zip(m.points.kind.get(p))
        .zip(m.points.ident.get(p))
        .zip(m.points.region.get(p))
        .zip(m.points.scope.get(p))
        .map(|(((((a, b), c), d), e), f)| (a, b, c, d, e, f))
    else {
        return Err(RouteFailure::MalformedModel);
    };
    if lat == model::POS_NONE
        || lon == model::POS_NONE
        || !(-900_000_000..=900_000_000).contains(&lat)
        || !(-1_800_000_000..=1_800_000_000).contains(&lon)
        || kind == 0
        || kind > model::point_kind::MAX
    {
        return Err(RouteFailure::MalformedModel);
    }
    let ident_text = text_or_empty(&m.strings, ident)?;
    let region_text = text_or_empty(&m.strings, region)?;
    let scope_text = text_or_empty(&m.strings, scope)?;
    if ident_text.is_empty() {
        return Err(RouteFailure::MalformedModel);
    }
    *out.lat_e7.get_mut(i).ok_or(RouteFailure::PointLimit)? = lat;
    *out.lon_e7.get_mut(i).ok_or(RouteFailure::PointLimit)? = lon;
    *out.ident.get_mut(i).ok_or(RouteFailure::PointLimit)? = label_from::<IDENT_BYTES>(ident_text)?;
    *out.region.get_mut(i).ok_or(RouteFailure::PointLimit)? =
        label_from::<REGION_BYTES>(region_text)?;
    *out.scope.get_mut(i).ok_or(RouteFailure::PointLimit)? = label_from::<SCOPE_BYTES>(scope_text)?;
    *out.class.get_mut(i).ok_or(RouteFailure::PointLimit)? = model::point_kind::class(kind);
    *out.source.get_mut(i).ok_or(RouteFailure::PointLimit)? = source as u8;
    *out.path_term.get_mut(i).ok_or(RouteFailure::PointLimit)? = term;
    *out.alt_desc.get_mut(i).ok_or(RouteFailure::PointLimit)? = alt_desc;
    *out.alt1_ft.get_mut(i).ok_or(RouteFailure::PointLimit)? = alt1;
    *out.alt2_ft.get_mut(i).ok_or(RouteFailure::PointLimit)? = alt2;
    *out.speed_kt.get_mut(i).ok_or(RouteFailure::PointLimit)? = speed;
    *out.turn.get_mut(i).ok_or(RouteFailure::PointLimit)? = turn;
    *out.mea_ft.get_mut(i).ok_or(RouteFailure::PointLimit)? = mea;
    *out.gps_mea_ft.get_mut(i).ok_or(RouteFailure::PointLimit)? = gps_mea;
    *out.mea_dir.get_mut(i).ok_or(RouteFailure::PointLimit)? = mea_dir;
    *out.gps_mea_dir.get_mut(i).ok_or(RouteFailure::PointLimit)? = gps_mea_dir;
    *out.connected.get_mut(i).ok_or(RouteFailure::PointLimit)? = u8::from(connected);
    out.len = out.len.checked_add(1).ok_or(RouteFailure::PointLimit)?;
    Ok(())
}

fn label_from<const N: usize>(s: &str) -> Result<Label<N>, RouteFailure> {
    if s.len() > N
        || s.len() > usize::from(u8::MAX)
        || !s.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err(RouteFailure::MalformedModel);
    }
    let mut out = Label {
        bytes: [0; N],
        len: s.len() as u8,
    };
    out.bytes
        .get_mut(..s.len())
        .ok_or(RouteFailure::MalformedModel)?
        .copy_from_slice(s.as_bytes());
    Ok(out)
}

fn annotate_airway(
    out: &mut ConvertedRoute,
    designator: Label<IDENT_BYTES>,
    location: u8,
) -> Result<(), RouteFailure> {
    let i = usize::from(out.len)
        .checked_sub(1)
        .ok_or(RouteFailure::MalformedModel)?;
    *out.airway_designator
        .get_mut(i)
        .ok_or(RouteFailure::PointLimit)? = designator;
    *out.airway_location
        .get_mut(i)
        .ok_or(RouteFailure::PointLimit)? = location;
    Ok(())
}

fn annotate_procedure(
    out: &mut ConvertedRoute,
    airport_ident: Label<IDENT_BYTES>,
    airport_region: Label<REGION_BYTES>,
    kind: u8,
    ident: Label<IDENT_BYTES>,
    route_type: u8,
    transition: Label<SCOPE_BYTES>,
) -> Result<(), RouteFailure> {
    let i = usize::from(out.len)
        .checked_sub(1)
        .ok_or(RouteFailure::MalformedModel)?;
    *out.procedure_airport_ident
        .get_mut(i)
        .ok_or(RouteFailure::PointLimit)? = airport_ident;
    *out.procedure_airport_region
        .get_mut(i)
        .ok_or(RouteFailure::PointLimit)? = airport_region;
    *out.procedure_ident
        .get_mut(i)
        .ok_or(RouteFailure::PointLimit)? = ident;
    *out.procedure_transition
        .get_mut(i)
        .ok_or(RouteFailure::PointLimit)? = transition;
    *out.procedure_kind
        .get_mut(i)
        .ok_or(RouteFailure::PointLimit)? = kind;
    *out.procedure_route_type
        .get_mut(i)
        .ok_or(RouteFailure::PointLimit)? = route_type;
    Ok(())
}

/// Converts a normalized bounded route atomically. On any failure `route.len` is zero; callers
/// retain their prior good route separately and must not forecast from a partial result.
pub fn convert_route(m: &Model, plan: &RoutePlan, out: &mut ConvertedRoute) -> RouteReport {
    *out = empty_converted_route();
    let mut report = RouteReport {
        failure: RouteFailure::None,
        offered: plan.len,
        resolved: 0,
        unsupported: 0,
        refused: 0,
        unexamined: 0,
        work_used: 0,
        work_limit: MAX_ROUTE_WORK,
    };
    if usize::from(plan.len) > MAX_ROUTE_STEPS || plan.len == 0 {
        report.failure = RouteFailure::InvalidInput;
        report.unexamined = plan.len;
        // No step was examined or refused individually; the request itself is invalid.
        return report;
    }
    let mut work = 0u32;
    for n in 0..usize::from(plan.len) {
        if let Err(reason) = charge(&mut work) {
            report.failure = reason;
            report.refused = 1;
            report.unexamined = plan.len.saturating_sub(n as u8 + 1);
            report.work_used = work;
            *out = empty_converted_route();
            break;
        }
        let step = match plan.steps.get(n).copied() {
            Some(step) => step,
            None => {
                report.failure = RouteFailure::InvalidInput;
                report.refused = 1;
                report.unexamined = plan.len.saturating_sub(n as u8 + 1);
                *out = empty_converted_route();
                break;
            }
        };
        let result = match step {
            RouteStep::Fix(key) => {
                convert_fix(m, key, out, &mut work, RouteSource::DirectFix, false)
            }
            RouteStep::Direct(key) => {
                convert_fix(m, key, out, &mut work, RouteSource::DirectConnector, true)
            }
            RouteStep::Airway {
                designator,
                location,
                entry,
                exit,
                planned_altitude_ft,
            } => convert_airway(
                m,
                designator,
                location,
                entry,
                exit,
                planned_altitude_ft,
                out,
                &mut work,
            ),
            RouteStep::Procedure {
                airport_ident,
                airport_region,
                kind,
                ident,
                route_type,
                transition,
            } => convert_procedure(
                m,
                airport_ident,
                airport_region,
                kind,
                ident,
                route_type,
                transition,
                out,
                &mut work,
            ),
        };
        report.work_used = work;
        if let Err(reason) = result {
            report.failure = reason;
            report.refused = 1;
            if matches!(
                reason,
                RouteFailure::UnsupportedPath
                    | RouteFailure::UnsupportedConstraint
                    | RouteFailure::UnsupportedUnresolved
            ) {
                report.unsupported = 1;
            }
            report.unexamined = plan.len.saturating_sub(n as u8 + 1);
            *out = empty_converted_route();
            break;
        }
        report.resolved += 1;
    }
    report
}

fn charge(work: &mut u32) -> Result<(), RouteFailure> {
    if *work >= MAX_ROUTE_WORK {
        return Err(RouteFailure::WorkLimit);
    }
    *work += 1;
    Ok(())
}

fn convert_fix(
    m: &Model,
    key: FixIdentity,
    out: &mut ConvertedRoute,
    work: &mut u32,
    source: RouteSource,
    direct: bool,
) -> Result<(), RouteFailure> {
    if !valid_identity(key) {
        return Err(RouteFailure::InvalidInput);
    }
    let n = m.points.kind.len();
    if [
        m.points.lat.len(),
        m.points.lon.len(),
        m.points.ident.len(),
        m.points.region.len(),
        m.points.scope.len(),
    ]
    .iter()
    .any(|v| *v != n)
    {
        return Err(RouteFailure::MalformedModel);
    }
    let found = find_exact_point(m, key, work)?;
    append_point(
        out,
        m,
        found,
        source,
        if direct {
            PATH_DF
        } else {
            model::path_term::NONE
        },
        model::alt_desc::NONE,
        model::ALT_NONE,
        model::ALT_NONE,
        model::NONE_U16,
        model::turn::NONE,
        model::ALT_NONE,
        model::ALT_NONE,
        model::compass::NONE,
        model::compass::NONE,
        direct && out.len > 0,
    )?;
    Ok(())
}

fn valid_identity(key: FixIdentity) -> bool {
    valid_label(key.ident, false)
        && valid_label(key.region, true)
        && valid_label(key.scope, true)
        && matches!(
            key.class,
            model::point_kind::FIX
                | model::point_kind::VOR
                | model::point_kind::NDB
                | model::point_kind::AIRPORT
                | model::point_kind::THRESHOLD
                | model::point_kind::LOCALIZER
        )
}

fn span_bounds<I>(
    span: model::Span<I>,
    start: u32,
    total: usize,
) -> Result<(usize, usize), RouteFailure> {
    let begin = usize::try_from(start).map_err(|_| RouteFailure::MalformedModel)?;
    let len = usize::try_from(span.len).map_err(|_| RouteFailure::MalformedModel)?;
    let end = begin.checked_add(len).ok_or(RouteFailure::MalformedModel)?;
    if end > total {
        return Err(RouteFailure::MalformedModel);
    }
    Ok((begin, end))
}

fn bit_value(bits: &model::BitSet, row: usize) -> bool {
    bits.words
        .get(row / 64)
        .is_some_and(|w| (w >> (row % 64)) & 1 == 1)
}

fn valid_bits(bits: &model::BitSet, rows: usize) -> bool {
    usize::try_from(bits.len).ok() == Some(rows) && bits.words.len() == rows.div_ceil(64)
}

fn find_exact_point(m: &Model, key: FixIdentity, work: &mut u32) -> Result<PointIx, RouteFailure> {
    if !valid_identity(key) {
        return Err(RouteFailure::InvalidInput);
    }
    let n = m.points.kind.len();
    if m.points.lat.len() != n
        || m.points.lon.len() != n
        || m.points.ident.len() != n
        || m.points.region.len() != n
        || m.points.scope.len() != n
    {
        return Err(RouteFailure::MalformedModel);
    }
    let mut found = None;
    for row in 0..n {
        charge(work)?;
        if point_identity_order(m, row, key)? == Ordering::Equal {
            if found.is_some() {
                return Err(RouteFailure::Ambiguous);
            }
            found = Some(PointIx::new(
                u32::try_from(row).map_err(|_| RouteFailure::MalformedModel)?,
            ));
        }
    }
    found.ok_or(RouteFailure::NotFound)
}

// D4: the private helper consumes the exact fields of RouteStep::Airway as one operation.
#[allow(clippy::too_many_arguments)]
fn convert_airway(
    m: &Model,
    designator: Label<IDENT_BYTES>,
    location: u8,
    entry: FixIdentity,
    exit: FixIdentity,
    altitude: i32,
    out: &mut ConvertedRoute,
    work: &mut u32,
) -> Result<(), RouteFailure> {
    if !valid_label(designator, false)
        || !valid_identity(entry)
        || !valid_identity(exit)
        || altitude < 0
        || !location.is_ascii_graphic()
    {
        return Err(RouteFailure::InvalidInput);
    }
    let a = &m.airways;
    let n = a.designator.len();
    if a.location.len() != n || a.legs.len() != n {
        return Err(RouteFailure::MalformedModel);
    }
    let mut airway_row = None;
    let mut airways_found = 0u8;
    for i in 0..n {
        charge(work)?;
        if value(&a.location, i)? == location
            && label_eq(&m.strings, value(&a.designator, i)?, designator)?
        {
            airways_found = airways_found.saturating_add(1);
            airway_row = Some(i);
            if airways_found > 1 {
                return Err(RouteFailure::Ambiguous);
            }
        }
    }
    if airways_found == 0 {
        return Err(RouteFailure::NotFound);
    }
    let legs = &m.airway_legs;
    let nl = legs.from.len();
    if legs.to.len() != nl
        || legs.mea_ft.len() != nl
        || legs.mea_opp_ft.len() != nl
        || legs.gps_mea_ft.len() != nl
        || legs.mea_dir.len() != nl
        || legs.gps_mea_dir.len() != nl
        || !valid_bits(&legs.gap, nl)
        || !valid_bits(&legs.signal_gap, nl)
        || !valid_bits(&legs.dogleg, nl)
    {
        return Err(RouteFailure::MalformedModel);
    }
    let airway_span = value(&a.legs, airway_row.ok_or(RouteFailure::NotFound)?)?;
    let (start, end) = span_bounds(airway_span, airway_span.start.get(), nl)?;
    let entry_ix = find_exact_point(m, entry, work)?;
    let exit_ix = find_exact_point(m, exit, work)?;
    let mut selected_start = None;
    let mut reverse = false;
    let mut candidates = 0u8;
    let mut saw_border = false;
    for row in start..end {
        charge(work)?;
        let from = value(&legs.from, row)?;
        let to = value(&legs.to, row)?;
        if (from != PointIx::NONE && from.row() >= m.points.kind.len())
            || (to != PointIx::NONE && to.row() >= m.points.kind.len())
        {
            return Err(RouteFailure::MalformedModel);
        }
        saw_border |= from == PointIx::NONE || to == PointIx::NONE;
        if from == entry_ix && to == exit_ix {
            candidates = candidates.saturating_add(1);
            selected_start = Some(row);
            reverse = false;
        }
        if to == entry_ix && from == exit_ix {
            candidates = candidates.saturating_add(1);
            selected_start = Some(row);
            reverse = true;
        }
    }
    // If entry and exit are separated by intermediate fixes, select a contiguous sequence.
    if candidates == 0 {
        for row in start..end {
            charge(work)?;
            if value(&legs.from, row)? == entry_ix {
                let mut cur = value(&legs.to, row)?;
                let mut cursor = row;
                let mut walked = 0usize;
                while cursor + 1 < end && cur != exit_ix && walked < end - start {
                    charge(work)?;
                    let next = cursor + 1;
                    if value(&legs.from, next)? != cur {
                        break;
                    }
                    cur = value(&legs.to, next)?;
                    cursor = next;
                    walked += 1;
                }
                if cur == exit_ix {
                    candidates = candidates.saturating_add(1);
                    selected_start = Some(row);
                    reverse = false;
                }
            }
            if value(&legs.to, row)? == entry_ix {
                let mut cur = value(&legs.from, row)?;
                let mut cursor = row;
                let mut walked = 0usize;
                while cursor > start && cur != exit_ix && walked < end - start {
                    charge(work)?;
                    let next = cursor - 1;
                    if value(&legs.to, next)? != cur {
                        break;
                    }
                    cur = value(&legs.from, next)?;
                    cursor = next;
                    walked += 1;
                }
                if cur == exit_ix {
                    candidates = candidates.saturating_add(1);
                    selected_start = Some(row);
                    reverse = true;
                }
            }
        }
    }
    if candidates == 0 {
        return Err(if saw_border {
            RouteFailure::BorderCrossing
        } else {
            RouteFailure::NotFound
        });
    }
    if candidates > 1 {
        return Err(RouteFailure::Ambiguous);
    }
    // E5/E6: the adaptation currently drops the opposite GPS MEA and opposite MEA direction.
    // Absence of the forward GPS value does not prove absence of a reverse-only restriction.
    // Reverse conversion cannot be qualified from this model and is explicitly refused.
    if reverse {
        return Err(RouteFailure::UnsupportedConstraint);
    }
    let selected_start = selected_start.ok_or(RouteFailure::NotFound)?;
    let mut rows = [0; MAX_ROUTE_POINTS];
    let mut count = 0usize;
    if reverse {
        let mut row = selected_start;
        let mut cur = entry_ix;
        loop {
            if row < start || row >= end || count >= MAX_ROUTE_POINTS {
                return Err(RouteFailure::PointLimit);
            }
            if value(&legs.to, row)? != cur {
                return Err(RouteFailure::AirwayGap);
            }
            *rows.get_mut(count).ok_or(RouteFailure::PointLimit)? = row;
            count += 1;
            cur = value(&legs.from, row)?;
            if cur == exit_ix {
                break;
            }
            if row == start {
                return Err(RouteFailure::AirwayGap);
            }
            row -= 1;
            charge(work)?;
            if value(&legs.to, row)? != cur {
                return Err(RouteFailure::AirwayGap);
            }
        }
    } else {
        let mut row = selected_start;
        let mut cur = entry_ix;
        loop {
            if row < start || row >= end || count >= MAX_ROUTE_POINTS {
                return Err(RouteFailure::PointLimit);
            }
            if value(&legs.from, row)? != cur {
                return Err(RouteFailure::AirwayGap);
            }
            *rows.get_mut(count).ok_or(RouteFailure::PointLimit)? = row;
            count += 1;
            cur = value(&legs.to, row)?;
            if cur == exit_ix {
                break;
            }
            row += 1;
            charge(work)?;
            if row >= end || value(&legs.from, row)? != cur {
                return Err(RouteFailure::AirwayGap);
            }
        }
    }
    let entry_already_last = usize::from(out.len).checked_sub(1).is_some_and(|i| {
        out.ident.get(i) == Some(&entry.ident)
            && out.region.get(i) == Some(&entry.region)
            && out.scope.get(i) == Some(&entry.scope)
            && out.class.get(i) == Some(&entry.class)
    });
    if !entry_already_last {
        append_point(
            out,
            m,
            entry_ix,
            RouteSource::Airway,
            model::path_term::NONE,
            model::alt_desc::NONE,
            model::ALT_NONE,
            model::ALT_NONE,
            model::NONE_U16,
            model::turn::NONE,
            model::ALT_NONE,
            model::ALT_NONE,
            model::compass::NONE,
            model::compass::NONE,
            false,
        )?;
    }
    for n in 0..count {
        charge(work)?;
        let row = value(&rows, n)?;
        if bit_value(&legs.gap, row)
            || bit_value(&legs.signal_gap, row)
            || bit_value(&legs.dogleg, row)
        {
            return Err(RouteFailure::AirwayGap);
        }
        let mea = if reverse {
            value(&legs.mea_opp_ft, row)?
        } else {
            value(&legs.mea_ft, row)?
        };
        // This model has no opposite-direction GPS MEA column. If a forward GPS MEA is present,
        // reverse eligibility is not established and must be refused rather than weakened.
        let source_gps = value(&legs.gps_mea_ft, row)?;
        if reverse && source_gps != model::ALT_NONE {
            return Err(RouteFailure::UnsupportedConstraint);
        }
        let gps = if reverse { model::ALT_NONE } else { source_gps };
        let mea_dir = if reverse {
            model::compass::NONE
        } else {
            value(&legs.mea_dir, row)?
        };
        let gps_dir = if reverse {
            model::compass::NONE
        } else {
            value(&legs.gps_mea_dir, row)?
        };
        if (mea != model::ALT_NONE && altitude < mea) || (gps != model::ALT_NONE && altitude < gps)
        {
            return Err(RouteFailure::MeaNotSatisfied);
        }
        let point = if reverse {
            value(&legs.from, row)?
        } else {
            value(&legs.to, row)?
        };
        if point == PointIx::NONE {
            return Err(RouteFailure::BorderCrossing);
        }
        let connected = out.len > 0;
        append_point(
            out,
            m,
            point,
            RouteSource::Airway,
            model::path_term::NONE,
            model::alt_desc::NONE,
            model::ALT_NONE,
            model::ALT_NONE,
            model::NONE_U16,
            model::turn::NONE,
            mea,
            gps,
            mea_dir,
            gps_dir,
            connected,
        )?;
        annotate_airway(out, designator, location)?;
    }
    Ok(())
}

// D4: the private helper consumes the full airport/procedure/transition identity together.
#[allow(clippy::too_many_arguments)]
fn convert_procedure(
    m: &Model,
    airport_ident: Label<IDENT_BYTES>,
    airport_region: Label<REGION_BYTES>,
    kind: u8,
    ident: Label<IDENT_BYTES>,
    route_type: u8,
    transition: Label<SCOPE_BYTES>,
    out: &mut ConvertedRoute,
    work: &mut u32,
) -> Result<(), RouteFailure> {
    if !valid_label(airport_ident, false)
        || !valid_label(airport_region, false)
        || !valid_label(ident, false)
        || !valid_label(transition, true)
        || kind == 0
        || route_type == 0
    {
        return Err(RouteFailure::InvalidInput);
    }
    let p = &m.procs;
    let np = p.kind.len();
    if p.airport.len() != np
        || p.ident.len() != np
        || p.routes.len() != np
        || !valid_bits(&p.coded, np)
    {
        return Err(RouteFailure::MalformedModel);
    }
    let mut proc_row = None;
    let mut found = 0u8;
    if m.points.ident.len() != m.points.kind.len() || m.points.region.len() != m.points.kind.len() {
        return Err(RouteFailure::MalformedModel);
    }
    for i in 0..np {
        charge(work)?;
        let apt = value(&p.airport, i)?.row();
        if apt >= m.points.kind.len() {
            return Err(RouteFailure::MalformedModel);
        }
        if value(&m.points.kind, apt)? != model::point_kind::AIRPORT {
            return Err(RouteFailure::MalformedModel);
        }
        if value(&p.kind, i)? == kind
            && label_eq(&m.strings, value(&p.ident, i)?, ident)?
            && label_eq(&m.strings, value(&m.points.ident, apt)?, airport_ident)?
            && label_eq(&m.strings, value(&m.points.region, apt)?, airport_region)?
        {
            found = found.saturating_add(1);
            proc_row = Some(i);
        }
    }
    if found == 0 {
        return Err(RouteFailure::NotFound);
    }
    if found > 1 {
        return Err(RouteFailure::ProcedureConflict);
    }
    let proc_row = proc_row.ok_or(RouteFailure::NotFound)?;
    if !bit_value(&p.coded, proc_row) {
        return Err(RouteFailure::UnsupportedUnresolved);
    }
    let r = &m.routes;
    let nr = r.route_type.len();
    if r.transition.len() != nr || r.legs.len() != nr {
        return Err(RouteFailure::MalformedModel);
    }
    let proc_span = value(&p.routes, proc_row)?;
    let (rs, re) = span_bounds(proc_span, proc_span.start.get(), nr)?;
    let mut route_row = None;
    let mut routes_found = 0u8;
    for i in rs..re {
        charge(work)?;
        if value(&r.route_type, i)? == route_type
            && label_eq(&m.strings, value(&r.transition, i)?, transition)?
        {
            routes_found = routes_found.saturating_add(1);
            route_row = Some(i);
        }
    }
    if routes_found == 0 {
        return Err(RouteFailure::NotFound);
    }
    if routes_found > 1 {
        return Err(RouteFailure::ProcedureConflict);
    }
    let lg = &m.legs;
    let nl = lg.fix.len();
    if lg.center.len() != nl
        || lg.path_term.len() != nl
        || lg.turn.len() != nl
        || lg.alt_desc.len() != nl
        || lg.alt1_ft.len() != nl
        || lg.alt2_ft.len() != nl
        || lg.speed_kt.len() != nl
        || lg.dist_or_time.len() != nl
        || lg.course_mag_cdeg.len() != nl
        || !valid_bits(&lg.dist_is_time, nl)
    {
        return Err(RouteFailure::MalformedModel);
    }
    let route_span = value(&r.legs, route_row.ok_or(RouteFailure::NotFound)?)?;
    let (ls, le) = span_bounds(route_span, route_span.start.get(), nl)?;
    for row in ls..le {
        charge(work)?;
        let term = value(&lg.path_term, row)?;
        if !matches!(term, PATH_IF | PATH_TF | PATH_DF) {
            return Err(RouteFailure::UnsupportedPath);
        }
        let ad = value(&lg.alt_desc, row)?;
        if !matches!(
            ad,
            model::alt_desc::NONE | model::alt_desc::AT_OR_ABOVE | model::alt_desc::BETWEEN
        ) {
            return Err(RouteFailure::UnsupportedConstraint);
        }
        let alt1 = value(&lg.alt1_ft, row)?;
        let alt2 = value(&lg.alt2_ft, row)?;
        if (ad == model::alt_desc::AT_OR_ABOVE && alt1 == model::ALT_NONE)
            || (ad == model::alt_desc::BETWEEN
                && (alt1 == model::ALT_NONE || alt2 == model::ALT_NONE || alt1 < alt2))
        {
            return Err(RouteFailure::MalformedModel);
        }
        // A populated altitude without a decoded description has no established meaning.
        if (ad == model::alt_desc::NONE && (alt1 != model::ALT_NONE || alt2 != model::ALT_NONE))
            || (ad == model::alt_desc::AT_OR_ABOVE && alt2 != model::ALT_NONE)
        {
            return Err(RouteFailure::UnsupportedConstraint);
        }
        if model::bit_get(&lg.dist_is_time, row)
            || value(&lg.dist_or_time, row)? != model::NONE_U32
            || value(&lg.course_mag_cdeg, row)? != model::NONE_U16
        {
            return Err(RouteFailure::UnsupportedConstraint);
        }
        let turn = value(&lg.turn, row)?;
        if !matches!(
            turn,
            model::turn::NONE | model::turn::LEFT | model::turn::RIGHT | model::turn::EITHER
        ) {
            return Err(RouteFailure::UnsupportedConstraint);
        }
        if value(&lg.center, row)? != PointIx::NONE {
            return Err(RouteFailure::UnsupportedConstraint);
        }
        let point = value(&lg.fix, row)?;
        if point == PointIx::NONE {
            return Err(RouteFailure::UnsupportedUnresolved);
        }
        let connected = out.len > 0 && matches!(term, PATH_TF | PATH_DF);
        append_point(
            out,
            m,
            point,
            RouteSource::Procedure,
            term,
            ad,
            alt1,
            alt2,
            value(&lg.speed_kt, row)?,
            turn,
            model::ALT_NONE,
            model::ALT_NONE,
            model::compass::NONE,
            model::compass::NONE,
            connected,
        )?;
        annotate_procedure(
            out,
            airport_ident,
            airport_region,
            kind,
            ident,
            route_type,
            transition,
        )?;
    }
    Ok(())
}
