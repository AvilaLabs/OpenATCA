#![forbid(unsafe_code)]
//! `oa-msaw`: runs the minimum safe altitude warning (`docs/M5_DESIGN.md` section 3) over the
//! tracks OpenATCA builds on a recorded clip. Class `shell` (B1): it reads the clip, the
//! configuration file and the terrain, obstacle, chart and procedure data, runs the tracker as the
//! scoring harness does, looks up the floor at the points the core `oa-msaw` asks for, keeps the
//! net state and the inhibitions in `oa-safety` tables and writes `alerts.csv`, `skipped.csv`,
//! `inhibitions.csv` and `summary.txt`. Every value of the alerting rules comes from the
//! configuration file; the tool has none of its own.
//!
//! Requirements: OA-MSAW-030 and the following, in REQUIREMENTS.md.

pub mod args;
pub mod chart_raster;
pub mod cifp;
pub mod coarse;
pub mod config;
pub mod diagnose;
pub mod eligibility;
pub mod evaluation;
pub mod floors;
pub mod guard;
pub mod qnh;
pub mod report;
pub mod run;
