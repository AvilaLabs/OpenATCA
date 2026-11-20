//! Counters over many documents. Plain data; updated by [`observe`], a free function over the
//! result of one decode. Core (pure).

use crate::decode::Outcome;
use crate::fields::{
    reject_reason, DocError, DOC_KIND_COUNT, ERROR_KIND_COUNT, FIELD_COUNT, REJECT_COUNT,
};
use crate::rows::{fkind, kind};

/// Totals over a run of documents.
// A7.2: cold struct, size guard waived (one per run)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stats {
    pub documents: u64,
    /// Rows dropped because their message's own time is in the window (OA-SEAL-014); a document
    /// that held only such messages is in no other count.
    pub sealed_window_dropped: u64,
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
    /// Refused messages (and TMI list entries) by the [`crate::Field`] that refused them, in
    /// accepted documents.
    pub field_errors: [u64; FIELD_COUNT],
    /// Messages refused for a field in accepted documents (their siblings were kept).
    pub rejected_messages: u64,
    /// The same refusals by reason ([`crate::error_kind_index`]).
    pub reject_kinds: [u64; ERROR_KIND_COUNT],
    /// Vocabulary words kept as unknown instead of refusing the message.
    pub unknown_words: u64,
    /// Speeds of a form that is neither knots nor Mach, kept as unknown.
    pub unknown_units: u64,
    /// Elements, attributes and namespaces the decoder does not know, in accepted documents.
    pub unknown_paths: u64,
    /// Known elements it deliberately does not store, in accepted documents.
    pub ignored_paths: u64,
}

/// All counters zero.
pub const fn new_stats() -> Stats {
    Stats {
        documents: 0,
        sealed_window_dropped: 0,
        accepted: 0,
        messages: 0,
        flight_rows: 0,
        flow_rows: 0,
        doc_kinds: [0; DOC_KIND_COUNT],
        flight_kinds: [0; kind::COUNT],
        flow_kinds: [0; fkind::COUNT],
        rejects: [0; REJECT_COUNT],
        field_errors: [0; FIELD_COUNT],
        rejected_messages: 0,
        reject_kinds: [0; ERROR_KIND_COUNT],
        unknown_words: 0,
        unknown_units: 0,
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
    if let Ok(o) = result {
        s.sealed_window_dropped += u64::from(o.sealed_dropped);
        if o.sealed_dropped > 0 && o.messages == 0 && o.rejected == 0 && o.dropped == 0 {
            return;
        }
    }
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
            s.rejected_messages += u64::from(o.rejected);
            s.unknown_words += u64::from(o.unknown_words);
            s.unknown_units += u64::from(o.unknown_units);
            for (t, &n) in s.field_errors.iter_mut().zip(&o.reject_fields) {
                *t += u64::from(n);
            }
            for (t, &n) in s.reject_kinds.iter_mut().zip(&o.reject_kinds) {
                *t += u64::from(n);
            }
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
