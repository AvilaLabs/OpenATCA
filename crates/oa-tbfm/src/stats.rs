//! Counters over many messages. Plain data; updated by [`observe`], a free function over the
//! result of one decode. Core (pure).

use crate::decode::Outcome;
use crate::fields::{reject_reason, DocError, FIELD_COUNT, MSG_KIND_COUNT, REJECT_COUNT};
use crate::rows::section;

/// Totals over a run of messages.
// A7.2: cold struct, size guard waived (one per run)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stats {
    pub messages: u64,
    pub accepted: u64,
    pub rows: u64,
    /// Accepted messages by [`crate::MsgKind`].
    pub kinds: [u64; MSG_KIND_COUNT],
    /// Rows by section code ([`section`]); index 0 is unused.
    pub sections: [u64; 6],
    /// Refused messages by reason ([`crate::REJECT_XML`] and its neighbours).
    pub rejects: [u64; REJECT_COUNT],
    /// Refused fields by [`crate::Field`] (only messages refused for a field).
    pub field_errors: [u64; FIELD_COUNT],
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
        accepted: 0,
        rows: 0,
        kinds: [0; MSG_KIND_COUNT],
        sections: [0; 6],
        rejects: [0; REJECT_COUNT],
        field_errors: [0; FIELD_COUNT],
        unknown_paths: 0,
        ignored_paths: 0,
    }
}

/// Folds one decode result into `s`. `sections_added` is the `section` column of the rows the
/// decode appended (`&rows.section[before..]`), counted by section.
pub fn observe(s: &mut Stats, result: &Result<Outcome, DocError>, sections_added: &[u8]) {
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
