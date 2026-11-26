//! Bounded coordinate and spacing calculations for CRDA and ATPA aids. Core (RULESET B1, B2).
//!
//! This module supplies the coordinate translation in the FAA CRDA definition and the warning
//! lead times in the FAA ATPA definition. It does not make a separation determination: ATPA
//! minimum spacing is a required caller-supplied adaptation value, and CRDA remains a planning
//! reference. It reads no clock, display, file, or network and allocates nothing.

use crate::plane::Local;

/// CRDA parent and projected ghost rows share ARV's 256-flight terminal-call envelope. At this
/// cap, the transform performs at most 512 bounded row visits and writes three 2 KiB f64 output
/// columns; 257 rows are refused before inspecting or writing rows. This is a product engineering
/// bound, not a facility traffic claim. Increasing it requires a revised allocation/work review.
pub const MAX_CRDA_ROWS: usize = arv::MAX_ARV_FLIGHTS;
/// One ATPA call evaluates up to every unordered pair in the same 256-flight envelope (32,640
/// rows). Its two passes visit at most 65,280 pair rows; the caller owns the three 32,640-row
/// outputs. Larger scans are refused before output mutation.
pub const MAX_ATPA_PAIRS: usize = MAX_CRDA_ROWS * (MAX_CRDA_ROWS - 1) / 2;
/// FAA JO 7110.10EE Basic with Changes 1 and 2 (1/22/26), Pilot/Controller Glossary, entry
/// “AUTOMATED TERMINAL PROXIMITY ALERT (ATPA)”: warning cone at 45 seconds before predicted loss.
/// This engine implements only this lead-time threshold over caller-adapted minimum spacing.
/// Source: <https://www.faa.gov/documentLibrary/media/Order/7110.10EE_Bsc_w_Chg_1_and_2_dtd_1-22-26_Final.pdf>,
/// PCG A-18, ATPA entry.
pub const ATPA_WARNING_LEAD_SECONDS: f64 = 45.0;
/// Same source and entry: alert cone at 24 seconds before predicted loss. This is not a claim to
/// implement the full facility ATPA process or approved minima.
pub const ATPA_ALERT_LEAD_SECONDS: f64 = 24.0;
// The supported caller geometry is the `oa-runway::plane::EXTENT_M` local-plane extent.
const MAX_LOCAL_M: f64 = crate::plane::EXTENT_M;
// Conservative rounded-down numeric guard near `oa-adsb-model::speed::SPEED_ENVELOPE_MPS`
// (4,088 kt). It deliberately does not widen that accepted ADS-B envelope; it is not an adapted
// approach-speed threshold, so the runtime must mark any faster input unavailable/refused.
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

/// One monitor call accepts the same maximum flight count as ARV's bounded terminal profile.
/// At 256 rows the result vectors hold at most 256 rows (about 3 KiB), the 32,640 unordered
/// pairs fit the fixed pair cap, and ATPA performs at most 65,280 preflight/evaluation visits.
/// This is a per-call engineering capacity shared with ARV, not a claim about facility demand;
/// larger scans are refused whole. The pair cap is `256 * 255 / 2`.
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

fn checked_loss_time_ms(
    distance_m: f64,
    closure_mps: f64,
    minimum_spacing_m: f64,
) -> Result<u32, AtpaError> {
    if distance_m <= minimum_spacing_m {
        return Ok(0);
    }
    if closure_mps <= 0.0 {
        return Ok(NO_PREDICTED_LOSS_MS);
    }
    let millis = ((distance_m - minimum_spacing_m) / closure_mps) * 1_000.0;
    if !millis.is_finite() || millis + 0.5 > f64::from(NO_PREDICTED_LOSS_MS - 1) {
        return Err(AtpaError::BadInput);
    }
    Ok((millis + 0.5) as u32)
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
    pub lead_runway_id: &'a [u32],
    pub trail_runway_id: &'a [u32],
    pub lead_end_name: &'a [[u8; 3]],
    pub trail_end_name: &'a [[u8; 3]],
    pub scan_ns: &'a [u64],
    pub lead_along_m: &'a [f64],
    pub trail_along_m: &'a [f64],
    pub lead_speed_mps: &'a [f64],
    pub trail_speed_mps: &'a [f64],
    pub minimum_spacing_m: &'a [f64],
}
const _: () = assert!(core::mem::size_of::<AtpaPairs<'static>>() == 160);

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
    let n = pairs.lead_runway_id.len();
    if n > MAX_ATPA_PAIRS {
        return Err(AtpaError::Capacity);
    }
    let pair_columns = [
        pairs.trail_runway_id.len(),
        pairs.lead_end_name.len(),
        pairs.trail_end_name.len(),
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
            Some(&lead_runway),
            Some(&trail_runway),
            Some(&lead_end),
            Some(&trail_end),
            Some(&scan),
            Some(&lead),
            Some(&trail),
            Some(&lead_speed),
            Some(&trail_speed),
            Some(&min_space),
        ) = (
            pairs.lead_runway_id.get(i),
            pairs.trail_runway_id.get(i),
            pairs.lead_end_name.get(i),
            pairs.trail_end_name.get(i),
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
        if lead_runway == 0
            || trail_runway != lead_runway
            || lead_end == [0; 3]
            || trail_end != lead_end
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
            || !(f64::MIN_POSITIVE..=MAX_LOCAL_M).contains(&min_space)
        {
            return Err(AtpaError::BadInput);
        }
        checked_loss_time_ms(lead - trail, trail_speed - lead_speed, min_space)?;
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
        // All rows were range-checked before any output mutation.
        let time_ms = checked_loss_time_ms(distance_m, closure_mps, min_space)?;
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

    // REQ: OA-RWY-022
    #[test]
    fn crda_validates_late_rows_and_geometry_boundaries_before_writing() {
        let line = ReferenceLine {
            origin: Local {
                east_m: 0.0,
                north_m: 0.0,
            },
            axis_east: 1.0,
            axis_north: 0.0,
        };
        let east = [10.0, 20.0, f64::NAN];
        let north = [0.0; 3];
        let speed = [100.0; 3];
        let mut ghost_east = [77.0; 3];
        let mut ghost_north = [78.0; 3];
        let mut ghost_speed = [79.0; 3];
        assert_eq!(
            project_crda_ghosts(
                CrdaTransform {
                    source: line,
                    target: line,
                    target_offset_m: 0.0,
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
            ),
            Err(CrdaError::BadInput)
        );
        assert_eq!(ghost_east, [77.0; 3]);
        assert_eq!(ghost_north, [78.0; 3]);
        assert_eq!(ghost_speed, [79.0; 3]);
        assert_eq!(MAX_CRDA_ROWS, 256);
    }

    // REQ: OA-RWY-022
    #[test]
    fn crda_accepts_256_rows_and_refuses_257_before_write() {
        let line = ReferenceLine {
            origin: Local {
                east_m: 0.0,
                north_m: 0.0,
            },
            axis_east: 1.0,
            axis_north: 0.0,
        };
        let east = vec![0.0; MAX_CRDA_ROWS];
        let north = vec![0.0; MAX_CRDA_ROWS];
        let speed = vec![100.0; MAX_CRDA_ROWS];
        let mut ghost_east = vec![42.0; MAX_CRDA_ROWS + 1];
        let mut ghost_north = vec![43.0; MAX_CRDA_ROWS + 1];
        let mut ghost_speed = vec![44.0; MAX_CRDA_ROWS + 1];
        let at_cap = project_crda_ghosts(
            CrdaTransform {
                source: line,
                target: line,
                target_offset_m: 0.0,
            },
            CrdaParents {
                east_m: &east,
                north_m: &north,
                ground_speed_mps: &speed,
            },
            CrdaGhosts {
                east_m: &mut ghost_east[..MAX_CRDA_ROWS],
                north_m: &mut ghost_north[..MAX_CRDA_ROWS],
                ground_speed_mps: &mut ghost_speed[..MAX_CRDA_ROWS],
            },
        )
        .expect("256-row engineering cap is accepted");
        assert_eq!(usize::from(at_cap.rows), MAX_CRDA_ROWS);
        let too_many_east = vec![0.0; MAX_CRDA_ROWS + 1];
        let too_many_north = vec![0.0; MAX_CRDA_ROWS + 1];
        let too_many_speed = vec![100.0; MAX_CRDA_ROWS + 1];
        assert_eq!(
            project_crda_ghosts(
                CrdaTransform {
                    source: line,
                    target: line,
                    target_offset_m: 0.0,
                },
                CrdaParents {
                    east_m: &too_many_east,
                    north_m: &too_many_north,
                    ground_speed_mps: &too_many_speed,
                },
                CrdaGhosts {
                    east_m: &mut ghost_east,
                    north_m: &mut ghost_north,
                    ground_speed_mps: &mut ghost_speed,
                },
            ),
            Err(CrdaError::Capacity)
        );
        assert_eq!(ghost_east[MAX_CRDA_ROWS], 42.0);
        assert_eq!(ghost_north[MAX_CRDA_ROWS], 43.0);
        assert_eq!(ghost_speed[MAX_CRDA_ROWS], 44.0);
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
                lead_runway_id: &runway,
                trail_runway_id: &runway,
                lead_end_name: &end,
                trail_end_name: &end,
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
        let scan = [10, 11];
        let lead_pos = [2_000.0, 2_000.0];
        let trail_pos = [0.0, 0.0];
        let speed = [100.0, 100.0];
        let minimum = [1_000.0, 1_000.0];
        let runways = [1, 1];
        let ends = [*b"27L", *b"27L"];
        let mut severity = [AtpaSeverity::Clear; 2];
        let mut distance = [99; 2];
        let mut time = [99; 2];
        let pairs = AtpaPairs {
            lead_runway_id: &runways,
            trail_runway_id: &runways,
            lead_end_name: &ends,
            trail_end_name: &ends,
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
        let one_runway = [1];
        let one_end = [*b"27L"];
        assert_eq!(
            evaluate_atpa_pairs(
                10,
                AtpaPairs {
                    lead_runway_id: &one_runway,
                    trail_runway_id: &one_runway,
                    lead_end_name: &one_end,
                    trail_end_name: &one_end,
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

    // REQ: OA-RWY-023
    #[test]
    fn atpa_rejects_bad_or_unrepresentable_late_row_without_output_mutation() {
        let runways = [1, 1];
        let ends = [*b"27L", *b"27L"];
        let scans = [77, 77];
        let leads = [2_000.0, 2_000.0];
        let trails = [0.0, 0.0];
        let lead_speeds = [100.0, 100.0];
        let trail_speeds = [100.0, 100.000_001];
        let minima = [1_000.0, 1_000.0];
        let mut severity = [AtpaSeverity::Clear; 2];
        let mut distance = [77; 2];
        let mut time = [88; 2];
        assert_eq!(
            evaluate_atpa_pairs(
                77,
                AtpaPairs {
                    lead_runway_id: &runways,
                    trail_runway_id: &runways,
                    lead_end_name: &ends,
                    trail_end_name: &ends,
                    scan_ns: &scans,
                    lead_along_m: &leads,
                    trail_along_m: &trails,
                    lead_speed_mps: &lead_speeds,
                    trail_speed_mps: &trail_speeds,
                    minimum_spacing_m: &minima,
                },
                AtpaRows {
                    severity: &mut severity,
                    current_distance_milli_nm: &mut distance,
                    seconds_to_loss_milli: &mut time,
                },
            ),
            Err(AtpaError::BadInput)
        );
        assert_eq!(severity, [AtpaSeverity::Clear; 2]);
        assert_eq!(distance, [77; 2]);
        assert_eq!(time, [88; 2]);

        let speeds = [100.0, f64::NAN];
        assert_eq!(
            evaluate_atpa_pairs(
                77,
                AtpaPairs {
                    lead_runway_id: &runways,
                    trail_runway_id: &runways,
                    lead_end_name: &ends,
                    trail_end_name: &ends,
                    scan_ns: &scans,
                    lead_along_m: &leads,
                    trail_along_m: &trails,
                    lead_speed_mps: &speeds,
                    trail_speed_mps: &trail_speeds,
                    minimum_spacing_m: &minima,
                },
                AtpaRows {
                    severity: &mut severity,
                    current_distance_milli_nm: &mut distance,
                    seconds_to_loss_milli: &mut time,
                },
            ),
            Err(AtpaError::BadInput)
        );
        assert_eq!(severity, [AtpaSeverity::Clear; 2]);
        assert_eq!(distance, [77; 2]);
        assert_eq!(time, [88; 2]);
    }

    // REQ: OA-RWY-023
    #[test]
    fn atpa_accepts_all_32640_pairs_and_refuses_pair_32641() {
        let count = MAX_ATPA_PAIRS;
        let runway = vec![1; count];
        let end = vec![*b"27L"; count];
        let scan = vec![1; count];
        let lead = vec![2_000.0; count];
        let trail = vec![0.0; count];
        let speed = vec![100.0; count];
        let minimum = vec![1_000.0; count];
        let mut severity = vec![AtpaSeverity::Clear; count];
        let mut distance = vec![0; count];
        let mut time = vec![0; count];
        let accepted = evaluate_atpa_pairs(
            1,
            AtpaPairs {
                lead_runway_id: &runway,
                trail_runway_id: &runway,
                lead_end_name: &end,
                trail_end_name: &end,
                scan_ns: &scan,
                lead_along_m: &lead,
                trail_along_m: &trail,
                lead_speed_mps: &speed,
                trail_speed_mps: &speed,
                minimum_spacing_m: &minimum,
            },
            AtpaRows {
                severity: &mut severity,
                current_distance_milli_nm: &mut distance,
                seconds_to_loss_milli: &mut time,
            },
        )
        .expect("all pair slots are accepted");
        assert_eq!(usize::from(accepted.examined), MAX_ATPA_PAIRS);
        let too_many_runway = vec![1; count + 1];
        let too_many_end = vec![*b"27L"; count + 1];
        let too_many_scan = vec![1; count + 1];
        let too_many_position = vec![2_000.0; count + 1];
        let too_many_trail = vec![0.0; count + 1];
        let too_many_speed = vec![100.0; count + 1];
        let too_many_minimum = vec![1_000.0; count + 1];
        let before = severity.clone();
        assert_eq!(
            evaluate_atpa_pairs(
                1,
                AtpaPairs {
                    lead_runway_id: &too_many_runway,
                    trail_runway_id: &too_many_runway,
                    lead_end_name: &too_many_end,
                    trail_end_name: &too_many_end,
                    scan_ns: &too_many_scan,
                    lead_along_m: &too_many_position,
                    trail_along_m: &too_many_trail,
                    lead_speed_mps: &too_many_speed,
                    trail_speed_mps: &too_many_speed,
                    minimum_spacing_m: &too_many_minimum,
                },
                AtpaRows {
                    severity: &mut severity,
                    current_distance_milli_nm: &mut distance,
                    seconds_to_loss_milli: &mut time,
                },
            ),
            Err(AtpaError::Capacity)
        );
        assert_eq!(severity, before);
    }
}
