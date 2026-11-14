//! The plain values the shell hands the core: everything parsed out of the two archives.
//! **Core** (B1): data only, no behaviour (A1). Built by `load` (shell), consumed by `build`
//! (core); tests build it from fixtures or by hand.
//!
//! The record types are the ones the parser crates define; nothing is copied or reshaped here,
//! so a test can take a parsed fixture and change one position to inject a defect.

use oa_arinc424::Cifp;
use oa_nasr::apt::{Airport, RunwayEnd};
use oa_nasr::arb::{ArbBase, Boundary};
use oa_nasr::awy::Airway;
use oa_nasr::cls::ClassAirspace;
use oa_nasr::fix::Fix;
use oa_nasr::nav::Navaid;
use oa_nasr::procedure::Procedure;
use oa_nasr::schema::SchemaViolation;
use oa_nasr::RowError;

/// An input archive: name, size, hash, and the hash that was expected of it, if any.
/// Cold struct (A7.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveInfo {
    /// File name without directory.
    pub name: String,
    pub bytes: u64,
    pub sha256: [u8; 32],
    /// The `--expect-sha256` value this archive was checked against; `None` when none was
    /// given (the report says so). A mismatch never gets this far: the load is refused.
    pub expected: Option<[u8; 32]>,
}

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
    pub class_airspace: Vec<ClassAirspace>,
}

/// What the validation of one NASR file found: schema violations and rows the typed parser
/// rejected. Cold struct (A7.2).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileDiag {
    /// File name inside the archive (`FIX_BASE.csv`).
    pub file: String,
    /// Data rows checked against the schema.
    pub rows: u64,
    /// True when the structure files had no schema for this file.
    pub schema_missing: bool,
    pub violations: Vec<SchemaViolation>,
    /// Violations beyond the validator's cap, counted but not kept.
    pub dropped: u64,
    pub rejected: Vec<RowError>,
    /// Rejected rows beyond the parser's cap (`oa_nasr::MAX_REJECTED_PER_FILE`), counted only.
    /// For a parser that joins several files this is booked on the last file it loaded.
    pub rejected_dropped: u64,
}

/// Everything the loaders noticed that is not a model object. Cold struct (A7.2).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Diagnostics {
    /// One entry per NASR data file used, in load order.
    pub nasr_files: Vec<FileDiag>,
    /// Problems in the structure files themselves.
    pub schema_warnings: Vec<String>,
    /// CIFP records that failed to decode (the parser keeps at most 1000).
    pub cifp_errors: Vec<String>,
    pub cifp_errors_dropped: usize,
    /// CIFP physical record counts by section (`PA`, `D`, ...).
    pub cifp_counts: Vec<(String, usize)>,
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
