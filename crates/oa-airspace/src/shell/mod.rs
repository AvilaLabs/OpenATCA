//! Shell (B1, B7): everything that reads files, spawns threads, parses command lines, or uses a
//! hash map. `build` is shell because its working maps are `HashMap`s (looked up only; every
//! output list is sorted, so no iteration order reaches the output), `load` and `cli` do I/O.

pub mod build;
pub mod cli;
pub mod load;
pub mod route_cli;
pub mod sources;
