#![forbid(unsafe_code)]
//! oa-feed: the Rust side of the SWIM SCDS bridge. The bridge (`adapters/scds-bridge`, Java)
//! runs the vendor's Solace client in its own process (RULESET F6) and writes OAF1 frames to its
//! stdout; this crate reads, validates and measures them. The frame format is specified in
//! `docs/FEED_FRAMES.md`.
//!
//! The crate is `split` (B7): `core/` holds the pure code, `shell/` the rest.
//!
//! * Core (pure, no I/O, no clock, no allocation after construction): [`frame`] (layout, error
//!   types, encoder), [`reader`] ([`FrameReader`], a resynchronising decoder over a
//!   caller-supplied buffer) and [`health`] ([`FeedHealth`] and the free functions that update
//!   it; time is always an input, B4).
//! * Shell: [`record`] (rotating, capped frame files), [`replay`] (recorded files back through
//!   the same reader and health code), [`cli`], `summary` (`format_summary`, the one-line text form of
//!   the health, which allocates a `String`) and the `oa-feed` binary (process, threads, clock,
//!   stdout). [`stream`] reads a whole stream through the reader, counting a frame cut
//!   short at its end.
//!
//! A frame that fails validation is refused and flagged, never passed on (INTERFACES IF-R5): the
//! reader reports the error with its byte offset, then scans for the next magic and counts the
//! bytes it skipped. Health is a function of frame fields and injected times only, so replaying
//! a recording gives the same summary every time (B6).

// The width of `usize` is fixed by construction: no 32-bit build of the core (B10).
#[cfg(not(target_pointer_width = "64"))]
compile_error!("this crate supports 64-bit targets only");

pub mod core;
pub mod shell;

pub use crate::core::{frame, health, reader};
pub use crate::shell::{cli, record, replay, stream};

pub use crate::core::frame::{
    encode_frame, encode_frame_mono, ErrorKind, Frame, FrameError, Kind, FLAG_BYTES,
    FLAG_REDELIVERED, HEADER_LEN, HEADER_LEN_V2, MAGIC, MAGIC_V2, MAX_FRAME_LEN, MAX_PAYLOAD,
    TRAILER_LEN,
};
pub use crate::core::health::{
    observe_error, observe_frame, observe_skipped, rates_milli, FeedHealth,
};
pub use crate::core::reader::{
    frame_work_refusal_retryable, BufferTooSmall, FrameReader, FrameWorkBudget, FrameWorkKind,
    FrameWorkRefusal, FrameWorkStats, DEFAULT_FRAME_WORK_LIMIT,
};
pub use crate::shell::stream::read_frames;
pub use crate::shell::summary::format_summary;
