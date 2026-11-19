//! The ICD's workload and latency figures, as data, for conformance tests that need them: the
//! per-flight message counts and sizes of NAS-IC-21009205 Rev G Table 29, the peak per-minute
//! rates of the CAATS design workload of NAS-IC-82422100 Table 3-IV (NAS-IR-82422100 Table
//! 3-II), and the latency, availability and size bounds. Plain constants.
//! Shell (input-format adapter, B1, C6): allocates per message, so not real-time core (C1).

use crate::model::Kind;

/// One row of Table 29 "Expected Message Rates and Sizes" (planning estimates).
#[derive(Debug, Clone, Copy, PartialEq)]
// A7.2: cold struct, size guard waived: constant figures for conformance tests
pub struct PerFlight {
    pub kind: Kind,
    /// Average messages of this type per flight.
    pub per_flight: f64,
    /// Average size in bytes, including an estimated 50 bytes of communications header.
    pub avg_bytes: u32,
    /// Which flights the count refers to: `near-border`, `other` or `every`.
    pub applies_to: &'static str,
}

const NB: &str = "near-border departure flight";
const OTHER: &str = "non near-border departure flight";
const EVERY: &str = "every flight";

/// Rev G Part III 6.3 Table 29. LAM/RLA and LRM are "sum of all above except RTU" at 80 and
/// 100 bytes; AOC is 1 per flight at 80 bytes (listed under responses).
pub const PER_FLIGHT: [PerFlight; 18] = [
    PerFlight {
        kind: Kind::Fpl,
        per_flight: 1.0,
        avg_bytes: 240,
        applies_to: NB,
    },
    PerFlight {
        kind: Kind::Chg,
        per_flight: 0.5,
        avg_bytes: 160,
        applies_to: NB,
    },
    PerFlight {
        kind: Kind::Est,
        per_flight: 1.0,
        avg_bytes: 120,
        applies_to: NB,
    },
    PerFlight {
        kind: Kind::Mod,
        per_flight: 0.5,
        avg_bytes: 120,
        applies_to: NB,
    },
    PerFlight {
        kind: Kind::Cpl,
        per_flight: 1.0,
        avg_bytes: 250,
        applies_to: OTHER,
    },
    PerFlight {
        kind: Kind::Mod,
        per_flight: 0.5,
        avg_bytes: 120,
        applies_to: OTHER,
    },
    PerFlight {
        kind: Kind::Cnl,
        per_flight: 0.01,
        avg_bytes: 100,
        applies_to: EVERY,
    },
    PerFlight {
        kind: Kind::Rti,
        per_flight: 1.0,
        avg_bytes: 150,
        applies_to: EVERY,
    },
    PerFlight {
        kind: Kind::Rtu,
        per_flight: 5.0,
        avg_bytes: 140,
        applies_to: EVERY,
    },
    PerFlight {
        kind: Kind::Rta,
        per_flight: 1.0,
        avg_bytes: 110,
        applies_to: EVERY,
    },
    PerFlight {
        kind: Kind::Poi,
        per_flight: 0.01,
        avg_bytes: 250,
        applies_to: EVERY,
    },
    PerFlight {
        kind: Kind::Pla,
        per_flight: 0.01,
        avg_bytes: 110,
        applies_to: EVERY,
    },
    PerFlight {
        kind: Kind::Poa,
        per_flight: 0.01,
        avg_bytes: 80,
        applies_to: EVERY,
    },
    PerFlight {
        kind: Kind::Poj,
        per_flight: 0.01,
        avg_bytes: 100,
        applies_to: EVERY,
    },
    PerFlight {
        kind: Kind::Toc,
        per_flight: 1.0,
        avg_bytes: 150,
        applies_to: EVERY,
    },
    PerFlight {
        kind: Kind::Abi,
        per_flight: 1.0,
        avg_bytes: 250,
        applies_to: EVERY,
    },
    PerFlight {
        kind: Kind::Mis,
        per_flight: 0.1,
        avg_bytes: 130,
        applies_to: EVERY,
    },
    PerFlight {
        kind: Kind::Aoc,
        per_flight: 1.0,
        avg_bytes: 80,
        applies_to: EVERY,
    },
];

/// Table 29: average size of an LAM or RLA (bytes, header included) and of an LRM.
pub const LAM_RLA_BYTES: u32 = 80;
pub const LRM_BYTES: u32 = 100;

/// RTU cadence assumed by Table 29: one every 6 seconds for 30 seconds (5 per flight).
pub const TABLE29_RTU_INTERVAL_S: u32 = 6;
pub const TABLE29_RTU_DURATION_S: u32 = 30;

/// One row of the CAATS design workload (NAS-IC-82422100 Table 3-IV).
#[derive(Debug, Clone, Copy, PartialEq)]
// A7.2: cold struct, size guard waived: constant figures for conformance tests
pub struct PeakRate {
    pub kind: Kind,
    /// Peak rate for one ARTCC, messages per minute.
    pub artcc_per_min: f64,
    /// Peak rate for all of En Route, messages per minute.
    pub en_route_per_min: f64,
    pub avg_bytes: u32,
}

pub const PEAK_RATES: [PeakRate; 12] = [
    PeakRate {
        kind: Kind::Cpl,
        artcc_per_min: 4.88,
        en_route_per_min: 9.06,
        avg_bytes: 383,
    },
    PeakRate {
        kind: Kind::Mod,
        artcc_per_min: 3.50,
        en_route_per_min: 6.50,
        avg_bytes: 407,
    },
    PeakRate {
        kind: Kind::Fpl,
        artcc_per_min: 2.26,
        en_route_per_min: 4.20,
        avg_bytes: 240,
    },
    PeakRate {
        kind: Kind::Chg,
        artcc_per_min: 1.13,
        en_route_per_min: 2.10,
        avg_bytes: 210,
    },
    PeakRate {
        kind: Kind::Est,
        artcc_per_min: 2.12,
        en_route_per_min: 3.94,
        avg_bytes: 164,
    },
    PeakRate {
        kind: Kind::Cnl,
        artcc_per_min: 0.07,
        en_route_per_min: 0.13,
        avg_bytes: 155,
    },
    PeakRate {
        kind: Kind::Rti,
        artcc_per_min: 7.00,
        en_route_per_min: 13.00,
        avg_bytes: 316,
    },
    PeakRate {
        kind: Kind::Rla,
        artcc_per_min: 6.65,
        en_route_per_min: 12.35,
        avg_bytes: 124,
    },
    PeakRate {
        kind: Kind::Rta,
        artcc_per_min: 7.00,
        en_route_per_min: 13.00,
        avg_bytes: 277,
    },
    PeakRate {
        kind: Kind::Rtu,
        artcc_per_min: 92.40,
        en_route_per_min: 171.60,
        avg_bytes: 309,
    },
    PeakRate {
        kind: Kind::Lam,
        artcc_per_min: 19.92,
        en_route_per_min: 36.99,
        avg_bytes: 124,
    },
    PeakRate {
        kind: Kind::Lrm,
        artcc_per_min: 1.40,
        en_route_per_min: 2.60,
        avg_bytes: 149,
    },
];

/// Table 3-IV footnote: RTUs arrive from CAATS every 10 seconds for 2.2 minutes during a
/// handoff (13.2 per handoff); ERAM sends none.
pub const CAATS_RTU_INTERVAL_S: u32 = 10;
pub const CAATS_RTU_DURATION_MIN: f64 = 2.2;

/// Latency, availability and size bounds. Each cites its section.
pub mod bounds {
    /// NAM ICD Part III 6.1: LAM or LRM within 60 s of the message being sent, at least 99%
    /// of the time under normal operations.
    pub const RESPONSE_S: u32 = 60;
    pub const RESPONSE_FRACTION: f64 = 0.99;
    /// NAM ICD Part III 6.1: communication across the interface for RTI, RTA, RTU: less than
    /// six seconds maximum.
    pub const SURVEILLANCE_TRANSIT_S: u32 = 6;
    /// NAM ICD Part III 6.1: TOC and AOC have the flight-planning response requirement.
    pub const TOC_AOC_RESPONSE_S: u32 = 60;
    /// NAS-IR-82422100 3.2.2.2-b: detect and recover from failures within 60 s.
    pub const RECOVERY_S: u32 = 60;
    /// NAS-IC-82422100 3.1.1: end-to-end availability goal and the longest interruption on an
    /// unplanned outage when redundant resources exist.
    pub const AVAILABILITY: f64 = 0.9999;
    pub const MAX_INTERRUPTION_S: u32 = 60;
    /// NAS-IC-82422100 3.1.1: 95th percentile one-way latency ERAM ECG RFW to NESG via FTI.
    pub const ECG_TO_NESG_P95_MS: u32 = 225;
    /// NAS-IC-82422100 3.1.1: 95th percentile one-way latency between two NESGs.
    pub const NESG_TO_NESG_P95_MS: u32 = 50;
    /// NAS-IC-82422100 3.1.2: NESG health-monitoring poll interval.
    pub const HEALTH_POLL_S: u32 = 5;
    /// NAM ICD Part III 6.3: simultaneous peer systems the implementation must support.
    pub const PEERS: u32 = 4;
    /// NAS-IC-82422100 3.2.2.3.1: message size limits, header excluded.
    pub const MIN_MESSAGE_BYTES: u32 = 17;
    pub const MAX_MESSAGE_BYTES: u32 = 1800;
}
