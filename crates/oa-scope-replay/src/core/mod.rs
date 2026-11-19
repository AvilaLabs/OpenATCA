//! The core (RULESET B1, B2): pure functions of plain data. No file, clock, environment or thread
//! appears under this module, and no tracker or ADS-B wire type appears in a signature (D3): the
//! shell hands over plain columns. Every module states its class in its header; the modules are
//! re-exported at the crate root.

pub mod frame;
pub mod rows;
