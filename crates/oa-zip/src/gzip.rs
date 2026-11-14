//! gzip member reader (RFC 1952) on the streaming inflater. Core (pure): bytes in, bytes out
//! through a closure.
//!
//! [`GzipReader`] takes input in pieces of any size, parses each member's header (including the
//! optional FEXTRA, FNAME, FCOMMENT and FHCRC fields), inflates it, and checks the CRC-32 and
//! ISIZE in the trailer. Members may follow one another (a concatenated gzip file). Decoded bytes
//! are delivered as they are produced, before the trailer can be checked; a bad trailer is then an
//! error, and the caller decides whether the delivered bytes stand.
//!
//! A file cut short, as a crash leaves it, is not an error of the reader: [`GzipReader::finish`]
//! reports it as [`Truncated`] with the byte counts, so it is never mistaken for a complete
//! file and never loses the data that was written. Delivered bytes include everything decoded
//! before the cut; [`Truncated::flushed_bytes`] says how many of them lie before the last sync
//! flush, the point a crash is guaranteed to leave whole.

use std::fmt;

use crate::crc32::crc32_update;
use crate::inflate::{InflateError, InflateLimits, Inflater, StreamError};

/// Longest FNAME or FCOMMENT field accepted, in bytes (the field is skipped, not kept).
pub const MAX_HEADER_FIELD: u32 = 64 * 1024;

const FLAG_HCRC: u8 = 2;
const FLAG_EXTRA: u8 = 4;
const FLAG_NAME: u8 = 8;
const FLAG_COMMENT: u8 = 16;
const FLAG_RESERVED: u8 = 0xe0;

/// Errors from reading gzip. Each kind of damage has its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GzipError<E> {
    /// A member does not start with the gzip magic 1f 8b.
    BadMagic,
    /// Compression method other than 8 (deflate).
    UnsupportedMethod(u8),
    /// Reserved flag bits set in the header.
    ReservedFlags(u8),
    /// An FNAME or FCOMMENT field longer than [`MAX_HEADER_FIELD`] with no terminator.
    HeaderFieldTooLong,
    /// The header's own CRC-16 does not match.
    HeaderCrc { expected: u16, actual: u16 },
    /// The DEFLATE data is invalid or over a limit.
    Data(InflateError),
    /// The trailer CRC-32 does not match the decoded data.
    CrcMismatch { expected: u32, actual: u32 },
    /// The trailer ISIZE does not match the decoded length (modulo 2^32).
    SizeMismatch { expected: u32, actual: u32 },
    /// An earlier error ended this reader; it takes no more input.
    Failed,
    /// The output closure returned this error.
    Sink(E),
}

impl<E: fmt::Display> fmt::Display for GzipError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GzipError::BadMagic => write!(f, "gzip: bad magic (not a gzip member)"),
            GzipError::UnsupportedMethod(m) => write!(f, "gzip: unsupported method {m}"),
            GzipError::ReservedFlags(b) => write!(f, "gzip: reserved header flags {b:#04x}"),
            GzipError::HeaderFieldTooLong => write!(f, "gzip: header name or comment too long"),
            GzipError::HeaderCrc { expected, actual } => write!(
                f,
                "gzip: header CRC-16 mismatch (stored {expected:#06x}, computed {actual:#06x})"
            ),
            GzipError::Data(e) => write!(f, "gzip: {e}"),
            GzipError::CrcMismatch { expected, actual } => write!(
                f,
                "gzip: CRC-32 mismatch (stored {expected:#010x}, computed {actual:#010x})"
            ),
            GzipError::SizeMismatch { expected, actual } => write!(
                f,
                "gzip: ISIZE mismatch (stored {expected}, decoded {actual} modulo 2^32)"
            ),
            GzipError::Failed => write!(f, "gzip: reader stopped after an earlier error"),
            GzipError::Sink(e) => write!(f, "gzip: output failed: {e}"),
        }
    }
}

impl<E: fmt::Debug + fmt::Display> std::error::Error for GzipError<E> {}

/// Where a cut-off member ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TruncStage {
    /// Inside the header (or the file is empty).
    Header,
    /// Inside the DEFLATE data.
    Data,
    /// All data is present; the 8-byte trailer is incomplete, so it is unverified.
    Trailer,
}

/// A member that was cut off. Plain record (A1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Truncated {
    /// Where it ended.
    pub stage: TruncStage,
    /// Bytes recovered from the unfinished member (delivered to the output closure).
    pub tail_bytes: u64,
    /// Of those, the bytes before the last sync flush: whole, flushed data.
    pub flushed_bytes: u64,
}

/// What a finished stream amounted to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct GzipSummary {
    /// Members read to the end with CRC-32 and ISIZE verified.
    pub members: u64,
    /// Every byte delivered, including a truncated tail.
    pub bytes_out: u64,
    /// Set if the input ended inside a member.
    pub truncated: Option<Truncated>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Id1,
    Id2,
    Method,
    Flags,
    /// MTIME, XFL, OS: this many bytes still to skip.
    Fixed(u8),
    ExtraLen(u8),
    ExtraSkip(u16),
    /// FNAME, with the bytes seen so far.
    Name(u32),
    /// FCOMMENT, with the bytes seen so far.
    Comment(u32),
    Hcrc(u8),
    Data,
    /// The trailer, with the bytes of it seen so far.
    Trailer(u8),
    Failed,
}

/// Streaming reader of concatenated gzip members. A subsystem under D5. It allocates only in
/// [`new`](Self::new) (the inflater's state).
// A7.2: cold struct (one per stream), size guard waived
pub struct GzipReader {
    inf: Inflater,
    limits: InflateLimits,
    state: State,
    flags: u8,
    /// CRC-32 of the header bytes so far, for FHCRC.
    hdr_crc: u32,
    field: u16,
    crc: u32,
    isize: u32,
    trailer: [u8; 8],
    members: u64,
    /// Output of the members already complete.
    done_out: u64,
    total_in: u64,
}

impl GzipReader {
    /// `limits.max_output` caps the output of the whole stream, all members together; the ratio
    /// limit applies to each member.
    pub fn new(limits: InflateLimits) -> GzipReader {
        GzipReader {
            inf: Inflater::new(limits),
            limits,
            state: State::Id1,
            flags: 0,
            hdr_crc: 0,
            field: 0,
            crc: 0,
            isize: 0,
            trailer: [0; 8],
            members: 0,
            done_out: 0,
            total_in: 0,
        }
    }

    /// Feeds `input`; every byte of it is taken. Decoded bytes go to `out` as they are made.
    /// After an error the reader is finished: further pushes return [`GzipError::Failed`].
    pub fn push<E>(
        &mut self,
        input: &[u8],
        out: &mut dyn FnMut(&[u8]) -> Result<(), E>,
    ) -> Result<(), GzipError<E>> {
        let r = self.run(input, out);
        if r.is_err() {
            self.state = State::Failed;
        }
        r
    }

    fn run<E>(
        &mut self,
        input: &[u8],
        out: &mut dyn FnMut(&[u8]) -> Result<(), E>,
    ) -> Result<(), GzipError<E>> {
        let mut pos = 0usize;
        while pos < input.len() {
            let rest = input.get(pos..).unwrap_or(&[]);
            match self.state {
                State::Failed => return Err(GzipError::Failed),
                State::Data => {
                    let (crc, isize) = (&mut self.crc, &mut self.isize);
                    let mut sink = |b: &[u8]| -> Result<(), E> {
                        *crc = crc32_update(*crc, b);
                        // ISIZE is the length modulo 2^32 by definition, so truncation is intended.
                        *isize = isize.wrapping_add(b.len() as u32);
                        out(b)
                    };
                    let c = self.inf.push(rest, &mut sink).map_err(|e| match e {
                        StreamError::Inflate(e) => GzipError::Data(e),
                        StreamError::Sink(e) => GzipError::Sink(e),
                    })?;
                    pos += c.input;
                    self.total_in += c.input as u64;
                    if c.finished {
                        self.state = State::Trailer(0);
                    }
                }
                State::Trailer(n) => {
                    let want = 8 - usize::from(n);
                    let take = want.min(rest.len());
                    let from = usize::from(n);
                    if let Some(dst) = self.trailer.get_mut(from..from + take) {
                        dst.copy_from_slice(rest.get(..take).unwrap_or(&[]));
                    }
                    pos += take;
                    self.total_in += take as u64;
                    if take < want {
                        self.state = State::Trailer(n + take as u8); // take < want <= 8
                    } else {
                        self.check_trailer()?;
                    }
                }
                _ => {
                    let b = rest.first().copied().unwrap_or(0);
                    pos += 1;
                    self.total_in += 1;
                    self.header_byte(b)?;
                }
            }
        }
        Ok(())
    }

    fn check_trailer<E>(&mut self) -> Result<(), GzipError<E>> {
        let (c, s) = self.trailer.split_at(4);
        let crc = u32::from_le_bytes(c.try_into().unwrap_or([0; 4]));
        let size = u32::from_le_bytes(s.try_into().unwrap_or([0; 4]));
        if crc != self.crc {
            return Err(GzipError::CrcMismatch {
                expected: crc,
                actual: self.crc,
            });
        }
        if size != self.isize {
            return Err(GzipError::SizeMismatch {
                expected: size,
                actual: self.isize,
            });
        }
        self.members += 1;
        self.done_out += self.inf.total_out();
        self.state = State::Id1;
        Ok(())
    }

    /// One header byte; moves to the next header field, or to the data when the header is done.
    fn header_byte<E>(&mut self, b: u8) -> Result<(), GzipError<E>> {
        if self.state == State::Id1 {
            self.hdr_crc = 0;
        }
        if !matches!(self.state, State::Hcrc(_)) {
            self.hdr_crc = crc32_update(self.hdr_crc, &[b]);
        }
        self.state = match self.state {
            State::Id1 if b == 0x1f => State::Id2,
            State::Id2 if b == 0x8b => State::Method,
            State::Id1 | State::Id2 => return Err(GzipError::BadMagic),
            State::Method if b == 8 => State::Flags,
            State::Method => return Err(GzipError::UnsupportedMethod(b)),
            State::Flags if b & FLAG_RESERVED != 0 => return Err(GzipError::ReservedFlags(b)),
            State::Flags => {
                self.flags = b;
                State::Fixed(6)
            }
            State::Fixed(n) if n > 1 => State::Fixed(n - 1),
            State::Fixed(_) => self.after_fixed(),
            State::ExtraLen(0) => {
                self.field = u16::from(b);
                State::ExtraLen(1)
            }
            State::ExtraLen(_) => {
                let len = self.field | u16::from(b) << 8;
                if len == 0 {
                    self.after_extra()
                } else {
                    State::ExtraSkip(len)
                }
            }
            State::ExtraSkip(n) if n > 1 => State::ExtraSkip(n - 1),
            State::ExtraSkip(_) => self.after_extra(),
            State::Name(_) | State::Comment(_) if b == 0 => {
                if matches!(self.state, State::Name(_)) {
                    self.after_name()
                } else {
                    self.after_comment()
                }
            }
            State::Name(n) | State::Comment(n) if n >= MAX_HEADER_FIELD => {
                return Err(GzipError::HeaderFieldTooLong)
            }
            State::Name(n) => State::Name(n + 1),
            State::Comment(n) => State::Comment(n + 1),
            State::Hcrc(0) => {
                self.field = u16::from(b);
                State::Hcrc(1)
            }
            State::Hcrc(_) => {
                let expected = self.field | u16::from(b) << 8;
                let actual = (self.hdr_crc & 0xffff) as u16;
                if expected != actual {
                    return Err(GzipError::HeaderCrc { expected, actual });
                }
                self.start_data()
            }
            s => s,
        };
        Ok(())
    }

    fn after_fixed(&mut self) -> State {
        if self.flags & FLAG_EXTRA != 0 {
            State::ExtraLen(0)
        } else {
            self.after_extra()
        }
    }

    fn after_extra(&mut self) -> State {
        if self.flags & FLAG_NAME != 0 {
            State::Name(0)
        } else {
            self.after_name()
        }
    }

    fn after_name(&mut self) -> State {
        if self.flags & FLAG_COMMENT != 0 {
            State::Comment(0)
        } else {
            self.after_comment()
        }
    }

    fn after_comment(&mut self) -> State {
        if self.flags & FLAG_HCRC != 0 {
            State::Hcrc(0)
        } else {
            self.start_data()
        }
    }

    fn start_data(&mut self) -> State {
        self.crc = 0;
        self.isize = 0;
        self.inf.reset(InflateLimits {
            max_output: self.limits.max_output.saturating_sub(self.done_out),
            ..self.limits
        });
        State::Data
    }

    /// Ends the stream and says what it amounted to. Call after the last push. A stream that
    /// ended inside a member (or was empty) comes back with `truncated` set; the bytes recovered
    /// were already delivered. After an error from [`push`](Self::push) the error was the report
    /// and `truncated` is `None`.
    pub fn finish(&self) -> GzipSummary {
        let tail = self.inf.total_out();
        let (stage, tail_bytes, flushed_bytes) = match self.state {
            State::Failed => (None, 0, 0),
            State::Id1 if self.total_in > 0 => (None, 0, 0),
            State::Id1 => (Some(TruncStage::Header), 0, 0),
            State::Data => (Some(TruncStage::Data), tail, self.inf.sync_out()),
            State::Trailer(_) => (Some(TruncStage::Trailer), tail, tail),
            _ => (Some(TruncStage::Header), 0, 0),
        };
        GzipSummary {
            members: self.members,
            bytes_out: self.done_out + if stage.is_some() { tail_bytes } else { 0 },
            truncated: stage.map(|stage| Truncated {
                stage,
                tail_bytes,
                flushed_bytes,
            }),
        }
    }
}
