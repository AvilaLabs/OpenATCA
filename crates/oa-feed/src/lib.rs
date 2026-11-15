#![forbid(unsafe_code)]
//! oa-feed: the Rust side of the SWIM SCDS bridge. The bridge (`adapters/scds-bridge`, Java)
//! runs the vendor's Solace client in its own process (RULESET F6) and writes OAF1 frames to its
//! stdout; this crate reads, validates and measures them. The frame format is specified in
//! `docs/FEED_FRAMES.md`.
//!
//! * Core (pure, no I/O, no clock, no allocation after construction): [`frame`] (layout, error
//!   types, encoder), [`reader`] ([`FrameReader`], a resynchronising decoder over a
//!   caller-supplied buffer) and [`health`] ([`FeedHealth`] and the free functions that update
//!   and format it; time is always an input, B4).
//! * Shell: [`record`] (rotating, capped frame files), [`replay`] (recorded files back through
//!   the same reader and health code), [`cli`] and the `oa-feed` binary (process, threads,
//!   clock, stdout). [`stream`] reads a whole stream through the reader, counting a frame cut
//!   short at its end.
//!
//! A frame that fails validation is refused and flagged, never passed on (INTERFACES IF-R5): the
//! reader reports the error with its byte offset, then scans for the next magic and counts the
//! bytes it skipped. Health is a function of frame fields and injected times only, so replaying
//! a recording gives the same summary every time (B6).

pub mod cli;
pub mod frame;
pub mod health;
pub mod reader;
pub mod record;
pub mod replay;
pub mod stream;

pub use frame::{
    encode_frame, ErrorKind, Frame, FrameError, Kind, FLAG_BYTES, FLAG_REDELIVERED, HEADER_LEN,
    MAGIC, MAX_FRAME_LEN, MAX_PAYLOAD, TRAILER_LEN,
};
pub use health::{
    format_summary, observe_error, observe_frame, observe_skipped, rates_milli, FeedHealth,
};
pub use reader::{BufferTooSmall, FrameReader};
pub use stream::read_frames;

#[cfg(test)]
mod tests;
