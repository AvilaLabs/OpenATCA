//! A uniform latitude and longitude grid over the live tracks of one association, so that the
//! coarse window of a plot is tested against the few tracks near it instead of every track.
//! Core (pure).
//!
//! The window of `fusion.rs` is in the radar's own polar terms (slant range and azimuth, scaled by
//! the track's ground range). The grid does not replace that test: it only narrows the tracks the
//! test is applied to, to a set certain to contain every track the test would pass, so the
//! outcome is the same as testing every track. The set is the tracks in the cells of a
//! latitude and longitude box around the region that can pass the window:
//!
//! * the slant range of a passing track is within the window width of the plot's, and its ground
//!   range is at most that plus the width, and at least what a height difference of the tallest
//!   track leaves of the smallest slant range;
//! * its cross-range from the plot's azimuth line is at most the width, so it lies in a
//!   rectangle of that half-width along the line from just behind the radar to the largest ground
//!   range;
//! * the rectangle is widened by [`EXTRA_FIXED_M`] plus [`EXTRA_PER_RANGE`] of the range, which
//!   covers the difference between the projection's distances and the radar model's (0.3 % of the
//!   sphere against the ellipsoid, and the height of the aircraft) with room.
//!
//! The box is the latitude and longitude extent of nine points of the rectangle, with a margin.
//! A pole that lies inside the rectangle (judged in the rectangle's own plane, with a margin, not
//! from the nine points) makes the box run to that pole and take every longitude; longitudes are taken relative to the box's
//! centre and wrapped, so a box across the +-180 degree seam is two runs of cells; a latitude
//! beyond the poles is clamped to the last cell.
//!
//! The index is a sorted array of one word per track (cell number above, track number below), built
//! in a buffer allocated when it is made and queried into one the caller owns: building is a sort in place, a query a binary
//! search per cell. Tracks whose polar terms are not finite pass the window test whatever their
//! place (the comparison with a NaN is false), so they have a cell of their own that every query
//! includes.

use oa_geo::aeqd::{frame_new, project, unproject, Frame};

/// Side of a cell, degrees (about 28 km of latitude).
pub const CELL_DEG: f64 = 0.25;
const N_LAT: i64 = 720;
const N_LON: i64 = 1440;
/// The cell of tracks every query includes.
const ALWAYS: u32 = u32::MAX;
/// Margin added to the rectangle, metres, and the fraction of the range added to it.
const EXTRA_FIXED_M: f64 = 2_000.0;
const EXTRA_PER_RANGE: f64 = 0.01;
/// Margin added to the latitude and longitude box, degrees.
const PAD_DEG: f64 = 0.02;
/// Metres of a degree of latitude, for the margin in the plane.
const M_PER_DEG: f64 = 111_200.0;

/// The index. A subsystem (D5): fields private, interface the free functions below.
// A7.2: cold struct (one per radar)
#[derive(Debug, Default)]
pub struct CoarseIndex {
    /// `cell << 32 | track`, ascending after a build.
    keys: Vec<u64>,
    frame: Option<Frame>,
}

/// An index with room for `capacity` tracks, which is all it will ever hold.
pub fn coarse_new(capacity: usize) -> CoarseIndex {
    CoarseIndex {
        keys: Vec::with_capacity(capacity),
        frame: None,
    }
}

/// The address and capacity of the buffer, to check that nothing is reallocated.
pub fn coarse_buffer(ix: &CoarseIndex) -> (usize, usize) {
    (ix.keys.as_ptr() as usize, ix.keys.capacity())
}

fn lat_cell(lat: f64) -> i64 {
    (((lat + 90.0) / CELL_DEG).floor() as i64).clamp(0, N_LAT - 1)
}

fn lon_cell(lon: f64) -> i64 {
    (((lon + 180.0) / CELL_DEG).floor() as i64).rem_euclid(N_LON)
}

fn cell_of(lat: f64, lon: f64) -> u32 {
    (lat_cell(lat) * N_LON + lon_cell(lon)) as u32
}

/// Builds the index of `n` tracks about a radar at `(site_lat, site_lon)`. `place(i)` is track
/// `i`'s latitude and longitude in degrees, or `None` for a track whose polar terms are not finite
/// (every query includes it) and `skip(i)` leaves the track out altogether. At most the
/// capacity is indexed.
pub fn coarse_build(
    ix: &mut CoarseIndex,
    site: (f64, f64),
    n: usize,
    place: impl Fn(usize) -> Option<(f64, f64)>,
    skip: impl Fn(usize) -> bool,
) {
    ix.frame = Some(frame_new(site.0, site.1));
    ix.keys.clear();
    for i in 0..n.min(ix.keys.capacity()) {
        if skip(i) {
            continue;
        }
        let cell = match place(i) {
            Some((lat, lon)) => cell_of(lat, lon),
            None => ALWAYS,
        };
        ix.keys.push(u64::from(cell) << 32 | i as u64);
    }
    ix.keys.sort_unstable();
}

/// Difference of two azimuths in degrees, in `[-180, 180)`.
pub fn az_diff(a: f64, b: f64) -> f64 {
    (a - b + 180.0).rem_euclid(360.0) - 180.0
}

/// Whether a track at slant range `rho`, azimuth `az` and ground range `ground` from the radar,
/// with position standard deviation `sigma`, is inside the coarse window of a plot at `prho` and
/// `paz`: the slant ranges and the cross-ranges are within `window + 8 sigma`. The test of
/// `fusion.rs` (it only saves work, the exact gate decides); a NaN term makes it pass.
pub fn coarse_passes(
    window: f64,
    (prho, paz): (f64, f64),
    (rho, az, ground, sigma): (f64, f64, f64, f64),
) -> bool {
    let coarse = window + 8.0 * sigma;
    !((prho - rho).abs() > coarse || az_diff(paz, az).to_radians().abs() * ground > coarse)
}

/// How far a track is from the plot, metres: the slant range difference and the cross-range
/// together. A NaN term gives 0 (such a track passes the window whatever its place). Orders the
/// candidates of a plot that cannot visit them all.
pub fn coarse_distance(
    (prho, paz): (f64, f64),
    (rho, az, ground, _sigma): (f64, f64, f64, f64),
) -> f64 {
    let d = oa_math::hypot(prho - rho, az_diff(paz, az).to_radians() * ground);
    if d.is_nan() {
        0.0
    } else {
        d
    }
}

/// The tracks of one cell, appended to `cand`.
fn push_cell(keys: &[u64], cand: &mut Vec<u32>, cell: u32) {
    let lo = u64::from(cell) << 32;
    let start = keys.partition_point(|&k| k < lo);
    for &k in keys.get(start..).unwrap_or(&[]) {
        if k >> 32 != u64::from(cell) {
            break;
        }
        cand.push(k as u32);
    }
}

fn push_all(ix: &CoarseIndex, out: &mut Vec<u32>) {
    for &k in &ix.keys {
        out.push(k as u32);
    }
    out.sort_unstable();
}

/// The tracks that may pass the coarse window of a plot at slant range `rho` and azimuth `az_deg`
/// (degrees clockwise from north), where the window is at most `width` wide and no track is more
/// than `dh_max` above or below the radar, written to `out` (cleared first) in ascending track
/// order. A superset of the tracks the window test passes, never smaller; `out` needs room for
/// every track of the index to avoid growing.
pub fn coarse_query(
    ix: &CoarseIndex,
    out: &mut Vec<u32>,
    rho: f64,
    az_deg: f64,
    dh_max: f64,
    width: f64,
) {
    out.clear();
    let Some(frame) = ix.frame else {
        return push_all(ix, out);
    };
    if !(rho.is_finite() && az_deg.is_finite() && dh_max.is_finite() && width.is_finite()) {
        return push_all(ix, out);
    }
    let width = width.max(0.0);
    let rho = rho.max(0.0);
    let extra = EXTRA_FIXED_M + EXTRA_PER_RANGE * rho;
    let w = width + extra;
    let g_hi = rho + w;
    let slant_lo = (rho - width).max(0.0);
    let g_lo = ((slant_lo * slant_lo - dh_max * dh_max).max(0.0).sqrt() - extra).max(0.0);
    // Close to the radar the window reaches back past it by no more than the width; far from it,
    // by no more than the width less than the smallest ground range.
    let a_lo = if g_lo > w { g_lo - w } else { -w };
    let (st, ct) = oa_math::sin_cos(az_deg.to_radians());
    // Latitude and longitude of nine points of the rectangle.
    let mut pts = [(0.0f64, 0.0f64); 9];
    let mut i = 0usize;
    for a in [a_lo, 0.5 * (a_lo + g_hi), g_hi] {
        for c in [-w, 0.0, w] {
            if let Some(p) = pts.get_mut(i) {
                // A point the chart cannot reach is a NaN, which the check below sends to the full scan.
                *p = unproject(&frame, a * st + c * ct, a * ct - c * st)
                    .unwrap_or((f64::NAN, f64::NAN));
            }
            i += 1;
        }
    }
    if pts.iter().any(|p| !(p.0.is_finite() && p.1.is_finite())) {
        return push_all(ix, out);
    }
    let mut lat_lo = pts.iter().map(|p| p.0).fold(f64::INFINITY, f64::min) - PAD_DEG;
    let mut lat_hi = pts.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max) + PAD_DEG;
    // A pole inside the rectangle (in its plane, with a margin), whatever the nine points show.
    let pole_in = |pole: f64| {
        let (x, y) = project(&frame, pole, 0.0);
        let (a, c) = (x * st + y * ct, x * ct - y * st);
        let pad = 2.0 * PAD_DEG * M_PER_DEG;
        !(a.is_finite() && c.is_finite())
            || (a >= a_lo - pad && a <= g_hi + pad && c.abs() <= w + pad)
    };
    let (north, south) = (pole_in(90.0), pole_in(-90.0));
    if north {
        lat_hi = 90.0;
    }
    if south {
        lat_lo = -90.0;
    }
    let lon_c = pts.get(4).map_or(0.0, |p| p.1);
    let wrap = |d: f64| (d + 180.0).rem_euclid(360.0) - 180.0;
    let (mut d_lo, mut d_hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for p in &pts {
        let d = wrap(p.1 - lon_c);
        d_lo = d_lo.min(d);
        d_hi = d_hi.max(d);
    }
    let reaches_pole = north || south;
    let lat_edge = lat_lo.abs().max(lat_hi.abs()).min(89.0);
    let pad_lon = PAD_DEG / oa_math::cos(lat_edge.to_radians()).max(1.0e-3);
    let (lon_from, lon_to) = (lon_c + d_lo - pad_lon, lon_c + d_hi + pad_lon);
    let (c_lo, c_hi) = (lat_cell(lat_lo), lat_cell(lat_hi));
    let mut l_lo = ((lon_from + 180.0) / CELL_DEG).floor() as i64;
    let mut l_hi = ((lon_to + 180.0) / CELL_DEG).floor() as i64;
    if reaches_pole || l_hi - l_lo + 1 >= N_LON {
        (l_lo, l_hi) = (0, N_LON - 1);
    }
    let cells = (c_hi - c_lo + 1).saturating_mul(l_hi - l_lo + 1);
    if cells > ix.keys.len() as i64 + 1 {
        // More cells than tracks: walking the tracks is cheaper than the cells.
        return push_all(ix, out);
    }
    for la in c_lo..=c_hi {
        for lo in l_lo..=l_hi {
            let cell = (la * N_LON + lo.rem_euclid(N_LON)) as u32;
            push_cell(&ix.keys, out, cell);
        }
    }
    push_cell(&ix.keys, out, ALWAYS);
    out.sort_unstable();
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use crate::fusion_params::COARSE_M;
    use crate::radar::{polar_of, polar_to_geodetic, radar_geom, RadarSite};
    use oa_sensorsim::prng::Rng;

    fn site(lat_deg: f64, lon_deg: f64) -> RadarSite {
        RadarSite {
            lat_deg,
            lon_deg,
            height_m: 100.0,
            sigma_range_m: 70.0,
            sigma_az_deg: 0.08,
            range_quantum_m: 0.0,
            alt_quantum_ft: 100,
            period_ns: 4_800_000_000,
            epoch_ns: 0,
            sensor: 1,
        }
    }

    /// One track as the association sees it: place, then polar terms from the radar.
    struct T {
        lat: f64,
        lon: f64,
        polar: (f64, f64, f64, f64),
    }

    /// `n` tracks round the radar at random slant ranges up to `max_m`, heights to 45,000 ft.
    fn tracks(s: &RadarSite, n: usize, max_m: f64, rng: &mut Rng) -> Vec<T> {
        let g = radar_geom(s);
        let mut v = Vec::new();
        while v.len() < n {
            let (rho, az) = (1_000.0 + (max_m - 1_000.0) * rng.unit(), 360.0 * rng.unit());
            let h = -100.0 + 13_800.0 * rng.unit();
            let Ok(geo) = polar_to_geodetic(&g, rho, az, h) else {
                continue;
            };
            let (rho, az) = polar_of(&g, geo.lat_deg, geo.lon_deg, h);
            let dh = h - g.height_m;
            let ground = (rho * rho - dh * dh).max(1.0).sqrt();
            let sigma = 3_400.0 * rng.unit();
            v.push(T {
                lat: geo.lat_deg,
                lon: geo.lon_deg,
                polar: (rho, az, ground, sigma),
            });
        }
        v
    }

    fn built(s: &RadarSite, t: &[T]) -> (CoarseIndex, f64, f64) {
        let mut ix = coarse_new(t.len());
        coarse_build(
            &mut ix,
            (s.lat_deg, s.lon_deg),
            t.len(),
            |i| {
                t.get(i)
                    .filter(|x| {
                        let p = x.polar;
                        p.0.is_finite() && p.1.is_finite() && p.2.is_finite() && p.3.is_finite()
                    })
                    .map(|x| (x.lat, x.lon))
            },
            |_| false,
        );
        let dh = t
            .iter()
            .map(|x| {
                (x.polar.0 * x.polar.0 - x.polar.2 * x.polar.2)
                    .max(0.0)
                    .sqrt()
            })
            .fold(0.0, f64::max);
        let width = t
            .iter()
            .map(|x| COARSE_M + 8.0 * x.polar.3)
            .fold(0.0, f64::max);
        (ix, dh, width)
    }

    // REQ: OA-TRK-177
    #[test]
    fn the_grid_finds_every_track_the_window_passes_anywhere_on_earth() {
        // Radars on the +-180 degree seam, at the equator, near each pole's limit of 85 degrees
        // (a plot at 600 km reaches past the pole) and ordinary; thousands of tracks and plots
        // each, plots near tracks, at random, and tracks with a NaN term.
        let mut rng = Rng::from_seed(77);
        let mut visited = 0usize;
        let mut total = 0usize;
        for (lat, lon) in [
            (41.98, -87.91),
            (0.0, 179.95),
            (-33.0, -179.99),
            (85.0, 20.0),
            (-85.0, -140.0),
            (84.9, 179.9),
            (60.0, 180.0),
        ] {
            let s = site(lat, lon);
            let mut t = tracks(&s, 3_000, 600_000.0, &mut rng);
            t.push(T {
                lat: 0.0,
                lon: 0.0,
                polar: (f64::NAN, 10.0, 5_000.0, 10.0),
            });
            let (ix, dh, width) = built(&s, &t);
            assert_eq!(coarse_buffer(&ix).1, t.len());
            let mut cand = Vec::with_capacity(t.len());
            let mut found = 0usize;
            for k in 0..3_000usize {
                // Near a track (within a few kilometres) or anywhere.
                let (prho, paz) = if k % 3 == 0 {
                    (1_000.0 + 599_000.0 * rng.unit(), 360.0 * rng.unit())
                } else {
                    let x = &t[k % (t.len() - 1)].polar;
                    (
                        x.0 + 4_000.0 * (rng.unit() - 0.5),
                        (x.1 + 0.05 * (rng.unit() - 0.5)).rem_euclid(360.0),
                    )
                };
                coarse_query(&ix, &mut cand, prho, paz, dh, width);
                assert!(cand.windows(2).all(|w| w[0] < w[1]), "not ascending");
                let brute: Vec<u32> = (0..t.len() as u32)
                    .filter(|&i| coarse_passes(COARSE_M, (prho, paz), t[i as usize].polar))
                    .collect();
                let narrowed: Vec<u32> = cand
                    .iter()
                    .copied()
                    .filter(|&i| coarse_passes(COARSE_M, (prho, paz), t[i as usize].polar))
                    .collect();
                assert_eq!(narrowed, brute, "site {lat} {lon} plot {prho} {paz}");
                found += brute.len();
                visited += cand.len();
                total += t.len();
            }
            assert!(found > 0, "site {lat} {lon}: the test found nothing");
        }
        // The grid is a narrowing, not the whole list (the poles and the NaN track excepted).
        assert!(visited * 4 < total, "{visited} of {total}");
    }

    // REQ: OA-TRK-177
    #[test]
    fn twenty_thousand_plots_over_twenty_thousand_tracks_across_the_conus_take_well_under_a_second()
    {
        // Tracks over the contiguous United States as seen from a radar in the middle of it
        // (every plot is a slant range and azimuth, so a plot is as far from the radar as the
        // track it is for); the unfiltered scan of 4e8 pairs is what this replaced.
        let s = site(39.0, -98.0);
        let g = radar_geom(&s);
        let mut rng = Rng::from_seed(5);
        let mut t = Vec::new();
        while t.len() < 20_000 {
            let (lat, lon) = (25.0 + 24.0 * rng.unit(), -125.0 + 58.0 * rng.unit());
            let h = 3_000.0 + 9_000.0 * rng.unit();
            let (rho, az) = polar_of(&g, lat, lon, h);
            let dh = h - g.height_m;
            t.push(T {
                lat,
                lon,
                polar: (rho, az, (rho * rho - dh * dh).max(1.0).sqrt(), 50.0),
            });
        }
        let started = std::time::Instant::now();
        let (ix, dh, width) = built(&s, &t);
        let mut cand = Vec::with_capacity(t.len());
        let mut visited = 0usize;
        let mut hits = 0usize;
        for k in 0..20_000usize {
            let x = &t[k].polar;
            let (prho, paz) = (
                x.0 + 100.0 * rng.normal(),
                (x.1 + 0.02 * rng.normal()).rem_euclid(360.0),
            );
            coarse_query(&ix, &mut cand, prho, paz, dh, width);
            visited += cand.len();
            if cand.contains(&(k as u32)) {
                hits += 1;
            }
            for &i in &cand {
                let _ = coarse_passes(COARSE_M, (prho, paz), t[i as usize].polar);
            }
        }
        let took = started.elapsed().as_secs_f64();
        assert_eq!(hits, 20_000, "a plot's own track is always a candidate");
        assert!(visited < 20_000 * 400, "{visited} candidates visited");
        // A debug build on a loaded machine; the scan of every pair took longer than this.
        assert!(took < 10.0, "{took} s");
    }

    /// A track at `(lat, lon)` at the radar's own height, seen from `s`, with standard deviation
    /// `sigma`.
    fn track_at(s: &RadarSite, lat: f64, lon: f64, sigma: f64) -> T {
        let g = radar_geom(s);
        let (rho, az) = polar_of(&g, lat, lon, g.height_m);
        T {
            lat,
            lon,
            polar: (rho, az, rho, sigma),
        }
    }

    /// `n` tracks at the radar's height round it, then `extra`, as an index; checks that the
    /// query for the plot keeps every track the window passes, and that the window passes `want`
    /// or more of the extras.
    fn finds_extras(
        s: &RadarSite,
        n: usize,
        extra: Vec<T>,
        plot: (f64, f64),
        width: f64,
        want: usize,
    ) {
        let mut rng = Rng::from_seed(n as u64);
        let g = radar_geom(s);
        let mut t = Vec::new();
        while t.len() < n {
            let (rho, az) = (1_000.0 + 599_000.0 * rng.unit(), 360.0 * rng.unit());
            let Ok(geo) = polar_to_geodetic(&g, rho, az, g.height_m) else {
                continue;
            };
            t.push(track_at(s, geo.lat_deg, geo.lon_deg, 0.0));
        }
        let first = t.len();
        t.extend(extra);
        let (ix, _, _) = built(s, &t);
        let mut cand = Vec::with_capacity(t.len());
        coarse_query(&ix, &mut cand, plot.0, plot.1, 0.0, width);
        let passes = |i: u32| coarse_passes(COARSE_M, plot, t[i as usize].polar);
        let brute: Vec<u32> = (0..t.len() as u32).filter(|&i| passes(i)).collect();
        let narrowed: Vec<u32> = cand.iter().copied().filter(|&i| passes(i)).collect();
        let hit = brute.iter().filter(|&&i| i as usize >= first).count();
        assert!(
            hit >= want,
            "{n} tracks: the window passes {hit} extras, wanted {want}"
        );
        assert_eq!(narrowed, brute, "{n} tracks, {} candidates", cand.len());
    }

    // REQ: OA-TRK-220
    #[test]
    fn a_track_beyond_the_pole_is_found_when_no_sample_point_is_near_it() {
        // Index about (85 N, 20 E); the track at (89.99 N, 170 E) is 550 km over the pole, the
        // rectangle (30 km wide) holds the pole and every sampled point is 5 degrees or more
        // from it. Sizes on both sides of the point where the cells outnumber the tracks.
        let s = site(85.0, 20.0);
        for n in [200, 1_000, 3_000, 5_000, 10_000, 20_000] {
            let target = track_at(&s, 89.99, 170.0, 3_375.0);
            finds_extras(&s, n, vec![target], (550_000.0, 0.0), 30_000.0, 1);
        }
    }

    // REQ: OA-TRK-221
    #[test]
    fn a_ring_of_tracks_round_a_pole_inside_the_rectangle_is_found() {
        // Site 80 N, plot at 1,100 km north: the pole (about 1,112 km) is inside the rectangle
        // (20 km wide) and the sampled points are all more than a degree from it.
        let s = site(80.0, 10.0);
        let frame = frame_new(s.lat_deg, s.lon_deg);
        let (px, py) = oa_geo::aeqd::project(&frame, 90.0, 0.0);
        let (along, cross) = (py, px);
        let w = 20_000.0 + EXTRA_FIXED_M + EXTRA_PER_RANGE * 1_100_000.0;
        assert!(
            along < 1_100_000.0 + w && cross.abs() < w,
            "{along} {cross}"
        );
        let ring = |r_m: f64| -> Vec<T> {
            (0..36)
                .map(|k| {
                    let lat = 90.0 - r_m / 111_200.0;
                    track_at(&s, lat, 10.0 * f64::from(k) - 180.0, 2_125.0)
                })
                .collect()
        };
        for n in [200, 1_000, 3_000, 5_000, 10_000, 20_000] {
            let mut extra = ring(3_000.0);
            extra.extend(ring(10_000.0));
            finds_extras(&s, n, extra, (1_100_000.0, 0.0), 20_000.0, 10);
        }
    }
}
