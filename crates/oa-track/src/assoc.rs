//! Global nearest-neighbour association: the assignment of plots to tracks over Mahalanobis-gated
//! candidate pairs, solved exactly. Core (pure).
//!
//! # The problem
//!
//! Each gated (track, plot) pair has a cost, the squared Mahalanobis distance of the plot from
//! the track's predicted position. A track takes at most one plot and a plot goes to at most one
//! track. The objective is the sum of the costs of the assigned pairs plus `gate / 2` for every
//! track and every plot left unassigned, so a pair is assigned in preference to leaving both
//! unassigned exactly when its cost is below the gate (the standard global-nearest-neighbour
//! objective). It is solved exactly, not greedily.
//!
//! # The algorithm
//!
//! The gated pairs form a bipartite graph. Its connected components are independent problems
//! (a union-find with path halving finds them), and most are a single pair. Each component is
//! padded to a square matrix with a dummy partner per track and per plot (cost `gate / 2` to the
//! own dummy, forbidden to every other, zero between dummies), which makes a perfect matching
//! always exist and encodes the unassigned penalty, and is solved by the Hungarian method with
//! shortest augmenting paths (Kuhn and Munkres, in the potentials formulation of Jonker and
//! Volgenant; `O(n^3)` for a component of `n = tracks + plots`). A component larger than
//! [`MAX_COMPONENT`] is not solved exactly: it is assigned greedily by ascending cost and
//! counted in [`GnnStats::greedy_components`], so the cost of a dense cluster is bounded.
//!
//! # Ambiguity
//!
//! [`ambiguous`] marks an assigned pair when the plot also lies within `margin` (in cost) of its
//! assigned cost of another track. The caller holds such a plot rather than letting kinematics
//! alone decide which of two close tracks it belongs to.
//!
//! The matrix code indexes arrays whose sizes are set at entry from the component's own
//! dimensions, so the module opts out of the indexing lint as `linalg.rs` does.
#![allow(clippy::indexing_slicing, clippy::needless_range_loop)]

/// Absent assignment.
pub const NONE: u32 = u32::MAX;
/// Limit: the largest component (tracks plus plots) solved exactly; 160 gives a padded matrix of
/// 160 rows and columns, about 4e6 operations worst case. Larger components are assigned
/// greedily and counted.
pub const MAX_COMPONENT: usize = 160;
/// Cost standing for "forbidden" inside the padded matrix; every real cost is far below it.
const FORBIDDEN: f64 = 1.0e12;

/// One gated candidate pair. Plain data (A1); hot (A7): built per plot per scan.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct Pair {
    pub track: u32,
    pub plot: u32,
    /// Squared Mahalanobis distance (horizontal plus vertical), below the gate.
    pub cost: f64,
}
const _: () = assert!(core::mem::size_of::<Pair>() == 16);

/// What a solve did. Plain record (A1); A7.2: cold struct.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GnnStats {
    /// Connected components of the gate graph.
    pub components: u64,
    /// Components with a single pair (no competition).
    pub single: u64,
    /// Components solved by the Hungarian method.
    pub solved: u64,
    /// Components too large, assigned greedily.
    pub greedy_components: u64,
    /// The largest component seen, tracks plus plots.
    pub largest: u64,
}

fn find(parent: &mut [u32], mut x: usize) -> usize {
    while parent[x] as usize != x {
        parent[x] = parent[parent[x] as usize];
        x = parent[x] as usize;
    }
    x
}

/// The exact minimum of the padded square assignment `cost[i * n + j]` (`n x n`): for each row
/// the column chosen. Hungarian method with potentials (rows and columns are 1-based inside).
fn hungarian(cost: &[f64], n: usize) -> Vec<usize> {
    let inf = f64::INFINITY;
    let mut u = vec![0.0; n + 1];
    let mut v = vec![0.0; n + 1];
    let mut p = vec![0usize; n + 1];
    let mut way = vec![0usize; n + 1];
    for i in 1..=n {
        p[0] = i;
        let mut j0 = 0usize;
        let mut minv = vec![inf; n + 1];
        let mut used = vec![false; n + 1];
        loop {
            used[j0] = true;
            let i0 = p[j0];
            let mut delta = inf;
            let mut j1 = 0usize;
            for j in 1..=n {
                if used[j] {
                    continue;
                }
                let cur = cost[(i0 - 1) * n + (j - 1)] - u[i0] - v[j];
                if cur < minv[j] {
                    minv[j] = cur;
                    way[j] = j0;
                }
                if minv[j] < delta {
                    delta = minv[j];
                    j1 = j;
                }
            }
            for j in 0..=n {
                if used[j] {
                    u[p[j]] += delta;
                    v[j] -= delta;
                } else {
                    minv[j] -= delta;
                }
            }
            j0 = j1;
            if p[j0] == 0 {
                break;
            }
        }
        loop {
            let j1 = way[j0];
            p[j0] = p[j1];
            j0 = j1;
            if j0 == 0 {
                break;
            }
        }
    }
    let mut row_to_col = vec![0usize; n];
    for j in 1..=n {
        if p[j] > 0 {
            row_to_col[p[j] - 1] = j - 1;
        }
    }
    row_to_col
}

/// The objective of an assignment: assigned costs plus `gate / 2` per unassigned track and
/// plot. `plot_track[j]` is the track of plot `j` or [`NONE`]. Used by the tests and by callers
/// that compare two assignments.
pub fn objective(pairs: &[Pair], n_tracks: usize, plot_track: &[u32], gate: f64) -> f64 {
    let mut sum = 0.0;
    let mut tracks_used = vec![false; n_tracks];
    let mut assigned = 0usize;
    for (j, &t) in plot_track.iter().enumerate() {
        if t == NONE {
            continue;
        }
        let c = pairs
            .iter()
            .find(|p| p.plot as usize == j && p.track == t)
            .map_or(FORBIDDEN, |p| p.cost);
        sum += c;
        assigned += 1;
        if let Some(u) = tracks_used.get_mut(t as usize) {
            *u = true;
        }
    }
    let unassigned_plots = plot_track.len() - assigned;
    let unassigned_tracks = tracks_used.iter().filter(|u| !**u).count();
    sum + 0.5 * gate * (unassigned_plots + unassigned_tracks) as f64
}

/// Global nearest-neighbour assignment of `n_plots` plots to `n_tracks` tracks over the gated
/// `pairs` (see the module documentation). Writes, for each plot, its track or [`NONE`] into
/// `plot_track` (resized to `n_plots`). Pairs with an index out of range are ignored. The result
/// depends only on the pairs' contents and order: ties are broken by the lowest track index.
pub fn gnn(
    n_tracks: usize,
    n_plots: usize,
    pairs: &[Pair],
    gate: f64,
    plot_track: &mut Vec<u32>,
) -> GnnStats {
    plot_track.clear();
    plot_track.resize(n_plots, NONE);
    let mut stats = GnnStats::default();
    let valid: Vec<Pair> = pairs
        .iter()
        .copied()
        .filter(|p| (p.track as usize) < n_tracks && (p.plot as usize) < n_plots)
        .collect();
    if valid.is_empty() {
        return stats;
    }
    // Union-find over tracks (0..n_tracks) and plots (n_tracks..n_tracks + n_plots).
    let mut parent: Vec<u32> = (0..(n_tracks + n_plots) as u32).collect();
    for p in &valid {
        let a = find(&mut parent, p.track as usize);
        let b = find(&mut parent, n_tracks + p.plot as usize);
        if a != b {
            let (lo, hi) = (a.min(b), a.max(b));
            parent[hi] = lo as u32;
        }
    }
    // Group pairs by component root, in ascending root order for a fixed result.
    let mut by_root: Vec<(usize, usize)> = valid
        .iter()
        .enumerate()
        .map(|(k, p)| (find(&mut parent, p.track as usize), k))
        .collect();
    by_root.sort_unstable();
    let mut at = 0usize;
    while at < by_root.len() {
        let root = by_root[at].0;
        let end = by_root[at..]
            .iter()
            .position(|&(r, _)| r != root)
            .map_or(by_root.len(), |d| at + d);
        let group: Vec<Pair> = by_root[at..end].iter().map(|&(_, k)| valid[k]).collect();
        at = end;
        stats.components += 1;
        solve_component(&group, gate, plot_track, &mut stats);
    }
    stats
}

fn solve_component(group: &[Pair], gate: f64, plot_track: &mut [u32], stats: &mut GnnStats) {
    // Local indexes of the component's tracks and plots, ascending.
    let mut tracks: Vec<u32> = group.iter().map(|p| p.track).collect();
    tracks.sort_unstable();
    tracks.dedup();
    let mut plots: Vec<u32> = group.iter().map(|p| p.plot).collect();
    plots.sort_unstable();
    plots.dedup();
    let (t, q) = (tracks.len(), plots.len());
    stats.largest = stats.largest.max((t + q) as u64);
    if group.len() == 1 {
        stats.single += 1;
        // A single gated pair is below the gate by construction (the caller's gate).
        if let Some(p) = group.first() {
            if p.cost < gate {
                plot_track[p.plot as usize] = p.track;
            }
        }
        return;
    }
    if t + q > MAX_COMPONENT {
        stats.greedy_components += 1;
        let mut order: Vec<usize> = (0..group.len()).collect();
        order.sort_by(|&a, &b| {
            group[a]
                .cost
                .total_cmp(&group[b].cost)
                .then(group[a].track.cmp(&group[b].track))
                .then(group[a].plot.cmp(&group[b].plot))
        });
        let mut track_used = vec![false; t];
        for k in order {
            let p = group[k];
            let ti = tracks.binary_search(&p.track).unwrap_or(0);
            if p.cost < gate && !track_used[ti] && plot_track[p.plot as usize] == NONE {
                track_used[ti] = true;
                plot_track[p.plot as usize] = p.track;
            }
        }
        return;
    }
    stats.solved += 1;
    // Padded square matrix: rows are tracks then plot-dummies, columns are plots then
    // track-dummies.
    let n = t + q;
    let mut m = vec![FORBIDDEN; n * n];
    for p in group {
        let (Ok(i), Ok(j)) = (tracks.binary_search(&p.track), plots.binary_search(&p.plot)) else {
            continue;
        };
        // The same pair twice keeps the cheaper cost.
        let c = &mut m[i * n + j];
        if p.cost < *c {
            *c = p.cost;
        }
    }
    let pen = 0.5 * gate;
    for i in 0..t {
        m[i * n + q + i] = pen;
    }
    for j in 0..q {
        m[(t + j) * n + j] = pen;
    }
    for i in t..n {
        for j in q..n {
            m[i * n + j] = 0.0;
        }
    }
    let choice = hungarian(&m, n);
    for (i, &j) in choice.iter().enumerate().take(t) {
        if j < q && m[i * n + j] < FORBIDDEN * 0.5 {
            plot_track[plots[j] as usize] = tracks[i];
        }
    }
}

/// Plain nearest neighbour, the baseline: each plot takes its cheapest gated track (the lowest
/// track index on a tie) and a track may take several plots. Writes the track of each plot or
/// [`NONE`] into `plot_track`.
pub fn nearest(n_plots: usize, pairs: &[Pair], plot_track: &mut Vec<u32>) {
    plot_track.clear();
    plot_track.resize(n_plots, NONE);
    let mut best = vec![f64::INFINITY; n_plots];
    for p in pairs {
        let j = p.plot as usize;
        if j >= n_plots {
            continue;
        }
        let better = p.cost < best[j] || (p.cost == best[j] && p.track < plot_track[j]);
        if better {
            best[j] = p.cost;
            plot_track[j] = p.track;
        }
    }
}

/// For each plot: its assigned track also has a competitor, another track whose gated cost for
/// this plot is within `margin` of the assigned cost. Unassigned plots are not ambiguous.
pub fn ambiguous(pairs: &[Pair], plot_track: &[u32], margin: f64) -> Vec<bool> {
    let mut out = vec![false; plot_track.len()];
    for (j, &t) in plot_track.iter().enumerate() {
        if t == NONE {
            continue;
        }
        let Some(own) = pairs.iter().find(|p| p.plot as usize == j && p.track == t) else {
            continue;
        };
        out[j] = pairs
            .iter()
            .any(|p| p.plot as usize == j && p.track != t && p.cost <= own.cost + margin);
    }
    out
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

    /// Deterministic LCG for test matrices.
    fn next(s: &mut u64) -> f64 {
        *s = s
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((*s >> 33) as f64) / f64::from(1u32 << 31)
    }

    /// The best objective over every partial matching, by exhaustive search.
    fn brute(pairs: &[Pair], n_tracks: usize, n_plots: usize, gate: f64) -> f64 {
        fn rec(
            j: usize,
            n_plots: usize,
            pairs: &[Pair],
            used: &mut Vec<bool>,
            acc: f64,
            assigned: usize,
            n_tracks: usize,
            gate: f64,
        ) -> f64 {
            if j == n_plots {
                let unassigned = (n_plots - assigned) + (n_tracks - assigned);
                return acc + 0.5 * gate * unassigned as f64;
            }
            let mut best = rec(j + 1, n_plots, pairs, used, acc, assigned, n_tracks, gate);
            for p in pairs.iter().filter(|p| p.plot as usize == j) {
                if !used[p.track as usize] {
                    used[p.track as usize] = true;
                    let v = rec(
                        j + 1,
                        n_plots,
                        pairs,
                        used,
                        acc + p.cost,
                        assigned + 1,
                        n_tracks,
                        gate,
                    );
                    used[p.track as usize] = false;
                    best = best.min(v);
                }
            }
            best
        }
        rec(
            0,
            n_plots,
            pairs,
            &mut vec![false; n_tracks],
            0.0,
            0,
            n_tracks,
            gate,
        )
    }

    // REQ: OA-TRK-035
    #[test]
    fn the_assignment_is_optimal_against_exhaustive_search() {
        let mut s = 12345u64;
        let gate = 16.0;
        let mut checked = 0;
        for case in 0..300 {
            let n_tracks = 1 + (next(&mut s) * 6.0) as usize;
            let n_plots = 1 + (next(&mut s) * 6.0) as usize;
            let density = 0.3 + 0.6 * next(&mut s);
            let mut pairs = Vec::new();
            for t in 0..n_tracks {
                for p in 0..n_plots {
                    if next(&mut s) < density {
                        pairs.push(Pair {
                            track: t as u32,
                            plot: p as u32,
                            cost: next(&mut s) * gate * 0.999,
                        });
                    }
                }
            }
            let mut out = Vec::new();
            gnn(n_tracks, n_plots, &pairs, gate, &mut out);
            // A valid matching: each track at most once, only gated pairs.
            let mut seen = vec![false; n_tracks];
            for (j, &t) in out.iter().enumerate() {
                if t != NONE {
                    assert!(!seen[t as usize], "case {case}: track {t} twice");
                    seen[t as usize] = true;
                    assert!(pairs.iter().any(|p| p.plot as usize == j && p.track == t));
                }
            }
            let got = objective(&pairs, n_tracks, &out, gate);
            let want = brute(&pairs, n_tracks, n_plots, gate);
            assert!(
                (got - want).abs() < 1e-9,
                "case {case}: got {got}, optimum {want}"
            );
            checked += 1;
        }
        assert_eq!(checked, 300);
    }

    // REQ: OA-TRK-035
    #[test]
    fn nearest_neighbour_can_be_worse_than_global_where_two_plots_want_one_track() {
        // Plot 0 prefers track 0 (cost 1) but plot 1 can only take track 0 (cost 2) or nothing;
        // plot 0's alternative is track 1 (cost 3). GNN: 0->1 (3) and 1->0 (2) = 5 against the
        // greedy 0->0 (1) with plot 1 and track 1 unassigned (1 + 2 * 8 = 17).
        let pairs = [
            Pair {
                track: 0,
                plot: 0,
                cost: 1.0,
            },
            Pair {
                track: 1,
                plot: 0,
                cost: 3.0,
            },
            Pair {
                track: 0,
                plot: 1,
                cost: 2.0,
            },
        ];
        let mut g = Vec::new();
        gnn(2, 2, &pairs, 16.0, &mut g);
        assert_eq!(g, vec![1, 0]);
        let mut nn = Vec::new();
        nearest(2, &pairs, &mut nn);
        // Plain nearest neighbour hands track 0 to both plots.
        assert_eq!(nn, vec![0, 0]);
    }

    // REQ: OA-TRK-035
    #[test]
    fn components_are_independent_and_large_ones_fall_back_to_greedy() {
        // 200 plots all gated to 200 tracks in one component (a chain): over MAX_COMPONENT.
        let n = 100;
        let mut pairs = Vec::new();
        for i in 0..n {
            pairs.push(Pair {
                track: i,
                plot: i,
                cost: 1.0,
            });
            pairs.push(Pair {
                track: i,
                plot: (i + 1) % n,
                cost: 5.0,
            });
        }
        let mut out = Vec::new();
        let st = gnn(n as usize, n as usize, &pairs, 16.0, &mut out);
        assert_eq!(st.components, 1);
        assert_eq!(st.greedy_components, 1);
        assert_eq!(st.solved, 0);
        // Greedy by ascending cost still finds the diagonal.
        assert!(out.iter().enumerate().all(|(j, &t)| t as usize == j));
        // Many tiny components are solved exactly and independently.
        let mut pairs = Vec::new();
        for i in 0..50u32 {
            pairs.push(Pair {
                track: i,
                plot: i,
                cost: 2.0,
            });
        }
        let st = gnn(50, 50, &pairs, 16.0, &mut out);
        assert_eq!(st.components, 50);
        assert_eq!(st.single, 50);
    }

    // REQ: OA-TRK-035
    #[test]
    fn a_pair_at_or_above_the_gate_is_not_assigned() {
        let pairs = [
            Pair {
                track: 0,
                plot: 0,
                cost: 15.9,
            },
            Pair {
                track: 1,
                plot: 1,
                cost: 16.0,
            },
        ];
        let mut out = Vec::new();
        gnn(2, 2, &pairs, 16.0, &mut out);
        assert_eq!(out, vec![0, NONE]);
    }

    // REQ: OA-TRK-036
    #[test]
    fn ambiguity_marks_plots_with_a_close_second_track() {
        let pairs = [
            Pair {
                track: 0,
                plot: 0,
                cost: 1.0,
            },
            Pair {
                track: 1,
                plot: 0,
                cost: 2.5,
            },
            Pair {
                track: 2,
                plot: 1,
                cost: 1.0,
            },
            Pair {
                track: 3,
                plot: 1,
                cost: 9.0,
            },
        ];
        let assigned = [0, 2, NONE];
        let a = ambiguous(&pairs, &assigned, 4.0);
        assert_eq!(a, vec![true, false, false]);
    }
}
