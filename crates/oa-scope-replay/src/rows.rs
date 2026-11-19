//! Tracker rows to displayed targets (**core**, B1, B2): pure functions of the rows, the
//! callsigns and the time asked for.
//!
//! A frame at time `t` shows, for each address, the newest row at or before `t`, unless that row
//! is older than [`ConvertConfig::stale_ns`] (the track has ended: a live track, coasting
//! included, has a row every tick). History positions are the newest rows at or before `t - k *
//! trail_step` for `k = 1..=trail_n`. A track counts as reacquired when a row with
//! `O_REACQUIRED` lies within the reacquired window before `t`. Callsigns are the newest one
//! heard at or before `t`, never a later one.

use oa_adsb::Reports;
use oa_display::project::Projection;
use oa_display::traffic::{
    on_map, targets_push, Target, TargetSet, MAX_TRAIL, NO_ALT, NO_REACQ, NO_U16, T_COAST,
    T_UNCERTAIN,
};
use oa_track::{r95_m_of, TrackOutput, O_COAST, O_HAS_ALT, O_REACQUIRED, O_UNCERTAIN};
use std::collections::BTreeMap;

const NS_PER_S: i64 = 1_000_000_000;
const MPS_TO_KT: f64 = 1.0 / 0.514_444_444_444;
/// Below this ground speed the track angle is noise and no leader is drawn, knots.
const MIN_TRACK_KT: f64 = 1.0;

/// One tracker row, the columns the display needs. Plain data; A7.2: cold (kept only for the
/// window asked for).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Row {
    pub t_ns: i64,
    pub lat_e7: i32,
    pub lon_e7: i32,
    pub vel_east_mps: f32,
    pub vel_north_mps: f32,
    pub pos_cov_m2: [f32; 3],
    pub alt_ft: f32,
    /// The tracker's `O_*` flags.
    pub flags: u8,
}

/// Rows by address, each in time order once [`store_finish`] has run.
// A7.2: cold struct, size guard waived (one per run).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RowStore {
    pub by_addr: BTreeMap<u32, Vec<Row>>,
}

/// Adds the rows of `out` at or after `keep_from_ns` (earlier ones are not needed for any frame).
pub fn store_add(store: &mut RowStore, out: &TrackOutput, keep_from_ns: i64) {
    for i in 0..out.t_ns.len() {
        let (Some(&t_ns), Some(&addr)) = (out.t_ns.get(i), out.addr.get(i)) else {
            continue;
        };
        if t_ns < keep_from_ns {
            continue;
        }
        let row = Row {
            t_ns,
            lat_e7: out.lat_e7.get(i).copied().unwrap_or(0),
            lon_e7: out.lon_e7.get(i).copied().unwrap_or(0),
            vel_east_mps: out.vel_east_mps.get(i).copied().unwrap_or(f32::NAN),
            vel_north_mps: out.vel_north_mps.get(i).copied().unwrap_or(f32::NAN),
            pos_cov_m2: out.pos_cov_m2.get(i).copied().unwrap_or([f32::NAN; 3]),
            alt_ft: out.alt_ft.get(i).copied().unwrap_or(f32::NAN),
            flags: out.flags.get(i).copied().unwrap_or(0),
        };
        store.by_addr.entry(addr).or_default().push(row);
    }
}

/// Puts every address's rows in time order (stable, so equal times keep the tracker's order).
pub fn store_finish(store: &mut RowStore) {
    for v in store.by_addr.values_mut() {
        v.sort_by_key(|r| r.t_ns);
    }
}

/// Callsigns by address: `(time, callsign)` at each change, in time order.
// A7.2: cold struct, size guard waived (one per run).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Callsigns {
    pub by_addr: BTreeMap<u32, Vec<(i64, [u8; 8])>>,
}

/// The callsigns heard in `reports`, in time order; an empty callsign is not a change.
pub fn callsigns_from_reports(r: &Reports) -> Callsigns {
    let mut all: Vec<(i64, u32, [u8; 8])> = Vec::new();
    for ((&t, &icao), &cs) in r.time_ns.iter().zip(&r.icao).zip(&r.callsign) {
        if cs.first().is_some_and(|&b| b != 0) {
            all.push((t, icao, cs));
        }
    }
    all.sort_unstable();
    let mut out = Callsigns::default();
    for (t, icao, cs) in all {
        let v = out.by_addr.entry(icao).or_default();
        if v.last().map(|x| x.1) != Some(cs) {
            v.push((t, cs));
        }
    }
    out
}

/// The newest callsign of `addr` at or before `t_ns`, or all NUL.
pub fn callsign_at(c: &Callsigns, addr: u32, t_ns: i64) -> [u8; 8] {
    let Some(v) = c.by_addr.get(&addr) else {
        return [0; 8];
    };
    let n = v.partition_point(|x| x.0 <= t_ns);
    n.checked_sub(1)
        .and_then(|i| v.get(i))
        .map_or([0; 8], |x| x.1)
}

/// How rows become targets. Plain data; A7.2: cold struct (one per run).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConvertConfig {
    /// History positions per target, at most [`MAX_TRAIL`].
    pub trail_n: u8,
    /// Time between history positions, ns.
    pub trail_step_ns: i64,
    /// Rows this old (ns before the time asked) mean the track has ended: not shown.
    pub stale_ns: i64,
    /// How far back a reacquisition is looked for, seconds: the display's reacquired window.
    pub reacq_window_s: u16,
}

/// Five history positions 5 s apart; a track silent for 3 s has ended (the tracker ticks every
/// second while a track lives); the default reacquired window of 10 s.
pub const DEFAULT_CONVERT: ConvertConfig = ConvertConfig {
    trail_n: 5,
    trail_step_ns: 5 * NS_PER_S,
    stale_ns: 3 * NS_PER_S,
    reacq_window_s: 10,
};

/// What a conversion left out. Plain record; A7.2: cold struct (one per frame).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConvertStats {
    /// Addresses with a row at or before the time, but too old (tracks that ended).
    pub ended: u32,
    /// Targets off the map area, kept out of the set so they cannot use its capacity.
    pub off_map: u32,
    /// Targets put in the set (the set counts those it refused itself).
    pub added: u32,
}

/// The newest row at or before `t_ns`.
fn newest(v: &[Row], t_ns: i64) -> Option<(usize, &Row)> {
    let n = v.partition_point(|r| r.t_ns <= t_ns);
    let i = n.checked_sub(1)?;
    v.get(i).map(|r| (i, r))
}

/// Ground speed in 0.1 knot and track in 0.1 degree of a velocity; absent values are [`NO_U16`].
/// The track is absent when the ground speed is under [`MIN_TRACK_KT`].
pub fn velocity_of(east_mps: f32, north_mps: f32) -> (u16, u16) {
    if !east_mps.is_finite() || !north_mps.is_finite() {
        return (NO_U16, NO_U16);
    }
    let (e, n) = (f64::from(east_mps), f64::from(north_mps));
    let kt = e.hypot(n) * MPS_TO_KT;
    let gs = (kt * 10.0).round();
    let gs = if gs < f64::from(NO_U16) {
        gs as u16
    } else {
        NO_U16 - 1
    };
    if kt < MIN_TRACK_KT {
        return (gs, NO_U16);
    }
    let deg = e.atan2(n).to_degrees().rem_euclid(360.0);
    let dd = (deg * 10.0).round() as u16;
    (gs, if dd >= 3600 { 0 } else { dd })
}

/// The target for the newest row `i` of `v` at time `t_ns`.
fn target_of(
    v: &[Row],
    i: usize,
    row: &Row,
    addr: u32,
    calls: &Callsigns,
    t_ns: i64,
    cfg: &ConvertConfig,
) -> Target {
    let (gs_dkt, track_ddeg) = velocity_of(row.vel_east_mps, row.vel_north_mps);
    let alt_ft = if row.flags & O_HAS_ALT != 0 && row.alt_ft.is_finite() {
        row.alt_ft.round() as i32
    } else {
        NO_ALT
    };
    let mut flags = 0;
    if row.flags & O_COAST != 0 {
        flags |= T_COAST;
    }
    if row.flags & O_UNCERTAIN != 0 {
        flags |= T_UNCERTAIN;
    }
    // Newest reacquired row inside the window, looking back from the newest row.
    let window = i64::from(cfg.reacq_window_s) * NS_PER_S;
    let mut reacq_age_s = NO_REACQ;
    for r in v.get(..=i).unwrap_or(&[]).iter().rev() {
        if t_ns - r.t_ns > window {
            break;
        }
        if r.flags & O_REACQUIRED != 0 {
            reacq_age_s = u16::try_from((t_ns - r.t_ns) / NS_PER_S).unwrap_or(NO_REACQ - 1);
            break;
        }
    }
    let mut trail = [(0, 0); MAX_TRAIL];
    let mut trail_len = 0u8;
    for k in 1..=i64::from(cfg.trail_n.min(MAX_TRAIL as u8)) {
        let tk = t_ns.saturating_sub(k.saturating_mul(cfg.trail_step_ns));
        if let Some((_, r)) = newest(v, tk).filter(|(_, r)| tk - r.t_ns <= cfg.stale_ns) {
            if let Some(slot) = trail.get_mut(usize::from(trail_len)) {
                *slot = (r.lat_e7, r.lon_e7);
                trail_len += 1;
            }
        }
    }
    Target {
        lat_e7: row.lat_e7,
        lon_e7: row.lon_e7,
        track_ddeg,
        gs_dkt,
        alt_ft,
        addr,
        callsign: callsign_at(calls, addr, t_ns),
        flags,
        reacq_age_s,
        r95_m: r95_m_of(&row.pos_cov_m2) as f32,
        trail,
        trail_len,
    }
}

/// Fills `set` (which the caller has cleared) with the targets shown at `t_ns`, in address order.
/// A set that fills up counts what it refuses; targets off the map are not offered to it.
pub fn targets_at(
    store: &RowStore,
    calls: &Callsigns,
    t_ns: i64,
    cfg: &ConvertConfig,
    proj: &Projection,
    set: &mut TargetSet,
) -> ConvertStats {
    let mut st = ConvertStats::default();
    for (&addr, v) in &store.by_addr {
        let Some((i, row)) = newest(v, t_ns) else {
            continue;
        };
        if t_ns - row.t_ns > cfg.stale_ns {
            st.ended += 1;
            continue;
        }
        if !on_map(proj, row.lat_e7, row.lon_e7) {
            st.off_map += 1;
            continue;
        }
        let t = target_of(v, i, row, addr, calls, t_ns, cfg);
        // A refusal is counted by the set and shown on the frame.
        if targets_push(set, &t).is_ok() {
            st.added += 1;
        }
    }
    st
}
