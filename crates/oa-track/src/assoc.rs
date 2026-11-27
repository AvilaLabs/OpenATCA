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
/// No selected assignment has regret evidence (including an unassigned plot).
pub const REGRET_NONE: u8 = 0;
/// Exact edge-exclusion regret from the padded Hungarian objective.
pub const REGRET_EXACT: u8 = 1;
/// Analytic edge-exclusion regret for a one-pair component.
pub const REGRET_SINGLE: u8 = 2;
/// Regret is unavailable because the bounded greedy solver was used.
pub const REGRET_GREEDY: u8 = 3;
/// Regret is unavailable because the per-solve work allowance was exhausted.
pub const REGRET_BUDGET: u8 = 4;
/// Regret is unavailable because inputs are outside its certified numeric domain.
pub const REGRET_UNSUPPORTED: u8 = 5;
/// Regret is unavailable because the floating-point optimality checks failed.
pub const REGRET_NUMERICAL: u8 = 6;
/// Regret is unavailable because the nearest-neighbour solver was used.
pub const REGRET_NOT_GNN: u8 = 7;
/// A selected decision was outside the measured radar-GNN path.
pub const REGRET_OUT_OF_SCOPE: u8 = 8;
/// Limit: the largest component (tracks plus plots) solved exactly; 160 gives a padded matrix of
/// 160 rows and columns, about 4e6 operations worst case. Larger components are assigned
/// greedily and counted.
pub const MAX_COMPONENT: usize = 160;
/// Maximum charged regret work per forensic solve. The allowance covers one maximum exact
/// component's `n^3 + 3n^2 + 5n` regret bound plus one full `MAX_SCRATCH_PAIRS` validation pass.
/// Components are precharged by that formula plus their input pair count, so repeated small
/// components remain bounded by this aggregate allowance too.
pub const MAX_REGRET_WORK: usize = MAX_COMPONENT * MAX_COMPONENT * MAX_COMPONENT
    + 3 * MAX_COMPONENT * MAX_COMPONENT
    + 5 * MAX_COMPONENT
    + MAX_SCRATCH_PAIRS;
/// Limit: tracks or plots a scratch is built for; a larger request is built at this size (a solve
/// asking for more is refused as too large).
pub const MAX_SCRATCH_ITEMS: usize = 1 << 22;
/// Limit: pairs a scratch is built for, the same way.
pub const MAX_SCRATCH_PAIRS: usize = 1 << 22;
/// Cost standing for "forbidden" inside the padded matrix; every real cost is far below it.
const FORBIDDEN: f64 = 1.0e12;
/// A conservative forensic-only ceiling: even a full padded objective at this gate is below
/// one quarter of the forbidden sentinel, leaving headroom for the row tie surcharge.
const MAX_REGRET_GATE: f64 = FORBIDDEN / (4.0 * MAX_COMPONENT as f64);

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
    /// Forensic regret was requested from scratch that was not created for forensics.
    ForensicsDisabled,
}

impl std::fmt::Display for GnnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GnnError::BadGate => write!(f, "association gate is not a finite positive number"),
            GnnError::TooLarge => write!(f, "association input exceeds the scratch capacity"),
            GnnError::ForensicsDisabled => {
                write!(f, "association regret requires forensic scratch")
            }
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
    /// Edge-exclusion objective increase for each selected plot; consult `regret_status` first.
    regret: Vec<f64>,
    /// One of the `REGRET_*` states for each selected plot.
    regret_status: Vec<u8>,
    /// Worst-case work units precharged for the most recent forensic solve.
    regret_work: usize,
    /// Fixed `MAX_COMPONENT` square scratch used only by forensic solves.
    regret_dist: Vec<f64>,
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
        regret: if collect_metadata {
            vec![0.0; max_plots]
        } else {
            Vec::new()
        },
        regret_status: if collect_metadata {
            vec![REGRET_NONE; max_plots]
        } else {
            Vec::new()
        },
        regret_work: 0,
        regret_dist: if collect_metadata {
            vec![f64::INFINITY; MAX_COMPONENT * MAX_COMPONENT]
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

/// Exact heap bytes reserved by this scratch's fixed-capacity vectors. Used by capacity
/// accounting; it does not include the inline `AssocScratch` value or allocator metadata.
pub fn assoc_scratch_storage_bytes(sc: &AssocScratch) -> u64 {
    fn bytes<T>(capacity: usize) -> u128 {
        (capacity as u128) * (core::mem::size_of::<T>() as u128)
    }
    let total = bytes::<u32>(sc.plot_track.capacity())
        + bytes::<u32>(sc.pair_of.capacity())
        + bytes::<bool>(sc.amb.capacity())
        + bytes::<u32>(sc.alternate_pair.capacity())
        + bytes::<u32>(sc.component_size.capacity())
        + bytes::<u8>(sc.component_path.capacity())
        + bytes::<f64>(sc.regret.capacity())
        + bytes::<u8>(sc.regret_status.capacity())
        + bytes::<f64>(sc.regret_dist.capacity())
        + bytes::<f64>(sc.own.capacity())
        + bytes::<f64>(sc.other.capacity())
        + bytes::<u32>(sc.parent.capacity())
        + bytes::<(u32, u32)>(sc.by_root.capacity())
        + bytes::<u32>(sc.comp_tracks.capacity())
        + bytes::<u32>(sc.comp_plots.capacity())
        + bytes::<bool>(sc.t_mark.capacity())
        + bytes::<bool>(sc.p_mark.capacity())
        + bytes::<u32>(sc.order.capacity())
        + bytes::<bool>(sc.track_used.capacity())
        + bytes::<f64>(sc.hung.cost.capacity())
        + bytes::<f64>(sc.hung.u.capacity())
        + bytes::<f64>(sc.hung.v.capacity())
        + bytes::<f64>(sc.hung.minv.capacity())
        + bytes::<usize>(sc.hung.p.capacity())
        + bytes::<usize>(sc.hung.way.capacity())
        + bytes::<bool>(sc.hung.used.capacity())
        + bytes::<usize>(sc.hung.row_to_col.capacity());
    u64::try_from(total).unwrap_or(u64::MAX)
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
        of(&sc.regret),
        of(&sc.regret_status),
        of(&sc.regret_dist),
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

/// Per-plot edge-exclusion objective increase. Read only entries whose status is exact or single.
pub fn assoc_regret(sc: &AssocScratch) -> &[f64] {
    sc.regret.get(..sc.n_plots).unwrap_or(&[])
}

/// Per-plot regret availability; unavailable values are never represented as a numeric sentinel.
pub fn assoc_regret_status(sc: &AssocScratch) -> &[u8] {
    sc.regret_status.get(..sc.n_plots).unwrap_or(&[])
}

/// Worst-case work units precharged during the most recent forensic GNN solve.
pub fn assoc_regret_work(sc: &AssocScratch) -> usize {
    sc.regret_work
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
            .saturating_add((sc.n_plots as u64).saturating_mul(5));
        for j in 0..sc.n_plots {
            sc.alternate_pair[j] = NONE;
            sc.component_size[j] = 0;
            sc.component_path[j] = PATH_NONE;
            sc.regret[j] = 0.0;
            sc.regret_status[j] = REGRET_NONE;
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
    gnn_inner(sc, n_tracks, n_plots, pairs, gate, false, 0)
}

/// Global nearest-neighbour association with opt-in, label-free edge-exclusion regret metadata.
/// The scratch must have been created by [`assoc_scratch_new_forensics`]. Assignments are
/// computed by the same solver and with the same costs as [`gnn`].
pub fn gnn_forensics(
    sc: &mut AssocScratch,
    n_tracks: usize,
    n_plots: usize,
    pairs: &[Pair],
    gate: f64,
) -> Result<GnnStats, GnnError> {
    if !sc.collect_metadata {
        sc.regret_work = 0;
        reset_result(sc, n_plots);
        return Err(GnnError::ForensicsDisabled);
    }
    gnn_inner(sc, n_tracks, n_plots, pairs, gate, true, MAX_REGRET_WORK)
}

fn gnn_inner(
    sc: &mut AssocScratch,
    n_tracks: usize,
    n_plots: usize,
    pairs: &[Pair],
    gate: f64,
    compute_regret: bool,
    regret_budget: usize,
) -> Result<GnnStats, GnnError> {
    let collect_metadata = sc.collect_metadata;
    if collect_metadata {
        sc.metadata_writes = 0;
    }
    sc.regret_work = 0;
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
    let mut regret_budget_left = regret_budget;
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
            compute_regret,
            &mut regret_budget_left,
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
    compute_regret: bool,
    regret_budget_left: &mut usize,
) {
    let collect_metadata = sc.collect_metadata;
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
                if compute_regret {
                    let plot = p.plot as usize;
                    let budget_ok = precharge_regret_work(1, regret_budget_left);
                    if budget_ok {
                        sc.regret_work = sc.regret_work.saturating_add(1);
                    }
                    if !budget_ok {
                        sc.regret_status[plot] = REGRET_BUDGET;
                    } else if regret_component_supported(group, pairs, gate) {
                        sc.regret[plot] = gate - p.cost;
                        sc.regret_status[plot] = REGRET_SINGLE;
                    } else {
                        sc.regret_status[plot] = REGRET_UNSUPPORTED;
                    }
                    sc.metadata_writes = sc.metadata_writes.saturating_add(2);
                }
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
                if compute_regret {
                    sc.regret_status[p.plot as usize] = REGRET_GREEDY;
                    sc.metadata_writes = sc.metadata_writes.saturating_add(1);
                }
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
    let regret_state = if compute_regret {
        if let Some(required) = regret_work_bound(n, group.len()) {
            if !precharge_regret_work(required, regret_budget_left) {
                REGRET_BUDGET
            } else {
                sc.regret_work = sc.regret_work.saturating_add(required);
                if !regret_component_supported(group, pairs, gate) {
                    REGRET_UNSUPPORTED
                } else {
                    match exact_component_regret(sc, n, t, q) {
                        Ok(()) => REGRET_EXACT,
                        Err(()) => REGRET_NUMERICAL,
                    }
                }
            }
        } else {
            REGRET_UNSUPPORTED
        }
    } else {
        REGRET_NONE
    };
    for i in 0..t {
        let j = sc.hung.row_to_col[i];
        if j < q && sc.hung.cost[i * n + j] < FORBIDDEN * 0.5 {
            let plot = sc.comp_plots[j] as usize;
            sc.plot_track[plot] = sc.comp_tracks[i];
            if compute_regret {
                if regret_state == REGRET_EXACT {
                    sc.regret_status[plot] = REGRET_EXACT;
                } else {
                    sc.regret_status[plot] = regret_state;
                }
                sc.metadata_writes = sc.metadata_writes.saturating_add(1);
            }
        }
    }
}

fn regret_work_bound(n: usize, pair_count: usize) -> Option<usize> {
    let n2 = n.checked_mul(n)?;
    let n3 = n2.checked_mul(n)?;
    n3.checked_add(n2.checked_mul(3)?)?
        .checked_add(n.checked_mul(5)?)?
        .checked_add(pair_count)
}

fn precharge_regret_work(required: usize, budget_left: &mut usize) -> bool {
    if required > *budget_left {
        return false;
    }
    *budget_left -= required;
    true
}

/// The diagnostic currently certifies only the bounded, nonnegative costs emitted by fusion.
/// The public GNN solver still accepts other finite inputs and continues to assign them normally.
fn regret_component_supported(group: &[(u32, u32)], pairs: &[Pair], gate: f64) -> bool {
    gate.is_finite()
        && gate > 0.0
        && gate <= MAX_REGRET_GATE
        && group.iter().all(|&(_, pair_ix)| {
            pairs
                .get(pair_ix as usize)
                .is_some_and(|pair| pair.cost.is_finite() && pair.cost >= 0.0 && pair.cost < gate)
        })
}

/// Computes selected real-edge exclusion deltas against the exact padded matrix held by the
/// Hungarian solver, including its tie surcharge. `own` stages outputs so a numerical refusal
/// never publishes only some rows from one component.
fn exact_component_regret(
    sc: &mut AssocScratch,
    n: usize,
    n_tracks: usize,
    n_plots: usize,
) -> Result<(), ()> {
    let cells = n.checked_mul(n).ok_or(())?;
    if n == 0 || n > MAX_COMPONENT || cells > sc.regret_dist.len() {
        return Err(());
    }
    let used = sc.hung.used.get_mut(..n).ok_or(())?;
    used.fill(false);
    let matching = sc.hung.row_to_col.get(..n).ok_or(())?;
    for &column in matching.iter() {
        if column >= n || used[column] {
            return Err(());
        }
        used[column] = true;
    }
    let matrix = &sc.hung.cost;
    let distance = sc.regret_dist.get_mut(..cells).ok_or(())?;
    distance.fill(f64::INFINITY);

    for row in 0..n {
        let own = matrix[row * n + matching[row]];
        if !own.is_finite() || own >= FORBIDDEN * 0.5 {
            return Err(());
        }
        distance[row * n + row] = 0.0;
        for target in 0..n {
            if target == row {
                continue;
            }
            let cost = matrix[row * n + matching[target]];
            if cost < FORBIDDEN * 0.5 {
                if !cost.is_finite() {
                    return Err(());
                }
                let delta = cost - own;
                if !delta.is_finite() {
                    return Err(());
                }
                distance[row * n + target] = delta;
            }
        }
    }

    for mid in 0..n {
        for from in 0..n {
            let left = distance[from * n + mid];
            if !left.is_finite() {
                continue;
            }
            for to in 0..n {
                let right = distance[mid * n + to];
                if !right.is_finite() {
                    continue;
                }
                let through = left + right;
                if !through.is_finite() {
                    return Err(());
                }
                if through < distance[from * n + to] {
                    distance[from * n + to] = through;
                }
            }
        }
    }
    for row in 0..n {
        if distance[row * n + row] < 0.0 {
            return Err(());
        }
    }

    for row in 0..n_tracks {
        let selected_column = matching[row];
        if selected_column >= n_plots {
            continue;
        }
        let own = matrix[row * n + selected_column];
        let mut best = f64::INFINITY;
        for next in 0..n {
            if next == row {
                continue;
            }
            let next_column = matching[next];
            let edge_cost = matrix[row * n + next_column];
            if edge_cost >= FORBIDDEN * 0.5 {
                continue;
            }
            if !edge_cost.is_finite() {
                return Err(());
            }
            let return_path = distance[next * n + row];
            if !return_path.is_finite() {
                continue;
            }
            let cycle = (edge_cost - own) + return_path;
            if !cycle.is_finite() || cycle < 0.0 {
                return Err(());
            }
            if cycle < best {
                best = cycle;
            }
        }
        if !best.is_finite() {
            return Err(());
        }
        let plot = sc.comp_plots[selected_column] as usize;
        let Some(slot) = sc.own.get_mut(plot) else {
            return Err(());
        };
        *slot = best;
    }

    for row in 0..n_tracks {
        let selected_column = matching[row];
        if selected_column >= n_plots {
            continue;
        }
        let plot = sc.comp_plots[selected_column] as usize;
        let Some(value) = sc.own.get(plot).copied() else {
            return Err(());
        };
        let Some(slot) = sc.regret.get_mut(plot) else {
            return Err(());
        };
        *slot = value;
        sc.metadata_writes = sc.metadata_writes.saturating_add(1);
    }
    Ok(())
}

/// Plain nearest neighbour, the baseline: each plot takes its cheapest gated track (the lowest
/// track index on a tie) and a track may take several plots. The track of each plot or
/// [`NONE`] is in [`assoc_plot_track`] afterwards.
pub fn nearest(sc: &mut AssocScratch, n_plots: usize, pairs: &[Pair]) -> Result<(), GnnError> {
    if sc.collect_metadata {
        sc.metadata_writes = 0;
    }
    sc.regret_work = 0;
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
            if sc.collect_metadata {
                sc.regret_status[j] = REGRET_NOT_GNN;
                sc.metadata_writes = sc.metadata_writes.saturating_add(1);
            }
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

    /// Exhaustive objective over the same real-edge tie surcharge used by the padded solver.
    fn brute_perturbed_excluding(
        pairs: &[Pair],
        track_ids: &[u32],
        plot_ids: &[u32],
        gate: f64,
        excluded: Option<(u32, u32)>,
    ) -> f64 {
        #[allow(clippy::too_many_arguments)]
        fn visit(
            plot: usize,
            pairs: &[Pair],
            used: &mut [bool],
            assigned: usize,
            sum: f64,
            track_ids: &[u32],
            plot_ids: &[u32],
            gate: f64,
            excluded: Option<(u32, u32)>,
            best: &mut f64,
        ) {
            if plot == plot_ids.len() {
                let unmatched = (track_ids.len() - assigned) + (plot_ids.len() - assigned);
                *best = best.min(sum + 0.5 * gate * unmatched as f64);
                return;
            }
            visit(
                plot + 1,
                pairs,
                used,
                assigned,
                sum,
                track_ids,
                plot_ids,
                gate,
                excluded,
                best,
            );
            for pair in pairs.iter().filter(|pair| {
                pair.plot == plot_ids[plot] && excluded != Some((pair.track, pair.plot))
            }) {
                let Ok(track) = track_ids.binary_search(&pair.track) else {
                    continue;
                };
                if used[track] {
                    continue;
                }
                used[track] = true;
                visit(
                    plot + 1,
                    pairs,
                    used,
                    assigned + 1,
                    sum + pair.cost + gate * TIE_BREAK * track as f64,
                    track_ids,
                    plot_ids,
                    gate,
                    excluded,
                    best,
                );
                used[track] = false;
            }
        }
        let mut best = f64::INFINITY;
        visit(
            0,
            pairs,
            &mut vec![false; track_ids.len()],
            0,
            0.0,
            track_ids,
            plot_ids,
            gate,
            excluded,
            &mut best,
        );
        best
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
            let mut regret_budget_left = MAX_REGRET_WORK;
            solve_component(
                &mut sc,
                &pairs,
                &group,
                1.0e9,
                &mut st,
                false,
                &mut regret_budget_left,
            );
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
        assert_eq!(sc.regret.capacity(), 0);
        assert_eq!(sc.regret_status.capacity(), 0);
        assert_eq!(sc.regret_dist.capacity(), 0);
    }

    #[test]
    fn forensic_regret_matches_exhaustive_perturbed_objective_including_raw_worse_choice() {
        let gate = 10.0;
        let track_ids = [2, 7];
        let plot_ids = [0];
        let pairs = [
            Pair {
                track: 2,
                plot: 0,
                cost: 1.0 + 0.5e-9,
            },
            Pair {
                track: 7,
                plot: 0,
                cost: 1.0,
            },
        ];
        let mut sc = assoc_scratch_new_forensics(8, 1, pairs.len());
        gnn_forensics(&mut sc, 8, 1, &pairs, gate).unwrap();
        let assigned = assoc_plot_track(&sc)[0];
        assert_eq!(
            assigned, 2,
            "local row surcharge intentionally picks raw-worse edge"
        );
        assert_eq!(assoc_regret_status(&sc), &[REGRET_EXACT]);
        let best = brute_perturbed_excluding(&pairs, &track_ids, &plot_ids, gate, None);
        let alternative =
            brute_perturbed_excluding(&pairs, &track_ids, &plot_ids, gate, Some((assigned, 0)));
        let expected = alternative - best;
        assert!((assoc_regret(&sc)[0] - expected).abs() < 1.0e-12);
        assert!(expected > 0.0 && expected < 1.0e-9);
    }

    #[test]
    fn forensic_regret_includes_unmatched_alternatives_and_duplicate_minimum() {
        let gate = 10.0;
        let pairs = [
            Pair {
                track: 0,
                plot: 0,
                cost: 1.0,
            },
            Pair {
                track: 0,
                plot: 0,
                cost: 3.0,
            },
            Pair {
                track: 1,
                plot: 1,
                cost: 2.0,
            },
        ];
        let mut sc = assoc_scratch_new_forensics(2, 2, pairs.len());
        gnn_forensics(&mut sc, 2, 2, &pairs, gate).unwrap();
        assert_eq!(assoc_plot_track(&sc), &[0, 1]);
        for plot in 0..2 {
            let track = assoc_plot_track(&sc)[plot];
            let component_pairs: Vec<Pair> = pairs
                .iter()
                .copied()
                .filter(|pair| pair.track == track && pair.plot == plot as u32)
                .collect();
            let track_ids = [track];
            let plot_ids = [plot as u32];
            let expected =
                brute_perturbed_excluding(
                    &component_pairs,
                    &track_ids,
                    &plot_ids,
                    gate,
                    Some((track, plot as u32)),
                ) - brute_perturbed_excluding(&component_pairs, &track_ids, &plot_ids, gate, None);
            assert_eq!(
                assoc_regret_status(&sc)[plot],
                if plot == 0 {
                    REGRET_EXACT
                } else {
                    REGRET_SINGLE
                }
            );
            assert!((assoc_regret(&sc)[plot] - expected).abs() < 1.0e-12);
        }
        assert!(assoc_regret(&sc)[0] > 0.0);
    }

    #[test]
    fn seeded_small_graphs_match_the_perturbed_exhaustive_oracle() {
        let mut seed = 0x7ac3_61d9_0b25_ef47u64;
        for case in 0..48 {
            let n_tracks = 1 + (next(&mut seed) * 3.0) as usize;
            let n_plots = 1 + (next(&mut seed) * 3.0) as usize;
            let mut pairs = Vec::new();
            for track in 0..n_tracks {
                for plot in 0..n_plots {
                    let cost = 0.25 * f64::from(1 + (next(&mut seed) * 20.0) as u32);
                    pairs.push(Pair {
                        track: track as u32,
                        plot: plot as u32,
                        cost,
                    });
                    if next(&mut seed) < 0.2 {
                        pairs.push(Pair {
                            track: track as u32,
                            plot: plot as u32,
                            cost: cost + 0.25,
                        });
                    }
                }
            }
            let track_ids: Vec<u32> = (0..n_tracks as u32).collect();
            let plot_ids: Vec<u32> = (0..n_plots as u32).collect();
            let mut sc = assoc_scratch_new_forensics(n_tracks, n_plots, pairs.len());
            gnn_forensics(&mut sc, n_tracks, n_plots, &pairs, 10.0).unwrap();
            for (plot, &track) in assoc_plot_track(&sc).iter().enumerate() {
                if track == NONE {
                    continue;
                }
                let best = brute_perturbed_excluding(&pairs, &track_ids, &plot_ids, 10.0, None);
                let alternative = brute_perturbed_excluding(
                    &pairs,
                    &track_ids,
                    &plot_ids,
                    10.0,
                    Some((track, plot as u32)),
                );
                assert!(
                    matches!(assoc_regret_status(&sc)[plot], REGRET_EXACT | REGRET_SINGLE),
                    "case {case}, plot {plot}: status {}",
                    assoc_regret_status(&sc)[plot]
                );
                let expected = alternative - best;
                assert!(
                    (assoc_regret(&sc)[plot] - expected).abs() < 1.0e-9,
                    "case {case}, plot {plot}: got {}, oracle {expected}",
                    assoc_regret(&sc)[plot]
                );
            }
        }
    }

    #[test]
    fn singleton_and_exact_work_boundary_are_explicit() {
        let pair = Pair {
            track: 0,
            plot: 0,
            cost: 1.25,
        };
        let mut one = assoc_scratch_new_forensics(1, 1, 1);
        gnn_forensics(&mut one, 1, 1, &[pair], 10.0).unwrap();
        assert_eq!(assoc_regret_status(&one), &[REGRET_SINGLE]);
        assert_eq!(assoc_regret(&one), &[8.75]);

        assert_eq!(
            MAX_REGRET_WORK,
            MAX_COMPONENT * MAX_COMPONENT * MAX_COMPONENT
                + 3 * MAX_COMPONENT * MAX_COMPONENT
                + 5 * MAX_COMPONENT
                + MAX_SCRATCH_PAIRS
        );
        let required = regret_work_bound(MAX_COMPONENT, 6_400).unwrap();
        assert!(required <= MAX_REGRET_WORK);
        let mut exactly = required;
        assert!(precharge_regret_work(required, &mut exactly));
        assert_eq!(exactly, 0);
        let mut one_short = required - 1;
        assert!(!precharge_regret_work(required, &mut one_short));
        assert_eq!(one_short, required - 1);
        let n = MAX_COMPONENT / 2;
        let dense: Vec<Pair> = (0..n)
            .flat_map(|track| {
                (0..n).map(move |plot| Pair {
                    track: track as u32,
                    plot: plot as u32,
                    cost: 1.0,
                })
            })
            .collect();
        let mut exact = assoc_scratch_new_forensics(n, n, dense.len());
        gnn_forensics(&mut exact, n, n, &dense, 10.0).unwrap();
        assert!(assoc_regret_status(&exact)
            .iter()
            .all(|&status| status == REGRET_EXACT));
        assert_eq!(assoc_regret_work(&exact), required);
    }

    #[test]
    fn aggregate_regret_budget_marks_whole_later_component_unavailable() {
        let side = 65;
        let components = 4;
        let pairs: Vec<Pair> = (0..components * side)
            .flat_map(|track| {
                let component = track / side;
                let first_plot = component * side;
                (first_plot..first_plot + side).map(move |plot| Pair {
                    track: track as u32,
                    plot: plot as u32,
                    cost: 1.0,
                })
            })
            .collect();
        let item_count = components * side;
        let mut sc = assoc_scratch_new_forensics(item_count, item_count, pairs.len());
        let mut plain = assoc_scratch_new(item_count, item_count, pairs.len());
        gnn(&mut plain, item_count, item_count, &pairs, 10.0).unwrap();
        gnn_forensics(&mut sc, item_count, item_count, &pairs, 10.0).unwrap();
        assert_eq!(assoc_plot_track(&plain), assoc_plot_track(&sc));
        assert!(assoc_regret_status(&sc)[..3 * side]
            .iter()
            .all(|&status| status == REGRET_EXACT));
        assert!(assoc_regret_status(&sc)[3 * side..]
            .iter()
            .all(|&status| status == REGRET_BUDGET));
        assert!(assoc_regret_work(&sc) <= MAX_REGRET_WORK);
    }

    #[test]
    fn one_unit_short_budget_refuses_the_whole_component_without_partial_regret() {
        let pairs = [
            Pair {
                track: 0,
                plot: 0,
                cost: 1.0,
            },
            Pair {
                track: 0,
                plot: 1,
                cost: 3.0,
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
        let required = regret_work_bound(4, pairs.len()).unwrap();
        let mut plain = assoc_scratch_new(2, 2, pairs.len());
        let mut one_short = assoc_scratch_new_forensics(2, 2, pairs.len());
        gnn(&mut plain, 2, 2, &pairs, 10.0).unwrap();
        gnn_inner(&mut one_short, 2, 2, &pairs, 10.0, true, required - 1).unwrap();
        assert_eq!(assoc_plot_track(&plain), assoc_plot_track(&one_short));
        assert!(assoc_regret_status(&one_short)
            .iter()
            .all(|&status| status == REGRET_BUDGET));
        assert_eq!(assoc_regret(&one_short), &[0.0, 0.0]);
        assert_eq!(assoc_regret_work(&one_short), 0);

        let mut exact = assoc_scratch_new_forensics(2, 2, pairs.len());
        gnn_inner(&mut exact, 2, 2, &pairs, 10.0, true, required).unwrap();
        assert!(assoc_regret_status(&exact)
            .iter()
            .all(|&status| status == REGRET_EXACT));
        assert_eq!(assoc_regret_work(&exact), required);
    }

    #[test]
    fn forensic_solver_matches_plain_decisions_and_keeps_scratch_fixed() {
        let pairs = [
            Pair {
                track: 0,
                plot: 0,
                cost: 1.0,
            },
            Pair {
                track: 0,
                plot: 1,
                cost: 4.0,
            },
            Pair {
                track: 1,
                plot: 0,
                cost: 3.0,
            },
            Pair {
                track: 1,
                plot: 1,
                cost: 2.0,
            },
        ];
        let mut plain = assoc_scratch_new(2, 2, pairs.len());
        let mut forensic = assoc_scratch_new_forensics(2, 2, pairs.len());
        let buffers = assoc_scratch_buffers(&forensic);
        gnn(&mut plain, 2, 2, &pairs, 10.0).unwrap();
        gnn_forensics(&mut forensic, 2, 2, &pairs, 10.0).unwrap();
        assert_eq!(assoc_plot_track(&plain), assoc_plot_track(&forensic));
        assert_eq!(assoc_scratch_buffers(&forensic), buffers);

        let mut no_metadata = assoc_scratch_new(2, 2, pairs.len());
        assert_eq!(
            gnn_forensics(&mut no_metadata, 2, 2, &pairs, 10.0),
            Err(GnnError::ForensicsDisabled)
        );
        assert_eq!(assoc_plot_track(&no_metadata), &[NONE, NONE]);
    }

    #[test]
    fn unsupported_inputs_keep_assignments_and_never_claim_regret() {
        let cases = [
            (
                Pair {
                    track: 0,
                    plot: 0,
                    cost: -1.0,
                },
                10.0,
            ),
            (
                Pair {
                    track: 0,
                    plot: 0,
                    cost: 10.0,
                },
                10.0,
            ),
            (
                Pair {
                    track: 0,
                    plot: 0,
                    cost: 1.0,
                },
                2.0e12,
            ),
            (
                Pair {
                    track: 0,
                    plot: 0,
                    cost: 1.0,
                },
                f64::MAX,
            ),
        ];
        for (pair, gate) in cases {
            let mut plain = assoc_scratch_new(1, 1, 1);
            let mut forensic = assoc_scratch_new_forensics(1, 1, 1);
            gnn(&mut plain, 1, 1, &[pair], gate).unwrap();
            gnn_forensics(&mut forensic, 1, 1, &[pair], gate).unwrap();
            assert_eq!(assoc_plot_track(&plain), assoc_plot_track(&forensic));
            if assoc_plot_track(&forensic)[0] != NONE {
                assert_eq!(assoc_regret_status(&forensic), &[REGRET_UNSUPPORTED]);
            } else {
                assert_eq!(assoc_regret_status(&forensic), &[REGRET_NONE]);
            }
        }
    }

    #[test]
    fn exact_limit_has_regret_and_greedy_limit_reports_unavailable() {
        let exact_tracks = MAX_COMPONENT / 2;
        let exact_pairs: Vec<Pair> = (0..exact_tracks)
            .flat_map(|track| {
                (0..exact_tracks).map(move |plot| Pair {
                    track: track as u32,
                    plot: plot as u32,
                    cost: 1.0,
                })
            })
            .collect();
        let mut exact = assoc_scratch_new_forensics(exact_tracks, exact_tracks, exact_pairs.len());
        gnn_forensics(&mut exact, exact_tracks, exact_tracks, &exact_pairs, 10.0).unwrap();
        assert!(assoc_component_size(&exact)
            .iter()
            .all(|&size| size as usize == MAX_COMPONENT));
        assert!(assoc_regret_status(&exact)
            .iter()
            .all(|&status| status == REGRET_EXACT));

        let greedy_tracks = exact_tracks;
        let greedy_plots = exact_tracks + 1;
        let greedy_pairs: Vec<Pair> = (0..greedy_tracks)
            .flat_map(|track| {
                (0..greedy_plots).map(move |plot| Pair {
                    track: track as u32,
                    plot: plot as u32,
                    cost: 1.0,
                })
            })
            .collect();
        let mut greedy =
            assoc_scratch_new_forensics(greedy_tracks, greedy_plots, greedy_pairs.len());
        gnn_forensics(
            &mut greedy,
            greedy_tracks,
            greedy_plots,
            &greedy_pairs,
            10.0,
        )
        .unwrap();
        assert!(assoc_component_size(&greedy)
            .iter()
            .all(|&size| size as usize == MAX_COMPONENT + 1));
        assert!(assoc_regret_status(&greedy)
            .iter()
            .zip(assoc_plot_track(&greedy))
            .all(|(&status, &track)| {
                if track == NONE {
                    status == REGRET_NONE
                } else {
                    status == REGRET_GREEDY
                }
            }));
    }
}
