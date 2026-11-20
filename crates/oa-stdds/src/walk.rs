//! The shared record walker: a table of expected element paths below a base element in, one
//! checked value per path out. Pure functions (crate class shell). Iterative (no recursion); the work is one visit per
//! element of the subtree, each visit a bounded path comparison against the table.

use crate::kind::{attr_present, attr_value, in_message_ns, name_of};
use crate::scalar::{
    parse_beacon, parse_code, parse_hex24, parse_int, parse_iso_ms, parse_uint, scaled_parts, trim,
    ScaledError,
};
use crate::{FieldError, FieldErrorKind, Shape};
use oa_xml::{decode_text_run, Doc, ElemIx, XmlError};

/// Deepest path (names below the base element) the walker follows.
pub(crate) const MAX_PATH: usize = 8;
/// Longest value text read (bytes); longer is [`FieldErrorKind::TooLong`].
pub(crate) const VALUE_BUF: usize = 64;
/// Most distinct unknown paths one message adds (the XML element limit bounds the rest).
const MAX_UNKNOWN_PER_MESSAGE: usize = 64;

/// How a path's text is read.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Val {
    /// ISO-8601 UTC to Unix milliseconds.
    TimeMs,
    /// Decimal degrees to 1e-7 degree, within +-`max_deg`.
    DegE7 { max_deg: i64 },
    /// Decimal with `scale` fraction digits, range `lo..=hi` after scaling.
    Scaled { scale: u32, lo: i64, hi: i64 },
    /// A heading in degrees to 0.01 degree: 0 up to but not including 360 (the exact value is
    /// compared, so 359.999 is in range and rounds to 0).
    Heading,
    /// A whole number in `lo..=hi` (see [`parse_int`]: no fraction, no `-0`, no leading zero).
    Int { lo: i64, hi: i64 },
    /// Four octal digits.
    Beacon,
    /// 1 to 6 hex digits.
    Hex24,
    /// Unsigned decimal of at most this many digits.
    Uint(usize),
    /// Up to 8 ASCII letters, digits and `-_/.`.
    Code,
    /// Up to 16 ASCII letters, digits and `-_/.` (a status word such as `terminated`).
    Word,
    /// Up to 8 ASCII letters, digits and `-_/.*+` (a runway designator as STARS prints it).
    Runway,
    /// Up to 8 ASCII letters, digits, `-_/.` and inner spaces (a callsign).
    Callsign,
    /// An `xs:boolean`: `true`, `false`, `1` or `0`, as 1 or 0.
    Flag,
}

/// One expected path and the field it fills.
#[derive(Clone, Copy)]
pub(crate) struct Spec {
    pub path: &'static [&'static str],
    pub val: Val,
}

/// What a value parsed to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Got {
    Int(i64),
    Code([u8; 8]),
    Word([u8; 16]),
}

/// The state of one field slot while a record is walked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Slot {
    Empty,
    Ok(Got),
    /// Present but refused; the error is already recorded.
    Bad,
    /// Marked no longer valid by the sender (`r="1"`): absent, not an error.
    Invalid,
    /// An element present with no text, in a field of [`BLANK_OK`] or carrying a [`NO_VALUE`]
    /// word: absent, counted apart from errors.
    Blank,
}

/// For each element, its first text node and how many text nodes it owns.
pub(crate) struct TextIndex {
    pub first: Vec<u32>,
    pub count: Vec<u32>,
}

pub(crate) fn text_index(doc: &Doc<'_>) -> TextIndex {
    let n = doc.elem_name.len();
    let mut first = vec![u32::MAX; n];
    let mut count = vec![0u32; n];
    for (t, owner) in doc.text_owner.iter().enumerate() {
        let o = owner.0 as usize;
        if let (Some(f), Some(c)) = (first.get_mut(o), count.get_mut(o)) {
            if *f == u32::MAX {
                *f = t as u32;
            }
            *c += 1;
        }
    }
    TextIndex { first, count }
}

/// Where the walker reports what it finds.
pub(crate) struct Sink<'s> {
    pub errors: &'s mut Vec<FieldError>,
    pub max_errors: usize,
    pub errors_dropped: &'s mut u64,
    pub unknown: &'s mut Vec<(String, u64)>,
    pub unknown_dropped: u64,
    pub invalidated: u64,
}

impl Sink<'_> {
    pub(crate) fn error(&mut self, field: &'static str, kind: FieldErrorKind, offset: u32) {
        self.error_shaped(field, kind, offset, Shape::NONE);
    }

    pub(crate) fn error_shaped(
        &mut self,
        field: &'static str,
        kind: FieldErrorKind,
        offset: u32,
        shape: Shape,
    ) {
        if self.errors.len() < self.max_errors {
            self.errors.push(FieldError {
                field,
                kind,
                offset,
                shape,
            });
        } else {
            *self.errors_dropped += 1;
        }
    }
}

fn offset_of(doc: &Doc<'_>, e: ElemIx) -> u32 {
    doc.elem_name.get(e.0 as usize).map_or(0, |s| s.start)
}

/// The path of `d` below `base` (names, root-most first) into `out`; the depth, or `None` if
/// deeper than [`MAX_PATH`].
fn path_of<'a>(
    doc: &Doc<'a>,
    base: ElemIx,
    d: ElemIx,
    out: &mut [&'a [u8]; MAX_PATH],
) -> Option<usize> {
    let mut rev = [&[][..]; MAX_PATH];
    let mut n = 0usize;
    let mut cur = d;
    while cur != base {
        *rev.get_mut(n)? = name_of(doc, cur);
        n += 1;
        cur = *doc.elem_parent.get(cur.0 as usize)?;
        if cur == ElemIx::NONE {
            return None;
        }
    }
    for i in 0..n {
        *out.get_mut(i)? = *rev.get(n - 1 - i)?;
    }
    Some(n)
}

fn path_eq(want: &[&str], got: &[&[u8]]) -> bool {
    want.len() == got.len() && want.iter().zip(got).all(|(w, g)| w.as_bytes() == *g)
}

fn is_prefix(want: &[&str], got: &[&[u8]]) -> bool {
    got.len() < want.len() && want.iter().zip(got).all(|(w, g)| w.as_bytes() == *g)
}

/// The element after `d` in preorder within `base`'s subtree, not descending when `skip`.
fn next(doc: &Doc<'_>, base: ElemIx, d: ElemIx, skip: bool) -> Option<ElemIx> {
    if !skip {
        let c = *doc.elem_first_child.get(d.0 as usize)?;
        if c != ElemIx::NONE {
            return Some(c);
        }
    }
    let mut cur = d;
    loop {
        if cur == base {
            return None;
        }
        let s = *doc.elem_next_sibling.get(cur.0 as usize)?;
        if s != ElemIx::NONE {
            return Some(s);
        }
        cur = *doc.elem_parent.get(cur.0 as usize)?;
        if cur == ElemIx::NONE {
            return None;
        }
    }
}

fn scaled_kind(e: ScaledError) -> FieldErrorKind {
    match e {
        ScaledError::Syntax => FieldErrorKind::Syntax,
        ScaledError::Range => FieldErrorKind::Range,
    }
}

fn code8(s: &[u8], extra: &[u8]) -> Result<Got, FieldErrorKind> {
    if s.len() > 8 {
        return Err(FieldErrorKind::TooLong);
    }
    parse_code::<8>(s, extra)
        .map(Got::Code)
        .ok_or(FieldErrorKind::Syntax)
}

/// Words a sender prints in place of a value: `(field name, text)`. The element is present and
/// carries no value: absent, counted apart from errors ([`Slot::Blank`]).
const NO_VALUE: [(&str, &[u8]); 3] = [
    ("status/vertRate", b"unavailable"),
    ("flightPlan/entryFix", b"unassigned"),
    ("flightPlan/exitFix", b"unassigned"),
];

/// Text fields whose element may be present with no text (observed on real data as the normal
/// form of an absent value): absent and counted apart from errors ([`Slot::Blank`]). Every other
/// field keeps an empty element as a refused field.
const BLANK_OK: [&str; 1] = ["flightPlan/rawFlightRules"];

fn parse_val(v: Val, s: &[u8]) -> Result<Got, FieldErrorKind> {
    use FieldErrorKind::{Range, Syntax, TooLong};
    match v {
        Val::TimeMs => parse_iso_ms(s).map(Got::Int).ok_or(Syntax),
        Val::DegE7 { max_deg } => {
            let p = scaled_parts(s, 7).map_err(scaled_kind)?;
            let b = max_deg * 10_000_000;
            if p.cmp_exact(b).is_gt() || p.cmp_exact(-b).is_lt() {
                return Err(Range);
            }
            p.rounded().map(Got::Int).ok_or(Range)
        }
        Val::Scaled { scale, lo, hi } => {
            let p = scaled_parts(s, scale).map_err(scaled_kind)?;
            if p.cmp_exact(hi).is_gt() || p.cmp_exact(lo).is_lt() {
                return Err(Range);
            }
            p.rounded().map(Got::Int).ok_or(Range)
        }
        Val::Heading => {
            let p = scaled_parts(s, 2).map_err(scaled_kind)?;
            if p.cmp_exact(36_000).is_ge() || p.cmp_exact(0).is_lt() {
                return Err(Range);
            }
            // A value that rounds up to 360.00 is the heading 0.
            Ok(Got::Int(p.rounded().ok_or(Range)? % 36_000))
        }
        Val::Int { lo, hi } => {
            let x = parse_int(s).map_err(scaled_kind)?;
            if x < lo || x > hi {
                return Err(Range);
            }
            Ok(Got::Int(x))
        }
        Val::Beacon => parse_beacon(s)
            .map(|b| Got::Int(i64::from(b)))
            .ok_or(Syntax),
        Val::Hex24 => parse_hex24(s).map(|b| Got::Int(i64::from(b))).ok_or(Syntax),
        Val::Uint(digits) => {
            let x = parse_uint(s, digits).ok_or(Syntax)?;
            i64::try_from(x).map(Got::Int).map_err(|_| Range)
        }
        Val::Flag => match s {
            b"true" | b"1" => Ok(Got::Int(1)),
            b"false" | b"0" => Ok(Got::Int(0)),
            _ => Err(Syntax),
        },
        Val::Code => code8(s, b"-_/."),
        Val::Runway => code8(s, b"-_/.*+"),
        Val::Callsign => code8(s, b"-_/. "),
        Val::Word => {
            if s.len() > 16 {
                return Err(TooLong);
            }
            parse_code::<16>(s, b"-_/.").map(Got::Word).ok_or(Syntax)
        }
    }
}

/// Reads the text of leaf element `e`, trimmed, into `buf`.
pub(crate) fn leaf_text<'b>(
    doc: &Doc<'_>,
    tx: &TextIndex,
    e: ElemIx,
    buf: &'b mut [u8; VALUE_BUF],
) -> Result<&'b [u8], FieldErrorKind> {
    let i = e.0 as usize;
    let child = doc.elem_first_child.get(i).copied().unwrap_or(ElemIx::NONE);
    if child != ElemIx::NONE {
        return Err(FieldErrorKind::NotLeaf);
    }
    let count = tx.count.get(i).copied().unwrap_or(0) as usize;
    let first = tx.first.get(i).copied().unwrap_or(u32::MAX) as usize;
    if count == 0 {
        return Err(FieldErrorKind::Empty);
    }
    let n = match decode_text_run(doc, first, count, buf) {
        Ok(n) => n,
        Err(XmlError::OutputTooSmall { .. }) => return Err(FieldErrorKind::TooLong),
        Err(_) => return Err(FieldErrorKind::Text),
    };
    let t = trim(buf.get(..n).unwrap_or(&[]));
    if t.is_empty() {
        Err(FieldErrorKind::Empty)
    } else {
        Ok(t)
    }
}

/// Walks the subtree of `base`, filling `slots[i]` for the path `specs[i]`. `skip` lists paths
/// that are known and deliberately not read (not counted as unknown). Everything else that is
/// not a container of a known path is counted once per topmost element in `sink.unknown`.
/// Returns `Err(offset)` if an element is nested deeper than [`MAX_PATH`].
#[allow(clippy::too_many_arguments)]
pub(crate) fn walk(
    doc: &Doc<'_>,
    tx: &TextIndex,
    base: ElemIx,
    specs: &[Spec],
    names: &[&'static str],
    skip: &[&[&str]],
    slots: &mut [Slot],
    sink: &mut Sink<'_>,
) -> Result<(), u32> {
    let mut cur = doc
        .elem_first_child
        .get(base.0 as usize)
        .copied()
        .unwrap_or(ElemIx::NONE);
    let mut pbuf = [&[][..]; MAX_PATH];
    let mut tbuf = [0u8; VALUE_BUF];
    let mut abuf = [0u8; 8];
    while cur != ElemIx::NONE {
        let Some(depth) = path_of(doc, base, cur, &mut pbuf) else {
            return Err(offset_of(doc, cur));
        };
        let path = pbuf.get(..depth).unwrap_or(&[]);
        let mut descend = true;
        if !in_message_ns(doc, cur) {
            // Not a field of this message: counted, nothing below it is looked at.
            descend = false;
            note_unknown(sink, path);
        } else if let Some(si) = specs.iter().position(|s| path_eq(s.path, path)) {
            descend = false;
            read_field(
                doc, tx, cur, specs, names, si, slots, sink, &mut tbuf, &mut abuf,
            );
        } else if skip.iter().any(|s| path_eq(s, path)) {
            // Known and deliberately not read: nothing below it is looked at.
            descend = false;
        } else if skip.iter().any(|s| is_prefix(s, path)) {
            // A container of skipped paths.
        } else if specs.iter().any(|s| is_prefix(s.path, path)) {
            // A container of known paths.
        } else {
            descend = false;
            note_unknown(sink, path);
        }
        cur = match next(doc, base, cur, !descend) {
            Some(n) if n != base => n,
            _ => break,
        };
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn read_field(
    doc: &Doc<'_>,
    tx: &TextIndex,
    e: ElemIx,
    specs: &[Spec],
    names: &[&'static str],
    si: usize,
    slots: &mut [Slot],
    sink: &mut Sink<'_>,
    tbuf: &mut [u8; VALUE_BUF],
    abuf: &mut [u8; 8],
) {
    let (Some(spec), Some(slot), Some(&name)) = (specs.get(si), slots.get_mut(si), names.get(si))
    else {
        return;
    };
    let off = offset_of(doc, e);
    if *slot != Slot::Empty {
        *slot = Slot::Bad;
        sink.error(name, FieldErrorKind::Duplicate, off);
        return;
    }
    match attr_value(doc, e, b"r", abuf) {
        Some(b"1") => {
            *slot = Slot::Invalid;
            sink.invalidated += 1;
            return;
        }
        Some(b"0") => {}
        None if !attr_present(doc, e, b"r") => {}
        _ => {
            // `r` is neither "1" nor "0" (or is too long to be either): the field is refused.
            *slot = Slot::Bad;
            sink.error(name, FieldErrorKind::Syntax, off);
            return;
        }
    }
    let (kind, shape) = match leaf_text(doc, tx, e, tbuf) {
        Ok(t) if NO_VALUE.iter().any(|&(n, w)| n == name && w == t) => {
            *slot = Slot::Blank;
            return;
        }
        Ok(t) => match parse_val(spec.val, t) {
            Ok(g) => {
                *slot = Slot::Ok(g);
                return;
            }
            Err(k) => (k, Shape::of(t)),
        },
        Err(k) => (k, Shape::NONE),
    };
    if kind == FieldErrorKind::Empty && BLANK_OK.contains(&name) {
        *slot = Slot::Blank;
        return;
    }
    *slot = Slot::Bad;
    sink.error_shaped(name, kind, off, shape);
}

pub(crate) fn note_unknown(sink: &mut Sink<'_>, path: &[&[u8]]) {
    let mut text = String::new();
    for (i, p) in path.iter().enumerate() {
        if i > 0 {
            text.push('/');
        }
        text.push_str(&String::from_utf8_lossy(p));
    }
    if let Some(e) = sink.unknown.iter_mut().find(|(t, _)| *t == text) {
        e.1 += 1;
    } else if sink.unknown.len() < MAX_UNKNOWN_PER_MESSAGE {
        sink.unknown.push((text, 1));
    } else {
        sink.unknown_dropped += 1;
    }
}

pub(crate) fn elem_offset(doc: &Doc<'_>, e: ElemIx) -> u32 {
    offset_of(doc, e)
}

/// The elements reached from `base` by the child names `path`, in document order.
fn reach(doc: &Doc<'_>, base: ElemIx, path: &[&[u8]], out: &mut Vec<ElemIx>) {
    let Some((first, rest)) = path.split_first() else {
        out.push(base);
        return;
    };
    for c in crate::kind::children(doc, base) {
        if name_of(doc, c) == *first {
            reach(doc, c, rest, out);
        }
    }
}

/// The first step of a message decode, before any count, table or record is touched: reads the
/// observation times of every record (`record` below the root, each time at one of `time_paths`) directly from the
/// parsed tree, whatever else in the message is valid. If any is in `window`, returns how many
/// records the message has (all are dropped, at least 1); otherwise 0. A time that cannot be
/// read is not in the window (the frame's receive time gates it). No record limit applies to
/// this scan, so a record in the window past `max_records` is still found.
pub(crate) fn prescan(
    doc: &Doc<'_>,
    tx: &TextIndex,
    window: oa_sealed::DropWindow,
    record: &[u8],
    time_paths: &[&[&[u8]]],
) -> u64 {
    if window.span().is_none() {
        return 0;
    }
    let (mut records, mut hit) = (0u64, false);
    let mut found = Vec::new();
    for c in crate::kind::children(doc, ElemIx(0)) {
        if name_of(doc, c) != record || !in_message_ns(doc, c) {
            continue;
        }
        records += 1;
        found.clear();
        for path in time_paths {
            reach(doc, c, path, &mut found);
        }
        for e in &found {
            let mut buf = [0u8; VALUE_BUF];
            if let Ok(t) = leaf_text(doc, tx, *e, &mut buf) {
                hit |= parse_iso_ms(t).is_some_and(|t| window.contains(t));
            }
        }
    }
    if hit {
        records.max(1)
    } else {
        0
    }
}
