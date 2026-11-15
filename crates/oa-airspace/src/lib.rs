#![forbid(unsafe_code)]
//! oa-airspace: the airspace model (RULESET A, B). One ARTCC and cycle as flat struct-of-arrays
//! tables, built from the FAA NASR CSV and CIFP files, written as a deterministic snapshot.
//!
//! Core versus shell (RULESET B1):
//!
//! | module     | class | role                                                              |
//! |------------|-------|-------------------------------------------------------------------|
//! | `model`    | core  | tables, index types, codes, validation, lookups                    |
//! | `geom`     | core  | fixed-point conversion, selection area, geodesic tests             |
//! | `airac`    | core  | a date to its 28-day AIRAC cycle                                   |
//! | `build`    | core  | parsed sources to a model: matching, selection, unresolved lists   |
//! | `snapshot` | core  | model to bytes and back, bounded reader                            |
//! | `report`   | core  | the build report text and boundary-geometry unknowns               |
//! | `diff`     | core  | two models to a change list                                        |
//! | `source`   | data  | plain data passed from the shell to the core                       |
//! | `load`     | shell | reads the archives, verifies hashes, parses with oa-nasr/oa-arinc424 |
//! | `cli`      | shell | the `oa-adapt` command line, file reads and writes                 |
pub mod airac;
pub mod build;
pub mod cli;
pub mod diff;
pub mod geom;
pub mod load;
pub mod model;
pub mod report;
pub mod snapshot;
pub mod source;
