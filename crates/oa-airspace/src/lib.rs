#![forbid(unsafe_code)]
//! oa-airspace: the airspace model (RULESET A, B). One ARTCC and cycle as flat struct-of-arrays
//! tables, built from the FAA NASR CSV and CIFP files, written as a deterministic snapshot.
//!
//! Class `split` (RULESET B1, B7). Every module keeps its public path through the re-exports
//! below.
//!
//! | module       | class | role                                                              |
//! |--------------|-------|-------------------------------------------------------------------|
//! | `model`      | core  | tables, index types, codes, validation, lookups                    |
//! | `geom`       | core  | fixed-point conversion, selection area, geodesic tests             |
//! | `airac`      | core  | a date to its 28-day AIRAC cycle                                   |
//! | `snapshot`   | core  | model to bytes and back, bounded reader                            |
//! | `report`     | core  | the build report text and boundary-geometry unknowns               |
//! | `diff`       | core  | two models to a change list                                        |
//! | `source`     | core  | plain data passed from the shell to the core                       |
//! | `build_info` | core  | what a build learns beside the model, and its thresholds           |
//! | `build`      | shell | parsed sources to a model; its working maps are `HashMap`s         |
//! | `load`       | shell | reads the archives, verifies hashes, parses with oa-nasr/oa-arinc424 |
//! | `cli`        | shell | the `oa-adapt` command line, file reads and writes                 |

pub mod core;
pub mod shell;

pub use crate::core::{airac, build_info, diff, geom, model, report, snapshot, source};
pub use crate::shell::{build, cli, load};
