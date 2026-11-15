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
//!   ADS-B, TFMS and FNS tables once a frame of that feed has been fed, so replays without them
//!   hash as ever), [`tfms`] ([`TfmsTable`], the latest estimates per TFMS flight and their link
//!   to SFDPS, and [`FlowTable`], the traffic management programmes), [`fns`] ([`FnsTable`], the
//!   NOTAMs in force), [`slots`] (their shared slot bookkeeping), [`errors`] and [`pipeline`]
//!   (frames in, state and counters out; replay time is each frame's `recv_unix_ns`).
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
pub mod clock;
pub mod errors;
pub mod fns;
pub mod hash;
pub mod index;
pub mod pipeline;
pub mod slots;
pub mod source;
pub mod table;
pub mod tfms;

pub use adsb::{
    adsb_advance, adsb_slot_of, adsb_sweep, apply_adsb, new_adsb_table, AdsbApplied, AdsbStats,
    AdsbTable, ADSB_CAPACITY, ADSB_HASH_TAG, ADSB_HASH_TAG_EVICT, ADSB_MAX_AGE_NS,
    ADSB_MAX_AHEAD_NS, ADSB_SWEEP_NS,
};
pub use apply::{apply_sfdps, apply_tbfm, Applied};
pub use clock::{clock_observe, Clock, FeedClock, Tick, CLOCK_CONFIRM_FRAMES, CLOCK_TOLERANCE_NS};
pub use errors::{count_error, errors_total, ErrKind, Errors, ERR_COUNT, ERR_NAMES};
pub use fns::{
    apply_fns, fns_in_force, fns_live, fns_slot_of, fns_stale_count, fns_sweep, new_fns_table,
    FnsApplied, FnsKey, FnsStats, FnsTable, FNS_CAPACITY, FNS_HASH_TAG, FNS_OPEN_AGE_NS,
};
pub use hash::{state_hash, HASH_TAG};
pub use pipeline::{
    add_errors, frames_total, new_pipeline, pipeline_stale_count, process_frame, replay_hash,
    rows_applied_total, snapshot, Checkpoint, FeedKind, Pipeline, PipelineConfig, PipelineError,
    FEED_COUNT, FEED_NAMES,
};
pub use source::{
    list_recordings, merge_key, merge_streams, run_streams, spec_from_bytes, spec_from_paths,
    MergeReport, Merged, StreamSpec,
};
pub use table::{
    advance_time, class_of_status, flights_live, is_stale, new_table, shift_times, slot_of_gufi,
    stale_count, sweep, ConfigError, FlightTable, TableConfig, TableStats, CLASS_ACTIVE,
    CLASS_FINISHED, CLASS_MASK, CLASS_PROPOSED, DEFAULT_ACTIVE_AGE_NS, DEFAULT_CAPACITY,
    DEFAULT_MAX_AGE_NS, DEFAULT_PROPOSED_AGE_NS, DEFAULT_SWEEP_NS, DEFAULT_TERMINAL_AGE_NS,
    NS_PER_S,
};
pub use tfms::{
    apply_tfms_flight, apply_tfms_flow, flow_live, flow_stale_count, flow_sweep, new_flow_table,
    new_tfms_table, tfms_linked, tfms_live, tfms_slot_of, tfms_stale_count, tfms_sweep,
    FlowApplied, FlowStats, FlowTable, TfmsApplied, TfmsStats, TfmsTable, FLOW_CAPACITY,
    FLOW_HASH_TAG, LINK_ACID, LINK_NONE, LINK_PLAN, TFMS_CAPACITY, TFMS_HASH_TAG,
};
