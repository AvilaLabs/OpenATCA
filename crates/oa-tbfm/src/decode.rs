//! The decoder: a parsed [`Doc`] in, metering rows out. Core (pure): no I/O, no clock, no
//! allocation (the output columns were reserved by [`crate::new_rows`]).
//!
//! * One row per `flt`, `mrp`, `eta`, `sta` or `sch` section of each `air` (see
//!   [`crate::Row`]). `adp`, `con` and `oth` messages (adaptation, rates and configurations,
//!   interface status) produce no rows; they are classified ([`MsgKind`]) and counted.
//! * All or nothing per flight, not per document: an `air` with a refused field (a repeated
//!   leaf, a bad number, a missing attribute) is rolled back to nothing and counted in
//!   [`Outcome::rejected`] with its reason, and the other `air` elements of the message are kept;
//!   a `tma` whose own attributes are refused goes the same way. Only a message whose
//!   structure is unreadable (wrong root, bad envelope attributes) or that overruns the
//!   capacity is refused whole, leaving the rows exactly as they were (RULESET E5, INTERFACES
//!   IF-R5).
//! * A schedule flag (`man`, `rfz`, `sfz`, `sus`) with a word other than `YES` or `NO` does not
//!   refuse the flight: the flag is stored as unknown ([`crate::sch_flag::UNKNOWN`]) and counted in
//!   [`Outcome::unknown_words`]. A speed whose form is neither knots nor Mach is stored with
//!   unit [`crate::spd_unit::UNKNOWN`] and counted in [`Outcome::unknown_units`].
//! * Text split by a comment or a CDATA section is joined (XML 1.0) before it is validated, and
//!   whitespace around a value is trimmed. The site-adapted labels (`gat`, `cfg`, `scn`, `cat`)
//!   may hold interior spaces; identifiers and fixes may not.
//! * An element that is present but empty is recorded in [`crate::Row::cleared`] (TBFM sends
//!   changes, so "sent empty" and "not sent" differ); its value column stays absent.
//! * Strict field parsers: names are printable ASCII within their column width, numbers are
//!   plain decimal digits, times are [`crate::parse_iso_ms`], change types and schedule flags
//!   are their exact wire words. An element that is present but empty means "absent".
//! * Unknown elements, attributes and namespaces are not errors: each is counted in
//!   [`Outcome::unknown_paths`] (a whole unknown subtree counts once). Known elements the
//!   decoder deliberately does not store are counted in [`Outcome::ignored_paths`].
//! * Bounded work: the walk visits each element once, and a value is decoded into a 64-byte
//!   stack buffer (a longer decoded value is a [`FieldErrorKind::TooLong`] or `Text` error).
//!   Nothing recurses; the tree is walked at the fixed depth of the schema.

use crate::fields::{
    error_kind_index, DocError, Field, FieldError, FieldErrorKind, MsgKind, ERROR_KIND_COUNT,
    FIELD_COUNT,
};
use crate::rows::{
    change, pt, push_row, rows_len, sch_flag, section, spd_unit, truncate_rows, Name12, Name14,
    Name16, Name20, Name24, Name4, Name8, Row, Rows, BLANK_ROW, NO_TIME,
};
use crate::time::parse_iso_ms;
use oa_sealed::DropWindow;
use oa_xml::{
    attr_needs_decode, decode_into, decode_text_run, ns_uri_bytes, resolve_elem_ns, span_bytes,
    text_needs_decode, Doc, ElemIx, Limits, Ns, Parser, Span, SpanKind,
};

/// The TBFM metering publication namespace the sampled feed declares.
pub const NAMESPACE: &[u8] = b"urn:us:gov:dot:faa:atm:tfm:tbfmmeteringpublication:1.1.0";

/// Largest decoded value the decoder handles. The longest stored value in the survey is 24
/// bytes (`cat`) and a timestamp is 24; the free-text fields (`a10`, `tcr`) are never read.
const MAX_VALUE: usize = 64;

/// What one accepted message was.
// Budget: returned once per message, never stored in quantity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub kind: MsgKind,
    /// Rows appended to the caller's [`Rows`].
    pub rows: u32,
    pub unknown_paths: u32,
    pub ignored_paths: u32,
    /// Centre identifier of the envelope (`envSrce` or `misSrce`).
    pub center: Name4,
    /// Envelope time (`envTime` or `misTime`), Unix ms.
    pub env_ms: i64,
    /// Rows read but not appended because `Rows` was full ([`decode_doc_prefix`] only; always 0
    /// from [`decode_doc`]).
    pub dropped: u32,
    /// `air` (or `tma`) elements refused for a field and rolled back; they add no rows.
    pub rejected: u32,
    /// The first refused field of the message, with its reason and offset.
    pub first_reject: Option<FieldError>,
    /// Refusals by [`Field`] (index `field as usize`) and by reason ([`error_kind_index`]); each
    /// sums to `rejected`.
    pub reject_fields: [u32; FIELD_COUNT],
    pub reject_kinds: [u32; ERROR_KIND_COUNT],
    /// Schedule-flag words kept as unknown instead of refusing the flight.
    pub unknown_words: u32,
    /// Speeds whose form is neither knots nor Mach, stored with the unknown unit.
    pub unknown_units: u32,
    /// Messages (`tma`) of a document dropped whole because one of them has its `msgTime` in
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

/// A parser with the library's default limits, which the survey shows are ample.
pub fn default_limits() -> Limits {
    Limits::default()
}

/// Decodes one parsed message into `rows`. On `Err` the rows are unchanged.
pub fn decode_doc(doc: &Doc<'_>, rows: &mut Rows) -> Result<Outcome, DocError> {
    decode_with(doc, rows, false)
}

/// Like [`decode_doc`], but a document with more rows than `rows` has room for is decoded as far
/// as it fits instead of being refused: the rows that fit are appended and [`Outcome::dropped`]
/// counts the others. Every other error still leaves `rows` as it was.
pub fn decode_doc_prefix(doc: &Doc<'_>, rows: &mut Rows) -> Result<Outcome, DocError> {
    decode_with(doc, rows, true)
}

/// Reads the `msgTime` of every `tma` message of the document, directly from the parsed tree and
/// whatever else is valid in it. If any is in `window`, returns how many messages the document
/// has (all are dropped); otherwise 0. A time that cannot be read is not in the window (the
/// frame's receive time gates it).
fn prescan(doc: &Doc<'_>, window: DropWindow) -> u32 {
    if window.span().is_none() {
        return 0;
    }
    let (mut messages, mut hit) = (0u32, false);
    for i in 0..doc.elem_name.len() {
        let e = ElemIx(i as u32);
        if elem_name(doc, e) != b"tma" {
            continue;
        }
        messages = messages.saturating_add(1);
        let n = doc.elem_attr_count.get(i).copied().unwrap_or(0);
        for k in 0..n {
            let a = attr_ref(doc, e, k);
            let mut buf = [0u8; MAX_VALUE];
            if a.name == b"msgTime" {
                if let Ok(v) = attr_val(doc, &a, Field::MsgTime, &mut buf) {
                    hit |= parse_time(v).is_ok_and(|t| window.contains(t));
                }
            }
        }
    }
    if hit {
        messages
    } else {
        0
    }
}

/// The outcome of a document dropped whole by [`prescan`]: only the count, nothing else.
fn sealed_outcome(messages: u32) -> Outcome {
    Outcome {
        kind: MsgKind::Empty,
        rows: 0,
        unknown_paths: 0,
        ignored_paths: 0,
        center: [0; 4],
        env_ms: NO_TIME,
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
    // The first step, before any count, row or table is touched: a document with a message in
    // the sealed window is dropped whole.
    let gone = prescan(doc, rows.window);
    if gone > 0 {
        return Ok(sealed_outcome(gone));
    }
    let start = rows_len(rows);
    let mut acc = Acc {
        t: 0,
        unknown: 0,
        ignored: 0,
        prefix,
        dropped: 0,
        rejected: 0,
        first_reject: None,
        reject_fields: [0; FIELD_COUNT],
        reject_kinds: [0; ERROR_KIND_COUNT],
        unknown_words: 0,
        unknown_units: 0,
    };
    let r = decode_root(doc, &mut acc, rows);
    if r.is_err() {
        truncate_rows(rows, start);
    }
    r
}

/// Running state of one decode: the text cursor and the path counters.
struct Acc {
    t: usize,
    unknown: u32,
    ignored: u32,
    /// Count rows that do not fit instead of failing.
    prefix: bool,
    dropped: u32,
    rejected: u32,
    first_reject: Option<FieldError>,
    reject_fields: [u32; FIELD_COUNT],
    reject_kinds: [u32; ERROR_KIND_COUNT],
    unknown_words: u32,
    unknown_units: u32,
}

/// Runs `f` (the decode of one flight or one `tma`); a refused field rolls the rows and the
/// counters back to where the element started and records the refusal, so its siblings are
/// kept. Any other error (a capacity overrun) passes through and refuses the message. `None`
/// means the element was refused.
fn guarded<T>(
    acc: &mut Acc,
    rows: &mut Rows,
    f: impl FnOnce(&mut Acc, &mut Rows) -> Result<T, DocError>,
) -> Result<Option<T>, DocError> {
    let start = rows_len(rows);
    let (unknown, ignored, dropped, words, units) = (
        acc.unknown,
        acc.ignored,
        acc.dropped,
        acc.unknown_words,
        acc.unknown_units,
    );
    match f(acc, rows) {
        Ok(v) => Ok(Some(v)),
        Err(DocError::Field(fe)) => {
            truncate_rows(rows, start);
            acc.unknown = unknown;
            acc.ignored = ignored;
            acc.dropped = dropped;
            acc.unknown_words = words;
            acc.unknown_units = units;
            acc.rejected = acc.rejected.saturating_add(1);
            acc.first_reject.get_or_insert(fe);
            if let Some(x) = acc.reject_fields.get_mut(fe.field as usize) {
                *x = x.saturating_add(1);
            }
            if let Some(x) = acc.reject_kinds.get_mut(error_kind_index(fe.kind)) {
                *x = x.saturating_add(1);
            }
            Ok(None)
        }
        Err(e) => Err(e),
    }
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

fn in_tbfm(doc: &Doc<'_>, e: ElemIx) -> bool {
    match resolve_elem_ns(doc, e) {
        Ns::Uri(s) => ns_uri_bytes(doc, s).is_some_and(|b| b == NAMESPACE),
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

/// An attribute of an element: its name (unprefixed only; a prefixed attribute has an empty
/// name here, so it matches nothing and is counted unknown by the caller) and its value span.
/// The value is decoded only for attributes the caller reads ([`attr_val`]).
struct AttrRef<'a> {
    name: &'a [u8],
    span: Span,
    flagged: bool,
    /// A namespace declaration (`xmlns` or `xmlns:p`), which is never data.
    decl: bool,
}

fn attr_ref<'a>(doc: &Doc<'a>, e: ElemIx, i: u32) -> AttrRef<'a> {
    let a = oa_xml::AttrIx(
        doc.elem_first_attr
            .get(e.0 as usize)
            .map_or(0, |f| f.0)
            .saturating_add(i),
    );
    let ai = a.0 as usize;
    let prefix = doc
        .attr_prefix
        .get(ai)
        .and_then(|&p| span_bytes(doc.input, p).ok())
        .unwrap_or(&[]);
    let prefixed = !prefix.is_empty();
    let name_raw = doc
        .attr_name
        .get(ai)
        .and_then(|&s| span_bytes(doc.input, s).ok())
        .unwrap_or(&[]);
    let decl = prefix == b"xmlns" || (!prefixed && name_raw == b"xmlns");
    let name = if prefixed {
        &[][..]
    } else {
        doc.attr_name
            .get(ai)
            .and_then(|&s| span_bytes(doc.input, s).ok())
            .unwrap_or(&[])
    };
    AttrRef {
        name,
        span: doc.attr_value.get(ai).copied().unwrap_or_default(),
        flagged: attr_needs_decode(doc, a),
        decl,
    }
}

fn attr_val<'x>(
    doc: &Doc<'x>,
    a: &AttrRef<'_>,
    field: Field,
    buf: &'x mut [u8; MAX_VALUE],
) -> Result<&'x [u8], DocError> {
    value_bytes(doc, a.span, a.flagged, SpanKind::AttrValue, buf)
        .map(trim)
        .map_err(|k| ferr(field, k, a.span.start))
}

/// The value without the whitespace (space, tab, line ends) around it.
fn trim(v: &[u8]) -> &[u8] {
    let is_ws = |b: &u8| matches!(b, b' ' | b'\t' | b'\n' | b'\r');
    let start = v.iter().position(|b| !is_ws(b)).unwrap_or(v.len());
    let end = v.iter().rposition(|b| !is_ws(b)).map_or(start, |i| i + 1);
    v.get(start..end).unwrap_or(&[])
}

fn value_bytes<'x>(
    doc: &Doc<'x>,
    span: Span,
    needs_decode: bool,
    kind: SpanKind,
    buf: &'x mut [u8; MAX_VALUE],
) -> Result<&'x [u8], FieldErrorKind> {
    if !needs_decode {
        return span_bytes(doc.input, span).map_err(|_| FieldErrorKind::Text);
    }
    match decode_into(span, doc.input, buf, kind) {
        Ok(n) => buf.get(..n).ok_or(FieldErrorKind::Text),
        Err(oa_xml::XmlError::OutputTooSmall { .. }) => Err(FieldErrorKind::TooLong),
        Err(_) => Err(FieldErrorKind::Text),
    }
}

/// The text of leaf element `e` (whitespace-trimmed), `""` when it has none. Elements must be asked for in document
/// order; the cursor in `acc` skips the text nodes of everything visited before.
fn leaf_text<'x>(
    doc: &Doc<'x>,
    acc: &mut Acc,
    e: ElemIx,
    field: Field,
    buf: &'x mut [u8; MAX_VALUE],
) -> Result<&'x [u8], DocError> {
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
    let span = doc.text_span.get(first).copied().unwrap_or_default();
    let v = match count {
        0 => return Ok(&[]),
        1 => {
            let flagged = text_needs_decode(doc, oa_xml::TextIx(first as u32));
            value_bytes(doc, span, flagged, SpanKind::Text, buf)
        }
        // A comment or CDATA section split the text: the pieces are one value.
        _ => match decode_text_run(doc, first, count, buf) {
            Ok(n) => buf.get(..n).ok_or(FieldErrorKind::Text),
            Err(oa_xml::XmlError::OutputTooSmall { .. }) => Err(FieldErrorKind::TooLong),
            Err(_) => Err(FieldErrorKind::Text),
        },
    };
    v.map(trim).map_err(|k| ferr(field, k, span.start))
}

// ---- field parsers -------------------------------------------------------------------------

fn parse_name<const N: usize>(v: &[u8]) -> Result<[u8; N], FieldErrorKind> {
    let mut out = [0u8; N];
    if v.len() > N {
        return Err(FieldErrorKind::TooLong);
    }
    for (d, &b) in out.iter_mut().zip(v) {
        if !(0x21..=0x7e).contains(&b) {
            return Err(FieldErrorKind::BadChar);
        }
        *d = b;
    }
    Ok(out)
}

/// Plain decimal digits, at most `max_len` of them (ten for a message number, which does not
/// fit `u32`).
fn parse_digits(v: &[u8], max_len: usize) -> Result<u64, FieldErrorKind> {
    if v.is_empty() || v.len() > max_len || !v.iter().all(u8::is_ascii_digit) {
        return Err(FieldErrorKind::Format);
    }
    let mut n = 0u64;
    for &b in v {
        n = n
            .checked_mul(10)
            .and_then(|n| n.checked_add(u64::from(b - b'0')))
            .ok_or(FieldErrorKind::Format)?;
    }
    Ok(n)
}

/// A speed: `digits{1,5}` with an optional `.digits{1,3}`, in thousandths, with its unit.
///
/// The feed has one `spd` element for two units and no marker (no attribute, no sibling; the
/// survey and the recorded sample are the evidence, docs/TBFM_SURVEY.md): knots arrive as
/// `ddd.d` (140.0 to 508.0 in the sample, piston and turboprop aircraft included) and Mach
/// numbers as `d.dd` (0.77 to 0.85, jets only). The unit is read from that form: two to four
/// integer digits with one decimal is knots, one integer digit with two decimals is Mach, and
/// any other form is [`spd_unit::UNKNOWN`] (the value is kept, the unit is not guessed).
fn parse_speed(v: &[u8]) -> Result<(i32, u8), FieldErrorKind> {
    let milli = parse_milli(v)?;
    let (int, frac) = match v.iter().position(|&b| b == b'.') {
        Some(i) => (i, v.len() - i - 1),
        None => (v.len(), 0),
    };
    let unit = match (int, frac) {
        (2..=4, 1) => spd_unit::KNOTS,
        (1, 2) => spd_unit::MACH,
        _ => spd_unit::UNKNOWN,
    };
    Ok((milli, unit))
}

/// `digits{1,5}` with an optional `.digits{1,3}`, in thousandths.
fn parse_milli(v: &[u8]) -> Result<i32, FieldErrorKind> {
    let (int, frac) = match v.iter().position(|&b| b == b'.') {
        Some(i) => (v.get(..i).unwrap_or(&[]), v.get(i + 1..).unwrap_or(&[])),
        None => (v, &[][..]),
    };
    let dot = int.len() != v.len();
    if dot && (frac.is_empty() || frac.len() > 3) {
        return Err(FieldErrorKind::Format);
    }
    let i = parse_digits(int, 5)?;
    let f = if dot {
        parse_digits(frac, 3)? * 10u64.pow(3 - frac.len() as u32)
    } else {
        0
    };
    i32::try_from(i * 1000 + f).map_err(|_| FieldErrorKind::Format)
}

/// A site-adapted label: printable ASCII with interior spaces allowed (the value was trimmed).
fn parse_label<const N: usize>(v: &[u8]) -> Result<[u8; N], FieldErrorKind> {
    let mut out = [0u8; N];
    if v.len() > N {
        return Err(FieldErrorKind::TooLong);
    }
    for (d, &b) in out.iter_mut().zip(v) {
        if !(0x20..=0x7e).contains(&b) {
            return Err(FieldErrorKind::BadChar);
        }
        *d = b;
    }
    Ok(out)
}

fn parse_bcn(v: &[u8]) -> Result<u16, FieldErrorKind> {
    if v.len() != 4 || !v.iter().all(|b| (b'0'..=b'7').contains(b)) {
        return Err(FieldErrorKind::Format);
    }
    Ok(v.iter().fold(0u16, |a, &b| a * 8 + u16::from(b - b'0')))
}

fn parse_time(v: &[u8]) -> Result<i64, FieldErrorKind> {
    parse_iso_ms(v).map_err(FieldErrorKind::Time)
}

fn parse_change(v: &[u8]) -> Result<u8, FieldErrorKind> {
    match v {
        b"NEW" => Ok(change::NEW),
        b"AMD" => Ok(change::AMD),
        b"DEL" => Ok(change::DEL),
        _ => Err(FieldErrorKind::Vocabulary),
    }
}

/// `TMA.<centre>.FAA.GOV` with a 2 to 4 character upper-case alphanumeric centre.
fn parse_center(v: &[u8]) -> Result<Name4, FieldErrorKind> {
    let mid = v
        .strip_prefix(b"TMA.")
        .and_then(|r| r.strip_suffix(b".FAA.GOV"))
        .ok_or(FieldErrorKind::Format)?;
    if mid.len() < 2
        || !mid
            .iter()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    {
        return Err(FieldErrorKind::Format);
    }
    parse_name::<4>(mid).map_err(|_| FieldErrorKind::Format)
}

// ---- section tables ------------------------------------------------------------------------

/// How a leaf's text is stored in a [`Row`]: a function giving the column to write, per type.
#[derive(Clone, Copy)]
enum Slot {
    N4(fn(&mut Row) -> &mut Name4),
    N8(fn(&mut Row) -> &mut Name8),
    N12(fn(&mut Row) -> &mut Name12),
    N14(fn(&mut Row) -> &mut Name14),
    /// Labels adapted per site, which may hold interior spaces.
    L16(fn(&mut Row) -> &mut Name16),
    L20(fn(&mut Row) -> &mut Name20),
    L24(fn(&mut Row) -> &mut Name24),
    /// A name that repeats an `air` attribute: equal to it, or sets it when `air` lacks it.
    Same4(fn(&mut Row) -> &mut Name4),
    Same8(fn(&mut Row) -> &mut Name8),
    Time(fn(&mut Row) -> &mut i64),
    /// Index into `eta_ms`.
    Eta(usize),
    /// Index into `sta_ms`.
    Sta(usize),
    Bcn,
    Speed,
    Altitude,
    Flag(u8),
    /// Known, not stored: counted in `ignored_paths`.
    Skip,
}

struct Ent {
    name: &'static [u8],
    field: Field,
    slot: Slot,
}

const fn ent(name: &'static [u8], field: Field, slot: Slot) -> Ent {
    Ent { name, field, slot }
}

const FLT: &[Ent] = &[
    ent(b"aid", Field::Aid, Slot::Same8(|r| &mut r.aid)),
    ent(b"apt", Field::Apt, Slot::Same4(|r| &mut r.apt)),
    ent(b"dap", Field::Dap, Slot::Same4(|r| &mut r.dap)),
    ent(b"fps", Field::Fps, Slot::N12(|r| &mut r.fps)),
    ent(b"acs", Field::Acs, Slot::N12(|r| &mut r.acs)),
    ent(b"typ", Field::Typ, Slot::N8(|r| &mut r.typ)),
    ent(b"eng", Field::Eng, Slot::N12(|r| &mut r.eng)),
    ent(b"bcn", Field::Bcn, Slot::Bcn),
    ent(b"spd", Field::Spd, Slot::Speed),
    ent(b"ara", Field::Ara, Slot::N8(|r| &mut r.ara)),
    ent(b"ina", Field::Ina, Slot::Altitude),
    ent(b"trw", Field::Trw, Slot::N4(|r| &mut r.trw)),
    ent(b"drw", Field::Drw, Slot::N8(|r| &mut r.drw)),
    ent(b"tds", Field::Tds, Slot::N4(|r| &mut r.tds)),
    ent(b"cfx", Field::Cfx, Slot::N14(|r| &mut r.cfx)),
    ent(b"ctm", Field::Ctm, Slot::Time(|r| &mut r.ctm_ms)),
    ent(b"etd", Field::Etd, Slot::Time(|r| &mut r.etd_ms)),
    ent(b"std", Field::Std, Slot::Time(|r| &mut r.std_ms)),
    // Free text (route string, crossing list) and two fields never seen with a value.
    ent(b"a10", Field::Aid, Slot::Skip),
    ent(b"tcr", Field::Aid, Slot::Skip),
    ent(b"est", Field::Aid, Slot::Skip),
    ent(b"etm", Field::Aid, Slot::Skip),
];

const MRP: &[Ent] = &[
    ent(b"tra", Field::Tra, Slot::N4(|r| &mut r.tra)),
    ent(b"mfx", Field::Mfx, Slot::N8(|r| &mut r.mfx)),
    ent(b"dfx", Field::Dfx, Slot::N8(|r| &mut r.dfx)),
    ent(b"sfx", Field::Sfx, Slot::N8(|r| &mut r.sfx)),
    ent(b"oma", Field::Oma, Slot::N12(|r| &mut r.oma)),
    ent(b"ooa", Field::Ooa, Slot::N12(|r| &mut r.ooa)),
    ent(b"o3a", Field::O3a, Slot::N12(|r| &mut r.o3a)),
    ent(b"o4a", Field::O4a, Slot::N12(|r| &mut r.o4a)),
    ent(b"gat", Field::Gat, Slot::L16(|r| &mut r.gat)),
    ent(b"rwy", Field::Rwy, Slot::N4(|r| &mut r.rwy)),
    ent(b"cfg", Field::Cfg, Slot::L20(|r| &mut r.cfg)),
    ent(b"cat", Field::Cat, Slot::L24(|r| &mut r.cat)),
    ent(b"scn", Field::Scn, Slot::L20(|r| &mut r.scn)),
];

const SCH: &[Ent] = &[
    ent(b"mfx", Field::Mfx, Slot::N8(|r| &mut r.mfx)),
    ent(b"man", Field::Man, Slot::Flag(sch_flag::MAN)),
    ent(b"rfz", Field::Rfz, Slot::Flag(sch_flag::RFZ)),
    ent(b"sfz", Field::Sfz, Slot::Flag(sch_flag::SFZ)),
    ent(b"sus", Field::Sus, Slot::Flag(sch_flag::SUS)),
];

const ETA: &[Ent] = &[
    ent(b"mfx", Field::Mfx, Slot::N8(|r| &mut r.mfx)),
    ent(b"eta_dfx", Field::EtaDfx, Slot::Eta(pt::DFX)),
    ent(b"eta_sfx", Field::EtaSfx, Slot::Eta(pt::SFX)),
    ent(b"eta_mfx", Field::EtaMfx, Slot::Eta(pt::MFX)),
    ent(b"eta_oma", Field::EtaOma, Slot::Eta(pt::OMA)),
    ent(b"eta_ooa", Field::EtaOoa, Slot::Eta(pt::OOA)),
    ent(b"eta_o3a", Field::EtaO3a, Slot::Eta(pt::O3A)),
    ent(b"eta_o4a", Field::EtaO4a, Slot::Eta(pt::O4A)),
    ent(b"eta_rwy", Field::EtaRwy, Slot::Eta(pt::RWY)),
];

const STA: &[Ent] = &[
    ent(b"mfx", Field::Mfx, Slot::N8(|r| &mut r.mfx)),
    ent(b"sta_dfx", Field::StaDfx, Slot::Sta(pt::DFX)),
    ent(b"sta_sfx", Field::StaSfx, Slot::Sta(pt::SFX)),
    ent(b"sta_oma", Field::StaOma, Slot::Sta(pt::OMA)),
    ent(b"sta_ooa", Field::StaOoa, Slot::Sta(pt::OOA)),
    ent(b"sta_o3a", Field::StaO3a, Slot::Sta(pt::O3A)),
    ent(b"sta_o4a", Field::StaO4a, Slot::Sta(pt::O4A)),
    ent(b"sta_rwy", Field::StaRwy, Slot::Sta(pt::RWY)),
];

/// Stores `v` through `slot`. `Ok(Stored::Unknown)` is a schedule flag with an unknown word,
/// kept as unknown; `Ok(Stored::UnknownUnit)` a speed of unknown unit.
fn store(slot: Slot, row: &mut Row, v: &[u8]) -> Result<Stored, FieldErrorKind> {
    let mut out = Stored::Plain;
    match slot {
        Slot::N4(f) => *f(row) = parse_name(v)?,
        Slot::N8(f) => *f(row) = parse_name(v)?,
        Slot::N12(f) => *f(row) = parse_name(v)?,
        Slot::N14(f) => *f(row) = parse_name(v)?,
        Slot::L16(f) => *f(row) = parse_label(v)?,
        Slot::L20(f) => *f(row) = parse_label(v)?,
        Slot::L24(f) => *f(row) = parse_label(v)?,
        Slot::Same4(f) => {
            let n: Name4 = parse_name(v)?;
            let d = f(row);
            if *d == [0; 4] {
                *d = n;
            } else if *d != n {
                return Err(FieldErrorKind::Inconsistent);
            }
        }
        Slot::Same8(f) => {
            let n: Name8 = parse_name(v)?;
            let d = f(row);
            if *d == [0; 8] {
                *d = n;
            } else if *d != n {
                return Err(FieldErrorKind::Inconsistent);
            }
        }
        Slot::Time(f) => *f(row) = parse_time(v)?,
        Slot::Eta(i) => *row.eta_ms.get_mut(i).ok_or(FieldErrorKind::Format)? = parse_time(v)?,
        Slot::Sta(i) => *row.sta_ms.get_mut(i).ok_or(FieldErrorKind::Format)? = parse_time(v)?,
        Slot::Bcn => row.bcn = parse_bcn(v)?,
        Slot::Speed => {
            let (milli, unit) = parse_speed(v)?;
            row.spd_milli = milli;
            row.spd_unit = unit;
            if unit == spd_unit::UNKNOWN {
                out = Stored::UnknownUnit;
            }
        }
        Slot::Altitude => {
            row.ina = i32::try_from(parse_digits(v, 6)?).map_err(|_| FieldErrorKind::Format)?;
        }
        Slot::Flag(pos) => {
            let code = match v {
                b"NO" => sch_flag::NO,
                b"YES" => sch_flag::YES,
                _ => {
                    out = Stored::Unknown;
                    sch_flag::UNKNOWN
                }
            };
            row.sch = (row.sch & !(3 << (pos * 2))) | (code << (pos * 2));
        }
        Slot::Skip => {}
    }
    Ok(out)
}

/// What [`store`] kept besides the plain value.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stored {
    Plain,
    Unknown,
    UnknownUnit,
}

/// Reads the children of section element `e` through `table` into `row`.
fn read_section(
    doc: &Doc<'_>,
    acc: &mut Acc,
    e: ElemIx,
    table: &[Ent],
    row: &mut Row,
) -> Result<(), DocError> {
    let mut seen = 0u128;
    let mut c = first_child(doc, e);
    while c != ElemIx::NONE {
        let name = elem_name(doc, c);
        let known = if in_tbfm(doc, c) {
            table.iter().find(|t| t.name == name)
        } else {
            None
        };
        match known {
            Some(t) => {
                if matches!(t.slot, Slot::Skip) {
                    acc.ignored += 1;
                } else {
                    let bit = 1u128 << (t.field as u32);
                    let at = elem_at(doc, c);
                    if seen & bit != 0 {
                        return Err(ferr(t.field, FieldErrorKind::Duplicate, at));
                    }
                    seen |= bit;
                    let mut buf = [0u8; MAX_VALUE];
                    let v = leaf_text(doc, acc, c, t.field, &mut buf)?;
                    if v.is_empty() {
                        row.cleared |= 1u64 << (t.field as u32);
                    } else {
                        match store(t.slot, row, v).map_err(|k| ferr(t.field, k, at))? {
                            Stored::Plain => {}
                            Stored::Unknown => acc.unknown_words += 1,
                            Stored::UnknownUnit => acc.unknown_units += 1,
                        }
                    }
                }
                count_attrs(doc, acc, c);
            }
            None => acc.unknown += 1,
        }
        c = next_sibling(doc, c);
    }
    Ok(())
}

/// Attributes on a leaf are not part of the schema: each is an unknown path (namespace
/// declarations are not attributes of the schema and are not counted).
fn count_attrs(doc: &Doc<'_>, acc: &mut Acc, e: ElemIx) {
    let n = doc.elem_attr_count.get(e.0 as usize).copied().unwrap_or(0);
    for i in 0..n {
        count_unknown(acc, &attr_ref(doc, e, i));
    }
}

fn count_unknown(acc: &mut Acc, a: &AttrRef<'_>) {
    if !a.decl {
        acc.unknown += 1;
    }
}

// ---- the walk ------------------------------------------------------------------------------

fn decode_root(doc: &Doc<'_>, acc: &mut Acc, rows: &mut Rows) -> Result<Outcome, DocError> {
    let root = ElemIx(0);
    let at = elem_at(doc, root);
    if doc.elem_name.is_empty() || !in_tbfm(doc, root) {
        return Err(DocError::NotTbfm { at });
    }
    match elem_name(doc, root) {
        b"env" => decode_env(doc, acc, root, rows),
        b"mis" => decode_mis(doc, acc, root),
        _ => Err(DocError::NotTbfm { at }),
    }
}

/// The envelope attributes shared by `env` and `mis`.
struct Env {
    center: Name4,
    time_ms: i64,
}

fn read_env_attrs(
    doc: &Doc<'_>,
    acc: &mut Acc,
    e: ElemIx,
    src_name: &[u8],
    time_name: &[u8],
    ignore: &[u8],
) -> Result<Env, DocError> {
    let at = elem_at(doc, e);
    let mut center = None;
    let mut time = None;
    let n = doc.elem_attr_count.get(e.0 as usize).copied().unwrap_or(0);
    for i in 0..n {
        let a = attr_ref(doc, e, i);
        let mut buf = [0u8; MAX_VALUE];
        if a.name == src_name {
            if center.is_some() {
                return Err(ferr(
                    Field::EnvSrce,
                    FieldErrorKind::Duplicate,
                    a.span.start,
                ));
            }
            let v = attr_val(doc, &a, Field::EnvSrce, &mut buf)?;
            center = Some(parse_center(v).map_err(|k| ferr(Field::EnvSrce, k, a.span.start))?);
        } else if a.name == time_name {
            if time.is_some() {
                return Err(ferr(
                    Field::EnvTime,
                    FieldErrorKind::Duplicate,
                    a.span.start,
                ));
            }
            let v = attr_val(doc, &a, Field::EnvTime, &mut buf)?;
            time = Some(parse_time(v).map_err(|k| ferr(Field::EnvTime, k, a.span.start))?);
        } else if a.name.is_empty() || a.name != ignore {
            count_unknown(acc, &a);
        }
    }
    Ok(Env {
        center: center.ok_or_else(|| ferr(Field::EnvSrce, FieldErrorKind::Missing, at))?,
        time_ms: time.ok_or_else(|| ferr(Field::EnvTime, FieldErrorKind::Missing, at))?,
    })
}

fn decode_mis(doc: &Doc<'_>, acc: &mut Acc, root: ElemIx) -> Result<Outcome, DocError> {
    let env = read_env_attrs(doc, acc, root, b"misSrce", b"misTime", b"")?;
    let mut kind = MsgKind::MisOther;
    let mut c = first_child(doc, root);
    while c != ElemIx::NONE {
        match elem_name(doc, c) {
            b"hb" if in_tbfm(doc, c) => kind = MsgKind::Heartbeat,
            b"sync" if in_tbfm(doc, c) => kind = MsgKind::Sync,
            _ => acc.unknown += 1,
        }
        c = next_sibling(doc, c);
    }
    Ok(Outcome {
        kind,
        rows: 0,
        unknown_paths: acc.unknown,
        ignored_paths: acc.ignored,
        center: env.center,
        env_ms: env.time_ms,
        dropped: 0,
        rejected: 0,
        first_reject: None,
        reject_fields: [0; FIELD_COUNT],
        reject_kinds: [0; ERROR_KIND_COUNT],
        unknown_words: 0,
        unknown_units: 0,
        sealed_dropped: 0,
    })
}

fn decode_env(
    doc: &Doc<'_>,
    acc: &mut Acc,
    root: ElemIx,
    rows: &mut Rows,
) -> Result<Outcome, DocError> {
    let env = read_env_attrs(doc, acc, root, b"envSrce", b"envTime", b"targetNamespace")?;
    let start = rows_len(rows);
    let mut kind = MsgKind::Empty;
    let mut c = first_child(doc, root);
    while c != ElemIx::NONE {
        if in_tbfm(doc, c) && elem_name(doc, c) == b"tma" {
            if let Some(k) = guarded(acc, rows, |acc, rows| decode_tma(doc, acc, c, &env, rows))? {
                if (k as u8) < (kind as u8) || kind == MsgKind::Empty {
                    kind = k;
                }
            }
        } else {
            acc.unknown += 1;
        }
        c = next_sibling(doc, c);
    }
    Ok(Outcome {
        kind,
        rows: u32::try_from(rows_len(rows) - start).unwrap_or(u32::MAX),
        unknown_paths: acc.unknown,
        ignored_paths: acc.ignored,
        center: env.center,
        env_ms: env.time_ms,
        dropped: acc.dropped,
        rejected: acc.rejected,
        first_reject: acc.first_reject,
        reject_fields: acc.reject_fields,
        reject_kinds: acc.reject_kinds,
        unknown_words: acc.unknown_words,
        unknown_units: acc.unknown_units,
        sealed_dropped: 0,
    })
}

fn decode_tma(
    doc: &Doc<'_>,
    acc: &mut Acc,
    tma: ElemIx,
    env: &Env,
    rows: &mut Rows,
) -> Result<MsgKind, DocError> {
    let at = elem_at(doc, tma);
    let mut msg_ms = None;
    let mut msg_id = None;
    let n = doc
        .elem_attr_count
        .get(tma.0 as usize)
        .copied()
        .unwrap_or(0);
    for i in 0..n {
        let a = attr_ref(doc, tma, i);
        let mut buf = [0u8; MAX_VALUE];
        let was = match a.name {
            b"msgTime" => msg_ms.is_some(),
            b"msgId" => msg_id.is_some(),
            _ => {
                count_unknown(acc, &a);
                continue;
            }
        };
        let field = if a.name == b"msgTime" {
            Field::MsgTime
        } else {
            Field::MsgId
        };
        if was {
            return Err(ferr(field, FieldErrorKind::Duplicate, a.span.start));
        }
        let v = attr_val(doc, &a, field, &mut buf)?;
        if field == Field::MsgTime {
            msg_ms = Some(parse_time(v).map_err(|k| ferr(field, k, a.span.start))?);
        } else {
            msg_id = Some(parse_digits(v, 10).map_err(|k| ferr(field, k, a.span.start))?);
        }
    }
    let msg_ms = msg_ms.ok_or_else(|| ferr(Field::MsgTime, FieldErrorKind::Missing, at))?;
    let msg_id = msg_id.ok_or_else(|| ferr(Field::MsgId, FieldErrorKind::Missing, at))?;
    let mut base = BLANK_ROW;
    base.center = env.center;
    base.env_ms = env.time_ms;
    base.msg_ms = msg_ms;
    base.msg_id = msg_id;

    let mut kind = MsgKind::Empty;
    let mut c = first_child(doc, tma);
    while c != ElemIx::NONE {
        let this = if in_tbfm(doc, c) {
            match elem_name(doc, c) {
                b"air" => {
                    // One flight is one message: a refused `air` goes alone.
                    guarded(acc, rows, |acc, rows| decode_air(doc, acc, c, &base, rows))?;
                    Some(MsgKind::Air)
                }
                b"adp" => Some(MsgKind::Adaptation),
                b"con" => Some(MsgKind::Control),
                b"oth" => Some(MsgKind::Other),
                _ => None,
            }
        } else {
            None
        };
        match this {
            Some(k) => {
                if k != MsgKind::Air {
                    acc.ignored += 1;
                }
                if (k as u8) < (kind as u8) || kind == MsgKind::Empty {
                    kind = k;
                }
            }
            None => acc.unknown += 1,
        }
        c = next_sibling(doc, c);
    }
    Ok(kind)
}

fn decode_air(
    doc: &Doc<'_>,
    acc: &mut Acc,
    air: ElemIx,
    base: &Row,
    rows: &mut Rows,
) -> Result<(), DocError> {
    let at = elem_at(doc, air);
    let mut b = *base;
    let mut seen = 0u16;
    let n = doc
        .elem_attr_count
        .get(air.0 as usize)
        .copied()
        .unwrap_or(0);
    for i in 0..n {
        let a = attr_ref(doc, air, i);
        let (field, bit) = match a.name {
            b"gufi" => (Field::Gufi, 0),
            b"cid" => (Field::Cid, 1),
            b"tmaId" => (Field::TmaId, 2),
            b"aid" => (Field::Aid, 3),
            b"airType" => (Field::AirType, 4),
            b"apt" => (Field::Apt, 5),
            b"dap" => (Field::Dap, 6),
            _ => {
                count_unknown(acc, &a);
                continue;
            }
        };
        let at = a.span.start;
        if seen & (1 << bit) != 0 {
            return Err(ferr(field, FieldErrorKind::Duplicate, at));
        }
        seen |= 1 << bit;
        let mut buf = [0u8; MAX_VALUE];
        let v = attr_val(doc, &a, field, &mut buf)?;
        let r = match field {
            Field::Gufi => parse_name(v).map(|n| b.gufi = n),
            Field::Cid => parse_name(v).map(|n| b.cid = n),
            Field::TmaId => parse_name(v).map(|n| b.tma_id = n),
            Field::Aid => parse_name(v).map(|n| b.aid = n),
            Field::AirType => parse_change(v).map(|c| b.air_type = c),
            Field::Apt => parse_name(v).map(|n| b.apt = n),
            _ => parse_name(v).map(|n| b.dap = n),
        };
        r.map_err(|k| ferr(field, k, at))?;
    }
    // aid, tmaId and airType identify the flight and the change; everything else is optional.
    for (bit, field) in [(2, Field::TmaId), (3, Field::Aid), (4, Field::AirType)] {
        if seen & (1 << bit) == 0 {
            return Err(ferr(field, FieldErrorKind::Missing, at));
        }
    }
    let mut c = first_child(doc, air);
    while c != ElemIx::NONE {
        let sec = if in_tbfm(doc, c) {
            match elem_name(doc, c) {
                b"flt" => Some((section::FLT, FLT)),
                b"mrp" => Some((section::MRP, MRP)),
                b"eta" => Some((section::ETA, ETA)),
                b"sta" => Some((section::STA, STA)),
                b"sch" => Some((section::SCH, SCH)),
                _ => None,
            }
        } else {
            None
        };
        match sec {
            Some((code, table)) => {
                let mut row = b;
                row.section = code;
                if code == section::MRP {
                    row.sect_type = read_mrp_type(doc, acc, c)?;
                } else {
                    count_attrs(doc, acc, c);
                }
                read_section(doc, acc, c, table, &mut row)?;
                if !push_row(rows, &row) {
                    if !acc.prefix {
                        return Err(DocError::Capacity);
                    }
                    acc.dropped = acc.dropped.saturating_add(1);
                }
            }
            None => acc.unknown += 1,
        }
        c = next_sibling(doc, c);
    }
    Ok(())
}

/// `mrpType` of an `mrp` element (absent is code 0); other attributes are unknown paths.
fn read_mrp_type(doc: &Doc<'_>, acc: &mut Acc, e: ElemIx) -> Result<u8, DocError> {
    let mut t = change::NONE;
    let mut got = false;
    let n = doc.elem_attr_count.get(e.0 as usize).copied().unwrap_or(0);
    for i in 0..n {
        let a = attr_ref(doc, e, i);
        if a.name == b"mrpType" {
            if got {
                return Err(ferr(
                    Field::MrpType,
                    FieldErrorKind::Duplicate,
                    a.span.start,
                ));
            }
            got = true;
            let mut buf = [0u8; MAX_VALUE];
            let v = attr_val(doc, &a, Field::MrpType, &mut buf)?;
            t = parse_change(v).map_err(|k| ferr(Field::MrpType, k, a.span.start))?;
        } else {
            count_unknown(acc, &a);
        }
    }
    Ok(t)
}
