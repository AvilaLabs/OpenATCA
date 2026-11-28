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
//! * **Velocity** (ground speed and track) enters with a per-axis sigma looked up by NACv in
//!   [`AdsbConfig::vel_sigma_mps`]; a velocity without a NACv in the carry age is used as
//!   [`DEFAULT_NACV`]. The default table is the calibrated
//!   [`NACV_VEL_SIGMA_MPS`](crate::params::NACV_VEL_SIGMA_MPS), set from the measured error of the
//!   reported velocity; the NACv bound as a 95 % radius divided by 2.448
//!   ([`NACV_BOUND_VEL_SIGMA_MPS`](crate::params::NACV_BOUND_VEL_SIGMA_MPS)) is far looser than the real error and made the filter derive
//!   its velocity from positions and lag behind accelerations. A table entry that is not
//!   positive (or is not a number) drops that velocity and counts it in `velocity_refused`.
//! * **Altitude** is barometric with the 25 ft quantisation; a row with only a geometric
//!   altitude passes it flagged geometric (the tracker counts and ignores it). A vertical rate
//!   flagged geometric by readsb is not passed.

use std::collections::{BTreeMap, BTreeSet};

use oa_adsb::{reports_len, reports_row, Reports, F_RATE_GEOM, NONE_I32, NONE_U16};
use oa_truth::{
    nacp_epu_m, quality_check, QualityState, TruthConfig, DEFAULT_CONFIG, REMOVE_KINDS, SIGMA95,
};

use crate::params::{BARO_QUANT_FT, DEFAULT_NACV, NACV_VEL_SIGMA_MPS, POS_SIGMA_SCALE, VEL_LAG_S};
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
    /// Per-axis 1-sigma velocity noise, m/s, by NACv (index 0 is unused: infinity).
    pub vel_sigma_mps: [f64; 5],
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
    vel_sigma_mps: NACV_VEL_SIGMA_MPS,
    baro_quant_ft: BARO_QUANT_FT,
    sensor: 0,
};

impl Default for AdsbConfig {
    fn default() -> Self {
        DEFAULT_ADSB_CONFIG
    }
}

/// Checks a configuration; the error names the first bad value. The velocity table must hold a
/// positive number, not NaN, for every NACv 1 to 4.
pub fn check_adsb_config(cfg: &AdsbConfig) -> Result<(), &'static str> {
    for sigma in cfg.vel_sigma_mps.iter().skip(1) {
        if sigma.is_nan() || *sigma <= 0.0 {
            return Err("vel_sigma_mps");
        }
    }
    if !(1..=4).contains(&cfg.default_nacv) {
        return Err("default_nacv");
    }
    Ok(())
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
    /// Velocities not used because the configured sigma for their NACv is not a positive number.
    pub velocity_refused: u64,
    pub with_altitude: u64,
    pub with_vrate: u64,
    /// Rows whose required per-aircraft carry state could not be retained at the configured cap.
    pub quality_state_capacity_refused: u64,
    /// Rows older than the newest held value; they cannot consume or replace future carry state.
    pub quality_state_order_refused: u64,
}

struct QualityCacheEntry {
    state: QualityState,
    expires_ns: i64,
}

/// Bounded carry-forward quality state for consecutive ADS-B frames. Entries expire only after
/// every held field is older than the configured carry interval.
pub struct AdsbQualityCache {
    entries: BTreeMap<u32, QualityCacheEntry>,
    expiries: BTreeSet<(i64, u32)>,
    capacity: usize,
}

impl AdsbQualityCache {
    /// A cache sized for one tracker population. Callers can use a smaller bound in tests.
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            expiries: BTreeSet::new(),
            capacity,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn expire_before(&mut self, time_ns: i64) {
        while self
            .expiries
            .first()
            .is_some_and(|(expires_ns, _)| *expires_ns < time_ns)
        {
            let Some((expires_ns, address)) = self.expiries.pop_first() else {
                break;
            };
            if self
                .entries
                .get(&address)
                .is_some_and(|entry| entry.expires_ns == expires_ns)
            {
                self.entries.remove(&address);
            }
        }
    }

    fn update(
        &mut self,
        address: u32,
        row: &oa_adsb::Row,
        time_ns: i64,
        cfg: &TruthConfig,
    ) -> (oa_truth::QualityVerdict, bool, bool) {
        self.expire_before(time_ns);
        let regresses_state = self
            .entries
            .get(&address)
            .and_then(|entry| state_newest_quality_ns(&entry.state))
            .is_some_and(|newest| time_ns < newest);
        if regresses_state {
            let mut state = QualityState::default();
            return (quality_check(&mut state, row, time_ns, cfg), false, true);
        }
        let existing = self.entries.contains_key(&address);
        let has_room = existing || self.entries.len() < self.capacity;
        if !has_room {
            let mut state = QualityState::default();
            let verdict = quality_check(&mut state, row, time_ns, cfg);
            return (verdict, state_has_quality(&state), false);
        }

        if let Some(expires_ns) = self.entries.get(&address).map(|entry| entry.expires_ns) {
            self.expiries.remove(&(expires_ns, address));
            let (verdict, next_expiry) = if let Some(entry) = self.entries.get_mut(&address) {
                let verdict = quality_check(&mut entry.state, row, time_ns, cfg);
                (verdict, quality_expiry(&entry.state, cfg))
            } else {
                let mut state = QualityState::default();
                return (quality_check(&mut state, row, time_ns, cfg), true, false);
            };
            if let Some(expires_ns) = next_expiry {
                if let Some(entry) = self.entries.get_mut(&address) {
                    entry.expires_ns = expires_ns;
                }
                self.expiries.insert((expires_ns, address));
            } else {
                self.entries.remove(&address);
            }
            return (verdict, false, false);
        }

        let mut state = QualityState::default();
        let verdict = quality_check(&mut state, row, time_ns, cfg);
        if let Some(expires_ns) = quality_expiry(&state, cfg) {
            self.entries
                .insert(address, QualityCacheEntry { state, expires_ns });
            self.expiries.insert((expires_ns, address));
        }
        (verdict, false, false)
    }
}

impl Default for AdsbQualityCache {
    fn default() -> Self {
        Self::new(crate::params::MAX_TRACKS)
    }
}

fn state_has_quality(state: &QualityState) -> bool {
    [state.nacp, state.nic, state.sil, state.version, state.nacv]
        .iter()
        .any(|held| held.at_ns != oa_truth::HELD_NONE.at_ns)
}

fn state_newest_quality_ns(state: &QualityState) -> Option<i64> {
    [state.nacp, state.nic, state.sil, state.version, state.nacv]
        .iter()
        .map(|held| held.at_ns)
        .filter(|time_ns| *time_ns != oa_truth::HELD_NONE.at_ns)
        .max()
}

fn quality_expiry(state: &QualityState, cfg: &TruthConfig) -> Option<i64> {
    if !state_has_quality(state) {
        return None;
    }
    let carry_ns = cfg.quality_carry_ms.checked_mul(1_000_000)?;
    let newest = [state.nacp, state.nic, state.sil, state.version, state.nacv]
        .iter()
        .map(|held| held.at_ns)
        .filter(|time_ns| *time_ns != oa_truth::HELD_NONE.at_ns)
        .max()?;
    Some(newest.checked_add(carry_ns).unwrap_or(i64::MAX))
}

/// Measurements of `reports`, in time order (ties by input order), and what was removed. The
/// per-aircraft carried quality values follow time order, as in oa-truth.
pub fn adsb_measurements(reports: &Reports, cfg: &AdsbConfig) -> (Vec<Measurement>, AdsbStats) {
    let mut quality = AdsbQualityCache::new(usize::MAX);
    adsb_measurements_with_cache(reports, cfg, &mut quality)
}

/// Measurements of `reports` using caller-owned bounded quality state. The ordinary
/// [`adsb_measurements`] API remains frame-local; streaming adapters keep this cache across frames.
pub fn adsb_measurements_with_cache(
    reports: &Reports,
    cfg: &AdsbConfig,
    quality: &mut AdsbQualityCache,
) -> (Vec<Measurement>, AdsbStats) {
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
    let mut out = Vec::with_capacity(n);
    for &(t, i) in &order {
        let Some(row) = reports_row(reports, i as usize) else {
            if let Some(c) = stats.removed.first_mut() {
                *c += 1;
            }
            continue;
        };
        let (v, capacity_refused, order_refused) = quality.update(row.icao, &row, t, &cfg.gates);
        if capacity_refused {
            stats.quality_state_capacity_refused += 1;
        }
        if order_refused {
            stats.quality_state_order_refused += 1;
        }
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
            vel_lag_s: 0.0,
        };
        if row.gs_dkt != NONE_U16 && row.track_ddeg != NONE_U16 {
            let nacv = match v.nacv {
                Some(k) if (1..=4).contains(&k) => k,
                _ => {
                    stats.velocity_default_nacv += 1;
                    cfg.default_nacv
                }
            };
            let sigma = cfg
                .vel_sigma_mps
                .get(usize::from(nacv))
                .copied()
                .filter(|x| !x.is_nan() && *x > 0.0);
            if let Some(sv) = sigma {
                let speed = f64::from(row.gs_dkt) * 0.1 * MPS_PER_KT;
                let ang = (f64::from(row.track_ddeg) * 0.1).to_radians();
                let (sa, ca) = oa_math::sin_cos(ang);
                m.vel_east_mps = (speed * sa) as f32;
                m.vel_north_mps = (speed * ca) as f32;
                let vv = (sv * sv) as f32;
                m.vel_cov_m2s2 = [vv, 0.0, vv];
                m.flags |= M_HAS_VEL;
                m.vel_lag_s = VEL_LAG_S as f32;
                stats.with_velocity += 1;
            } else {
                stats.velocity_refused += 1;
            }
        }
        if row.baro_alt_ft != NONE_I32 {
            m.alt_ft = row.baro_alt_ft;
            m.alt_quant_ft = cfg.baro_quant_ft;
            stats.with_altitude += 1;
            if row.flags & F_RATE_GEOM == 0 && row.vrate_fpm != NONE_I32 {
                m.vrate_fpm = row.vrate_fpm;
                stats.with_vrate += 1;
            }
        } else if row.geom_alt_ft != NONE_I32 {
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
