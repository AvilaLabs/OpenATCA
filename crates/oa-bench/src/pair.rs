//! Paired samples: every ERAM track sample of an associated flight with the truth at its
//! position time. Core (pure).
//!
//! Along-track and cross-track errors are taken in the truth's frame: the error vector (ERAM
//! position minus truth position, in the tangent plane at the truth point) is projected on the
//! truth ground-track direction (along, positive when ERAM is ahead) and on the direction to its
//! right (cross, positive when ERAM is to the right of the truth track). Speed error is ERAM
//! track speed minus truth ground speed; heading error is the wrapped difference of the two
//! headings; altitude error is ERAM reported altitude minus truth altitude on ERAM's own reference:
//! at or above [`TRANSITION_FT`] ERAM reports a flight level (pressure altitude, as the truth
//! is), below it an altitude corrected with the local altimeter setting, so the truth pressure
//! altitude is first brought to indicated altitude with the setting of the nearest station in
//! space and time ([`indicated_ft`]); a pair with no setting has no altitude error and is
//! counted.

use crate::assoc::Assoc;
use crate::geo::{enu, heading_deg, wrap180, KT_MPS};
use crate::legacy::{Legacy, NONE, NO_ALT, NO_VEL};
use crate::stats::{band, exclude, phase, Exclusions};
use crate::thindex::{samples_of, ThIndex};
use crate::truth::{indicated_ft, Truth, NO_TRUTH_NAMES};

/// The US transition altitude, feet: a reported value below it is an altitude (altimeter setting
/// applied), a value at or above it a flight level (standard setting). The decoded feed carries
/// only feet, not which of the two ERAM meant, so the rule is by value.
pub const TRANSITION_FT: i32 = 18_000;

/// How the altitude error of a pair was formed (the `alt_mode` column).
pub mod altmode {
    /// No altitude on one side: no altitude error.
    pub const NONE: u8 = 0;
    /// Reported as a flight level: compared with the truth pressure altitude as it is.
    pub const FLIGHT_LEVEL: u8 = 1;
    /// Reported as an altitude: truth brought to indicated altitude with an altimeter setting.
    pub const CORRECTED: u8 = 2;
    /// Reported as an altitude with no altimeter setting found: not compared.
    pub const NO_SETTING: u8 = 3;
}

/// The altimeter lookup of a pairing run: `(lat, lon, time Unix ms)` to inches of mercury.
pub type AltimeterAt<'a> = &'a dyn Fn(f64, f64, i64) -> Option<f64>;

/// Position cell of the altimeter lookup, 1e-7 degrees (0.1 degree), and its time cell,
/// milliseconds (one minute): the setting is looked up once per cell, at the cell's centre, so
/// the answer depends on the cell only.
const ALT_CELL_E7: i64 = 1_000_000;
const ALT_CELL_MS: i64 = 60_000;

/// Pairing settings (cold: A7.2).
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PairConfig {
    /// A truth sample with a 95% horizontal uncertainty above this is not used (M2_DESIGN 1.5).
    pub max_sigma_m: f64,
    /// Below this truth ground speed the along and cross directions are undefined, metres per
    /// second (about 19 knots).
    pub min_speed_mps: f64,
    /// Below this truth ground speed the heading is undefined, metres per second (30 knots).
    pub min_heading_mps: f64,
}

impl Default for PairConfig {
    fn default() -> PairConfig {
        PairConfig {
            max_sigma_m: 50.0,
            min_speed_mps: 10.0,
            min_heading_mps: 30.0 * KT_MPS,
        }
    }
}

/// Paired samples, one row per ERAM sample with truth; absent errors are NaN.
// A3: one Vec per column.
#[derive(Clone, Debug, Default)]
pub struct PairTable {
    /// Index into the legacy `ThTable`.
    pub th: Vec<u32>,
    pub flight: Vec<u32>,
    /// Truth aircraft address.
    pub addr: Vec<u32>,
    /// ERAM position time, Unix ms.
    pub t_ms: Vec<i64>,
    pub cross_m: Vec<f32>,
    pub along_m: Vec<f32>,
    pub speed_err_kt: Vec<f32>,
    pub hdg_err_deg: Vec<f32>,
    /// ERAM position, 1e-7 degrees (the diagnostic of the heading error against longitude).
    pub lat_e7: Vec<i32>,
    pub lon_e7: Vec<i32>,
    pub alt_err_ft: Vec<f32>,
    /// How the altitude error was formed ([`altmode`]).
    pub alt_mode: Vec<u8>,
    /// The correction applied to the truth altitude, feet (NaN unless [`altmode::CORRECTED`]).
    pub alt_corr_ft: Vec<f32>,
    /// Truth ground speed, metres per second.
    pub truth_speed_mps: Vec<f32>,
    pub method: Vec<u8>,
    pub phase: Vec<u8>,
    pub band: Vec<u8>,
    pub coast: Vec<u8>,
}

/// Pairs every sample of every associated flight with truth, with no altimeter settings known:
/// a sample reported below [`TRANSITION_FT`] has no altitude error.
pub fn build_pairs(
    leg: &Legacy,
    ix: &ThIndex,
    assoc: &Assoc,
    truth: &Truth<'_>,
    cfg: &PairConfig,
) -> (PairTable, Exclusions) {
    build_pairs_with(leg, ix, assoc, truth, cfg, &|_, _, _| None)
}

/// Pairs every sample of every associated flight with truth; `altimeter` gives the setting for
/// the altitude error of samples reported below [`TRANSITION_FT`].
pub fn build_pairs_with(
    leg: &Legacy,
    ix: &ThIndex,
    assoc: &Assoc,
    truth: &Truth<'_>,
    cfg: &PairConfig,
    altimeter: AltimeterAt<'_>,
) -> (PairTable, Exclusions) {
    let mut memo: Option<((i64, i64, i64), Option<f64>)> = None;
    let mut p = PairTable::default();
    let mut ex = Exclusions::new();
    for f in 0..leg.flights.gufi.len() {
        let samples = samples_of(ix, f as u32);
        let addr = assoc.addr.get(f).copied().unwrap_or(NONE);
        if addr == NONE {
            let st = assoc.status.get(f).copied().unwrap_or(0);
            let why = crate::assoc::astatus::NAMES
                .get(usize::from(st))
                .copied()
                .unwrap_or("not associated");
            for _ in samples {
                exclude(&mut ex, "pairing", why);
            }
            continue;
        }
        let meth = assoc.method.get(f).copied().unwrap_or(0);
        let mut last_t = i64::MIN;
        for &s in samples {
            let i = s as usize;
            let (Some(&t), Some(&lat), Some(&lon)) = (
                leg.th.pos_ms.get(i),
                leg.th.lat_e7.get(i),
                leg.th.lon_e7.get(i),
            ) else {
                continue;
            };
            if t == last_t {
                exclude(&mut ex, "pairing", "duplicate position time");
                continue;
            }
            last_t = t;
            let ts = match (truth.query)(addr, t) {
                Ok(ts) => ts,
                Err(e) => {
                    exclude(
                        &mut ex,
                        "pairing",
                        NO_TRUTH_NAMES
                            .get(e as usize)
                            .copied()
                            .unwrap_or("no truth"),
                    );
                    continue;
                }
            };
            if ts.sigma95_h_m > cfg.max_sigma_m {
                exclude(&mut ex, "pairing", NO_TRUTH_NAMES[2]);
                continue;
            }
            let (e, n) = enu(ts.lat, ts.lon, f64::from(lat) * 1e-7, f64::from(lon) * 1e-7);
            if !(e.is_finite() && n.is_finite()) {
                exclude(
                    &mut ex,
                    "pairing",
                    "position not a valid latitude and longitude",
                );
                continue;
            }
            let v = ts.vel_east_mps.hypot(ts.vel_north_mps);
            let (mut cross, mut along) = (f32::NAN, f32::NAN);
            if v >= cfg.min_speed_mps {
                let (ue, un) = (ts.vel_east_mps / v, ts.vel_north_mps / v);
                along = (e * ue + n * un) as f32;
                cross = (e * un - n * ue) as f32;
            } else {
                exclude(&mut ex, "T1-T2", "truth nearly stationary");
            }
            let vx = leg.th.vx_dkt.get(i).copied().unwrap_or(NO_VEL);
            let vy = leg.th.vy_dkt.get(i).copied().unwrap_or(NO_VEL);
            let (mut spd, mut hdg) = (f32::NAN, f32::NAN);
            if vx != NO_VEL && vy != NO_VEL {
                let (ex_kt, ny_kt) = (f64::from(vx) * 0.1, f64::from(vy) * 0.1);
                spd = (ex_kt.hypot(ny_kt) - v / KT_MPS) as f32;
                if v >= cfg.min_heading_mps {
                    hdg = wrap180(
                        heading_deg(ex_kt, ny_kt) - heading_deg(ts.vel_east_mps, ts.vel_north_mps),
                    ) as f32;
                } else {
                    exclude(&mut ex, "T3-heading", "truth speed below 30 kt");
                }
            } else {
                exclude(&mut ex, "T3", "no ERAM velocity");
            }
            let alt = leg.th.alt_ft.get(i).copied().unwrap_or(NO_ALT);
            let (mut alt_err, mut mode, mut corr) = (f32::NAN, altmode::NONE, f32::NAN);
            match ts.baro_alt_ft {
                Some(ta) if alt != NO_ALT && alt >= TRANSITION_FT => {
                    alt_err = (alt - ta) as f32;
                    mode = altmode::FLIGHT_LEVEL;
                }
                Some(ta) if alt != NO_ALT => {
                    let key = (
                        (f64::from(lat) / ALT_CELL_E7 as f64).floor() as i64,
                        (f64::from(lon) / ALT_CELL_E7 as f64).floor() as i64,
                        t.div_euclid(ALT_CELL_MS),
                    );
                    let inhg = match memo {
                        Some((k, v)) if k == key => v,
                        _ => {
                            let v = altimeter(
                                (key.0 as f64 + 0.5) * ALT_CELL_E7 as f64 * 1e-7,
                                (key.1 as f64 + 0.5) * ALT_CELL_E7 as f64 * 1e-7,
                                key.2 * ALT_CELL_MS + ALT_CELL_MS / 2,
                            );
                            memo = Some((key, v));
                            v
                        }
                    };
                    match inhg {
                        Some(h) => {
                            let ind = indicated_ft(f64::from(ta), h);
                            alt_err = (f64::from(alt) - ind) as f32;
                            corr = (ind - f64::from(ta)) as f32;
                            mode = altmode::CORRECTED;
                        }
                        None => {
                            exclude(&mut ex, "T4", "no altimeter setting");
                            mode = altmode::NO_SETTING;
                        }
                    }
                }
                _ => exclude(&mut ex, "T4", "no altitude on one side"),
            }
            let ph = match ts.vert_rate_fpm {
                None => phase::UNKNOWN,
                Some(r) if r > phase::RATE_FPM => phase::CLIMB,
                Some(r) if r < -phase::RATE_FPM => phase::DESCENT,
                Some(_) => phase::CRUISE,
            };
            p.th.push(s);
            p.flight.push(f as u32);
            p.addr.push(addr);
            p.t_ms.push(t);
            p.cross_m.push(cross);
            p.along_m.push(along);
            p.speed_err_kt.push(spd);
            p.hdg_err_deg.push(hdg);
            p.lat_e7.push(lat);
            p.lon_e7.push(lon);
            p.alt_err_ft.push(alt_err);
            p.alt_mode.push(mode);
            p.alt_corr_ft.push(corr);
            p.truth_speed_mps.push(v as f32);
            p.method.push(meth);
            p.phase.push(ph);
            p.band.push(band::of((alt != NO_ALT).then_some(alt)));
            p.coast.push(leg.th.coast.get(i).copied().unwrap_or(0));
        }
    }
    (p, ex)
}
