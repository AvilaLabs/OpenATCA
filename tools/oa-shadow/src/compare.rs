//! The comparison: every difference between the two systems, classified by the fixed thresholds.
//! Pure: plain values in, events out. Truth, when given, is asked only after an event is
//! classified, for the adjudication column.

use std::collections::BTreeMap;

use crate::geo::{adjudicate, oa_at, separation, Closer, OaPoint};
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
    /// A paired aircraft: ERAM reports where OpenATCA has no sample (first to last such report).
    EramOnlySpan,
    /// A paired aircraft: OpenATCA rows outside ERAM's track by more than the timing threshold.
    OaOnlySpan,
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
pub const CLASSES: [Class; 15] = [
    Class::UnpairedEram,
    Class::UnpairedOa,
    Class::EramOnlySpan,
    Class::OaOnlySpan,
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
            Class::EramOnlySpan => "ERAM_ONLY_SPAN",
            Class::OaOnlySpan => "OA_ONLY_SPAN",
            Class::PosCross05 => "POS_CROSS_GT_0.5NM",
            Class::PosCross10 => "POS_CROSS_GT_1NM",
            Class::PosAlong05 => "POS_ALONG_GT_0.5NM",
            Class::PosAlong10 => "POS_ALONG_GT_1NM",
            Class::Alt => "ALT_GT_300FT",
            Class::IdBeacon => "ID_BEACON",
            Class::IdCallsign => "ID_CALLSIGN",
            Class::StartTiming => "START_GT_12S",
            Class::DropTiming => "DROP_GT_12S",
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

    /// Whether the event describes a span of time (its duration is the statistic).
    pub fn is_span(self) -> bool {
        matches!(
            self,
            Class::UnpairedEram | Class::UnpairedOa | Class::EramOnlySpan | Class::OaOnlySpan
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
    /// The class's statistic: span seconds, separation metres, altitude feet or timing seconds
    /// (magnitudes).
    pub value: f64,
    pub cross_m: f64,
    pub along_raw_m: f64,
    pub along_adj_m: f64,
    pub horiz_adj_m: f64,
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
        along_raw_m: f64::NAN,
        along_adj_m: f64::NAN,
        horiz_adj_m: f64::NAN,
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
    pub worst_along_adj_m: f64,
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
    pub coast_compared: u64,
    /// Signed ERAM minus OpenATCA altitude, by ERAM altitude below and at/above 18,000 ft.
    pub alt_diffs_below_fl180: Vec<f64>,
    pub alt_diffs_at_or_above_fl180: Vec<f64>,
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
    let mut out: Vec<(i64, i64)> = Vec::new();
    for &t in times {
        match out.last_mut() {
            Some(s) if t - s.1 <= gap_ms => s.1 = t.max(s.1),
            _ => out.push((t, t)),
        }
    }
    out
}

fn seconds(ms: i64) -> f64 {
    ms as f64 / 1000.0
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
        let flights_of: Vec<&EramFlight> =
            g.eram.iter().filter_map(|&(i, _)| eram.get(i)).collect();
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
            worst_along_adj_m: 0.0,
            worst_alt_ft: 0.0,
            events: 0,
            eram_only_s: 0.0,
            oa_only_s: 0.0,
        };
        let before = events.len();
        compare_group(
            flight,
            ac,
            &flights_of,
            truth,
            &mut events,
            &mut row,
            &mut totals,
        );
        row.events = (events.len() - before) as u64;
        flights.push(row);
    }
    for &i in &pairing.unpaired_eram {
        let Some(f) = eram.get(i) else { continue };
        let flight = flights.len() as u32;
        let (Some(a), Some(b)) = (f.samples.first(), f.samples.last()) else {
            continue;
        };
        let mut e = event(flight, Class::UnpairedEram, a.t_ms);
        e.end_ms = b.t_ms;
        e.value = seconds(b.t_ms - a.t_ms);
        e.detail = format!("samples={}", f.samples.len());
        events.push(e);
        flights.push(lone_row(
            flight,
            f.addr,
            "eram_only",
            f.samples.len() as u64,
            seconds(b.t_ms - a.t_ms),
            true,
        ));
    }
    for &addr in &pairing.unpaired_oa {
        let Some(ac) = oa.get(&addr) else { continue };
        let flight = flights.len() as u32;
        let (Some(a), Some(b)) = (ac.samples.first(), ac.samples.last()) else {
            continue;
        };
        let mut e = event(flight, Class::UnpairedOa, a.t_ms);
        e.end_ms = b.t_ms;
        e.value = seconds(b.t_ms - a.t_ms);
        e.detail = format!("rows={}", ac.samples.len());
        events.push(e);
        flights.push(lone_row(
            flight,
            Some(addr),
            "oa_only",
            ac.samples.len() as u64,
            seconds(b.t_ms - a.t_ms),
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
        worst_along_adj_m: f64::NAN,
        worst_alt_ft: f64::NAN,
        events: 1,
        eram_only_s: if eram { span_s } else { 0.0 },
        oa_only_s: if eram { 0.0 } else { span_s },
    }
}

/// Coalesces a run of uncovered times into one span event each; `covered` marks each time.
fn span_events(
    flight: u32,
    class: Class,
    marks: &[(i64, bool)],
    gap_ms: i64,
    out: &mut Vec<Event>,
) -> f64 {
    let mut total_s = 0.0;
    let mut run: Option<(i64, i64, u64)> = None;
    let mut flush = |run: &mut Option<(i64, i64, u64)>, out: &mut Vec<Event>| {
        if let Some((a, b, n)) = run.take() {
            let mut e = event(flight, class, a);
            e.end_ms = b;
            e.value = seconds(b - a);
            e.detail = format!("reports={n}");
            total_s += e.value;
            out.push(e);
        }
    };
    for &(t, covered) in marks {
        if covered {
            flush(&mut run, out);
            continue;
        }
        match run.as_mut() {
            Some(r) if t - r.1 <= gap_ms => {
                r.1 = t;
                r.2 += 1;
            }
            _ => {
                flush(&mut run, out);
                run = Some((t, t, 1));
            }
        }
    }
    flush(&mut run, out);
    total_s
}

fn compare_group(
    flight: u32,
    ac: &OaAircraft,
    flights: &[&EramFlight],
    truth: Option<TruthAt<'_>>,
    events: &mut Vec<Event>,
    row: &mut FlightRow,
    totals: &mut Totals,
) {
    let th = &THRESHOLDS;
    // ERAM's samples of every flight of the group, in time order (equal times keep flight order).
    let mut eram: Vec<&EramSample> = flights.iter().flat_map(|f| f.samples.iter()).collect();
    eram.sort_by_key(|s| s.t_ms);
    row.eram_samples = eram.len() as u64;

    // Per report: position, altitude, coast; presence of OpenATCA.
    let mut eram_marks: Vec<(i64, bool)> = Vec::with_capacity(eram.len());
    for s in &eram {
        let Some(p) = oa_at(&ac.samples, s.t_ms, th) else {
            eram_marks.push((s.t_ms, false));
            continue;
        };
        eram_marks.push((s.t_ms, true));
        row.compared += 1;
        totals.compared += 1;
        compare_sample(flight, ac.addr, s, &p, truth, events, row, totals);
    }
    row.eram_only_s = span_events(
        flight,
        Class::EramOnlySpan,
        &eram_marks,
        th.segment_gap_ms,
        events,
    );

    // OpenATCA rows outside ERAM's track (each ERAM segment widened by the timing threshold).
    let eram_times: Vec<i64> = eram.iter().map(|s| s.t_ms).collect();
    let eram_segs = segments(&eram_times, th.segment_gap_ms);
    let oa_marks: Vec<(i64, bool)> = ac
        .samples
        .iter()
        .map(|s| {
            let i = eram_segs.partition_point(|g| g.0 - th.timing_ms <= s.t_ms);
            let covered = i
                .checked_sub(1)
                .and_then(|j| eram_segs.get(j))
                .is_some_and(|g| s.t_ms <= g.1 + th.timing_ms);
            (s.t_ms, covered)
        })
        .collect();
    row.oa_only_s = span_events(
        flight,
        Class::OaOnlySpan,
        &oa_marks,
        th.segment_gap_ms,
        events,
    );

    // Start and drop timing: each ERAM segment against the OpenATCA segment it overlaps most.
    let oa_times: Vec<i64> = ac.samples.iter().map(|s| s.t_ms).collect();
    let oa_segs = segments(&oa_times, th.segment_gap_ms);
    for &(es, ee) in &eram_segs {
        let mut best: Option<(i64, (i64, i64))> = None;
        for &(os, oe) in &oa_segs {
            let overlap = ee.min(oe) - es.max(os);
            if overlap >= 0 && best.is_none_or(|(b, _)| overlap > b) {
                best = Some((overlap, (os, oe)));
            }
        }
        let Some((_, (os, oe))) = best else { continue };
        for (class, e_t, o_t) in [(Class::StartTiming, es, os), (Class::DropTiming, ee, oe)] {
            let delta = o_t - e_t;
            if th.timing_differs(delta) {
                let mut e = event(flight, class, e_t);
                e.end_ms = o_t;
                e.value = seconds(delta.abs());
                e.detail = if delta > 0 { "oa_later" } else { "oa_earlier" }.to_string();
                events.push(e);
            }
        }
    }

    // Identity, once per ERAM flight against what the adapter was offered near its span.
    for f in flights {
        let (Some(a), Some(b)) = (f.samples.first(), f.samples.last()) else {
            continue;
        };
        let offered = ac.idents_in(a.t_ms - th.segment_gap_ms, b.t_ms + th.segment_gap_ms);
        let squawks: Vec<u16> = {
            let mut v: Vec<u16> = offered.iter().filter_map(|p| p.squawk).collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        if let Some(b0) = f.beacon {
            if !squawks.is_empty() && !squawks.contains(&b0) {
                let mut e = event(flight, Class::IdBeacon, a.t_ms);
                e.detail = format!(
                    "eram={b0:04} oa={}",
                    squawks
                        .iter()
                        .map(|s| format!("{s:04}"))
                        .collect::<Vec<_>>()
                        .join(",")
                );
                events.push(e);
            }
        }
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

#[allow(clippy::too_many_arguments)]
fn compare_sample(
    flight: u32,
    addr: u32,
    s: &EramSample,
    p: &OaPoint,
    truth: Option<TruthAt<'_>>,
    events: &mut Vec<Event>,
    row: &mut FlightRow,
    totals: &mut Totals,
) {
    let th = &THRESHOLDS;
    let sep = separation(s, p, th);
    if sep.cross_m.abs() > row.worst_cross_m {
        row.worst_cross_m = sep.cross_m.abs();
    }
    if sep.along_adj_m.abs() > row.worst_along_adj_m {
        row.worst_along_adj_m = sep.along_adj_m.abs();
    }
    let levels = [
        (sep.cross_m.abs(), [Class::PosCross05, Class::PosCross10]),
        (
            sep.along_adj_m.abs(),
            [Class::PosAlong05, Class::PosAlong10],
        ),
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
        let c = *closer.get_or_insert_with(|| match truth {
            Some(q) => {
                let at = |t: i64| q(addr, t);
                let q3 = [
                    at(s.t_ms - th.eram_time_quant_ms),
                    at(s.t_ms),
                    at(s.t_ms + th.eram_time_quant_ms),
                ];
                adjudicate(s, p, q3, th)
            }
            None => Closer::NoTruth,
        });
        let mut e = event(flight, class, s.t_ms);
        e.value = m;
        e.cross_m = sep.cross_m;
        e.along_raw_m = sep.along_raw_m;
        e.along_adj_m = sep.along_adj_m;
        e.horiz_adj_m = sep.horiz_adj_m;
        e.closer = c;
        events.push(e);
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
            if diff.abs() > row.worst_alt_ft {
                row.worst_alt_ft = diff.abs();
            }
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
    totals.coast_compared += 1;
    if s.coast != p.coast {
        let class = if s.coast {
            Class::CoastEramOnly
        } else {
            Class::CoastOaOnly
        };
        events.push(event(flight, class, s.t_ms));
    }
}
