//! Radar registration: each radar's range and azimuth bias, estimated online against ADS-B
//! tracks of high integrity, with their uncertainty. Core (pure).
//!
//! # Estimator
//!
//! The two biases are independent scalars, each a constant (a random walk of small density,
//! [`crate::fusion_params::REG_Q_RANGE_M2_PER_S`]) observed directly: a Kalman filter per bias
//! with state `b`, variance `P`, prediction `P += q dt`, and measurement `z = plot - predicted`
//! where the prediction is the reference track's position seen from the radar by the exact
//! forward model (`radar::polar_of`). The measurement variance is the plot's random error
//! (range or azimuth sigma, range quantum) plus the reference track's own position variance
//! projected on the radial or tangential direction, plus the height term, so an imprecise
//! reference counts less. The starting variance is the assumed prior
//! ([`crate::fusion_params::REG_PRIOR_SIGMA_RANGE_M`]); no sourced bias exists, so the prior is a
//! stated assumption and the report sweeps the true bias instead.
//!
//! A residual more than [`REG_GATE_SIGMAS`] standard deviations from the estimate is rejected
//! and counted (a wrong association or an outlier must not pull the estimate). The estimate is
//! clamped to a limit and the clamp counted.
//!
//! # Agreement between references, and a step change
//!
//! A residual is offered with the key of the reference aircraft that made it ([`reg_offer`]).
//! One reference is never trusted alone: a spoofed ADS-B position (or a bad GNSS fix) shifts
//! every residual of that aircraft, and used directly it would shift the estimate for every
//! plot of the radar. The estimator keeps, for each of up to a configured number of slots
//! ([`REG_REF_SLOTS`] by default) of references, the mean of its last residuals ([`REG_REF_MEMORY`] of them at most) with the
//! variance of that mean, and uses a residual only when
//!
//! 1. its reference has at least [`REG_REF_MIN_SAMPLES`] residuals (so the mean is no longer
//!    one noisy plot), and at least [`REG_MIN_REFS`] distinct references are current (seen
//!    within [`REG_REF_WINDOW_NS`]); until then the residual is counted in
//!    [`RegState::building`] and not used;
//! 2. the reference's mean agrees with the median of the other current references' means, within
//!    [`REG_AGREE_SIGMAS`] standard deviations of the two means' difference; a reference that
//!    disagrees is counted in [`RegState::inconsistent`] and not used. The median is robust to
//!    a minority of references: with [`REG_MIN_REFS`] references one can lie.
//!
//! A radar that is realigned steps its true bias, after which every residual is far from an
//! estimate whose variance has shrunk to a metre or two, and the Kalman filter moves a few
//! percent of a metre per update: it would never recover. A CUSUM per axis ([`REG_CUSUM_K`],
//! [`REG_CUSUM_H`]) runs over the residuals that passed the agreement test (so a lone reference
//! cannot trigger it), each standardised by the plot's own scatter and clipped at
//! [`REG_CUSUM_CLIP`] so a few wild plots do not count as a shift. When a sum passes the
//! threshold the estimate restarts at the mean of the residuals since the sum last left zero,
//! with the variance of that mean (at least the plot's own variance over the count); the
//! restart is counted in [`RegState::resets`] and published in [`RegSample`]. The first residual
//! used from a reference is its mean so far (with the variance of that mean), so the residuals
//! made while it was being checked still count.
//!
//! The estimate is *subtracted* from the radar's plots and its variance added to theirs
//! (`radar::plot_convert`), so a radar that is not yet registered is weighted as the
//! uncertainty says. [`RegSample`] rows are the published history per radar over time.
//!
//! # Reference slots
//!
//! A reference keeps its slot for as long as its aircraft keeps producing residuals. A slot is
//! given up only when its aircraft has produced none for [`REG_REF_STALE_SCANS`] scans of the
//! radar (it left coverage, or stopped being eligible); an aircraft that finds every slot held
//! by a live reference is ignored and counted in [`RegState::ignored`]. Replacing the stalest
//! slot on demand instead would, with more aircraft than slots, give each a fresh slot before
//! it had made the residuals it needs, and registration would never start in busy airspace.

use crate::fusion_params::{
    REG_AGREE_SIGMAS, REG_CUSUM_CLIP, REG_CUSUM_H, REG_CUSUM_K, REG_DEFAULT_SCAN_NS,
    REG_GATE_SIGMAS, REG_LIMIT_AZ_DEG, REG_LIMIT_RANGE_M, REG_MIN_REFS, REG_PRIOR_SIGMA_AZ_DEG,
    REG_PRIOR_SIGMA_RANGE_M, REG_Q_AZ_DEG2_PER_S, REG_Q_RANGE_M2_PER_S, REG_REF_MEMORY,
    REG_REF_MIN_SAMPLES, REG_REF_SLOTS, REG_REF_SLOTS_MAX, REG_REF_STALE_SCANS, REG_REF_WINDOW_NS,
};
use crate::radar::{polar_of, Bias, RadarGeom, RadarPlot, RadarSite, FT_M};

/// What one reference aircraft's residuals have averaged to. Plain data (A1); A7.2: cold struct
/// (a slot of a radar's estimator).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RegRef {
    /// The reference's key; [`crate::types::NO_ADDR`] for an empty slot.
    pub key: u32,
    /// Residuals seen.
    pub n: u32,
    /// Time of the newest, Unix ns.
    pub t_ns: i64,
    pub range_m: f64,
    pub az_deg: f64,
    /// Mean variance of one residual (the mean's own variance is this over the count, at most
    /// [`REG_REF_MEMORY`]).
    pub var_range_m2: f64,
    pub var_az_deg2: f64,
    /// The reference's mean (all the residuals it made before it qualified) has been used.
    pub seeded: bool,
}

const EMPTY_REF: RegRef = RegRef {
    key: crate::types::NO_ADDR,
    n: 0,
    t_ns: i64::MIN,
    range_m: 0.0,
    az_deg: 0.0,
    var_range_m2: 0.0,
    var_az_deg2: 0.0,
    seeded: false,
};

/// A one-sided and a two-sided CUSUM of one axis. Plain data (A1); A7.2: cold struct.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Cusum {
    pos: f64,
    neg: f64,
}

/// One radar's bias estimate. Plain data (A1); A7.2: cold struct (one per radar).
#[derive(Debug, Clone, PartialEq)]
pub struct RegState {
    pub range_m: f64,
    pub var_range_m2: f64,
    pub az_deg: f64,
    pub var_az_deg2: f64,
    /// Time of the last prediction or update, Unix ns; `i64::MIN` before the first.
    pub t_ns: i64,
    /// Residual pairs used.
    pub used: u64,
    /// Residual pairs rejected by the gate.
    pub rejected: u64,
    /// Updates that hit the limit.
    pub clamped: u64,
    /// Residuals held back: the reference or the radar did not yet have enough references.
    pub building: u64,
    /// Residuals not used because their reference disagrees with the others.
    pub inconsistent: u64,
    /// Times the estimate restarted on a persistent shift.
    pub resets: u64,
    /// Residuals from aircraft that found every reference slot held by a live reference.
    pub ignored: u64,
    /// The reference slots, all allocated when the estimator is made.
    refs: Vec<RegRef>,
    /// Scratch of the agreement test (four columns of one slot count each).
    scratch: Vec<f64>,
    /// A slot is free for another aircraft when its own has made no residual for this long, ns.
    stale_ns: i64,
    cs_r: Cusum,
    cs_a: Cusum,
    /// Residuals, and their sums, since the sums last left zero.
    cs_n: u32,
    cs_sum_r: f64,
    cs_sum_a: f64,
    cs_var_r: f64,
    cs_var_a: f64,
}

/// One published estimate. Plain data (A1); A7.2: cold struct (a log row).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RegSample {
    pub t_ns: i64,
    pub range_m: f64,
    pub range_sigma_m: f64,
    pub az_deg: f64,
    pub az_sigma_deg: f64,
    pub used: u64,
    /// Restarts on a persistent shift so far.
    pub resets: u64,
    /// Residuals refused for disagreeing with the other references so far.
    pub inconsistent: u64,
}

/// A residual of a plot against a reference position, with variances. Plain data (A1); A7.2:
/// transient.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Residual {
    pub range_m: f64,
    pub var_range_m2: f64,
    pub az_deg: f64,
    pub var_az_deg2: f64,
}

/// The estimator at the prior, with the default slot count and scan period.
pub fn reg_new() -> RegState {
    reg_new_with(REG_REF_SLOTS, REG_DEFAULT_SCAN_NS)
}

/// The estimator at the prior with `slots` reference slots (at least one) for a radar whose scan
/// takes `scan_ns`: a slot is given up after [`REG_REF_STALE_SCANS`] scans without a residual.
pub fn reg_new_with(slots: usize, scan_ns: i64) -> RegState {
    let slots = slots.clamp(1, REG_REF_SLOTS_MAX);
    RegState {
        range_m: 0.0,
        var_range_m2: REG_PRIOR_SIGMA_RANGE_M * REG_PRIOR_SIGMA_RANGE_M,
        az_deg: 0.0,
        var_az_deg2: REG_PRIOR_SIGMA_AZ_DEG * REG_PRIOR_SIGMA_AZ_DEG,
        t_ns: i64::MIN,
        used: 0,
        rejected: 0,
        clamped: 0,
        building: 0,
        inconsistent: 0,
        resets: 0,
        ignored: 0,
        refs: vec![EMPTY_REF; slots],
        scratch: vec![0.0; 4 * slots],
        stale_ns: i64::from(REG_REF_STALE_SCANS).saturating_mul(scan_ns.max(1)),
        cs_r: Cusum { pos: 0.0, neg: 0.0 },
        cs_a: Cusum { pos: 0.0, neg: 0.0 },
        cs_n: 0,
        cs_sum_r: 0.0,
        cs_sum_a: 0.0,
        cs_var_r: 0.0,
        cs_var_a: 0.0,
    }
}

/// The correction to apply: the estimate and its variance.
pub fn reg_bias(r: &RegState) -> Bias {
    Bias {
        range_m: r.range_m,
        az_deg: r.az_deg,
        var_range_m2: r.var_range_m2,
        var_az_deg2: r.var_az_deg2,
    }
}

/// The estimate as a published row.
pub fn reg_sample(r: &RegState, t_ns: i64) -> RegSample {
    RegSample {
        t_ns,
        range_m: r.range_m,
        range_sigma_m: r.var_range_m2.max(0.0).sqrt(),
        az_deg: r.az_deg,
        az_sigma_deg: r.var_az_deg2.max(0.0).sqrt(),
        used: r.used,
        resets: r.resets,
        inconsistent: r.inconsistent,
    }
}

/// Wraps a difference of azimuths in degrees to `[-180, 180)`.
fn wrap180(d: f64) -> f64 {
    (d + 180.0).rem_euclid(360.0) - 180.0
}

/// The residual of `plot` against a reference at geodetic `(lat, lon, h_m)` whose position
/// covariance in the local plane is `cov = [ee, en, nn]` (m^2) and height variance `var_h_m2`.
// Eight plain inputs of one residual; a struct would only rename them.
#[allow(clippy::too_many_arguments)]
pub fn reg_residual(
    g: &RadarGeom,
    s: &RadarSite,
    plot: &RadarPlot,
    lat_deg: f64,
    lon_deg: f64,
    h_m: f64,
    cov: [f64; 3],
    var_h_m2: f64,
) -> Residual {
    let (rho, az) = polar_of(g, lat_deg, lon_deg, h_m);
    let dh = h_m - g.height_m;
    let ground = (rho * rho - dh * dh).max(1.0).sqrt();
    let cos_e = (ground / rho).clamp(0.05, 1.0);
    let sin_e = dh / rho;
    let (st, ct) = oa_math::sin_cos(az.to_radians());
    // Radial (u) and tangential (v) horizontal unit vectors.
    let (u, v) = ([st, ct], [ct, -st]);
    let quad =
        |a: [f64; 2]| a[0] * a[0] * cov[0] + 2.0 * a[0] * a[1] * cov[1] + a[1] * a[1] * cov[2];
    let var_r_track = cos_e * cos_e * quad(u) + sin_e * sin_e * var_h_m2;
    let var_t_track = quad(v) / (ground * ground);
    let sa = s.sigma_az_deg.to_radians();
    let deg2 = oa_math::powi(180.0 / core::f64::consts::PI, 2);
    Residual {
        range_m: plot.range_m - rho,
        var_range_m2: s.sigma_range_m * s.sigma_range_m
            + s.range_quantum_m * s.range_quantum_m / 12.0
            + var_r_track,
        az_deg: wrap180(plot.az_deg - az),
        var_az_deg2: (sa * sa + var_t_track) * deg2,
    }
}

/// Advances the estimate to `t_ns` (adds the random walk). Time never goes backwards.
pub fn reg_predict(r: &mut RegState, t_ns: i64) {
    if r.t_ns != i64::MIN && t_ns > r.t_ns {
        let dt = (t_ns - r.t_ns) as f64 / 1.0e9;
        r.var_range_m2 += REG_Q_RANGE_M2_PER_S * dt;
        r.var_az_deg2 += REG_Q_AZ_DEG2_PER_S * dt;
    }
    if r.t_ns == i64::MIN || t_ns > r.t_ns {
        r.t_ns = t_ns;
    }
}

/// Applies a residual at `t_ns`; returns whether it was used (false: rejected by the gate).
pub fn reg_update(r: &mut RegState, t_ns: i64, z: &Residual) -> bool {
    reg_predict(r, t_ns);
    let (sr, sa) = (
        (r.var_range_m2 + z.var_range_m2).sqrt(),
        (r.var_az_deg2 + z.var_az_deg2).sqrt(),
    );
    let (dr, da) = (z.range_m - r.range_m, z.az_deg - r.az_deg);
    if !(dr.is_finite() && da.is_finite())
        || dr.abs() > REG_GATE_SIGMAS * sr
        || da.abs() > REG_GATE_SIGMAS * sa
    {
        r.rejected += 1;
        return false;
    }
    let kr = r.var_range_m2 / (r.var_range_m2 + z.var_range_m2);
    let ka = r.var_az_deg2 / (r.var_az_deg2 + z.var_az_deg2);
    r.range_m += kr * dr;
    r.var_range_m2 *= 1.0 - kr;
    r.az_deg += ka * da;
    r.var_az_deg2 *= 1.0 - ka;
    let (cr, ca) = (
        r.range_m.clamp(-REG_LIMIT_RANGE_M, REG_LIMIT_RANGE_M),
        r.az_deg.clamp(-REG_LIMIT_AZ_DEG, REG_LIMIT_AZ_DEG),
    );
    if cr != r.range_m || ca != r.az_deg {
        r.clamped += 1;
    }
    r.range_m = cr;
    r.az_deg = ca;
    r.used += 1;
    true
}

/// What [`reg_offer`] did with a residual.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegOutcome {
    /// Used: the estimate moved.
    Used,
    /// Not used: the estimate's own gate rejected it (an outlier).
    Gated,
    /// Not used: too few references or too few residuals from this one yet.
    Building,
    /// Not used: the reference disagrees with the other references.
    Inconsistent,
    /// The residual was part of a persistent shift: the estimate restarted (and then used it).
    Reset,
    /// Not used: every reference slot is held by an aircraft that is still making residuals, so
    /// this aircraft was not taken as a reference (counted in [`RegState::ignored`]).
    Ignored,
}

/// The median of `v[..n]` (reordered); `n` at least 1.
fn median(v: &mut [f64], n: usize) -> f64 {
    let s = v.get_mut(..n).unwrap_or(&mut []);
    s.sort_unstable_by(|a, b| oa_math::cmp_canon_f64(*a, *b));
    let h = n / 2;
    let at = |i: usize| s.get(i).copied().unwrap_or(0.0);
    if n % 2 == 1 {
        at(h)
    } else {
        0.5 * (at(h.saturating_sub(1)) + at(h))
    }
}

/// The variance of a reference's mean residual: one residual's over the count it holds.
fn mean_var(s: &RegRef) -> (f64, f64) {
    let m = f64::from(s.n.clamp(1, REG_REF_MEMORY));
    (s.var_range_m2 / m, s.var_az_deg2 / m)
}

/// Folds `z` into the slot of reference `key`: its own, else an empty one, else the stalest
/// whose aircraft has made no residual for the stale time. `None` when every slot is held by a
/// live reference (the residual is ignored).
fn note_reference(r: &mut RegState, t_ns: i64, key: u32, z: &Residual) -> Option<usize> {
    let stale_before = t_ns.saturating_sub(r.stale_ns);
    let at = r
        .refs
        .iter()
        .position(|s| s.key == key)
        .or_else(|| r.refs.iter().position(|s| s.key == crate::types::NO_ADDR))
        .or_else(|| {
            r.refs
                .iter()
                .enumerate()
                .filter(|(_, s)| s.t_ns < stale_before)
                .min_by_key(|(_, s)| s.t_ns)
                .map(|(i, _)| i)
        })?;
    if let Some(s) = r.refs.get_mut(at) {
        if s.key != key {
            *s = RegRef { key, ..EMPTY_REF };
        }
        s.n = s.n.saturating_add(1);
        let m = f64::from(s.n.min(REG_REF_MEMORY));
        // A running mean over the last `m` residuals (an exponential one once the memory is
        // full), and the variance of that mean.
        if s.n == 1 {
            s.range_m = z.range_m;
            s.az_deg = z.az_deg;
            s.var_range_m2 = z.var_range_m2;
            s.var_az_deg2 = z.var_az_deg2;
        } else {
            s.range_m += (z.range_m - s.range_m) / m;
            s.az_deg += wrap180(z.az_deg - s.az_deg) / m;
            s.var_range_m2 += (z.var_range_m2 - s.var_range_m2) / m;
            s.var_az_deg2 += (z.var_az_deg2 - s.var_az_deg2) / m;
        }
        s.t_ns = t_ns;
    }
    Some(at)
}

/// The reference in slot `at` agrees with the median of the other current references (at least
/// `REG_MIN_REFS - 1` of them, each with enough residuals): `None` if there are too few others
/// to say, else whether it agrees.
fn agrees(r: &mut RegState, t_ns: i64, at: usize) -> Option<bool> {
    // The columns are views of the scratch allocated with the slots (taken out so the slots can
    // be read beside them, and put back: no allocation).
    let mut scratch = std::mem::take(&mut r.scratch);
    let verdict = agrees_in(r, &mut scratch, t_ns, at);
    r.scratch = scratch;
    verdict
}

fn agrees_in(r: &RegState, scratch: &mut [f64], t_ns: i64, at: usize) -> Option<bool> {
    let slots = r.refs.len();
    let (rs, rest) = scratch.split_at_mut(slots);
    let (az, rest) = rest.split_at_mut(slots);
    let (vr, va) = rest.split_at_mut(slots);
    let mut n = 0usize;
    for (i, s) in r.refs.iter().enumerate() {
        if i == at
            || s.key == crate::types::NO_ADDR
            || s.n < REG_REF_MIN_SAMPLES
            || t_ns - s.t_ns > REG_REF_WINDOW_NS
        {
            continue;
        }
        if let (Some(a), Some(b), Some(c), Some(d)) =
            (rs.get_mut(n), az.get_mut(n), vr.get_mut(n), va.get_mut(n))
        {
            let (mvr, mva) = mean_var(s);
            (*a, *b, *c, *d) = (s.range_m, s.az_deg, mvr, mva);
            n += 1;
        }
    }
    if n + 1 < REG_MIN_REFS {
        return None;
    }
    let me = r.refs.get(at)?;
    // The azimuths are compared as differences from this reference's, so the 0/360 seam does
    // not split them.
    for a in az.iter_mut().take(n) {
        *a = me.az_deg + wrap180(*a - me.az_deg);
    }
    let (mr, ma) = (median(rs, n), median(az, n));
    // The median of n means has about pi/2 times the variance of one mean over n.
    let k = core::f64::consts::FRAC_PI_2 / n as f64;
    let (vmr, vma) = (median(vr, n) * k, median(va, n) * k);
    let (mvr, mva) = mean_var(me);
    let tr = REG_AGREE_SIGMAS * (mvr + vmr).sqrt();
    let ta = REG_AGREE_SIGMAS * (mva + vma).sqrt();
    Some((me.range_m - mr).abs() <= tr && wrap180(me.az_deg - ma).abs() <= ta)
}

/// Offers the residual `z` of reference `key` at `t_ns`: used (through [`reg_update`]) only if
/// the references agree, and a persistent shift of the agreed residuals restarts the estimate
/// (see the module documentation). Returns what became of it.
pub fn reg_offer(r: &mut RegState, t_ns: i64, key: u32, z: &Residual) -> RegOutcome {
    if !(z.range_m.is_finite()
        && z.az_deg.is_finite()
        && z.var_range_m2 > 0.0
        && z.var_az_deg2 > 0.0)
    {
        r.rejected += 1;
        return RegOutcome::Gated;
    }
    reg_predict(r, t_ns);
    let Some(at) = note_reference(r, t_ns, key, z) else {
        r.ignored += 1;
        return RegOutcome::Ignored;
    };
    let enough = r.refs.get(at).is_some_and(|s| s.n >= REG_REF_MIN_SAMPLES);
    let verdict = if enough { agrees(r, t_ns, at) } else { None };
    match verdict {
        None => {
            r.building += 1;
            return RegOutcome::Building;
        }
        Some(false) => {
            r.inconsistent += 1;
            return RegOutcome::Inconsistent;
        }
        Some(true) => {}
    }
    let reset = cusum_step(r, z);
    // The first residual used from a reference is its mean so far: the residuals it made while
    // it was being checked are not thrown away.
    let seed = r.refs.get_mut(at).filter(|s| !s.seeded).map(|s| {
        s.seeded = true;
        let (vr, va) = mean_var(s);
        Residual {
            range_m: s.range_m,
            var_range_m2: vr,
            az_deg: s.az_deg,
            var_az_deg2: va,
        }
    });
    if reg_update(r, t_ns, seed.as_ref().unwrap_or(z)) {
        if reset {
            RegOutcome::Reset
        } else {
            RegOutcome::Used
        }
    } else {
        RegOutcome::Gated
    }
}

/// One step of the shift detector on an agreed residual; on detection the estimate restarts at
/// the mean since the sums left zero. Returns whether it did.
fn cusum_step(r: &mut RegState, z: &Residual) -> bool {
    let nr = ((z.range_m - r.range_m) / (r.var_range_m2 + z.var_range_m2).sqrt())
        .clamp(-REG_CUSUM_CLIP, REG_CUSUM_CLIP);
    let na = (wrap180(z.az_deg - r.az_deg) / (r.var_az_deg2 + z.var_az_deg2).sqrt())
        .clamp(-REG_CUSUM_CLIP, REG_CUSUM_CLIP);
    let step = |c: &mut Cusum, x: f64| {
        c.pos = (c.pos + x - REG_CUSUM_K).max(0.0);
        c.neg = (c.neg - x - REG_CUSUM_K).max(0.0);
    };
    step(&mut r.cs_r, nr);
    step(&mut r.cs_a, na);
    let quiet = |c: &Cusum| c.pos == 0.0 && c.neg == 0.0;
    if quiet(&r.cs_r) && quiet(&r.cs_a) {
        r.cs_n = 0;
        r.cs_sum_r = 0.0;
        r.cs_sum_a = 0.0;
        r.cs_var_r = 0.0;
        r.cs_var_a = 0.0;
        return false;
    }
    r.cs_n += 1;
    r.cs_sum_r += z.range_m;
    r.cs_sum_a += z.az_deg;
    r.cs_var_r += z.var_range_m2;
    r.cs_var_a += z.var_az_deg2;
    let hit = |c: &Cusum| c.pos > REG_CUSUM_H || c.neg > REG_CUSUM_H;
    if !(hit(&r.cs_r) || hit(&r.cs_a)) || r.cs_n == 0 {
        return false;
    }
    let n = f64::from(r.cs_n);
    // Restart: the shifted axis at the mean since onset; an axis that did not shift keeps its
    // estimate (its variance is only widened to the prior's, so it can follow too).
    if hit(&r.cs_r) {
        r.range_m = (r.cs_sum_r / n).clamp(-REG_LIMIT_RANGE_M, REG_LIMIT_RANGE_M);
        r.var_range_m2 = r.cs_var_r / (n * n);
    }
    if hit(&r.cs_a) {
        r.az_deg = (r.cs_sum_a / n).clamp(-REG_LIMIT_AZ_DEG, REG_LIMIT_AZ_DEG);
        r.var_az_deg2 = r.cs_var_a / (n * n);
    }
    r.resets += 1;
    r.cs_r = Cusum { pos: 0.0, neg: 0.0 };
    r.cs_a = Cusum { pos: 0.0, neg: 0.0 };
    r.cs_n = 0;
    r.cs_sum_r = 0.0;
    r.cs_sum_a = 0.0;
    r.cs_var_r = 0.0;
    r.cs_var_a = 0.0;
    true
}

/// The height of a track in metres from its altitude in feet (the model's pressure-altitude
/// convention, `radar` module documentation).
pub fn height_of_ft(alt_ft: f64) -> f64 {
    alt_ft * FT_M
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
    use crate::radar::{polar_to_geodetic, radar_geom, NONE_CODE};
    use oa_sensorsim::prng::Rng;

    const SITE: RadarSite = RadarSite {
        lat_deg: 41.976_940_27,
        lon_deg: -87.908_149_72,
        height_m: 207.0,
        sigma_range_m: 70.0,
        sigma_az_deg: 0.08,
        range_quantum_m: 0.0,
        alt_quantum_ft: 100,
        period_ns: 4_800_000_000,
        epoch_ns: 0,
        sensor: 1,
    };

    // REQ: OA-TRK-037
    #[test]
    fn the_estimate_converges_to_the_true_bias_with_an_honest_uncertainty() {
        let g = radar_geom(&SITE);
        let mut rng = Rng::from_seed(11);
        let (true_r, true_a) = (120.0, 0.12);
        let mut r = reg_new();
        let mut errs = Vec::new();
        for trial in 0..40 {
            let mut st = reg_new();
            let mut t = 0i64;
            for k in 0..150 {
                // A reference aircraft somewhere in the coverage, its position known to 5 m.
                let (lat, lon, h) = (
                    41.0 + 2.0 * rng.unit(),
                    -89.0 + 2.5 * rng.unit(),
                    3_000.0 + 8_000.0 * rng.unit(),
                );
                let (rho, az) = polar_of(&g, lat, lon, h);
                let plot = RadarPlot {
                    time_ns: t,
                    range_m: rho + true_r + 70.0 * rng.normal(),
                    az_deg: az + true_a + 0.08 * rng.normal(),
                    alt_ft: 0,
                    addr: u32::MAX,
                    label: u32::MAX,
                    mode_a: NONE_CODE,
                };
                let z = reg_residual(&g, &SITE, &plot, lat, lon, h, [25.0, 0.0, 25.0], 100.0);
                reg_update(&mut st, t, &z);
                t += 1_000_000_000 * (1 + i64::from(k % 3));
            }
            errs.push((
                (st.range_m - true_r) / st.var_range_m2.sqrt(),
                (st.az_deg - true_a) / st.var_az_deg2.sqrt(),
            ));
            if trial == 0 {
                r = st;
            }
        }
        // 150 residuals of 70 m: standard error about 5.7 m and 0.0065 degree.
        assert!(r.var_range_m2.sqrt() < 8.0, "{}", r.var_range_m2.sqrt());
        assert!(r.var_az_deg2.sqrt() < 0.01, "{}", r.var_az_deg2.sqrt());
        // Honest: the standardised error has unit-order spread across trials.
        let n = errs.len() as f64;
        let rms_r = (errs.iter().map(|e| e.0 * e.0).sum::<f64>() / n).sqrt();
        let rms_a = (errs.iter().map(|e| e.1 * e.1).sum::<f64>() / n).sqrt();
        assert!((0.6..1.5).contains(&rms_r), "range z rms {rms_r}");
        assert!((0.6..1.5).contains(&rms_a), "az z rms {rms_a}");
    }

    /// What a reference aircraft's residual looks like: the radar's true bias, the plot's own
    /// scatter, and `lie_r` / `lie_a` of a position the reference misreports.
    fn residual(rng: &mut Rng, bias: (f64, f64), lie: (f64, f64)) -> Residual {
        Residual {
            range_m: bias.0 + lie.0 + 70.0 * rng.normal(),
            var_range_m2: 70.0 * 70.0 + 25.0,
            az_deg: bias.1 + lie.1 + 0.08 * rng.normal(),
            var_az_deg2: 0.08 * 0.08 + 1.0e-6,
        }
    }

    /// Feeds one residual the way the fusion layer does.
    fn feed(st: &mut RegState, t_ns: i64, key: u32, z: &Residual) {
        reg_offer(st, t_ns, key, z);
    }

    /// `rounds` rounds, one residual from each of `refs` references per round (the first
    /// `liars` of them misreport by `lie`), against a bias that is `bias(round)`.
    fn run_rounds(
        st: &mut RegState,
        rng: &mut Rng,
        refs: u32,
        liars: u32,
        lie: (f64, f64),
        rounds: std::ops::Range<i64>,
        bias: impl Fn(i64) -> (f64, f64),
    ) {
        run_rounds_spaced(st, rng, refs, liars, lie, rounds, bias, 100_000_000);
    }

    /// [`run_rounds`] with `stride_ns` between the references of a round (rounds are 5 s).
    #[allow(clippy::too_many_arguments)]
    fn run_rounds_spaced(
        st: &mut RegState,
        rng: &mut Rng,
        refs: u32,
        liars: u32,
        lie: (f64, f64),
        rounds: std::ops::Range<i64>,
        bias: impl Fn(i64) -> (f64, f64),
        stride_ns: i64,
    ) {
        for round in rounds {
            for k in 0..refs {
                let l = if k < liars { lie } else { (0.0, 0.0) };
                let z = residual(rng, bias(round), l);
                feed(
                    st,
                    round * 5_000_000_000 + i64::from(k) * stride_ns,
                    0xA00000 + k,
                    &z,
                );
            }
        }
    }

    // REQ: OA-TRK-174
    #[test]
    fn registration_starts_in_busy_airspace_with_hundreds_of_eligible_aircraft_a_scan() {
        // 300 aircraft make a residual every scan, two of them misreporting. With the 32 slots
        // and replacement of the stalest this replaced, no reference ever held its five
        // residuals; now each keeps its slot.
        let mut rng = Rng::from_seed(3);
        let mut st = reg_new();
        run_rounds_spaced(
            &mut st,
            &mut rng,
            300,
            2,
            (250.0, 0.25),
            0..8,
            |_| (120.0, 0.12),
            10_000_000,
        );
        assert!(
            st.used > 0,
            "nothing used in 8 scans: building {}",
            st.building
        );
        run_rounds_spaced(
            &mut st,
            &mut rng,
            300,
            2,
            (250.0, 0.25),
            8..120,
            |_| (120.0, 0.12),
            10_000_000,
        );
        // The same estimate quality as with twelve references.
        assert!(
            (st.range_m - 120.0).abs() < 10.0,
            "range est {}",
            st.range_m
        );
        assert!((st.az_deg - 0.12).abs() < 0.015, "az est {}", st.az_deg);
        assert!(st.inconsistent > 100, "inconsistent {}", st.inconsistent);
        assert!(st.used > 800, "used {}", st.used);
        assert_eq!(st.resets, 0);
    }

    // REQ: OA-TRK-174
    #[test]
    fn a_slot_is_held_while_its_aircraft_makes_residuals_and_freed_after_six_scans() {
        let mut rng = Rng::from_seed(9);
        let z = |rng: &mut Rng| residual(rng, (120.0, 0.12), (0.0, 0.0));
        let mut st = reg_new_with(4, 5_000_000_000);
        // Six aircraft every scan for five scans: the first four hold the slots throughout
        // and the other two are ignored and counted every time.
        for round in 0..5i64 {
            for k in 0..6u32 {
                let r = z(&mut rng);
                let out = reg_offer(
                    &mut st,
                    round * 5_000_000_000 + i64::from(k) * 1_000_000,
                    0xA00000 + k,
                    &r,
                );
                assert_eq!(
                    out == RegOutcome::Ignored,
                    k >= 4,
                    "round {round} aircraft {k}"
                );
            }
        }
        assert_eq!(st.ignored, 10);
        assert!(st.refs.iter().all(|s| s.n == 5 && s.key < 0xA00004));
        // The four stop; two others turn up. Until six scans (30 s) pass since the last residual
        // of an aircraft its slot is held (the first of them made its last at `last`), then the new ones take them.
        let last = 4 * 5_000_000_000; // the oldest of the four residuals made last
        let r = z(&mut rng);
        assert_eq!(
            reg_offer(&mut st, last + 30_000_000_000, 0xB00000, &r),
            RegOutcome::Ignored
        );
        assert_ne!(
            reg_offer(&mut st, last + 30_000_000_000 + 1, 0xB00000, &r),
            RegOutcome::Ignored
        );
        assert!(st.refs.iter().any(|s| s.key == 0xB00000 && s.n == 1));
    }

    // REQ: OA-TRK-074
    #[test]
    fn a_few_lying_references_cannot_move_the_estimate() {
        // Twelve references, two of which misreport their position by 250 m and 0.25 degree.
        let mut rng = Rng::from_seed(3);
        let mut st = reg_new();
        run_rounds(&mut st, &mut rng, 12, 2, (250.0, 0.25), 0..120, |_| {
            (120.0, 0.12)
        });
        assert!(
            (st.range_m - 120.0).abs() < 10.0,
            "range est {}",
            st.range_m
        );
        assert!((st.az_deg - 0.12).abs() < 0.015, "az est {}", st.az_deg);
        // The liars were seen and refused, the honest majority was used.
        assert!(st.inconsistent > 100, "inconsistent {}", st.inconsistent);
        assert!(st.used > 800, "used {}", st.used);
        assert_eq!(st.resets, 0);
    }

    // REQ: OA-TRK-074
    #[test]
    fn nothing_is_learned_from_too_few_references() {
        // Two references, however many residuals, are never enough to tell a liar from the
        // truth.
        let mut rng = Rng::from_seed(4);
        let mut st = reg_new();
        run_rounds(&mut st, &mut rng, 2, 0, (0.0, 0.0), 0..100, |_| {
            (120.0, 0.12)
        });
        assert_eq!(st.used, 0);
        assert_eq!(st.range_m, 0.0);
        assert_eq!(st.building, 200);
    }

    // REQ: OA-TRK-075
    #[test]
    fn a_step_change_of_the_radar_bias_is_followed() {
        // The radar is realigned at round 150: 50 m and 0.05 degree become 200 m and 0.2 degree.
        let mut rng = Rng::from_seed(5);
        let mut st = reg_new();
        let bias = |r: i64| if r < 150 { (50.0, 0.05) } else { (200.0, 0.20) };
        run_rounds(&mut st, &mut rng, 12, 0, (0.0, 0.0), 0..150, bias);
        assert!(
            (st.range_m - 50.0).abs() < 10.0,
            "before the step {}",
            st.range_m
        );
        run_rounds(&mut st, &mut rng, 12, 0, (0.0, 0.0), 150..230, bias);
        assert!(
            (st.range_m - 200.0).abs() < 15.0,
            "after the step {}",
            st.range_m
        );
        assert!(
            (st.az_deg - 0.20).abs() < 0.02,
            "az after the step {}",
            st.az_deg
        );
        // Found once (or twice at most: range and azimuth may fire separately), and published.
        assert!((1..=3).contains(&st.resets), "resets {}", st.resets);
        assert_eq!(reg_sample(&st, 0).resets, st.resets);
    }

    // REQ: OA-TRK-075
    #[test]
    fn an_honest_steady_radar_is_never_restarted() {
        // 24 references for 400 rounds against a constant bias: 9,600 residuals, no restart and
        // none refused as inconsistent beyond the stated tail rate.
        let mut rng = Rng::from_seed(6);
        let mut st = reg_new();
        run_rounds(&mut st, &mut rng, 24, 0, (0.0, 0.0), 0..400, |_| {
            (90.0, 0.07)
        });
        assert_eq!(st.resets, 0);
        assert!(st.inconsistent < 200, "inconsistent {}", st.inconsistent);
        assert!((st.range_m - 90.0).abs() < 5.0, "{}", st.range_m);
    }

    // REQ: OA-TRK-038
    #[test]
    fn outliers_are_rejected_and_the_estimate_is_clamped() {
        let mut r = reg_new();
        let ok = Residual {
            range_m: 10.0,
            var_range_m2: 5_000.0,
            az_deg: 0.01,
            var_az_deg2: 0.01,
        };
        for k in 0..60 {
            reg_update(&mut r, k * 1_000_000_000, &ok);
        }
        let before = r.clone();
        // A residual 3 km away (a wrong association) is rejected, estimate unchanged.
        let wild = Residual {
            range_m: 3_000.0,
            ..ok
        };
        assert!(!reg_update(&mut r, 61_000_000_000, &wild));
        assert_eq!(r.rejected, before.rejected + 1);
        assert!((r.range_m - before.range_m).abs() < 1e-9);
        // An estimate near the limit, pulled further by a consistent residual, is clamped and
        // counted.
        let mut r2 = RegState {
            range_m: 1_990.0,
            var_range_m2: 400.0,
            ..reg_new()
        };
        let beyond = Residual {
            range_m: 2_050.0,
            var_range_m2: 400.0,
            az_deg: 0.0,
            var_az_deg2: 0.01,
        };
        assert!(reg_update(&mut r2, 0, &beyond));
        assert!((r2.range_m - REG_LIMIT_RANGE_M).abs() < 1e-9);
        assert_eq!(r2.clamped, 1);
    }

    // REQ: OA-TRK-039
    #[test]
    fn a_less_certain_reference_counts_less() {
        let g = radar_geom(&SITE);
        let (lat, lon, h) = (42.3, -87.5, 9_000.0);
        let (rho, az) = polar_of(&g, lat, lon, h);
        let plot = RadarPlot {
            time_ns: 0,
            range_m: rho + 50.0,
            az_deg: az,
            alt_ft: 0,
            addr: u32::MAX,
            label: u32::MAX,
            mode_a: NONE_CODE,
        };
        let tight = reg_residual(&g, &SITE, &plot, lat, lon, h, [16.0, 0.0, 16.0], 25.0);
        let loose = reg_residual(&g, &SITE, &plot, lat, lon, h, [2_500.0, 0.0, 2_500.0], 25.0);
        assert!((tight.range_m - 50.0).abs() < 1e-6);
        assert!(loose.var_range_m2 > tight.var_range_m2 + 1_000.0);
        assert!(loose.var_az_deg2 > tight.var_az_deg2);
        // The unpredictable part: variance never below the plot's own.
        assert!(tight.var_range_m2 >= 70.0 * 70.0);
        // The conversion round trip is consistent with the residual's geometry.
        let geo = polar_to_geodetic(&g, rho, az, h).unwrap();
        assert!((geo.lat_deg - lat).abs() < 1e-6);
    }
}
