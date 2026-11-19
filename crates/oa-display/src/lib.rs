#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]
//! (Layout: `core` holds the pure modules, `shell` the ones that touch files; the modules are
//! re-exported here so their paths are `oa_display::list`, `oa_display::traffic` and so on.)

pub mod core;
pub mod shell;

pub use crate::core::raster::MAX_DISPLAY_PIXELS;
pub use crate::core::{font, list, palette, project, raster, scene, text, traffic};
pub use crate::shell::{cli, ppm};
