//! The comparison: every difference between the two systems, classified by the fixed thresholds.
//! Pure: plain values in, events out. Truth, when given, is asked only after an event is
//! classified, for the adjudication column.

use std::collections::BTreeMap;

use crate::geo::{adjudicate, oa_at, oa_exact, separation, Closer, OaPoint, Separation};
use crate::model::{callsign_text, EramFlight, EramSample, OaAircraft};
use crate::pairing::{pair, Rule};
use crate::thresholds::THRESHOLDS;

/// The difference classes, in report order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    /// An ERAM flight with no OpenATCA aircraft; the span is its in-box samples.
    UnpairedEram,
    /// An OpenATCA aircraft with no ERAM flight; the span is its in-box rows.
    UnpairedOa,
    /// Two ERAM flights of one aircraft over the same time: the overlap; inside it only the
    /// flight with the earliest first report is compared.
    DupEramTrack,
    /// An address-paired ERAM flight far from the aircraft of its address; its reports in the
    /// span are left out of the position classes.
    AddrContradictsTrack,
    /// A paired aircraft: ERAM reports where OpenATCA has no sample.
    EramOnlySpan,
    /// A paired aircraft: OpenATCA rows outside the span of every ERAM flight (widened by the
    /// start/drop allowance).
    OaOnlySpan,
    /// A gap over the segment gap between two reports of one ERAM flight.
    EramReportGap,
    /// Position beyond 0.5 NM, up to 1 NM.
    PosCross05,
    PosCross10,
    PosAlong05,
    PosAlong10,
    Alt,
    IdBeacon,
    IdCallsign,
    StartTiming,
    DropTiming,
    /// ERAM flags coasting, OpenATCA does not.
    CoastEramOnly,
    /// OpenATCA flags coasting, ERAM does not.
    CoastOaOnly,
}

/// All classes in report order.
pub const CLASSES: [Class; 18] = [
    Class::UnpairedEram,
    Class::UnpairedOa,
    Class::DupEramTrack,
    Class::AddrContradictsTrack,
    Class::EramOnlySpan,
    Class::OaOnlySpan,
    Class::EramReportGap,
    Class::PosCross05,
    Class::PosCross10,
    Class::PosAlong05,
    Class::PosAlong10,
    Class::Alt,
    Class::IdBeacon,
    Class::IdCallsign,
    Class::StartTiming,
    Class::DropTiming,
    Class::CoastEramOnly,
    Class::CoastOaOnly,
];

impl Class {
    pub fn name(self) -> &'static str {
        match self {
            Class::UnpairedEram => "UNPAIRED_ERAM",
            Class::UnpairedOa => "UNPAIRED_OA",
            Class::DupEramTrack => "DUP_ERAM_TRACK",
            Class::AddrContradictsTrack => "ADDR_CONTRADICTS_TRACK",
            Class::EramOnlySpan => "ERAM_ONLY_SPAN",
            Class::OaOnlySpan => "OA_ONLY_SPAN",
            Class::EramReportGap => "ERAM_REPORT_GAP",
            Class::PosCross05 => "POS_CROSS_0P5_TO_1NM",
            Class::PosCross10 => "POS_CROSS_GT_1NM",
            Class::PosAlong05 => "POS_ALONG_0P5_TO_1NM",
            Class::PosAlong10 => "POS_ALONG_GT_1NM",
            Class::Alt => "ALT_GT_300FT",
            Class::IdBeacon => "ID_BEACON",
            Class::IdCallsign => "ID_CALLSIGN",
            Class::StartTiming => "START_TIMING",
            Class::DropTiming => "DROP_TIMING",
            Class::CoastEramOnly => "COAST_ERAM_ONLY",
            Class::CoastOaOnly => "COAST_OA_ONLY",
        }
    }

    /// Whether the event is a position divergence.
    pub fn is_position(self) -> bool {
        matches!(
            self,
            Class::PosCross05 | Class::PosCross10 | Class::PosAlong05 | Class::PosAlong10
        )
    }

    /// Whether the event is an along-track position divergence.
    pub fn is_along(self) -> bool {
        matches!(self, Class::PosAlong05 | Class::PosAlong10)
    }

    /// Whether the event describes a span of time (its value is a duration, seconds).
    pub fn is_span(self) -> bool {
        matches!(
            self,
            Class::UnpairedEram
                | Class::UnpairedOa
                | Class::DupEramTrack
                | Class::AddrContradictsTrack
                | Class::EramOnlySpan
                | Class::OaOnlySpan
                | Class::EramReportGap
        )
    }
}

/// One difference. A value that does not apply to the class is `NaN` (an empty cell).
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    /// The report's flight index (see [`Comparison::flights`]).
    pub flight: u32,
    pub class: Class,
    /// Start time (the ERAM report time for a sample event), Unix ms.
    pub t_ms: i64,
    /// End time for a span; for a timing event the OpenATCA time; else equal to `t_ms`.
    pub end_ms: i64,
    /// The class's statistic: span seconds (padded, see [`padded_ms`]), separation metres,
    /// altitude feet or timing seconds (magnitudes).
    pub value: f64,
    pub cross_m: f64,
    pub along_m: f64,
    pub credit_m: f64,
    pub horiz_m: f64,
    /// Along-track events: whether ground speed times 1 s would bring the along-track
    /// separation under this event's threshold (the ERAM whole-second stamp could explain it).
    pub within_stamp_allowance: Option<bool>,
    /// Signed ERAM minus OpenATCA, feet.
    pub alt_diff_ft: f64,
    pub detail: String,
    pub closer: Closer,
}

fn event(flight: u32, class: Class, t_ms: i64) -> Event {
    Event {
        flight,
        class,
        t_ms,
        end_ms: t_ms,
        value: f64::NAN,
        cross_m: f64::NAN,
        along_m: f64::NAN,
        credit_m: f64::NAN,
        horiz_m: f64::NAN,
        within_stamp_allowance: None,
        alt_diff_ft: f64::NAN,
        detail: String::new(),
        closer: Closer::NoTruth,
    }
}

/// One row of the per-flight table: a pair group, or an unpaired flight or aircraft.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq)]
pub struct FlightRow {
    pub flight: u32,
    /// The aircraft address (OpenATCA's for a group, ERAM's for an ERAM-only flight).
    pub addr: Option<u32>,
    /// `paired`, `eram_only` or `oa_only`.
    pub kind: &'static str,
    /// Pairing rules used, e.g. `address`.
    pub rules: String,
    pub eram_samples: u64,
    pub compared: u64,
    pub worst_cross_m: f64,
    pub worst_along_m: f64,
    pub worst_alt_ft: f64,
    pub events: u64,
    pub eram_only_s: f64,
    pub oa_only_s: f64,
}

/// What was compared, for the summary.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Totals {
    pub eram_flights: u64,
    pub oa_aircraft: u64,
    pub eram_samples: u64,
    pub oa_rows: u64,
    pub groups: u64,
    pub paired_by_address: u64,
    pub paired_by_beacon: u64,
    pub compared: u64,
    pub alt_compared: u64,
    pub alt_not_comparable: u64,
    /// Reports with an OpenATCA row at exactly the report time.
    pub coast_compared: u64,
    pub beacon_compared: u64,
    /// Signed ERAM minus OpenATCA altitude, by ERAM altitude below and at/above 18,000 ft.
    pub alt_diffs_below_fl180: Vec<f64>,
    pub alt_diffs_at_or_above_fl180: Vec<f64>,
    /// Intervals between consecutive reports of the compared ERAM tracks (within a segment).
    pub eram_intervals_ms: Vec<i64>,
}

/// The whole comparison.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq)]
pub struct Comparison {
    /// In (flight, time, class) order.
    pub events: Vec<Event>,
    /// By flight index.
    pub flights: Vec<FlightRow>,
    pub totals: Totals,
}

/// The truth position (degrees) of an address at a time, if there is one.
pub type TruthAt<'a> = &'a dyn Fn(u32, i64) -> Option<(f64, f64)>;

/// Maximal runs of times (in order) with no gap over `gap_ms`, as `(first, last)`.
pub fn segments(times: &[i64], gap_ms: i64) -> Vec<(i64, i64)> {
    segment_ix(times, gap_ms)
        .into_iter()
        .filter_map(|(i, j)| Some((*times.get(i)?, *times.get(j)?)))
        .collect()
}

/// The same runs as index ranges `(first, last)`.
pub fn segment_ix(times: &[i64], gap_ms: i64) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    for (k, &t) in times.iter().enumerate() {
        let prev = k.checked_sub(1).and_then(|p| times.get(p));
        match (out.last_mut(), prev) {
            (Some(s), Some(&p)) if t - p <= gap_ms => s.1 = k,
            _ => out.push((k, k)),
        }
    }
    out
}

fn seconds(ms: i64) -> f64 {
    ms as f64 / 1000.0
}

/// The duration, ms, of the run of samples `i..=j` of `times` with half the local sample
/// interval added at each end: the interval on the end's own side, or the other side's when it
/// has no neighbour; a lone sample gets one interval. An interval is at most the segment gap.
pub fn padded_ms(times: &[i64], i: usize, j: usize) -> i64 {
    let gap = THRESHOLDS.segment_gap_ms;
    let iv = |a: usize, b: usize| match (times.get(a), times.get(b)) {
        (Some(x), Some(y)) => (y - x).abs().min(gap),
        _ => 0,
    };
    let n = times.len();
    let left = if i > 0 {
        iv(i - 1, i)
    } else if i + 1 < n {
        iv(i, i + 1)
    } else {
        0
    };
    let right = if j + 1 < n {
        iv(j, j + 1)
    } else if j > 0 {
        iv(j - 1, j)
    } else {
        0
    };
    let core = match (times.get(i), times.get(j)) {
        (Some(a), Some(b)) => b - a,
        _ => 0,
    };
    core + left / 2 + right / 2
}

/// Compares `eram` with `oa`. `eram` flights and each flight's samples must be in a fixed order
/// (time order within a flight); `oa` rows are in time order per aircraft. With `truth`, each
/// position event carries which system was closer; nothing else changes.
pub fn compare_all(
    eram: &[EramFlight],
    oa: &BTreeMap<u32, OaAircraft>,
    truth: Option<TruthAt<'_>>,
) -> Comparison {
    let pairing = pair(eram, oa);
    let mut events: Vec<Event> = Vec::new();
    let mut flights: Vec<FlightRow> = Vec::new();
    let mut totals = Totals {
        eram_flights: eram.len() as u64,
        oa_aircraft: oa.len() as u64,
        eram_samples: eram.iter().map(|f| f.samples.len() as u64).sum(),
        oa_rows: oa.values().map(|a| a.samples.len() as u64).sum(),
        groups: pairing.groups.len() as u64,
        ..Totals::default()
    };
    for g in &pairing.groups {
        let flight = flights.len() as u32;
        let Some(ac) = oa.get(&g.oa_addr) else {
            continue;
        };
        let flights_of: Vec<(&EramFlight, Rule)> = g
            .eram
            .iter()
            .filter_map(|&(i, r)| eram.get(i).map(|f| (f, r)))
            .collect();
        for &(_, r) in &g.eram {
            match r {
                Rule::Address => totals.paired_by_address += 1,
                Rule::BeaconProximity => totals.paired_by_beacon += 1,
            }
        }
        let rules: std::collections::BTreeSet<&str> =
            g.eram.iter().map(|&(_, r)| r.name()).collect();
        let rules: Vec<&str> = rules.into_iter().collect();
        let mut row = FlightRow {
            flight,
            addr: Some(g.oa_addr),
            kind: "paired",
            rules: rules.join("+"),
            eram_samples: 0,
            compared: 0,
            worst_cross_m: 0.0,
            worst_along_m: 0.0,
            worst_alt_ft: 0.0,
            events: 0,
            eram_only_s: 0.0,
            oa_only_s: 0.0,
        };
        let before = events.len();
        let ctx = Ctx { flight, ac, truth };
        compare_group(&ctx, &flights_of, &mut events, &mut row, &mut totals);
        row.events = (events.len() - before) as u64;
        flights.push(row);
    }
    for &i in &pairing.unpaired_eram {
        let Some(f) = eram.get(i) else { continue };
        let flight = flights.len() as u32;
        let times: Vec<i64> = f.samples.iter().map(|s| s.t_ms).collect();
        let (Some(&a), Some(&b)) = (times.first(), times.last()) else {
            continue;
        };
        let mut e = event(flight, Class::UnpairedEram, a);
        e.end_ms = b;
        e.value = seconds(padded_ms(&times, 0, times.len() - 1));
        e.detail = format!("samples={}", f.samples.len());
        let v = e.value;
        events.push(e);
        flights.push(lone_row(
            flight,
            f.addr,
            "eram_only",
            f.samples.len() as u64,
            v,
            true,
        ));
    }
    for &addr in &pairing.unpaired_oa {
        let Some(ac) = oa.get(&addr) else { continue };
        let flight = flights.len() as u32;
        let times: Vec<i64> = ac.samples.iter().map(|s| s.t_ms).collect();
        let (Some(&a), Some(&b)) = (times.first(), times.last()) else {
            continue;
        };
        let mut e = event(flight, Class::UnpairedOa, a);
        e.end_ms = b;
        e.value = seconds(padded_ms(&times, 0, times.len() - 1));
        e.detail = format!("rows={}", ac.samples.len());
        let v = e.value;
        events.push(e);
        flights.push(lone_row(
            flight,
            Some(addr),
            "oa_only",
            ac.samples.len() as u64,
            v,
            false,
        ));
    }
    events.sort_by(|a, b| {
        (a.flight, a.t_ms, a.class, a.end_ms).cmp(&(b.flight, b.t_ms, b.class, b.end_ms))
    });
    Comparison {
        events,
        flights,
        totals,
    }
}

fn lone_row(
    flight: u32,
    addr: Option<u32>,
    kind: &'static str,
    samples: u64,
    span_s: f64,
    eram: bool,
) -> FlightRow {
    FlightRow {
        flight,
        addr,
        kind,
        rules: String::new(),
        eram_samples: if eram { samples } else { 0 },
        compared: 0,
        worst_cross_m: f64::NAN,
        worst_along_m: f64::NAN,
        worst_alt_ft: f64::NAN,
        events: 1,
        eram_only_s: if eram { span_s } else { 0.0 },
        oa_only_s: if eram { 0.0 } else { span_s },
    }
}

/// Runs of uncovered times as one span event each (a covered time ends a run, and so does a gap
/// over the segment gap); returns the total of the spans' seconds.
fn span_events(
    flight: u32,
    class: Class,
    times: &[i64],
    covered: &[bool],
    out: &mut Vec<Event>,
) -> f64 {
    let gap = THRESHOLDS.segment_gap_ms;
    let mut total_s = 0.0;
    let mut run: Option<(usize, usize)> = None;
    let mut flush = |run: &mut Option<(usize, usize)>, out: &mut Vec<Event>| {
        if let Some((i, j)) = run.take() {
            if let (Some(&a), Some(&b)) = (times.get(i), times.get(j)) {
                let mut e = event(flight, class, a);
                e.end_ms = b;
                e.value = seconds(padded_ms(times, i, j));
                e.detail = format!("reports={}", j - i + 1);
                total_s += e.value;
                out.push(e);
            }
        }
    };
    for (k, &t) in times.iter().enumerate() {
        if covered.get(k).copied().unwrap_or(true) {
            flush(&mut run, out);
            continue;
        }
        match run {
            Some((_, j)) if times.get(j).is_some_and(|&p| t - p <= gap) => {
                run = run.map(|(i, _)| (i, k));
            }
            _ => {
                flush(&mut run, out);
                run = Some((k, k));
            }
        }
    }
    flush(&mut run, out);
    total_s
}

/// What one group's comparison shares.
struct Ctx<'a> {
    flight: u32,
    ac: &'a OaAircraft,
    truth: Option<TruthAt<'a>>,
}

/// One ERAM report being compared.
struct Rep<'a> {
    flight_pos: usize,
    s: &'a EramSample,
    p: Option<OaPoint>,
    sep: Option<Separation>,
    contradicts: bool,
}

fn compare_group(
    ctx: &Ctx<'_>,
    flights: &[(&EramFlight, Rule)],
    events: &mut Vec<Event>,
    row: &mut FlightRow,
    totals: &mut Totals,
) {
    let th = &THRESHOLDS;
    let (flight, ac) = (ctx.flight, ctx.ac);
    row.eram_samples = flights.iter().map(|f| f.0.samples.len() as u64).sum();

    // Priority: the earliest first report, then the lower flight index (the position here).
    let spans: Vec<Option<(i64, i64)>> = flights.iter().map(|f| f.0.span()).collect();
    let mut order: Vec<usize> = (0..flights.len()).collect();
    order.sort_by_key(|&k| (spans.get(k).copied().flatten().map_or(i64::MAX, |s| s.0), k));

    // Duplicate tracks: inside an overlap with a higher-priority flight a flight is not compared.
    let mut reps: Vec<Rep<'_>> = Vec::new();
    for (rank, &k) in order.iter().enumerate() {
        let (Some((a, b)), Some(f)) = (spans.get(k).copied().flatten(), flights.get(k)) else {
            continue;
        };
        let mut earlier: Vec<(i64, i64)> = Vec::new();
        for &i in order.iter().take(rank) {
            if let Some((c, d)) = spans.get(i).copied().flatten() {
                let (lo, hi) = (a.max(c), b.min(d));
                if lo <= hi {
                    let mut e = event(flight, Class::DupEramTrack, lo);
                    e.end_ms = hi;
                    e.value = seconds(hi - lo);
                    e.detail = format!(
                        "primary_rank={}",
                        order.iter().position(|&x| x == i).unwrap_or(0)
                    );
                    events.push(e);
                    earlier.push((c, d));
                }
            }
        }
        for s in &f.0.samples {
            if earlier.iter().any(|&(c, d)| s.t_ms >= c && s.t_ms <= d) {
                continue;
            }
            let p = oa_at(&ac.samples, s.t_ms, th);
            let sep = p.as_ref().map(|p| separation(s, p, th));
            reps.push(Rep {
                flight_pos: k,
                s,
                p,
                sep,
                contradicts: false,
            });
        }
    }
    reps.sort_by_key(|r| (r.s.t_ms, r.flight_pos));

    // Address contradicts track: a run of far reports of one address-paired flight.
    for (k, (_, rule)) in flights.iter().enumerate() {
        if *rule != Rule::Address {
            continue;
        }
        let idx: Vec<usize> = reps
            .iter()
            .enumerate()
            .filter(|(_, r)| r.flight_pos == k)
            .map(|(i, _)| i)
            .collect();
        let times: Vec<i64> = idx
            .iter()
            .filter_map(|&i| reps.get(i).map(|r| r.s.t_ms))
            .collect();
        let far: Vec<bool> = idx
            .iter()
            .map(|&i| {
                reps.get(i)
                    .and_then(|r| r.sep)
                    .is_some_and(|s| s.horiz_m > th.contradict_m)
            })
            .collect();
        let mut m = 0;
        while m < far.len() {
            if !far.get(m).copied().unwrap_or(false) {
                m += 1;
                continue;
            }
            let mut n = m;
            while far.get(n + 1).copied().unwrap_or(false) {
                n += 1;
            }
            if n + 1 - m >= th.contradict_run {
                if let (Some(&a), Some(&b)) = (times.get(m), times.get(n)) {
                    let mut e = event(flight, Class::AddrContradictsTrack, a);
                    e.end_ms = b;
                    e.value = seconds(padded_ms(&times, m, n));
                    e.detail = format!("reports={}", n + 1 - m);
                    events.push(e);
                }
                for &i in idx.get(m..=n).unwrap_or(&[]) {
                    if let Some(r) = reps.get_mut(i) {
                        r.contradicts = true;
                    }
                }
            }
            m = n + 1;
        }
    }

    for r in &reps {
        let Some(p) = &r.p else { continue };
        row.compared += 1;
        totals.compared += 1;
        compare_sample(ctx, r, p, flights, events, row, totals);
    }

    // Presence of OpenATCA at ERAM's reports.
    let times: Vec<i64> = reps.iter().map(|r| r.s.t_ms).collect();
    let covered: Vec<bool> = reps.iter().map(|r| r.p.is_some()).collect();
    row.eram_only_s = span_events(flight, Class::EramOnlySpan, &times, &covered, events);

    // OpenATCA rows outside every ERAM flight's span, each widened by the timing threshold and
    // the report interval at that end.
    let widened: Vec<(i64, i64)> = flights
        .iter()
        .filter_map(|f| {
            let t: Vec<i64> = f.0.samples.iter().map(|s| s.t_ms).collect();
            let (a, b) = (*t.first()?, *t.last()?);
            let ia = t.get(1).map_or(0, |&x| x - a);
            let ib = t
                .len()
                .checked_sub(2)
                .and_then(|k| t.get(k))
                .map_or(0, |&x| b - x);
            Some((a - th.timing_ms - ia, b + th.timing_ms + ib))
        })
        .collect();
    let oa_times: Vec<i64> = ac.samples.iter().map(|s| s.t_ms).collect();
    let oa_covered: Vec<bool> = oa_times
        .iter()
        .map(|&t| widened.iter().any(|&(a, b)| t >= a && t <= b))
        .collect();
    row.oa_only_s = span_events(flight, Class::OaOnlySpan, &oa_times, &oa_covered, events);

    // Gaps inside one ERAM flight.
    for (f, _) in flights {
        for w in f.samples.windows(2) {
            if let [a, b] = w {
                if b.t_ms - a.t_ms > th.segment_gap_ms {
                    let mut e = event(flight, Class::EramReportGap, a.t_ms);
                    e.end_ms = b.t_ms;
                    e.value = seconds(b.t_ms - a.t_ms);
                    events.push(e);
                }
            }
        }
    }

    // Start and drop timing: each ERAM segment against the OpenATCA segment it overlaps most,
    // a difference only beyond the threshold plus the ERAM report interval at that end.
    let eram_ix = segment_ix(&times, th.segment_gap_ms);
    let oa_segs = segments(&oa_times, th.segment_gap_ms);
    for &(i, j) in &eram_ix {
        let (Some(&es), Some(&ee)) = (times.get(i), times.get(j)) else {
            continue;
        };
        for w in times.get(i..=j).unwrap_or(&[]).windows(2) {
            if let [a, b] = w {
                totals.eram_intervals_ms.push(b - a);
            }
        }
        let start_iv = if j > i {
            times.get(i + 1).map_or(0, |&x| x - es)
        } else {
            0
        };
        let end_iv = if j > i {
            j.checked_sub(1)
                .and_then(|k| times.get(k))
                .map_or(0, |&x| ee - x)
        } else {
            0
        };
        let mut best: Option<(i64, (i64, i64))> = None;
        for &(os, oe) in &oa_segs {
            let overlap = ee.min(oe) - es.max(os);
            if overlap >= 0 && best.is_none_or(|(b, _)| overlap > b) {
                best = Some((overlap, (os, oe)));
            }
        }
        let Some((_, (os, oe))) = best else { continue };
        for (class, e_t, o_t, iv) in [
            (Class::StartTiming, es, os, start_iv),
            (Class::DropTiming, ee, oe, end_iv),
        ] {
            let delta = o_t - e_t;
            if th.timing_differs(delta, iv) {
                let mut e = event(flight, class, e_t);
                e.end_ms = o_t;
                e.value = seconds(delta.abs());
                e.detail = if delta > 0 { "oa_later" } else { "oa_earlier" }.to_string();
                events.push(e);
            }
        }
    }

    // Callsign, once per ERAM flight against what the adapter was offered near its span.
    for (f, _) in flights {
        let (Some(a), Some(b)) = (f.samples.first(), f.samples.last()) else {
            continue;
        };
        let offered = ac.idents_in(a.t_ms - th.segment_gap_ms, b.t_ms + th.segment_gap_ms);
        let names: Vec<String> = {
            let mut v: Vec<String> = offered
                .iter()
                .filter_map(|p| p.callsign.as_ref().map(callsign_text))
                .filter(|s| !s.is_empty())
                .collect();
            v.sort();
            v.dedup();
            v
        };
        if let Some(cs) = f
            .callsign
            .as_ref()
            .map(callsign_text)
            .filter(|s| !s.is_empty())
        {
            if !names.is_empty() && !names.contains(&cs) {
                let mut e = event(flight, Class::IdCallsign, a.t_ms);
                e.detail = format!("eram={cs} oa={}", names.join(","));
                events.push(e);
            }
        }
    }
}

fn compare_sample(
    ctx: &Ctx<'_>,
    r: &Rep<'_>,
    p: &OaPoint,
    flights: &[(&EramFlight, Rule)],
    events: &mut Vec<Event>,
    row: &mut FlightRow,
    totals: &mut Totals,
) {
    let th = &THRESHOLDS;
    let (flight, s) = (ctx.flight, r.s);
    if let (Some(sep), false) = (r.sep, r.contradicts) {
        row.worst_cross_m = row.worst_cross_m.max(sep.cross_m.abs());
        row.worst_along_m = row.worst_along_m.max(sep.along_m.abs());
        let levels = [
            (sep.cross_m.abs(), [Class::PosCross05, Class::PosCross10]),
            (sep.along_m.abs(), [Class::PosAlong05, Class::PosAlong10]),
        ];
        let mut closer: Option<Closer> = None;
        for (m, classes) in levels {
            let Some(level) = th.pos_level(m) else {
                continue;
            };
            let Some(&class) = classes.get(level) else {
                continue;
            };
            // Adjudication, after the class is decided; one answer per report.
            let c = *closer.get_or_insert_with(|| match ctx.truth {
                Some(q) => adjudicate(s, p, q(ctx.ac.addr, s.t_ms), th),
                None => Closer::NoTruth,
            });
            let mut e = event(flight, class, s.t_ms);
            e.value = m;
            e.cross_m = sep.cross_m;
            e.along_m = sep.along_m;
            e.credit_m = sep.credit_m;
            e.horiz_m = sep.horiz_m;
            if class.is_along() {
                let threshold = th.pos_m.get(level).copied().unwrap_or(f64::NAN);
                e.within_stamp_allowance = Some(m - sep.credit_m <= threshold);
            }
            e.closer = c;
            events.push(e);
        }
    }
    match (s.alt_ft, p.alt_ft) {
        (Some(ea), Some(oa_alt)) => {
            totals.alt_compared += 1;
            let diff = f64::from(ea) - oa_alt;
            if ea < 18_000 {
                totals.alt_diffs_below_fl180.push(diff);
            } else {
                totals.alt_diffs_at_or_above_fl180.push(diff);
            }
            row.worst_alt_ft = row.worst_alt_ft.max(diff.abs());
            if th.alt_differs(diff) {
                let mut e = event(flight, Class::Alt, s.t_ms);
                e.detail = if ea < 18_000 {
                    "eram_below_fl180"
                } else {
                    "eram_at_or_above_fl180"
                }
                .to_string();
                e.value = diff.abs();
                e.alt_diff_ft = diff;
                events.push(e);
            }
        }
        _ => totals.alt_not_comparable += 1,
    }
    // Coast flags only at an OpenATCA row at exactly the ERAM time: a flag is not interpolated.
    if let Some(o) = oa_exact(&ctx.ac.samples, s.t_ms) {
        totals.coast_compared += 1;
        if s.coast != o.coast {
            let class = if s.coast {
                Class::CoastEramOnly
            } else {
                Class::CoastOaOnly
            };
            events.push(event(flight, class, s.t_ms));
        }
    }
    // Beacon per report: ERAM's code in force against the squawk the adapter had in force.
    let eram_code = flights
        .get(r.flight_pos)
        .and_then(|f| f.0.beacon_at(s.t_ms));
    if let (Some(e_code), Some(o_code)) = (eram_code, ctx.ac.squawk_at(s.t_ms)) {
        totals.beacon_compared += 1;
        if e_code != o_code {
            let mut e = event(flight, Class::IdBeacon, s.t_ms);
            e.detail = format!("eram={e_code:04} oa={o_code:04}");
            events.push(e);
        }
    }
}
