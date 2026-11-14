#![forbid(unsafe_code)]
//! oa-bench: the M2 benchmark harness (`docs/M2_DESIGN.md`). It measures the systems OpenATCA must
//! beat (ERAM's published tracks, TBFM and TFMS time estimates, ERAM's expanded routes and
//! handoffs) against independent truth, with scoring code that will later score OpenATCA's own
//! output unchanged.
//!
//! * Core (pure, B1): [`truth`] (the interface to `oa-truth`: a query function value and a list
//!   of legs, plus the airspace lookups as function values), [`legacy`] (struct-of-arrays tables of
//!   ERAM track samples, handoffs, expanded routes, TBFM and TFMS estimates), [`ingest`] and
//!   [`scan`] (decoded messages into those tables), [`thindex`], [`assoc`] (address first, then
//!   callsign and a gate), [`pair`] (paired samples), [`stats`] (the exact percentile method),
//!   [`metrics_th`] (T1 to T4), [`metrics_flow`] (T5 to T9), [`metrics_event`] (F1 to F3, R1,
//!   H1), [`run`], [`synth`] (the synthetic self-test) and [`report`] (text and CSV).
//! * Shell: [`input`] (reads the recorded message directories, hashes the input files) and the
//!   `oa-bench` binary.
//!
//! Truth is not built here. Until `oa-truth` is wired in, the binary runs on the synthetic world
//! only and refuses a recorded ADS-B clip with a message saying so.

pub mod assoc;
pub mod geo;
pub mod ingest;
pub mod input;
pub mod legacy;
pub mod metrics_event;
pub mod metrics_flow;
pub mod metrics_th;
pub mod pair;
pub mod report;
pub mod run;
pub mod scan;
pub mod stats;
pub mod synth;
pub mod thindex;
pub mod truth;
