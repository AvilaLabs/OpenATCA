//! Opt-in bounded per-received-second SFDPS processing timing. Shell only: bins are grouped by
//! injected receive timestamps, while elapsed durations come from `Instant` around frame work.

const NS_PER_S: i64 = 1_000_000_000;
/// At most one day of receive-second bins can be retained for one timing report.
pub const MAX_TIMING_BINS: usize = 86_400;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimingBoundsError {
    pub measure_from_ns: i64,
    pub from_ns: i64,
    pub to_ns: i64,
    pub bins: Option<usize>,
}

/// Summary for one inclusive receive-time window. A7.2: bounded offline diagnostic result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReplayTimingReport {
    pub measure_from_ns: i64,
    pub measure_to_ns: i64,
    pub expected_seconds: usize,
    pub observed_seconds: usize,
    pub empty_seconds: usize,
    /// SFDPS MESSAGE frames entering processing, including parse and decoder refusals.
    /// Document-sealed removals, other frame kinds, read errors, and gated frames are excluded.
    pub timed_frames: u64,
    pub first_received_ns: Option<i64>,
    pub last_received_ns: Option<i64>,
    pub processing_wall_ns: u64,
    /// Nearest-rank p95 of summed processing wall time for observed receive-second bins.
    pub p95_observed_second_ns: u64,
    pub max_observed_second_ns: u64,
}

/// Bounded SoA accumulator. Bin zero is `measure_from_ns.div_euclid(NS_PER_S)`; the last bin is
/// `measure_to_ns.div_euclid(NS_PER_S)`, matching the replay's inclusive `--to` gate.
pub(crate) struct TimingBins {
    measure_from_ns: i64,
    measure_to_ns: i64,
    first_second: i64,
    frames: Vec<u64>,
    wall_ns: Vec<u64>,
    timed_frames: u64,
    first_received_ns: Option<i64>,
    last_received_ns: Option<i64>,
}

impl TimingBins {
    pub(crate) fn new(
        from_ns: i64,
        measure_from_ns: i64,
        to_ns: i64,
    ) -> Result<Self, TimingBoundsError> {
        let invalid_time = from_ns <= 0
            || to_ns == i64::MAX
            || measure_from_ns < from_ns
            || measure_from_ns > to_ns;
        if invalid_time {
            return Err(TimingBoundsError {
                measure_from_ns,
                from_ns,
                to_ns,
                bins: None,
            });
        }
        let first_second = measure_from_ns.div_euclid(NS_PER_S);
        let last_second = to_ns.div_euclid(NS_PER_S);
        let bins = last_second
            .checked_sub(first_second)
            .and_then(|n| n.checked_add(1))
            .and_then(|n| usize::try_from(n).ok());
        let Some(count) = bins.filter(|n| *n > 0 && *n <= MAX_TIMING_BINS) else {
            return Err(TimingBoundsError {
                measure_from_ns,
                from_ns,
                to_ns,
                bins,
            });
        };
        Ok(Self {
            measure_from_ns,
            measure_to_ns: to_ns,
            first_second,
            frames: vec![0; count],
            wall_ns: vec![0; count],
            timed_frames: 0,
            first_received_ns: None,
            last_received_ns: None,
        })
    }

    pub(crate) fn bin(&self, recv_ns: i64) -> Option<usize> {
        if recv_ns < self.measure_from_ns || recv_ns > self.measure_to_ns {
            return None;
        }
        let second = recv_ns.div_euclid(NS_PER_S);
        usize::try_from(second.checked_sub(self.first_second)?).ok()
    }

    pub(crate) fn observe(&mut self, bin: usize, recv_ns: i64, elapsed_ns: u128) {
        let (Some(frames), Some(wall_ns)) = (self.frames.get_mut(bin), self.wall_ns.get_mut(bin))
        else {
            return;
        };
        *frames = frames.saturating_add(1);
        *wall_ns = wall_ns.saturating_add(u64::try_from(elapsed_ns).unwrap_or(u64::MAX));
        self.first_received_ns = Some(self.first_received_ns.map_or(recv_ns, |n| n.min(recv_ns)));
        self.last_received_ns = Some(self.last_received_ns.map_or(recv_ns, |n| n.max(recv_ns)));
        self.timed_frames = self.timed_frames.saturating_add(1);
    }

    pub(crate) fn report(&self) -> ReplayTimingReport {
        let observed = self.frames.iter().filter(|&&n| n > 0).count();
        let mut observed_work: Vec<u64> = self
            .frames
            .iter()
            .zip(&self.wall_ns)
            .filter_map(|(&frames, &wall_ns)| (frames > 0).then_some(wall_ns))
            .collect();
        observed_work.sort_unstable();
        let p95 = if observed_work.is_empty() {
            0
        } else {
            let rank = (observed_work.len() * 95).div_ceil(100).saturating_sub(1);
            observed_work.get(rank).copied().unwrap_or(0)
        };
        ReplayTimingReport {
            measure_from_ns: self.measure_from_ns,
            measure_to_ns: self.measure_to_ns,
            expected_seconds: self.frames.len(),
            observed_seconds: observed,
            empty_seconds: self.frames.len().saturating_sub(observed),
            timed_frames: self.timed_frames,
            first_received_ns: self.first_received_ns,
            last_received_ns: self.last_received_ns,
            processing_wall_ns: self.wall_ns.iter().copied().fold(0, u64::saturating_add),
            p95_observed_second_ns: p95,
            max_observed_second_ns: observed_work.last().copied().unwrap_or(0),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const S: i64 = NS_PER_S;

    #[test]
    fn received_second_binning_is_inclusive_bounded_and_order_independent() {
        let from = 10 * S;
        let start = 11 * S + 250;
        let to = 13 * S;
        let mut forward = TimingBins::new(from, start, to).unwrap();
        let mut reverse = TimingBins::new(from, start, to).unwrap();
        let samples = [(start, 10), (start + 100, 15), (12 * S + 1, 20), (to, 30)];
        for (recv, elapsed) in samples {
            let bin = forward.bin(recv).unwrap();
            forward.observe(bin, recv, elapsed);
        }
        for (recv, elapsed) in samples.into_iter().rev() {
            let bin = reverse.bin(recv).unwrap();
            reverse.observe(bin, recv, elapsed);
        }
        let report = forward.report();
        assert_eq!(report, reverse.report());
        assert_eq!((report.expected_seconds, report.observed_seconds), (3, 3));
        assert_eq!(report.empty_seconds, 0);
        assert_eq!(report.timed_frames, 4);
        assert_eq!(report.first_received_ns, Some(start));
        assert_eq!(report.last_received_ns, Some(to));
        assert_eq!(report.processing_wall_ns, 75);
        assert_eq!(report.p95_observed_second_ns, 30);
        assert_eq!(report.max_observed_second_ns, 30);
        assert!(forward.bin(start - 1).is_none());
        assert_eq!(forward.bin(to), Some(2));
        assert!(forward.bin(to + 1).is_none());
    }

    #[test]
    fn empty_seconds_and_empty_window_have_exact_counts() {
        let mut bins = TimingBins::new(5 * S, 5 * S, 8 * S).unwrap();
        bins.observe(bins.bin(5 * S).unwrap(), 5 * S, 100);
        bins.observe(bins.bin(8 * S).unwrap(), 8 * S, 300);
        let report = bins.report();
        assert_eq!(report.expected_seconds, 4);
        assert_eq!(report.observed_seconds, 2);
        assert_eq!(report.empty_seconds, 2);
        assert_eq!(report.p95_observed_second_ns, 300);

        let empty = TimingBins::new(5 * S, 5 * S, 8 * S).unwrap().report();
        assert_eq!(empty.timed_frames, 0);
        assert_eq!(empty.observed_seconds, 0);
        assert_eq!(empty.empty_seconds, 4);
        assert_eq!(empty.p95_observed_second_ns, 0);
        assert_eq!(empty.max_observed_second_ns, 0);
    }

    #[test]
    fn bounds_reject_bad_signed_times_and_ranges_over_the_named_limit() {
        for (from, start, to) in [
            (0, 1, S),
            (1, 0, S),
            (1, S, 0),
            (10, 9, S),
            (10, S, S - 1),
            (1, S, i64::MAX),
            (1, 1, i64::MAX),
        ] {
            assert!(TimingBins::new(from, start, to).is_err());
        }
        assert!(TimingBins::new(1, 1, (MAX_TIMING_BINS as i64 + 1) * S).is_err());
        assert!(TimingBins::new(1, S, S).is_ok());
    }
}
