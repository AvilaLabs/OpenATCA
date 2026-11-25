//! IWXXM 2025-2 METAR/SPECI subset reader. Allocation and XML parsing stay in this shell.

use crate::{
    Measure, NilValue, NilValueObservation, Observation, PermissibleUsage, QuantityUnit,
    RelationalOperator, Report, ReportKind, ReportStatus, WeatherCode, MAX_ATTRIBUTES,
    MAX_DECODE_WORK, MAX_DOCUMENT_BYTES, MAX_ELEMENTS, MAX_NAMESPACE_BINDINGS, MAX_SCALAR_BYTES,
    MAX_TEXT_NODES,
};
use oa_xml::{
    attr_needs_decode, decode_into, resolve_attr_ns, resolve_elem_ns, span_bytes, Limits, Ns,
    Parser, SpanKind,
};
use std::fmt;

const IWXXM: &[u8] = b"http://icao.int/iwxxm/2025-2";
const AIXM: &[u8] = b"http://www.aixm.aero/schema/5.1.1";
const GML: &[u8] = b"http://www.opengis.net/gml/3.2";
const XSI: &[u8] = b"http://www.w3.org/2001/XMLSchema-instance";

/// Non-sensitive outcome counters for a single document.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DecodeStats {
    pub elements_seen: u32,
    pub attributes_seen: u32,
    pub unsupported_constructs: u32,
    pub refused_constructs: u32,
    pub work_units: u32,
}

/// A successful decode or a bounded, visible refusal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodeResult {
    pub report: Report,
    pub stats: DecodeStats,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeFailure {
    InputTooLarge,
    Xml,
    UnsupportedRoot,
    UnsupportedNamespace,
    UnsupportedConstruct,
    MissingRequiredField,
    InvalidValue,
    Limit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodeError {
    pub failure: DecodeFailure,
    pub stats: DecodeStats,
    /// Narrow parser phase label for diagnosing bounded refusals.
    pub context: &'static str,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "IWXXM decode refused: {:?}", self.failure)
    }
}

impl std::error::Error for DecodeError {}

fn err(failure: DecodeFailure, stats: DecodeStats) -> DecodeError {
    DecodeError {
        failure,
        stats,
        context: "document",
    }
}

fn in_context<T>(result: Result<T, DecodeError>, context: &'static str) -> Result<T, DecodeError> {
    result.map_err(|mut error| {
        if error.context == "document" {
            error.context = context;
        }
        error
    })
}

/// Decode one bounded IWXXM 2025-2 METAR or SPECI report.
///
/// A present field outside the supported subset refuses the whole report. This prevents a
/// successful-looking normalized row from concealing hazards or operational fields.
pub fn decode_report(input: &[u8]) -> Result<DecodeResult, DecodeError> {
    let mut stats = DecodeStats::default();
    if input.len() > MAX_DOCUMENT_BYTES {
        return Err(err(DecodeFailure::InputTooLarge, stats));
    }
    let mut parser = Parser::new(Limits {
        max_input_bytes: MAX_DOCUMENT_BYTES,
        max_depth: 64,
        max_elements: MAX_ELEMENTS,
        max_attributes: MAX_ATTRIBUTES,
        max_attrs_per_elem: 64,
        max_name_len: 256,
        max_text_nodes: MAX_TEXT_NODES,
        max_ns_bindings: MAX_NAMESPACE_BINDINGS,
    })
    .map_err(|_| err(DecodeFailure::Limit, stats))?;
    let doc = parser
        .parse(input)
        .map_err(|_| err(DecodeFailure::Xml, stats))?;
    stats.elements_seen = u32::try_from(doc.elem_name.len()).unwrap_or(u32::MAX);
    stats.attributes_seen = u32::try_from(doc.attr_name.len()).unwrap_or(u32::MAX);
    if doc.elem_name.is_empty() {
        return Err(err(DecodeFailure::UnsupportedRoot, stats));
    }
    let root = 0usize;
    no_significant_text(&doc, root, input, &mut stats)?;
    if !elem_is(&doc, root, input, IWXXM)? {
        return Err(err(DecodeFailure::UnsupportedNamespace, stats));
    }
    require_gml_id(&doc, root, input, &mut stats)?;
    let root_name = name(&doc, root, input)?;
    let kind = match root_name {
        b"METAR" => ReportKind::Metar,
        b"SPECI" => ReportKind::Speci,
        _ => return Err(err(DecodeFailure::UnsupportedRoot, stats)),
    };
    in_context(
        check_attributes(
            &doc,
            root,
            input,
            &[b"reportStatus", b"permissibleUsage", b"automatedStation"],
            &mut stats,
        ),
        "root-attributes",
    )?;
    let status = match attr_value(&doc, root, input, b"reportStatus", &mut stats)? {
        Some(b"NORMAL") => ReportStatus::Normal,
        Some(b"AMENDMENT") => ReportStatus::Amendment,
        Some(b"CORRECTION") => ReportStatus::Correction,
        _ => return Err(err(DecodeFailure::InvalidValue, stats)),
    };
    let permissible_usage = match attr_value(&doc, root, input, b"permissibleUsage", &mut stats)? {
        Some(b"OPERATIONAL") => PermissibleUsage::Operational,
        Some(b"NON-OPERATIONAL") => PermissibleUsage::NonOperational,
        _ => return Err(err(DecodeFailure::InvalidValue, stats)),
    };
    let automated_station = match attr_value(&doc, root, input, b"automatedStation", &mut stats)? {
        Some(b"true") | Some(b"1") => Some(true),
        Some(b"false") | Some(b"0") => Some(false),
        None => None,
        _ => return Err(err(DecodeFailure::InvalidValue, stats)),
    };

    let mut issue_time = None;
    let mut observation_time = None;
    let mut station = None;
    let mut observation_ix = None;
    let mut next_root_child = 0u8;
    let mut child = first_child(&doc, root);
    while let Some(ix) = child {
        work(&mut stats)?;
        let local = name(&doc, ix, input)?;
        if elem_is(&doc, ix, input, IWXXM)? && local == b"issueTime" {
            if next_root_child != 0 {
                return Err(err(DecodeFailure::InvalidValue, stats));
            }
            set_once(
                &mut issue_time,
                in_context(time_reference(&doc, ix, input, &mut stats), "issue-time")?,
                &stats,
            )?;
            next_root_child = 1;
        } else if elem_is(&doc, ix, input, IWXXM)? && local == b"observationTime" {
            if next_root_child != 2 {
                return Err(err(DecodeFailure::InvalidValue, stats));
            }
            set_once(
                &mut observation_time,
                in_context(
                    time_reference(&doc, ix, input, &mut stats),
                    "observation-time",
                )?,
                &stats,
            )?;
            next_root_child = 3;
        } else if elem_is(&doc, ix, input, IWXXM)? && local == b"aerodrome" {
            if next_root_child != 1 {
                return Err(err(DecodeFailure::InvalidValue, stats));
            }
            set_once(
                &mut station,
                in_context(aerodrome(&doc, ix, input, &mut stats), "aerodrome")?,
                &stats,
            )?;
            next_root_child = 2;
        } else if elem_is(&doc, ix, input, IWXXM)? && local == b"observation" {
            if next_root_child != 3 {
                return Err(err(DecodeFailure::InvalidValue, stats));
            }
            set_once(&mut observation_ix, ix, &stats)?;
            next_root_child = 4;
        } else {
            refuse(&mut stats);
            let context = if elem_is(&doc, ix, input, IWXXM)? {
                "unexpected-iwxxm-root-child"
            } else {
                "non-iwxxm-root-child"
            };
            return Err(DecodeError {
                failure: DecodeFailure::UnsupportedConstruct,
                stats,
                context,
            });
        }
        child = next_sibling(&doc, ix);
    }
    let issue_time_ms =
        issue_time.ok_or_else(|| err(DecodeFailure::MissingRequiredField, stats))?;
    let observation_time_ms =
        observation_time.ok_or_else(|| err(DecodeFailure::MissingRequiredField, stats))?;
    let station = station.ok_or_else(|| err(DecodeFailure::MissingRequiredField, stats))?;

    let observation = match observation_ix {
        Some(ix) => in_context(
            decode_observation(&doc, ix, input, &mut stats),
            "observation",
        )?,
        None => NilValueObservation::Missing,
    };
    Ok(DecodeResult {
        report: Report {
            kind,
            status,
            permissible_usage,
            station,
            issue_time_ms,
            observation_time_ms,
            automated_station,
            observation,
        },
        stats,
    })
}

fn refuse(stats: &mut DecodeStats) {
    stats.unsupported_constructs = stats.unsupported_constructs.saturating_add(1);
    stats.refused_constructs = stats.refused_constructs.saturating_add(1);
}

fn work(stats: &mut DecodeStats) -> Result<(), DecodeError> {
    stats.work_units = stats.work_units.saturating_add(1);
    if stats.work_units as usize > MAX_DECODE_WORK {
        return Err(err(DecodeFailure::Limit, *stats));
    }
    Ok(())
}

fn elem_is(
    doc: &oa_xml::Doc<'_>,
    ix: usize,
    input: &[u8],
    want: &[u8],
) -> Result<bool, DecodeError> {
    match resolve_elem_ns(doc, oa_xml::ElemIx(ix as u32)) {
        Ns::Uri(span) => Ok(oa_xml::ns_uri_bytes(doc, span) == Some(want)),
        _ => {
            let _ = input;
            Ok(false)
        }
    }
}

fn name<'a>(doc: &oa_xml::Doc<'a>, ix: usize, input: &'a [u8]) -> Result<&'a [u8], DecodeError> {
    let s = *doc
        .elem_name
        .get(ix)
        .ok_or_else(|| err(DecodeFailure::InvalidValue, DecodeStats::default()))?;
    span_bytes(input, s).map_err(|_| err(DecodeFailure::InvalidValue, DecodeStats::default()))
}

fn first_child(doc: &oa_xml::Doc<'_>, parent: usize) -> Option<usize> {
    let ix = *doc.elem_first_child.get(parent)?;
    (ix != oa_xml::ElemIx::NONE).then_some(ix.0 as usize)
}

fn next_sibling(doc: &oa_xml::Doc<'_>, ix: usize) -> Option<usize> {
    let next = *doc.elem_next_sibling.get(ix)?;
    (next != oa_xml::ElemIx::NONE).then_some(next.0 as usize)
}

fn attr_value<'a>(
    doc: &oa_xml::Doc<'a>,
    elem: usize,
    input: &'a [u8],
    want: &[u8],
    stats: &mut DecodeStats,
) -> Result<Option<&'a [u8]>, DecodeError> {
    let first = *doc
        .elem_first_attr
        .get(elem)
        .ok_or_else(|| err(DecodeFailure::InvalidValue, DecodeStats::default()))?;
    let count = *doc
        .elem_attr_count
        .get(elem)
        .ok_or_else(|| err(DecodeFailure::InvalidValue, DecodeStats::default()))?
        as usize;
    for i in first.0 as usize..first.0 as usize + count {
        work(stats)?;
        let n = span_bytes(
            input,
            *doc.attr_name
                .get(i)
                .ok_or_else(|| err(DecodeFailure::InvalidValue, DecodeStats::default()))?,
        )
        .map_err(|_| err(DecodeFailure::InvalidValue, DecodeStats::default()))?;
        if n == want {
            if !matches!(doc.attr_prefix.get(i), Some(p) if p.len == 0) {
                return Err(err(DecodeFailure::InvalidValue, *stats));
            }
            let s = *doc
                .attr_value
                .get(i)
                .ok_or_else(|| err(DecodeFailure::InvalidValue, DecodeStats::default()))?;
            if attr_needs_decode(doc, oa_xml::AttrIx(i as u32)) {
                return Err(err(DecodeFailure::InvalidValue, *stats));
            }
            return span_bytes(input, s)
                .map(Some)
                .map_err(|_| err(DecodeFailure::InvalidValue, *stats));
        }
    }
    Ok(None)
}

fn check_attributes(
    doc: &oa_xml::Doc<'_>,
    elem: usize,
    input: &[u8],
    allowed_unqualified: &[&[u8]],
    stats: &mut DecodeStats,
) -> Result<(), DecodeError> {
    let first = *doc
        .elem_first_attr
        .get(elem)
        .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?;
    let count = *doc
        .elem_attr_count
        .get(elem)
        .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))? as usize;
    for i in first.0 as usize..first.0 as usize + count {
        work(stats)?;
        let prefix = span_bytes(
            input,
            *doc.attr_prefix
                .get(i)
                .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?,
        )
        .map_err(|_| err(DecodeFailure::InvalidValue, *stats))?;
        let local = span_bytes(
            input,
            *doc.attr_name
                .get(i)
                .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?,
        )
        .map_err(|_| err(DecodeFailure::InvalidValue, *stats))?;
        // Namespace declarations are retained as XML attributes by oa-xml, but they are not
        // application attributes. Their values and reserved-prefix rules were validated by the
        // parser; don't treat `xmlns:iwxxm` as an unqualified IWXXM field here.
        if prefix == b"xmlns" || prefix.is_empty() && local == b"xmlns" {
            continue;
        }
        let accepted =
            match resolve_attr_ns(doc, oa_xml::ElemIx(elem as u32), oa_xml::AttrIx(i as u32)) {
                Ns::None => allowed_unqualified.contains(&local),
                Ns::Uri(ns) => {
                    let uri = oa_xml::ns_uri_bytes(doc, ns)
                        .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?;
                    // This profile is inline-only; GML ownership controls are not consumed.
                    uri == GML && local == b"id" || uri == XSI && local == b"schemaLocation"
                }
                Ns::Xml => local == b"lang",
            };
        if !accepted {
            refuse(stats);
            return Err(err(DecodeFailure::UnsupportedConstruct, *stats));
        }
    }
    Ok(())
}

fn require_gml_id(
    doc: &oa_xml::Doc<'_>,
    elem: usize,
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<(), DecodeError> {
    let first = *doc
        .elem_first_attr
        .get(elem)
        .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?;
    let count = *doc
        .elem_attr_count
        .get(elem)
        .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))? as usize;
    for i in first.0 as usize..first.0 as usize + count {
        work(stats)?;
        let local = span_bytes(
            input,
            *doc.attr_name
                .get(i)
                .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?,
        )
        .map_err(|_| err(DecodeFailure::InvalidValue, *stats))?;
        if local == b"id" {
            if let Ns::Uri(ns) =
                resolve_attr_ns(doc, oa_xml::ElemIx(elem as u32), oa_xml::AttrIx(i as u32))
            {
                if oa_xml::ns_uri_bytes(doc, ns) == Some(GML) {
                    let value = *doc
                        .attr_value
                        .get(i)
                        .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?;
                    if value.len == 0
                        || value.len as usize > MAX_SCALAR_BYTES
                        || attr_needs_decode(doc, oa_xml::AttrIx(i as u32))
                    {
                        return Err(err(DecodeFailure::InvalidValue, *stats));
                    }
                    return Ok(());
                }
            }
        }
    }
    Err(err(DecodeFailure::MissingRequiredField, *stats))
}

fn scalar_text(
    doc: &oa_xml::Doc<'_>,
    elem: usize,
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<Vec<u8>, DecodeError> {
    if first_child(doc, elem).is_some() {
        return Err(err(DecodeFailure::InvalidValue, *stats));
    }
    work(stats)?;
    let mut out = Vec::new();
    let mut found = false;
    for (i, owner) in doc.text_owner.iter().enumerate() {
        work(stats)?;
        if owner.0 as usize == elem {
            if out.len() >= MAX_SCALAR_BYTES {
                return Err(err(DecodeFailure::Limit, *stats));
            }
            let span = *doc
                .text_span
                .get(i)
                .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?;
            let mut buf = [0u8; MAX_SCALAR_BYTES];
            let len = decode_into(span, input, &mut buf, SpanKind::Text)
                .map_err(|_| err(DecodeFailure::InvalidValue, *stats))?;
            if out
                .len()
                .checked_add(len)
                .filter(|n| *n <= MAX_SCALAR_BYTES)
                .is_none()
            {
                return Err(err(DecodeFailure::Limit, *stats));
            }
            let decoded = buf
                .get(..len)
                .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?;
            out.extend_from_slice(decoded);
            found = true;
        }
    }
    if !found {
        return Err(err(DecodeFailure::MissingRequiredField, *stats));
    }
    let trimmed = trim_xml_ws(&out).ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?;
    Ok(trimmed.to_vec())
}

fn trim_xml_ws(v: &[u8]) -> Option<&[u8]> {
    let mut a = 0;
    let mut b = v.len();
    while a < b {
        let byte = *v.get(a)?;
        if !matches!(byte, b' ' | b'\t' | b'\r' | b'\n') {
            break;
        }
        a = a.checked_add(1)?;
    }
    while b > a {
        let end = b.checked_sub(1)?;
        let byte = *v.get(end)?;
        if !matches!(byte, b' ' | b'\t' | b'\r' | b'\n') {
            break;
        }
        b = end;
    }
    v.get(a..b)
}

fn time_reference(
    doc: &oa_xml::Doc<'_>,
    wrapper: usize,
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<i64, DecodeError> {
    check_attributes(doc, wrapper, input, &[], stats)?;
    let time = only_child(doc, wrapper, GML, b"TimeInstant", input, stats)?;
    require_gml_id(doc, time, input, stats)?;
    check_attributes(doc, time, input, &[], stats)?;
    let pos = only_child(doc, time, GML, b"timePosition", input, stats)?;
    check_attributes(doc, pos, input, &[], stats)?;
    let text = scalar_text(doc, pos, input, stats)?;
    parse_time(&text).ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))
}

fn aerodrome(
    doc: &oa_xml::Doc<'_>,
    wrapper: usize,
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<String, DecodeError> {
    check_attributes(doc, wrapper, input, &[], stats)?;
    let aerodrome = only_child(doc, wrapper, AIXM, b"AirportHeliport", input, stats)?;
    require_gml_id(doc, aerodrome, input, stats)?;
    check_attributes(doc, aerodrome, input, &[], stats)?;
    // AirportHeliport has required sibling metadata such as gml:identifier; extract exactly one
    // inline timeSlice while leaving the other AIXM feature properties outside this projection.
    let time_slice = unique_child(doc, aerodrome, AIXM, b"timeSlice", input, stats)?;
    check_attributes(doc, time_slice, input, &[], stats)?;
    let slice = only_child(
        doc,
        time_slice,
        AIXM,
        b"AirportHeliportTimeSlice",
        input,
        stats,
    )?;
    require_gml_id(doc, slice, input, stats)?;
    check_attributes(doc, slice, input, &[], stats)?;
    validate_aixm_slice_time(doc, slice, input, stats)?;
    let interpretation = unique_child(doc, slice, AIXM, b"interpretation", input, stats)?;
    check_attributes(doc, interpretation, input, &[], stats)?;
    if scalar_text(doc, interpretation, input, stats)?.as_slice() != b"BASELINE" {
        refuse(stats);
        return Err(err(DecodeFailure::UnsupportedConstruct, *stats));
    }
    let designator = unique_child(doc, slice, AIXM, b"designator", input, stats)?;
    check_attributes(doc, designator, input, &[], stats)?;
    let raw = scalar_text(doc, designator, input, stats)?;
    if raw.len() < 3 || raw.len() > 8 || !raw.iter().all(u8::is_ascii_alphanumeric) {
        return Err(err(DecodeFailure::InvalidValue, *stats));
    }
    String::from_utf8(raw).map_err(|_| err(DecodeFailure::InvalidValue, *stats))
}

fn validate_aixm_slice_time(
    doc: &oa_xml::Doc<'_>,
    slice: usize,
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<(), DecodeError> {
    // AirportHeliportTimeSlice contains other required siblings such as interpretation and
    // designator; select its one GML validTime property without rejecting those AIXM fields.
    let valid_time = unique_child(doc, slice, GML, b"validTime", input, stats)?;
    check_attributes(doc, valid_time, input, &[], stats)?;
    let period = only_child(doc, valid_time, GML, b"TimePeriod", input, stats)?;
    require_gml_id(doc, period, input, stats)?;
    let (begin, end) = exact_children2(
        doc,
        period,
        GML,
        b"beginPosition",
        b"endPosition",
        input,
        stats,
    )?;
    check_attributes(doc, begin, input, &[], stats)?;
    let begin_ms = parse_time(&scalar_text(doc, begin, input, stats)?)
        .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?;
    check_attributes(doc, end, input, &[b"indeterminatePosition"], stats)?;
    match attr_value(doc, end, input, b"indeterminatePosition", stats)? {
        Some(b"unknown") => {
            if first_child(doc, end).is_some() {
                return Err(err(DecodeFailure::InvalidValue, *stats));
            }
            no_significant_text(doc, end, input, stats)?;
            Ok(())
        }
        Some(_) => Err(err(DecodeFailure::UnsupportedConstruct, *stats)),
        None => {
            let end_ms = parse_time(&scalar_text(doc, end, input, stats)?)
                .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?;
            if end_ms < begin_ms {
                return Err(err(DecodeFailure::InvalidValue, *stats));
            }
            Ok(())
        }
    }
}

fn unique_child(
    doc: &oa_xml::Doc<'_>,
    parent: usize,
    uri: &[u8],
    local: &[u8],
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<usize, DecodeError> {
    no_significant_text(doc, parent, input, stats)?;
    let mut found = None;
    let mut child = first_child(doc, parent);
    while let Some(ix) = child {
        work(stats)?;
        if elem_is(doc, ix, input, uri)?
            && name(doc, ix, input)? == local
            && found.replace(ix).is_some()
        {
            return Err(err(DecodeFailure::InvalidValue, *stats));
        }
        child = next_sibling(doc, ix);
    }
    found.ok_or_else(|| err(DecodeFailure::MissingRequiredField, *stats))
}

/// Require a property wrapper to contain precisely one inline element of the expected expanded
/// name. This is intentionally narrower than the schema's remote-reference alternatives.
fn only_child(
    doc: &oa_xml::Doc<'_>,
    parent: usize,
    uri: &[u8],
    local: &[u8],
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<usize, DecodeError> {
    no_significant_text(doc, parent, input, stats)?;
    let Some(ix) = first_child(doc, parent) else {
        return Err(err(DecodeFailure::MissingRequiredField, *stats));
    };
    work(stats)?;
    if !elem_is(doc, ix, input, uri)? || name(doc, ix, input)? != local {
        refuse(stats);
        return Err(err(DecodeFailure::UnsupportedConstruct, *stats));
    }
    if next_sibling(doc, ix).is_some() {
        work(stats)?;
        refuse(stats);
        return Err(err(DecodeFailure::UnsupportedConstruct, *stats));
    }
    Ok(ix)
}

/// Require the two explicit temporal endpoints accepted by this subset, in schema order.
fn exact_children2(
    doc: &oa_xml::Doc<'_>,
    parent: usize,
    uri: &[u8],
    first: &[u8],
    second: &[u8],
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<(usize, usize), DecodeError> {
    no_significant_text(doc, parent, input, stats)?;
    let Some(a) = first_child(doc, parent) else {
        return Err(err(DecodeFailure::MissingRequiredField, *stats));
    };
    work(stats)?;
    let Some(b) = next_sibling(doc, a) else {
        return Err(err(DecodeFailure::MissingRequiredField, *stats));
    };
    work(stats)?;
    if next_sibling(doc, b).is_some() {
        work(stats)?;
        refuse(stats);
        return Err(err(DecodeFailure::UnsupportedConstruct, *stats));
    }
    if !elem_is(doc, a, input, uri)?
        || name(doc, a, input)? != first
        || !elem_is(doc, b, input, uri)?
        || name(doc, b, input)? != second
    {
        refuse(stats);
        return Err(err(DecodeFailure::UnsupportedConstruct, *stats));
    }
    Ok((a, b))
}

/// Element-only property wrappers may contain formatting whitespace, but no hidden text payload.
/// Charge every text-table visit so malformed structures cannot evade the per-call work ceiling.
fn no_significant_text(
    doc: &oa_xml::Doc<'_>,
    elem: usize,
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<(), DecodeError> {
    for (i, owner) in doc.text_owner.iter().enumerate() {
        work(stats)?;
        if owner.0 as usize != elem {
            continue;
        }
        let span = *doc
            .text_span
            .get(i)
            .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?;
        let raw = span_bytes(input, span).map_err(|_| err(DecodeFailure::InvalidValue, *stats))?;
        if trim_xml_ws(raw)
            .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?
            .is_empty()
        {
            continue;
        }
        if raw.len() > MAX_SCALAR_BYTES {
            return Err(err(DecodeFailure::Limit, *stats));
        }
        let mut buf = [0u8; MAX_SCALAR_BYTES];
        let len = decode_into(span, input, &mut buf, SpanKind::Text)
            .map_err(|_| err(DecodeFailure::Limit, *stats))?;
        let decoded = buf
            .get(..len)
            .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?;
        if !trim_xml_ws(decoded)
            .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?
            .is_empty()
        {
            return Err(err(DecodeFailure::InvalidValue, *stats));
        }
    }
    Ok(())
}

pub(crate) fn parse_time(v: &[u8]) -> Option<i64> {
    // Supported XML Schema dateTime subset: positive four-digit years, UTC `Z` only, and at most
    // millisecond precision. Offsets are deliberately refused until XML Schema's ±14:00 bound
    // and canonicalization rules are implemented here.
    if v.len() < 20 || v.last() != Some(&b'Z') {
        return None;
    }
    let year = dec(v.get(0..4)?)? as i32;
    let month = dec(v.get(5..7)?)? as u32;
    let day = dec(v.get(8..10)?)? as u32;
    if v.get(4) != Some(&b'-')
        || v.get(7) != Some(&b'-')
        || v.get(10) != Some(&b'T')
        || v.get(13) != Some(&b':')
        || v.get(16) != Some(&b':')
    {
        return None;
    }
    let hour = dec(v.get(11..13)?)? as i64;
    let minute = dec(v.get(14..16)?)? as i64;
    let second = dec(v.get(17..19)?)? as i64;
    if year == 0
        || month == 0
        || month > 12
        || day == 0
        || day > month_days(year, month)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    let mut millis = 0i64;
    let mut end = 19usize;
    if v.get(end) == Some(&b'.') {
        end += 1;
        let start = end;
        while v.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
        }
        let digits = end - start;
        if !(1..=3).contains(&digits) {
            return None;
        }
        let n = dec(v.get(start..end)?)? as i64;
        millis = match digits {
            1 => n * 100,
            2 => n * 10,
            _ => n,
        };
    }
    if v.get(end..) != Some(b"Z") {
        return None;
    }
    let days = days_from_civil(year, month, day);
    days.checked_mul(86_400_000)?
        .checked_add(hour * 3_600_000 + minute * 60_000 + second * 1_000 + millis)
}

fn dec(v: &[u8]) -> Option<u32> {
    let mut n = 0u32;
    if v.is_empty() {
        return None;
    }
    for b in v {
        if !b.is_ascii_digit() {
            return None;
        }
        n = n.checked_mul(10)?.checked_add((*b - b'0') as u32)?;
    }
    Some(n)
}
fn month_days(y: i32, m: u32) -> u32 {
    match m {
        2 if leap(y) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}
fn leap(y: i32) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = y - if m <= 2 { 1 } else { 0 };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = m as i32 + if m > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + d as i32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) as i64
}

fn decode_observation(
    doc: &oa_xml::Doc<'_>,
    wrapper: usize,
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<NilValueObservation, DecodeError> {
    check_attributes(doc, wrapper, input, &[b"nilReason"], stats)?;
    if let Some(reason) = attr_value(doc, wrapper, input, b"nilReason", stats)? {
        if first_child(doc, wrapper).is_some() {
            return Err(err(DecodeFailure::InvalidValue, *stats));
        }
        no_significant_text(doc, wrapper, input, stats)?;
        if reason.is_empty() {
            return Err(err(DecodeFailure::InvalidValue, *stats));
        }
        if reason.len() > MAX_SCALAR_BYTES {
            return Err(err(DecodeFailure::Limit, *stats));
        }
        let reason_uri = Some(
            String::from_utf8(reason.to_vec())
                .map_err(|_| err(DecodeFailure::InvalidValue, *stats))?,
        );
        return Ok(NilValueObservation::Nil { reason_uri });
    }
    let ix = only_child(
        doc,
        wrapper,
        IWXXM,
        b"MeteorologicalAerodromeObservation",
        input,
        stats,
    )?;
    require_gml_id(doc, ix, input, stats)?;
    no_significant_text(doc, ix, input, stats)?;
    check_attributes(doc, ix, input, &[b"cloudAndVisibilityOK"], stats)?;
    let cavok = match attr_value(doc, ix, input, b"cloudAndVisibilityOK", stats)? {
        Some(b"true") | Some(b"1") => true,
        Some(b"false") | Some(b"0") => false,
        _ => return Err(err(DecodeFailure::InvalidValue, *stats)),
    };
    let mut air_temperature = None;
    let mut dewpoint_temperature = None;
    let mut qnh = None;
    let mut surface_wind = None;
    let mut next_field = 0u8;
    let mut child = first_child(doc, ix);
    while let Some(field) = child {
        work(stats)?;
        if !elem_is(doc, field, input, IWXXM)? {
            refuse(stats);
            return Err(err(DecodeFailure::UnsupportedConstruct, *stats));
        }
        match name(doc, field, input)? {
            b"airTemperature" if next_field == 0 => {
                air_temperature = Some(measure_value(
                    doc,
                    field,
                    input,
                    QuantityUnit::Celsius,
                    b"Cel",
                    stats,
                )?);
                next_field = 1;
            }
            b"dewpointTemperature" if next_field == 1 => {
                dewpoint_temperature = Some(measure_value(
                    doc,
                    field,
                    input,
                    QuantityUnit::Celsius,
                    b"Cel",
                    stats,
                )?);
                next_field = 2;
            }
            b"qnh" if next_field == 2 => {
                qnh = Some(measure_value(
                    doc,
                    field,
                    input,
                    QuantityUnit::Hectopascal,
                    b"hPa",
                    stats,
                )?);
                next_field = 3;
            }
            b"surfaceWind" if next_field == 3 => {
                surface_wind = Some(decode_surface_wind(doc, field, input, stats)?);
                next_field = 4;
            }
            _ => {
                refuse(stats);
                return Err(err(DecodeFailure::UnsupportedConstruct, *stats));
            }
        }
        child = next_sibling(doc, field);
    }
    if air_temperature.is_none()
        || dewpoint_temperature.is_none()
        || qnh.is_none()
        || surface_wind.is_none()
    {
        return Err(err(DecodeFailure::MissingRequiredField, *stats));
    }
    if !cavok {
        refuse(stats);
        return Err(err(DecodeFailure::UnsupportedConstruct, *stats));
    }
    let wind = surface_wind.ok_or_else(|| err(DecodeFailure::MissingRequiredField, *stats))?;
    Ok(NilValueObservation::Observed(Observation {
        cloud_and_visibility_ok: cavok,
        surface_wind_nil_reason_uri: wind.nil_reason,
        wind_direction: wind.direction,
        wind_speed: wind.speed,
        wind_gust: wind.gust,
        variable_wind_direction: wind.variable_direction,
        prevailing_visibility: None,
        prevailing_visibility_operator: None,
        minimum_visibility: None,
        minimum_visibility_direction: None,
        air_temperature,
        dewpoint_temperature,
        qnh,
        present_weather: Vec::<WeatherCode>::new(),
        cloud: None,
    }))
}

struct DecodedWind {
    nil_reason: Option<String>,
    direction: Option<NilValue>,
    speed: Option<NilValue>,
    gust: Option<NilValue>,
    variable_direction: Option<bool>,
}

fn decode_surface_wind(
    doc: &oa_xml::Doc<'_>,
    wrapper: usize,
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<DecodedWind, DecodeError> {
    check_attributes(doc, wrapper, input, &[b"nilReason"], stats)?;
    if let Some(reason) = attr_value(doc, wrapper, input, b"nilReason", stats)? {
        if first_child(doc, wrapper).is_some() || reason.is_empty() {
            return Err(err(DecodeFailure::InvalidValue, *stats));
        }
        no_significant_text(doc, wrapper, input, stats)?;
        if reason.len() > MAX_SCALAR_BYTES {
            return Err(err(DecodeFailure::Limit, *stats));
        }
        let nil_reason = String::from_utf8(reason.to_vec())
            .map_err(|_| err(DecodeFailure::InvalidValue, *stats))?;
        return Ok(DecodedWind {
            nil_reason: Some(nil_reason),
            direction: None,
            speed: None,
            gust: None,
            variable_direction: None,
        });
    }
    let ix = only_child(doc, wrapper, IWXXM, b"AerodromeSurfaceWind", input, stats)?;
    require_gml_id(doc, ix, input, stats)?;
    no_significant_text(doc, ix, input, stats)?;
    check_attributes(doc, ix, input, &[b"variableWindDirection"], stats)?;
    let variable_direction = match attr_value(doc, ix, input, b"variableWindDirection", stats)? {
        Some(b"true") | Some(b"1") => Some(true),
        Some(b"false") | Some(b"0") => Some(false),
        None => None,
        _ => return Err(err(DecodeFailure::InvalidValue, *stats)),
    };
    let mut direction = None;
    let mut speed = None;
    let mut speed_operator = None;
    let mut gust = None;
    let mut gust_operator = None;
    let mut next = 0u8;
    let mut child = first_child(doc, ix);
    while let Some(field) = child {
        work(stats)?;
        if !elem_is(doc, field, input, IWXXM)? {
            refuse(stats);
            return Err(err(DecodeFailure::UnsupportedConstruct, *stats));
        }
        match name(doc, field, input)? {
            b"meanWindDirection" if next == 0 => {
                direction = Some(measure_value_units(
                    doc,
                    field,
                    input,
                    &[(b"deg", QuantityUnit::Degree)],
                    stats,
                )?);
                if let Some(NilValue::Measure { measure, .. }) = direction.as_ref() {
                    if measure.scale > 3
                        || measure.mantissa < 0
                        || measure.mantissa > 360 * 10i64.pow(measure.scale as u32)
                    {
                        return Err(err(DecodeFailure::InvalidValue, *stats));
                    }
                }
                next = 1;
            }
            b"meanWindSpeed" if next <= 1 => {
                speed = Some(measure_value_units(
                    doc,
                    field,
                    input,
                    &[
                        (b"m/s", QuantityUnit::MeterPerSecond),
                        (b"[kn_i]", QuantityUnit::Knot),
                    ],
                    stats,
                )?);
                if matches!(speed.as_ref(), Some(NilValue::Measure { measure: Measure { mantissa, .. }, .. }) if *mantissa < 0)
                {
                    return Err(err(DecodeFailure::InvalidValue, *stats));
                }
                next = 2;
            }
            b"meanWindSpeedOperator" if next == 2 => {
                speed_operator = Some(read_operator(doc, field, input, stats)?);
                next = 3;
            }
            b"windGustSpeed" if next <= 3 => {
                gust = Some(measure_value_units(
                    doc,
                    field,
                    input,
                    &[
                        (b"m/s", QuantityUnit::MeterPerSecond),
                        (b"[kn_i]", QuantityUnit::Knot),
                    ],
                    stats,
                )?);
                if matches!(gust.as_ref(), Some(NilValue::Measure { measure: Measure { mantissa, .. }, .. }) if *mantissa < 0)
                {
                    return Err(err(DecodeFailure::InvalidValue, *stats));
                }
                next = 4;
            }
            b"windGustSpeedOperator" if next == 4 => {
                gust_operator = Some(read_operator(doc, field, input, stats)?);
                next = 5;
            }
            _ => {
                refuse(stats);
                return Err(err(DecodeFailure::UnsupportedConstruct, *stats));
            }
        }
        child = next_sibling(doc, field);
    }
    let mut speed = speed.ok_or_else(|| err(DecodeFailure::MissingRequiredField, *stats))?;
    if let Some(operator) = speed_operator {
        if let NilValue::Measure { operator: slot, .. } = &mut speed {
            *slot = Some(operator);
        } else {
            return Err(err(DecodeFailure::InvalidValue, *stats));
        }
    }
    if let Some(operator) = gust_operator {
        if let Some(NilValue::Measure { operator: slot, .. }) = gust.as_mut() {
            *slot = Some(operator);
        } else {
            return Err(err(DecodeFailure::InvalidValue, *stats));
        }
    }
    Ok(DecodedWind {
        nil_reason: None,
        direction,
        speed: Some(speed),
        gust,
        variable_direction,
    })
}

fn read_operator(
    doc: &oa_xml::Doc<'_>,
    elem: usize,
    input: &[u8],
    stats: &mut DecodeStats,
) -> Result<RelationalOperator, DecodeError> {
    check_attributes(doc, elem, input, &[], stats)?;
    match scalar_text(doc, elem, input, stats)?.as_slice() {
        b"ABOVE" => Ok(RelationalOperator::Above),
        b"BELOW" => Ok(RelationalOperator::Below),
        _ => Err(err(DecodeFailure::InvalidValue, *stats)),
    }
}

fn set_once<T>(slot: &mut Option<T>, value: T, stats: &DecodeStats) -> Result<(), DecodeError> {
    if slot.replace(value).is_some() {
        return Err(err(DecodeFailure::InvalidValue, *stats));
    }
    Ok(())
}

fn measure_value(
    doc: &oa_xml::Doc<'_>,
    elem: usize,
    input: &[u8],
    unit: QuantityUnit,
    uom: &[u8],
    stats: &mut DecodeStats,
) -> Result<NilValue, DecodeError> {
    measure_value_units(doc, elem, input, &[(uom, unit)], stats)
}

fn measure_value_units(
    doc: &oa_xml::Doc<'_>,
    elem: usize,
    input: &[u8],
    units: &[(&[u8], QuantityUnit)],
    stats: &mut DecodeStats,
) -> Result<NilValue, DecodeError> {
    check_attributes(doc, elem, input, &[b"nilReason", b"uom"], stats)?;
    let nil_reason = attr_value(doc, elem, input, b"nilReason", stats)?;
    let unit_text = attr_value(doc, elem, input, b"uom", stats)?;
    if let Some(reason) = nil_reason {
        if reason.is_empty() {
            return Err(err(DecodeFailure::InvalidValue, *stats));
        }
        if unit_text.is_some() || first_child(doc, elem).is_some() {
            return Err(err(DecodeFailure::InvalidValue, *stats));
        }
        no_significant_text(doc, elem, input, stats)?;
        let reason = if reason.len() > MAX_SCALAR_BYTES {
            return Err(err(DecodeFailure::Limit, *stats));
        } else {
            Some(
                String::from_utf8(reason.to_vec())
                    .map_err(|_| err(DecodeFailure::InvalidValue, *stats))?,
            )
        };
        return Ok(NilValue::Nil { reason_uri: reason });
    }
    let unit = units
        .iter()
        .find_map(|(name, unit)| (unit_text == Some(*name)).then_some(*unit))
        .ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?;
    let raw = scalar_text(doc, elem, input, stats)?;
    let raw = core::str::from_utf8(&raw).map_err(|_| err(DecodeFailure::InvalidValue, *stats))?;
    let value = parse_decimal(raw).ok_or_else(|| err(DecodeFailure::InvalidValue, *stats))?;
    let decimal_scale = 10i128.pow(value.1 as u32);
    let scaled = value.0 as i128;
    match unit {
        QuantityUnit::Celsius if scaled * 100 < -27_315 * decimal_scale => {
            return Err(err(DecodeFailure::InvalidValue, *stats));
        }
        QuantityUnit::Hectopascal if value.0 <= 0 => {
            return Err(err(DecodeFailure::InvalidValue, *stats));
        }
        _ => {}
    }
    Ok(NilValue::Measure {
        measure: Measure {
            mantissa: value.0,
            scale: value.1,
            unit,
        },
        operator: None,
    })
}

pub(crate) fn parse_decimal(s: &str) -> Option<(i64, u8)> {
    let (negative, body) = if let Some(body) = s.strip_prefix('-') {
        (true, body)
    } else if let Some(body) = s.strip_prefix('+') {
        (false, body)
    } else {
        (false, s)
    };
    let mut magnitude = 0u64;
    let mut scale = 0u8;
    let mut dot = false;
    let mut digits = 0usize;
    for b in body.bytes() {
        if b == b'.' && !dot {
            dot = true;
            continue;
        }
        if !b.is_ascii_digit() || dot && scale >= 6 {
            return None;
        }
        magnitude = magnitude.checked_mul(10)?.checked_add((b - b'0') as u64)?;
        if dot {
            scale += 1;
        }
        digits += 1;
    }
    if digits == 0 {
        return None;
    }
    let mantissa = if negative && magnitude == (i64::MAX as u64) + 1 {
        i64::MIN
    } else {
        let positive = i64::try_from(magnitude).ok()?;
        if negative {
            positive.checked_neg()?
        } else {
            positive
        }
    };
    Some((mantissa, scale))
}
