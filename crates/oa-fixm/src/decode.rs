//! The decoder: a parsed SFDPS document in, flight-event rows out. Core (pure).

use crate::known::KNOWN;
use crate::scalar::{
    parse_beacon, parse_code, parse_hex24, parse_iso_ms, parse_scaled, parse_uint, parse_uuid,
    rescale, trim,
};
use crate::{
    field_bit, truncate_events, Events, Field, FixmError, Limits, Reject, Stats, Summary,
    UnknownPath, ALT_FORM_AT_FIX, ALT_FORM_BLOCK, ALT_FORM_SIMPLE, ALT_FORM_VFR, ALT_FORM_VFR_PLUS,
    FIELD_COUNT, FLAG_ASSIGNED_VFR, FLAG_ASSIGNED_VFR_PLUS, FLAG_INTERIM_NIL, FTYPE_GENERAL,
    FTYPE_NON_SCHEDULED, FTYPE_SCHEDULED, HANDOFF_ACCEPTANCE, HANDOFF_FAILURE, HANDOFF_INITIATION,
    HANDOFF_RETRACTION, HANDOFF_TAKE_CONTROL, HANDOFF_UPDATE, MAX_PATH_TEXT, REJECT_COUNT,
    STATUS_ACTIVE, STATUS_CANCELLED, STATUS_COMPLETED, STATUS_DROPPED, STATUS_PROPOSED,
};
use oa_xml::{
    attr_needs_decode, decode_into, ns_uri_bytes, resolve_attr_ns, resolve_elem_ns, resolve_prefix,
    span_bytes, text_needs_decode, AttrIx, Doc, ElemIx, Ns, Span, SpanKind, TextIx,
};

/// The bytes of a name or namespace span. Spans of a parsed `Doc` always lie inside its input; a
/// span that did not would read as empty, which matches no name, so the element or attribute
/// is unknown and nothing is decoded from it. Values use `span_bytes` and report the error.
fn name_bytes(input: &[u8], sp: Span) -> &[u8] {
    span_bytes(input, sp).unwrap_or(&[])
}

/// The expanded bytes of a namespace URI span from `resolve_*_ns`; empty (matching nothing) if
/// the span is outside the document.
fn uri_bytes<'a>(doc: &Doc<'a>, sp: Span) -> &'a [u8] {
    ns_uri_bytes(doc, sp).unwrap_or(&[])
}

const NS_NAS: &[u8] = b"http://www.faa.aero/nas/3.0";
const NS_XSI: &[u8] = b"http://www.w3.org/2001/XMLSchema-instance";

const ALT_MIN_FT: i64 = -2_000;
const ALT_MAX_FT: i64 = 100_000;
/// Speed bound, 0.1 knot (2,000 knots).
const SPEED_MAX_DKT: i64 = 20_000;

// Path hashes. A path is the chain of local names from the `message` element, folded with
// 64-bit FNV-1a; the same function runs at compile time (the decoded paths below) and at run
// time (every element). A collision between two real paths is a 2^-64 event per pair.
const SEED: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x0000_0100_0000_01b3;

const fn extend(h: u64, name: &[u8]) -> u64 {
    let mut h = (h ^ 0x2f).wrapping_mul(PRIME);
    let mut rest = name;
    while let [b, tail @ ..] = rest {
        h = (h ^ *b as u64).wrapping_mul(PRIME);
        rest = tail;
    }
    h
}

const fn path(parts: &[&str]) -> u64 {
    let mut h = extend(SEED, b"message");
    let mut rest = parts;
    while let [p, tail @ ..] = rest {
        h = extend(h, p.as_bytes());
        rest = tail;
    }
    h
}

const fn attr_hash(path_h: u64, name: &[u8]) -> u64 {
    extend(extend(path_h, b"@"), name)
}

const P_MESSAGE: u64 = path(&[]);
const P_FLIGHT: u64 = path(&["flight"]);
const P_GUFI: u64 = path(&["flight", "gufi"]);
const P_FLIGHT_ID: u64 = path(&["flight", "flightIdentification"]);
const P_FLIGHT_PLAN: u64 = path(&["flight", "flightPlan"]);
const P_STATUS: u64 = path(&["flight", "flightStatus"]);
const P_ARRIVAL: u64 = path(&["flight", "arrival"]);
const P_DEPARTURE: u64 = path(&["flight", "departure"]);
const P_DEP_ACTUAL: u64 = path(&[
    "flight",
    "departure",
    "runwayPositionAndTime",
    "runwayTime",
    "actual",
]);
const P_ARR_EST: u64 = path(&[
    "flight",
    "arrival",
    "runwayPositionAndTime",
    "runwayTime",
    "estimated",
]);
const P_ASSIGNED_SIMPLE: u64 = path(&["flight", "assignedAltitude", "simple"]);
const P_ASSIGNED_VFRPLUS: u64 = path(&["flight", "assignedAltitude", "vfrPlus"]);
const P_ASSIGNED_VFR: u64 = path(&["flight", "assignedAltitude", "vfr"]);
const P_ASSIGNED_BLOCK: u64 = path(&["flight", "assignedAltitude", "block"]);
const P_BLOCK_ABOVE: u64 = path(&["flight", "assignedAltitude", "block", "above"]);
const P_BLOCK_BELOW: u64 = path(&["flight", "assignedAltitude", "block", "below"]);
const P_ALTFIX: u64 = path(&["flight", "assignedAltitude", "altFixAlt"]);
const P_ALTFIX_POINT: u64 = path(&["flight", "assignedAltitude", "altFixAlt", "point"]);
const P_ALTFIX_PRE: u64 = path(&["flight", "assignedAltitude", "altFixAlt", "pre"]);
const P_ALTFIX_POST: u64 = path(&["flight", "assignedAltitude", "altFixAlt", "post"]);
const P_INTERIM: u64 = path(&["flight", "interimAltitude"]);
const P_REQ_SIMPLE: u64 = path(&["flight", "requestedAltitude", "simple"]);
const P_REQ_VFRPLUS: u64 = path(&["flight", "requestedAltitude", "vfrPlus"]);
const P_REQ_VFR: u64 = path(&["flight", "requestedAltitude", "vfr"]);
const P_AIRCRAFT_DESC: u64 = path(&["flight", "aircraftDescription"]);
const P_HANDOFF: u64 = path(&["flight", "enRoute", "boundaryCrossings", "handoff"]);
const P_ACPT: u64 = path(&[
    "flight",
    "enRoute",
    "boundaryCrossings",
    "handoff",
    "acceptingUnit",
]);
const P_POSITION: u64 = path(&["flight", "enRoute", "position"]);
const P_REPORTED_ALT: u64 = path(&["flight", "enRoute", "position", "altitude"]);
const P_GS: u64 = path(&[
    "flight",
    "enRoute",
    "position",
    "actualSpeed",
    "surveillance",
]);
const P_VX: u64 = path(&["flight", "enRoute", "position", "trackVelocity", "x"]);
const P_VY: u64 = path(&["flight", "enRoute", "position", "trackVelocity", "y"]);
const P_POS: u64 = path(&[
    "flight", "enRoute", "position", "position", "location", "pos",
]);
const P_AIRCRAFT_TYPE: u64 = path(&[
    "flight",
    "aircraftDescription",
    "aircraftType",
    "icaoModelIdentifier",
]);
const P_ROUTE: u64 = path(&["flight", "agreed", "route"]);
const P_OPERATOR: u64 = path(&[
    "flight",
    "operator",
    "operatingOrganization",
    "organization",
]);
const P_BEACON: u64 = path(&[
    "flight",
    "enRoute",
    "beaconCodeAssignment",
    "currentBeaconCode",
]);
const P_CTRL: u64 = path(&["flight", "controllingUnit"]);
const P_RECV: u64 = path(&[
    "flight",
    "enRoute",
    "boundaryCrossings",
    "handoff",
    "receivingUnit",
]);
const P_XFER: u64 = path(&[
    "flight",
    "enRoute",
    "boundaryCrossings",
    "handoff",
    "transferringUnit",
]);
const P_NAMEVALUE: u64 = path(&[
    "flight",
    "supplementalData",
    "additionalFlightInformation",
    "nameValue",
]);

/// `nameValue@name` values seen in real data (all known; MSG_SEQ_NO and FLIGHT_PLAN_SEQ_NO are
/// decoded, the rest are known and ignored). Any other name is an unknown path
/// `.../nameValue[NAME]`.
const NAMEVALUE_KNOWN: [&[u8]; 12] = [
    b"SOURCE_TIME_AND_SEQ",
    b"SOURCE_TIME",
    b"MSG_SEQ_NO",
    b"FLIGHT_PLAN_SEQ_NO",
    b"FDPS_GUFI",
    b"ADSB_02M_52B",
    b"ADSB_VEL_176A",
    b"ADSB_TIME_177A",
    b"ADSB_POS_174A",
    b"ADSB_ALT_175A",
    b"FLIGHT_PLAN_REV_NO",
    b"4TH_ADAPTED_FIELD",
];

/// Element states while walking.
const S_KNOWN: u8 = 0;
const S_UNKNOWN: u8 = 1;
const S_BELOW: u8 = 2;

const NO_TEXT: u32 = u32::MAX;
const MANY_TEXT: u32 = u32::MAX - 1;

/// The decoder: scratch tables and cumulative counts, all allocated in [`Decoder::new`].
// Subsystem (RULESET A1.1, D5): precomputed tables and interface methods.
#[derive(Clone, Debug)]
pub struct Decoder {
    limits: Limits,
    known_paths: Vec<u64>,
    known_attrs: Vec<u64>,
    hash: Vec<u64>,
    state: Vec<u8>,
    dflt_ns: Vec<bool>,
    elem_text: Vec<u32>,
    stats: Stats,
}

/// A value read from an attribute or text: missing, present but undecodable, or its bytes.
#[derive(Clone, Copy)]
enum Raw<'x> {
    Absent,
    Bad,
    Val(&'x [u8]),
}

/// The row being built, in model units.
#[derive(Clone, Copy, Default)]
struct Row {
    present: u64,
    errors: u64,
    flags: u32,
    source: [u8; 2],
    centre: [u8; 4],
    ts_ms: i64,
    gufi: u128,
    callsign: [u8; 8],
    computer_id: [u8; 4],
    plan_id: [u8; 12],
    departure: [u8; 12],
    arrival: [u8; 12],
    status: u8,
    flight_type: u8,
    assigned_ft: i32,
    reported_ft: i32,
    lat_e7: i32,
    lon_e7: i32,
    pos_time_ms: i64,
    gs_dkt: u16,
    vx_dkt: i16,
    vy_dkt: i16,
    aircraft_type: [u8; 4],
    route_off: u32,
    route_len: u32,
    operator: [u8; 24],
    beacon: u16,
    ctrl: [u8; 8],
    recv: [u8; 8],
    xfer: [u8; 8],
    dep_actual_ms: i64,
    arr_est_ms: i64,
    msg_seq: u64,
    plan_seq: u32,
    assigned_form: u8,
    requested_form: u8,
    requested_ft: i32,
    interim_ft: i32,
    block_above_ft: i32,
    block_below_ft: i32,
    fixalt_pre_ft: i32,
    fixalt_post_ft: i32,
    fixalt_point: [u8; 12],
    handoff_event: u8,
    acpt: [u8; 8],
    address: u32,
    /// `coastIndicator` carries nothing beyond its presence; this only receives the parse.
    coast: u8,
}

/// The message being decoded.
struct Msg {
    row: Row,
    reject: Option<Reject>,
    has_flight: bool,
    has_gufi: bool,
    has_ts: bool,
    vx: Option<i16>,
    vy: Option<i16>,
    vel_seen: u8,
    vel_bad: bool,
    /// Bytes of route text written past `strings_used` (committed with the row).
    route_bytes: usize,
    /// The altitude-form elements seen under `assignedAltitude` and `requestedAltitude`, one
    /// bit per form (`FORM_BIT_*`); a form that is not alone is a field error.
    aforms: u8,
    rforms: u8,
    /// How many form elements were seen (a repeated one is a bad form).
    acount: u8,
    rcount: u8,
    /// The values of a block and of an altitude at a fix, each `Some` once read well, and
    /// whether any of those elements was bad or repeated.
    block_above: Option<i32>,
    block_below: Option<i32>,
    fix_pre: Option<i32>,
    fix_post: Option<i32>,
    fix_point: Option<[u8; 12]>,
    aform_bad: bool,
    rform_bad: bool,
}

fn new_msg() -> Msg {
    Msg {
        row: Row::default(),
        reject: None,
        has_flight: false,
        has_gufi: false,
        has_ts: false,
        vx: None,
        vy: None,
        vel_seen: 0,
        vel_bad: false,
        route_bytes: 0,
        aforms: 0,
        rforms: 0,
        acount: 0,
        rcount: 0,
        block_above: None,
        block_below: None,
        fix_pre: None,
        fix_post: None,
        fix_point: None,
        aform_bad: false,
        rform_bad: false,
    }
}

/// Bits of `Msg::aforms` and `Msg::rforms`: which form element was seen.
const FORM_BIT_SIMPLE: u8 = 1;
const FORM_BIT_VFR_PLUS: u8 = 2;
const FORM_BIT_VFR: u8 = 4;
const FORM_BIT_BLOCK: u8 = 8;
const FORM_BIT_AT_FIX: u8 = 16;

fn reject(m: &mut Msg, why: Reject) {
    if m.reject.is_none() {
        m.reject = Some(why);
    }
}

fn seen(m: &Msg, f: Field) -> bool {
    (m.row.present | m.row.errors) & field_bit(f) != 0
}

fn mark_ok(m: &mut Msg, f: Field) {
    m.row.present |= field_bit(f);
}

fn mark_err(m: &mut Msg, f: Field) {
    m.row.present &= !field_bit(f);
    m.row.errors |= field_bit(f);
}

/// Stores `parsed` through `dst` unless the field was already seen; a duplicate, or a value
/// that did not parse, is a field error.
fn store<T>(m: &mut Msg, f: Field, parsed: Option<T>, dst: fn(&mut Row) -> &mut T) {
    if seen(m, f) {
        mark_err(m, f);
        return;
    }
    match parsed {
        Some(v) => {
            *dst(&mut m.row) = v;
            mark_ok(m, f);
        }
        None => mark_err(m, f),
    }
}

/// `store` over a raw read: absent does nothing, undecodable is an error.
fn field<T>(
    m: &mut Msg,
    f: Field,
    raw: Raw<'_>,
    parse: impl FnOnce(&[u8]) -> Option<T>,
    dst: fn(&mut Row) -> &mut T,
) {
    match raw {
        Raw::Absent => {}
        Raw::Bad => store(m, f, None, dst),
        Raw::Val(b) => store(m, f, parse(b), dst),
    }
}

fn raw_bytes<'x>(
    input: &'x [u8],
    sp: Span,
    entity: bool,
    kind: SpanKind,
    buf: &'x mut [u8],
) -> Raw<'x> {
    if !entity {
        return match span_bytes(input, sp) {
            Ok(b) => Raw::Val(b),
            Err(_) => Raw::Bad,
        };
    }
    let Ok(n) = decode_into(sp, input, &mut *buf, kind) else {
        return Raw::Bad;
    };
    let buf: &'x [u8] = buf;
    match buf.get(..n) {
        Some(b) => Raw::Val(b),
        None => Raw::Bad,
    }
}

/// The unprefixed attribute `name` of element `i`.
fn find_attr(doc: &Doc<'_>, i: usize, name: &[u8]) -> Option<usize> {
    let first = doc.elem_first_attr.get(i)?.0 as usize;
    let count = *doc.elem_attr_count.get(i)? as usize;
    for a in first..first.saturating_add(count) {
        let unprefixed = doc.attr_prefix.get(a).is_some_and(|p| p.len == 0);
        let hit = doc
            .attr_name
            .get(a)
            .is_some_and(|&n| name_bytes(doc.input, n) == name);
        if unprefixed && hit {
            return Some(a);
        }
    }
    None
}

fn attr<'x>(doc: &Doc<'x>, i: usize, name: &[u8], buf: &'x mut [u8]) -> Raw<'x> {
    let Some(a) = find_attr(doc, i, name) else {
        return Raw::Absent;
    };
    let Some(&sp) = doc.attr_value.get(a) else {
        return Raw::Bad;
    };
    let ent = attr_needs_decode(doc, AttrIx(a as u32));
    raw_bytes(doc.input, sp, ent, SpanKind::AttrValue, buf)
}

/// The text of element `i`, trimmed. An element with no text, or with several text nodes, is
/// `Bad`: a value element is never empty.
fn text<'x>(doc: &Doc<'x>, texts: &[u32], i: usize, buf: &'x mut [u8]) -> Raw<'x> {
    let t = texts.get(i).copied().unwrap_or(NO_TEXT);
    if t == NO_TEXT || t == MANY_TEXT {
        return Raw::Bad;
    }
    let Some(&sp) = doc.text_span.get(t as usize) else {
        return Raw::Bad;
    };
    let ent = text_needs_decode(doc, TextIx(t));
    match raw_bytes(doc.input, sp, ent, SpanKind::Text, buf) {
        Raw::Val(b) => Raw::Val(trim(b)),
        other => other,
    }
}

fn parse_alt(text: &[u8], uom: &[u8]) -> Option<i32> {
    let ft = match uom {
        b"FEET" => parse_scaled(text, 0)?,
        b"METERS" => rescale(parse_scaled(text, 1)?, 1000, 3048)?,
        _ => return None,
    };
    if (ALT_MIN_FT..=ALT_MAX_FT).contains(&ft) {
        i32::try_from(ft).ok()
    } else {
        None
    }
}

/// A speed in 0.1 knot from its text and `uom`.
fn parse_speed(text: &[u8], uom: &[u8]) -> Option<i64> {
    let v = parse_scaled(text, 1)?;
    match uom {
        b"KNOTS" => Some(v),
        b"KILOMETERS_PER_HOUR" => rescale(v, 539_957, 1_000_000),
        b"METERS_PER_SECOND" => rescale(v, 1_943_844, 1_000_000),
        _ => None,
    }
}

fn parse_gs(text: &[u8], uom: &[u8]) -> Option<u16> {
    let v = parse_speed(text, uom)?;
    if (0..=SPEED_MAX_DKT).contains(&v) {
        u16::try_from(v).ok()
    } else {
        None
    }
}

fn parse_vel(text: &[u8], uom: &[u8]) -> Option<i16> {
    let v = parse_speed(text, uom)?;
    if (-SPEED_MAX_DKT..=SPEED_MAX_DKT).contains(&v) {
        i16::try_from(v).ok()
    } else {
        None
    }
}

/// `lat lon` in decimal degrees, one space apart, to 1e-7 degree.
fn parse_latlon(s: &[u8]) -> Option<(i32, i32)> {
    let sp = s.iter().position(|&b| b == b' ')?;
    let lat = parse_scaled(s.get(..sp)?, 7)?;
    let lon = parse_scaled(s.get(sp + 1..)?, 7)?;
    if !(-900_000_000..=900_000_000).contains(&lat)
        || !(-1_800_000_000..=1_800_000_000).contains(&lon)
    {
        return None;
    }
    Some((i32::try_from(lat).ok()?, i32::try_from(lon).ok()?))
}

fn parse_status(s: &[u8]) -> Option<u8> {
    match s {
        b"PROPOSED" => Some(STATUS_PROPOSED),
        b"ACTIVE" => Some(STATUS_ACTIVE),
        b"COMPLETED" => Some(STATUS_COMPLETED),
        b"CANCELLED" => Some(STATUS_CANCELLED),
        b"DROPPED" => Some(STATUS_DROPPED),
        _ => None,
    }
}

fn parse_ftype(s: &[u8]) -> Option<u8> {
    match s {
        b"SCHEDULED" => Some(FTYPE_SCHEDULED),
        b"NON_SCHEDULED" => Some(FTYPE_NON_SCHEDULED),
        b"GENERAL" => Some(FTYPE_GENERAL),
        _ => None,
    }
}

fn parse_handoff_event(s: &[u8]) -> Option<u8> {
    match s {
        b"INITIATION" => Some(HANDOFF_INITIATION),
        b"ACCEPTANCE" => Some(HANDOFF_ACCEPTANCE),
        b"UPDATE" => Some(HANDOFF_UPDATE),
        b"RETRACTION" => Some(HANDOFF_RETRACTION),
        b"FAILURE" => Some(HANDOFF_FAILURE),
        b"TAKE_CONTROL" => Some(HANDOFF_TAKE_CONTROL),
        _ => None,
    }
}

fn parse_coast(s: &[u8]) -> Option<u8> {
    (s == b"COASTING").then_some(1)
}

/// Letters and digits plus `/` for fix and point names.
fn parse_point(s: &[u8]) -> Option<[u8; 12]> {
    parse_code::<12>(s, b"/")
}

/// An aircraft-type code reads as at most four letters and digits.
fn parse_type(s: &[u8]) -> Option<[u8; 4]> {
    parse_code::<4>(s, b"")
}

/// A unit identifier and an optional sector identifier packed into eight bytes.
fn unit_pair(unit: Raw<'_>, sector: Raw<'_>) -> Option<Option<[u8; 8]>> {
    let u = match unit {
        Raw::Absent => return Some(None),
        Raw::Bad => return None,
        Raw::Val(b) => parse_code::<4>(b, b"?")?,
    };
    let mut out = [0u8; 8];
    for (o, b) in out.iter_mut().zip(u) {
        *o = b;
    }
    match sector {
        Raw::Absent => {}
        Raw::Bad => return None,
        Raw::Val(b) => {
            let s = parse_code::<4>(b, b"")?;
            for (o, b) in out.iter_mut().skip(4).zip(s) {
                *o = b;
            }
        }
    }
    Some(Some(out))
}

fn is_xmlns(doc: &Doc<'_>, a: usize) -> bool {
    let prefix = doc.attr_prefix.get(a).copied().unwrap_or_default();
    let name = doc.attr_name.get(a).copied().unwrap_or_default();
    (prefix.len == 0 && name_bytes(doc.input, name) == b"xmlns")
        || name_bytes(doc.input, prefix) == b"xmlns"
}

/// Reads the `timestamp` and `positionTime` of every element of the document that has one,
/// directly from the parsed tree and whatever else is valid in it. If any is in `window`,
/// returns how many messages the document has (all are dropped, at least 1); otherwise 0. A time
/// that cannot be read is not in the window (the frame's receive time gates it).
fn prescan(doc: &Doc<'_>, window: oa_sealed::DropWindow) -> u32 {
    if window.span().is_none() {
        return 0;
    }
    let mut hit = false;
    for i in 1..doc.elem_name.len() {
        for name in [&b"timestamp"[..], b"positionTime"] {
            let mut buf = [0u8; 40];
            if let Raw::Val(v) = attr(doc, i, name, &mut buf) {
                hit |= parse_iso_ms(v).is_some_and(|t| window.contains(t));
            }
        }
    }
    if !hit {
        return 0;
    }
    let messages = (1..doc.elem_name.len())
        .filter(|&i| doc.elem_parent.get(i).is_some_and(|p| p.0 == 0) && is_message_elem(doc, i))
        .count();
    u32::try_from(messages).unwrap_or(u32::MAX).max(1)
}

fn attr_range(doc: &Doc<'_>, i: usize) -> core::ops::Range<usize> {
    let first = doc.elem_first_attr.get(i).map_or(0, |a| a.0 as usize);
    let count = doc.elem_attr_count.get(i).map_or(0, |&c| c as usize);
    first..first.saturating_add(count)
}

/// Whether element `i` carries `xsi:nil`: `Some(true)` for `"true"` or `"1"`, `Some(false)` for
/// `"false"` or `"0"`, `None` when there is no such attribute or its value is neither (the
/// caller counts that as a bad value).
fn xsi_nil(doc: &Doc<'_>, i: usize) -> Option<bool> {
    let mut buf = [0u8; 16];
    for a in attr_range(doc, i) {
        let prefix = doc.attr_prefix.get(a).copied().unwrap_or_default();
        let name = doc.attr_name.get(a).copied().unwrap_or_default();
        if prefix.len == 0 || name_bytes(doc.input, name) != b"nil" {
            continue;
        }
        let xsi = matches!(
            resolve_attr_ns(doc, ElemIx(i as u32), AttrIx(a as u32)),
            Ns::Uri(u) if uri_bytes(doc, u) == NS_XSI
        );
        if !xsi {
            continue;
        }
        let &sp = doc.attr_value.get(a)?;
        let ent = attr_needs_decode(doc, AttrIx(a as u32));
        return match raw_bytes(doc.input, sp, ent, SpanKind::AttrValue, &mut buf) {
            Raw::Val(b"true" | b"1") => Some(true),
            Raw::Val(b"false" | b"0") => Some(false),
            _ => None,
        };
    }
    None
}

/// Whether element `i` has an `xsi:nil` attribute at all, whatever its value.
fn find_xsi_nil_attr(doc: &Doc<'_>, i: usize) -> bool {
    attr_range(doc, i).any(|a| {
        let prefix = doc.attr_prefix.get(a).copied().unwrap_or_default();
        let name = doc.attr_name.get(a).copied().unwrap_or_default();
        prefix.len > 0
            && name_bytes(doc.input, name) == b"nil"
            && matches!(
                resolve_attr_ns(doc, ElemIx(i as u32), AttrIx(a as u32)),
                Ns::Uri(u) if uri_bytes(doc, u) == NS_XSI
            )
    })
}

/// A feet value from an altitude element: its text and `uom`.
fn alt_value(doc: &Doc<'_>, texts: &[u32], i: usize, b1: &mut [u8], b2: &mut [u8]) -> Option<i32> {
    let uom = attr(doc, i, b"uom", b2);
    match (text(doc, texts, i, b1), uom) {
        (Raw::Val(t), Raw::Val(u)) => parse_alt(t, u),
        _ => None,
    }
}

impl Decoder {
    /// Builds a decoder: every table is allocated here and never again.
    pub fn new(limits: Limits) -> Decoder {
        let mut known_paths = Vec::with_capacity(KNOWN.len());
        let mut known_attrs = Vec::new();
        for (p, attrs) in KNOWN {
            let mut h = extend(SEED, b"message");
            for seg in p.split('/').filter(|s| !s.is_empty()) {
                h = extend(h, seg.as_bytes());
            }
            known_paths.push(h);
            for a in *attrs {
                known_attrs.push(attr_hash(h, a.as_bytes()));
            }
        }
        known_paths.sort_unstable();
        known_attrs.sort_unstable();
        let n = limits.max_elements;
        Decoder {
            limits,
            known_paths,
            known_attrs,
            hash: vec![0; n],
            state: vec![0; n],
            dflt_ns: vec![false; n],
            elem_text: vec![NO_TEXT; n],
            stats: Stats {
                docs: 0,
                sealed_window_dropped: 0,
                unsupported_roots: 0,
                messages: 0,
                rows: 0,
                rejected: [0; REJECT_COUNT],
                field_errors: [0; FIELD_COUNT],
                unknown_elements: 0,
                unknown_attributes: 0,
                unknown_overflow: 0,
                unknown: Vec::with_capacity(limits.max_unknown_paths),
            },
        }
    }

    /// The limits this decoder was built with.
    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// Cumulative counts since construction.
    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    /// Decodes every `message` of `doc` (parsed from `input`) and appends one row per accepted
    /// message to `out`. Either the whole document is appended or, on `Err`, `out` is exactly
    /// as it was. A message refused whole, or with field errors, is counted in the summary and
    /// in [`Decoder::stats`]; those are not errors of the call.
    pub fn decode(
        &mut self,
        doc: &Doc<'_>,
        input: &[u8],
        out: &mut Events,
    ) -> Result<Summary, FixmError> {
        self.decode_with(doc, input, out, false)
    }

    /// Like [`Decoder::decode`], but a document that does not fit `out` (rows or route text) is
    /// decoded as far as it fits instead of being refused: the messages before the first one that
    /// does not fit are appended whole, and [`Summary::not_decoded`] counts the others. The
    /// other errors still leave `out` as it was.
    pub fn decode_prefix(
        &mut self,
        doc: &Doc<'_>,
        input: &[u8],
        out: &mut Events,
    ) -> Result<Summary, FixmError> {
        self.decode_with(doc, input, out, true)
    }

    fn decode_with(
        &mut self,
        doc: &Doc<'_>,
        input: &[u8],
        out: &mut Events,
        prefix: bool,
    ) -> Result<Summary, FixmError> {
        if doc.input.len() != input.len() || doc.input.as_ptr() != input.as_ptr() {
            return Err(FixmError::InputMismatch);
        }
        // The first step, before any count, row or table is touched (and before any refusal an
        // in-window message could cause): a document with a message timed in the sealed window
        // is dropped whole.
        let gone = prescan(doc, self.limits.drop_window);
        if gone > 0 {
            self.stats.sealed_window_dropped = self
                .stats
                .sealed_window_dropped
                .saturating_add(u64::from(gone));
            return Ok(Summary {
                sealed_window_dropped: gone,
                ..Summary::default()
            });
        }
        self.stats.docs = self.stats.docs.saturating_add(1);
        let n = doc.elem_name.len();
        if n > self.limits.max_elements {
            return Err(FixmError::TooManyElements {
                elements: n,
                limit: self.limits.max_elements,
            });
        }
        let root_ok = n > 0
            && name_bytes(input, doc.elem_name.first().copied().unwrap_or_default())
                == b"MessageCollection"
            && matches!(resolve_elem_ns(doc, ElemIx(0)), Ns::Uri(u) if uri_bytes(doc, u) == NS_NAS);
        if !root_ok {
            self.stats.unsupported_roots = self.stats.unsupported_roots.saturating_add(1);
            return Err(FixmError::UnsupportedRoot);
        }
        // Rows needed in the worst case: every message accepted.
        let mut messages = 0usize;
        for (i, p) in doc.elem_parent.iter().enumerate() {
            if i > 0 && p.0 == 0 && is_message_elem(doc, i) {
                messages += 1;
            }
        }
        if !prefix && out.len.saturating_add(messages) > out.capacity {
            return Err(FixmError::RowCapacity {
                capacity: out.capacity,
            });
        }
        let start_len = out.len;
        let start_strings = out.strings_used;
        let mut sum = Summary::default();
        let r = self.walk(doc, out, &mut sum);
        match r {
            // Every row appended so far was committed whole, with its route text.
            Err(FixmError::RowCapacity { .. } | FixmError::StringCapacity { .. }) if prefix => {
                let done = sum.rows.saturating_add(sum.rejected);
                sum.not_decoded = u32::try_from(messages)
                    .unwrap_or(u32::MAX)
                    .saturating_sub(done);
            }
            Err(e) => {
                truncate_events(out, start_len, start_strings);
                return Err(e);
            }
            Ok(()) => {}
        }
        Ok(sum)
    }

    fn walk(
        &mut self,
        doc: &Doc<'_>,
        out: &mut Events,
        sum: &mut Summary,
    ) -> Result<(), FixmError> {
        let n = doc.elem_name.len();
        // First text node of each element, for value elements.
        self.elem_text.iter_mut().take(n).for_each(|t| *t = NO_TEXT);
        for (t, owner) in doc.text_owner.iter().enumerate() {
            if let Some(slot) = self.elem_text.get_mut(owner.0 as usize) {
                *slot = if *slot == NO_TEXT {
                    t as u32
                } else {
                    MANY_TEXT
                };
            }
        }
        if let Some(d) = self.dflt_ns.first_mut() {
            *d = attr_range(doc, 0).any(|a| {
                doc.attr_prefix.get(a).is_some_and(|p| p.len == 0)
                    && doc
                        .attr_name
                        .get(a)
                        .is_some_and(|&nm| name_bytes(doc.input, nm) == b"xmlns")
            });
        }
        if let Some(s) = self.state.first_mut() {
            *s = S_KNOWN;
        }
        let mut cur: Option<Msg> = None;
        for i in 1..n {
            let p = doc.elem_parent.get(i).copied().unwrap_or(ElemIx::NONE);
            let pi = p.0 as usize;
            let pstate = self.state.get(pi).copied().unwrap_or(S_BELOW);
            // Default namespace in scope: the parent's, or declared here.
            let declares = attr_range(doc, i).any(|a| {
                doc.attr_prefix.get(a).is_some_and(|x| x.len == 0)
                    && doc
                        .attr_name
                        .get(a)
                        .is_some_and(|&nm| name_bytes(doc.input, nm) == b"xmlns")
            });
            let dflt = declares || self.dflt_ns.get(pi).copied().unwrap_or(false);
            if let Some(d) = self.dflt_ns.get_mut(i) {
                *d = dflt;
            }
            if pstate != S_KNOWN {
                if let Some(s) = self.state.get_mut(i) {
                    *s = S_BELOW;
                }
                continue;
            }
            let is_msg_level = p.0 == 0;
            if is_msg_level {
                if let Some(m) = cur.take() {
                    self.finish(m, out, sum)?;
                }
            }
            // Path hash of this element.
            let prefix = doc.elem_prefix.get(i).copied().unwrap_or_default();
            let name = doc.elem_name.get(i).copied().unwrap_or_default();
            let ns = if prefix.len > 0 || dflt {
                resolve_elem_ns(doc, ElemIx(i as u32))
            } else {
                Ns::None
            };
            let base = if is_msg_level {
                SEED
            } else {
                self.hash.get(pi).copied().unwrap_or(0)
            };
            let mut h = extend(base, name_bytes(doc.input, name));
            match ns {
                Ns::None => {}
                Ns::Xml => h = extend(h, b"{xml"),
                Ns::Uri(u) => h = extend(extend(h, b"{"), uri_bytes(doc, u)),
            }
            if let Some(x) = self.hash.get_mut(i) {
                *x = h;
            }
            let known = if is_msg_level {
                is_message_elem(doc, i)
            } else {
                self.known_paths.binary_search(&h).is_ok()
            };
            if !known {
                if let Some(s) = self.state.get_mut(i) {
                    *s = S_UNKNOWN;
                }
                self.note_unknown(doc, i, None, h, sum);
                continue;
            }
            if let Some(s) = self.state.get_mut(i) {
                *s = S_KNOWN;
            }
            self.check_attrs(doc, i, h, sum);
            if is_msg_level {
                let mut m = new_msg();
                sum.messages = sum.messages.saturating_add(1);
                self.stats.messages = self.stats.messages.saturating_add(1);
                let mut buf = [0u8; 64];
                if !is_flight_message(doc, i, &mut buf) {
                    reject(&mut m, Reject::NotFlightMessage);
                }
                cur = Some(m);
                continue;
            }
            if let Some(m) = cur.as_mut() {
                self.visit(doc, i, h, m, out, sum)?;
            }
        }
        if let Some(m) = cur.take() {
            self.finish(m, out, sum)?;
        }
        Ok(())
    }

    /// Counts every attribute of known element `i` that is not in the known table.
    fn check_attrs(&mut self, doc: &Doc<'_>, i: usize, h: u64, sum: &mut Summary) {
        for a in attr_range(doc, i) {
            if is_xmlns(doc, a) {
                continue;
            }
            let prefix = doc.attr_prefix.get(a).copied().unwrap_or_default();
            let name = doc.attr_name.get(a).copied().unwrap_or_default();
            let nb = name_bytes(doc.input, name);
            if prefix.len > 0 {
                let xsi = matches!(
                    resolve_attr_ns(doc, ElemIx(i as u32), AttrIx(a as u32)),
                    Ns::Uri(u) if uri_bytes(doc, u) == NS_XSI
                );
                if xsi && (nb == b"type" || nb == b"nil") {
                    continue;
                }
                let ah = extend(attr_hash(h, name_bytes(doc.input, prefix)), nb);
                self.note_unknown(doc, i, Some(a), ah, sum);
                continue;
            }
            let ah = attr_hash(h, nb);
            if self.known_attrs.binary_search(&ah).is_err() {
                self.note_unknown(doc, i, Some(a), ah, sum);
            }
        }
    }

    /// Counts an unknown element (`attr` is `None`) or attribute.
    fn note_unknown(
        &mut self,
        doc: &Doc<'_>,
        i: usize,
        attr: Option<usize>,
        h: u64,
        sum: &mut Summary,
    ) {
        if attr.is_some() {
            self.stats.unknown_attributes = self.stats.unknown_attributes.saturating_add(1);
            sum.unknown_attributes = sum.unknown_attributes.saturating_add(1);
        } else {
            self.stats.unknown_elements = self.stats.unknown_elements.saturating_add(1);
            sum.unknown_elements = sum.unknown_elements.saturating_add(1);
        }
        if let Some(u) = self.stats.unknown.iter_mut().find(|u| u.hash == h) {
            u.count = u.count.saturating_add(1);
            return;
        }
        if self.stats.unknown.len() >= self.limits.max_unknown_paths {
            self.stats.unknown_overflow = self.stats.unknown_overflow.saturating_add(1);
            return;
        }
        let mut u = UnknownPath {
            hash: h,
            count: 1,
            is_attr: attr.is_some(),
            len: 0,
            text: [0; MAX_PATH_TEXT],
        };
        let len = path_text(doc, i, attr, &mut u.text);
        u.len = u8::try_from(len).unwrap_or(u8::MAX);
        self.stats.unknown.push(u);
    }

    /// Reads whatever element `i` (a known path) contributes to the message.
    fn visit(
        &mut self,
        doc: &Doc<'_>,
        i: usize,
        h: u64,
        m: &mut Msg,
        out: &mut Events,
        sum: &mut Summary,
    ) -> Result<(), FixmError> {
        let mut b1 = [0u8; 64];
        let mut b2 = [0u8; 64];
        let texts = &self.elem_text;
        match h {
            P_FLIGHT => {
                if m.has_flight {
                    reject(m, Reject::DuplicateFlight);
                }
                m.has_flight = true;
                field(
                    m,
                    Field::Source,
                    attr(doc, i, b"source", &mut b1),
                    |b| parse_code::<2>(b, b""),
                    |r| &mut r.source,
                );
                field(
                    m,
                    Field::Centre,
                    attr(doc, i, b"centre", &mut b1),
                    |b| parse_code::<4>(b, b""),
                    |r| &mut r.centre,
                );
                field(
                    m,
                    Field::FlightType,
                    attr(doc, i, b"flightType", &mut b1),
                    parse_ftype,
                    |r| &mut r.flight_type,
                );
                match attr(doc, i, b"timestamp", &mut b1) {
                    Raw::Absent => reject(m, Reject::MissingTimestamp),
                    Raw::Bad => reject(m, Reject::BadTimestamp),
                    Raw::Val(b) => match parse_iso_ms(b) {
                        Some(ms) if !m.has_ts => {
                            m.row.ts_ms = ms;
                            m.has_ts = true;
                        }
                        _ => reject(m, Reject::BadTimestamp),
                    },
                }
            }
            P_GUFI => {
                let code_ok = matches!(attr(doc, i, b"codeSpace", &mut b2), Raw::Val(b"urn:uuid"));
                let parsed = match text(doc, texts, i, &mut b1) {
                    Raw::Val(b) if code_ok => parse_uuid(b),
                    _ => None,
                };
                match parsed {
                    Some(g) if !m.has_gufi => {
                        m.row.gufi = g;
                        m.has_gufi = true;
                    }
                    _ => {
                        let empty = texts.get(i).copied().unwrap_or(NO_TEXT) == NO_TEXT;
                        if empty && !m.has_gufi {
                            reject(m, Reject::MissingGufi);
                        } else {
                            reject(m, Reject::BadGufi);
                        }
                    }
                }
            }
            P_FLIGHT_ID => {
                field(
                    m,
                    Field::Callsign,
                    attr(doc, i, b"aircraftIdentification", &mut b1),
                    |b| parse_code::<8>(b, b""),
                    |r| &mut r.callsign,
                );
                field(
                    m,
                    Field::ComputerId,
                    attr(doc, i, b"computerId", &mut b1),
                    |b| parse_code::<4>(b, b""),
                    |r| &mut r.computer_id,
                );
            }
            P_FLIGHT_PLAN => {
                field(
                    m,
                    Field::FlightPlanId,
                    attr(doc, i, b"identifier", &mut b1),
                    |b| parse_code::<12>(b, b""),
                    |r| &mut r.plan_id,
                );
            }
            P_STATUS => {
                field(
                    m,
                    Field::Status,
                    attr(doc, i, b"fdpsFlightStatus", &mut b1),
                    parse_status,
                    |r| &mut r.status,
                );
            }
            P_ARRIVAL => {
                field(
                    m,
                    Field::Arrival,
                    attr(doc, i, b"arrivalPoint", &mut b1),
                    parse_point,
                    |r| &mut r.arrival,
                );
            }
            P_DEPARTURE => {
                field(
                    m,
                    Field::Departure,
                    attr(doc, i, b"departurePoint", &mut b1),
                    parse_point,
                    |r| &mut r.departure,
                );
            }
            P_DEP_ACTUAL => {
                field(
                    m,
                    Field::DepartureActual,
                    attr(doc, i, b"time", &mut b1),
                    parse_iso_ms,
                    |r| &mut r.dep_actual_ms,
                );
            }
            P_ARR_EST => {
                field(
                    m,
                    Field::ArrivalEstimated,
                    attr(doc, i, b"time", &mut b1),
                    parse_iso_ms,
                    |r| &mut r.arr_est_ms,
                );
            }
            P_ASSIGNED_SIMPLE | P_ASSIGNED_VFRPLUS => {
                if h == P_ASSIGNED_VFRPLUS {
                    m.row.flags |= FLAG_ASSIGNED_VFR_PLUS;
                    m.aforms |= FORM_BIT_VFR_PLUS;
                } else {
                    m.aforms |= FORM_BIT_SIMPLE;
                }
                m.acount = m.acount.saturating_add(1);
                let uom = attr(doc, i, b"uom", &mut b2);
                let parsed = match (text(doc, texts, i, &mut b1), uom) {
                    (Raw::Val(t), Raw::Val(u)) => parse_alt(t, u),
                    _ => None,
                };
                store(m, Field::AssignedAlt, parsed, |r| &mut r.assigned_ft);
            }
            P_ASSIGNED_VFR => {
                m.row.flags |= FLAG_ASSIGNED_VFR;
                m.aforms |= FORM_BIT_VFR;
                m.acount = m.acount.saturating_add(1);
            }
            P_ASSIGNED_BLOCK => {
                m.aforms |= FORM_BIT_BLOCK;
                m.acount = m.acount.saturating_add(1);
            }
            P_BLOCK_ABOVE | P_BLOCK_BELOW => {
                let v = alt_value(doc, texts, i, &mut b1, &mut b2);
                let slot = if h == P_BLOCK_ABOVE {
                    &mut m.block_above
                } else {
                    &mut m.block_below
                };
                if v.is_none() || slot.is_some() {
                    m.aform_bad = true;
                }
                *slot = v.or(*slot);
            }
            P_ALTFIX => {
                m.aforms |= FORM_BIT_AT_FIX;
                m.acount = m.acount.saturating_add(1);
            }
            P_ALTFIX_PRE | P_ALTFIX_POST => {
                let v = alt_value(doc, texts, i, &mut b1, &mut b2);
                let slot = if h == P_ALTFIX_PRE {
                    &mut m.fix_pre
                } else {
                    &mut m.fix_post
                };
                if v.is_none() || slot.is_some() {
                    m.aform_bad = true;
                }
                *slot = v.or(*slot);
            }
            P_ALTFIX_POINT => {
                let point = match attr(doc, i, b"fix", &mut b1) {
                    Raw::Val(b) => parse_point(b),
                    _ => None,
                };
                if point.is_none() || m.fix_point.is_some() {
                    m.aform_bad = true;
                }
                m.fix_point = point.or(m.fix_point);
            }
            P_INTERIM => {
                let nil = xsi_nil(doc, i);
                let has_nil_attr = find_xsi_nil_attr(doc, i);
                if has_nil_attr && nil.is_none() {
                    store(m, Field::InterimAlt, None, |r| &mut r.interim_ft);
                } else if nil == Some(true) {
                    // ERAM says there is no interim altitude. The element has no text.
                    if seen(m, Field::InterimAlt) {
                        mark_err(m, Field::InterimAlt);
                    } else {
                        m.row.flags |= FLAG_INTERIM_NIL;
                        mark_ok(m, Field::InterimAlt);
                    }
                } else {
                    let v = alt_value(doc, texts, i, &mut b1, &mut b2);
                    store(m, Field::InterimAlt, v, |r| &mut r.interim_ft);
                }
            }
            P_REQ_SIMPLE | P_REQ_VFRPLUS => {
                m.rforms |= if h == P_REQ_SIMPLE {
                    FORM_BIT_SIMPLE
                } else {
                    FORM_BIT_VFR_PLUS
                };
                m.rcount = m.rcount.saturating_add(1);
                let v = alt_value(doc, texts, i, &mut b1, &mut b2);
                if v.is_none() {
                    m.rform_bad = true;
                }
                if let Some(v) = v {
                    m.row.requested_ft = v;
                }
            }
            P_REQ_VFR => {
                m.rforms |= FORM_BIT_VFR;
                m.rcount = m.rcount.saturating_add(1);
            }
            P_AIRCRAFT_DESC => {
                let raw = attr(doc, i, b"aircraftAddress", &mut b1);
                field(m, Field::AircraftAddress, raw, parse_hex24, |r| {
                    &mut r.address
                });
            }
            P_HANDOFF => {
                field(
                    m,
                    Field::HandoffEvent,
                    attr(doc, i, b"event", &mut b1),
                    parse_handoff_event,
                    |r| &mut r.handoff_event,
                );
            }
            P_POSITION => {
                field(
                    m,
                    Field::PositionTime,
                    attr(doc, i, b"positionTime", &mut b1),
                    parse_iso_ms,
                    |r| &mut r.pos_time_ms,
                );
                field(
                    m,
                    Field::Coast,
                    attr(doc, i, b"coastIndicator", &mut b2),
                    parse_coast,
                    |r| &mut r.coast,
                );
            }
            P_REPORTED_ALT => {
                let uom = attr(doc, i, b"uom", &mut b2);
                let parsed = match (text(doc, texts, i, &mut b1), uom) {
                    (Raw::Val(t), Raw::Val(u)) => parse_alt(t, u),
                    _ => None,
                };
                store(m, Field::ReportedAlt, parsed, |r| &mut r.reported_ft);
            }
            P_GS => {
                let uom = attr(doc, i, b"uom", &mut b2);
                let parsed = match (text(doc, texts, i, &mut b1), uom) {
                    (Raw::Val(t), Raw::Val(u)) => parse_gs(t, u),
                    _ => None,
                };
                store(m, Field::GroundSpeed, parsed, |r| &mut r.gs_dkt);
            }
            P_VX | P_VY => {
                let uom = attr(doc, i, b"uom", &mut b2);
                let v = match (text(doc, texts, i, &mut b1), uom) {
                    (Raw::Val(t), Raw::Val(u)) => parse_vel(t, u),
                    _ => None,
                };
                let bit = if h == P_VX { 1 } else { 2 };
                if m.vel_seen & bit != 0 || v.is_none() {
                    m.vel_bad = true;
                }
                m.vel_seen |= bit;
                if h == P_VX {
                    m.vx = v;
                } else {
                    m.vy = v;
                }
            }
            P_POS => {
                let srs_ok = {
                    let p = doc.elem_parent.get(i).copied().unwrap_or(ElemIx::NONE);
                    p != ElemIx::NONE
                        && matches!(
                            attr(doc, p.0 as usize, b"srsName", &mut b2),
                            Raw::Val(b"urn:ogc:def:crs:EPSG::4326")
                        )
                };
                let parsed = match text(doc, texts, i, &mut b1) {
                    Raw::Val(t) if srs_ok => parse_latlon(t),
                    _ => None,
                };
                if seen(m, Field::Position) {
                    mark_err(m, Field::Position);
                } else if let Some((lat, lon)) = parsed {
                    m.row.lat_e7 = lat;
                    m.row.lon_e7 = lon;
                    mark_ok(m, Field::Position);
                } else {
                    mark_err(m, Field::Position);
                }
            }
            P_AIRCRAFT_TYPE => {
                let raw = text(doc, texts, i, &mut b1);
                field(m, Field::AircraftType, raw, parse_type, |r| {
                    &mut r.aircraft_type
                });
            }
            P_OPERATOR => {
                field(
                    m,
                    Field::Operator,
                    attr(doc, i, b"name", &mut b1),
                    |b| parse_code::<24>(b, b" -&.'/"),
                    |r| &mut r.operator,
                );
            }
            P_BEACON => {
                let raw = text(doc, texts, i, &mut b1);
                field(m, Field::Beacon, raw, parse_beacon, |r| &mut r.beacon);
            }
            P_CTRL | P_RECV | P_XFER | P_ACPT => {
                let unit = attr(doc, i, b"unitIdentifier", &mut b1);
                let sector = attr(doc, i, b"sectorIdentifier", &mut b2);
                let (f, dst): (Field, fn(&mut Row) -> &mut [u8; 8]) = match h {
                    P_CTRL => (Field::ControllingUnit, |r| &mut r.ctrl),
                    P_RECV => (Field::HandoffReceiving, |r| &mut r.recv),
                    P_ACPT => (Field::HandoffAccepting, |r| &mut r.acpt),
                    _ => (Field::HandoffTransferring, |r| &mut r.xfer),
                };
                match unit_pair(unit, sector) {
                    // An element without a unit says nothing about who has the flight.
                    Some(None) => store(m, f, None, dst),
                    Some(Some(v)) => store(m, f, Some(v), dst),
                    None => store(m, f, None, dst),
                }
            }
            P_ROUTE => {
                let Some(a) = find_attr(doc, i, b"nasRouteText") else {
                    return Ok(());
                };
                self.route(doc, a, m, out)?;
            }
            P_NAMEVALUE => {
                let name = attr(doc, i, b"name", &mut b1);
                let value = attr(doc, i, b"value", &mut b2);
                if let Raw::Val(nm) = name {
                    if nm == b"MSG_SEQ_NO" {
                        let parsed = match value {
                            Raw::Val(v) => parse_uint(v, 18),
                            _ => None,
                        };
                        store(m, Field::MsgSeq, parsed, |r| &mut r.msg_seq);
                    } else if nm == b"FLIGHT_PLAN_SEQ_NO" {
                        let parsed = match value {
                            Raw::Val(v) => parse_uint(v, 9).and_then(|x| u32::try_from(x).ok()),
                            _ => None,
                        };
                        store(m, Field::PlanSeq, parsed, |r| &mut r.plan_seq);
                    } else if !NAMEVALUE_KNOWN.contains(&nm) {
                        self.note_unknown_name(doc, i, nm, sum);
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Counts an unknown `nameValue` name as the path `.../nameValue[NAME]`.
    fn note_unknown_name(&mut self, doc: &Doc<'_>, i: usize, name: &[u8], sum: &mut Summary) {
        let h = extend(extend(self.hash.get(i).copied().unwrap_or(0), b"["), name);
        self.stats.unknown_elements = self.stats.unknown_elements.saturating_add(1);
        sum.unknown_elements = sum.unknown_elements.saturating_add(1);
        if let Some(u) = self.stats.unknown.iter_mut().find(|u| u.hash == h) {
            u.count = u.count.saturating_add(1);
            return;
        }
        if self.stats.unknown.len() >= self.limits.max_unknown_paths {
            self.stats.unknown_overflow = self.stats.unknown_overflow.saturating_add(1);
            return;
        }
        let mut u = UnknownPath {
            hash: h,
            count: 1,
            is_attr: false,
            len: 0,
            text: [0; MAX_PATH_TEXT],
        };
        let mut len = path_text(doc, i, None, &mut u.text);
        let mut push = |b: u8, len: &mut usize| {
            if let Some(s) = u.text.get_mut(*len) {
                *s = b;
                *len += 1;
            }
        };
        push(b'[', &mut len);
        for &b in name {
            if len + 2 > MAX_PATH_TEXT {
                break;
            }
            if b.is_ascii_graphic() {
                push(b, &mut len);
            }
        }
        push(b']', &mut len);
        u.len = u8::try_from(len).unwrap_or(u8::MAX);
        self.stats.unknown.push(u);
    }

    /// Writes route text (attribute `a`) after the committed part of the string table.
    fn route(
        &mut self,
        doc: &Doc<'_>,
        a: usize,
        m: &mut Msg,
        out: &mut Events,
    ) -> Result<(), FixmError> {
        if seen(m, Field::Route) {
            mark_err(m, Field::Route);
            return Ok(());
        }
        let Some(&sp) = doc.attr_value.get(a) else {
            mark_err(m, Field::Route);
            return Ok(());
        };
        let start = out.strings_used;
        let avail = out.strings.len().saturating_sub(start);
        let limit = self.limits.max_route_len;
        let ent = attr_needs_decode(doc, AttrIx(a as u32));
        let written = if ent {
            let end = start + avail.min(limit);
            let Some(dst) = out.strings.get_mut(start..end) else {
                return Err(FixmError::StringCapacity {
                    capacity: out.strings.len(),
                });
            };
            match decode_into(sp, doc.input, dst, SpanKind::AttrValue) {
                Ok(n) => n,
                // A bad reference or span, or a value longer than the route limit.
                Err(e)
                    if !matches!(e, oa_xml::XmlError::OutputTooSmall { .. }) || avail > limit =>
                {
                    mark_err(m, Field::Route);
                    return Ok(());
                }
                Err(_) => {
                    return Err(FixmError::StringCapacity {
                        capacity: out.strings.len(),
                    });
                }
            }
        } else {
            let Ok(src) = span_bytes(doc.input, sp) else {
                mark_err(m, Field::Route);
                return Ok(());
            };
            if src.len() > limit {
                mark_err(m, Field::Route);
                return Ok(());
            }
            let Some(dst) = out.strings.get_mut(start..start + src.len()) else {
                return Err(FixmError::StringCapacity {
                    capacity: out.strings.len(),
                });
            };
            dst.copy_from_slice(src);
            src.len()
        };
        let bytes = out.strings.get(start..start + written).unwrap_or(&[]);
        if written == 0 || std::str::from_utf8(bytes).is_err() {
            mark_err(m, Field::Route);
            return Ok(());
        }
        m.row.route_off = u32::try_from(start).unwrap_or(u32::MAX);
        m.row.route_len = u32::try_from(written).unwrap_or(u32::MAX);
        m.route_bytes = written;
        mark_ok(m, Field::Route);
        Ok(())
    }

    /// Closes a message: refuses it, or appends its row.
    fn finish(&mut self, mut m: Msg, out: &mut Events, sum: &mut Summary) -> Result<(), FixmError> {
        if m.reject.is_none() {
            if !m.has_flight || !m.has_gufi {
                reject(&mut m, Reject::MissingGufi);
            } else if !m.has_ts {
                reject(&mut m, Reject::MissingTimestamp);
            }
        }
        if let Some(why) = m.reject {
            if let Some(c) = self.stats.rejected.get_mut(why as usize) {
                *c = c.saturating_add(1);
            }
            sum.rejected = sum.rejected.saturating_add(1);
            return Ok(());
        }
        // Velocity needs both components.
        if m.vel_seen != 0 {
            match (m.vel_bad, m.vx, m.vy) {
                (false, Some(x), Some(y)) if m.vel_seen == 3 => {
                    m.row.vx_dkt = x;
                    m.row.vy_dkt = y;
                    mark_ok(&mut m, Field::Velocity);
                }
                _ => mark_err(&mut m, Field::Velocity),
            }
        }
        compose_forms(&mut m);
        if out.len >= out.capacity {
            return Err(FixmError::RowCapacity {
                capacity: out.capacity,
            });
        }
        let r = &m.row;
        out.present.push(r.present);
        out.errors.push(r.errors);
        out.flags.push(r.flags);
        out.source.push(r.source);
        out.centre.push(r.centre);
        out.ts_ms.push(r.ts_ms);
        out.gufi.push(r.gufi);
        out.callsign.push(r.callsign);
        out.computer_id.push(r.computer_id);
        out.plan_id.push(r.plan_id);
        out.departure.push(r.departure);
        out.arrival.push(r.arrival);
        out.status.push(r.status);
        out.flight_type.push(r.flight_type);
        out.assigned_ft.push(r.assigned_ft);
        out.reported_ft.push(r.reported_ft);
        out.lat_e7.push(r.lat_e7);
        out.lon_e7.push(r.lon_e7);
        out.pos_time_ms.push(r.pos_time_ms);
        out.gs_dkt.push(r.gs_dkt);
        out.vx_dkt.push(r.vx_dkt);
        out.vy_dkt.push(r.vy_dkt);
        out.aircraft_type.push(r.aircraft_type);
        out.route_off.push(r.route_off);
        out.route_len.push(r.route_len);
        out.operator.push(r.operator);
        out.beacon.push(r.beacon);
        out.ctrl.push(r.ctrl);
        out.recv.push(r.recv);
        out.xfer.push(r.xfer);
        out.dep_actual_ms.push(r.dep_actual_ms);
        out.arr_est_ms.push(r.arr_est_ms);
        out.msg_seq.push(r.msg_seq);
        out.plan_seq.push(r.plan_seq);
        out.assigned_form.push(r.assigned_form);
        out.requested_form.push(r.requested_form);
        out.requested_ft.push(r.requested_ft);
        out.interim_ft.push(r.interim_ft);
        out.block_above_ft.push(r.block_above_ft);
        out.block_below_ft.push(r.block_below_ft);
        out.fixalt_pre_ft.push(r.fixalt_pre_ft);
        out.fixalt_post_ft.push(r.fixalt_post_ft);
        out.fixalt_point.push(r.fixalt_point);
        out.handoff_event.push(r.handoff_event);
        out.acpt.push(r.acpt);
        out.address.push(r.address);
        out.len += 1;
        out.strings_used += m.route_bytes;
        self.stats.rows = self.stats.rows.saturating_add(1);
        sum.rows = sum.rows.saturating_add(1);
        for (f, c) in self.stats.field_errors.iter_mut().enumerate() {
            if r.errors >> f & 1 == 1 {
                *c = c.saturating_add(1);
                sum.field_errors = sum.field_errors.saturating_add(1);
            }
        }
        Ok(())
    }
}

/// The form of a one-of altitude element: its code, or `None` when there is no form, more than
/// one (the single one that is meant cannot be known), or the form's content was bad.
fn single_form(forms: u8, count: u8, bad: bool) -> Option<u8> {
    if forms == 0 || forms.count_ones() != 1 || count != 1 || bad {
        return None;
    }
    Some(forms)
}

/// Turns the form elements of the assigned and the requested altitude into the form columns.
/// A message with two forms of one altitude, or a form whose content is bad, has that altitude's
/// form marked as a field error; the plain `AssignedAlt` field keeps its own rule.
fn compose_forms(m: &mut Msg) {
    if m.aforms != 0 {
        let bits = m.aforms;
        let simple_ok = bits & (FORM_BIT_SIMPLE | FORM_BIT_VFR_PLUS) == 0
            || m.row.present & field_bit(Field::AssignedAlt) != 0;
        let block_ok = bits & FORM_BIT_BLOCK == 0
            || matches!((m.block_above, m.block_below), (Some(a), Some(b)) if a <= b);
        let fix_ok = bits & FORM_BIT_AT_FIX == 0
            || (m.fix_pre.is_some() && m.fix_post.is_some() && m.fix_point.is_some());
        match single_form(
            bits,
            m.acount,
            m.aform_bad || !simple_ok || !block_ok || !fix_ok,
        ) {
            Some(FORM_BIT_SIMPLE) => set_form(m, Field::AssignedForm, ALT_FORM_SIMPLE, true),
            Some(FORM_BIT_VFR_PLUS) => set_form(m, Field::AssignedForm, ALT_FORM_VFR_PLUS, true),
            Some(FORM_BIT_VFR) => set_form(m, Field::AssignedForm, ALT_FORM_VFR, true),
            Some(FORM_BIT_BLOCK) => {
                m.row.block_above_ft = m.block_above.unwrap_or(0);
                m.row.block_below_ft = m.block_below.unwrap_or(0);
                set_form(m, Field::AssignedForm, ALT_FORM_BLOCK, true);
            }
            Some(FORM_BIT_AT_FIX) => {
                m.row.fixalt_pre_ft = m.fix_pre.unwrap_or(0);
                m.row.fixalt_post_ft = m.fix_post.unwrap_or(0);
                m.row.fixalt_point = m.fix_point.unwrap_or([0; 12]);
                set_form(m, Field::AssignedForm, ALT_FORM_AT_FIX, true);
            }
            _ => mark_err(m, Field::AssignedForm),
        }
    }
    if m.rforms != 0 {
        match single_form(m.rforms, m.rcount, m.rform_bad) {
            Some(FORM_BIT_SIMPLE) => set_form(m, Field::RequestedForm, ALT_FORM_SIMPLE, false),
            Some(FORM_BIT_VFR_PLUS) => set_form(m, Field::RequestedForm, ALT_FORM_VFR_PLUS, false),
            Some(FORM_BIT_VFR) => set_form(m, Field::RequestedForm, ALT_FORM_VFR, false),
            _ => mark_err(m, Field::RequestedForm),
        }
    }
}

fn set_form(m: &mut Msg, f: Field, code: u8, assigned: bool) {
    if assigned {
        m.row.assigned_form = code;
    } else {
        m.row.requested_form = code;
    }
    mark_ok(m, f);
}

/// Whether element `i` (a child of the root) is a `message`: that local name in no namespace.
/// The walk and the row-capacity preflight both use this, so they count the same messages.
fn is_message_elem(doc: &Doc<'_>, i: usize) -> bool {
    let name = doc.elem_name.get(i).copied().unwrap_or_default();
    extend(SEED, name_bytes(doc.input, name)) == P_MESSAGE
        && resolve_elem_ns(doc, ElemIx(i as u32)) == Ns::None
}

/// Whether `message` element `i` carries `xsi:type` `FlightMessageType` resolved through the namespaces in scope (any prefix bound to the NAS namespace).
fn is_flight_message(doc: &Doc<'_>, i: usize, buf: &mut [u8]) -> bool {
    for a in attr_range(doc, i) {
        let prefix = doc.attr_prefix.get(a).copied().unwrap_or_default();
        let name = doc.attr_name.get(a).copied().unwrap_or_default();
        if prefix.len == 0 || name_bytes(doc.input, name) != b"type" {
            continue;
        }
        let xsi = matches!(
            resolve_attr_ns(doc, ElemIx(i as u32), AttrIx(a as u32)),
            Ns::Uri(u) if uri_bytes(doc, u) == NS_XSI
        );
        if !xsi {
            continue;
        }
        let Some(&sp) = doc.attr_value.get(a) else {
            return false;
        };
        let ent = attr_needs_decode(doc, AttrIx(a as u32));
        if let Raw::Val(v) = raw_bytes(doc.input, sp, ent, SpanKind::AttrValue, buf) {
            // A QName: the prefix means whatever is bound to it in scope at this element.
            let (pre, local) = match v.iter().position(|&b| b == b':') {
                Some(c) => (v.get(..c).unwrap_or(&[]), v.get(c + 1..).unwrap_or(&[])),
                None => (&[][..], v),
            };
            return local == b"FlightMessageType"
                && matches!(
                    resolve_prefix(doc, ElemIx(i as u32), pre),
                    Ns::Uri(u) if uri_bytes(doc, u) == NS_NAS
                );
        }
        return false;
    }
    false
}

/// Writes the path of element `i` (and `@name` for attribute `attr`) into `out` as local names
/// joined by `/`, from the root's child down. Returns the length; a path that does not fit
/// ends in `~` after the last whole name.
fn path_text(doc: &Doc<'_>, i: usize, attr: Option<usize>, out: &mut [u8; MAX_PATH_TEXT]) -> usize {
    let mut chain = [0u32; 64];
    let mut depth = 0;
    let mut e = ElemIx(i as u32);
    while e != ElemIx::NONE && e.0 != 0 && depth < chain.len() {
        if let Some(c) = chain.get_mut(depth) {
            *c = e.0;
        }
        depth += 1;
        e = doc
            .elem_parent
            .get(e.0 as usize)
            .copied()
            .unwrap_or(ElemIx::NONE);
    }
    let mut len = 0usize;
    let mut put = |bytes: &[u8], sep: Option<u8>, len: &mut usize| -> bool {
        let need = bytes.len() + usize::from(sep.is_some());
        if *len + need + 1 > MAX_PATH_TEXT {
            return false;
        }
        if let Some(s) = sep {
            if let Some(d) = out.get_mut(*len) {
                *d = s;
            }
            *len += 1;
        }
        for &b in bytes {
            if let Some(d) = out.get_mut(*len) {
                *d = b;
            }
            *len += 1;
        }
        true
    };
    let mut first = true;
    for k in (0..depth).rev() {
        let ix = chain.get(k).copied().unwrap_or(0) as usize;
        let name = name_bytes(
            doc.input,
            doc.elem_name.get(ix).copied().unwrap_or_default(),
        );
        if !put(name, if first { None } else { Some(b'/') }, &mut len) {
            if let Some(d) = out.get_mut(len) {
                *d = b'~';
                len += 1;
            }
            return len;
        }
        first = false;
    }
    if let Some(a) = attr {
        let name = name_bytes(doc.input, doc.attr_name.get(a).copied().unwrap_or_default());
        if !put(name, Some(b'@'), &mut len) {
            if let Some(d) = out.get_mut(len) {
                *d = b'~';
                len += 1;
            }
        }
    }
    len
}
