//! Core (B1): the intervals of one feed over the scored window.

/// An interval over this many nanoseconds is damage (P-006 section 6: 10 minutes). Exactly this
/// long is not over.
pub const OVER_NS: i64 = 600_000_000_000;

/// The most intervals over the limit that are listed; the rest are counted.
pub const GAP_CAP: usize = 1000;

/// The scored window, Unix nanoseconds, both ends inclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub from_ns: i64,
    pub to_ns: i64,
}

impl Window {
    /// `None` unless `from` is before `to`.
    pub fn new(from_ns: i64, to_ns: i64) -> Option<Window> {
        (from_ns < to_ns).then_some(Window { from_ns, to_ns })
    }
}

/// One interval, Unix nanoseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gap {
    pub start_ns: i64,
    pub end_ns: i64,
}

impl Gap {
    pub fn len_ns(&self) -> i64 {
        self.end_ns.saturating_sub(self.start_ns)
    }
}

/// What one feed's times came to.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stats {
    pub window: Window,
    /// Times inside the window.
    pub count: u64,
    pub first_ns: Option<i64>,
    pub last_ns: Option<i64>,
    /// The longest interval, the two window edges included; the earliest of equals.
    pub largest: Option<Gap>,
    /// The intervals over [`OVER_NS`] in time order, at most [`GAP_CAP`].
    pub over: Vec<Gap>,
    /// Every interval over the limit, listed or not.
    pub over_total: u64,
    /// Intervals over the limit that did not fit in `over`.
    pub overflow: u64,
    /// Times earlier than the one before them (left out of the intervals).
    pub out_of_order: u64,
}

/// The state while times arrive in time order.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug)]
pub struct Scan {
    s: Stats,
}

impl Scan {
    pub fn new(window: Window) -> Scan {
        Scan {
            s: Stats {
                window,
                count: 0,
                first_ns: None,
                last_ns: None,
                largest: None,
                over: Vec::new(),
                over_total: 0,
                overflow: 0,
                out_of_order: 0,
            },
        }
    }

    fn record(&mut self, g: Gap) {
        if self.s.largest.is_none_or(|l| g.len_ns() > l.len_ns()) {
            self.s.largest = Some(g);
        }
        if g.len_ns() > OVER_NS {
            self.s.over_total = self.s.over_total.saturating_add(1);
            if self.s.over.len() < GAP_CAP {
                self.s.over.push(g);
            } else {
                self.s.overflow = self.s.overflow.saturating_add(1);
            }
        }
    }

    /// One time. A time outside the window is not counted; one earlier than the last counted
    /// time is counted as out of order and otherwise ignored.
    pub fn observe(&mut self, t_ns: i64) {
        let w = self.s.window;
        if t_ns < w.from_ns || t_ns > w.to_ns {
            return;
        }
        match self.s.last_ns {
            None => {
                self.s.first_ns = Some(t_ns);
                self.record(Gap {
                    start_ns: w.from_ns,
                    end_ns: t_ns,
                });
            }
            Some(prev) if t_ns < prev => {
                self.s.out_of_order = self.s.out_of_order.saturating_add(1);
                return;
            }
            Some(prev) => self.record(Gap {
                start_ns: prev,
                end_ns: t_ns,
            }),
        }
        self.s.last_ns = Some(t_ns);
        self.s.count = self.s.count.saturating_add(1);
    }

    /// Closes the window: the interval from the last time to the end of the window counts.
    pub fn finish(mut self) -> Stats {
        if let Some(last) = self.s.last_ns {
            self.record(Gap {
                start_ns: last,
                end_ns: self.s.window.to_ns,
            });
        }
        self.s
    }
}

/// The times that fall in the window, sorted, as a finished scan.
pub fn scan_unsorted(window: Window, times: &mut [i64]) -> Stats {
    times.sort_unstable();
    let mut sc = Scan::new(window);
    for &t in times.iter() {
        sc.observe(t);
    }
    sc.finish()
}

/// The reasons one feed is damaged, empty when it is not. `counts` are the reader's damage
/// counts by name; any non-zero one is a reason.
pub fn reasons(feed: &str, st: &Stats, counts: &[(String, u64)]) -> Vec<String> {
    let mut out = Vec::new();
    if st.count == 0 {
        out.push(format!("{feed} has no message in the window"));
    }
    if st.over_total > 0 {
        let l = st.largest.map_or(0, |g| g.len_ns());
        out.push(format!(
            "{feed} has {} interval(s) over 600 s, the largest {}",
            st.over_total,
            super::timefmt::secs(l)
        ));
    }
    if st.overflow > 0 {
        out.push(format!(
            "{feed} lists {GAP_CAP} intervals and {} more are not listed",
            st.overflow
        ));
    }
    if st.out_of_order > 0 {
        out.push(format!(
            "{feed} has {} out-of-order time(s)",
            st.out_of_order
        ));
    }
    for (name, n) in counts {
        if *n > 0 {
            out.push(format!("{feed} {name}: {n}"));
        }
    }
    out
}
