//! The fixed thresholds of every difference class, in one table. A value is a difference only
//! when it is strictly greater than its threshold; a value exactly at it is not.

/// One metre per nautical mile (the exact definition), for the position thresholds.
pub const M_PER_NM: f64 = 1852.0;

/// The thresholds (cold: A7.2).
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Thresholds {
    /// Position divergence levels, metres: 0.5 NM and 1 NM, each for cross-track and along-track.
    pub pos_m: [f64; 2],
    /// Altitude disagreement, feet.
    pub alt_ft: f64,
    /// Track start or drop timing difference, milliseconds, beyond which it counts once the ERAM
    /// report interval at that end is added.
    pub timing_ms: i64,
    /// A gap longer than this ends a track segment, milliseconds.
    pub segment_gap_ms: i64,
    /// OpenATCA is interpolated to an ERAM time only between two rows at most this far apart,
    /// milliseconds; otherwise it has no sample there.
    pub interp_gap_ms: i64,
    /// ERAM stamps positions on whole seconds: the along-track comparison allows ERAM's true
    /// time to differ from its stamp by this much either way, milliseconds.
    pub eram_time_quant_ms: i64,
    /// Beacon pairing: the OpenATCA aircraft must be within this of ERAM's first report,
    /// metres.
    pub pair_radius_m: f64,
    /// Below this ground speed the direction of travel is undefined, metres per second (2 kt).
    pub low_speed_mps: f64,
    /// An address-paired ERAM flight farther than this from the paired aircraft, metres, for at
    /// least `contradict_run` consecutive reports contradicts its address.
    pub contradict_m: f64,
    pub contradict_run: usize,
    /// Adjudication: two distances to truth closer than this are a tie, metres.
    pub tie_m: f64,
}

/// The table.
pub const THRESHOLDS: Thresholds = Thresholds {
    pos_m: [0.5 * M_PER_NM, M_PER_NM],
    alt_ft: 300.0,
    timing_ms: 12_000,
    segment_gap_ms: 30_000,
    interp_gap_ms: 2_000,
    eram_time_quant_ms: 1_000,
    pair_radius_m: 5.0 * M_PER_NM,
    low_speed_mps: 2.0 * M_PER_NM / 3600.0,
    contradict_m: 20.0 * M_PER_NM,
    contradict_run: 3,
    tie_m: 1.0,
};

impl Thresholds {
    /// The position level a distance reaches: `Some(0)` above 0.5 NM up to 1 NM (inclusive),
    /// `Some(1)` above 1 NM.
    pub fn pos_level(&self, metres: f64) -> Option<usize> {
        if metres > self.pos_m[1] {
            Some(1)
        } else if metres > self.pos_m[0] {
            Some(0)
        } else {
            None
        }
    }

    /// Whether an altitude difference (either sign) is a disagreement.
    pub fn alt_differs(&self, diff_ft: f64) -> bool {
        diff_ft.abs() > self.alt_ft
    }

    /// Whether a start or drop time difference (either sign) is a difference: it must exceed the
    /// threshold plus the ERAM report interval at that end, milliseconds.
    pub fn timing_differs(&self, delta_ms: i64, interval_ms: i64) -> bool {
        delta_ms.unsigned_abs() > self.timing_ms.unsigned_abs() + interval_ms.unsigned_abs()
    }

    /// The table as text lines for the summary.
    pub fn lines(&self) -> Vec<String> {
        vec![
            format!(
                "position divergence, cross-track and along-track separately (raw): {:.1} to {:.1} NM and > {:.1} NM",
                self.pos_m[0] / M_PER_NM,
                self.pos_m[1] / M_PER_NM,
                self.pos_m[1] / M_PER_NM
            ),
            format!("altitude disagreement: > {:.0} ft", self.alt_ft),
            format!(
                "track start or drop timing: > {} s plus the ERAM report interval at that end",
                self.timing_ms / 1000
            ),
            format!(
                "track segment: a gap over {} s ends it",
                self.segment_gap_ms / 1000
            ),
            format!(
                "OpenATCA at an ERAM time: interpolated between rows at most {} s apart",
                self.interp_gap_ms / 1000
            ),
            format!(
                "ERAM whole-second stamp: along-track events are flagged when ground speed x {} s would bring them under their threshold",
                self.eram_time_quant_ms / 1000
            ),
            format!(
                "beacon pairing radius: {:.1} NM at ERAM's first report",
                self.pair_radius_m / M_PER_NM
            ),
            format!(
                "direction of travel undefined below {:.2} m/s (2 kt): the whole separation counts as cross-track",
                self.low_speed_mps
            ),
            format!(
                "address contradicts track: ERAM position over {:.0} NM from the paired aircraft for {} or more consecutive reports",
                self.contradict_m / M_PER_NM,
                self.contradict_run
            ),
            format!("adjudication tie: distances to truth within {:.1} m", self.tie_m),
        ]
    }
}
