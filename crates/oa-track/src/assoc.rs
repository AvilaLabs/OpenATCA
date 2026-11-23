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
//! [`ambiguous_pairs`] runs in one pass over the pairs, and [`assigned_pairs`] finds the pair
//! each assigned plot took in one pass, so a bucket's association is near-linear in its pairs
//! after the solve (the solve itself is bounded by [`MAX_COMPONENT`]).
//!
//! # Memory and refusals
//!
//! Every buffer the solve and the passes after it use lives in an [`AssocScratch`] sized once
//! at start (RULESET C1, C3): nothing here allocates after [`assoc_scratch_new`]. A solve
//! refuses, with a typed [`GnnError`] and every plot left unassigned, a gate that is not a
//! finite positive number (a NaN or infinite penalty makes the padded matrix meaningless) and
//! an input larger than the scratch (tracks, plots or pairs beyond the capacities it was
//! built with); the caller counts the refusal.
//!
//! The matrix code indexes arrays whose sizes are set at entry from the component's own
//! dimensions, so the module opts out of the indexing lint as `linalg.rs` does.
#![allow(clippy::indexing_slicing, clippy::needless_range_loop)]

/// Absent assignment.
pub const NONE: u32 = u32::MAX;
/// Association path was not recorded for this plot.
pub const PATH_NONE: u8 = 0;
/// One-pair component (no competition).
pub const PATH_SINGLE: u8 = 1;
/// Component solved by the exact Hungarian method.
pub const PATH_EXACT: u8 = 2;
/// Component assigned by the bounded greedy fallback.
pub const PATH_GREEDY: u8 = 3;
/// Plain nearest-neighbour baseline; no component solve was run.
pub const PATH_NEAREST: u8 = 4;
/// Limit: the largest component (tracks plus plots) solved exactly; 160 gives a padded matrix of
/// 160 rows and columns, about 4e6 operations worst case. Larger components are assigned
/// greedily and counted.
pub const MAX_COMPONENT: usize = 160;
/// Limit: tracks or plots a scratch is built for; a larger request is built at this size (a solve
/// asking for more is refused as too large).
pub const MAX_SCRATCH_ITEMS: usize = 1 << 22;
/// Limit: pairs a scratch is built for, the same way.
pub const MAX_SCRATCH_PAIRS: usize = 1 << 22;
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

/// Why a solve was refused; every plot is left unassigned. Plain data (A1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GnnError {
    /// The gate is not a finite positive number.
    BadGate,
    /// More tracks, plots or pairs than the scratch was built for.
    TooLarge,
}

impl std::fmt::Display for GnnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GnnError::BadGate => write!(f, "association gate is not a finite positive number"),
            GnnError::TooLarge => write!(f, "association input exceeds the scratch capacity"),
        }
    }
}

impl std::error::Error for GnnError {}

/// The buffers of the Hungarian method for one component (at most [`MAX_COMPONENT`] square).
// A7.2: cold struct, size guard waived
struct HungBufs {
    cost: Vec<f64>,
    u: Vec<f64>,
    v: Vec<f64>,
    minv: Vec<f64>,
    p: Vec<usize>,
    way: Vec<usize>,
    used: Vec<bool>,
    row_to_col: Vec<usize>,
}

/// Everything a solve and the passes after it need, allocated once. A subsystem's memory (C4):
/// fields private, the interface the free functions of this module.
// A7.2: cold struct, size guard waived
pub struct AssocScratch {
    max_tracks: usize,
    max_plots: usize,
    max_pairs: usize,
    collect_metadata: bool,
    metadata_writes: u64,
    /// Result: the track of each plot, or [`NONE`]; the first `n_plots` entries are valid.
    plot_track: Vec<u32>,
    n_plots: usize,
    /// Result of [`assigned_pairs`]: the index of the pair each plot took, or [`NONE`].
    pair_of: Vec<u32>,
    /// Result of [`ambiguous_pairs`].
    amb: Vec<bool>,
    /// Index of the least-cost alternate pair for each assigned plot, or [`NONE`].
    alternate_pair: Vec<u32>,
    /// Number of tracks plus plots in the plot's connected component, or 0 when unavailable.
    component_size: Vec<u32>,
    /// [`PATH_SINGLE`], [`PATH_EXACT`], [`PATH_GREEDY`] or [`PATH_NONE`].
    component_path: Vec<u8>,
    own: Vec<f64>,
    other: Vec<f64>,
    parent: Vec<u32>,
    /// `(component root, pair index)`, sorted.
    by_root: Vec<(u32, u32)>,
    comp_tracks: Vec<u32>,
    comp_plots: Vec<u32>,
    t_mark: Vec<bool>,
    p_mark: Vec<bool>,
    order: Vec<u32>,
    track_used: Vec<bool>,
    hung: HungBufs,
}

/// A scratch for up to `max_tracks` tracks, `max_plots` plots and `max_pairs` pairs per solve,
/// each limited to [`MAX_SCRATCH_ITEMS`] (pairs [`MAX_SCRATCH_PAIRS`]).
pub fn assoc_scratch_new(max_tracks: usize, max_plots: usize, max_pairs: usize) -> AssocScratch {
    assoc_scratch_new_inner(max_tracks, max_plots, max_pairs, false)
}

/// A scratch with the bounded per-plot metadata used only by the opt-in forensics sidecar.
pub fn assoc_scratch_new_forensics(
    max_tracks: usize,
    max_plots: usize,
    max_pairs: usize,
) -> AssocScratch {
    assoc_scratch_new_inner(max_tracks, max_plots, max_pairs, true)
}

fn assoc_scratch_new_inner(
    max_tracks: usize,
    max_plots: usize,
    max_pairs: usize,
    collect_metadata: bool,
) -> AssocScratch {
    let (max_tracks, max_plots) = (
        max_tracks.min(MAX_SCRATCH_ITEMS),
        max_plots.min(MAX_SCRATCH_ITEMS),
    );
    let max_pairs = max_pairs.min(MAX_SCRATCH_PAIRS);
    let hn = MAX_COMPONENT + 1;
    AssocScratch {
        max_tracks,
        max_plots,
        max_pairs,
        collect_metadata,
        metadata_writes: 0,
        plot_track: vec![NONE; max_plots],
        n_plots: 0,
        pair_of: vec![NONE; max_plots],
        amb: vec![false; max_plots],
        alternate_pair: if collect_metadata {
            vec![NONE; max_plots]
        } else {
            Vec::new()
        },
        component_size: if collect_metadata {
            vec![0; max_plots]
        } else {
            Vec::new()
        },
        component_path: if collect_metadata {
            vec![PATH_NONE; max_plots]
        } else {
            Vec::new()
        },
        own: vec![0.0; max_plots],
        other: vec![0.0; max_plots],
        parent: vec![0; max_tracks + max_plots],
        by_root: Vec::with_capacity(max_pairs),
        comp_tracks: Vec::with_capacity(max_tracks),
        comp_plots: Vec::with_capacity(max_plots),
        t_mark: vec![false; max_tracks],
        p_mark: vec![false; max_plots],
        order: Vec::with_capacity(max_pairs),
        track_used: vec![false; max_tracks],
        hung: HungBufs {
            cost: vec![0.0; MAX_COMPONENT * MAX_COMPONENT],
            u: vec![0.0; hn],
            v: vec![0.0; hn],
            minv: vec![0.0; hn],
            p: vec![0; hn],
            way: vec![0; hn],
            used: vec![false; hn],
            row_to_col: vec![0; MAX_COMPONENT],
        },
    }
}

/// The capacities `(tracks, plots, pairs)` the scratch was built with.
pub fn assoc_scratch_caps(sc: &AssocScratch) -> (usize, usize, usize) {
    (sc.max_tracks, sc.max_plots, sc.max_pairs)
}

/// The address and capacity of every buffer the scratch owns. A test compares it before and
/// after a run: equal means nothing was reallocated (RULESET C5, checked on the buffers because
/// a counting allocator needs `unsafe`, which A0 forbids here).
pub fn assoc_scratch_buffers(sc: &AssocScratch) -> Vec<(usize, usize)> {
    fn of<T>(v: &Vec<T>) -> (usize, usize) {
        (v.as_ptr() as usize, v.capacity())
    }
    vec![
        of(&sc.plot_track),
        of(&sc.pair_of),
        of(&sc.amb),
        of(&sc.alternate_pair),
        of(&sc.component_size),
        of(&sc.component_path),
        of(&sc.own),
        of(&sc.other),
        of(&sc.parent),
        of(&sc.by_root),
        of(&sc.comp_tracks),
        of(&sc.comp_plots),
        of(&sc.t_mark),
        of(&sc.p_mark),
        of(&sc.order),
        of(&sc.track_used),
        of(&sc.hung.cost),
        of(&sc.hung.u),
        of(&sc.hung.v),
        of(&sc.hung.minv),
        of(&sc.hung.p),
        of(&sc.hung.way),
        of(&sc.hung.used),
        of(&sc.hung.row_to_col),
    ]
}

/// The track of each plot (or [`NONE`]) after the last solve; empty for a refused one too large.
pub fn assoc_plot_track(sc: &AssocScratch) -> &[u32] {
    sc.plot_track.get(..sc.n_plots).unwrap_or(&[])
}

/// After [`assigned_pairs`]: the index of the pair each plot took, or [`NONE`].
pub fn assoc_pair_of(sc: &AssocScratch) -> &[u32] {
    sc.pair_of.get(..sc.n_plots).unwrap_or(&[])
}

/// After [`ambiguous_pairs`]: whether each plot is ambiguous.
pub fn assoc_ambiguous(sc: &AssocScratch) -> &[bool] {
    sc.amb.get(..sc.n_plots).unwrap_or(&[])
}

/// After [`ambiguous_pairs`]: the pair index of the least-cost alternate track for each assigned
/// plot, or [`NONE`] if there is no alternate.
pub fn assoc_alternate_pair(sc: &AssocScratch) -> &[u32] {
    sc.alternate_pair.get(..sc.n_plots).unwrap_or(&[])
}

/// Number of tracks plus plots in each plot's connected component; 0 when no component was
/// available (for example, after a refused solve or on the nearest-neighbour path).
pub fn assoc_component_size(sc: &AssocScratch) -> &[u32] {
    sc.component_size.get(..sc.n_plots).unwrap_or(&[])
}

/// Solve path for each plot: [`PATH_SINGLE`], [`PATH_EXACT`], [`PATH_GREEDY`], or [`PATH_NONE`].
pub fn assoc_component_path(sc: &AssocScratch) -> &[u8] {
    sc.component_path.get(..sc.n_plots).unwrap_or(&[])
}

/// Writes to the opt-in, fixed-size metadata columns in the most recent solve.
pub fn assoc_metadata_writes(sc: &AssocScratch) -> u64 {
    sc.metadata_writes
}

/// Sets the result for `n_plots` plots to all unassigned (as far as the scratch holds them).
fn reset_result(sc: &mut AssocScratch, n_plots: usize) {
    sc.n_plots = n_plots.min(sc.max_plots);
    for t in sc.plot_track.iter_mut().take(sc.n_plots) {
        *t = NONE;
    }
    if sc.collect_metadata {
        sc.metadata_writes = sc
            .metadata_writes
            .saturating_add((sc.n_plots as u64).saturating_mul(3));
        for j in 0..sc.n_plots {
            sc.alternate_pair[j] = NONE;
            sc.component_size[j] = 0;
            sc.component_path[j] = PATH_NONE;
        }
    }
}

fn find(parent: &mut [u32], mut x: usize) -> usize {
    while parent[x] as usize != x {
        parent[x] = parent[parent[x] as usize];
        x = parent[x] as usize;
    }
    x
}

/// Relative cost per track index that settles exact ties in the matrix solve (times the gate):
/// far below any difference of two costs that are not equal, far above rounding.
const TIE_BREAK: f64 = 1.0e-10;

/// The exact minimum of the padded square assignment `h.cost[i * n + j]` (`n x n`): for each
/// row the column chosen, in `h.row_to_col`. Hungarian method with potentials (rows and columns
/// are 1-based inside).
fn hungarian(h: &mut HungBufs, n: usize) {
    let inf = f64::INFINITY;
    for k in 0..=n {
        h.u[k] = 0.0;
        h.v[k] = 0.0;
        h.p[k] = 0;
        h.way[k] = 0;
    }
    for i in 1..=n {
        h.p[0] = i;
        let mut j0 = 0usize;
        for k in 0..=n {
            h.minv[k] = inf;
            h.used[k] = false;
        }
        loop {
            h.used[j0] = true;
            let i0 = h.p[j0];
            let mut delta = inf;
            let mut j1 = 0usize;
            for j in 1..=n {
                if h.used[j] {
                    continue;
                }
                let cur = h.cost[(i0 - 1) * n + (j - 1)] - h.u[i0] - h.v[j];
                if cur < h.minv[j] {
                    h.minv[j] = cur;
                    h.way[j] = j0;
                }
                if h.minv[j] < delta {
                    delta = h.minv[j];
                    j1 = j;
                }
            }
            for j in 0..=n {
                if h.used[j] {
                    let pj = h.p[j];
                    h.u[pj] += delta;
                    h.v[j] -= delta;
                } else {
                    h.minv[j] -= delta;
                }
            }
            j0 = j1;
            if h.p[j0] == 0 {
                break;
            }
        }
        loop {
            let j1 = h.way[j0];
            h.p[j0] = h.p[j1];
            j0 = j1;
            if j0 == 0 {
                break;
            }
        }
    }
    for k in 0..n {
        h.row_to_col[k] = 0;
    }
    for j in 1..=n {
        if h.p[j] > 0 {
            h.row_to_col[h.p[j] - 1] = j - 1;
        }
    }
}

/// The objective of an assignment: assigned costs plus `gate / 2` per unassigned track and
/// plot. `plot_track[j]` is the track of plot `j` or [`NONE`]. Used by the tests and by callers
/// that compare two assignments (an offline helper, it allocates).
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
/// `pairs` (see the module documentation). The track of each plot, or [`NONE`], is in
/// [`assoc_plot_track`] afterwards. Pairs with an index out of range or a cost that is not a
/// finite number are ignored. The result depends only on the pairs' contents and order: ties
/// are broken by the lowest track index. A refusal ([`GnnError`]) leaves every plot unassigned.
pub fn gnn(
    sc: &mut AssocScratch,
    n_tracks: usize,
    n_plots: usize,
    pairs: &[Pair],
    gate: f64,
) -> Result<GnnStats, GnnError> {
    let collect_metadata = sc.collect_metadata;
    if collect_metadata {
        sc.metadata_writes = 0;
    }
    reset_result(sc, n_plots);
    if !(gate.is_finite() && gate > 0.0) {
        return Err(GnnError::BadGate);
    }
    if n_tracks > sc.max_tracks || n_plots > sc.max_plots || pairs.len() > sc.max_pairs {
        sc.n_plots = 0;
        return Err(GnnError::TooLarge);
    }
    let mut stats = GnnStats::default();
    let valid = |p: &Pair| {
        (p.track as usize) < n_tracks && (p.plot as usize) < n_plots && p.cost.is_finite()
    };
    if !pairs.iter().any(valid) {
        return Ok(stats);
    }
    // Union-find over tracks (0..n_tracks) and plots (n_tracks..n_tracks + n_plots).
    for (k, e) in sc.parent.iter_mut().take(n_tracks + n_plots).enumerate() {
        *e = k as u32;
    }
    for p in pairs.iter().filter(|p| valid(p)) {
        let a = find(&mut sc.parent, p.track as usize);
        let b = find(&mut sc.parent, n_tracks + p.plot as usize);
        if a != b {
            let (lo, hi) = (a.min(b), a.max(b));
            sc.parent[hi] = lo as u32;
        }
    }
    // Group pairs by component root, in ascending root order for a fixed result.
    let mut by_root = std::mem::take(&mut sc.by_root);
    by_root.clear();
    for (k, p) in pairs.iter().enumerate() {
        if valid(p) {
            by_root.push((find(&mut sc.parent, p.track as usize) as u32, k as u32));
        }
    }
    by_root.sort_unstable();
    let mut at = 0usize;
    while at < by_root.len() {
        let root = by_root[at].0;
        let end = by_root[at..]
            .iter()
            .position(|&(r, _)| r != root)
            .map_or(by_root.len(), |d| at + d);
        stats.components += 1;
        solve_component(
            sc,
            pairs,
            &by_root[at..end],
            gate,
            &mut stats,
            collect_metadata,
        );
        at = end;
    }
    sc.by_root = by_root;
    Ok(stats)
}

/// Solves one component: `group` are `(root, pair index)` entries into `pairs`.
fn solve_component(
    sc: &mut AssocScratch,
    pairs: &[Pair],
    group: &[(u32, u32)],
    gate: f64,
    stats: &mut GnnStats,
    collect_metadata: bool,
) {
    let pr = |e: &(u32, u32)| pairs[e.1 as usize];
    // Local indexes of the component's tracks and plots, ascending.
    sc.comp_tracks.clear();
    sc.comp_plots.clear();
    for e in group {
        let p = pr(e);
        if !sc.t_mark[p.track as usize] {
            sc.t_mark[p.track as usize] = true;
            sc.comp_tracks.push(p.track);
        }
        if !sc.p_mark[p.plot as usize] {
            sc.p_mark[p.plot as usize] = true;
            sc.comp_plots.push(p.plot);
        }
    }
    for &t in &sc.comp_tracks {
        sc.t_mark[t as usize] = false;
    }
    for &q in &sc.comp_plots {
        sc.p_mark[q as usize] = false;
    }
    sc.comp_tracks.sort_unstable();
    sc.comp_plots.sort_unstable();
    let (t, q) = (sc.comp_tracks.len(), sc.comp_plots.len());
    stats.largest = stats.largest.max((t + q) as u64);
    if collect_metadata {
        let path = if group.len() == 1 {
            PATH_SINGLE
        } else if t + q > MAX_COMPONENT {
            PATH_GREEDY
        } else {
            PATH_EXACT
        };
        sc.metadata_writes = sc
            .metadata_writes
            .saturating_add((q as u64).saturating_mul(2));
        for &plot in &sc.comp_plots {
            let j = plot as usize;
            sc.component_size[j] = (t + q) as u32;
            sc.component_path[j] = path;
        }
    }
    if group.len() == 1 {
        stats.single += 1;
        // A single gated pair is below the gate by construction (the caller's gate).
        if let Some(e) = group.first() {
            let p = pr(e);
            if p.cost < gate {
                sc.plot_track[p.plot as usize] = p.track;
            }
        }
        return;
    }
    if t + q > MAX_COMPONENT {
        stats.greedy_components += 1;
        sc.order.clear();
        sc.order.extend(0..group.len() as u32);
        sc.order.sort_unstable_by(|&a, &b| {
            let (pa, pb) = (pr(&group[a as usize]), pr(&group[b as usize]));
            oa_math::cmp_canon_f64(pa.cost, pb.cost)
                .then(pa.track.cmp(&pb.track))
                .then(pa.plot.cmp(&pb.plot))
        });
        for u in sc.track_used.iter_mut().take(t) {
            *u = false;
        }
        for &k in &sc.order {
            let p = pr(&group[k as usize]);
            let ti = sc.comp_tracks.binary_search(&p.track).unwrap_or(0);
            if p.cost < gate && !sc.track_used[ti] && sc.plot_track[p.plot as usize] == NONE {
                sc.track_used[ti] = true;
                sc.plot_track[p.plot as usize] = p.track;
            }
        }
        return;
    }
    stats.solved += 1;
    // Padded square matrix: rows are tracks then plot-dummies, columns are plots then
    // track-dummies.
    let n = t + q;
    let m = &mut sc.hung.cost;
    for c in m.iter_mut().take(n * n) {
        *c = FORBIDDEN;
    }
    for e in group {
        let p = pr(e);
        let (Ok(i), Ok(j)) = (
            sc.comp_tracks.binary_search(&p.track),
            sc.comp_plots.binary_search(&p.plot),
        ) else {
            continue;
        };
        // The same pair twice keeps the cheaper cost. A track pays a hair more for each place
        // after the first in the component, so an exact tie goes to the lowest track index
        // (the padded dummies carry no such term, so every solution pays the same for them).
        let c = &mut m[i * n + j];
        let cost = p.cost + gate * TIE_BREAK * i as f64;
        if cost < *c {
            *c = cost;
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
    hungarian(&mut sc.hung, n);
    for i in 0..t {
        let j = sc.hung.row_to_col[i];
        if j < q && sc.hung.cost[i * n + j] < FORBIDDEN * 0.5 {
            sc.plot_track[sc.comp_plots[j] as usize] = sc.comp_tracks[i];
        }
    }
}

/// Plain nearest neighbour, the baseline: each plot takes its cheapest gated track (the lowest
/// track index on a tie) and a track may take several plots. The track of each plot or
/// [`NONE`] is in [`assoc_plot_track`] afterwards.
pub fn nearest(sc: &mut AssocScratch, n_plots: usize, pairs: &[Pair]) -> Result<(), GnnError> {
    if sc.collect_metadata {
        sc.metadata_writes = 0;
    }
    reset_result(sc, n_plots);
    if n_plots > sc.max_plots {
        sc.n_plots = 0;
        return Err(GnnError::TooLarge);
    }
    if sc.collect_metadata {
        sc.metadata_writes = sc.metadata_writes.saturating_add(n_plots as u64);
        for path in sc.component_path.iter_mut().take(n_plots) {
            *path = PATH_NEAREST;
        }
    }
    for v in sc.own.iter_mut().take(n_plots) {
        *v = f64::INFINITY;
    }
    for p in pairs {
        let j = p.plot as usize;
        if j >= n_plots {
            continue;
        }
        let better = p.cost < sc.own[j] || (p.cost == sc.own[j] && p.track < sc.plot_track[j]);
        if better {
            sc.own[j] = p.cost;
            sc.plot_track[j] = p.track;
        }
    }
    Ok(())
}

/// For each plot, the index in `pairs` of the pair it took (the one whose track is the plot's
/// assigned track), or [`NONE`]; one pass over the pairs. Read it with [`assoc_pair_of`].
pub fn assigned_pairs(sc: &mut AssocScratch, pairs: &[Pair]) {
    let n = sc.n_plots;
    for v in sc.pair_of.iter_mut().take(n) {
        *v = NONE;
    }
    for (k, p) in pairs.iter().enumerate() {
        let j = p.plot as usize;
        if j < n && sc.plot_track[j] == p.track && sc.pair_of[j] == NONE {
            sc.pair_of[j] = k as u32;
        }
    }
}

/// For each plot: its assigned track also has a competitor, another track whose gated cost for
/// this plot is within `margin` of the assigned cost. Unassigned plots are not ambiguous. One
/// pass over the pairs; read it with [`assoc_ambiguous`].
pub fn ambiguous_pairs(sc: &mut AssocScratch, pairs: &[Pair], margin: f64) {
    let n = sc.n_plots;
    for k in 0..n {
        sc.own[k] = f64::INFINITY;
        sc.other[k] = f64::INFINITY;
        sc.amb[k] = false;
    }
    if sc.collect_metadata {
        for k in 0..n {
            sc.alternate_pair[k] = NONE;
        }
    }
    if sc.collect_metadata {
        for (pair_ix, p) in pairs.iter().enumerate() {
            let j = p.plot as usize;
            if j >= n || sc.plot_track[j] == NONE {
                continue;
            }
            if p.track == sc.plot_track[j] {
                sc.own[j] = sc.own[j].min(p.cost);
            } else {
                let prior = sc.other[j];
                sc.other[j] = sc.other[j].min(p.cost);
                let current = sc.alternate_pair[j];
                // Match the existing `f64::min` NaN behavior: NaNs and +infinity do not become
                // alternate evidence. Equal costs use the lowest track, then first pair.
                let better = match pairs.get(current as usize) {
                    Some(prev) => p.cost < prior || (p.cost == prior && p.track < prev.track),
                    None => p.cost < prior,
                };
                if better {
                    sc.alternate_pair[j] = u32::try_from(pair_ix).unwrap_or(NONE);
                }
            }
        }
    } else {
        // Keep the default tracker path identical to the original scoring-only pass.
        for p in pairs {
            let j = p.plot as usize;
            if j >= n || sc.plot_track[j] == NONE {
                continue;
            }
            if p.track == sc.plot_track[j] {
                sc.own[j] = sc.own[j].min(p.cost);
            } else {
                sc.other[j] = sc.other[j].min(p.cost);
            }
        }
    }
    for j in 0..n {
        sc.amb[j] = sc.own[j].is_finite() && sc.other[j] <= sc.own[j] + margin;
    }
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

    /// A solve on a scratch sized for the input: the assignment and the stats.
    fn solve(n_tracks: usize, n_plots: usize, pairs: &[Pair], gate: f64) -> (Vec<u32>, GnnStats) {
        let mut sc = assoc_scratch_new(n_tracks, n_plots, pairs.len());
        let st = gnn(&mut sc, n_tracks, n_plots, pairs, gate).unwrap();
        (assoc_plot_track(&sc).to_vec(), st)
    }

    /// The best objective over every partial matching, by exhaustive search.
    fn brute(pairs: &[Pair], n_tracks: usize, n_plots: usize, gate: f64) -> f64 {
        #[allow(clippy::too_many_arguments)]
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

    // REQ: OA-TRK-151
    #[test]
    fn the_hungarian_branch_breaks_an_exact_cost_tie_by_lowest_track_index() {
        let pr = |track, plot, cost| Pair { track, plot, cost };
        // Two tracks equally good for one plot, in either order of the pairs.
        for pairs in [
            [pr(0, 0, 1.0), pr(1, 0, 1.0)],
            [pr(1, 0, 1.0), pr(0, 0, 1.0)],
        ] {
            let (a, st) = solve(2, 1, &pairs, 10.0);
            assert_eq!(st.solved, 1);
            assert_eq!(a, vec![0]);
        }
        // Three tracks, two plots, every cost equal: the two lowest tracks win, in plot order.
        let mut pairs = Vec::new();
        for t in (0..3).rev() {
            for q in 0..2 {
                pairs.push(pr(t, q, 2.0));
            }
        }
        let (a, st) = solve(3, 2, &pairs, 10.0);
        assert_eq!(st.solved, 1);
        let mut won = a.clone();
        won.sort_unstable();
        assert_eq!(won, vec![0, 1]);
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
            let (out, _) = solve(n_tracks, n_plots, &pairs, gate);
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
        let (g, _) = solve(2, 2, &pairs, 16.0);
        assert_eq!(g, vec![1, 0]);
        let mut sc = assoc_scratch_new(2, 2, pairs.len());
        nearest(&mut sc, 2, &pairs).unwrap();
        // Plain nearest neighbour hands track 0 to both plots.
        assert_eq!(assoc_plot_track(&sc), &[0, 0]);
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
        let (out, st) = solve(n as usize, n as usize, &pairs, 16.0);
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
        let (_, st) = solve(50, 50, &pairs, 16.0);
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
        let (out, _) = solve(2, 2, &pairs, 16.0);
        assert_eq!(out, vec![0, NONE]);
    }

    // REQ: OA-TRK-070
    #[test]
    fn an_invalid_gate_is_refused_with_every_plot_unassigned() {
        // Two plots and two tracks all gated to each other: a component the Hungarian method
        // solves. A NaN, infinite, zero or negative gate must be refused, never solved on.
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
            Pair {
                track: 1,
                plot: 1,
                cost: 2.5,
            },
        ];
        let mut sc = assoc_scratch_new(2, 2, pairs.len());
        for gate in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0.0, -4.0] {
            let r = gnn(&mut sc, 2, 2, &pairs, gate);
            assert_eq!(r, Err(GnnError::BadGate), "gate {gate}");
            assert_eq!(assoc_plot_track(&sc), &[NONE, NONE], "gate {gate}");
        }
        // The same scratch solves a valid gate afterwards.
        assert!(gnn(&mut sc, 2, 2, &pairs, 16.0).is_ok());
        assert_eq!(assoc_plot_track(&sc), &[0, 1]);
    }

    // REQ: OA-TRK-071
    #[test]
    fn a_solve_allocates_nothing_and_refuses_what_exceeds_its_buffers() {
        // A scratch of fixed capacity: its buffers are the same before and after solves of
        // every shape (the greedy, exact and single-pair paths), and an input beyond the
        // capacity is refused and counted by the caller, not grown into.
        let mut sc = assoc_scratch_new(300, 300, 2_000);
        let before = assoc_scratch_buffers(&sc);
        let mut s = 7u64;
        for case in 0..40 {
            let n = 3 + (next(&mut s) * 290.0) as usize;
            let mut pairs = Vec::new();
            for t in 0..n.min(300) {
                pairs.push(Pair {
                    track: t as u32,
                    plot: t as u32,
                    cost: 1.0 + next(&mut s),
                });
                if next(&mut s) < 0.7 {
                    pairs.push(Pair {
                        track: t as u32,
                        plot: ((t + 1) % n) as u32,
                        cost: 4.0 + next(&mut s),
                    });
                }
            }
            gnn(&mut sc, n, n, &pairs, 16.0).unwrap();
            assigned_pairs(&mut sc, &pairs);
            ambiguous_pairs(&mut sc, &pairs, 4.0);
            assert_eq!(assoc_scratch_buffers(&sc), before, "case {case}");
        }
        let pairs = [Pair {
            track: 0,
            plot: 0,
            cost: 1.0,
        }];
        assert_eq!(gnn(&mut sc, 301, 1, &pairs, 16.0), Err(GnnError::TooLarge));
        assert_eq!(gnn(&mut sc, 1, 301, &pairs, 16.0), Err(GnnError::TooLarge));
        let many = vec![pairs[0]; 2_001];
        assert_eq!(gnn(&mut sc, 1, 1, &many, 16.0), Err(GnnError::TooLarge));
        assert!(assoc_plot_track(&sc).is_empty());
        assert_eq!(assoc_scratch_buffers(&sc), before);
    }

    // REQ: OA-TRK-073
    #[test]
    fn a_flooded_bucket_is_associated_in_bounded_time() {
        // 20,000 plots over 20,000 tracks, each plot gated to its own track and the next one: a
        // chain of 40,000 pairs. Every step must be near-linear (the old ambiguity and
        // pair-lookup scans were quadratic: over a billion comparisons).
        let n = 20_000u32;
        let mut pairs = Vec::new();
        for i in 0..n {
            pairs.push(Pair {
                track: i,
                plot: i,
                cost: 1.0,
            });
            pairs.push(Pair {
                track: (i + 1) % n,
                plot: i,
                cost: 2.0,
            });
        }
        let mut sc = assoc_scratch_new(n as usize, n as usize, pairs.len());
        let t0 = std::time::Instant::now();
        gnn(&mut sc, n as usize, n as usize, &pairs, 16.0).unwrap();
        ambiguous_pairs(&mut sc, &pairs, 4.0);
        assigned_pairs(&mut sc, &pairs);
        let took = t0.elapsed();
        assert_eq!(assoc_plot_track(&sc).len(), n as usize);
        assert!(assoc_ambiguous(&sc).iter().all(|&a| a));
        assert!(assoc_pair_of(&sc).iter().all(|&k| k != NONE));
        assert!(took.as_secs_f64() < 3.0, "took {took:?}");
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
        let mut sc = assoc_scratch_new(4, 3, pairs.len());
        gnn(&mut sc, 4, 3, &pairs, 16.0).unwrap();
        // The solver's own assignment is the one the test describes.
        assert_eq!(assoc_plot_track(&sc), &[0, 2, NONE]);
        ambiguous_pairs(&mut sc, &pairs, 4.0);
        assert_eq!(assoc_ambiguous(&sc), &[true, false, false]);
        assigned_pairs(&mut sc, &pairs);
        assert_eq!(assoc_pair_of(&sc), &[0, 2, NONE]);
    }

    // REQ: OA-TRK-036
    #[test]
    fn local_ambiguity_is_not_global_assignment_regret() {
        let regret_without = |pairs: &[Pair], selected: Pair, best: f64, gate: f64| {
            let remaining: Vec<_> = pairs
                .iter()
                .copied()
                .filter(|p| {
                    !(p.track == selected.track
                        && p.plot == selected.plot
                        && p.cost == selected.cost)
                })
                .collect();
            brute(&remaining, 2, 2, gate) - best
        };

        // The two complete matchings cost 11 and 11.1. Excluding either selected edge changes
        // both plot identities for only 0.1 more total cost, but the local rule marks only plot
        // 1: plot 0's other edge is nine cost units above its selected edge.
        let under = [
            Pair {
                track: 0,
                plot: 0,
                cost: 1.0,
            },
            Pair {
                track: 0,
                plot: 1,
                cost: 1.1,
            },
            Pair {
                track: 1,
                plot: 0,
                cost: 10.0,
            },
            Pair {
                track: 1,
                plot: 1,
                cost: 10.0,
            },
        ];
        let mut sc = assoc_scratch_new(2, 2, under.len());
        gnn(&mut sc, 2, 2, &under, 16.0).unwrap();
        assert_eq!(assoc_plot_track(&sc), &[0, 1]);
        ambiguous_pairs(&mut sc, &under, 4.0);
        assert_eq!(assoc_ambiguous(&sc), &[false, true]);
        let best = objective(&under, 2, assoc_plot_track(&sc), 16.0);
        let regret = regret_without(&under, under[0], best, 16.0);
        assert!((regret - 0.1).abs() < 1.0e-9, "regret {regret}");

        // Here plot 0 has a close local competitor (1.0 versus 1.5), but using it forces the
        // other plot onto the 9.9 edge. The whole alternate matching costs 10.4 more. A local
        // ambiguity flag therefore also cannot be read as low whole-matching regret.
        let over = [
            Pair {
                track: 0,
                plot: 0,
                cost: 1.0,
            },
            Pair {
                track: 0,
                plot: 1,
                cost: 9.9,
            },
            Pair {
                track: 1,
                plot: 0,
                cost: 1.5,
            },
            Pair {
                track: 1,
                plot: 1,
                cost: 0.0,
            },
        ];
        let mut sc = assoc_scratch_new(2, 2, over.len());
        gnn(&mut sc, 2, 2, &over, 16.0).unwrap();
        assert_eq!(assoc_plot_track(&sc), &[0, 1]);
        ambiguous_pairs(&mut sc, &over, 4.0);
        assert_eq!(assoc_ambiguous(&sc), &[true, false]);
        let best = objective(&over, 2, assoc_plot_track(&sc), 16.0);
        let regret = regret_without(&over, over[0], best, 16.0);
        assert!((regret - 10.4).abs() < 1.0e-9, "regret {regret}");
    }

    // REQ: OA-TRK-264
    #[test]
    fn the_greedy_order_of_a_large_component_does_not_depend_on_the_sign_of_a_nan_cost() {
        // 100 tracks and 100 plots (over MAX_COMPONENT together): plot j has a finite pair to track
        // j and a NaN pair to track j + 1. The raw `total_cmp` put a negative NaN first and a
        // positive one last, so the two signs took different tracks.
        let n = 100usize;
        let solved = |nan: f64| {
            let mut pairs = Vec::new();
            for j in 0..n as u32 {
                pairs.push(Pair {
                    track: j,
                    plot: j,
                    cost: 1.0 + f64::from(j),
                });
                pairs.push(Pair {
                    track: (j + 1) % n as u32,
                    plot: j,
                    cost: nan,
                });
            }
            let mut sc = assoc_scratch_new(n, n, pairs.len());
            reset_result(&mut sc, n);
            let group: Vec<(u32, u32)> = (0..pairs.len() as u32).map(|k| (0, k)).collect();
            let mut st = GnnStats::default();
            solve_component(&mut sc, &pairs, &group, 1.0e9, &mut st, false);
            assert_eq!(st.greedy_components, 1);
            assoc_plot_track(&sc).to_vec()
        };
        let pos = solved(f64::from_bits(0x7ff8_0000_0000_0000));
        let neg = solved(f64::from_bits(0xfff8_0000_0000_0000));
        let payload = solved(f64::from_bits(0xfff8_0000_0000_1234));
        assert_eq!(pos, neg);
        assert_eq!(pos, payload);
        // the finite pairs came first: every plot took its own track
        assert!(
            pos.iter().enumerate().all(|(j, &t)| t as usize == j),
            "{pos:?}"
        );
    }

    #[test]
    fn forensics_metadata_preserves_assignments_and_classifies_paths() {
        let pairs = [
            Pair {
                track: 0,
                plot: 0,
                cost: 1.0,
            },
            Pair {
                track: 1,
                plot: 0,
                cost: 2.0,
            },
            Pair {
                track: 1,
                plot: 1,
                cost: 1.0,
            },
        ];
        let mut plain = assoc_scratch_new(2, 2, pairs.len());
        let mut forensic = assoc_scratch_new_forensics(2, 2, pairs.len());
        gnn(&mut plain, 2, 2, &pairs, 10.0).unwrap();
        gnn(&mut forensic, 2, 2, &pairs, 10.0).unwrap();
        assert_eq!(assoc_plot_track(&plain), assoc_plot_track(&forensic));
        assert_eq!(assoc_component_path(&forensic), &[PATH_EXACT, PATH_EXACT]);
        assert_eq!(assoc_component_size(&forensic), &[4, 4]);

        let single = [Pair {
            track: 0,
            plot: 0,
            cost: 1.0,
        }];
        gnn(&mut forensic, 1, 1, &single, 10.0).unwrap();
        assert_eq!(assoc_component_path(&forensic), &[PATH_SINGLE]);

        let n = MAX_COMPONENT / 2 + 1;
        let dense: Vec<Pair> = (0..n)
            .flat_map(|track| {
                (0..n).map(move |plot| Pair {
                    track: track as u32,
                    plot: plot as u32,
                    cost: 1.0,
                })
            })
            .collect();
        let mut greedy = assoc_scratch_new_forensics(n, n, dense.len());
        gnn(&mut greedy, n, n, &dense, 10.0).unwrap();
        assert!(assoc_component_path(&greedy)
            .iter()
            .all(|&p| p == PATH_GREEDY));
        assert!(assoc_component_size(&greedy)
            .iter()
            .all(|&size| size == (2 * n) as u32));
    }

    #[test]
    fn alternate_evidence_keeps_nan_minimum_semantics_and_stable_ties() {
        for nan in [
            f64::from_bits(0x7ff8_0000_0000_0001),
            f64::from_bits(0xfff8_0000_0000_0001),
        ] {
            let pairs = [
                Pair {
                    track: 0,
                    plot: 0,
                    cost: 1.0,
                },
                Pair {
                    track: 2,
                    plot: 0,
                    cost: nan,
                },
                Pair {
                    track: 1,
                    plot: 0,
                    cost: 1.1,
                },
                Pair {
                    track: 3,
                    plot: 0,
                    cost: 1.1,
                },
            ];
            let mut plain = assoc_scratch_new(4, 1, pairs.len());
            let mut sc = assoc_scratch_new_forensics(4, 1, pairs.len());
            gnn(&mut plain, 4, 1, &pairs, 10.0).unwrap();
            gnn(&mut sc, 4, 1, &pairs, 10.0).unwrap();
            assert_eq!(assoc_plot_track(&plain), assoc_plot_track(&sc));
            assigned_pairs(&mut plain, &pairs);
            assigned_pairs(&mut sc, &pairs);
            ambiguous_pairs(&mut plain, &pairs, 0.2);
            ambiguous_pairs(&mut sc, &pairs, 0.2);
            assert_eq!(assoc_ambiguous(&plain), assoc_ambiguous(&sc));
            assert_eq!(assoc_ambiguous(&sc), &[true]);
            let alt = assoc_alternate_pair(&sc)[0] as usize;
            assert_eq!(pairs[alt].track, 1);
            assert_eq!(pairs[alt].cost, 1.1);
        }
    }

    #[test]
    fn forensic_nearest_path_is_explicit_and_assignments_match_plain_scratch() {
        let pairs = [
            Pair {
                track: 1,
                plot: 0,
                cost: 1.0,
            },
            Pair {
                track: 0,
                plot: 0,
                cost: 1.0,
            },
            Pair {
                track: 2,
                plot: 1,
                cost: 2.0,
            },
        ];
        let mut plain = assoc_scratch_new(3, 3, pairs.len());
        let mut forensic = assoc_scratch_new_forensics(3, 3, pairs.len());
        nearest(&mut plain, 3, &pairs).unwrap();
        nearest(&mut forensic, 3, &pairs).unwrap();
        assert_eq!(assoc_plot_track(&plain), assoc_plot_track(&forensic));
        assert_eq!(assoc_plot_track(&forensic), &[0, 2, NONE]);
        assert_eq!(
            assoc_component_path(&forensic),
            &[PATH_NEAREST, PATH_NEAREST, PATH_NEAREST]
        );
        assert_eq!(assoc_component_size(&forensic), &[0, 0, 0]);
    }

    #[test]
    fn ordinary_scratch_has_no_metadata_buffers() {
        let sc = assoc_scratch_new(4, 5, 9);
        assert!(assoc_alternate_pair(&sc).is_empty());
        assert!(assoc_component_size(&sc).is_empty());
        assert!(assoc_component_path(&sc).is_empty());
        assert_eq!(sc.alternate_pair.capacity(), 0);
        assert_eq!(sc.component_size.capacity(), 0);
        assert_eq!(sc.component_path.capacity(), 0);
    }
}
