//! The parts of an SFDPS message `oa-fixm` does not decode: the aircraft address, the handoff
//! event kind and the expanded route points. Core (pure): a function of the parsed document.
//!
//! The scan walks the same parsed [`Doc`] the `oa-fixm` decoder reads, message by message
//! (children of the root), and keys everything by the message's GUFI so it needs no row
//! alignment with the decoder's events.

use oa_fixm::{parse_iso_ms, parse_uuid};
use oa_xml::{attr_needs_decode, span_bytes, text_needs_decode, Doc, ElemIx};

use crate::legacy::{
    flight_ix, hev, pad, push_handoff, push_route_received, set_addr, set_addr_hint, Legacy,
    RoutePointIn, NO_COORD, NO_TIME,
};

/// Most route points kept per route; a longer route is cut at this and the rest ignored (the
/// longest in the 2026-11-14 sample had 80).
pub const MAX_POINTS_PER_ROUTE: usize = 512;

fn ename<'a>(doc: &Doc<'a>, i: usize) -> &'a [u8] {
    doc.elem_name
        .get(i)
        .and_then(|s| span_bytes(doc.input, *s).ok())
        .unwrap_or(&[])
}

fn eattr<'a>(doc: &Doc<'a>, i: usize, name: &[u8]) -> Option<&'a [u8]> {
    let first = doc.elem_first_attr.get(i)?.0 as usize;
    let count = *doc.elem_attr_count.get(i)? as usize;
    for a in first..first.checked_add(count)? {
        let n = span_bytes(doc.input, *doc.attr_name.get(a)?).ok()?;
        if n == name {
            if attr_needs_decode(doc, oa_xml::AttrIx(a as u32)) {
                return None;
            }
            return span_bytes(doc.input, *doc.attr_value.get(a)?).ok();
        }
    }
    None
}

/// Text of each element: the first text node, or `None`. Built once per document.
fn first_texts(doc: &Doc<'_>, out: &mut Vec<u32>) {
    out.clear();
    out.resize(doc.elem_name.len(), u32::MAX);
    for (t, owner) in doc.text_owner.iter().enumerate() {
        if let Some(slot) = out.get_mut(owner.0 as usize) {
            if *slot == u32::MAX {
                *slot = t as u32;
            }
        }
    }
}

fn etext<'a>(doc: &Doc<'a>, first: &[u32], i: usize) -> Option<&'a [u8]> {
    let t = *first.get(i)?;
    if t == u32::MAX || text_needs_decode(doc, oa_xml::TextIx(t)) {
        return None;
    }
    span_bytes(doc.input, *doc.text_span.get(t as usize)?).ok()
}

fn child(doc: &Doc<'_>, e: usize, name: &[u8]) -> Option<usize> {
    let mut c = doc.elem_first_child.get(e)?.0;
    while c != ElemIx::NONE.0 {
        if ename(doc, c as usize) == name {
            return Some(c as usize);
        }
        c = doc.elem_next_sibling.get(c as usize)?.0;
    }
    None
}

fn unit(doc: &Doc<'_>, e: usize) -> [u8; 8] {
    let mut out = [0u8; 8];
    let u: [u8; 4] = eattr(doc, e, b"unitIdentifier").map(pad).unwrap_or([0; 4]);
    let s: [u8; 4] = eattr(doc, e, b"sectorIdentifier")
        .map(pad)
        .unwrap_or([0; 4]);
    for (o, b) in out.iter_mut().zip(u.iter().chain(s.iter())) {
        *o = *b;
    }
    out
}

fn hex_addr(s: &[u8]) -> Option<u32> {
    if s.len() != 6 {
        return None;
    }
    let mut v = 0u32;
    for &b in s {
        let d = match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => b - b'a' + 10,
            b'A'..=b'F' => b - b'A' + 10,
            _ => return None,
        };
        v = (v << 4) | u32::from(d);
    }
    Some(v)
}

fn event_code(s: &[u8]) -> u8 {
    match s {
        b"INITIATION" => hev::INITIATION,
        b"ACCEPTANCE" => hev::ACCEPTANCE,
        b"UPDATE" => hev::UPDATE,
        b"RETRACTION" => hev::RETRACTION,
        b"FAILURE" => hev::FAILURE,
        _ => hev::OTHER,
    }
}

fn pos_e7(s: &[u8]) -> Option<(i32, i32)> {
    let text = std::str::from_utf8(s).ok()?;
    let mut it = text.split_whitespace();
    let lat: f64 = it.next()?.parse().ok()?;
    let lon: f64 = it.next()?.parse().ok()?;
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return None;
    }
    Some(((lat * 1e7).round() as i32, (lon * 1e7).round() as i32))
}

fn route_points(doc: &Doc<'_>, first: &[u32], er: usize, leg: &mut Legacy) -> Vec<RoutePointIn> {
    let mut pts = Vec::new();
    let mut c = doc.elem_first_child.get(er).map_or(u32::MAX, |c| c.0);
    while c != ElemIx::NONE.0 && pts.len() < MAX_POINTS_PER_ROUTE {
        let rp = c as usize;
        if ename(doc, rp) == b"routePoint" {
            let est = eattr(doc, rp, b"estimatedTime")
                .and_then(parse_iso_ms)
                .unwrap_or(NO_TIME);
            if let Some(p) = child(doc, rp, b"point") {
                let fix = eattr(doc, p, b"fix").map(pad).unwrap_or([0; 8]);
                let mut ll = (NO_COORD, NO_COORD);
                if let Some(pos) = child(doc, p, b"location").and_then(|l| child(doc, l, b"pos")) {
                    if let Some(v) = etext(doc, first, pos).and_then(pos_e7) {
                        ll = v;
                    }
                }
                if eattr(doc, p, b"fix").is_some() && child(doc, p, b"radial").is_some() {
                    leg.counts.route_relative_points += 1;
                }
                pts.push(RoutePointIn {
                    fix,
                    est_ms: est,
                    lat_e7: ll.0,
                    lon_e7: ll.1,
                });
            }
        }
        c = doc.elem_next_sibling.get(rp).map_or(u32::MAX, |c| c.0);
    }
    pts
}

/// Scans every message of `doc` (an SFDPS `MessageCollection`) into `leg`. The caller runs it
/// only on a document the decoder did not drop for a message timed in the sealed window, so no
/// such message reaches a hint, address, handoff or route count or table (OA-SEAL-014).
pub fn scan_doc(doc: &Doc<'_>, scratch: &mut Vec<u32>, leg: &mut Legacy) {
    scan_received(doc, scratch, leg, NO_TIME);
}

pub(crate) fn scan_received(doc: &Doc<'_>, scratch: &mut Vec<u32>, leg: &mut Legacy, recv_ms: i64) {
    first_texts(doc, scratch);
    let n = doc.elem_name.len();
    let starts: Vec<usize> = (1..n)
        .filter(|&i| doc.elem_parent.get(i).is_some_and(|p| p.0 == 0))
        .collect();
    for (k, &s) in starts.iter().enumerate() {
        let e = starts.get(k + 1).copied().unwrap_or(n);
        scan_message(doc, scratch, leg, s, e, recv_ms);
    }
}

fn scan_message(doc: &Doc<'_>, first: &[u32], leg: &mut Legacy, s: usize, e: usize, recv_ms: i64) {
    let mut gufi: Option<u128> = None;
    let mut ts = NO_TIME;
    let mut addr: Option<u32> = None;
    let mut handoffs: Vec<(u8, [u8; 8], [u8; 8])> = Vec::new();
    let mut route: Option<usize> = None;
    let mut hint = false;
    let mut hint_addr: Option<u32> = None;
    for i in s..e {
        match ename(doc, i) {
            b"flight" => {
                ts = eattr(doc, i, b"timestamp")
                    .and_then(parse_iso_ms)
                    .unwrap_or(NO_TIME);
            }
            b"gufi" => {
                if gufi.is_none() {
                    gufi = etext(doc, first, i).and_then(parse_uuid);
                }
            }
            b"aircraftDescription" => {
                if let Some(a) = eattr(doc, i, b"aircraftAddress").and_then(hex_addr) {
                    addr = Some(a);
                }
            }
            b"handoff" => {
                let ev = eattr(doc, i, b"event").map_or(hev::OTHER, event_code);
                let to = child(doc, i, b"receivingUnit").map_or([0; 8], |u| unit(doc, u));
                let from = child(doc, i, b"transferringUnit").map_or([0; 8], |u| unit(doc, u));
                handoffs.push((ev, from, to));
            }
            b"expandedRoute" => {
                if route.is_none() {
                    route = Some(i);
                }
            }
            b"nameValue" if eattr(doc, i, b"name").is_some_and(|n| n.starts_with(b"ADSB_02M")) => {
                hint = true;
                hint_addr = eattr(doc, i, b"value")
                    .map(|v| v.strip_prefix(b"-").or(v.strip_prefix(b"+")).unwrap_or(v))
                    .and_then(hex_addr);
            }
            _ => {}
        }
    }
    if hint {
        leg.counts.th_adsb_hint += 1;
    }
    let Some(g) = gufi else { return };
    let Some(f) = flight_ix(leg, g) else { return };
    if let Some(a) = addr {
        set_addr(leg, f, a);
    }
    if let Some(a) = hint_addr {
        set_addr_hint(leg, f, a);
    }
    for (ev, from, to) in handoffs {
        push_handoff(leg, f, ts, ev, from, to);
    }
    if let Some(er) = route {
        let pts = route_points(doc, first, er, leg);
        push_route_received(leg, f, ts, recv_ms, &pts);
    }
}
