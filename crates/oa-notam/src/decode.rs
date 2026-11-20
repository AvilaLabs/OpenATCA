//! The decoder: a parsed AIXM 5.1 basic message with the FAA event extension in, one NOTAM
//! event row out. Core (pure): a function of the parsed document, no I/O, no clock, no
//! allocation (the output columns and the decoder's tables were reserved at construction).
//!
//! * One row per document: the survey (`docs/FNS_SURVEY.md`) shows exactly one `event:Event`
//!   in every message, with its features (airport, runway, navaid, ...) as sibling members. A
//!   document with none or several is refused whole.
//! * All or nothing per document: a refusal, a capacity overrun or a wrong root leaves the
//!   caller's [`Notams`] exactly as it was (INTERFACES IF-R1, IF-R5). Capacities are stated:
//!   rows, text bytes, polygons, vertices.
//! * Strict fields. A field that is present but malformed (bad syntax, out of range, unknown
//!   unit, given twice, or disagreeing with a second source of the same value) is a counted
//!   field error: the row is kept, the field is absent and its bit is set in the row's error
//!   word. Nothing is defaulted. A bad coordinate in a polygon fails the `Polygons` field of the
//!   row and keeps none of the message's polygons; it is never silently dropped.
//! * Unknown elements and attributes (outside the structure seen in real data) are counted per
//!   path in the caller's [`Stats`], never errors; only the topmost unknown element of a
//!   subtree is counted.
//! * Bounded work: one walk over the elements, no recursion, scratch tables sized at
//!   construction, values decoded into small stack buffers (or straight into the string table).

use crate::fields::*;
use crate::known::KNOWN;
use crate::rows::{field_bit, mark, push_row, truncate_notams, Mark, Notams, Row, BLANK_ROW};
use crate::scalar::{
    parse_code, parse_compact_ms, parse_coord, parse_eff_end, parse_fl3, parse_iso_ms,
    parse_lat_lon, parse_limit, parse_uint, parse_uuid, trim, EffEnd,
};
use crate::stats::{Stats, UnknownPath, MAX_PATH_TEXT};
use oa_sealed::DropWindow;
use oa_xml::{
    attr_needs_decode, decode_into, ns_uri_bytes, resolve_attr_ns, resolve_elem_ns, span_bytes,
    text_needs_decode, AttrIx, Doc, ElemIx, Ns, Parser, Span, SpanKind, TextIx,
};

/// Hard bounds on decoding. The defaults are far above the measured feed (2026-11-14 sample: 112
/// messages of up to 35.9 KB and 183 elements, texts up to 341 bytes, a polygon of 588 vertices).
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Largest document, in elements, the decoder accepts (scratch tables are this long).
    pub max_elements: usize,
    /// Longest NOTAM text or schedule kept; a longer one is a field error, never truncated.
    pub max_text_len: usize,
    /// Most vertices (closing vertex included) in one polygon; more is a field error.
    pub max_poly_vertices: usize,
    /// A NOTAM issued (`issued`) or cancelled (`canceled`) in this window is dropped whole before
    /// any count is updated: no row, and no `docs`, `accepted`, `rows`, field, feature or unknown
    /// count; it is counted in [`Stats::sealed_window_dropped`]. A document refused whole is
    /// counted as refused (it has no known time). Its validity span is not a message time and is
    /// not checked. Required: there is no `Default`, so a caller cannot forget it.
    pub drop_window: DropWindow,
}

impl Limits {
    /// The default bounds, dropping NOTAMs timed in `window`. A tool passes
    /// [`DropWindow::sealed`]; tests and fuzz targets pass [`DropWindow::empty_for_tests_and_fuzz`].
    pub const fn new(window: DropWindow) -> Limits {
        Limits {
            max_elements: 50_000,
            max_text_len: 8192,
            max_poly_vertices: 4096,
            drop_window: window,
        }
    }
}

/// What one accepted document turned out to be.
// Budget: returned once per document, never stored in quantity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// Index of the row appended to the caller's [`Notams`].
    pub row: u32,
    /// Fields of that row that were refused.
    pub field_errors: u32,
    pub unknown_elements: u32,
    pub unknown_attributes: u32,
    /// The NOTAM was dropped for its time in [`Limits::drop_window`]: no row was appended (`row`
    /// is then 0 and names nothing) and no count but `Stats::sealed_window_dropped` has it.
    pub sealed_dropped: bool,
}

/// The namespaces of the surveyed feed, and the short alias a known path uses for each. The
/// alias is not the prefix the document happens to use.
const NAMESPACES: [(&[u8], &[u8]); 9] = [
    (b"http://www.aixm.aero/schema/5.1/message", b"message"),
    (b"http://www.opengis.net/gml/3.2", b"gml"),
    (b"http://www.aixm.aero/schema/5.1", b"aixm"),
    (b"http://www.aixm.aero/schema/5.1/event", b"event"),
    (
        b"http://www.aixm.aero/schema/5.1/extensions/FAA/FNSE",
        b"fnse",
    ),
    (b"urn:us.gov.dot.faa.aim.fns", b"fns"),
    (b"http://www.w3.org/1999/xhtml", b"html"),
    (b"http://www.w3.org/1999/xlink", b"xlink"),
    (b"http://www.w3.org/2001/XMLSchema-instance", b"xsi"),
];

// Path hashes. A path is the chain of `alias:local` names from the root, folded with 64-bit
// FNV-1a; the same function runs at compile time (the paths decoded below, the known table)
// and at run time (every element). A collision between two real paths is a 2^-64 event per
// pair.
const SEED: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x0000_0100_0000_01b3;

const fn mix(h: u64, b: u8) -> u64 {
    (h ^ b as u64).wrapping_mul(PRIME)
}

const fn extend(h: u64, name: &[u8]) -> u64 {
    let mut h = mix(h, b'/');
    let mut rest = name;
    while let [b, tail @ ..] = rest {
        h = mix(h, *b);
        rest = tail;
    }
    h
}

const ROOT: &str = "message:AIXMBasicMessage";

const fn path(parts: &[&str]) -> u64 {
    let mut h = extend(SEED, ROOT.as_bytes());
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

const P_ROOT: u64 = path(&[]);
const P_MEMBER: u64 = path(&["message:hasMember"]);

const P_EVENT: u64 = path(&["message:hasMember", "event:Event"]);
const P_EVENT_ID: u64 = path(&["message:hasMember", "event:Event", "gml:identifier"]);

/// `event:Event/event:timeSlice/event:EventTimeSlice` and what hangs off it.
const TS: [&str; 4] = [
    "message:hasMember",
    "event:Event",
    "event:timeSlice",
    "event:EventTimeSlice",
];

const fn ts(tail: &[&str]) -> u64 {
    let mut h = path(&TS);
    let mut rest = tail;
    while let [p, t @ ..] = rest {
        h = extend(h, p.as_bytes());
        rest = t;
    }
    h
}

const P_BEGIN: u64 = ts(&["gml:validTime", "gml:TimePeriod", "gml:beginPosition"]);
const P_END: u64 = ts(&["gml:validTime", "gml:TimePeriod", "gml:endPosition"]);
const P_SCENARIO: u64 = ts(&["event:scenario"]);
const P_NOTAM: u64 = ts(&["event:textNOTAM", "event:NOTAM"]);
const fn nt(name: &str) -> u64 {
    ts(&["event:textNOTAM", "event:NOTAM", name])
}
const P_NUMBER: u64 = nt("event:number");
const P_YEAR: u64 = nt("event:year");
const P_TYPE: u64 = nt("event:type");
const P_SERIES: u64 = nt("event:series");
const P_ISSUED: u64 = nt("event:issued");
const P_FIR: u64 = nt("event:affectedFIR");
const P_SELECTION: u64 = nt("event:selectionCode");
const P_MIN_FL: u64 = nt("event:minimumFL");
const P_MAX_FL: u64 = nt("event:maximumFL");
const P_COORDS: u64 = nt("event:coordinates");
const P_RADIUS: u64 = nt("event:radius");
const P_LOCATION: u64 = nt("event:location");
const P_EFF_START: u64 = nt("event:effectiveStart");
const P_EFF_END: u64 = nt("event:effectiveEnd");
const P_TEXT: u64 = nt("event:text");
const P_SCHEDULE: u64 = nt("event:schedule");
const P_TRAFFIC: u64 = nt("event:traffic");
const P_PURPOSE: u64 = nt("event:purpose");
const P_SCOPE: u64 = nt("event:scope");
const P_LOWER: u64 = nt("event:lowerLimit");
const P_UPPER: u64 = nt("event:upperLimit");
const P_REF_SERIES: u64 = nt("event:referredSeries");
const P_REF_NUMBER: u64 = nt("event:referredNumber");
const P_REF_YEAR: u64 = nt("event:referredYear");

const fn xt(name: &str) -> u64 {
    ts(&["event:extension", "fnse:EventExtension", name])
}
const P_CLASS: u64 = xt("fnse:classification");
const P_CANCELED: u64 = xt("fnse:canceled");
const P_ICAO_LOC: u64 = xt("fnse:icaoLocation");
const P_XOVER: u64 = xt("fnse:xovernotamID");

/// The slice of a feature that carries its data: `member/aixm:K/aixm:timeSlice/aixm:KTimeSlice`.
const fn slice(feature: &str, slice_name: &str, tail: &[&str]) -> u64 {
    let mut h = extend(path(&["message:hasMember"]), feature.as_bytes());
    h = extend(h, b"aixm:timeSlice");
    h = extend(h, slice_name.as_bytes());
    let mut rest = tail;
    while let [p, t @ ..] = rest {
        h = extend(h, p.as_bytes());
        rest = t;
    }
    h
}

const P_APT_DESIG: u64 = slice(
    "aixm:AirportHeliport",
    "aixm:AirportHeliportTimeSlice",
    &["aixm:designator"],
);
const P_APT_ICAO: u64 = slice(
    "aixm:AirportHeliport",
    "aixm:AirportHeliportTimeSlice",
    &["aixm:locationIndicatorICAO"],
);
const P_RWY_DESIG: u64 = slice("aixm:Runway", "aixm:RunwayTimeSlice", &["aixm:designator"]);
const P_RWD_DESIG: u64 = slice(
    "aixm:RunwayDirection",
    "aixm:RunwayDirectionTimeSlice",
    &["aixm:designator"],
);
const P_UNIT_DESIG: u64 = slice("aixm:Unit", "aixm:UnitTimeSlice", &["aixm:designator"]);
const P_NAVAID_TYPE: u64 = slice("aixm:Navaid", "aixm:NavaidTimeSlice", &["aixm:type"]);

const POLY_TAIL: [&str; 7] = [
    "aixm:extent",
    "aixm:ElevatedSurface",
    "gml:patches",
    "gml:PolygonPatch",
    "gml:exterior",
    "gml:LinearRing",
    "gml:posList",
];
const P_RWE_POSLIST: u64 = slice(
    "aixm:RunwayElement",
    "aixm:RunwayElementTimeSlice",
    &POLY_TAIL,
);
const P_TWE_POSLIST: u64 = slice(
    "aixm:TaxiwayElement",
    "aixm:TaxiwayElementTimeSlice",
    &POLY_TAIL,
);

/// Feature members by element path, and their kind code.
const FEATURES: [(u64, u8); FEAT_COUNT] = [
    (
        path(&["message:hasMember", "aixm:AirportHeliport"]),
        FEAT_AIRPORT,
    ),
    (path(&["message:hasMember", "aixm:Runway"]), FEAT_RUNWAY),
    (
        path(&["message:hasMember", "aixm:RunwayDirection"]),
        FEAT_RUNWAY_DIRECTION,
    ),
    (
        path(&["message:hasMember", "aixm:RunwayElement"]),
        FEAT_RUNWAY_ELEMENT,
    ),
    (path(&["message:hasMember", "aixm:Taxiway"]), FEAT_TAXIWAY),
    (
        path(&["message:hasMember", "aixm:TaxiwayElement"]),
        FEAT_TAXIWAY_ELEMENT,
    ),
    (path(&["message:hasMember", "aixm:Navaid"]), FEAT_NAVAID),
    (path(&["message:hasMember", "aixm:Unit"]), FEAT_UNIT),
    (
        path(&["message:hasMember", "aixm:ObstacleArea"]),
        FEAT_OBSTACLE_AREA,
    ),
    (
        path(&["message:hasMember", "aixm:VerticalStructure"]),
        FEAT_VERTICAL_STRUCTURE,
    ),
    (
        path(&["message:hasMember", "aixm:RadarSystem"]),
        FEAT_RADAR_SYSTEM,
    ),
    (
        path(&["message:hasMember", "aixm:PrimarySurveillanceRadar"]),
        FEAT_PRIMARY_RADAR,
    ),
];

/// The feature kinds in order of specificity: the first one present is the row's `feat_kind`.
/// A navaid, a runway direction or a runway says more about what a NOTAM affects than the
/// airport it is at, which every airport-related message carries.
const PRIORITY: [u8; FEAT_COUNT] = [
    FEAT_NAVAID,
    FEAT_RUNWAY_DIRECTION,
    FEAT_RUNWAY,
    FEAT_RUNWAY_ELEMENT,
    FEAT_TAXIWAY_ELEMENT,
    FEAT_TAXIWAY,
    FEAT_OBSTACLE_AREA,
    FEAT_VERTICAL_STRUCTURE,
    FEAT_PRIMARY_RADAR,
    FEAT_RADAR_SYSTEM,
    FEAT_UNIT,
    FEAT_AIRPORT,
];

/// Element states while walking.
const S_KNOWN: u8 = 0;
const S_UNKNOWN: u8 = 1;
const S_BELOW: u8 = 2;

const NO_TEXT: u32 = u32::MAX;
const MANY_TEXT: u32 = u32::MAX - 1;

/// Source bits for a value given by two elements (`Msg::src`).
const SRC_ISO_START: u8 = 1;
const SRC_EFF_START: u8 = 2;
const SRC_ISO_END: u8 = 4;
const SRC_EFF_END: u8 = 8;

/// The decoder: scratch tables allocated once in [`new_decoder`].
// Subsystem (RULESET A1.1, D5): precomputed tables, no behaviour of its own.
#[derive(Clone, Debug)]
pub struct Decoder {
    pub limits: Limits,
    known_paths: Vec<u64>,
    known_attrs: Vec<u64>,
    hash: Vec<u64>,
    state: Vec<u8>,
    feat: Vec<u8>,
    elem_text: Vec<u32>,
}

/// Builds a decoder: every table is allocated here and never again.
pub fn new_decoder(limits: Limits) -> Decoder {
    let mut known_paths = Vec::with_capacity(KNOWN.len());
    let mut known_attrs = Vec::new();
    for (p, attrs) in KNOWN {
        let mut h = extend(SEED, ROOT.as_bytes());
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
        feat: vec![0; n],
        elem_text: vec![NO_TEXT; n],
    }
}

/// A value read from an attribute or text: missing, present but undecodable, or its bytes.
#[derive(Clone, Copy)]
enum Raw<'x> {
    Absent,
    Bad,
    Val(&'x [u8]),
}

/// The message being decoded.
struct Msg {
    row: Row,
    start: Mark,
    events: u32,
    notams: u32,
    ids: u32,
    id_bad: bool,
    src: u8,
    /// Which of the three referred-NOTAM parts were seen (bits 1, 2, 4), and whether any
    /// failed or repeated.
    ref_seen: u8,
    ref_bad: bool,
    lo: Option<(i32, u8)>,
    hi: Option<(i32, u8)>,
    min_fl: Option<(i32, u8)>,
    max_fl: Option<(i32, u8)>,
    lo_seen: u8,
    poly_failed: bool,
    /// Designator of the first feature of each kind, indexed by kind code.
    desig: [[u8; 12]; FEAT_COUNT + 1],
    desig_set: u32,
}

fn new_msg(start: Mark) -> Msg {
    Msg {
        row: BLANK_ROW,
        start,
        events: 0,
        notams: 0,
        ids: 0,
        id_bad: false,
        src: 0,
        ref_seen: 0,
        ref_bad: false,
        lo: None,
        hi: None,
        min_fl: None,
        max_fl: None,
        lo_seen: 0,
        poly_failed: false,
        desig: [[0; 12]; FEAT_COUNT + 1],
        desig_set: 0,
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

fn errored(m: &Msg, f: Field) -> bool {
    m.row.errors & field_bit(f) != 0
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

/// `store` over a raw read: an undecodable value is an error, an absent one cannot happen for
/// an element that exists (an empty element is `Bad`).
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

fn name_bytes(input: &[u8], sp: Span) -> &[u8] {
    span_bytes(input, sp).unwrap_or(&[])
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

fn attr_range(doc: &Doc<'_>, i: usize) -> core::ops::Range<usize> {
    let first = doc.elem_first_attr.get(i).map_or(0, |a| a.0 as usize);
    let count = doc.elem_attr_count.get(i).map_or(0, |&c| c as usize);
    first..first.saturating_add(count)
}

/// The unprefixed attribute `name` of element `i`.
fn attr<'x>(doc: &Doc<'x>, i: usize, name: &[u8], buf: &'x mut [u8]) -> Raw<'x> {
    for a in attr_range(doc, i) {
        let unprefixed = doc.attr_prefix.get(a).is_some_and(|p| p.len == 0);
        let hit = doc
            .attr_name
            .get(a)
            .is_some_and(|&n| name_bytes(doc.input, n) == name);
        if unprefixed && hit {
            let Some(&sp) = doc.attr_value.get(a) else {
                return Raw::Bad;
            };
            let ent = attr_needs_decode(doc, AttrIx(a as u32));
            return raw_bytes(doc.input, sp, ent, SpanKind::AttrValue, buf);
        }
    }
    Raw::Absent
}

/// The text of element `i`. An element with no text, or with several text nodes, is `Bad`: a
/// value element is never empty here.
fn text<'x>(doc: &Doc<'x>, texts: &[u32], i: usize, buf: &'x mut [u8]) -> Raw<'x> {
    let t = texts.get(i).copied().unwrap_or(NO_TEXT);
    if t == NO_TEXT || t == MANY_TEXT {
        return Raw::Bad;
    }
    let Some(&sp) = doc.text_span.get(t as usize) else {
        return Raw::Bad;
    };
    let ent = text_needs_decode(doc, TextIx(t));
    raw_bytes(doc.input, sp, ent, SpanKind::Text, buf)
}

/// The trimmed text of element `i`.
fn short<'x>(doc: &Doc<'x>, texts: &[u32], i: usize, buf: &'x mut [u8]) -> Raw<'x> {
    match text(doc, texts, i, buf) {
        Raw::Val(b) => Raw::Val(trim(b)),
        other => other,
    }
}

// ---- field parsers -------------------------------------------------------------------------

fn parse_number(b: &[u8]) -> Option<u32> {
    u32::try_from(parse_uint(b, 6)?).ok()
}

fn parse_year(b: &[u8]) -> Option<u16> {
    if b.len() != 4 {
        return None;
    }
    let y = parse_uint(b, 4)?;
    if y < 1970 {
        return None;
    }
    u16::try_from(y).ok()
}

fn parse_type(b: &[u8]) -> Option<u8> {
    match b {
        b"N" => Some(TYPE_NEW),
        b"R" => Some(TYPE_REPLACE),
        b"C" => Some(TYPE_CANCEL),
        _ => None,
    }
}

fn parse_series(b: &[u8]) -> Option<[u8; 12]> {
    parse_code::<12>(b, b":_-")
}

fn parse_selection(b: &[u8]) -> Option<[u8; 5]> {
    if b.len() == 5 && b.iter().all(u8::is_ascii_uppercase) {
        parse_code::<5>(b, b"")
    } else {
        None
    }
}

fn parse_class(b: &[u8]) -> Option<u8> {
    match b {
        b"INTL" => Some(CLASS_INTL),
        b"DOM" => Some(CLASS_DOM),
        b"MIL" => Some(CLASS_MIL),
        _ => None,
    }
}

/// A set of distinct letters from `allowed` (letter, bit), at least one.
fn parse_letters(b: &[u8], allowed: &[(u8, u8)]) -> Option<u8> {
    if b.is_empty() {
        return None;
    }
    let mut set = 0u8;
    for &c in b {
        let bit = allowed.iter().find(|(l, _)| *l == c)?.1;
        if set & bit != 0 {
            return None;
        }
        set |= bit;
    }
    Some(set)
}

fn parse_traffic(b: &[u8]) -> Option<u8> {
    parse_letters(
        b,
        &[(b'I', TRAFFIC_I), (b'V', TRAFFIC_V), (b'K', TRAFFIC_K)],
    )
}

fn parse_purpose(b: &[u8]) -> Option<u8> {
    parse_letters(
        b,
        &[
            (b'N', PURPOSE_N),
            (b'B', PURPOSE_B),
            (b'O', PURPOSE_O),
            (b'M', PURPOSE_M),
            (b'K', PURPOSE_K),
        ],
    )
}

fn parse_scope(b: &[u8]) -> Option<u8> {
    parse_letters(
        b,
        &[
            (b'A', SCOPE_A),
            (b'E', SCOPE_E),
            (b'W', SCOPE_W),
            (b'K', SCOPE_K),
        ],
    )
}

fn parse_scenario(b: &[u8]) -> Option<u16> {
    u16::try_from(parse_uint(b, 5)?).ok()
}

/// A radius in nautical miles, one to three digits, 1 to 999, as metres.
fn parse_radius(b: &[u8]) -> Option<u32> {
    let nm = u32::try_from(parse_uint(b, 3)?).ok()?;
    if (1..=999).contains(&nm) {
        nm.checked_mul(1852)
    } else {
        None
    }
}

/// An ICAO or FAA location code: one to four letters or digits.
fn parse_loc4(b: &[u8]) -> Option<[u8; 4]> {
    parse_code::<4>(b, b"")
}

/// A designator (airport, runway, unit, navaid type): letters, digits and `/`, `-`, `_`.
fn parse_desig<const N: usize>(b: &[u8]) -> Option<[u8; N]> {
    parse_code::<N>(b, b"/-_")
}

// ---- walking -------------------------------------------------------------------------------

/// The alias of the namespace of element `i`, as the hash sees it.
enum Alias<'a> {
    None,
    Known(&'static [u8]),
    Other(&'a [u8]),
}

fn alias_of<'a>(doc: &Doc<'a>, ns: Ns) -> Alias<'a> {
    match ns {
        Ns::None => Alias::None,
        Ns::Xml => Alias::Known(b"xml"),
        Ns::Uri(u) => {
            let uri = ns_uri_bytes(doc, u).unwrap_or(&[]);
            match NAMESPACES.iter().find(|(k, _)| *k == uri) {
                Some((_, a)) => Alias::Known(a),
                None => Alias::Other(uri),
            }
        }
    }
}

/// Extends `h` by one `alias:local` segment (or `local` with no namespace); the result equals
/// [`extend`] of the same text for a known alias, and matches no known path for another URI.
fn seg_hash(h: u64, alias: &Alias<'_>, local: &[u8]) -> u64 {
    let mut h = mix(h, b'/');
    match alias {
        Alias::None => {}
        Alias::Known(a) => {
            for &b in *a {
                h = mix(h, b);
            }
            h = mix(h, b':');
        }
        Alias::Other(u) => {
            h = mix(h, b'{');
            for &b in *u {
                h = mix(h, b);
            }
            h = mix(h, b'}');
            h = mix(h, b':');
        }
    }
    for &b in local {
        h = mix(h, b);
    }
    h
}

fn is_xmlns(doc: &Doc<'_>, a: usize) -> bool {
    let prefix = doc.attr_prefix.get(a).copied().unwrap_or_default();
    let name = doc.attr_name.get(a).copied().unwrap_or_default();
    (prefix.len == 0 && name_bytes(doc.input, name) == b"xmlns")
        || name_bytes(doc.input, prefix) == b"xmlns"
}

fn feature_kind(h: u64) -> u8 {
    FEATURES
        .iter()
        .find(|(p, _)| *p == h)
        .map_or(0, |&(_, k)| k)
}

/// Decodes `input` with `parser` and `dec`. A parse error is [`DocError::Xml`], counted in
/// `stats` like every other refusal.
pub fn decode_bytes(
    parser: &mut Parser,
    dec: &mut Decoder,
    input: &[u8],
    out: &mut Notams,
    stats: &mut Stats,
) -> Result<Outcome, DocError> {
    match parser.parse(input) {
        Ok(doc) => decode_doc(dec, &doc, out, stats),
        Err(e) => {
            let err = DocError::Xml(e);
            stats.docs = stats.docs.saturating_add(1);
            count_reject(stats, &err);
            Err(err)
        }
    }
}

fn count_reject(stats: &mut Stats, e: &DocError) {
    if let Some(c) = stats.rejects.get_mut(reject_index(e)) {
        *c = c.saturating_add(1);
    }
}

/// Decodes one parsed document into `out`: appends one row, or on `Err` leaves `out` exactly
/// as it was. Refusals, field errors and unknown paths are counted in `stats`.
pub fn decode_doc(
    dec: &mut Decoder,
    doc: &Doc<'_>,
    out: &mut Notams,
    stats: &mut Stats,
) -> Result<Outcome, DocError> {
    stats.docs = stats.docs.saturating_add(1);
    let start = mark(out);
    // What the walk may change before the time of the NOTAM is known, to put back if it is
    // dropped for its time.
    let snap = (
        stats.unknown.clone(),
        stats.unknown_elements,
        stats.unknown_attributes,
        stats.unknown_overflow,
    );
    let r = decode_inner(dec, doc, out, stats, start);
    if let Ok(o) = &r {
        let i = o.row as usize;
        let w = dec.limits.drop_window;
        let timed = |v: &[i64]| v.get(i).is_some_and(|&t| w.contains(t));
        if timed(&out.issued_ms) || timed(&out.canceled_ms) {
            truncate_notams(out, start);
            stats.docs = stats.docs.saturating_sub(1);
            (
                stats.unknown,
                stats.unknown_elements,
                stats.unknown_attributes,
                stats.unknown_overflow,
            ) = snap;
            stats.sealed_window_dropped = stats.sealed_window_dropped.saturating_add(1);
            return Ok(Outcome {
                row: 0,
                field_errors: 0,
                unknown_elements: 0,
                unknown_attributes: 0,
                sealed_dropped: true,
            });
        }
    }
    match &r {
        Ok(o) => {
            stats.accepted = stats.accepted.saturating_add(1);
            stats.rows = stats.rows.saturating_add(1);
            let i = o.row as usize;
            let errs = out.errors.get(i).copied().unwrap_or(0);
            for (f, c) in stats.field_errors.iter_mut().enumerate() {
                if errs & (1u64 << f) != 0 {
                    *c = c.saturating_add(1);
                }
            }
            let mask = out.feat_mask.get(i).copied().unwrap_or(0);
            for (k, c) in stats.features.iter_mut().enumerate() {
                if mask & (1u32 << (k + 1)) != 0 {
                    *c = c.saturating_add(1);
                }
            }
            if crate::rows::has_circle(out, i) {
                stats.circles = stats.circles.saturating_add(1);
            }
            let polys = crate::rows::polygons_of(out, i);
            if !polys.is_empty() {
                stats.with_polygons = stats.with_polygons.saturating_add(1);
            }
            stats.polygons = stats.polygons.saturating_add(polys.len() as u64);
            let verts: u64 = polys
                .map(|p| u64::from(out.poly_vlen.get(p).copied().unwrap_or(0)))
                .sum();
            stats.vertices = stats.vertices.saturating_add(verts);
        }
        Err(e) => {
            truncate_notams(out, start);
            count_reject(stats, e);
        }
    }
    r
}

fn decode_inner(
    dec: &mut Decoder,
    doc: &Doc<'_>,
    out: &mut Notams,
    stats: &mut Stats,
    start: Mark,
) -> Result<Outcome, DocError> {
    let n = doc.elem_name.len();
    if n > dec.limits.max_elements {
        return Err(DocError::TooManyElements {
            elements: n,
            limit: dec.limits.max_elements,
        });
    }
    let root = ElemIx(0);
    let root_name = doc.elem_name.first().copied().unwrap_or_default();
    let root_ns = alias_of(doc, resolve_elem_ns(doc, root));
    let root_hash = seg_hash(SEED, &root_ns, name_bytes(doc.input, root_name));
    if n == 0 || root_hash != P_ROOT {
        return Err(DocError::Rejected(Reject::NotAixm));
    }
    if out.len >= out.capacity {
        return Err(DocError::Capacity(CapacityKind::Rows));
    }
    // First text node of each element, for value elements.
    dec.elem_text.iter_mut().take(n).for_each(|t| *t = NO_TEXT);
    for (t, owner) in doc.text_owner.iter().enumerate() {
        if let Some(slot) = dec.elem_text.get_mut(owner.0 as usize) {
            *slot = if *slot == NO_TEXT {
                t as u32
            } else {
                MANY_TEXT
            };
        }
    }
    let mut m = new_msg(start);
    let mut unknown_el = 0u32;
    let mut unknown_at = 0u32;
    if let Some(h) = dec.hash.first_mut() {
        *h = root_hash;
    }
    if let Some(s) = dec.state.first_mut() {
        *s = S_KNOWN;
    }
    if let Some(f) = dec.feat.first_mut() {
        *f = 0;
    }
    check_attrs(dec, doc, 0, root_hash, stats, &mut unknown_at);
    for i in 1..n {
        let p = doc.elem_parent.get(i).copied().unwrap_or(ElemIx::NONE);
        let pi = p.0 as usize;
        let pstate = dec.state.get(pi).copied().unwrap_or(S_BELOW);
        if pstate != S_KNOWN {
            if let Some(s) = dec.state.get_mut(i) {
                *s = S_BELOW;
            }
            continue;
        }
        let name = doc.elem_name.get(i).copied().unwrap_or_default();
        let alias = alias_of(doc, resolve_elem_ns(doc, ElemIx(i as u32)));
        let ph = dec.hash.get(pi).copied().unwrap_or(0);
        let h = seg_hash(ph, &alias, name_bytes(doc.input, name));
        if let Some(x) = dec.hash.get_mut(i) {
            *x = h;
        }
        if dec.known_paths.binary_search(&h).is_err() {
            if let Some(s) = dec.state.get_mut(i) {
                *s = S_UNKNOWN;
            }
            note_unknown(stats, doc, i, None, h);
            unknown_el = unknown_el.saturating_add(1);
            continue;
        }
        if let Some(s) = dec.state.get_mut(i) {
            *s = S_KNOWN;
        }
        check_attrs(dec, doc, i, h, stats, &mut unknown_at);
        let mut kind = dec.feat.get(pi).copied().unwrap_or(0);
        if ph == P_MEMBER {
            kind = feature_kind(h);
            if kind != 0 {
                m.row.feat_mask |= 1u32 << kind;
            }
        }
        if let Some(f) = dec.feat.get_mut(i) {
            *f = kind;
        }
        visit(dec, doc, i, h, kind, &mut m, out)?;
    }
    let row = finish(m, out)?;
    Ok(Outcome {
        row,
        field_errors: out.errors.get(row as usize).map_or(0, |w| w.count_ones()),
        unknown_elements: unknown_el,
        unknown_attributes: unknown_at,
        sealed_dropped: false,
    })
}

/// Counts every attribute of known element `i` that is not in the known table.
fn check_attrs(
    dec: &Decoder,
    doc: &Doc<'_>,
    i: usize,
    h: u64,
    stats: &mut Stats,
    unknown_at: &mut u32,
) {
    for a in attr_range(doc, i) {
        if is_xmlns(doc, a) {
            continue;
        }
        let name = doc.attr_name.get(a).copied().unwrap_or_default();
        let local = name_bytes(doc.input, name);
        let prefixed = doc.attr_prefix.get(a).is_some_and(|p| p.len > 0);
        let ns = if prefixed {
            resolve_attr_ns(doc, ElemIx(i as u32), AttrIx(a as u32))
        } else {
            Ns::None
        };
        let ah = seg_hash(extend(h, b"@"), &alias_of(doc, ns), local);
        if dec.known_attrs.binary_search(&ah).is_err() {
            note_unknown(stats, doc, i, Some(a), ah);
            *unknown_at = unknown_at.saturating_add(1);
        }
    }
}

/// Counts an unknown element (`attr` is `None`) or attribute.
fn note_unknown(stats: &mut Stats, doc: &Doc<'_>, i: usize, attr: Option<usize>, h: u64) {
    if attr.is_some() {
        stats.unknown_attributes = stats.unknown_attributes.saturating_add(1);
    } else {
        stats.unknown_elements = stats.unknown_elements.saturating_add(1);
    }
    if let Some(u) = stats.unknown.iter_mut().find(|u| u.hash == h) {
        u.count = u.count.saturating_add(1);
        return;
    }
    if stats.unknown.len() >= stats.max_unknown_paths {
        stats.unknown_overflow = stats.unknown_overflow.saturating_add(1);
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
    stats.unknown.push(u);
}

/// Writes the path of element `i` (and attribute `attr`) as `prefix:local` names joined by `/`
/// from the root's child down. Returns the length; a path that does not fit ends in `~` after
/// the last whole name.
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
    let put = |bytes: &[u8], sep: Option<u8>, len: &mut usize, out: &mut [u8; MAX_PATH_TEXT]| {
        let need = bytes.len() + usize::from(sep.is_some());
        if *len + need + 1 > MAX_PATH_TEXT {
            return false;
        }
        for &b in sep.iter().chain(bytes) {
            if let Some(d) = out.get_mut(*len) {
                *d = b;
            }
            *len += 1;
        }
        true
    };
    let qname = |name: Span, prefix: Span| -> ([u8; 96], usize) {
        let mut q = [0u8; 96];
        let mut n = 0;
        let p = name_bytes(doc.input, prefix);
        let l = name_bytes(doc.input, name);
        let parts: [&[u8]; 3] = [p, if p.is_empty() { b"" } else { b":" }, l];
        for part in parts {
            for &b in part {
                if let Some(d) = q.get_mut(n) {
                    *d = b;
                    n += 1;
                }
            }
        }
        (q, n)
    };
    let mut first = true;
    for k in (0..depth).rev() {
        let ix = chain.get(k).copied().unwrap_or(0) as usize;
        let (q, qn) = qname(
            doc.elem_name.get(ix).copied().unwrap_or_default(),
            doc.elem_prefix.get(ix).copied().unwrap_or_default(),
        );
        let sep = if first { None } else { Some(b'/') };
        if !put(q.get(..qn).unwrap_or(&[]), sep, &mut len, out) {
            if let Some(d) = out.get_mut(len) {
                *d = b'~';
                len += 1;
            }
            return len;
        }
        first = false;
    }
    if let Some(a) = attr {
        let (q, qn) = qname(
            doc.attr_name.get(a).copied().unwrap_or_default(),
            doc.attr_prefix.get(a).copied().unwrap_or_default(),
        );
        if !put(q.get(..qn).unwrap_or(&[]), Some(b'@'), &mut len, out) {
            if let Some(d) = out.get_mut(len) {
                *d = b'~';
                len += 1;
            }
        }
    }
    len
}

/// The ElevatedSurface ancestor's `srsName` and `srsDimension`: EPSG:4326 in the URN form,
/// latitude then longitude, two dimensions.
fn surface_ok(doc: &Doc<'_>, i: usize) -> bool {
    let mut e = doc.elem_parent.get(i).copied().unwrap_or(ElemIx::NONE);
    for _ in 0..8 {
        if e == ElemIx::NONE {
            return false;
        }
        let ei = e.0 as usize;
        let name = doc.elem_name.get(ei).copied().unwrap_or_default();
        if name_bytes(doc.input, name) == b"ElevatedSurface" {
            let mut b1 = [0u8; 40];
            let mut b2 = [0u8; 8];
            let srs = matches!(
                attr(doc, ei, b"srsName", &mut b1),
                Raw::Val(b"urn:ogc:def:crs:EPSG::4326")
            );
            let dim = matches!(attr(doc, ei, b"srsDimension", &mut b2), Raw::Val(b"2"));
            return srs && dim;
        }
        e = doc.elem_parent.get(ei).copied().unwrap_or(ElemIx::NONE);
    }
    false
}

/// A single-valued designator of a feature: the first of its kind wins, a later one is
/// ignored, a malformed one is an error of `f`.
fn feature_desig<const N: usize>(
    m: &mut Msg,
    kind: u8,
    f: Field,
    raw: Raw<'_>,
    store_in: Option<fn(&mut Row) -> &mut [u8; N]>,
) {
    let bit = 1u32 << kind;
    if m.desig_set & bit != 0 {
        return;
    }
    let parsed = match raw {
        Raw::Val(b) => parse_desig::<N>(b),
        _ => None,
    };
    m.desig_set |= bit;
    match parsed {
        Some(v) => {
            if let Some(d) = m.desig.get_mut(usize::from(kind)) {
                for (x, y) in d.iter_mut().zip(v) {
                    *x = y;
                }
            }
            if let Some(dst) = store_in {
                if !seen(m, f) {
                    *dst(&mut m.row) = v;
                    mark_ok(m, f);
                }
            }
        }
        None => mark_err(m, f),
    }
}

/// A time given by two elements that must agree: `bit` names the source, `parsed` the value.
fn time_source(m: &mut Msg, f: Field, bit: u8, parsed: Option<i64>, dst: fn(&mut Row) -> &mut i64) {
    if m.src & bit != 0 {
        mark_err(m, f);
        return;
    }
    m.src |= bit;
    let Some(v) = parsed else {
        mark_err(m, f);
        return;
    };
    if errored(m, f) {
        return;
    }
    if m.row.present & field_bit(f) != 0 {
        if *dst(&mut m.row) != v {
            mark_err(m, f);
        }
    } else {
        *dst(&mut m.row) = v;
        mark_ok(m, f);
    }
}

/// Copies the text of element `i` into the string table (entity-decoded, white space kept).
/// Returns the offset and length, `Ok(None)` after a field error (the field is marked, no
/// table space is taken) and `Err` when the table is full.
fn table_text(
    dec: &Decoder,
    doc: &Doc<'_>,
    i: usize,
    f: Field,
    max_len: usize,
    m: &mut Msg,
    out: &mut Notams,
) -> Result<Option<(u32, u32)>, DocError> {
    if seen(m, f) {
        mark_err(m, f);
        return Ok(None);
    }
    let t = dec.elem_text.get(i).copied().unwrap_or(NO_TEXT);
    let sp = match doc.text_span.get(t as usize) {
        Some(&sp) if t != NO_TEXT && t != MANY_TEXT => sp,
        _ => {
            mark_err(m, f);
            return Ok(None);
        }
    };
    let used = out.strings_used;
    let remaining = out.strings.len().saturating_sub(used);
    let window = remaining.min(max_len);
    let entity = text_needs_decode(doc, TextIx(t));
    let len = if entity {
        let Some(dst) = out.strings.get_mut(used..used + window) else {
            return Err(DocError::Capacity(CapacityKind::Strings));
        };
        match decode_into(sp, doc.input, dst, SpanKind::Text) {
            Ok(n) => n,
            Err(oa_xml::XmlError::OutputTooSmall { .. }) => {
                if remaining < max_len {
                    return Err(DocError::Capacity(CapacityKind::Strings));
                }
                mark_err(m, f);
                return Ok(None);
            }
            Err(_) => {
                mark_err(m, f);
                return Ok(None);
            }
        }
    } else {
        let Ok(src) = span_bytes(doc.input, sp) else {
            mark_err(m, f);
            return Ok(None);
        };
        if src.len() > max_len {
            mark_err(m, f);
            return Ok(None);
        }
        let Some(dst) = out.strings.get_mut(used..used + src.len()) else {
            return Err(DocError::Capacity(CapacityKind::Strings));
        };
        dst.copy_from_slice(src);
        src.len()
    };
    if len == 0 {
        mark_err(m, f);
        return Ok(None);
    }
    out.strings_used = used + len;
    mark_ok(m, f);
    Ok(Some((used as u32, len as u32)))
}

/// Parses a `posList` into vertices appended to `out`: pairs of latitude and longitude, a
/// closed ring of at least four points. Returns `Ok(false)` after a field error, with `out`
/// restored to where this message's polygons started.
fn polygon(
    dec: &Decoder,
    doc: &Doc<'_>,
    i: usize,
    kind: u8,
    m: &mut Msg,
    out: &mut Notams,
) -> Result<(), DocError> {
    if m.poly_failed {
        return Ok(());
    }
    let ok = read_ring(dec, doc, i, kind, out)?;
    if !ok {
        m.poly_failed = true;
        mark_err(m, Field::Polygons);
        out.poly_vstart.truncate(m.start.polygons);
        out.poly_vlen.truncate(m.start.polygons);
        out.poly_kind.truncate(m.start.polygons);
        out.vtx_lat.truncate(m.start.vertices);
        out.vtx_lon.truncate(m.start.vertices);
    } else {
        mark_ok(m, Field::Polygons);
    }
    Ok(())
}

/// Appends one polygon, or nothing and `Ok(false)` when its text is not a valid ring.
fn read_ring(
    dec: &Decoder,
    doc: &Doc<'_>,
    i: usize,
    kind: u8,
    out: &mut Notams,
) -> Result<bool, DocError> {
    let t = dec.elem_text.get(i).copied().unwrap_or(NO_TEXT);
    if t == NO_TEXT || t == MANY_TEXT || !surface_ok(doc, i) {
        return Ok(false);
    }
    let Some(&sp) = doc.text_span.get(t as usize) else {
        return Ok(false);
    };
    let Ok(bytes) = span_bytes(doc.input, sp) else {
        return Ok(false);
    };
    // Line ends are white space either way; a reference could be anything, and has no place
    // among numbers.
    if text_needs_decode(doc, TextIx(t)) && bytes.contains(&b'&') {
        return Ok(false);
    }
    if out.poly_vstart.len() >= out.poly_capacity {
        return Err(DocError::Capacity(CapacityKind::Polygons));
    }
    let first = out.vtx_lat.len();
    let mut count = 0usize;
    let mut tokens = bytes
        .split(u8::is_ascii_whitespace)
        .filter(|t| !t.is_empty());
    let mut ring_first: Option<(i32, i32)> = None;
    // The last point read is held back until the next one arrives: a closing point equal to the
    // first is never stored, so the table needs room for the vertices only.
    let mut held: Option<(i32, i32)> = None;
    while let Some(la) = tokens.next() {
        let Some(lo) = tokens.next() else {
            return Ok(false);
        };
        let Some(p) = parse_lat_lon(la, lo) else {
            return Ok(false);
        };
        count += 1;
        if count > dec.limits.max_poly_vertices {
            return Ok(false);
        }
        if let Some(h) = held {
            if out.vtx_lat.len() >= out.vtx_capacity {
                return Err(DocError::Capacity(CapacityKind::Vertices));
            }
            out.vtx_lat.push(h.0);
            out.vtx_lon.push(h.1);
        }
        if ring_first.is_none() {
            ring_first = Some(p);
        }
        held = Some(p);
    }
    // closed ring of at least four points; the closing vertex is not kept
    if count < 4 || ring_first != held {
        out.vtx_lat.truncate(first);
        out.vtx_lon.truncate(first);
        return Ok(false);
    }
    out.poly_vstart.push(first as u32);
    out.poly_vlen.push((count - 1) as u32);
    out.poly_kind.push(kind);
    Ok(true)
}

/// Reads whatever element `i` (a known path) contributes to the message.
fn visit(
    dec: &Decoder,
    doc: &Doc<'_>,
    i: usize,
    h: u64,
    kind: u8,
    m: &mut Msg,
    out: &mut Notams,
) -> Result<(), DocError> {
    let mut b1 = [0u8; 64];
    let texts = &dec.elem_text;
    match h {
        P_EVENT => m.events = m.events.saturating_add(1),
        P_NOTAM => m.notams = m.notams.saturating_add(1),
        P_EVENT_ID => {
            m.ids = m.ids.saturating_add(1);
            let mut b2 = [0u8; 16];
            let code_ok = matches!(attr(doc, i, b"codeSpace", &mut b2), Raw::Val(b"urn:uuid:"));
            let parsed = match text(doc, texts, i, &mut b1) {
                Raw::Val(b) if code_ok => parse_uuid(trim(b)),
                _ => None,
            };
            match parsed {
                Some(id) if m.ids == 1 => m.row.event_id = id,
                _ => m.id_bad = true,
            }
        }
        P_BEGIN => {
            let v = match text(doc, texts, i, &mut b1) {
                Raw::Val(b) => parse_iso_ms(trim(b)),
                _ => None,
            };
            time_source(m, Field::Start, SRC_ISO_START, v, |r| &mut r.start_ms);
        }
        P_END => {
            let v = match text(doc, texts, i, &mut b1) {
                Raw::Val(b) => parse_iso_ms(trim(b)),
                _ => None,
            };
            time_source(m, Field::End, SRC_ISO_END, v, |r| &mut r.end_ms);
        }
        P_EFF_START => {
            let v = match text(doc, texts, i, &mut b1) {
                Raw::Val(b) => parse_compact_ms(trim(b)),
                _ => None,
            };
            time_source(m, Field::Start, SRC_EFF_START, v, |r| &mut r.start_ms);
        }
        P_EFF_END => {
            let eff = match text(doc, texts, i, &mut b1) {
                Raw::Val(b) => parse_eff_end(trim(b)),
                _ => None,
            };
            match eff {
                Some(EffEnd::At(v)) => {
                    time_source(m, Field::End, SRC_EFF_END, Some(v), |r| &mut r.end_ms);
                }
                Some(EffEnd::Estimated(v)) => {
                    m.row.flags |= FLAG_ESTIMATED;
                    time_source(m, Field::End, SRC_EFF_END, Some(v), |r| &mut r.end_ms);
                }
                Some(EffEnd::Permanent) => {
                    m.row.flags |= FLAG_PERMANENT;
                    if m.src & SRC_EFF_END != 0 {
                        mark_err(m, Field::End);
                    }
                    m.src |= SRC_EFF_END;
                }
                None => time_source(m, Field::End, SRC_EFF_END, None, |r| &mut r.end_ms),
            }
        }
        P_SCENARIO => field(
            m,
            Field::Scenario,
            short(doc, texts, i, &mut b1),
            parse_scenario,
            |r| &mut r.scenario,
        ),
        P_NUMBER => field(
            m,
            Field::Number,
            short(doc, texts, i, &mut b1),
            parse_number,
            |r| &mut r.number,
        ),
        P_YEAR => field(
            m,
            Field::Year,
            short(doc, texts, i, &mut b1),
            parse_year,
            |r| &mut r.year,
        ),
        P_TYPE => field(
            m,
            Field::Type,
            short(doc, texts, i, &mut b1),
            parse_type,
            |r| &mut r.ntype,
        ),
        P_SERIES => field(
            m,
            Field::Series,
            short(doc, texts, i, &mut b1),
            parse_series,
            |r| &mut r.series,
        ),
        P_ISSUED => field(
            m,
            Field::Issued,
            short(doc, texts, i, &mut b1),
            parse_iso_ms,
            |r| &mut r.issued_ms,
        ),
        P_FIR => field(
            m,
            Field::Fir,
            short(doc, texts, i, &mut b1),
            parse_loc4,
            |r| &mut r.fir,
        ),
        P_LOCATION => field(
            m,
            Field::Location,
            short(doc, texts, i, &mut b1),
            parse_loc4,
            |r| &mut r.location,
        ),
        P_ICAO_LOC => field(
            m,
            Field::IcaoLocation,
            short(doc, texts, i, &mut b1),
            parse_loc4,
            |r| &mut r.icao_location,
        ),
        P_SELECTION => field(
            m,
            Field::Selection,
            short(doc, texts, i, &mut b1),
            parse_selection,
            |r| &mut r.selection,
        ),
        P_CLASS => field(
            m,
            Field::Classification,
            short(doc, texts, i, &mut b1),
            parse_class,
            |r| &mut r.class,
        ),
        P_TRAFFIC => field(
            m,
            Field::Traffic,
            short(doc, texts, i, &mut b1),
            parse_traffic,
            |r| &mut r.traffic,
        ),
        P_PURPOSE => field(
            m,
            Field::Purpose,
            short(doc, texts, i, &mut b1),
            parse_purpose,
            |r| &mut r.purpose,
        ),
        P_SCOPE => field(
            m,
            Field::Scope,
            short(doc, texts, i, &mut b1),
            parse_scope,
            |r| &mut r.scope,
        ),
        P_CANCELED => field(
            m,
            Field::Canceled,
            short(doc, texts, i, &mut b1),
            parse_iso_ms,
            |r| &mut r.canceled_ms,
        ),
        P_XOVER => field(
            m,
            Field::Xover,
            short(doc, texts, i, &mut b1),
            |b| parse_code::<16>(b, b"/"),
            |r| &mut r.xover,
        ),
        P_COORDS => {
            let v = match short(doc, texts, i, &mut b1) {
                Raw::Val(b) => parse_coord(b),
                _ => None,
            };
            if seen(m, Field::Centre) {
                mark_err(m, Field::Centre);
            } else if let Some((la, lo)) = v {
                m.row.lat_e7 = la;
                m.row.lon_e7 = lo;
                mark_ok(m, Field::Centre);
            } else {
                mark_err(m, Field::Centre);
            }
        }
        P_RADIUS => field(
            m,
            Field::Radius,
            short(doc, texts, i, &mut b1),
            parse_radius,
            |r| &mut r.radius_m,
        ),
        P_LOWER | P_UPPER | P_MIN_FL | P_MAX_FL => {
            let upper = h == P_UPPER || h == P_MAX_FL;
            let f = if upper { Field::Ceiling } else { Field::Floor };
            let by_level = h == P_MIN_FL || h == P_MAX_FL;
            let v = match short(doc, texts, i, &mut b1) {
                Raw::Val(b) if by_level => parse_fl3(b),
                Raw::Val(b) => parse_limit(b),
                _ => None,
            };
            // Which of the four slots, and whether it was already given.
            let (slot, bit) = match h {
                P_LOWER => (&mut m.lo, 1u8),
                P_UPPER => (&mut m.hi, 2),
                P_MIN_FL => (&mut m.min_fl, 4),
                _ => (&mut m.max_fl, 8),
            };
            let dup = m.lo_seen & bit != 0;
            m.lo_seen |= bit;
            if dup || v.is_none() {
                mark_err(m, f);
            } else {
                *slot = v;
            }
        }
        P_REF_SERIES | P_REF_NUMBER | P_REF_YEAR => {
            let bit = if h == P_REF_SERIES {
                1u8
            } else if h == P_REF_NUMBER {
                2
            } else {
                4
            };
            if m.ref_seen & bit != 0 {
                m.ref_bad = true;
            }
            m.ref_seen |= bit;
            let raw = short(doc, texts, i, &mut b1);
            let ok = match (h, raw) {
                (P_REF_SERIES, Raw::Val(b)) => parse_series(b).map(|v| m.row.ref_series = v),
                (P_REF_NUMBER, Raw::Val(b)) => parse_number(b).map(|v| m.row.ref_number = v),
                (_, Raw::Val(b)) => parse_year(b).map(|v| m.row.ref_year = v),
                _ => None,
            };
            if ok.is_none() {
                m.ref_bad = true;
            }
        }
        P_TEXT => {
            let max = dec.limits.max_text_len;
            if let Some((off, len)) = table_text(dec, doc, i, Field::Text, max, m, out)? {
                m.row.text_off = off;
                m.row.text_len = len;
            }
        }
        P_SCHEDULE => {
            let max = dec.limits.max_text_len;
            if let Some((off, len)) = table_text(dec, doc, i, Field::Schedule, max, m, out)? {
                m.row.sched_off = off;
                m.row.sched_len = len;
            }
        }
        P_APT_DESIG => {
            let raw = short(doc, texts, i, &mut b1);
            feature_desig::<8>(m, FEAT_AIRPORT, Field::Airport, raw, Some(|r| &mut r.apt));
        }
        P_APT_ICAO => {
            let raw = short(doc, texts, i, &mut b1);
            // a second airport's ICAO indicator is not recorded; the first wins
            if !seen(m, Field::AirportIcao) {
                match raw {
                    Raw::Val(b) => store(m, Field::AirportIcao, parse_loc4(b), |r| &mut r.apt_icao),
                    _ => mark_err(m, Field::AirportIcao),
                }
            }
        }
        P_RWY_DESIG => {
            let raw = short(doc, texts, i, &mut b1);
            feature_desig::<12>(m, FEAT_RUNWAY, Field::Runway, raw, None);
        }
        P_RWD_DESIG => {
            let raw = short(doc, texts, i, &mut b1);
            feature_desig::<12>(m, FEAT_RUNWAY_DIRECTION, Field::Runway, raw, None);
        }
        P_UNIT_DESIG => {
            let raw = short(doc, texts, i, &mut b1);
            feature_desig::<8>(m, FEAT_UNIT, Field::Unit, raw, Some(|r| &mut r.unit));
        }
        P_NAVAID_TYPE => {
            let raw = short(doc, texts, i, &mut b1);
            feature_desig::<8>(m, FEAT_NAVAID, Field::Navaid, raw, Some(|r| &mut r.navaid));
        }
        P_RWE_POSLIST | P_TWE_POSLIST => polygon(dec, doc, i, kind, m, out)?,
        _ => {}
    }
    Ok(())
}

/// Ends the message: decides refusal, combines the parts that need two sources and appends the
/// row.
fn finish(mut m: Msg, out: &mut Notams) -> Result<u32, DocError> {
    if m.events == 0 {
        return Err(DocError::Rejected(Reject::NoEvent));
    }
    if m.events > 1 {
        return Err(DocError::Rejected(Reject::MultipleEvents));
    }
    if m.id_bad {
        return Err(DocError::Rejected(Reject::BadId));
    }
    if m.ids == 0 {
        return Err(DocError::Rejected(Reject::MissingId));
    }
    if m.notams != 1 {
        return Err(DocError::Rejected(Reject::NoNotam));
    }
    // Referred NOTAM: all three parts, or number and year alone (the local format names the
    // referred NOTAM without a series, then `ref_series` stays zero), or none.
    if m.ref_seen != 0 {
        if m.ref_bad || (m.ref_seen != 7 && m.ref_seen != 6) {
            mark_err(&mut m, Field::Ref);
        } else {
            mark_ok(&mut m, Field::Ref);
        }
    }
    // Altitude limits: an explicit limit wins over the flight-level pair.
    for (f, explicit, level) in [
        (Field::Floor, m.lo, m.min_fl),
        (Field::Ceiling, m.hi, m.max_fl),
    ] {
        if errored(&m, f) {
            continue;
        }
        if let Some((ft, r)) = explicit.or(level) {
            if f == Field::Floor {
                m.row.floor_ft = ft;
                m.row.floor_ref = r;
            } else {
                m.row.ceil_ft = ft;
                m.row.ceil_ref = r;
            }
            mark_ok(&mut m, f);
        }
    }
    if m.row.present & field_bit(Field::Floor) != 0
        && m.row.present & field_bit(Field::Ceiling) != 0
        && m.row.floor_ref == m.row.ceil_ref
        && m.row.ceil_ft != ALT_UNLIMITED
        && m.row.floor_ft > m.row.ceil_ft
    {
        mark_err(&mut m, Field::Ceiling);
    }
    // Runway: the runway's designator, else the runway direction's.
    if !errored(&m, Field::Runway) {
        let pick = [FEAT_RUNWAY, FEAT_RUNWAY_DIRECTION]
            .into_iter()
            .find(|&k| m.desig_set & (1u32 << k) != 0);
        if let Some(k) = pick {
            if let Some(d) = m.desig.get(usize::from(k)) {
                m.row.rwy = *d;
                mark_ok(&mut m, Field::Runway);
            }
        }
    }
    // The most specific feature, and its designator.
    if let Some(&k) = PRIORITY
        .iter()
        .find(|&&k| m.row.feat_mask & (1u32 << k) != 0)
    {
        m.row.feat_kind = k;
        if m.desig_set & (1u32 << k) != 0 && k != FEAT_NAVAID {
            m.row.feat_desig = m.desig.get(usize::from(k)).copied().unwrap_or([0; 12]);
        }
    }
    m.row.poly_first = m.start.polygons as u32;
    m.row.poly_count = out.poly_vstart.len().saturating_sub(m.start.polygons) as u32;
    if m.poly_failed {
        m.row.poly_count = 0;
    }
    if m.row.poly_count == 0 {
        m.row.present &= !field_bit(Field::Polygons);
    }
    let idx = out.len as u32;
    if !push_row(out, &m.row) {
        return Err(DocError::Capacity(CapacityKind::Rows));
    }
    Ok(idx)
}
