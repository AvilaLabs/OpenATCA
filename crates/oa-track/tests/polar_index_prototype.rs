//! Non-production characterization of a bounded polar candidate index.
//!
//! This integration test is intentionally unwired from all library code. It mirrors the exact
//! coarse predicate at base b45fa6910c43173d9955dac7e54ac24380fe1079 and compares candidates to
//! that brute-force reference. No production source is changed.

use oa_track::coarse::coarse_passes;

const MAX_TRACKS: usize = 512;
const MAX_WIDTH_BAND: u8 = 127;
const MAX_GROUND_POWER: u8 = 25;
const MAX_RANGE_M: f64 = 16_000_000.0;
const MAX_GROUND_M: f64 = 33_554_432.0;
const RANGE_CELL_M: f64 = 500.0;
const AZ_ABS_MAX_DEG: f64 = 720.0;
const QUERY_WORK_MAX: usize = 6_000;
const BUILD_WORK_MAX: usize = 500_000;
const ALWAYS_BAND: u8 = u8::MAX;
const RANGE_BIN_PROBE_COST: usize = 20;
const MAX_WIDTH_M: f64 = 65_536.0;
const WIDTH_CELL_M: f64 = 512.0;

#[derive(Clone, Copy, Debug)]
struct Track {
    rho: f64,
    az: f64,
    ground: f64,
    sigma: f64,
}

#[derive(Clone, Copy, Debug)]
struct Posting {
    width_band: u8,
    range_cell: i32,
    az: f64,
    ground: f64,
    track: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BuildResult {
    Complete,
    CapacityFull,
    Capped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QueryResult {
    Complete { count: usize, work: usize },
    ScanRequired,
}

/// A fixed-capacity test-only index. `posts` is allocated once at construction. One input track
/// creates exactly one posting, either in a regular cell or the explicit always-scan class.
struct PolarIndex {
    // This insertion-sort characterization is intentionally limited to MAX_TRACKS. It does not
    // claim to be an appropriate build strategy at the production 65,536-track ceiling.
    posts: Vec<Posting>,
    max_tracks: usize,
    active_width: [u8; MAX_TRACKS],
    active_count: usize,
    valid: bool,
}

fn polar_new(capacity: usize) -> PolarIndex {
    assert!(capacity <= MAX_TRACKS);
    PolarIndex {
        posts: Vec::with_capacity(capacity),
        max_tracks: capacity,
        active_width: [0; MAX_TRACKS],
        active_count: 0,
        valid: false,
    }
}

fn polar_build(ix: &mut PolarIndex, tracks: &[Track], limit: usize) -> BuildResult {
    ix.posts.clear();
    ix.active_count = 0;
    ix.valid = false;
    if tracks.len() > ix.max_tracks
        || tracks.len() > ix.posts.capacity()
        || tracks.len() > MAX_TRACKS
    {
        return BuildResult::CapacityFull;
    }
    let mut work = 0;
    for (i, t) in tracks.iter().enumerate() {
        // One charged input visit covers validation, arithmetic, and band/cell classification.
        if !charge_many(&mut work, limit, 8) {
            ix.posts.clear();
            ix.active_count = 0;
            return BuildResult::Capped;
        }
        let width = 3_000.0 + 8.0 * t.sigma;
        let (width_band, range_cell) = match regular_bands(t.rho, t.az, t.ground, width) {
            Some((w, r)) => {
                let mut found = false;
                for pair in 0..ix.active_count {
                    if !charge(&mut work, limit) {
                        ix.posts.clear();
                        ix.active_count = 0;
                        return BuildResult::Capped;
                    }
                    if ix
                        .active_width
                        .get(pair)
                        .is_some_and(|candidate| *candidate == w)
                    {
                        found = true;
                        break;
                    }
                }
                if !found {
                    let Some(slot) = ix.active_width.get_mut(ix.active_count) else {
                        ix.posts.clear();
                        ix.active_count = 0;
                        return BuildResult::CapacityFull;
                    };
                    *slot = w;
                    ix.active_count += 1;
                }
                (w, r)
            }
            None => (ALWAYS_BAND, 0),
        };
        ix.posts.push(Posting {
            width_band,
            range_cell,
            az: t.az,
            ground: t.ground,
            track: i as u32,
        });
    }
    // The bounded insertion sort charges every comparison and move. A capped sort clears all
    // partial state and never marks the index usable.
    for i in 1..ix.posts.len() {
        let mut j = i;
        while j > 0 {
            if !charge(&mut work, limit) {
                ix.posts.clear();
                ix.active_count = 0;
                return BuildResult::Capped;
            }
            let Some(previous) = ix.posts.get(j - 1).copied() else {
                ix.posts.clear();
                ix.active_count = 0;
                return BuildResult::CapacityFull;
            };
            let Some(current) = ix.posts.get(j).copied() else {
                ix.posts.clear();
                ix.active_count = 0;
                return BuildResult::CapacityFull;
            };
            if posting_key(previous) <= posting_key(current) {
                break;
            }
            if !charge(&mut work, limit) {
                ix.posts.clear();
                ix.active_count = 0;
                return BuildResult::Capped;
            }
            ix.posts.swap(j - 1, j);
            j -= 1;
        }
    }
    ix.valid = true;
    BuildResult::Complete
}

fn polar_query(
    ix: &PolarIndex,
    plot_rho: f64,
    plot_az: f64,
    out: &mut [u32; MAX_TRACKS],
    out_len: &mut usize,
    limit: usize,
) -> QueryResult {
    *out_len = 0;
    if !(ix.valid
        && plot_rho.is_finite()
        && (0.0..=MAX_RANGE_M).contains(&plot_rho)
        && plot_az.is_finite()
        && plot_az.abs() <= AZ_ABS_MAX_DEG)
    {
        return QueryResult::ScanRequired;
    }
    let mut work = 0;
    let mut selected = [0u64; MAX_TRACKS / 64];
    for pair in 0..ix.active_count {
        if !charge_many(&mut work, limit, 8) {
            return QueryResult::ScanRequired;
        }
        let Some(&wb) = ix.active_width.get(pair) else {
            return QueryResult::ScanRequired;
        };
        let width_hi = (f64::from(wb) + 1.0) * WIDTH_CELL_M;
        let lo_rho = (plot_rho - width_hi).max(0.0);
        let hi_rho = (plot_rho + width_hi).min(MAX_RANGE_M);
        let first_range = ((lo_rho / RANGE_CELL_M).floor() as i32 - 1).max(0);
        let max_range_cell = (MAX_RANGE_M / RANGE_CELL_M).floor() as i32;
        let last_range = ((hi_rho / RANGE_CELL_M).floor() as i32 + 1).min(max_range_cell);
        // Search the whole conservative radial interval with two prefix lookups, then apply the
        // conservative angular bound to each posting in that interval. This avoids spending work
        // on empty azimuth cells while retaining one posting per track.
        if !charge_many(&mut work, limit, RANGE_BIN_PROBE_COST) {
            return QueryResult::ScanRequired;
        }
        let first_key = (wb, first_range);
        let last_key = (wb, last_range);
        let start = ix
            .posts
            .partition_point(|p| (p.width_band, p.range_cell) < first_key);
        let end = ix
            .posts
            .partition_point(|p| (p.width_band, p.range_cell) <= last_key);
        let Some(postings) = ix.posts.get(start..end) else {
            return QueryResult::ScanRequired;
        };
        for p in postings {
            if !charge(&mut work, limit) {
                return QueryResult::ScanRequired;
            }
            let angular = (az_diff(plot_az, p.az).to_radians() * p.ground).abs();
            if angular <= width_hi {
                let bit = p.track as usize;
                let Some(word) = selected.get_mut(bit / 64) else {
                    return QueryResult::ScanRequired;
                };
                *word |= 1u64 << (bit % 64);
            }
        }
    }
    for p in &ix.posts {
        if !charge(&mut work, limit) {
            return QueryResult::ScanRequired;
        }
        if p.width_band == ALWAYS_BAND {
            let bit = p.track as usize;
            let Some(word) = selected.get_mut(bit / 64) else {
                return QueryResult::ScanRequired;
            };
            *word |= 1u64 << (bit % 64);
        }
    }
    for (word_i, word) in selected.iter().enumerate() {
        if !charge(&mut work, limit) {
            *out_len = 0;
            return QueryResult::ScanRequired;
        }
        let mut bits = *word;
        while bits != 0 {
            if !charge(&mut work, limit) {
                *out_len = 0;
                return QueryResult::ScanRequired;
            }
            let bit = bits.trailing_zeros() as usize;
            let Some(slot) = out.get_mut(*out_len) else {
                *out_len = 0;
                return QueryResult::ScanRequired;
            };
            *slot = (word_i * 64 + bit) as u32;
            *out_len += 1;
            bits &= bits - 1;
        }
    }
    QueryResult::Complete {
        count: *out_len,
        work,
    }
}

fn posting_key(p: Posting) -> (u8, i32, u32) {
    (p.width_band, p.range_cell, p.track)
}

fn regular_bands(rho: f64, az: f64, ground: f64, width: f64) -> Option<(u8, i32)> {
    if !(rho.is_finite()
        && (0.0..=MAX_RANGE_M).contains(&rho)
        && az.is_finite()
        && az.abs() <= AZ_ABS_MAX_DEG
        && ground.is_finite()
        && (1.0..MAX_GROUND_M).contains(&ground)
        && width.is_finite()
        && (1.0..MAX_WIDTH_M).contains(&width))
    {
        return None;
    }
    let width_band = (width / WIDTH_CELL_M).floor() as u8;
    Some((width_band, (rho / RANGE_CELL_M).floor() as i32))
}

fn az_diff(a: f64, b: f64) -> f64 {
    (a - b + 180.0).rem_euclid(360.0) - 180.0
}

fn charge(work: &mut usize, limit: usize) -> bool {
    if *work >= limit {
        false
    } else {
        *work += 1;
        true
    }
}

fn charge_many(work: &mut usize, limit: usize, units: usize) -> bool {
    let Some(next) = work.checked_add(units) else {
        return false;
    };
    if next > limit {
        false
    } else {
        *work = next;
        true
    }
}

fn all_matches(tracks: &[Track], plot: (f64, f64)) -> Vec<u32> {
    tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| coarse_passes(3_000.0, plot, (t.rho, t.az, t.ground, t.sigma)))
        .map(|(i, _)| i as u32)
        .collect()
}

fn indexed(index: &PolarIndex, plot: (f64, f64), budget: usize) -> (QueryResult, Vec<u32>) {
    let mut out = [u32::MAX; MAX_TRACKS];
    let mut len = 0;
    let result = polar_query(index, plot.0, plot.1, &mut out, &mut len, budget);
    match out.get(..len) {
        Some(values) => (result, values.to_vec()),
        None => (QueryResult::ScanRequired, Vec::new()),
    }
}

fn next_up(x: f64) -> f64 {
    if x.is_nan() || x == f64::INFINITY {
        return x;
    }
    if x == 0.0 {
        return f64::from_bits(1);
    }
    let bits = x.to_bits();
    f64::from_bits(if x > 0.0 { bits + 1 } else { bits - 1 })
}

fn next_down(x: f64) -> f64 {
    -next_up(-x)
}

fn angular_accept_reject_neighbors(
    plot_rho: f64,
    width: f64,
    ground: f64,
    sigma: f64,
) -> (f64, f64) {
    let mut accepted = width / ground * (180.0 / std::f64::consts::PI);
    for _ in 0..128 {
        if coarse_passes(
            3_000.0,
            (plot_rho, 0.0),
            (plot_rho, accepted, ground, sigma),
        ) {
            break;
        }
        accepted = next_down(accepted);
    }
    assert!(coarse_passes(
        3_000.0,
        (plot_rho, 0.0),
        (plot_rho, accepted, ground, sigma)
    ));

    let mut rejected = next_up(accepted);
    for _ in 0..128 {
        if !coarse_passes(
            3_000.0,
            (plot_rho, 0.0),
            (plot_rho, rejected, ground, sigma),
        ) {
            break;
        }
        accepted = rejected;
        rejected = next_up(accepted);
    }
    assert!(!coarse_passes(
        3_000.0,
        (plot_rho, 0.0),
        (plot_rho, rejected, ground, sigma)
    ));
    assert_eq!(next_up(accepted), rejected);
    (accepted, rejected)
}

#[test]
fn exhaustive_small_discretized_domain_is_a_superset_and_reduces_obvious_non_candidates() {
    let rhos = [0.0, 250.0, 500.0, 1_000.0, 10_000.0];
    let azimuths = [-180.0, -179.0, -1.0, 0.0, 1.0, 179.0, 180.0];
    let grounds = [2.0, 32.0, 1_024.0, 65_536.0];
    let sigmas = [0.0, 125.0, 500.0];
    let mut tracks = Vec::new();
    for &rho in &rhos {
        for &az in &azimuths {
            for &ground in &grounds {
                for &sigma in &sigmas {
                    tracks.push(Track {
                        rho,
                        az,
                        ground,
                        sigma,
                    });
                }
            }
        }
    }
    let mut index = polar_new(tracks.len());
    assert_eq!(
        polar_build(&mut index, &tracks, BUILD_WORK_MAX),
        BuildResult::Complete
    );
    let (result, candidates) = indexed(&index, (500.0, 179.5), QUERY_WORK_MAX);
    assert!(matches!(result, QueryResult::Complete { .. }));
    let exact = all_matches(&tracks, (500.0, 179.5));
    assert!(exact.iter().all(|i| candidates.binary_search(i).is_ok()));
    assert!(candidates.len() < tracks.len());

    for &rho in &rhos {
        for &az in &azimuths {
            let (result, candidate_set) = indexed(&index, (rho, az), QUERY_WORK_MAX);
            assert!(matches!(result, QueryResult::Complete { .. }));
            let exact_set = all_matches(&tracks, (rho, az));
            assert!(exact_set
                .iter()
                .all(|i| candidate_set.binary_search(i).is_ok()));
        }
    }
}

#[test]
fn polar_cells_materially_exclude_far_range_and_wrapped_azimuth_tracks() {
    let mut tracks = vec![Track {
        rho: 100_000.0,
        az: 359.5,
        ground: 100_000.0,
        sigma: 0.0,
    }];
    for i in 0..31 {
        tracks.push(Track {
            rho: 100_000.0,
            az: 20.0 + f64::from(i),
            ground: 100_000.0,
            sigma: 0.0,
        });
    }
    for _ in 0..32 {
        tracks.push(Track {
            rho: 200_000.0,
            az: 0.0,
            ground: 100_000.0,
            sigma: 0.0,
        });
    }
    let mut index = polar_new(tracks.len());
    assert_eq!(
        polar_build(&mut index, &tracks, BUILD_WORK_MAX),
        BuildResult::Complete
    );
    let (result, candidates) = indexed(&index, (100_000.0, -0.5), QUERY_WORK_MAX);
    assert!(matches!(result, QueryResult::Complete { .. }));
    let exact = all_matches(&tracks, (100_000.0, -0.5));
    assert!(exact.iter().all(|i| candidates.binary_search(i).is_ok()));
    assert!(candidates.len() <= 2);
    assert!(candidates.len() < tracks.len() / 16);
}

#[test]
fn width_and_ground_band_edges_and_nextafter_are_included() {
    for band in 5..=MAX_WIDTH_BAND {
        let edge = (f64::from(band) + 1.0) * WIDTH_CELL_M;
        let mut tracks = [Track {
            rho: 100_000.0,
            az: 45.0,
            ground: 8_192.0,
            sigma: 0.0,
        }; 3];
        for (slot, width) in tracks
            .iter_mut()
            .zip([next_down(edge), edge, next_up(edge)])
        {
            let sigma = (width - 3_000.0) / 8.0;
            let actual = 3_000.0 + 8.0 * sigma;
            assert!(actual == width || next_up(actual) == width || next_down(actual) == width);
            *slot = Track {
                rho: 100_000.0,
                az: 45.0,
                ground: 8_192.0,
                sigma,
            };
        }
        assert_edge_case(&tracks, (100_000.0, 45.0));
    }
    for band in 0..=MAX_GROUND_POWER {
        let edge = 2.0_f64.powi(i32::from(band));
        let mut tracks = [Track {
            rho: 100_000.0,
            az: 45.0,
            ground: 8_192.0,
            sigma: 125.0,
        }; 3];
        for (slot, ground) in tracks
            .iter_mut()
            .zip([next_down(edge), edge, next_up(edge)])
        {
            *slot = Track {
                rho: 100_000.0,
                az: 45.0,
                ground,
                sigma: 125.0,
            };
        }
        assert_edge_case(&tracks, (100_000.0, 45.0));
    }
}

fn assert_edge_case(tracks: &[Track; 3], plot: (f64, f64)) {
    let mut index = polar_new(tracks.len());
    assert_eq!(
        polar_build(&mut index, tracks, BUILD_WORK_MAX),
        BuildResult::Complete
    );
    let (result, candidates) = indexed(&index, plot, QUERY_WORK_MAX);
    assert!(matches!(result, QueryResult::Complete { .. }));
    let exact = all_matches(tracks, plot);
    assert!(exact.iter().all(|i| candidates.binary_search(i).is_ok()));
}

#[test]
fn range_azimuth_wrap_and_exact_boundary_neighbors_are_supersets() {
    let width = 4_000.0_f64;
    let sigma = (width - 3_000.0) / 8.0;
    let angular_deg = (width / 100_000.0).to_degrees();
    let tracks = [
        Track {
            rho: 100_000.0 - width,
            az: 179.0,
            ground: 100_000.0,
            sigma,
        },
        Track {
            rho: 100_000.0 + width,
            az: -179.0,
            ground: 100_000.0,
            sigma,
        },
        Track {
            rho: 100_000.0,
            az: 180.0 - angular_deg,
            ground: 100_000.0,
            sigma,
        },
        Track {
            rho: 100_000.0,
            az: next_down(180.0 - angular_deg),
            ground: 100_000.0,
            sigma,
        },
        Track {
            rho: 100_000.0,
            az: 0.0,
            ground: 100_000.0,
            sigma,
        },
    ];
    let mut index = polar_new(tracks.len());
    assert_eq!(
        polar_build(&mut index, &tracks, BUILD_WORK_MAX),
        BuildResult::Complete
    );
    let plot = (100_000.0, -179.0);
    let (result, candidates) = indexed(&index, plot, QUERY_WORK_MAX);
    assert!(matches!(result, QueryResult::Complete { .. }));
    let exact = all_matches(&tracks, plot);
    assert!(exact.iter().all(|i| candidates.binary_search(i).is_ok()));
}

#[test]
fn exact_and_nextafter_range_and_azimuth_boundaries_match_the_gate() {
    let plot_rho = 100_000.0;
    let width = 4_000.0_f64;
    let sigma = (width - 3_000.0) / 8.0;
    let ground = 100_000.0;
    let (accepted_angle, rejected_angle) =
        angular_accept_reject_neighbors(plot_rho, width, ground, sigma);
    let tracks = [
        Track {
            rho: plot_rho + width,
            az: 0.0,
            ground,
            sigma,
        },
        Track {
            rho: next_down(plot_rho + width),
            az: 0.0,
            ground,
            sigma,
        },
        Track {
            rho: next_up(plot_rho + width),
            az: 0.0,
            ground,
            sigma,
        },
        Track {
            rho: plot_rho,
            az: next_down(accepted_angle),
            ground,
            sigma,
        },
        Track {
            rho: plot_rho,
            az: accepted_angle,
            ground,
            sigma,
        },
        Track {
            rho: plot_rho,
            az: rejected_angle,
            ground,
            sigma,
        },
    ];
    assert!(coarse_passes(
        3_000.0,
        (plot_rho, 0.0),
        (tracks[0].rho, tracks[0].az, ground, sigma)
    ));
    assert!(!coarse_passes(
        3_000.0,
        (plot_rho, 0.0),
        (tracks[2].rho, tracks[2].az, ground, sigma)
    ));
    assert!(coarse_passes(
        3_000.0,
        (plot_rho, 0.0),
        (tracks[3].rho, tracks[3].az, ground, sigma)
    ));
    assert!(!coarse_passes(
        3_000.0,
        (plot_rho, 0.0),
        (tracks[5].rho, tracks[5].az, ground, sigma)
    ));
    let mut index = polar_new(tracks.len());
    assert_eq!(
        polar_build(&mut index, &tracks, BUILD_WORK_MAX),
        BuildResult::Complete
    );
    let (result, candidates) = indexed(&index, (plot_rho, 0.0), QUERY_WORK_MAX);
    assert!(matches!(result, QueryResult::Complete { .. }));
    let exact = all_matches(&tracks, (plot_rho, 0.0));
    assert!(exact.iter().all(|i| candidates.binary_search(i).is_ok()));
}

#[test]
fn exceptional_geometry_and_high_uncertainty_use_always_scan_bucket() {
    let tracks = [
        Track {
            rho: 5_000.0,
            az: 1.0,
            ground: 0.0,
            sigma: 0.0,
        },
        Track {
            rho: 5_000.0,
            az: f64::NAN,
            ground: 100.0,
            sigma: 0.0,
        },
        Track {
            rho: f64::INFINITY,
            az: 1.0,
            ground: 100.0,
            sigma: 0.0,
        },
        Track {
            rho: 5_000.0,
            az: 1.0,
            ground: f64::INFINITY,
            sigma: 0.0,
        },
        Track {
            rho: f64::NAN,
            az: 1.0,
            ground: 100.0,
            sigma: 0.0,
        },
        Track {
            rho: 5_000.0,
            az: f64::INFINITY,
            ground: 100.0,
            sigma: 0.0,
        },
        Track {
            rho: 5_000.0,
            az: 1.0,
            ground: f64::NAN,
            sigma: 0.0,
        },
        Track {
            rho: 5_000.0,
            az: 1.0,
            ground: 100.0,
            sigma: f64::INFINITY,
        },
        Track {
            rho: 5_000.0,
            az: 1.0,
            ground: 100.0,
            sigma: f64::NAN,
        },
        Track {
            rho: 5_000.0,
            az: 1.0,
            ground: -1.0,
            sigma: 0.0,
        },
    ];
    let mut index = polar_new(tracks.len());
    assert_eq!(
        polar_build(&mut index, &tracks, BUILD_WORK_MAX),
        BuildResult::Complete
    );
    assert!(
        index
            .posts
            .iter()
            .filter(|p| p.width_band == ALWAYS_BAND)
            .count()
            >= 9
    );
    for plot in [(0.0, 0.0), (5_000.0, 180.0), (f64::NAN, 0.0)] {
        let (result, candidates) = indexed(&index, plot, QUERY_WORK_MAX);
        if plot.0.is_finite() {
            assert!(matches!(result, QueryResult::Complete { .. }));
            let exact = all_matches(&tracks, plot);
            assert!(exact.iter().all(|i| candidates.binary_search(i).is_ok()));
        } else {
            assert_eq!(result, QueryResult::ScanRequired);
            assert!(candidates.is_empty());
        }
    }
    for plot in [
        (1.0, f64::INFINITY),
        (-1.0, 0.0),
        (MAX_RANGE_M + 1.0, 0.0),
        (0.0, 1.0e100),
    ] {
        let (result, candidates) = indexed(&index, plot, QUERY_WORK_MAX);
        assert_eq!(result, QueryResult::ScanRequired);
        assert!(candidates.is_empty());
    }
}

#[test]
fn capacity_and_incomplete_operations_never_expose_a_partial_index_or_query() {
    let tracks = [Track {
        rho: 10_000.0,
        az: 0.0,
        ground: 10_000.0,
        sigma: 0.0,
    }; 4];
    let mut too_small = polar_new(3);
    assert_eq!(
        polar_build(&mut too_small, &tracks, BUILD_WORK_MAX),
        BuildResult::CapacityFull
    );
    let (result, candidates) = indexed(&too_small, (10_000.0, 0.0), QUERY_WORK_MAX);
    assert_eq!(result, QueryResult::ScanRequired);
    assert!(candidates.is_empty());

    let mut capped_build = polar_new(tracks.len());
    assert_eq!(
        polar_build(&mut capped_build, &tracks, 15),
        BuildResult::Capped
    );
    let (result, candidates) = indexed(&capped_build, (10_000.0, 0.0), QUERY_WORK_MAX);
    assert_eq!(result, QueryResult::ScanRequired);
    assert!(candidates.is_empty());

    let mut index = polar_new(tracks.len());
    assert_eq!(
        polar_build(&mut index, &tracks, BUILD_WORK_MAX),
        BuildResult::Complete
    );
    let (result, candidates) = indexed(&index, (10_000.0, 0.0), 0);
    assert_eq!(result, QueryResult::ScanRequired);
    assert!(candidates.is_empty());
    let (result, candidates) = indexed(&index, (10_000.0, 0.0), 1);
    assert_eq!(result, QueryResult::ScanRequired);
    assert!(candidates.is_empty());
    // Four candidates are emitted by work unit 41; the following bitset-word visit exhausts the
    // allowance. The query must retract that partial output before requesting the full scan.
    let (result, candidates) = indexed(&index, (10_000.0, 0.0), 41);
    assert_eq!(result, QueryResult::ScanRequired);
    assert!(candidates.is_empty());
}

#[test]
fn deterministic_seeded_cases_match_brute_force_or_request_full_scan() {
    let mut state = 0x4d59_5df4_d0f3_3173u64;
    let mut next = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        (state >> 11) as f64 / ((1u64 << 53) as f64)
    };
    let mut tracks = Vec::new();
    for _ in 0..MAX_TRACKS {
        let rho = next() * 300_000.0;
        let az = next() * 720.0 - 360.0;
        let ground = 500.0 + next() * 299_500.0;
        let sigma = next() * 400.0;
        tracks.push(Track {
            rho,
            az,
            ground,
            sigma,
        });
    }
    let mut index = polar_new(tracks.len());
    assert_eq!(
        polar_build(&mut index, &tracks, BUILD_WORK_MAX),
        BuildResult::Complete
    );
    let mut complete_queries = 0;
    for _ in 0..80 {
        let plot = (next() * 300_000.0, next() * 720.0 - 360.0);
        let (result, candidates) = indexed(&index, plot, QUERY_WORK_MAX);
        match result {
            QueryResult::Complete { count, work } => {
                complete_queries += 1;
                assert_eq!(count, candidates.len());
                assert!(work <= QUERY_WORK_MAX);
                assert!(candidates.windows(2).all(|w| matches!(w, [a, b] if a < b)));
                let exact = all_matches(&tracks, plot);
                assert!(exact.iter().all(|i| candidates.binary_search(i).is_ok()));
            }
            QueryResult::ScanRequired => assert!(candidates.is_empty()),
        }
    }
    assert!(
        complete_queries >= 64,
        "only {complete_queries} of 80 queries completed"
    );
}
