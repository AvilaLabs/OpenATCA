//! Plain data of the tracker interface: the measurement, the configuration, the output tables
//! and the counters. Core (pure). Hot structs carry exact size guards (A7).

use std::fmt;

use crate::params::{
    COAST_AFTER_INTERVALS, DROP_AFTER_NS, EXPECTED_INTERVAL_NS, INIT_M, INIT_N, LOG_CAPACITY,
    MAX_TRACKS, OOO_WINDOW_NS, OUT_CAPACITY, RING_DEPTH, TICK_NS,
};

/// Absent value of an altitude or rate column.
pub const NONE_I32: i32 = i32::MIN;
/// Absent address: a measurement without an address cannot be keyed to an ADS-B track.
pub const NO_ADDR: u32 = u32::MAX;
/// Absent identity (Mode A) code.
pub const NO_IDENT: u16 = u16::MAX;

/// [`Measurement::flags`]: the velocity fields are present.
pub const M_HAS_VEL: u8 = 1;
/// [`Measurement::flags`]: the altitude is geometric (height above the ellipsoid), not
/// barometric. The vertical filter follows barometric altitude, so a geometric altitude is
/// counted and not applied.
pub const M_ALT_GEOMETRIC: u8 = 2;

/// One sensor measurement: the interface radar plugs into later. `toa_ns` is the time of
/// applicability (Unix ns), never the arrival time.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct Measurement {
    /// Time of applicability, Unix ns.
    pub toa_ns: i64,
    /// Position covariance in the local east-north plane, `[ee, en, nn]`, m^2.
    pub pos_cov_m2: [f32; 3],
    /// Velocity covariance, `[ee, en, nn]`, (m/s)^2; zero unless [`M_HAS_VEL`].
    pub vel_cov_m2s2: [f32; 3],
    /// True east and north velocity, m/s; meaningful only with [`M_HAS_VEL`].
    pub vel_east_mps: f32,
    pub vel_north_mps: f32,
    pub lat_e7: i32,
    pub lon_e7: i32,
    /// Altitude, feet, or [`NONE_I32`].
    pub alt_ft: i32,
    /// Vertical rate, feet per minute, or [`NONE_I32`].
    pub vrate_fpm: i32,
    /// 24-bit aircraft address (bit 24 set for a non-ICAO address), or [`NO_ADDR`].
    pub addr: u32,
    /// Altitude quantisation step, feet (25, 100); 0 if the altitude has none.
    pub alt_quant_ft: u16,
    /// Sensor identifier.
    pub sensor: u16,
    /// Identity (Mode A) code, or [`NO_IDENT`].
    pub ident: u16,
    /// [`M_HAS_VEL`] and [`M_ALT_GEOMETRIC`].
    pub flags: u8,
}

// Budget: 8 (time) + 12 + 12 (covariances) + 8 (velocity) + 5 x 4 (position, altitude, rate,
// address) + 3 x 2 (quantisation, sensor, identity) + 1 (flags) = 67 packed; 72 with alignment.
// A7.1: three more bytes were cheaper than reordering around the 8-byte time.
const _: () = assert!(core::mem::size_of::<Measurement>() == 72);

/// Why a measurement was refused (E5). Indexes [`TrackerStats::refused`].
pub const REFUSE_NAMES: [&str; REFUSE_KINDS] = [
    "position_out_of_range",
    "bad_position_covariance",
    "bad_velocity",
    "bad_altitude",
    "no_address",
];
/// Number of refusal reasons.
pub const REFUSE_KINDS: usize = 5;
pub const REF_POSITION: usize = 0;
pub const REF_POS_COV: usize = 1;
pub const REF_VELOCITY: usize = 2;
pub const REF_ALTITUDE: usize = 3;
pub const REF_NO_ADDRESS: usize = 4;

/// Why a track ended. Indexes [`TrackerStats::ended`] and [`TrackLog::reason`].
pub const END_NAMES: [&str; END_KINDS] =
    ["dropped_no_update", "tentative_expired", "flushed_at_end"];
pub const END_KINDS: usize = 3;
pub const END_DROPPED: u8 = 0;
pub const END_TENTATIVE: u8 = 1;
pub const END_FLUSHED: u8 = 2;

/// Settings of a tracker. Plain record (A1); A7.2: cold struct, size guard waived.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackerConfig {
    /// Simultaneous tracks (the capacity of every per-track table).
    pub max_tracks: usize,
    /// Output table capacity between drains.
    pub out_capacity: usize,
    /// Track log capacity.
    pub log_capacity: usize,
    /// Fixed output rate, ns.
    pub tick_ns: i64,
    /// Expected interval between reports of one aircraft, ns.
    pub expected_interval_ns: i64,
    /// The interval the initiation window is `init_n` times, ns. Equal to the expected interval
    /// for a regular feed; for a thinned feed with a long tail of slow aircraft, a high
    /// percentile of the measured intervals, so slow aircraft can still be confirmed.
    pub init_interval_ns: i64,
    /// Coast after this many expected intervals without an update.
    pub coast_after_intervals: i64,
    /// Drop after this long without an update, ns.
    pub drop_after_ns: i64,
    /// Out-of-order window, ns.
    pub ooo_window_ns: i64,
    /// Reports to confirm (M).
    pub init_m: u8,
    /// Window of the M, in reports (N).
    pub init_n: u8,
    /// Horizontal filter: 0 the interacting pair, 1 to 3 a single straight-model baseline
    /// ([`crate::params::CV_ONLY_Q`]).
    pub h_mode: u8,
}

/// The values of the design.
pub const DEFAULT_CONFIG: TrackerConfig = TrackerConfig {
    max_tracks: MAX_TRACKS,
    out_capacity: OUT_CAPACITY,
    log_capacity: LOG_CAPACITY,
    tick_ns: TICK_NS,
    expected_interval_ns: EXPECTED_INTERVAL_NS,
    init_interval_ns: EXPECTED_INTERVAL_NS,
    coast_after_intervals: COAST_AFTER_INTERVALS,
    drop_after_ns: DROP_AFTER_NS,
    ooo_window_ns: OOO_WINDOW_NS,
    init_m: INIT_M,
    init_n: INIT_N,
    h_mode: 0,
};

impl Default for TrackerConfig {
    fn default() -> Self {
        DEFAULT_CONFIG
    }
}

/// Why a configuration or a call was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackError {
    /// A configuration value is out of range; the name says which.
    BadConfig(&'static str),
}

impl fmt::Display for TrackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TrackError::BadConfig(w) => write!(f, "bad configuration: {w}"),
        }
    }
}

impl std::error::Error for TrackError {}

/// Checks a configuration; the error names the first bad value.
pub fn check_config(c: &TrackerConfig) -> Result<(), TrackError> {
    let bad = |w: &'static str| Err(TrackError::BadConfig(w));
    if c.max_tracks == 0 || c.max_tracks > (1 << 24) {
        return bad("max_tracks");
    }
    if c.out_capacity == 0 || c.log_capacity == 0 {
        return bad("out_capacity or log_capacity");
    }
    if c.tick_ns <= 0 || c.expected_interval_ns <= 0 || c.init_interval_ns <= 0 {
        return bad("tick_ns, expected_interval_ns or init_interval_ns");
    }
    if c.coast_after_intervals <= 0 || c.drop_after_ns <= 0 || c.ooo_window_ns < 0 {
        return bad("coast_after_intervals, drop_after_ns or ooo_window_ns");
    }
    if c.init_m == 0 || c.init_n < c.init_m || c.init_n > 16 {
        return bad("init_m or init_n");
    }
    // Times are compared in ns; keep the products far from overflow.
    if c.drop_after_ns
        .max(c.ooo_window_ns)
        .max(c.expected_interval_ns)
        .max(c.init_interval_ns)
        > 86_400 * 1_000_000_000
    {
        return bad("a time over one day");
    }
    if c.h_mode > 3 {
        return bad("h_mode");
    }
    if RING_DEPTH < 2 {
        return bad("ring depth");
    }
    Ok(())
}

/// [`TrackOutput::flags`]: the track is coasting.
pub const O_COAST: u8 = 1;
/// The altitude columns are meaningful. Without it `alt_ft`, `alt_var_ft2` and `vrate_fpm` are
/// NaN (absent, not 0 ft).
pub const O_HAS_ALT: u8 = 2;
/// The row is a fixed-rate tick, not an update.
pub const O_TICK: u8 = 4;

/// Output rows, struct-of-arrays (A3), bounded: a push past the capacity is counted in
/// `overflow` and the row is lost (C2). The consumer calls [`crate::output_clear`] after reading.
// A7.2: cold struct (a collection of columns), size guard waived
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackOutput {
    pub capacity: usize,
    /// Rows lost because the table was full.
    pub overflow: u64,
    /// Time of the state, Unix ns.
    pub t_ns: Vec<i64>,
    pub addr: Vec<u32>,
    pub lat_e7: Vec<i32>,
    pub lon_e7: Vec<i32>,
    /// True east and north velocity, m/s.
    pub vel_east_mps: Vec<f32>,
    pub vel_north_mps: Vec<f32>,
    /// Position covariance `[ee, en, nn]` in the east-north plane, m^2.
    pub pos_cov_m2: Vec<[f32; 3]>,
    /// Velocity covariance `[ee, en, nn]`, (m/s)^2.
    pub vel_cov_m2s2: Vec<[f32; 3]>,
    /// Turn rate (positive clockwise seen from above), rad/s.
    pub turn_rate_rps: Vec<f32>,
    /// Barometric altitude, feet, and its variance, ft^2; NaN when the row has no altitude
    /// ([`O_HAS_ALT`] clear).
    pub alt_ft: Vec<f32>,
    pub alt_var_ft2: Vec<f32>,
    /// Vertical rate, feet per minute; NaN when the row has no altitude.
    pub vrate_fpm: Vec<f32>,
    /// Horizontal model probabilities `[straight, turn]`.
    pub mu_h: Vec<[f32; 2]>,
    /// Vertical model probabilities `[level, climb or descent]`.
    pub mu_v: Vec<[f32; 2]>,
    /// [`O_COAST`], [`O_HAS_ALT`], [`O_TICK`].
    pub flags: Vec<u8>,
}

/// One row of a [`TrackOutput`] to be pushed. Plain data (A1); A7.2: transient.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OutRow {
    pub t_ns: i64,
    pub addr: u32,
    pub lat_e7: i32,
    pub lon_e7: i32,
    pub vel_east_mps: f32,
    pub vel_north_mps: f32,
    pub pos_cov_m2: [f32; 3],
    pub vel_cov_m2s2: [f32; 3],
    pub turn_rate_rps: f32,
    pub alt_ft: f32,
    pub alt_var_ft2: f32,
    pub vrate_fpm: f32,
    pub mu_h: [f32; 2],
    pub mu_v: [f32; 2],
    pub flags: u8,
}

/// An output table with every column allocated for `capacity` rows.
pub fn output_new(capacity: usize) -> TrackOutput {
    TrackOutput {
        capacity,
        overflow: 0,
        t_ns: Vec::with_capacity(capacity),
        addr: Vec::with_capacity(capacity),
        lat_e7: Vec::with_capacity(capacity),
        lon_e7: Vec::with_capacity(capacity),
        vel_east_mps: Vec::with_capacity(capacity),
        vel_north_mps: Vec::with_capacity(capacity),
        pos_cov_m2: Vec::with_capacity(capacity),
        vel_cov_m2s2: Vec::with_capacity(capacity),
        turn_rate_rps: Vec::with_capacity(capacity),
        alt_ft: Vec::with_capacity(capacity),
        alt_var_ft2: Vec::with_capacity(capacity),
        vrate_fpm: Vec::with_capacity(capacity),
        mu_h: Vec::with_capacity(capacity),
        mu_v: Vec::with_capacity(capacity),
        flags: Vec::with_capacity(capacity),
    }
}

/// Rows in the table.
pub fn output_len(o: &TrackOutput) -> usize {
    o.t_ns.len()
}

/// Removes every row, keeping the allocation; the overflow count is kept.
pub fn output_clear(o: &mut TrackOutput) {
    o.t_ns.clear();
    o.addr.clear();
    o.lat_e7.clear();
    o.lon_e7.clear();
    o.vel_east_mps.clear();
    o.vel_north_mps.clear();
    o.pos_cov_m2.clear();
    o.vel_cov_m2s2.clear();
    o.turn_rate_rps.clear();
    o.alt_ft.clear();
    o.alt_var_ft2.clear();
    o.vrate_fpm.clear();
    o.mu_h.clear();
    o.mu_v.clear();
    o.flags.clear();
}

/// Appends a row, or counts the overflow.
pub(crate) fn output_push(o: &mut TrackOutput, r: &OutRow) {
    if o.t_ns.len() >= o.capacity {
        o.overflow = o.overflow.saturating_add(1);
        return;
    }
    o.t_ns.push(r.t_ns);
    o.addr.push(r.addr);
    o.lat_e7.push(r.lat_e7);
    o.lon_e7.push(r.lon_e7);
    o.vel_east_mps.push(r.vel_east_mps);
    o.vel_north_mps.push(r.vel_north_mps);
    o.pos_cov_m2.push(r.pos_cov_m2);
    o.vel_cov_m2s2.push(r.vel_cov_m2s2);
    o.turn_rate_rps.push(r.turn_rate_rps);
    o.alt_ft.push(r.alt_ft);
    o.alt_var_ft2.push(r.alt_var_ft2);
    o.vrate_fpm.push(r.vrate_fpm);
    o.mu_h.push(r.mu_h);
    o.mu_v.push(r.mu_v);
    o.flags.push(r.flags);
}

/// One finished track (or one flushed at the end of the input), struct-of-arrays, bounded like
/// the output: overflow is counted. Written when a track ends.
// A7.2: cold struct (a collection of columns), size guard waived
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackLog {
    pub capacity: usize,
    pub overflow: u64,
    pub addr: Vec<u32>,
    /// Time of the first report, Unix ns.
    pub first_ns: Vec<i64>,
    /// Time the track was confirmed, Unix ns; `i64::MIN` if it never was.
    pub confirmed_ns: Vec<i64>,
    /// Time of the last applied update, Unix ns.
    pub last_ns: Vec<i64>,
    /// Updates applied.
    pub updates: Vec<u32>,
    /// `END_*`.
    pub reason: Vec<u8>,
}

/// A log with room for `capacity` entries.
pub fn log_new(capacity: usize) -> TrackLog {
    TrackLog {
        capacity,
        overflow: 0,
        addr: Vec::with_capacity(capacity),
        first_ns: Vec::with_capacity(capacity),
        confirmed_ns: Vec::with_capacity(capacity),
        last_ns: Vec::with_capacity(capacity),
        updates: Vec::with_capacity(capacity),
        reason: Vec::with_capacity(capacity),
    }
}

/// The counters of a tracker. Plain record (A1); A7.2: cold struct, size guard waived.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TrackerStats {
    /// Measurements offered.
    pub offered: u64,
    /// Measurements applied to a filter (including those by re-filtering).
    pub applied: u64,
    /// Refused by validation, by reason ([`REFUSE_NAMES`]).
    pub refused: [u64; REFUSE_KINDS],
    /// Same time and sensor as the newest update: counted and dropped.
    pub duplicates: u64,
    /// Older than the out-of-order window: counted and dropped.
    pub late_window: u64,
    /// Inside the window but older than every stored state of the track: counted and dropped.
    pub late_ring: u64,
    /// Applied out of order by re-filtering.
    pub refiltered: u64,
    /// Stored updates re-applied during re-filtering.
    pub replayed: u64,
    /// Position innovation too large; not applied.
    pub jump_rejected: u64,
    /// Filters re-initialised after repeated rejections.
    pub resyncs: u64,
    /// Altitude innovation too large; altitude not applied.
    pub alt_rejected: u64,
    /// Geometric altitudes not applied to the barometric filter.
    pub alt_geometric_skipped: u64,
    /// No free track slot: measurement refused.
    pub capacity_refused: u64,
    /// Tracks started.
    pub started: u64,
    /// Tracks confirmed.
    pub confirmed: u64,
    /// Tracks ended, by reason ([`END_NAMES`]).
    pub ended: [u64; END_KINDS],
    /// Tentative tracks restarted because the second report was not plausible.
    pub init_implausible: u64,
    /// Tentative tracks restarted because the window passed without confirmation.
    pub init_window_expired: u64,
    /// Fixed-rate ticks emitted.
    pub ticks: u64,
    /// Sum and count of the position normalised innovation squared (expected 2 per update).
    pub nis_sum: f64,
    pub nis_n: u64,
}

/// Tag and version of [`output_hash_into`].
pub const OUTPUT_HASH_TAG: &[u8; 5] = b"OATK1";

fn put(h: &mut oa_sha256::Sha256, b: &[u8]) {
    // Hashing a few megabytes cannot exceed the hasher's length limit; a failure would only
    // change the digest, which the determinism checks would then show.
    let _ = h.update(b);
}

/// Writes the canonical serialisation of the output rows to the hasher: the tag, the count, the
/// overflow count, then every column, integers little-endian and floats as their IEEE bits.
/// Equal digests mean the outputs are bit-identical.
pub fn output_hash_into(h: &mut oa_sha256::Sha256, o: &TrackOutput) {
    put(h, OUTPUT_HASH_TAG);
    put(h, &(o.t_ns.len() as u64).to_le_bytes());
    put(h, &o.overflow.to_le_bytes());
    for v in &o.t_ns {
        put(h, &v.to_le_bytes());
    }
    for v in &o.addr {
        put(h, &v.to_le_bytes());
    }
    for v in o.lat_e7.iter().chain(o.lon_e7.iter()) {
        put(h, &v.to_le_bytes());
    }
    for v in o
        .vel_east_mps
        .iter()
        .chain(o.vel_north_mps.iter())
        .chain(o.turn_rate_rps.iter())
    {
        put(h, &v.to_bits().to_le_bytes());
    }
    // The altitude columns of a row without an altitude are hashed as zero, which is what they
    // held before absence was written as NaN: the flags (hashed below) already say "absent", and
    // digests recorded earlier stay valid.
    for col in [&o.alt_ft, &o.alt_var_ft2, &o.vrate_fpm] {
        for (i, v) in col.iter().enumerate() {
            let has = o.flags.get(i).is_some_and(|f| f & O_HAS_ALT != 0);
            put(h, &(if has { *v } else { 0.0 }).to_bits().to_le_bytes());
        }
    }
    for c in o.pos_cov_m2.iter().chain(o.vel_cov_m2s2.iter()) {
        for v in c {
            put(h, &v.to_bits().to_le_bytes());
        }
    }
    for c in o.mu_h.iter().chain(o.mu_v.iter()) {
        for v in c {
            put(h, &v.to_bits().to_le_bytes());
        }
    }
    put(h, &o.flags);
}
