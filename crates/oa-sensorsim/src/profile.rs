//! Named radar error profiles. Core (pure).
//!
//! Every number here is a row of docs/BENCHMARKS.md section 3.1, "Simulated sensor inputs",
//! and is cited at its constant. Nothing is invented. Two things the table does not give, and
//! which therefore are parameters of the site and not of the profile: range and azimuth bias
//! (no sourced value; see [`crate::radar::RadarSite`]) and the split-plot geometry.
//!
//! The detection, false and split rates are one row of the table ("SSR detection; false target
//! reports; split reports; Mode A / Mode C code detection", ICAO SURICG/6 IP/04 Table 1, quoting
//! the 1997 EUROCONTROL standard, grade S). The table gives them as bounds (">97%", "<0.1%",
//! "<0.3%", ">98% / >96%"); each profile uses the bound itself as the rate. They are the
//! only SSR rates in the table, so the two ATC-302 profiles reuse them (the ATC-302 rows give
//! error figures only); that reuse is an assumption.

/// Scan period of a terminal radar, seconds (docs/BENCHMARKS.md 3.1: Stone and Anderson Table 1;
/// ATC-302 section 2).
pub const SCAN_TERMINAL_S: f64 = 4.8;
/// Scan period of an en-route radar, seconds (same row).
pub const SCAN_EN_ROUTE_S: f64 = 12.0;
/// Effective earth radius factor of the standard atmosphere, 4/3. An assumption of the
/// line-of-sight model (the usual radar-horizon convention), not a row of the table.
pub const EARTH_K_STANDARD: f64 = 4.0 / 3.0;

/// SSR detection probability: ">97%" (ICAO SURICG/6 IP/04 Table 1, row 3 of the sensor table).
pub const SSR_PD: f64 = 0.97;
/// Mode A code detection: ">98%" (same row).
pub const SSR_PD_MODE_A: f64 = 0.98;
/// Mode C code detection: ">96%" (same row).
pub const SSR_PD_MODE_C: f64 = 0.96;
/// False target reports: "<0.1%" of plots (same row).
pub const SSR_FALSE_RATE: f64 = 0.001;
/// Split reports: "<0.3%" of plots (same row).
pub const SSR_SPLIT_RATE: f64 = 0.003;

/// One radar's error and detection model. Plain data (A1); A7.2: cold struct, size guard
/// waived.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RadarProfile {
    pub name: &'static str,
    /// Random range error, one sigma, metres.
    pub range_sigma_m: f64,
    /// Random azimuth error, one sigma, degrees.
    pub az_sigma_deg: f64,
    /// Range quantum, metres; 0 for none.
    pub range_quantum_m: f64,
    /// Altitude resolution, feet (100 Mode C, 25 Mode S).
    pub alt_quantum_ft: i32,
    /// Whether plots carry the Mode S address.
    pub mode_s: bool,
    /// Probability that a target in coverage is detected on a scan.
    pub pd: f64,
    /// Probability that a detected plot carries its Mode A code.
    pub pd_mode_a: f64,
    /// Probability that a detected plot carries its altitude code.
    pub pd_alt: f64,
    /// Probability, per true plot, of one extra false plot in the same scan.
    pub false_rate: f64,
    /// Probability that a true plot is split in two.
    pub split_rate: f64,
}

/// MSSR, the EUROCONTROL 1997 standard's random errors: range 70 m, azimuth 0.08 degree (one
/// sigma). Table 3.1 row "MSSR random error": NLR-CR-2004-472 section 7.1 Table 4, quoting
/// SUR.ET1.ST01.1000-STD-01-01. Mode C resolution 100 ft (NLR-CR-2004-472 section 3). Not a
/// Mode S sensor: no address on the plot (use struct update with `mode_s: true` for one).
pub const MSSR_EUROCONTROL_1997: RadarProfile = RadarProfile {
    name: "MSSR_EUROCONTROL_1997",
    range_sigma_m: 70.0,
    az_sigma_deg: 0.08,
    range_quantum_m: 0.0,
    alt_quantum_ft: 100,
    mode_s: false,
    pd: SSR_PD,
    pd_mode_a: SSR_PD_MODE_A,
    pd_alt: SSR_PD_MODE_C,
    false_rate: SSR_FALSE_RATE,
    split_rate: SSR_SPLIT_RATE,
};

/// Mode S sensor, the FAA test assumption of MIT Lincoln Laboratory ATC-302 (Gertz and
/// Kaminsky 2002) section 4.1: range 7.6 m (25 ft), azimuth 1 mrad (0.057 degree), one sigma.
/// Table 3.1 row "Mode S sensor". Altitude resolution 25 ft (NLR-CR-2004-472 section 3).
pub const MODE_S_ATC302: RadarProfile = RadarProfile {
    name: "MODE_S_ATC302",
    range_sigma_m: 7.6,
    az_sigma_deg: 1.0e-3 * 180.0 / std::f64::consts::PI,
    range_quantum_m: 0.0,
    alt_quantum_ft: 25,
    mode_s: true,
    pd: SSR_PD,
    pd_mode_a: SSR_PD_MODE_A,
    pd_alt: SSR_PD_MODE_C,
    false_rate: SSR_FALSE_RATE,
    split_rate: SSR_SPLIT_RATE,
};

/// Older SSR CD-2, the FAA test assumption of ATC-302 section 4.1: range 30.5 m (100 ft) with a
/// 1/8 NM quantum, azimuth 3 mrad (one sigma). Table 3.1 row "Older SSR CD-2". Mode C 100 ft.
pub const SSR_CD2_ATC302: RadarProfile = RadarProfile {
    name: "SSR_CD2_ATC302",
    range_sigma_m: 30.5,
    az_sigma_deg: 3.0e-3 * 180.0 / std::f64::consts::PI,
    range_quantum_m: 1852.0 / 8.0,
    alt_quantum_ft: 100,
    mode_s: false,
    pd: SSR_PD,
    pd_mode_a: SSR_PD_MODE_A,
    pd_alt: SSR_PD_MODE_C,
    false_rate: SSR_FALSE_RATE,
    split_rate: SSR_SPLIT_RATE,
};

/// Every named profile.
pub const PROFILES: [RadarProfile; 3] = [MSSR_EUROCONTROL_1997, MODE_S_ATC302, SSR_CD2_ATC302];

/// The profile called `name`, if any.
pub fn profile_by_name(name: &str) -> Option<RadarProfile> {
    PROFILES.iter().copied().find(|p| p.name == name)
}
