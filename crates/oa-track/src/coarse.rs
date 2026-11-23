//! A grid over the live tracks of one association, in cells of about [`CELL_DEG`] degrees (5.6
//! km) on a side, so that the coarse window of a plot is tested against the few tracks near it
//! instead of every track. Core (pure).
//!
//! The window of `fusion.rs` is in the radar's own polar terms (slant range and azimuth, scaled by
//! the track's ground range). The grid does not replace that test: it only narrows the tracks the
//! test is applied to, to a set certain to contain every track the test would pass, so the
//! outcome is the same as testing every track. The cells are bands of [`CELL_DEG`] of latitude,
//! each cut into as many cells of longitude as keep a cell at least as wide as it is tall at the
//! band's edge nearest a pole (about 7,200 at the equator, one in the polar band), so a cell is the
//! same size everywhere on earth. The set is the tracks in the cells of the latitude and longitude
//! boxes of the region that can pass the window:
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
//! The rectangle is cut along its length into pieces of at most [`SEGMENT_M`], and each piece has a
//! box: the latitude and longitude extent of nine points of the piece, with a margin. A box over
//! the whole rectangle would, on a long rectangle at an angle, hold many times its area in cells.
//! A pole that lies inside a piece (judged in the rectangle's own plane, with a margin, not
//! from the nine points) makes that piece's box run to that pole and take every longitude of the
//! bands it spans (few, near a pole); longitudes are taken relative to the box's centre and
//! wrapped, so a box across the +-180 degree seam is two runs of cells; a latitude beyond the
//! poles is clamped to the last cell.
//!
//! **Superset.** Every track is in the cell of its own latitude and longitude. A piece's box
//! contains the piece (the margin covers the curvature between the nine points, which is smaller
//! the shorter the piece), the pieces' union is the rectangle, and the query takes every cell that
//! meets a box (every band of the box's latitudes, and in each the cells of the box's longitudes
//! or all of them). A track the window passes is in the rectangle, so in a cell taken. Anything
//! the box cannot say (a point the chart cannot reach, a count of cells above the number of tracks)
//! takes every track.
//!
//! The index is a sorted array of one word per track (cell number above, track number below), built
//! in a buffer allocated when it is made and queried into one the caller owns: building is a sort in place, a query a binary
//! search per cell. Tracks whose polar terms are not finite pass the window test whatever their
//! place (the comparison with a NaN is false), so they have a cell of their own that every query
//! includes.

use oa_geo::aeqd::{frame_new, project, unproject, Frame};

/// Height of a band of cells, degrees (about 5.6 km of latitude).
pub const CELL_DEG: f64 = 0.05;
const N_LAT: i64 = 3_600;
/// Longest piece of the window's rectangle that has a box of its own, metres.
pub const SEGMENT_M: f64 = 20_000.0;
/// Most pieces of one rectangle (a rectangle longer than this times [`SEGMENT_M`] has longer pieces).
const SEGMENTS_MAX: usize = 64;
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
    /// Per band of latitude: the cells of longitude in it, and the number of the first cell.
    lon_cells: Vec<u32>,
    first_cell: Vec<u32>,
}

/// An index with room for `capacity` tracks, which is all it will ever hold.
pub fn coarse_new(capacity: usize) -> CoarseIndex {
    let mut lon_cells = Vec::with_capacity(N_LAT as usize);
    let mut first_cell = Vec::with_capacity(N_LAT as usize);
    let mut next = 0u32;
    for la in 0..N_LAT {
        let lo = -90.0 + CELL_DEG * la as f64;
        let edge = lo.abs().max((lo + CELL_DEG).abs()).min(90.0);
        let n = ((360.0 * oa_math::cos(edge.to_radians()) / CELL_DEG).floor() as u32).max(1);
        lon_cells.push(n);
        first_cell.push(next);
        next += n;
    }
    CoarseIndex {
        keys: Vec::with_capacity(capacity),
        frame: None,
        lon_cells,
        first_cell,
    }
}

/// The address and capacity of the buffer, to check that nothing is reallocated.
pub fn coarse_buffer(ix: &CoarseIndex) -> (usize, usize) {
    (ix.keys.as_ptr() as usize, ix.keys.capacity())
}

fn lat_cell(lat: f64) -> i64 {
    (((lat + 90.0) / CELL_DEG).floor() as i64).clamp(0, N_LAT - 1)
}

/// The cells of longitude of band `la`.
fn band_cells(ix: &CoarseIndex, la: i64) -> i64 {
    ix.lon_cells.get(la as usize).map_or(1, |&n| i64::from(n))
}

/// The cell number of the place, or [`ALWAYS`] for an index without its bands.
fn cell_of(ix: &CoarseIndex, lat: f64, lon: f64) -> u32 {
    let la = lat_cell(lat);
    let n = band_cells(ix, la);
    let lo = (((lon + 180.0) / 360.0 * n as f64).floor() as i64).rem_euclid(n);
    match ix.first_cell.get(la as usize) {
        Some(&f) => f + lo as u32,
        None => ALWAYS,
    }
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
            Some((lat, lon)) => cell_of(ix, lat, lon),
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

/// Result of a point-grid query with an explicit work allowance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointGridQueryResult {
    /// The query completed; `false` means the caller should use its wide fallback.
    Complete(bool),
    /// Work ran out before the next cell/probe/entry visit. `out` contains a partial result.
    Capped,
    /// The caller's output buffer cannot hold every indexed point without reallocating.
    OutputFull,
}

fn push_all(ix: &CoarseIndex, out: &mut Vec<u32>) {
    out.clear();
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
    if ix.lon_cells.len() != N_LAT as usize {
        return push_all(ix, out);
    }
    // The rectangle in pieces, each with a box of its own. A cell that meets the boxes of two
    // pieces is taken with the first, so no track is appended twice and `out` never holds more
    // than the tracks.
    let length = g_hi - a_lo;
    let pieces = ((length / SEGMENT_M).ceil() as usize).clamp(1, SEGMENTS_MAX);
    let step = length / pieces as f64;
    let mut boxes = [PieceBox::default(); SEGMENTS_MAX];
    for k in 0..pieces {
        let (p_lo, p_hi) = (a_lo + step * k as f64, a_lo + step * (k + 1) as f64);
        let p_hi = if k + 1 == pieces { g_hi } else { p_hi };
        let Some(b) = piece_box(&frame, (st, ct), (p_lo, p_hi), w) else {
            return push_all(ix, out);
        };
        if let Some(slot) = boxes.get_mut(k) {
            *slot = b;
        }
        if !piece_cells(ix, out, &boxes, k) {
            return push_all(ix, out);
        }
    }
    push_cell(&ix.keys, out, ALWAYS);
    out.sort_unstable();
}

/// The box of one piece of the rectangle: the bands of latitude and the longitudes it meets.
#[derive(Debug, Clone, Copy, Default)]
struct PieceBox {
    c_lo: i64,
    c_hi: i64,
    lon_from: f64,
    lon_to: f64,
    reaches_pole: bool,
}

/// The cells of longitude of band `la` that the box takes: from the cell of its west edge to its
/// east edge (the first and last, possibly past `0..n` before wrapping), or all of them; and the
/// number of cells of the band.
fn box_span(ix: &CoarseIndex, b: &PieceBox, la: i64) -> (i64, i64, i64) {
    let n = band_cells(ix, la);
    let at = |lon: f64| ((lon + 180.0) / 360.0 * n as f64).floor() as i64;
    let (l_lo, l_hi) = (at(b.lon_from), at(b.lon_to));
    if b.reaches_pole || l_hi - l_lo + 1 >= n {
        (0, n - 1, n)
    } else {
        (l_lo, l_hi, n)
    }
}

/// Whether cell `lo` of band `la` is taken by the box.
fn box_takes(ix: &CoarseIndex, b: &PieceBox, la: i64, lo: i64) -> bool {
    if la < b.c_lo || la > b.c_hi {
        return false;
    }
    let (l_lo, l_hi, n) = box_span(ix, b, la);
    (lo - l_lo).rem_euclid(n) <= l_hi - l_lo
}

/// The box of the piece of the rectangle from `a_lo` to `a_hi` along the plot's azimuth line
/// (`st`, `ct`: its sine and cosine) and `w` either side of it. `None` if the tracks are better
/// all taken (a point the chart cannot reach).
fn piece_box(
    frame: &Frame,
    (st, ct): (f64, f64),
    (a_lo, a_hi): (f64, f64),
    w: f64,
) -> Option<PieceBox> {
    // Latitude and longitude of nine points of the piece.
    let mut pts = [(0.0f64, 0.0f64); 9];
    let mut i = 0usize;
    for a in [a_lo, 0.5 * (a_lo + a_hi), a_hi] {
        for c in [-w, 0.0, w] {
            if let Some(p) = pts.get_mut(i) {
                // A point the chart cannot reach is a NaN, which the check below sends to the full scan.
                *p = unproject(frame, a * st + c * ct, a * ct - c * st)
                    .unwrap_or((f64::NAN, f64::NAN));
            }
            i += 1;
        }
    }
    if pts.iter().any(|p| !(p.0.is_finite() && p.1.is_finite())) {
        return None;
    }
    let mut lat_lo = pts.iter().map(|p| p.0).fold(f64::INFINITY, f64::min) - PAD_DEG;
    let mut lat_hi = pts.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max) + PAD_DEG;
    // A pole inside the piece (in its plane, with a margin), whatever the nine points show.
    let pole_in = |pole: f64| {
        let (x, y) = project(frame, pole, 0.0);
        let (a, c) = (x * st + y * ct, x * ct - y * st);
        let pad = 2.0 * PAD_DEG * M_PER_DEG;
        !(a.is_finite() && c.is_finite())
            || (a >= a_lo - pad && a <= a_hi + pad && c.abs() <= w + pad)
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
    Some(PieceBox {
        c_lo,
        c_hi,
        lon_from,
        lon_to,
        reaches_pole,
    })
}

/// Appends the tracks of the cells of piece `k` that no earlier piece's box takes. `false` if the
/// tracks are better all taken (more cells than tracks), with nothing certain about `out`.
fn piece_cells(ix: &CoarseIndex, out: &mut Vec<u32>, boxes: &[PieceBox], k: usize) -> bool {
    let Some(b) = boxes.get(k) else {
        return false;
    };
    let cells: i64 = (b.c_lo..=b.c_hi)
        .map(|la| {
            let (l_lo, l_hi, _) = box_span(ix, b, la);
            l_hi - l_lo + 1
        })
        .sum();
    if cells > ix.keys.len() as i64 + 1 {
        // More cells than tracks: walking the tracks is cheaper than the cells.
        return false;
    }
    let earlier = boxes.get(..k).unwrap_or(&[]);
    for la in b.c_lo..=b.c_hi {
        let (l_lo, l_hi, n) = box_span(ix, b, la);
        let first = ix.first_cell.get(la as usize).copied().unwrap_or(0);
        for lo in l_lo..=l_hi {
            let lo = lo.rem_euclid(n);
            if earlier.iter().any(|e| box_takes(ix, e, la, lo)) {
                continue;
            }
            push_cell(&ix.keys, out, first + lo as u32);
        }
    }
    true
}

// ---- points in a plane --------------------------------------------------------------------

/// Most rings of cells a [`point_grid_query`] searches round the cell of the point; a larger
/// radius is answered with `false`, for the caller to search every point.
pub const POINT_RINGS_MAX: i64 = 3;
/// Cells either side of the origin along each axis.
const POINT_CELLS: i64 = 1 << 15;
/// Large point grids use four stable byte passes over the packed 32-bit cell key.
const POINT_RADIX_THRESHOLD: usize = 256;
const POINT_RADIX_BUCKETS: usize = 256;
const POINT_RADIX_PASSES: usize = 4;

/// A uniform grid of points in a plane (metres), for the initiation of tracks: the candidates of
/// a radar are put in by predicted position, and the plot looks only at the cells round it. The
/// same layout as [`CoarseIndex`]: one word a point (cell above, number below), sorted, in a
/// buffer allocated when it is made. Core (pure).
// A7.2: cold struct (one per fusion layer)
#[derive(Debug, Default)]
pub struct PointGrid {
    keys: Vec<u64>,
    radix_scratch: Vec<u64>,
    cell_m: f64,
}

/// A grid of cells `cell_m` wide with room for `capacity` points, which is all it will ever hold.
pub fn point_grid_new(capacity: usize, cell_m: f64) -> PointGrid {
    let keys = Vec::with_capacity(capacity);
    let radix_scratch = vec![0; keys.capacity()];
    PointGrid {
        keys,
        radix_scratch,
        cell_m: cell_m.max(1.0),
    }
}

/// The address and capacity of the buffer, to check that nothing is reallocated.
pub fn point_grid_buffer(g: &PointGrid) -> (usize, usize) {
    (g.keys.as_ptr() as usize, g.keys.capacity())
}

/// The address and capacity of the preallocated large-grid radix scratch.
pub fn point_grid_radix_buffer(g: &PointGrid) -> (usize, usize) {
    (
        g.radix_scratch.as_ptr() as usize,
        g.radix_scratch.capacity(),
    )
}

fn point_cell(g: &PointGrid, x: f64, y: f64) -> (i64, i64) {
    let c = |v: f64| ((v / g.cell_m).floor() as i64).clamp(-POINT_CELLS, POINT_CELLS - 1);
    (c(x), c(y))
}

fn point_key(cx: i64, cy: i64) -> u32 {
    (((cx + POINT_CELLS) as u32) << 16) | ((cy + POINT_CELLS) as u32)
}

/// Builds the grid of points `0..n`: `place(i)` is point `i`'s place, or `None` to leave it out
/// (the caller keeps such points in a list of its own). At most the capacity is indexed.
pub fn point_grid_build(g: &mut PointGrid, n: usize, place: impl Fn(usize) -> Option<(f64, f64)>) {
    g.keys.clear();
    for i in 0..n.min(g.keys.capacity()) {
        let Some((x, y)) = place(i).filter(|p| p.0.is_finite() && p.1.is_finite()) else {
            continue;
        };
        let (cx, cy) = point_cell(g, x, y);
        g.keys.push(u64::from(point_key(cx, cy)) << 32 | i as u64);
    }
    g.keys.sort_unstable();
}

/// Result of building a point grid under an explicit work allowance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointGridBuildResult {
    /// Every input was visited and the complete grid was sorted.
    Complete,
    /// The work allowance ended before the grid could be completed; the grid is empty.
    Capped,
    /// The declared input count exceeds the preallocated key capacity; the grid is empty.
    CapacityFull,
}

/// Builds and sorts a point grid without allocation. Large grids use a stable radix sort of the
/// packed 32-bit cell key; small grids retain the budgeted heapsort. Work is charged before each
/// input visit or, for radix, before the complete fixed-pass bound. On any incomplete result, the
/// partial grid is cleared and must not be queried.
pub fn point_grid_build_budgeted(
    g: &mut PointGrid,
    n: usize,
    place: impl Fn(usize) -> Option<(f64, f64)>,
    remaining: &mut usize,
    spent: &mut usize,
) -> PointGridBuildResult {
    g.keys.clear();
    if n > g.keys.capacity() {
        return PointGridBuildResult::CapacityFull;
    }
    for i in 0..n {
        if !charge_work(remaining, spent) {
            g.keys.clear();
            return PointGridBuildResult::Capped;
        }
        let Some((x, y)) = place(i).filter(|p| p.0.is_finite() && p.1.is_finite()) else {
            continue;
        };
        let (cx, cy) = point_cell(g, x, y);
        g.keys.push(u64::from(point_key(cx, cy)) << 32 | i as u64);
    }
    let sorted = if g.keys.len() > POINT_RADIX_THRESHOLD {
        point_grid_radix_sort_budgeted(g, remaining, spent)
    } else {
        sort_budgeted(&mut g.keys, remaining, spent)
    };
    if !sorted {
        g.keys.clear();
        return PointGridBuildResult::Capped;
    }
    PointGridBuildResult::Complete
}

/// Stable radix sorting works on the 32-bit cell half of each key. Input indexes are appended in
/// ascending order by the builder, so stability also preserves the low 32-bit index ordering.
/// The complete bound is reserved before either buffer is modified: two length scans (count and
/// scatter), histogram clear, and prefix sum per byte pass.
fn point_grid_radix_sort_budgeted(
    g: &mut PointGrid,
    remaining: &mut usize,
    spent: &mut usize,
) -> bool {
    let len = g.keys.len();
    let Some(per_pass) = len
        .checked_mul(2)
        .and_then(|n| n.checked_add(2 * POINT_RADIX_BUCKETS))
    else {
        return false;
    };
    let Some(required) = per_pass.checked_mul(POINT_RADIX_PASSES) else {
        return false;
    };
    let Some(scratch) = g.radix_scratch.get_mut(..len) else {
        return false;
    };
    if !charge_work_many(remaining, spent, required) {
        return false;
    }

    let keys = g.keys.as_mut_slice();
    for pass in (0..POINT_RADIX_PASSES).step_by(2) {
        let shift = 32 + pass * 8;
        if !point_grid_radix_pass(keys, scratch, shift)
            || !point_grid_radix_pass(scratch, keys, shift + 8)
        {
            return false;
        }
    }
    true
}

fn point_grid_radix_pass(source: &[u64], target: &mut [u64], shift: usize) -> bool {
    let bucket = |key| ((key >> shift) & 0xff_u64) as usize;
    let mut positions = [0usize; POINT_RADIX_BUCKETS];
    for &key in source {
        let Some(count) = positions.get_mut(bucket(key)) else {
            return false;
        };
        *count += 1;
    }
    let mut offset = 0usize;
    for position in &mut positions {
        // The histogram counts each source element once, so the total fits the source length.
        let next = offset + *position;
        *position = offset;
        offset = next;
    }
    for &key in source {
        let Some(position) = positions.get_mut(bucket(key)) else {
            return false;
        };
        let Some(slot) = target.get_mut(*position) else {
            return false;
        };
        *slot = key;
        *position += 1;
    }
    true
}

/// In-place heapsort with no allocation. Each sort-loop iteration, comparison, and swap consumes
/// one work unit before it occurs; `false` means the caller must discard the partially sorted
/// slice. The grid uses packed `u64` keys.
pub fn sort_budgeted(items: &mut [u64], remaining: &mut usize, spent: &mut usize) -> bool {
    sort_values_budgeted(items, remaining, spent)
}

/// The tracker also uses the same counted ordering for packed radar distance keys.
pub(crate) fn sort_values_budgeted<T: Ord>(
    items: &mut [T],
    remaining: &mut usize,
    spent: &mut usize,
) -> bool {
    let len = items.len();
    if len < 2 {
        return true;
    }

    // Build a max heap, starting at the last parent and moving toward the root.
    for root in (0..len / 2).rev() {
        if !charge_work(remaining, spent) {
            return false;
        }
        if !sift_down(items, root, len, remaining, spent) {
            return false;
        }
    }

    // Move the maximum to the end, then restore the heap in the remaining prefix.
    for end in (1..len).rev() {
        if !charge_work(remaining, spent) {
            return false;
        }
        if !charge_work(remaining, spent) {
            return false;
        }
        items.swap(0, end);
        if !sift_down(items, 0, end, remaining, spent) {
            return false;
        }
    }
    true
}

fn sift_down<T: Ord>(
    items: &mut [T],
    mut root: usize,
    end: usize,
    remaining: &mut usize,
    spent: &mut usize,
) -> bool {
    while root < end / 2 {
        if !charge_work(remaining, spent) {
            return false;
        }
        // E8: root < end / 2 proves 2*root + 2 <= end, so both child-index operations fit usize.
        let left = root * 2 + 1;
        let right = left + 1;
        let mut largest = left;
        if right < end {
            if !charge_work(remaining, spent) {
                return false;
            }
            let (Some(left_item), Some(right_item)) = (items.get(left), items.get(right)) else {
                return false;
            };
            if left_item < right_item {
                largest = right;
            }
        }
        if !charge_work(remaining, spent) {
            return false;
        }
        let (Some(root_item), Some(child_item)) = (items.get(root), items.get(largest)) else {
            return false;
        };
        if root_item >= child_item {
            break;
        }
        if !charge_work(remaining, spent) {
            return false;
        }
        items.swap(root, largest);
        root = largest;
    }
    true
}

fn charge_work(remaining: &mut usize, spent: &mut usize) -> bool {
    let Some(next_spent) = spent.checked_add(1) else {
        return false;
    };
    if *remaining == 0 {
        return false;
    }
    *remaining -= 1;
    *spent = next_spent;
    true
}

fn charge_work_many(remaining: &mut usize, spent: &mut usize, amount: usize) -> bool {
    let Some(next_spent) = spent.checked_add(amount) else {
        return false;
    };
    if *remaining < amount {
        return false;
    }
    *remaining -= amount;
    *spent = next_spent;
    true
}

/// Result of sorting and deduplicating candidate indexes through a bounded bitset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortUniqueResult {
    /// The first `len` input entries now contain the ascending unique indexes.
    Complete(usize),
    /// The full finite work bound did not fit; neither input nor scratch was modified.
    Capped,
    /// An input candidate index is outside `0..domain`; input stays unchanged and scratch clears.
    InvalidIndex,
    /// The scratch slice cannot represent the declared domain; no buffers are modified.
    ScratchTooSmall,
}

/// Sorts and deduplicates indexes in place using caller-owned bitset scratch. The active scratch
/// prefix is `ceil(domain / 64)` words. Before mutation, the full finite bound `2*words + 2*N`
/// is reserved: clearing and scanning the active words, marking N inputs, and emitting at most N
/// set indexes. `Capped` leaves both buffers unchanged. Invalid indexes leave the input unchanged
/// and clear the active scratch prefix. Scratch beyond the active prefix is untouched.
pub fn sort_unique_indices_budgeted(
    indices: &mut [u32],
    scratch: &mut [u64],
    domain: usize,
    remaining: &mut usize,
    spent: &mut usize,
) -> SortUniqueResult {
    let word_count = if domain % 64 == 0 {
        domain / 64
    } else {
        domain / 64 + 1
    };
    if scratch.len() < word_count {
        return SortUniqueResult::ScratchTooSmall;
    }
    let Some(index_work) = indices.len().checked_mul(2) else {
        return SortUniqueResult::Capped;
    };
    let Some(word_work) = word_count.checked_mul(2) else {
        return SortUniqueResult::Capped;
    };
    let Some(required) = word_work.checked_add(index_work) else {
        return SortUniqueResult::Capped;
    };
    let Some(next_spent) = spent.checked_add(required) else {
        return SortUniqueResult::Capped;
    };
    if *remaining < required {
        return SortUniqueResult::Capped;
    }
    *remaining -= required;
    *spent = next_spent;

    let Some(active) = scratch.get_mut(..word_count) else {
        return SortUniqueResult::ScratchTooSmall;
    };
    active.fill(0);
    for &index in indices.iter() {
        let index = index as usize;
        if index >= domain {
            active.fill(0);
            return SortUniqueResult::InvalidIndex;
        }
        let word = index / 64;
        let bit = index % 64;
        let Some(word_value) = active.get_mut(word) else {
            active.fill(0);
            return SortUniqueResult::ScratchTooSmall;
        };
        *word_value |= 1u64 << bit;
    }

    let mut output_len = 0usize;
    for (word_index, word) in active.iter_mut().enumerate() {
        let mut bits = *word;
        *word = 0;
        while bits != 0 {
            let bit = bits.trailing_zeros() as usize;
            let index = word_index * 64 + bit;
            let Some(output) = indices.get_mut(output_len) else {
                return SortUniqueResult::ScratchTooSmall;
            };
            *output = index as u32;
            output_len += 1;
            bits &= bits - 1;
        }
    }
    SortUniqueResult::Complete(output_len)
}

/// Appends to `out` every point within `r` of `(x, y)`, and some others, in no order; `true`.
/// `false`, with nothing appended, when `r` is not a finite number or reaches more than
/// [`POINT_RINGS_MAX`] cells. `out` needs room for the points to avoid growing.
pub fn point_grid_query(g: &PointGrid, out: &mut Vec<u32>, x: f64, y: f64, r: f64) -> bool {
    if !(r.is_finite() && x.is_finite() && y.is_finite()) {
        return false;
    }
    let rings = (r.max(0.0) / g.cell_m).ceil() as i64;
    if rings > POINT_RINGS_MAX {
        return false;
    }
    let (cx, cy) = point_cell(g, x, y);
    for ix in (cx - rings).max(-POINT_CELLS)..=(cx + rings).min(POINT_CELLS - 1) {
        for iy in (cy - rings).max(-POINT_CELLS)..=(cy + rings).min(POINT_CELLS - 1) {
            push_cell(&g.keys, out, point_key(ix, iy));
        }
    }
    true
}

/// Point-grid query that charges before every column, binary-search comparison, and key visit.
/// `remaining` is decreased only for completed/attempted work, and `spent` reports the same
/// units for the owner's counter. `out` needs room for every indexed point to avoid growing.
/// On `Capped`, callers must discard `out` and refuse the query.
pub fn point_grid_query_budgeted(
    g: &PointGrid,
    out: &mut Vec<u32>,
    x: f64,
    y: f64,
    r: f64,
    remaining: &mut usize,
    spent: &mut usize,
) -> PointGridQueryResult {
    if out.capacity() < g.keys.len() {
        return PointGridQueryResult::OutputFull;
    }
    out.clear();
    if !(r.is_finite() && x.is_finite() && y.is_finite()) {
        return PointGridQueryResult::Complete(false);
    }
    let rings = (r.max(0.0) / g.cell_m).ceil() as i64;
    if rings > POINT_RINGS_MAX {
        return PointGridQueryResult::Complete(false);
    }
    let (cx, cy) = point_cell(g, x, y);
    let min_y = (cy - rings).max(-POINT_CELLS);
    let max_y = (cy + rings).min(POINT_CELLS - 1);
    for ix in (cx - rings).max(-POINT_CELLS)..=(cx + rings).min(POINT_CELLS - 1) {
        if !charge_point_grid(remaining, spent) {
            return PointGridQueryResult::Capped;
        }
        // Keys sort by x, then y, then point index. The y range is contiguous within each x.
        let lo_key = u64::from(point_key(ix, min_y)) << 32;
        let hi_cell = u64::from(point_key(ix, max_y));
        let mut lo = 0usize;
        let mut hi = g.keys.len();
        while lo < hi {
            if !charge_point_grid(remaining, spent) {
                return PointGridQueryResult::Capped;
            }
            let mid = lo + (hi - lo) / 2;
            if g.keys.get(mid).is_some_and(|&key| key < lo_key) {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        for &key in g.keys.get(lo..).unwrap_or_default() {
            if !charge_point_grid(remaining, spent) {
                return PointGridQueryResult::Capped;
            }
            if key >> 32 > hi_cell {
                break;
            }
            out.push(key as u32);
        }
    }
    PointGridQueryResult::Complete(true)
}

fn charge_point_grid(remaining: &mut usize, spent: &mut usize) -> bool {
    if *remaining == 0 {
        return false;
    }
    *remaining -= 1;
    *spent = (*spent).saturating_add(1);
    true
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

    // REQ: OA-TRK-177, OA-TRK-300
    #[test]
    fn candidate_query_has_no_false_negatives_against_independent_exact_reference() {
        // Deterministic bounds: 192 indexed tracks and 72 queries per world. The expected side
        // computes the polar window directly and never calls the production gate predicate.
        const TRACK_CAP: usize = 192;
        const QUERY_CAP: usize = 72;
        const SIGMA_HIGH_M: f64 = 30_000.0;

        let reference_passes =
            |(prho, paz): (f64, f64), (rho, az, ground, sigma): (f64, f64, f64, f64)| {
                let limit = COARSE_M + 8.0 * sigma;
                let mut turn = (paz - az + 180.0).rem_euclid(360.0) - 180.0;
                if turn > 180.0 {
                    turn -= 360.0;
                }
                let radial_error = (prho - rho).abs();
                let lateral_error = turn.abs() * core::f64::consts::PI / 180.0 * ground;
                !(radial_error > limit || lateral_error > limit)
            };

        let mut rng = Rng::from_seed(0xA71C_0A55);
        for (world, (lat, lon, max_range)) in [
            (0.0, 179.999, 90_000.0),
            (89.95, 30.0, 40_000.0),
            (-89.9, -120.0, 40_000.0),
            (41.98, -87.91, 90_000.0),
        ]
        .into_iter()
        .enumerate()
        {
            let s = site(lat, lon);
            let g = radar_geom(&s);
            let mut t = tracks(&s, TRACK_CAP - 4, max_range, &mut rng);

            // Crafted exact and near-boundary cases: close to the radar, azimuth seam, parallel
            // and crossing snapshots, curved-path point, missing position, and high covariance.
            for (rho, az, height, sigma) in [
                (0.0, 359.999, 100.0, 0.0),
                (COARSE_M, 0.001, 1_000.0, 0.0),
                (max_range * 0.5, 90.0, 6_000.0, 0.01),
                (max_range * 0.5, 90.0001, 6_000.0, 0.01),
            ] {
                let geo = polar_to_geodetic(&g, rho.max(1.0), az, height).unwrap();
                let (rho, az) = polar_of(&g, geo.lat_deg, geo.lon_deg, height);
                let ground = (rho * rho - (height - g.height_m).powi(2)).max(1.0).sqrt();
                t.push(T {
                    lat: geo.lat_deg,
                    lon: geo.lon_deg,
                    polar: (rho, az, ground, sigma),
                });
            }
            if let Some(wide) = t.get_mut(TRACK_CAP - 2) {
                wide.polar.3 = SIGMA_HIGH_M;
            }
            if let Some(unplaced) = t.get_mut(TRACK_CAP - 1) {
                unplaced.lat = f64::NAN;
                unplaced.lon = f64::NAN;
                unplaced.polar = (f64::NAN, 0.0, 1.0, 0.0);
            }

            let (ix, dh, width) = built(&s, &t);
            let mut candidates = Vec::with_capacity(TRACK_CAP);
            for query in 0..QUERY_CAP {
                let target = &t[query % (TRACK_CAP - 4)].polar;
                let plot = if query == 0 {
                    // Directly exercise the crafted close-range azimuth-seam track.
                    let crafted = t[TRACK_CAP - 4].polar;
                    (crafted.0, crafted.1)
                } else if query == 1 {
                    // Just inside the crafted track's exact radial coarse-window boundary.
                    let crafted = t[TRACK_CAP - 3].polar;
                    let limit = COARSE_M + 8.0 * crafted.3;
                    (crafted.0 + limit * (1.0 - 1.0e-12), crafted.1)
                } else if query == 2 {
                    // Just inside the same track's exact cross-range boundary.
                    let crafted = t[TRACK_CAP - 3].polar;
                    let limit = COARSE_M + 8.0 * crafted.3;
                    let da = (limit * (1.0 - 1.0e-12) / crafted.2).to_degrees();
                    (crafted.0, (crafted.1 + da).rem_euclid(360.0))
                } else if query == 3 {
                    // The crafted high-covariance track is on the always-tested wide list.
                    let crafted = t[TRACK_CAP - 2].polar;
                    (crafted.0, crafted.1)
                } else if query % 4 == 0 {
                    (target.0, target.1)
                } else if query % 4 == 1 {
                    (1.0 + max_range * rng.unit(), 360.0 * rng.unit())
                } else if query % 4 == 2 {
                    (
                        target.0 + COARSE_M,
                        (target.1 + 180.0 - 1.0e-7).rem_euclid(360.0),
                    )
                } else {
                    (0.0, 0.0)
                };
                coarse_query(&ix, &mut candidates, plot.0, plot.1, dh, width);
                assert!(candidates.windows(2).all(|pair| pair[0] < pair[1]));

                let mut reference = Vec::new();
                for (index, track) in t.iter().enumerate() {
                    if reference_passes(plot, track.polar) {
                        reference.push(index as u32);
                    }
                }
                for index in reference {
                    assert!(
                        candidates.binary_search(&index).is_ok(),
                        "world {world} query {query} omitted reference track {index}: plot {plot:?}"
                    );
                }
            }
        }
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

    /// The query as it was before the pieces were deduplicated while appending: every piece's
    /// cells appended, then sorted and deduplicated. `None` where the tracks are all taken.
    fn appended_then_deduplicated(
        ix: &CoarseIndex,
        rho: f64,
        az_deg: f64,
        dh_max: f64,
        width: f64,
    ) -> Option<Vec<u32>> {
        let frame = ix.frame?;
        let extra = EXTRA_FIXED_M + EXTRA_PER_RANGE * rho;
        let w = width + extra;
        let g_hi = rho + w;
        let slant_lo = (rho - width).max(0.0);
        let g_lo = ((slant_lo * slant_lo - dh_max * dh_max).max(0.0).sqrt() - extra).max(0.0);
        let a_lo = if g_lo > w { g_lo - w } else { -w };
        let (st, ct) = oa_math::sin_cos(az_deg.to_radians());
        let pieces = (((g_hi - a_lo) / SEGMENT_M).ceil() as usize).clamp(1, SEGMENTS_MAX);
        let step = (g_hi - a_lo) / pieces as f64;
        let mut out = Vec::new();
        for k in 0..pieces {
            let p_hi = if k + 1 == pieces {
                g_hi
            } else {
                a_lo + step * (k + 1) as f64
            };
            let b = piece_box(&frame, (st, ct), (a_lo + step * k as f64, p_hi), w)?;
            let cells: i64 = (b.c_lo..=b.c_hi)
                .map(|la| box_span(ix, &b, la).1 - box_span(ix, &b, la).0 + 1)
                .sum();
            if cells > ix.keys.len() as i64 + 1 {
                return None;
            }
            for la in b.c_lo..=b.c_hi {
                let (l_lo, l_hi, n) = box_span(ix, &b, la);
                let first = ix.first_cell.get(la as usize).copied().unwrap_or(0);
                for lo in l_lo..=l_hi {
                    push_cell(&ix.keys, &mut out, first + lo.rem_euclid(n) as u32);
                }
            }
        }
        push_cell(&ix.keys, &mut out, ALWAYS);
        out.sort_unstable();
        out.dedup();
        Some(out)
    }

    // REQ: OA-TRK-318
    #[test]
    fn tracks_on_a_piece_boundary_are_listed_once_and_the_output_never_outgrows_the_tracks() {
        // Plot 100 km north, 4.5 km wide: two pieces that meet at 96.25 km north.
        let s = site(0.0, 0.0);
        let g = radar_geom(&s);
        let (rho, width) = (100_000.0, 4_500.0);
        for n in [1usize, 50, 5_000] {
            let mut t = Vec::new();
            for i in 0..n {
                // On the boundary, spread a little across it.
                let r = 96_250.0 + (i % 7) as f64 * 0.5;
                let geo = polar_to_geodetic(&g, r, 0.0, g.height_m).unwrap();
                t.push(track_at(&s, geo.lat_deg, geo.lon_deg, 0.0));
            }
            let (ix, _, _) = built(&s, &t);
            let mut out = Vec::with_capacity(t.len());
            let before = (out.as_ptr() as usize, out.capacity());
            coarse_query(&ix, &mut out, rho, 0.0, 0.0, width);
            assert!(out.len() <= n, "{n} tracks, {} entries", out.len());
            assert_eq!(
                (out.as_ptr() as usize, out.capacity()),
                before,
                "{n} tracks: the output buffer was reallocated"
            );
            assert!(out.windows(2).all(|w| w[0] < w[1]));
            assert_eq!(out.len(), n, "every track is a candidate");
        }
    }

    // REQ: OA-TRK-318
    #[test]
    fn queries_over_piece_boundaries_equal_the_brute_force_and_stay_within_the_tracks() {
        let mut rng = Rng::from_seed(312);
        for (lat, lon) in [(0.0, 0.0), (45.0, -100.0), (80.0, 20.0), (0.0, 179.9)] {
            let s = site(lat, lon);
            let g = radar_geom(&s);
            for _ in 0..40 {
                let az = 360.0 * rng.unit();
                let rho = 20_000.0 + 400_000.0 * rng.unit();
                let width = 500.0 + 6_000.0 * rng.unit();
                let n = 1 + (rng.unit() * 400.0) as usize;
                // Same rectangle as the query: pieces of equal length from a_lo to g_hi.
                let extra = EXTRA_FIXED_M + EXTRA_PER_RANGE * rho;
                let w = width + extra;
                let g_hi = rho + w;
                let slant_lo = (rho - width).max(0.0);
                let g_lo = (slant_lo - extra).max(0.0);
                let a_lo = if g_lo > w { g_lo - w } else { -w };
                let pieces = (((g_hi - a_lo) / SEGMENT_M).ceil() as usize).clamp(1, SEGMENTS_MAX);
                let step = (g_hi - a_lo) / pieces as f64;
                let mut t = Vec::new();
                while t.len() < n {
                    // Along the line at a piece boundary (or anywhere in the window), across it.
                    let k = 1 + (rng.unit() * pieces as f64) as usize % pieces;
                    let along = (a_lo + step * k as f64 + 60.0 * (rng.unit() - 0.5)).max(1.0);
                    let cross = 2.0 * width * (rng.unit() - 0.5);
                    let r = oa_math::hypot(along, cross);
                    let a = az + cross.atan2(along).to_degrees();
                    let Ok(geo) = polar_to_geodetic(&g, r, a, g.height_m) else {
                        continue;
                    };
                    t.push(track_at(&s, geo.lat_deg, geo.lon_deg, 0.0));
                }
                let (ix, _, _) = built(&s, &t);
                let mut out = Vec::with_capacity(t.len());
                let cap = out.capacity();
                coarse_query(&ix, &mut out, rho, az, 0.0, width);
                assert!(out.len() <= t.len());
                assert_eq!(out.capacity(), cap, "reallocated at {lat} N");
                assert!(out.windows(2).all(|w| w[0] < w[1]));
                let plot = (rho, az);
                let passes = |i: u32| coarse_passes(width, plot, t[i as usize].polar);
                let brute: Vec<u32> = (0..t.len() as u32).filter(|&i| passes(i)).collect();
                let narrowed: Vec<u32> = out.iter().copied().filter(|&i| passes(i)).collect();
                assert_eq!(narrowed, brute, "{lat} N {az} deg {rho} m");
                if let Some(old) = appended_then_deduplicated(&ix, rho, az, 0.0, width) {
                    assert_eq!(out, old, "{lat} N {az} deg {rho} m");
                }
            }
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

    // REQ: OA-TRK-325
    #[test]
    fn budgeted_point_grid_returns_the_same_keys_in_column_order() {
        let points = [
            (1.99, -2.0),
            (0.0, 4.0),
            (-2.0, -3.0),
            (4.0, 0.0),
            (1.1, -2.3),
            (3.1, 3.1),
            (0.0, -4.1),
            (0.0, 0.0),
            (32_767.1, -32_769.0),
        ];
        let mut grid = point_grid_new(points.len(), 1.0);
        point_grid_build(&mut grid, points.len(), |i| points.get(i).copied());
        for (x, y) in [(0.0, 0.0), (32_767.5, -32_768.1)] {
            let mut expected = Vec::with_capacity(points.len());
            assert!(point_grid_query(&grid, &mut expected, x, y, 3.0));
            assert!(!expected.is_empty());
            let mut out = Vec::with_capacity(points.len());
            let (mut remaining, mut spent) = (49, 0);
            assert_eq!(
                point_grid_query_budgeted(&grid, &mut out, x, y, 3.0, &mut remaining, &mut spent),
                PointGridQueryResult::Complete(true)
            );
            assert_eq!(out, expected);
            assert!(spent < 49);
            assert_eq!(remaining + spent, 49);
        }
    }

    // REQ: OA-TRK-325
    #[test]
    fn point_grid_budget_stops_before_the_next_probe() {
        let points = [(0.0, 0.0), (1.0, 1.0)];
        let mut grid = point_grid_new(points.len(), 1.0);
        point_grid_build(&mut grid, points.len(), |i| points.get(i).copied());
        let mut out = Vec::with_capacity(points.len());
        let (mut remaining, mut spent) = (1usize, 0usize);
        assert_eq!(
            point_grid_query_budgeted(&grid, &mut out, 0.0, 0.0, 1.0, &mut remaining, &mut spent,),
            PointGridQueryResult::Capped
        );
        assert_eq!(remaining, 0);
        assert_eq!(spent, 1);
        assert!(out.is_empty());
    }

    // REQ: OA-TRK-325
    #[test]
    fn point_grid_budget_refuses_output_that_could_reallocate() {
        let points = [(0.0, 0.0)];
        let mut grid = point_grid_new(points.len(), 1.0);
        point_grid_build(&mut grid, points.len(), |i| points.get(i).copied());
        let mut out = Vec::new();
        let before = (out.as_ptr(), out.capacity());
        let (mut remaining, mut spent) = (10usize, 0usize);
        assert_eq!(
            point_grid_query_budgeted(&grid, &mut out, 0.0, 0.0, 0.0, &mut remaining, &mut spent,),
            PointGridQueryResult::OutputFull
        );
        assert_eq!((out.as_ptr(), out.capacity()), before);
        assert_eq!(remaining, 10);
        assert_eq!(spent, 0);
    }

    // REQ: OA-TRK-325
    #[test]
    fn budgeted_sort_orders_empty_single_duplicate_and_general_inputs() {
        for (mut values, expected) in [
            (Vec::<u64>::new(), vec![]),
            (vec![7], vec![7]),
            (vec![4, 1, 4, 2, 1, 0], vec![0, 1, 1, 2, 4, 4]),
        ] {
            let (mut remaining, mut spent) = (1_000usize, 0usize);
            assert!(sort_budgeted(&mut values, &mut remaining, &mut spent));
            assert_eq!(values, expected);
            assert_eq!(remaining + spent, 1_000);
        }
    }

    // REQ: OA-TRK-325
    #[test]
    fn budgeted_sort_caps_before_any_unfunded_operation() {
        let mut values = [3u64, 1, 2];
        let before = values;
        let (mut remaining, mut spent) = (0usize, 0usize);
        assert!(!sort_budgeted(&mut values, &mut remaining, &mut spent));
        assert_eq!((remaining, spent), (0, 0));
        assert_eq!(values, before);
    }

    // REQ: OA-TRK-325
    #[test]
    fn budgeted_sort_matches_standard_order_for_exhaustive_small_arrays() {
        for len in 0..=7usize {
            let cases = 3usize.pow(len as u32);
            for mut encoded in 0..cases {
                let mut values = Vec::with_capacity(len);
                for _ in 0..len {
                    values.push((encoded % 3) as u64);
                    encoded /= 3;
                }
                let mut expected = values.clone();
                expected.sort_unstable();
                let (mut remaining, mut spent) = (1_000usize, 0usize);
                assert!(sort_budgeted(&mut values, &mut remaining, &mut spent));
                assert_eq!(values, expected);
                assert_eq!(remaining + spent, 1_000);
            }
        }
    }

    // REQ: OA-TRK-325
    #[test]
    fn budgeted_sort_caps_at_each_work_boundary_without_reallocation() {
        for source in [
            vec![1u64, 0],
            vec![4, 3, 2, 1],
            vec![2, 1, 2, 0, 1, 0, 3, 2],
        ] {
            let mut fully_sorted = source.clone();
            let (mut remaining, mut full_work) = (10_000usize, 0usize);
            assert!(sort_budgeted(
                &mut fully_sorted,
                &mut remaining,
                &mut full_work
            ));
            for allowance in 0..=full_work {
                let mut values = source.clone();
                let before = (values.as_ptr(), values.capacity());
                let (mut remaining, mut spent) = (allowance, 0usize);
                let complete = sort_budgeted(&mut values, &mut remaining, &mut spent);
                assert_eq!((values.as_ptr(), values.capacity()), before);
                assert_eq!(remaining + spent, allowance);
                if complete {
                    assert_eq!(allowance, full_work);
                    assert_eq!(values, fully_sorted);
                } else {
                    assert_eq!(spent, allowance);
                }
            }
        }
    }

    // REQ: OA-TRK-325
    #[test]
    fn budgeted_point_grid_stays_in_its_buffer_and_clears_on_failure() {
        let mut grid = point_grid_new(4, 1.0);
        let before = point_grid_buffer(&grid);
        let (mut remaining, mut spent) = (10_000usize, 0usize);
        assert_eq!(
            point_grid_build_budgeted(
                &mut grid,
                4,
                |i| Some((3.0 - i as f64, 0.0)),
                &mut remaining,
                &mut spent,
            ),
            PointGridBuildResult::Complete
        );
        assert_eq!(point_grid_buffer(&grid), before);
        assert_eq!(grid.keys.len(), 4);

        let (mut remaining, mut spent) = (1usize, 0usize);
        assert_eq!(
            point_grid_build_budgeted(
                &mut grid,
                4,
                |i| Some((i as f64, 0.0)),
                &mut remaining,
                &mut spent,
            ),
            PointGridBuildResult::Capped
        );
        assert!(grid.keys.is_empty());
        assert_eq!(point_grid_buffer(&grid), before);
    }

    // REQ: OA-TRK-325
    #[test]
    fn budgeted_point_grid_clears_at_every_cap_and_matches_legacy_output() {
        let points = [
            Some((3.0, 2.0)),
            None,
            Some((1.0, 1.0)),
            Some((f64::NAN, 4.0)),
            Some((3.0, 2.0)),
            Some((-2.0, 7.0)),
        ];
        let mut legacy = point_grid_new(points.len(), 1.0);
        point_grid_build(&mut legacy, points.len(), |i| {
            points.get(i).copied().flatten()
        });
        let mut reference = point_grid_new(points.len(), 1.0);
        let (mut remaining, mut full_work) = (10_000usize, 0usize);
        assert_eq!(
            point_grid_build_budgeted(
                &mut reference,
                points.len(),
                |i| points.get(i).copied().flatten(),
                &mut remaining,
                &mut full_work,
            ),
            PointGridBuildResult::Complete
        );
        assert_eq!(reference.keys, legacy.keys);

        for allowance in 0..=full_work {
            let mut grid = point_grid_new(points.len(), 1.0);
            let before = point_grid_buffer(&grid);
            let (mut remaining, mut spent) = (allowance, 0usize);
            let result = point_grid_build_budgeted(
                &mut grid,
                points.len(),
                |i| points.get(i).copied().flatten(),
                &mut remaining,
                &mut spent,
            );
            assert_eq!(point_grid_buffer(&grid), before);
            assert_eq!(remaining + spent, allowance);
            if result == PointGridBuildResult::Complete {
                assert_eq!(allowance, full_work);
                assert_eq!(grid.keys, legacy.keys);
            } else {
                assert_eq!(result, PointGridBuildResult::Capped);
                assert_eq!(spent, allowance);
                assert!(grid.keys.is_empty());
            }
        }
    }

    // REQ: OA-TRK-325
    #[test]
    fn budgeted_point_grid_refuses_declared_over_capacity_before_visiting() {
        let mut grid = point_grid_new(1, 1.0);
        let visits = std::cell::Cell::new(0usize);
        let (mut remaining, mut spent) = (100usize, 0usize);
        assert_eq!(
            point_grid_build_budgeted(
                &mut grid,
                2,
                |_| {
                    visits.set(visits.get() + 1);
                    Some((0.0, 0.0))
                },
                &mut remaining,
                &mut spent,
            ),
            PointGridBuildResult::CapacityFull
        );
        assert_eq!(visits.get(), 0);
        assert_eq!((remaining, spent), (100, 0));
        assert!(grid.keys.is_empty());
    }

    // REQ: OA-TRK-325
    #[test]
    fn large_budgeted_point_grid_matches_offline_order_and_refuses_short_radix_budget() {
        let n = 4_096usize;
        let place = |i: usize| {
            let x = ((i * 37 % 31) as i32 - 15) as f64 * 1_000.0;
            let y = ((i * 83 % 11) as i32 - 5) as f64 * 1_000.0;
            Some((x, y))
        };
        let mut expected = point_grid_new(n, 1_000.0);
        point_grid_build(&mut expected, n, place);
        let mut grid = point_grid_new(n + 1_024, 1_000.0);
        let keys_before = point_grid_buffer(&grid);
        let scratch_before = point_grid_radix_buffer(&grid);
        let radix_work = POINT_RADIX_PASSES * (2 * n + 2 * POINT_RADIX_BUCKETS);
        let (mut remaining, mut spent) = (n + radix_work, 0usize);
        assert_eq!(
            point_grid_build_budgeted(&mut grid, n, place, &mut remaining, &mut spent,),
            PointGridBuildResult::Complete
        );
        assert_eq!(grid.keys, expected.keys);
        assert_eq!((remaining, spent), (0, n + radix_work));
        assert_eq!(point_grid_buffer(&grid), keys_before);
        assert_eq!(point_grid_radix_buffer(&grid), scratch_before);

        let (mut remaining, mut spent) = (n + radix_work - 1, 0usize);
        assert_eq!(
            point_grid_build_budgeted(&mut grid, n, place, &mut remaining, &mut spent,),
            PointGridBuildResult::Capped
        );
        assert!(grid.keys.is_empty());
        assert_eq!((remaining, spent), (radix_work - 1, n));
        assert_eq!(point_grid_buffer(&grid), keys_before);
        assert_eq!(point_grid_radix_buffer(&grid), scratch_before);
    }

    // REQ: OA-TRK-325
    #[test]
    fn bitset_sort_unique_orders_boundaries_and_deduplicates_with_finite_budget() {
        let mut indices = [16_383u32, 64, 63, 0, 64, 16_383];
        let mut scratch = [u64::MAX; 256];
        let index_address = indices.as_ptr();
        let scratch_address = scratch.as_ptr();
        let allowance = 2 * 256 + 2 * indices.len();
        let (mut remaining, mut spent) = (allowance, 0usize);
        assert_eq!(
            sort_unique_indices_budgeted(
                &mut indices,
                &mut scratch,
                16_384,
                &mut remaining,
                &mut spent,
            ),
            SortUniqueResult::Complete(4)
        );
        assert_eq!(&indices[..4], &[0, 63, 64, 16_383]);
        assert_eq!((remaining, spent), (0, allowance));
        assert_eq!(indices.as_ptr(), index_address);
        assert_eq!(scratch.as_ptr(), scratch_address);
        assert!(scratch.iter().all(|&word| word == 0));
    }

    // REQ: OA-TRK-325
    #[test]
    fn bitset_sort_unique_budget_precheck_preserves_both_buffers() {
        let mut indices = [64u32, 1, 64];
        let before_indices = indices;
        let mut scratch = [0x55u64; 2];
        let before_scratch = scratch;
        let (mut remaining, mut spent) = (0usize, 0usize);
        assert_eq!(
            sort_unique_indices_budgeted(
                &mut indices,
                &mut scratch,
                128,
                &mut remaining,
                &mut spent,
            ),
            SortUniqueResult::Capped
        );
        assert_eq!(indices, before_indices);
        assert_eq!(scratch, before_scratch);
        assert_eq!((remaining, spent), (0, 0));
    }

    // REQ: OA-TRK-325
    #[test]
    fn bitset_sort_unique_refuses_invalid_indexes_and_short_scratch() {
        let mut indices = [3u32, 64, 1];
        let before = indices;
        let mut scratch = [u64::MAX];
        let (mut remaining, mut spent) = (100usize, 0usize);
        assert_eq!(
            sort_unique_indices_budgeted(
                &mut indices,
                &mut scratch,
                64,
                &mut remaining,
                &mut spent,
            ),
            SortUniqueResult::InvalidIndex
        );
        assert_eq!(indices, before);
        assert_eq!(scratch[0], 0);
        assert_eq!(remaining + spent, 100);

        let mut no_scratch = [];
        let (mut remaining, mut spent) = (100usize, 0usize);
        assert_eq!(
            sort_unique_indices_budgeted(
                &mut indices,
                &mut no_scratch,
                65,
                &mut remaining,
                &mut spent,
            ),
            SortUniqueResult::ScratchTooSmall
        );
        assert_eq!(indices, before);
        assert_eq!((remaining, spent), (100, 0));
    }

    // REQ: OA-TRK-301
    #[test]
    fn the_point_grid_returns_every_point_within_the_radius() {
        let mut rng = Rng::from_seed(77);
        let pts: Vec<(f64, f64)> = (0..400)
            .map(|_| {
                (
                    600_000.0 * (rng.unit() - 0.5),
                    600_000.0 * (rng.unit() - 0.5),
                )
            })
            .collect();
        let mut g = point_grid_new(pts.len(), 12_000.0);
        point_grid_build(&mut g, pts.len(), |i| pts.get(i).copied());
        let before = point_grid_buffer(&g);
        let mut out = Vec::with_capacity(pts.len());
        for _ in 0..300 {
            let (x, y) = (
                600_000.0 * (rng.unit() - 0.5),
                600_000.0 * (rng.unit() - 0.5),
            );
            let r = 40_000.0 * rng.unit();
            out.clear();
            if !point_grid_query(&g, &mut out, x, y, r) {
                assert!(r > POINT_RINGS_MAX as f64 * 12_000.0);
                continue;
            }
            for (i, p) in pts.iter().enumerate() {
                if oa_math::hypot(p.0 - x, p.1 - y) <= r {
                    assert!(out.contains(&(i as u32)), "point {i} at {r}");
                }
            }
        }
        // A point somewhere else entirely is found by a query at its own place; a point with no
        // place is left out; nothing was reallocated.
        let mut h = point_grid_new(4, 12_000.0);
        point_grid_build(&mut h, 3, |i| {
            [Some((1.0e9, -1.0e9)), None, Some((5.0, 5.0))][i]
        });
        out.clear();
        assert!(point_grid_query(&h, &mut out, 1.0e9, -1.0e9, 100.0));
        assert!(out.contains(&0));
        out.clear();
        assert!(!point_grid_query(&h, &mut out, 0.0, 0.0, f64::NAN));
        assert!(!point_grid_query(&h, &mut out, 0.0, 0.0, 1.0e6));
        assert_eq!(point_grid_buffer(&g), before);
    }

    // REQ: OA-TRK-306
    #[test]
    fn the_fine_grid_finds_every_track_of_the_window_at_the_poles_and_the_seam() {
        // Dense tracks of standard uncertainty (a standard deviation of 100 m or less, so the
        // windows are the fusion layer's usual width) round radars on the +-180 degree seam, at
        // the equator, and beside and over each pole; plots near the tracks and at random. The
        // grid's candidates hold every track the window passes, and are a small part of the whole
        // wherever no pole is in the window.
        let mut rng = Rng::from_seed(91);
        let (mut visited, mut total) = (0usize, 0usize);
        for (lat, lon, reach_m) in [
            (0.0, 179.999, 90_000.0),
            (-33.0, -179.99, 90_000.0),
            (41.98, -87.91, 90_000.0),
            (89.95, 30.0, 40_000.0),
            (-89.9, -120.0, 40_000.0),
            (89.5, 179.9, 120_000.0),
            (-89.99, 0.0, 20_000.0),
            (88.0, -179.9, 250_000.0),
        ] {
            let s = site(lat, lon);
            let mut t = tracks(&s, 4_000, reach_m, &mut rng);
            for x in &mut t {
                x.polar.3 = 100.0 * rng.unit();
            }
            let (ix, dh, width) = built(&s, &t);
            let mut cand = Vec::with_capacity(t.len());
            let mut found = 0usize;
            for k in 0..2_000usize {
                let x = &t[k % t.len()].polar;
                let (prho, paz) = if k % 4 == 0 {
                    (
                        1_000.0 + (reach_m - 1_000.0) * rng.unit(),
                        360.0 * rng.unit(),
                    )
                } else {
                    (
                        x.0 + 3_000.0 * (rng.unit() - 0.5),
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
                if lat.abs() <= 85.0 {
                    visited += cand.len();
                    total += t.len();
                }
            }
            assert!(found > 0, "site {lat} {lon}: the test found nothing");
        }
        // Away from the poles the grid is a narrowing to a few per cent of the tracks.
        assert!(visited * 10 < total, "{visited} of {total}");
    }

    // REQ: OA-TRK-306
    #[test]
    fn a_cell_is_about_the_same_size_everywhere_and_the_numbers_of_cells_are_distinct() {
        let ix = coarse_new(1);
        assert_eq!(ix.lon_cells.len(), N_LAT as usize);
        // Cell widths on the ground, at the band's edge nearest a pole, at least the height.
        for la in [0i64, 700, 1_800, 2_400, 3_000, 3_599] {
            let edge = (-90.0 + CELL_DEG * la as f64)
                .abs()
                .max((-90.0 + CELL_DEG * (la + 1) as f64).abs());
            let n = band_cells(&ix, la);
            let width_m = 360.0 / n as f64 * M_PER_DEG * oa_math::cos(edge.to_radians());
            let height_m = CELL_DEG * M_PER_DEG;
            assert!(
                n == 1 || width_m >= height_m * 0.999,
                "band {la}: {width_m} m"
            );
            assert!(n == 1 || width_m < 2.0 * height_m, "band {la}: {width_m} m");
        }
        // Cell numbers run on without a gap or a repeat, below the cell of the unplaced tracks.
        let mut next = 0u32;
        for la in 0..N_LAT {
            assert_eq!(ix.first_cell[la as usize], next);
            next += ix.lon_cells[la as usize];
        }
        assert!(next < ALWAYS);
        // Both sides of the seam are cells of the same band, the first and the last.
        let (west, east) = (cell_of(&ix, 10.0, -179.999), cell_of(&ix, 10.0, 179.999));
        let la = lat_cell(10.0) as usize;
        assert_eq!(west, ix.first_cell[la]);
        assert_eq!(east, ix.first_cell[la] + ix.lon_cells[la] - 1);
    }

    // REQ: OA-TRK-264
    #[test]
    fn the_candidate_order_does_not_depend_on_the_sign_of_a_nan_in_a_distance() {
        // the order the fusion keeps candidates in when a plot has more than it can visit: by
        // `coarse_distance`, then by track number. A NaN term gives 0 whatever its sign, and the
        // canonical order puts every NaN in one place, so both signs choose the same tracks.
        let nans = [
            f64::from_bits(0x7ff8_0000_0000_0000),
            f64::from_bits(0xfff8_0000_0000_0000),
            f64::from_bits(0xfff8_0000_0000_4321),
        ];
        let order = |nan: f64| {
            let tracks = [
                (3000.0, 1.0, 5.0e4, 10.0),
                (nan, 1.0, 5.0e4, 10.0),
                (1000.0, nan, 5.0e4, 10.0),
                (2000.0, 1.0, nan, 10.0),
                (1500.0, 1.0, 5.0e4, 10.0),
                (nan, nan, nan, nan),
            ];
            let key = |ti: u32| coarse_distance((1200.0, 1.0), tracks[ti as usize]);
            let mut cand: Vec<u32> = (0..tracks.len() as u32).rev().collect();
            cand.sort_unstable_by(|&x, &y| oa_math::cmp_canon_f64(key(x), key(y)).then(x.cmp(&y)));
            let bits: Vec<u64> = cand.iter().map(|&c| key(c).to_bits()).collect();
            (cand, bits)
        };
        let want = order(nans[0]);
        for nan in nans {
            assert_eq!(order(nan), want);
        }
        // the four tracks with a NaN term sort first (distance 0), by track number
        assert_eq!(&want.0[..4], &[1, 2, 3, 5], "{want:?}");
        assert!(want.1[..4].iter().all(|&b| b == 0), "{want:?}");
    }
}
