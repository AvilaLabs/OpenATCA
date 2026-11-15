//! Per-stage latency of a measurement through the tracker. Core (pure) except where the
//! injected [`Clock`] is read: the tracker itself never sees a clock, the shell passes one in.
//!
//! Every measurement carries the stamps of the stages it passes ([`Stamps`]):
//!
//! 1. its **sensor time** of applicability (`toa_ns`, Unix ns, from the data);
//! 2. its **receive time** (`recv_ns`, Unix ns, from the frame header of the recording);
//! 3. the shell's **monotonic** reading when its frame was **received** (taken from the file),
//!    when its frame was **decoded**, when [`tracker_update`] was **called** and **returned**, and
//!    when the rows it produced were **emitted** (handed to the output consumer).
//!
//! Stages 1 and 2 are wall-clock data; the monotonic readings are only differenced with each
//! other. The log keeps the differences of each stage per measurement (stage 0 as a signed
//! `i64` ns, the others as saturating `u32` ns, at most [`LATENCY_CAPACITY`] measurements; the
//! rest are counted, not stored), so the percentiles are exact over what is stored.
//!
//! **A replay is not live.** In a recording the frame's receive time is the time of its first
//! report (the clip has no receive wall clock, `docs/ADSB_PAYLOAD.md`), so the first stage
//! (receive stamp minus sensor time) is at most 0 and is the frame grouping, not a network; frames are read from a file at once, so no
//! measurement waits for a socket or a busy core, and the "decode done to update done" stage
//! includes only the measurements ahead of it in the same frame. The numbers are what this code
//! costs per stage on this machine, not what a deployment would see.

use crate::tracker::{tracker_update, Tracker};
use crate::types::{Measurement, TrackOutput};

/// A monotonic clock the shell provides, in nanoseconds from any fixed start.
pub trait Clock {
    /// The current reading.
    fn now_ns(&mut self) -> u64;
}

/// A deterministic clock for tests: each reading is the previous one plus `step_ns`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepClock {
    pub now: u64,
    pub step_ns: u64,
}

impl Clock for StepClock {
    fn now_ns(&mut self) -> u64 {
        let t = self.now;
        self.now = self.now.saturating_add(self.step_ns);
        t
    }
}

/// The stamps of one measurement (see the module documentation). Plain data (A1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamps {
    pub toa_ns: i64,
    pub recv_ns: i64,
    pub received: u64,
    pub decoded: u64,
    pub update_start: u64,
    pub updated: u64,
    pub emitted: u64,
}

const _: () = assert!(core::mem::size_of::<Stamps>() == 56);

/// The frame-level part of the stamps, known before the measurement is offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameStamps {
    pub recv_ns: i64,
    pub received: u64,
    pub decoded: u64,
}

/// The stages, as indexes into [`LatencyLog::stage`].
pub const ST_SENSOR_TO_RECEIVE: usize = 0;
pub const ST_RECEIVE_TO_DECODED: usize = 1;
pub const ST_DECODED_TO_UPDATED: usize = 2;
pub const ST_UPDATED_TO_EMITTED: usize = 3;
pub const ST_END_TO_END: usize = 4;
pub const ST_UPDATE_CALL: usize = 5;
/// Number of stages.
pub const STAGES: usize = 6;
/// Names of the stages.
pub const STAGE_NAMES: [&str; STAGES] = [
    "sensor time to receive (data; frame stamp)",
    "receive to decode done",
    "decode done to update done (includes the measurements ahead in the frame)",
    "update done to output emitted",
    "receive to output emitted (end to end)",
    "tracker update call (per measurement, includes tick bursts)",
];

/// Most measurements a log stores (24 bytes each).
pub const LATENCY_CAPACITY: usize = 1 << 20;

/// The per-stage differences of the measurements seen, nanoseconds. Cold container (A7.2).
// A7.2: cold struct, size guard waived
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LatencyLog {
    pub capacity: usize,
    /// Measurements offered to [`latency_record`] after the log was full.
    pub overflow: u64,
    /// Differences that did not fit a `u32` (above about 4.29 s), stored as the maximum.
    pub saturated: u64,
    /// Monotonic differences that were negative (a clock that went back), stored as 0.
    pub negative: u64,
    /// Stage 0 differences that were negative (a frame received before its sensor time: clock
    /// skew); the signed value is stored as it is.
    pub sensor_negative: u64,
    /// Stage 0: receive time minus sensor time, signed, ns.
    pub sensor_to_receive: Vec<i64>,
    /// Stages 1 to 5 (index `stage - 1`), ns.
    pub stage: [Vec<u32>; STAGES - 1],
}

/// A log for at most `capacity` measurements.
pub fn latency_new(capacity: usize) -> LatencyLog {
    LatencyLog {
        capacity,
        sensor_to_receive: Vec::with_capacity(capacity.min(1 << 16)),
        stage: core::array::from_fn(|_| Vec::with_capacity(capacity.min(1 << 16))),
        ..LatencyLog::default()
    }
}

/// Measurements stored.
pub fn latency_count(log: &LatencyLog) -> usize {
    log.sensor_to_receive.len()
}

fn narrow(log: &mut LatencyLog, d: i128) -> u32 {
    if d < 0 {
        log.negative += 1;
        0
    } else if d > i128::from(u32::MAX) {
        log.saturated += 1;
        u32::MAX
    } else {
        d as u32
    }
}

/// Records the stages of one measurement.
pub fn latency_record(log: &mut LatencyLog, s: &Stamps) {
    if latency_count(log) >= log.capacity {
        log.overflow += 1;
        return;
    }
    let skew = s.recv_ns.saturating_sub(s.toa_ns);
    if skew < 0 {
        log.sensor_negative += 1;
    }
    log.sensor_to_receive.push(skew);
    let d = [
        i128::from(s.decoded) - i128::from(s.received),
        i128::from(s.updated) - i128::from(s.decoded),
        i128::from(s.emitted) - i128::from(s.updated),
        i128::from(s.emitted) - i128::from(s.received),
        i128::from(s.updated) - i128::from(s.update_start),
    ];
    for (i, x) in d.into_iter().enumerate() {
        let v = narrow(log, x);
        if let Some(c) = log.stage.get_mut(i) {
            c.push(v);
        }
    }
}

/// Offers one measurement to the tracker, reading `clock` before and after the update and after
/// `emit` (which hands the output rows to the consumer and normally clears them), and records
/// the stamps. The tracker's own behaviour is that of [`tracker_update`]: the clock readings are
/// not seen by it.
pub fn update_timed(
    t: &mut Tracker,
    m: &Measurement,
    frame: &FrameStamps,
    out: &mut TrackOutput,
    clock: &mut dyn Clock,
    emit: &mut dyn FnMut(&mut TrackOutput),
    log: &mut LatencyLog,
) {
    let update_start = clock.now_ns();
    tracker_update(t, m, out);
    let updated = clock.now_ns();
    emit(out);
    let emitted = clock.now_ns();
    latency_record(
        log,
        &Stamps {
            toa_ns: m.toa_ns,
            recv_ns: frame.recv_ns,
            received: frame.received,
            decoded: frame.decoded,
            update_start,
            updated,
            emitted,
        },
    );
}

/// Exact order statistics of one stage, nanoseconds (nearest rank; for stage 0 over the signed
/// values, `min` and `max` the extremes). Plain record (A1); A7.2: cold struct.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StageStats {
    pub n: u64,
    pub min: i64,
    pub p50: i64,
    pub p95: i64,
    pub p99: i64,
    pub max: i64,
}

/// The statistics of stage `stage` over the stored measurements.
pub fn latency_stats(log: &LatencyLog, stage: usize) -> StageStats {
    let mut v: Vec<i64> = match stage {
        0 => log.sensor_to_receive.clone(),
        _ => log
            .stage
            .get(stage - 1)
            .map(|c| c.iter().map(|&x| i64::from(x)).collect())
            .unwrap_or_default(),
    };
    if v.is_empty() {
        return StageStats::default();
    }
    v.sort_unstable();
    let at = |p: usize| {
        let k = (p * v.len()).div_ceil(100).clamp(1, v.len());
        v.get(k - 1).copied().unwrap_or(0)
    };
    StageStats {
        n: v.len() as u64,
        min: v.first().copied().unwrap_or(0),
        p50: at(50),
        p95: at(95),
        p99: at(99),
        max: v.last().copied().unwrap_or(0),
    }
}
