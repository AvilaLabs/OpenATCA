#![forbid(unsafe_code)]
//! `oa-ca`: runs conflict alert (`docs/M5_DESIGN.md` section 2) over the tracks OpenATCA builds
//! on a recorded clip. Class `shell` (B1): it reads the clip and the configuration file, runs the
//! tracker as the scoring harness does, feeds the one-second rows to the core crate `oa-ca`, keeps
//! the net state in an `oa-safety` table and writes `alerts.csv`, `altitude_lost.csv` and
//! `summary.txt` (counts only, no address or callsign). Every value of the alerting rules comes
//! from the configuration file; the tool has none of its own.
//!
//! Requirements: OA-CA-001 and the following, in REQUIREMENTS.md.

pub mod args;
pub mod config;
pub mod guard;
pub mod report;
pub mod run;
pub mod synth;
