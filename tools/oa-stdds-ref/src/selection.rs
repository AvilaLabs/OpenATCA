//! Published-time cohort selection for STDDS rows. The caller applies this before offset fitting.

use crate::model::Rec;

/// A half-open interval of STDDS `mrtTime` values, Unix milliseconds UTC.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeInterval {
    from_ms: i64,
    to_ms: i64,
}

impl TimeInterval {
    /// Builds `[from_ms, to_ms)`, rejecting empty or reversed intervals.
    pub fn new(from_ms: i64, to_ms: i64) -> Result<Self, String> {
        if from_ms >= to_ms {
            return Err("published-time interval requires from-ms < to-ms".to_string());
        }
        Ok(Self { from_ms, to_ms })
    }

    pub fn from_ms(self) -> i64 {
        self.from_ms
    }

    pub fn to_ms(self) -> i64 {
        self.to_ms
    }
}

/// Whether `time_ms` belongs to the interval's half-open span.
pub fn contains(interval: TimeInterval, time_ms: i64) -> bool {
    interval.from_ms <= time_ms && time_ms < interval.to_ms
}

/// Whether the interval overlaps another half-open time span.
pub fn overlaps(interval: TimeInterval, from_ms: i64, to_ms: i64) -> bool {
    interval.from_ms < to_ms && interval.to_ms > from_ms
}

/// Accounting before and after selecting rows. Sealed-window rows are already separately
/// removed and counted by the decoder, and therefore are not in this denominator.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SelectionCounts {
    pub records_after_sealed_filter: u64,
    pub selected_records: u64,
    pub excluded_outside_interval: u64,
    pub excluded_untimed: u64,
}

/// Retains timed rows in the interval; with no interval, retains every timed row.
/// This must run before any fit calculation so out-of-cohort rows cannot influence calibration.
pub fn select_published_time(recs: &mut Vec<Rec>, interval: Option<TimeInterval>) -> SelectionCounts {
    let records_after_sealed_filter = u64::try_from(recs.len()).unwrap_or(u64::MAX);
    let mut counts = SelectionCounts {
        records_after_sealed_filter,
        ..SelectionCounts::default()
    };
    recs.retain(|rec| match rec.t_ms {
        None => {
            counts.excluded_untimed += 1;
            false
        }
        Some(t) if interval.is_some_and(|range| !contains(range, t)) => {
            counts.excluded_outside_interval += 1;
            false
        }
        Some(_) => {
            counts.selected_records += 1;
            true
        }
    });
    counts
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::model::Class;

    fn rec(time: Option<i64>) -> Rec {
        Rec {
            file: 0,
            index: 0,
            class: Class::AdsbAbsent,
            addr: Some(0x123456),
            src: *b"KAAA    ",
            src_present: true,
            track_num: Some(1),
            t_ms: time,
            pos: Some((1.0, 2.0)),
            alt_ft: Some(1000.0),
            vel: Some((1.0, 0.0)),
        }
    }

    #[test]
    fn interval_is_half_open_and_supports_negative_unix_times() {
        let range = TimeInterval::new(-10, 20).unwrap();
        assert!(contains(range, -10));
        assert!(contains(range, 19));
        assert!(!contains(range, -11));
        assert!(!contains(range, 20));
        assert!(TimeInterval::new(20, 20).is_err());
        assert!(TimeInterval::new(21, 20).is_err());
        let sealed = TimeInterval::new(20, 30).unwrap();
        assert!(!overlaps(sealed, 10, 20));
        assert!(overlaps(sealed, 19, 21));
        assert!(!overlaps(sealed, 30, 40));
    }

    #[test]
    fn selection_preserves_cohort_and_accounts_for_every_parsed_row() {
        let mut rows = vec![rec(Some(9)), rec(Some(10)), rec(Some(19)), rec(Some(20)), rec(None)];
        let counts = select_published_time(&mut rows, Some(TimeInterval::new(10, 20).unwrap()));
        assert_eq!(counts.records_after_sealed_filter, 5);
        assert_eq!(counts.selected_records, 2);
        assert_eq!(counts.excluded_outside_interval, 2);
        assert_eq!(counts.excluded_untimed, 1);
        assert_eq!(
            counts.selected_records + counts.excluded_outside_interval + counts.excluded_untimed,
            counts.records_after_sealed_filter
        );
        assert_eq!(rows.iter().map(|r| r.t_ms.unwrap()).collect::<Vec<_>>(), [10, 19]);
    }

    #[test]
    fn no_interval_keeps_all_timed_rows_and_counts_missing_time() {
        let mut rows = vec![rec(Some(-1)), rec(Some(0)), rec(None)];
        let counts = select_published_time(&mut rows, None);
        assert_eq!(counts.records_after_sealed_filter, 3);
        assert_eq!(counts.selected_records, 2);
        assert_eq!(counts.excluded_outside_interval, 0);
        assert_eq!(counts.excluded_untimed, 1);
    }
}
