#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]
//! (Layout: `core` holds the pure modules, `shell` the ones that touch files; the modules are
//! re-exported here so their paths are `oa_display::list`, `oa_display::traffic` and so on.)

// B10: 64-bit native targets and the audited wasm32-unknown-unknown target are supported.
#[cfg(not(any(
    target_pointer_width = "64",
    all(
        target_arch = "wasm32",
        target_pointer_width = "32",
        target_os = "unknown",
        target_vendor = "unknown"
    )
)))]
compile_error!(
    "this crate supports 64-bit targets and the audited wasm32-unknown-unknown target only"
);

pub mod core;
#[cfg(not(all(
    target_arch = "wasm32",
    target_pointer_width = "32",
    target_os = "unknown",
    target_vendor = "unknown"
)))]
pub mod shell;

pub use crate::core::raster::MAX_DISPLAY_PIXELS;
pub use crate::core::{
    alert_markers, font, interaction, list, metering, ownership, palette, project, raster, safety,
    scene, status_strip, text, traffic, workstation,
};
#[cfg(not(all(
    target_arch = "wasm32",
    target_pointer_width = "32",
    target_os = "unknown",
    target_vendor = "unknown"
)))]
pub use crate::shell::{cli, ppm, probe};
