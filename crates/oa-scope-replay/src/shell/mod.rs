//! The shell (RULESET B1, B3): files, the clock, the tracker run and the conversion of the
//! tracker's and the decoder's types into the plain columns the core takes. Every module states
//! its class in its header; the modules are re-exported at the crate root.

pub mod adapt;
pub mod cli;
pub mod store;
