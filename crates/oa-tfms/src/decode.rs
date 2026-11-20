//! The decoder: a parsed [`Doc`] in, flight rows and flow rows out. Core (pure): no I/O, no
//! clock, no allocation (the output columns and the text table were reserved by
//! [`crate::new_rows`]).
//!
//! * One flight row per `fltdMessage` of a `fltdOutput`, and one per `flightData` entry of a
//!   `TMI_FLIGHT_LIST`; one flow row per `RSTR`, `GADV`, `FXA` or `APTC` message. Interface
//!   status documents (`tfmsStatusOutput`) produce no rows and are counted.
//! * All or nothing per message, not per document: a message with a refused field (a repeated
//!   leaf, a bad number, a missing identity) is rolled back to nothing and counted in
//!   [`Outcome::rejected`] with its reason, and its valid sibling messages are kept (a TMI
//!   flight list keeps its other entries the same way). Only a document whose structure is
//!   unreadable (XML error, wrong root) or that overruns a capacity is refused whole, leaving
//!   the rows and the text table exactly as they were (RULESET E5, INTERFACES IF-R5).
//! * A word outside a vocabulary (a trigger, a status, a category, a time type, a program, a
//!   scope, a true/false flag) does not refuse the message: the column holds the unknown code
//!   ([`crate::rows::UNKNOWN_WORD`], [`crate::rows::flag::UNKNOWN`]) and the word is counted in
//!   [`Outcome::unknown_words`]. The one exception is the direction of a position, which the
//!   position cannot be read without.
//! * Element text split by a comment or a CDATA section is joined (XML 1.0) before it is
//!   validated; the size bounds apply to the joined value.
//! * The walk is driven by the static tables of [`crate::schema`]: each message type has a body
//!   table, elements are matched by namespace URI and local name, and each visited element's
//!   attributes and text go to the field the table names. Strict field parsers: names are
//!   printable ASCII within their column, numbers are plain decimal digits, times are
//!   [`crate::parse_iso_ms`], words come from closed vocabularies, free text is valid UTF-8
//!   without control characters. An element that is present but empty is absent.
//! * A field met twice in one message is refused (`Duplicate`), except the identity fields that
//!   the message attributes and the body both carry (`acid`, departure, arrival), which must
//!   agree (`Inconsistent`).
//! * Unknown elements and attributes are not errors: each is counted in
//!   [`Outcome::unknown_paths`] (a whole unknown subtree counts once). Known elements the
//!   decoder deliberately does not store (route traversal lists, filters, polygons) are counted
//!   in [`Outcome::ignored_paths`], once per subtree. Namespace declarations and the root's
//!   `xsi:schemaLocation` are not counted.
//! * Bounded work: the walk visits each known element once on a fixed-depth stack (nothing
//!   recurses), skips ignored subtrees without entering them, and decodes a value into a 64-byte
//!   stack buffer; free text goes straight into the caller's text table.

use crate::fields::{
    error_kind_index, CapKind, DocError, DocKind, Field, FieldError, FieldErrorKind,
    ERROR_KIND_COUNT, FIELD_COUNT,
};
use crate::rows::{
    accat, divert, fkind, flag, flights_len, flows_len, kind, prog, push_flight, push_flow, scope,
    spd_unit, status, text, trig, truncate_rows, ttype, ucat, FlightRow, FlowRow, Rows, StrRef,
    BLANK_FLIGHT, BLANK_FLOW, NO_TIME, UNKNOWN_WORD,
};
use crate::scalar::{
    contains_word, dms_e7, parse_altitude, parse_bool, parse_decimal_e7, parse_digits,
    parse_digits64, parse_letter, parse_name, parse_speed_unit, parse_time, parse_vocab,
    parse_words,
};
use crate::schema::{
    ns_uri, At, El, Nsx, BODIES, EMPTY, FI_MESSAGE, FLIGHT_DATA, FLOW_BODIES, FLTD_MESSAGE,
    MAX_DEPTH, TMI_LIST_BODY, TMI_LIST_TYPE,
};
use oa_sealed::DropWindow;
use oa_xml::{
    attr_needs_decode, decode_into, decode_text_run, ns_uri_bytes, resolve_elem_ns, span_bytes,
    text_needs_decode, Doc, ElemIx, Limits, Ns, Parser, Span, SpanKind, XmlError,
};

/// The TFM data service namespace, of the root and its outputs.
pub const NAMESPACE: &[u8] = b"urn:us:gov:dot:faa:atm:tfm:tfmdataservice";

/// Largest decoded value the decoder handles outside the text table. The longest stored name is
/// 16 bytes and a timestamp is 24; free text is decoded straight into the table.
const MAX_VALUE: usize = 64;

/// What one accepted document turned out to be.
// Budget: returned once per document, never stored in quantity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub kind: DocKind,
    /// `fltdMessage`, `fiMessage` and status messages seen (decoded into a row or not).
    pub messages: u32,
    /// Flight rows appended to the caller's [`Rows`].
    pub flight_rows: u32,
    /// Flow rows appended.
    pub flow_rows: u32,
    pub unknown_paths: u32,
    pub ignored_paths: u32,
    /// Flight and flow rows read but not appended because `Rows` was full
    /// ([`decode_doc_prefix`] only; always 0 from [`decode_doc`]).
    pub dropped: u32,
    /// Messages (or TMI flight list entries) refused for a field and rolled back; they are
    /// counted in `messages` too, and add no rows.
    pub rejected: u32,
    /// The first refused field of the document, with its reason and offset.
    pub first_reject: Option<FieldError>,
    /// Refused messages by [`Field`] (index `field as usize`) and by reason
    /// ([`error_kind_index`]); each sums to `rejected`.
    pub reject_fields: [u32; FIELD_COUNT],
    pub reject_kinds: [u32; ERROR_KIND_COUNT],
    /// Words outside a vocabulary that were kept as unknown instead of refusing the message.
    pub unknown_words: u32,
    /// Speeds of a form that is neither knots nor Mach, kept with unit
    /// [`spd_unit::UNKNOWN`] instead of refusing the message.
    pub unknown_units: u32,
    /// Messages of a document dropped whole because one of them has its `sourceTimeStamp` in
    /// [`Rows::window`] (OA-SEAL-014). The document was not decoded: every other field of the
    /// outcome is zero and nothing was touched.
    pub sealed_dropped: u32,
}

/// Parses `input` with `parser` and decodes it. A parse error is [`DocError::Xml`].
pub fn decode_bytes(
    parser: &mut Parser,
    input: &[u8],
    rows: &mut Rows,
) -> Result<Outcome, DocError> {
    match parser.parse(input) {
        Ok(doc) => decode_doc(&doc, rows),
        Err(e) => Err(DocError::Xml(e)),
    }
}

/// The parser limits the survey shows are ample: the defaults (1 MiB of input; the largest
/// sampled document is 213 KB and has 2,541 elements).
pub fn default_limits() -> Limits {
    Limits::default()
}

/// Decodes one parsed document into `rows`. On `Err` the rows and the text table are unchanged.
pub fn decode_doc(doc: &Doc<'_>, rows: &mut Rows) -> Result<Outcome, DocError> {
    decode_with(doc, rows, false)
}

/// Like [`decode_doc`], but a document with more flight or flow rows than `rows` has room for is
/// decoded as far as it fits instead of being refused: the rows that fit are appended and
/// [`Outcome::dropped`] counts the others. Every other error, including a full text table, still
/// leaves `rows` as it was.
pub fn decode_doc_prefix(doc: &Doc<'_>, rows: &mut Rows) -> Result<Outcome, DocError> {
    decode_with(doc, rows, true)
}

/// Reads every observation time and every actual-event time of the document: the
/// `sourceTimeStamp` of every element that has one (the `fltdMessage` and `fiMessage` messages,
/// which also time their TMI flight list entries), the position times, and the actual times of
/// departure, arrival, gate, runway, boundary crossing and coordination (a message stamped after
/// the window can report an event inside it), directly from the parsed tree and whatever else is valid in it. If any is in `window`, returns how many
/// messages the document has (all are dropped, at least 1); otherwise 0. A time that cannot be
/// read is not in the window (the frame's receive time gates it).
fn prescan(doc: &Doc<'_>, window: DropWindow) -> u32 {
    if window.span().is_none() {
        return 0;
    }
    // Attributes that are an observation or the time of an actual event, on any element.
    const ATTRS: [&[u8]; 11] = [
        b"sourceTimeStamp",
        b"airlineOutTime",
        b"airlineOffTime",
        b"airlineOnTime",
        b"airlineInTime",
        b"runwayDeparture",
        b"runwayArrival",
        b"gateDeparture",
        b"gateArrival",
        b"boundaryCrossingTime",
        b"lastUpdateTime",
    ];
    // Whether attribute `name` of `e` has the value `want`.
    let attr_is = |e: ElemIx, name: &[u8], want: &[u8]| {
        (0..attr_count(doc, e)).any(|k| {
            let a = attr_ref(doc, e, k);
            let mut buf = [0u8; MAX_VALUE];
            a.name == name && value_bytes(doc, attr_val(&a), &mut buf).is_ok_and(|v| v == want)
        })
    };
    let in_window = |v: &[u8]| parse_time(v).is_ok_and(|t| window.contains(t));
    let (mut messages, mut hit) = (0u32, false);
    for i in 0..doc.elem_name.len() {
        let e = ElemIx(i as u32);
        let name = elem_name(doc, e);
        if name == b"fltdMessage" || name == b"fiMessage" || name == b"tfmsStatusOutput" {
            messages = messages.saturating_add(1);
        }
        // `eta` and `etd` carry an actual event time when their type says so.
        let typed_actual = (name == b"etd" && attr_is(e, b"etdType", b"ACTUAL"))
            || (name == b"eta" && attr_is(e, b"etaType", b"ACTUAL"));
        for k in 0..attr_count(doc, e) {
            let a = attr_ref(doc, e, k);
            let key = ATTRS.contains(&a.name);
            let typed = typed_actual && a.name == b"timeValue";
            if !key && !typed {
                continue;
            }
            let mut buf = [0u8; MAX_VALUE];
            if let Ok(v) = value_bytes(doc, attr_val(&a), &mut buf) {
                hit |= in_window(v);
            }
        }
    }
    // The observation and actual-event times that are element text: the position time
    // (`timeAtPosition`, an oceanic position's `time`), the actual time of departure or arrival
    // (`estimated` is not "true"), an actual coordination time, and the times a flow message
    // reports (`eventTime`, `dateSent`, `lastUpdate`). A text that a comment or CDATA section
    // splits is one value.
    let total = doc.text_owner.len();
    let mut t = 0usize;
    while t < total {
        let Some(&owner) = doc.text_owner.get(t) else {
            break;
        };
        let first = t;
        while doc.text_owner.get(t) == Some(&owner) {
            t += 1;
        }
        let name = elem_name(doc, owner);
        let key = match name {
            b"timeAtPosition" | b"time" | b"eventTime" | b"dateSent" | b"lastUpdate" => true,
            b"timeOfDeparture" | b"timeOfArrival" => !attr_is(owner, b"estimated", b"true"),
            b"coordinationTime" | b"newCoordinationTime" => attr_is(owner, b"type", b"ACTUAL"),
            _ => false,
        };
        if key {
            let mut buf = [0u8; MAX_VALUE];
            if let Ok(n) = decode_text_run(doc, first, t - first, &mut buf) {
                hit |= buf.get(..n).is_some_and(in_window);
            }
        }
    }
    if hit {
        messages.max(1)
    } else {
        0
    }
}

/// The outcome of a document dropped whole by [`prescan`]: only the count, nothing else.
fn sealed_outcome(messages: u32) -> Outcome {
    Outcome {
        kind: DocKind::Empty,
        messages: 0,
        flight_rows: 0,
        flow_rows: 0,
        unknown_paths: 0,
        ignored_paths: 0,
        dropped: 0,
        rejected: 0,
        first_reject: None,
        reject_fields: [0; FIELD_COUNT],
        reject_kinds: [0; ERROR_KIND_COUNT],
        unknown_words: 0,
        unknown_units: 0,
        sealed_dropped: messages,
    }
}

fn decode_with(doc: &Doc<'_>, rows: &mut Rows, prefix: bool) -> Result<Outcome, DocError> {
    // The first step, before any count, row, string or table is touched: a document with a
    // message in the sealed window is dropped whole.
    let gone = prescan(doc, rows.window);
    if gone > 0 {
        return Ok(sealed_outcome(gone));
    }
    let f0 = flights_len(&rows.flights);
    let w0 = flows_len(&rows.flows);
    let s0 = rows.strings_used;
    let mut acc = Acc {
        t: 0,
        unknown: 0,
        ignored: 0,
        rejected: 0,
        first_reject: None,
        reject_fields: [0; FIELD_COUNT],
        reject_kinds: [0; ERROR_KIND_COUNT],
    };
    let r = decode_root(doc, &mut acc, rows, prefix);
    if r.is_err() {
        truncate_rows(rows, f0, w0, s0);
    }
    r
}

/// Running state of one decode: the text cursor and the path counters.
struct Acc {
    t: usize,
    unknown: u32,
    ignored: u32,
    rejected: u32,
    first_reject: Option<FieldError>,
    reject_fields: [u32; FIELD_COUNT],
    reject_kinds: [u32; ERROR_KIND_COUNT],
}

/// Degrees, minutes, seconds and hemisphere of one coordinate, collected from four attributes.
#[derive(Clone, Copy, Default)]
struct Dms {
    deg: u32,
    min: u32,
    sec: u32,
    neg: bool,
    /// Bit 0 degrees, 1 minutes, 2 seconds, 3 direction.
    have: u8,
}

/// The working state of one message: the staged rows, the duplicate set and the text table.
struct Ctx<'r> {
    rows: &'r mut Rows,
    f: FlightRow,
    w: FlowRow,
    /// Fields go to the flow row when set (the shared `sourceTimeStamp`, `sourceFacility`).
    flow: bool,
    seen: u128,
    lat: Dms,
    lon: Dms,
    /// A row that does not fit is counted in `dropped` instead of failing the document.
    prefix: bool,
    dropped: u32,
    /// Vocabulary words kept as unknown.
    unknown_words: u32,
    /// Speeds whose form is no known unit, kept as unknown.
    unknown_units: u32,
}

/// Where a value's bytes are: one span (an attribute value, or a text node), or a run of text
/// nodes that a comment or CDATA section split (`count` of them from `first`; 0 for a span).
#[derive(Clone, Copy)]
struct Val {
    span: Span,
    flagged: bool,
    kind: SpanKind,
    first: usize,
    count: usize,
}

fn ferr(field: Field, kind: FieldErrorKind, at: u32) -> DocError {
    DocError::Field(FieldError { field, kind, at })
}

fn elem_name<'a>(doc: &Doc<'a>, e: ElemIx) -> &'a [u8] {
    doc.elem_name
        .get(e.0 as usize)
        .and_then(|&s| span_bytes(doc.input, s).ok())
        .unwrap_or(&[])
}

fn elem_at(doc: &Doc<'_>, e: ElemIx) -> u32 {
    doc.elem_name.get(e.0 as usize).map_or(0, |s| s.start)
}

fn in_ns(doc: &Doc<'_>, e: ElemIx, ns: Nsx) -> bool {
    match resolve_elem_ns(doc, e) {
        Ns::Uri(s) => ns_uri_bytes(doc, s).is_some_and(|b| b == ns_uri(ns)),
        _ => false,
    }
}

fn first_child(doc: &Doc<'_>, e: ElemIx) -> ElemIx {
    doc.elem_first_child
        .get(e.0 as usize)
        .copied()
        .unwrap_or(ElemIx::NONE)
}

fn next_sibling(doc: &Doc<'_>, e: ElemIx) -> ElemIx {
    doc.elem_next_sibling
        .get(e.0 as usize)
        .copied()
        .unwrap_or(ElemIx::NONE)
}

fn attr_count(doc: &Doc<'_>, e: ElemIx) -> u32 {
    doc.elem_attr_count.get(e.0 as usize).copied().unwrap_or(0)
}

/// An attribute of an element: its name (unprefixed only; a prefixed attribute has an empty
/// name here, so it matches nothing and is counted unknown by the caller) and its value span.
struct AttrRef<'a> {
    name: &'a [u8],
    span: Span,
    flagged: bool,
    /// A namespace declaration (`xmlns` or `xmlns:p`), which is never data.
    decl: bool,
    /// The root's `xsi:schemaLocation`-style schema hint (a prefixed `schemaLocation`).
    schema_hint: bool,
}

fn attr_ref<'a>(doc: &Doc<'a>, e: ElemIx, i: u32) -> AttrRef<'a> {
    let a = oa_xml::AttrIx(
        doc.elem_first_attr
            .get(e.0 as usize)
            .map_or(0, |f| f.0)
            .saturating_add(i),
    );
    let ai = a.0 as usize;
    let get = |s: Option<&Span>| {
        s.and_then(|&s| span_bytes(doc.input, s).ok())
            .unwrap_or(&[][..])
    };
    let prefix = get(doc.attr_prefix.get(ai));
    let name_raw = get(doc.attr_name.get(ai));
    let prefixed = !prefix.is_empty();
    AttrRef {
        name: if prefixed { &[] } else { name_raw },
        span: doc.attr_value.get(ai).copied().unwrap_or_default(),
        flagged: attr_needs_decode(doc, a),
        decl: prefix == b"xmlns" || (!prefixed && name_raw == b"xmlns"),
        schema_hint: prefixed && name_raw == b"schemaLocation",
    }
}

fn value_bytes<'x>(
    doc: &Doc<'x>,
    v: Val,
    buf: &'x mut [u8; MAX_VALUE],
) -> Result<&'x [u8], FieldErrorKind> {
    if v.count > 1 {
        return match decode_text_run(doc, v.first, v.count, buf) {
            Ok(n) => buf.get(..n).ok_or(FieldErrorKind::Text),
            Err(XmlError::OutputTooSmall { .. }) => Err(FieldErrorKind::TooLong),
            Err(_) => Err(FieldErrorKind::Text),
        };
    }
    if !v.flagged {
        return span_bytes(doc.input, v.span).map_err(|_| FieldErrorKind::Text);
    }
    match decode_into(v.span, doc.input, buf, v.kind) {
        Ok(n) => buf.get(..n).ok_or(FieldErrorKind::Text),
        Err(XmlError::OutputTooSmall { .. }) => Err(FieldErrorKind::TooLong),
        Err(_) => Err(FieldErrorKind::Text),
    }
}

/// An attribute value as a [`Val`].
fn attr_val(a: &AttrRef<'_>) -> Val {
    Val {
        span: a.span,
        flagged: a.flagged,
        kind: SpanKind::AttrValue,
        first: 0,
        count: 0,
    }
}

/// The text of leaf element `e`, `None` when it has none. Elements must be asked for in
/// document order; the cursor in `acc` skips the text nodes of everything visited before. Text
/// that a comment or CDATA section split is one value over several nodes.
fn leaf_text(
    doc: &Doc<'_>,
    acc: &mut Acc,
    e: ElemIx,
    field: Field,
) -> Result<Option<Val>, DocError> {
    let at = elem_at(doc, e);
    if first_child(doc, e) != ElemIx::NONE {
        return Err(ferr(field, FieldErrorKind::NotLeaf, at));
    }
    let n = doc.text_owner.len();
    while acc.t < n && doc.text_owner.get(acc.t).is_some_and(|o| o.0 < e.0) {
        acc.t += 1;
    }
    let first = acc.t;
    while acc.t < n && doc.text_owner.get(acc.t) == Some(&e) {
        acc.t += 1;
    }
    let count = acc.t - first;
    if count == 0 {
        return Ok(None);
    }
    let span = doc.text_span.get(first).copied().unwrap_or_default();
    let flagged = text_needs_decode(doc, oa_xml::TextIx(first as u32));
    Ok(Some(Val {
        span,
        flagged,
        kind: SpanKind::Text,
        first,
        count,
    }))
}

// ---- storing a value -----------------------------------------------------------------------

/// Fields whose value is free text kept in the text table.
fn is_text(f: Field) -> bool {
    matches!(
        f,
        Field::Route
            | Field::RouteLegacy
            | Field::Remarks
            | Field::Fca
            | Field::FText
            | Field::FTitle
            | Field::FElement
            | Field::FAirports
            | Field::FRemarks
    )
}

/// Fields that the message attributes and the body both carry: no duplicate check, an
/// agreement check instead.
fn is_same(f: Field) -> bool {
    matches!(f, Field::Acid | Field::Dep | Field::Arr)
}

fn same_name<const N: usize>(dst: &mut [u8; N], v: &[u8]) -> Result<(), FieldErrorKind> {
    let n: [u8; N] = parse_name(v)?;
    if *dst == [0; N] {
        *dst = n;
    } else if *dst != n {
        return Err(FieldErrorKind::Inconsistent);
    }
    Ok(())
}

fn set_flag(flags: &mut u16, pos: u8, code: u16) {
    *flags = (*flags & !(3 << pos)) | (code << pos);
}

fn dms_part(d: &mut Dms, bit: u8, v: &[u8], max_len: usize) -> Result<(), FieldErrorKind> {
    let n = parse_digits(v, max_len)?;
    match bit {
        0 => d.deg = n,
        1 => d.min = n,
        _ => d.sec = n,
    }
    d.have |= 1 << bit;
    Ok(())
}

fn dms_dir(d: &mut Dms, v: &[u8], pos: &[u8], neg: &[u8]) -> Result<(), FieldErrorKind> {
    d.neg = if v == pos {
        false
    } else if v == neg {
        true
    } else {
        return Err(FieldErrorKind::Vocabulary);
    };
    d.have |= 8;
    Ok(())
}

fn restriction_program(v: &[u8]) -> Result<u8, FieldErrorKind> {
    match v {
        b"MIT" => Ok(prog::MIT),
        b"MINIT" => Ok(prog::MINIT),
        b"STOP" => Ok(prog::STOP),
        b"APREQ" => Ok(prog::APREQ),
        b"TBM" => Ok(prog::TBM),
        _ => Err(FieldErrorKind::Vocabulary),
    }
}

/// Stores a small (non-text) value into the staged row.
fn store_small(c: &mut Ctx<'_>, field: Field, v: &[u8]) -> Result<(), FieldErrorKind> {
    use Field as F;
    let flow = c.flow;
    match field {
        F::MsgType => {}
        F::Acid => same_name(&mut c.f.acid, v)?,
        F::FlightRef => {
            c.f.flight_ref = parse_digits64(v, 10)?;
            c.f.flags |= flag::FLIGHT_REF;
        }
        F::Airline => c.f.airline = parse_name(v)?,
        F::Major => c.f.major = parse_name(v)?,
        F::Dep => same_name(&mut c.f.dep, v)?,
        F::Arr => same_name(&mut c.f.arr, v)?,
        F::SrcFac => {
            if flow {
                c.w.facility = parse_name(v)?;
            } else {
                c.f.src_fac = parse_name(v)?;
            }
        }
        F::SrcTime => {
            let t = parse_time(v)?;
            if flow {
                c.w.src_ms = t;
            } else {
                c.f.src_ms = t;
            }
        }
        F::Trigger => c.f.trigger = parse_vocab(v, &trig::NAMES)?,
        F::CdmPart => set_flag(&mut c.f.flags, flag::CDM_PART, parse_bool(v)?),
        F::Gufi => c.f.gufi = parse_name(v)?,
        F::Igtd => c.f.igtd_ms = parse_time(v)?,
        F::CidFac => c.f.cid_fac = parse_name(v)?,
        F::CidNum => c.f.cid_num = parse_name(v)?,
        F::AcCat => c.f.ac_cat = parse_vocab(v, &accat::NAMES)?,
        F::UserCat => c.f.user_cat = parse_vocab(v, &ucat::NAMES)?,
        F::Eta => c.f.eta_ms = parse_time(v)?,
        F::EtaType => c.f.eta_type = parse_vocab(v, &ttype::NAMES)?,
        F::Etd => c.f.etd_ms = parse_time(v)?,
        F::EtdType => c.f.etd_type = parse_vocab(v, &ttype::NAMES)?,
        F::Status | F::TmiStatus => c.f.status = parse_vocab(v, &status::NAMES)?,
        F::AcType => c.f.ac_type = parse_name(v)?,
        F::EngClass => c.f.eng_class = parse_vocab(v, &accat::NAMES)?,
        F::Equip => c.f.equip = parse_letter(v)?,
        F::Special => c.f.special = parse_words(v)?,
        F::Diversion => c.f.diversion = parse_vocab(v, &divert::NAMES)?,
        F::RvsmEq => set_flag(&mut c.f.flags, flag::RVSM_EQ, parse_bool(v)?),
        F::RvsmCur => set_flag(&mut c.f.flags, flag::RVSM_CUR, parse_bool(v)?),
        F::RvsmFut => set_flag(&mut c.f.flags, flag::RVSM_FUT, parse_bool(v)?),
        F::OutT => c.f.out_ms = parse_time(v)?,
        F::OffT => c.f.off_ms = parse_time(v)?,
        F::OnT => c.f.on_ms = parse_time(v)?,
        F::InT => c.f.in_ms = parse_time(v)?,
        F::RwyDep => c.f.rwy_dep_ms = parse_time(v)?,
        F::RwyArr => c.f.rwy_arr_ms = parse_time(v)?,
        F::GateDep => c.f.gate_dep_ms = parse_time(v)?,
        F::GateArr => c.f.gate_arr_ms = parse_time(v)?,
        F::Created => c.f.created_ms = parse_time(v)?,
        F::OrigDep => c.f.orig_dep_ms = parse_time(v)?,
        F::OrigArr => c.f.orig_arr_ms = parse_time(v)?,
        F::ArrFix => c.f.arr_fix = parse_name(v)?,
        F::ArrFixTime => c.f.arrfix_ms = parse_time(v)?,
        F::DepFix => c.f.dep_fix = parse_name(v)?,
        F::DepFixTime => c.f.depfix_ms = parse_time(v)?,
        F::LatDeg => dms_part(&mut c.lat, 0, v, 3)?,
        F::LatMin => dms_part(&mut c.lat, 1, v, 2)?,
        F::LatSec => dms_part(&mut c.lat, 2, v, 2)?,
        F::LatDir => dms_dir(&mut c.lat, v, b"NORTH", b"SOUTH")?,
        F::LonDeg => dms_part(&mut c.lon, 0, v, 3)?,
        F::LonMin => dms_part(&mut c.lon, 1, v, 2)?,
        F::LonSec => dms_part(&mut c.lon, 2, v, 2)?,
        F::LonDir => dms_dir(&mut c.lon, v, b"EAST", b"WEST")?,
        F::NextLat => c.f.next_lat_e7 = parse_decimal_e7(v, 90)?,
        F::NextLon => c.f.next_lon_e7 = parse_decimal_e7(v, 180)?,
        F::PosTime => c.f.pos_ms = parse_time(v)?,
        F::Alt => {
            let (ft, sfx) = parse_altitude(v)?;
            c.f.alt_ft = ft;
            c.f.alt_sfx = sfx;
        }
        F::AsgAlt => c.f.asg_ft = parse_altitude(v)?.0,
        F::ReqAlt => c.f.req_ft = parse_altitude(v)?.0,
        F::Speed => {
            (c.f.speed_milli, c.f.speed_unit) = speed_of(c, v)?;
        }
        F::Tas => {
            (c.f.tas_milli, c.f.tas_unit) = speed_of(c, v)?;
        }
        F::Dp => c.f.dp = parse_name(v)?,
        F::Star => c.f.star = parse_name(v)?,
        F::Tod => c.f.tod_ms = parse_time(v)?,
        F::TodEst => set_flag(&mut c.f.flags, flag::TOD_EST, parse_bool(v)?),
        F::Toa => c.f.toa_ms = parse_time(v)?,
        F::ToaEst => set_flag(&mut c.f.flags, flag::TOA_EST, parse_bool(v)?),
        F::Coord => c.f.coord_ms = parse_time(v)?,
        F::CoordType => c.f.coord_type = parse_vocab(v, &ttype::NAMES)?,
        F::CoordFix => c.f.coord_fix = parse_name(v)?,
        F::Bnd => c.f.bnd_ms = parse_time(v)?,
        F::TmiUpdate => {
            c.f.tmi_update = match v {
                b"UPDATE" => crate::rows::tmiu::UPDATE,
                b"NEW" => crate::rows::tmiu::NEW,
                b"DROPOUT" => crate::rows::tmiu::DROPOUT,
                _ => return Err(FieldErrorKind::Vocabulary),
            };
        }
        F::TmiTime => c.f.tmi_ms = parse_time(v)?,
        F::Category => c.w.program = restriction_program(v)?,
        F::FcaType => {
            c.w.program = match v {
                b"FEA" => prog::FEA,
                b"FCA" => prog::FCA,
                _ => return Err(FieldErrorKind::Vocabulary),
            };
        }
        F::Scope => {
            c.w.scope = match v {
                b"ARRIVAL" => scope::ARRIVAL,
                b"DEPARTURE" => scope::DEPARTURE,
                b"ENROUTE" => scope::ENROUTE,
                _ => return Err(FieldErrorKind::Vocabulary),
            };
        }
        F::FId => c.w.id = parse_digits64(v, 10)?,
        F::FNumber => c.w.number = parse_digits64(v, 10)?,
        F::FRefId => c.w.ref_id = parse_digits64(v, 10)?,
        F::FAction => {
            c.w.action = u16::try_from(parse_digits(v, 3)?).map_err(|_| FieldErrorKind::Format)?;
        }
        F::FStart => c.w.start_ms = parse_time(v)?,
        F::FEnd => c.w.end_ms = parse_time(v)?,
        F::FEvent => c.w.event_ms = parse_time(v)?,
        F::FOrigin => c.w.origin = parse_words(v)?,
        F::FRate => c.w.rate = parse_speed_wide(v)?,
        F::FRateDep => c.w.rate_dep = parse_speed_wide(v)?,
        F::FRateStrat => c.w.rate_strat = parse_speed_wide(v)?,
        F::FStatus => {
            c.w.status = match v {
                b"NEW" => crate::rows::fstatus::NEW,
                b"UPDATED" => crate::rows::fstatus::UPDATED,
                _ => return Err(FieldErrorKind::Vocabulary),
            };
        }
        // Text fields, the position pair and the counter are not stored through here.
        F::Position
        | F::NFxa
        | F::Route
        | F::RouteLegacy
        | F::Remarks
        | F::Fca
        | F::FText
        | F::FTitle
        | F::FElement
        | F::FAirports
        | F::FRemarks => {}
    }
    Ok(())
}

/// A rate or similar count: one to five digits.
/// A speed and its unit; a shape that is none of the known forms is kept as unknown and counted.
fn speed_of(c: &mut Ctx<'_>, v: &[u8]) -> Result<(i32, u8), FieldErrorKind> {
    let (milli, unit) = parse_speed_unit(v)?;
    if unit == spd_unit::UNKNOWN {
        c.unknown_units = c.unknown_units.saturating_add(1);
    }
    Ok((milli, unit))
}

fn parse_speed_wide(v: &[u8]) -> Result<i32, FieldErrorKind> {
    i32::try_from(parse_digits(v, 5)?).map_err(|_| FieldErrorKind::Format)
}

/// Free text must be valid UTF-8 (the parser guarantees it) without control characters other
/// than line feed and tab.
fn text_ok(b: &[u8]) -> bool {
    b.iter()
        .all(|&c| (c >= 0x20 && c != 0x7f) || c == b'\n' || c == b'\t')
}

/// Decodes a free-text value into the text table and records its reference. An empty value is
/// absent; text that does not fit is the `Strings` capacity reject.
fn put_text(
    doc: &Doc<'_>,
    c: &mut Ctx<'_>,
    field: Field,
    val: Val,
    at: u32,
) -> Result<(), DocError> {
    let Val {
        span,
        flagged,
        kind,
        first,
        count,
    } = val;
    if field == Field::Route && c.f.flags & flag::ROUTE_LEGACY != 0 {
        return Ok(());
    }
    let used = c.rows.strings_used;
    let Some(dst) = c.rows.strings.get_mut(used..) else {
        return Err(DocError::Capacity(CapKind::Strings));
    };
    let n = if count > 1 {
        match decode_text_run(doc, first, count, dst) {
            Ok(n) => n,
            Err(XmlError::OutputTooSmall { .. }) => {
                return Err(DocError::Capacity(CapKind::Strings));
            }
            Err(_) => return Err(ferr(field, FieldErrorKind::Text, at)),
        }
    } else if flagged {
        match decode_into(span, doc.input, dst, kind) {
            Ok(n) => n,
            Err(XmlError::OutputTooSmall { .. }) => {
                return Err(DocError::Capacity(CapKind::Strings));
            }
            Err(_) => return Err(ferr(field, FieldErrorKind::Text, at)),
        }
    } else {
        let src = span_bytes(doc.input, span).map_err(|_| ferr(field, FieldErrorKind::Text, at))?;
        let Some(d) = dst.get_mut(..src.len()) else {
            return Err(DocError::Capacity(CapKind::Strings));
        };
        d.copy_from_slice(src);
        src.len()
    };
    let ok = dst.get(..n).is_some_and(text_ok);
    if !ok {
        return Err(ferr(field, FieldErrorKind::BadChar, at));
    }
    if n == 0 {
        return Ok(());
    }
    let r = StrRef {
        off: u32::try_from(used).map_err(|_| DocError::Capacity(CapKind::Strings))?,
        len: u32::try_from(n).map_err(|_| DocError::Capacity(CapKind::Strings))?,
    };
    c.rows.strings_used = used + n;
    match field {
        Field::Route => c.f.route = r,
        Field::RouteLegacy => {
            c.f.route = r;
            c.f.flags |= flag::ROUTE_LEGACY;
        }
        Field::Remarks => c.f.remarks = r,
        Field::Fca => c.f.fca = r,
        Field::FText => c.w.text = r,
        Field::FTitle => c.w.title = r,
        Field::FElement => c.w.element = r,
        Field::FAirports => c.w.airports = r,
        _ => c.w.remarks = r,
    }
    Ok(())
}

/// Stores the value at `span` into `field`, after the duplicate check.
fn store_value(
    doc: &Doc<'_>,
    c: &mut Ctx<'_>,
    field: Field,
    val: Val,
    at: u32,
) -> Result<(), DocError> {
    if !is_same(field) {
        let bit = 1u128 << (field as u32);
        if c.seen & bit != 0 {
            return Err(ferr(field, FieldErrorKind::Duplicate, at));
        }
        c.seen |= bit;
    }
    if is_text(field) {
        return put_text(doc, c, field, val, at);
    }
    let mut buf = [0u8; MAX_VALUE];
    let v = value_bytes(doc, val, &mut buf).map_err(|k| ferr(field, k, at))?;
    match store_small(c, field, v) {
        Ok(()) => Ok(()),
        Err(FieldErrorKind::Vocabulary) if mark_unknown(c, field) => {
            c.unknown_words = c.unknown_words.saturating_add(1);
            Ok(())
        }
        Err(k) => Err(ferr(field, k, at)),
    }
}

/// Records that `field` held a word outside its vocabulary, in the field's column. False for a
/// field whose message cannot be kept without the word (the direction of a position).
fn mark_unknown(c: &mut Ctx<'_>, field: Field) -> bool {
    use Field as F;
    match field {
        F::Trigger => c.f.trigger = UNKNOWN_WORD,
        F::CdmPart => set_flag(&mut c.f.flags, flag::CDM_PART, flag::UNKNOWN),
        F::RvsmEq => set_flag(&mut c.f.flags, flag::RVSM_EQ, flag::UNKNOWN),
        F::RvsmCur => set_flag(&mut c.f.flags, flag::RVSM_CUR, flag::UNKNOWN),
        F::RvsmFut => set_flag(&mut c.f.flags, flag::RVSM_FUT, flag::UNKNOWN),
        F::TodEst => set_flag(&mut c.f.flags, flag::TOD_EST, flag::UNKNOWN),
        F::ToaEst => set_flag(&mut c.f.flags, flag::TOA_EST, flag::UNKNOWN),
        F::AcCat => c.f.ac_cat = UNKNOWN_WORD,
        F::UserCat => c.f.user_cat = UNKNOWN_WORD,
        F::EtaType => c.f.eta_type = UNKNOWN_WORD,
        F::EtdType => c.f.etd_type = UNKNOWN_WORD,
        F::Status | F::TmiStatus => c.f.status = UNKNOWN_WORD,
        F::EngClass => c.f.eng_class = UNKNOWN_WORD,
        F::Diversion => c.f.diversion = UNKNOWN_WORD,
        F::CoordType => c.f.coord_type = UNKNOWN_WORD,
        F::TmiUpdate => c.f.tmi_update = UNKNOWN_WORD,
        F::Category | F::FcaType => c.w.program = UNKNOWN_WORD,
        F::Scope => c.w.scope = UNKNOWN_WORD,
        F::FStatus => c.w.status = UNKNOWN_WORD,
        _ => return false,
    }
    true
}

// ---- the walk ------------------------------------------------------------------------------

fn bump(c: &mut Ctx<'_>, f: Field) {
    if f == Field::NFxa {
        c.f.n_fxa = c.f.n_fxa.saturating_add(1);
    }
}

/// Reads element `e`: its attributes through `def.attrs`, its counter, its text.
fn enter(
    doc: &Doc<'_>,
    acc: &mut Acc,
    c: &mut Ctx<'_>,
    e: ElemIx,
    def: &El,
) -> Result<(), DocError> {
    for i in 0..attr_count(doc, e) {
        let a = attr_ref(doc, e, i);
        if a.decl {
            continue;
        }
        let known = if a.name.is_empty() {
            None
        } else {
            def.attrs.iter().find(|x| x.name == a.name)
        };
        match known {
            Some(&At { f: Some(f), .. }) => {
                store_value(doc, c, f, attr_val(&a), a.span.start)?;
            }
            Some(_) => acc.ignored += 1,
            None => acc.unknown += 1,
        }
    }
    if let Some(f) = def.enter {
        bump(c, f);
    }
    if let Some(f) = def.text {
        if let Some(v) = leaf_text(doc, acc, e, f)? {
            store_value(doc, c, f, v, v.span.start)?;
        }
    }
    Ok(())
}

/// The table entry for child `ch` of an element with table entry `def`.
fn find_kid(doc: &Doc<'_>, def: &'static El, ch: ElemIx) -> Option<&'static El> {
    let name = elem_name(doc, ch);
    def.kids
        .iter()
        .find(|k| k.name == name && in_ns(doc, ch, k.ns))
}

/// An open element during the walk.
#[derive(Clone, Copy)]
struct Frame {
    def: &'static El,
    next: ElemIx,
}

/// Walks the subtree of `start` (table entry `def`) in document order on a fixed-depth stack.
fn walk(
    doc: &Doc<'_>,
    acc: &mut Acc,
    c: &mut Ctx<'_>,
    start: ElemIx,
    def: &'static El,
) -> Result<(), DocError> {
    enter(doc, acc, c, start, def)?;
    let blank = Frame {
        def: &EMPTY,
        next: ElemIx::NONE,
    };
    let mut stack = [blank; MAX_DEPTH];
    let mut depth = 0usize;
    if let Some(s) = stack.get_mut(0) {
        *s = Frame {
            def,
            next: first_child(doc, start),
        };
        depth = 1;
    }
    while depth > 0 {
        let Some(top) = stack.get(depth - 1).copied() else {
            break;
        };
        if top.next == ElemIx::NONE {
            depth -= 1;
            continue;
        }
        let ch = top.next;
        if let Some(s) = stack.get_mut(depth - 1) {
            s.next = next_sibling(doc, ch);
        }
        match find_kid(doc, top.def, ch) {
            None => acc.unknown += 1,
            Some(k) if k.ignore => {
                acc.ignored += 1;
                if let Some(f) = k.enter {
                    bump(c, f);
                }
            }
            Some(k) => {
                enter(doc, acc, c, ch, k)?;
                let kid = first_child(doc, ch);
                if kid != ElemIx::NONE {
                    match stack.get_mut(depth) {
                        Some(s) => {
                            *s = Frame { def: k, next: kid };
                            depth += 1;
                        }
                        None => acc.unknown += 1,
                    }
                }
            }
        }
    }
    Ok(())
}

// ---- messages ------------------------------------------------------------------------------

/// The `msgType` attribute of message element `m`.
fn msg_type<'x>(
    doc: &Doc<'x>,
    m: ElemIx,
    buf: &'x mut [u8; MAX_VALUE],
) -> Result<&'x [u8], DocError> {
    for i in 0..attr_count(doc, m) {
        let a = attr_ref(doc, m, i);
        if a.name == b"msgType" {
            return value_bytes(doc, attr_val(&a), buf)
                .map_err(|k| ferr(Field::MsgType, k, a.span.start));
        }
    }
    Err(ferr(
        Field::MsgType,
        FieldErrorKind::Missing,
        elem_at(doc, m),
    ))
}

/// The state a message starts from, to roll back to when the message is refused.
#[derive(Clone, Copy)]
struct Mark {
    flights: usize,
    flows: usize,
    strings: usize,
    unknown: u32,
    ignored: u32,
    words: u32,
    units: u32,
    dropped: u32,
}

fn mark(acc: &Acc, c: &Ctx<'_>) -> Mark {
    Mark {
        flights: flights_len(&c.rows.flights),
        flows: flows_len(&c.rows.flows),
        strings: c.rows.strings_used,
        unknown: acc.unknown,
        ignored: acc.ignored,
        words: c.unknown_words,
        units: c.unknown_units,
        dropped: c.dropped,
    }
}

/// Runs `f` (the decode of one message); a refused field rolls the rows, the text table and the
/// counters back to the start of the message and records the refusal, so the sibling messages
/// are kept. Any other error (a capacity overrun) passes through and refuses the document.
fn guarded(
    acc: &mut Acc,
    c: &mut Ctx<'_>,
    f: impl FnOnce(&mut Acc, &mut Ctx<'_>) -> Result<(), DocError>,
) -> Result<(), DocError> {
    let m = mark(acc, c);
    match f(acc, c) {
        Ok(()) => Ok(()),
        Err(DocError::Field(fe)) => {
            truncate_rows(c.rows, m.flights, m.flows, m.strings);
            acc.unknown = m.unknown;
            acc.ignored = m.ignored;
            c.unknown_words = m.words;
            c.unknown_units = m.units;
            c.dropped = m.dropped;
            acc.rejected = acc.rejected.saturating_add(1);
            acc.first_reject.get_or_insert(fe);
            if let Some(x) = acc.reject_fields.get_mut(fe.field as usize) {
                *x = x.saturating_add(1);
            }
            if let Some(x) = acc.reject_kinds.get_mut(error_kind_index(fe.kind)) {
                *x = x.saturating_add(1);
            }
            Ok(())
        }
        r => r,
    }
}

fn reset(c: &mut Ctx<'_>) {
    c.seen = 0;
    c.lat = Dms::default();
    c.lon = Dms::default();
}

/// Converts the collected degrees, minutes and seconds into `lat_e7` and `lon_e7`.
fn finish_position(c: &mut Ctx<'_>, at: u32) -> Result<(), DocError> {
    if c.lat.have == 0 && c.lon.have == 0 {
        return Ok(());
    }
    let need = 1 | 2 | 8;
    if c.lat.have & need != need || c.lon.have & need != need {
        return Err(ferr(Field::Position, FieldErrorKind::Missing, at));
    }
    let (la, lo) = (c.lat, c.lon);
    c.f.lat_e7 =
        dms_e7(la.deg, la.min, la.sec, la.neg, 90).map_err(|k| ferr(Field::Position, k, at))?;
    c.f.lon_e7 =
        dms_e7(lo.deg, lo.min, lo.sec, lo.neg, 180).map_err(|k| ferr(Field::Position, k, at))?;
    Ok(())
}

/// The identity every flight row needs: an aircraft identifier, a flight reference and a time.
fn require_flight(c: &Ctx<'_>, at: u32, need_time: bool) -> Result<(), DocError> {
    if c.f.acid == [0; 8] {
        return Err(ferr(Field::Acid, FieldErrorKind::Missing, at));
    }
    if c.seen & (1u128 << (Field::FlightRef as u32)) == 0 {
        return Err(ferr(Field::FlightRef, FieldErrorKind::Missing, at));
    }
    if need_time && c.f.src_ms == NO_TIME {
        return Err(ferr(Field::SrcTime, FieldErrorKind::Missing, at));
    }
    Ok(())
}

fn push_staged_flight(c: &mut Ctx<'_>) -> Result<(), DocError> {
    let row = c.f;
    if push_flight(&mut c.rows.flights, &row) {
        Ok(())
    } else if c.prefix {
        c.dropped = c.dropped.saturating_add(1);
        Ok(())
    } else {
        Err(DocError::Capacity(CapKind::Flights))
    }
}

fn decode_fltd_message(
    doc: &Doc<'_>,
    acc: &mut Acc,
    c: &mut Ctx<'_>,
    m: ElemIx,
) -> Result<(), DocError> {
    c.f = BLANK_FLIGHT;
    c.flow = false;
    reset(c);
    let at = elem_at(doc, m);
    let mut buf = [0u8; MAX_VALUE];
    let mt = msg_type(doc, m, &mut buf)?;
    let body = BODIES.iter().find(|b| b.msg_type == mt);
    c.f.kind = body.map_or(kind::OTHER, |b| b.kind);
    enter(doc, acc, c, m, &FLTD_MESSAGE)?;
    require_flight(c, at, true)?;
    let mut ch = first_child(doc, m);
    while ch != ElemIx::NONE {
        match body {
            Some(b) if elem_name(doc, ch) == b.el.name && in_ns(doc, ch, b.el.ns) => {
                walk(doc, acc, c, ch, &b.el)?;
            }
            _ => acc.unknown += 1,
        }
        ch = next_sibling(doc, ch);
    }
    finish_position(c, at)?;
    push_staged_flight(c)
}

/// The program of an advisory from the words of its title.
fn advisory_program(title: &[u8]) -> (u8, bool) {
    let p = if contains_word(title, b"GROUND DELAY PROGRAM") || contains_word(title, b"GDP") {
        prog::GROUND_DELAY
    } else if contains_word(title, b"GROUND STOP") || contains_word(title, b"GS") {
        prog::GROUND_STOP
    } else if contains_word(title, b"AIRSPACE FLOW PROGRAM") || contains_word(title, b"AFP") {
        prog::AIRSPACE_FLOW
    } else if contains_word(title, b"REROUTE") {
        prog::REROUTE
    } else {
        prog::ADVISORY
    };
    let cancel = contains_word(title, b"CNX")
        || contains_word(title, b"CANCELLATION")
        || contains_word(title, b"CANCELLED");
    (p, cancel)
}

fn decode_flow_message(
    doc: &Doc<'_>,
    acc: &mut Acc,
    c: &mut Ctx<'_>,
    m: ElemIx,
    fb: &'static crate::schema::FlowBody,
) -> Result<(), DocError> {
    c.w = BLANK_FLOW;
    c.w.kind = fb.kind;
    c.flow = true;
    reset(c);
    let at = elem_at(doc, m);
    enter(doc, acc, c, m, &FI_MESSAGE)?;
    if c.w.src_ms == NO_TIME {
        return Err(ferr(Field::SrcTime, FieldErrorKind::Missing, at));
    }
    if fb.kind == fkind::APTC {
        c.w.program = prog::AIRPORT_CONFIG;
    }
    let mut ch = first_child(doc, m);
    while ch != ElemIx::NONE {
        if elem_name(doc, ch) == fb.el.name && in_ns(doc, ch, fb.el.ns) {
            walk(doc, acc, c, ch, &fb.el)?;
        } else {
            acc.unknown += 1;
        }
        ch = next_sibling(doc, ch);
    }
    if fb.kind == fkind::GADV {
        let title = text(c.rows, c.w.title).as_bytes();
        let (p, cancel) = advisory_program(title);
        c.w.program = p;
        if cancel {
            c.w.flags |= crate::rows::fflag::CANCEL;
        }
    }
    let row = c.w;
    if push_flow(&mut c.rows.flows, &row) {
        Ok(())
    } else if c.prefix {
        c.dropped = c.dropped.saturating_add(1);
        Ok(())
    } else {
        Err(DocError::Capacity(CapKind::Flows))
    }
}

fn decode_tmi_list(
    doc: &Doc<'_>,
    acc: &mut Acc,
    c: &mut Ctx<'_>,
    m: ElemIx,
) -> Result<(), DocError> {
    c.f = BLANK_FLIGHT;
    c.flow = false;
    reset(c);
    let at = elem_at(doc, m);
    enter(doc, acc, c, m, &FI_MESSAGE)?;
    if c.f.src_ms == NO_TIME {
        return Err(ferr(Field::SrcTime, FieldErrorKind::Missing, at));
    }
    let base = c.f;
    let mut ch = first_child(doc, m);
    while ch != ElemIx::NONE {
        if elem_name(doc, ch) == TMI_LIST_BODY && in_ns(doc, ch, Nsx::Fi) {
            let mut fd = first_child(doc, ch);
            while fd != ElemIx::NONE {
                if elem_name(doc, fd) == FLIGHT_DATA.name && in_ns(doc, fd, FLIGHT_DATA.ns) {
                    // One entry is one message of the list: a refused entry is rolled back
                    // alone and the others stay.
                    guarded(acc, c, |acc, c| {
                        c.f = base;
                        c.f.kind = kind::TMI_LIST;
                        reset(c);
                        walk(doc, acc, c, fd, &FLIGHT_DATA)?;
                        require_flight(c, elem_at(doc, fd), false)?;
                        push_staged_flight(c)
                    })?;
                } else {
                    acc.unknown += 1;
                }
                fd = next_sibling(doc, fd);
            }
        } else {
            acc.unknown += 1;
        }
        ch = next_sibling(doc, ch);
    }
    Ok(())
}

fn decode_fi_message(
    doc: &Doc<'_>,
    acc: &mut Acc,
    c: &mut Ctx<'_>,
    m: ElemIx,
) -> Result<(), DocError> {
    let mut buf = [0u8; MAX_VALUE];
    let mt = msg_type(doc, m, &mut buf)?;
    if mt == TMI_LIST_TYPE {
        return decode_tmi_list(doc, acc, c, m);
    }
    match FLOW_BODIES.iter().find(|b| b.msg_type == mt) {
        Some(fb) => decode_flow_message(doc, acc, c, m, fb),
        None => {
            acc.unknown += 1;
            Ok(())
        }
    }
}

// ---- the document --------------------------------------------------------------------------

fn merge(old: Option<DocKind>, new: DocKind) -> DocKind {
    match old {
        None => new,
        Some(k) if k == new => k,
        Some(_) => DocKind::Mixed,
    }
}

fn decode_root(
    doc: &Doc<'_>,
    acc: &mut Acc,
    rows: &mut Rows,
    prefix: bool,
) -> Result<Outcome, DocError> {
    let root = ElemIx(0);
    let at = elem_at(doc, root);
    if doc.elem_name.is_empty()
        || elem_name(doc, root) != b"tfmDataService"
        || !in_ns(doc, root, Nsx::Svc)
    {
        return Err(DocError::NotTfms { at });
    }
    for i in 0..attr_count(doc, root) {
        let a = attr_ref(doc, root, i);
        if !a.decl && !a.schema_hint {
            acc.unknown += 1;
        }
    }
    let f0 = flights_len(&rows.flights);
    let w0 = flows_len(&rows.flows);
    let mut c = Ctx {
        rows,
        f: BLANK_FLIGHT,
        w: BLANK_FLOW,
        flow: false,
        seen: 0,
        lat: Dms::default(),
        lon: Dms::default(),
        prefix,
        dropped: 0,
        unknown_words: 0,
        unknown_units: 0,
    };
    let mut kind = None;
    let mut messages = 0u32;
    let mut out = first_child(doc, root);
    while out != ElemIx::NONE {
        let name = elem_name(doc, out);
        let svc = in_ns(doc, out, Nsx::Svc);
        if svc && name == b"fltdOutput" {
            kind = Some(merge(kind, DocKind::Flight));
            let mut m = first_child(doc, out);
            while m != ElemIx::NONE {
                if elem_name(doc, m) == FLTD_MESSAGE.name && in_ns(doc, m, Nsx::Fd) {
                    messages += 1;
                    guarded(acc, &mut c, |acc, c| decode_fltd_message(doc, acc, c, m))?;
                } else {
                    acc.unknown += 1;
                }
                m = next_sibling(doc, m);
            }
        } else if svc && name == b"fiOutput" {
            kind = Some(merge(kind, DocKind::FlowInfo));
            let mut m = first_child(doc, out);
            while m != ElemIx::NONE {
                if elem_name(doc, m) == FI_MESSAGE.name && in_ns(doc, m, Nsx::Fi) {
                    messages += 1;
                    guarded(acc, &mut c, |acc, c| decode_fi_message(doc, acc, c, m))?;
                } else {
                    acc.unknown += 1;
                }
                m = next_sibling(doc, m);
            }
        } else if svc && name == b"tfmsStatusOutput" {
            kind = Some(merge(kind, DocKind::Status));
            messages += 1;
            acc.ignored += 1;
        } else {
            acc.unknown += 1;
        }
        out = next_sibling(doc, out);
    }
    let f1 = flights_len(&c.rows.flights);
    let w1 = flows_len(&c.rows.flows);
    Ok(Outcome {
        kind: kind.unwrap_or(DocKind::Empty),
        messages,
        flight_rows: u32::try_from(f1 - f0).unwrap_or(u32::MAX),
        flow_rows: u32::try_from(w1 - w0).unwrap_or(u32::MAX),
        unknown_paths: acc.unknown,
        ignored_paths: acc.ignored,
        dropped: c.dropped,
        rejected: acc.rejected,
        first_reject: acc.first_reject,
        reject_fields: acc.reject_fields,
        reject_kinds: acc.reject_kinds,
        unknown_words: c.unknown_words,
        unknown_units: c.unknown_units,
        sealed_dropped: 0,
    })
}
