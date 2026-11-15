#![forbid(unsafe_code)]
//! oa-perf: the speed and capacity harness of OpenATCA. It drives `oa-track`'s fusion layer
//! and tracker through their public interface and measures
//!
//! * the per-update latency on the real clip ([`clip`]);
//! * throughput, per-advance time, capacity counters and memory with `n` simultaneous aircraft
//!   at their real update rates, the capacity knee and the behaviour past it ([`capacity`],
//!   [`cli`]);
//! * the age of the data in every output row.
//!
//! Shell code: it reads the clock, `/proc` and the clip, starts child processes and writes files
//! (RULESET C6); it adds no hook to the crates it measures. Every number it prints is a number
//! from the machine it ran on, with that machine's load beside it: a run on a busy machine is
//! labelled preliminary.

pub mod capacity;
pub mod cli;
pub mod clip;
pub mod gen;
pub mod stats;
pub mod sys;
