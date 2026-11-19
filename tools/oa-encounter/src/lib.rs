#![forbid(unsafe_code)]
//! `oa-encounter`: writes the record of close encounters (`docs/M5_DESIGN.md` section 5) from
//! the tracks OpenATCA builds on a recorded clip. Class `shell` (B1): it reads the clip, runs the
//! tracker as the scoring harness does, feeds the one-second rows to the core crate
//! `oa-encounter` and writes `encounters.csv` (one row per encounter, aircraft addresses) and
//! `summary.txt` (counts only, no address or callsign). The record volume, classes, hysteresis
//! and budgets are stated here in [`run::tool_config`]; the core has no defaults.
//!
//! Requirements: OA-ENC-001 and the following, in REQUIREMENTS.md.

pub mod args;
pub mod guard;
pub mod report;
pub mod run;
