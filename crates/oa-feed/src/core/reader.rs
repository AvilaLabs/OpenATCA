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

use crate::core::crc32::crc32;

use crate::core::frame::{
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

/// Maximum per-call work used by the compatibility `next`/`push` profiles. Runtime readers
/// with larger buffers should pass an explicit budget derived from their declared capacity.
pub const DEFAULT_FRAME_WORK_LIMIT: u64 = MAX_FRAME_LEN as u64;

/// Per-call work allowance. Units are exact byte operations, not time estimates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameWorkBudget {
    /// Candidate start positions inspected during resynchronisation.
    pub scan_positions: u64,
    /// Frame bytes read by CRC calculation.
    pub crc_bytes: u64,
    /// Input bytes copied into the reader buffer.
    pub copied_bytes: u64,
    /// Existing buffered bytes moved during compaction.
    pub compacted_bytes: u64,
}

impl FrameWorkBudget {
    const fn default_profile(capacity: usize) -> Self {
        let limit = if capacity as u64 > DEFAULT_FRAME_WORK_LIMIT {
            DEFAULT_FRAME_WORK_LIMIT
        } else {
            capacity as u64
        };
        Self {
            scan_positions: limit,
            crc_bytes: DEFAULT_FRAME_WORK_LIMIT,
            copied_bytes: limit,
            compacted_bytes: limit,
        }
    }

    /// A finite allowance sized to this reader's caller-owned buffer and one maximum frame.
    pub const fn for_capacity(capacity: usize) -> Self {
        let capacity = capacity as u64;
        Self {
            scan_positions: capacity,
            crc_bytes: MAX_FRAME_LEN as u64,
            copied_bytes: capacity,
            compacted_bytes: capacity,
        }
    }
}

/// Cumulative measured work. Counters count positions/bytes actually examined or copied.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct FrameWorkStats {
    pub scan_positions: u64,
    pub crc_bytes: u64,
    pub copied_bytes: u64,
    pub compacted_bytes: u64,
}
// Budget: cumulative decoder work counters inspected on feed operations; four u64 counters.
const _: () = assert!(core::mem::size_of::<FrameWorkStats>() == 32);

/// Which named allowance rejected a call before its corresponding work began.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameWorkKind {
    ScanPositions,
    CrcBytes,
    CopiedBytes,
    CompactedBytes,
    CounterOverflow,
}

/// Recoverable refusal. The reader retains ownership of every buffered byte; retry with a
/// sufficient allowance or cancel the stream explicitly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameWorkRefusal {
    pub kind: FrameWorkKind,
    pub required: u64,
    pub available: u64,
}

/// Whether a fresh budget of the given profile can resolve this refusal. Counter overflow is
/// permanent for this reader because its cumulative exact work statistics cannot advance.
pub fn frame_work_refusal_retryable(
    refusal: FrameWorkRefusal,
    fresh_budget: FrameWorkBudget,
) -> bool {
    let available = match refusal.kind {
        FrameWorkKind::ScanPositions => fresh_budget.scan_positions,
        FrameWorkKind::CrcBytes => fresh_budget.crc_bytes,
        FrameWorkKind::CopiedBytes => fresh_budget.copied_bytes,
        FrameWorkKind::CompactedBytes => fresh_budget.compacted_bytes,
        FrameWorkKind::CounterOverflow => return false,
    };
    refusal.required <= available
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
    work: FrameWorkStats,
    refusal: Option<FrameWorkRefusal>,
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
            work: FrameWorkStats::default(),
            refusal: None,
        })
    }

    /// Copies as much of `data` as fits under the finite compatibility profile and returns the
    /// count taken. A zero caused by refusal is exposed by [`last_work_refusal`](Self::last_work_refusal);
    /// use [`push_with_budget`](Self::push_with_budget) when the caller must handle refusal directly.
    pub fn push(&mut self, data: &[u8]) -> usize {
        match self.push_with_budget(data, FrameWorkBudget::default_profile(self.buf.len())) {
            Ok(taken) => taken,
            Err(refusal) => {
                self.refusal = Some(refusal);
                0
            }
        }
    }

    /// Bounded counterpart to [`push`](Self::push). A refusal is atomic: no bytes are
    /// compacted or copied, so the caller can retry with a larger allowance.
    pub fn push_with_budget(
        &mut self,
        data: &[u8],
        budget: FrameWorkBudget,
    ) -> Result<usize, FrameWorkRefusal> {
        let mut remaining = budget;
        self.push_with_remaining_budget(data, &mut remaining)
    }

    /// Bounded push that consumes allowance across a caller's larger work unit, such as one
    /// live input chunk. The exact copy and compaction costs are charged before any buffer edit.
    pub fn push_with_remaining_budget(
        &mut self,
        data: &[u8],
        budget: &mut FrameWorkBudget,
    ) -> Result<usize, FrameWorkRefusal> {
        let start = if self.start == self.end {
            0
        } else {
            self.start
        };
        let end = if self.start == self.end { 0 } else { self.end };
        let compact = start > 0 && self.buf.len() - end < data.len();
        let compacted = if compact { end - start } else { 0 };
        let free = if compact {
            self.buf.len() - (end - start)
        } else {
            self.buf.len() - end
        };
        let taken = free.min(data.len());
        check_work(
            compacted as u64,
            budget.compacted_bytes,
            FrameWorkKind::CompactedBytes,
        )?;
        check_work(
            taken as u64,
            budget.copied_bytes,
            FrameWorkKind::CopiedBytes,
        )?;
        check_counter(self.work.compacted_bytes, compacted as u64)?;
        check_counter(self.work.copied_bytes, taken as u64)?;
        budget.compacted_bytes -= compacted as u64;
        budget.copied_bytes -= taken as u64;
        self.refusal = None;
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
            self.work.compacted_bytes = self.work.compacted_bytes.saturating_add(compacted as u64);
            self.work.copied_bytes = self.work.copied_bytes.saturating_add(n as u64);
            Ok(n)
        } else {
            Ok(0)
        }
    }

    /// The next validated frame or input error. `None` means more bytes are needed unless
    /// [`last_work_refusal`](Self::last_work_refusal) reports that the finite compatibility
    /// allowance was insufficient; use [`next_with_budget`](Self::next_with_budget) to handle
    /// that outcome directly.
    #[allow(clippy::should_implement_trait)] // borrows the reader, so it cannot be an Iterator
    pub fn next(&mut self) -> Option<Result<Frame<'_>, FrameError>> {
        self.next_with_budget(FrameWorkBudget::default_profile(self.buf.len()))
            .ok()
            .flatten()
    }

    /// Bounded counterpart to [`next`](Self::next). Work refusal is distinct from “need more
    /// bytes". It leaves bytes at the refused operation's start for an explicit retry; any
    /// completed earlier resynchronisation work in the same call remains accounted.
    pub fn next_with_budget(
        &mut self,
        budget: FrameWorkBudget,
    ) -> Result<Option<Result<Frame<'_>, FrameError>>, FrameWorkRefusal> {
        let mut remaining = budget;
        self.next_with_remaining_budget(&mut remaining)
    }

    /// Bounded next that consumes allowance across repeated calls in one caller-defined work
    /// unit. Refusals are distinct from “need more bytes” and retain bytes at the refused
    /// operation's start; any completed earlier resynchronisation work remains accounted.
    pub fn next_with_remaining_budget(
        &mut self,
        budget: &mut FrameWorkBudget,
    ) -> Result<Option<Result<Frame<'_>, FrameError>>, FrameWorkRefusal> {
        self.refusal = None;
        loop {
            let avail = self.end - self.start;
            if self.resync {
                let Some(data) = self.buf.get(self.start..self.end) else {
                    return Ok(None);
                };
                let scan_allowance = avail.saturating_sub(MAGIC.len() - 1) as u64;
                if let Err(refusal) = check_work(
                    scan_allowance,
                    budget.scan_positions,
                    FrameWorkKind::ScanPositions,
                ) {
                    self.refusal = Some(refusal);
                    return Err(refusal);
                }
                if let Err(refusal) = check_counter(self.work.scan_positions, scan_allowance) {
                    self.refusal = Some(refusal);
                    return Err(refusal);
                }
                // Reserve the scan's worst case before inspecting the slice. If a magic is found
                // early, refund the positions that were not visited after the search completes.
                budget.scan_positions -= scan_allowance;
                match data
                    .windows(MAGIC.len())
                    .position(|w| w == MAGIC || w == MAGIC_V2)
                {
                    Some(i) => {
                        let scanned = i as u64 + 1;
                        budget.scan_positions += scan_allowance - scanned;
                        self.work.scan_positions = self.work.scan_positions.saturating_add(scanned);
                        self.discard(i);
                        self.resync = false;
                    }
                    None => {
                        self.work.scan_positions =
                            self.work.scan_positions.saturating_add(scan_allowance);
                        // Keep a possible partial magic at the tail.
                        self.discard(avail.saturating_sub(MAGIC.len() - 1));
                        return Ok(None);
                    }
                }
                continue;
            }
            if avail < MAGIC.len() {
                return Ok(None);
            }
            let Some(data) = self.buf.get(self.start..self.end) else {
                return Ok(None);
            };
            let (Some(a), Some(b), Some(c), Some(d)) =
                (data.first(), data.get(1), data.get(2), data.get(3))
            else {
                return Ok(None);
            };
            let magic = [*a, *b, *c, *d];
            let v2 = magic == MAGIC_V2;
            if magic != MAGIC && !v2 {
                return Ok(Some(Err(self.fail(ErrorKind::BadMagic { found: magic }))));
            }
            let header_len = if v2 { HEADER_LEN_V2 } else { HEADER_LEN };
            if avail < header_len {
                return Ok(None);
            }
            let Some(kind_byte) = data.get(4).copied() else {
                return Ok(None);
            };
            let kind = match kind_byte {
                1 => Kind::Message,
                2 => Kind::Reject,
                3 => Kind::Status,
                k => return Ok(Some(Err(self.fail(ErrorKind::UnknownKind { kind: k })))),
            };
            let (Some(r0), Some(r1)) = (data.get(6), data.get(7)) else {
                return Ok(None);
            };
            let reserved = u16::from_le_bytes([*r0, *r1]);
            if reserved != 0 {
                return Ok(Some(
                    Err(self.fail(ErrorKind::Reserved { value: reserved })),
                ));
            }
            let Some(len) = le_u32(data, header_len - 4) else {
                return Ok(None);
            };
            if len as usize > MAX_PAYLOAD {
                return Ok(Some(Err(self.fail(ErrorKind::Oversize { len }))));
            }
            let total = header_len + len as usize + TRAILER_LEN;
            if avail < total {
                return Ok(None);
            }
            let raw_len = total;
            let Some(expected) = le_u32(data, raw_len - TRAILER_LEN) else {
                return Ok(None);
            };
            let crc_work = (raw_len - TRAILER_LEN) as u64;
            if let Err(refusal) = check_work(crc_work, budget.crc_bytes, FrameWorkKind::CrcBytes) {
                self.refusal = Some(refusal);
                return Err(refusal);
            }
            if let Err(refusal) = check_counter(self.work.crc_bytes, crc_work) {
                self.refusal = Some(refusal);
                return Err(refusal);
            }
            budget.crc_bytes -= crc_work;
            let Some(crc_input) = data.get(..raw_len - TRAILER_LEN) else {
                return Ok(None);
            };
            let actual = crc32(crc_input);
            self.work.crc_bytes = self.work.crc_bytes.saturating_add(crc_work);
            if expected != actual {
                return Ok(Some(Err(self.fail(ErrorKind::Crc { expected, actual }))));
            }
            let Some(flags) = data.get(5).copied() else {
                return Ok(None);
            };
            let Some(seq) = le_u64(data, 8) else {
                return Ok(None);
            };
            let Some(recv_unix_ns) = le_u64(data, 16) else {
                return Ok(None);
            };
            let Some(jms_timestamp_ms) = le_u64(data, 24) else {
                return Ok(None);
            };
            let mono_ns = if v2 {
                let Some(value) = le_u64(data, 32) else {
                    return Ok(None);
                };
                Some(value)
            } else {
                None
            };
            let offset = self.base;
            let skipped_before = std::mem::take(&mut self.skipped_pending);
            let s = self.start;
            self.start += total;
            self.base += total as u64;
            let Some(raw) = self.buf.get(s..s + total) else {
                return Ok(None);
            };
            let Some(payload) = raw.get(header_len..total - TRAILER_LEN) else {
                return Ok(None);
            };
            return Ok(Some(Ok(Frame::new(
                seq,
                recv_unix_ns as i64,
                jms_timestamp_ms as i64,
                mono_ns,
                offset,
                skipped_before,
                payload,
                raw,
                kind,
                flags,
            ))));
        }
    }

    /// Exact cumulative work counters for this reader.
    pub fn work_stats(&self) -> FrameWorkStats {
        self.work
    }

    /// Caller-owned buffer capacity that bounds scan, copy, and compaction work.
    pub fn capacity(&self) -> usize {
        self.buf.len()
    }

    /// Bytes still owned by the reader buffer, including partial frames and resync input.
    pub fn buffered_len(&self) -> usize {
        self.end - self.start
    }

    /// Last refusal observed by a compatibility wrapper. Checked methods return refusals
    /// directly and do not require this side channel.
    pub fn last_work_refusal(&self) -> Option<FrameWorkRefusal> {
        self.refusal
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

fn check_work(required: u64, available: u64, kind: FrameWorkKind) -> Result<(), FrameWorkRefusal> {
    if required > available {
        Err(FrameWorkRefusal {
            kind,
            required,
            available,
        })
    } else {
        Ok(())
    }
}

fn check_counter(current: u64, additional: u64) -> Result<(), FrameWorkRefusal> {
    if current.checked_add(additional).is_none() {
        Err(FrameWorkRefusal {
            kind: FrameWorkKind::CounterOverflow,
            required: additional,
            available: u64::MAX - current,
        })
    } else {
        Ok(())
    }
}

fn le_u64(d: &[u8], at: usize) -> Option<u64> {
    let b: [u8; 8] = d.get(at..at + 8)?.try_into().ok()?;
    Some(u64::from_le_bytes(b))
}
