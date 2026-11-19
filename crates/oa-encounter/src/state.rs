//! The state of the record: the tables, all allocated when it is made. Core (pure).

use alloc::vec::Vec;

use crate::grid::{grid_tables, BAND_MARGIN, M_PER_DEG_LOW};
use crate::types::{CycleStats, Encounter, EncounterConfig, EncounterLog, Obs};

/// One pair inside the grid's reach, ordered by separation then by track numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Cand {
    /// Squared flat-earth separation, whole square metres.
    pub(crate) d2: u64,
    pub(crate) i: u32,
    pub(crate) j: u32,
}
// Budget: one per pair inside the grid's reach, sorted every cycle.
const _: () = assert!(core::mem::size_of::<Cand>() == 16);

/// An open encounter under its pair key (`addr_a << 32 | addr_b`).
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub(crate) struct Open {
    pub(crate) e: Encounter,
    pub(crate) key: u64,
    /// The pair's relative position (east, north of the second from the first, metres) and time
    /// at the last tick it was evaluated, to interpolate to the next.
    pub(crate) prev_t: i64,
    pub(crate) prev_x: f64,
    pub(crate) prev_y: f64,
}
// Budget: one per open encounter, searched every evaluated pair.
const _: () = assert!(core::mem::size_of::<Open>() == 136);

/// The record of one facility. A subsystem (D5): fields private to the crate, interface the free
/// functions. Rows are one array of structs, sorted by address each cycle: every evaluation reads
/// the whole row, so columns would only split it (A3 exception, recorded).
// A7.2: cold struct (one per run), size guard waived
#[derive(Debug)]
pub struct EncounterState {
    pub(crate) cfg: EncounterConfig,
    pub(crate) band_deg: f64,
    pub(crate) lat_limit_deg: f64,
    pub(crate) lon_cells: Vec<u32>,
    pub(crate) first_cell: Vec<u32>,
    pub(crate) cos_edge: Vec<f64>,
    pub(crate) scratch: Vec<Obs>,
    pub(crate) obs: Vec<Obs>,
    pub(crate) cos_lat: Vec<f64>,
    pub(crate) keys: Vec<u64>,
    pub(crate) cands: Vec<Cand>,
    pub(crate) open: Vec<Open>,
    pub(crate) last_ns: i64,
    pub(crate) prev_ns: i64,
    pub(crate) started: bool,
    pub(crate) totals: CycleStats,
}

const MAX_TRACKS_LIMIT: usize = 1 << 20;
const MAX_OPEN_LIMIT: usize = 1 << 20;
const MAX_CANDIDATES_LIMIT: usize = 1 << 24;

/// Refuses a configuration that is out of range; the text names the value.
pub fn encounter_check(c: &EncounterConfig) -> Result<(), &'static str> {
    let pos = |x: f64| x.is_finite() && x > 0.0;
    if c.max_tracks == 0 || c.max_tracks > MAX_TRACKS_LIMIT {
        return Err("max_tracks");
    }
    if c.max_open == 0 || c.max_open > MAX_OPEN_LIMIT {
        return Err("max_open");
    }
    if c.max_candidates == 0 || c.max_candidates > MAX_CANDIDATES_LIMIT {
        return Err("max_candidates");
    }
    if !(c.volume_h_m.is_finite() && (1_000.0..=100_000.0).contains(&c.volume_h_m)) {
        return Err("volume_h_m");
    }
    if !pos(c.volume_v_ft) {
        return Err("volume_v_ft");
    }
    if c.n_classes > c.classes.len() {
        return Err("n_classes");
    }
    for cl in c.classes.iter().take(c.n_classes) {
        if !pos(cl.h_m) || !pos(cl.v_ft) {
            return Err("class limit");
        }
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
    Ok(())
}

/// A record with every table allocated for `cfg`.
pub fn encounter_new(cfg: &EncounterConfig) -> Result<EncounterState, &'static str> {
    encounter_check(cfg)?;
    let band_deg = cfg.volume_h_m * BAND_MARGIN / M_PER_DEG_LOW;
    let (lon_cells, first_cell, cos_edge) = grid_tables(band_deg).ok_or("grid too fine")?;
    Ok(EncounterState {
        cfg: *cfg,
        band_deg,
        lat_limit_deg: lat_limit(cfg.volume_h_m),
        lon_cells,
        first_cell,
        cos_edge,
        scratch: Vec::with_capacity(cfg.max_tracks),
        obs: Vec::with_capacity(cfg.max_tracks),
        cos_lat: Vec::with_capacity(cfg.max_tracks),
        keys: Vec::with_capacity(cfg.max_tracks),
        cands: Vec::with_capacity(cfg.max_candidates),
        open: Vec::with_capacity(cfg.max_open),
        last_ns: i64::MIN,
        prev_ns: i64::MIN,
        started: false,
        totals: CycleStats::default(),
    })
}

/// The latitude, degrees, up to which the grid's flat-earth test is a superset test for this
/// record radius. The test measures east-west distance at the first track's latitude, so against
/// the true separation it is off by at most `tan(lat) * dphi`, `dphi` being the radius as an angle
/// on the smallest sphere radius (6,356,752 m); the test accepts 2 % over the radius, so the
/// limit is `atan(0.02 / dphi)`, 85.8 degrees for 5 NM. (The flat north-south scale is a lower
/// bound and errs toward accepting.)
fn lat_limit(volume_h_m: f64) -> f64 {
    let dphi = volume_h_m / 6_356_752.0;
    oa_math::atan(0.02 / dphi).to_degrees()
}

/// The latitude limit of this record's grid test, degrees (see the crate documentation).
pub fn encounter_lat_limit_deg(st: &EncounterState) -> f64 {
    st.lat_limit_deg
}

/// Encounters open now.
pub fn encounter_open_len(st: &EncounterState) -> usize {
    st.open.len()
}

/// The counts of every cycle so far, summed.
pub fn encounter_totals(st: &EncounterState) -> CycleStats {
    st.totals
}

/// A log with room for `capacity` encounters.
pub fn encounter_log_new(capacity: usize) -> EncounterLog {
    EncounterLog {
        capacity,
        overflow: 0,
        items: Vec::with_capacity(capacity),
    }
}

/// Removes the encounters, keeping the allocation and the overflow count.
pub fn encounter_log_clear(l: &mut EncounterLog) {
    l.items.clear();
}

/// Appends an encounter, or counts the overflow.
pub(crate) fn log_push(l: &mut EncounterLog, e: &Encounter) {
    if l.items.len() >= l.capacity {
        l.overflow = l.overflow.saturating_add(1);
        return;
    }
    l.items.push(*e);
}
