#![forbid(unsafe_code)]
//! oa-arinc424: parser for ARINC 424-18 navigation data as published by the FAA in the
//! CIFP (Coded Instrument Flight Procedures), `FAACIFP18`. See docs/M0_DESIGN.md.
//!
//! The file is fixed-width: every record is 132 characters, columns are 1-based. A record
//! starts with `S` (standard) or `T` (tailored), a three-character customer/area code, a
//! section code and subsection code, and ends with a file record number (cols 124-128) and
//! the cycle in which the record was last changed (cols 129-132).
//!
//! Decoded record types: airports (PA), runways (PG), VHF navaids (D), NDBs (DB), enroute
//! waypoints (EA), terminal waypoints (PC), localizer/glide slopes (PI), airways (ER),
//! SIDs/STARs/approaches (PD/PE/PF, with continuation records), class B/C/D airspace (UC),
//! special use airspace (UR) and Grid MORA (AS). Every other section is counted and skipped.
//!
//! The FAA's exceptions to ARINC 424-18 ("CIFP Readme") are followed where they change field
//! usage; the field docs say where. Field positions are verified against real records in
//! `tests/layouts.rs` (OA-424-007).
//!
//! Limits (OA-GEN-003): [`MAX_LINE_LEN`], [`MAX_RECORDS`], [`MAX_CONTINUATIONS_PER_PRIMARY`],
//! [`MAX_HEADER_RECORDS`] and the per-group limits below. Exceeding one is a fatal [`Error`]
//! naming the source, line and limit. Nothing in this crate panics on any input (OA-GEN-004).

pub mod airport;
pub mod airspace;
pub mod airway;
mod field;
mod file;
pub mod header;
pub mod mora;
pub mod navaid;
pub mod procedure;
mod record;
pub mod types;
pub mod waypoint;

mod error;

pub use airport::{runway_designator, Airport, Localizer, Runway};
pub use airspace::{
    controlling_agency, is_closed, Airspace, AirspaceContinuation, AirspaceKind, BoundaryPoint,
    BoundaryVia,
};
pub use airway::{Airway, AirwayFix};
pub use error::{Error, ErrorKind};
pub use field::{parse_lat, parse_lon};
pub use file::{
    cifp_cycle, find_airport, is_clean, mora_at, parse_bytes, parse_reader, procedures_at,
    runways_at, Cifp,
};
pub use header::Header;
pub use mora::{mora_value_at, GridMora};
pub use navaid::{Ndb, VhfNavaid};
pub use procedure::{
    levels_of_service, parse_path_terminator, DistanceOrTime, Leg, LegContinuation, LevelOfService,
    PathTerminator, Procedure, ProcedureKind, Route,
};
pub use record::{parse_record, record_meta, Record};
pub use types::{
    altitude_feet, cycle_is_after, parse_cycle, Altitude, AltitudeDescription, AltitudeLimit,
    AltitudeUnit, Cycle, FixRef, Meta, Position, TurnDirection,
};
pub use waypoint::{Waypoint, WaypointKind};

/// Every record is exactly this many characters (without the line terminator).
pub const RECORD_LEN: usize = 132;
/// Longest accepted line in bytes, terminator included. Longer lines are rejected before
/// they are buffered.
pub const MAX_LINE_LEN: usize = 256;
/// Most lines accepted in one file (the 2026-10 CIFP has about 397,000).
pub const MAX_RECORDS: usize = 2_000_000;
/// Most continuation records attached to one primary record.
pub const MAX_CONTINUATIONS_PER_PRIMARY: usize = 16;
/// Most header records at the top of a file (the FAA file has 5).
pub const MAX_HEADER_RECORDS: usize = 16;
/// Most fixes in one airway (the 2026-10 file's longest has 125).
pub const MAX_AIRWAY_FIXES: usize = 1_024;
/// Most routes (transitions) in one procedure (the longest has 25).
pub const MAX_ROUTES_PER_PROCEDURE: usize = 256;
/// Most legs in one route (the longest has 21).
pub const MAX_ROUTE_LEGS: usize = 256;
/// Most boundary records in one airspace volume (the largest has 886).
pub const MAX_BOUNDARY_POINTS: usize = 4_096;
/// Most record-level errors kept in [`Cifp::errors`]; further ones are only counted.
pub const MAX_RECORD_ERRORS: usize = 1_000;
