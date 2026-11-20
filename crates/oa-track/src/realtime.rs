//! Per-second processing time of a run against the traffic it handled (the M9 interim marker:
//! national tracking in real time). Core (pure): the shell measures the time of each update with
//! its own clock and hands the nanoseconds here; nothing in this module reads a clock.
//!
//! A second of traffic is a whole second of the measurements' time of applicability. Queue model:
//! each second's reports form one batch, available when the second ends, processed during the
//! next second, with work left over carried to the one after (backlog = max(0, backlog + work -
//! 1 s)); a second finishes late when backlog is left. A report therefore waits at most 1 s plus
//! the backlog ahead of its batch before processing starts, and because real reports arrive
//! spread through the second and could be processed earlier, the lateness is an upper bound.

use crate::types::{TrackLog, TrackerStats};

/// The most seconds one log holds (two days); later seconds are counted in `dropped`.
pub const MAX_SECONDS: usize = 2 * 86_400;

/// Nanoseconds in a second.
const NS_PER_S: u64 = 1_000_000_000;

/// Reports, processing time and track counts of each second, from the first second with traffic.
/// Seconds are appended in order, so every second between the first and the last has an entry.
#[derive(Debug, Clone, Default)]
// A7.2: cold struct, size guard waived
pub struct SecondLog {
    /// Unix second of the first entry.
    pub first_s: i64,
    /// Processing nanoseconds of each second.
    pub work_ns: Vec<u64>,
    /// Measurements of each second.
    pub reports: Vec<u32>,
    /// Confirmed tracks after the last update of each second (a second with no updates carries
    /// the previous second's count; the tracker is not advanced to the second boundary).
    pub confirmed: Vec<u32>,
    /// Tentative tracks, counted the same way.
    pub tentative: Vec<u32>,
    /// Measurements refused because the second was before the first, not after the previous one,
    /// or more than [`MAX_SECONDS`] from the first. A run with any of these has no valid figures.
    pub dropped: u64,
}

impl SecondLog {
    /// Closes second `sec`: `reports` measurements took `work_ns` nanoseconds in all, and the
    /// tracker then, after the last of those updates, held `confirmed` and `tentative` tracks.
    /// The counts are sampled once per second, so a peak between two samples is not seen. Seconds between the previous entry
    /// and `sec` have no traffic, no work and the previous second's counts (nothing changed them).
    pub fn push_second(
        &mut self,
        sec: i64,
        reports: u32,
        work_ns: u64,
        confirmed: u32,
        tentative: u32,
    ) {
        if self.work_ns.is_empty() {
            self.first_s = sec;
        }
        let idx = sec
            .checked_sub(self.first_s)
            .and_then(|d| usize::try_from(d).ok());
        let Some(idx) = idx.filter(|&i| i >= self.work_ns.len() && i < MAX_SECONDS) else {
            self.dropped += u64::from(reports);
            return;
        };
        let (c, t) = (
            self.confirmed.last().copied().unwrap_or(0),
            self.tentative.last().copied().unwrap_or(0),
        );
        while self.work_ns.len() < idx {
            self.work_ns.push(0);
            self.reports.push(0);
            self.confirmed.push(c);
            self.tentative.push(t);
        }
        self.work_ns.push(work_ns);
        self.reports.push(reports);
        self.confirmed.push(confirmed);
        self.tentative.push(tentative);
    }
}

/// The summary of a [`SecondLog`]. Plain record (A1).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
// A7.2: cold struct, size guard waived
pub struct RealtimeSummary {
    /// Seconds from the first to the last with traffic, inclusive.
    pub seconds: usize,
    /// Seconds with at least one measurement.
    pub busy_seconds: usize,
    pub reports: u64,
    pub reports_mean: f64,
    pub reports_peak: u32,
    /// Unix second of the peak.
    pub reports_peak_s: i64,
    /// Processing time of a second, nanoseconds, nearest-rank percentiles over all seconds.
    pub work_p50_ns: u64,
    pub work_p99_ns: u64,
    pub work_max_ns: u64,
    pub work_max_s: i64,
    /// Nearest-rank percentiles over the seconds with at least one measurement only.
    pub busy_work_p50_ns: u64,
    pub busy_work_p99_ns: u64,
    /// The sum of the per-second timers.
    pub work_total_ns: u64,
    /// Seconds whose work alone took longer than the second.
    pub over_one_second: usize,
    /// Seconds that finished after their end because of work queued behind earlier seconds.
    pub finished_late: usize,
    /// The longest the queue got behind the traffic, nanoseconds.
    pub backlog_max_ns: u64,
    /// The most a report waits before its batch starts processing: 1 s plus the longest backlog.
    pub wait_bound_ns: u64,
    /// Peak confirmed and tentative tracks, sampled once per second after its last update.
    pub confirmed_peak: u32,
    pub tentative_peak: u32,
}

/// Nearest-rank percentile of a sorted slice, `p` in percent.
fn rank(sorted: &[u64], p: usize) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let k = (p * sorted.len()).div_ceil(100).clamp(1, sorted.len());
    sorted.get(k - 1).copied().unwrap_or(0)
}

/// Summarises a log.
pub fn realtime_summary(log: &SecondLog) -> RealtimeSummary {
    let mut s = RealtimeSummary {
        seconds: log.work_ns.len(),
        ..RealtimeSummary::default()
    };
    let mut backlog = 0u64;
    for (i, (&w, &r)) in log.work_ns.iter().zip(log.reports.iter()).enumerate() {
        let sec = log.first_s + i as i64;
        s.reports += u64::from(r);
        s.busy_seconds += usize::from(r > 0);
        if r > s.reports_peak {
            s.reports_peak = r;
            s.reports_peak_s = sec;
        }
        if w > s.work_max_ns {
            s.work_max_ns = w;
            s.work_max_s = sec;
        }
        s.work_total_ns = s.work_total_ns.saturating_add(w);
        s.over_one_second += usize::from(w > NS_PER_S);
        // The second's work starts when the earlier work is done and has the second to finish.
        backlog = backlog.saturating_add(w).saturating_sub(NS_PER_S);
        s.finished_late += usize::from(backlog > 0);
        s.backlog_max_ns = s.backlog_max_ns.max(backlog);
    }
    s.reports_mean = s.reports as f64 / s.seconds.max(1) as f64;
    let mut sorted = log.work_ns.clone();
    sorted.sort_unstable();
    s.work_p50_ns = rank(&sorted, 50);
    s.work_p99_ns = rank(&sorted, 99);
    let mut busy: Vec<u64> = log
        .work_ns
        .iter()
        .zip(log.reports.iter())
        .filter(|(_, &r)| r > 0)
        .map(|(&w, _)| w)
        .collect();
    busy.sort_unstable();
    s.busy_work_p50_ns = rank(&busy, 50);
    s.busy_work_p99_ns = rank(&busy, 99);
    s.wait_bound_ns = NS_PER_S.saturating_add(s.backlog_max_ns);
    s.confirmed_peak = log.confirmed.iter().copied().max().unwrap_or(0);
    s.tentative_peak = log.tentative.iter().copied().max().unwrap_or(0);
    s
}

/// The update window's wall seconds not inside the per-second timers (track counts, `push_second`,
/// batching), or `None` when the timers sum to more than the window (the readings are wrong).
pub fn outside_timers_s(window_s: f64, timers_ns: u64) -> Option<f64> {
    let d = window_s - timers_ns as f64 / 1e9;
    if d.is_nan() || d < 0.0 {
        None
    } else {
        Some(d)
    }
}

/// The CPU source note: empty when every one of the `readings` used the per-thread counter, else
/// the number that fell back to the process's `/proc/self/stat` ticks and the assumed tick rate
/// (the clock tick rate cannot be read without `unsafe` or a new dependency, so 100 Hz is assumed).
pub fn cpu_fallback_note(fallback: usize, readings: usize) -> Option<String> {
    (fallback > 0).then(|| {
        format!(
            "{fallback} of {readings} CPU readings came from /proc/self/stat (whole process, ticks of an assumed 100 Hz clock, 10 ms resolution)"
        )
    })
}

/// The csv of a log: one line per second.
pub fn seconds_csv(log: &SecondLog) -> String {
    let mut o = String::from("unix_second,reports,work_us,confirmed_tracks,tentative_tracks\n");
    for i in 0..log.work_ns.len() {
        let g = |v: &[u32]| v.get(i).copied().unwrap_or(0);
        o.push_str(&format!(
            "{},{},{:.1},{},{}\n",
            log.first_s + i as i64,
            g(&log.reports),
            log.work_ns.get(i).copied().unwrap_or(0) as f64 / 1e3,
            g(&log.confirmed),
            g(&log.tentative)
        ));
    }
    o
}

/// Why a realtime run's figures cannot be trusted, or `None`: measurements dropped from the
/// log, no measurements at all, or table capacity refusals (the run was not at full load).
pub fn realtime_failure(
    log: &SecondLog,
    measurements: usize,
    capacity_refused: u64,
    output_overflow: u64,
    log_overflow: u64,
) -> Option<String> {
    if measurements == 0 {
        return Some(
            "no measurements after the adapter (empty clip, or every report gated out)".to_string(),
        );
    }
    if log.dropped > 0 {
        return Some(format!(
            "{} measurements fall outside the log (before the first second, out of time order, or more than {MAX_SECONDS} s from it): the figures would cover a truncated span",
            log.dropped
        ));
    }
    if capacity_refused > 0 {
        return Some(format!(
            "{capacity_refused} measurements refused for lack of a track slot: not a full-load measurement, raise --max-tracks"
        ));
    }
    if output_overflow > 0 || log_overflow > 0 {
        return Some(format!(
            "{output_overflow} output rows and {log_overflow} track log entries were lost to a full table: the digests then cover truncated data"
        ));
    }
    None
}

/// CPU counter readings (nanoseconds on the process's own counter), in the order the shell takes
/// them: at the start, after reading and the adapter, at the start and end of the update window,
/// and right after the final advance and flush, before any digest is computed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct CpuMarks {
    pub start: u64,
    pub loaded: u64,
    pub loop_start: u64,
    pub looped: u64,
    pub finished: u64,
}

/// CPU seconds and shares of one core over the span of the traffic. Plain record (A1).
#[derive(Debug, Clone, Copy, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct CpuUse {
    /// Reading, decoding and the adapter.
    pub ingest_s: f64,
    /// The update window alone.
    pub update_s: f64,
    /// From the start to the final flush: ingest, set-up, updates and the flush.
    pub total_s: f64,
    /// `update_s` and `total_s` as percent of one core, the denominator being `traffic_s`
    /// (the seconds from the first to the last of the traffic, at least one).
    pub update_pct: f64,
    pub total_pct: f64,
}

/// The CPU use from the readings, or `None` when they are not in the order they were taken
/// (a counter that went backwards means the figures cannot be trusted).
pub fn cpu_use(m: &CpuMarks, traffic_s: f64) -> Option<CpuUse> {
    let in_order = m.start <= m.loaded
        && m.loaded <= m.loop_start
        && m.loop_start <= m.looped
        && m.looped <= m.finished;
    if !in_order {
        return None;
    }
    let s = |ns: u64| ns as f64 / 1e9;
    let span = if traffic_s < 1.0 { 1.0 } else { traffic_s };
    let (update_s, total_s) = (s(m.looped - m.loop_start), s(m.finished - m.start));
    Some(CpuUse {
        ingest_s: s(m.loaded - m.start),
        update_s,
        total_s,
        update_pct: 100.0 * update_s / span,
        total_pct: 100.0 * total_s / span,
    })
}

/// Why the flags given to `oa-track-run` do not go together, or `None`. `--realtime` takes the
/// whole clip through the tracker and nothing else; the flags of the scored run and the flags of
/// the realtime run do not mix.
pub fn check_flags(
    realtime: bool,
    with_output: bool,
    max_tracks: bool,
    scored_run_flags: &[&str],
) -> Option<String> {
    if !realtime {
        for (given, name) in [(with_output, "--with-output"), (max_tracks, "--max-tracks")] {
            if given {
                return Some(format!("{name} needs --realtime"));
            }
        }
    } else if let Some(f) = scored_run_flags.first() {
        return Some(format!("{f} does not apply with --realtime"));
    }
    None
}

/// The SHA-256 of what a run ended with: the track log (every track's address, times, updates and
/// end reason) and the tracker's counters, as lowercase hex. It is not the tracker's state hash
/// (none exists in `oa-track`) and not the digest of the output rows; it is computed after the
/// timed window, so it costs nothing in it.
pub fn run_summary_digest(log: &TrackLog, stats: &TrackerStats) -> String {
    let mut h = oa_sha256::Sha256::new();
    let mut put = |b: &[u8]| {
        let _ = h.update(b);
    };
    put(b"OATKRT1");
    put(&(log.addr.len() as u64).to_le_bytes());
    put(&log.overflow.to_le_bytes());
    for v in &log.addr {
        put(&v.to_le_bytes());
    }
    for v in log
        .first_ns
        .iter()
        .chain(&log.confirmed_ns)
        .chain(&log.last_ns)
    {
        put(&v.to_le_bytes());
    }
    for v in &log.updates {
        put(&v.to_le_bytes());
    }
    put(&log.reason);
    put(format!("{stats:?}").as_bytes());
    crate::canon::hex32(&h.finalize())
}
