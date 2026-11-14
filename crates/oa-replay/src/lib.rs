#![forbid(unsafe_code)]
//! oa-replay: turns recorded SWIM feeds into one deterministic flight state (milestone M1,
//! "record and replay; bit-exact replay harness"). Every later stage (tracker, alerts) runs
//! against this replayed state.
//!
//! * Core (pure, no I/O, no clock, no allocation after construction, B1-B5): [`index`] (a
//!   fixed-size open-addressing hash index), [`table`] ([`FlightTable`], a struct-of-arrays of
//!   fixed capacity, and its eviction), [`apply`] ([`apply_sfdps`], [`apply_tbfm`]), [`adsb`]
//!   ([`AdsbTable`], the latest ADS-B report per aircraft address, bounded by
//!   [`ADSB_CAPACITY`]), [`hash`] ([`state_hash`] of the flight table; [`replay_hash`] adds the
//!   ADS-B table once an ADS-B frame has been fed, so replays without ADS-B hash as ever), [`errors`] and [`pipeline`] (frames in, state and counters out; replay
//!   time is each frame's `recv_unix_ns`).
//! * Shell: [`source`] (reads `.oaf` and `.oaf.gz` recordings as streams and merges them into one
//!   total order in constant memory), [`cli`] and the `oa-replay` binary (arguments, files, clock for the throughput
//!   line, stdout).
//!
//! ADS-B reports (`docs/ADSB_PAYLOAD.md`, written by `oa-adsb-clip`) are a fifth feed kind. At an
//! equal receive time the merge puts SWIM frames first (SFDPS, TBFM, TFMS, FNS) and ADS-B last;
//! a malformed ADS-B payload is refused whole and counted under its own error kind.
//!
//! Replaying the same recordings gives the same checkpoints and the same final state hash on
//! every run, however the files are chunked on reading (RULESET B6). A flight table that is
//! full refuses and counts new flights instead of evicting early (INTERFACES IF-R1), and every
//! parse, decode and apply failure is counted by kind (IF-R5, IF-R6).

pub mod adsb;
pub mod apply;
pub mod cli;
pub mod errors;
pub mod hash;
pub mod index;
pub mod pipeline;
pub mod source;
pub mod table;

pub use adsb::{
    adsb_slot_of, apply_adsb, new_adsb_table, AdsbApplied, AdsbStats, AdsbTable, ADSB_CAPACITY,
    ADSB_HASH_TAG,
};
pub use apply::{apply_sfdps, apply_tbfm, Applied};
pub use errors::{count_error, errors_total, ErrKind, Errors, ERR_COUNT, ERR_NAMES};
pub use hash::{state_hash, HASH_TAG};
pub use pipeline::{
    add_errors, frames_total, new_pipeline, process_frame, replay_hash, rows_applied_total,
    snapshot, Checkpoint, FeedKind, Pipeline, PipelineConfig, PipelineError, FEED_COUNT,
    FEED_NAMES,
};
pub use source::{
    list_recordings, merge_key, merge_streams, run_streams, spec_from_bytes, spec_from_paths,
    MergeReport, Merged, StreamSpec,
};
pub use table::{
    advance_time, flights_live, new_table, slot_of_gufi, sweep, ConfigError, FlightTable,
    TableConfig, TableStats, DEFAULT_CAPACITY, DEFAULT_MAX_AGE_NS, DEFAULT_SWEEP_NS, NS_PER_S,
};
