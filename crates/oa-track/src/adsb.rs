//! The adapter from decoded adsb.lol reports to tracker measurements. Core (pure): a function of
//! the reports and the settings.
//!
//! Gating is oa-truth's, not a copy: [`oa_truth::quality_check`] applies the same rules (valid
//! position, readsb stale flag, ADS-B source, carried-forward NACp, NIC, SIL and version
//! gates) in the same units, and the removal reasons are `oa_truth::REMOVE_NAMES`. What differs
//! is the use of the quality: truth uses NACp to weight a smoother, the tracker uses it to set
//! the measurement covariance.
//!
//! * **Position noise** is the NACp bound as a 95 % radius divided by 2.448 per axis
//!   ([`oa_truth::SIGMA95`], the radius of a circular normal holding 95 %), times
//!   [`POS_SIGMA_SCALE`].
//! * **Velocity** (ground speed and track) enters with the NACv bound as a 95 % radius divided
//!   by the same factor, per axis; a velocity without a NACv in the carry age is used as
//!   [`DEFAULT_NACV`].
//! * **Altitude** is barometric with the 25 ft quantisation; a row with only a geometric
//!   altitude passes it flagged geometric (the tracker counts and ignores it). A vertical rate
//!   flagged geometric by readsb is not passed.

use std::collections::BTreeMap;

use oa_adsb::{reports_len, reports_row, Reports, F_ALT_GEOM, F_RATE_GEOM, NONE_I32, NONE_U16};
use oa_truth::{
    nacp_epu_m, nacv_epu_mps, quality_check, QualityState, TruthConfig, DEFAULT_CONFIG,
    REMOVE_KINDS, SIGMA95,
};

use crate::params::{BARO_QUANT_FT, DEFAULT_NACV, POS_SIGMA_SCALE};
use crate::types::{Measurement, M_ALT_GEOMETRIC, M_HAS_VEL, NONE_I32 as NONE_ALT, NO_IDENT};

const MPS_PER_KT: f64 = 0.514_444_444_444;

/// Settings of the adapter. Plain record (A1); A7.2: cold struct, size guard waived.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AdsbConfig {
    /// The quality gates (oa-truth's configuration).
    pub gates: TruthConfig,
    /// Multiplier on the NACp-derived position sigma.
    pub pos_sigma_scale: f64,
    /// NACv used for a velocity with none in the carry age.
    pub default_nacv: u8,
    /// Barometric altitude quantisation, feet.
    pub baro_quant_ft: u16,
    /// Sensor identifier written to every measurement.
    pub sensor: u16,
}

/// The design's values.
pub const DEFAULT_ADSB_CONFIG: AdsbConfig = AdsbConfig {
    gates: DEFAULT_CONFIG,
    pos_sigma_scale: POS_SIGMA_SCALE,
    default_nacv: DEFAULT_NACV,
    baro_quant_ft: BARO_QUANT_FT,
    sensor: 0,
};

impl Default for AdsbConfig {
    fn default() -> Self {
        DEFAULT_ADSB_CONFIG
    }
}

/// What the adapter did. Plain record (A1); A7.2: cold struct.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AdsbStats {
    pub rows_in: u64,
    /// Reports that became measurements.
    pub passed: u64,
    /// Reports removed, by oa-truth's reason (`oa_truth::REMOVE_NAMES`; the last two, jump and
    /// short leg, are truth's own and stay zero here).
    pub removed: [u64; REMOVE_KINDS],
    pub with_velocity: u64,
    /// Velocities used with the default NACv.
    pub velocity_default_nacv: u64,
    pub with_altitude: u64,
    pub with_vrate: u64,
}

/// Measurements of `reports`, in time order (ties by input order), and what was removed. The
/// per-aircraft carried quality values follow time order, as in oa-truth.
pub fn adsb_measurements(reports: &Reports, cfg: &AdsbConfig) -> (Vec<Measurement>, AdsbStats) {
    let n = reports_len(reports);
    let mut order: Vec<(i64, u32)> = Vec::with_capacity(n);
    for (i, &t) in reports.time_ns.iter().enumerate() {
        order.push((t, i as u32));
    }
    order.sort_unstable();
    let mut stats = AdsbStats {
        rows_in: n as u64,
        ..AdsbStats::default()
    };
    let mut quality: BTreeMap<u32, QualityState> = BTreeMap::new();
    let mut out = Vec::with_capacity(n);
    for &(t, i) in &order {
        let Some(row) = reports_row(reports, i as usize) else {
            if let Some(c) = stats.removed.first_mut() {
                *c += 1;
            }
            continue;
        };
        let q = quality.entry(row.icao).or_default();
        let v = quality_check(q, &row, t, &cfg.gates);
        if let Some(why) = v.removal {
            if let Some(c) = stats.removed.get_mut(why) {
                *c += 1;
            }
            continue;
        }
        let sigma = nacp_epu_m(v.nacp.unwrap_or(0)) / SIGMA95 * cfg.pos_sigma_scale;
        if !sigma.is_finite() {
            // NACp 0 cannot pass the gate (min_nacp >= 1); a refusal here is a gate misuse.
            if let Some(c) = stats.removed.get_mut(oa_truth::REM_NACP) {
                *c += 1;
            }
            continue;
        }
        let var = (sigma * sigma) as f32;
        let mut m = Measurement {
            toa_ns: t,
            pos_cov_m2: [var, 0.0, var],
            vel_cov_m2s2: [0.0; 3],
            vel_east_mps: 0.0,
            vel_north_mps: 0.0,
            lat_e7: row.lat_e7,
            lon_e7: row.lon_e7,
            alt_ft: NONE_ALT,
            vrate_fpm: NONE_ALT,
            addr: row.icao,
            alt_quant_ft: 0,
            sensor: cfg.sensor,
            ident: if row.squawk == NONE_U16 {
                NO_IDENT
            } else {
                row.squawk
            },
            flags: 0,
            category: row.category,
        };
        if row.gs_dkt != NONE_U16 && row.track_ddeg != NONE_U16 {
            let nacv = match v.nacv {
                Some(k) if (1..=4).contains(&k) => k,
                _ => {
                    stats.velocity_default_nacv += 1;
                    cfg.default_nacv
                }
            };
            let sv = nacv_epu_mps(nacv) / SIGMA95;
            let speed = f64::from(row.gs_dkt) * 0.1 * MPS_PER_KT;
            let ang = (f64::from(row.track_ddeg) * 0.1).to_radians();
            m.vel_east_mps = (speed * ang.sin()) as f32;
            m.vel_north_mps = (speed * ang.cos()) as f32;
            let vv = (sv * sv) as f32;
            m.vel_cov_m2s2 = [vv, 0.0, vv];
            m.flags |= M_HAS_VEL;
            stats.with_velocity += 1;
        }
        if row.baro_alt_ft != NONE_I32 {
            m.alt_ft = row.baro_alt_ft;
            m.alt_quant_ft = cfg.baro_quant_ft;
            stats.with_altitude += 1;
            if row.flags & F_RATE_GEOM == 0 && row.vrate_fpm != NONE_I32 {
                m.vrate_fpm = row.vrate_fpm;
                stats.with_vrate += 1;
            }
        } else if row.geom_alt_ft != NONE_I32 && row.flags & F_ALT_GEOM == 0 {
            // No barometric altitude: pass the geometric one, flagged, so the interface is
            // exercised and the tracker counts what it does not use.
            m.alt_ft = row.geom_alt_ft;
            m.flags |= M_ALT_GEOMETRIC;
        }
        stats.passed += 1;
        out.push(m);
    }
    (out, stats)
}
