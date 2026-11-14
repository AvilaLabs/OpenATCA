#![forbid(unsafe_code)]
//! oa-replay: turns recorded SWIM feeds into one deterministic flight state (milestone M1,
//! "record and replay; bit-exact replay harness"). Every later stage (tracker, alerts) runs
//! against this replayed state.
//!
//! * Core (pure, no I/O, no clock, no allocation after construction, B1-B5): [`index`] (a
//!   fixed-size open-addressing hash index), [`table`] ([`FlightTable`], a struct-of-arrays of
//!   fixed capacity, and its eviction), [`apply`] ([`apply_sfdps`], [`apply_tbfm`]), [`hash`]
//!   ([`state_hash`]), [`errors`] and [`pipeline`] (frames in, state and counters out; replay
//!   time is each frame's `recv_unix_ns`).
//! * Shell: [`source`] (reads `.oaf` and `.oaf.gz` recordings and merges them into one total
//!   order), [`cli`] and the `oa-replay` binary (arguments, files, clock for the throughput
//!   line, stdout).
//!
//! Replaying the same recordings gives the same checkpoints and the same final state hash on
//! every run, however the files are chunked on reading (RULESET B6). A flight table that is
//! full refuses and counts new flights instead of evicting early (INTERFACES IF-R1), and every
//! parse, decode and apply failure is counted by kind (IF-R5, IF-R6).

pub mod apply;
pub mod cli;
pub mod errors;
pub mod hash;
pub mod index;
pub mod pipeline;
pub mod source;
pub mod table;

pub use apply::{apply_sfdps, apply_tbfm, Applied};
pub use errors::{count_error, errors_total, ErrKind, Errors, ERR_COUNT, ERR_NAMES};
pub use hash::{state_hash, HASH_TAG};
pub use pipeline::{
    add_errors, frames_total, new_pipeline, process_frame, rows_applied_total, snapshot,
    Checkpoint, FeedKind, Pipeline, PipelineConfig, PipelineError, FEED_COUNT, FEED_NAMES,
};
pub use source::{
    load_files, load_sources, merge_key, merge_order, payload, run_loaded, Loaded, Rec,
};
pub use table::{
    advance_time, flights_live, new_table, slot_of_gufi, sweep, ConfigError, FlightTable,
    TableConfig, TableStats, DEFAULT_CAPACITY, DEFAULT_MAX_AGE_NS, DEFAULT_SWEEP_NS, NS_PER_S,
};
