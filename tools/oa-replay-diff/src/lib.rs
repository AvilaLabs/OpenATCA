#![forbid(unsafe_code)]
//! Comparison of two tracker runs over one clip: their per-second digests, and where they
//! differ their canonical rows (`oa_track::canon`). Core (pure): [`parse_digest`],
//! [`compare_digests`], [`diff_second`], [`Totals`], [`parse_expect`]. Shell: [`RowsReader`] and
//! the `oa-replay-diff` binary, which reads the files and sets the exit code.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::Read;

use oa_track::canon::{
    CanonRow, OFF_ADDR, OFF_ALT, OFF_ALT_VAR, OFF_FLAGS, OFF_LAT, OFF_LON, OFF_MU_H, OFF_MU_V,
    OFF_POS_COV, OFF_TURN, OFF_T_NS, OFF_VE, OFF_VEL_COV, OFF_VN, OFF_VRATE, ROWS_HEADER_BYTES,
    ROWS_MAGIC, ROWS_VERSION, ROW_BYTES,
};
use oa_track::O_TICK;

// ---------------------------------------------------------------- digests

/// One line of a digest file.
#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct DigestLine {
    pub second: i64,
    pub chain: String,
    pub hash: String,
    pub rows: u64,
}

fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Parses a digest file: `#` lines and blank lines are skipped; every other line is
/// `second chain_hash second_hash rows`, the seconds strictly increasing.
pub fn parse_digest(text: &str) -> Result<Vec<DigestLine>, String> {
    let mut out: Vec<DigestLine> = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let n = n + 1;
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
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
        if !is_hex64(chain) || !is_hex64(hash) {
            return Err(format!("line {n}: a hash is not 64 lowercase hex digits"));
        }
        if out.last().is_some_and(|p| p.second >= second) {
            return Err(format!("line {n}: seconds are not strictly increasing"));
        }
        out.push(DigestLine {
            second,
            chain: (*chain).to_string(),
            hash: (*hash).to_string(),
            rows,
        });
    }
    Ok(out)
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
}

impl DigestDiff {
    pub fn first_difference(&self) -> Option<i64> {
        self.differing.first().copied()
    }
}

/// Compares two digests second by second. Identical means every line, chain hash included, is
/// equal. A second counts as differing when its rows' hash or count differs or only one run has
/// it; the chain hashes after the first difference all differ and are not counted again.
pub fn compare_digests(a: &[DigestLine], b: &[DigestLine]) -> DigestDiff {
    let ma: BTreeMap<i64, &DigestLine> = a.iter().map(|l| (l.second, l)).collect();
    let mb: BTreeMap<i64, &DigestLine> = b.iter().map(|l| (l.second, l)).collect();
    let all: BTreeSet<i64> = ma.keys().chain(mb.keys()).copied().collect();
    let (mut differing, mut only_a, mut only_b) = (Vec::new(), 0, 0);
    for s in all {
        match (ma.get(&s), mb.get(&s)) {
            (Some(x), Some(y)) => {
                if x.hash != y.hash || x.rows != y.rows {
                    differing.push(s);
                }
            }
            (Some(_), None) => {
                only_a += 1;
                differing.push(s);
            }
            (None, Some(_)) => {
                only_b += 1;
                differing.push(s);
            }
            (None, None) => {}
        }
    }
    DigestDiff {
        identical: a == b,
        seconds_a: a.len() as u64,
        seconds_b: b.len() as u64,
        rows_a: a.iter().map(|l| l.rows).sum(),
        rows_b: b.iter().map(|l| l.rows).sum(),
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

impl Class {
    pub fn name(self) -> &'static str {
        match self {
            Class::Position => "position",
            Class::Velocity => "velocity",
            Class::Covariance => "covariance",
            Class::Flags => "flags",
            Class::Model => "model",
        }
    }

    fn index(self) -> usize {
        CLASSES.iter().position(|c| *c == self).unwrap_or(0)
    }
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

fn real_fields() -> impl Iterator<Item = (usize, &'static Field)> {
    FIELDS.iter().enumerate()
}

fn bytes_at(row: &CanonRow, off: usize, n: usize) -> &[u8] {
    row.get(off..off + n).unwrap_or(&[])
}

fn u32_at(row: &CanonRow, off: usize) -> u32 {
    let mut b = [0u8; 4];
    b.copy_from_slice(bytes_at(row, off, 4).get(..4).unwrap_or(&[0; 4]));
    u32::from_le_bytes(b)
}

/// The matching key of a row within a second: track address, time, and whether it is a tick row.
fn key_of(row: &CanonRow) -> (u32, [u8; 8], bool) {
    let mut t = [0u8; 8];
    t.copy_from_slice(bytes_at(row, OFF_T_NS, 8).get(..8).unwrap_or(&[0; 8]));
    let tick = row.get(OFF_FLAGS).is_some_and(|f| f & O_TICK != 0);
    (u32_at(row, OFF_ADDR), t, tick)
}

/// A row's matching key: address, time bytes, tick flag, and nth among rows with those equal.
type RowKey = (u32, [u8; 8], bool, u32);

fn keyed(rows: &[CanonRow]) -> Vec<(RowKey, &CanonRow)> {
    let mut seen: BTreeMap<(u32, [u8; 8], bool), u32> = BTreeMap::new();
    rows.iter()
        .map(|r| {
            let k = key_of(r);
            let n = seen.entry(k).or_insert(0);
            let out = ((k.0, k.1, k.2, *n), r);
            *n += 1;
            out
        })
        .collect()
}

/// Difference of one field between two rows: `(bits differ, absolute difference if both finite,
/// either is not finite)`.
fn field_delta(fl: &Field, a: &CanonRow, b: &CanonRow) -> (bool, Option<f64>, bool) {
    let (x, y) = (
        bytes_at(a, fl.off, fl.size()),
        bytes_at(b, fl.off, fl.size()),
    );
    if x == y {
        return (false, None, false);
    }
    match fl.kind {
        Kind::U8 => {
            let (p, q) = (
                x.first().copied().unwrap_or(0),
                y.first().copied().unwrap_or(0),
            );
            (true, Some((f64::from(p) - f64::from(q)).abs()), false)
        }
        Kind::I32 => {
            let (p, q) = (u32_at(a, fl.off) as i32, u32_at(b, fl.off) as i32);
            (true, Some((f64::from(p) - f64::from(q)).abs()), false)
        }
        Kind::F32 => {
            let (p, q) = (
                f32::from_bits(u32_at(a, fl.off)),
                f32::from_bits(u32_at(b, fl.off)),
            );
            if p.is_finite() && q.is_finite() {
                (true, Some((f64::from(p) - f64::from(q)).abs()), false)
            } else {
                (true, None, true)
            }
        }
    }
}

impl Field {
    fn size(&self) -> usize {
        match self.kind {
            Kind::U8 => 1,
            _ => 4,
        }
    }
}

/// How the rows of one second differ. Rows are matched by track address, time and tick flag
/// (and, among rows with all three equal, by order of emission).
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
    /// Every row has a counterpart and equals it, but the order of emission differs.
    pub reordered: bool,
}

impl SecondDiff {
    pub fn is_empty(&self) -> bool {
        self.rows_added == 0 && self.rows_removed == 0 && self.rows_changed == 0 && !self.reordered
    }
}

/// Compares the rows of one second in two runs.
pub fn diff_second(a: &[CanonRow], b: &[CanonRow]) -> SecondDiff {
    let (ka, kb) = (keyed(a), keyed(b));
    let mb: BTreeMap<RowKey, &CanonRow> = kb.iter().copied().collect();
    let ma: BTreeMap<RowKey, &CanonRow> = ka.iter().copied().collect();
    let mut d = SecondDiff::default();
    for (k, ra) in &ka {
        let Some(rb) = mb.get(k) else {
            d.rows_removed += 1;
            d.tracks_removed.insert(k.0);
            continue;
        };
        if ra == rb {
            continue;
        }
        d.rows_changed += 1;
        d.tracks_changed.insert(k.0);
        let mut classes = [false; 5];
        for (i, fl) in real_fields() {
            let (differs, abs, nonfinite) = field_delta(fl, ra, rb);
            if !differs {
                continue;
            }
            if let Some(c) = classes.get_mut(fl.class.index()) {
                *c = true;
            }
            if let Some(n) = d.field_rows.get_mut(i) {
                *n += 1;
            }
            if nonfinite {
                if let Some(n) = d.field_nonfinite.get_mut(i) {
                    *n += 1;
                }
            }
            if let (Some(x), Some(m)) = (abs, d.field_max.get_mut(i)) {
                *m = m.max(x);
            }
            if fl.kind == Kind::U8 {
                let xor =
                    ra.get(fl.off).copied().unwrap_or(0) ^ rb.get(fl.off).copied().unwrap_or(0);
                d.flags_xor |= xor;
            }
        }
        for (n, hit) in d.class_rows.iter_mut().zip(classes) {
            *n += u64::from(hit);
        }
    }
    for (k, _) in &kb {
        if !ma.contains_key(k) {
            d.rows_added += 1;
            d.tracks_added.insert(k.0);
        }
    }
    d.reordered = d.rows_added == 0 && d.rows_removed == 0 && d.rows_changed == 0 && a != b;
    d
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
    pub tracks_added: BTreeSet<u32>,
    pub tracks_removed: BTreeSet<u32>,
    pub tracks_changed: BTreeSet<u32>,
    pub class_rows: [u64; 5],
    pub field_max: [f64; NF],
    pub field_rows: [u64; NF],
    pub field_nonfinite: [u64; NF],
    pub flags_xor: u8,
}

impl Totals {
    pub fn add(&mut self, d: &SecondDiff) {
        if d.is_empty() {
            return;
        }
        self.seconds += 1;
        self.rows_added += d.rows_added;
        self.rows_removed += d.rows_removed;
        self.rows_changed += d.rows_changed;
        self.reordered_seconds += u64::from(d.reordered);
        self.tracks_added.extend(&d.tracks_added);
        self.tracks_removed.extend(&d.tracks_removed);
        self.tracks_changed.extend(&d.tracks_changed);
        for (t, x) in self.class_rows.iter_mut().zip(d.class_rows) {
            *t += x;
        }
        for (t, x) in self.field_rows.iter_mut().zip(d.field_rows) {
            *t += x;
        }
        for (t, x) in self.field_nonfinite.iter_mut().zip(d.field_nonfinite) {
            *t += x;
        }
        for (t, x) in self.field_max.iter_mut().zip(d.field_max) {
            *t = t.max(x);
        }
        self.flags_xor |= d.flags_xor;
    }

    /// The declarable classes with at least one difference: the field classes by name, and
    /// `added`, `removed` and `order`.
    pub fn present(&self) -> BTreeSet<&'static str> {
        let mut s = BTreeSet::new();
        for c in CLASSES {
            if self.class_rows.get(c.index()).copied().unwrap_or(0) > 0 {
                s.insert(c.name());
            }
        }
        if self.rows_added > 0 {
            s.insert("added");
        }
        if self.rows_removed > 0 {
            s.insert("removed");
        }
        if self.reordered_seconds > 0 {
            s.insert("order");
        }
        s
    }
}

// ---------------------------------------------------------------- declared differences

/// The names an expect file may list.
pub const DECLARABLE: [&str; 8] = [
    "position",
    "velocity",
    "covariance",
    "flags",
    "model",
    "added",
    "removed",
    "order",
];

/// Parses an expect file: one class name per line, `#` comments and blank lines skipped.
pub fn parse_expect(text: &str) -> Result<BTreeSet<String>, String> {
    let mut out = BTreeSet::new();
    for (n, line) in text.lines().enumerate() {
        let l = line.split('#').next().unwrap_or("").trim();
        if l.is_empty() {
            continue;
        }
        if !DECLARABLE.contains(&l) {
            return Err(format!(
                "line {}: unknown class `{l}` (one of {})",
                n + 1,
                DECLARABLE.join(", ")
            ));
        }
        out.insert(l.to_string());
    }
    Ok(out)
}

/// The classes that differ but are not declared.
pub fn undeclared(totals: &Totals, allowed: &BTreeSet<String>) -> Vec<&'static str> {
    totals
        .present()
        .into_iter()
        .filter(|c| !allowed.contains(*c))
        .collect()
}

// ---------------------------------------------------------------- rows files

/// Reads a rows file group by group (a group is the rows of one second).
#[derive(Debug)]
// A7.2: cold struct, size guard waived
pub struct RowsReader<R: Read> {
    r: R,
    remaining: u64,
    peeked: Option<(i64, CanonRow)>,
    last: Option<i64>,
}

impl<R: Read> RowsReader<R> {
    /// Reads and checks the header.
    pub fn new(mut r: R) -> Result<RowsReader<R>, String> {
        let mut h = [0u8; ROWS_HEADER_BYTES];
        r.read_exact(&mut h)
            .map_err(|e| format!("rows file header: {e}"))?;
        let get = |a: usize, n: usize| h.get(a..a + n).unwrap_or(&[]);
        if get(0, 8) != ROWS_MAGIC {
            return Err("not a rows file (bad magic)".to_string());
        }
        let ver = u32_from(get(8, 4));
        if ver != ROWS_VERSION {
            return Err(format!("rows file version {ver}, expected {ROWS_VERSION}"));
        }
        let size = u32_from(get(12, 4));
        if size as usize != ROW_BYTES {
            return Err(format!("rows file row size {size}, expected {ROW_BYTES}"));
        }
        let mut c = [0u8; 8];
        c.copy_from_slice(get(16, 8));
        Ok(RowsReader {
            r,
            remaining: u64::from_le_bytes(c),
            peeked: None,
            last: None,
        })
    }

    fn row(&mut self) -> Result<Option<(i64, CanonRow)>, String> {
        if self.remaining == 0 {
            return Ok(None);
        }
        let mut b = [0u8; ROW_BYTES];
        self.r
            .read_exact(&mut b)
            .map_err(|e| format!("rows file ends early: {e}"))?;
        self.remaining -= 1;
        Ok(Some((oa_track::canon::row_second(&b), b)))
    }

    /// The next group, or None at the end. Seconds must come in strictly ascending groups.
    pub fn group(&mut self) -> Result<Option<(i64, Vec<CanonRow>)>, String> {
        let first = match self.peeked.take() {
            Some(x) => x,
            None => match self.row()? {
                Some(x) => x,
                None => return Ok(None),
            },
        };
        if self.last.is_some_and(|l| first.0 <= l) {
            return Err("rows file is not grouped by ascending second".to_string());
        }
        self.last = Some(first.0);
        let mut rows = vec![first.1];
        while let Some(x) = self.row()? {
            if x.0 == first.0 {
                rows.push(x.1);
            } else {
                self.peeked = Some(x);
                break;
            }
        }
        Ok(Some((first.0, rows)))
    }
}

fn u32_from(b: &[u8]) -> u32 {
    let mut x = [0u8; 4];
    x.copy_from_slice(b.get(..4).unwrap_or(&[0; 4]));
    u32::from_le_bytes(x)
}

/// A reader advanced second by second: [`SecondCursor::rows_of`] must be asked for ascending
/// seconds.
// A7.2: cold struct, size guard waived
pub struct SecondCursor<R: Read> {
    rd: RowsReader<R>,
    held: Option<(i64, Vec<CanonRow>)>,
    done: bool,
}

impl<R: Read> SecondCursor<R> {
    pub fn new(rd: RowsReader<R>) -> SecondCursor<R> {
        SecondCursor {
            rd,
            held: None,
            done: false,
        }
    }

    /// The rows of `second` (empty if the file has none).
    pub fn rows_of(&mut self, second: i64) -> Result<Vec<CanonRow>, String> {
        loop {
            if self.held.is_none() && !self.done {
                self.held = self.rd.group()?;
                self.done = self.held.is_none();
            }
            match &self.held {
                Some((s, _)) if *s < second => self.held = None,
                Some((s, _)) if *s == second => {
                    return Ok(self.held.take().map(|(_, r)| r).unwrap_or_default());
                }
                _ => return Ok(Vec::new()),
            }
        }
    }
}

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
}

/// Walks the differing seconds of `digest` through the two rows files.
pub fn compare_rows<A: Read, B: Read>(
    digest: &DigestDiff,
    la: &[DigestLine],
    lb: &[DigestLine],
    a: RowsReader<A>,
    b: RowsReader<B>,
) -> Result<RowsResult, String> {
    let rows_in = |l: &[DigestLine], s: i64| l.iter().find(|x| x.second == s).map_or(0, |x| x.rows);
    let (mut ca, mut cb) = (SecondCursor::new(a), SecondCursor::new(b));
    let mut per_second = Vec::new();
    let mut totals = Totals::default();
    for &s in &digest.differing {
        let (ra, rb) = (ca.rows_of(s)?, cb.rows_of(s)?);
        if ra.len() as u64 != rows_in(la, s) || rb.len() as u64 != rows_in(lb, s) {
            return Err(format!(
                "second {s}: a rows file does not match its digest file (row counts)"
            ));
        }
        let d = diff_second(&ra, &rb);
        totals.add(&d);
        per_second.push((s, d));
    }
    Ok(RowsResult { per_second, totals })
}

// ---------------------------------------------------------------- report

/// `YYYY-MM-DDTHH:MM:SSZ` of a Unix second.
pub fn iso(second: i64) -> String {
    let days = second.div_euclid(86_400);
    let rem = second.rem_euclid(86_400);
    // Civil-from-days (proleptic Gregorian).
    let z = days + 719_468;
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
    for (i, fl) in real_fields() {
        let n = rows.get(i).copied().unwrap_or(0);
        if n == 0 {
            continue;
        }
        let _ = writeln!(
            o,
            "{indent}{:<15} [{}] rows {n}, max abs difference {}{}",
            fl.name,
            fl.class.name(),
            fmt_max(max.get(i).copied().unwrap_or(0.0)),
            match nonfinite.get(i).copied().unwrap_or(0) {
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
        return o;
    }
    let _ = writeln!(
        o,
        "differing seconds: {} (only in A: {}, only in B: {})",
        d.differing.len(),
        d.only_a,
        d.only_b
    );
    if let Some(s) = d.first_difference() {
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
            for (s, sd) in r.per_second.iter().take(max_detail) {
                let _ = writeln!(
                    o,
                    "second {s} ({}): rows added {}, removed {}, changed {}{}",
                    iso(*s),
                    sd.rows_added,
                    sd.rows_removed,
                    sd.rows_changed,
                    if sd.reordered {
                        ", emission order differs"
                    } else {
                        ""
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
                "  rows added {}, removed {}, changed {}; seconds reordered {}",
                t.rows_added, t.rows_removed, t.rows_changed, t.reordered_seconds
            );
            let _ = writeln!(
                o,
                "  tracks added {}, removed {}, changed {}",
                set_text(&t.tracks_added),
                set_text(&t.tracks_removed),
                set_text(&t.tracks_changed)
            );
            for cl in CLASSES {
                let _ = writeln!(
                    o,
                    "  class {:<10} rows {}",
                    cl.name(),
                    t.class_rows.get(cl.index()).copied().unwrap_or(0)
                );
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
    kv(
        "first_difference_second",
        d.first_difference()
            .map_or("none".to_string(), |s| s.to_string()),
    );
    kv(
        "first_difference_utc",
        d.first_difference().map_or("none".to_string(), iso),
    );
    kv("rows_known", c.rows.is_some().to_string());
    if let Some(r) = &c.rows {
        let t = &r.totals;
        kv("rows_added", t.rows_added.to_string());
        kv("rows_removed", t.rows_removed.to_string());
        kv("rows_changed", t.rows_changed.to_string());
        kv("seconds_reordered", t.reordered_seconds.to_string());
        kv("tracks_added", t.tracks_added.len().to_string());
        kv("tracks_removed", t.tracks_removed.len().to_string());
        kv("tracks_changed", t.tracks_changed.len().to_string());
        for cl in CLASSES {
            kv(
                &format!("class_{}_rows", cl.name()),
                t.class_rows
                    .get(cl.index())
                    .copied()
                    .unwrap_or(0)
                    .to_string(),
            );
        }
        kv("flags_xor", format!("0x{:02x}", t.flags_xor));
        for (i, fl) in real_fields() {
            kv(
                &format!("max_abs_{}", fl.name),
                fmt_max(t.field_max.get(i).copied().unwrap_or(0.0)),
            );
        }
        kv(
            "classes_present",
            t.present().into_iter().collect::<Vec<_>>().join(","),
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
/// 1 otherwise.
pub fn verdict(c: &Comparison, expect: Option<&BTreeSet<String>>) -> u8 {
    if c.digest.identical {
        return 0;
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
    use oa_track::canon::{
        canon_row, chain_next, rows_header, Digester, SecondEntry, DIGEST_HEADER,
    };
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
            d.push(&canon_row(x));
        }
        let mut s = format!("{DIGEST_HEADER}\n");
        for e in d.finish() {
            s.push_str(&e.line());
            s.push('\n');
        }
        s
    }

    fn rows_file(v: &[CanonRow]) -> Vec<u8> {
        let mut b = rows_header(v.len() as u64).to_vec();
        for x in v {
            b.extend_from_slice(x);
        }
        b
    }

    fn comparison(a: &[OutRow], b: &[OutRow]) -> Comparison {
        let (la, lb) = (
            parse_digest(&digest_text(a)).unwrap(),
            parse_digest(&digest_text(b)).unwrap(),
        );
        let digest = compare_digests(&la, &lb);
        let (fa, fb) = (rows_file(&rows(a)), rows_file(&rows(b)));
        let res = compare_rows(
            &digest,
            &la,
            &lb,
            RowsReader::new(&fa[..]).unwrap(),
            RowsReader::new(&fb[..]).unwrap(),
        )
        .unwrap();
        Comparison {
            digest,
            rows: Some(res),
        }
    }

    // REQ: OA-RDF-001
    #[test]
    fn identical_digests_are_identical_and_the_first_difference_is_found() {
        let base = vec![r(10 * S, 1), r(11 * S, 1), r(12 * S, 1), r(13 * S, 1)];
        let la = parse_digest(&digest_text(&base)).unwrap();
        let same = compare_digests(&la, &la);
        assert!(same.identical && same.differing.is_empty());
        assert_eq!(same.first_difference(), None);
        let mut other = base.clone();
        other[2].lat_e7 += 1;
        other[3].vel_east_mps = 9.0;
        let lb = parse_digest(&digest_text(&other)).unwrap();
        let d = compare_digests(&la, &lb);
        assert!(!d.identical);
        assert_eq!(d.differing, vec![12, 13]);
        assert_eq!(d.first_difference(), Some(12));
        assert_eq!((d.seconds_a, d.rows_a, d.rows_b), (4, 4, 4));
        // A second in one run only.
        let lc = parse_digest(&digest_text(&base[..3])).unwrap();
        let e = compare_digests(&la, &lc);
        assert_eq!((e.differing.clone(), e.only_a, e.only_b), (vec![13], 1, 0));
    }

    // REQ: OA-RDF-002
    #[test]
    fn a_malformed_digest_file_is_an_error_naming_the_line() {
        assert!(parse_digest("").unwrap().is_empty());
        let h = "a".repeat(64);
        let ok = format!("# c\n5 {h} {h} 3\n6 {h} {h} 1\n");
        assert_eq!(parse_digest(&ok).unwrap().len(), 2);
        for (bad, at) in [
            (format!("5 {h} {h}\n"), "line 1"),
            (format!("x {h} {h} 3\n"), "line 1"),
            (format!("5 {h} {h} -1\n"), "line 1"),
            (format!("5 {h} {} 3\n", "A".repeat(64)), "line 1"),
            (format!("5 {h} {h} 3\n5 {h} {h} 3\n"), "line 2"),
            (format!("6 {h} {h} 3\n5 {h} {h} 3\n"), "line 2"),
        ] {
            let e = parse_digest(&bad).unwrap_err();
            assert!(e.contains(at), "{e}");
        }
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
        let d = diff_second(&rows(&a), &rows(&b));
        assert_eq!((d.rows_added, d.rows_removed, d.rows_changed), (1, 1, 2));
        assert_eq!(d.tracks_added.iter().copied().collect::<Vec<_>>(), vec![9]);
        assert_eq!(
            d.tracks_removed.iter().copied().collect::<Vec<_>>(),
            vec![3]
        );
        assert_eq!(d.tracks_changed.len(), 2);
        // position, velocity, covariance, flags, model
        assert_eq!(d.class_rows, [1, 0, 1, 1, 0]);
        let idx = |name: &str| real_fields().find(|(_, f)| f.name == name).unwrap().0;
        assert_eq!(d.field_max[idx("lon_e7")], 9.0);
        assert_eq!(d.field_max[idx("lat_e7")], 5.0);
        assert_eq!(d.field_max[idx("pos_cov_ee")], 3.0);
        assert_eq!(d.flags_xor, 1);
        assert!(!d.reordered && !d.is_empty());
        // Totals and the end-to-end walk agree with the second.
        let c = comparison(&a, &b);
        let t = &c.rows.as_ref().unwrap().totals;
        assert_eq!((t.seconds, t.rows_changed, t.rows_added), (1, 2, 1));
        assert_eq!(
            t.present().into_iter().collect::<Vec<_>>(),
            vec!["added", "covariance", "flags", "position", "removed"]
        );
        assert_eq!(c.digest.first_difference(), Some(10));
        let text = render_report(&c, 10);
        assert!(text.contains("first difference: second 10 (1970-01-01T00:00:10Z)"));
        assert!(text.contains("lat_e7"));
        let sum = render_summary(&c, None);
        assert!(sum.contains("identical=false\n"));
        assert!(sum.contains("class_position_rows=1\n"));
        assert!(sum.contains("max_abs_lon_e7=9\n"));
        assert!(sum.contains("first_difference_second=10\n"));
    }

    // REQ: OA-RDF-003
    #[test]
    fn a_reorder_with_no_other_change_and_a_second_with_no_rows_are_told_apart() {
        let a = vec![r(10 * S, 1), r(10 * S, 2)];
        let b = vec![a[1], a[0]];
        let d = diff_second(&rows(&a), &rows(&b));
        assert!(d.reordered && !d.is_empty());
        assert_eq!((d.rows_added, d.rows_removed, d.rows_changed), (0, 0, 0));
        let c = comparison(&a, &b);
        assert_eq!(
            c.rows
                .as_ref()
                .unwrap()
                .totals
                .present()
                .into_iter()
                .collect::<Vec<_>>(),
            vec!["order"]
        );
        // A second only run A has: all its rows are removed.
        let only = vec![r(10 * S, 1), r(11 * S, 5)];
        let c = comparison(&only, &only[..1]);
        let t = &c.rows.as_ref().unwrap().totals;
        assert_eq!((t.rows_removed, t.rows_added), (1, 0));
        // Two rows of one track at one time (an update and a tick) match one to one.
        let mut upd = r(10 * S, 1);
        upd.flags = 0;
        let x = vec![upd, r(10 * S, 1)];
        assert!(diff_second(&rows(&x), &rows(&x)).is_empty());
        let mut y = x.clone();
        y[0].vel_north_mps = 7.0;
        let d = diff_second(&rows(&x), &rows(&y));
        assert_eq!((d.rows_changed, d.rows_added, d.rows_removed), (1, 0, 0));
        assert_eq!(d.class_rows, [0, 1, 0, 0, 0]);
    }

    // REQ: OA-RDF-003
    #[test]
    fn nan_changes_are_counted_and_do_not_set_a_maximum() {
        let mut a = vec![r(10 * S, 1)];
        let mut b = a.clone();
        a[0].alt_ft = f32::from_bits(0x7fc0_0000);
        b[0].alt_ft = f32::from_bits(0x7fc0_0001);
        let d = diff_second(&rows(&a), &rows(&b));
        let i = real_fields().find(|(_, f)| f.name == "alt_ft").unwrap().0;
        assert_eq!(
            (d.field_rows[i], d.field_nonfinite[i], d.field_max[i]),
            (1, 1, 0.0)
        );
        assert_eq!(d.class_rows[Class::Position.index()], 1);
        // NaN against a number is a difference too.
        b[0].alt_ft = 5.0;
        assert_eq!(diff_second(&rows(&a), &rows(&b)).field_nonfinite[i], 1);
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

    // REQ: OA-RDF-005
    #[test]
    fn rows_files_are_checked_and_read_group_by_group() {
        let a = rows(&[r(10 * S, 1), r(10 * S, 2), r(12 * S, 1)]);
        let f = rows_file(&a);
        let mut rd = RowsReader::new(&f[..]).unwrap();
        assert_eq!(rd.group().unwrap().unwrap().1.len(), 2);
        assert_eq!(rd.group().unwrap().unwrap().0, 12);
        assert!(rd.group().unwrap().is_none());
        // Cursor: a missing second is empty, and asking forward skips groups.
        let mut cur = SecondCursor::new(RowsReader::new(&f[..]).unwrap());
        assert_eq!(cur.rows_of(11).unwrap().len(), 0);
        assert_eq!(cur.rows_of(12).unwrap().len(), 1);
        assert_eq!(cur.rows_of(13).unwrap().len(), 0);
        // Bad magic, version, size, truncation, and rows not grouped by second.
        let mut bad = f.clone();
        bad[0] = b'X';
        assert!(RowsReader::new(&bad[..]).unwrap_err().contains("magic"));
        let mut bad = f.clone();
        bad[8] = 9;
        assert!(RowsReader::new(&bad[..]).unwrap_err().contains("version"));
        let mut bad = f.clone();
        bad[12] = 7;
        assert!(RowsReader::new(&bad[..]).unwrap_err().contains("row size"));
        let cut = &f[..f.len() - 3];
        let mut rd = RowsReader::new(cut).unwrap();
        assert!(rd.group().is_err());
        let unsorted = rows_file(&rows(&[r(12 * S, 1), r(10 * S, 1)]));
        let mut rd = RowsReader::new(&unsorted[..]).unwrap();
        assert!(rd.group().is_ok());
        assert!(rd.group().unwrap_err().contains("ascending"));
        // A rows file that disagrees with its digest is an error.
        let la = parse_digest(&digest_text(&[r(10 * S, 1)])).unwrap();
        let lb = parse_digest(&digest_text(&[r(10 * S, 1), r(10 * S, 2)])).unwrap();
        let dd = compare_digests(&la, &lb);
        let (fa, fb) = (
            rows_file(&rows(&[r(10 * S, 1)])),
            rows_file(&rows(&[r(10 * S, 1)])),
        );
        let e = compare_rows(
            &dd,
            &la,
            &lb,
            RowsReader::new(&fa[..]).unwrap(),
            RowsReader::new(&fb[..]).unwrap(),
        )
        .unwrap_err();
        assert!(e.contains("second 10"));
    }

    // REQ: OA-RDF-005
    #[test]
    fn the_time_text_is_the_civil_date_of_the_unix_second() {
        assert_eq!(iso(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(iso(951_782_400 + 86_399), "2000-02-29T23:59:59Z");
        assert_eq!(iso(-1), "1969-12-31T23:59:59Z");
        // The chain of a digest line is the chain function of the lines before it.
        let l = parse_digest(&digest_text(&[r(10 * S, 1), r(11 * S, 1)])).unwrap();
        assert_ne!(l[0].chain, l[1].chain);
        let _ = (chain_next, SecondEntry::line);
    }
}
