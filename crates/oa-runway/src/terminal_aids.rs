//! Bounded coordinate and spacing calculations for CRDA and ATPA aids. Core (RULESET B1, B2).
//!
//! This module supplies the coordinate translation in the FAA CRDA definition and the warning
//! lead times in the FAA ATPA definition. It does not make a separation determination: ATPA
//! minimum spacing is a required caller-supplied adaptation value, and CRDA remains a planning
//! reference. It reads no clock, display, file, or network and allocates nothing.

use crate::plane::Local;

/// CRDA parent and projected ghost rows are bounded to the airport local-plane extent.
pub const MAX_CRDA_ROWS: usize = 256;
/// One ATPA call may evaluate at most every unordered pair of 256 final tracks.
pub const MAX_ATPA_PAIRS: usize = MAX_CRDA_ROWS * (MAX_CRDA_ROWS - 1) / 2;
/// FAA Pilot/Controller Glossary ATPA warning lead time, seconds before predicted loss.
pub const ATPA_WARNING_LEAD_SECONDS: f64 = 45.0;
/// FAA Pilot/Controller Glossary ATPA alert lead time, seconds before predicted loss.
pub const ATPA_ALERT_LEAD_SECONDS: f64 = 24.0;
const MAX_LOCAL_M: f64 = 20_000.0;
const MAX_GROUND_SPEED_MPS: f64 = 2_102.0;
const NO_PREDICTED_LOSS_MS: u32 = u32::MAX;

/// One reference point and directed line; the normal is derived as the left perpendicular.
#[derive(Clone, Copy, Debug, PartialEq)]
// A7.2: cold adaptation value, size guard waived.
pub struct ReferenceLine {
    pub origin: Local,
    pub axis_east: f64,
    pub axis_north: f64,
}

/// Source and target transforms plus an explicitly adapted signed along-track ghost offset.
#[derive(Clone, Copy, Debug, PartialEq)]
// A7.2: cold adaptation value, size guard waived.
pub struct CrdaTransform {
    pub source: ReferenceLine,
    pub target: ReferenceLine,
    pub target_offset_m: f64,
}

/// Parent aircraft positions and ground speeds, one row per aircraft.
#[repr(C)]
pub struct CrdaParents<'a> {
    pub east_m: &'a [f64],
    pub north_m: &'a [f64],
    pub ground_speed_mps: &'a [f64],
}
const _: () = assert!(core::mem::size_of::<CrdaParents<'static>>() == 48);

/// Caller-owned ghost positions and parent-matching speeds, in input row order.
#[repr(C)]
pub struct CrdaGhosts<'a> {
    pub east_m: &'a mut [f64],
    pub north_m: &'a mut [f64],
    pub ground_speed_mps: &'a mut [f64],
}
const _: () = assert!(core::mem::size_of::<CrdaGhosts<'static>>() == 48);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CrdaError {
    BadInput = 1,
    Capacity = 2,
}
const _: () = assert!(core::mem::size_of::<CrdaError>() == 1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct CrdaResult {
    pub rows: u16,
}
const _: () = assert!(core::mem::size_of::<CrdaResult>() == 2);

fn valid_line(line: ReferenceLine) -> bool {
    line.origin.east_m.is_finite()
        && line.origin.north_m.is_finite()
        && line.origin.east_m.abs() <= MAX_LOCAL_M
        && line.origin.north_m.abs() <= MAX_LOCAL_M
        && line.axis_east.is_finite()
        && line.axis_north.is_finite()
        && (line.axis_east * line.axis_east + line.axis_north * line.axis_north - 1.0).abs()
            <= 1.0e-9
}

/// Translate parent `(x,y)` coordinates relative to a source point/line onto a target
/// point/line. The row order is preserved, ground speed is copied exactly, and output capacity
/// and every input row are validated before the first output write. A positive offset moves the
/// ghost along the target line; this is caller adaptation, not a default staggering rule.
pub fn project_crda_ghosts(
    transform: CrdaTransform,
    parents: CrdaParents<'_>,
    ghosts: CrdaGhosts<'_>,
) -> Result<CrdaResult, CrdaError> {
    let n = parents.east_m.len();
    if n > MAX_CRDA_ROWS {
        return Err(CrdaError::Capacity);
    }
    if parents.north_m.len() != n
        || parents.ground_speed_mps.len() != n
        || ghosts.east_m.len() != ghosts.north_m.len()
        || ghosts.east_m.len() != ghosts.ground_speed_mps.len()
    {
        return Err(CrdaError::BadInput);
    }
    if ghosts.east_m.len() < n {
        return Err(CrdaError::Capacity);
    }
    if !valid_line(transform.source)
        || !valid_line(transform.target)
        || !transform.target_offset_m.is_finite()
        || transform.target_offset_m.abs() > MAX_LOCAL_M
    {
        return Err(CrdaError::BadInput);
    }
    for ((&east, &north), &speed) in parents
        .east_m
        .iter()
        .zip(parents.north_m)
        .zip(parents.ground_speed_mps)
    {
        if !east.is_finite()
            || !north.is_finite()
            || east.abs() > MAX_LOCAL_M
            || north.abs() > MAX_LOCAL_M
            || !speed.is_finite()
            || !(0.0..=MAX_GROUND_SPEED_MPS).contains(&speed)
        {
            return Err(CrdaError::BadInput);
        }
        let along = (east - transform.source.origin.east_m) * transform.source.axis_east
            + (north - transform.source.origin.north_m) * transform.source.axis_north;
        let cross = (east - transform.source.origin.east_m) * -transform.source.axis_north
            + (north - transform.source.origin.north_m) * transform.source.axis_east;
        let ghost_east = transform.target.origin.east_m
            + (along + transform.target_offset_m) * transform.target.axis_east
            - cross * transform.target.axis_north;
        let ghost_north = transform.target.origin.north_m
            + (along + transform.target_offset_m) * transform.target.axis_north
            + cross * transform.target.axis_east;
        if !ghost_east.is_finite()
            || !ghost_north.is_finite()
            || ghost_east.abs() > MAX_LOCAL_M
            || ghost_north.abs() > MAX_LOCAL_M
        {
            return Err(CrdaError::BadInput);
        }
    }
    let (out_east, out_north, out_speed) = (
        &mut *ghosts.east_m,
        &mut *ghosts.north_m,
        &mut *ghosts.ground_speed_mps,
    );
    for (i, ((&east, &north), &speed)) in parents
        .east_m
        .iter()
        .zip(parents.north_m)
        .zip(parents.ground_speed_mps)
        .enumerate()
    {
        let along = (east - transform.source.origin.east_m) * transform.source.axis_east
            + (north - transform.source.origin.north_m) * transform.source.axis_north;
        let cross = (east - transform.source.origin.east_m) * -transform.source.axis_north
            + (north - transform.source.origin.north_m) * transform.source.axis_east;
        if let (Some(out_e), Some(out_n), Some(out_v)) = (
            out_east.get_mut(i),
            out_north.get_mut(i),
            out_speed.get_mut(i),
        ) {
            *out_e = transform.target.origin.east_m
                + (along + transform.target_offset_m) * transform.target.axis_east
                - cross * transform.target.axis_north;
            *out_n = transform.target.origin.north_m
                + (along + transform.target_offset_m) * transform.target.axis_north
                + cross * transform.target.axis_east;
            *out_v = speed;
        } else {
            return Err(CrdaError::Capacity);
        }
    }
    Ok(CrdaResult {
        rows: u16::try_from(n).unwrap_or(u16::MAX),
    })
}

/// Same-final flight pair inputs from one caller-selected scan. Distances increase toward the
/// threshold. The caller supplies the applicable minimum spacing for each pair; no universal
/// separation minimum is fabricated here.
#[repr(C)]
pub struct AtpaPairs<'a> {
    pub runway_id: &'a [u32],
    pub end_name: &'a [[u8; 3]],
    pub scan_ns: &'a [u64],
    pub lead_along_m: &'a [f64],
    pub trail_along_m: &'a [f64],
    pub lead_speed_mps: &'a [f64],
    pub trail_speed_mps: &'a [f64],
    pub minimum_spacing_m: &'a [f64],
}
const _: () = assert!(core::mem::size_of::<AtpaPairs<'static>>() == 128);

/// Result columns, one row for every input pair, in input order.
#[repr(C)]
pub struct AtpaRows<'a> {
    pub severity: &'a mut [AtpaSeverity],
    pub current_distance_milli_nm: &'a mut [u32],
    pub seconds_to_loss_milli: &'a mut [u32],
}
const _: () = assert!(core::mem::size_of::<AtpaRows<'static>>() == 48);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum AtpaSeverity {
    Clear = 0,
    Warning = 1,
    Alert = 2,
}
const _: () = assert!(core::mem::size_of::<AtpaSeverity>() == 1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum AtpaError {
    BadInput = 1,
    Capacity = 2,
    ScanMismatch = 3,
}
const _: () = assert!(core::mem::size_of::<AtpaError>() == 1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct AtpaResult {
    pub examined: u16,
    pub warning_or_alert: u16,
}
const _: () = assert!(core::mem::size_of::<AtpaResult>() == 4);

/// Evaluate a bounded collection of same-final lead/trail pairs. An output row is emitted for
/// every pair, so clear cases remain explicit. Warning is at most 45 seconds before the caller's
/// adapted minimum-spacing crossing; alert is at most 24 seconds. Loss already present is an
/// immediate alert. A pair with no positive closure has no predicted loss. All validation and
/// output-capacity checks precede writes.
pub fn evaluate_atpa_pairs(
    expected_scan_ns: u64,
    pairs: AtpaPairs<'_>,
    rows: AtpaRows<'_>,
) -> Result<AtpaResult, AtpaError> {
    let n = pairs.runway_id.len();
    if n > MAX_ATPA_PAIRS {
        return Err(AtpaError::Capacity);
    }
    let pair_columns = [
        pairs.end_name.len(),
        pairs.scan_ns.len(),
        pairs.lead_along_m.len(),
        pairs.trail_along_m.len(),
        pairs.lead_speed_mps.len(),
        pairs.trail_speed_mps.len(),
        pairs.minimum_spacing_m.len(),
    ];
    if pair_columns.iter().any(|len| *len != n) {
        return Err(AtpaError::BadInput);
    }
    if rows.severity.len() != rows.current_distance_milli_nm.len()
        || rows.severity.len() != rows.seconds_to_loss_milli.len()
    {
        return Err(AtpaError::BadInput);
    }
    if rows.severity.len() < n {
        return Err(AtpaError::Capacity);
    }
    if expected_scan_ns == 0 {
        return Err(AtpaError::BadInput);
    }
    for i in 0..n {
        let (
            Some(&runway),
            Some(&end),
            Some(&scan),
            Some(&lead),
            Some(&trail),
            Some(&lead_speed),
            Some(&trail_speed),
            Some(&min_space),
        ) = (
            pairs.runway_id.get(i),
            pairs.end_name.get(i),
            pairs.scan_ns.get(i),
            pairs.lead_along_m.get(i),
            pairs.trail_along_m.get(i),
            pairs.lead_speed_mps.get(i),
            pairs.trail_speed_mps.get(i),
            pairs.minimum_spacing_m.get(i),
        )
        else {
            return Err(AtpaError::BadInput);
        };
        if scan != expected_scan_ns {
            return Err(AtpaError::ScanMismatch);
        }
        if runway == 0
            || end == [0; 3]
            || !lead.is_finite()
            || !trail.is_finite()
            || lead.abs() > MAX_LOCAL_M
            || trail.abs() > MAX_LOCAL_M
            || lead <= trail
            || !lead_speed.is_finite()
            || !trail_speed.is_finite()
            || !(0.0..=MAX_GROUND_SPEED_MPS).contains(&lead_speed)
            || !(0.0..=MAX_GROUND_SPEED_MPS).contains(&trail_speed)
            || !min_space.is_finite()
            || !(0.0..=MAX_LOCAL_M).contains(&min_space)
        {
            return Err(AtpaError::BadInput);
        }
    }
    let (severities, distances, times) = (
        &mut *rows.severity,
        &mut *rows.current_distance_milli_nm,
        &mut *rows.seconds_to_loss_milli,
    );
    let mut warned = 0u16;
    for i in 0..n {
        let (Some(&lead), Some(&trail), Some(&lead_speed), Some(&trail_speed), Some(&min_space)) = (
            pairs.lead_along_m.get(i),
            pairs.trail_along_m.get(i),
            pairs.lead_speed_mps.get(i),
            pairs.trail_speed_mps.get(i),
            pairs.minimum_spacing_m.get(i),
        ) else {
            return Err(AtpaError::BadInput);
        };
        let distance_m = lead - trail;
        let closure_mps = trail_speed - lead_speed;
        let time_s = if distance_m <= min_space {
            Some(0.0)
        } else if closure_mps > 0.0 {
            Some((distance_m - min_space) / closure_mps)
        } else {
            None
        };
        let severity = match time_s {
            Some(seconds) if seconds <= ATPA_ALERT_LEAD_SECONDS => AtpaSeverity::Alert,
            Some(seconds) if seconds <= ATPA_WARNING_LEAD_SECONDS => AtpaSeverity::Warning,
            _ => AtpaSeverity::Clear,
        };
        let distance_nm_milli = ((distance_m / 1_852.0) * 1_000.0 + 0.5) as u32;
        let time_ms = time_s.map_or(NO_PREDICTED_LOSS_MS, |seconds| {
            (seconds * 1_000.0 + 0.5) as u32
        });
        if let (Some(out_severity), Some(out_distance), Some(out_time)) = (
            severities.get_mut(i),
            distances.get_mut(i),
            times.get_mut(i),
        ) {
            *out_severity = severity;
            *out_distance = distance_nm_milli;
            *out_time = time_ms;
        } else {
            return Err(AtpaError::Capacity);
        }
        if severity != AtpaSeverity::Clear {
            warned = warned.saturating_add(1);
        }
    }
    Ok(AtpaResult {
        examined: u16::try_from(n).unwrap_or(u16::MAX),
        warning_or_alert: warned,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    // REQ: OA-RWY-022
    #[test]
    fn translated_ghost_preserves_parent_offset_and_speed() {
        let source = ReferenceLine {
            origin: Local {
                east_m: 0.0,
                north_m: 0.0,
            },
            axis_east: 1.0,
            axis_north: 0.0,
        };
        let target = ReferenceLine {
            origin: Local {
                east_m: 1_000.0,
                north_m: 2_000.0,
            },
            axis_east: 0.0,
            axis_north: 1.0,
        };
        let east = [120.0];
        let north = [30.0];
        let speed = [75.0];
        let mut ghost_east = [0.0];
        let mut ghost_north = [0.0];
        let mut ghost_speed = [0.0];
        let result = project_crda_ghosts(
            CrdaTransform {
                source,
                target,
                target_offset_m: 10.0,
            },
            CrdaParents {
                east_m: &east,
                north_m: &north,
                ground_speed_mps: &speed,
            },
            CrdaGhosts {
                east_m: &mut ghost_east,
                north_m: &mut ghost_north,
                ground_speed_mps: &mut ghost_speed,
            },
        )
        .expect("valid source/target reference lines");
        assert_eq!(result.rows, 1);
        assert_eq!(ghost_east, [970.0]);
        assert_eq!(ghost_north, [2_130.0]);
        assert_eq!(ghost_speed, speed);
    }

    // REQ: OA-RWY-022
    #[test]
    fn invalid_reference_or_capacity_refuses_without_partial_write() {
        let source = ReferenceLine {
            origin: Local {
                east_m: 0.0,
                north_m: 0.0,
            },
            axis_east: 1.0,
            axis_north: 0.0,
        };
        let target = ReferenceLine {
            origin: Local {
                east_m: 0.0,
                north_m: 0.0,
            },
            axis_east: 1.0,
            axis_north: 0.0,
        };
        let east = [10.0, 20.0];
        let north = [0.0, 0.0];
        let speed = [50.0, 60.0];
        let mut ghost_east = [77.0, 77.0];
        let mut ghost_north = [77.0, 77.0];
        let mut ghost_speed = [77.0, 77.0];
        let bad_target = ReferenceLine {
            axis_east: 2.0,
            ..target
        };
        assert_eq!(
            project_crda_ghosts(
                CrdaTransform {
                    source,
                    target: bad_target,
                    target_offset_m: 0.0
                },
                CrdaParents {
                    east_m: &east,
                    north_m: &north,
                    ground_speed_mps: &speed
                },
                CrdaGhosts {
                    east_m: &mut ghost_east,
                    north_m: &mut ghost_north,
                    ground_speed_mps: &mut ghost_speed
                },
            ),
            Err(CrdaError::BadInput)
        );
        assert_eq!(ghost_east, [77.0; 2]);
        assert_eq!(ghost_north, [77.0; 2]);
        assert_eq!(ghost_speed, [77.0; 2]);
        assert_eq!(
            project_crda_ghosts(
                CrdaTransform {
                    source,
                    target,
                    target_offset_m: 0.0
                },
                CrdaParents {
                    east_m: &east,
                    north_m: &north,
                    ground_speed_mps: &speed
                },
                CrdaGhosts {
                    east_m: &mut ghost_east[..1],
                    north_m: &mut ghost_north[..1],
                    ground_speed_mps: &mut ghost_speed[..1]
                },
            ),
            Err(CrdaError::Capacity)
        );
        assert_eq!(ghost_east, [77.0; 2]);
    }

    fn atpa_case(lead: f64, trail: f64, lead_speed: f64, trail_speed: f64) -> AtpaSeverity {
        let runway = [1];
        let end = [*b"27L"];
        let scan = [1_000];
        let lead_pos = [lead];
        let trail_pos = [trail];
        let lead_v = [lead_speed];
        let trail_v = [trail_speed];
        let minimum = [1_000.0];
        let mut severity = [AtpaSeverity::Clear];
        let mut distance = [0];
        let mut time = [0];
        evaluate_atpa_pairs(
            1_000,
            AtpaPairs {
                runway_id: &runway,
                end_name: &end,
                scan_ns: &scan,
                lead_along_m: &lead_pos,
                trail_along_m: &trail_pos,
                lead_speed_mps: &lead_v,
                trail_speed_mps: &trail_v,
                minimum_spacing_m: &minimum,
            },
            AtpaRows {
                severity: &mut severity,
                current_distance_milli_nm: &mut distance,
                seconds_to_loss_milli: &mut time,
            },
        )
        .expect("valid ATPA pair");
        severity[0]
    }

    // REQ: OA-RWY-023
    #[test]
    fn atpa_uses_warning_and_alert_lead_times_for_predicted_loss() {
        assert_eq!(atpa_case(1_900.0, 0.0, 80.0, 100.0), AtpaSeverity::Warning);
        assert_eq!(atpa_case(1_480.0, 0.0, 80.0, 100.0), AtpaSeverity::Alert);
    }

    // REQ: OA-RWY-023
    #[test]
    fn atpa_immediate_loss_and_nonclosing_cases_are_distinct() {
        assert_eq!(atpa_case(900.0, 0.0, 100.0, 80.0), AtpaSeverity::Alert);
        assert_eq!(atpa_case(3_000.0, 0.0, 100.0, 80.0), AtpaSeverity::Clear);
    }

    // REQ: OA-RWY-023
    #[test]
    fn atpa_invalid_pairs_and_capacity_refuse_atomically() {
        let runway = [1];
        let end = [*b"27L"];
        let scan = [10, 11];
        let lead_pos = [2_000.0, 2_000.0];
        let trail_pos = [0.0, 0.0];
        let speed = [100.0, 100.0];
        let minimum = [1_000.0, 1_000.0];
        let mut severity = [AtpaSeverity::Clear; 2];
        let mut distance = [99; 2];
        let mut time = [99; 2];
        let pairs = AtpaPairs {
            runway_id: &[1, 1],
            end_name: &[end[0], end[0]],
            scan_ns: &scan,
            lead_along_m: &lead_pos,
            trail_along_m: &trail_pos,
            lead_speed_mps: &speed,
            trail_speed_mps: &speed,
            minimum_spacing_m: &minimum,
        };
        assert_eq!(
            evaluate_atpa_pairs(
                10,
                pairs,
                AtpaRows {
                    severity: &mut severity,
                    current_distance_milli_nm: &mut distance,
                    seconds_to_loss_milli: &mut time,
                }
            ),
            Err(AtpaError::ScanMismatch)
        );
        assert_eq!(severity, [AtpaSeverity::Clear; 2]);
        assert_eq!(distance, [99; 2]);
        assert_eq!(time, [99; 2]);
        let one_runway = [runway[0]];
        let one_end = [end[0]];
        assert_eq!(
            evaluate_atpa_pairs(
                10,
                AtpaPairs {
                    runway_id: &one_runway,
                    end_name: &one_end,
                    scan_ns: &[10],
                    lead_along_m: &[2_000.0],
                    trail_along_m: &[0.0],
                    lead_speed_mps: &[100.0],
                    trail_speed_mps: &[100.0],
                    minimum_spacing_m: &[1_000.0],
                },
                AtpaRows {
                    severity: &mut severity[..0],
                    current_distance_milli_nm: &mut distance[..0],
                    seconds_to_loss_milli: &mut time[..0],
                }
            ),
            Err(AtpaError::Capacity)
        );
    }
}
