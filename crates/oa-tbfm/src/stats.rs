//! Counters over many messages. Plain data; updated by [`observe`], a free function over the
//! result of one decode. Core (pure).

use crate::decode::Outcome;
use crate::fields::{
    reject_reason, DocError, ERROR_KIND_COUNT, FIELD_COUNT, MSG_KIND_COUNT, REJECT_COUNT,
};
use crate::rows::section;

/// Totals over a run of messages.
// A7.2: cold struct, size guard waived (one per run)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stats {
    pub messages: u64,
    /// Rows dropped because their own time is in the window (OA-SEAL-014); a message that held
    /// only such rows is in no other count.
    pub sealed_window_dropped: u64,
    pub accepted: u64,
    pub rows: u64,
    /// Accepted messages by [`crate::MsgKind`].
    pub kinds: [u64; MSG_KIND_COUNT],
    /// Rows by section code ([`section`]); index 0 is unused.
    pub sections: [u64; 6],
    /// Refused messages by reason ([`crate::REJECT_XML`] and its neighbours).
    pub rejects: [u64; REJECT_COUNT],
    /// Refused flights (`air` or `tma` elements) by the [`crate::Field`] that refused them, in
    /// accepted messages, plus the messages refused whole for a field (an envelope attribute).
    pub field_errors: [u64; FIELD_COUNT],
    /// Flights refused for a field in accepted messages (their siblings were kept).
    pub rejected_flights: u64,
    /// The same refusals by reason ([`crate::error_kind_index`]).
    pub reject_kinds: [u64; ERROR_KIND_COUNT],
    /// Schedule-flag words kept as unknown instead of refusing the flight.
    pub unknown_words: u64,
    /// Speeds stored with the unknown unit (neither knots nor Mach in form).
    pub unknown_units: u64,
    /// Elements, attributes and namespaces the decoder does not know, in accepted messages.
    pub unknown_paths: u64,
    /// Known elements it deliberately does not store (long free text, adaptation and control
    /// bodies), in accepted messages.
    pub ignored_paths: u64,
}

/// All counters zero.
pub const fn new_stats() -> Stats {
    Stats {
        messages: 0,
        sealed_window_dropped: 0,
        accepted: 0,
        rows: 0,
        kinds: [0; MSG_KIND_COUNT],
        sections: [0; 6],
        rejects: [0; REJECT_COUNT],
        field_errors: [0; FIELD_COUNT],
        rejected_flights: 0,
        reject_kinds: [0; ERROR_KIND_COUNT],
        unknown_words: 0,
        unknown_units: 0,
        unknown_paths: 0,
        ignored_paths: 0,
    }
}

/// Folds one decode result into `s`. `sections_added` is the `section` column of the rows the
/// decode appended (`&rows.section[before..]`), counted by section.
pub fn observe(s: &mut Stats, result: &Result<Outcome, DocError>, sections_added: &[u8]) {
    if let Ok(o) = result {
        s.sealed_window_dropped += u64::from(o.sealed_dropped);
        // A message whose every row was dropped for its time (nothing refused, nothing else in
        // it) is in no message, kind or other count.
        if o.sealed_dropped > 0 && o.rows == 0 && o.rejected == 0 && o.dropped == 0 {
            return;
        }
    }
    s.messages += 1;
    match result {
        Ok(o) => {
            s.accepted += 1;
            s.rows += u64::from(o.rows);
            if let Some(k) = s.kinds.get_mut(o.kind as usize) {
                *k += 1;
            }
            s.unknown_paths += u64::from(o.unknown_paths);
            s.ignored_paths += u64::from(o.ignored_paths);
            s.rejected_flights += u64::from(o.rejected);
            s.unknown_words += u64::from(o.unknown_words);
            s.unknown_units += u64::from(o.unknown_units);
            for (t, &n) in s.field_errors.iter_mut().zip(&o.reject_fields) {
                *t += u64::from(n);
            }
            for (t, &n) in s.reject_kinds.iter_mut().zip(&o.reject_kinds) {
                *t += u64::from(n);
            }
            for &c in sections_added {
                if let Some(x) = s.sections.get_mut(usize::from(c.min(section::SCH))) {
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
