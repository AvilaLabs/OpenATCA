//! Bounded, pure active-volume query over shared trajectory knots (OA-ENC-023).

use super::geom::{
    area_edge_count, area_edge_distance_e7, area_inside_e7, distance_e7, interpolate_e7, Area,
};

pub const SUA_KNOTS: usize = 553;
pub const SUA_HORIZON_MS: i64 = 1_200_000;
pub const MAX_SUA_PROBE_EDGES: usize = 4_096;
/// Up to three clipped segment intervals add at most three subdivision points, plus one endpoint.
pub const MAX_SUA_PROBE_POINTS: usize = SUA_KNOTS + 4;
/// Containment and edge-distance each scan no more than the fixed edge budget per query.
pub const MAX_SUA_EDGE_WORK: usize = 2 * MAX_SUA_PROBE_POINTS * MAX_SUA_PROBE_EDGES;
pub const SUA_SUBSTEP_MS: i64 = 30_000;
/// E7 coordinates are centimetre-scale locally; avoid interpreting a boundary-quantized point
/// as an unambiguous interior entry.
pub const SUA_BOUNDARY_AMBIGUITY_M: f64 = 0.02;
pub const NO_SUA_ENTRY_MS: i64 = i64::MIN;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum SuaStatus {
    Clear = 0,
    Entry = 1,
    Unavailable = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum SuaRefusal {
    None = 0,
    UnknownSchedule = 1,
    InvalidWindow = 2,
    UnknownAltitude = 3,
    GeometryTooLarge = 4,
    InvalidPosition = 5,
    UnsampledPossibleEntry = 6,
}

/// Plain SoA trajectory view; samples include 30-second times and route vertices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct SuaTrajectoryKnots {
    pub state_ms: i64,
    pub sample_count: u16,
    pub reserved_sample_count: [u8; 6],
    pub time_ms: [i64; SUA_KNOTS],
    pub lat_e7: [i32; SUA_KNOTS],
    pub lon_e7: [i32; SUA_KNOTS],
    pub altitude_msl_ft: [i32; SUA_KNOTS],
}
const _: () = assert!(core::mem::size_of::<SuaTrajectoryKnots>() == 11_080);

/// Bounded result with explicit refusal. `entry_ms` is the first detected interpolated point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct SuaProbeResult {
    pub status: SuaStatus,
    pub refusal: SuaRefusal,
    pub reserved: [u8; 6],
    pub entry_ms: i64,
}
const _: () = assert!(core::mem::size_of::<SuaProbeResult>() == 16);

fn unavailable(refusal: SuaRefusal) -> SuaProbeResult {
    SuaProbeResult {
        status: SuaStatus::Unavailable,
        refusal,
        reserved: [0; 6],
        entry_ms: NO_SUA_ENTRY_MS,
    }
}

/// Query a caller-supplied active schedule and MSL altitude band against bounded geometry.
/// The complete 20-minute horizon is required. Between shared predictor knots, position follows
/// the geodesic connecting those emitted knots and altitude is linearly interpolated. Subdivision
/// finds interior entries; boundary-quantized or otherwise unresolved points return unavailable.
pub fn probe_sua(
    trajectory: &SuaTrajectoryKnots,
    area: &Area,
    schedule_known: bool,
    active_start_ms: i64,
    active_end_ms: i64,
    floor_msl_ft: i32,
    ceiling_msl_ft: i32,
) -> SuaProbeResult {
    if !schedule_known {
        return unavailable(SuaRefusal::UnknownSchedule);
    }
    if active_start_ms >= active_end_ms {
        return unavailable(SuaRefusal::InvalidWindow);
    }
    let count = usize::from(trajectory.sample_count);
    if trajectory.reserved_sample_count != [0; 6] || !(2..=SUA_KNOTS).contains(&count) {
        return unavailable(SuaRefusal::InvalidWindow);
    }
    if matches!(
        floor_msl_ft,
        crate::core::model::ALT_NONE
            | crate::core::model::ALT_GROUND
            | crate::core::model::ALT_UNKNOWN
            | crate::core::model::ALT_UNLIMITED
    ) || matches!(
        ceiling_msl_ft,
        crate::core::model::ALT_NONE
            | crate::core::model::ALT_GROUND
            | crate::core::model::ALT_UNKNOWN
            | crate::core::model::ALT_UNLIMITED
    ) || floor_msl_ft > ceiling_msl_ft
        || trajectory
            .altitude_msl_ft
            .get(..count)
            .is_none_or(|values| values.contains(&i32::MIN))
    {
        return unavailable(SuaRefusal::UnknownAltitude);
    }
    let edge_count = area_edge_count(area);
    let Some(edge_work) = edge_count.checked_mul(2 * MAX_SUA_PROBE_POINTS) else {
        return unavailable(SuaRefusal::GeometryTooLarge);
    };
    if edge_count > MAX_SUA_PROBE_EDGES || edge_work > MAX_SUA_EDGE_WORK {
        return unavailable(SuaRefusal::GeometryTooLarge);
    }
    let Some(times) = trajectory.time_ms.get(..count) else {
        return unavailable(SuaRefusal::InvalidWindow);
    };
    let Some(horizon_start) = times.first().copied() else {
        return unavailable(SuaRefusal::InvalidWindow);
    };
    if horizon_start != trajectory.state_ms
        || times
            .last()
            .and_then(|last| last.checked_sub(trajectory.state_ms))
            .is_none_or(|span| span != SUA_HORIZON_MS)
        || times
            .windows(2)
            .any(|pair| match (pair.first(), pair.get(1)) {
                (Some(a), Some(b)) => b
                    .checked_sub(*a)
                    .is_none_or(|gap| !(1..=SUA_SUBSTEP_MS).contains(&gap)),
                _ => true,
            })
    {
        return unavailable(SuaRefusal::InvalidWindow);
    }
    let Some(horizon_end) = horizon_start.checked_add(SUA_HORIZON_MS) else {
        return unavailable(SuaRefusal::InvalidWindow);
    };
    if active_end_ms <= horizon_start || active_start_ms >= horizon_end {
        return SuaProbeResult {
            status: SuaStatus::Clear,
            refusal: SuaRefusal::None,
            reserved: [0; 6],
            entry_ms: NO_SUA_ENTRY_MS,
        };
    }
    let mut all_active_segments_certified = true;
    let mut previous: Option<(i64, i32, i32, i32, bool, f64)> = None;
    for index in 0..(count - 1) {
        let (
            Some(&t0),
            Some(&t1),
            Some(&lat0),
            Some(&lon0),
            Some(&lat1),
            Some(&lon1),
            Some(&alt0),
            Some(&alt1),
        ) = (
            trajectory.time_ms.get(index),
            trajectory.time_ms.get(index + 1),
            trajectory.lat_e7.get(index),
            trajectory.lon_e7.get(index),
            trajectory.lat_e7.get(index + 1),
            trajectory.lon_e7.get(index + 1),
            trajectory.altitude_msl_ft.get(index),
            trajectory.altitude_msl_ft.get(index + 1),
        )
        else {
            return unavailable(SuaRefusal::InvalidPosition);
        };
        let start = t0.max(active_start_ms);
        let end = t1.min(active_end_ms);
        if start >= end {
            continue;
        }
        let segment_duration = t1 - t0;
        let active_duration = end - start;
        let subdivisions = ((active_duration + SUA_SUBSTEP_MS - 1) / SUA_SUBSTEP_MS).max(1);
        for sample_index in 0..=subdivisions {
            let elapsed = active_duration * sample_index / subdivisions;
            let time = start + elapsed;
            let fraction = (time - t0) as f64 / segment_duration as f64;
            let (lat, lon) = match interpolate_e7(lat0, lon0, lat1, lon1, fraction) {
                Ok(value) => value,
                Err(_) => return unavailable(SuaRefusal::InvalidPosition),
            };
            let altitude_value =
                oa_ieee::round(f64::from(alt0) + (f64::from(alt1) - f64::from(alt0)) * fraction);
            if !altitude_value.is_finite()
                || altitude_value < f64::from(i32::MIN)
                || altitude_value > f64::from(i32::MAX)
            {
                return unavailable(SuaRefusal::UnknownAltitude);
            }
            let altitude = altitude_value as i32;
            if previous.is_some_and(|(prior_time, ..)| prior_time == time) {
                continue;
            }
            let inside = match area_inside_e7(area, lat, lon) {
                Ok(value) => value,
                Err(_) => return unavailable(SuaRefusal::InvalidPosition),
            };
            let edge_distance = match area_edge_distance_e7(area, lat, lon) {
                Ok(value) => value,
                Err(_) => return unavailable(SuaRefusal::InvalidPosition),
            };
            if time < active_end_ms && (floor_msl_ft..=ceiling_msl_ft).contains(&altitude) && inside
            {
                if edge_distance <= SUA_BOUNDARY_AMBIGUITY_M {
                    return unavailable(SuaRefusal::UnsampledPossibleEntry);
                }
                return SuaProbeResult {
                    status: SuaStatus::Entry,
                    refusal: SuaRefusal::None,
                    reserved: [0; 6],
                    entry_ms: time,
                };
            }
            if let Some((
                prior_time,
                prior_lat,
                prior_lon,
                prior_alt,
                prior_inside,
                prior_edge_distance,
            )) = previous
            {
                let movement = match distance_e7(prior_lat, prior_lon, lat, lon) {
                    Ok(value) => value,
                    Err(_) => return unavailable(SuaRefusal::InvalidPosition),
                };
                let altitude_outside_same_side = (prior_alt < floor_msl_ft
                    && altitude < floor_msl_ft)
                    || (prior_alt > ceiling_msl_ft && altitude > ceiling_msl_ft);
                let geometry_outside_for_interval = !prior_inside
                    && !inside
                    && prior_edge_distance > movement
                    && edge_distance > movement;
                if time <= active_end_ms
                    && prior_time < active_end_ms
                    && !altitude_outside_same_side
                    && !geometry_outside_for_interval
                {
                    all_active_segments_certified = false;
                }
            }
            previous = Some((time, lat, lon, altitude, inside, edge_distance));
        }
    }
    if all_active_segments_certified {
        SuaProbeResult {
            status: SuaStatus::Clear,
            refusal: SuaRefusal::None,
            reserved: [0; 6],
            entry_ms: NO_SUA_ENTRY_MS,
        }
    } else {
        unavailable(SuaRefusal::UnsampledPossibleEntry)
    }
}
