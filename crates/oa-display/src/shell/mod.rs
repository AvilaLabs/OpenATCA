//! The shell (RULESET B1, B3): everything that touches a file, a stream, the environment or the
//! clock. Thin; the work is done by [`crate::core`]. Every module states its class in its header;
//! the modules are re-exported at the crate root.

pub mod cli;
pub mod ppm;
