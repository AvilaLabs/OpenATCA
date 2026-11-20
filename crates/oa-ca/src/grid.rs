//! The spatial grid over one cycle's tracks and the scan for pairs near each other. Core (pure).
//!
//! The scheme of `oa-encounter`'s grid (and `oa-track`'s coarse grid): bands of latitude of the
//! configured height, each cut into as many cells of longitude as keep a cell at least as wide
//! as a band is tall at its edge nearest a pole. Unlike the record's, the radius is not fixed:
//! for track `i` it is `T (v_i + v_max) + H_max + r_i + r_max` (look-ahead times the closing
//! speed bound, the largest horizontal minimum, and both 95 % radii), so the scan visits as many
//! bands either side as the radius needs, and each pair is then accepted by its own radius
//! `T (v_i + v_j) + H_max + r_i + r_j`. Each pair is found once, from its lower-numbered track.
//!
//! The pair test is the flat-earth one (east-west scale at the first track's latitude) while the
//! pair's own radius makes it a superset test there ([`flat_valid`]); a pair for which it does not
//! is tested by the chord between the two positions on the sphere ([`chord_d2`]), which has no
//! latitude limit and is no longer than the arc, so it accepts every pair the arc test does.

use alloc::vec::Vec;

use crate::solve::vertical_span;
use crate::state::{CaState, Cand};
use crate::types::{CycleStats, COAST_V_ALLOWANCE_FT, F_COAST, F_UNCERTAIN};

/// Metres of a degree of latitude, a lower bound over the sphere the tool measures on.
pub(crate) const M_PER_DEG_LOW: f64 = 110_900.0;
/// The flat-earth test accepts this multiple of the radius, covering its own error.
pub(crate) const CHEAP_MARGIN: f64 = 1.02;
/// The exact test's reach is this multiple of the pair radius (the grid's flat test accepts
/// [`CHEAP_MARGIN`], more, so the exact test never sees less than the grid passes on).
pub(crate) const EXACT_MARGIN: f64 = 1.01;
const MAX_CELLS: u64 = u32::MAX as u64;
/// Metres of the smallest sphere radius the flat test is proved against.
pub(crate) const R_MIN_M: f64 = 6_356_752.0;
/// The flat test is a superset test for a pair while `tan(latitude) * radius / R_MIN_M` is at most
/// this (the error of its east-west scale across the pair, kept inside [`CHEAP_MARGIN`]).
pub(crate) const FLAT_TOL: f64 = 0.02;
/// Latitude above which a row is not evaluated, degrees: the cell search is shown to reach every
/// pair within the radius up to it (`tests/ca.rs`, the pair at the tangent point), and the
/// windows of longitude it uses are not above it.
pub(crate) const MAX_LAT_DEG: f64 = 85.0;

/// `(cells of longitude per band, first cell number per band, cosine of the band's edge nearest
/// a pole)`, or `None` if the cells would not fit a `u32`.
pub(crate) fn grid_tables(band_deg: f64) -> Option<(Vec<u32>, Vec<u32>, Vec<f64>)> {
    let n_bands = oa_ieee::ceil(180.0 / band_deg) as usize;
    let mut lon_cells = Vec::with_capacity(n_bands);
    let mut first_cell = Vec::with_capacity(n_bands);
    let mut cos_edge = Vec::with_capacity(n_bands);
    let mut next = 0u64;
    for b in 0..n_bands {
        let lo = -90.0 + band_deg * b as f64;
        let hi = lo + band_deg;
        let edge = if lo.abs() > hi.abs() {
            lo.abs()
        } else {
            hi.abs()
        };
        let edge = if edge > 90.0 { 90.0 } else { edge };
        let c = oa_math::cos(edge.to_radians());
        let c = if c > 0.0 { c } else { 0.0 };
        let n = (oa_ieee::floor(360.0 * c / band_deg) as u64).max(1);
        if next + n > MAX_CELLS {
            return None;
        }
        lon_cells.push(n as u32);
        first_cell.push(next as u32);
        cos_edge.push(c);
        next += n;
    }
    Some((lon_cells, first_cell, cos_edge))
}

fn lat_band(st: &CaState, lat: f64) -> usize {
    let n = st.lon_cells.len();
    let b = oa_ieee::floor((lat + 90.0) / st.band_deg);
    if b < 0.0 {
        0
    } else if b >= n as f64 {
        n.saturating_sub(1)
    } else {
        b as usize
    }
}

fn lon_cell(st: &CaState, band: usize, lon: f64) -> i64 {
    let n = i64::from(st.lon_cells.get(band).copied().unwrap_or(1));
    i64::rem_euclid(oa_ieee::floor((lon + 180.0) / 360.0 * n as f64) as i64, n)
}

/// Fills the per-track tables and the sorted keys from `st.obs`.
pub(crate) fn grid_build(st: &mut CaState) {
    st.cos_lat.clear();
    st.tan_lat.clear();
    st.speed.clear();
    st.keys.clear();
    for i in 0..st.obs.len() {
        let Some(o) = st.obs.get(i).copied() else {
            break;
        };
        st.cos_lat.push(oa_math::cos(o.lat_deg.to_radians()));
        let (e, n) = (f64::from(o.vel_east_mps), f64::from(o.vel_north_mps));
        st.speed.push(oa_ieee::sqrt(e * e + n * n));
        let band = lat_band(st, o.lat_deg);
        let lo = lon_cell(st, band, o.lon_deg);
        let first = st.first_cell.get(band).copied().unwrap_or(0);
        let cell = u64::from(first) + lo as u64;
        st.keys.push(cell << 32 | i as u64);
    }
    st.keys.sort_unstable();
    // A tangent is kept only for the rows the cycle's largest pair radius does not already cover:
    // at or below `cut` degrees the flat test is valid for every pair of the cycle (stored as 0).
    let radius = max_radius(st) * CHEAP_MARGIN;
    let cut = if radius > 0.0 {
        oa_math::atan(FLAT_TOL * R_MIN_M / radius).to_degrees() * (1.0 - 1e-9)
    } else {
        90.0
    };
    for i in 0..st.obs.len() {
        let lat = st.obs.get(i).map_or(90.0, |o| o.lat_deg.abs());
        let t = if lat <= cut {
            0.0
        } else {
            oa_math::tan(lat.to_radians())
        };
        st.tan_lat
            .push(if t.is_finite() { t } else { f64::INFINITY });
    }
    // Unit vectors are made when a pair needs the chord (rarely), and kept for the cycle.
    st.unit.clear();
    st.unit.resize(st.obs.len(), [f64::NAN; 3]);
}

/// The largest search radius of the cycle, metres (zero for no tracks).
fn max_radius(st: &CaState) -> f64 {
    let vmax = st.speed.iter().copied().fold(0.0f64, oa_math::max_f64);
    let rmax = st
        .obs
        .iter()
        .map(|o| f64::from(o.r95_m))
        .fold(0.0f64, oa_math::max_f64);
    if st.obs.is_empty() {
        0.0
    } else {
        pair_radius(st, vmax, vmax, rmax, rmax)
    }
}

/// Unit vector of the position at `lat_deg`, `lon_deg` on the sphere.
pub(crate) fn unit_vector(lat_deg: f64, lon_deg: f64) -> [f64; 3] {
    let (lat, lon) = (lat_deg.to_radians(), lon_deg.to_radians());
    let cl = oa_math::cos(lat);
    [
        cl * oa_math::cos(lon),
        cl * oa_math::sin(lon),
        oa_math::sin(lat),
    ]
}

/// The unit vector of track `k`, made on first use in the cycle.
fn unit_of(units: &mut [[f64; 3]], obs: &[crate::types::CaTrack], k: usize) -> Option<[f64; 3]> {
    let u = units.get_mut(k)?;
    if u[0].is_nan() {
        let o = obs.get(k)?;
        *u = unit_vector(o.lat_deg, o.lon_deg);
    }
    Some(*u)
}

/// True if the flat test is a superset test for a pair of these latitude tangents (absolute) and
/// this radius, metres: the cosine it takes at the first track's latitude is wrong across the pair
/// by a fraction of at most the larger tangent times the radius over [`R_MIN_M`].
pub(crate) fn flat_valid(tan_a: f64, tan_b: f64, radius_m: f64) -> bool {
    oa_math::max_f64(tan_a, tan_b) * radius_m <= FLAT_TOL * R_MIN_M
}

/// Squared flat-earth separation of `a` and `b` with the east-west scale `cos_a` of `a`'s
/// latitude, metres squared.
pub(crate) fn flat_d2(a: &crate::types::CaTrack, cos_a: f64, b: &crate::types::CaTrack) -> f64 {
    let dy = (b.lat_deg - a.lat_deg) * M_PER_DEG_LOW;
    let mut dl = b.lon_deg - a.lon_deg;
    if dl > 180.0 {
        dl -= 360.0;
    } else if dl < -180.0 {
        dl += 360.0;
    }
    let dx = dl * cos_a * M_PER_DEG_LOW;
    dx * dx + dy * dy
}

/// Squared chord between two unit vectors of positions on the sphere of radius [`R_MIN_M`],
/// metres squared: never more than the squared arc, whatever the latitude.
pub(crate) fn chord_d2(a: &[f64; 3], b: &[f64; 3]) -> f64 {
    let (x, y, z) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]);
    (x * x + y * y + z * z) * (R_MIN_M * R_MIN_M)
}

/// The radius of the pair `(a, b)` of speeds and 95 % radii: the closest they can start and
/// still be inside the largest horizontal minimum plus both radii within the look-ahead.
fn pair_radius(st: &CaState, va: f64, vb: f64, ra: f64, rb: f64) -> f64 {
    st.look_ahead_s * (va + vb) + st.h_max_m + ra + rb
}

/// True if `a` and `b`, with both altitudes and rates known, cannot come within the largest
/// vertical limit (plus the coast allowance of the pair) at any time in the look-ahead.
fn cannot_converge(st: &CaState, a: &crate::types::CaTrack, b: &crate::types::CaTrack) -> bool {
    if !(a.alt_ft.is_finite()
        && b.alt_ft.is_finite()
        && a.vrate_fpm.is_finite()
        && b.vrate_fpm.is_finite())
    {
        return false;
    }
    let mut v = st.v_max_ft;
    for t in [a, b] {
        if t.flags & (F_COAST | F_UNCERTAIN) != 0 {
            v += COAST_V_ALLOWANCE_FT;
        }
    }
    let dz = f64::from(b.alt_ft) - f64::from(a.alt_ft);
    let dvz = (f64::from(b.vrate_fpm) - f64::from(a.vrate_fpm)) / 60.0;
    vertical_span(dz, dvz, v, st.look_ahead_s).is_none()
}

/// Scans every track's neighbourhood, appending the pairs within the flat-earth radius to
/// `st.cands`. Each band step, cell probe and track met in a cell is one unit of work, checked
/// against the budget before it is done; when it runs out the scan stops and the tracks not
/// reached are counted.
pub(crate) fn grid_scan(st: &mut CaState, stats: &mut CycleStats) {
    let n = st.obs.len();
    let n_bands = st.lon_cells.len();
    let last = n_bands.saturating_sub(1);
    let vmax = st.speed.iter().copied().fold(0.0f64, oa_math::max_f64);
    let rmax = st
        .obs
        .iter()
        .map(|o| f64::from(o.r95_m))
        .fold(0.0f64, oa_math::max_f64);
    let budget = st.cfg.work_budget;
    let mut work = 0u64;
    'tracks: for i in 0..n {
        let (Some(oi), Some(cosl), Some(vi), Some(ti)) = (
            st.obs.get(i).copied(),
            st.cos_lat.get(i).copied(),
            st.speed.get(i).copied(),
            st.tan_lat.get(i).copied(),
        ) else {
            break;
        };
        let ri = f64::from(oi.r95_m);
        let search = pair_radius(st, vi, vmax, ri, rmax) * CHEAP_MARGIN;
        let rdeg = search / M_PER_DEG_LOW;
        let la = lat_band(st, oi.lat_deg);
        // Bands either side the radius can reach; at least one, at most all of them.
        let k = (oa_math::max_f64(oa_ieee::ceil(rdeg / st.band_deg), 1.0) as usize).min(n_bands);
        let b0 = la.saturating_sub(k);
        let b1 = if la + k > last { last } else { la + k };
        let mut cmin = 1.0f64;
        for b in b0..=b1 {
            if work >= budget {
                stats.tracks_unscanned = (n - i) as u64;
                break 'tracks;
            }
            work += 1;
            let c = st.cos_edge.get(b).copied().unwrap_or(0.0);
            if c < cmin {
                cmin = c;
            }
        }
        // Half-width of the longitude window, degrees; 360 takes every cell of a band.
        let w = if cmin > 0.02 { rdeg / cmin } else { 360.0 };
        for lb in b0..=b1 {
            let nl = i64::from(st.lon_cells.get(lb).copied().unwrap_or(1));
            let first = u64::from(st.first_cell.get(lb).copied().unwrap_or(0));
            let lo = oa_ieee::floor((oi.lon_deg - w + 180.0) / 360.0 * nl as f64) as i64;
            let hi = oa_ieee::floor((oi.lon_deg + w + 180.0) / 360.0 * nl as f64) as i64;
            let (lo, hi) = if hi - lo + 1 >= nl {
                (0, nl - 1)
            } else {
                (lo, hi)
            };
            for kk in lo..=hi {
                if work >= budget {
                    stats.tracks_unscanned = (n - i) as u64;
                    break 'tracks;
                }
                work += 1;
                let cell = first + i64::rem_euclid(kk, nl) as u64;
                let mut p = st.keys.partition_point(|&x| x < cell << 32);
                while let Some(&key) = st.keys.get(p) {
                    if key >> 32 != cell {
                        break;
                    }
                    if work >= budget {
                        stats.tracks_unscanned = (n - i) as u64;
                        break 'tracks;
                    }
                    work += 1;
                    p += 1;
                    let j = (key & 0xFFFF_FFFF) as usize;
                    if j <= i {
                        continue;
                    }
                    let (Some(oj), Some(vj), Some(tj)) = (
                        st.obs.get(j),
                        st.speed.get(j).copied(),
                        st.tan_lat.get(j).copied(),
                    ) else {
                        continue;
                    };
                    let r = pair_radius(st, vi, vj, ri, f64::from(oj.r95_m)) * CHEAP_MARGIN;
                    let d2 = if flat_valid(ti, tj, r) {
                        flat_d2(&oi, cosl, oj)
                    } else {
                        stats.pairs_chord += 1;
                        let (Some(ua), Some(ub)) = (
                            unit_of(&mut st.unit, &st.obs, i),
                            unit_of(&mut st.unit, &st.obs, j),
                        ) else {
                            continue;
                        };
                        chord_d2(&ua, &ub)
                    };
                    if d2 <= r * r {
                        if cannot_converge(st, &oi, oj) {
                            stats.pairs_skipped_vertical += 1;
                            continue;
                        }
                        stats.candidates += 1;
                        let c = Cand {
                            d2: d2 as u64,
                            i: i as u32,
                            j: j as u32,
                        };
                        if !keep_nearest(&mut st.cands, st.cfg.max_candidates, c) {
                            stats.candidates_dropped += 1;
                        }
                    }
                }
            }
        }
    }
    stats.grid_work = work;
}

/// Keeps the `cap` smallest candidates (nearest first, ties by track numbers) in `v`, a binary
/// max-heap on `Cand`'s order, so a full table gives up its largest separation, whatever the
/// scan order; bounded (`log cap` steps) and deterministic (the order is total). True if `c` is
/// nothing was dropped; false is one pair dropped (`c`, or the largest pair it displaced), which
/// the caller counts.
fn keep_nearest(v: &mut alloc::vec::Vec<Cand>, cap: usize, c: Cand) -> bool {
    if v.len() < cap {
        v.push(c);
        let mut k = v.len() - 1;
        while k > 0 {
            let p = (k - 1) / 2;
            let (Some(&a), Some(&b)) = (v.get(k), v.get(p)) else {
                break;
            };
            if a <= b {
                break;
            }
            v.swap(k, p);
            k = p;
        }
        return true;
    }
    let Some(&worst) = v.first() else {
        return false;
    };
    if c >= worst {
        return false;
    }
    if let Some(root) = v.first_mut() {
        *root = c;
    }
    let n = v.len();
    let mut k = 0;
    loop {
        let (l, r) = (2 * k + 1, 2 * k + 2);
        let mut m = k;
        if l < n && v.get(l) > v.get(m) {
            m = l;
        }
        if r < n && v.get(r) > v.get(m) {
            m = r;
        }
        if m == k {
            break;
        }
        v.swap(k, m);
        k = m;
    }
    // The old largest went out in place of `c`.
    false
}

/// The exact pair radius, metres: beyond it two tracks cannot be inside the largest minimum and
/// both 95 % radii at any time in the look-ahead, with a margin of 1 % for the scale of the
/// azimuthal projection the pair is measured in.
pub(crate) fn exact_radius(st: &CaState, i: usize, j: usize) -> Option<f64> {
    let (a, b) = (st.obs.get(i)?, st.obs.get(j)?);
    let (va, vb) = (st.speed.get(i).copied()?, st.speed.get(j).copied()?);
    Some(pair_radius(st, va, vb, f64::from(a.r95_m), f64::from(b.r95_m)) * EXACT_MARGIN)
}

#[cfg(test)]
mod tests {
    //! The pair tests against the exact ones, on deterministic scatters (the geometry is not
    //! proved: it rests on the trigonometry of `oa_math`, which the Kani harnesses do not reach).
    use super::*;
    use crate::types::{CaTrack, AIRBORNE};
    use oa_geo::aeqd::{frame_new, project};

    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    fn track(lat: f64, lon: f64) -> CaTrack {
        CaTrack {
            lat_deg: lat,
            lon_deg: lon,
            addr: 0,
            vel_east_mps: 0.0,
            vel_north_mps: 0.0,
            r95_m: 0.0,
            alt_ft: 0.0,
            vrate_fpm: 0.0,
            flags: 0,
            ground: AIRBORNE,
        }
    }

    /// The unit vector, the cosine and the absolute tangent of the latitude of `o`.
    fn vectors_of(o: &CaTrack) -> ([f64; 3], f64, f64) {
        let (sl, cl) = (
            oa_math::sin(o.lat_deg.to_radians()),
            oa_math::cos(o.lat_deg.to_radians()),
        );
        (unit_vector(o.lat_deg, o.lon_deg), cl, sl.abs() / cl)
    }

    /// A position `d` metres (on the sphere of `R_MIN_M`) from `(lat, lon)` toward a bearing.
    fn offset(lat: f64, lon: f64, d: f64, bearing: f64) -> (f64, f64) {
        let (th, p, b) = (d / R_MIN_M, lat.to_radians(), bearing);
        let sp2 = oa_math::sin(p) * oa_math::cos(th)
            + oa_math::cos(p) * oa_math::sin(th) * oa_math::cos(b);
        let p2 = oa_math::asin(sp2.clamp(-1.0, 1.0));
        let dl = oa_math::atan2(
            oa_math::sin(b) * oa_math::sin(th) * oa_math::cos(p),
            oa_math::cos(th) - oa_math::sin(p) * oa_math::sin(p2),
        );
        let mut l = lon + dl.to_degrees();
        if l > 180.0 {
            l -= 360.0;
        } else if l < -180.0 {
            l += 360.0;
        }
        (p2.to_degrees(), l)
    }

    // REQ: OA-CA-032
    #[test]
    fn the_chord_is_never_longer_than_the_arc_at_any_latitude() {
        let mut g = Lcg(11);
        for _ in 0..400_000 {
            let lat = (g.next() - 0.5) * 180.0;
            let lon = (g.next() - 0.5) * 360.0;
            let d = 1000.0 + g.next() * 1.0e6;
            let (lat2, lon2) = offset(lat, lon, d, g.next() * core::f64::consts::TAU);
            let (a, b) = (track(lat, lon), track(lat2, lon2));
            let (ua, ub) = (vectors_of(&a).0, vectors_of(&b).0);
            // The arc from the unit vectors: the angle between them, stable for small angles.
            let cross = [
                ua[1] * ub[2] - ua[2] * ub[1],
                ua[2] * ub[0] - ua[0] * ub[2],
                ua[0] * ub[1] - ua[1] * ub[0],
            ];
            let sin_t =
                oa_ieee::sqrt(cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2]);
            let cos_t = ua[0] * ub[0] + ua[1] * ub[1] + ua[2] * ub[2];
            let arc = R_MIN_M * oa_math::atan2(sin_t, cos_t);
            let chord = oa_ieee::sqrt(chord_d2(&ua, &ub));
            assert!(
                chord <= arc * (1.0 + 1e-9),
                "{lat} {lon} {d}: {chord} {arc}"
            );
            // And no shorter than the arc by more than its cube term (about d^2 / 24 R^2).
            assert!(
                chord >= arc * (1.0 - 2.0e-2),
                "{lat} {lon} {d}: {chord} {arc}"
            );
        }
    }

    // REQ: OA-CA-032
    #[test]
    fn where_the_flat_test_is_valid_it_accepts_every_pair_the_exact_test_accepts() {
        let mut g = Lcg(5);
        let (mut valid, mut invalid, mut near) = (0u64, 0u64, 0u64);
        for _ in 0..600_000 {
            let lat = (g.next() - 0.5) * 2.0 * MAX_LAT_DEG;
            let lon = (g.next() - 0.5) * 360.0;
            // A pair radius as the grid forms it, and a separation up to the exact test's reach.
            let pr = 9_000.0 + g.next() * 260_000.0;
            let r = pr * CHEAP_MARGIN;
            let d = pr * EXACT_MARGIN * oa_ieee::sqrt(oa_ieee::sqrt(g.next()));
            let (lat2, lon2) = offset(lat, lon, d, g.next() * core::f64::consts::TAU);
            let (a, b) = (track(lat, lon), track(lat2, lon2));
            let (_, cos_a, tan_a) = vectors_of(&a);
            let (_, _, tan_b) = vectors_of(&b);
            let (x, y) = project(&frame_new(lat, lon), lat2, lon2);
            let exact = oa_ieee::sqrt(x * x + y * y);
            if exact > pr * EXACT_MARGIN {
                continue;
            }
            if flat_valid(tan_a, tan_b, r) {
                valid += 1;
                let f = oa_ieee::sqrt(flat_d2(&a, cos_a, &b));
                assert!(
                    f <= r,
                    "{lat} {lon} pr {pr}: flat {f} reach {r} exact {exact}"
                );
                if f > pr * 1.005 {
                    near += 1;
                }
            } else {
                invalid += 1;
                // Whatever the flat test would say, the chord test accepts.
                let (ua, ub) = (vectors_of(&a).0, vectors_of(&b).0);
                assert!(
                    oa_ieee::sqrt(chord_d2(&ua, &ub)) <= r,
                    "{lat} {lon} pr {pr}"
                );
            }
        }
        assert!(
            valid > 100_000 && invalid > 50_000,
            "{valid} {invalid} {near}"
        );
    }

    // REQ: OA-CA-033
    #[test]
    fn the_chord_is_finite_symmetric_and_not_negative_and_flat_validity_only_narrows() {
        let mut g = Lcg(23);
        for _ in 0..500_000 {
            let mut v = || {
                [
                    g.next() * 2.0 - 1.0,
                    g.next() * 2.0 - 1.0,
                    g.next() * 2.0 - 1.0,
                ]
            };
            let (a, b) = (v(), v());
            let d = chord_d2(&a, &b);
            assert!(d.is_finite() && d >= 0.0 && d == chord_d2(&b, &a));
            assert_eq!(chord_d2(&a, &a), 0.0);
            // Validity: a pair that is valid stays valid with smaller tangents and radius.
            let t = |x: f64| if x < 0.02 { f64::INFINITY } else { x * x * 5.0 };
            let (ta, tb, r) = (t(g.next()), t(g.next()), 1.0 + g.next() * 3.0e5);
            let (ta2, tb2, r2) = (ta * g.next(), tb * g.next(), r * g.next());
            if flat_valid(ta, tb, r) {
                assert!(ta.is_finite() && tb.is_finite() && flat_valid(ta2, tb2, r2));
            }
        }
    }

    // REQ: OA-CA-032
    #[test]
    fn the_flat_test_is_invalid_exactly_where_it_could_miss_a_pair() {
        // At a radius of 120 km the old cycle-wide limit was 66 degrees; the pair's own radius
        // keeps the flat test valid to there and sends the rest to the chord.
        let r = 120_000.0 * CHEAP_MARGIN;
        let t = |deg: f64| oa_math::tan(deg.to_radians());
        assert!(flat_valid(t(30.0), t(30.0), r));
        assert!(!flat_valid(t(80.0), t(80.0), r));
        // The larger of the two tangents decides.
        assert!(!flat_valid(t(10.0), t(80.0), r));
        assert!(flat_valid(t(0.0), t(0.0), 1.0e6));
        // A pole is never valid.
        assert!(!flat_valid(f64::INFINITY, 0.0, 1.0));
    }
}

#[cfg(kani)]
mod proofs {
    //! Kani proof harnesses (docs/VERIFICATION.md). Compiled only under `cargo kani`.
    use super::*;

    fn unit_like() -> [f64; 3] {
        let v: [f64; 3] = [kani::any(), kani::any(), kani::any()];
        for c in v {
            kani::assume(c.is_finite() && (-1.0..=1.0).contains(&c));
        }
        v
    }

    // REQ: OA-CA-033
    #[kani::proof]
    fn chord_d2_of_unit_vectors_is_finite_and_not_negative() {
        let (a, b) = (unit_like(), unit_like());
        let d = chord_d2(&a, &b);
        assert!(d.is_finite() && d >= 0.0);
    }
}
