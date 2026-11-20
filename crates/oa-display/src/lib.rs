#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]
//! (Layout: `core` holds the pure modules, `shell` the ones that touch files; the modules are
//! re-exported here so their paths are `oa_display::list`, `oa_display::traffic` and so on.)

// The width of `usize` is fixed by construction: no 32-bit build of the core (B10).
#[cfg(not(target_pointer_width = "64"))]
compile_error!("this crate supports 64-bit targets only");

pub mod core;
pub mod shell;

pub use crate::core::raster::MAX_DISPLAY_PIXELS;
pub use crate::core::{font, list, palette, project, raster, safety, scene, text, traffic};
pub use crate::shell::{cli, ppm, probe};
