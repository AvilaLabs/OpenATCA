//! Feeding recorded messages into the legacy tables. Core (pure): each function takes the bytes of
//! one message document and its receive time and appends rows to a [`Legacy`]. Reusing the
//! decoders: `oa-fixm` for SFDPS flight data and track positions (with [`crate::scan`] for the
//! few fields it does not decode), `oa-tbfm` for TBFM, `oa-tfms` for TFMS.

use oa_fixm::{clear_events, code_str, Decoder, Events, Field};
use oa_tbfm::rows::{pt, section};
use oa_tbfm::NO_TIME as TBFM_NO_TIME;
use oa_tfms::rows::ttype;
use oa_tfms::NO_TIME as TFMS_NO_TIME;
use oa_xml::Parser;

use crate::legacy::{
    flight_ix, pad, push_tbfm, push_tfms, push_th, row_has, tbfm_kind, tfms_kind, Legacy, TfmsIn,
    NO_ALT, NO_TIME,
};
use crate::scan::scan_doc;

// The parser limits, decode mode (decode as far as the tables fit, count the rest) and table
// sizes are the replay pipeline's own (`oa_replay::doc_xml_limits` and the others), so a document
// is accepted here exactly when the replay accepts it.

/// Reusable decoder state for the three feeds (cold: A7.2).
// A7.2: cold struct, size guard waived
pub struct Reader {
    parser: Parser,
    fixm: Decoder,
    events: Events,
    tbfm_rows: oa_tbfm::Rows,
    tfms_rows: oa_tfms::Rows,
    text: Vec<u32>,
}

/// Why a reader could not be made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReaderError;

impl std::fmt::Display for ReaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the XML parser limits were refused")
    }
}

impl std::error::Error for ReaderError {}

/// A reader with the replay pipeline's parser limits and table sizes.
pub fn new_reader() -> Result<Reader, ReaderError> {
    let parser = Parser::new(oa_replay::doc_xml_limits()).map_err(|_| ReaderError)?;
    Ok(Reader {
        parser,
        fixm: Decoder::new(oa_replay::doc_fixm_limits()),
        events: oa_replay::new_doc_events(),
        tbfm_rows: oa_replay::new_doc_tbfm_rows(),
        tfms_rows: oa_replay::new_doc_tfms_rows(),
        text: Vec::new(),
    })
}

/// Ingests one SFDPS document. `recv_ms` is the receive time, [`NO_TIME`] when not recorded.
pub fn ingest_sfdps(leg: &mut Legacy, r: &mut Reader, bytes: &[u8], recv_ms: i64) {
    leg.counts.docs[0] += 1;
    clear_events(&mut r.events);
    let Ok(doc) = r.parser.parse(bytes) else {
        leg.counts.doc_errors[0] += 1;
        return;
    };
    match r.fixm.decode_prefix(&doc, bytes, &mut r.events) {
        Ok(sum) => {
            leg.counts.sfdps_rejected += u64::from(sum.rejected);
            leg.counts.rows_not_decoded += u64::from(sum.not_decoded);
        }
        Err(_) => {
            leg.counts.doc_errors[0] += 1;
            return;
        }
    }
    let ev = &r.events;
    for row in 0..ev.len {
        let Some(&gufi) = ev.gufi.get(row) else {
            continue;
        };
        let Some(f) = flight_ix(leg, gufi) else {
            continue;
        };
        let fi = f as usize;
        if row_has(ev, row, Field::Callsign) {
            if let (Some(c), Some(slot)) = (ev.callsign.get(row), leg.flights.callsign.get_mut(fi))
            {
                *slot = *c;
            }
        }
        if row_has(ev, row, Field::FlightPlanId) {
            if let (Some(c), Some(slot)) = (ev.plan_id.get(row), leg.flights.plan_id.get_mut(fi)) {
                *slot = *c;
            }
        }
        if row_has(ev, row, Field::Departure) {
            if let (Some(c), Some(slot)) = (ev.departure.get(row), leg.flights.dep.get_mut(fi)) {
                *slot = *c;
            }
        }
        if row_has(ev, row, Field::Arrival) {
            if let (Some(c), Some(slot)) = (ev.arrival.get(row), leg.flights.arr.get_mut(fi)) {
                *slot = *c;
            }
        }
        if row_has(ev, row, Field::Position) && row_has(ev, row, Field::PositionTime) {
            let (Some(&lat), Some(&lon), Some(&pt)) = (
                ev.lat_e7.get(row),
                ev.lon_e7.get(row),
                ev.pos_time_ms.get(row),
            ) else {
                continue;
            };
            let alt = if row_has(ev, row, Field::ReportedAlt) {
                ev.reported_ft.get(row).copied().unwrap_or(NO_ALT)
            } else {
                NO_ALT
            };
            let vel = if row_has(ev, row, Field::Velocity) {
                ev.vx_dkt.get(row).copied().zip(ev.vy_dkt.get(row).copied())
            } else {
                None
            };
            let coast = row_has(ev, row, Field::Coast);
            push_th(leg, f, pt, recv_ms, lat, lon, alt, vel, coast);
        }
    }
    scan_doc(&doc, &mut r.text, leg);
}

fn tb_time(t: i64) -> Option<i64> {
    (t != TBFM_NO_TIME).then_some(t)
}

/// Ingests one TBFM document.
pub fn ingest_tbfm(leg: &mut Legacy, r: &mut Reader, bytes: &[u8]) {
    leg.counts.docs[1] += 1;
    oa_tbfm::clear_rows(&mut r.tbfm_rows);
    let Ok(doc) = r.parser.parse(bytes) else {
        leg.counts.doc_errors[1] += 1;
        return;
    };
    match oa_tbfm::decode_doc_prefix(&doc, &mut r.tbfm_rows) {
        Ok(out) => leg.counts.rows_not_decoded += u64::from(out.dropped),
        Err(_) => {
            leg.counts.doc_errors[1] += 1;
            return;
        }
    }
    let rows = &r.tbfm_rows;
    for i in 0..oa_tbfm::rows_len(rows) {
        let Some(row) = oa_tbfm::row_at(rows, i) else {
            continue;
        };
        let entries: [(u8, usize, bool); 4] = [
            (tbfm_kind::ETA_MFX, pt::MFX, true),
            (tbfm_kind::ETA_RWY, pt::RWY, true),
            (tbfm_kind::STA_SFX, pt::SFX, false),
            (tbfm_kind::STA_RWY, pt::RWY, false),
        ];
        for (kind, p, is_eta) in entries {
            let want = if is_eta {
                row.section == section::ETA
            } else {
                row.section == section::STA
            };
            if !want {
                continue;
            }
            let t = if is_eta {
                row.eta_ms.get(p)
            } else {
                row.sta_ms.get(p)
            };
            let Some(time) = t.copied().and_then(tb_time) else {
                continue;
            };
            let issue = tb_time(row.msg_ms).unwrap_or(NO_TIME);
            push_tbfm(
                leg,
                pad(code_bytes(&row.gufi)),
                pad(code_bytes(&row.aid)),
                pad(code_bytes(&row.mfx)),
                kind,
                time,
                issue,
            );
        }
    }
}

fn code_bytes(b: &[u8]) -> &[u8] {
    code_str(b).as_bytes()
}

/// Ingests one TFMS document. Only flight rows with an `ESTIMATED` arrival or departure time
/// become entries.
pub fn ingest_tfms(leg: &mut Legacy, r: &mut Reader, bytes: &[u8]) {
    leg.counts.docs[2] += 1;
    oa_tfms::clear_rows(&mut r.tfms_rows);
    let Ok(doc) = r.parser.parse(bytes) else {
        leg.counts.doc_errors[2] += 1;
        return;
    };
    match oa_tfms::decode_doc_prefix(&doc, &mut r.tfms_rows) {
        Ok(out) => leg.counts.rows_not_decoded += u64::from(out.dropped),
        Err(_) => {
            leg.counts.doc_errors[2] += 1;
            return;
        }
    }
    for i in 0..oa_tfms::flights_len(&r.tfms_rows.flights) {
        let Some(f) = oa_tfms::flight_at(&r.tfms_rows.flights, i) else {
            continue;
        };
        let issue = if f.src_ms == TFMS_NO_TIME {
            NO_TIME
        } else {
            f.src_ms
        };
        let base = TfmsIn {
            plan_id: pad(code_bytes(&f.gufi)),
            acid: pad(code_bytes(&f.acid)),
            dep: pad(code_bytes(&f.dep)),
            arr: pad(code_bytes(&f.arr)),
            kind: 0,
            time_ms: 0,
            issue_ms: issue,
        };
        if f.eta_ms != TFMS_NO_TIME && f.eta_type == ttype::ESTIMATED {
            push_tfms(
                leg,
                &TfmsIn {
                    kind: tfms_kind::ETA,
                    time_ms: f.eta_ms,
                    ..base
                },
            );
        }
        if f.etd_ms != TFMS_NO_TIME && f.etd_type == ttype::ESTIMATED {
            push_tfms(
                leg,
                &TfmsIn {
                    kind: tfms_kind::ETD,
                    time_ms: f.etd_ms,
                    ..base
                },
            );
        }
    }
}
