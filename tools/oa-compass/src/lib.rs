#![forbid(unsafe_code)]
//! oa-compass: writes a recorded UTC day of OpenATCA tracker output and of ERAM tracks as raw
//! ASTERIX CAT062 Ed 1.21 data blocks, with the data-source file and the import notes an outside
//! evaluation tool (OpenATS COMPASS) needs. Class `split` (B7).
//!
//! * `core`: the pure conversion of one output row (an OpenATCA tracker tick row, or an ERAM
//!   track sample) into the plain values of one CAT062 record, in the item LSBs of the
//!   specification, with the list of what the row did not have. No I/O, no allocation.
//! * `shell`: the encoder glue onto `oa-asterix` (which allocates per record, so it is shell),
//!   the files, the tracker run, the ERAM load, the read-back verification, the notes, and the
//!   `oa-compass` binary.
//!
//! Requirements: OA-CMP-001 and the following, in REQUIREMENTS.md.

pub mod core;
pub mod shell;
