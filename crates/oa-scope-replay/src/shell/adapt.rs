//! The tracker's and the decoder's types as plain columns (**shell**, B1, D3): the one place where
//! `oa_track::TrackOutput` and `oa_adsb::Reports` meet the core. Every function here is shell:
//! each is a field-by-field view or a unit conversion (covariance to 95% radius, the tracker's
//! `O_*` bits to the core's `RF_*` bits), with no decision in it.

use crate::core::rows::{
    CallsignStats, Callsigns, RowColumns, RowsError, RF_COAST, RF_HAS_ALT, RF_REACQUIRED,
    RF_UNCERTAIN,
};
use crate::store::callsigns_build;
use oa_adsb::Reports;
use oa_display::safety::{FeedState, FeedStatus};
use oa_feed::health::{FeedHealth, STATE_CONNECTED, STATE_DISCONNECTED, STATE_UNKNOWN};
use oa_track::fusion::FusionStats;
use oa_track::{r95_m_of, TrackOutput, O_COAST, O_HAS_ALT, O_REACQUIRED, O_UNCERTAIN};

/// Caller-supplied feed freshness limits. The slow threshold is inclusive for `Slow`; the lost
/// threshold is inclusive for `Lost`. No facility-independent defaults are provided (IF-R4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeedFreshnessLimits {
    slow_after_ns: i64,
    lost_after_ns: i64,
}

/// A freshness limit pair that cannot classify `Slow` before `Lost`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeedFreshnessLimitError {
    SlowMustBePositive,
    LostMustExceedSlow,
}

impl FeedFreshnessLimits {
    /// Builds a validated limit pair in nanoseconds.
    pub fn new(slow_after_ns: i64, lost_after_ns: i64) -> Result<Self, FeedFreshnessLimitError> {
        if slow_after_ns <= 0 {
            return Err(FeedFreshnessLimitError::SlowMustBePositive);
        }
        if lost_after_ns <= slow_after_ns {
            return Err(FeedFreshnessLimitError::LostMustExceedSlow);
        }
        Ok(Self {
            slow_after_ns,
            lost_after_ns,
        })
    }
}

/// Converts one health snapshot to the display's plain feed status. `now_ns` must be in the same
/// clock domain as the receive timestamps supplied to `oa_feed::observe_frame`; a future frame or
/// message timestamp is surfaced as `ClockMismatch`, never treated as fresh. A MESSAGE means a framed
/// source message arrived, not that its payload decoded into an accepted track update.
pub fn feed_status_from_health(
    name: [u8; 8],
    health: &FeedHealth,
    now_ns: i64,
    limits: FeedFreshnessLimits,
) -> FeedStatus {
    let state = if !matches!(
        health.state,
        STATE_UNKNOWN | STATE_CONNECTED | STATE_DISCONNECTED
    ) || u128::from(health.messages) + u128::from(health.rejects)
        > u128::from(health.frames)
        || (health.state != STATE_UNKNOWN && health.frames == 0)
        || (health.messages == 0) != health.last_message_recv_ns.is_none()
    {
        FeedState::InvalidHealth
    } else if health.frames > 0 && health.last_recv_ns > now_ns {
        FeedState::ClockMismatch
    } else if let Some(last_message_ns) = health.last_message_recv_ns {
        if last_message_ns > now_ns {
            FeedState::ClockMismatch
        } else if health.state == STATE_DISCONNECTED {
            FeedState::Disconnected
        } else {
            let age_ns = i128::from(now_ns) - i128::from(last_message_ns);
            let age_ms = u32::try_from(age_ns / 1_000_000).ok();
            if age_ns >= i128::from(limits.lost_after_ns) {
                age_ms.map_or(FeedState::LostAgeOverflow, |age_ms| FeedState::Lost {
                    age_ms,
                })
            } else if age_ns >= i128::from(limits.slow_after_ns) {
                age_ms.map_or(FeedState::SlowAgeOverflow, |age_ms| FeedState::Slow {
                    age_ms,
                })
            } else {
                FeedState::Live
            }
        }
    } else if health.state == STATE_DISCONNECTED {
        FeedState::Disconnected
    } else if health.rejects > 0 {
        FeedState::RejectedInput {
            count: health.rejects,
        }
    } else {
        FeedState::NoMessageSeen
    };
    FeedStatus { name, state }
}

// The counters count different, sometimes overlapping work units. Comparing them separately
// preserves the indication without claiming a total number of affected aircraft.
fn radar_work_counters(stats: &FusionStats) -> [u64; 12] {
    [
        stats.bucket_plot_overflow,
        stats.bucket_work_capped,
        stats.assoc_refused,
        stats.prediction_refused,
        stats.cand_capacity,
        stats.track_refused,
        stats.scratch_full,
        stats.anon_plots_ungated,
        stats.init_capped,
        stats.init_plots_refused,
        stats.wide_full,
        stats.init_wide_full,
    ]
}

/// Supplies an extra scope banner when radar association or initiation loses work. Both
/// snapshots must refer to the same fusion instance and span the frame's reporting interval.
/// A decrease makes the work counters inconsistent. Use source freshness telemetry for arrival
/// and connectivity status. Each loss counter remains available in the fusion report.
pub fn radar_work_notice(previous: &FusionStats, current: &FusionStats) -> Option<FeedStatus> {
    let before = radar_work_counters(previous);
    let after = radar_work_counters(current);
    let state = if before.iter().zip(after).any(|(&old, new)| old > new) {
        FeedState::InvalidHealth
    } else if before != after {
        FeedState::CoverageLimited
    } else {
        return None;
    };
    Some(FeedStatus {
        name: *b"RADAR\0\0\0",
        state,
    })
}

/// The two columns that are computed, not viewed: reused from drain to drain. Plain data.
// A7.2: cold struct, size guard waived (one per run).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ColumnBuf {
    r95_m: Vec<f64>,
    flags: Vec<u8>,
}

/// The `RF_*` bits of the tracker's `O_*` bits.
pub fn row_flags(o: u8) -> u8 {
    let mut f = 0;
    for (from, to) in [
        (O_COAST, RF_COAST),
        (O_HAS_ALT, RF_HAS_ALT),
        (O_REACQUIRED, RF_REACQUIRED),
        (O_UNCERTAIN, RF_UNCERTAIN),
    ] {
        if o & from != 0 {
            f |= to;
        }
    }
    f
}

/// The columns of a tracker output the display needs, as slices; the radius and flag columns are
/// filled into `buf`.
pub fn row_columns<'a>(out: &'a TrackOutput, buf: &'a mut ColumnBuf) -> RowColumns<'a> {
    buf.r95_m.clear();
    buf.r95_m.extend(out.pos_cov_m2.iter().map(r95_m_of));
    buf.flags.clear();
    buf.flags.extend(out.flags.iter().map(|&o| row_flags(o)));
    RowColumns {
        t_ns: &out.t_ns,
        addr: &out.addr,
        lat_e7: &out.lat_e7,
        lon_e7: &out.lon_e7,
        vel_east_mps: &out.vel_east_mps,
        vel_north_mps: &out.vel_north_mps,
        r95_m: &buf.r95_m,
        alt_ft: &out.alt_ft,
        flags: &buf.flags,
    }
}

/// The callsigns heard in `reports` (see [`callsigns_build`]).
pub fn callsigns_from_reports(r: &Reports) -> Result<(Callsigns, CallsignStats), RowsError> {
    callsigns_build(&r.time_ns, &r.icao, &r.callsign)
}
