//! Per-second processing time of a run against the traffic it handled (the M9 interim marker:
//! national tracking in real time). Core (pure): the shell measures the time of each update with
//! its own clock and hands the nanoseconds here; nothing in this module reads a clock.
//!
//! A second of traffic is a whole second of the measurements' time of applicability. Work is
//! queued the way a live feed would queue it: a second's work starts when the previous second's
//! work is finished, and a second finishes in real time when the backlog it leaves is zero.

/// The most seconds one log holds (two days); later seconds are counted in `dropped`.
pub const MAX_SECONDS: usize = 2 * 86_400;

/// Nanoseconds in a second.
const NS_PER_S: u64 = 1_000_000_000;

/// Reports and processing time of each second, from the first second with traffic.
#[derive(Debug, Clone, Default)]
// A7.2: cold struct, size guard waived
pub struct SecondLog {
    /// Unix second of the first entry.
    pub first_s: i64,
    /// Processing nanoseconds of each second.
    pub work_ns: Vec<u64>,
    /// Measurements of each second.
    pub reports: Vec<u32>,
    /// Confirmed tracks alive at the end of each second (0 until set).
    pub tracks: Vec<u32>,
    /// Entries refused because the log was full or the second was before the first.
    pub dropped: u64,
}

impl SecondLog {
    /// Adds one measurement of second `sec` that took `ns` nanoseconds to process.
    pub fn add(&mut self, sec: i64, ns: u64) {
        if self.work_ns.is_empty() {
            self.first_s = sec;
        }
        let Some(idx) = sec
            .checked_sub(self.first_s)
            .and_then(|d| usize::try_from(d).ok())
        else {
            self.dropped += 1;
            return;
        };
        if idx >= MAX_SECONDS {
            self.dropped += 1;
            return;
        }
        if self.work_ns.len() <= idx {
            self.work_ns.resize(idx + 1, 0);
            self.reports.resize(idx + 1, 0);
            self.tracks.resize(idx + 1, 0);
        }
        if let (Some(w), Some(r)) = (self.work_ns.get_mut(idx), self.reports.get_mut(idx)) {
            *w = w.saturating_add(ns);
            *r = r.saturating_add(1);
        }
    }

    /// Records the confirmed-track count at the end of second `sec`.
    pub fn set_tracks(&mut self, sec: i64, n: u32) {
        let idx = sec
            .checked_sub(self.first_s)
            .and_then(|d| usize::try_from(d).ok());
        if let Some(t) = idx.and_then(|i| self.tracks.get_mut(i)) {
            *t = n;
        }
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
    pub work_total_ns: u64,
    /// Seconds whose work alone took longer than the second.
    pub over_one_second: usize,
    /// Seconds that finished after their end because of work queued behind earlier seconds.
    pub finished_late: usize,
    /// The longest the queue got behind the traffic, nanoseconds.
    pub backlog_max_ns: u64,
    /// Peak confirmed tracks at the end of a second.
    pub tracks_peak: u32,
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
        backlog = (backlog + w).saturating_sub(NS_PER_S);
        s.finished_late += usize::from(backlog > 0);
        s.backlog_max_ns = s.backlog_max_ns.max(backlog);
    }
    s.reports_mean = s.reports as f64 / s.seconds.max(1) as f64;
    let mut sorted = log.work_ns.clone();
    sorted.sort_unstable();
    s.work_p50_ns = rank(&sorted, 50);
    s.work_p99_ns = rank(&sorted, 99);
    s.tracks_peak = log.tracks.iter().copied().max().unwrap_or(0);
    s
}

/// The csv of a log: one line per second.
pub fn seconds_csv(log: &SecondLog) -> String {
    let mut o = String::from("unix_second,reports,work_us,confirmed_tracks\n");
    for (i, ((w, r), t)) in log
        .work_ns
        .iter()
        .zip(log.reports.iter())
        .zip(log.tracks.iter())
        .enumerate()
    {
        o.push_str(&format!(
            "{},{r},{:.1},{t}\n",
            log.first_s + i as i64,
            *w as f64 / 1e3
        ));
    }
    o
}
