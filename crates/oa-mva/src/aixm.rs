//! The AIXM 5.1 reader. Shell (offline): reads files, otherwise a pure function of its input.
//!
//! # What is read
//!
//! One FAA message holds one chart (`<ID>_MVA_FUS3`, `<ID>_MVA_FUS5` or `<ID>_TAV`). The
//! structure below is the whole of what the 35 real files use (checked element by element on
//! the 2024-2025 files); anything else, in an element or an attribute, is refused with
//! [`MvaError`] and nothing from the file is kept. Namespaces are matched by URI
//! (`http://www.aixm.aero/schema/5.1` [aixm], `.../5.1/message` [msg],
//! `http://www.opengis.net/gml/3.2` [gml]); the prefixes (`ns1`, `ns3`, ...) change from file
//! to file and are never relied on.
//!
//! ```text
//! msg:AIXMBasicMessage
//!   gml:description       "<facility>_MVA_FUS3|FUS5[_suffix]" or "<facility>_TAV[_suffix]"
//!   gml:boundedBy         empty (xsi:nil)
//!   aixm:sequenceNumber   optional, digits
//!   aixm:messageMetadata  optional, empty
//!   msg:hasMember+        each holds exactly one aixm:Airspace
//! aixm:Airspace
//!   gml:description       "MINIMUM VECTORING ALTITUDE (MVA)" (FUS3, FUS5) or
//!                         "MINIMUM IFR ALTITUDE (MIA)" (TAV)
//!   gml:boundedBy         optional, empty
//!   aixm:timeSlice        exactly one, holding exactly one aixm:AirspaceTimeSlice
//! aixm:AirspaceTimeSlice
//!   gml:validTime/gml:TimePeriod   beginPosition (non-empty) and an endPosition that is empty
//!                                  with indeterminatePosition="unknown" (open ended)
//!   aixm:interpretation   BASELINE        aixm:sequenceNumber   1
//!   aixm:type             OTHER           aixm:name             the sector id, 1-8 [A-Za-z0-9]
//!   aixm:designator, localType, designatorICAO, controlType, upperLowerSeparation,
//!   protectedRoute        each optional and empty
//!   aixm:geometryComponent  exactly one: AirspaceGeometryComponent with optional empty
//!                           operation and operationSequence and exactly one theAirspaceVolume
//! aixm:AirspaceVolume
//!   aixm:minimumLimit uom="FT"        integer feet, 0 to 18,000
//!   aixm:minimumLimitReference        MSL
//!   aixm:horizontalProjection/aixm:Surface srsName="urn:ogc:def:crs:OGC:1.3:CRS84"
//!     gml:patches/gml:PolygonPatch interpolation="planar"   exactly one patch
//!       gml:exterior   exactly one;  gml:interior  any number (holes)
//!         gml:LinearRing/gml:posList   "lon lat lon lat ..." decimal degrees, at most 7
//!                                      decimals, closed (first vertex = last), 2 dimensions
//! ```
//!
//! Refused, by name, because the real files do not use them and their meaning would have to be
//! guessed: `upperLimit` and `maximumLimit` (a ceiling), any limit reference other than `MSL`,
//! any unit other than `FT`, several patches or time slices, a `TimePeriod` with an end,
//! `gml:Arc`/`gml:Circle`/curve segments or `gml:pos` lists, and every element or attribute not
//! named above. A ring that is not closed is refused; a ring that crosses itself is not (that
//! is a property of the data, counted by [`crate::check`]).
//!
//! # Bounds
//!
//! [`crate::MAX_FILE_BYTES`], [`crate::MAX_SECTORS_PER_CHART`], [`crate::MAX_RINGS_PER_SECTOR`],
//! [`crate::MAX_VERTICES_PER_RING`], [`crate::MAX_POSLIST_BYTES`], [`crate::MAX_SECTORS`],
//! [`crate::MAX_CHARTS`], [`crate::MAX_VERTICES`], [`crate::MAX_SECTOR_SPAN_DEG`], and the
//! `oa-xml` limits of [`default_limits`].
//!
//! A message that is refused leaves the model exactly as it was.

use crate::model::{Model, SetKind};
use crate::{
    MvaError, E7, MAX_ALT_FT, MAX_CHARTS, MAX_FACILITY_BYTES, MAX_FILE_BYTES, MAX_POSLIST_BYTES,
    MAX_RINGS_PER_SECTOR, MAX_SECTORS, MAX_SECTORS_PER_CHART, MAX_SECTOR_ID_BYTES,
    MAX_SECTOR_SPAN_DEG, MAX_VERTICES, MAX_VERTICES_PER_RING, MIN_ALT_FT, MIN_VERTICES_PER_RING,
};
use oa_xml::{
    attr_needs_decode, decode_into, resolve_elem_ns, span_bytes, text_needs_decode, AttrIx, Doc,
    ElemIx, Limits, Ns, Parser, SpanKind, TextIx,
};
use std::borrow::Cow;
use std::path::Path;

const NS_AIXM: &[u8] = b"http://www.aixm.aero/schema/5.1";
const NS_GML: &[u8] = b"http://www.opengis.net/gml/3.2";
const NS_MSG: &[u8] = b"http://www.aixm.aero/schema/5.1/message";
const CRS84: &[u8] = b"urn:ogc:def:crs:OGC:1.3:CRS84";
const DESC_MVA: &[u8] = b"MINIMUM VECTORING ALTITUDE (MVA)";
const DESC_MIA: &[u8] = b"MINIMUM IFR ALTITUDE (MIA)";

/// Attribute names (local part) the reader understands anywhere; their values are checked where
/// they matter. Namespace declarations are skipped.
const KNOWN_ATTRS: [&str; 8] = [
    "id",
    "type",
    "nil",
    "uom",
    "srsName",
    "srsDimension",
    "interpolation",
    "indeterminatePosition",
];

/// The `oa-xml` limits for a chart message. The input is capped at [`MAX_FILE_BYTES`]; the
/// element and text limits are the `oa-xml` defaults (50,000 elements, 50,000 text nodes), which
/// are about 30 times what the largest real chart (about 1,800 elements) needs.
pub fn default_limits() -> Limits {
    Limits {
        max_input_bytes: MAX_FILE_BYTES as usize,
        ..Limits::default()
    }
}

/// A parser with [`default_limits`]; reuse it for every file.
pub fn new_parser() -> Result<Parser, MvaError> {
    Parser::new(default_limits()).map_err(MvaError::Xml)
}

/// Shell: reads a whole file, refusing one over [`MAX_FILE_BYTES`].
pub fn read_file(path: &Path) -> Result<Vec<u8>, MvaError> {
    let meta = std::fs::metadata(path).map_err(|e| MvaError::Io(e.to_string()))?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(MvaError::FileTooLarge(meta.len()));
    }
    std::fs::read(path).map_err(|e| MvaError::Io(e.to_string()))
}

/// What one loaded message added. Plain data (A1).
// A7.2: cold struct, size guard waived (one per file)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Loaded {
    /// Chart row.
    pub chart: usize,
    /// Sectors added.
    pub sectors: usize,
    /// Rings added (exteriors and holes).
    pub rings: usize,
    /// Vertices added.
    pub vertices: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Q {
    Aixm,
    Gml,
    Msg,
    Other,
}

const NO_TEXT: u32 = u32::MAX;
const MANY_TEXT: u32 = u32::MAX - 1;

struct W<'a> {
    doc: Doc<'a>,
    /// First text node of each element, `NO_TEXT` or `MANY_TEXT`.
    texts: Vec<u32>,
}

fn at(w: &W<'_>, e: ElemIx) -> u32 {
    w.doc.elem_name.get(e.0 as usize).map_or(0, |s| s.start)
}

fn local<'a>(w: &W<'a>, e: ElemIx) -> &'a [u8] {
    w.doc
        .elem_name
        .get(e.0 as usize)
        .and_then(|&s| span_bytes(w.doc.input, s).ok())
        .unwrap_or(&[])
}

fn qname<'a>(w: &W<'a>, e: ElemIx) -> (Q, &'a [u8]) {
    let q = match resolve_elem_ns(&w.doc, e) {
        Ns::Uri(u) => match span_bytes(w.doc.input, u) {
            Ok(b) if b == NS_AIXM => Q::Aixm,
            Ok(b) if b == NS_GML => Q::Gml,
            Ok(b) if b == NS_MSG => Q::Msg,
            _ => Q::Other,
        },
        _ => Q::Other,
    };
    (q, local(w, e))
}

fn kids(w: &W<'_>, e: ElemIx) -> Vec<ElemIx> {
    let mut v = Vec::new();
    let mut c = w
        .doc
        .elem_first_child
        .get(e.0 as usize)
        .copied()
        .unwrap_or(ElemIx::NONE);
    while c != ElemIx::NONE && v.len() <= w.doc.elem_name.len() {
        v.push(c);
        c = w
            .doc
            .elem_next_sibling
            .get(c.0 as usize)
            .copied()
            .unwrap_or(ElemIx::NONE);
    }
    v
}

fn short_name(b: &[u8]) -> String {
    let s = String::from_utf8_lossy(b);
    s.chars().take(64).collect()
}

/// Refuses a child of `e` that is not in `allowed`.
fn only(
    w: &W<'_>,
    e: ElemIx,
    context: &'static str,
    allowed: &[(Q, &str)],
) -> Result<Vec<ElemIx>, MvaError> {
    let ks = kids(w, e);
    for &k in &ks {
        let (q, n) = qname(w, k);
        if !allowed
            .iter()
            .any(|&(aq, an)| aq == q && an.as_bytes() == n)
        {
            return Err(MvaError::UnexpectedElement {
                context,
                name: short_name(n),
                at: at(w, k),
            });
        }
    }
    Ok(ks)
}

fn matching(w: &W<'_>, ks: &[ElemIx], q: Q, name: &str) -> Vec<ElemIx> {
    ks.iter()
        .copied()
        .filter(|&k| qname(w, k) == (q, name.as_bytes()))
        .collect()
}

/// The single child named `name`; none or several is a [`MvaError::Cardinality`].
fn one(
    w: &W<'_>,
    e: ElemIx,
    ks: &[ElemIx],
    context: &'static str,
    q: Q,
    name: &'static str,
) -> Result<ElemIx, MvaError> {
    match matching(w, ks, q, name).as_slice() {
        [k] => Ok(*k),
        other => Err(MvaError::Cardinality {
            context,
            name,
            found: other.len(),
            at: at(w, e),
        }),
    }
}

/// The child named `name` if present; several is a [`MvaError::Cardinality`].
fn opt(
    w: &W<'_>,
    e: ElemIx,
    ks: &[ElemIx],
    context: &'static str,
    q: Q,
    name: &'static str,
) -> Result<Option<ElemIx>, MvaError> {
    match matching(w, ks, q, name).as_slice() {
        [] => Ok(None),
        [k] => Ok(Some(*k)),
        other => Err(MvaError::Cardinality {
            context,
            name,
            found: other.len(),
            at: at(w, e),
        }),
    }
}

/// An element that must carry nothing: no child element and no text.
fn empty(w: &W<'_>, e: ElemIx, field: &'static str) -> Result<(), MvaError> {
    let has_text = w.texts.get(e.0 as usize).is_some_and(|&t| t != NO_TEXT);
    if has_text || !kids(w, e).is_empty() {
        return Err(MvaError::BadValue {
            field,
            at: at(w, e),
        });
    }
    Ok(())
}

fn empty_opt(
    w: &W<'_>,
    e: ElemIx,
    ks: &[ElemIx],
    context: &'static str,
    q: Q,
    name: &'static str,
) -> Result<(), MvaError> {
    if let Some(k) = opt(w, e, ks, context, q, name)? {
        empty(w, k, name)?;
    }
    Ok(())
}

/// The text of a leaf element (exactly one text node, no child element), decoded.
fn leaf_text<'a>(w: &W<'a>, e: ElemIx, field: &'static str) -> Result<Cow<'a, [u8]>, MvaError> {
    let bad = || MvaError::BadValue {
        field,
        at: at(w, e),
    };
    let t = w.texts.get(e.0 as usize).copied().unwrap_or(NO_TEXT);
    if t == NO_TEXT || t == MANY_TEXT || !kids(w, e).is_empty() {
        return Err(bad());
    }
    let sp = w.doc.text_span.get(t as usize).copied().ok_or_else(bad)?;
    if sp.len as usize > MAX_POSLIST_BYTES {
        return Err(MvaError::Limit {
            what: "text bytes",
            max: MAX_POSLIST_BYTES as u64,
            at: at(w, e),
        });
    }
    if text_needs_decode(&w.doc, TextIx(t)) {
        let mut buf = vec![0u8; sp.len as usize];
        let n = decode_into(sp, w.doc.input, &mut buf, SpanKind::Text).map_err(MvaError::Xml)?;
        buf.truncate(n);
        Ok(Cow::Owned(buf))
    } else {
        Ok(Cow::Borrowed(
            span_bytes(w.doc.input, sp).map_err(MvaError::Xml)?,
        ))
    }
}

fn trim(b: &[u8]) -> &[u8] {
    let s = b
        .iter()
        .position(|c| !c.is_ascii_whitespace())
        .unwrap_or(b.len());
    let e = b
        .iter()
        .rposition(|c| !c.is_ascii_whitespace())
        .map_or(s, |p| p + 1);
    b.get(s..e).unwrap_or(&[])
}

/// The value of attribute `name` (local part) of `e`, decoded.
fn attr<'a>(w: &W<'a>, e: ElemIx, name: &str) -> Result<Option<Cow<'a, [u8]>>, MvaError> {
    let first = w
        .doc
        .elem_first_attr
        .get(e.0 as usize)
        .map_or(0, |a| a.0 as usize);
    let n = w
        .doc
        .elem_attr_count
        .get(e.0 as usize)
        .map_or(0, |&c| c as usize);
    for a in first..first.saturating_add(n) {
        let nm = w
            .doc
            .attr_name
            .get(a)
            .and_then(|&s| span_bytes(w.doc.input, s).ok())
            .unwrap_or(&[]);
        let is_decl = is_xmlns(w, a);
        if nm == name.as_bytes() && !is_decl {
            let sp = w.doc.attr_value.get(a).copied().ok_or(MvaError::BadValue {
                field: "attribute",
                at: at(w, e),
            })?;
            if attr_needs_decode(&w.doc, AttrIx(a as u32)) {
                let mut buf = vec![0u8; sp.len as usize];
                let k = decode_into(sp, w.doc.input, &mut buf, SpanKind::AttrValue)
                    .map_err(MvaError::Xml)?;
                buf.truncate(k);
                return Ok(Some(Cow::Owned(buf)));
            }
            return Ok(Some(Cow::Borrowed(
                span_bytes(w.doc.input, sp).map_err(MvaError::Xml)?,
            )));
        }
    }
    Ok(None)
}

fn is_xmlns(w: &W<'_>, a: usize) -> bool {
    let prefix = w
        .doc
        .attr_prefix
        .get(a)
        .and_then(|&s| span_bytes(w.doc.input, s).ok())
        .unwrap_or(&[]);
    let name = w
        .doc
        .attr_name
        .get(a)
        .and_then(|&s| span_bytes(w.doc.input, s).ok())
        .unwrap_or(&[]);
    prefix == b"xmlns" || (prefix.is_empty() && name == b"xmlns")
}

/// Refuses an attribute (other than a namespace declaration) not in [`KNOWN_ATTRS`], on any
/// element of the message.
fn check_attributes(w: &W<'_>) -> Result<(), MvaError> {
    for e in 0..w.doc.elem_name.len() {
        let first = w.doc.elem_first_attr.get(e).map_or(0, |a| a.0 as usize);
        let n = w.doc.elem_attr_count.get(e).map_or(0, |&c| c as usize);
        for a in first..first.saturating_add(n) {
            if is_xmlns(w, a) {
                continue;
            }
            let nm = w
                .doc
                .attr_name
                .get(a)
                .and_then(|&s| span_bytes(w.doc.input, s).ok())
                .unwrap_or(&[]);
            if !KNOWN_ATTRS.iter().any(|k| k.as_bytes() == nm) {
                return Err(MvaError::UnexpectedAttribute {
                    name: short_name(nm),
                    at: at(w, ElemIx(e as u32)),
                });
            }
        }
    }
    Ok(())
}

fn expect_attr(
    w: &W<'_>,
    e: ElemIx,
    name: &str,
    want: &[u8],
    field: &'static str,
) -> Result<(), MvaError> {
    match attr(w, e, name)? {
        Some(v) if v.as_ref() == want => Ok(()),
        _ => Err(MvaError::BadValue {
            field,
            at: at(w, e),
        }),
    }
}

fn expect_text(w: &W<'_>, e: ElemIx, want: &[u8], field: &'static str) -> Result<(), MvaError> {
    let t = leaf_text(w, e, field)?;
    if trim(&t) == want {
        Ok(())
    } else {
        Err(MvaError::BadValue {
            field,
            at: at(w, e),
        })
    }
}

/// A decimal degree string to 1e-7 degrees with no floating point: optional `-`, digits,
/// optionally `.` and 1 to 7 digits; no sign `+`, no exponent, no empty part. `None` when the
/// text is not that or its magnitude exceeds `max_deg`.
pub(crate) fn parse_deg_e7(tok: &[u8], max_deg: i64) -> Option<i32> {
    let (neg, rest) = match tok.split_first() {
        Some((b'-', r)) => (true, r),
        _ => (false, tok),
    };
    let (int_s, frac_s) = match rest.iter().position(|&c| c == b'.') {
        Some(p) => (rest.get(..p)?, Some(rest.get(p + 1..)?)),
        None => (rest, None),
    };
    if int_s.is_empty() || int_s.len() > 3 || !int_s.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let mut v: i64 = 0;
    for &c in int_s {
        v = v * 10 + i64::from(c - b'0');
    }
    let mut frac: i64 = 0;
    let mut digits = 0u32;
    if let Some(f) = frac_s {
        if f.is_empty() || f.len() > 7 || !f.iter().all(u8::is_ascii_digit) {
            return None;
        }
        for &c in f {
            frac = frac * 10 + i64::from(c - b'0');
            digits += 1;
        }
    }
    let scaled = v * E7 + frac * 10_i64.pow(7 - digits);
    if scaled > max_deg * E7 {
        return None;
    }
    let s = if neg { -scaled } else { scaled };
    i32::try_from(s).ok()
}

/// One parsed ring: (lat, lon) in 1e-7 degrees per vertex.
struct Ring {
    lat: Vec<i32>,
    lon: Vec<i32>,
}

fn parse_ring(w: &W<'_>, e: ElemIx, ctx: &'static str) -> Result<Ring, MvaError> {
    let ks = only(w, e, ctx, &[(Q::Gml, "LinearRing")])?;
    let lr = one(w, e, &ks, ctx, Q::Gml, "LinearRing")?;
    let ks = only(w, lr, "LinearRing", &[(Q::Gml, "posList")])?;
    let pl = one(w, lr, &ks, "LinearRing", Q::Gml, "posList")?;
    if let Some(d) = attr(w, pl, "srsDimension")? {
        if d.as_ref() != b"2" {
            return Err(MvaError::BadValue {
                field: "posList srsDimension",
                at: at(w, pl),
            });
        }
    }
    let text = leaf_text(w, pl, "posList")?;
    let mut lat = Vec::new();
    let mut lon = Vec::new();
    let mut pending: Option<i32> = None;
    let mut count = 0usize;
    for tok in text
        .split(|c| c.is_ascii_whitespace())
        .filter(|t| !t.is_empty())
    {
        count += 1;
        if count > MAX_VERTICES_PER_RING * 2 {
            return Err(MvaError::Limit {
                what: "vertices per ring",
                max: MAX_VERTICES_PER_RING as u64,
                at: at(w, pl),
            });
        }
        match pending.take() {
            None => {
                let x = parse_deg_e7(tok, 180).ok_or(MvaError::BadValue {
                    field: "posList longitude",
                    at: at(w, pl),
                })?;
                pending = Some(x);
            }
            Some(x) => {
                let y = parse_deg_e7(tok, 90).ok_or(MvaError::BadValue {
                    field: "posList latitude",
                    at: at(w, pl),
                })?;
                lon.push(x);
                lat.push(y);
            }
        }
    }
    if pending.is_some() {
        return Err(MvaError::BadValue {
            field: "posList (odd number of values)",
            at: at(w, pl),
        });
    }
    if lat.len() < MIN_VERTICES_PER_RING {
        return Err(MvaError::BadValue {
            field: "posList (fewer than 4 vertices)",
            at: at(w, pl),
        });
    }
    if lat.first() != lat.last() || lon.first() != lon.last() {
        return Err(MvaError::RingNotClosed { at: at(w, pl) });
    }
    Ok(Ring { lat, lon })
}

/// A sector parsed out of one airspace feature.
struct Sector {
    id: [u8; MAX_SECTOR_ID_BYTES],
    alt: i32,
    rings: Vec<Ring>,
    at: u32,
}

fn parse_airspace(w: &W<'_>, e: ElemIx, want_desc: &[u8]) -> Result<Sector, MvaError> {
    let ks = only(
        w,
        e,
        "Airspace",
        &[
            (Q::Gml, "description"),
            (Q::Gml, "boundedBy"),
            (Q::Aixm, "timeSlice"),
        ],
    )?;
    let d = one(w, e, &ks, "Airspace", Q::Gml, "description")?;
    expect_text(w, d, want_desc, "Airspace description")?;
    if let Some(b) = opt(w, e, &ks, "Airspace", Q::Gml, "boundedBy")? {
        empty(w, b, "boundedBy")?;
    }
    let tsl = one(w, e, &ks, "Airspace", Q::Aixm, "timeSlice")?;
    let ks = only(w, tsl, "timeSlice", &[(Q::Aixm, "AirspaceTimeSlice")])?;
    let ts = one(w, tsl, &ks, "timeSlice", Q::Aixm, "AirspaceTimeSlice")?;
    let ks = only(
        w,
        ts,
        "AirspaceTimeSlice",
        &[
            (Q::Gml, "validTime"),
            (Q::Aixm, "interpretation"),
            (Q::Aixm, "sequenceNumber"),
            (Q::Aixm, "type"),
            (Q::Aixm, "designator"),
            (Q::Aixm, "localType"),
            (Q::Aixm, "name"),
            (Q::Aixm, "designatorICAO"),
            (Q::Aixm, "controlType"),
            (Q::Aixm, "upperLowerSeparation"),
            (Q::Aixm, "protectedRoute"),
            (Q::Aixm, "geometryComponent"),
        ],
    )?;
    let c = "AirspaceTimeSlice";
    let vt = one(w, ts, &ks, c, Q::Gml, "validTime")?;
    parse_valid_time(w, vt)?;
    let i = one(w, ts, &ks, c, Q::Aixm, "interpretation")?;
    expect_text(w, i, b"BASELINE", "interpretation")?;
    let sn = one(w, ts, &ks, c, Q::Aixm, "sequenceNumber")?;
    expect_text(w, sn, b"1", "time slice sequenceNumber")?;
    let ty = one(w, ts, &ks, c, Q::Aixm, "type")?;
    expect_text(w, ty, b"OTHER", "airspace type")?;
    for name in [
        "designator",
        "localType",
        "designatorICAO",
        "controlType",
        "upperLowerSeparation",
        "protectedRoute",
    ] {
        empty_opt(w, ts, &ks, c, Q::Aixm, name)?;
    }
    let nm = one(w, ts, &ks, c, Q::Aixm, "name")?;
    let name_at = at(w, nm);
    let id = parse_sector_id(w, nm)?;
    let gc = one(w, ts, &ks, c, Q::Aixm, "geometryComponent")?;
    let ks = only(
        w,
        gc,
        "geometryComponent",
        &[(Q::Aixm, "AirspaceGeometryComponent")],
    )?;
    let agc = one(
        w,
        gc,
        &ks,
        "geometryComponent",
        Q::Aixm,
        "AirspaceGeometryComponent",
    )?;
    let ks = only(
        w,
        agc,
        "AirspaceGeometryComponent",
        &[
            (Q::Aixm, "operation"),
            (Q::Aixm, "operationSequence"),
            (Q::Aixm, "theAirspaceVolume"),
        ],
    )?;
    let c = "AirspaceGeometryComponent";
    empty_opt(w, agc, &ks, c, Q::Aixm, "operation")?;
    empty_opt(w, agc, &ks, c, Q::Aixm, "operationSequence")?;
    let tv = one(w, agc, &ks, c, Q::Aixm, "theAirspaceVolume")?;
    let ks = only(w, tv, "theAirspaceVolume", &[(Q::Aixm, "AirspaceVolume")])?;
    let vol = one(w, tv, &ks, "theAirspaceVolume", Q::Aixm, "AirspaceVolume")?;
    let ks = only(
        w,
        vol,
        "AirspaceVolume",
        &[
            (Q::Aixm, "minimumLimit"),
            (Q::Aixm, "minimumLimitReference"),
            (Q::Aixm, "horizontalProjection"),
        ],
    )?;
    let c = "AirspaceVolume";
    let ml = one(w, vol, &ks, c, Q::Aixm, "minimumLimit")?;
    expect_attr(w, ml, "uom", b"FT", "minimumLimit unit")?;
    let alt = parse_alt(w, ml)?;
    let mr = one(w, vol, &ks, c, Q::Aixm, "minimumLimitReference")?;
    expect_text(w, mr, b"MSL", "minimumLimitReference")?;
    let hp = one(w, vol, &ks, c, Q::Aixm, "horizontalProjection")?;
    let rings = parse_surface(w, hp)?;
    Ok(Sector {
        id,
        alt,
        rings,
        at: name_at,
    })
}

fn parse_valid_time(w: &W<'_>, vt: ElemIx) -> Result<(), MvaError> {
    let ks = only(w, vt, "validTime", &[(Q::Gml, "TimePeriod")])?;
    let tp = one(w, vt, &ks, "validTime", Q::Gml, "TimePeriod")?;
    let ks = only(
        w,
        tp,
        "TimePeriod",
        &[(Q::Gml, "beginPosition"), (Q::Gml, "endPosition")],
    )?;
    let b = one(w, tp, &ks, "TimePeriod", Q::Gml, "beginPosition")?;
    if trim(&leaf_text(w, b, "beginPosition")?).is_empty() {
        return Err(MvaError::BadValue {
            field: "beginPosition",
            at: at(w, b),
        });
    }
    let en = one(w, tp, &ks, "TimePeriod", Q::Gml, "endPosition")?;
    expect_attr(
        w,
        en,
        "indeterminatePosition",
        b"unknown",
        "endPosition (not open ended)",
    )?;
    empty(w, en, "endPosition (not open ended)")
}

fn parse_sector_id(w: &W<'_>, nm: ElemIx) -> Result<[u8; MAX_SECTOR_ID_BYTES], MvaError> {
    let t = leaf_text(w, nm, "sector name")?;
    let t = trim(&t);
    if t.is_empty() || t.len() > MAX_SECTOR_ID_BYTES || !t.iter().all(u8::is_ascii_alphanumeric) {
        return Err(MvaError::BadValue {
            field: "sector name",
            at: at(w, nm),
        });
    }
    let mut id = [0u8; MAX_SECTOR_ID_BYTES];
    for (d, s) in id.iter_mut().zip(t) {
        *d = *s;
    }
    Ok(id)
}

fn parse_alt(w: &W<'_>, ml: ElemIx) -> Result<i32, MvaError> {
    let t = leaf_text(w, ml, "minimumLimit")?;
    let t = trim(&t);
    let bad = MvaError::BadValue {
        field: "minimumLimit",
        at: at(w, ml),
    };
    if t.is_empty() || t.len() > 6 || !t.iter().all(u8::is_ascii_digit) {
        return Err(bad);
    }
    let mut v: i32 = 0;
    for &c in t {
        v = v * 10 + i32::from(c - b'0');
    }
    if !(MIN_ALT_FT..=MAX_ALT_FT).contains(&v) {
        return Err(bad);
    }
    Ok(v)
}

fn parse_surface(w: &W<'_>, hp: ElemIx) -> Result<Vec<Ring>, MvaError> {
    let ks = only(w, hp, "horizontalProjection", &[(Q::Aixm, "Surface")])?;
    let sf = one(w, hp, &ks, "horizontalProjection", Q::Aixm, "Surface")?;
    expect_attr(w, sf, "srsName", CRS84, "Surface srsName")?;
    let ks = only(w, sf, "Surface", &[(Q::Gml, "patches")])?;
    let ps = one(w, sf, &ks, "Surface", Q::Gml, "patches")?;
    let ks = only(w, ps, "patches", &[(Q::Gml, "PolygonPatch")])?;
    let pp = one(w, ps, &ks, "patches", Q::Gml, "PolygonPatch")?;
    expect_attr(
        w,
        pp,
        "interpolation",
        b"planar",
        "PolygonPatch interpolation",
    )?;
    let ks = only(
        w,
        pp,
        "PolygonPatch",
        &[(Q::Gml, "exterior"), (Q::Gml, "interior")],
    )?;
    let ex = one(w, pp, &ks, "PolygonPatch", Q::Gml, "exterior")?;
    let holes = matching(w, &ks, Q::Gml, "interior");
    if holes.len() + 1 > MAX_RINGS_PER_SECTOR {
        return Err(MvaError::Limit {
            what: "rings per sector",
            max: MAX_RINGS_PER_SECTOR as u64,
            at: at(w, pp),
        });
    }
    let mut rings = vec![parse_ring(w, ex, "exterior")?];
    for h in holes {
        rings.push(parse_ring(w, h, "interior")?);
    }
    Ok(rings)
}

/// The facility, set and the expected feature description from the message's own name.
fn parse_chart_name(w: &W<'_>, d: ElemIx) -> Result<([u8; MAX_FACILITY_BYTES], SetKind), MvaError> {
    let t = leaf_text(w, d, "message description")?;
    let t = trim(&t);
    let bad = MvaError::BadValue {
        field: "message description",
        at: at(w, d),
    };
    // `ID_MVA_FUS3_2024`, `ID_MVA_FUS5_2024_v1`, `ZAU_TAV_2025`: the first tokens name the
    // chart; the rest is the FAA's edition label and is not interpreted.
    let mut parts = t.split(|&c| c == b'_');
    let fac = parts.next().ok_or_else(|| bad.clone())?;
    if fac.len() < 2
        || fac.len() > MAX_FACILITY_BYTES
        || !fac
            .iter()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    {
        return Err(bad);
    }
    let set = match (parts.next(), parts.next()) {
        (Some(b"MVA"), Some(b"FUS3")) => SetKind::Fus3,
        (Some(b"MVA"), Some(b"FUS5")) => SetKind::Fus5,
        (Some(b"TAV"), _) => SetKind::Tav,
        _ => return Err(bad),
    };
    let mut f = [0u8; MAX_FACILITY_BYTES];
    for (d, s) in f.iter_mut().zip(fac) {
        *d = *s;
    }
    Ok((f, set))
}

/// Where the model ended before a message, for rollback.
struct Marks {
    sectors: usize,
    rings: usize,
    verts: usize,
    charts: usize,
}

fn marks(m: &Model) -> Marks {
    Marks {
        sectors: m.min_alt_ft.len(),
        rings: m.ring_vert_count.len(),
        verts: m.vert_lat_e7.len(),
        charts: m.chart_set.len(),
    }
}

fn rollback(m: &mut Model, k: &Marks) {
    m.facility.truncate(k.sectors);
    m.set.truncate(k.sectors);
    m.sector_id.truncate(k.sectors);
    m.min_alt_ft.truncate(k.sectors);
    m.ring_first.truncate(k.sectors);
    m.ring_count.truncate(k.sectors);
    m.bbox_lat_min.truncate(k.sectors);
    m.bbox_lat_max.truncate(k.sectors);
    m.bbox_lon_min.truncate(k.sectors);
    m.bbox_lon_max.truncate(k.sectors);
    m.ring_vert_first.truncate(k.rings);
    m.ring_vert_count.truncate(k.rings);
    m.ring_hole.truncate(k.rings.div_ceil(64));
    if !k.rings.is_multiple_of(64) {
        if let Some(w) = m.ring_hole.last_mut() {
            *w &= (1u64 << (k.rings % 64)) - 1;
        }
    }
    m.vert_lat_e7.truncate(k.verts);
    m.vert_lon_e7.truncate(k.verts);
    m.chart_facility.truncate(k.charts);
    m.chart_set.truncate(k.charts);
    m.chart_first_sector.truncate(k.charts);
    m.chart_sector_count.truncate(k.charts);
}

fn append(
    m: &mut Model,
    fac: [u8; MAX_FACILITY_BYTES],
    set: SetKind,
    secs: &[Sector],
) -> Result<Loaded, MvaError> {
    let (s0, r0, v0) = (
        m.min_alt_ft.len(),
        m.ring_vert_count.len(),
        m.vert_lat_e7.len(),
    );
    for s in secs {
        let ring_first = m.ring_vert_count.len();
        for (ri, ring) in s.rings.iter().enumerate() {
            let r = m.ring_vert_count.len();
            m.ring_vert_first.push(m.vert_lat_e7.len() as u32);
            m.ring_vert_count.push(ring.lat.len() as u32);
            if ri > 0 {
                let word = r >> 6;
                if m.ring_hole.len() <= word {
                    m.ring_hole.resize(word + 1, 0);
                }
                if let Some(wd) = m.ring_hole.get_mut(word) {
                    *wd |= 1u64 << (r & 63);
                }
            }
            m.vert_lat_e7.extend_from_slice(&ring.lat);
            m.vert_lon_e7.extend_from_slice(&ring.lon);
        }
        let ext = s.rings.first();
        let min = |v: Option<&Ring>, lat: bool| {
            v.and_then(|r| if lat { r.lat.iter() } else { r.lon.iter() }.min().copied())
        };
        let max = |v: Option<&Ring>, lat: bool| {
            v.and_then(|r| if lat { r.lat.iter() } else { r.lon.iter() }.max().copied())
        };
        let (la0, la1, lo0, lo1) = (
            min(ext, true).unwrap_or(0),
            max(ext, true).unwrap_or(0),
            min(ext, false).unwrap_or(0),
            max(ext, false).unwrap_or(0),
        );
        let span = MAX_SECTOR_SPAN_DEG * E7;
        if i64::from(la1) - i64::from(la0) > span || i64::from(lo1) - i64::from(lo0) > span {
            return Err(MvaError::Limit {
                what: "sector extent in degrees",
                max: MAX_SECTOR_SPAN_DEG as u64,
                at: s.at,
            });
        }
        m.facility.push(fac);
        m.set.push(set);
        m.sector_id.push(s.id);
        m.min_alt_ft.push(s.alt);
        m.ring_first.push(ring_first as u32);
        m.ring_count.push(s.rings.len() as u32);
        m.bbox_lat_min.push(la0);
        m.bbox_lat_max.push(la1);
        m.bbox_lon_min.push(lo0);
        m.bbox_lon_max.push(lo1);
    }
    let chart = m.chart_set.len();
    m.chart_facility.push(fac);
    m.chart_set.push(set);
    m.chart_first_sector.push(s0 as u32);
    m.chart_sector_count.push(secs.len() as u32);
    Ok(Loaded {
        chart,
        sectors: secs.len(),
        rings: m.ring_vert_count.len() - r0,
        vertices: m.vert_lat_e7.len() - v0,
    })
}

/// Parses one message with `parser` and adds its chart to `m`. On any error the model is left
/// exactly as it was.
pub fn load_message(m: &mut Model, parser: &mut Parser, input: &[u8]) -> Result<Loaded, MvaError> {
    if input.len() as u64 > MAX_FILE_BYTES {
        return Err(MvaError::FileTooLarge(input.len() as u64));
    }
    let k = marks(m);
    let r = load_inner(m, parser, input);
    if r.is_err() {
        rollback(m, &k);
    }
    r
}

fn load_inner(m: &mut Model, parser: &mut Parser, input: &[u8]) -> Result<Loaded, MvaError> {
    let doc = parser.parse(input).map_err(MvaError::Xml)?;
    let mut texts = vec![NO_TEXT; doc.elem_name.len()];
    for (t, owner) in doc.text_owner.iter().enumerate() {
        if let Some(slot) = texts.get_mut(owner.0 as usize) {
            *slot = if *slot == NO_TEXT {
                t as u32
            } else {
                MANY_TEXT
            };
        }
    }
    let w = W { doc, texts };
    let root = ElemIx(0);
    if qname(&w, root) != (Q::Msg, b"AIXMBasicMessage".as_slice()) {
        return Err(MvaError::NotAixmMessage { at: at(&w, root) });
    }
    check_attributes(&w)?;
    let ks = only(
        &w,
        root,
        "AIXMBasicMessage",
        &[
            (Q::Gml, "description"),
            (Q::Gml, "boundedBy"),
            (Q::Aixm, "sequenceNumber"),
            (Q::Aixm, "messageMetadata"),
            (Q::Msg, "hasMember"),
        ],
    )?;
    let c = "AIXMBasicMessage";
    let d = one(&w, root, &ks, c, Q::Gml, "description")?;
    let (fac, set) = parse_chart_name(&w, d)?;
    if let Some(b) = opt(&w, root, &ks, c, Q::Gml, "boundedBy")? {
        empty(&w, b, "boundedBy")?;
    }
    empty_opt(&w, root, &ks, c, Q::Aixm, "messageMetadata")?;
    if let Some(sn) = opt(&w, root, &ks, c, Q::Aixm, "sequenceNumber")? {
        let t = leaf_text(&w, sn, "message sequenceNumber")?;
        if trim(&t).is_empty() || !trim(&t).iter().all(u8::is_ascii_digit) {
            return Err(MvaError::BadValue {
                field: "message sequenceNumber",
                at: at(&w, sn),
            });
        }
    }
    if m.chart_facility
        .iter()
        .zip(&m.chart_set)
        .any(|(f, s)| *f == fac && *s == set)
    {
        return Err(MvaError::DuplicateChart {
            facility: short_name(trim_nul(&fac)),
            set: set.name(),
        });
    }
    if m.chart_set.len() >= MAX_CHARTS {
        return Err(MvaError::Limit {
            what: "charts",
            max: MAX_CHARTS as u64,
            at: at(&w, root),
        });
    }
    let members = matching(&w, &ks, Q::Msg, "hasMember");
    if members.len() > MAX_SECTORS_PER_CHART {
        return Err(MvaError::Limit {
            what: "sectors per chart",
            max: MAX_SECTORS_PER_CHART as u64,
            at: at(&w, root),
        });
    }
    if members.is_empty() {
        return Err(MvaError::Cardinality {
            context: c,
            name: "hasMember",
            found: 0,
            at: at(&w, root),
        });
    }
    let want: &[u8] = if set == SetKind::Tav {
        DESC_MIA
    } else {
        DESC_MVA
    };
    let mut secs: Vec<Sector> = Vec::with_capacity(members.len());
    for hm in members {
        let mk = only(&w, hm, "hasMember", &[(Q::Aixm, "Airspace")])?;
        let a = one(&w, hm, &mk, "hasMember", Q::Aixm, "Airspace")?;
        let s = parse_airspace(&w, a, want)?;
        if secs.iter().any(|o| o.id == s.id) {
            return Err(MvaError::DuplicateSector {
                name: short_name(trim_nul(&s.id)),
                at: s.at,
            });
        }
        secs.push(s);
    }
    let rings: usize = secs.iter().map(|s| s.rings.len()).sum();
    let verts: usize = secs
        .iter()
        .flat_map(|s| s.rings.iter())
        .map(|r| r.lat.len())
        .sum();
    if m.min_alt_ft.len() + secs.len() > MAX_SECTORS {
        return Err(MvaError::Limit {
            what: "sectors in the model",
            max: MAX_SECTORS as u64,
            at: at(&w, root),
        });
    }
    if m.vert_lat_e7.len() + verts > MAX_VERTICES || rings > u32::MAX as usize {
        return Err(MvaError::Limit {
            what: "vertices in the model",
            max: MAX_VERTICES as u64,
            at: at(&w, root),
        });
    }
    append(m, fac, set, &secs)
}

fn trim_nul(b: &[u8]) -> &[u8] {
    let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    b.get(..n).unwrap_or(&[])
}

/// The outcome of loading a directory of charts. Plain data (A1).
// A7.2: cold struct, size guard waived (one per run)
#[derive(Debug)]
pub struct DirLoad {
    pub model: Model,
    /// Each `*.xml` file in file-name order with what loading it produced; a refused file added
    /// nothing to the model.
    pub files: Vec<(String, Result<Loaded, MvaError>)>,
}

/// Shell: loads every `*.xml` file of `dir`, in file-name order, into one model. A file that is
/// refused is reported in [`DirLoad::files`] and the rest still load.
pub fn load_directory(dir: &Path) -> Result<DirLoad, MvaError> {
    let mut names: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| MvaError::Io(e.to_string()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "xml"))
        .collect();
    names.sort();
    let mut parser = new_parser()?;
    let mut model = Model::default();
    let mut files = Vec::new();
    for p in names {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let r = read_file(&p).and_then(|b| load_message(&mut model, &mut parser, &b));
        files.push((name, r));
    }
    Ok(DirLoad { model, files })
}
