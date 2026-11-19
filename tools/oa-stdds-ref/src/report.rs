//! The summary text and the per-sample CSV. Both are pure functions of the analysis and the
//! header lines: no clock, no path of the run's own, so the same inputs give the same bytes.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use oa_bench::stats::{stats_of, Stats};

use crate::engine::{Analysis, PairErr, Row, Sample, SET_CODES, SET_NAMES};
use crate::model::{Class, Unpaired, CLASSES};

/// The two finished outputs.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    /// The summary text; its last line is the digest line.
    pub summary: String,
    pub csv: String,
}

type Metric = (&'static str, fn(&PairErr) -> Option<f64>);

/// The metrics of every set, with their units.
const METRICS: [Metric; 5] = [
    ("horizontal (m)", |e| Some(e.h_m)),
    ("along track (m)", |e| e.along_m),
    ("cross track (m)", |e| e.cross_m),
    ("altitude (ft)", |e| e.dalt_ft),
    ("velocity (m/s)", |e| e.dvel_mps),
];

/// Milliseconds as seconds with one decimal, by integer arithmetic.
fn secs(ms: i64) -> String {
    let sign = if ms < 0 { "-" } else { "" };
    let a = ms.unsigned_abs();
    format!("{sign}{}.{}", a / 1000, (a % 1000) / 100)
}

fn stat_line(s: &Stats) -> String {
    if s.n == 0 {
        return "no data".to_string();
    }
    format!(
        "n {:>7}  rms {:>9.2}  mean {:>9.2}  p50 {:>9.2}  p95 {:>9.2}  p99 {:>9.2}",
        s.n, s.rms, s.mean, s.median, s.p95, s.p99
    )
}

/// Nearest-rank percentile `p` of sorted integers.
fn rank(sorted: &[i64], p: usize) -> Option<i64> {
    let n = sorted.len();
    let r = (p * n).div_ceil(100).clamp(1, n.max(1));
    sorted.get(r.checked_sub(1)?).copied()
}

fn counts(
    out: &mut String,
    label: &str,
    rows: &[&Row],
    pick: fn(&Row) -> &Result<Sample, Unpaired>,
) {
    let mut paired = 0u64;
    let mut extrapolated = 0u64;
    let mut why: BTreeMap<Unpaired, u64> = BTreeMap::new();
    for r in rows {
        match pick(r) {
            Ok(s) => {
                paired += 1;
                extrapolated += u64::from(s.tracker_extrapolated);
            }
            Err(e) => *why.entry(*e).or_default() += 1,
        }
    }
    let _ = writeln!(
        out,
        "{label}: paired {paired} (tracker fix extrapolated within 1 s: {extrapolated}), unpaired {}",
        rows.len() as u64 - paired
    );
    for (e, c) in &why {
        let _ = writeln!(out, "    unpaired, {e}: {c}");
    }
}

fn metrics(
    out: &mut String,
    label: &str,
    rows: &[&Row],
    pick: fn(&Row) -> &Result<Sample, Unpaired>,
) {
    let _ = writeln!(out, "  {label}");
    for (si, set) in SET_NAMES.iter().enumerate() {
        let _ = writeln!(out, "    set: {set}");
        for (name, f) in METRICS {
            let v: Vec<f64> = rows
                .iter()
                .filter_map(|r| pick(r).as_ref().ok())
                .filter_map(|s| s.errs.get(si).and_then(f))
                .collect();
            let _ = writeln!(out, "      {name:<16} {}", stat_line(&stats_of(&v)));
        }
    }
}

fn plain(r: &Row) -> &Result<Sample, Unpaired> {
    &r.plain
}

fn fitted(r: &Row) -> &Result<Sample, Unpaired> {
    &r.fitted
}

fn class_section(out: &mut String, class: Class, a: &Analysis) {
    let rows: Vec<&Row> = a.rows.iter().filter(|r| r.rec.class == class).collect();
    let with_addr = rows.iter().filter(|r| r.rec.addr.is_some()).count();
    let _ = writeln!(out);
    let _ = writeln!(out, "== independence class: {} ==", class.name());
    let _ = writeln!(
        out,
        "records {}, with Mode S address {with_addr}, without address {} (counted, never paired)",
        rows.len(),
        rows.len() - with_addr
    );
    counts(out, "stamp as published", &rows, plain);
    counts(
        out,
        "stamp corrected by the fitted track offset",
        &rows,
        fitted,
    );

    let fits: Vec<_> = a.fits.iter().filter(|f| f.class == class).collect();
    let mut offs: Vec<i64> = fits
        .iter()
        .filter_map(|f| f.result.ok().map(|x| x.offset_ms))
        .collect();
    offs.sort_unstable();
    let edge = fits
        .iter()
        .filter(|f| f.result.is_ok_and(|x| x.at_edge))
        .count();
    let _ = writeln!(
        out,
        "time offsets: tracks {}, fitted {}, not fitted (fewer than 8 usable samples) {}, at the edge of the search range {edge}",
        fits.len(),
        offs.len(),
        fits.len() - offs.len()
    );
    if !offs.is_empty() {
        let at = |p| rank(&offs, p).map_or("NA".to_string(), secs);
        let _ = writeln!(
            out,
            "    offset s (stamp late when positive): min {} p05 {} p50 {} p95 {} max {}",
            offs.first().map_or("NA".to_string(), |&m| secs(m)),
            at(5),
            at(50),
            at(95),
            offs.last().map_or("NA".to_string(), |&m| secs(m)),
        );
    }
    metrics(out, "stamp as published (offset 0)", &rows, plain);
    metrics(
        out,
        "stamp corrected by the fitted track offset",
        &rows,
        fitted,
    );
}

fn cell(v: Option<f64>) -> String {
    v.map_or(String::new(), |x| format!("{x:.3}"))
}

fn status(r: &Result<Sample, Unpaired>) -> String {
    match r {
        Ok(_) => "paired".to_string(),
        Err(e) => e.to_string(),
    }
}

fn csv_of(a: &Analysis) -> String {
    let mut o =
        String::from("file,record,class,addr,track,t_ms,status_plain,offset_ms,status_fitted");
    for mode in ["p", "f"] {
        for set in SET_CODES {
            for (name, _) in METRICS {
                let short = name.split(' ').next().unwrap_or(name);
                let _ = write!(o, ",{mode}_{set}_{short}");
            }
        }
    }
    o.push('\n');
    for r in &a.rows {
        let rec = &r.rec;
        let _ = write!(
            o,
            "{},{},{},{},{},{},{},{},{}",
            rec.file,
            rec.index,
            rec.class.name(),
            rec.addr.map_or(String::new(), |x| format!("{x:06x}")),
            rec.track_num.map_or(String::new(), |x| x.to_string()),
            rec.t_ms.map_or(String::new(), |x| x.to_string()),
            status(&r.plain),
            r.offset_ms.map_or(String::new(), |x| x.to_string()),
            status(&r.fitted),
        );
        for res in [&r.plain, &r.fitted] {
            for si in 0..SET_NAMES.len() {
                let e = res.as_ref().ok().and_then(|s| s.errs.get(si));
                for (_, f) in METRICS {
                    o.push(',');
                    o.push_str(&cell(e.and_then(f)));
                }
            }
        }
        o.push('\n');
    }
    o
}

/// The report of `a`. `header` lines (inputs and their digests) are printed first, verbatim.
pub fn build_report(header: &[String], a: &Analysis) -> Report {
    let mut s = String::new();
    s.push_str("OpenATCA tracker and ADS-B truth against STDDS terminal radar tracks (TAIS)\n");
    for h in header {
        let _ = writeln!(s, "{h}");
    }
    s.push_str("Independence: STARS in fusion mode uses ADS-B as one of its sensors, so every number is per class and there is no all-class line.\n");
    s.push_str("Error sets, all on the same paired samples (first minus second): tracker - stdds, ADS-B truth - stdds, tracker - ADS-B truth.\n");
    s.push_str("Diagnostic: if STDDS follows ADS-B, the set ADS-B truth - stdds is small.\n");
    s.push_str("Along and cross track use the direction of the ADS-B truth velocity (cross positive to the right); a truth slower than 2 m/s has none.\n");
    s.push_str("Statistics: rms and signed mean over the values; p50, p95, p99 nearest-rank over absolute values.\n");
    s.push_str("Pairing: Mode S address only; fixes at the STDDS stamp by interpolation (tracker rows up to 5 s apart; at most 1 s of extrapolation at a track end, counted).\n");
    s.push_str("Offsets: per STDDS track, minimising along-track error against ADS-B truth only, -15 s to +15 s in 0.1 s steps, ties toward 0.\n");
    let mut any = false;
    for c in CLASSES {
        if a.rows.iter().any(|r| r.rec.class == c) {
            any = true;
            class_section(&mut s, c, a);
        }
    }
    if !any {
        s.push_str("\nno STDDS records\n");
    }
    let csv = csv_of(a);
    let mut all = s.clone().into_bytes();
    all.extend_from_slice(b"\n--csv--\n");
    all.extend_from_slice(csv.as_bytes());
    let digest = oa_sha256::to_hex(&oa_sha256::sha256(&all));
    let _ = writeln!(s, "\ndigest (summary above and the csv): {digest}");
    Report { summary: s, csv }
}
