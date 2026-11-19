//! Cumulative counts over many messages. Core (pure): a plain accumulator with no clock and no
//! I/O. Counts only: no position, identity or value is kept (the unknown-element table keeps
//! element names and root names, which are schema text, never message values).

use crate::kind::{name_of, ns_of, Kind, KIND_COUNT};
use crate::smes::{SmesMessage, SMES_FIELD_COUNT};
use crate::tais::{TaisMessage, TAIS_FIELD_COUNT};
use crate::{std_error_index, FieldError, Limits, StdError, ERROR_KIND_COUNT, STD_ERROR_KINDS};
use oa_xml::{Doc, ElemIx};

/// Longest root name or namespace text kept (bytes).
const MAX_TEXT: usize = 120;
/// Most distinct unknown roots kept.
const MAX_ROOTS: usize = 64;

/// One unknown element path (below the record, or below the root) and how often it occurred.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownPath {
    pub text: String,
    pub count: u64,
}

/// Counts. Fields are public plain data; [`new_stats`] makes an empty one.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug)]
pub struct Stats {
    /// Messages seen (parsed XML or not).
    pub messages: u64,
    /// Messages per [`Kind`].
    pub kinds: [u64; KIND_COUNT],
    /// Messages refused whole, per [`StdError`] variant.
    pub errors: [u64; STD_ERROR_KINDS],
    /// TAIS records and SMES reports seen.
    pub records: u64,
    /// Records with at least one refused field.
    pub records_with_errors: u64,
    /// Records with no recognised field at all.
    pub empty_records: u64,
    /// Fields marked `r="1"` by the sender.
    pub invalidated: u64,
    /// Field errors per [`crate::FieldErrorKind`].
    pub field_error_kinds: [u64; ERROR_KIND_COUNT],
    /// TAIS records in which each field was present and valid.
    pub tais_present: [u64; TAIS_FIELD_COUNT],
    /// TAIS records in which each field was refused.
    pub tais_errors: [u64; TAIS_FIELD_COUNT],
    pub smes_present: [u64; SMES_FIELD_COUNT],
    pub smes_errors: [u64; SMES_FIELD_COUNT],
    /// Unknown element paths, at most [`Limits::max_unknown_paths`] distinct.
    pub unknown: Vec<UnknownPath>,
    /// Unknown elements whose path did not fit the table or the per-message cap.
    pub unknown_overflow: u64,
    /// Roots of messages of [`Kind::Unknown`]: `{namespace} name` text and count.
    pub unknown_roots: Vec<(String, u64)>,
    /// Unknown roots past the table.
    pub unknown_roots_overflow: u64,
}

/// An empty accumulator.
pub fn new_stats() -> Stats {
    Stats {
        messages: 0,
        kinds: [0; KIND_COUNT],
        errors: [0; STD_ERROR_KINDS],
        records: 0,
        records_with_errors: 0,
        empty_records: 0,
        invalidated: 0,
        field_error_kinds: [0; ERROR_KIND_COUNT],
        tais_present: [0; TAIS_FIELD_COUNT],
        tais_errors: [0; TAIS_FIELD_COUNT],
        smes_present: [0; SMES_FIELD_COUNT],
        smes_errors: [0; SMES_FIELD_COUNT],
        unknown: Vec::new(),
        unknown_overflow: 0,
        unknown_roots: Vec::new(),
        unknown_roots_overflow: 0,
    }
}

fn bump_bits(counts: &mut [u64], word: u32) {
    for (i, c) in counts.iter_mut().enumerate() {
        if word & (1u32 << i) != 0 {
            *c += 1;
        }
    }
}

impl Stats {
    /// Counts a message refused whole.
    pub fn observe_error(&mut self, e: &StdError) {
        self.messages += 1;
        if let Some(c) = self.errors.get_mut(std_error_index(e)) {
            *c += 1;
        }
    }

    /// Counts a message of kind `k`; the root of an unknown one is kept.
    pub fn observe_kind(&mut self, doc: &Doc<'_>, k: Kind) {
        self.messages += 1;
        if let Some(c) = self.kinds.get_mut(k as usize) {
            *c += 1;
        }
        if k != Kind::Unknown || doc.elem_name.is_empty() {
            return;
        }
        let root = ElemIx(0);
        let cut = |b: &[u8]| {
            String::from_utf8_lossy(b.get(..b.len().min(MAX_TEXT)).unwrap_or(&[])).into_owned()
        };
        let text = format!("{{{}}} {}", cut(ns_of(doc, root)), cut(name_of(doc, root)));
        if let Some(e) = self.unknown_roots.iter_mut().find(|(t, _)| *t == text) {
            e.1 += 1;
        } else if self.unknown_roots.len() < MAX_ROOTS {
            self.unknown_roots.push((text, 1));
        } else {
            self.unknown_roots_overflow += 1;
        }
    }

    fn observe_errors(&mut self, errors: &[FieldError]) {
        for e in errors {
            if let Some(c) = self.field_error_kinds.get_mut(e.kind as usize) {
                *c += 1;
            }
        }
    }

    fn merge_unknown(&mut self, unknown: &[(String, u64)], dropped: u64, limits: &Limits) {
        self.unknown_overflow += dropped;
        for (t, n) in unknown {
            if let Some(e) = self.unknown.iter_mut().find(|u| u.text == *t) {
                e.count += n;
            } else if self.unknown.len() < limits.max_unknown_paths {
                self.unknown.push(UnknownPath {
                    text: t.clone(),
                    count: *n,
                });
            } else {
                self.unknown_overflow += n;
            }
        }
    }

    pub(crate) fn observe_tais(
        &mut self,
        m: &TaisMessage,
        unknown: &[(String, u64)],
        dropped: u64,
        invalidated: u64,
        limits: &Limits,
    ) {
        self.invalidated += invalidated;
        self.observe_errors(&m.errors);
        self.merge_unknown(unknown, dropped, limits);
        for r in &m.records {
            self.records += 1;
            if r.errors != 0 {
                self.records_with_errors += 1;
            }
            if r.present == 0 && r.errors == 0 && r.invalidated == 0 {
                self.empty_records += 1;
            }
            bump_bits(&mut self.tais_present, r.present);
            bump_bits(&mut self.tais_errors, r.errors);
        }
    }

    pub(crate) fn observe_smes(
        &mut self,
        m: &SmesMessage,
        unknown: &[(String, u64)],
        dropped: u64,
        invalidated: u64,
        limits: &Limits,
    ) {
        self.invalidated += invalidated;
        self.observe_errors(&m.errors);
        self.merge_unknown(unknown, dropped, limits);
        for r in &m.records {
            self.records += 1;
            if r.errors != 0 {
                self.records_with_errors += 1;
            }
            if r.present == 0 && r.errors == 0 && r.invalidated == 0 {
                self.empty_records += 1;
            }
            bump_bits(&mut self.smes_present, r.present);
            bump_bits(&mut self.smes_errors, r.errors);
        }
    }
}
