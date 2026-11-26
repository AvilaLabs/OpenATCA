//! CIFP resolution, OASD parsing and other I/O-adjacent preparation interfaces.

mod artifact;
mod resolve;

pub use artifact::{ArtifactError, ArtifactHeader, ScenarioArtifact, ScenarioArtifactRecord};
pub use resolve::{
    resolve_procedure_route, resolve_route_chain, AltitudeValueKind, RouteResolveError,
};
