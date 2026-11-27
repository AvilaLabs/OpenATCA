//! Plain runway-aid status packets for shell/native presentation (core value contract only).
//! No geometry, policy, styling, clock, file, or authority behavior lives in this module.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Availability {
    Unavailable = 0,
    Available = 1,
    Degraded = 2,
}
const _: () = assert!(core::mem::size_of::<Availability>() == 1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Severity {
    Clear = 0,
    Caution = 1,
    Warning = 2,
    Alert = 3,
    /// The final monitor did not evaluate this scan; no clear result is asserted.
    NotEvaluated = 4,
}
const _: () = assert!(core::mem::size_of::<Severity>() == 1);

/// One bounded per-scan aggregate. Zero identifiers are reserved for explicit Unavailable rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct RunwayAidPacket {
    pub scan_sequence: u64,
    /// Time of the final-course monitor evaluation backing its counts/severity; zero means none.
    /// This may precede the packet scan when a valid evaluation is reused within its cadence.
    pub final_monitor_as_of_ns: u64,
    pub first_runway_id: u32,
    pub first_end_name: [u8; 3],
    pub second_runway_id: u32,
    pub second_end_name: [u8; 3],
    pub availability: Availability,
    pub maximum_severity: Severity,
    pub predicted_tracks: u16,
    pub caution_tracks: u16,
    pub warning_tracks: u16,
    pub alert_tracks: u16,
    pub atpa_warning_pairs: u16,
    pub atpa_alert_pairs: u16,
    pub crda_ghosts: u16,
    /// Rows excluded due to absent/stale/invalid prediction evidence; never silently treated clear.
    pub unavailable_tracks: u16,
}
const _: () = assert!(core::mem::size_of::<RunwayAidPacket>() == 56);

/// One accepted assignment from the runtime's full-key operator boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct AssignmentPacket {
    pub identity: super::interaction::FlightIdentity,
    pub generation: u32,
    pub sequence: u32,
    pub runway_id: u32,
    pub end_name: [u8; 3],
    pub reserved: u8,
}
const _: () = assert!(core::mem::size_of::<AssignmentPacket>() == 52);

/// One projected CRDA ghost in the runway pair's local east/north plane.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct CrdaGhostPacket {
    pub scan_sequence: u64,
    pub first_runway_id: u32,
    pub second_runway_id: u32,
    pub parent_address: u32,
    pub origin_lat_e7: i32,
    pub origin_lon_e7: i32,
    pub east_m: f64,
    pub north_m: f64,
    pub ground_speed_mps: f64,
}
const _: () = assert!(core::mem::size_of::<CrdaGhostPacket>() == 56);

/// Scope of the issued candidate geometry available to the runtime ARV bridge. `PairOnly` is
/// never enough to claim a clear result because other airport paths may be absent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ArvCoverage {
    Unavailable = 0,
    PairOnly = 1,
    IssuedAirportComplete = 2,
}
const _: () = assert!(core::mem::size_of::<ArvCoverage>() == 1);

/// Runtime interpretation of one assigned-runway verification row. `Unavailable` includes stale,
/// ambiguous, incomplete-coverage, or over-budget evidence; it is never a clear indication.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ArvStatus {
    NotEvaluated = 0,
    Clear = 1,
    WrongRunway = 2,
    Taxiway = 3,
    GoAroundCleared = 4,
    AmbiguousPath = 5,
}
const _: () = assert!(core::mem::size_of::<ArvStatus>() == 1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ArvAvailability {
    Unavailable = 0,
    Available = 1,
    Degraded = 2,
}
const _: () = assert!(core::mem::size_of::<ArvAvailability>() == 1);

/// One bounded per-flight ARV result. Identity, assignment sequence and prediction times are
/// preserved so downstream code cannot join the result by address alone or mistake a stale
/// prediction for the current frame. This is status data; this module contains no styling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct ArvPacket {
    pub scan_sequence: u64,
    pub predicted_at_ns: u64,
    pub newest_source_ns: u64,
    pub identity: super::interaction::FlightIdentity,
    pub generation: u32,
    pub assignment_sequence: u32,
    pub assigned_runway_id: u32,
    pub assigned_end_name: [u8; 3],
    pub matched_path_id: u32,
    pub coverage: ArvCoverage,
    pub availability: ArvAvailability,
    pub status: ArvStatus,
    pub acknowledged: u8,
    /// 1 only when the matching full-key target is in the current runtime view.
    pub current_view: u8,
    pub reserved: u8,
}
const _: () = assert!(core::mem::size_of::<ArvPacket>() == 88);
