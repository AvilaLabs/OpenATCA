//! Whole-set checks over decoded reports. Core (pure).

use crate::{
    category_observe, jump_verdict, reports_len, CategoryState, JumpPair, JumpVerdict, Reports,
    CAP_ORDINARY_KT, F_NEW_LEG, NONE_U16, NONE_U8,
};

/// Pairs closer than this are position noise, not jumps: two reports a few metres apart a
/// hundredth of a second apart (different sources, or CPR rounding) imply thousands of knots
/// while the aircraft has not moved. Real data has many (about 60 % of the pairs over 700 kt in
/// the 2026-11-13 sample). Half a nautical mile is about 900 m.
pub const MIN_JUMP_NM: f64 = 0.5;

/// Counts over a set of reports. Plain record (A1).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct Summary {
    pub reports: u64,
    pub aircraft: u64,
    /// Reports whose aircraft object gave `nac_p`.
    pub nac_p_known: u64,
    /// Of those, with `nac_p >= 8` (position accuracy better than 0.05 NM, 95 %).
    pub nac_p_ge_8: u64,
    /// Consecutive report pairs of one aircraft compared.
    pub pairs: u64,
    /// Pairs the speed rule ([`jump_verdict`], no position allowance) calls impossible and which are at least
    /// [`MIN_JUMP_NM`] apart, not counting pairs whose second report starts a new leg.
    pub jumps: u64,
    /// Pairs the speed rule calls impossible but closer than [`MIN_JUMP_NM`]: position noise.
    pub jitter: u64,
    /// Largest implied speed among the compared pairs at least [`MIN_JUMP_NM`] apart, knots.
    pub max_implied_kt: f64,
}

/// Great-circle distance in nautical miles (haversine on a 3440.065 NM sphere; the error
/// against the ellipsoid, under 0.5 %, is far below what the jump check needs).
fn distance_nm(lat1_e7: i32, lon1_e7: i32, lat2_e7: i32, lon2_e7: i32) -> f64 {
    let d = |e7: i32| f64::from(e7) * 1e-7_f64.to_radians();
    let (p1, p2) = (d(lat1_e7), d(lat2_e7));
    let dl = d(lon2_e7) - d(lon1_e7);
    let a = ((p2 - p1) / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * 3440.065 * a.sqrt().min(1.0).asin()
}

/// Summarizes `r`: distinct aircraft, how many reports carry `nac_p >= 8`, and a position-jump
/// check over consecutive reports of each aircraft in time order.
pub fn summarize(r: &Reports) -> Summary {
    let n = reports_len(r);
    let mut ix: Vec<u32> = (0..n as u32).collect();
    // Stable sort by (aircraft, time); ties keep input order.
    ix.sort_by_key(|&i| {
        (
            r.icao.get(i as usize).copied().unwrap_or(0),
            r.time_ns.get(i as usize).copied().unwrap_or(0),
        )
    });
    let mut s = Summary {
        reports: n as u64,
        ..Summary::default()
    };
    let mut prev: Option<usize> = None;
    // Emitter category trusted for the aircraft being walked (printed on a minority of rows; a
    // cap-raising one must persist, see `category_observe`).
    let mut cat = CategoryState::default();
    for &i in &ix {
        let i = i as usize;
        let (Some(&icao), Some(&t), Some(&lat), Some(&lon), Some(&flags)) = (
            r.icao.get(i),
            r.time_ns.get(i),
            r.lat_e7.get(i),
            r.lon_e7.get(i),
            r.flags.get(i),
        ) else {
            continue;
        };
        if let Some(&np) = r.nac_p.get(i) {
            if np != NONE_U8 {
                s.nac_p_known += 1;
                if np >= 8 {
                    s.nac_p_ge_8 += 1;
                }
            }
        }
        let same = prev.and_then(|p| {
            let pi = r.icao.get(p).copied()?;
            (pi == icao).then_some(p)
        });
        if same.is_none() {
            cat = CategoryState::default();
        }
        // Judged by the category as this report leaves it, kept only if the pair is plausible,
        // so a report that is itself a jump cannot vouch for the category that excuses it.
        let mut trial = cat;
        let category = category_observe(&mut trial, r.category.get(i).copied().unwrap_or(0));
        match same {
            Some(p) => {
                if flags & F_NEW_LEG == 0 {
                    s.pairs += 1;
                    let (Some(&pt), Some(&plat), Some(&plon)) =
                        (r.time_ns.get(p), r.lat_e7.get(p), r.lon_e7.get(p))
                    else {
                        continue;
                    };
                    let nm = distance_nm(plat, plon, lat, lon);
                    let hours = t.saturating_sub(pt) as f64 / 3.6e12;
                    // Same instant: any displacement is infinite speed; the distance floor
                    // decides whether it is noise.
                    let kt = if hours > 0.0 {
                        nm / hours
                    } else if nm > 0.01 {
                        f64::INFINITY
                    } else {
                        0.0
                    };
                    if kt > s.max_implied_kt && nm >= MIN_JUMP_NM {
                        s.max_implied_kt = kt;
                    }
                    let pair = JumpPair {
                        dist_m: nm * 1852.0,
                        dt_s: hours * 3600.0,
                        slack_m: 0.0,
                        category,
                        gs_a_dkt: r.gs_dkt.get(p).copied().unwrap_or(NONE_U16),
                        gs_b_dkt: r.gs_dkt.get(i).copied().unwrap_or(NONE_U16),
                    };
                    if jump_verdict(&pair, CAP_ORDINARY_KT) == JumpVerdict::Plausible {
                        cat = trial;
                    } else {
                        if nm >= MIN_JUMP_NM {
                            s.jumps += 1;
                        } else {
                            s.jitter += 1;
                        }
                    }
                }
            }
            None => {
                s.aircraft += 1;
                cat = trial;
            }
        }
        prev = Some(i);
    }
    s
}
