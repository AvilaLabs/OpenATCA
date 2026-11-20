#![forbid(unsafe_code)]
//! Comparison of two tracker runs over one clip: their per-second digests, and where they
//! differ their canonical rows (`oa_track::canon`).
//!
//! Classification (B1): [`diff`] is the pure core (parsing and checking digest text, pairing and
//! comparing rows, rendering, the verdict) and does no I/O; [`files`] is the shell (files, the
//! `Read` of a rows file, output with its errors handled). The binary in `main.rs` is shell too.
//! Every public item is classified in the header of its module.

pub mod diff;
pub mod files;

pub use diff::*;
pub use files::{load_digest_text, load_expect_text, open_rows, write_out, RowsReader};
