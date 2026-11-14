#![forbid(unsafe_code)]
//! oa-zip: read-only ZIP archive access with DEFLATE, entirely in memory. Core for the byte
//! functions ([`Archive::parse`], [`Archive::extract`], [`inflate`], [`crc32`]); shell only for
//! [`read_archive_file`], which reads a file under a size limit.
//!
//! The inflater also streams: [`Inflater`] takes input in pieces of any size and [`GzipReader`]
//! reads concatenated gzip members (RFC 1952) on top of it, recovering the good part of a file
//! cut short by a crash. Both are here, not in `oa-deflate`, because they decode untrusted input
//! and share the hardened tables and limits of the one-shot [`inflate`], which is built on them.
//!
//! Nothing here ever writes a file: entries are extracted to memory, so a hostile entry name
//! cannot reach the file system. Names with `..`, absolute paths, drive letters or NUL are still
//! listed, but [`Archive::extract`] refuses them ([`ZipError::UnsafePath`]).
//!
//! `Archive` is a subsystem under D5 (it owns the parsed directory), so it keeps methods;
//! [`Limits`] is a plain record (A1). [`Entry`] is a validated value (A1.1): its fields are
//! private, only the parser builds one, and callers read it through plain accessors, so an
//! entry that did not pass the directory checks cannot exist. Cold, offline code: A7.2 applies.

mod crc32;
mod gzip;
mod inflate;

pub use crc32::{crc32, crc32_update};
pub use gzip::{GzipError, GzipReader, GzipSummary, TruncStage, Truncated, MAX_HEADER_FIELD};
pub use inflate::{
    inflate, Consumed, InflateError, InflateLimits, Inflater, StreamError, WINDOW_LEN,
};

use std::fmt;
use std::io::Read;
use std::path::Path;

/// Limit: most entries in one archive. Real FAA archives hold 114 (NASR CSV) and 6 (CIFP); this
/// leaves ~90x headroom while keeping the directory under a megabyte or so of memory.
pub const MAX_ENTRIES: usize = 10_000;
/// Limit: largest uncompressed entry, 256 MiB. The largest real entry (FAACIFP18) is ~53 MB, so
/// ~5x headroom; the output buffer of one extraction can never exceed this.
pub const MAX_ENTRY_BYTES: u64 = 256 * 1024 * 1024;
/// Limit: sum of declared uncompressed sizes over all entries, 1 GiB. Real archives expand to
/// 114 MB (NASR CSV) and 55 MB (CIFP).
pub const MAX_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;
/// Limit: uncompressed / compressed ratio for an entry. The highest ratio among large real
/// entries is ~10.6; 200 is ~19x that and well under DEFLATE's theoretical ~1032.
pub const MAX_RATIO: u64 = 200;
/// Entries at or below this uncompressed size are exempt from [`MAX_RATIO`] (a few KiB of
/// repeated bytes legitimately compress extremely well, and cost nothing to expand).
pub const RATIO_EXEMPT_BYTES: u64 = 1024 * 1024;
/// Limit: longest entry name in bytes (the format allows 65535).
pub const MAX_NAME_BYTES: usize = 4096;
/// Limit: archive size read from a file by [`read_archive_file`], 1 GiB.
pub const MAX_ARCHIVE_BYTES: u64 = 1024 * 1024 * 1024;
/// How far back from the end of the file the end-of-central-directory record is searched: the
/// 22-byte record plus the longest possible comment (65535).
pub const MAX_EOCD_SEARCH: usize = 22 + 65_535;

/// The enforced limits, gathered so tests (and callers with tighter needs) can lower them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Limits {
    pub max_entries: usize,
    pub max_entry_bytes: u64,
    pub max_total_bytes: u64,
    pub max_ratio: u64,
    pub ratio_exempt_bytes: u64,
    pub max_name_bytes: usize,
}

/// The limits above, as one value.
pub const DEFAULT_LIMITS: Limits = Limits {
    max_entries: MAX_ENTRIES,
    max_entry_bytes: MAX_ENTRY_BYTES,
    max_total_bytes: MAX_TOTAL_BYTES,
    max_ratio: MAX_RATIO,
    ratio_exempt_bytes: RATIO_EXEMPT_BYTES,
    max_name_bytes: MAX_NAME_BYTES,
};

/// Storage method 0.
pub const METHOD_STORED: u16 = 0;
/// Storage method 8.
pub const METHOD_DEFLATE: u16 = 8;

/// Errors from reading an archive.
#[derive(Debug)]
pub enum ZipError {
    /// The archive ends inside the named structure.
    Truncated(&'static str),
    /// No end-of-central-directory record within [`MAX_EOCD_SEARCH`] bytes of the end.
    NoEndOfCentralDirectory,
    /// A structure's signature is wrong.
    BadSignature(&'static str),
    /// Multi-disk (spanned) archive.
    MultiDisk,
    /// Entry is encrypted (traditional, strong or AES).
    Encrypted(String),
    /// Compression method other than stored (0) and deflate (8).
    UnsupportedMethod { name: String, method: u16 },
    /// ZIP64 structures missing or inconsistent.
    Zip64(&'static str),
    /// Entry name is a path-traversal or absolute path; it is listed but never extracted.
    UnsafePath(String),
    /// Two entries share a name.
    DuplicateName(String),
    /// No entry with this name.
    NameNotFound(String),
    /// A named limit was exceeded.
    Limit {
        name: &'static str,
        max: u64,
        value: u64,
    },
    /// Directory and local header (or descriptor-less sizes) disagree.
    Inconsistent(&'static str),
    /// Data descriptor disagrees with the central directory.
    DataDescriptorMismatch(String),
    /// Extracted size differs from the declared size.
    SizeMismatch {
        name: String,
        expected: u64,
        actual: u64,
    },
    /// Extracted data fails its CRC-32.
    CrcMismatch {
        name: String,
        expected: u32,
        actual: u32,
    },
    /// The DEFLATE stream is invalid.
    Inflate(InflateError),
    /// File read failure.
    Io(std::io::Error),
}

impl fmt::Display for ZipError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ZipError::Truncated(w) => write!(f, "zip: truncated in {w}"),
            ZipError::NoEndOfCentralDirectory => {
                write!(f, "zip: no end-of-central-directory within MAX_EOCD_SEARCH")
            }
            ZipError::BadSignature(w) => write!(f, "zip: bad signature in {w}"),
            ZipError::MultiDisk => write!(f, "zip: multi-disk archives are not supported"),
            ZipError::Encrypted(n) => write!(f, "zip: entry {n:?} is encrypted"),
            ZipError::UnsupportedMethod { name, method } => {
                write!(f, "zip: entry {name:?} uses unsupported method {method}")
            }
            ZipError::Zip64(w) => write!(f, "zip: zip64 problem: {w}"),
            ZipError::UnsafePath(n) => write!(f, "zip: unsafe entry path {n:?}"),
            ZipError::DuplicateName(n) => write!(f, "zip: duplicate entry name {n:?}"),
            ZipError::NameNotFound(n) => write!(f, "zip: no entry named {n:?}"),
            ZipError::Limit { name, max, value } => {
                write!(f, "zip: limit {name} exceeded ({value} > {max})")
            }
            ZipError::Inconsistent(w) => write!(f, "zip: inconsistent: {w}"),
            ZipError::DataDescriptorMismatch(n) => {
                write!(
                    f,
                    "zip: data descriptor of {n:?} disagrees with central directory"
                )
            }
            ZipError::SizeMismatch {
                name,
                expected,
                actual,
            } => {
                write!(f, "zip: entry {name:?} size {actual}, expected {expected}")
            }
            ZipError::CrcMismatch {
                name,
                expected,
                actual,
            } => {
                write!(
                    f,
                    "zip: entry {name:?} crc32 {actual:08x}, expected {expected:08x}"
                )
            }
            ZipError::Inflate(e) => write!(f, "{e}"),
            ZipError::Io(e) => write!(f, "zip: read error: {e}"),
        }
    }
}

impl std::error::Error for ZipError {}

impl From<InflateError> for ZipError {
    fn from(e: InflateError) -> Self {
        ZipError::Inflate(e)
    }
}

/// One central-directory record that passed the directory checks (A1.1: private fields, a
/// parser-only constructor, plain accessors). A7.2: cold struct, size guard waived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    name: String,
    method: u16,
    flags: u16,
    crc32: u32,
    compressed_size: u64,
    uncompressed_size: u64,
    local_offset: u64,
    zip64: bool,
}

impl Entry {
    /// Name as stored (invalid UTF-8 is replaced, not rejected).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Storage method ([`METHOD_STORED`] or [`METHOD_DEFLATE`]).
    pub fn method(&self) -> u16 {
        self.method
    }

    /// General purpose bit flags as stored.
    pub fn flags(&self) -> u16 {
        self.flags
    }

    /// CRC-32 of the uncompressed data, as declared.
    pub fn crc32(&self) -> u32 {
        self.crc32
    }

    /// Compressed size in bytes, as declared.
    pub fn compressed_size(&self) -> u64 {
        self.compressed_size
    }

    /// Uncompressed size in bytes, as declared; within the archive's limits.
    pub fn uncompressed_size(&self) -> u64 {
        self.uncompressed_size
    }

    /// Offset of the entry's local header.
    pub fn local_offset(&self) -> u64 {
        self.local_offset
    }

    /// Sizes or offset came from a ZIP64 extra field.
    pub fn is_zip64(&self) -> bool {
        self.zip64
    }
}

const SIG_LOCAL: u32 = 0x0403_4b50;
const SIG_CENTRAL: u32 = 0x0201_4b50;
const SIG_EOCD: u32 = 0x0605_4b50;
const SIG_ZIP64_EOCD: u32 = 0x0606_4b50;
const SIG_ZIP64_LOCATOR: u32 = 0x0706_4b50;
const SIG_DESCRIPTOR: u32 = 0x0807_4b50;
const EOCD_LEN: usize = 22;
const LOCATOR_LEN: usize = 20;
const ZIP64_EOCD_LEN: usize = 56;
const CENTRAL_LEN: usize = 46;
const LOCAL_LEN: usize = 30;
const FLAG_ENCRYPTED: u16 = 1;
const FLAG_DESCRIPTOR: u16 = 1 << 3;
const FLAG_STRONG: u16 = 1 << 6;
const METHOD_AES: u16 = 99;

fn u16_at(d: &[u8], o: usize, what: &'static str) -> Result<u16, ZipError> {
    d.get(o..o.saturating_add(2))
        .and_then(|s| s.try_into().ok())
        .map(u16::from_le_bytes)
        .ok_or(ZipError::Truncated(what))
}

fn u32_at(d: &[u8], o: usize, what: &'static str) -> Result<u32, ZipError> {
    d.get(o..o.saturating_add(4))
        .and_then(|s| s.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or(ZipError::Truncated(what))
}

fn u64_at(d: &[u8], o: usize, what: &'static str) -> Result<u64, ZipError> {
    d.get(o..o.saturating_add(8))
        .and_then(|s| s.try_into().ok())
        .map(u64::from_le_bytes)
        .ok_or(ZipError::Truncated(what))
}

fn to_usize(v: u64, what: &'static str) -> Result<usize, ZipError> {
    usize::try_from(v).map_err(|_| ZipError::Zip64(what))
}

/// Whether an entry name is safe to treat as a relative path: non-empty, no NUL, not absolute,
/// no drive letter, and no `..` component (split on both `/` and `\`).
pub fn is_safe_path(name: &str) -> bool {
    if name.is_empty() || name.contains('\0') {
        return false;
    }
    if name.starts_with('/') || name.starts_with('\\') {
        return false;
    }
    if matches!(name.as_bytes().get(..2), Some([c, b':']) if c.is_ascii_alphabetic()) {
        return false;
    }
    !name.split(['/', '\\']).any(|c| c == "..")
}

/// Fields from the end-of-central-directory record(s), after ZIP64 resolution.
// A7.2: cold struct, size guard waived
struct Eocd {
    entries: u64,
    cd_size: u64,
    cd_offset: u64,
}

fn find_eocd(d: &[u8]) -> Result<(usize, Eocd), ZipError> {
    if d.len() < EOCD_LEN {
        return Err(ZipError::NoEndOfCentralDirectory);
    }
    let lowest = d.len().saturating_sub(MAX_EOCD_SEARCH);
    let mut pos = d.len() - EOCD_LEN;
    loop {
        if u32_at(d, pos, "eocd")? == SIG_EOCD {
            let clen = usize::from(u16_at(d, pos + 20, "eocd")?);
            if pos + EOCD_LEN + clen == d.len() {
                if let Some(e) = read_eocd(d, pos)? {
                    return Ok((pos, e));
                }
            }
        }
        if pos <= lowest {
            return Err(ZipError::NoEndOfCentralDirectory);
        }
        pos -= 1;
    }
}

/// Parse the EOCD at `pos`. `Ok(None)` means "not a plausible record, keep scanning".
fn read_eocd(d: &[u8], pos: usize) -> Result<Option<Eocd>, ZipError> {
    let disk = u16_at(d, pos + 4, "eocd")?;
    let cd_disk = u16_at(d, pos + 6, "eocd")?;
    let here = u16_at(d, pos + 8, "eocd")?;
    let total = u16_at(d, pos + 10, "eocd")?;
    let cd_size = u32_at(d, pos + 12, "eocd")?;
    let cd_offset = u32_at(d, pos + 16, "eocd")?;
    let needs64 = total == 0xFFFF
        || here == 0xFFFF
        || cd_size == 0xFFFF_FFFF
        || cd_offset == 0xFFFF_FFFF
        || disk == 0xFFFF
        || cd_disk == 0xFFFF;
    let mut e = Eocd {
        entries: u64::from(total),
        cd_size: u64::from(cd_size),
        cd_offset: u64::from(cd_offset),
    };
    if needs64 {
        let loc = pos
            .checked_sub(LOCATOR_LEN)
            .ok_or(ZipError::Zip64("locator missing"))?;
        if u32_at(d, loc, "zip64 locator")? != SIG_ZIP64_LOCATOR {
            return Err(ZipError::Zip64("locator missing"));
        }
        let loc_disk = u32_at(d, loc + 4, "zip64 locator")?;
        let rec = to_usize(u64_at(d, loc + 8, "zip64 locator")?, "record offset")?;
        let n_disks = u32_at(d, loc + 16, "zip64 locator")?;
        if loc_disk != 0 || n_disks > 1 {
            return Err(ZipError::MultiDisk);
        }
        if u32_at(d, rec, "zip64 eocd")? != SIG_ZIP64_EOCD {
            return Err(ZipError::BadSignature("zip64 eocd"));
        }
        if d.len() < rec.saturating_add(ZIP64_EOCD_LEN) {
            return Err(ZipError::Truncated("zip64 eocd"));
        }
        let z_disk = u32_at(d, rec + 16, "zip64 eocd")?;
        let z_cd_disk = u32_at(d, rec + 20, "zip64 eocd")?;
        let z_here = u64_at(d, rec + 24, "zip64 eocd")?;
        let z_total = u64_at(d, rec + 32, "zip64 eocd")?;
        if z_disk != 0 || z_cd_disk != 0 || z_here != z_total {
            return Err(ZipError::MultiDisk);
        }
        e.entries = z_total;
        e.cd_size = u64_at(d, rec + 40, "zip64 eocd")?;
        e.cd_offset = u64_at(d, rec + 48, "zip64 eocd")?;
    } else if disk != 0 || cd_disk != 0 || here != total {
        return Err(ZipError::MultiDisk);
    }
    // The directory must lie wholly before the EOCD; otherwise this is not the real record.
    match e.cd_offset.checked_add(e.cd_size) {
        Some(end) if end <= pos as u64 => Ok(Some(e)),
        _ => Ok(None),
    }
}

/// An opened archive: the bytes plus the parsed central directory.
#[derive(Debug)]
// A7.2: cold struct, size guard waived
pub struct Archive<'a> {
    data: &'a [u8],
    entries: Vec<Entry>,
    cd_offset: u64,
}

impl<'a> Archive<'a> {
    /// Parse the archive's central directory under [`DEFAULT_LIMITS`].
    pub fn parse(data: &'a [u8]) -> Result<Archive<'a>, ZipError> {
        Archive::parse_with_limits(data, DEFAULT_LIMITS)
    }

    /// Parse the central directory, enforcing `limits` on count, per-entry and total size, ratio.
    pub fn parse_with_limits(data: &'a [u8], limits: Limits) -> Result<Archive<'a>, ZipError> {
        let (_, eocd) = find_eocd(data)?;
        if eocd.entries > limits.max_entries as u64 {
            return Err(ZipError::Limit {
                name: "MAX_ENTRIES",
                max: limits.max_entries as u64,
                value: eocd.entries,
            });
        }
        let count = to_usize(eocd.entries, "entry count")?;
        let cd_start = to_usize(eocd.cd_offset, "directory offset")?;
        let cd_end = to_usize(eocd.cd_offset.saturating_add(eocd.cd_size), "directory end")?;
        let cd = data
            .get(cd_start..cd_end)
            .ok_or(ZipError::Truncated("central directory"))?;
        if count.saturating_mul(CENTRAL_LEN) > cd.len() {
            return Err(ZipError::Truncated("central directory"));
        }
        let mut entries: Vec<Entry> = Vec::with_capacity(count);
        let mut total: u64 = 0;
        let mut p = 0usize;
        for _ in 0..count {
            let (entry, next) = read_central(cd, p, &limits)?;
            if entries.iter().any(|e| e.name == entry.name) {
                return Err(ZipError::DuplicateName(entry.name));
            }
            total = total.saturating_add(entry.uncompressed_size);
            if total > limits.max_total_bytes {
                return Err(ZipError::Limit {
                    name: "MAX_TOTAL_BYTES",
                    max: limits.max_total_bytes,
                    value: total,
                });
            }
            entries.push(entry);
            p = next;
        }
        Ok(Archive {
            data,
            entries,
            cd_offset: eocd.cd_offset,
        })
    }

    /// Every entry, in directory order (unsafe names included).
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Extract the entry called `name` into memory.
    pub fn extract(&self, name: &str) -> Result<Vec<u8>, ZipError> {
        let e = self
            .entries
            .iter()
            .find(|e| e.name == name)
            .ok_or_else(|| ZipError::NameNotFound(name.to_string()))?;
        self.extract_entry(e)
    }

    /// Extract an entry (obtained from [`Archive::entries`]) into memory, verifying its size and
    /// CRC-32.
    pub fn extract_entry(&self, e: &Entry) -> Result<Vec<u8>, ZipError> {
        if !is_safe_path(&e.name) {
            return Err(ZipError::UnsafePath(e.name.clone()));
        }
        let data = self.entry_data(e)?;
        let expected = e.uncompressed_size;
        let out = match e.method {
            METHOD_STORED => {
                if e.compressed_size != expected {
                    return Err(ZipError::Inconsistent("stored entry with differing sizes"));
                }
                data.to_vec()
            }
            _ => {
                // Declared size is within max_entry_bytes (checked at parse), so it fits usize
                // wherever the data does; the cap makes a lying stream an error, not a growth.
                let cap = to_usize(expected, "entry size")?;
                inflate(data, cap, cap).map_err(|err| match err {
                    InflateError::OutputLimit { .. } => ZipError::SizeMismatch {
                        name: e.name.clone(),
                        expected,
                        actual: expected.saturating_add(1),
                    },
                    other => ZipError::Inflate(other),
                })?
            }
        };
        if out.len() as u64 != expected {
            return Err(ZipError::SizeMismatch {
                name: e.name.clone(),
                expected,
                actual: out.len() as u64,
            });
        }
        let actual = crc32(&out);
        if actual != e.crc32 {
            return Err(ZipError::CrcMismatch {
                name: e.name.clone(),
                expected: e.crc32,
                actual,
            });
        }
        Ok(out)
    }

    /// Validate the local header against the directory and return the entry's compressed bytes.
    fn entry_data(&self, e: &Entry) -> Result<&'a [u8], ZipError> {
        let d = self.data;
        let off = to_usize(e.local_offset, "local header offset")?;
        if u32_at(d, off, "local header")? != SIG_LOCAL {
            return Err(ZipError::BadSignature("local header"));
        }
        let flags = u16_at(d, off + 6, "local header")?;
        let method = u16_at(d, off + 8, "local header")?;
        let crc = u32_at(d, off + 14, "local header")?;
        let csize = u32_at(d, off + 18, "local header")?;
        let usize_ = u32_at(d, off + 22, "local header")?;
        let nlen = usize::from(u16_at(d, off + 26, "local header")?);
        let elen = usize::from(u16_at(d, off + 28, "local header")?);
        if flags & (FLAG_ENCRYPTED | FLAG_STRONG) != 0 {
            return Err(ZipError::Encrypted(e.name.clone()));
        }
        if method != e.method {
            return Err(ZipError::Inconsistent("local and central method differ"));
        }
        let name_bytes = d
            .get(off + LOCAL_LEN..off + LOCAL_LEN + nlen)
            .ok_or(ZipError::Truncated("local header name"))?;
        if String::from_utf8_lossy(name_bytes) != e.name {
            return Err(ZipError::Inconsistent("local and central name differ"));
        }
        let has_descriptor = flags & FLAG_DESCRIPTOR != 0;
        if !has_descriptor {
            let same32 =
                |local: u32, central: u64| local == 0xFFFF_FFFF || u64::from(local) == central;
            if crc != e.crc32
                || !same32(csize, e.compressed_size)
                || !same32(usize_, e.uncompressed_size)
            {
                return Err(ZipError::Inconsistent(
                    "local header sizes or crc differ from directory",
                ));
            }
        }
        let start = off
            .checked_add(LOCAL_LEN)
            .and_then(|v| v.checked_add(nlen))
            .and_then(|v| v.checked_add(elen))
            .ok_or(ZipError::Truncated("local header"))?;
        let end = start
            .checked_add(to_usize(e.compressed_size, "compressed size")?)
            .ok_or(ZipError::Truncated("entry data"))?;
        if end as u64 > self.cd_offset {
            return Err(ZipError::Truncated("entry data"));
        }
        let data = d.get(start..end).ok_or(ZipError::Truncated("entry data"))?;
        if has_descriptor {
            check_descriptor(d, end, e)?;
        }
        Ok(data)
    }
}

/// Check the data descriptor following an entry's data against the central directory. Both the
/// signed and unsigned forms, and 32- and 64-bit sizes for ZIP64 entries, are accepted.
fn check_descriptor(d: &[u8], at: usize, e: &Entry) -> Result<(), ZipError> {
    let try_form = |o: usize, wide: bool| -> bool {
        let Ok(crc) = u32_at(d, o, "data descriptor") else {
            return false;
        };
        let (c, u) = if wide {
            match (
                u64_at(d, o + 4, "data descriptor"),
                u64_at(d, o + 12, "data descriptor"),
            ) {
                (Ok(c), Ok(u)) => (c, u),
                _ => return false,
            }
        } else {
            match (
                u32_at(d, o + 4, "data descriptor"),
                u32_at(d, o + 8, "data descriptor"),
            ) {
                (Ok(c), Ok(u)) => (u64::from(c), u64::from(u)),
                _ => return false,
            }
        };
        crc == e.crc32 && c == e.compressed_size && u == e.uncompressed_size
    };
    let wide = e.zip64;
    let signed = u32_at(d, at, "data descriptor").map(|s| s == SIG_DESCRIPTOR);
    let ok = matches!(signed, Ok(true)) && try_form(at + 4, wide) || try_form(at, wide);
    if ok {
        Ok(())
    } else if signed.is_err() {
        Err(ZipError::Truncated("data descriptor"))
    } else {
        Err(ZipError::DataDescriptorMismatch(e.name.clone()))
    }
}

/// Parse one central-directory record at `p`; returns the entry and the offset of the next.
fn read_central(cd: &[u8], p: usize, limits: &Limits) -> Result<(Entry, usize), ZipError> {
    const W: &str = "central directory";
    if u32_at(cd, p, W)? != SIG_CENTRAL {
        return Err(ZipError::BadSignature(W));
    }
    let flags = u16_at(cd, p + 8, W)?;
    let method = u16_at(cd, p + 10, W)?;
    let crc = u32_at(cd, p + 16, W)?;
    let csize = u32_at(cd, p + 20, W)?;
    let usize32 = u32_at(cd, p + 24, W)?;
    let nlen = usize::from(u16_at(cd, p + 28, W)?);
    let elen = usize::from(u16_at(cd, p + 30, W)?);
    let clen = usize::from(u16_at(cd, p + 32, W)?);
    let disk = u16_at(cd, p + 34, W)?;
    let loff = u32_at(cd, p + 42, W)?;
    if nlen > limits.max_name_bytes {
        return Err(ZipError::Limit {
            name: "MAX_NAME_BYTES",
            max: limits.max_name_bytes as u64,
            value: nlen as u64,
        });
    }
    let name_at = p + CENTRAL_LEN;
    let name_bytes = cd
        .get(name_at..name_at + nlen)
        .ok_or(ZipError::Truncated(W))?;
    let name = String::from_utf8_lossy(name_bytes).into_owned();
    let extra = cd
        .get(name_at + nlen..name_at + nlen + elen)
        .ok_or(ZipError::Truncated(W))?;
    let next = name_at + nlen + elen + clen;
    if next > cd.len() {
        return Err(ZipError::Truncated(W));
    }
    if flags & (FLAG_ENCRYPTED | FLAG_STRONG) != 0 || method == METHOD_AES {
        return Err(ZipError::Encrypted(name));
    }
    if method != METHOD_STORED && method != METHOD_DEFLATE {
        return Err(ZipError::UnsupportedMethod { name, method });
    }
    let mut e = Entry {
        name,
        method,
        flags,
        crc32: crc,
        compressed_size: u64::from(csize),
        uncompressed_size: u64::from(usize32),
        local_offset: u64::from(loff),
        zip64: false,
    };
    let mut disk_wide = u32::from(disk);
    let want_usize = usize32 == 0xFFFF_FFFF;
    let want_csize = csize == 0xFFFF_FFFF;
    let want_off = loff == 0xFFFF_FFFF;
    let want_disk = disk == 0xFFFF;
    // Walk extra fields; id 0x0001 carries the ZIP64 values for whichever fields were 0xFFFF...
    let mut x = 0usize;
    let mut seen_zip64 = false;
    while x < extra.len() {
        let id = u16_at(extra, x, "extra field")?;
        let sz = usize::from(u16_at(extra, x + 2, "extra field")?);
        let body = extra
            .get(x + 4..x + 4 + sz)
            .ok_or(ZipError::Truncated("extra field"))?;
        if id == 0x0001 {
            if seen_zip64 {
                return Err(ZipError::Zip64("more than one zip64 extra field"));
            }
            seen_zip64 = true;
            e.zip64 = true;
            // The extra field holds exactly the values whose 32-bit field is 0xFFFFFFFF, in
            // this order (APPNOTE 4.5.3): a different length means the flag and the field
            // disagree.
            let want = 8
                * (usize::from(want_usize) + usize::from(want_csize) + usize::from(want_off))
                + 4 * usize::from(want_disk);
            if body.len() != want {
                return Err(ZipError::Zip64(
                    "zip64 extra field does not match the 0xFFFFFFFF fields",
                ));
            }
            let mut q = 0usize;
            let next_u64 = |q: &mut usize| -> Result<u64, ZipError> {
                let v = u64_at(body, *q, "zip64 extra")?;
                *q += 8;
                Ok(v)
            };
            if want_usize {
                e.uncompressed_size = next_u64(&mut q)?;
            }
            if want_csize {
                e.compressed_size = next_u64(&mut q)?;
            }
            if want_off {
                e.local_offset = next_u64(&mut q)?;
            }
            if want_disk {
                disk_wide = u32_at(body, q, "zip64 extra")?;
            }
        }
        x += 4 + sz;
    }
    if (want_usize || want_csize || want_off || want_disk) && !e.zip64 {
        return Err(ZipError::Zip64("0xFFFFFFFF field without zip64 extra"));
    }
    // The converse: a zip64 extra field that no 0xFFFFFFFF field asks for is inconsistent
    // (rejected above for any body that is not empty; an empty one is rejected here).
    if e.zip64 && !(want_usize || want_csize || want_off || want_disk) {
        return Err(ZipError::Zip64(
            "zip64 extra field without a 0xFFFFFFFF field",
        ));
    }
    if disk_wide != 0 {
        return Err(ZipError::MultiDisk);
    }
    if e.uncompressed_size > limits.max_entry_bytes {
        return Err(ZipError::Limit {
            name: "MAX_ENTRY_BYTES",
            max: limits.max_entry_bytes,
            value: e.uncompressed_size,
        });
    }
    if e.uncompressed_size > limits.ratio_exempt_bytes
        && e.uncompressed_size > e.compressed_size.max(1).saturating_mul(limits.max_ratio)
    {
        return Err(ZipError::Limit {
            name: "MAX_RATIO",
            max: limits.max_ratio,
            value: e.uncompressed_size / e.compressed_size.max(1),
        });
    }
    Ok((e, next))
}

/// Read a whole archive file into memory, refusing files above [`MAX_ARCHIVE_BYTES`].
pub fn read_archive_file(path: &Path) -> Result<Vec<u8>, ZipError> {
    let f = std::fs::File::open(path).map_err(ZipError::Io)?;
    let mut buf = Vec::new();
    // Read one byte past the limit so an oversize file is detected, not silently truncated.
    f.take(MAX_ARCHIVE_BYTES + 1)
        .read_to_end(&mut buf)
        .map_err(ZipError::Io)?;
    if buf.len() as u64 > MAX_ARCHIVE_BYTES {
        return Err(ZipError::Limit {
            name: "MAX_ARCHIVE_BYTES",
            max: MAX_ARCHIVE_BYTES,
            value: buf.len() as u64,
        });
    }
    Ok(buf)
}
