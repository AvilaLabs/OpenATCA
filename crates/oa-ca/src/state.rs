//! The state of conflict alert: the tables, all allocated when it is made. Core (pure).

use alloc::vec::Vec;
use oa_geo::aeqd::Frame;

use crate::grid::{grid_tables, M_PER_DEG_LOW};
use crate::solve::{MAX_ABS_VRATE_FPS, MAX_COORD_M, MAX_LOOK_AHEAD_S, MAX_RADIUS_M};
use crate::types::{
    Alert, AlertLog, Area, CaConfig, CaTrack, CycleStats, EventLog, Volume, MAX_VOLUMES,
};
use crate::volume::volume_maxima;

/// One pair inside the grid's reach, ordered by flat separation then by track numbers (tracks
/// are in address order, so ties break by address).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Cand {
    /// Squared flat-earth separation, whole square metres.
    pub(crate) d2: u64,
    pub(crate) i: u32,
    pub(crate) j: u32,
}
// Budget: one per pair inside the grid's reach, sorted every cycle.
const _: () = assert!(core::mem::size_of::<Cand>() == 16);

/// A track's altitude at the last cycle, to see it lost.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct AltRec {
    pub(crate) addr: u32,
    /// NaN when the track had none.
    pub(crate) alt_ft: f32,
}
// Budget: one per live track, merged every cycle.
const _: () = assert!(core::mem::size_of::<AltRec>() == 8);

/// Conflict alert of one facility. A subsystem (D5): fields private to the crate, interface the
/// free functions. Rows are one array of structs sorted by address each cycle: every evaluation
/// reads the whole row, so columns would only split it (A3 exception, as `oa-encounter`).
// A7.2: cold struct (one per run), size guard waived
#[derive(Debug)]
pub struct CaState {
    pub(crate) cfg: CaConfig,
    pub(crate) look_ahead_s: f64,
    pub(crate) h_max_m: f64,
    pub(crate) v_max_ft: f64,
    pub(crate) band_deg: f64,
    pub(crate) lon_cells: Vec<u32>,
    pub(crate) first_cell: Vec<u32>,
    pub(crate) cos_edge: Vec<f64>,
    pub(crate) scratch: Vec<CaTrack>,
    pub(crate) obs: Vec<CaTrack>,
    pub(crate) cos_lat: Vec<f64>,
    pub(crate) speed: Vec<f64>,
    pub(crate) frames: Vec<Frame>,
    pub(crate) keys: Vec<u64>,
    pub(crate) cands: Vec<Cand>,
    pub(crate) open: Vec<Alert>,
    pub(crate) alt_prev: Vec<AltRec>,
    pub(crate) alt_next: Vec<AltRec>,
    pub(crate) last_ns: i64,
    pub(crate) started: bool,
    pub(crate) totals: CycleStats,
}

const MAX_TRACKS_LIMIT: usize = 1 << 20;
const MAX_OPEN_LIMIT: usize = 1 << 20;
const MAX_CANDIDATES_LIMIT: usize = 1 << 24;
/// The largest speed the configuration may allow, metres per second (a bound of the solver).
const MAX_SPEED_LIMIT_MPS: f64 = 1.0e3;
const MAX_VOLUME_H_M: f64 = 1.0e5;
const MAX_VOLUME_V_FT: f64 = 1.0e5;
const NS_PER_S: f64 = 1.0e9;

fn pos(x: f64) -> bool {
    x.is_finite() && x > 0.0
}

fn check_area(a: &Area) -> Result<(), &'static str> {
    match a {
        Area::Box {
            lat_min_e7,
            lat_max_e7,
            lon_min_e7,
            lon_max_e7,
        } => {
            if *lat_min_e7 >= *lat_max_e7 || *lon_min_e7 >= *lon_max_e7 {
                return Err("volume box empty or reversed");
            }
            if *lat_min_e7 < -900_000_000
                || *lat_max_e7 > 900_000_000
                || *lon_min_e7 < -1_800_000_000
                || *lon_max_e7 > 1_800_000_000
            {
                return Err("volume box off the globe");
            }
            Ok(())
        }
        Area::Polygon(p) => {
            if p.n < 3 || usize::from(p.n) > oa_safety::geom::MAX_VERTICES {
                return Err("volume polygon vertex count");
            }
            Ok(())
        }
    }
}

fn check_volume(v: &Volume) -> Result<(), &'static str> {
    if !(pos(v.h_m) && v.h_m <= MAX_VOLUME_H_M) {
        return Err("volume h_m");
    }
    if !(pos(v.v_ft) && v.v_ft <= MAX_VOLUME_V_FT) {
        return Err("volume v_ft");
    }
    if v.alt_lo_ft > v.alt_hi_ft {
        return Err("volume altitude band reversed");
    }
    check_area(&v.area)
}

/// Refuses a configuration that is out of range; the text names the value. The largest radius
/// the candidate scan can need (look-ahead times twice the largest speed, plus the largest
/// horizontal minimum and two radii) must stay within the solver's bound.
pub fn ca_check(c: &CaConfig) -> Result<(), &'static str> {
    if c.max_tracks == 0 || c.max_tracks > MAX_TRACKS_LIMIT {
        return Err("max_tracks");
    }
    if c.max_open == 0 || c.max_open > MAX_OPEN_LIMIT {
        return Err("max_open");
    }
    if c.max_candidates == 0 || c.max_candidates > MAX_CANDIDATES_LIMIT {
        return Err("max_candidates");
    }
    if c.look_ahead_ns <= 0 || (c.look_ahead_ns as f64) > MAX_LOOK_AHEAD_S * NS_PER_S {
        return Err("look_ahead_ns");
    }
    if c.hysteresis_ns <= 0 {
        return Err("hysteresis_ns");
    }
    if c.work_budget == 0 {
        return Err("work_budget");
    }
    if c.eval_budget == 0 {
        return Err("eval_budget");
    }
    if !(pos(c.max_speed_mps) && c.max_speed_mps <= MAX_SPEED_LIMIT_MPS) {
        return Err("max_speed_mps");
    }
    if !(pos(c.r95_cap_m) && c.r95_cap_m <= MAX_RADIUS_M / 10.0) {
        return Err("r95_cap_m");
    }
    if !(c.grid_band_m.is_finite() && (1_000.0..=MAX_COORD_M).contains(&c.grid_band_m)) {
        return Err("grid_band_m");
    }
    if c.n_volumes > MAX_VOLUMES {
        return Err("n_volumes");
    }
    check_volume(&c.default_volume)?;
    for v in c.volumes.iter().take(c.n_volumes) {
        check_volume(v)?;
    }
    let (h, _) = volume_maxima(c);
    let reach = c.look_ahead_ns as f64 / NS_PER_S * 2.0 * c.max_speed_mps + h + 2.0 * c.r95_cap_m;
    // 1.02: the cheap test of the grid accepts this much over the radius.
    if reach * 1.02 > MAX_COORD_M {
        return Err("look-ahead reach beyond the solver bound");
    }
    // The vertical solver's bound on the relative rate (the data-sanity rate is 1,000 ft/s).
    if 2.0 * crate::types::MAX_ABS_VRATE_FPM / 60.0 > MAX_ABS_VRATE_FPS {
        return Err("internal: vertical rate bound");
    }
    Ok(())
}

/// Conflict alert with every table allocated for `cfg`.
pub fn ca_new(cfg: &CaConfig) -> Result<CaState, &'static str> {
    ca_check(cfg)?;
    let band_deg = cfg.grid_band_m / M_PER_DEG_LOW;
    let (lon_cells, first_cell, cos_edge) = grid_tables(band_deg).ok_or("grid too fine")?;
    let (h_max_m, v_max_ft) = volume_maxima(cfg);
    Ok(CaState {
        cfg: *cfg,
        look_ahead_s: cfg.look_ahead_ns as f64 / NS_PER_S,
        h_max_m,
        v_max_ft,
        band_deg,
        lon_cells,
        first_cell,
        cos_edge,
        scratch: Vec::with_capacity(cfg.max_tracks),
        obs: Vec::with_capacity(cfg.max_tracks),
        cos_lat: Vec::with_capacity(cfg.max_tracks),
        speed: Vec::with_capacity(cfg.max_tracks),
        frames: Vec::with_capacity(cfg.max_tracks),
        keys: Vec::with_capacity(cfg.max_tracks),
        cands: Vec::with_capacity(cfg.max_candidates),
        open: Vec::with_capacity(cfg.max_open),
        alt_prev: Vec::with_capacity(cfg.max_tracks),
        alt_next: Vec::with_capacity(cfg.max_tracks),
        last_ns: i64::MIN,
        started: false,
        totals: CycleStats::default(),
    })
}

/// Alerts open now.
pub fn ca_open_len(st: &CaState) -> usize {
    st.open.len()
}

/// The alerts open now, in pair order (addresses).
pub fn ca_open(st: &CaState) -> &[Alert] {
    &st.open
}

/// The counts of every cycle so far, summed.
pub fn ca_totals(st: &CaState) -> CycleStats {
    st.totals
}

/// A log with room for `capacity` alerts.
pub fn alert_log_new(capacity: usize) -> AlertLog {
    AlertLog {
        capacity,
        overflow: 0,
        items: Vec::with_capacity(capacity),
    }
}

/// Removes the alerts, keeping the allocation and the overflow count.
pub fn alert_log_clear(l: &mut AlertLog) {
    l.items.clear();
}

/// A log with room for `capacity` altitude-loss events.
pub fn event_log_new(capacity: usize) -> EventLog {
    EventLog {
        capacity,
        overflow: 0,
        items: Vec::with_capacity(capacity),
    }
}

/// Removes the events, keeping the allocation and the overflow count.
pub fn event_log_clear(l: &mut EventLog) {
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
