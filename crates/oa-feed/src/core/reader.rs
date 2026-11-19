//! Streaming frame decoder with resynchronisation. Core (pure, B7): no I/O, no clock, and no
//! allocation; it works in a buffer the caller supplies once.
//!
//! Bytes go in through [`FrameReader::push`] in any chunking; [`FrameReader::next`] yields
//! complete validated frames or errors. It reads `OAF1` frames and `OAF2` frames (which also carry
//! the recorder's monotonic time). After an error the reader scans for the next magic and
//! discards what lies between, counting every discarded byte (INTERFACES IF-R5: refuse and flag,
//! never pass on). Errors found while hunting are not reported again; the skipped-byte count
//! covers them. A frame whose own bytes contain `OAF1` inside a corrupted region can cause one
//! further reported error at that later offset, which is honest: that is where a frame failed.
//!
//! Capacity (C2): the buffer must hold one maximum frame ([`MAX_FRAME_LEN`]). A frame is decided
//! within that many bytes, so the reader can always make progress; nothing is ever dropped for
//! lack of room, because `push` accepts only what fits and says how much it took.

use oa_zip::crc32;

use crate::frame::{
    ErrorKind, Frame, FrameError, Kind, HEADER_LEN, HEADER_LEN_V2, MAGIC, MAGIC_V2, MAX_FRAME_LEN,
    MAX_PAYLOAD, TRAILER_LEN,
};

/// The supplied buffer cannot hold one maximum-size frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferTooSmall {
    /// Length of the buffer supplied.
    pub have: usize,
    /// Minimum length needed ([`MAX_FRAME_LEN`]).
    pub need: usize,
}

/// A resynchronising frame decoder over a caller-supplied buffer (a subsystem under D5).
pub struct FrameReader<'b> {
    buf: &'b mut [u8],
    start: usize,
    end: usize,
    /// Stream offset of `buf[start]`.
    base: u64,
    resync: bool,
    skipped_total: u64,
    skipped_pending: u64,
}

impl<'b> FrameReader<'b> {
    /// Wraps `buf`, which must be at least [`MAX_FRAME_LEN`] bytes.
    pub fn new(buf: &'b mut [u8]) -> Result<Self, BufferTooSmall> {
        if buf.len() < MAX_FRAME_LEN {
            return Err(BufferTooSmall {
                have: buf.len(),
                need: MAX_FRAME_LEN,
            });
        }
        Ok(FrameReader {
            buf,
            start: 0,
            end: 0,
            base: 0,
            resync: false,
            skipped_total: 0,
            skipped_pending: 0,
        })
    }

    /// Copies as much of `data` as fits and returns the count taken. If it is less than
    /// `data.len()`, call [`next`](Self::next) until it returns `None`, then push the rest.
    pub fn push(&mut self, data: &[u8]) -> usize {
        if self.start == self.end {
            self.start = 0;
            self.end = 0;
        }
        if self.start > 0 && self.buf.len() - self.end < data.len() {
            self.buf.copy_within(self.start..self.end, 0);
            self.end -= self.start;
            self.start = 0;
        }
        let n = (self.buf.len() - self.end).min(data.len());
        if let (Some(dst), Some(src)) = (self.buf.get_mut(self.end..self.end + n), data.get(..n)) {
            dst.copy_from_slice(src);
            self.end += n;
            n
        } else {
            0
        }
    }

    /// The next validated frame or error, or `None` when more bytes are needed.
    #[allow(clippy::should_implement_trait)] // borrows the reader, so it cannot be an Iterator
    pub fn next(&mut self) -> Option<Result<Frame<'_>, FrameError>> {
        loop {
            let avail = self.end - self.start;
            if self.resync {
                let data = self.buf.get(self.start..self.end)?;
                match data
                    .windows(MAGIC.len())
                    .position(|w| w == MAGIC || w == MAGIC_V2)
                {
                    Some(i) => {
                        self.discard(i);
                        self.resync = false;
                    }
                    None => {
                        // Keep a possible partial magic at the tail.
                        self.discard(avail.saturating_sub(MAGIC.len() - 1));
                        return None;
                    }
                }
                continue;
            }
            if avail < MAGIC.len() {
                return None;
            }
            let data = self.buf.get(self.start..self.end)?;
            let magic = [*data.first()?, *data.get(1)?, *data.get(2)?, *data.get(3)?];
            let v2 = magic == MAGIC_V2;
            if magic != MAGIC && !v2 {
                return Some(Err(self.fail(ErrorKind::BadMagic { found: magic })));
            }
            let header_len = if v2 { HEADER_LEN_V2 } else { HEADER_LEN };
            if avail < header_len {
                return None;
            }
            let kind_byte = *data.get(4)?;
            let kind = match kind_byte {
                1 => Kind::Message,
                2 => Kind::Reject,
                3 => Kind::Status,
                k => return Some(Err(self.fail(ErrorKind::UnknownKind { kind: k }))),
            };
            let reserved = u16::from_le_bytes([*data.get(6)?, *data.get(7)?]);
            if reserved != 0 {
                return Some(Err(self.fail(ErrorKind::Reserved { value: reserved })));
            }
            let len = le_u32(data, header_len - 4)?;
            if len as usize > MAX_PAYLOAD {
                return Some(Err(self.fail(ErrorKind::Oversize { len })));
            }
            let total = header_len + len as usize + TRAILER_LEN;
            if avail < total {
                return None;
            }
            let raw_len = total;
            let expected = le_u32(data, raw_len - TRAILER_LEN)?;
            let actual = crc32(data.get(..raw_len - TRAILER_LEN)?);
            if expected != actual {
                return Some(Err(self.fail(ErrorKind::Crc { expected, actual })));
            }
            let flags = *data.get(5)?;
            let seq = le_u64(data, 8)?;
            let recv_unix_ns = le_u64(data, 16)? as i64;
            let jms_timestamp_ms = le_u64(data, 24)? as i64;
            let mono_ns = if v2 { Some(le_u64(data, 32)?) } else { None };
            let offset = self.base;
            let skipped_before = std::mem::take(&mut self.skipped_pending);
            let s = self.start;
            self.start += total;
            self.base += total as u64;
            let raw = self.buf.get(s..s + total)?;
            let payload = raw.get(header_len..total - TRAILER_LEN)?;
            return Some(Ok(Frame::new(
                seq,
                recv_unix_ns,
                jms_timestamp_ms,
                mono_ns,
                offset,
                skipped_before,
                payload,
                raw,
                kind,
                flags,
            )));
        }
    }

    /// Call once the stream has ended. A partial frame left in the buffer is reported as
    /// [`ErrorKind::Truncated`]; leftover bytes in resynchronisation are only counted as
    /// skipped (the error that caused the hunt was already reported).
    pub fn finish(&mut self) -> Option<FrameError> {
        let avail = self.end - self.start;
        if avail == 0 {
            return None;
        }
        let err = if self.resync {
            None
        } else {
            Some(FrameError {
                offset: self.base,
                kind: ErrorKind::Truncated { have: avail as u32 },
            })
        };
        self.discard(avail);
        err
    }

    /// Every byte discarded so far while resynchronising or at a truncated end.
    pub fn skipped_total(&self) -> u64 {
        self.skipped_total
    }

    /// Stream offset of the next unconsumed byte.
    pub fn offset(&self) -> u64 {
        self.base
    }

    /// Records an error at the current frame start, drops its first byte and starts hunting.
    fn fail(&mut self, kind: ErrorKind) -> FrameError {
        let err = FrameError {
            offset: self.base,
            kind,
        };
        self.discard(1);
        self.resync = true;
        err
    }

    fn discard(&mut self, n: usize) {
        self.start += n;
        self.base += n as u64;
        self.skipped_total += n as u64;
        self.skipped_pending += n as u64;
    }
}

fn le_u32(d: &[u8], at: usize) -> Option<u32> {
    let b: [u8; 4] = d.get(at..at + 4)?.try_into().ok()?;
    Some(u32::from_le_bytes(b))
}

fn le_u64(d: &[u8], at: usize) -> Option<u64> {
    let b: [u8; 8] = d.get(at..at + 8)?.try_into().ok()?;
    Some(u64::from_le_bytes(b))
}
