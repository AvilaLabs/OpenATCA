//! OAF1 and OAF2 frame layout, decoded-frame and error types, and a tiny encoder. Core (pure, B7).
//!
//! Layout (all integers little-endian): magic `OAF1` (4), kind u8, flags u8, reserved u16 (0),
//! seq u64, recv_unix_ns i64, jms_timestamp_ms i64, payload_len u32, payload, crc32 u32 over
//! every preceding byte of the frame. An `OAF2` frame is the same with a u64 `mono_ns` (the
//! recorder's monotonic time since its start) between `jms_timestamp_ms` and `payload_len`. See
//! `docs/FEED_FRAMES.md`.

use std::fmt;

use crate::core::crc32::crc32;

/// The four magic bytes that start every frame the bridge writes.
pub const MAGIC: [u8; 4] = *b"OAF1";
/// The magic of a frame that also carries the recorder's monotonic time.
pub const MAGIC_V2: [u8; 4] = *b"OAF2";
/// Bytes before the payload.
pub const HEADER_LEN: usize = 36;
/// Bytes before the payload of an `OAF2` frame.
pub const HEADER_LEN_V2: usize = 44;
/// Bytes after the payload (the CRC-32).
pub const TRAILER_LEN: usize = 4;
/// Largest payload a frame may carry (IF-R1: bounded work per input). The bridge refuses to
/// frame a message above this; the reader refuses a frame that claims more.
pub const MAX_PAYLOAD: usize = 4 * 1024 * 1024;
/// Largest whole frame.
pub const MAX_FRAME_LEN: usize = HEADER_LEN_V2 + MAX_PAYLOAD + TRAILER_LEN;

/// Flag bit 0: the JMS message was a redelivery.
pub const FLAG_REDELIVERED: u8 = 1;
/// Flag bit 1: the payload came from a BytesMessage (otherwise TextMessage UTF-8).
pub const FLAG_BYTES: u8 = 2;

/// What a frame carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    /// A SWIM message payload.
    Message = 1,
    /// A message the bridge refused to frame; the payload says why.
    Reject = 2,
    /// A connection state change (`state=` and `delay_ms=` lines).
    Status = 3,
}

/// One validated frame, borrowed from the reader's buffer; valid until the next reader call.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct Frame<'a> {
    /// Bridge's own counter, 0 at bridge start, +1 per frame.
    pub seq: u64,
    /// Bridge wall clock at receipt, nanoseconds since the Unix epoch.
    pub recv_unix_ns: i64,
    /// JMS timestamp in milliseconds; 0 if the broker sent none.
    pub jms_timestamp_ms: i64,
    /// The recorder's monotonic time at receipt, ns since its start; meaningful only if
    /// `has_mono` (read it with [`Frame::mono_ns`]).
    mono: u64,
    /// Byte offset of this frame in the stream.
    pub offset: u64,
    /// Bytes the reader discarded immediately before this frame (resynchronisation).
    pub skipped_before: u64,
    /// The payload bytes.
    pub payload: &'a [u8],
    /// The whole frame exactly as received, for verbatim recording.
    pub raw: &'a [u8],
    /// Frame kind.
    pub kind: Kind,
    /// Flag bits ([`FLAG_REDELIVERED`], [`FLAG_BYTES`]); unknown bits are carried, not refused.
    pub flags: u8,
    /// Whether the frame carries a monotonic time (an `OAF2` frame).
    has_mono: bool,
}
// Budget: six 8-byte integers, two 16-byte slices (80), three bytes, 5 bytes tail padding = 88. A transient
// view built per frame, not held in quantity; the guard still pins the layout (A7).
const _: () = assert!(core::mem::size_of::<Frame<'static>>() == 88);

impl Frame<'_> {
    /// The recorder's monotonic time at receipt, ns since the recorder started, or `None` for an
    /// `OAF1` frame (a recording made before the field existed, or the bridge's own output).
    pub fn mono_ns(&self) -> Option<u64> {
        self.has_mono.then_some(self.mono)
    }

    /// Builds the view of a decoded frame; the reader's constructor.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new<'a>(
        seq: u64,
        recv_unix_ns: i64,
        jms_timestamp_ms: i64,
        mono_ns: Option<u64>,
        offset: u64,
        skipped_before: u64,
        payload: &'a [u8],
        raw: &'a [u8],
        kind: Kind,
        flags: u8,
    ) -> Frame<'a> {
        Frame {
            seq,
            recv_unix_ns,
            jms_timestamp_ms,
            mono: mono_ns.unwrap_or(0),
            offset,
            skipped_before,
            payload,
            raw,
            kind,
            flags,
            has_mono: mono_ns.is_some(),
        }
    }
}

/// Why a frame was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// The bytes at the frame start are not `OAF1`.
    BadMagic {
        /// The four bytes found instead.
        found: [u8; 4],
    },
    /// The kind byte is not 1, 2 or 3.
    UnknownKind {
        /// The kind byte found.
        kind: u8,
    },
    /// The length field exceeds [`MAX_PAYLOAD`].
    Oversize {
        /// The claimed payload length.
        len: u32,
    },
    /// The CRC-32 does not match.
    Crc {
        /// The CRC carried in the frame.
        expected: u32,
        /// The CRC computed over the frame bytes.
        actual: u32,
    },
    /// The reserved header bytes (offsets 6 and 7) are not zero: a frame variant this reader does
    /// not know.
    Reserved {
        /// The reserved field as found.
        value: u16,
    },
    /// The stream ended inside a frame.
    Truncated {
        /// Bytes of the partial frame that were present.
        have: u32,
    },
}

/// A refused frame, with the byte offset in the stream where it starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameError {
    /// Byte offset of the start of the refused frame.
    pub offset: u64,
    /// What was wrong.
    pub kind: ErrorKind,
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "frame error at byte offset {}: ", self.offset)?;
        match self.kind {
            ErrorKind::BadMagic { found } => write!(f, "bad magic {found:02x?}, expected OAF1"),
            ErrorKind::UnknownKind { kind } => write!(f, "unknown frame kind {kind}"),
            ErrorKind::Oversize { len } => {
                write!(
                    f,
                    "payload length {len} exceeds the {MAX_PAYLOAD} byte limit"
                )
            }
            ErrorKind::Crc { expected, actual } => {
                write!(
                    f,
                    "crc32 mismatch: frame says {expected:08x}, computed {actual:08x}"
                )
            }
            ErrorKind::Reserved { value } => {
                write!(f, "reserved header bytes are {value:#06x}, expected zero")
            }
            ErrorKind::Truncated { have } => {
                write!(f, "stream ends inside a frame ({have} bytes present)")
            }
        }
    }
}

impl std::error::Error for FrameError {}

/// Encodes one frame into `out`; returns its length, or `None` if the payload exceeds
/// [`MAX_PAYLOAD`] or `out` is too small. Used by tests, emulators and fixtures; the bridge has
/// its own encoder in Java.
pub fn encode_frame(
    out: &mut [u8],
    kind: u8,
    flags: u8,
    seq: u64,
    recv_unix_ns: i64,
    jms_timestamp_ms: i64,
    payload: &[u8],
) -> Option<usize> {
    encode_any(
        out,
        kind,
        flags,
        seq,
        recv_unix_ns,
        jms_timestamp_ms,
        None,
        payload,
    )
}

/// As [`encode_frame`], writing an `OAF2` frame that carries `mono_ns`, the recorder's monotonic
/// time at receipt. The recorder's shell uses it to store each frame it records.
#[allow(clippy::too_many_arguments)]
pub fn encode_frame_mono(
    out: &mut [u8],
    kind: u8,
    flags: u8,
    seq: u64,
    recv_unix_ns: i64,
    jms_timestamp_ms: i64,
    mono_ns: u64,
    payload: &[u8],
) -> Option<usize> {
    encode_any(
        out,
        kind,
        flags,
        seq,
        recv_unix_ns,
        jms_timestamp_ms,
        Some(mono_ns),
        payload,
    )
}

#[allow(clippy::too_many_arguments)]
fn encode_any(
    out: &mut [u8],
    kind: u8,
    flags: u8,
    seq: u64,
    recv_unix_ns: i64,
    jms_timestamp_ms: i64,
    mono_ns: Option<u64>,
    payload: &[u8],
) -> Option<usize> {
    if payload.len() > MAX_PAYLOAD {
        return None;
    }
    let header_len = if mono_ns.is_some() {
        HEADER_LEN_V2
    } else {
        HEADER_LEN
    };
    let body = header_len + payload.len();
    let total = body + TRAILER_LEN;
    let dst = out.get_mut(..total)?;
    let mut head = [0u8; HEADER_LEN_V2];
    let kf = [kind, flags];
    let reserved = 0u16.to_le_bytes();
    let (seq_b, recv_b, jms_b) = (
        seq.to_le_bytes(),
        recv_unix_ns.to_le_bytes(),
        jms_timestamp_ms.to_le_bytes(),
    );
    let mono_b = mono_ns.unwrap_or(0).to_le_bytes();
    let len_b = (payload.len() as u32).to_le_bytes();
    let magic: &[u8] = if mono_ns.is_some() { &MAGIC_V2 } else { &MAGIC };
    let mono_part: &[u8] = if mono_ns.is_some() { &mono_b } else { &[] };
    let parts: [&[u8]; 8] = [
        magic, &kf, &reserved, &seq_b, &recv_b, &jms_b, mono_part, &len_b,
    ];
    let mut at = 0usize;
    for part in parts {
        head.get_mut(at..at + part.len())?.copy_from_slice(part);
        at += part.len();
    }
    dst.get_mut(..header_len)?
        .copy_from_slice(head.get(..header_len)?);
    dst.get_mut(header_len..body)?.copy_from_slice(payload);
    let crc = crc32(dst.get(..body)?);
    dst.get_mut(body..)?.copy_from_slice(&crc.to_le_bytes());
    Some(total)
}
