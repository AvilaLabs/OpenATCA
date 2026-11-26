#![forbid(unsafe_code)]
//! Deterministic, synthetic scenario intent for the browser demonstration.
//!
//! This crate contains no flight data, private adaptation, sensor input, random runtime source,
//! serialization, or UI assumptions. A guarded preparation shell supplies verified public
//! procedure references and resolved route fixes; synthetic path samples use explicit scenario
//! assumptions, not an aircraft performance simulator.

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// Version of the portable intent and deterministic generation rules.
pub const SCENARIO_FORMAT_VERSION: u16 = 1;
/// The first scenario's planned synthetic aircraft count.
pub const NORMAL_ORD_AIRCRAFT: u16 = 60;
/// Planned normal-push scenario duration, in virtual milliseconds.
pub const NORMAL_ORD_DURATION_MS: u64 = 45 * 60 * 1_000;
/// Fixed seed for the baseline normal-push source scenario (`ORD-NPV1` in ASCII bytes).
pub const NORMAL_ORD_SEED: u64 = 0x4F52_442D_4E50_5631;
/// Maximum accepted scenario duration (DEMO_SPEC: 20–60 minutes).
pub const MAX_DURATION_MS: u64 = 60 * 60 * 1_000;
/// Event time unit for host integration.
pub const EVENT_TIME_UNIT: &str = "virtual milliseconds since scenario T0";
/// Boeing 737-800 NG maximum operating indicated airspeed, KCAS. FAA TCDS A16WE lists
/// 340 KCAS / Mach 0.82 for the 737-800 series; see source note in `docs/design/DEMO_SCENARIOS.md`.
pub const B738_NG_VMO_KCAS: u16 = 340;
/// Boeing 737-800 NG maximum operating Mach, in thousandths. This is a hard synthetic-model
/// bound, not a target or a prediction of aircraft performance.
pub const B738_NG_MMO_MILLI: u16 = 820;

/// Scenario-owned four-stream runway sequence. All sixty flights reach the threshold in the
/// hour; the two 27L streams alternate around the 28C stream, so each runway has at least90s.
/// Constant220kt and240s per-stream headway exceed5NM outside40NM and3NM inside it.
pub fn normal_arrival_slot(index: usize) -> Option<(u16, u64)> {
    let routes = [0_u16, 2, 1, 3];
    // Source-owned runway schedule, validated over the complete native scenario with unchanged CA.
    let phases_ms = [0_u64, 165_000, 105_000, 50_000];
    if index >= 60 {
        return None;
    }
    Some((
        *routes.get(index % 4)?,
        (index / 4) as u64 * 240_000 + *phases_ms.get(index % 4)?,
    ))
}

/// A sourced CIFP route reference or explicit synthetic direct segment. For CIFP routes,
/// `transition` is the exact CIFP transition identifier. A `Direct` reference uses airport `SYN`,
/// procedure `DCT`, and `transition=FROM>TO`; its endpoints must be adjacent referenced CIFP legs.
/// It is scenario geometry, not a published CIFP leg or an operational flight plan.
// A7.2: cold scenario-source reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcedureRouteRef {
    pub airport: String,
    pub kind: ProcedureKind,
    pub procedure: String,
    pub transition: String,
    /// CIFP route type where the procedure section has multiple route classes.
    pub route_type: Option<char>,
}

/// CIFP procedure kind or an explicitly synthetic direct segment used in the scenario manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcedureKind {
    Sid,
    Star,
    Approach,
    Airway,
    /// A synthetic straight-line flight-plan segment between two public CIFP fixes. This is not
    /// a published CIFP leg and must be labeled as an OpenATCA scenario assumption.
    Direct,
}

/// An explicitly sourced aircraft performance envelope. Values must be sourced and versioned
/// by the preparation tool; this crate has no default aircraft type or operational values.
// A7.2: cold provenance record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerformanceProfileRef {
    pub source_title: String,
    pub source_url: String,
    pub source_version: String,
    pub source_sha256: String,
    pub aircraft_type: String,
    pub license: String,
    /// Certified maximum operating indicated airspeed, KCAS, for the selected airframe.
    pub maximum_operating_ias_kcas: u16,
    /// Certified maximum operating Mach in thousandths (820 represents Mach 0.820).
    pub maximum_operating_mach_milli: u16,
}

/// Public-data provenance for one route variant.
// A7.2: cold provenance record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteSource {
    pub cifp_archive_name: String,
    pub cifp_archive_sha256: String,
    pub cifp_entry_name: String,
    pub cifp_entry_sha256: String,
    pub route: Vec<ProcedureRouteRef>,
    /// Ordered fix identifiers emitted by the bounded CIFP resolver.
    pub fix_sequence: Vec<String>,
}

/// A selected route with public-source provenance. A route with missing fixes is invalid.
// A7.2: cold scenario configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteVariant {
    pub source: RouteSource,
    pub performance: PerformanceProfileRef,
}

/// Deterministic scenario request. The seed affects only assignment and arrival-slot jitter; it
/// never creates runtime entropy or changes route geometry/performance inputs.
// A7.2: cold generator request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScenarioRequest {
    pub scenario_id: String,
    pub seed: u64,
    pub aircraft_count: u16,
    pub duration_ms: u64,
    pub routes: Vec<RouteVariant>,
}

/// A generated synthetic flight plan. There are no real callsigns, addresses, or copied
/// surveillance reports. The route reference resolves to a public CIFP procedure.
// A7.2: cold one-per-aircraft plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntheticFlightPlan {
    pub aircraft_id: String,
    pub arrival_time_ms: u64,
    pub route_variant: u16,
    pub route: Vec<ProcedureRouteRef>,
    pub performance: PerformanceProfileRef,
}

/// Fully synthetic and versioned scenario intent, before route-geometry materialization.
// A7.2: cold scenario aggregate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntheticScenario {
    pub format_version: u16,
    pub scenario_id: String,
    pub seed: u64,
    pub duration_ms: u64,
    pub routes: Vec<RouteVariant>,
    pub flights: Vec<SyntheticFlightPlan>,
}

/// Why a scenario request was refused.
// A7.2: cold error value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScenarioError {
    BadScenarioId,
    AircraftCount,
    Duration,
    NoRoutes,
    TooManyRoutes,
    BadRoute(usize),
    BadPerformance(usize),
}

impl fmt::Display for ScenarioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScenarioError::BadScenarioId => {
                f.write_str("scenario id must be 1–64 ASCII letters, digits, '-' or '_'")?
            }
            ScenarioError::AircraftCount => f.write_str("aircraft count must be 1–60")?,
            ScenarioError::Duration => f.write_str("duration must be 20–60 minutes")?,
            ScenarioError::NoRoutes => f.write_str("at least one sourced route is required")?,
            ScenarioError::TooManyRoutes => {
                f.write_str("at most 256 route variants are supported")?
            }
            ScenarioError::BadRoute(n) => write!(
                f,
                "route variant {n} has missing public provenance or fixes"
            )?,
            ScenarioError::BadPerformance(n) => {
                write!(f, "route variant {n} has missing performance provenance")?
            }
        }
        Ok(())
    }
}

/// Generates bounded, reproducible synthetic arrival slots and consistent route references.
/// Slots are evenly distributed with deterministic +/-15 second jitter and remain inside the
/// requested window. The schedule is not claimed to reproduce measured ORD demand. Geometry and
/// performance remain sourced input.
pub fn generate(request: &ScenarioRequest) -> Result<SyntheticScenario, ScenarioError> {
    if !valid_id(&request.scenario_id) {
        return Err(ScenarioError::BadScenarioId);
    }
    if request.aircraft_count == 0 || request.aircraft_count > NORMAL_ORD_AIRCRAFT {
        return Err(ScenarioError::AircraftCount);
    }
    if request.duration_ms < 20 * 60 * 1_000 || request.duration_ms > MAX_DURATION_MS {
        return Err(ScenarioError::Duration);
    }
    if request.routes.is_empty() {
        return Err(ScenarioError::NoRoutes);
    }
    if request.routes.len() > 256 || request.routes.len() > usize::from(u16::MAX) {
        return Err(ScenarioError::TooManyRoutes);
    }
    for (i, route) in request.routes.iter().enumerate() {
        if route.source.cifp_archive_name.is_empty()
            || !is_sha256(&route.source.cifp_archive_sha256)
            || route.source.cifp_entry_name.is_empty()
            || !is_sha256(&route.source.cifp_entry_sha256)
            || route.source.route.is_empty()
            || route.source.route.iter().any(|r| {
                !valid_id(&r.airport)
                    || !valid_id(&r.procedure)
                    || (!r.transition.is_empty()
                        && !((r.kind == ProcedureKind::Airway || r.kind == ProcedureKind::Direct)
                            && r.transition.bytes().filter(|b| *b == b'>').count() == 1
                            && r.transition.bytes().all(|b| {
                                b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'>'
                            }))
                        && !valid_id(&r.transition))
            })
            || route.source.route.iter().enumerate().any(|(index, r)| {
                if r.kind != ProcedureKind::Direct {
                    return false;
                }
                let previous = index
                    .checked_sub(1)
                    .and_then(|previous| route.source.route.get(previous));
                let next = route.source.route.get(index.saturating_add(1));
                r.airport != "SYN"
                    || r.procedure != "DCT"
                    || r.route_type.is_some()
                    || !matches!((previous, next), (Some(a), Some(b)) if a.kind == ProcedureKind::Star && b.kind == ProcedureKind::Approach)
            })
            || route.source.fix_sequence.is_empty()
        {
            return Err(ScenarioError::BadRoute(i));
        }
        let p = &route.performance;
        if p.source_title.is_empty()
            || p.source_url.is_empty()
            || p.source_version.is_empty()
            || !is_sha256(&p.source_sha256)
            || p.aircraft_type.is_empty()
            || p.license.is_empty()
            || p.maximum_operating_ias_kcas == 0
            || p.maximum_operating_mach_milli == 0
        {
            return Err(ScenarioError::BadPerformance(i));
        }
    }

    let mut flights = Vec::with_capacity(usize::from(request.aircraft_count));
    let mut previous_slot = 0u64;
    for n in 0..request.aircraft_count {
        let nominal = if request.aircraft_count == 1 {
            request.duration_ms / 2
        } else {
            (u64::from(n) * request.duration_ms) / u64::from(request.aircraft_count - 1)
        };
        let jitter_ms = (mix(request.seed.wrapping_add(u64::from(n))) % 30_001) as i64 - 15_000;
        let jittered = if jitter_ms < 0 {
            nominal.saturating_sub(jitter_ms.unsigned_abs())
        } else {
            nominal.saturating_add(jitter_ms as u64)
        }
        .min(request.duration_ms);
        let slot = if n == 0 {
            jittered
        } else {
            jittered
                .max(previous_slot.saturating_add(1))
                .min(request.duration_ms)
        };
        previous_slot = slot;
        let route_ix =
            usize::from(mix(request.seed.wrapping_add(u64::from(n))) as u16) % request.routes.len();
        let Some(route_index) = u16::try_from(route_ix).ok() else {
            return Err(ScenarioError::TooManyRoutes);
        };
        let Some(route) = request.routes.get(route_ix) else {
            return Err(ScenarioError::BadRoute(route_ix));
        };
        flights.push(SyntheticFlightPlan {
            aircraft_id: format!("SYN-{n:04}"),
            arrival_time_ms: slot,
            route_variant: route_index,
            route: route.source.route.clone(),
            performance: route.performance.clone(),
        });
    }

    Ok(SyntheticScenario {
        format_version: SCENARIO_FORMAT_VERSION,
        scenario_id: request.scenario_id.clone(),
        seed: request.seed,
        duration_ms: request.duration_ms,
        routes: request.routes.clone(),
        flights,
    })
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

    use super::*;

    fn request(connector: ProcedureRouteRef) -> ScenarioRequest {
        let sha = "a".repeat(64);
        ScenarioRequest {
            scenario_id: "direct-test".into(),
            seed: NORMAL_ORD_SEED,
            aircraft_count: 1,
            duration_ms: 20 * 60 * 1_000,
            routes: vec![RouteVariant {
                source: RouteSource {
                    cifp_archive_name: "CIFP_261029.zip".into(),
                    cifp_archive_sha256: sha.clone(),
                    cifp_entry_name: "FAACIFP18".into(),
                    cifp_entry_sha256: sha.clone(),
                    route: vec![
                        ProcedureRouteRef {
                            airport: "KORD".into(),
                            kind: ProcedureKind::Star,
                            procedure: "STAR1".into(),
                            transition: "INBD".into(),
                            route_type: Some('4'),
                        },
                        connector,
                        ProcedureRouteRef {
                            airport: "KORD".into(),
                            kind: ProcedureKind::Approach,
                            procedure: "I04R".into(),
                            transition: "ENTRY".into(),
                            route_type: Some('A'),
                        },
                    ],
                    fix_sequence: vec!["FROM".into(), "TO".into()],
                },
                performance: PerformanceProfileRef {
                    source_title: "synthetic fixture".into(),
                    source_url: "https://example.invalid/".into(),
                    source_version: "fixture".into(),
                    source_sha256: sha,
                    aircraft_type: "fixture".into(),
                    license: "fixture".into(),
                    maximum_operating_ias_kcas: 340,
                    maximum_operating_mach_milli: 820,
                },
            }],
        }
    }

    #[test]
    fn scenario_manifest_accepts_explicit_synthetic_dct_shape() {
        let scenario = generate(&request(ProcedureRouteRef {
            airport: "SYN".into(),
            kind: ProcedureKind::Direct,
            procedure: "DCT".into(),
            transition: "FROM>TO".into(),
            route_type: None,
        }))
        .expect("synthetic DCT is explicit in source route");
        assert_eq!(
            scenario.routes[0].source.route[1].kind,
            ProcedureKind::Direct
        );
    }

    #[test]
    fn scenario_manifest_rejects_ambiguous_direct_label() {
        assert_eq!(
            generate(&request(ProcedureRouteRef {
                airport: "KORD".into(),
                kind: ProcedureKind::Direct,
                procedure: "DCT".into(),
                transition: "FROM>TO".into(),
                route_type: None,
            })),
            Err(ScenarioError::BadRoute(0))
        );
    }
}
