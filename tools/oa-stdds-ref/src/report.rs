//! The summary text and the per-sample CSV. Both are pure functions of the analysis and the
//! header lines: no clock, no path of the run's own, so the same inputs give the same bytes.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use oa_bench::stats::{stats_of, Stats};

use crate::engine::{
    Analysis, FitMiss, PairErr, Row, Sample, SET_CODES, SET_NAMES, TRACK_REUSE_GAP_MS,
};
use crate::model::{Class, Unpaired, CLASSES, MIN_SPEED_MPS};
use crate::selection::{SelectionCounts, TimeInterval};

/// The two finished outputs.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    /// The summary text; its last line is the digest line.
    pub summary: String,
    pub csv: String,
    /// Aggregate, machine-readable cohort and exclusion counts.
    pub json: String,
    /// Create-new final marker binding the exact three report files and provenance header.
    pub completion_marker: String,
    /// SHA-256 of exact provenance header lines, used to validate the marker before publication.
    pub provenance_digest: String,
}

/// Conservative output envelope: CSV row values are bounded by parsed fields and metric formats.
pub const MAX_REPORT_BYTES: usize = 256 * 1024 * 1024;
const MAX_CSV_ROW_BYTES: usize = 2_048;

/// Refuses before analysis/report loops when the conservative report size would exceed its cap.
pub fn check_report_bound(records: usize, provenance_bytes: usize) -> Result<(), String> {
    let estimated = records
        .checked_mul(MAX_CSV_ROW_BYTES)
        .and_then(|csv| csv.checked_add(provenance_bytes))
        .and_then(|n| n.checked_add(2 * 1024 * 1024))
        .ok_or_else(|| "report size estimate overflow".to_string())?;
    if estimated > MAX_REPORT_BYTES {
        return Err(format!(
            "report estimate {estimated} bytes exceeds the {}-byte report limit",
            MAX_REPORT_BYTES
        ));
    }
    Ok(())
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

#[derive(Clone, Copy)]
enum Cohort {
    Raw,
    Fitted,
    Both,
}

fn stat_number(value: f64) -> String {
    if value.is_finite() {
        format!("{value:.6}")
    } else {
        "null".to_string()
    }
}

fn stats_json(rows: &[&Row], cohort: Cohort, mode: Cohort, set: usize, metric: Metric) -> String {
    let mut values = Vec::new();
    for row in rows {
        let raw = row.plain.as_ref().ok();
        let fitted = row.fitted.as_ref().ok();
        let selected = match cohort {
            Cohort::Raw => raw,
            Cohort::Fitted => fitted,
            Cohort::Both if raw.is_some() && fitted.is_some() => match mode {
                Cohort::Raw => raw,
                Cohort::Fitted => fitted,
                Cohort::Both => None,
            },
            Cohort::Both => None,
        };
        if let Some(value) = selected
            .and_then(|sample| sample.errs.get(set))
            .and_then(|pair| (metric.1)(pair))
        {
            values.push(value);
        }
    }
    let stats = stats_of(&values);
    format!(
        "{{\"n\":{},\"rms\":{},\"signed_mean\":{},\"p50_abs\":{},\"p95_abs\":{},\"p99_abs\":{}}}",
        stats.n,
        stat_number(stats.rms),
        stat_number(stats.mean),
        stat_number(stats.median),
        stat_number(stats.p95),
        stat_number(stats.p99)
    )
}

fn metric_json(rows: &[&Row], cohort: Cohort, mode: Cohort) -> String {
    let units: [&str; METRICS.len()] = ["m", "m", "m", "ft", "m/s"];
    let mut out = String::from("{");
    for (i, ((name, metric), unit)) in METRICS.iter().zip(&units).enumerate() {
        if i != 0 {
            out.push(',');
        }
        let _ = write!(
            out,
            "{}:{{\"unit\":{},\"sets\":{{",
            json_string(name),
            json_string(unit)
        );
        for (set_index, code) in SET_CODES.iter().enumerate() {
            if set_index != 0 {
                out.push(',');
            }
            let _ = write!(
                out,
                "{}:{}",
                json_string(code),
                stats_json(rows, cohort, mode, set_index, (*name, *metric))
            );
        }
        out.push_str("}}");
    }
    out.push('}');
    out
}

fn plain(r: &Row) -> &Result<Sample, Unpaired> {
    &r.plain
}

fn fitted(r: &Row) -> &Result<Sample, Unpaired> {
    &r.fitted
}

/// Altitude medians further apart than this factor (values floored at [`ALT_FLOOR_FT`]) are suspect.
const ALT_FACTOR: f64 = 10.0;
/// Altitudes below this are taken as this, so aircraft on the ground do not trip the check, feet.
const ALT_FLOOR_FT: f64 = 100.0;
/// The median STDDS to truth speed ratio is suspect outside this range.
const SPEED_RATIO: (f64, f64) = (0.8, 1.25);

/// Whether two altitude medians differ by more than [`ALT_FACTOR`].
pub fn altitude_suspect(std_ft: f64, truth_ft: f64) -> bool {
    let (a, b) = (
        std_ft.abs().max(ALT_FLOOR_FT),
        truth_ft.abs().max(ALT_FLOOR_FT),
    );
    a.max(b) / a.min(b) > ALT_FACTOR
}

/// Whether a median speed ratio is outside [`SPEED_RATIO`].
pub fn speed_suspect(ratio: f64) -> bool {
    !(SPEED_RATIO.0..=SPEED_RATIO.1).contains(&ratio)
}

/// The unit sanity lines of one class, on the paired samples at the published stamp.
fn unit_check(out: &mut String, rows: &[&Row]) {
    let samples: Vec<&Sample> = rows.iter().filter_map(|r| r.plain.as_ref().ok()).collect();
    let pairs: Vec<(f64, f64)> = samples
        .iter()
        .filter_map(|s| Some((s.std_alt_ft?, s.tru_alt_ft?)))
        .collect();
    let ratios: Vec<f64> = samples
        .iter()
        .filter_map(|s| {
            let (a, b) = (s.std_speed_mps?, s.tru_speed_mps?);
            (b >= MIN_SPEED_MPS).then_some(a / b)
        })
        .collect();
    let (sa, ta): (Vec<f64>, Vec<f64>) = pairs.into_iter().unzip();
    let (sm, tm) = (stats_of(&sa), stats_of(&ta));
    let rm = stats_of(&ratios);
    if sm.n == 0 {
        let _ = writeln!(out, "unit check, altitude: no data");
    } else {
        let flag = if altitude_suspect(sm.median, tm.median) {
            "  units suspect"
        } else {
            ""
        };
        let _ = writeln!(
            out,
            "unit check, altitude: median STDDS {:.0} ft, median ADS-B truth {:.0} ft (n {}){flag}",
            sm.median, tm.median, sm.n
        );
    }
    if rm.n == 0 {
        let _ = writeln!(out, "unit check, speed: no data");
    } else {
        let flag = if speed_suspect(rm.median) {
            "  units suspect"
        } else {
            ""
        };
        let _ = writeln!(
            out,
            "unit check, speed: median ratio STDDS to ADS-B truth {:.3} (n {}){flag}",
            rm.median, rm.n
        );
    }
}

fn class_section(out: &mut String, class: Class, a: &Analysis) {
    let rows: Vec<&Row> = a.rows.iter().filter(|r| r.rec.class == class).collect();
    let with_addr = rows
        .iter()
        .filter(|r| r.rec.addr.is_some_and(|a| a != 0))
        .count();
    let zero_addr = rows.iter().filter(|r| r.rec.addr == Some(0)).count();
    let _ = writeln!(out);
    let _ = writeln!(out, "== independence class: {} ==", class.name());
    let _ = writeln!(
        out,
        "records {}, with positive Mode S address {with_addr}, without address {}, zero address {zero_addr} (both counted, never paired)",
        rows.len(),
        rows.len() - with_addr - zero_addr
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
    let miss = |m: FitMiss| fits.iter().filter(|f| f.result == Err(m)).count();
    let _ = writeln!(
        out,
        "time offsets: tracks {}, class/key segments {}, fitted {}, not fitted {}, at the edge of the search range {edge}",
        fits.len(),
        fits.len(),
        offs.len(),
        fits.len() - offs.len()
    );
    let _ = writeln!(
        out,
        "    not fitted, no ADS-B truth over the +-15 s window: {}",
        miss(FitMiss::NoTruthCoverage)
    );
    let _ = writeln!(
        out,
        "    not fitted, fewer than 8 usable samples: {}",
        miss(FitMiss::TooFewSamples)
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
    unit_check(out, &rows);
    metrics(out, "stamp as published (offset 0)", &rows, plain);
    metrics(
        out,
        "stamp corrected by the fitted track offset",
        &rows,
        fitted,
    );
    let both: Vec<&Row> = rows
        .iter()
        .copied()
        .filter(|r| r.plain.is_ok() && r.fitted.is_ok())
        .collect();
    let _ = writeln!(
        out,
        "same-record raw/fitted intersection: {} records",
        both.len()
    );
    metrics(out, "raw stamp, same-record intersection", &both, plain);
    metrics(out, "fitted stamp, same-record intersection", &both, fitted);
}

fn exclusion_object(rows: &[&Row], fitted: bool) -> String {
    let mut reasons: BTreeMap<String, usize> = BTreeMap::new();
    for row in rows {
        let result = if fitted { &row.fitted } else { &row.plain };
        if let Err(reason) = result {
            *reasons.entry(reason.to_string()).or_default() += 1;
        }
    }
    let mut out = String::from("{");
    for (i, (reason, count)) in reasons.into_iter().enumerate() {
        if i != 0 {
            out.push(',');
        }
        let _ = write!(out, "{}:{}", json_string(&reason), count);
    }
    out.push('}');
    out
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

/// One row per STDDS record: its place in the input (file and record number), class, status, fitted
/// offset and errors. No address, track number, callsign, time or position: the real recordings are
/// reported as counts and aggregate statistics only.
fn csv_of(a: &Analysis) -> String {
    let mut o = String::from("file,record,class,status_plain,raw_common_cohort,offset_ms,status_fitted,fitted_common_cohort,segment");
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
            rec.class.code(),
            status(&r.plain),
            r.plain.is_ok(),
            r.offset_ms.map_or(String::new(), |x| x.to_string()),
            status(&r.fitted),
            r.fitted.is_ok(),
            r.segment.map_or(String::new(), |x| x.to_string()),
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

fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn selection_json(
    cohort_id: &str,
    interval: Option<TimeInterval>,
    counts: SelectionCounts,
) -> String {
    match interval {
        Some(range) => format!(
            "{{\"cohort_id\":{},\"policy\":\"mrtTime in half-open [from_ms,to_ms), before offset fitting\",\"from_ms\":{},\"to_ms\":{},\"records_after_sealed_filter\":{},\"selected_records\":{},\"excluded_outside_interval\":{},\"excluded_untimed\":{}}}",
            json_string(cohort_id), range.from_ms(), range.to_ms(), counts.records_after_sealed_filter, counts.selected_records,
            counts.excluded_outside_interval, counts.excluded_untimed
        ),
        None => format!(
            "{{\"cohort_id\":{},\"policy\":\"all timed mrtTime rows, before offset fitting\",\"from_ms\":null,\"to_ms\":null,\"records_after_sealed_filter\":{},\"selected_records\":{},\"excluded_outside_interval\":{},\"excluded_untimed\":{}}}",
            json_string(cohort_id), counts.records_after_sealed_filter, counts.selected_records,
            counts.excluded_outside_interval, counts.excluded_untimed
        ),
    }
}

fn json_of(
    a: &Analysis,
    header: &[String],
    selection: Option<(&str, Option<TimeInterval>, SelectionCounts)>,
) -> String {
    let selection =
        selection.map(|(cohort_id, interval, counts)| selection_json(cohort_id, interval, counts));
    let mut out = String::from("{\n  \"schema\": \"oa-stdds-ref-v1\",\n  \"time_basis\": \"mrtTime Unix milliseconds UTC\",\n  \"units\": {\"position_error\": \"m\", \"altitude_error\": \"ft (feed unit assumed)\", \"velocity_error\": \"m/s (feed unit assumed)\", \"time\": \"ms\"},\n  \"calibration\": \"in-sample: offset fit uses the same records later scored in fitted mode; fitted metrics are calibrated agreement, not prospective accuracy\",\n  \"cohort\": \"all three error sets share each mode's paired records; zero/missing address and duplicate key timestamps are excluded\",\n");
    if let Some(selection) = selection {
        let _ = writeln!(out, "  \"selection\": {selection},");
    }
    out.push_str("  \"policy\": {\"track_reuse_gap_ms\": ");
    let _ = write!(out, "{TRACK_REUSE_GAP_MS}, \"ambiguous_timestamp_rows\": ");
    let _ = write!(out, "{}, \"nonmonotonic_arrivals\": {}, \"class_churn_transitions\": {}, \"reuse_segments\": {} }},\n  \"provenance\": [", a.ambiguous_timestamp_rows, a.nonmonotonic_arrivals, a.class_churn_transitions, a.reuse_segments);
    for (i, h) in header.iter().enumerate() {
        if i != 0 {
            out.push(',');
        }
        out.push_str(&json_string(h));
    }
    out.push_str("],\n  \"classes\": [\n");
    for (i, class) in CLASSES.iter().enumerate() {
        let rows: Vec<&Row> = a.rows.iter().filter(|r| r.rec.class == *class).collect();
        let raw = rows.iter().filter(|r| r.plain.is_ok()).count();
        let fitted = rows.iter().filter(|r| r.fitted.is_ok()).count();
        let both = rows
            .iter()
            .filter(|r| r.plain.is_ok() && r.fitted.is_ok())
            .count();
        let _ = write!(out, "    {{\"class\": {}, \"records\": {}, \"raw_common_cohort\": {}, \"raw_excluded\": {}, \"raw_exclusion_reasons\": {}, \"fitted_common_cohort\": {}, \"fitted_excluded\": {}, \"fitted_exclusion_reasons\": {}, \"raw_fitted_intersection_records\": {}, \"metrics\": {{\"raw_mode_cohort\": ",
            json_string(class.code()), rows.len(), raw, rows.len() - raw, exclusion_object(&rows, false), fitted, rows.len() - fitted, exclusion_object(&rows, true), both);
        out.push_str(&metric_json(&rows, Cohort::Raw, Cohort::Raw));
        out.push_str(",\"fitted_mode_cohort\":");
        out.push_str(&metric_json(&rows, Cohort::Fitted, Cohort::Fitted));
        out.push_str(",\"same_record_intersection_raw\":");
        out.push_str(&metric_json(&rows, Cohort::Both, Cohort::Raw));
        out.push_str(",\"same_record_intersection_fitted\":");
        out.push_str(&metric_json(&rows, Cohort::Both, Cohort::Fitted));
        let _ = writeln!(out, "}}}}{}", if i + 1 == CLASSES.len() { "" } else { "," });
    }
    out.push_str("  ]\n}\n");
    out
}

/// The report of `a`. `header` lines (inputs and their digests) are printed first, verbatim.
pub fn build_report(header: &[String], a: &Analysis) -> Report {
    build_report_inner(header, a, None)
}

/// Builds a report with explicit pre-fit published-time cohort accounting.
pub fn build_report_with_selection(
    header: &[String],
    a: &Analysis,
    cohort_id: &str,
    interval: Option<TimeInterval>,
    counts: SelectionCounts,
) -> Report {
    build_report_inner(header, a, Some((cohort_id, interval, counts)))
}

fn build_report_inner(
    header: &[String],
    a: &Analysis,
    selection: Option<(&str, Option<TimeInterval>, SelectionCounts)>,
) -> Report {
    let mut s = String::new();
    s.push_str("OpenATCA tracker and ADS-B truth against STDDS terminal radar tracks (TAIS)\n");
    for h in header {
        let _ = writeln!(s, "{h}");
    }
    s.push_str("Units assumed, not verified against the feed (oa-stdds tais.rs): STDDS altitude in feet, STDDS velocity in m/s east and north; each class prints a unit check.\n");
    s.push_str("Altitude compared: the tracker row's altitude is the barometric one of the ADS-B report where it has one and the geometric one otherwise (the row does not say which); the ADS-B truth's is barometric.\n");
    s.push_str("Independence: STARS in fusion mode uses ADS-B as one of its sensors, so every number is per class (class names state the flag value, not a meaning) and there is no all-class line.\n");
    s.push_str("Error sets, all on the same paired samples (first minus second): tracker - stdds, ADS-B truth - stdds, tracker - ADS-B truth.\n");
    s.push_str("Diagnostic: if STDDS follows ADS-B, the set ADS-B truth - stdds is small.\n");
    s.push_str("Along and cross track use the direction of the ADS-B truth velocity (cross positive to the right); a truth slower than 2 m/s has none.\n");
    s.push_str("Statistics: rms and signed mean over the values; p50, p95, p99 nearest-rank over absolute values.\n");
    s.push_str("Pairing: Mode S address only; fixes at the STDDS stamp by interpolation (tracker rows up to 5 s apart; at most 1 s of extrapolation at a track end, counted).\n");
    s.push_str("Offsets: per STDDS track, minimising along-track error against ADS-B truth only, -15 s to +15 s in 0.1 s steps, ties toward 0.\n");
    s.push_str("Calibration: fitted-time metrics are in-sample agreement because each track offset is estimated from the same records later scored. They are not prospective accuracy estimates. A key silence over 60 s starts a new fit segment by policy; this is a reuse heuristic, not proof of flight identity.\n");
    let _ = writeln!(s, "History diagnostics: ambiguous duplicate timestamp rows excluded {}; nonmonotonic file/record arrival timestamps {}; adjacent class-flag changes {}; 60 s reuse splits {}.", a.ambiguous_timestamp_rows, a.nonmonotonic_arrivals, a.class_churn_transitions, a.reuse_segments);
    s.push_str("The CSV marks raw_common_cohort and fitted_common_cohort per record; each cohort contributes all three error sets together. Sample timestamps, addresses and positions are omitted from outputs. Time basis is mrtTime Unix milliseconds UTC.\n");
    s.push_str("Raw-mode and fitted-mode cohorts can differ. The report also gives raw and fitted metrics on their same-record intersection; metric n can still be smaller when a field is absent. JSON includes numeric per-metric denominators and statistics for all three sets.\n");
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
    let json = json_of(a, header, selection);
    let mut all = s.clone().into_bytes();
    all.extend_from_slice(b"\n--csv--\n");
    all.extend_from_slice(csv.as_bytes());
    all.extend_from_slice(b"\n--json--\n");
    all.extend_from_slice(json.as_bytes());
    let digest = oa_sha256::to_hex(&oa_sha256::sha256(&all));
    let _ = writeln!(s, "\ndigest (summary above, csv and json): {digest}");
    let provenance = header.join("\n");
    let provenance_digest = oa_sha256::to_hex(&oa_sha256::sha256(provenance.as_bytes()));
    let marker = format!(
        "OASRF001\nsummary {}\ncsv {}\njson {}\nprovenance {}\n",
        oa_sha256::to_hex(&oa_sha256::sha256(s.as_bytes())),
        oa_sha256::to_hex(&oa_sha256::sha256(csv.as_bytes())),
        oa_sha256::to_hex(&oa_sha256::sha256(json.as_bytes())),
        provenance_digest,
    );
    Report {
        summary: s,
        csv,
        json,
        completion_marker: marker,
        provenance_digest,
    }
}
