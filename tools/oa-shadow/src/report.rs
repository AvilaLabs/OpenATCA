//! The difference report: one CSV row per event, one CSV row per flight, a summary text and a
//! digest line. Everything is a function of the comparison and the header lines given, so two
//! runs on the same inputs give the same bytes (no clock, no paths, no run time).

use std::fmt::Write as _;

use oa_sha256::{sha256, to_hex};

use crate::compare::{Class, Comparison, Event, CLASSES};
use crate::geo::Closer;
use crate::thresholds::THRESHOLDS;

/// The report files' contents.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rendered {
    pub events_csv: String,
    pub flights_csv: String,
    /// Ends with the digest line.
    pub summary: String,
    /// The digest, lower-case hex.
    pub digest: String,
}

fn num(v: f64, dec: usize) -> String {
    if !v.is_finite() {
        return String::new();
    }
    let s = format!("{v:.dec$}");
    // A value that rounds to zero is written without a sign.
    if s.starts_with('-') && s.chars().all(|c| matches!(c, '-' | '0' | '.')) {
        s.trim_start_matches('-').to_string()
    } else {
        s
    }
}

/// CSV text: a field with a comma, quote or newline is quoted.
fn field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn addr_text(a: Option<u32>) -> String {
    a.map_or(String::new(), |a| format!("{a:06x}"))
}

/// The events CSV. The aircraft address is a column; the summary never names one.
pub fn events_csv(c: &Comparison) -> String {
    let mut o = String::from(
        "flight,addr,class,t_ms,end_ms,duration_s,value,cross_m,along_m,along_credit_m,horiz_m,within_1s_stamp_allowance,alt_diff_ft,closer,detail\n",
    );
    for e in &c.events {
        let addr = c
            .flights
            .get(e.flight as usize)
            .map_or(String::new(), |f| addr_text(f.addr));
        let duration = if e.class.is_span() {
            num(e.value, 1)
        } else {
            String::new()
        };
        let flag = e
            .within_stamp_allowance
            .map_or(String::new(), |b| b.to_string());
        let _ = writeln!(
            o,
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
            e.flight,
            addr,
            e.class.name(),
            e.t_ms,
            e.end_ms,
            duration,
            num(e.value, 1),
            num(e.cross_m, 1),
            num(e.along_m, 1),
            num(e.credit_m, 1),
            num(e.horiz_m, 1),
            flag,
            num(e.alt_diff_ft, 1),
            e.closer.name(),
            field(&e.detail)
        );
    }
    o
}

/// The per-flight CSV: counts and the worst case of each measure.
pub fn flights_csv(c: &Comparison) -> String {
    let mut o = String::from(
        "flight,addr,kind,rules,eram_samples,compared,worst_cross_m,worst_along_m,worst_alt_ft,events,eram_only_s,oa_only_s\n",
    );
    for f in &c.flights {
        let _ = writeln!(
            o,
            "{},{},{},{},{},{},{},{},{},{},{},{}",
            f.flight,
            addr_text(f.addr),
            f.kind,
            f.rules,
            f.eram_samples,
            f.compared,
            num(f.worst_cross_m, 1),
            num(f.worst_along_m, 1),
            num(f.worst_alt_ft, 1),
            f.events,
            num(f.eram_only_s, 1),
            num(f.oa_only_s, 1)
        );
    }
    o
}

fn median_ms(v: &[i64]) -> Option<i64> {
    let mut s = v.to_vec();
    s.sort_unstable();
    // The lower middle value.
    s.get(s.len().checked_sub(1)? / 2).copied()
}

fn median(v: &[f64]) -> Option<f64> {
    let mut s: Vec<f64> = v.iter().copied().filter(|x| x.is_finite()).collect();
    s.sort_by(|a, b| oa_math::cmp_canon_f64(*a, *b));
    // The lower middle value.
    s.get(s.len().checked_sub(1)? / 2).copied()
}

/// The `n` largest position divergences, one per (flight, time): `(horiz_m, flight, t_ms)`,
/// largest first, ties by flight then time.
pub fn largest_divergences(events: &[Event], n: usize) -> Vec<(f64, u32, i64)> {
    let mut v: Vec<(f64, u32, i64)> = Vec::new();
    for e in events.iter().filter(|e| e.class.is_position()) {
        if let Some(x) = v.iter_mut().find(|x| x.1 == e.flight && x.2 == e.t_ms) {
            x.0 = x.0.max(e.horiz_m);
        } else {
            v.push((e.horiz_m, e.flight, e.t_ms));
        }
    }
    v.sort_by(|a, b| {
        oa_math::cmp_canon_f64(b.0, a.0)
            .then(a.1.cmp(&b.1))
            .then(a.2.cmp(&b.2))
    });
    v.truncate(n);
    v
}

/// Renders the report. `header` lines (input digests, the split, run labels) come first in the
/// summary.
pub fn render(c: &Comparison, header: &[String]) -> Rendered {
    let events_csv = events_csv(c);
    let flights_csv = flights_csv(c);
    let mut s = String::new();
    let _ = writeln!(
        s,
        "OpenATCA shadow comparison against ERAM (SFDPS), offline"
    );
    for h in header {
        let _ = writeln!(s, "  {h}");
    }
    let _ = writeln!(
        s,
        "\nThresholds (a value is a difference only when strictly greater):"
    );
    for l in THRESHOLDS.lines() {
        let _ = writeln!(s, "  {l}");
    }
    let t = &c.totals;
    let _ = writeln!(
        s,
        "\nPairing (Mode S address, else beacon code plus proximity):"
    );
    let _ = writeln!(
        s,
        "  ERAM flights {} ({} samples in the box), OpenATCA aircraft {} ({} rows in the box)",
        t.eram_flights, t.eram_samples, t.oa_aircraft, t.oa_rows
    );
    let _ = writeln!(
        s,
        "  groups {}; ERAM flights paired by address {}, by beacon+proximity {}; unpaired ERAM flights {}, unpaired OpenATCA aircraft {}",
        t.groups,
        t.paired_by_address,
        t.paired_by_beacon,
        c.events.iter().filter(|e| e.class == Class::UnpairedEram).count(),
        c.events.iter().filter(|e| e.class == Class::UnpairedOa).count()
    );
    let _ = writeln!(
        s,
        "\nCompared at ERAM report times: {} reports with an OpenATCA sample; altitude on both {} (not comparable {}); coast status {}",
        t.compared, t.alt_compared, t.alt_not_comparable, t.coast_compared
    );
    let _ = writeln!(
        s,
        "  altitude ERAM minus OpenATCA, median: below 18,000 ft {} ft (n={}), at or above {} ft (n={}); ERAM's lower altitudes are altimeter-set, OpenATCA's are pressure altitude, and no altimeter setting is applied here",
        median(&t.alt_diffs_below_fl180).map_or("n/a".into(), |m| format!("{m:.0}")),
        t.alt_diffs_below_fl180.len(),
        median(&t.alt_diffs_at_or_above_fl180).map_or("n/a".into(), |m| format!("{m:.0}")),
        t.alt_diffs_at_or_above_fl180.len()
    );
    let _ = writeln!(
        s,
        "  median ERAM report interval (consecutive reports within a track segment, {} intervals): {} s",
        t.eram_intervals_ms.len(),
        median_ms(&t.eram_intervals_ms)
            .map_or("n/a".into(), |m| format!("{:.1}", m as f64 / 1000.0))
    );
    let _ = writeln!(
        s,
        "  Coast flags are not the same concept: ERAM's is a message indicator in SFDPS, OpenATCA's is the tracker's own state when no measurement arrived. Compared only at OpenATCA rows at the exact ERAM report time."
    );
    let _ = writeln!(
        s,
        "  coast compared {}; beacon compared {}",
        t.coast_compared, t.beacon_compared
    );
    let _ = writeln!(
        s,
        "\nThe span classes (UNPAIRED_*, ERAM_ONLY_SPAN, OA_ONLY_SPAN, ERAM_REPORT_GAP, DUP_ERAM_TRACK) are disjoint by construction, and start/drop timing is not reported where a span already covers it, so span totals add."
    );
    let _ = writeln!(
        s,
        "\nDifferences by class (events, flights affected, total span s, largest):"
    );
    for class in CLASSES {
        let ev: Vec<&Event> = c.events.iter().filter(|e| e.class == class).collect();
        let mut fs: Vec<u32> = ev.iter().map(|e| e.flight).collect();
        fs.sort_unstable();
        fs.dedup();
        let span: f64 = if class.is_span() {
            ev.iter().map(|e| e.value).sum()
        } else {
            0.0
        };
        let worst = ev
            .iter()
            .map(|e| e.value)
            .filter(|v| v.is_finite())
            .fold(f64::NAN, f64::max);
        let unit = match class {
            Class::Alt => "ft",
            Class::StartTiming | Class::DropTiming => "s",
            Class::CoastEramOnly | Class::CoastOaOnly => "",
            c if c.is_span() => "s",
            _ => "m",
        };
        let _ = writeln!(
            s,
            "  {:<22} events {:>8}  flights {:>6}  span {:>10.1} s  largest {}{}",
            class.name(),
            ev.len(),
            fs.len(),
            span,
            num(worst, 1),
            if worst.is_finite() { unit } else { "" }
        );
    }
    let alt_low = c
        .events
        .iter()
        .filter(|e| e.class == Class::Alt && e.detail == "eram_below_fl180")
        .count();
    let alt_all = c.events.iter().filter(|e| e.class == Class::Alt).count();
    let _ = writeln!(
        s,
        "  ALT_GT_300FT by ERAM altitude: below 18,000 ft {alt_low}, at or above {}",
        alt_all - alt_low
    );
    for class in [Class::PosAlong05, Class::PosAlong10] {
        let n = |flag: bool| {
            c.events
                .iter()
                .filter(|e| e.class == class && e.within_stamp_allowance == Some(flag))
                .count()
        };
        let _ = writeln!(
            s,
            "  {:<22} within_1s_stamp_allowance: true {}, false {}",
            class.name(),
            n(true),
            n(false)
        );
    }
    let pos: Vec<&Event> = c.events.iter().filter(|e| e.class.is_position()).collect();
    let _ = writeln!(
        s,
        "\nAdjudication: distance of each system to the ADS-B truth at the ERAM report time. The truth is built from ADS-B reports (set B) of the same aircraft whose other reports (set A) feed OpenATCA, so their errors are correlated and this column favours OpenATCA; the count of events where ERAM is closer is the more informative one."
    );
    let _ = writeln!(
        s,
        "  position events {}; truth query gave no answer for {}",
        pos.len(),
        pos.iter().filter(|e| e.closer == Closer::NoTruth).count()
    );
    for class in [
        Class::PosCross05,
        Class::PosCross10,
        Class::PosAlong05,
        Class::PosAlong10,
    ] {
        let n = |k: Closer| {
            c.events
                .iter()
                .filter(|e| e.class == class && e.closer == k)
                .count()
        };
        let _ = writeln!(
            s,
            "  {:<22} ERAM closer {:>7}  OpenATCA closer {:>7}  tie {:>6}  no truth {:>7}",
            class.name(),
            n(Closer::Eram),
            n(Closer::OpenAtca),
            n(Closer::Tie),
            n(Closer::NoTruth)
        );
    }
    let _ = writeln!(
        s,
        "\nLargest position divergences (raw) (flight index, time ms, metres):"
    );
    for (m, f, t_ms) in largest_divergences(&c.events, 10) {
        let _ = writeln!(s, "  flight {f:>6}  t {t_ms}  {m:.1} m");
    }
    let _ = writeln!(
        s,
        "\nPer-flight worst cases are in flights.csv; every event is in events.csv."
    );
    let mut d = Vec::new();
    d.extend_from_slice(events_csv.as_bytes());
    d.extend_from_slice(flights_csv.as_bytes());
    d.extend_from_slice(s.as_bytes());
    let digest = to_hex(&sha256(&d));
    let _ = writeln!(
        s,
        "digest sha256:{digest} (events.csv, flights.csv and this summary above)"
    );
    Rendered {
        events_csv,
        flights_csv,
        summary: s,
        digest,
    }
}
