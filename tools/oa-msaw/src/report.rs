//! The output files, as text. Pure functions of plain values: the same alerts give the same
//! bytes (the rows are sorted here, so the order they came in does not matter).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use oa_msaw::{
    Alert, CycleStats, FloorKind, Grade, MsawConfig, SkipWhy, AF_COAST, AF_GROUND_UNKNOWN,
    AF_IDENTITY_UNCONFIRMED, AF_PARTIAL_COVERAGE, AF_RADAR_ONLY, AF_RATE_UNKNOWN, AF_UNCERTAIN,
};
use oa_safety::net::{reason_text, NetState};

use crate::floors::FloorStats;
use crate::run::{HiddenAgg, MadeInhibit, SkipAgg};

/// The flag bits of an alert with their names, in the order the summary lists them.
pub const FLAG_NAMES: [(u16, &str); 7] = [
    (AF_RATE_UNKNOWN, "rate_unknown"),
    (AF_COAST, "coast"),
    (AF_UNCERTAIN, "uncertain"),
    (AF_RADAR_ONLY, "radar_only"),
    (AF_IDENTITY_UNCONFIRMED, "identity_unconfirmed"),
    (AF_PARTIAL_COVERAGE, "partial_coverage"),
    (AF_GROUND_UNKNOWN, "ground_unknown"),
];

/// The names of the bits of `f` joined by `|`; empty if none.
pub fn flags_text(f: u16) -> String {
    let v: Vec<&str> = FLAG_NAMES
        .iter()
        .filter(|(b, _)| f & b != 0)
        .map(|&(_, n)| n)
        .collect();
    v.join("|")
}

/// The text of a floor kind.
pub fn kind_text(k: FloorKind) -> &'static str {
    match k {
        FloorKind::Terrain => "terrain",
        FloorKind::Obstacle => "obstacle",
        FloorKind::Mva => "mva",
        FloorKind::Procedure => "procedure",
    }
}

/// The text of a grade.
pub fn grade_text(g: Grade) -> &'static str {
    match g {
        Grade::Caution => "CAUTION",
        Grade::Warning => "WARNING",
    }
}

/// The text of a skip reason.
pub fn why_text(w: u8) -> &'static str {
    match w {
        x if x == SkipWhy::AltitudeMissing as u8 => "ALTITUDE_MISSING",
        x if x == SkipWhy::NoCoverage as u8 => "NO_COVERAGE",
        x if x == SkipWhy::OverBudget as u8 => "OVER_BUDGET",
        x if x == SkipWhy::BadRow as u8 => "BAD_ROW",
        x if x == SkipWhy::OverCapacity as u8 => "OVER_CAPACITY",
        _ => "?",
    }
}

fn f(x: f32, prec: usize) -> String {
    if x.is_finite() {
        format!("{x:.prec$}")
    } else {
        String::new()
    }
}

fn secs(ns: i64) -> String {
    format!("{:.3}", ns as f64 / 1e9)
}

fn opt_secs(ns: Option<i64>) -> String {
    ns.map_or_else(String::new, secs)
}

/// `alerts.csv`: one row per alert, sorted by start time then address. It holds addresses and is
/// not for publication.
pub fn alerts_csv(alerts: &[Alert]) -> String {
    let mut o = String::from(
        "address_hex,start_s,end_s,caution_s,warning_s,peak,lat_deg,lon_deg,altitude_ft,worst_margin_ft,worst_floor_ft,worst_floor_kind,ttv_min_s,cycles,flags,flags_raw\n",
    );
    let mut v = alerts.to_vec();
    v.sort_by_key(|a| (a.start_ns, a.addr));
    for a in v {
        let _ = writeln!(
            o,
            "{:08X},{},{},{},{},{},{:.5},{:.5},{},{},{},{},{},{},{},{}",
            a.addr,
            secs(a.start_ns),
            secs(a.end_ns),
            opt_secs(a.caution_ns),
            opt_secs(a.warning_ns),
            grade_text(a.peak),
            f64::from(a.lat_e7) / 1e7,
            f64::from(a.lon_e7) / 1e7,
            f(a.alt_ft, 0),
            f(a.worst_margin_ft, 0),
            a.worst_floor_ft,
            kind_text(a.worst_kind),
            f(a.ttv_min_s, 1),
            a.cycles,
            flags_text(a.flags),
            a.flags
        );
    }
    o
}

/// `skipped.csv`: one row per track and reason that the net did not (fully) evaluate, with the
/// cycles it was left out, sorted by first time then address. It holds addresses and is not for
/// publication.
pub fn skipped_csv(skips: &BTreeMap<(u32, u8), SkipAgg>) -> String {
    let mut o = String::from("address_hex,reason,first_s,last_s,cycles,lat_deg,lon_deg\n");
    let mut v: Vec<(&(u32, u8), &SkipAgg)> = skips.iter().collect();
    v.sort_by_key(|(k, a)| (a.first_ns, k.0, k.1));
    for ((addr, why), a) in v {
        let _ = writeln!(
            o,
            "{:08X},{},{},{},{},{:.5},{:.5}",
            addr,
            why_text(*why),
            secs(a.first_ns),
            secs(a.last_ns),
            a.cycles,
            f64::from(a.lat_e7) / 1e7,
            f64::from(a.lon_e7) / 1e7
        );
    }
    o
}

/// `inhibitions.csv`: every inhibition of the run with its area, band, start, end, reason and
/// author, the alerts it hid (count and times) and when it expired for review. The commas of the
/// texts are replaced by spaces.
pub fn inhibitions_csv(
    made: &[MadeInhibit],
    hidden: &BTreeMap<u32, HiddenAgg>,
    expired_ns: &BTreeMap<u32, i64>,
) -> String {
    let mut o = String::from(
        "id,net,start_s,end_s,alt_lo_ft,alt_hi_ft,reason,author,vertices,hidden_cycles,first_hidden_s,last_hidden_s,expired_for_review_s\n",
    );
    for m in made {
        let h = hidden.get(&m.id);
        let _ = writeln!(
            o,
            "{},MSAW,{},{},{},{},{},{},{},{},{},{},{}",
            m.id,
            secs(m.start_ns),
            secs(m.end_ns),
            m.spec.alt_lo_ft,
            m.spec.alt_hi_ft,
            m.spec.reason.replace(',', " "),
            m.spec.author.replace(',', " "),
            m.spec.vertices.len(),
            h.map_or(0, |x| x.count),
            opt_secs(h.map(|x| x.first_ns)),
            opt_secs(h.map(|x| x.last_ns)),
            opt_secs(expired_ns.get(&m.id).copied())
        );
    }
    o
}

/// The text of a net state as the summary counts it, without counts (those are in the shortfall
/// totals).
pub fn state_text(s: &NetState) -> String {
    match s {
        NetState::Running => "RUNNING".to_string(),
        NetState::Degraded { reason, .. } => format!("DEGRADED: {}", reason_text(*reason)),
        NetState::Inhibited => "INHIBITED".to_string(),
        NetState::Unavailable { reason } => format!("UNAVAILABLE: {}", reason_text(*reason)),
    }
}

/// What data the run had. A7.2: cold struct, size guard waived.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DataUsed {
    /// A terrain directory was given.
    pub terrain: bool,
    /// Charts were given.
    pub chart: bool,
    /// Obstacles indexed, if a DOF was given.
    pub obstacles: Option<usize>,
    /// Chart files loaded, refused, and the sectors of the model.
    pub chart_files: Option<(usize, usize, usize)>,
    /// The CIFP areas: label, floor in feet and approaches that gave it.
    pub cifp_areas: Vec<(String, i32, usize)>,
}

/// What the summary is made from. A7.2: cold struct, size guard waived.
pub struct SummaryInput<'a> {
    pub cfg: &'a MsawConfig,
    pub alerts: &'a [Alert],
    pub totals: CycleStats,
    /// Cycles by the text of the state the net showed after each.
    pub net_cycles: &'a BTreeMap<String, u64>,
    pub alert_overflow: u64,
    pub skip_overflow: u64,
    pub tracker_overflow: u64,
    pub measurements: u64,
    pub tick_rows: u64,
    /// Rows of the tracker's output left out for a missing column.
    pub rows_skipped: u64,
    /// The net state after the last report of the run, the end of it included.
    pub final_net: &'a str,
    pub made: &'a [MadeInhibit],
    pub hidden: &'a BTreeMap<u32, HiddenAgg>,
    pub expired_ns: &'a BTreeMap<u32, i64>,
    pub suppressed_overflow: u64,
    pub floor_stats: FloorStats,
    pub data: &'a DataUsed,
    /// Listed (address, reason) rows of `skipped.csv`.
    pub skipped_listed: usize,
    pub clip_name: &'a str,
    pub clip_sha256: &'a str,
    /// Clip rows left out because their time is in a sealed window (OA-SEAL-014).
    pub sealed_window_dropped: u64,
    pub config_name: &'a str,
    pub config_sha256: &'a str,
    pub scope: &'a str,
    pub alerts_sha256: &'a str,
    /// The first measurement of the clip, Unix ns: the zero of the configured inhibition times.
    pub t0_ns: i64,
}

fn yes(b: bool) -> &'static str {
    if b {
        "yes"
    } else {
        "NO (every point of the path reads as without coverage)"
    }
}

/// `summary.txt`: counts only, no address or callsign.
pub fn summary_text(i: &SummaryInput) -> String {
    let mut o = String::new();
    let t = &i.totals;
    let _ = writeln!(o, "oa-msaw: minimum safe altitude warning");
    let _ = writeln!(
        o,
        "DEVELOPMENT VALUES, NOT FAA VALUES: every clearance, time and budget below is from the configuration file"
    );
    let _ = writeln!(o, "clip {} (sha256 {})", i.clip_name, i.clip_sha256);
    let _ = writeln!(o, "config {} (sha256 {})", i.config_name, i.config_sha256);
    let _ = writeln!(o, "scope {}", i.scope);
    let _ = writeln!(o, "first measurement {} s (Unix)", secs(i.t0_ns));
    let _ = writeln!(o, "sealed-window rows dropped: {}", i.sealed_window_dropped);
    let c = i.cfg;
    let _ = writeln!(
        o,
        "look-ahead {} s, warning {} s, {} points per path, hysteresis {} s",
        c.look_ahead_s,
        c.warning_s,
        c.samples,
        c.hysteresis_ns as f64 / 1e9
    );
    let _ = writeln!(
        o,
        "clearance above terrain {} ft, above obstacles {} ft; track budget {}, track capacity {}",
        c.terrain_clearance_ft, c.obstacle_clearance_ft, c.track_budget, c.max_tracks
    );
    let _ = writeln!(o, "data:");
    let _ = writeln!(o, "  terrain tiles given: {}", yes(i.data.terrain));
    let _ = writeln!(
        o,
        "  terrain tiles loaded {}, absent {}; terrain queries {} of which without a complete answer {}",
        i.floor_stats.tiles_loaded,
        i.floor_stats.tiles_absent,
        i.floor_stats.terrain_queries,
        i.floor_stats.terrain_absent
    );
    let _ = writeln!(o, "  charts given: {}", yes(i.data.chart));
    if let Some((ok, bad, sectors)) = i.data.chart_files {
        let _ = writeln!(
            o,
            "  chart files loaded {ok}, refused {bad}, sectors {sectors}"
        );
    }
    let _ = writeln!(
        o,
        "  points without a chart sector {}",
        i.floor_stats.chart_absent
    );
    match i.data.obstacles {
        Some(n) => {
            let _ = writeln!(o, "  obstacles indexed {n}");
        }
        None => {
            let _ = writeln!(
                o,
                "  obstacles: none given (the floor is terrain and charts only)"
            );
        }
    }
    let _ = writeln!(
        o,
        "  points looked up {} ({} from the cell cache)",
        i.floor_stats.points, i.floor_stats.cache_hits
    );
    let _ = writeln!(
        o,
        "procedure areas {} ({} from the CIFP)",
        i.cfg.n_procs,
        i.data.cifp_areas.len()
    );
    for (label, floor, n) in &i.data.cifp_areas {
        let _ = writeln!(o, "  {label}: floor {floor} ft from {n} approaches");
    }
    let _ = writeln!(
        o,
        "measurements {}, tracker rows (ticks) {}, tracker rows lost {}, columns missing {}",
        i.measurements, i.tick_rows, i.tracker_overflow, i.rows_skipped
    );
    let _ = writeln!(o, "cycles {} (out of order {})", t.cycles, t.out_of_order);
    let _ = writeln!(o, "track rows {}", t.rows);
    let _ = writeln!(o, "tracks evaluated {}", t.evaluated);
    let _ = writeln!(o, "excluded on the ground {}", t.ground_excluded);
    let _ = writeln!(o, "ground state unknown (evaluated) {}", t.ground_unknown);
    let _ = writeln!(o, "no altitude (listed, rule 4) {}", t.alt_missing);
    let _ = writeln!(
        o,
        "tracks with a point without coverage (listed, rule 1) {}",
        t.no_coverage
    );
    let _ = writeln!(
        o,
        "not evaluated for want of budget (listed, rule 5) {}",
        t.over_budget
    );
    let _ = writeln!(o, "rows past the track capacity {}", t.rows_over_capacity);
    let _ = writeln!(o, "bad rows {}", t.rows_bad);
    let _ = writeln!(o, "rows lost upstream {}", t.rows_lost_upstream);
    let _ = writeln!(
        o,
        "vertical rate unknown (predicted level) {}",
        t.rate_unknown
    );
    let _ = writeln!(o, "95 % radius held to the cap {}", t.r95_held);
    let _ = writeln!(
        o,
        "track cycles below the floor within the look-ahead {}",
        t.violations
    );
    let _ = writeln!(o, "  hidden by an inhibition {}", t.suppressed);
    let _ = writeln!(
        o,
        "alerts opened {}, closed {}, logged at once (open table full) {}",
        t.opened, t.closed, t.open_full
    );
    let _ = writeln!(
        o,
        "alerts dropped (log full) {}, skipped rows dropped (log full) {}",
        t.alerts_dropped, t.skips_dropped
    );
    let _ = writeln!(
        o,
        "floors that did not match the points {}",
        t.floor_mismatch
    );
    let _ = writeln!(o, "reports the net state table refused {}", t.net_refused);
    let _ = writeln!(
        o,
        "alert log overflow {}, skip log overflow {}",
        i.alert_overflow, i.skip_overflow
    );
    let _ = writeln!(o, "skipped.csv rows {}", i.skipped_listed);
    let warnings = i.alerts.iter().filter(|a| a.peak == Grade::Warning).count();
    let _ = writeln!(
        o,
        "alerts {}: reached warning {}, caution only {}",
        i.alerts.len(),
        warnings,
        i.alerts.len() - warnings
    );
    let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
    for a in i.alerts {
        *kinds.entry(kind_text(a.worst_kind)).or_default() += 1;
    }
    for (k, n) in kinds {
        let _ = writeln!(o, "  worst floor set by {k}: {n}");
    }
    let _ = writeln!(o, "net state by cycle:");
    for (s, n) in i.net_cycles {
        let _ = writeln!(o, "  {s}: {n}");
    }
    let _ = writeln!(o, "net state at the end: {}", i.final_net);
    let _ = writeln!(o, "inhibitions {}", i.made.len());
    for m in i.made {
        let h = i.hidden.get(&m.id);
        let _ = writeln!(
            o,
            "  id {} from {} s to {} s, band {} to {} ft, hid {} track cycles, expired for review {}",
            m.id,
            secs(m.start_ns),
            secs(m.end_ns),
            m.spec.alt_lo_ft,
            m.spec.alt_hi_ft,
            h.map_or(0, |x| x.count),
            i.expired_ns
                .get(&m.id)
                .map_or_else(|| "no".to_string(), |&e| format!("at {} s", secs(e)))
        );
    }
    let _ = writeln!(o, "suppressions lost (log full) {}", i.suppressed_overflow);
    let _ = writeln!(o, "alerts.csv sha256 {}", i.alerts_sha256);
    o
}
