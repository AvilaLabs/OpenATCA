#![forbid(unsafe_code)]
//! OpenATCA Core replays recorded ADS-B surveillance through the OpenATCA tracker.
//!
//! Open a clip with [`SurveillanceReplay::open`] and call [`SurveillanceReplay::next_frame`]
//! to consume the tracker updates. The clip determines the replay clock. Each update retains
//! the timestamp assigned by the tracker.
//!
//! ```no_run
//! use openatca_core::{ReplaySettings, SurveillanceReplay};
//!
//! # fn main() -> Result<(), openatca_core::ReplayError> {
//! let mut replay = SurveillanceReplay::open("clip.oaf", ReplaySettings::default())?;
//! while let Some(frame) = replay.next_frame() {
//!     for track in frame.tracks() {
//!         println!("{track:?}");
//!     }
//! }
//! # Ok(())
//! # }
//! ```

use std::fmt;

use oa_track::{
    adsb::{adsb_measurements, DEFAULT_ADSB_CONFIG},
    output_clear, output_len, output_new,
    score::{interval_stats, scoring_config},
    tracker_new, tracker_stats, tracker_update, TrackError, TrackOutput, Tracker, TrackerStats,
    DEFAULT_CONFIG, O_COAST, O_HAS_ALT, O_MOVE_UNCONFIRMED, O_REACQUIRED, O_UNCERTAIN,
};
use oa_truth::clip::{read_clip_counted, ClipError};

/// Default maximum input file size: 64 MiB, enough for an offline sample while bounding reads.
pub const DEFAULT_MAX_CLIP_BYTES: u64 = 64 * 1024 * 1024;
/// Default maximum rows claimed by frame headers: 500,000, keeping decoded replay memory bounded.
pub const DEFAULT_MAX_CLIP_ROWS: u64 = 500_000;

/// Settings for bounded offline replay. As in scope replay, the tracker uses the clip's
/// measured report intervals when they are available.
#[derive(Clone, Copy, Debug)]
pub struct ReplaySettings {
    /// Maximum input file size at open, in bytes.
    pub max_clip_bytes: u64,
    /// Maximum rows claimed by clip frame headers.
    pub max_clip_rows: u64,
    /// Maximum emitted rows held for one timestamp group. Excess rows are counted as loss.
    pub output_capacity: usize,
}

impl Default for ReplaySettings {
    fn default() -> Self {
        Self {
            max_clip_bytes: DEFAULT_MAX_CLIP_BYTES,
            max_clip_rows: DEFAULT_MAX_CLIP_ROWS,
            output_capacity: DEFAULT_CONFIG.out_capacity,
        }
    }
}

/// Failure to open the clip or initialize the configured tracker.
#[derive(Debug)]
pub enum ReplayError {
    /// The clip could not be opened or read. Holds the reader's diagnostic.
    ClipIo(String),
    /// The clip declares too many bytes.
    ClipTooLarge { bytes: u64, limit: u64 },
    /// The clip claims too many report rows.
    TooManyReports { rows: u64, limit: u64 },
    /// The tracker rejected a configuration field. Holds the field name.
    TrackerConfiguration(&'static str),
}

impl fmt::Display for ReplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ClipIo(message) => f.write_str(message),
            Self::ClipTooLarge { bytes, limit } => {
                write!(f, "clip is {bytes} bytes, over the limit of {limit}")
            }
            Self::TooManyReports { rows, limit } => {
                write!(
                    f,
                    "clip claims {rows} report rows, over the limit of {limit}"
                )
            }
            Self::TrackerConfiguration(field) => write!(f, "invalid tracker setting: {field}"),
        }
    }
}

impl std::error::Error for ReplayError {}

/// Counts from reading and adapting the input, including on an otherwise empty replay.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputHealth {
    /// Damaged frames or frames with unreadable ADS-B payloads.
    pub damaged_or_undecodable_frames: u64,
    /// Records discarded by the lenient clip decoder.
    pub records_dropped: u64,
    /// Rows rejected by the ADS-B quality gates before tracking.
    pub quality_rejected: u64,
    /// Rows dropped by the sealed-window guard.
    pub sealed_window_dropped: u64,
    /// Measurements available for replay after decoding and adaptation.
    pub measurements: usize,
}

/// Cumulative tracker health at the time this frame was produced.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReplayHealth {
    /// Tracker output rows lost to buffer overflow or an unrepresentable position or covariance.
    pub rows_lost: u64,
    /// Measurements refused by validation, capacity, duplication or the out-of-order window.
    pub tracker_refusals: u64,
    /// Measurements rejected by the track position gate.
    pub jump_rejected: u64,
    /// Numerical failures counted by the tracker.
    pub numerical_refused: u64,
    /// Tracks confirmed so far, including tracks that have since ended.
    pub confirmed_tracks: u64,
}

/// A typed public copy of one emitted tracker row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackUpdate {
    /// Timestamp of this emitted estimate, Unix nanoseconds.
    pub time_ns: i64,
    /// Aircraft address, with bit 24 set for a non-ICAO address.
    pub address: u32,
    /// Latitude in 1e-7 degree units.
    pub latitude_e7: i32,
    /// Longitude in 1e-7 degree units.
    pub longitude_e7: i32,
    /// Barometric altitude in feet, absent when no barometric estimate is available.
    pub barometric_altitude_ft: Option<f32>,
    /// Tracker's estimated 95% horizontal uncertainty radius in metres.
    pub uncertainty_radius_m: f64,
    /// Whether this estimate is coasting between measurements.
    pub coasting: bool,
    /// Whether the tracker marked this estimate uncertain.
    pub uncertain: bool,
    /// Whether this row marks reacquisition after a gap, uncertainty or a re-anchor.
    pub reacquired: bool,
    /// Whether a large change in position is awaiting corroboration.
    pub move_unconfirmed: bool,
}

/// A group of tracker rows emitted while processing one measurement timestamp.
/// Each emitted row carries its own timestamp, which can differ from the input timestamp.
pub struct ReplayFrame<'a> {
    /// Timestamp shared by the measurements processed for this group.
    pub time_ns: i64,
    rows: &'a TrackOutput,
    /// Cumulative tracker health after processing the group.
    pub health: ReplayHealth,
}

impl ReplayFrame<'_> {
    /// Iterates the rows emitted during this measurement group, in tracker emission order.
    pub fn tracks(&self) -> impl Iterator<Item = TrackUpdate> + '_ {
        (0..output_len(self.rows)).filter_map(|i| row_at(self.rows, i))
    }
}

/// Offline replay of ADS-B reports from a bounded `.oaf` clip.
pub struct SurveillanceReplay {
    measurements: Vec<oa_track::Measurement>,
    cursor: usize,
    tracker: Tracker,
    output: TrackOutput,
    input_health: InputHealth,
}

impl SurveillanceReplay {
    /// Opens and decodes an ADS-B `.oaf` clip using the supplied file and row limits.
    pub fn open(path: &str, settings: ReplaySettings) -> Result<Self, ReplayError> {
        let (reports, bad_frames, dropped, sealed) =
            read_clip_counted(path, settings.max_clip_bytes, settings.max_clip_rows).map_err(
                |e| match e {
                    ClipError::Io(message) => ReplayError::ClipIo(message),
                    ClipError::TooManyBytes { bytes, cap } => {
                        ReplayError::ClipTooLarge { bytes, limit: cap }
                    }
                    ClipError::TooManyRows { rows, cap } => {
                        ReplayError::TooManyReports { rows, limit: cap }
                    }
                },
            )?;
        let (measurements, adapter_stats) = adsb_measurements(&reports, &DEFAULT_ADSB_CONFIG);
        let quality_rejected = adapter_stats
            .removed
            .iter()
            .copied()
            .fold(0u64, u64::saturating_add);
        let mut tracker_config = interval_stats(&measurements).map_or(DEFAULT_CONFIG, |interval| {
            scoring_config(&DEFAULT_CONFIG, &interval)
        });
        tracker_config.out_capacity = settings.output_capacity;
        let tracker = tracker_new(&tracker_config)
            .map_err(|TrackError::BadConfig(field)| ReplayError::TrackerConfiguration(field))?;
        let output = output_new(settings.output_capacity);
        let input_health = InputHealth {
            damaged_or_undecodable_frames: bad_frames,
            records_dropped: dropped,
            quality_rejected,
            sealed_window_dropped: sealed,
            measurements: measurements.len(),
        };
        Ok(Self {
            measurements,
            cursor: 0,
            tracker,
            output,
            input_health,
        })
    }

    /// Accumulated clip and quality-gate counters, even when there are no measurement groups.
    pub fn input_health(&self) -> InputHealth {
        self.input_health
    }

    /// Advances one equal-timestamp measurement group. The output allocation is cleared and
    /// reused on each call. A group can produce zero rows, for example during track initiation.
    pub fn next_frame(&mut self) -> Option<ReplayFrame<'_>> {
        let first = self.measurements.get(self.cursor)?;
        let time_ns = first.toa_ns;
        output_clear(&mut self.output);
        while let Some(m) = self.measurements.get(self.cursor) {
            if m.toa_ns != time_ns {
                break;
            }
            tracker_update(&mut self.tracker, m, &mut self.output);
            self.cursor += 1;
        }
        Some(ReplayFrame {
            time_ns,
            rows: &self.output,
            health: health(&self.output, tracker_stats(&self.tracker)),
        })
    }
}

fn health(out: &TrackOutput, stats: &TrackerStats) -> ReplayHealth {
    let refusals = stats
        .refused
        .iter()
        .copied()
        .fold(0u64, u64::saturating_add)
        .saturating_add(stats.capacity_refused)
        .saturating_add(stats.late_window)
        .saturating_add(stats.late_ring)
        .saturating_add(stats.duplicates);
    ReplayHealth {
        rows_lost: out
            .overflow
            .saturating_add(stats.rows_unrepresentable)
            .saturating_add(stats.nan_covariance),
        tracker_refusals: refusals,
        jump_rejected: stats.jump_rejected,
        numerical_refused: stats.numerical_refused,
        confirmed_tracks: stats.confirmed,
    }
}

fn row_at(out: &TrackOutput, i: usize) -> Option<TrackUpdate> {
    let flags = *out.flags.get(i)?;
    let cov = out.pos_cov_m2.get(i)?;
    let radius = oa_track::r95_m_of(cov);
    Some(TrackUpdate {
        time_ns: *out.t_ns.get(i)?,
        address: *out.addr.get(i)?,
        latitude_e7: *out.lat_e7.get(i)?,
        longitude_e7: *out.lon_e7.get(i)?,
        barometric_altitude_ft: if flags & O_HAS_ALT != 0 {
            out.alt_ft.get(i).copied()
        } else {
            None
        },
        uncertainty_radius_m: radius,
        coasting: flags & O_COAST != 0,
        uncertain: flags & O_UNCERTAIN != 0,
        reacquired: flags & O_REACQUIRED != 0,
        move_unconfirmed: flags & O_MOVE_UNCONFIRMED != 0,
    })
}
