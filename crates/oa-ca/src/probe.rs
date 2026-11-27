//! First-slice conflict probe over caller-aligned shared-trajectory samples.
//!
//! The probe does not forecast aircraft motion. Its caller supplies the positions emitted by
//! the shared issue-time predictor; each adjacent pair of knots is joined by the straight-line
//! segment used by `solve`. The result is bounded to 20 minutes and uses the requested 5 NM / 1,000
//! ft engineering thresholds. Those values are not facility minima or an operational claim.

use oa_geo::{wgs84, LatLon};

use crate::solve::{
    closest_time, horizontal_miss, horizontal_span, meet, vertical_miss, vertical_span,
};

pub const PROBE_MAX_SAMPLES: usize = 553;
pub const PROBE_HORIZON_MS: i64 = 1_200_000;
pub const PROBE_MAX_GAP_MS: i64 = 30_000;
pub const PROBE_HORIZONTAL_M: f64 = 9_260.0;
pub const PROBE_VERTICAL_FT: f64 = 1_000.0;
pub const NO_PROBE_TIME_MS: i64 = i64::MIN;
pub const NO_PROBE_DISTANCE_M: f64 = -1.0;
pub const NO_PROBE_VERTICAL_FT: f64 = -1.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ProbeStatus {
    Clear = 0,
    Loss = 1,
    Unavailable = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ProbeRefusal {
    None = 0,
    SameAircraft = 1,
    InvalidIdentity = 2,
    InvalidTimeAxis = 3,
    InvalidPosition = 4,
    UnknownAltitude = 5,
    ProjectionOutOfBounds = 6,
    MissingTrack = 7,
}

/// Shared-predictor samples in structure-of-arrays form. Capacity is the union of 41 uniform
/// 30-second samples and up to 256 actual route vertices from each flight. Times are absolute
/// Unix milliseconds, positions are degrees × 1e7, altitude feet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct ProbeTrajectory {
    pub gufi: [u8; 16],
    pub aircraft_address: u32,
    pub centre: [u8; 4],
    pub plan_id: [u8; 12],
    pub reserved_key_align: [u8; 4],
    pub state_ms: i64,
    pub sample_count: u16,
    pub reserved_sample_count: [u8; 6],
    pub time_ms: [i64; PROBE_MAX_SAMPLES],
    pub lat_e7: [i32; PROBE_MAX_SAMPLES],
    pub lon_e7: [i32; PROBE_MAX_SAMPLES],
    pub altitude_ft: [i32; PROBE_MAX_SAMPLES],
}
const _: () = assert!(core::mem::size_of::<ProbeTrajectory>() == 11_120);

#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct ProbeResult {
    pub status: ProbeStatus,
    pub refusal: ProbeRefusal,
    pub examined_segments: u16,
    pub reserved_header: [u8; 4],
    pub first_gufi: [u8; 16],
    pub first_aircraft_address: u32,
    pub second_gufi: [u8; 16],
    pub second_aircraft_address: u32,
    pub first_loss_ms: i64,
    pub cpa_ms: i64,
    pub cpa_horizontal_m: f64,
    pub cpa_vertical_ft: f64,
    pub reserved_tail: [u8; 8],
}
const _: () = assert!(core::mem::size_of::<ProbeResult>() == 88);

fn result(
    a: &ProbeTrajectory,
    b: &ProbeTrajectory,
    status: ProbeStatus,
    refusal: ProbeRefusal,
) -> ProbeResult {
    ProbeResult {
        status,
        refusal,
        examined_segments: 0,
        reserved_header: [0; 4],
        first_gufi: a.gufi,
        first_aircraft_address: a.aircraft_address,
        second_gufi: b.gufi,
        second_aircraft_address: b.aircraft_address,
        first_loss_ms: NO_PROBE_TIME_MS,
        cpa_ms: NO_PROBE_TIME_MS,
        cpa_horizontal_m: NO_PROBE_DISTANCE_M,
        cpa_vertical_ft: NO_PROBE_VERTICAL_FT,
        reserved_tail: [0; 8],
    }
}

fn identity_ok(t: &ProbeTrajectory) -> bool {
    t.gufi != [0; 16]
        && t.aircraft_address != 0
        && t.aircraft_address <= 0x01ff_ffff
        && padded_ascii(&t.centre)
        && padded_ascii(&t.plan_id)
        && t.reserved_key_align.iter().all(|b| *b == 0)
}

fn padded_ascii(bytes: &[u8]) -> bool {
    let mut padding = false;
    let mut present = false;
    for byte in bytes {
        if *byte == 0 {
            padding = true;
        } else if padding || !byte.is_ascii_graphic() {
            return false;
        } else {
            present = true;
        }
    }
    present
}

fn position_ok(t: &ProbeTrajectory, count: usize) -> bool {
    t.lat_e7
        .iter()
        .take(count)
        .all(|v| (-900_000_000..=900_000_000).contains(v))
        && t.lon_e7
            .iter()
            .take(count)
            .all(|v| (-1_800_000_000..=1_800_000_000).contains(v))
}

fn time_axis_ok(t: &ProbeTrajectory, count: usize) -> bool {
    let Some(times) = t.time_ms.get(..count) else {
        return false;
    };
    times.first().copied() == Some(t.state_ms)
        && times
            .last()
            .and_then(|last| last.checked_sub(t.state_ms))
            .is_some_and(|span| span == PROBE_HORIZON_MS)
        && times
            .windows(2)
            .all(|pair| match (pair.first(), pair.get(1)) {
                (Some(a), Some(b)) => b
                    .checked_sub(*a)
                    .is_some_and(|gap| (1..=PROBE_MAX_GAP_MS).contains(&gap)),
                _ => false,
            })
}

fn local(origin: LatLon, point: LatLon) -> Option<(f64, f64)> {
    let separation = wgs84().inverse(origin, point).ok()?;
    if !separation.distance_m.is_finite() || separation.distance_m > 1_000_000.0 {
        return None;
    }
    let radians = separation.azimuth1_deg.to_radians();
    let (sin, cos) = oa_math::sin_cos(radians);
    Some((separation.distance_m * sin, separation.distance_m * cos))
}

fn quantized_time_ms(start_ms: i64, seconds: f64) -> Option<i64> {
    if !seconds.is_finite() || seconds < 0.0 || seconds > PROBE_HORIZON_MS as f64 / 1_000.0 {
        return None;
    }
    let offset = oa_ieee::round(seconds * 1_000.0);
    if !offset.is_finite() || offset < 0.0 || offset > PROBE_HORIZON_MS as f64 {
        return None;
    }
    start_ms.checked_add(offset as i64)
}

fn first_inside_ms(start_ms: i64, seconds: f64) -> Option<i64> {
    if !seconds.is_finite() || seconds < 0.0 || seconds > PROBE_HORIZON_MS as f64 / 1_000.0 {
        return None;
    }
    // The OA-CA loss edges are strict. Report the first integer millisecond after the open entry
    // boundary; a loss already present at the knot is represented by `Span::now` and time zero.
    let offset = oa_ieee::floor(seconds * 1_000.0) + 1.0;
    if !offset.is_finite() || offset < 1.0 || offset > PROBE_HORIZON_MS as f64 {
        return None;
    }
    start_ms.checked_add(offset as i64)
}

/// Compare two forecasts emitted by the shared issue-time predictor. The local chart is
/// azimuthal-equidistant from the first aircraft at each segment start. Segment velocity is the
/// straight line between adjacent shared knots; CPA is quantized to the nearest millisecond, ties
/// away from zero, and the first strict loss is the first integer millisecond after its boundary.
/// This is a probe over predictor knots, not an independent trajectory predictor.
pub fn probe_pair(a: &ProbeTrajectory, b: &ProbeTrajectory) -> ProbeResult {
    if !identity_ok(a) || !identity_ok(b) {
        return result(
            a,
            b,
            ProbeStatus::Unavailable,
            ProbeRefusal::InvalidIdentity,
        );
    }
    if a.gufi == b.gufi || a.aircraft_address == b.aircraft_address {
        return result(a, b, ProbeStatus::Unavailable, ProbeRefusal::SameAircraft);
    }
    let count = usize::from(a.sample_count);
    if a.reserved_sample_count != [0; 6]
        || b.reserved_sample_count != [0; 6]
        || !(2..=PROBE_MAX_SAMPLES).contains(&count)
        || a.sample_count != b.sample_count
        || a.state_ms != b.state_ms
        || !time_axis_ok(a, count)
        || !time_axis_ok(b, count)
        || a.time_ms.get(..count) != b.time_ms.get(..count)
    {
        return result(
            a,
            b,
            ProbeStatus::Unavailable,
            ProbeRefusal::InvalidTimeAxis,
        );
    }
    if !position_ok(a, count) || !position_ok(b, count) {
        return result(
            a,
            b,
            ProbeStatus::Unavailable,
            ProbeRefusal::InvalidPosition,
        );
    }
    if a.altitude_ft
        .get(..count)
        .is_none_or(|values| values.contains(&i32::MIN))
        || b.altitude_ft
            .get(..count)
            .is_none_or(|values| values.contains(&i32::MIN))
    {
        return result(
            a,
            b,
            ProbeStatus::Unavailable,
            ProbeRefusal::UnknownAltitude,
        );
    }
    let mut out = result(a, b, ProbeStatus::Clear, ProbeRefusal::None);
    let mut best_cpa_m = f64::INFINITY;
    for index in 0..(count - 1) {
        let (
            Some(&alat0),
            Some(&alon0),
            Some(&alat1),
            Some(&alon1),
            Some(&blat0),
            Some(&blon0),
            Some(&blat1),
            Some(&blon1),
        ) = (
            a.lat_e7.get(index),
            a.lon_e7.get(index),
            a.lat_e7.get(index + 1),
            a.lon_e7.get(index + 1),
            b.lat_e7.get(index),
            b.lon_e7.get(index),
            b.lat_e7.get(index + 1),
            b.lon_e7.get(index + 1),
        )
        else {
            return result(
                a,
                b,
                ProbeStatus::Unavailable,
                ProbeRefusal::InvalidPosition,
            );
        };
        let (Some(&time0), Some(&time1), Some(&aa0), Some(&aa1), Some(&ba0), Some(&ba1)) = (
            a.time_ms.get(index),
            a.time_ms.get(index + 1),
            a.altitude_ft.get(index),
            a.altitude_ft.get(index + 1),
            b.altitude_ft.get(index),
            b.altitude_ft.get(index + 1),
        ) else {
            return result(
                a,
                b,
                ProbeStatus::Unavailable,
                ProbeRefusal::InvalidPosition,
            );
        };
        let (Ok(a0), Ok(a1), Ok(b0), Ok(b1)) = (
            LatLon::new(f64::from(alat0) * 1e-7, f64::from(alon0) * 1e-7),
            LatLon::new(f64::from(alat1) * 1e-7, f64::from(alon1) * 1e-7),
            LatLon::new(f64::from(blat0) * 1e-7, f64::from(blon0) * 1e-7),
            LatLon::new(f64::from(blat1) * 1e-7, f64::from(blon1) * 1e-7),
        ) else {
            return result(
                a,
                b,
                ProbeStatus::Unavailable,
                ProbeRefusal::InvalidPosition,
            );
        };
        let (Some((bx0, by0)), Some((ax1, ay1)), Some((bx1, by1))) =
            (local(a0, b0), local(a0, a1), local(a0, b1))
        else {
            return result(
                a,
                b,
                ProbeStatus::Unavailable,
                ProbeRefusal::ProjectionOutOfBounds,
            );
        };
        let duration_ms = time1 - time0;
        let duration_s = duration_ms as f64 / 1_000.0;
        let (rx, ry) = (bx0, by0);
        let (avx, avy) = (ax1 / duration_s, ay1 / duration_s);
        let (bvx, bvy) = ((bx1 - bx0) / duration_s, (by1 - by0) / duration_s);
        let (vx, vy) = (bvx - avx, bvy - avy);
        if ![rx, ry, vx, vy].iter().all(|v| v.is_finite())
            || rx.abs() > 1_000_000.0
            || ry.abs() > 1_000_000.0
            || vx.abs() > 4_000.0
            || vy.abs() > 4_000.0
        {
            return result(
                a,
                b,
                ProbeStatus::Unavailable,
                ProbeRefusal::ProjectionOutOfBounds,
            );
        }
        let dz = f64::from(ba0) - f64::from(aa0);
        let dz_end = f64::from(ba1) - f64::from(aa1);
        let dvz = (dz_end - dz) / duration_s;
        if dz.abs() > 1_000_000.0 || dz_end.abs() > 1_000_000.0 || dvz.abs() > 2_000.0 {
            return result(
                a,
                b,
                ProbeStatus::Unavailable,
                ProbeRefusal::ProjectionOutOfBounds,
            );
        }
        if let Some(horizontal) = horizontal_span(rx, ry, vx, vy, PROBE_HORIZONTAL_M, duration_s) {
            if let Some(vertical) = vertical_span(dz, dvz, PROBE_VERTICAL_FT, duration_s) {
                if let Some(loss) = meet(&horizontal, &vertical) {
                    let loss_offset_ms = if loss.now {
                        Some(0)
                    } else {
                        match first_inside_ms(0, loss.lo) {
                            Some(value) if value <= duration_ms => Some(value),
                            Some(_) => None,
                            None => {
                                return result(
                                    a,
                                    b,
                                    ProbeStatus::Unavailable,
                                    ProbeRefusal::InvalidTimeAxis,
                                );
                            }
                        }
                    };
                    if let Some(loss_offset_ms) = loss_offset_ms {
                        let loss_ms = match time0.checked_add(loss_offset_ms) {
                            Some(value) => value,
                            None => {
                                return result(
                                    a,
                                    b,
                                    ProbeStatus::Unavailable,
                                    ProbeRefusal::InvalidTimeAxis,
                                );
                            }
                        };
                        if out.first_loss_ms == NO_PROBE_TIME_MS || loss_ms < out.first_loss_ms {
                            out.first_loss_ms = loss_ms;
                        }
                    }
                }
            }
        }
        let tc = closest_time(rx, ry, vx, vy, duration_s);
        let hm = horizontal_miss(rx, ry, vx, vy, duration_s);
        let vm = vertical_miss(dz, dvz, duration_s);
        if !hm.is_finite() || !vm.is_finite() || hm < 0.0 || vm < 0.0 {
            return result(
                a,
                b,
                ProbeStatus::Unavailable,
                ProbeRefusal::ProjectionOutOfBounds,
            );
        }
        if hm < best_cpa_m {
            let cpa = match quantized_time_ms(time0, tc) {
                Some(value) => value,
                None => {
                    return result(
                        a,
                        b,
                        ProbeStatus::Unavailable,
                        ProbeRefusal::InvalidTimeAxis,
                    );
                }
            };
            best_cpa_m = hm;
            out.cpa_ms = cpa;
            out.cpa_horizontal_m = hm;
            out.cpa_vertical_ft = (dz + dvz * tc).abs();
        }
        out.examined_segments = out.examined_segments.saturating_add(1);
    }
    if out.first_loss_ms != NO_PROBE_TIME_MS {
        out.status = ProbeStatus::Loss;
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing, clippy::unwrap_used)]

    use super::*;

    fn trajectory(
        address: u32,
        gufi_byte: u8,
        lon: [i32; 4],
        altitude: [i32; 4],
    ) -> ProbeTrajectory {
        let mut time_ms = [0; PROBE_MAX_SAMPLES];
        let lat_e7 = [0; PROBE_MAX_SAMPLES];
        let mut lon_e7 = [0; PROBE_MAX_SAMPLES];
        let mut altitude_ft = [0; PROBE_MAX_SAMPLES];
        for sample in 0..=40 {
            let (segment, numerator, denominator) = if sample <= 10 {
                (0, sample, 10)
            } else if sample <= 20 {
                (1, sample - 10, 10)
            } else {
                (2, sample - 20, 20)
            };
            let interpolate = |values: [i32; 4]| {
                i64::from(values[segment])
                    + (i64::from(values[segment + 1]) - i64::from(values[segment]))
                        * i64::try_from(numerator).unwrap()
                        / i64::from(denominator)
            };
            time_ms[sample] = 1_000_000 + i64::try_from(sample).unwrap() * PROBE_MAX_GAP_MS;
            lon_e7[sample] = i32::try_from(interpolate(lon)).unwrap();
            altitude_ft[sample] = i32::try_from(interpolate(altitude)).unwrap();
        }
        ProbeTrajectory {
            gufi: [gufi_byte; 16],
            aircraft_address: address,
            centre: *b"KZNY",
            plan_id: *b"PLAN1\0\0\0\0\0\0\0",
            reserved_key_align: [0; 4],
            state_ms: 1_000_000,
            sample_count: 41,
            reserved_sample_count: [0; 6],
            time_ms,
            lat_e7,
            lon_e7,
            altitude_ft,
        }
    }

    #[test]
    fn converging_pair_reports_first_loss_and_cpa() {
        let a = trajectory(1, 1, [0; 4], [10_000; 4]);
        let b = trajectory(
            2,
            2,
            [2_000_000, 500_000, -500_000, -2_500_000],
            [10_000; 4],
        );
        let out = probe_pair(&a, &b);
        assert_eq!(out.status, ProbeStatus::Loss);
        assert_ne!(out.first_loss_ms, NO_PROBE_TIME_MS);
        assert_ne!(out.cpa_ms, NO_PROBE_TIME_MS);
        assert!(out.cpa_horizontal_m < PROBE_HORIZONTAL_M);
    }

    #[test]
    fn diverging_pair_is_clear_but_still_reports_cpa() {
        let a = trajectory(1, 1, [0; 4], [10_000; 4]);
        let b = trajectory(
            2,
            2,
            [2_000_000, 3_000_000, 4_000_000, 6_000_000],
            [10_000; 4],
        );
        let out = probe_pair(&a, &b);
        assert_eq!(out.status, ProbeStatus::Clear);
        assert_eq!(out.first_loss_ms, NO_PROBE_TIME_MS);
        assert_ne!(out.cpa_ms, NO_PROBE_TIME_MS);
    }

    #[test]
    fn vertical_only_closure_reports_loss() {
        let a = trajectory(1, 1, [0; 4], [10_000; 4]);
        let b = trajectory(2, 2, [50_000; 4], [12_000, 11_000, 10_000, 8_000]);
        let out = probe_pair(&a, &b);
        assert_eq!(out.status, ProbeStatus::Loss);
        assert_ne!(out.first_loss_ms, NO_PROBE_TIME_MS);
        assert!(out.cpa_horizontal_m < PROBE_HORIZONTAL_M);
    }

    #[test]
    fn identical_aircraft_identity_is_unavailable() {
        let a = trajectory(1, 1, [0; 4], [10_000; 4]);
        let b = trajectory(1, 2, [0; 4], [10_000; 4]);
        let out = probe_pair(&a, &b);
        assert_eq!(out.status, ProbeStatus::Unavailable);
        assert_eq!(out.refusal, ProbeRefusal::SameAircraft);
    }

    // REQ: OA-ENC-023
    #[test]
    fn pair_requires_full_horizon_and_matching_sample_epochs() {
        let a = trajectory(1, 1, [0; 4], [10_000; 4]);
        let mut short = trajectory(2, 2, [0; 4], [10_000; 4]);
        short.sample_count = 40;
        let truncated = probe_pair(&a, &short);
        assert_eq!(truncated.status, ProbeStatus::Unavailable);
        assert_eq!(truncated.refusal, ProbeRefusal::InvalidTimeAxis);

        let mut sparse = trajectory(2, 2, [0; 4], [10_000; 4]);
        sparse.sample_count = 2;
        sparse.time_ms[1] = sparse.state_ms + PROBE_MAX_GAP_MS;
        let sparse_result = probe_pair(&a, &sparse);
        assert_eq!(sparse_result.status, ProbeStatus::Unavailable);
        assert_eq!(sparse_result.refusal, ProbeRefusal::InvalidTimeAxis);

        let mut wide_gap = trajectory(2, 2, [0; 4], [10_000; 4]);
        wide_gap.sample_count = 2;
        wide_gap.time_ms[1] = wide_gap.state_ms + PROBE_HORIZON_MS;
        let wide_gap_result = probe_pair(&a, &wide_gap);
        assert_eq!(wide_gap_result.status, ProbeStatus::Unavailable);
        assert_eq!(wide_gap_result.refusal, ProbeRefusal::InvalidTimeAxis);

        let mut shifted = trajectory(2, 2, [0; 4], [10_000; 4]);
        shifted.state_ms += 1_000;
        for time in shifted
            .time_ms
            .iter_mut()
            .take(usize::from(shifted.sample_count))
        {
            *time += 1_000;
        }
        let shifted_result = probe_pair(&a, &shifted);
        assert_eq!(shifted_result.status, ProbeStatus::Unavailable);
        assert_eq!(shifted_result.refusal, ProbeRefusal::InvalidTimeAxis);
    }
}
