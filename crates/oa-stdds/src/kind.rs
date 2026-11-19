//! Classification of a message by root element and namespace. Core (pure).
//!
//! STDDS native ("SimpleXML") messages put their root in a namespace of the form
//! `urn:us:gov:dot:faa:atm:terminal:entities:vN-M:<service>:<name>`, for example
//! `...:v3-0:tais:terminalautomationinformation` and `...:v4-0:smes:surfacemovementevent`
//! (FAA FIXM-mediated STDDS overview, section 2.2.1; STDDS R6P2 release notes, section 4.2). The
//! namespace version stopped incrementing after R4, so the version is never compared: any
//! `vN-M` matches. The FIXM-mediated form is a `MessageCollection` in a FIXM messaging
//! namespace whose `metadata@messageType` names the original message type.

use oa_xml::{
    attr_needs_decode, decode_into, ns_uri_bytes, resolve_elem_ns, span_bytes, Doc, ElemIx, Ns,
    SpanKind,
};

/// What a message is. Ordered; [`KIND_NAMES`] has one name per value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    /// No product recognised (the root name and namespace are counted in the stats).
    Unknown = 0,
    /// TAIS `TATrackAndFlightPlan`: parsed by [`crate::parse_tais`].
    TaisTrackAndFlightPlan,
    /// A TAIS-namespace message with another root.
    TaisOther,
    /// SMES `asdexMsg` with a Cat11 `positionReport`: parsed by [`crate::parse_smes`].
    AsdexPositionReport,
    /// SMES `asdexMsg` with a Cat10 `adsbReport` (classified only).
    AsdexAdsbReport,
    /// SMES `asdexMsg` with a Cat10 `mlatReport` (classified only).
    AsdexMlatReport,
    /// SMES `asdexMsg` with no recognised report child.
    AsdexOther,
    /// A SMES-namespace message with another root (surface movement events and the like).
    SmesOther,
    /// STDDS APDS namespace (service token `apds`, ASSUMED).
    Apds,
    /// STDDS ISMC namespace (service token `ismc`, ASSUMED).
    Ismc,
    /// STDDS RVR namespace (service token `rvr`, ASSUMED).
    Rvr,
    /// STDDS TDES namespace (service token `tdes`, ASSUMED).
    Tdes,
    /// Any other STDDS terminal-entities namespace.
    StddsOther,
    /// FIXM-mediated TAIS (`metadata@messageType` is `TAIS_TRACK_AND_FLIGHT_PLAN`).
    FixmTais,
    /// FIXM-mediated ASDE-X (`ASDEX_*` message types).
    FixmAsdex,
    /// A FIXM messaging `MessageCollection` of another or unreadable message type.
    FixmOther,
}

/// Number of [`Kind`] values.
pub const KIND_COUNT: usize = 16;

/// Display names of the kinds, in order.
pub const KIND_NAMES: [&str; KIND_COUNT] = [
    "unknown",
    "tais-track-and-flight-plan",
    "tais-other",
    "asdex-position-report",
    "asdex-adsb-report",
    "asdex-mlat-report",
    "asdex-other",
    "smes-other",
    "apds",
    "ismc",
    "rvr",
    "tdes",
    "stdds-other",
    "fixm-tais",
    "fixm-asdex",
    "fixm-other",
];

/// The display name of a kind.
pub fn kind_name(k: Kind) -> &'static str {
    KIND_NAMES.get(k as usize).copied().unwrap_or("?")
}

const STDDS_PREFIX: &[u8] = b"urn:us:gov:dot:faa:atm:terminal:entities:";
const FIXM_MESSAGING_PREFIX: &[u8] = b"http://www.fixm.aero/messaging/";

/// `vN-M` with one or more digits on each side.
fn is_version(s: &[u8]) -> bool {
    let Some(rest) = s.strip_prefix(b"v") else {
        return false;
    };
    let mut parts = rest.split(|&b| b == b'-');
    let (Some(a), Some(b), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    !a.is_empty()
        && !b.is_empty()
        && a.iter().all(u8::is_ascii_digit)
        && b.iter().all(u8::is_ascii_digit)
}

/// The `(service, name)` of an STDDS terminal-entities namespace, whatever its version.
pub fn stdds_namespace(uri: &[u8]) -> Option<(&[u8], &[u8])> {
    let rest = uri.strip_prefix(STDDS_PREFIX)?;
    let mut parts = rest.split(|&b| b == b':');
    let (Some(ver), Some(service), Some(name), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    (is_version(ver) && !service.is_empty() && !name.is_empty()).then_some((service, name))
}

/// The raw bytes of element `e`'s name; empty if the span is outside the document.
pub(crate) fn name_of<'a>(doc: &Doc<'a>, e: ElemIx) -> &'a [u8] {
    match doc.elem_name.get(e.0 as usize) {
        Some(&sp) => span_bytes(doc.input, sp).unwrap_or(&[]),
        None => &[],
    }
}

/// The namespace URI bytes of element `e`, empty if it has none.
pub(crate) fn ns_of<'a>(doc: &Doc<'a>, e: ElemIx) -> &'a [u8] {
    match resolve_elem_ns(doc, e) {
        Ns::Uri(sp) => ns_uri_bytes(doc, sp).unwrap_or(&[]),
        _ => &[],
    }
}

/// The children of `e` in document order.
pub(crate) fn children<'a>(doc: &'a Doc<'a>, e: ElemIx) -> impl Iterator<Item = ElemIx> + 'a {
    let first = doc
        .elem_first_child
        .get(e.0 as usize)
        .copied()
        .unwrap_or(ElemIx::NONE);
    let mut cur = first;
    std::iter::from_fn(move || {
        if cur == ElemIx::NONE {
            return None;
        }
        let this = cur;
        cur = doc
            .elem_next_sibling
            .get(this.0 as usize)
            .copied()
            .unwrap_or(ElemIx::NONE);
        Some(this)
    })
}

/// The decoded value of attribute `name` of element `e` into `out`, or `None` if absent or
/// longer than `out`.
pub(crate) fn attr_value<'o>(
    doc: &Doc<'_>,
    e: ElemIx,
    name: &[u8],
    out: &'o mut [u8],
) -> Option<&'o [u8]> {
    let first = doc.elem_first_attr.get(e.0 as usize)?.0 as usize;
    let count = *doc.elem_attr_count.get(e.0 as usize)? as usize;
    for a in first..first.checked_add(count)? {
        let n = span_bytes(doc.input, *doc.attr_name.get(a)?).ok()?;
        if n != name {
            continue;
        }
        let v = *doc.attr_value.get(a)?;
        let raw = span_bytes(doc.input, v).ok()?;
        if attr_needs_decode(doc, oa_xml::AttrIx(a as u32)) {
            let n = decode_into(v, doc.input, out, SpanKind::AttrValue).ok()?;
            return out.get(..n);
        }
        let dst = out.get_mut(..raw.len())?;
        dst.copy_from_slice(raw);
        return Some(dst);
    }
    None
}

/// Classifies a parsed message. Pure: the root element's local name and namespace, and for the
/// FIXM-mediated form the first `metadata@messageType`. Never fails: no match is
/// [`Kind::Unknown`].
pub fn classify(doc: &Doc<'_>) -> Kind {
    if doc.elem_name.is_empty() {
        return Kind::Unknown;
    }
    let root = ElemIx(0);
    let name = name_of(doc, root);
    let ns = ns_of(doc, root);
    if let Some((service, ns_name)) = stdds_namespace(ns) {
        return classify_stdds(doc, root, name, service, ns_name);
    }
    if ns.starts_with(FIXM_MESSAGING_PREFIX) && name == b"MessageCollection" {
        return classify_fixm(doc);
    }
    Kind::Unknown
}

fn classify_stdds(
    doc: &Doc<'_>,
    root: ElemIx,
    name: &[u8],
    service: &[u8],
    ns_name: &[u8],
) -> Kind {
    match service {
        b"tais" => {
            if ns_name == b"terminalautomationinformation"
                && (name == b"TATrackAndFlightPlan"
                    || name == b"TerminalAutomationTrackAndFlightPlanData")
            {
                Kind::TaisTrackAndFlightPlan
            } else {
                Kind::TaisOther
            }
        }
        b"smes" => {
            if name == b"asdexMsg" {
                for c in children(doc, root) {
                    match name_of(doc, c) {
                        b"positionReport" => return Kind::AsdexPositionReport,
                        b"adsbReport" => return Kind::AsdexAdsbReport,
                        b"mlatReport" => return Kind::AsdexMlatReport,
                        _ => {}
                    }
                }
                Kind::AsdexOther
            } else {
                Kind::SmesOther
            }
        }
        b"apds" => Kind::Apds,
        b"ismc" => Kind::Ismc,
        b"rvr" => Kind::Rvr,
        b"tdes" => Kind::Tdes,
        _ => Kind::StddsOther,
    }
}

fn classify_fixm(doc: &Doc<'_>) -> Kind {
    let mut buf = [0u8; 64];
    for i in 0..doc.elem_name.len() {
        let e = ElemIx(i as u32);
        if name_of(doc, e) != b"metadata" {
            continue;
        }
        if let Some(v) = attr_value(doc, e, b"messageType", &mut buf) {
            return if v == b"TAIS_TRACK_AND_FLIGHT_PLAN" {
                Kind::FixmTais
            } else if v.starts_with(b"ASDEX_") {
                Kind::FixmAsdex
            } else {
                Kind::FixmOther
            };
        }
    }
    Kind::FixmOther
}
