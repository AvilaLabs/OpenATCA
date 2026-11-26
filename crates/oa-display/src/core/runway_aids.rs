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
}
const _: () = assert!(core::mem::size_of::<Severity>() == 1);

/// One bounded per-scan aggregate. Zero identifiers are reserved for explicit Unavailable rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct RunwayAidPacket {
    pub scan_sequence: u64,
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
    /// Rows excluded due to absent/stale/invalid prediction evidence; never silently treated clear.
    pub unavailable_tracks: u16,
}
const _: () = assert!(core::mem::size_of::<RunwayAidPacket>() == 40);

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
