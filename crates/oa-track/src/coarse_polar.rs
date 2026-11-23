//! Bounded index for the radar-polar coarse association predicate. Core (pure).
//!
//! A regular posting is ordered by the upper-bounded uncertainty band, slant-range cell, then
//! track index. Queries seek the radial interval for each occupied band and retain postings whose
//! wrapped cross-range is within that band's upper edge. Every other track has one ALWAYS
//! posting. The posting ranges only narrow the work; `coarse_passes` remains the deciding
//! predicate while each posting is visited.

use crate::coarse::{sort_budgeted, sort_values_budgeted};

const RANGE_CELL_M: f64 = 500.0;
const RANGE_MAX_M: f64 = 16_000_000.0;
const GROUND_MAX_M: f64 = 33_554_432.0;
const AZ_ABS_MAX_DEG: f64 = 720.0;
const WIDTH_CELL_M: f64 = 512.0;
const WIDTH_BANDS: u8 = 9;
const ALWAYS_BAND: u8 = u8::MAX;
const RADIAL_CELLS_MAX: u32 = 32_000;
const RADIX_BUCKETS: usize = 256;
const RADIX_PASSES: usize = 2;

/// Fixed-capacity polar candidate index, one per radar. Capacity is the configured track limit.
// A7.2: cold subsystem.
#[derive(Debug, Default)]
pub(crate) struct PolarIndex {
    /// Packed `(band: 8, range cell: 24, track index: 32)` keys, sorted at build.
    keys: Vec<u64>,
    /// Fixed scratch for allocation-free ordering of candidate track indexes.
    sort_scratch: Vec<u32>,
    /// Half-open offsets of each regular width band in `keys`; the last is the ALWAYS start.
    band_starts: [usize; WIDTH_BANDS as usize + 1],
    max_tracks: usize,
}

/// Allocates all posting storage once for at most `capacity` tracks.
pub(crate) fn polar_new(capacity: usize) -> PolarIndex {
    PolarIndex {
        keys: Vec::with_capacity(capacity),
        sort_scratch: vec![0; capacity],
        band_starts: [0; WIDTH_BANDS as usize + 1],
        max_tracks: capacity,
    }
}

/// Addresses and capacities of the posting and ordering buffers, for the allocation check.
pub(crate) fn polar_buffers(ix: &PolarIndex) -> [(usize, usize); 2] {
    [
        (ix.keys.as_ptr() as usize, ix.keys.capacity()),
        (
            ix.sort_scratch.as_ptr() as usize,
            ix.sort_scratch.capacity(),
        ),
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PolarBuildResult {
    Complete { work: usize, track_visits: usize },
    Capped { work: usize, track_visits: usize },
}

/// Builds one posting for each included track. `skip` excludes hard-taken and wide-list tracks.
/// Invalid or unsupported terms use ALWAYS so IEEE NaN comparison behavior cannot hide a track.
/// Build work charges each input visit and every sort operation before it occurs.
pub(crate) fn polar_build(
    ix: &mut PolarIndex,
    n: usize,
    mut terms: impl FnMut(usize) -> Option<(f64, f64, f64, f64)>,
    mut skip: impl FnMut(usize) -> bool,
    remaining: &mut usize,
    spent: &mut usize,
) -> PolarBuildResult {
    ix.keys.clear();
    ix.band_starts.fill(0);
    if n > ix.max_tracks || n > ix.keys.capacity() || n > ix.sort_scratch.len() {
        return PolarBuildResult::Capped {
            work: 0,
            track_visits: 0,
        };
    }
    let mut track_visits = 0usize;
    let mut band_counts = [0usize; WIDTH_BANDS as usize];
    for i in 0..n {
        if !charge(remaining, spent) {
            ix.keys.clear();
            return PolarBuildResult::Capped {
                work: *spent,
                track_visits,
            };
        }
        track_visits = track_visits.saturating_add(1);
        if skip(i) {
            continue;
        }
        let packed = match terms(i) {
            Some((rho, az, ground, sigma)) => regular_key(rho, az, ground, sigma, i as u32),
            None => None,
        }
        .unwrap_or_else(|| key(ALWAYS_BAND, 0, i as u32));
        if let Some(count) = band_counts.get_mut((packed >> 56) as usize) {
            let Some(next) = count.checked_add(1) else {
                ix.keys.clear();
                ix.band_starts.fill(0);
                return PolarBuildResult::Capped {
                    work: *spent,
                    track_visits,
                };
            };
            *count = next;
        }
        ix.keys.push(packed);
    }
    if !sort_budgeted(&mut ix.keys, remaining, spent) {
        ix.keys.clear();
        ix.band_starts.fill(0);
        return PolarBuildResult::Capped {
            work: *spent,
            track_visits,
        };
    }
    let mut start = 0usize;
    for (band, count) in band_counts.into_iter().enumerate() {
        let Some(next) = start.checked_add(count) else {
            ix.keys.clear();
            ix.band_starts.fill(0);
            return PolarBuildResult::Capped {
                work: *spent,
                track_visits,
            };
        };
        if let Some(slot) = ix.band_starts.get_mut(band) {
            *slot = start;
        }
        start = next;
    }
    if let Some(slot) = ix.band_starts.last_mut() {
        *slot = start;
    }
    PolarBuildResult::Complete {
        work: *spent,
        track_visits,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PolarQueryResult {
    Complete { work: usize },
    Capped { work: usize },
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PolarQuerySpec {
    pub plot: (f64, f64),
    pub search_width: f64,
    pub exact_window: f64,
}

/// Returns the sorted tracks accepted by `coarse_passes` for a plot, after a conservative posting
/// lookup at `search_width`. Unsupported query values scan every posting. A cap or undersized
/// output clears `out`; callers must mark the graph incomplete and defer all anonymous plots in
/// the radar-bucket.
pub(crate) fn polar_query(
    ix: &mut PolarIndex,
    out: &mut Vec<u32>,
    spec: PolarQuerySpec,
    mut terms: impl FnMut(u32) -> Option<(f64, f64, f64, f64)>,
    remaining: &mut usize,
    spent: &mut usize,
) -> PolarQueryResult {
    let (plot_rho, plot_az) = spec.plot;
    let (search_width, exact_window) = (spec.search_width, spec.exact_window);
    out.clear();
    if !(plot_rho.is_finite()
        && plot_az.is_finite()
        && plot_az.abs() <= AZ_ABS_MAX_DEG
        && search_width.is_finite()
        && (0.0..=WIDTH_BANDS as f64 * WIDTH_CELL_M).contains(&search_width)
        && exact_window.is_finite()
        && exact_window >= 0.0)
    {
        return all(
            ix,
            out,
            (plot_rho, plot_az),
            exact_window,
            &mut terms,
            remaining,
            spent,
        );
    }

    for band in 0..WIDTH_BANDS {
        let width_hi = (f64::from(band) + 1.0) * WIDTH_CELL_M;
        if f64::from(band) * WIDTH_CELL_M > search_width {
            continue;
        }
        let lo = (plot_rho - width_hi).max(0.0);
        let hi = (plot_rho + width_hi).min(RANGE_MAX_M);
        if hi < 0.0 || lo > RANGE_MAX_M {
            continue;
        }
        // The extra full cell on either side covers endpoint rounding, including equality at a
        // cell boundary. All regular rho values are in [0, RANGE_MAX_M].
        let first = ((lo / RANGE_CELL_M) as u32)
            .saturating_sub(1)
            .min(RADIAL_CELLS_MAX);
        let last = ((hi / RANGE_CELL_M) as u32)
            .saturating_add(1)
            .min(RADIAL_CELLS_MAX);
        let band_index = band as usize;
        let Some((&band_start, &band_end)) = ix
            .band_starts
            .get(band_index)
            .zip(ix.band_starts.get(band_index.saturating_add(1)))
        else {
            return capped(out, *spent);
        };
        if band_start == band_end {
            continue;
        }
        let Some(band_keys) = ix.keys.get(band_start..band_end) else {
            return capped(out, *spent);
        };
        let start = match lower_bound(band_keys, band, first, false, remaining, spent) {
            Some(i) => match band_start.checked_add(i) {
                Some(value) => value,
                None => return capped(out, *spent),
            },
            None => return capped(out, *spent),
        };
        let end = match lower_bound(band_keys, band, last, true, remaining, spent) {
            Some(i) => match band_start.checked_add(i) {
                Some(value) => value,
                None => return capped(out, *spent),
            },
            None => return capped(out, *spent),
        };
        for &packed in ix.keys.get(start..end).unwrap_or(&[]) {
            if !charge(remaining, spent) {
                return capped(out, *spent);
            }
            let track = packed as u32;
            let Some(track_terms) = terms(track) else {
                return capped(out, *spent);
            };
            if crate::coarse::coarse_passes(exact_window, (plot_rho, plot_az), track_terms)
                && !push_within_capacity(out, track)
            {
                return capped(out, *spent);
            }
        }
    }
    let always = ix.band_starts.last().copied().unwrap_or(ix.keys.len());
    for &packed in ix.keys.get(always..).unwrap_or(&[]) {
        if !charge(remaining, spent) {
            return capped(out, *spent);
        }
        if (packed >> 56) as u8 != ALWAYS_BAND {
            break;
        }
        let track = packed as u32;
        let Some(track_terms) = terms(track) else {
            return capped(out, *spent);
        };
        if crate::coarse::coarse_passes(exact_window, (plot_rho, plot_az), track_terms)
            && !push_within_capacity(out, track)
        {
            return capped(out, *spent);
        }
    }
    order(ix, out, remaining, spent)
}

fn regular_key(rho: f64, az: f64, ground: f64, sigma: f64, track: u32) -> Option<u64> {
    let width = 3_000.0 + 8.0 * sigma;
    if !(rho.is_finite()
        && (0.0..=RANGE_MAX_M).contains(&rho)
        && az.is_finite()
        && az.abs() <= AZ_ABS_MAX_DEG
        && ground.is_finite()
        && (1.0..GROUND_MAX_M).contains(&ground)
        && sigma.is_finite()
        && width.is_finite()
        && (1.0..=WIDTH_BANDS as f64 * WIDTH_CELL_M).contains(&width))
    {
        return None;
    }
    let band = (width / WIDTH_CELL_M) as u8;
    if band >= WIDTH_BANDS {
        return None;
    }
    let cell = (rho / RANGE_CELL_M) as u32;
    Some(key(band, cell, track))
}

fn key(band: u8, cell: u32, track: u32) -> u64 {
    (u64::from(band) << 56) | (u64::from(cell) << 32) | u64::from(track)
}

fn lower_bound(
    keys: &[u64],
    band: u8,
    cell: u32,
    after: bool,
    remaining: &mut usize,
    spent: &mut usize,
) -> Option<usize> {
    let target = key(band, cell, if after { u32::MAX } else { 0 });
    let (mut lo, mut hi) = (0usize, keys.len());
    while lo < hi {
        if !charge(remaining, spent) {
            return None;
        }
        let mid = lo + (hi - lo) / 2;
        let value = *keys.get(mid)?;
        if if after {
            value <= target
        } else {
            value < target
        } {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    Some(lo)
}

fn all(
    ix: &mut PolarIndex,
    out: &mut Vec<u32>,
    plot: (f64, f64),
    exact_window: f64,
    terms: &mut impl FnMut(u32) -> Option<(f64, f64, f64, f64)>,
    remaining: &mut usize,
    spent: &mut usize,
) -> PolarQueryResult {
    for &packed in &ix.keys {
        if !charge(remaining, spent) {
            return capped(out, *spent);
        }
        let track = packed as u32;
        let Some(track_terms) = terms(track) else {
            return capped(out, *spent);
        };
        if crate::coarse::coarse_passes(exact_window, plot, track_terms)
            && !push_within_capacity(out, track)
        {
            return capped(out, *spent);
        }
    }
    order(ix, out, remaining, spent)
}

fn order(
    ix: &mut PolarIndex,
    out: &mut Vec<u32>,
    remaining: &mut usize,
    spent: &mut usize,
) -> PolarQueryResult {
    let len = out.len();
    if len < 2 {
        return PolarQueryResult::Complete { work: *spent };
    }
    let levels = len.ilog2() as usize;
    let heap_bound = len.saturating_mul(5usize.saturating_add(6usize.saturating_mul(levels)));
    let radix_work = len
        .checked_mul(2 * RADIX_PASSES)
        .and_then(|n| n.checked_add(2 * RADIX_BUCKETS * RADIX_PASSES))
        .unwrap_or(usize::MAX);
    let ordered = if radix_work < heap_bound {
        radix_order_budgeted(ix, out, radix_work, remaining, spent)
    } else {
        sort_values_budgeted(out, remaining, spent)
    };
    if !ordered {
        return capped(out, *spent);
    }
    PolarQueryResult::Complete { work: *spent }
}

fn radix_order_budgeted(
    ix: &mut PolarIndex,
    out: &mut [u32],
    required: usize,
    remaining: &mut usize,
    spent: &mut usize,
) -> bool {
    let Some(scratch) = ix.sort_scratch.get_mut(..out.len()) else {
        return false;
    };
    if !charge_many(remaining, spent, required) {
        return false;
    }
    radix_pass(out, scratch, 0) && radix_pass(scratch, out, 8)
}

fn radix_pass(source: &[u32], target: &mut [u32], shift: usize) -> bool {
    let bucket = |value: u32| ((value >> shift) & 0xff) as usize;
    let mut positions = [0usize; RADIX_BUCKETS];
    for &value in source {
        let Some(count) = positions.get_mut(bucket(value)) else {
            return false;
        };
        let Some(next) = count.checked_add(1) else {
            return false;
        };
        *count = next;
    }
    let mut offset = 0usize;
    for position in &mut positions {
        let Some(next) = offset.checked_add(*position) else {
            return false;
        };
        *position = offset;
        offset = next;
    }
    for &value in source {
        let Some(position) = positions.get_mut(bucket(value)) else {
            return false;
        };
        let Some(slot) = target.get_mut(*position) else {
            return false;
        };
        *slot = value;
        let Some(next) = position.checked_add(1) else {
            return false;
        };
        *position = next;
    }
    true
}

fn capped(out: &mut Vec<u32>, work: usize) -> PolarQueryResult {
    out.clear();
    PolarQueryResult::Capped { work }
}

fn push_within_capacity(out: &mut Vec<u32>, value: u32) -> bool {
    if out.len() >= out.capacity() {
        return false;
    }
    out.push(value);
    true
}

fn charge(remaining: &mut usize, spent: &mut usize) -> bool {
    if *remaining == 0 {
        return false;
    }
    *remaining -= 1;
    *spent = spent.saturating_add(1);
    true
}

fn charge_many(remaining: &mut usize, spent: &mut usize, amount: usize) -> bool {
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

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use crate::coarse::coarse_passes;

    const BUILD_WORK: usize = 4_000_000;
    const QUERY_WORK: usize = 300_000;

    fn build(rows: &[(f64, f64, f64, f64)]) -> PolarIndex {
        let mut index = polar_new(rows.len());
        let mut remaining = BUILD_WORK;
        let mut spent = 0;
        let result = polar_build(
            &mut index,
            rows.len(),
            |i| rows.get(i).copied(),
            |_| false,
            &mut remaining,
            &mut spent,
        );
        assert!(matches!(
            result,
            PolarBuildResult::Complete { work, .. } if work <= BUILD_WORK
        ));
        index
    }

    fn query(
        index: &mut PolarIndex,
        rows: &[(f64, f64, f64, f64)],
        plot: (f64, f64),
        budget: usize,
    ) -> (PolarQueryResult, Vec<u32>) {
        let mut out = Vec::with_capacity(rows.len());
        let mut remaining = budget;
        let mut spent = 0;
        let result = polar_query(
            index,
            &mut out,
            PolarQuerySpec {
                plot,
                search_width: 4_500.0,
                exact_window: 3_000.0,
            },
            |i| rows.get(i as usize).copied(),
            &mut remaining,
            &mut spent,
        );
        (result, out)
    }

    fn next_up(x: f64) -> f64 {
        if x == 0.0 {
            return f64::from_bits(1);
        }
        let bits = x.to_bits();
        f64::from_bits(if x > 0.0 { bits + 1 } else { bits - 1 })
    }

    #[test]
    fn exact_predicate_superset_covers_width_range_and_azimuth_boundaries() {
        let plot_rho = 100_000.0;
        let ground = 100_000.0;
        let mut rows = Vec::new();
        for band in 5..=8 {
            let edge = (f64::from(band) + 1.0) * WIDTH_CELL_M;
            for width in [next_up(f64::from(band) * WIDTH_CELL_M), edge, next_up(edge)] {
                let sigma = (width - 3_000.0) / 8.0;
                rows.push((plot_rho + width, 359.999, ground, sigma));
                rows.push((plot_rho - width, 0.001, ground, sigma));
                rows.push((plot_rho, 360.0 + width.to_degrees() / ground, ground, sigma));
            }
        }
        let mut index = build(&rows);
        let (result, candidates) = query(&mut index, &rows, (plot_rho, 0.0), QUERY_WORK);
        assert!(matches!(result, PolarQueryResult::Complete { work } if work <= QUERY_WORK));
        assert!(candidates.windows(2).all(|w| w[0] < w[1]));
        for (i, track) in rows.iter().enumerate() {
            if coarse_passes(3_000.0, (plot_rho, 0.0), *track) {
                assert!(candidates.binary_search(&(i as u32)).is_ok(), "missed {i}");
            }
        }
    }

    #[test]
    fn exceptional_terms_are_always_included_and_invalid_plot_uses_full_scan() {
        let rows = [
            (5_000.0, 1.0, 0.0, 0.0),
            (5_000.0, f64::NAN, 100.0, 0.0),
            (f64::INFINITY, 1.0, 100.0, 0.0),
            (5_000.0, 1.0, f64::INFINITY, 0.0),
            (5_000.0, 1.0, 100.0, f64::NAN),
            (5_000.0, 1.0e6, 100.0, 0.0),
        ];
        let mut index = build(&rows);
        let (result, candidates) = query(&mut index, &rows, (5_000.0, 0.0), QUERY_WORK);
        assert!(matches!(result, PolarQueryResult::Complete { work } if work <= QUERY_WORK));
        for (i, track) in rows.iter().enumerate() {
            if coarse_passes(3_000.0, (5_000.0, 0.0), *track) {
                assert!(candidates.binary_search(&(i as u32)).is_ok(), "missed {i}");
            }
        }
        let (result, candidates) = query(&mut index, &rows, (5_000.0, f64::INFINITY), QUERY_WORK);
        assert!(matches!(result, PolarQueryResult::Complete { work } if work <= QUERY_WORK));
        for (i, track) in rows.iter().enumerate() {
            assert_eq!(
                candidates.binary_search(&(i as u32)).is_ok(),
                coarse_passes(3_000.0, (5_000.0, f64::INFINITY), *track),
                "track {i}"
            );
        }
    }

    // REQ: OA-TRK-220, OA-TRK-221, OA-TRK-306
    #[test]
    fn real_radar_geometry_at_poles_seam_and_nadir_matches_every_coarse_edge() {
        use crate::radar::{
            plot_convert, polar_of, radar_geom, RadarPlot, RadarSite, NONE_ALT, NONE_CODE, NO_BIAS,
        };
        use crate::types::NO_ADDR;
        for (lat, lon) in [(0.0, 179.999), (85.0, 179.999), (90.0, 0.0), (-90.0, 30.0)] {
            let site = RadarSite {
                lat_deg: lat,
                lon_deg: lon,
                height_m: 0.0,
                sigma_range_m: 70.0,
                sigma_az_deg: 0.08,
                range_quantum_m: 0.0,
                alt_quantum_ft: 100,
                period_ns: 4_800_000_000,
                epoch_ns: 0,
                sensor: 1,
            };
            let geom = radar_geom(&site);
            let frame = oa_geo::aeqd::frame_new(lat, lon);
            let mut rows = Vec::new();
            for (east, north) in [
                (0.0, 0.0),
                (0.01, 0.0),
                (-0.01, 0.0),
                (30_000.0, 0.0),
                (-30_000.0, 0.0),
                (0.0, 30_000.0),
            ] {
                let (target_lat, target_lon) =
                    oa_geo::aeqd::unproject(&frame, east, north).unwrap();
                let (rho, az) = polar_of(&geom, target_lat, target_lon, 8_000.0);
                let plot = RadarPlot {
                    time_ns: 0,
                    range_m: rho,
                    az_deg: az,
                    alt_ft: NONE_ALT,
                    addr: NO_ADDR,
                    label: NO_ADDR,
                    mode_a: NONE_CODE,
                };
                let converted =
                    plot_convert(&geom, &site, &plot, &NO_BIAS, Some((8_000.0, 25.0)), 1).unwrap();
                rows.push((rho, az, converted.geo.ground_m, 20.0));
            }
            assert!(rows[0].2 < 1.0, "overhead geometry: {:?}", rows[0]);
            let mut index = build(&rows);
            for &(rho, az, _, _) in &rows {
                let (result, candidates) = query(&mut index, &rows, (rho, az), QUERY_WORK);
                assert!(matches!(result, PolarQueryResult::Complete { .. }));
                let expected: Vec<u32> = rows
                    .iter()
                    .enumerate()
                    .filter(|(_, track)| coarse_passes(3_000.0, (rho, az), **track))
                    .map(|(i, _)| i as u32)
                    .collect();
                assert_eq!(candidates, expected, "site {lat}, {lon}; plot {rho}, {az}");
            }
        }
    }

    #[test]
    fn incomplete_build_and_query_clear_partial_outputs() {
        let rows = [(20_000.0, 0.0, 20_000.0, 0.0); 8];
        let mut index = polar_new(rows.len());
        let mut remaining = 1;
        let mut spent = 0;
        assert!(matches!(
            polar_build(
                &mut index,
                rows.len(),
                |i| rows.get(i).copied(),
                |_| false,
                &mut remaining,
                &mut spent,
            ),
            PolarBuildResult::Capped { .. }
        ));
        assert!(index.keys.is_empty());

        let mut index = build(&rows);
        let mut out = Vec::with_capacity(rows.len());
        out.push(999);
        let mut remaining = 0;
        let mut spent = 0;
        assert!(matches!(
            polar_query(
                &mut index,
                &mut out,
                PolarQuerySpec {
                    plot: (20_000.0, 0.0),
                    search_width: 4_500.0,
                    exact_window: 3_000.0,
                },
                |i| rows.get(i as usize).copied(),
                &mut remaining,
                &mut spent,
            ),
            PolarQueryResult::Capped { .. }
        ));
        assert!(out.is_empty());

        let mut out = Vec::with_capacity(rows.len() - 1);
        let mut remaining = QUERY_WORK;
        let mut spent = 0;
        assert!(matches!(
            polar_query(
                &mut index,
                &mut out,
                PolarQuerySpec {
                    plot: (f64::NAN, 0.0),
                    search_width: 4_500.0,
                    exact_window: 3_000.0,
                },
                |i| rows.get(i as usize).copied(),
                &mut remaining,
                &mut spent,
            ),
            PolarQueryResult::Capped { .. }
        ));
        assert!(out.is_empty());
    }

    #[test]
    // The polar lookup is independent of plot altitude; this isolates its 8,192-track candidate
    // reduction in the missing-altitude association workload.
    fn polar_query_8192_track_case_excludes_irrelevant_candidates() {
        let rows: Vec<_> = (0..8_192)
            .map(|i| {
                let az = (i as f64) * (360.0 / 8_192.0);
                let rho = 100_000.0 + ((i % 9) as f64 - 4.0) * 750.0;
                (rho, az, 100_000.0, 0.0)
            })
            .collect();
        let mut index = build(&rows);
        let (result, candidates) = query(&mut index, &rows, (100_000.0, 0.0), QUERY_WORK);
        assert!(matches!(result, PolarQueryResult::Complete { work } if work <= QUERY_WORK));
        assert!(
            candidates.len() < rows.len() / 8,
            "{} candidates",
            candidates.len()
        );
        for (i, track) in rows.iter().enumerate() {
            if coarse_passes(3_000.0, (100_000.0, 0.0), *track) {
                assert!(candidates.binary_search(&(i as u32)).is_ok(), "missed {i}");
            }
        }
    }

    #[test]
    fn radix_order_has_an_exact_budget_boundary_and_keeps_its_buffer() {
        let rows: Vec<_> = (0..128)
            .map(|i| (20_000.0 + i as f64, i as f64, 20_000.0, 0.0))
            .collect();
        let mut index = build(&rows);
        let buffers = polar_buffers(&index);
        let (result, candidates) = query(&mut index, &rows, (f64::NAN, f64::NAN), QUERY_WORK);
        let PolarQueryResult::Complete { work } = result else {
            panic!("complete query was capped");
        };
        assert_eq!(candidates, (0..rows.len() as u32).collect::<Vec<_>>());
        assert_eq!(polar_buffers(&index), buffers);

        let (result, candidates) = query(
            &mut index,
            &rows,
            (f64::NAN, f64::NAN),
            work.saturating_sub(1),
        );
        assert!(matches!(result, PolarQueryResult::Capped { .. }));
        assert!(candidates.is_empty());
        assert_eq!(polar_buffers(&index), buffers);

        let (result, candidates) = query(&mut index, &rows, (f64::NAN, f64::NAN), work);
        assert!(matches!(
            result,
            PolarQueryResult::Complete { work: exact } if exact == work
        ));
        assert_eq!(candidates, (0..rows.len() as u32).collect::<Vec<_>>());
        assert_eq!(polar_buffers(&index), buffers);
    }
}
