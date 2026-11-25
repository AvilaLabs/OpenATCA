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
//! | `route`      | core  | bounded, atomic NASR/CIFP route expansion                          |
//! | `airac`      | core  | a date to its 28-day AIRAC cycle                                   |
//! | `snapshot`   | core  | model to bytes and back, bounded reader                            |
//! | `report`     | core  | the build report text and boundary-geometry unknowns               |
//! | `diff`       | core  | two models to a change list                                        |
//! | `source`     | core  | plain values (archive info, worded diagnostics) from shell to core |
//! | `sources`    | shell | the parsed oa-nasr / oa-arinc424 records `load` hands to `build`   |
//! | `build_info` | core  | what a build learns beside the model, and its thresholds           |
//! | `build`      | shell | parsed sources to a model; its working maps are `HashMap`s         |
//! | `load`       | shell | verifies supplied archive bytes and parses NASR/CIFP               |
//! | `route_parse`| shell | parses bounded normalized route instructions                        |

// The width of `usize` is fixed by construction: no 32-bit build of the core (B10).
#[cfg(not(target_pointer_width = "64"))]
compile_error!("this crate supports 64-bit targets only");

pub mod core;
pub mod shell;

pub use crate::core::{airac, build_info, diff, geom, model, report, route, snapshot, source};
pub use crate::shell::{build, load, sources};
