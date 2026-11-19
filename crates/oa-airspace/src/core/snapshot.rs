//! Snapshot encoding and decoding (OA-ASP-006, RULESET A8). **Core** (B1, B7): pure functions between
//! a [`Model`] and bytes; reading and writing the file is the shell's job.
//!
//! Format, all integers little-endian, nothing implicit:
//!
//! ```text
//! magic "OATCA-ADAPT" (11 bytes), format version u32
//! nasr_effective u32, cifp_cycle u32, buffer_m u32, artcc (u32 length, bytes),
//! nasr_sha256 [32], cifp_sha256 [32]
//! (version 4 only: cycle_override, u32 length and UTF-8 bytes)
//! then the tables in fixed order, each: 4-byte tag, row count u32, its columns in the order
//! of the field declarations in `model.rs`; a bitset column is ceil(rows/64) u64 words.
//! tables: STRS PNTS VERT BNDS SEGS AWYS AWLG PROC RTES LEGS MORA UNRS DISA
//! ```
//!
//! Version 3 is written whenever the NASR and CIFP cycles agree; version 4 only when the build
//! allowed a mismatch and so carries the cycle_override text. Both are read.
//!
//! The snapshot's identity is the SHA-256 of the whole file. Because every table is written in
//! its sorted key order and nothing is derived from hash-map order, the same inputs always give
//! the same bytes.
//!
//! The reader is bounded: the file size, the artcc length, every table's row count (against the
//! bytes left, before anything is allocated) and every index and span are checked, and no input
//! makes it panic. Trailing bytes are an error.

use super::model::*;
use std::fmt;

/// File magic.
pub const MAGIC: &[u8; 11] = b"OATCA-ADAPT";
/// Format version written and accepted.
pub const FORMAT_VERSION: u32 = 3;
/// Format version of a snapshot that carries a cycle-mismatch note (the version 3 layout plus
/// the note after the header).
pub const FORMAT_VERSION_NOTE: u32 = 4;
/// Limit: largest snapshot the reader accepts, 512 MiB. A ZAU model is a few MiB; the whole
/// national data set would stay under about 100 MiB.
pub const MAX_SNAPSHOT_BYTES: usize = 512 * 1024 * 1024;
/// Limit: longest ARTCC identifier in the header, bytes.
pub const MAX_ARTCC_BYTES: usize = 64;

const TAG_STRS: &[u8; 4] = b"STRS";
const TAG_PNTS: &[u8; 4] = b"PNTS";
const TAG_VERT: &[u8; 4] = b"VERT";
const TAG_BNDS: &[u8; 4] = b"BNDS";
const TAG_SEGS: &[u8; 4] = b"SEGS";
const TAG_AWYS: &[u8; 4] = b"AWYS";
const TAG_AWLG: &[u8; 4] = b"AWLG";
const TAG_PROC: &[u8; 4] = b"PROC";
const TAG_RTES: &[u8; 4] = b"RTES";
const TAG_LEGS: &[u8; 4] = b"LEGS";
const TAG_MORA: &[u8; 4] = b"MORA";
const TAG_UNRS: &[u8; 4] = b"UNRS";
const TAG_DISA: &[u8; 4] = b"DISA";

/// Why a snapshot could not be written or read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotError {
    /// The model to write is not internally consistent.
    Invalid(ModelError),
    /// A table is too large for the format's `u32` counts.
    TooLarge(&'static str),
    /// The input is larger than [`MAX_SNAPSHOT_BYTES`].
    FileTooLarge { len: usize },
    /// The file does not start with the magic.
    BadMagic,
    /// A format version this reader does not know.
    BadVersion(u32),
    /// The file ends inside the named structure.
    Truncated(&'static str),
    /// A table tag is not the expected one.
    BadTag { expected: &'static str },
    /// A count claims more rows than the remaining bytes could hold.
    CountExceedsFile { table: &'static str, rows: u64 },
    /// The cycle note is longer than [`MAX_CYCLE_OVERRIDE_BYTES`] (or, reading, not UTF-8).
    BadNote,
    /// The artcc field is longer than [`MAX_ARTCC_BYTES`] (or, reading, not UTF-8).
    BadArtcc,
    /// The string table text is not valid UTF-8.
    BadText,
    /// Bytes remain after the last table.
    TrailingBytes { extra: usize },
    /// The decoded tables are inconsistent (an index out of range, a span out of place...).
    Model(ModelError),
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SnapshotError::Invalid(e) => write!(f, "snapshot: model is inconsistent: {e}"),
            SnapshotError::TooLarge(t) => write!(f, "snapshot: table {t} is too large to write"),
            SnapshotError::FileTooLarge { len } => {
                write!(f, "snapshot: {len} bytes exceeds limit MAX_SNAPSHOT_BYTES")
            }
            SnapshotError::BadMagic => write!(f, "snapshot: not an OATCA-ADAPT file"),
            SnapshotError::BadVersion(v) => write!(
                f,
                "snapshot: format version {v} is not supported (this reader reads versions {FORMAT_VERSION} and {FORMAT_VERSION_NOTE} only); rebuild the snapshot with oa-adapt build"
            ),
            SnapshotError::Truncated(w) => write!(f, "snapshot: truncated in {w}"),
            SnapshotError::BadTag { expected } => write!(f, "snapshot: expected table {expected}"),
            SnapshotError::CountExceedsFile { table, rows } => {
                write!(
                    f,
                    "snapshot: table {table} claims {rows} rows, more than the file holds"
                )
            }
            SnapshotError::BadArtcc => write!(f, "snapshot: bad ARTCC field"),
            SnapshotError::BadNote => write!(f, "snapshot: bad cycle note"),
            SnapshotError::BadText => write!(f, "snapshot: string table is not UTF-8"),
            SnapshotError::TrailingBytes { extra } => {
                write!(f, "snapshot: {extra} unexpected bytes after the last table")
            }
            SnapshotError::Model(e) => write!(f, "snapshot: {e}"),
        }
    }
}

impl std::error::Error for SnapshotError {}

// ---------------------------------------------------------------------------------------------
// Writing

fn count(table: &'static str, n: usize) -> Result<u32, SnapshotError> {
    u32::try_from(n).map_err(|_| SnapshotError::TooLarge(table))
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&u32::to_le_bytes(v));
}

fn put_header(out: &mut Vec<u8>, tag: &[u8; 4], rows: u32) {
    out.extend_from_slice(tag);
    put_u32(out, rows);
}

fn put_i32s(out: &mut Vec<u8>, col: &[i32]) {
    for v in col {
        out.extend_from_slice(&i32::to_le_bytes(*v));
    }
}

fn put_u32s(out: &mut Vec<u8>, col: &[u32]) {
    for v in col {
        put_u32(out, *v);
    }
}

fn put_u16s(out: &mut Vec<u8>, col: &[u16]) {
    for v in col {
        out.extend_from_slice(&u16::to_le_bytes(*v));
    }
}

fn put_i16s(out: &mut Vec<u8>, col: &[i16]) {
    for v in col {
        out.extend_from_slice(&i16::to_le_bytes(*v));
    }
}

fn put_strs(out: &mut Vec<u8>, col: &[StrIx]) {
    for v in col {
        put_u32(out, v.get());
    }
}

fn put_points(out: &mut Vec<u8>, col: &[PointIx]) {
    for v in col {
        put_u32(out, v.get());
    }
}

fn put_spans<I: Copy>(out: &mut Vec<u8>, col: &[Span<I>], raw: impl Fn(I) -> u32) {
    for s in col {
        put_u32(out, raw(s.start));
        put_u32(out, s.len);
    }
}

fn put_bits(out: &mut Vec<u8>, b: &BitSet) {
    for w in &b.words {
        out.extend_from_slice(&u64::to_le_bytes(*w));
    }
}

/// Refuses a snapshot length the reader would refuse (OA-ASP-012).
fn check_len(len: usize) -> Result<(), SnapshotError> {
    if len > MAX_SNAPSHOT_BYTES {
        return Err(SnapshotError::FileTooLarge { len });
    }
    Ok(())
}

/// Encodes a model. The model is validated first; an inconsistent model is refused (E5). The
/// writer enforces the limits the reader enforces ([`MAX_ARTCC_BYTES`], [`MAX_SNAPSHOT_BYTES`]),
/// so every file it produces can be read back (OA-ASP-012).
pub fn encode(m: &Model) -> Result<Vec<u8>, SnapshotError> {
    validate_model(m).map_err(SnapshotError::Invalid)?;
    if m.meta.artcc.len() > MAX_ARTCC_BYTES {
        return Err(SnapshotError::BadArtcc);
    }
    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(MAGIC);
    if m.cycle_override
        .as_ref()
        .is_some_and(|n| n.len() > MAX_CYCLE_OVERRIDE_BYTES)
    {
        return Err(SnapshotError::BadNote);
    }
    put_u32(
        &mut out,
        if m.cycle_override.is_some() {
            FORMAT_VERSION_NOTE
        } else {
            FORMAT_VERSION
        },
    );
    put_u32(&mut out, m.meta.nasr_effective);
    put_u32(&mut out, m.meta.cifp_cycle);
    put_u32(&mut out, m.meta.buffer_m);
    put_u32(&mut out, count("artcc", m.meta.artcc.len())?);
    out.extend_from_slice(m.meta.artcc.as_bytes());
    out.extend_from_slice(&m.meta.nasr_sha256);
    out.extend_from_slice(&m.meta.cifp_sha256);
    if let Some(note) = &m.cycle_override {
        put_u32(&mut out, count("cycle note", note.len())?);
        out.extend_from_slice(note.as_bytes());
    }

    put_header(&mut out, TAG_STRS, count("strings", str_count(&m.strings))?);
    put_u32(&mut out, count("strings", m.strings.text.len())?);
    put_u32s(&mut out, &m.strings.offsets);
    out.extend_from_slice(m.strings.text.as_bytes());

    let p = &m.points;
    put_header(&mut out, TAG_PNTS, count("points", p.kind.len())?);
    out.extend_from_slice(&p.kind);
    put_i32s(&mut out, &p.lat);
    put_i32s(&mut out, &p.lon);
    put_i32s(&mut out, &p.elev_ft);
    put_strs(&mut out, &p.ident);
    put_strs(&mut out, &p.alt_ident);
    put_strs(&mut out, &p.region);
    put_strs(&mut out, &p.scope);
    put_strs(&mut out, &p.name);
    for b in [
        &p.src_nasr,
        &p.src_cifp,
        &p.in_area,
        &p.cls_b,
        &p.cls_c,
        &p.cls_d,
        &p.cls_e,
    ] {
        put_bits(&mut out, b);
    }

    put_header(&mut out, TAG_VERT, count("vertices", m.vertices.len())?);
    for v in &m.vertices {
        put_u32(&mut out, v.lat as u32);
        put_u32(&mut out, v.lon as u32);
    }

    let b = &m.boundaries;
    put_header(&mut out, TAG_BNDS, count("boundaries", b.source.len())?);
    out.extend_from_slice(&b.source);
    out.extend_from_slice(&b.stratum);
    out.extend_from_slice(&b.edge_interp);
    out.extend_from_slice(&b.multiple);
    put_strs(&mut out, &b.facility);
    put_strs(&mut out, &b.name);
    put_i32s(&mut out, &b.floor_ft);
    put_i32s(&mut out, &b.ceiling_ft);
    put_spans(&mut out, &b.vertices, |i| i.get());
    put_spans(&mut out, &b.segs, |i| i.get());
    put_bits(&mut out, &b.floor_agl);
    put_bits(&mut out, &b.ceiling_agl);
    out.extend_from_slice(&b.time_code);
    out.extend_from_slice(&b.level);
    put_bits(&mut out, &b.incomplete);

    let g = &m.segments;
    put_header(&mut out, TAG_SEGS, count("segments", g.via.len())?);
    out.extend_from_slice(&g.via);
    put_i32s(&mut out, &g.lat);
    put_i32s(&mut out, &g.lon);
    put_i32s(&mut out, &g.arc_lat);
    put_i32s(&mut out, &g.arc_lon);
    put_u32s(&mut out, &g.radius_m);
    put_u16s(&mut out, &g.bearing_cdeg);

    let a = &m.airways;
    put_header(&mut out, TAG_AWYS, count("airways", a.designator.len())?);
    put_strs(&mut out, &a.designator);
    out.extend_from_slice(&a.location);
    put_spans(&mut out, &a.legs, |i| i.get());

    let l = &m.airway_legs;
    put_header(&mut out, TAG_AWLG, count("airway_legs", l.from.len())?);
    put_points(&mut out, &l.from);
    put_points(&mut out, &l.to);
    put_i32s(&mut out, &l.mea_ft);
    put_i32s(&mut out, &l.mea_opp_ft);
    put_i32s(&mut out, &l.gps_mea_ft);
    out.extend_from_slice(&l.mea_dir);
    out.extend_from_slice(&l.gps_mea_dir);
    for bs in [&l.gap, &l.signal_gap, &l.dogleg] {
        put_bits(&mut out, bs);
    }

    let pr = &m.procs;
    put_header(&mut out, TAG_PROC, count("procs", pr.kind.len())?);
    out.extend_from_slice(&pr.kind);
    put_points(&mut out, &pr.airport);
    put_strs(&mut out, &pr.ident);
    put_strs(&mut out, &pr.name);
    put_spans(&mut out, &pr.routes, |i| i.get());
    for bs in [&pr.src_nasr, &pr.src_cifp, &pr.coded] {
        put_bits(&mut out, bs);
    }

    let r = &m.routes;
    put_header(&mut out, TAG_RTES, count("routes", r.route_type.len())?);
    out.extend_from_slice(&r.route_type);
    put_strs(&mut out, &r.transition);
    put_spans(&mut out, &r.legs, |i| i.get());

    let lg = &m.legs;
    put_header(&mut out, TAG_LEGS, count("legs", lg.fix.len())?);
    put_points(&mut out, &lg.fix);
    put_points(&mut out, &lg.center);
    out.extend_from_slice(&lg.path_term);
    out.extend_from_slice(&lg.turn);
    put_u16s(&mut out, &lg.course_mag_cdeg);
    put_u32s(&mut out, &lg.dist_or_time);
    out.extend_from_slice(&lg.alt_desc);
    put_i32s(&mut out, &lg.alt1_ft);
    put_i32s(&mut out, &lg.alt2_ft);
    put_u16s(&mut out, &lg.speed_kt);
    put_bits(&mut out, &lg.dist_is_time);

    let mo = &m.mora;
    put_header(&mut out, TAG_MORA, count("mora", mo.lat.len())?);
    put_i16s(&mut out, &mo.lat);
    put_i16s(&mut out, &mo.lon);
    put_i16s(&mut out, &mo.hundreds_ft);

    let u = &m.unresolved;
    put_header(&mut out, TAG_UNRS, count("unresolved", u.role.len())?);
    out.extend_from_slice(&u.role);
    out.extend_from_slice(&u.owner_kind);
    put_strs(&mut out, &u.owner);
    put_strs(&mut out, &u.scope);
    put_u32s(&mut out, &u.ordinal);
    put_strs(&mut out, &u.name);
    put_strs(&mut out, &u.region);
    put_strs(&mut out, &u.wanted);

    let d = &m.disagreements;
    put_header(&mut out, TAG_DISA, count("disagreements", d.point.len())?);
    put_points(&mut out, &d.point);
    put_i32s(&mut out, &d.nasr_lat);
    put_i32s(&mut out, &d.nasr_lon);
    put_u32s(&mut out, &d.dist_mm);
    check_len(out.len())?;
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Reading

/// A bounded cursor over the input. A subsystem (D5): it owns its position.
// A7.2: cold struct, size guard waived
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    fn take(&mut self, n: usize, what: &'static str) -> Result<&'a [u8], SnapshotError> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or(SnapshotError::Truncated(what))?;
        let s = self
            .data
            .get(self.pos..end)
            .ok_or(SnapshotError::Truncated(what))?;
        self.pos = end;
        Ok(s)
    }

    fn u32(&mut self, what: &'static str) -> Result<u32, SnapshotError> {
        let b: [u8; 4] = self
            .take(4, what)?
            .try_into()
            .map_err(|_| SnapshotError::Truncated(what))?;
        Ok(u32::from_le_bytes(b))
    }

    /// Reads a table header: the tag and the row count. `row_bytes` is the fixed bytes per row
    /// of the table's columns; the count is checked against the bytes left before any column
    /// is allocated. `extra_bits` is the number of bitset columns (each at least `rows / 8`).
    fn table(
        &mut self,
        tag: &[u8; 4],
        name: &'static str,
        row_bytes: usize,
        bitsets: usize,
    ) -> Result<usize, SnapshotError> {
        let got = self.take(4, name)?;
        if got != tag {
            return Err(SnapshotError::BadTag { expected: name });
        }
        let rows = self.u32(name)? as usize;
        let need = rows
            .checked_mul(row_bytes)
            .and_then(|v| v.checked_add(bit_words(rows).checked_mul(8)?.checked_mul(bitsets)?))
            .ok_or(SnapshotError::CountExceedsFile {
                table: name,
                rows: rows as u64,
            })?;
        if need > self.remaining() {
            return Err(SnapshotError::CountExceedsFile {
                table: name,
                rows: rows as u64,
            });
        }
        Ok(rows)
    }

    fn bytes(&mut self, n: usize, what: &'static str) -> Result<Vec<u8>, SnapshotError> {
        Ok(self.take(n, what)?.to_vec())
    }

    fn i32s(&mut self, n: usize, what: &'static str) -> Result<Vec<i32>, SnapshotError> {
        let raw = self.take(
            n.checked_mul(4).ok_or(SnapshotError::Truncated(what))?,
            what,
        )?;
        Ok(raw
            .as_chunks::<4>()
            .0
            .iter()
            .map(|a| i32::from_le_bytes(*a))
            .collect())
    }

    fn u32s(&mut self, n: usize, what: &'static str) -> Result<Vec<u32>, SnapshotError> {
        let raw = self.take(
            n.checked_mul(4).ok_or(SnapshotError::Truncated(what))?,
            what,
        )?;
        Ok(raw
            .as_chunks::<4>()
            .0
            .iter()
            .map(|a| u32::from_le_bytes(*a))
            .collect())
    }

    fn u16s(&mut self, n: usize, what: &'static str) -> Result<Vec<u16>, SnapshotError> {
        let raw = self.take(
            n.checked_mul(2).ok_or(SnapshotError::Truncated(what))?,
            what,
        )?;
        Ok(raw
            .as_chunks::<2>()
            .0
            .iter()
            .map(|a| u16::from_le_bytes(*a))
            .collect())
    }

    fn i16s(&mut self, n: usize, what: &'static str) -> Result<Vec<i16>, SnapshotError> {
        let raw = self.take(
            n.checked_mul(2).ok_or(SnapshotError::Truncated(what))?,
            what,
        )?;
        Ok(raw
            .as_chunks::<2>()
            .0
            .iter()
            .map(|a| i16::from_le_bytes(*a))
            .collect())
    }

    fn strs(&mut self, n: usize, what: &'static str) -> Result<Vec<StrIx>, SnapshotError> {
        Ok(self.u32s(n, what)?.into_iter().map(StrIx::new).collect())
    }

    fn points(&mut self, n: usize, what: &'static str) -> Result<Vec<PointIx>, SnapshotError> {
        Ok(self.u32s(n, what)?.into_iter().map(PointIx::new).collect())
    }

    fn spans<I>(
        &mut self,
        n: usize,
        what: &'static str,
        make: impl Fn(u32) -> I,
    ) -> Result<Vec<Span<I>>, SnapshotError> {
        let raw = self.u32s(
            n.checked_mul(2).ok_or(SnapshotError::Truncated(what))?,
            what,
        )?;
        Ok(raw
            .as_chunks::<2>()
            .0
            .iter()
            .map(|[start, len]| Span {
                start: make(*start),
                len: *len,
            })
            .collect())
    }

    fn bits(&mut self, n: usize, what: &'static str) -> Result<BitSet, SnapshotError> {
        let w = bit_words(n);
        let raw = self.take(
            w.checked_mul(8).ok_or(SnapshotError::Truncated(what))?,
            what,
        )?;
        let words = raw
            .as_chunks::<8>()
            .0
            .iter()
            .map(|a| u64::from_le_bytes(*a))
            .collect();
        Ok(BitSet {
            words,
            len: u32::try_from(n).map_err(|_| SnapshotError::Truncated(what))?,
        })
    }
}

/// Decodes and validates a snapshot. Never panics; a corrupt or truncated file is an error.
pub fn decode(bytes: &[u8]) -> Result<Model, SnapshotError> {
    if bytes.len() > MAX_SNAPSHOT_BYTES {
        return Err(SnapshotError::FileTooLarge { len: bytes.len() });
    }
    let mut r = Reader {
        data: bytes,
        pos: 0,
    };
    if r.take(MAGIC.len(), "magic")? != MAGIC {
        return Err(SnapshotError::BadMagic);
    }
    let version = r.u32("version")?;
    if version != FORMAT_VERSION && version != FORMAT_VERSION_NOTE {
        return Err(SnapshotError::BadVersion(version));
    }
    let nasr_effective = r.u32("header")?;
    let cifp_cycle = r.u32("header")?;
    let buffer_m = r.u32("header")?;
    let artcc_len = r.u32("header")? as usize;
    if artcc_len > MAX_ARTCC_BYTES {
        return Err(SnapshotError::BadArtcc);
    }
    let artcc =
        String::from_utf8(r.bytes(artcc_len, "header")?).map_err(|_| SnapshotError::BadArtcc)?;
    let nasr_sha256: [u8; 32] = r
        .take(32, "header")?
        .try_into()
        .map_err(|_| SnapshotError::Truncated("header"))?;
    let cifp_sha256: [u8; 32] = r
        .take(32, "header")?
        .try_into()
        .map_err(|_| SnapshotError::Truncated("header"))?;
    let meta = Meta {
        nasr_effective,
        cifp_cycle,
        artcc,
        buffer_m,
        nasr_sha256,
        cifp_sha256,
    };
    let cycle_override = if version == FORMAT_VERSION_NOTE {
        let n = r.u32("header")? as usize;
        if n > MAX_CYCLE_OVERRIDE_BYTES {
            return Err(SnapshotError::BadNote);
        }
        Some(String::from_utf8(r.bytes(n, "header")?).map_err(|_| SnapshotError::BadNote)?)
    } else {
        None
    };

    // Strings: the count is of strings, with count + 1 offsets, then the text.
    let n_str = r.table(TAG_STRS, "STRS", 4, 0)?;
    let text_len = r.u32("STRS")? as usize;
    let offsets = r.u32s(
        n_str
            .checked_add(1)
            .ok_or(SnapshotError::Truncated("STRS"))?,
        "STRS",
    )?;
    let text = String::from_utf8(r.bytes(text_len, "STRS")?).map_err(|_| SnapshotError::BadText)?;
    let strings = StrTable { text, offsets };

    let n = r.table(TAG_PNTS, "PNTS", 1 + 4 * 8, 7)?;
    let points = Points {
        kind: r.bytes(n, "PNTS")?,
        lat: r.i32s(n, "PNTS")?,
        lon: r.i32s(n, "PNTS")?,
        elev_ft: r.i32s(n, "PNTS")?,
        ident: r.strs(n, "PNTS")?,
        alt_ident: r.strs(n, "PNTS")?,
        region: r.strs(n, "PNTS")?,
        scope: r.strs(n, "PNTS")?,
        name: r.strs(n, "PNTS")?,
        src_nasr: r.bits(n, "PNTS")?,
        src_cifp: r.bits(n, "PNTS")?,
        in_area: r.bits(n, "PNTS")?,
        cls_b: r.bits(n, "PNTS")?,
        cls_c: r.bits(n, "PNTS")?,
        cls_d: r.bits(n, "PNTS")?,
        cls_e: r.bits(n, "PNTS")?,
    };

    let n = r.table(TAG_VERT, "VERT", 8, 0)?;
    let raw = r.i32s(
        n.checked_mul(2).ok_or(SnapshotError::Truncated("VERT"))?,
        "VERT",
    )?;
    let vertices: Vertices = raw
        .as_chunks::<2>()
        .0
        .iter()
        .map(|[lat, lon]| PosE7 {
            lat: *lat,
            lon: *lon,
        })
        .collect();

    let n = r.table(TAG_BNDS, "BNDS", 4 + 4 * 4 + 16 + 2, 3)?;
    let boundaries = Boundaries {
        source: r.bytes(n, "BNDS")?,
        stratum: r.bytes(n, "BNDS")?,
        edge_interp: r.bytes(n, "BNDS")?,
        multiple: r.bytes(n, "BNDS")?,
        facility: r.strs(n, "BNDS")?,
        name: r.strs(n, "BNDS")?,
        floor_ft: r.i32s(n, "BNDS")?,
        ceiling_ft: r.i32s(n, "BNDS")?,
        vertices: r.spans(n, "BNDS", VertexIx::new)?,
        segs: r.spans(n, "BNDS", SegIx::new)?,
        floor_agl: r.bits(n, "BNDS")?,
        ceiling_agl: r.bits(n, "BNDS")?,
        time_code: r.bytes(n, "BNDS")?,
        level: r.bytes(n, "BNDS")?,
        incomplete: r.bits(n, "BNDS")?,
    };

    let n = r.table(TAG_SEGS, "SEGS", 1 + 4 * 5 + 2, 0)?;
    let segments = Segments {
        via: r.bytes(n, "SEGS")?,
        lat: r.i32s(n, "SEGS")?,
        lon: r.i32s(n, "SEGS")?,
        arc_lat: r.i32s(n, "SEGS")?,
        arc_lon: r.i32s(n, "SEGS")?,
        radius_m: r.u32s(n, "SEGS")?,
        bearing_cdeg: r.u16s(n, "SEGS")?,
    };

    let n = r.table(TAG_AWYS, "AWYS", 4 + 1 + 8, 0)?;
    let airways = Airways {
        designator: r.strs(n, "AWYS")?,
        location: r.bytes(n, "AWYS")?,
        legs: r.spans(n, "AWYS", AirwayLegIx::new)?,
    };

    let n = r.table(TAG_AWLG, "AWLG", 4 * 5 + 2, 3)?;
    let airway_legs = AirwayLegs {
        from: r.points(n, "AWLG")?,
        to: r.points(n, "AWLG")?,
        mea_ft: r.i32s(n, "AWLG")?,
        mea_opp_ft: r.i32s(n, "AWLG")?,
        gps_mea_ft: r.i32s(n, "AWLG")?,
        mea_dir: r.bytes(n, "AWLG")?,
        gps_mea_dir: r.bytes(n, "AWLG")?,
        gap: r.bits(n, "AWLG")?,
        signal_gap: r.bits(n, "AWLG")?,
        dogleg: r.bits(n, "AWLG")?,
    };

    let n = r.table(TAG_PROC, "PROC", 1 + 4 * 3 + 8, 3)?;
    let procs = Procs {
        kind: r.bytes(n, "PROC")?,
        airport: r.points(n, "PROC")?,
        ident: r.strs(n, "PROC")?,
        name: r.strs(n, "PROC")?,
        routes: r.spans(n, "PROC", RouteIx::new)?,
        src_nasr: r.bits(n, "PROC")?,
        src_cifp: r.bits(n, "PROC")?,
        coded: r.bits(n, "PROC")?,
    };

    let n = r.table(TAG_RTES, "RTES", 1 + 4 + 8, 0)?;
    let routes = Routes {
        route_type: r.bytes(n, "RTES")?,
        transition: r.strs(n, "RTES")?,
        legs: r.spans(n, "RTES", LegIx::new)?,
    };

    let n = r.table(TAG_LEGS, "LEGS", 4 * 2 + 2 + 2 + 4 + 1 + 8 + 2, 1)?;
    let legs = Legs {
        fix: r.points(n, "LEGS")?,
        center: r.points(n, "LEGS")?,
        path_term: r.bytes(n, "LEGS")?,
        turn: r.bytes(n, "LEGS")?,
        course_mag_cdeg: r.u16s(n, "LEGS")?,
        dist_or_time: r.u32s(n, "LEGS")?,
        alt_desc: r.bytes(n, "LEGS")?,
        alt1_ft: r.i32s(n, "LEGS")?,
        alt2_ft: r.i32s(n, "LEGS")?,
        speed_kt: r.u16s(n, "LEGS")?,
        dist_is_time: r.bits(n, "LEGS")?,
    };

    let n = r.table(TAG_MORA, "MORA", 6, 0)?;
    let mora = Mora {
        lat: r.i16s(n, "MORA")?,
        lon: r.i16s(n, "MORA")?,
        hundreds_ft: r.i16s(n, "MORA")?,
    };

    let n = r.table(TAG_UNRS, "UNRS", 2 + 4 * 5 + 4, 0)?;
    let unresolved = Unresolved {
        role: r.bytes(n, "UNRS")?,
        owner_kind: r.bytes(n, "UNRS")?,
        owner: r.strs(n, "UNRS")?,
        scope: r.strs(n, "UNRS")?,
        ordinal: r.u32s(n, "UNRS")?,
        name: r.strs(n, "UNRS")?,
        region: r.strs(n, "UNRS")?,
        wanted: r.strs(n, "UNRS")?,
    };

    let n = r.table(TAG_DISA, "DISA", 16, 0)?;
    let disagreements = Disagreements {
        point: r.points(n, "DISA")?,
        nasr_lat: r.i32s(n, "DISA")?,
        nasr_lon: r.i32s(n, "DISA")?,
        dist_mm: r.u32s(n, "DISA")?,
    };

    if r.remaining() != 0 {
        return Err(SnapshotError::TrailingBytes {
            extra: r.remaining(),
        });
    }
    let m = Model {
        meta,
        strings,
        points,
        vertices,
        boundaries,
        segments,
        airways,
        airway_legs,
        procs,
        routes,
        legs,
        mora,
        unresolved,
        disagreements,
        cycle_override,
    };
    validate_model(&m).map_err(SnapshotError::Model)?;
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    // REQ: OA-ASP-012
    #[test]
    fn the_writer_refuses_a_length_the_reader_refuses() {
        assert!(check_len(MAX_SNAPSHOT_BYTES).is_ok());
        assert_eq!(
            check_len(MAX_SNAPSHOT_BYTES + 1),
            Err(SnapshotError::FileTooLarge {
                len: MAX_SNAPSHOT_BYTES + 1
            })
        );
    }
}
