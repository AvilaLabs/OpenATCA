//! Shell (B1, B7): everything that touches a file or the clock-free inputs from disk. The encoder
//! it drives (`oa-asterix`) allocates per record (C6), so none of this can be core. `encode` lays
//! record values into `oa_asterix` records and blocks and checks every block by decoding it;
//! `export` owns the output files, the track numbers and the per-day split; `run` feeds rows from
//! the tracker and from the ERAM tables into `export`; `job` is one whole run (all files partial,
//! one rename pass, everything removed on any error); `notes` writes the README and the
//! data-source file. The `oa-compass` binary (`src/main.rs`) is shell too.

pub mod adsb_export;
pub mod args;
pub mod dirs;
pub mod encode;
pub mod error;
pub mod export;
pub mod guard;
pub mod job;
pub mod notes;
mod output_fs;
pub mod radar;
pub mod run;
pub mod slice;
