//! The core (RULESET B1, B2): pure functions of plain data. Nothing under this module reads a
//! file, a clock, the environment or a thread, and a frame allocates nothing after the tables are
//! built (C1). Every module states its class in its own header; the modules are re-exported at
//! the crate root, so `oa_display::traffic` is `oa_display::core::traffic`.

pub mod alert_markers;
pub mod font;
pub mod interaction;
pub mod list;
pub mod ownership;
pub mod palette;
pub mod project;
pub mod raster;
pub mod safety;
pub mod scene;
pub mod text;
pub mod traffic;
pub mod workstation;
