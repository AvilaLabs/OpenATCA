//! A string pool (one `String`, offsets into it) and the column helpers the tables share.

use crate::error::{ParseError, Reason, Refusal};
use oa_json::{Doc, Kind, NodeIx};

/// Index of a string in a [`Pool`]. `NONE` means "no string".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct StrIx(pub u32);
const _: () = assert!(core::mem::size_of::<StrIx>() == 4);

impl StrIx {
    pub const NONE: StrIx = StrIx(u32::MAX);
}

/// Strings stored back to back; string `i` is `text[off[i]..off[i + 1]]`.
// A7.2: cold struct (one per table), size guard waived
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pool {
    pub text: String,
    pub off: Vec<u32>,
}

/// An empty pool.
pub fn new_pool() -> Pool {
    Pool {
        text: String::new(),
        off: vec![0],
    }
}

/// Number of strings.
pub fn pool_len(p: &Pool) -> usize {
    p.off.len().saturating_sub(1)
}

/// Appends a string; `None` if the pool would exceed 4 GiB.
pub fn pool_push(p: &mut Pool, s: &str) -> Option<StrIx> {
    let ix = u32::try_from(pool_len(p)).ok()?;
    let end = u32::try_from(p.text.len().checked_add(s.len())?).ok()?;
    p.text.push_str(s);
    p.off.push(end);
    Some(StrIx(ix))
}

/// The string at `ix`, or `""` if out of range.
pub fn pool_get(p: &Pool, ix: StrIx) -> &str {
    let i = ix.0 as usize;
    let a = p.off.get(i).copied().unwrap_or(0) as usize;
    let b = p.off.get(i + 1).copied().unwrap_or(0) as usize;
    p.text.get(a..b).unwrap_or("")
}

/// A new pool holding strings `keep[0]`, `keep[1]`, ... of `p`, in that order.
pub fn pool_select(p: &Pool, keep: &[u32]) -> Pool {
    let mut out = new_pool();
    for &k in keep {
        let _ = pool_push(&mut out, pool_get(p, StrIx(k)));
    }
    out
}

/// Replaces `v` by `v[keep[0]], v[keep[1]], ...`. Indexes out of range are skipped (callers
/// build `keep` from the column length, so none are).
pub fn select<T: Clone>(v: &mut Vec<T>, keep: &[u32]) {
    let out: Vec<T> = keep
        .iter()
        .filter_map(|&k| v.get(k as usize).cloned())
        .collect();
    *v = out;
}

/// Checks the top-level value is an array within `max_records` and that `rows + records` stays
/// within `max_rows`; returns the array node and its length.
pub fn top_array(
    doc: &Doc<'_>,
    rows: usize,
    max_records: usize,
    max_rows: usize,
) -> Result<(NodeIx, usize), ParseError> {
    if doc.kind(NodeIx::ROOT) != Some(Kind::Array) {
        return Err(ParseError::NotArray);
    }
    let n = doc.len(NodeIx::ROOT);
    if n > max_records {
        return Err(ParseError::TooManyRecords);
    }
    if rows.saturating_add(n) > max_rows {
        return Err(ParseError::TableFull);
    }
    Ok((NodeIx::ROOT, n))
}

/// A refusal for a string longer than `max` bytes.
pub fn check_len(s: &str, max: usize, field: &'static str) -> Result<(), Refusal> {
    if s.len() > max {
        Err(Refusal {
            reason: Reason::OutOfRange,
            field,
        })
    } else {
        Ok(())
    }
}
