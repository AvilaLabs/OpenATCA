//! Counters over many documents, and the bounded table of unknown paths. Plain data; updated
//! by the decoder. Core (pure).

use crate::fields::{FEAT_COUNT, FIELD_COUNT, REJECT_SLOTS};

/// Longest unknown-path text kept; a longer path is cut and ends in `~`.
pub const MAX_PATH_TEXT: usize = 160;

/// One unknown element or attribute path and how often it was seen.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug)]
pub struct UnknownPath {
    pub hash: u64,
    pub count: u64,
    /// True for an attribute (`path@name`), false for an element.
    pub is_attr: bool,
    pub len: u8,
    pub text: [u8; MAX_PATH_TEXT],
}

/// Totals over a run of documents, and the unknown paths seen.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug)]
pub struct Stats {
    /// Rows of the unknown-path table; paths beyond it are counted in `unknown_overflow`.
    pub max_unknown_paths: usize,
    pub docs: u64,
    pub accepted: u64,
    pub rows: u64,
    /// Refused documents by reason ([`crate::reject_index`]).
    pub rejects: [u64; REJECT_SLOTS],
    /// Field errors by [`crate::Field`], over accepted rows.
    pub field_errors: [u64; FIELD_COUNT],
    /// Accepted rows with a feature of kind `k + 1`.
    pub features: [u64; FEAT_COUNT],
    /// Accepted rows with a circle, and with at least one polygon.
    pub circles: u64,
    pub with_polygons: u64,
    pub polygons: u64,
    pub vertices: u64,
    /// Elements and attributes outside the surveyed structure (a whole unknown subtree counts
    /// once), over every document that reached the walk, accepted or not.
    pub unknown_elements: u64,
    pub unknown_attributes: u64,
    /// Unknown occurrences that did not fit the unknown-path table.
    pub unknown_overflow: u64,
    pub unknown: Vec<UnknownPath>,
}

/// All counters zero, with room for `max_unknown_paths` distinct unknown paths.
pub fn new_stats(max_unknown_paths: usize) -> Stats {
    Stats {
        max_unknown_paths,
        docs: 0,
        accepted: 0,
        rows: 0,
        rejects: [0; REJECT_SLOTS],
        field_errors: [0; FIELD_COUNT],
        features: [0; FEAT_COUNT],
        circles: 0,
        with_polygons: 0,
        polygons: 0,
        vertices: 0,
        unknown_elements: 0,
        unknown_attributes: 0,
        unknown_overflow: 0,
        unknown: Vec::with_capacity(max_unknown_paths),
    }
}

/// The display text of an unknown-path entry: element paths are `a/b/c` from the root's child
/// down, attribute paths end in `@name`.
pub fn unknown_text(u: &UnknownPath) -> &str {
    u.text
        .get(..usize::from(u.len))
        .and_then(|b| std::str::from_utf8(b).ok())
        .unwrap_or("")
}
