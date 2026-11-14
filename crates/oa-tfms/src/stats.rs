//! Counters over many documents. Plain data; updated by [`observe`], a free function over the
//! result of one decode. Core (pure).

use crate::decode::Outcome;
use crate::fields::{reject_reason, DocError, DOC_KIND_COUNT, FIELD_COUNT, REJECT_COUNT};
use crate::rows::{fkind, kind};

/// Totals over a run of documents.
// A7.2: cold struct, size guard waived (one per run)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stats {
    pub documents: u64,
    pub accepted: u64,
    /// Messages in accepted documents.
    pub messages: u64,
    pub flight_rows: u64,
    pub flow_rows: u64,
    /// Accepted documents by [`crate::DocKind`].
    pub doc_kinds: [u64; DOC_KIND_COUNT],
    /// Flight rows by [`kind`] code.
    pub flight_kinds: [u64; kind::COUNT],
    /// Flow rows by [`fkind`] code.
    pub flow_kinds: [u64; fkind::COUNT],
    /// Refused documents by reason ([`crate::REJECT_XML`] and its neighbours).
    pub rejects: [u64; REJECT_COUNT],
    /// Refused fields by [`crate::Field`] (only documents refused for a field).
    pub field_errors: [u64; FIELD_COUNT],
    /// Elements, attributes and namespaces the decoder does not know, in accepted documents.
    pub unknown_paths: u64,
    /// Known elements it deliberately does not store, in accepted documents.
    pub ignored_paths: u64,
}

/// All counters zero.
pub const fn new_stats() -> Stats {
    Stats {
        documents: 0,
        accepted: 0,
        messages: 0,
        flight_rows: 0,
        flow_rows: 0,
        doc_kinds: [0; DOC_KIND_COUNT],
        flight_kinds: [0; kind::COUNT],
        flow_kinds: [0; fkind::COUNT],
        rejects: [0; REJECT_COUNT],
        field_errors: [0; FIELD_COUNT],
        unknown_paths: 0,
        ignored_paths: 0,
    }
}

/// Folds one decode result into `s`. `flight_kinds` and `flow_kinds` are the `kind` columns of
/// the rows the decode appended (`&rows.flights.kind[before..]`, `&rows.flows.kind[before..]`),
/// counted by kind.
pub fn observe(
    s: &mut Stats,
    result: &Result<Outcome, DocError>,
    flight_kinds: &[u8],
    flow_kinds: &[u8],
) {
    s.documents += 1;
    match result {
        Ok(o) => {
            s.accepted += 1;
            s.messages += u64::from(o.messages);
            s.flight_rows += u64::from(o.flight_rows);
            s.flow_rows += u64::from(o.flow_rows);
            if let Some(k) = s.doc_kinds.get_mut(o.kind as usize) {
                *k += 1;
            }
            s.unknown_paths += u64::from(o.unknown_paths);
            s.ignored_paths += u64::from(o.ignored_paths);
            for &k in flight_kinds {
                if let Some(x) = s.flight_kinds.get_mut(usize::from(k)) {
                    *x += 1;
                }
            }
            for &k in flow_kinds {
                if let Some(x) = s.flow_kinds.get_mut(usize::from(k)) {
                    *x += 1;
                }
            }
        }
        Err(e) => {
            if let Some(r) = s.rejects.get_mut(reject_reason(e)) {
                *r += 1;
            }
            if let DocError::Field(f) = e {
                if let Some(x) = s.field_errors.get_mut(f.field as usize) {
                    *x += 1;
                }
            }
        }
    }
}
