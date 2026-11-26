#![forbid(unsafe_code)]
//! Deterministic synthetic scenario library with separate pure core and preparation shell.

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
compile_error!("oa-demo-scenario supports 64-bit targets and audited wasm32-unknown-unknown only");

pub mod core;
pub mod shell;

pub use crate::core::*;
pub use crate::shell::{
    resolve_procedure_route, resolve_route_chain, AltitudeValueKind, ArtifactError, ArtifactHeader,
    RouteResolveError, ScenarioArtifact, ScenarioArtifactRecord,
};
