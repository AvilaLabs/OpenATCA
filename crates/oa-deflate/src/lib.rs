#![forbid(unsafe_code)]
//! oa-deflate: RFC 1951 DEFLATE compression and RFC 1952 gzip framing. Core (pure): the
//! compressor does no I/O; compressed bytes go to a caller-supplied closure, so the shell
//! decides whether they land in a file, a socket or a buffer.
//!
//! Why a crate of its own rather than part of `oa-zip`: `oa-zip` is deliberately read-only
//! ("nothing here ever writes a file") and decodes untrusted archives; this crate is the
//! writing side, used to compress recordings of live feeds as they are written. It uses
//! `oa-zip` for CRC-32 and, in tests, the inflater, so the two halves check each other.
//!
//! [`Compressor`] is a streaming raw-DEFLATE compressor with fixed preallocated state (a 32 KiB
//! window inside a 256 KiB buffer, 2^15 hash chains, a bounded chain-search depth set by the
//! level) and no allocation after [`Compressor::new`]. [`GzipWriter`] wraps it in a gzip member.
//! Both offer a sync flush so a crash loses at most the unflushed tail.
//!
//! Levels: 1 (fast), 6 (default) and 9 (best); 2..=8 interpolate, anything else is clamped.

mod deflate;
mod gzip;
mod huffman;

pub use deflate::{Compressor, MAX_LEVEL, MIN_LEVEL, WINDOW};
pub use gzip::{GzipWriter, GZIP_HEADER_LEN, GZIP_TRAILER_LEN};

/// The default compression level.
pub const DEFAULT_LEVEL: u8 = 6;
