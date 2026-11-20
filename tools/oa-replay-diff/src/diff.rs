//! The pure core of `oa-replay-diff` (B1, B2): comparison of two tracker runs over one clip from
//! their per-second digests and, where they differ, their canonical rows
//! (`oa_track::canon`). Nothing here opens a file, reads a stream or prints; the shell
//! ([`crate::files`]) feeds it text, rows and groups of rows and writes what it returns.
//!
//! Classification of every public item (all core, pure, unless noted):
//!
//! | item | what |
//! |---|---|
//! | [`second_in_window`], [`digest_seconds_in_window`], [`row_in_window`] | the sealed window against seconds and rows |
//! | [`parse_digest`] | text to verified [`DigestLine`]s: header, fields, order, limits, chain |
//! | [`compare_digests`], [`first_difference`] | [`DigestDiff`] of two digests |
//! | [`diff_second`] | [`SecondDiff`] of the rows of one second |
//! | [`totals_add`], [`totals_present`], [`second_diff_is_empty`] | sums and class names |
//! | [`parse_expect`], [`undeclared`] | declared classes |
//! | [`group_rows`], [`Grouper`], [`grouper_push`], [`grouper_finish`] | rows to per-second groups |
//! | [`compare_rows`] | verifies both rows files against their digests, diffs differing seconds |
//! | [`render_report`], [`render_summary`], [`verdict`], [`iso`] | text and exit code |
//! | [`class_name`], [`class_index`], [`field_size`] | tables |
//! | plain data | [`DigestLine`], [`DigestDiff`], [`SecondDiff`], [`Totals`], [`RowsResult`], [`Comparison`], [`Class`] |
//!
//! # Trust (E5, A8)
//!
//! A digest is accepted only with its header, every field parsed, seconds strictly increasing,
//! sizes within the named limits and the chain hash of every line recomputed from the line and
//! the one before it. A rows file is used only after [`compare_rows`] has recomputed the hash and
//! the count of every one of its seconds from its rows and found them equal to the digest line,
//! and has found no digest second missing from it and no second in it that the digest lacks. A
//! difference found without that is not reported.
//!
//! # Matching rows (E6, E7)
//!
//! Within a second rows are paired first by (track address, time bytes, tick flag, n-th among
//! rows with these equal in emission order). A row left over on each side with the same (address,
//! time) is paired next, so a flip of the tick bit is a `flags` change of one row, not a removal
//! plus an addition. A key held by more than one row in either run is an ambiguous pairing: which
//! row goes with which is decided by emission order alone, so it is counted and is a class that a
//! declared difference must name (`ambiguous`). `reordered` is read from the matched pairs
//! alone, whatever else changed.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use oa_sealed::DropWindow;
use oa_track::canon::{
    chain_next, field_at, from_hex32, hash_rows, row_second, CanonRow, DIGEST_HEADER,
    MAX_ROWS_PER_SECOND, MAX_SECONDS, OFF_ADDR, OFF_ALT, OFF_ALT_VAR, OFF_FLAGS, OFF_LAT, OFF_LON,
    OFF_MU_H, OFF_MU_V, OFF_POS_COV, OFF_TURN, OFF_T_NS, OFF_VE, OFF_VEL_COV, OFF_VN, OFF_VRATE,
    ROW_BYTES,
};
use oa_track::O_TICK;

// ---------------------------------------------------------------- limits

/// Most lines a digest file may have (C2): the digest's own second limit.
pub const MAX_DIGEST_LINES: usize = MAX_SECONDS;
/// Most bytes a digest file may have: a line is at most 4 + 20 + 1 + 64 + 1 + 64 + 1 + 20 bytes.
pub const MAX_DIGEST_BYTES: u64 = (MAX_DIGEST_LINES as u64) * 192;
/// Most bytes an expect file may have.
pub const MAX_EXPECT_BYTES: u64 = 1 << 16;

/// `usize` to `u64` without a cast that could truncate.
fn count(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

// ---------------------------------------------------------------- digests

/// One line of a digest file.
#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct DigestLine {
    pub second: i64,
    pub chain: [u8; 32],
    pub hash: [u8; 32],
    pub rows: u64,
}

/// Parses a digest file. The first non-blank line must be the version header
/// ([`DIGEST_HEADER`]); the last must be `end <seconds> <rows> <chain>` and equal what the lines
/// before it give (a file cut at a line boundary has no such line); every other non-blank line
/// is `second chain_hash second_hash rows`, the seconds strictly increasing. A line count, byte
/// count or row count above the named limits, a total that overflows, and a chain hash that is
/// not the chain of the lines before it are errors naming the line.
pub fn parse_digest(text: &str) -> Result<Vec<DigestLine>, String> {
    if count(text.len()) > MAX_DIGEST_BYTES {
        return Err(format!(
            "digest is {} bytes, more than the limit of {MAX_DIGEST_BYTES}",
            text.len()
        ));
    }
    let mut out: Vec<DigestLine> = Vec::new();
    let mut header = false;
    let mut ended = false;
    let mut prev = [0u8; 32];
    let mut total = 0u64;
    for (n, line) in text.lines().enumerate() {
        let n = n.saturating_add(1);
        if line.trim().is_empty() {
            if !header {
                return Err(format!("line {n}: a blank line before the digest header"));
            }
            continue;
        }
        if ended {
            return Err(format!("line {n}: text after the end line"));
        }
        if !header {
            if line.trim_end() != DIGEST_HEADER {
                return Err(format!(
                    "line {n}: not the digest header `{DIGEST_HEADER}` (another version?)"
                ));
            }
            header = true;
            continue;
        }
        if line.split_whitespace().next() == Some("end") {
            let want = format!(
                "end {} {} {}",
                out.len(),
                total,
                oa_track::canon::hex32(&prev)
            );
            if line != want {
                return Err(format!(
                    "line {n}: the end line does not match the lines before it (expected exactly `{want}`, fields on single spaces)"
                ));
            }
            ended = true;
            continue;
        }
        if out.len() >= MAX_DIGEST_LINES {
            return Err(format!(
                "line {n}: more than {MAX_DIGEST_LINES} seconds (limit)"
            ));
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        let [sec, chain, hash, rows] = f.as_slice() else {
            return Err(format!("line {n}: expected 4 fields, found {}", f.len()));
        };
        let second = sec
            .parse::<i64>()
            .map_err(|_| format!("line {n}: bad second `{sec}`"))?;
        let rows = rows
            .parse::<u64>()
            .map_err(|_| format!("line {n}: bad row count `{rows}`"))?;
        let (Some(chain), Some(hash)) = (from_hex32(chain), from_hex32(hash)) else {
            return Err(format!("line {n}: a hash is not 64 lowercase hex digits"));
        };
        if rows == 0 || rows > MAX_ROWS_PER_SECOND {
            return Err(format!(
                "line {n}: {rows} rows in a second (between 1 and {MAX_ROWS_PER_SECOND} allowed)"
            ));
        }
        if out.last().is_some_and(|p| p.second >= second) {
            return Err(format!("line {n}: seconds are not strictly increasing"));
        }
        total = total
            .checked_add(rows)
            .ok_or_else(|| format!("line {n}: the row counts add up past the largest count"))?;
        prev = chain_next(&prev, second, &hash, rows);
        if prev != chain {
            return Err(format!(
                "line {n}: the chain hash is not the chain of the lines before it"
            ));
        }
        out.push(DigestLine {
            second,
            chain,
            hash,
            rows,
        });
    }
    if !header {
        return Err(format!("no digest header `{DIGEST_HEADER}`"));
    }
    if !ended {
        return Err("no end line: the digest is cut short or was not finished".to_string());
    }
    Ok(out)
}

/// The total rows of a digest (saturating; a parsed digest cannot overflow).
pub fn digest_rows(lines: &[DigestLine]) -> u64 {
    lines.iter().fold(0u64, |a, l| a.saturating_add(l.rows))
}

/// Two digests with no seconds have nothing to compare; calling them identical would pass an
/// empty run against an empty run.
pub fn nothing_to_compare(a: &[DigestLine], b: &[DigestLine]) -> Result<(), String> {
    if a.is_empty() && b.is_empty() {
        return Err("no seconds to compare: both digests are empty".to_string());
    }
    Ok(())
}

/// Whether any part of the Unix second `second` is in the window (a second is a span of 1000
/// milliseconds; the fail-closed all-time window holds every second).
pub fn second_in_window(w: DropWindow, second: i64) -> bool {
    if w == DropWindow::all_time() {
        return true;
    }
    w.span().is_some_and(|(from, to)| {
        second.saturating_mul(1000) < to && second.saturating_add(1).saturating_mul(1000) > from
    })
}

/// How many seconds of a digest are in the window.
pub fn digest_seconds_in_window(lines: &[DigestLine], w: DropWindow) -> u64 {
    lines
        .iter()
        .filter(|l| second_in_window(w, l.second))
        .count() as u64
}

/// Whether the time of a canonical row is in the window.
pub fn row_in_window(w: DropWindow, row: &CanonRow) -> bool {
    w.contains(i64::from_le_bytes(field_at::<OFF_T_NS, 8>(row)).div_euclid(1_000_000))
}

/// How two digests compare.
#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct DigestDiff {
    pub identical: bool,
    pub seconds_a: u64,
    pub seconds_b: u64,
    pub rows_a: u64,
    pub rows_b: u64,
    /// Seconds present in only one run, or whose rows differ, ascending.
    pub differing: Vec<i64>,
    pub only_a: u64,
    pub only_b: u64,
    /// The lines differ (a chain hash) although every second's rows are equal and present in
    /// both: not a case a verified digest can show, kept as a difference when it is given.
    pub chain_only: bool,
}

/// The first differing second, if any.
pub fn first_difference(d: &DigestDiff) -> Option<i64> {
    d.differing.first().copied()
}

/// Compares two digests second by second. Identical means every line, chain hash included, is
/// equal. A second counts as differing when its rows' hash or count differs or only one run has
/// it; the chain hashes after the first difference all differ and are not counted again.
pub fn compare_digests(a: &[DigestLine], b: &[DigestLine]) -> DigestDiff {
    let ma: BTreeMap<i64, &DigestLine> = a.iter().map(|l| (l.second, l)).collect();
    let mb: BTreeMap<i64, &DigestLine> = b.iter().map(|l| (l.second, l)).collect();
    let all: BTreeSet<i64> = ma.keys().chain(mb.keys()).copied().collect();
    let (mut differing, mut only_a, mut only_b) = (Vec::new(), 0u64, 0u64);
    for s in all {
        match (ma.get(&s), mb.get(&s)) {
            (Some(x), Some(y)) => {
                if x.hash != y.hash || x.rows != y.rows {
                    differing.push(s);
                }
            }
            (Some(_), None) => {
                only_a = only_a.saturating_add(1);
                differing.push(s);
            }
            (None, Some(_)) => {
                only_b = only_b.saturating_add(1);
                differing.push(s);
            }
            (None, None) => {}
        }
    }
    let identical = a == b;
    DigestDiff {
        identical,
        seconds_a: count(a.len()),
        seconds_b: count(b.len()),
        rows_a: digest_rows(a),
        rows_b: digest_rows(b),
        chain_only: !identical && differing.is_empty(),
        differing,
        only_a,
        only_b,
    }
}

// ---------------------------------------------------------------- rows

/// A class of output field, for what a declared difference may touch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    Position,
    Velocity,
    Covariance,
    Flags,
    Model,
}

/// Every class, in report order.
pub const CLASSES: [Class; 5] = [
    Class::Position,
    Class::Velocity,
    Class::Covariance,
    Class::Flags,
    Class::Model,
];

// The class index is the position in CLASSES, so `class_rows` is indexed by it.
#[allow(clippy::indexing_slicing)]
const _: () = {
    let mut i = 0;
    while i < CLASSES.len() {
        assert!(CLASSES[i] as usize == i);
        i += 1;
    }
};

pub fn class_name(c: Class) -> &'static str {
    match c {
        Class::Position => "position",
        Class::Velocity => "velocity",
        Class::Covariance => "covariance",
        Class::Flags => "flags",
        Class::Model => "model",
    }
}

/// The position of a class in [`CLASSES`] (checked at compile time).
pub fn class_index(c: Class) -> usize {
    c as usize
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    I32,
    F32,
    U8,
}

/// One field of a canonical row.
// A7.2: cold struct, size guard waived
struct Field {
    name: &'static str,
    off: usize,
    kind: Kind,
    class: Class,
}

const fn f(name: &'static str, off: usize, kind: Kind, class: Class) -> Field {
    Field {
        name,
        off,
        kind,
        class,
    }
}

fn field_size(fl: &Field) -> usize {
    match fl.kind {
        Kind::U8 => 1,
        Kind::I32 | Kind::F32 => 4,
    }
}

const fn kind_size(k: Kind) -> usize {
    match k {
        Kind::U8 => 1,
        Kind::I32 | Kind::F32 => 4,
    }
}

/// The fields compared, with their class. Latitude and longitude are in 1e-7 degrees, altitude
/// and its variance in feet and feet squared, vertical rate in feet per minute.
pub const NF: usize = 19;
const FIELDS: [Field; NF] = [
    f("lat_e7", OFF_LAT, Kind::I32, Class::Position),
    f("lon_e7", OFF_LON, Kind::I32, Class::Position),
    f("alt_ft", OFF_ALT, Kind::F32, Class::Position),
    f("vel_east_mps", OFF_VE, Kind::F32, Class::Velocity),
    f("vel_north_mps", OFF_VN, Kind::F32, Class::Velocity),
    f("turn_rate_rps", OFF_TURN, Kind::F32, Class::Velocity),
    f("vrate_fpm", OFF_VRATE, Kind::F32, Class::Velocity),
    f("alt_var_ft2", OFF_ALT_VAR, Kind::F32, Class::Covariance),
    f("pos_cov_ee", OFF_POS_COV, Kind::F32, Class::Covariance),
    f("pos_cov_en", OFF_POS_COV + 4, Kind::F32, Class::Covariance),
    f("pos_cov_nn", OFF_POS_COV + 8, Kind::F32, Class::Covariance),
    f("vel_cov_ee", OFF_VEL_COV, Kind::F32, Class::Covariance),
    f("vel_cov_en", OFF_VEL_COV + 4, Kind::F32, Class::Covariance),
    f("vel_cov_nn", OFF_VEL_COV + 8, Kind::F32, Class::Covariance),
    f("flags", OFF_FLAGS, Kind::U8, Class::Flags),
    f("mu_h_straight", OFF_MU_H, Kind::F32, Class::Model),
    f("mu_h_turn", OFF_MU_H + 4, Kind::F32, Class::Model),
    f("mu_v_level", OFF_MU_V, Kind::F32, Class::Model),
    f("mu_v_climb", OFF_MU_V + 4, Kind::F32, Class::Model),
];

// D4/F18: the compared fields cover every byte of the row except the key (address and time,
// bytes 0..OFF_LAT), each byte once. A new byte in the row without a field here is a compile error.
#[allow(clippy::indexing_slicing)]
const _: () = {
    let mut seen = [false; ROW_BYTES];
    let mut i = 0;
    while i < NF {
        let mut b = FIELDS[i].off;
        while b < FIELDS[i].off + kind_size(FIELDS[i].kind) {
            assert!(
                b < ROW_BYTES && !seen[b],
                "a field lies outside or overlaps"
            );
            seen[b] = true;
            b += 1;
        }
        i += 1;
    }
    let mut b = OFF_LAT;
    while b < ROW_BYTES {
        assert!(seen[b], "a byte of the row has no compared field");
        b += 1;
    }
    assert!(OFF_T_NS + 8 == OFF_LAT && OFF_ADDR + 4 == OFF_T_NS);
};

/// The bytes of one field of a row; an error if the field lies outside the row.
fn field_bytes<'a>(fl: &Field, row: &'a CanonRow) -> Result<&'a [u8], String> {
    row.get(fl.off..fl.off + field_size(fl))
        .ok_or_else(|| format!("field {} lies outside the canonical row", fl.name))
}

fn word(b: &[u8]) -> Result<u32, String> {
    <[u8; 4]>::try_from(b)
        .map(u32::from_le_bytes)
        .map_err(|_| "a 4-byte field is not 4 bytes".to_string())
}

/// The matching key of a row within a second: track address, time, whether it is a tick row.
type RowKey = (u32, [u8; 8], bool);

fn key_of(row: &CanonRow) -> RowKey {
    (
        u32::from_le_bytes(field_at::<OFF_ADDR, 4>(row)),
        field_at::<OFF_T_NS, 8>(row),
        field_at::<OFF_FLAGS, 1>(row)[0] & O_TICK != 0,
    )
}

/// Difference of one field between two rows: `(bits differ, absolute difference if both finite,
/// either is not finite)`.
fn field_delta(
    fl: &Field,
    a: &CanonRow,
    b: &CanonRow,
) -> Result<(bool, Option<f64>, bool), String> {
    let (x, y) = (field_bytes(fl, a)?, field_bytes(fl, b)?);
    if x == y {
        return Ok((false, None, false));
    }
    Ok(match fl.kind {
        Kind::U8 => {
            let (p, q) = (
                x.first().copied().unwrap_or(0),
                y.first().copied().unwrap_or(0),
            );
            (true, Some((f64::from(p) - f64::from(q)).abs()), false)
        }
        Kind::I32 => {
            let (p, q) = (word(x)? as i32, word(y)? as i32);
            (true, Some((f64::from(p) - f64::from(q)).abs()), false)
        }
        Kind::F32 => {
            let (p, q) = (f32::from_bits(word(x)?), f32::from_bits(word(y)?));
            if p.is_finite() && q.is_finite() {
                (true, Some((f64::from(p) - f64::from(q)).abs()), false)
            } else {
                (true, None, true)
            }
        }
    })
}

/// How the rows of one second differ. Rows are paired as the module header says.
#[derive(Debug, Clone, Default, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct SecondDiff {
    pub rows_added: u64,
    pub rows_removed: u64,
    pub rows_changed: u64,
    /// Tracks (addresses) with at least one row added, removed or changed.
    pub tracks_added: BTreeSet<u32>,
    pub tracks_removed: BTreeSet<u32>,
    pub tracks_changed: BTreeSet<u32>,
    /// Changed rows by class (a row that changed in two classes counts in both).
    pub class_rows: [u64; 5],
    /// Largest absolute difference per field, over the finite pairs.
    pub field_max: [f64; NF],
    /// Rows where the field differs.
    pub field_rows: [u64; NF],
    /// Rows where the field differs and one value is NaN or infinite.
    pub field_nonfinite: [u64; NF],
    /// Bits of the flags that differ in any row.
    pub flags_xor: u8,
    /// The matched rows are not in the same order in both runs (whatever else differs).
    pub reordered: bool,
    /// Keys held by more than one row in either run: their pairing is by emission order alone.
    pub ambiguous_pairs: u64,
}

/// Whether nothing differs in the second.
pub fn second_diff_is_empty(d: &SecondDiff) -> bool {
    d.rows_added == 0 && d.rows_removed == 0 && d.rows_changed == 0 && !d.reordered
}

/// The pairing of the rows of one second.
struct Pairing {
    /// `(index in a, index in b)`, ascending in the first.
    pairs: Vec<(usize, usize)>,
    removed: Vec<usize>,
    added: Vec<usize>,
    ambiguous: u64,
}

fn index_by_key(rows: &[CanonRow], idx: &[usize]) -> BTreeMap<RowKey, Vec<usize>> {
    let mut m: BTreeMap<RowKey, Vec<usize>> = BTreeMap::new();
    for &i in idx {
        if let Some(r) = rows.get(i) {
            m.entry(key_of(r)).or_default().push(i);
        }
    }
    m
}

fn index_by_time(rows: &[CanonRow], idx: &[usize]) -> BTreeMap<(u32, [u8; 8]), Vec<usize>> {
    let mut m: BTreeMap<(u32, [u8; 8]), Vec<usize>> = BTreeMap::new();
    for &i in idx {
        if let Some(r) = rows.get(i) {
            let k = key_of(r);
            m.entry((k.0, k.1)).or_default().push(i);
        }
    }
    m
}

fn pair_rows(a: &[CanonRow], b: &[CanonRow]) -> Pairing {
    let all_a: Vec<usize> = (0..a.len()).collect();
    let all_b: Vec<usize> = (0..b.len()).collect();
    let (ma, mb) = (index_by_key(a, &all_a), index_by_key(b, &all_b));
    let mut p = Pairing {
        pairs: Vec::new(),
        removed: Vec::new(),
        added: Vec::new(),
        ambiguous: 0,
    };
    let (mut left_a, mut left_b) = (Vec::new(), Vec::new());
    let none = Vec::new();
    let keys: BTreeSet<&RowKey> = ma.keys().chain(mb.keys()).collect();
    for k in keys {
        let (ia, ib) = (ma.get(k).unwrap_or(&none), mb.get(k).unwrap_or(&none));
        if ia.len() > 1 || ib.len() > 1 {
            p.ambiguous = p.ambiguous.saturating_add(1);
        }
        let n = ia.len().min(ib.len());
        p.pairs.extend(ia.iter().copied().zip(ib.iter().copied()));
        left_a.extend(ia.iter().copied().skip(n));
        left_b.extend(ib.iter().copied().skip(n));
    }
    // Second pass: what is left, paired by address and time alone (a tick-bit flip).
    left_a.sort_unstable();
    left_b.sort_unstable();
    let (ta, tb) = (index_by_time(a, &left_a), index_by_time(b, &left_b));
    let tkeys: BTreeSet<&(u32, [u8; 8])> = ta.keys().chain(tb.keys()).collect();
    let none = Vec::new();
    for k in tkeys {
        let (ia, ib) = (ta.get(k).unwrap_or(&none), tb.get(k).unwrap_or(&none));
        if ia.len() > 1 || ib.len() > 1 {
            p.ambiguous = p.ambiguous.saturating_add(1);
        }
        let n = ia.len().min(ib.len());
        p.pairs.extend(ia.iter().copied().zip(ib.iter().copied()));
        p.removed.extend(ia.iter().copied().skip(n));
        p.added.extend(ib.iter().copied().skip(n));
    }
    p.pairs.sort_unstable();
    p
}

/// Compares the rows of one second in two runs.
pub fn diff_second(a: &[CanonRow], b: &[CanonRow]) -> Result<SecondDiff, String> {
    let p = pair_rows(a, b);
    let mut d = SecondDiff {
        ambiguous_pairs: p.ambiguous,
        ..SecondDiff::default()
    };
    for r in p.removed.iter().filter_map(|&i| a.get(i)) {
        d.rows_removed = d.rows_removed.saturating_add(1);
        d.tracks_removed.insert(key_of(r).0);
    }
    for r in p.added.iter().filter_map(|&i| b.get(i)) {
        d.rows_added = d.rows_added.saturating_add(1);
        d.tracks_added.insert(key_of(r).0);
    }
    for &(ia, ib) in &p.pairs {
        let (Some(ra), Some(rb)) = (a.get(ia), b.get(ib)) else {
            return Err("a paired row index is outside its second".to_string());
        };
        if ra == rb {
            continue;
        }
        d.rows_changed = d.rows_changed.saturating_add(1);
        d.tracks_changed.insert(key_of(ra).0);
        let mut mask = 0u8;
        for ((fl, rows_n), (nonfin_n, max)) in FIELDS
            .iter()
            .zip(d.field_rows.iter_mut())
            .zip(d.field_nonfinite.iter_mut().zip(d.field_max.iter_mut()))
        {
            let (differs, abs, nonfinite) = field_delta(fl, ra, rb)?;
            if !differs {
                continue;
            }
            mask |= 1 << class_index(fl.class);
            *rows_n = rows_n.saturating_add(1);
            if nonfinite {
                *nonfin_n = nonfin_n.saturating_add(1);
            }
            if let Some(x) = abs {
                *max = max.max(x);
            }
            if fl.kind == Kind::U8 {
                d.flags_xor |= field_at::<OFF_FLAGS, 1>(ra)[0] ^ field_at::<OFF_FLAGS, 1>(rb)[0];
            }
        }
        for (bit, n) in d.class_rows.iter_mut().enumerate() {
            *n = n.saturating_add(u64::from((mask >> bit) & 1));
        }
    }
    d.reordered = p.pairs.windows(2).any(|w| match w {
        [x, y] => x.1 > y.1,
        _ => false,
    });
    Ok(d)
}

/// The differences of a whole comparison, summed.
#[derive(Debug, Clone, Default, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct Totals {
    /// Seconds whose rows differ in any way.
    pub seconds: u64,
    pub rows_added: u64,
    pub rows_removed: u64,
    pub rows_changed: u64,
    pub reordered_seconds: u64,
    pub ambiguous_pairs: u64,
    pub tracks_added: BTreeSet<u32>,
    pub tracks_removed: BTreeSet<u32>,
    pub tracks_changed: BTreeSet<u32>,
    pub class_rows: [u64; 5],
    pub field_max: [f64; NF],
    pub field_rows: [u64; NF],
    pub field_nonfinite: [u64; NF],
    pub flags_xor: u8,
}

/// Adds one second's differences to the totals.
pub fn totals_add(t: &mut Totals, d: &SecondDiff) {
    t.ambiguous_pairs = t.ambiguous_pairs.saturating_add(d.ambiguous_pairs);
    if second_diff_is_empty(d) {
        return;
    }
    t.seconds = t.seconds.saturating_add(1);
    t.rows_added = t.rows_added.saturating_add(d.rows_added);
    t.rows_removed = t.rows_removed.saturating_add(d.rows_removed);
    t.rows_changed = t.rows_changed.saturating_add(d.rows_changed);
    t.reordered_seconds = t.reordered_seconds.saturating_add(u64::from(d.reordered));
    t.tracks_added.extend(&d.tracks_added);
    t.tracks_removed.extend(&d.tracks_removed);
    t.tracks_changed.extend(&d.tracks_changed);
    for (a, x) in t.class_rows.iter_mut().zip(d.class_rows) {
        *a = a.saturating_add(x);
    }
    for (a, x) in t.field_rows.iter_mut().zip(d.field_rows) {
        *a = a.saturating_add(x);
    }
    for (a, x) in t.field_nonfinite.iter_mut().zip(d.field_nonfinite) {
        *a = a.saturating_add(x);
    }
    for (a, x) in t.field_max.iter_mut().zip(d.field_max) {
        *a = a.max(x);
    }
    t.flags_xor |= d.flags_xor;
}

/// The declarable classes with at least one difference: the field classes by name, and
/// `added`, `removed`, `order` and `ambiguous`.
pub fn totals_present(t: &Totals) -> BTreeSet<&'static str> {
    let mut s = BTreeSet::new();
    for (c, n) in CLASSES.iter().zip(t.class_rows) {
        if n > 0 {
            s.insert(class_name(*c));
        }
    }
    if t.rows_added > 0 {
        s.insert("added");
    }
    if t.rows_removed > 0 {
        s.insert("removed");
    }
    if t.reordered_seconds > 0 {
        s.insert("order");
    }
    if t.ambiguous_pairs > 0 {
        s.insert("ambiguous");
    }
    s
}

// ---------------------------------------------------------------- declared differences

/// The names an expect file may list.
pub const DECLARABLE: [&str; 9] = [
    "position",
    "velocity",
    "covariance",
    "flags",
    "model",
    "added",
    "removed",
    "order",
    "ambiguous",
];

/// Parses an expect file: one class name per line, `#` comments and blank lines skipped.
pub fn parse_expect(text: &str) -> Result<BTreeSet<String>, String> {
    if count(text.len()) > MAX_EXPECT_BYTES {
        return Err(format!("expect file is over {MAX_EXPECT_BYTES} bytes"));
    }
    let mut out = BTreeSet::new();
    for (n, line) in text.lines().enumerate() {
        let l = line.split('#').next().unwrap_or("").trim();
        if l.is_empty() {
            continue;
        }
        if !DECLARABLE.contains(&l) {
            return Err(format!(
                "line {}: unknown class `{l}` (one of {})",
                n.saturating_add(1),
                DECLARABLE.join(", ")
            ));
        }
        out.insert(l.to_string());
    }
    Ok(out)
}

/// The classes that differ but are not declared.
pub fn undeclared(totals: &Totals, allowed: &BTreeSet<String>) -> Vec<&'static str> {
    totals_present(totals)
        .into_iter()
        .filter(|c| !allowed.contains(*c))
        .collect()
}

// ---------------------------------------------------------------- groups of rows

/// The rows of one Unix second, in file order.
pub type Group = (i64, Vec<CanonRow>);

/// Builds [`Group`]s from rows pushed in file order (A1: plain data; [`grouper_push`] and
/// [`grouper_finish`] are its functions).
#[derive(Debug, Default)]
// A7.2: cold struct, size guard waived
pub struct Grouper {
    cur: Option<Group>,
}

/// Adds a row; returns the group it completed, if its second is a new one. Seconds must ascend
/// strictly from group to group, and a group may hold [`MAX_ROWS_PER_SECOND`] rows at most.
pub fn grouper_push(g: &mut Grouper, row: CanonRow) -> Result<Option<Group>, String> {
    let s = row_second(&row);
    match &mut g.cur {
        Some((cs, rows)) if *cs == s => {
            if count(rows.len()) >= MAX_ROWS_PER_SECOND {
                return Err(format!(
                    "second {s} has more than {MAX_ROWS_PER_SECOND} rows (limit)"
                ));
            }
            rows.push(row);
            Ok(None)
        }
        Some((cs, _)) if s < *cs => Err("rows file is not grouped by ascending second".to_string()),
        _ => Ok(g.cur.replace((s, vec![row]))),
    }
}

/// The last group.
pub fn grouper_finish(g: Grouper) -> Option<Group> {
    g.cur
}

/// Groups rows in file order (the in-memory form of a rows file).
pub fn group_rows(rows: &[CanonRow]) -> Result<Vec<Group>, String> {
    let mut g = Grouper::default();
    let mut out = Vec::new();
    let mut last = None;
    for r in rows {
        if let Some(done) = grouper_push(&mut g, *r)? {
            last = Some(done.0);
            out.push(done);
        }
    }
    if let Some(done) = grouper_finish(g) {
        if last.is_some_and(|l| l >= done.0) {
            return Err("rows file is not grouped by ascending second".to_string());
        }
        out.push(done);
    }
    Ok(out)
}

// ---------------------------------------------------------------- comparing rows files

/// Everything the comparison found.
#[derive(Debug, Clone, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct Comparison {
    pub digest: DigestDiff,
    /// Present when both rows files were given.
    pub rows: Option<RowsResult>,
}

/// The row-level result: per differing second, and in total.
#[derive(Debug, Clone, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct RowsResult {
    pub per_second: Vec<(i64, SecondDiff)>,
    pub totals: Totals,
    /// Rows of each rows file whose second hash and count were recomputed and found equal to
    /// the digest line.
    pub verified_rows_a: u64,
    pub verified_rows_b: u64,
}

/// What one rows file has been checked against.
// A7.2: cold struct, size guard waived
struct Verify<'a> {
    side: &'static str,
    lines: &'a [DigestLine],
    next: usize,
    rows: u64,
}

/// Checks one group against the next digest line: the same second, the same row count, and the
/// hash of the rows equal to the line's. Any digest second skipped is named as missing.
fn verify_group(v: &mut Verify<'_>, g: &Group) -> Result<(), String> {
    let (s, rows) = (g.0, &g.1);
    let side = v.side;
    let line = match v.lines.get(v.next) {
        Some(l) if l.second == s => l,
        Some(l) if l.second < s => {
            return Err(format!(
                "second {}: listed in digest {side} but missing from rows file {side}",
                l.second
            ));
        }
        _ => {
            return Err(format!(
                "second {s}: rows file {side} has a second its digest does not list"
            ));
        }
    };
    if count(rows.len()) != line.rows {
        return Err(format!(
            "second {s}: rows file {side} has {} rows, its digest says {} (row counts)",
            rows.len(),
            line.rows
        ));
    }
    if hash_rows(rows) != Some(line.hash) {
        return Err(format!(
            "second {s}: rows file {side} does not hash to its digest line (the rows are not those of the digest)"
        ));
    }
    v.next += 1;
    v.rows = v.rows.saturating_add(line.rows);
    Ok(())
}

fn verify_end(v: &Verify<'_>) -> Result<(), String> {
    match v.lines.get(v.next) {
        Some(l) => Err(format!(
            "second {}: listed in digest {} but missing from rows file {}",
            l.second, v.side, v.side
        )),
        None => Ok(()),
    }
}

fn pull<I: Iterator<Item = Result<Group, String>>>(it: &mut I) -> Result<Option<Group>, String> {
    it.next().transpose()
}

/// The rows of `second` from one side (verified), advancing it; empty if the side has none.
fn step<I: Iterator<Item = Result<Group, String>>>(
    slot: &mut Option<Group>,
    second: i64,
    v: &mut Verify<'_>,
    it: &mut I,
) -> Result<Vec<CanonRow>, String> {
    match slot {
        Some(g) if g.0 == second => {
            verify_group(v, g)?;
            let rows = slot.take().map(|g| g.1).unwrap_or_default();
            *slot = pull(it)?;
            Ok(rows)
        }
        _ => Ok(Vec::new()),
    }
}

/// Walks both rows files, given as streams of groups in ascending second. Every group of each is
/// verified against its digest line (count and recomputed hash) and every digest second must be
/// present; the first bad second is named. Seconds listed as differing in `digest` are diffed.
pub fn compare_rows<A, B>(
    digest: &DigestDiff,
    la: &[DigestLine],
    lb: &[DigestLine],
    mut a: A,
    mut b: B,
) -> Result<RowsResult, String>
where
    A: Iterator<Item = Result<Group, String>>,
    B: Iterator<Item = Result<Group, String>>,
{
    let mut va = Verify {
        side: "A",
        lines: la,
        next: 0,
        rows: 0,
    };
    let mut vb = Verify {
        side: "B",
        lines: lb,
        next: 0,
        rows: 0,
    };
    let (mut ga, mut gb) = (pull(&mut a)?, pull(&mut b)?);
    let mut per_second = Vec::new();
    let mut totals = Totals::default();
    loop {
        let s = match (&ga, &gb) {
            (None, None) => break,
            (Some(x), None) => x.0,
            (None, Some(y)) => y.0,
            (Some(x), Some(y)) => x.0.min(y.0),
        };
        let ra = step(&mut ga, s, &mut va, &mut a)?;
        let rb = step(&mut gb, s, &mut vb, &mut b)?;
        if digest.differing.binary_search(&s).is_ok() {
            let d = diff_second(&ra, &rb)?;
            if second_diff_is_empty(&d) {
                return Err(format!(
                    "second {s}: the digests differ but the rows do not (inconsistent inputs)"
                ));
            }
            totals_add(&mut totals, &d);
            per_second.push((s, d));
        }
    }
    verify_end(&va)?;
    verify_end(&vb)?;
    Ok(RowsResult {
        per_second,
        totals,
        verified_rows_a: va.rows,
        verified_rows_b: vb.rows,
    })
}

// ---------------------------------------------------------------- report

/// `YYYY-MM-DDTHH:MM:SSZ` of a Unix second.
pub fn iso(second: i64) -> String {
    let days = second.div_euclid(86_400);
    let rem = second.rem_euclid(86_400);
    // Civil-from-days (proleptic Gregorian); `days` is at most about 1.1e14, far from overflow.
    let z = days.saturating_add(719_468);
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

fn fmt_max(x: f64) -> String {
    format!("{x}")
}

fn field_lines(
    o: &mut String,
    rows: &[u64; NF],
    nonfinite: &[u64; NF],
    max: &[f64; NF],
    indent: &str,
) {
    for (((fl, n), nf), m) in FIELDS.iter().zip(rows).zip(nonfinite).zip(max) {
        if *n == 0 {
            continue;
        }
        let _ = writeln!(
            o,
            "{indent}{:<15} [{}] rows {n}, max abs difference {}{}",
            fl.name,
            class_name(fl.class),
            fmt_max(*m),
            match nf {
                0 => String::new(),
                k => format!(", {k} with a non-finite value"),
            }
        );
    }
}

fn set_text(s: &BTreeSet<u32>) -> String {
    format!("{}", s.len())
}

/// The text report. At most `max_detail` differing seconds are listed in full.
pub fn render_report(c: &Comparison, max_detail: usize) -> String {
    let d = &c.digest;
    let mut o = String::new();
    let _ = writeln!(
        o,
        "result: {}",
        if d.identical {
            "identical"
        } else {
            "different"
        }
    );
    let _ = writeln!(
        o,
        "run A: {} seconds, {} rows; run B: {} seconds, {} rows",
        d.seconds_a, d.rows_a, d.seconds_b, d.rows_b
    );
    if d.identical {
        if let Some(r) = &c.rows {
            let _ = writeln!(
                o,
                "rows files verified against their digests: {} rows of A, {} rows of B (hash and count of every second recomputed)",
                r.verified_rows_a, r.verified_rows_b
            );
        }
        return o;
    }
    let _ = writeln!(
        o,
        "differing seconds: {} (only in A: {}, only in B: {})",
        d.differing.len(),
        d.only_a,
        d.only_b
    );
    if d.chain_only {
        let _ = writeln!(
            o,
            "the chain hashes differ although every second's rows are equal and present in both"
        );
    }
    if let Some(s) = first_difference(d) {
        let _ = writeln!(o, "first difference: second {s} ({})", iso(s));
    }
    match &c.rows {
        None => {
            let _ = writeln!(
                o,
                "rows files not given: the kind of difference is not known"
            );
        }
        Some(r) => {
            let _ = writeln!(
                o,
                "rows files verified against their digests: {} rows of A, {} rows of B (hash and count of every second recomputed)",
                r.verified_rows_a, r.verified_rows_b
            );
            for (s, sd) in r.per_second.iter().take(max_detail) {
                let _ = writeln!(
                    o,
                    "second {s} ({}): rows added {}, removed {}, changed {}{}{}",
                    iso(*s),
                    sd.rows_added,
                    sd.rows_removed,
                    sd.rows_changed,
                    if sd.reordered {
                        ", emission order differs"
                    } else {
                        ""
                    },
                    if sd.ambiguous_pairs > 0 {
                        format!(", {} ambiguous pairings", sd.ambiguous_pairs)
                    } else {
                        String::new()
                    }
                );
                let _ = writeln!(
                    o,
                    "  tracks added {}, removed {}, changed {}",
                    set_text(&sd.tracks_added),
                    set_text(&sd.tracks_removed),
                    set_text(&sd.tracks_changed)
                );
                field_lines(
                    &mut o,
                    &sd.field_rows,
                    &sd.field_nonfinite,
                    &sd.field_max,
                    "  ",
                );
            }
            if r.per_second.len() > max_detail {
                let _ = writeln!(
                    o,
                    "... {} more differing seconds not listed",
                    r.per_second.len() - max_detail
                );
            }
            let t = &r.totals;
            let _ = writeln!(o, "totals over {} differing seconds:", t.seconds);
            let _ = writeln!(
                o,
                "  rows added {}, removed {}, changed {}; seconds reordered {}; ambiguous pairings {}",
                t.rows_added, t.rows_removed, t.rows_changed, t.reordered_seconds, t.ambiguous_pairs
            );
            let _ = writeln!(
                o,
                "  tracks added {}, removed {}, changed {}",
                set_text(&t.tracks_added),
                set_text(&t.tracks_removed),
                set_text(&t.tracks_changed)
            );
            for (cl, n) in CLASSES.iter().zip(t.class_rows) {
                let _ = writeln!(o, "  class {:<10} rows {n}", class_name(*cl));
            }
            let _ = writeln!(o, "  flag bits that differ: 0x{:02x}", t.flags_xor);
            field_lines(
                &mut o,
                &t.field_rows,
                &t.field_nonfinite,
                &t.field_max,
                "  ",
            );
        }
    }
    o
}

/// The machine-readable summary: `key=value` lines. `expect` is the declared classes, if a file
/// was given.
pub fn render_summary(c: &Comparison, expect: Option<&BTreeSet<String>>) -> String {
    let d = &c.digest;
    let mut o = String::new();
    let mut kv = |k: &str, v: String| {
        let _ = writeln!(o, "{k}={v}");
    };
    kv("identical", d.identical.to_string());
    kv("seconds_a", d.seconds_a.to_string());
    kv("seconds_b", d.seconds_b.to_string());
    kv("rows_a", d.rows_a.to_string());
    kv("rows_b", d.rows_b.to_string());
    kv("differing_seconds", d.differing.len().to_string());
    kv("seconds_only_a", d.only_a.to_string());
    kv("seconds_only_b", d.only_b.to_string());
    kv("chain_only", d.chain_only.to_string());
    kv(
        "first_difference_second",
        first_difference(d).map_or("none".to_string(), |s| s.to_string()),
    );
    kv(
        "first_difference_utc",
        first_difference(d).map_or("none".to_string(), iso),
    );
    kv("rows_known", c.rows.is_some().to_string());
    if let Some(r) = &c.rows {
        let t = &r.totals;
        kv("rows_verified_a", r.verified_rows_a.to_string());
        kv("rows_verified_b", r.verified_rows_b.to_string());
        kv("rows_added", t.rows_added.to_string());
        kv("rows_removed", t.rows_removed.to_string());
        kv("rows_changed", t.rows_changed.to_string());
        kv("seconds_reordered", t.reordered_seconds.to_string());
        kv("ambiguous_pairs", t.ambiguous_pairs.to_string());
        kv("tracks_added", t.tracks_added.len().to_string());
        kv("tracks_removed", t.tracks_removed.len().to_string());
        kv("tracks_changed", t.tracks_changed.len().to_string());
        for (cl, n) in CLASSES.iter().zip(t.class_rows) {
            kv(&format!("class_{}_rows", class_name(*cl)), n.to_string());
        }
        kv("flags_xor", format!("0x{:02x}", t.flags_xor));
        for (fl, m) in FIELDS.iter().zip(t.field_max) {
            kv(&format!("max_abs_{}", fl.name), fmt_max(m));
        }
        kv(
            "classes_present",
            totals_present(t).into_iter().collect::<Vec<_>>().join(","),
        );
        if let Some(allowed) = expect {
            let bad = undeclared(t, allowed);
            kv(
                "expect",
                if bad.is_empty() { "met" } else { "violated" }.to_string(),
            );
            kv("undeclared", bad.join(","));
        }
    }
    o
}

/// The exit code: 0 identical (or, with a declared-difference list, every difference declared),
/// 1 different, 2 when the rows do not account for the digests' differing seconds. A difference no row explains (the chain alone) is never declared away.
pub fn verdict(c: &Comparison, expect: Option<&BTreeSet<String>>) -> u8 {
    if c.digest.identical {
        return 0;
    }
    if c.digest.chain_only {
        return 1;
    }
    // Every differing second must have been diffed and have a difference: anything else is an
    // inconsistency, not a comparison.
    if let Some(r) = &c.rows {
        if r.per_second.len() != c.digest.differing.len()
            || r.per_second.iter().any(|(_, d)| second_diff_is_empty(d))
        {
            return 2;
        }
    }
    match (expect, &c.rows) {
        (Some(allowed), Some(r)) if undeclared(&r.totals, allowed).is_empty() => 0,
        _ => 1,
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use oa_track::canon::{canon_row, digest_end_line, second_line, Digester};
    use oa_track::OutRow;

    const S: i64 = 1_000_000_000;

    fn r(t_ns: i64, addr: u32) -> OutRow {
        OutRow {
            t_ns,
            addr,
            lat_e7: 100,
            lon_e7: 200,
            vel_east_mps: 1.0,
            vel_north_mps: 2.0,
            pos_cov_m2: [1.0, 0.0, 1.0],
            vel_cov_m2s2: [1.0, 0.0, 1.0],
            turn_rate_rps: 0.0,
            alt_ft: 30_000.0,
            alt_var_ft2: 100.0,
            vrate_fpm: 0.0,
            mu_h: [0.9, 0.1],
            mu_v: [0.8, 0.2],
            flags: O_TICK,
        }
    }

    fn rows(v: &[OutRow]) -> Vec<CanonRow> {
        v.iter().map(canon_row).collect()
    }

    fn digest_text(v: &[OutRow]) -> String {
        let mut d = Digester::new();
        for x in v {
            d.push(&canon_row(x)).unwrap();
        }
        let mut s = format!("{DIGEST_HEADER}\n");
        let entries = d.finish();
        for e in &entries {
            s.push_str(&second_line(e));
            s.push('\n');
        }
        s.push_str(&digest_end_line(&entries));
        s.push('\n');
        s
    }

    /// Groups of a rows file holding `v` in `v`'s order, as the shell would stream them.
    fn groups(v: &[CanonRow]) -> std::vec::IntoIter<Result<Group, String>> {
        group_rows(v)
            .unwrap()
            .into_iter()
            .map(Ok)
            .collect::<Vec<_>>()
            .into_iter()
    }

    fn comparison_of(a: &[OutRow], b: &[OutRow]) -> Result<Comparison, String> {
        let (la, lb) = (
            parse_digest(&digest_text(a)).unwrap(),
            parse_digest(&digest_text(b)).unwrap(),
        );
        let digest = compare_digests(&la, &lb);
        let res = compare_rows(&digest, &la, &lb, groups(&rows(a)), groups(&rows(b)))?;
        Ok(Comparison {
            digest,
            rows: Some(res),
        })
    }

    fn comparison(a: &[OutRow], b: &[OutRow]) -> Comparison {
        comparison_of(a, b).unwrap()
    }

    fn totals(c: &Comparison) -> &Totals {
        &c.rows.as_ref().unwrap().totals
    }

    fn field_index(name: &str) -> usize {
        FIELDS.iter().position(|f| f.name == name).unwrap()
    }

    // REQ: OA-RDF-001
    #[test]
    fn identical_digests_are_identical_and_the_first_difference_is_found() {
        let base = vec![r(10 * S, 1), r(11 * S, 1), r(12 * S, 1), r(13 * S, 1)];
        let la = parse_digest(&digest_text(&base)).unwrap();
        let same = compare_digests(&la, &la);
        assert!(same.identical && same.differing.is_empty() && !same.chain_only);
        assert_eq!(first_difference(&same), None);
        let mut other = base.clone();
        other[2].lat_e7 += 1;
        other[3].vel_east_mps = 9.0;
        let lb = parse_digest(&digest_text(&other)).unwrap();
        let d = compare_digests(&la, &lb);
        assert!(!d.identical);
        assert_eq!(d.differing, vec![12, 13]);
        assert_eq!(first_difference(&d), Some(12));
        assert_eq!((d.seconds_a, d.rows_a, d.rows_b), (4, 4, 4));
        // A second in one run only.
        let lc = parse_digest(&digest_text(&base[..3])).unwrap();
        let e = compare_digests(&la, &lc);
        assert_eq!((e.differing.clone(), e.only_a, e.only_b), (vec![13], 1, 0));
    }

    // REQ: OA-RDF-002
    #[test]
    fn a_malformed_digest_file_is_an_error_naming_the_line() {
        let h = "a".repeat(64);
        let hdr = format!("{DIGEST_HEADER}\n");
        // A first line that is right, with its true chain, so the error is in the second line.
        let c5 = chain_next(&[0; 32], 5, &[0xaa; 32], 3);
        let first = format!("5 {} {h} 3\n", oa_track::canon::hex32(&c5));
        for (bad, at) in [
            (format!("{hdr}5 {h} {h}\n"), "line 2"),
            (format!("{hdr}x {h} {h} 3\n"), "line 2"),
            (format!("{hdr}5 {h} {h} -1\n"), "line 2"),
            (format!("{hdr}5 {h} {} 3\n", "A".repeat(64)), "line 2"),
            (format!("{hdr}{first}5 {h} {h} 3\n"), "line 3"),
            (format!("{hdr}{first}4 {h} {h} 3\n"), "line 3"),
        ] {
            let e = parse_digest(&bad).unwrap_err();
            assert!(e.contains(at), "{e}");
        }
        // A header and an end line with nothing between is an empty run; the header alone is a
        // truncated file; no header at all is not a digest.
        let empty = format!("{hdr}{}\n", digest_end_line(&[]));
        assert!(parse_digest(&empty).unwrap().is_empty());
        assert!(parse_digest(&hdr).unwrap_err().contains("end line"));
        assert!(parse_digest("").unwrap_err().contains("header"));
    }

    // REQ: OA-RDF-007
    #[test]
    fn a_digest_without_its_v2_header_or_with_a_wrong_chain_is_refused() {
        let good = digest_text(&[r(10 * S, 1), r(11 * S, 1), r(12 * S, 1)]);
        assert_eq!(parse_digest(&good).unwrap().len(), 3);
        // Another version header, a comment instead of the header, the header missing.
        let v2 = good.replacen("digest v2", "digest v1", 1);
        assert!(parse_digest(&v2).unwrap_err().contains("line 1"));
        let comment = good.replacen(DIGEST_HEADER, "# a comment", 1);
        assert!(parse_digest(&comment).unwrap_err().contains("line 1"));
        // A v1 digest (no end line, no legacy path) is refused at its header.
        assert!(parse_digest(&v2).unwrap_err().contains("another version"));
        // A blank line before the header is refused; after it, blank lines are skipped.
        let e = parse_digest(&format!("\n{good}")).unwrap_err();
        assert!(e.contains("line 1") && e.contains("blank line"), "{e}");
        let no_header: String = good.lines().skip(1).map(|l| format!("{l}\n")).collect();
        assert!(parse_digest(&no_header).is_err());
        // Edit one chain hash (line 3 is the second of three): that line is named.
        let lines: Vec<&str> = good.lines().collect();
        let parts: Vec<&str> = lines[2].split(' ').collect();
        let flipped = if parts[1].starts_with('0') { "1" } else { "0" };
        let edited = format!("{}{}", flipped, &parts[1][1..]);
        let bad_line = lines[2].replacen(parts[1], &edited, 1);
        let bad = format!("{}\n{}\n{}\n{}\n", lines[0], lines[1], bad_line, lines[3]);
        let e = parse_digest(&bad).unwrap_err();
        assert!(e.contains("line 3") && e.contains("chain"), "{e}");
        // Dropping a line breaks the chain of the next.
        let dropped = format!("{}\n{}\n{}\n", lines[0], lines[1], lines[3]);
        assert!(parse_digest(&dropped).unwrap_err().contains("line 3"));
    }

    // REQ: OA-RDF-017
    #[test]
    fn a_digest_cut_at_a_line_boundary_or_with_a_wrong_end_line_is_refused() {
        let good = digest_text(&[r(10 * S, 1), r(11 * S, 1), r(12 * S, 1)]);
        assert_eq!(parse_digest(&good).unwrap().len(), 3);
        let lines: Vec<&str> = good.lines().collect();
        // Cut after any data line: the end line is gone.
        for keep in 1..lines.len() - 1 {
            let cut: String = lines[..keep].iter().map(|l| format!("{l}\n")).collect();
            assert!(
                parse_digest(&cut).unwrap_err().contains("end line"),
                "{keep}"
            );
        }
        // Cut in the middle but with the original end line pasted back: the counts disagree.
        let mut forged: Vec<&str> = lines[..3].to_vec();
        forged.push(lines[lines.len() - 1]);
        let forged: String = forged.iter().map(|l| format!("{l}\n")).collect();
        let e = parse_digest(&forged).unwrap_err();
        assert!(
            e.contains("line 4") && e.contains("end line does not match"),
            "{e}"
        );
        // Text after the end line.
        let extra = format!("{good}{}\n", lines[1]);
        assert!(parse_digest(&extra)
            .unwrap_err()
            .contains("after the end line"));
    }

    // REQ: OA-RDF-017
    #[test]
    fn the_end_line_is_compared_field_by_field_on_single_spaces() {
        let good = digest_text(&[r(10 * S, 1), r(11 * S, 1)]);
        assert_eq!(parse_digest(&good).unwrap().len(), 2);
        let end = good.lines().last().unwrap();
        for bad in [
            end.replacen(' ', "  ", 1),
            end.replacen(' ', "\t", 1),
            format!("{end} "),
        ] {
            let text = good.replacen(end, &bad, 1);
            let e = parse_digest(&text).unwrap_err();
            assert!(e.contains("end line") && e.contains("single spaces"), "{e}");
        }
    }

    // REQ: OA-RDF-018
    #[test]
    fn two_empty_digests_are_nothing_to_compare() {
        let none: Vec<DigestLine> = Vec::new();
        let some = parse_digest(&digest_text(&[r(10 * S, 1)])).unwrap();
        assert!(nothing_to_compare(&none, &none)
            .unwrap_err()
            .contains("no seconds"));
        assert!(nothing_to_compare(&none, &some).is_ok());
        assert!(nothing_to_compare(&some, &some).is_ok());
    }

    // REQ: OA-RDF-019
    #[test]
    fn rows_files_are_verified_even_when_the_digests_are_identical() {
        let a = vec![r(10 * S, 1), r(11 * S, 1)];
        let la = parse_digest(&digest_text(&a)).unwrap();
        let dd = compare_digests(&la, &la);
        assert!(dd.identical);
        let ok = compare_rows(&dd, &la, &la, groups(&rows(&a)), groups(&rows(&a))).unwrap();
        assert_eq!((ok.verified_rows_a, ok.verified_rows_b), (2, 2));
        let c = Comparison {
            digest: dd.clone(),
            rows: Some(ok),
        };
        assert!(render_summary(&c, None).contains("rows_known=true\n"));
        assert!(render_report(&c, 5).contains("verified against their digests"));
        // A rows file of other bytes is refused although the digests are equal.
        let mut other = a.clone();
        other[1].lat_e7 += 1;
        let e = compare_rows(&dd, &la, &la, groups(&rows(&a)), groups(&rows(&other))).unwrap_err();
        assert!(e.contains("second 11") && e.contains("rows file B"), "{e}");
    }

    // REQ: OA-RDF-020
    #[test]
    fn a_differing_second_without_a_difference_is_inconsistent_not_a_pass() {
        // Digests that say second 10 differs, rows files that are equal there.
        let a = vec![r(10 * S, 1)];
        let mut b = a.clone();
        b[0].lat_e7 += 1;
        let (la, lb) = (
            parse_digest(&digest_text(&a)).unwrap(),
            parse_digest(&digest_text(&b)).unwrap(),
        );
        let dd = compare_digests(&la, &lb);
        // Rows of A supplied for B: B's rows then do not match its digest line at all.
        assert!(compare_rows(&dd, &la, &lb, groups(&rows(&a)), groups(&rows(&a))).is_err());
        // A hand-built result with an empty diff for a differing second gives exit 2.
        let c = Comparison {
            digest: dd.clone(),
            rows: Some(RowsResult {
                per_second: vec![(10, SecondDiff::default())],
                totals: Totals::default(),
                verified_rows_a: 1,
                verified_rows_b: 1,
            }),
        };
        assert_eq!(verdict(&c, None), 2);
        let missing = Comparison {
            digest: dd,
            rows: Some(RowsResult {
                per_second: Vec::new(),
                totals: Totals::default(),
                verified_rows_a: 1,
                verified_rows_b: 1,
            }),
        };
        assert_eq!(
            verdict(&missing, Some(&parse_expect("position\n").unwrap())),
            2
        );
    }

    // REQ: OA-RDF-008
    #[test]
    fn hostile_row_counts_and_sizes_are_errors_not_panics() {
        // Two lines of u64::MAX rows with their true chain: past the per-second limit.
        let (h, mut prev) = ([7u8; 32], [0u8; 32]);
        let mut text = format!("{DIGEST_HEADER}\n");
        for s in [1i64, 2] {
            prev = chain_next(&prev, s, &h, u64::MAX);
            text.push_str(&format!(
                "{s} {} {} {}\n",
                oa_track::canon::hex32(&prev),
                oa_track::canon::hex32(&h),
                u64::MAX
            ));
        }
        let e = parse_digest(&text).unwrap_err();
        assert!(
            e.contains("line 2") && e.contains("rows in a second"),
            "{e}"
        );
        // Zero rows in a second, and a second near the i64 extremes, are errors or handled.
        let mut z = format!("{DIGEST_HEADER}\n");
        let c = chain_next(&[0; 32], i64::MIN, &h, 0);
        z.push_str(&format!(
            "{} {} {} 0\n",
            i64::MIN,
            oa_track::canon::hex32(&c),
            oa_track::canon::hex32(&h)
        ));
        assert!(parse_digest(&z).unwrap_err().contains("line 2"));
        assert!(!iso(i64::MIN).is_empty() && !iso(i64::MAX).is_empty());
        // Sums over lines saturate and never panic.
        let big = |s: i64| DigestLine {
            second: s,
            chain: [0; 32],
            hash: [0; 32],
            rows: u64::MAX,
        };
        let d = compare_digests(&[big(1), big(2)], &[big(1)]);
        assert_eq!((d.rows_a, d.rows_b), (u64::MAX, u64::MAX));
        // A digest past the byte limit is refused before it is read line by line.
        let huge = "x".repeat(usize::try_from(MAX_DIGEST_BYTES).unwrap() + 1);
        assert!(parse_digest(&huge).unwrap_err().contains("bytes"));
        // Totals add with saturation.
        let mut t = Totals {
            rows_added: u64::MAX,
            ..Totals::default()
        };
        let sd = SecondDiff {
            rows_added: 5,
            ..SecondDiff::default()
        };
        totals_add(&mut t, &sd);
        assert_eq!(t.rows_added, u64::MAX);
    }

    // REQ: OA-RDF-003
    #[test]
    fn rows_added_removed_and_changed_are_classed_with_the_largest_difference() {
        let a = vec![r(10 * S, 1), r(10 * S, 2), r(10 * S, 3)];
        let mut b = a.clone();
        b[0].lat_e7 += 5; // position
        b[0].lon_e7 -= 9;
        b[1].pos_cov_m2[0] = 4.0; // covariance
        b[1].flags |= 1; // flags
        b.remove(2); // addr 3 removed
        b.push(r(10 * S, 9)); // addr 9 added
        let d = diff_second(&rows(&a), &rows(&b)).unwrap();
        assert_eq!((d.rows_added, d.rows_removed, d.rows_changed), (1, 1, 2));
        assert_eq!(d.tracks_added.iter().copied().collect::<Vec<_>>(), vec![9]);
        assert_eq!(
            d.tracks_removed.iter().copied().collect::<Vec<_>>(),
            vec![3]
        );
        assert_eq!(d.tracks_changed.len(), 2);
        // position, velocity, covariance, flags, model
        assert_eq!(d.class_rows, [1, 0, 1, 1, 0]);
        assert_eq!(d.field_max[field_index("lon_e7")], 9.0);
        assert_eq!(d.field_max[field_index("lat_e7")], 5.0);
        assert_eq!(d.field_max[field_index("pos_cov_ee")], 3.0);
        assert_eq!(d.flags_xor, 1);
        assert!(!d.reordered && !second_diff_is_empty(&d) && d.ambiguous_pairs == 0);
        // Totals and the end-to-end walk agree with the second.
        let c = comparison(&a, &b);
        let t = totals(&c);
        assert_eq!((t.seconds, t.rows_changed, t.rows_added), (1, 2, 1));
        assert_eq!(
            totals_present(t).into_iter().collect::<Vec<_>>(),
            vec!["added", "covariance", "flags", "position", "removed"]
        );
        assert_eq!(first_difference(&c.digest), Some(10));
        let text = render_report(&c, 10);
        assert!(text.contains("first difference: second 10 (1970-01-01T00:00:10Z)"));
        assert!(text.contains("lat_e7") && text.contains("verified against their digests"));
        let sum = render_summary(&c, None);
        assert!(sum.contains("identical=false\n"));
        assert!(sum.contains("class_position_rows=1\n"));
        assert!(sum.contains("max_abs_lon_e7=9\n"));
        assert!(sum.contains("first_difference_second=10\n"));
        assert!(sum.contains("rows_verified_a=3\n") && sum.contains("ambiguous_pairs=0\n"));
    }

    // REQ: OA-RDF-003
    #[test]
    fn a_reorder_with_no_other_change_and_a_second_with_no_rows_are_told_apart() {
        let a = vec![r(10 * S, 1), r(10 * S, 2)];
        let b = vec![a[1], a[0]];
        let d = diff_second(&rows(&a), &rows(&b)).unwrap();
        assert!(d.reordered && !second_diff_is_empty(&d));
        assert_eq!((d.rows_added, d.rows_removed, d.rows_changed), (0, 0, 0));
        let c = comparison(&a, &b);
        assert_eq!(
            totals_present(totals(&c)).into_iter().collect::<Vec<_>>(),
            vec!["order"]
        );
        // A second only run A has: all its rows are removed.
        let only = vec![r(10 * S, 1), r(11 * S, 5)];
        let c = comparison(&only, &only[..1]);
        let t = totals(&c);
        assert_eq!((t.rows_removed, t.rows_added), (1, 0));
        // An update row and a tick row of one track at one time pair one to one.
        let mut upd = r(10 * S, 1);
        upd.flags = 0;
        let x = vec![upd, r(10 * S, 1)];
        assert!(second_diff_is_empty(
            &diff_second(&rows(&x), &rows(&x)).unwrap()
        ));
        let mut y = x.clone();
        y[0].vel_north_mps = 7.0;
        let d = diff_second(&rows(&x), &rows(&y)).unwrap();
        assert_eq!((d.rows_changed, d.rows_added, d.rows_removed), (1, 0, 0));
        assert_eq!(d.class_rows, [0, 1, 0, 0, 0]);
    }

    // REQ: OA-RDF-009
    #[test]
    fn duplicate_keys_are_counted_and_a_tick_flip_is_a_flags_change() {
        // Two rows with the same address, time and tick flag: an ambiguous pairing, counted.
        let dup = vec![r(10 * S, 1), r(10 * S, 1)];
        let mut moved = dup.clone();
        moved[1].lat_e7 += 3;
        let d = diff_second(&rows(&dup), &rows(&moved)).unwrap();
        assert_eq!(d.ambiguous_pairs, 1);
        assert_eq!((d.rows_changed, d.rows_added, d.rows_removed), (1, 0, 0));
        let c = comparison(&dup, &moved);
        assert_eq!(totals(&c).ambiguous_pairs, 1);
        assert!(totals_present(totals(&c)).contains("ambiguous"));
        assert!(render_summary(&c, None).contains("ambiguous_pairs=1\n"));
        // An extra duplicate early in run B does not make every later pair look changed: the
        // pairing stays by key, and the extra row is one added.
        let a = vec![r(10 * S, 1), r(10 * S, 2), r(10 * S, 3)];
        let mut b = a.clone();
        b.insert(0, r(10 * S, 1));
        let d = diff_second(&rows(&a), &rows(&b)).unwrap();
        assert_eq!((d.rows_added, d.rows_removed, d.rows_changed), (1, 0, 0));
        assert_eq!(d.ambiguous_pairs, 1);
        // Tick flip: the same address and time, tick bit cleared in B: one row changed in flags.
        let a = vec![r(10 * S, 1), r(10 * S, 2)];
        let mut b = a.clone();
        b[0].flags &= !O_TICK;
        let d = diff_second(&rows(&a), &rows(&b)).unwrap();
        assert_eq!((d.rows_added, d.rows_removed, d.rows_changed), (0, 0, 1));
        assert_eq!(d.class_rows, [0, 0, 0, 1, 0]);
        assert_eq!(d.flags_xor, O_TICK);
        let c = comparison(&a, &b);
        assert_eq!(
            totals_present(totals(&c)).into_iter().collect::<Vec<_>>(),
            vec!["flags"]
        );
    }

    // REQ: OA-RDF-010
    #[test]
    fn a_reorder_next_to_a_declared_class_fails_the_expectation() {
        let a = vec![r(10 * S, 1), r(10 * S, 2), r(10 * S, 3)];
        // B changes a position and also emits the rows in another order.
        let mut b = vec![a[2], a[0], a[1]];
        b[1].lat_e7 += 10;
        let d = diff_second(&rows(&a), &rows(&b)).unwrap();
        assert!(d.reordered, "reorder is read whatever else changed");
        assert_eq!(d.rows_changed, 1);
        let c = comparison(&a, &b);
        let only_position = parse_expect("position\n").unwrap();
        assert_eq!(verdict(&c, Some(&only_position)), 1);
        let s = render_summary(&c, Some(&only_position));
        assert!(s.contains("undeclared=order\n"), "{s}");
        let both = parse_expect("position\norder\n").unwrap();
        assert_eq!(verdict(&c, Some(&both)), 0);
        // The same change in the same order passes with the position declared alone.
        let mut same_order = a.clone();
        same_order[0].lat_e7 += 10;
        assert_eq!(
            verdict(&comparison(&a, &same_order), Some(&only_position)),
            0
        );
    }

    // REQ: OA-RDF-003
    #[test]
    fn nan_changes_are_counted_and_do_not_set_a_maximum() {
        let mut a = vec![r(10 * S, 1)];
        let mut b = a.clone();
        a[0].alt_ft = f32::from_bits(0x7fc0_0000);
        b[0].alt_ft = 5.0;
        let d = diff_second(&rows(&a), &rows(&b)).unwrap();
        let i = field_index("alt_ft");
        assert_eq!(
            (d.field_rows[i], d.field_nonfinite[i], d.field_max[i]),
            (1, 1, 0.0)
        );
        assert_eq!(d.class_rows[class_index(Class::Position)], 1);
        // Infinity against a number is counted the same way.
        a[0].alt_ft = f32::INFINITY;
        assert_eq!(
            diff_second(&rows(&a), &rows(&b)).unwrap().field_nonfinite[i],
            1
        );
        // Two NaNs that differ only in payload are one canonical NaN in a row (OA-TRK-360), so
        // they are not a difference.
        a[0].alt_ft = f32::from_bits(0x7fc0_0000);
        b[0].alt_ft = f32::from_bits(0x7fc0_0001);
        assert!(second_diff_is_empty(
            &diff_second(&rows(&a), &rows(&b)).unwrap()
        ));
    }

    // REQ: OA-RDF-004
    #[test]
    fn declared_classes_pass_and_an_undeclared_class_fails() {
        let a = vec![r(10 * S, 1), r(11 * S, 1)];
        let mut flags_only = a.clone();
        flags_only[0].flags |= 8;
        let mut both = flags_only.clone();
        both[1].lat_e7 += 1;
        let allowed = parse_expect("# the radar flag\nflags\n").unwrap();
        let c1 = comparison(&a, &flags_only);
        assert_eq!(verdict(&c1, None), 1);
        assert_eq!(verdict(&c1, Some(&allowed)), 0);
        assert!(render_summary(&c1, Some(&allowed)).contains("expect=met\n"));
        let c2 = comparison(&a, &both);
        assert_eq!(verdict(&c2, Some(&allowed)), 1);
        let s = render_summary(&c2, Some(&allowed));
        assert!(s.contains("expect=violated\n") && s.contains("undeclared=position\n"));
        // Identical passes whatever is declared; no rows means a declaration cannot be checked.
        assert_eq!(verdict(&comparison(&a, &a), Some(&BTreeSet::new())), 0);
        let mut c3 = c2.clone();
        c3.rows = None;
        assert_eq!(verdict(&c3, Some(&allowed)), 1);
        // The file format: names, comments, blank lines; anything else is refused.
        assert_eq!(
            parse_expect("position # moved\n\n flags \n").unwrap().len(),
            2
        );
        assert!(parse_expect("posiiton\n").unwrap_err().contains("line 1"));
    }

    // REQ: OA-RDF-011
    #[test]
    fn a_chain_only_difference_is_a_difference_and_cannot_be_declared_away() {
        let l = parse_digest(&digest_text(&[r(10 * S, 1), r(11 * S, 1)])).unwrap();
        let mut other = l.clone();
        other[1].chain = [9; 32];
        let d = compare_digests(&l, &other);
        assert!(!d.identical && d.differing.is_empty() && d.chain_only);
        let c = Comparison {
            digest: d,
            rows: Some(RowsResult {
                per_second: Vec::new(),
                totals: Totals::default(),
                verified_rows_a: 2,
                verified_rows_b: 2,
            }),
        };
        assert_eq!(verdict(&c, None), 1);
        assert_eq!(verdict(&c, Some(&parse_expect("position\n").unwrap())), 1);
        assert!(render_report(&c, 5).contains("chain hashes differ"));
        assert!(render_summary(&c, None).contains("chain_only=true\n"));
    }

    // REQ: OA-RDF-012
    #[test]
    fn rows_that_do_not_belong_to_the_digest_are_refused_naming_the_second() {
        let a = vec![r(10 * S, 1), r(11 * S, 1), r(12 * S, 1)];
        let mut b = a.clone();
        b[1].lat_e7 += 1;
        let (la, lb) = (
            parse_digest(&digest_text(&a)).unwrap(),
            parse_digest(&digest_text(&b)).unwrap(),
        );
        let dd = compare_digests(&la, &lb);
        // Run A's rows file is from another run: same counts per second, different bytes.
        let mut foreign = a.clone();
        foreign[2].lon_e7 += 1;
        let e =
            compare_rows(&dd, &la, &lb, groups(&rows(&foreign)), groups(&rows(&b))).unwrap_err();
        assert!(e.contains("second 12") && e.contains("rows file A"), "{e}");
        // A tampered row in a second that is not a differing one is caught too.
        let mut t = b.clone();
        t[0].alt_ft = 1.0;
        let e = compare_rows(&dd, &la, &lb, groups(&rows(&a)), groups(&rows(&t))).unwrap_err();
        assert!(e.contains("second 10") && e.contains("rows file B"), "{e}");
        // A digest second missing from the rows file, and a second the digest does not list.
        let e = compare_rows(&dd, &la, &lb, groups(&rows(&a[..2])), groups(&rows(&b))).unwrap_err();
        assert!(e.contains("second 12") && e.contains("missing"), "{e}");
        let extra = [a.clone(), vec![r(13 * S, 1)]].concat();
        let e = compare_rows(&dd, &la, &lb, groups(&rows(&extra)), groups(&rows(&b))).unwrap_err();
        assert!(
            e.contains("second 13") && e.contains("does not list"),
            "{e}"
        );
        let ok = compare_rows(&dd, &la, &lb, groups(&rows(&a)), groups(&rows(&b))).unwrap();
        assert_eq!((ok.verified_rows_a, ok.verified_rows_b), (3, 3));
        assert_eq!(ok.per_second.len(), 1);
    }

    // REQ: OA-RDF-005
    #[test]
    fn groups_are_built_in_ascending_seconds_and_refuse_a_disorder() {
        let v = rows(&[r(10 * S, 1), r(10 * S, 2), r(12 * S, 1)]);
        let g = group_rows(&v).unwrap();
        assert_eq!(
            g.iter().map(|x| (x.0, x.1.len())).collect::<Vec<_>>(),
            vec![(10, 2), (12, 1)]
        );
        let unsorted = rows(&[r(12 * S, 1), r(10 * S, 1)]);
        assert!(group_rows(&unsorted).unwrap_err().contains("ascending"));
        let split = rows(&[r(10 * S, 1), r(11 * S, 1), r(10 * S, 2)]);
        assert!(group_rows(&split).unwrap_err().contains("ascending"));
    }

    // REQ: OA-RDF-005
    #[test]
    fn the_time_text_is_the_civil_date_of_the_unix_second() {
        assert_eq!(iso(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(iso(951_782_400 + 86_399), "2000-02-29T23:59:59Z");
        assert_eq!(iso(-1), "1969-12-31T23:59:59Z");
    }
}
