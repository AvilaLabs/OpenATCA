//! Paired samples: every ERAM track sample of an associated flight with the truth at its
//! position time. Core (pure).
//!
//! Along-track and cross-track errors are taken in the truth's frame: the error vector (ERAM
//! position minus truth position, in the tangent plane at the truth point) is projected on the
//! truth ground-track direction (along, positive when ERAM is ahead) and on the direction to its
//! right (cross, positive when ERAM is to the right of the truth track). Speed error is ERAM
//! track speed minus truth ground speed; heading error is the wrapped difference of the two
//! headings; altitude error is ERAM reported altitude minus truth barometric altitude.

use crate::assoc::Assoc;
use crate::geo::{enu, heading_deg, wrap180, KT_MPS};
use crate::legacy::{Legacy, NONE, NO_ALT, NO_VEL};
use crate::stats::{band, exclude, phase, Exclusions};
use crate::thindex::{samples_of, ThIndex};
use crate::truth::{Truth, NO_TRUTH_NAMES};

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
    /// Truth ground speed, metres per second.
    pub truth_speed_mps: Vec<f32>,
    pub method: Vec<u8>,
    pub phase: Vec<u8>,
    pub band: Vec<u8>,
    pub coast: Vec<u8>,
}

/// Pairs every sample of every associated flight with truth.
pub fn build_pairs(
    leg: &Legacy,
    ix: &ThIndex,
    assoc: &Assoc,
    truth: &Truth<'_>,
    cfg: &PairConfig,
) -> (PairTable, Exclusions) {
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
                }
            } else {
                exclude(&mut ex, "T3", "no ERAM velocity");
            }
            let alt = leg.th.alt_ft.get(i).copied().unwrap_or(NO_ALT);
            let alt_err = match ts.baro_alt_ft {
                Some(ta) if alt != NO_ALT => (alt - ta) as f32,
                _ => {
                    exclude(&mut ex, "T4", "no altitude on one side");
                    f32::NAN
                }
            };
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
            p.truth_speed_mps.push(v as f32);
            p.method.push(meth);
            p.phase.push(ph);
            p.band.push(band::of((alt != NO_ALT).then_some(alt)));
            p.coast.push(leg.th.coast.get(i).copied().unwrap_or(0));
        }
    }
    (p, ex)
}
