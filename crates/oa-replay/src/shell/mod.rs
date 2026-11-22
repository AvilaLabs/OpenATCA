//! Shell (B1, B3, B7, C6): argument parsing, recordings read from files and merged, and the text
//! `oa-replay` prints. The `oa-replay` binary (`src/main.rs`) is shell too. Also the code that
//! names the decoder crates, which core may not (B8): `pipeline` (frames in, state out, runs the
//! decoders), `apply` (decoded SFDPS and TBFM rows to the flight table) and the tables of
//! decoded rows, `adsb`, `tfms` and `fns`. They are pure and keep the replay's rule of no
//! allocation after construction (C1, C5, checked by `tests/alloc.rs`); they are shell because
//! of what they name, not because they do I/O. `hash` (the state hash) is shell because it
//! allocates a slot list per checkpoint (C6).

pub mod adsb;
pub mod apply;
pub mod cli;
pub mod fns;
pub mod hash;
pub mod input_fingerprint;
pub mod pipeline;
pub mod report;
pub mod source;
pub mod tfms;
