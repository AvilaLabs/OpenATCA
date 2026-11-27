//! The parsed archives: the records the parser crates produced, one allocated record per input
//! row. **Shell** (B1, B7, C6): offline, built once per cycle by `load` and read by `build`; it
//! holds `oa-nasr` and `oa-arinc424` types, which core may not name (B8), so only the plain
//! values (`source`, core) cross to the report.
//!
//! The record types are the ones the parser crates define; nothing is copied or reshaped here,
//! so a test can take a parsed fixture and change one position to inject a defect.

use crate::source::{ArchiveInfo, Diagnostics};
use oa_arinc424::Cifp;
use oa_nasr::apt::{Airport, RunwayEnd};
use oa_nasr::arb::{ArbBase, Boundary};
use oa_nasr::awy::Airway;
use oa_nasr::cls::ClassAirspace;
use oa_nasr::fix::Fix;
use oa_nasr::nav::Navaid;
use oa_nasr::pfr::PreferredRoute;
use oa_nasr::procedure::Procedure;

/// The NASR files the model consumes, parsed. Files the model does not take (`FIX_NAV`,
/// `APT_RWY`, ...) are not loaded.
#[derive(Clone, Debug, Default)]
// A7.2: cold struct, size guard waived
pub struct NasrData {
    pub arb_base: Vec<ArbBase>,
    pub boundaries: Vec<Boundary>,
    pub fixes: Vec<Fix>,
    pub navaids: Vec<Navaid>,
    pub airports: Vec<Airport>,
    pub runway_ends: Vec<RunwayEnd>,
    pub airways: Vec<Airway>,
    pub stars: Vec<Procedure>,
    pub dps: Vec<Procedure>,
    pub preferred_routes: Vec<PreferredRoute>,
    pub class_airspace: Vec<ClassAirspace>,
}

/// Both archives, parsed. Cold container (A7.2).
#[derive(Debug)]
pub struct Sources {
    pub nasr_archive: ArchiveInfo,
    pub cifp_archive: ArchiveInfo,
    pub nasr: NasrData,
    pub cifp: Cifp,
    pub diag: Diagnostics,
}
