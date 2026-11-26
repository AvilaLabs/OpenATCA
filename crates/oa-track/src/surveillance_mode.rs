//! Explicit FAA surveillance-mode minimum selection (core, A1).
//!
//! The caller supplies the facility's selected mode, adaptation, target reliability and ISR
//! state. This module does not guess a reliability threshold or select a mode from sensor
//! availability. It implements only the published JO 7110.65BB 5-5-4 minima and the
//! corresponding JO 7210.3EE 3-6-7 multi-sensor restriction. Output is a policy result for the
//! caller to apply; the regional session may display the selected mode, but this module does not
//! set the conflict-alert engine's volume or claim to implement facility-specific adaptation.
//!
//! A coasted target is always marked ineligible for separation. A controller-initiated coast
//! also requires the caller to identify a flight-plan-aided track (FLAT); missing that evidence
//! is an explicit refusal. No flight-plan identity or association is synthesized here.

use oa_geo::{wgs84, LatLon};

/// Modes with different published horizontal minima. Caller-selected adaptation, not inferred.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SurveillanceMode {
    TerminalSingleSensor = 1,
    TerminalFusion = 2,
    StarsMultiSensor = 3,
    Eram = 4,
    EramMosaic = 5,
}

/// Radar category used by the published single-sensor and preferred-radar distance rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RadarClass {
    AsrOrDtas = 1,
    MonopulseSecondary = 2,
}

/// Caller-reported target source. Unknown is never treated as reliable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TargetSource {
    Radar = 1,
    AdsB = 2,
    Wam = 3,
    Unknown = 4,
}

/// Reliability evidence supplied by the selected sensor adapter. Unknown never enables 3 NM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ReliabilityEvidence {
    Unknown = 0,
    Reliable = 1,
    Unreliable = 2,
}

/// ISR evidence from the selected automation mode. Unknown remains distinct from an affirmed
/// no-ISR state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IsrEvidence {
    Unknown = 0,
    Clear = 1,
    Required = 2,
}

/// Fixed adaptation supplied by the facility configuration. Coordinates are degrees * 1e7.
/// A7.2: cold configuration; exact size guarded because it crosses the core boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct ModeAdaptation {
    pub mode: SurveillanceMode,
    pub radar_class: RadarClass,
    pub preferred_radar_lat_e7: i32,
    pub preferred_radar_lon_e7: i32,
    pub has_preferred_radar: u8,
    pub track_based_display: u8,
    pub area_3nm_adapted: u8,
    pub map_3nm_visible: u8,
}
const _: () = assert!(core::mem::size_of::<ModeAdaptation>() == 16);

/// State which must come from the track/mode adapter; no hidden clock or reliability policy.
/// Flight level is in hundreds of feet (e.g. 230 means FL230); `i16::MIN` means unavailable.
/// A7.2: transient input; exact size guarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct TargetModeState {
    pub lat_e7: i32,
    pub lon_e7: i32,
    pub flight_level_hundreds: i16,
    pub source: TargetSource,
    pub reliability: ReliabilityEvidence,
    pub isr: IsrEvidence,
    pub coasting: u8,
    pub controller_initiated_coast: u8,
    pub flight_plan_aided: u8,
}
const _: () = assert!(core::mem::size_of::<TargetModeState>() == 16);

/// Caller-asserted target evidence for one regional receipt epoch. `state_ns` must equal that
/// receipt's time. This row does not authenticate its claims; absence is represented by the
/// explicit `Unknown` enum values and missing-flight-level sentinel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct TargetModeEvidence {
    pub address: u32,
    pub state_ns: i64,
    pub flight_level_hundreds: i16,
    pub source: TargetSource,
    pub reliability: ReliabilityEvidence,
    pub isr: IsrEvidence,
    pub controller_initiated_coast: u8,
    pub flight_plan_aided: u8,
}
const _: () = assert!(core::mem::size_of::<TargetModeEvidence>() == 24);

/// Explicit failure; a missing prerequisite is not converted to a permissive mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ModeRefusal {
    BadBoolean = 1,
    BadPosition = 2,
    MissingPreferredRadar = 3,
    MissingFlightLevel = 4,
    UnsupportedSingleSensorSource = 6,
    ControllerCoastRequiresFlat = 7,
}

/// The selected minimum and the limits on its use. `three_nm_symbol` is a presentation request;
/// the actual FAA symbol geometry/adaptation remains a renderer integration requirement.
/// `separation_eligible` is false for every coasted row and for a controller coast without FLAT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct ModeResult {
    pub minimum_nm: u8,
    /// Candidate indicates that the rule set calls for a 3 NM target symbol. Platform-specific
    /// selection is exposed separately; JO 7110.65BB does not define ERAM symbol geometry.
    pub three_nm_symbol: u8,
    pub separation_eligible: u8,
    pub flat_aided: u8,
    /// `1` when the selected terminal FUSION mode uses the solid-circle symbol per N JO 7110.676.
    pub fusion_solid_circle: u8,
    pub reserved: [u8; 2],
    /// Metres; `u32::MAX` when no preferred-radar distance was required/available.
    pub preferred_radar_distance_m: u32,
}
const _: () = assert!(core::mem::size_of::<ModeResult>() == 12);

/// Select the published horizontal minimum for one target.
///
/// Boundaries are inclusive on the fallback side: exactly 40 NM (ASR/DTAS) or 60 NM (MSSR)
/// uses 5 NM. ERAM's 3 NM option requires FL230 or below, adapted area, visible map, reliable
/// target, and either track-based display or a preferred-radar distance below the sensor-specific
/// limit. ERAM otherwise uses 5 NM below FL600 and 10 NM at/above it. These thresholds come
/// directly from JO 7110.65BB 5-5-4; mode selection and target reliability remain caller inputs.
pub fn surveillance_mode_result(
    adaptation: ModeAdaptation,
    target: TargetModeState,
) -> Result<ModeResult, ModeRefusal> {
    if adaptation.has_preferred_radar > 1
        || adaptation.track_based_display > 1
        || adaptation.area_3nm_adapted > 1
        || adaptation.map_3nm_visible > 1
        || target.coasting > 1
        || target.controller_initiated_coast > 1
        || target.flight_plan_aided > 1
    {
        return Err(ModeRefusal::BadBoolean);
    }
    if target.controller_initiated_coast == 1
        && (target.coasting != 1 || target.flight_plan_aided != 1)
    {
        return Err(ModeRefusal::ControllerCoastRequiresFlat);
    }

    let target_position = LatLon::new(
        f64::from(target.lat_e7) * 1.0e-7,
        f64::from(target.lon_e7) * 1.0e-7,
    )
    .map_err(|_| ModeRefusal::BadPosition)?;

    if adaptation.mode == SurveillanceMode::TerminalSingleSensor
        && adaptation.has_preferred_radar != 1
    {
        return Err(ModeRefusal::MissingPreferredRadar);
    }
    let needs_distance = adaptation.mode == SurveillanceMode::TerminalSingleSensor
        || (adaptation.mode == SurveillanceMode::Eram
            && adaptation.track_based_display == 0
            && adaptation.has_preferred_radar == 1);
    let distance_m = if needs_distance {
        let radar = LatLon::new(
            f64::from(adaptation.preferred_radar_lat_e7) * 1.0e-7,
            f64::from(adaptation.preferred_radar_lon_e7) * 1.0e-7,
        )
        .map_err(|_| ModeRefusal::BadPosition)?;
        let distance = wgs84()
            .inverse(radar, target_position)
            .map_err(|_| ModeRefusal::BadPosition)?
            .distance_m;
        if !distance.is_finite() || distance < 0.0 || distance > u32::MAX as f64 {
            return Err(ModeRefusal::BadPosition);
        }
        distance as u32
    } else {
        u32::MAX
    };

    let minimum_nm = match adaptation.mode {
        SurveillanceMode::TerminalSingleSensor => {
            if target.source != TargetSource::Radar {
                return Err(ModeRefusal::UnsupportedSingleSensorSource);
            }
            let cutoff_nm = match adaptation.radar_class {
                RadarClass::AsrOrDtas => 40.0,
                RadarClass::MonopulseSecondary => 60.0,
            };
            if f64::from(distance_m) < cutoff_nm * 1852.0 {
                3
            } else {
                5
            }
        }
        SurveillanceMode::TerminalFusion => {
            if target.isr != IsrEvidence::Clear
                || target.reliability != ReliabilityEvidence::Reliable
            {
                5
            } else {
                3
            }
        }
        SurveillanceMode::StarsMultiSensor | SurveillanceMode::EramMosaic => 5,
        SurveillanceMode::Eram => {
            if target.flight_level_hundreds == i16::MIN || target.flight_level_hundreds < 0 {
                return Err(ModeRefusal::MissingFlightLevel);
            }
            let fl = target.flight_level_hundreds;
            let distance_limit_nm = match adaptation.radar_class {
                RadarClass::AsrOrDtas => 40.0,
                RadarClass::MonopulseSecondary => 60.0,
            };
            let in_preferred_range = adaptation.has_preferred_radar == 1
                && f64::from(distance_m) < distance_limit_nm * 1852.0;
            let source_can_support_3nm =
                target.source == TargetSource::Radar || target.source == TargetSource::AdsB;
            let allows_3nm = fl <= 230
                && adaptation.area_3nm_adapted == 1
                && adaptation.map_3nm_visible == 1
                && target.reliability == ReliabilityEvidence::Reliable
                && source_can_support_3nm
                && (adaptation.track_based_display == 1 || in_preferred_range);
            if allows_3nm {
                3
            } else if fl >= 600 {
                10
            } else {
                5
            }
        }
    };
    let flat_aided = target.flight_plan_aided;
    let separation_eligible =
        target.coasting == 0 && (target.controller_initiated_coast == 0 || flat_aided == 1);
    Ok(ModeResult {
        minimum_nm,
        three_nm_symbol: u8::from(minimum_nm == 3),
        separation_eligible: u8::from(separation_eligible),
        flat_aided,
        fusion_solid_circle: u8::from(adaptation.mode == SurveillanceMode::TerminalFusion),
        reserved: [0; 2],
        preferred_radar_distance_m: distance_m,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const SITE: (i32, i32) = (0, 0);

    fn adaptation(mode: SurveillanceMode) -> ModeAdaptation {
        ModeAdaptation {
            mode,
            radar_class: RadarClass::AsrOrDtas,
            preferred_radar_lat_e7: SITE.0,
            preferred_radar_lon_e7: SITE.1,
            has_preferred_radar: 1,
            track_based_display: 0,
            area_3nm_adapted: 1,
            map_3nm_visible: 1,
        }
    }

    fn target() -> TargetModeState {
        TargetModeState {
            lat_e7: 0,
            lon_e7: 0,
            flight_level_hundreds: 100,
            source: TargetSource::Radar,
            reliability: ReliabilityEvidence::Reliable,
            isr: IsrEvidence::Clear,
            coasting: 0,
            controller_initiated_coast: 0,
            flight_plan_aided: 0,
        }
    }

    #[test]
    fn fusion_isr_falls_back_to_five_and_requests_no_three_nm_symbol() {
        let mut t = target();
        t.isr = IsrEvidence::Required;
        let r = surveillance_mode_result(adaptation(SurveillanceMode::TerminalFusion), t).unwrap();
        assert_eq!(r.minimum_nm, 5);
        assert_eq!(r.three_nm_symbol, 0);
        assert_eq!(r.fusion_solid_circle, 1);
    }

    #[test]
    fn unknown_fusion_health_is_not_treated_as_clear_or_reliable() {
        let mut t = target();
        t.reliability = ReliabilityEvidence::Unknown;
        let r = surveillance_mode_result(adaptation(SurveillanceMode::TerminalFusion), t).unwrap();
        assert_eq!(r.minimum_nm, 5);
        t.reliability = ReliabilityEvidence::Reliable;
        t.isr = IsrEvidence::Unknown;
        let r = surveillance_mode_result(adaptation(SurveillanceMode::TerminalFusion), t).unwrap();
        assert_eq!(r.minimum_nm, 5);
    }

    #[test]
    fn multisensor_and_mosaic_never_select_three_nm() {
        for mode in [
            SurveillanceMode::StarsMultiSensor,
            SurveillanceMode::EramMosaic,
        ] {
            let r = surveillance_mode_result(adaptation(mode), target()).unwrap();
            assert_eq!(r.minimum_nm, 5);
            assert_eq!(r.three_nm_symbol, 0);
        }
    }

    #[test]
    fn coast_is_never_separation_eligible_and_controller_coast_requires_flat() {
        let mut t = target();
        t.coasting = 1;
        assert_eq!(
            surveillance_mode_result(adaptation(SurveillanceMode::TerminalFusion), t)
                .unwrap()
                .separation_eligible,
            0
        );
        t.controller_initiated_coast = 1;
        assert_eq!(
            surveillance_mode_result(adaptation(SurveillanceMode::TerminalFusion), t),
            Err(ModeRefusal::ControllerCoastRequiresFlat)
        );
        t.flight_plan_aided = 1;
        assert_eq!(
            surveillance_mode_result(adaptation(SurveillanceMode::TerminalFusion), t)
                .unwrap()
                .separation_eligible,
            0
        );
    }

    #[test]
    fn eram_three_nm_requires_every_adapted_prerequisite() {
        let r = surveillance_mode_result(adaptation(SurveillanceMode::Eram), target()).unwrap();
        assert_eq!(r.minimum_nm, 3);
        let mut a = adaptation(SurveillanceMode::Eram);
        a.map_3nm_visible = 0;
        assert_eq!(surveillance_mode_result(a, target()).unwrap().minimum_nm, 5);
        a = adaptation(SurveillanceMode::Eram);
        a.has_preferred_radar = 0;
        a.track_based_display = 1;
        assert_eq!(surveillance_mode_result(a, target()).unwrap().minimum_nm, 3);
    }

    #[test]
    fn eram_requires_known_level_and_uses_ten_nm_at_fl600() {
        let mut t = target();
        t.flight_level_hundreds = 600;
        assert_eq!(
            surveillance_mode_result(adaptation(SurveillanceMode::Eram), t)
                .unwrap()
                .minimum_nm,
            10
        );
        t.flight_level_hundreds = i16::MIN;
        assert_eq!(
            surveillance_mode_result(adaptation(SurveillanceMode::Eram), t),
            Err(ModeRefusal::MissingFlightLevel)
        );
    }

    #[test]
    fn terminal_single_sensor_uses_the_sensor_specific_range_boundary() {
        let origin = LatLon::new(0.0, 0.0).unwrap();
        let mut a = adaptation(SurveillanceMode::TerminalSingleSensor);
        let mut t = target();
        t.source = TargetSource::Radar;
        let below_40 = wgs84()
            .direct(origin, 90.0, 40.0 * 1852.0 - 2.0)
            .unwrap()
            .position;
        t.lat_e7 = (below_40.lat() * 1.0e7).round() as i32;
        t.lon_e7 = (below_40.lon() * 1.0e7).round() as i32;
        assert_eq!(surveillance_mode_result(a, t).unwrap().minimum_nm, 3);

        let at_40 = wgs84()
            .direct(origin, 90.0, 40.0 * 1852.0 + 2.0)
            .unwrap()
            .position;
        t.lat_e7 = (at_40.lat() * 1.0e7).round() as i32;
        t.lon_e7 = (at_40.lon() * 1.0e7).round() as i32;
        assert_eq!(surveillance_mode_result(a, t).unwrap().minimum_nm, 5);

        a.radar_class = RadarClass::MonopulseSecondary;
        let below_60 = wgs84()
            .direct(origin, 90.0, 60.0 * 1852.0 - 2.0)
            .unwrap()
            .position;
        t.lat_e7 = (below_60.lat() * 1.0e7).round() as i32;
        t.lon_e7 = (below_60.lon() * 1.0e7).round() as i32;
        assert_eq!(surveillance_mode_result(a, t).unwrap().minimum_nm, 3);
        let at_60 = wgs84()
            .direct(origin, 90.0, 60.0 * 1852.0 + 2.0)
            .unwrap()
            .position;
        t.lat_e7 = (at_60.lat() * 1.0e7).round() as i32;
        t.lon_e7 = (at_60.lon() * 1.0e7).round() as i32;
        assert_eq!(surveillance_mode_result(a, t).unwrap().minimum_nm, 5);
    }
}
