//! The state of the net: the tables, all allocated when it is made. Core (pure).

use alloc::vec::Vec;

use crate::corridor::{corridor_index_new, CorridorIndex};
use crate::types::{
    Alert, AlertLog, CycleStats, MsawConfig, MsawTrack, SamplePoint, SkipLog, Skipped, CK_APPROACH,
    CK_DEPARTURE, MAX_CLEARANCE_FT, MAX_CORRIDORS, MAX_CORRIDOR_LENGTH_M, MAX_LOOK_AHEAD_S,
    MAX_OPEN_LIMIT, MAX_R95_M, MAX_RADIUS_M, MAX_SAMPLES, MAX_SPEED_LIMIT_MPS, MAX_TRACKS_LIMIT,
};

/// Most points of one cycle (`max_tracks` times `samples`): the table of points is allocated
/// whole.
const MAX_POINTS_LIMIT: usize = 1 << 24;

/// The net of one facility. A subsystem (D5): fields private to the crate, interface the free
/// functions.
// A7.2: cold struct (one per run), size guard waived
#[derive(Debug)]
pub struct MsawState {
    pub(crate) cfg: MsawConfig,
    /// The grid index of the corridors.
    pub(crate) cidx: CorridorIndex,
    /// The tracks planned this cycle, in address order.
    pub(crate) plan: Vec<MsawTrack>,
    /// Their sample points, `cfg.samples` for each.
    pub(crate) points: Vec<SamplePoint>,
    /// Rows accepted before the budget cut.
    pub(crate) scratch: Vec<MsawTrack>,
    pub(crate) open: Vec<Alert>,
    pub(crate) last_ns: i64,
    pub(crate) started: bool,
    /// The plan phase's counts and time, taken by the evaluation.
    pub(crate) pending: CycleStats,
    pub(crate) pending_t: i64,
    pub(crate) pending_refused: bool,
    pub(crate) totals: CycleStats,
    /// Rows lost before reaching the next cycle, told by the caller.
    pub(crate) lost_input: u64,
}

fn pos(x: f64) -> bool {
    x.is_finite() && x > 0.0
}

/// Refuses a configuration that is out of range; the text names the key.
pub fn msaw_check(c: &MsawConfig) -> Result<(), &'static str> {
    if c.max_tracks == 0 || c.max_tracks > MAX_TRACKS_LIMIT {
        return Err("max_tracks");
    }
    if c.max_open == 0 || c.max_open > MAX_OPEN_LIMIT {
        return Err("max_open");
    }
    if c.track_budget == 0 {
        return Err("track_budget");
    }
    if !(pos(c.look_ahead_s) && c.look_ahead_s <= MAX_LOOK_AHEAD_S) {
        return Err("look_ahead_s");
    }
    if !(c.warning_s.is_finite() && c.warning_s >= 0.0 && c.warning_s <= c.look_ahead_s) {
        return Err("warning_s");
    }
    if c.samples < 2 || c.samples > MAX_SAMPLES {
        return Err("samples");
    }
    if c.max_tracks.saturating_mul(c.samples) > MAX_POINTS_LIMIT {
        return Err("max_tracks times samples");
    }
    if c.hysteresis_ns <= 0 {
        return Err("hysteresis_ns");
    }
    for (v, name) in [
        (c.terrain_clearance_ft, "terrain_clearance_ft"),
        (c.obstacle_clearance_ft, "obstacle_clearance_ft"),
        (c.approach_allowance_ft, "approach_allowance_ft"),
        (c.approach_min_clearance_ft, "approach_min_clearance_ft"),
        (c.departure_min_clearance_ft, "departure_min_clearance_ft"),
    ] {
        if !(0..=MAX_CLEARANCE_FT).contains(&v) {
            return Err(name);
        }
    }
    if !(pos(c.max_speed_mps) && c.max_speed_mps <= MAX_SPEED_LIMIT_MPS) {
        return Err("max_speed_mps");
    }
    if !(c.wide_degrade_share.is_finite()
        && c.wide_degrade_share >= 0.0
        && c.wide_degrade_share <= 1.0)
    {
        return Err("wide_degrade_share");
    }
    if !(c.max_altitude_age_s.is_finite() && c.max_altitude_age_s > 0.0) {
        return Err("max_altitude_age_s");
    }
    if !(c.stale_hold_s.is_finite()
        && c.stale_hold_s >= c.max_altitude_age_s
        && c.stale_hold_s <= MAX_LOOK_AHEAD_S)
    {
        return Err("stale_hold_s");
    }
    if !(c.altitude_loss_degrade_share.is_finite()
        && c.altitude_loss_degrade_share >= 0.0
        && c.altitude_loss_degrade_share <= 1.0)
    {
        return Err("altitude_loss_degrade_share");
    }
    if !(c.terminal_lookahead_s.is_finite()
        && c.terminal_lookahead_s > 0.0
        && c.terminal_lookahead_s <= c.look_ahead_s)
    {
        return Err("terminal_lookahead_s");
    }
    if !(c.capture_heading_deg.is_finite()
        && c.capture_heading_deg > 0.0
        && c.capture_heading_deg <= 90.0)
    {
        return Err("capture_heading_deg");
    }
    if !(c.velocity_uncertainty_mps.is_finite()
        && c.velocity_uncertainty_mps >= 0.0
        && c.velocity_uncertainty_mps <= MAX_SPEED_LIMIT_MPS)
    {
        return Err("velocity_uncertainty_mps");
    }
    // The query radius of a point is not held down (a held radius would not be conservative):
    // the configuration must keep it inside the bound.
    let half_step = c.look_ahead_s / ((c.samples - 1) as f64) / 2.0;
    if MAX_R95_M + c.velocity_uncertainty_mps * c.look_ahead_s + c.max_speed_mps * half_step
        > MAX_RADIUS_M
    {
        return Err("look_ahead_s and samples: query radius beyond the bound");
    }
    if c.corridors.len() > MAX_CORRIDORS {
        return Err("corridors");
    }
    for p in c.corridors.iter() {
        let sane_m = |v: f64| v.is_finite() && v > 0.0 && v <= MAX_CORRIDOR_LENGTH_M;
        if !((CK_APPROACH..=CK_DEPARTURE).contains(&p.kind)
            && sane_m(p.length_m)
            && sane_m(p.near_half_width_m)
            && sane_m(p.far_half_width_m)
            && p.out_bearing_deg.is_finite()
            && (0.0..360.0).contains(&p.out_bearing_deg)
            && p.thr_lat_e7.unsigned_abs() <= 850_000_000
            && p.thr_lon_e7.unsigned_abs() <= 1_800_000_000)
        {
            return Err("corridor geometry");
        }
        if !(p.path_angle_deg.is_finite()
            && p.path_angle_deg > 0.0
            && p.path_angle_deg <= 10.0
            && (0..=MAX_CLEARANCE_FT).contains(&p.tch_ft))
        {
            return Err("corridor path");
        }
        if !(p.pave_half_m.is_finite()
            && p.pave_half_m >= 0.0
            && p.pave_half_m <= MAX_CORRIDOR_LENGTH_M
            && (-MAX_CLEARANCE_FT..=MAX_CLEARANCE_FT).contains(&p.far_elev_ft))
        {
            return Err("corridor runway");
        }
        if !(-MAX_CLEARANCE_FT..=MAX_CLEARANCE_FT).contains(&p.thr_elev_ft)
            || !(-MAX_CLEARANCE_FT..=MAX_CLEARANCE_FT).contains(&p.faf_alt_ft)
        {
            return Err("corridor elevation");
        }
    }
    Ok(())
}

/// An empty vector with room for `n`, or the refusal if the allocator has none (a capacity the
/// machine cannot hold is an error, never an abort).
fn table<T>(n: usize, key: &'static str) -> Result<Vec<T>, &'static str> {
    let mut v = Vec::new();
    v.try_reserve_exact(n).map_err(|_| key)?;
    Ok(v)
}

/// The net with every table allocated for `cfg`. A capacity the allocator cannot give is refused
/// with the name of the key.
pub fn msaw_new(cfg: &MsawConfig) -> Result<MsawState, &'static str> {
    msaw_check(cfg)?;
    Ok(MsawState {
        cfg: cfg.clone(),
        cidx: corridor_index_new(cfg)?,
        plan: table(cfg.max_tracks, "max_tracks: cannot be allocated")?,
        points: table(
            cfg.max_tracks.saturating_mul(cfg.samples),
            "max_tracks times samples: cannot be allocated",
        )?,
        scratch: table(cfg.max_tracks, "max_tracks: cannot be allocated")?,
        open: table(cfg.max_open, "max_open: cannot be allocated")?,
        last_ns: i64::MIN,
        started: false,
        pending: CycleStats::default(),
        pending_t: 0,
        pending_refused: false,
        totals: CycleStats::default(),
        lost_input: 0,
    })
}

/// Alerts open now.
pub fn msaw_open_len(st: &MsawState) -> usize {
    st.open.len()
}

/// The alerts open now, in address order.
pub fn msaw_open(st: &MsawState) -> &[Alert] {
    &st.open
}

/// The sample points of the cycle planned last: `samples` for each planned track, in the order
/// of [`msaw_planned`]. The shell looks up the floor at each and gives them back in order.
pub fn msaw_points(st: &MsawState) -> &[SamplePoint] {
    &st.points
}

/// The tracks planned last, in address order.
pub fn msaw_planned(st: &MsawState) -> &[MsawTrack] {
    &st.plan
}

/// Tells the net that `n` rows were lost before they reached it: the next cycle counts them and
/// does not report running.
pub fn msaw_note_lost_input(st: &mut MsawState, n: u64) {
    st.lost_input = st.lost_input.saturating_add(n);
}

/// The counts of every cycle so far, summed.
pub fn msaw_totals(st: &MsawState) -> CycleStats {
    st.totals
}

/// A log with room for `capacity` alerts, or the refusal if that much cannot be allocated.
pub fn alert_log_new(capacity: usize) -> Result<AlertLog, &'static str> {
    Ok(AlertLog {
        capacity,
        overflow: 0,
        items: table(capacity, "alert_log_capacity: cannot be allocated")?,
    })
}

/// Removes the alerts, keeping the allocation and the overflow count.
pub fn alert_log_clear(l: &mut AlertLog) {
    l.items.clear();
}

/// A log with room for `capacity` skipped tracks, or the refusal if that much cannot be allocated.
pub fn skip_log_new(capacity: usize) -> Result<SkipLog, &'static str> {
    Ok(SkipLog {
        capacity,
        overflow: 0,
        items: table(capacity, "skip_log_capacity: cannot be allocated")?,
    })
}

/// Removes the skipped tracks, keeping the allocation and the overflow count.
pub fn skip_log_clear(l: &mut SkipLog) {
    l.items.clear();
}

/// Appends an alert, or counts the overflow; true if it was kept.
pub(crate) fn alert_push(l: &mut AlertLog, a: &Alert) -> bool {
    if l.items.len() >= l.capacity {
        l.overflow = l.overflow.saturating_add(1);
        return false;
    }
    l.items.push(*a);
    true
}

/// Appends a skipped track, or counts the overflow; true if it was kept.
pub(crate) fn skip_push(l: &mut SkipLog, s: &Skipped) -> bool {
    if l.items.len() >= l.capacity {
        l.overflow = l.overflow.saturating_add(1);
        return false;
    }
    l.items.push(*s);
    true
}
