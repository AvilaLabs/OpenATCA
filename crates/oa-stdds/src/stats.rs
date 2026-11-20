//! Cumulative counts over many messages. Pure functions (crate class shell): a plain accumulator with no clock and no
//! I/O. Counts only: no position, identity or value is kept (the unknown-element table keeps
//! element names and root names, which are schema text, never message values).

use crate::kind::{name_of, ns_of, Kind, KIND_COUNT};
use crate::smes::{SmesMessage, SMES_FIELD_COUNT};
use crate::tais::TaisField;
use crate::tais::{TaisMessage, TAIS_FIELD_COUNT};
use crate::{
    std_error_index, FieldError, FieldErrorKind, Limits, StdError, ERROR_KIND_COUNT,
    STD_ERROR_KINDS,
};
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

/// Most distinct refused-field shapes kept.
const MAX_SHAPES: usize = 256;
/// Most distinct `recSTARSSrcID` values counted.
const MAX_SRC_IDS: usize = 4096;
/// Most distinct status values kept.
const MAX_STATUS_VALUES: usize = 64;

/// Number of latency buckets.
pub const LATENCY_BUCKETS: usize = 9;

/// Names of the latency buckets of `recSAFAReceiptTime - recSTARSTimestamp`, in milliseconds.
pub const LATENCY_BUCKET_NAMES: [&str; LATENCY_BUCKETS] = [
    "<0",
    "0-9",
    "10-49",
    "50-99",
    "100-249",
    "250-499",
    "500-999",
    "1000-4999",
    ">=5000",
];

/// The latency bucket of `ms`.
fn latency_bucket(ms: i64) -> usize {
    match ms {
        i64::MIN..=-1 => 0,
        0..=9 => 1,
        10..=49 => 2,
        50..=99 => 3,
        100..=249 => 4,
        250..=499 => 5,
        500..=999 => 6,
        1000..=4999 => 7,
        _ => 8,
    }
}

/// How often a refused field had one shape (see [`crate::Shape`]).
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShapeCount {
    pub field: &'static str,
    pub kind: FieldErrorKind,
    pub len: u16,
    pub pattern: String,
    pub count: u64,
}

/// Counts. Fields are public plain data; [`new_stats`] makes an empty one.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stats {
    /// Messages seen (parsed XML or not).
    pub messages: u64,
    /// Messages per [`Kind`].
    pub kinds: [u64; KIND_COUNT],
    /// Records and reports dropped because their own time is in [`Limits::drop_window`]: not in
    /// any other count.
    pub sealed_window_dropped: u64,
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
    /// Records in which a text field's element was present with no text (not an error).
    pub tais_present_empty: [u64; TAIS_FIELD_COUNT],
    pub smes_present_empty: [u64; SMES_FIELD_COUNT],
    /// Refused-field shapes (length and character classes, never a value), at most 256.
    pub shapes: Vec<ShapeCount>,
    /// Refused fields whose shape did not fit the table.
    pub shapes_overflow: u64,
    /// TAIS tracks per flag (`pseudo`, `frozen`, `new`, `adsb`, `coast`) as `[false, true]`.
    pub tais_flags: [[u64; 2]; 5],
    /// TAIS records per latency bucket of `recSAFAReceiptTime - recSTARSTimestamp`.
    pub latency: [u64; LATENCY_BUCKETS],
    /// TAIS records lacking either time.
    pub latency_missing: u64,
    /// Distinct `recSTARSSrcID` values seen (the values are kept to count them, never printed).
    pub src_ids: Vec<[u8; 8]>,
    /// `recSTARSSrcID` values past the table.
    pub src_ids_overflow: u64,
    /// Service-status messages: `service name / status value` text and count.
    pub status_values: Vec<(String, u64)>,
    /// Status states found in a message but not kept (more than the per-message limit, or an
    /// empty or unreadable name or value).
    pub status_dropped: u64,
    /// Status values whose text is longer than the limit, and keys likewise.
    pub status_value_too_long: u64,
    pub status_key_too_long: u64,
    /// Status states past the table of distinct values.
    pub status_table_overflow: u64,
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
        sealed_window_dropped: 0,
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
        tais_present_empty: [0; TAIS_FIELD_COUNT],
        smes_present_empty: [0; SMES_FIELD_COUNT],
        shapes: Vec::new(),
        shapes_overflow: 0,
        tais_flags: [[0; 2]; 5],
        latency: [0; LATENCY_BUCKETS],
        latency_missing: 0,
        src_ids: Vec::new(),
        src_ids_overflow: 0,
        status_values: Vec::new(),
        status_dropped: 0,
        status_value_too_long: 0,
        status_key_too_long: 0,
        status_table_overflow: 0,
        unknown: Vec::new(),
        unknown_overflow: 0,
        unknown_roots: Vec::new(),
        unknown_roots_overflow: 0,
    }
}

/// `t` cut to [`MAX_TEXT`] bytes at a character boundary.
fn cap_text(t: &str) -> String {
    let mut n = t.len().min(MAX_TEXT);
    while !t.is_char_boundary(n) {
        n -= 1;
    }
    t.get(..n).unwrap_or("").to_string()
}

fn bump_bits(counts: &mut [u64], word: u64) {
    for (i, c) in counts.iter_mut().enumerate() {
        if word & (1u64 << i) != 0 {
            *c += 1;
        }
    }
}

impl Stats {
    /// Counts a refusal by reason. It does not count a message: [`crate::parse_message`]
    /// counts each message once (a refusal after classification is already in `kinds`).
    pub fn observe_error(&mut self, e: &StdError) {
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
            let pattern = e.shape.pattern();
            let hit = self.shapes.iter_mut().find(|s| {
                s.field == e.field
                    && s.kind == e.kind
                    && s.len == e.shape.len
                    && s.pattern == pattern
            });
            if let Some(h) = hit {
                h.count += 1;
            } else if self.shapes.len() < MAX_SHAPES {
                self.shapes.push(ShapeCount {
                    field: e.field,
                    kind: e.kind,
                    len: e.shape.len,
                    pattern,
                    count: 1,
                });
            } else {
                self.shapes_overflow += 1;
            }
        }
    }

    fn merge_unknown(&mut self, unknown: &[(String, u64)], dropped: u64, limits: &Limits) {
        self.unknown_overflow += dropped;
        for (t, n) in unknown {
            let t = &cap_text(t);
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
            if r.present == 0 && r.errors == 0 && r.invalidated == 0 && r.present_empty == 0 {
                self.empty_records += 1;
            }
            bump_bits(&mut self.tais_present, r.present);
            bump_bits(&mut self.tais_errors, r.errors);
            bump_bits(&mut self.tais_present_empty, r.present_empty);
            let has = |f: TaisField| r.present & (1u64 << (f as u32)) != 0;
            for (i, (f, v)) in [
                (TaisField::TrackPseudo, r.pseudo),
                (TaisField::TrackFrozen, r.frozen),
                (TaisField::TrackNew, r.is_new),
                (TaisField::TrackAdsb, r.adsb),
                (TaisField::TrackCoast, r.coast),
            ]
            .into_iter()
            .enumerate()
            {
                if has(f) {
                    if let Some(c) = self
                        .tais_flags
                        .get_mut(i)
                        .and_then(|p| p.get_mut(usize::from(v)))
                    {
                        *c += 1;
                    }
                }
            }
            if has(TaisField::RecStarsTimestamp) && has(TaisField::RecSafaReceiptTime) {
                // Receipt time of day minus the STARS time of day, wrapped into a day and then
                // folded into -12 h..=12 h, so a skew of a few seconds is negative and a true
                // midnight crossing is a small positive latency.
                let mut d = (r.rec_safa_receipt_ms.rem_euclid(86_400_000)
                    - i64::from(r.rec_stars_tod_ms))
                .rem_euclid(86_400_000);
                if d > 43_200_000 {
                    d -= 86_400_000;
                }
                if let Some(c) = self.latency.get_mut(latency_bucket(d)) {
                    *c += 1;
                }
            } else {
                self.latency_missing += 1;
            }
            if has(TaisField::RecStarsSrcId) {
                if self.src_ids.contains(&r.rec_stars_src_id) {
                } else if self.src_ids.len() < MAX_SRC_IDS {
                    self.src_ids.push(r.rec_stars_src_id);
                } else {
                    self.src_ids_overflow += 1;
                }
            }
        }
    }

    pub(crate) fn observe_status(&mut self, m: &crate::status::StatusMessage) {
        self.status_dropped += m.dropped;
        self.status_value_too_long += m.value_too_long;
        self.status_key_too_long += m.key_too_long;
        for st in &m.states {
            let text = format!("{} {}={}", m.root, st.name, st.value);
            if let Some(e) = self.status_values.iter_mut().find(|(t, _)| *t == text) {
                e.1 += 1;
            } else if self.status_values.len() < MAX_STATUS_VALUES {
                self.status_values.push((text, 1));
            } else {
                self.status_table_overflow += 1;
            }
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
            if r.present == 0 && r.errors == 0 && r.invalidated == 0 && r.present_empty == 0 {
                self.empty_records += 1;
            }
            bump_bits(&mut self.smes_present, r.present);
            bump_bits(&mut self.smes_errors, r.errors);
            bump_bits(&mut self.smes_present_empty, r.present_empty);
        }
    }
}
