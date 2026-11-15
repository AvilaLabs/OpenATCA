#![forbid(unsafe_code)]
//! oa-tar: a streaming, bounded reader of POSIX ustar and GNU tar archives. Shell code (it
//! reads `std::io::Read` sources), but all decisions are made on plain bytes (RULESET B1).
//!
//! * Streaming: a [`TarReader`] holds one 512-byte header and a small scratch buffer. Entry data
//!   is handed out in pieces by [`TarReader::read_data`]; whatever the caller does not read is
//!   skipped by the next [`TarReader::next_entry`]. Nothing is seeked, so any `Read` works.
//! * Split archives: the reader takes a sequence of readers and treats them as one byte stream,
//!   the way adsb.lol releases are split (`name.tar.aa`, `name.tar.ab`, ... are slices of one
//!   tar). A boundary may fall anywhere, inside a header or inside data.
//! * Truncation is a result, not a crash: if the sequence ends inside a header, an entry's data
//!   or its padding, or before both blocks of the end-of-archive marker have been read, the
//!   reader reports [`Truncated`] (the offset, the stage, the entry and how much of its data
//!   was there) and is finished. Entries read before the cut are complete and
//!   stand. A stream that ends exactly between two entries, with no end marker, is also
//!   [`Next::Truncated`] with stage [`Stage::Header`]: a finished archive has the marker.
//! * Formats: ustar (name plus prefix), old V7-style headers (no magic), GNU long names (type
//!   `L`; long link names, type `K`, are read and ignored), GNU base-256 sizes, PAX extended
//!   headers (type `x`: `path` and `size` are honoured, other keys ignored; type `g` global
//!   headers are skipped). Sizes may be octal or base-256.
//! * Validated: every header's checksum (unsigned or signed sum), numeric fields, UTF-8 names.
//!   A header that fails is [`TarError::BadHeader`] with its offset; it is never passed on.
//!   Names are returned as stored: this crate never writes files, so a name with `..` or a
//!   leading `/` is the caller's to refuse.
//! * Bounds ([`Limits`]): entry data size ([`MAX_ENTRY_BYTES`]), name length
//!   ([`MAX_NAME_BYTES`]) and the size of a long-name or PAX record ([`MAX_META_BYTES`]).
//!   An entry above the size limit is refused ([`TarError::EntryTooLarge`]) before any of its
//!   data is read.
//! * Data length: only types that carry data (regular files, long names, PAX, unknown types)
//!   use the size field; hard and symbolic links, devices, directories and FIFOs (types `1`-`6`)
//!   carry none whatever the field says.
//!
//! The reader is a subsystem under D5 (it owns the stream position), so it keeps methods;
//! [`Header`] and [`Truncated`] are plain records (A1). Cold, offline code: A7.2 applies.

use std::fmt;
use std::io::{self, Read};

/// Bytes in a tar block (header size, and the unit data is padded to).
pub const BLOCK: usize = 512;
/// Limit: entry data size, 8 GiB. adsb.lol trace files are kilobytes to a few megabytes; the
/// limit exists so a corrupt size field cannot send the reader skipping through the stream.
pub const MAX_ENTRY_BYTES: u64 = 8 * 1024 * 1024 * 1024;
/// Limit: a name (long name, PAX path, or prefix + name) in bytes. Matches PATH_MAX.
pub const MAX_NAME_BYTES: usize = 4096;
/// Limit: bytes of one GNU long-name/long-link record or PAX header.
pub const MAX_META_BYTES: usize = 64 * 1024;
/// Limit: long-name, long-link and PAX records consumed by one `next_entry` call (a run of
/// zero-length ones counts), so a caller's per-call work is bounded.
pub const MAX_META_RECORDS: usize = 64;
/// Limit: bytes of content of those records, together, in one `next_entry` call.
pub const MAX_META_TOTAL_BYTES: usize = 1024 * 1024;

/// The enforced limits, gathered so tests (and callers with tighter needs) can lower them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Limits {
    pub max_entry_bytes: u64,
    pub max_name_bytes: usize,
    pub max_meta_bytes: usize,
    pub max_meta_records: usize,
    pub max_meta_total_bytes: usize,
}

/// The limits above, as one value.
pub const DEFAULT_LIMITS: Limits = Limits {
    max_entry_bytes: MAX_ENTRY_BYTES,
    max_name_bytes: MAX_NAME_BYTES,
    max_meta_bytes: MAX_META_BYTES,
    max_meta_records: MAX_META_RECORDS,
    max_meta_total_bytes: MAX_META_TOTAL_BYTES,
};

/// One entry's header, after long names and PAX overrides are applied.
#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Header {
    /// Full name as stored (prefix and long-name handling applied).
    pub name: String,
    /// Data bytes that follow (0 for entries that carry none).
    pub size: u64,
    /// Type flag byte as stored (`b'0'` for NUL and old-style regular files).
    pub typeflag: u8,
    /// Modification time, seconds since the Unix epoch.
    pub mtime: u64,
    /// Offset in the whole stream of this entry's header block (of the first extension block if
    /// the entry has a long name or PAX header).
    pub offset: u64,
}

/// Where the stream ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Inside a header block, or between entries without the end marker.
    Header,
    /// Inside an entry's data.
    Data,
    /// All of an entry's data was there; its padding to the block boundary was not.
    Padding,
}

/// The stream ended early. Plain record (A1).
#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Truncated {
    /// Total bytes of the stream that were present.
    pub offset: u64,
    /// What was being read.
    pub stage: Stage,
    /// The entry the cut fell in (empty if between entries or in its extension headers; for a
    /// cut in a long-name or PAX record, `entry_size` and `data_present` describe that record).
    pub entry: String,
    /// The entry's declared data size (0 if not in an entry).
    pub entry_size: u64,
    /// Bytes of the entry's data that were present.
    pub data_present: u64,
}

/// What [`TarReader::next_entry`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    /// A header; its data is read with [`TarReader::read_data`].
    Entry(Header),
    /// The end-of-archive marker: the archive is complete.
    End,
    /// The stream ended early; the reader is finished.
    Truncated(Truncated),
}

/// What [`TarReader::read_data`] produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Data {
    /// This many bytes (at least one) were written to the start of the buffer.
    Bytes(usize),
    /// All of the entry's data has been read.
    End,
    /// The stream ended inside the entry; the reader is finished.
    Truncated(Truncated),
}

/// Why an archive was refused.
#[derive(Debug)]
pub enum TarError {
    /// A header failed validation (checksum, number, name); the offset is its block.
    BadHeader { offset: u64, what: &'static str },
    /// An entry's size is above [`Limits::max_entry_bytes`].
    EntryTooLarge { offset: u64, size: u64 },
    /// A name or long-name record is above its limit.
    NameTooLong { offset: u64, len: u64 },
    /// A long-name or PAX record is above [`Limits::max_meta_bytes`].
    MetaTooLarge { offset: u64, size: u64 },
    /// One call met more extension records, or more of their content, than
    /// [`Limits::max_meta_records`] or [`Limits::max_meta_total_bytes`] allow; the offset is the
    /// record over the cap.
    TooManyMeta { offset: u64, records: u64 },
    /// The underlying reader failed.
    Io { offset: u64, error: io::Error },
    /// An earlier error ended this reader.
    Failed,
}

impl fmt::Display for TarError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TarError::BadHeader { offset, what } => {
                write!(f, "tar: bad header at byte {offset}: {what}")
            }
            TarError::EntryTooLarge { offset, size } => {
                write!(
                    f,
                    "tar: entry at byte {offset} declares {size} bytes, over the limit"
                )
            }
            TarError::NameTooLong { offset, len } => {
                write!(
                    f,
                    "tar: name at byte {offset} is {len} bytes, over the limit"
                )
            }
            TarError::MetaTooLarge { offset, size } => {
                write!(
                    f,
                    "tar: extension record at byte {offset} is {size} bytes, over the limit"
                )
            }
            TarError::TooManyMeta { offset, records } => {
                write!(
                    f,
                    "tar: extension record {records} at byte {offset} is over the per-entry cap"
                )
            }
            TarError::Io { offset, error } => {
                write!(f, "tar: read failed at byte {offset}: {error}")
            }
            TarError::Failed => write!(f, "tar: reader stopped after an earlier error"),
        }
    }
}

impl std::error::Error for TarError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Between entries: the next call reads a header.
    Idle,
    /// Inside an entry: `left` data bytes then `pad` padding bytes remain.
    InEntry,
    Done,
    Failed,
}

/// Streaming reader over a sequence of readers that together form one tar archive.
// A7.2: cold struct (one per archive), size guard waived
pub struct TarReader<R: Read> {
    parts: Vec<R>,
    cur: usize,
    limits: Limits,
    /// Bytes consumed from the stream so far.
    pos: u64,
    state: State,
    block: [u8; BLOCK],
    scratch: Vec<u8>,
    /// Entry in progress: name, declared size, data bytes still unread, padding still unread.
    name: String,
    size: u64,
    left: u64,
    pad: u64,
}

/// Outcome of reading up to a given number of bytes across parts.
enum Fill {
    Full,
    /// The stream ended after this many bytes (fewer than asked).
    Short(usize),
}

impl<R: Read> TarReader<R> {
    /// An archive that is `parts` read in order. One part is the usual case.
    pub fn new(parts: Vec<R>, limits: Limits) -> TarReader<R> {
        TarReader {
            parts,
            cur: 0,
            limits,
            pos: 0,
            state: State::Idle,
            block: [0; BLOCK],
            scratch: Vec::new(),
            name: String::new(),
            size: 0,
            left: 0,
            pad: 0,
        }
    }

    /// Bytes of the stream consumed so far.
    pub fn position(&self) -> u64 {
        self.pos
    }

    fn read_some(&mut self, buf: &mut [u8]) -> Result<usize, TarError> {
        loop {
            let Some(part) = self.parts.get_mut(self.cur) else {
                return Ok(0);
            };
            match part.read(buf) {
                Ok(0) => self.cur += 1,
                Ok(n) => {
                    self.pos += n as u64;
                    return Ok(n);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => {
                    self.state = State::Failed;
                    return Err(TarError::Io {
                        offset: self.pos,
                        error,
                    });
                }
            }
        }
    }

    /// Fills `buf` completely unless the stream ends first.
    fn fill(&mut self, buf: &mut [u8]) -> Result<Fill, TarError> {
        let mut got = 0;
        while got < buf.len() {
            let n = self.read_some(buf.get_mut(got..).unwrap_or(&mut []))?;
            if n == 0 {
                return Ok(Fill::Short(got));
            }
            got += n;
        }
        Ok(Fill::Full)
    }

    fn fill_block(&mut self) -> Result<Fill, TarError> {
        let mut b = [0u8; BLOCK];
        let r = self.fill(&mut b)?;
        self.block = b;
        Ok(r)
    }

    /// Discards `n` bytes. Returns how many were actually there.
    fn skip(&mut self, mut n: u64) -> Result<u64, TarError> {
        let mut buf = std::mem::take(&mut self.scratch);
        buf.resize(8192, 0);
        let mut skipped = 0;
        while n > 0 {
            let want = n.min(buf.len() as u64) as usize;
            let r = self.read_some(buf.get_mut(..want).unwrap_or(&mut []));
            match r {
                Ok(0) => break,
                Ok(got) => {
                    n -= got as u64;
                    skipped += got as u64;
                }
                Err(e) => {
                    self.scratch = buf;
                    return Err(e);
                }
            }
        }
        self.scratch = buf;
        Ok(skipped)
    }

    fn truncated(&mut self, stage: Stage, data_present: u64) -> Truncated {
        self.state = State::Done;
        Truncated {
            offset: self.pos,
            stage,
            entry: std::mem::take(&mut self.name),
            entry_size: self.size,
            data_present,
        }
    }

    fn fail<T>(&mut self, e: TarError) -> Result<T, TarError> {
        self.state = State::Failed;
        Err(e)
    }

    /// Reads `size` bytes of metadata (long name, PAX) into a vector, then its padding.
    fn read_meta(
        &mut self,
        size: u64,
        offset: u64,
    ) -> Result<Result<Vec<u8>, Truncated>, TarError> {
        if size > self.limits.max_meta_bytes as u64 {
            return self.fail(TarError::MetaTooLarge { offset, size });
        }
        let mut v = vec![0u8; size as usize];
        match self.fill(&mut v)? {
            Fill::Full => {}
            Fill::Short(n) => {
                self.size = size;
                return Ok(Err(self.truncated(Stage::Data, n as u64)));
            }
        }
        let pad = pad_len(size);
        if self.skip(pad)? < pad {
            self.size = size;
            return Ok(Err(self.truncated(Stage::Padding, size)));
        }
        Ok(Ok(v))
    }

    /// Advances to the next entry, skipping any unread data of the current one.
    pub fn next_entry(&mut self) -> Result<Next, TarError> {
        match self.state {
            State::Failed => return Err(TarError::Failed),
            State::Done => {
                return Ok(Next::Truncated(Truncated {
                    offset: self.pos,
                    stage: Stage::Header,
                    entry: String::new(),
                    entry_size: 0,
                    data_present: 0,
                }))
            }
            State::InEntry => {
                let (left, pad) = (self.left, self.pad);
                let got = self.skip(left)?;
                if got < left {
                    let present = self.size - left + got;
                    return Ok(Next::Truncated(self.truncated(Stage::Data, present)));
                }
                if self.skip(pad)? < pad {
                    let size = self.size;
                    return Ok(Next::Truncated(self.truncated(Stage::Padding, size)));
                }
                self.state = State::Idle;
            }
            State::Idle => {}
        }
        self.name.clear();
        self.size = 0;
        let mut long_name: Option<String> = None;
        let mut pax_path: Option<String> = None;
        let mut pax_size: Option<u64> = None;
        let first_offset = self.pos;
        let mut meta_records = 0usize;
        let mut meta_bytes = 0usize;
        loop {
            let offset = self.pos;
            match self.fill_block()? {
                Fill::Full => {}
                Fill::Short(_) => return Ok(Next::Truncated(self.truncated(Stage::Header, 0))),
            }
            if self.block.iter().all(|&b| b == 0) {
                // End marker: two zero blocks. A second one that is missing is a truncation.
                return match self.fill_block()? {
                    Fill::Full if self.block.iter().all(|&b| b == 0) => {
                        self.state = State::Done;
                        Ok(Next::End)
                    }
                    Fill::Full => self.fail(TarError::BadHeader {
                        offset: offset + BLOCK as u64,
                        what: "data after the first zero block",
                    }),
                    Fill::Short(_) => Ok(Next::Truncated(self.truncated(Stage::Header, 0))),
                };
            }
            let parsed = match parse_header(&self.block, offset, &self.limits) {
                Ok(p) => p,
                Err(e) => return self.fail(e),
            };
            let is_ext = matches!(parsed.typeflag, b'L' | b'K' | b'x' | b'g');
            // A pending PAX size belongs to the next entry that is not itself an extension record.
            let mut size = parsed.size;
            if let Some(s) = pax_size.filter(|_| !is_ext) {
                size = s;
            }
            if is_ext {
                // A record over the per-record limit is refused by `read_meta` with its own error.
                meta_records += 1;
                meta_bytes = meta_bytes.saturating_add(size.min(usize::MAX as u64) as usize);
                if size <= self.limits.max_meta_bytes as u64
                    && (meta_records > self.limits.max_meta_records
                        || meta_bytes > self.limits.max_meta_total_bytes)
                {
                    return self.fail(TarError::TooManyMeta {
                        offset,
                        records: meta_records as u64,
                    });
                }
            }
            match parsed.typeflag {
                b'L' | b'K' => {
                    let v = match self.read_meta(size, offset)? {
                        Ok(v) => v,
                        Err(t) => return Ok(Next::Truncated(t)),
                    };
                    if parsed.typeflag == b'L' {
                        match cstring(&v, self.limits.max_name_bytes, offset) {
                            Ok(s) => long_name = Some(s),
                            Err(e) => return self.fail(e),
                        }
                    }
                    continue;
                }
                b'x' | b'g' => {
                    let v = match self.read_meta(size, offset)? {
                        Ok(v) => v,
                        Err(t) => return Ok(Next::Truncated(t)),
                    };
                    if parsed.typeflag == b'x' {
                        match parse_pax(&v, &self.limits, offset) {
                            Ok((p, s)) => {
                                if p.is_some() {
                                    pax_path = p;
                                }
                                if s.is_some() {
                                    pax_size = s;
                                }
                            }
                            Err(e) => return self.fail(e),
                        }
                    }
                    continue;
                }
                _ => {}
            }
            let data = match parsed.typeflag {
                b'1'..=b'6' => 0,
                _ => size,
            };
            if data > self.limits.max_entry_bytes {
                return self.fail(TarError::EntryTooLarge { offset, size: data });
            }
            let name = pax_path.or(long_name).unwrap_or(parsed.name);
            self.name = name.clone();
            self.size = data;
            self.left = data;
            self.pad = pad_len(data);
            self.state = State::InEntry;
            return Ok(Next::Entry(Header {
                name,
                size: data,
                typeflag: parsed.typeflag,
                mtime: parsed.mtime,
                offset: first_offset,
            }));
        }
    }

    /// Reads up to `buf.len()` bytes of the current entry's data.
    pub fn read_data(&mut self, buf: &mut [u8]) -> Result<Data, TarError> {
        match self.state {
            State::Failed => return Err(TarError::Failed),
            State::InEntry => {}
            State::Idle | State::Done => return Ok(Data::End),
        }
        if self.left == 0 || buf.is_empty() {
            return Ok(Data::End);
        }
        let want = (self.left).min(buf.len() as u64) as usize;
        let n = self.read_some(buf.get_mut(..want).unwrap_or(&mut []))?;
        if n == 0 {
            let present = self.size - self.left;
            return Ok(Data::Truncated(self.truncated(Stage::Data, present)));
        }
        self.left -= n as u64;
        Ok(Data::Bytes(n))
    }

    /// Reads the rest of the current entry into `out`. Returns `Ok(None)` when the entry was
    /// complete, `Ok(Some(t))` when the stream ended inside it (`out` then holds what was there).
    pub fn read_entry_to_vec(&mut self, out: &mut Vec<u8>) -> Result<Option<Truncated>, TarError> {
        // The entry size is bounded by Limits::max_entry_bytes; reserve only a sane amount at
        // once so a lying size field cannot allocate it up front.
        out.reserve(self.left.min(1 << 20) as usize);
        let mut buf = [0u8; 16 * 1024];
        loop {
            match self.read_data(&mut buf)? {
                Data::Bytes(n) => out.extend_from_slice(buf.get(..n).unwrap_or(&[])),
                Data::End => return Ok(None),
                Data::Truncated(t) => return Ok(Some(t)),
            }
        }
    }
}

fn pad_len(size: u64) -> u64 {
    (BLOCK as u64 - size % BLOCK as u64) % BLOCK as u64
}

struct Parsed {
    name: String,
    size: u64,
    typeflag: u8,
    mtime: u64,
}

fn bad<T>(offset: u64, what: &'static str) -> Result<T, TarError> {
    Err(TarError::BadHeader { offset, what })
}

/// A NUL-terminated (or field-filling) name as a validated string.
fn cstring(field: &[u8], max: usize, offset: u64) -> Result<String, TarError> {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    let bytes = field.get(..end).unwrap_or(&[]);
    if bytes.len() > max {
        return Err(TarError::NameTooLong {
            offset,
            len: bytes.len() as u64,
        });
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => Ok(s.to_string()),
        Err(_) => bad(offset, "name is not valid UTF-8"),
    }
}

/// Octal number in a header field: leading spaces, digits, ended by NUL or space.
fn octal(field: &[u8]) -> Option<u64> {
    let mut v: u64 = 0;
    let mut seen = false;
    let mut ended = false;
    for &b in field {
        match b {
            b' ' if !seen => {}
            b'0'..=b'7' if !ended => {
                seen = true;
                v = v.checked_mul(8)?.checked_add(u64::from(b - b'0'))?;
            }
            0 | b' ' if seen => ended = true,
            0 => ended = true,
            _ => return None,
        }
    }
    seen.then_some(v)
}

/// A size field: octal, or GNU base-256 (high bit of the first byte set).
fn number(field: &[u8]) -> Option<u64> {
    match field.first() {
        Some(&b) if b & 0x80 != 0 => {
            if b == 0xff {
                return None; // negative
            }
            let mut v = u64::from(b & 0x7f);
            for &d in field.get(1..).unwrap_or(&[]) {
                if v > u64::MAX >> 8 {
                    return None;
                }
                v = (v << 8) | u64::from(d);
            }
            Some(v)
        }
        _ => octal(field),
    }
}

fn parse_header(b: &[u8; BLOCK], offset: u64, lim: &Limits) -> Result<Parsed, TarError> {
    let f = |a: usize, z: usize| b.get(a..z).unwrap_or(&[]);
    let Some(stored) = octal(f(148, 156)) else {
        return bad(offset, "checksum field is not a number");
    };
    let mut unsigned: u64 = 0;
    let mut signed: i64 = 0;
    for (i, &x) in b.iter().enumerate() {
        let x = if (148..156).contains(&i) { b' ' } else { x };
        unsigned += u64::from(x);
        signed += i64::from(x as i8);
    }
    if stored != unsigned && i64::try_from(stored).ok() != Some(signed) {
        return bad(offset, "checksum mismatch");
    }
    let Some(size) = number(f(124, 136)) else {
        return bad(offset, "size field is not a number");
    };
    let Some(mtime) = number(f(136, 148)) else {
        return bad(offset, "mtime field is not a number");
    };
    let typeflag = match b.get(156).copied().unwrap_or(0) {
        0 => b'0',
        t => t,
    };
    let mut name = cstring(f(0, 100), lim.max_name_bytes, offset)?;
    let magic = f(257, 263);
    if magic.starts_with(b"ustar") {
        let prefix = cstring(f(345, 500), lim.max_name_bytes, offset)?;
        if !prefix.is_empty() {
            name = format!("{prefix}/{name}");
            if name.len() > lim.max_name_bytes {
                return Err(TarError::NameTooLong {
                    offset,
                    len: name.len() as u64,
                });
            }
        }
    }
    Ok(Parsed {
        name,
        size,
        typeflag,
        mtime,
    })
}

/// PAX records `"<len> <key>=<value>\n"`; returns the `path` and `size` overrides.
fn parse_pax(
    data: &[u8],
    lim: &Limits,
    offset: u64,
) -> Result<(Option<String>, Option<u64>), TarError> {
    let (mut path, mut size) = (None, None);
    let mut i = 0usize;
    while i < data.len() {
        let rest = data.get(i..).unwrap_or(&[]);
        let Some(sp) = rest.iter().position(|&b| b == b' ') else {
            return bad(offset, "pax record without length");
        };
        let len: usize = match std::str::from_utf8(rest.get(..sp).unwrap_or(&[]))
            .ok()
            .and_then(|s| s.parse().ok())
        {
            Some(l) if l > sp + 1 && l <= rest.len() => l,
            _ => return bad(offset, "pax record length is wrong"),
        };
        let rec = rest.get(sp + 1..len).unwrap_or(&[]);
        let Some(rec) = rec.strip_suffix(b"\n") else {
            return bad(offset, "pax record does not end in a newline");
        };
        if let Some(eq) = rec.iter().position(|&b| b == b'=') {
            let (k, v) = (
                rec.get(..eq).unwrap_or(&[]),
                rec.get(eq + 1..).unwrap_or(&[]),
            );
            if k == b"path" {
                path = Some(cstring(v, lim.max_name_bytes, offset)?);
            } else if k == b"size" {
                size = match std::str::from_utf8(v)
                    .ok()
                    .and_then(|s| s.parse::<u64>().ok())
                {
                    Some(s) => Some(s),
                    None => return bad(offset, "pax size is not a number"),
                };
            }
        } else {
            return bad(offset, "pax record without =");
        }
        i += len;
    }
    Ok((path, size))
}
