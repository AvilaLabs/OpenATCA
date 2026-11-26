//! Tracker rows to displayed targets (**core**, B1, B2): pure functions of plain columns, the
//! callsigns and the time asked for. Every public function here is core. The functions that build
//! the store and the callsigns (`store_add`, `store_finish`, `callsigns_build`) are shell
//! ([`crate::shell::store`]): they allocate and run once per drain, before the frames (C1, C6).
//! This module owns the data types and [`targets_at`], which allocates nothing.
//!
//! The core sees no tracker or decoder type (D3): the shell hands over [`RowColumns`], plain
//! slices, with the 95% radius already reduced to a value and the flags in this module's own
//! `RF_*` bits.
//!
//! A frame at time `t` shows, for each address, the newest row at or before `t`, unless that row
//! is older than [`ConvertConfig::stale_ns`] (the track has ended: a live track, coasting
//! included, has a row every tick). History positions are the newest rows at or before `t - k *
//! trail_step` for `k = 1..=trail_n`. A track counts as reacquired when a row with
//! `RF_REACQUIRED` lies within the reacquired window before `t`. Callsigns are the newest one
//! heard at or before `t`, never a later one; two different callsigns heard for one address at
//! one instant are a conflict, reported as such and not resolved by sort order (E6).
//!
//! # Nothing is substituted, nothing is dropped unseen (E6, E8)
//!
//! A row is refused, and counted by reason in [`StoreStats`], when a column it needs is missing;
//! columns of different lengths are an error before any row is read. A value that cannot be shown
//! as it is (an altitude beyond `i32`, a speed beyond the field, a 95% radius that is not a
//! number or does not fit `f32`) is clamped or marked unknown and counted, and the sentinels
//! ([`NO_ALT`], [`NO_U16`], [`NO_REACQ`]) are unreachable from real data. A time difference that
//! does not fit an `i64` is larger than any window, and is treated as such.

use oa_display::core::project::Projection;
use oa_display::core::traffic::{
    map_position, position_valid, targets_push, MapPos, Target, TargetSet, IDENT_CONFLICT,
    IDENT_OBSERVED, IDENT_UNKNOWN, MAX_REACQ_AGE_S, MAX_TRAIL, NO_ALT, NO_IDENT, NO_IDENT_NS,
    NO_REACQ, NO_U16, T_COAST, T_CS_CONFLICT, T_UNCERTAIN,
};
use std::collections::BTreeMap;
use std::fmt;

pub const NS_PER_S: i64 = 1_000_000_000;
const MPS_TO_KT: f64 = 1.0 / 0.514_444_444_444;
/// Below this ground speed the track angle is noise and no leader is drawn, knots.
const MIN_TRACK_KT: f64 = 1.0;

/// [`RowColumns::flags`] and [`Row::flags`]: the track is coasting.
pub const RF_COAST: u8 = 1;
/// The row has an altitude.
pub const RF_HAS_ALT: u8 = 2;
/// The row is one at which the track was reacquired.
pub const RF_REACQUIRED: u8 = 4;
/// The position is uncertain.
pub const RF_UNCERTAIN: u8 = 8;

/// Most distinct addresses the store holds. Analysis: the tracker holds at most 4096 tracks at a
/// time (`DEFAULT_CONFIG.max_tracks`); a clip window of up to a day can see every address in the
/// world once, about 10^5 at the busiest hour; 2^17 is 131 072 with a margin.
pub const MAX_STORE_ADDRESSES: usize = 1 << 17;
/// Most rows one address holds. Analysis: a track ticks once a second; a window is at most a day
/// of frames plus a day of warm-up (both limited to 86 400 s by the command line), 172 800 rows,
/// under 2^18 = 262 144.
pub const MAX_ROWS_PER_ADDRESS: usize = 1 << 18;
/// Most rows the store holds in all. Analysis: 4096 simultaneous tracks at one row a second for
/// about 4096 s is 2^24 = 16.7 million rows (about 0.625 GiB at 40 bytes a row) which is the largest
/// window this tool is sized for.
pub const MAX_STORE_ROWS: usize = 1 << 24;
// Work per frame (C7), in terms of the capacities above. `targets_at` visits every address once:
// at most MAX_STORE_ADDRESSES (2^17). For each it does a fixed number of binary searches over
// that address's time-sorted columns, each of at most 18 steps (log2 of MAX_ROWS_PER_ADDRESS):
// one for the newest row, one over the callsign changes, one over the reacquired times, and at
// most MAX_TRAIL (16) for the history dots; so a frame is at most 2^17 * 19 searches of 18 steps
// (about 4.5 * 10^7 steps), with no loop over rows. Nothing is looked at that is not found by
// searching, so the work does not depend on the length of the reacquired window or on how many
// rows an address holds beyond the logarithm.
//
// Rows of one address with the same time: the stable sort of `store_finish` keeps them in the
// order the tracker wrote them, and the newest-row search (`partition_point(t <= time)`) picks the
// last of them, so the later row written wins.

/// Most reports `callsigns_build` takes: the same bound as the clip window of the shell.
pub const MAX_CALLSIGN_REPORTS: usize = 1 << 25;

/// Why the store or the callsigns could not be built (nothing is built from the input then).
// A7.2: cold struct, size guard waived (errors are rare).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowsError {
    /// A column is not as long as the first.
    ColumnLength {
        column: &'static str,
        got: usize,
        want: usize,
    },
    /// The input is bigger than a named limit.
    TooMany {
        what: &'static str,
        wanted: usize,
        max: usize,
    },
    /// A configuration value is outside its range.
    Config(&'static str),
}

impl fmt::Display for RowsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RowsError::ColumnLength { column, got, want } => write!(
                f,
                "column {column} has {got} entries where the others have {want}"
            ),
            RowsError::TooMany { what, wanted, max } => {
                write!(f, "{wanted} {what} is over the limit of {max}")
            }
            RowsError::Config(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for RowsError {}

/// The tracker output columns the display needs, as plain slices of one length. A7.2: cold struct
/// (a view, one per drain).
#[derive(Clone, Copy, Debug)]
pub struct RowColumns<'a> {
    pub t_ns: &'a [i64],
    pub addr: &'a [u32],
    pub ident: &'a [u16],
    pub ident_ns: &'a [i64],
    pub ident_conflict: &'a [u8],
    pub lat_e7: &'a [i32],
    pub lon_e7: &'a [i32],
    pub vel_east_mps: &'a [f32],
    pub vel_north_mps: &'a [f32],
    /// 95% horizontal radius, metres (the shell reduces the covariance); NaN when not computed.
    pub r95_m: &'a [f64],
    pub alt_ft: &'a [f32],
    /// `RF_*` bits.
    pub flags: &'a [u8],
}

/// One stored row, the columns the display needs, ready to show (fixed point where it is state).
/// Plain data (A1).
///
/// A9 exception: `r95_m` is an `f32`, the tracker's own output precision, used only to size a
/// drawn circle; every other column is fixed point.
// Budget: one per kept tracker row, built per row on the way into the columns of an address
// (the store itself is struct-of-arrays, A3). 40 bytes packed with no padding after field
// reordering. A7.1: +8 bytes over the former 32-byte record carries the code, source time, and
// conflict state so the display shows the observation age without claiming it is current.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct Row {
    pub t_ns: i64,
    pub ident_ns: i64,
    pub lat_e7: i32,
    pub lon_e7: i32,
    /// Feet, or [`NO_ALT`]; a real altitude is never `NO_ALT`.
    pub alt_ft: i32,
    /// 95% radius, metres; NaN when it could not be computed.
    pub r95_m: f32,
    pub ident: u16,
    /// Ground speed 0.1 kt, or [`NO_U16`].
    pub gs_dkt: u16,
    /// Track 0.1 degree, or [`NO_U16`].
    pub track_ddeg: u16,
    /// `RF_*` bits.
    pub flags: u8,
    pub ident_conflict: u8,
}
const _: () = assert!(core::mem::size_of::<Row>() == 40);

/// The rows of one address in columns (A3), sharing an index; in time order once `store_finish`
/// (shell) has run. Visible to this crate only, for the shell's builder (A4, A5).
// A7.2: cold struct, size guard waived (one per address, struct-of-arrays).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct AddrRows {
    pub(crate) t_ns: Vec<i64>,
    pub(crate) ident: Vec<u16>,
    pub(crate) ident_ns: Vec<i64>,
    pub(crate) ident_conflict: Vec<u8>,
    pub(crate) lat_e7: Vec<i32>,
    pub(crate) lon_e7: Vec<i32>,
    pub(crate) alt_ft: Vec<i32>,
    pub(crate) r95_m: Vec<f32>,
    pub(crate) gs_dkt: Vec<u16>,
    pub(crate) track_ddeg: Vec<u16>,
    pub(crate) flags: Vec<u8>,
    /// The times of the rows flagged `RF_REACQUIRED`, in time order once finished: the newest one
    /// at or before a time is found by binary search (C7).
    pub(crate) reacq_t: Vec<i64>,
}

/// Rows by address. Capacities: [`MAX_STORE_ADDRESSES`], [`MAX_ROWS_PER_ADDRESS`],
/// [`MAX_STORE_ROWS`]; reaching one refuses the row and counts it in [`StoreStats`].
// A7.2: cold struct, size guard waived (one per run); the per-address columns are SoA (A3).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RowStore {
    pub(crate) by_addr: BTreeMap<u32, AddrRows>,
    pub(crate) rows: usize,
}

/// Rows in the store.
pub fn store_rows(s: &RowStore) -> usize {
    s.rows
}

/// Addresses in the store.
pub fn store_addresses(s: &RowStore) -> usize {
    s.by_addr.len()
}

/// What adding rows did, by reason (E6). Plain record; additive with [`store_stats_add`].
// A7.2: cold struct, size guard waived (one per drain).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StoreStats {
    /// Rows offered.
    pub rows_in: u64,
    /// Rows earlier than the window asked for: not needed by any frame, not a loss.
    pub before_window: u64,
    /// Rows stored.
    pub kept: u64,
    /// LOSS: rows refused because a column they need was missing (cannot happen once the lengths
    /// are checked, and counted so that it cannot be silent).
    pub missing_column: u64,
    /// LOSS: rows with an invalid Mode 3/A code, observation time, or conflict-state combination.
    pub invalid_identity: u64,
    /// LOSS: rows refused because the store already held [`MAX_STORE_ADDRESSES`] addresses.
    pub address_table_full: u64,
    /// LOSS: rows refused because their address already held [`MAX_ROWS_PER_ADDRESS`] rows.
    pub address_rows_full: u64,
    /// Altitudes beyond the range of `i32` feet, clamped to it (one above [`NO_ALT`] at the low end).
    pub alt_clamped: u64,
    /// Rows flagged as having an altitude whose altitude is not a number: shown as absent.
    pub alt_not_finite: u64,
    /// Ground speeds at or beyond the largest the field holds, clamped to it.
    pub speed_clamped: u64,
    /// 95% radii that are not a number or do not fit `f32`: stored as unknown (NaN), and the
    /// data block of an uncertain target says `R95?`.
    pub r95_bad: u64,
    /// Rows whose velocity is not a number: stored with the ground speed and the track unknown.
    pub velocity_not_finite: u64,
}

/// `a` plus `b`, every count saturating.
pub fn store_stats_add(a: &mut StoreStats, b: &StoreStats) {
    a.rows_in = a.rows_in.saturating_add(b.rows_in);
    a.before_window = a.before_window.saturating_add(b.before_window);
    a.kept = a.kept.saturating_add(b.kept);
    a.missing_column = a.missing_column.saturating_add(b.missing_column);
    a.invalid_identity = a.invalid_identity.saturating_add(b.invalid_identity);
    a.address_table_full = a.address_table_full.saturating_add(b.address_table_full);
    a.address_rows_full = a.address_rows_full.saturating_add(b.address_rows_full);
    a.alt_clamped = a.alt_clamped.saturating_add(b.alt_clamped);
    a.alt_not_finite = a.alt_not_finite.saturating_add(b.alt_not_finite);
    a.speed_clamped = a.speed_clamped.saturating_add(b.speed_clamped);
    a.r95_bad = a.r95_bad.saturating_add(b.r95_bad);
    a.velocity_not_finite = a.velocity_not_finite.saturating_add(b.velocity_not_finite);
}

/// The rows of `store_stats` that are work lost: refused for want of room or of a column.
pub fn store_loss(s: &StoreStats) -> u64 {
    s.missing_column
        .saturating_add(s.invalid_identity)
        .saturating_add(s.address_table_full)
        .saturating_add(s.address_rows_full)
}

/// An altitude as whole feet, clamped to `NO_ALT + 1 ..= i32::MAX`; the flag says it was clamped.
pub(crate) fn alt_to_ft(alt: f32) -> (i32, bool) {
    let r = oa_ieee::round(f64::from(alt));
    let lo = f64::from(NO_ALT) + 1.0;
    if r < lo {
        (NO_ALT + 1, true)
    } else if r > f64::from(i32::MAX) {
        (i32::MAX, true)
    } else {
        (r as i32, false)
    }
}

/// The callsign events of one address in time order (A3): from `t_ns` on, the callsign is
/// `callsign`, or two different ones were heard at exactly that instant (`conflict`).
// A7.2: cold struct, size guard waived (one per address, struct-of-arrays).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct AddrCalls {
    pub(crate) t_ns: Vec<i64>,
    pub(crate) callsign: Vec<[u8; 8]>,
    pub(crate) conflict: Vec<bool>,
}

/// Callsigns by address.
// A7.2: cold struct, size guard waived (one per run); the per-address columns are SoA (A3).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Callsigns {
    pub(crate) by_addr: BTreeMap<u32, AddrCalls>,
}

/// What building the callsigns did, by reason (E6).
// A7.2: cold struct, size guard waived (one per run).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CallsignStats {
    pub reports_in: u64,
    /// Reports with no callsign (a first byte of NUL): nothing to record, not a loss.
    pub empty: u64,
    /// Instants at which one address had two different callsigns: shown as a conflict.
    pub conflicts: u64,
    /// Changes of callsign recorded (a repeat of the callsign already heard is not one).
    pub changes: u64,
}

/// What [`callsign_at`] found.
// A7.2: cold struct, size guard waived (one per target, on the stack).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallsignAt {
    /// The newest callsign at or before the time; all NUL when none, or when `conflict`.
    pub callsign: [u8; 8],
    /// Two different callsigns were heard at the newest instant.
    pub conflict: bool,
}

/// The newest callsign of `addr` at or before `t_ns`: all NUL and no conflict when none.
pub fn callsign_at(c: &Callsigns, addr: u32, t_ns: i64) -> CallsignAt {
    let none = CallsignAt {
        callsign: [0; 8],
        conflict: false,
    };
    let Some(v) = c.by_addr.get(&addr) else {
        return none;
    };
    let n = v.t_ns.partition_point(|&t| t <= t_ns);
    let found = n.checked_sub(1).and_then(|i| {
        v.callsign
            .get(i)
            .zip(v.conflict.get(i))
            .map(|(&callsign, &conflict)| CallsignAt { callsign, conflict })
    });
    found.unwrap_or(none)
}

/// Changes of callsign recorded for `addr` (a repeat of the same callsign is not one).
pub fn callsign_changes(c: &Callsigns, addr: u32) -> usize {
    c.by_addr.get(&addr).map_or(0, |v| v.t_ns.len())
}

/// How rows become targets. Plain data; A7.2: cold struct (one per run).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConvertConfig {
    /// History positions per target, at most [`MAX_TRAIL`].
    pub trail_n: u8,
    /// Time between history positions, ns, not negative.
    pub trail_step_ns: i64,
    /// Rows this old (ns before the time asked) mean the track has ended: not shown. Not negative.
    pub stale_ns: i64,
    /// How far back a reacquisition is looked for, seconds: the display's reacquired window, at
    /// most [`MAX_REACQ_AGE_S`] so that an age never equals the absent value.
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

/// Refuses a configuration outside its ranges (E8).
pub fn convert_check(cfg: &ConvertConfig) -> Result<(), RowsError> {
    if usize::from(cfg.trail_n) > MAX_TRAIL {
        return Err(RowsError::Config("trail length above the maximum"));
    }
    if cfg.trail_step_ns < 0 || cfg.stale_ns < 0 {
        return Err(RowsError::Config("a trail step or stale time below zero"));
    }
    if cfg.reacq_window_s > MAX_REACQ_AGE_S {
        return Err(RowsError::Config(
            "reacquired window above the largest age (65534 s)",
        ));
    }
    Ok(())
}

/// What a conversion left out. Plain record; A7.2: cold struct (one per frame).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConvertStats {
    /// Addresses with a row at or before the time, but too old (tracks that ended). Not a loss.
    pub ended: u32,
    /// Targets placed off the map area, kept out of the set so they cannot use its capacity. Not a
    /// loss.
    pub off_map: u32,
    /// Targets put in the set (the set counts those it refused itself).
    pub added: u32,
    /// LOSS: a reacquisition whose age does not fit the field (cannot happen for a checked
    /// configuration).
    pub reacq_age_unrepresentable: u32,
    /// LOSS: history positions not looked up because the time arithmetic overflowed.
    pub time_overflow: u32,
    /// LOSS: rows whose position is not on the globe, not the scope's edge.
    pub invalid_position: u32,
    /// LOSS: rows with a valid position that does not project, not the scope's edge.
    pub unprojectable: u32,
    /// LOSS: history dots left out because their position is not on the globe (the target is
    /// kept, only that dot is dropped).
    pub trail_dropped: u32,
    /// LOSS: a stored row whose columns could not all be read (cannot happen: they have one length).
    pub columns_inconsistent: u32,
}

/// The newest row at or before `t_ns`.
fn newest(v: &AddrRows, t_ns: i64) -> Option<usize> {
    v.t_ns.partition_point(|&t| t <= t_ns).checked_sub(1)
}

/// Ground speed in 0.1 knot and track in 0.1 degree of a velocity; absent values are [`NO_U16`].
/// The track is absent when the ground speed is under [`MIN_TRACK_KT`]. A speed beyond the field is
/// the largest the field holds (`NO_U16 - 1`), never the absent value.
pub fn velocity_of(east_mps: f32, north_mps: f32) -> (u16, u16) {
    if !east_mps.is_finite() || !north_mps.is_finite() {
        return (NO_U16, NO_U16);
    }
    let (e, n) = (f64::from(east_mps), f64::from(north_mps));
    let kt = oa_math::hypot(e, n) * MPS_TO_KT;
    let gs = oa_ieee::round(kt * 10.0);
    let gs = if gs < f64::from(NO_U16) {
        gs as u16
    } else {
        NO_U16 - 1
    };
    if kt < MIN_TRACK_KT {
        return (gs, NO_U16);
    }
    let deg = oa_math::rem_euclid(oa_math::atan2(e, n).to_degrees(), 360.0);
    let dd = oa_ieee::round(deg * 10.0) as u16;
    (gs, if dd >= 3600 { 0 } else { dd })
}

/// The row `i` of `v` as a record.
fn row_at(v: &AddrRows, i: usize) -> Option<Row> {
    Some(Row {
        t_ns: *v.t_ns.get(i)?,
        ident: *v.ident.get(i)?,
        ident_ns: *v.ident_ns.get(i)?,
        ident_conflict: *v.ident_conflict.get(i)?,
        lat_e7: *v.lat_e7.get(i)?,
        lon_e7: *v.lon_e7.get(i)?,
        alt_ft: *v.alt_ft.get(i)?,
        r95_m: *v.r95_m.get(i)?,
        gs_dkt: *v.gs_dkt.get(i)?,
        track_ddeg: *v.track_ddeg.get(i)?,
        flags: *v.flags.get(i)?,
    })
}

/// The target for the newest row `i` of `v` at time `t_ns`.
fn target_of(
    v: &AddrRows,
    row: &Row,
    addr: u32,
    calls: &Callsigns,
    t_ns: i64,
    cfg: &ConvertConfig,
    st: &mut ConvertStats,
) -> Target {
    let mut flags = 0;
    if row.flags & RF_COAST != 0 {
        flags |= T_COAST;
    }
    if row.flags & RF_UNCERTAIN != 0 {
        flags |= T_UNCERTAIN;
    }
    let call = callsign_at(calls, addr, t_ns);
    if call.conflict {
        flags |= T_CS_CONFLICT;
    }
    // Newest reacquired row at or before the time, by binary search over the address's
    // reacquired times (sorted); inside the window or not at all. A difference that does not fit
    // an i64 is longer than any window.
    let window = i64::from(cfg.reacq_window_s) * NS_PER_S;
    let mut reacq_age_s = NO_REACQ;
    if let Some(age) = v
        .reacq_t
        .partition_point(|&rt| rt <= t_ns)
        .checked_sub(1)
        .and_then(|k| v.reacq_t.get(k))
        .and_then(|&rt| t_ns.checked_sub(rt))
        .filter(|&a| a <= window)
    {
        match u16::try_from(age / NS_PER_S)
            .ok()
            .filter(|&a| a <= MAX_REACQ_AGE_S)
        {
            Some(a) => reacq_age_s = a,
            None => st.reacq_age_unrepresentable += 1,
        }
    }
    let mut trail = [(0, 0); MAX_TRAIL];
    let mut trail_len = 0u8;
    for k in 1..=i64::from(cfg.trail_n.min(MAX_TRAIL as u8)) {
        let Some(tk) = k
            .checked_mul(cfg.trail_step_ns)
            .and_then(|d| t_ns.checked_sub(d))
        else {
            st.time_overflow += 1;
            break;
        };
        let hit = newest(v, tk)
            .and_then(|j| row_at(v, j))
            .filter(|r| tk.checked_sub(r.t_ns).is_some_and(|d| d <= cfg.stale_ns));
        if let Some(r) = hit {
            // A bad history position drops that dot only (E5, E6); the target stays.
            if !position_valid(r.lat_e7, r.lon_e7) {
                st.trail_dropped += 1;
            } else if let Some(slot) = trail.get_mut(usize::from(trail_len)) {
                *slot = (r.lat_e7, r.lon_e7);
                trail_len += 1;
            }
        }
    }
    let (ident, ident_age_s, ident_state) = if row.ident_ns > t_ns || row.ident_ns == NO_IDENT_NS {
        (NO_IDENT, u32::MAX, IDENT_UNKNOWN)
    } else if row.ident_conflict != 0 {
        (NO_IDENT, u32::MAX, IDENT_CONFLICT)
    } else if row.ident == NO_IDENT {
        (NO_IDENT, u32::MAX, IDENT_UNKNOWN)
    } else {
        let age_ns = t_ns.checked_sub(row.ident_ns).unwrap_or(i64::MAX);
        let age_s = u32::try_from(age_ns / NS_PER_S).unwrap_or(u32::MAX);
        (row.ident, age_s, IDENT_OBSERVED)
    };
    Target {
        lat_e7: row.lat_e7,
        lon_e7: row.lon_e7,
        alt_ft: row.alt_ft,
        addr,
        ident,
        ident_age_s,
        ident_state,
        r95_m: row.r95_m,
        trail,
        callsign: call.callsign,
        track_ddeg: row.track_ddeg,
        gs_dkt: row.gs_dkt,
        reacq_age_s,
        flags,
        trail_len,
    }
}

/// Fills `set` (which the caller has cleared) with the targets shown at `t_ns`, in address order.
/// A set that fills up counts what it refuses; targets off the map are not offered to it. The
/// work is at most [`MAX_STORE_ADDRESSES`] addresses times a fixed number of binary searches
/// (see the analysis above). Allocates nothing.
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
        let Some(i) = newest(v, t_ns) else {
            continue;
        };
        let Some(row) = row_at(v, i) else {
            st.columns_inconsistent += 1;
            continue;
        };
        // A difference beyond i64 is older than any stale time.
        if t_ns
            .checked_sub(row.t_ns)
            .is_none_or(|age| age > cfg.stale_ns)
        {
            st.ended += 1;
            continue;
        }
        match map_position(proj, row.lat_e7, row.lon_e7) {
            MapPos::On(_) => {}
            MapPos::Off => {
                st.off_map += 1;
                continue;
            }
            MapPos::Invalid => {
                st.invalid_position += 1;
                continue;
            }
            MapPos::Unprojectable => {
                st.unprojectable += 1;
                continue;
            }
        }
        let t = target_of(v, &row, addr, calls, t_ns, cfg, &mut st);
        // A refusal is counted by the set and shown on the frame.
        if targets_push(set, &t).is_ok() {
            st.added += 1;
        }
    }
    st
}
