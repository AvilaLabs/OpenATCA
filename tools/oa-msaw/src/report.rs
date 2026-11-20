//! The output files, as text. Pure functions of plain values: the same alerts give the same
//! bytes (the rows are sorted here, so the order they came in does not matter).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use oa_msaw::{
    Alert, CycleStats, FloorKind, Grade, MsawConfig, SkipWhy, AF_COAST, AF_COLD_CORRECTED,
    AF_COLD_HIGH, AF_COLD_UNKNOWN, AF_GROUND_UNKNOWN, AF_IDENTITY_UNCONFIRMED, AF_PARTIAL_COVERAGE,
    AF_QNH_MISSING, AF_RADAR_ONLY, AF_RATE_UNKNOWN, AF_STALE_EXTRAPOLATED, AF_UNCERTAIN,
    AF_WIDE_UNCERTAINTY, CK_APPROACH, CK_DEPARTURE, CK_RUNWAY, END_LOST_ALTITUDE_MISSING,
    END_LOST_ALTITUDE_STALE,
};
use oa_safety::net::{reason_text, NetState};

use crate::cifp::{nearest_airport_nm, CifpCorridor};
use crate::eligibility::{Basis, EligibilityReport, BASIS_COUNT};
use crate::floors::FloorStats;
use crate::qnh::QnhStats;
use crate::run::{HiddenAgg, MadeInhibit, SkipAgg};

/// The flag bits of an alert with their names, in the order the summary lists them.
pub const FLAG_NAMES: [(u16, &str); 13] = [
    (AF_RATE_UNKNOWN, "rate_unknown"),
    (AF_COAST, "coast"),
    (AF_UNCERTAIN, "uncertain"),
    (AF_RADAR_ONLY, "radar_only"),
    (AF_IDENTITY_UNCONFIRMED, "identity_unconfirmed"),
    (AF_PARTIAL_COVERAGE, "partial_coverage"),
    (AF_GROUND_UNKNOWN, "ground_unknown"),
    (AF_QNH_MISSING, "qnh_missing"),
    (AF_STALE_EXTRAPOLATED, "stale_extrapolated"),
    (AF_COLD_CORRECTED, "cold_corrected"),
    (AF_COLD_UNKNOWN, "cold_unknown"),
    (AF_COLD_HIGH, "cold_high"),
    (AF_WIDE_UNCERTAINTY, "wide_uncertainty"),
];

/// The text of how an alert ended.
pub fn end_text(e: u8) -> &'static str {
    match e {
        x if x == END_LOST_ALTITUDE_STALE => "protection lost: altitude stale",
        x if x == END_LOST_ALTITUDE_MISSING => "protection lost: altitude missing",
        _ => "cleared",
    }
}

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
        FloorKind::Procedure => "approach_surface",
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
        x if x == SkipWhy::QnhMissing as u8 => "QNH_MISSING",
        x if x == SkipWhy::BelowMva as u8 => "BELOW_MVA",
        x if x == SkipWhy::AltitudeStale as u8 => "ALTITUDE_STALE",
        x if x == SkipWhy::AltitudeExtrapolated as u8 => "ALTITUDE_STALE_EXTRAPOLATED",
        x if x == SkipWhy::ProtectionLost as u8 => {
            "MSAW protection lost: altitude (alert held open)"
        }
        x if x == SkipWhy::GroundExcluded as u8 => "GROUND_EXCLUDED",
        _ => "?",
    }
}

/// The gap, seconds, within which two alerts of one track are one episode.
pub const EPISODE_GAP_S: i64 = 20;

/// Alert episodes: the alerts of one track whose gaps (from the end of one to the start of the next)
/// are at most `gap_s` seconds count as one. Returns the episodes and the tracks with an alert.
/// A report only: the alerts and the hysteresis are as they were.
pub fn episodes(alerts: &[Alert], gap_s: i64) -> (usize, usize) {
    let mut by: BTreeMap<u32, Vec<(i64, i64)>> = BTreeMap::new();
    for a in alerts {
        by.entry(a.addr).or_default().push((a.start_ns, a.end_ns));
    }
    let gap_ns = gap_s.saturating_mul(1_000_000_000);
    let mut n = 0;
    for v in by.values_mut() {
        v.sort_unstable();
        let mut end: Option<i64> = None;
        for &(s, e) in v.iter() {
            if end.is_none_or(|p| s.saturating_sub(p) > gap_ns) {
                n += 1;
            }
            end = Some(end.map_or(e, |p| p.max(e)));
        }
    }
    (n, by.len())
}

/// The status the controller reads on a track the net did not monitor: a gap of that aircraft,
/// never a fault of the net. Empty for the other reasons.
pub fn status_text(w: u8) -> &'static str {
    match w {
        x if x == SkipWhy::AltitudeMissing as u8 => "MSAW not monitoring: altitude missing",
        x if x == SkipWhy::AltitudeStale as u8 => "MSAW not monitoring: altitude stale",
        x if x == SkipWhy::AltitudeExtrapolated as u8 => {
            "MSAW monitoring: altitude stale-extrapolated"
        }
        x if x == SkipWhy::GroundExcluded as u8 => "MSAW not monitoring: on the ground",
        _ => "",
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

/// What a track was doing at its alert, from a first look at its rate, speed, height and distance
/// to an airport with an approach (development thresholds, not an operational rule: the data has
/// no flight rules or intent).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Doing {
    OnApproach,
    Departing,
    VfrLow,
    Other,
}

/// The names of [`Doing`], in order.
pub const DOING_NAMES: [(Doing, &str); 4] = [
    (Doing::OnApproach, "on approach"),
    (Doing::Departing, "departing"),
    (Doing::VfrLow, "VFR at low level"),
    (Doing::Other, "other"),
];

/// Within this of an airport with an approach, nautical miles.
pub const NEAR_AIRPORT_NM: f64 = 10.0;
/// Descending or climbing at least this much, feet per minute, near an airport.
const CLIMB_DESCENT_FPM: f32 = 300.0;
/// Low level and slow: below this altitude (feet) and this speed (metres per second).
const LOW_FT: f32 = 5000.0;
const SLOW_MPS: f32 = 100.0;

/// What the track of an alert was doing (a first look, see [`Doing`]).
pub fn classify(a: &Alert, near_nm: Option<f64>) -> Doing {
    let near = near_nm.is_some_and(|d| d <= NEAR_AIRPORT_NM);
    if near && a.vrate_fpm <= -CLIMB_DESCENT_FPM {
        Doing::OnApproach
    } else if near && a.vrate_fpm >= CLIMB_DESCENT_FPM {
        Doing::Departing
    } else if a.alt_ft < LOW_FT && a.speed_mps < SLOW_MPS {
        Doing::VfrLow
    } else {
        Doing::Other
    }
}

fn near_of(a: &Alert, airports: &[(String, f64, f64)]) -> Option<f64> {
    nearest_airport_nm(
        airports,
        f64::from(a.lat_e7) / 1e7,
        f64::from(a.lon_e7) / 1e7,
    )
}

/// `alerts.csv`: one row per alert, sorted by start time then address, with the distance to the
/// nearest airport that has an approach. It holds addresses and is not for publication.
/// The corridor of the CIFP whose landing zone holds a point: the runway strip, or the last 1 NM of
/// a capture box (where the clearance is nil). The one function that says which alerts are
/// landing alerts, for every count of them.
pub fn landing_corridor<'a>(
    corridors: &'a [CifpCorridor],
    lat_e7: i32,
    lon_e7: i32,
    cfg: &MsawConfig,
) -> Option<&'a CifpCorridor> {
    corridors.iter().find(|k| {
        oa_msaw::corridor_hit_ft(&k.corridor, lat_e7, lon_e7, 0.0, cfg)
            .is_some_and(|h| h.clearance_ft == 0)
    })
}

pub fn alerts_csv(alerts: &[Alert], airports: &[(String, f64, f64)]) -> String {
    let rows: Vec<(Alert, Option<&str>)> = alerts.iter().map(|a| (*a, None)).collect();
    csv_of(&rows, airports)
}

/// [`alerts_csv`] with a last column, `basis`: why the track is alerted (or, in `advisory.csv`,
/// why it is not).
pub fn alerts_csv_with_basis(rows: &[(Alert, Basis)], airports: &[(String, f64, f64)]) -> String {
    let rows: Vec<(Alert, Option<&str>)> = rows.iter().map(|(a, b)| (*a, Some(b.text()))).collect();
    csv_of(&rows, airports)
}

fn csv_of(rows: &[(Alert, Option<&str>)], airports: &[(String, f64, f64)]) -> String {
    let labelled = rows.iter().any(|r| r.1.is_some());
    let mut o = String::from(
        "address_hex,start_s,end_s,caution_s,warning_s,peak,lat_deg,lon_deg,altitude_ft,vrate_fpm,speed_mps,nearest_approach_airport_nm,worst_margin_ft,worst_floor_ft,worst_floor_kind,ttv_min_s,cycles,flags,flags_raw,end_reason,worst_lat_deg,worst_lon_deg,worst_flags_raw",
    );
    if labelled {
        o.push_str(",basis");
    }
    o.push('\n');
    let mut v = rows.to_vec();
    v.sort_by_key(|(a, _)| (a.start_ns, a.addr));
    for (a, basis) in v {
        let _ = write!(
            o,
            "{:08X},{},{},{},{},{},{:.5},{:.5},{},{},{},{},{},{},{},{},{},{},{},{},{:.5},{:.5},{}",
            a.addr,
            secs(a.start_ns),
            secs(a.end_ns),
            opt_secs(a.caution_ns),
            opt_secs(a.warning_ns),
            grade_text(a.peak),
            f64::from(a.lat_e7) / 1e7,
            f64::from(a.lon_e7) / 1e7,
            f(a.alt_ft, 0),
            f(a.vrate_fpm, 0),
            f(a.speed_mps, 1),
            near_of(&a, airports).map_or_else(String::new, |d| format!("{d:.1}")),
            f(a.worst_margin_ft, 0),
            a.worst_floor_ft,
            kind_text(a.worst_kind),
            f(a.ttv_min_s, 1),
            a.cycles,
            flags_text(a.flags),
            a.flags,
            end_text(a.end_reason),
            f64::from(a.worst_lat_e7) / 1e7,
            f64::from(a.worst_lon_e7) / 1e7,
            a.worst_flags
        );
        if let Some(b) = basis {
            let _ = write!(o, ",{b}");
        }
        o.push('\n');
    }
    o
}

/// `skipped.csv`: one row per track and reason that the net did not (fully) evaluate, with the
/// cycles it was left out, sorted by first time then address. It holds addresses and is not for
/// publication.
pub fn skipped_csv(skips: &BTreeMap<(u32, u8), SkipAgg>) -> String {
    skip_rows(skips, false)
}

/// `below_mva.csv`: the tracks predicted below the chart altitude and not below their alert
/// floor, in the same columns as `skipped.csv` (they are no alert and no shortfall).
pub fn below_mva_csv(skips: &BTreeMap<(u32, u8), SkipAgg>) -> String {
    skip_rows(skips, true)
}

fn skip_rows(skips: &BTreeMap<(u32, u8), SkipAgg>, below: bool) -> String {
    let mut o = String::from("address_hex,reason,first_s,last_s,cycles,lat_deg,lon_deg,status\n");
    let mut v: Vec<(&(u32, u8), &SkipAgg)> = skips
        .iter()
        .filter(|(k, _)| (k.1 == SkipWhy::BelowMva as u8) == below)
        .collect();
    v.sort_by_key(|(k, a)| (a.first_ns, k.0, k.1));
    for ((addr, why), a) in v {
        let _ = writeln!(
            o,
            "{:08X},{},{},{},{},{:.5},{:.5},{}",
            addr,
            why_text(*why),
            secs(a.first_ns),
            secs(a.last_ns),
            a.cycles,
            f64::from(a.lat_e7) / 1e7,
            f64::from(a.lon_e7) / 1e7,
            status_text(*why)
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
    /// The corridors the CIFP gave, and the runway ends with an approach that got none.
    pub cifp_corridors: Vec<CifpCorridor>,
    pub without_corridor: Vec<String>,
    /// Per alert (address, start), the distance in metres to the cell that set its floor.
    pub setter_m: BTreeMap<(u32, i64), f32>,
    /// The terrain tiles a query needed that the directory did not hold, by file name.
    pub tiles_absent: Vec<String>,
    /// Rows whose altitude had no barometric measurement behind it: counted, taken as no altitude.
    pub alt_age_unknown: u64,
    pub cov_rows: u64,
    pub cov_uncovered_rows: u64,
    pub cov_uncovered_cells: usize,
    pub obstacle_cells: Option<usize>,
    /// The extent of the obstacle file and how many states it names.
    pub obstacle_extent: Option<((i32, i32, i32, i32), usize)>,
    /// The airports of the CIFP that have an approach: identifier, latitude, longitude.
    pub airports: Vec<(String, f64, f64)>,
    /// The altimeter files read (`None`: no `--metar`).
    pub metar: Option<QnhStats>,
    /// The one-off loads, nanoseconds, by name; the peak resident memory (kB) after them and at the
    /// end; the CPU seconds of the steady state; and its wall times by part.
    pub load_ns: Vec<(&'static str, u64)>,
    pub rss_load_kb: Option<u64>,
    pub rss_end_kb: Option<u64>,
    pub steady_cpu_s: Option<f64>,
    /// CPU seconds of the diagnostics after the last cycle (not in the steady state).
    pub diag_cpu_s: f64,
    pub times: crate::run::RunTimes,
    /// Who was alerted and why; `None` when the run did not judge (a test of the report alone).
    pub eligibility: Option<EligibilityReport>,
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
    /// The altimeter lookups of the run (`None`: altitudes were used as given).
    pub qnh_stats: Option<QnhStats>,
    pub data: &'a DataUsed,
    /// Listed (address, reason) rows of `skipped.csv`.
    pub skipped_listed: usize,
    /// Tracks listed in `below_mva.csv`.
    pub below_mva_tracks: usize,
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
        "general clearance above terrain {} ft (the chart altitude is no alert floor), above obstacles {} ft; track budget {}, track capacity {}",
        c.terrain_clearance_ft, c.obstacle_clearance_ft, c.track_budget, c.max_tracks
    );
    let _ = writeln!(
        o,
        "approach corridors: surface {} ft below the fix altitude, clearance above terrain and obstacles {} ft; in the runway strip and the last 1 NM of a capture box no clearance and the surface lowered by the allowance, and a path that reaches the runway elevation over the runway is a landing; radius grows {} m/s of prediction time",
        c.approach_allowance_ft, c.approach_min_clearance_ft, c.velocity_uncertainty_mps
    );
    let _ = writeln!(o, "data:");
    let _ = writeln!(o, "  terrain tiles given: {}", yes(i.data.terrain));
    let fs = &i.floor_stats;
    let _ = writeln!(
        o,
        "  terrain tiles loaded {}, absent {}, read again {}; terrain queries {} of which without a complete answer {}",
        fs.tiles_loaded, fs.tiles_absent, fs.tiles_reloaded, fs.terrain_queries, fs.terrain_absent
    );
    let _ = writeln!(
        o,
        "  terrain grids resident {:.1} MB, chart raster blocks {}",
        fs.terrain_grid_bytes as f64 / 1_048_576.0,
        fs.chart_blocks
    );
    if !i.data.tiles_absent.is_empty() {
        let _ = writeln!(
            o,
            "  terrain tiles needed and not in the directory ({}): {}",
            i.data.tiles_absent.len(),
            i.data.tiles_absent.join(", ")
        );
    }
    let answered = fs.terrain_queries - fs.terrain_absent;
    let _ = writeln!(
        o,
        "  terrain answer rate {:.4} % ({} of {} queries)",
        if fs.terrain_queries == 0 {
            0.0
        } else {
            100.0 * answered as f64 / fs.terrain_queries as f64
        },
        answered,
        fs.terrain_queries
    );
    let _ = writeln!(
        o,
        "  unanswered: area outside the tiles {}, nodata {}, tiles not on one lattice {}, other {}; blocks of tiles not on one lattice {}",
        fs.terrain_missing,
        fs.terrain_nodata,
        fs.terrain_misaligned,
        fs.terrain_refused,
        fs.blocks_misaligned
    );
    match (&i.qnh_stats, &i.data.metar) {
        (Some(q), Some(m)) => {
            let _ = writeln!(
                o,
                "  altimeter: {} METAR files, {} rows, {} stations; altitudes corrected {}, at or above FL180 {}, without a setting {} (of which the lowest setting of the last 12 h within 200 NM was used {}, the allowance for the rest); lookups {} ({} from the cache)",
                m.files, m.rows, m.stations, q.corrected, q.standard, q.missing, q.fallback, q.lookups, q.cache_hits
            );
        }
        (Some(q), None) => {
            let _ = writeln!(
                o,
                "  altimeter: no METAR given; altitudes below FL180 without a setting {} (lowered by the allowance), at or above FL180 {}",
                q.missing, q.standard
            );
        }
        _ => {
            let _ = writeln!(o, "  altimeter: altitudes used as given");
        }
    }
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
            if let Some((_, regions)) = i.data.obstacle_extent {
                let _ = writeln!(
                    o,
                    "  obstacle data covers {} cells of 0.5 degree ({} states or countries): a cell with an obstacle of a loaded file, or an empty cell with obstacle cells on both sides within 4 cells along its row and along its column (an interior gap); edges and borders are not filled; a point outside is obstacle data absent (counted as without coverage)",
                    i.data.obstacle_cells.unwrap_or(0),
                    regions
                );
                let share = if i.data.cov_rows > 0 {
                    100.0 * i.data.cov_uncovered_rows as f64 / i.data.cov_rows as f64
                } else {
                    0.0
                };
                let _ = writeln!(
                    o,
                    "  track rows outside the obstacle coverage: {} of {} ({share:.3} %), in {} cells of 0.5 degree",
                    i.data.cov_uncovered_rows, i.data.cov_rows, i.data.cov_uncovered_cells
                );
            }
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
    let from_cifp = i
        .data
        .cifp_corridors
        .iter()
        .filter(|c| c.kind == CK_APPROACH && c.faf_from_cifp)
        .count();
    let of_kind = |k: u8| i.data.cifp_corridors.iter().filter(|c| c.kind == k).count();
    let _ = writeln!(
        o,
        "corridors {} ({} from the CIFP: {} approach, {} along runways, {} departure; {} approach corridors with the fix placed by the CIFP); runway ends with an approach and no corridor {}; airports with an approach {}",
        i.cfg.n_corridors,
        i.data.cifp_corridors.len(),
        of_kind(CK_APPROACH),
        of_kind(CK_RUNWAY),
        of_kind(CK_DEPARTURE),
        from_cifp,
        i.data.without_corridor.len(),
        i.data.airports.len()
    );
    let apps: Vec<&CifpCorridor> = i
        .data
        .cifp_corridors
        .iter()
        .filter(|c| c.kind == CK_APPROACH)
        .collect();
    let _ = writeln!(
        o,
        "approach surface from the procedure's own path (threshold crossing height over the threshold elevation, rising at the lowest published angle, less the allowance): angle from the CIFP {} of {} runway ends, crossing height from the CIFP {}, neither (3 degrees, 50 ft) {}",
        apps.iter().filter(|c| c.angle_from_cifp).count(),
        apps.len(),
        apps.iter().filter(|c| c.tch_from_cifp).count(),
        apps.iter().filter(|c| !c.angle_from_cifp && !c.tch_from_cifp).count()
    );
    for c in apps {
        let _ = writeln!(
            o,
            "  {}: {:.1} NM{}, threshold {} ft, fix {} ft, from {} approaches",
            c.label,
            c.faf_nm,
            if c.faf_from_cifp { "" } else { " (default)" },
            c.corridor.thr_elev_ft,
            c.corridor.faf_alt_ft,
            c.approaches
        );
    }
    for w in &i.data.without_corridor {
        let _ = writeln!(o, "  no corridor, {w}");
    }
    let _ = writeln!(o, "compute:");
    for (n, ns) in &i.data.load_ns {
        let _ = writeln!(o, "  one-off load, {n}: {:.2} s", *ns as f64 / 1e9);
    }
    let _ = writeln!(
        o,
        "  peak resident memory after the loads {} MB, at the end {} MB",
        i.data.rss_load_kb.map_or(0, |k| k / 1024),
        i.data.rss_end_kb.map_or(0, |k| k / 1024)
    );
    let sim_s = i.totals.cycles as f64;
    let w = &i.data.times;
    let fl = &i.floor_stats;
    let sec = |ns: u64| ns as f64 / 1e9;
    let _ =
        writeln!(
        o,
        "  steady state, {} simulated seconds: wall {:.2} s, CPU {} s, CPU per simulated second {}",
        i.totals.cycles,
        sec(w.steady_ns),
        i.data.steady_cpu_s.map_or("?".to_string(), |c| format!("{c:.2}")),
        i.data
            .steady_cpu_s
            .map_or("?".to_string(), |c| format!("{:.4}", c / sim_s.max(1.0)))
    );
    if let Some(c) = i.data.steady_cpu_s {
        let _ = writeln!(
            o,
            "  steady state without the terrain tile reads: CPU {:.2} s, per simulated second {:.4}",
            c - sec(fl.ns_tile_read),
            (c - sec(fl.ns_tile_read)) / sim_s.max(1.0)
        );
    }
    let _ = writeln!(
        o,
        "  diagnostics after the last cycle (distance to the cell that set each floor): CPU {:.2} s",
        i.data.diag_cpu_s
    );
    let _ = writeln!(
        o,
        "    wall seconds in: planning {:.2}, floor lookups {:.2} (terrain {:.2}, tile reads {:.2} of which reducing to grids {:.2}, obstacles {:.2}, chart {:.2}), evaluation {:.2}, altimeter correction {:.2}, ranking over the budget {:.2}, tracker and the rest {:.2}",
        sec(w.plan_ns),
        sec(w.floors_ns),
        sec(fl.ns_terrain),
        sec(fl.ns_tile_read),
        sec(fl.ns_tile_reduce),
        sec(fl.ns_obstacle),
        sec(fl.ns_chart),
        sec(w.eval_ns),
        sec(w.qnh_ns),
        sec(w.rank_ns),
        sec(w.steady_ns.saturating_sub(w.plan_ns + w.floors_ns + w.eval_ns + w.qnh_ns))
    );
    let _ = writeln!(
        o,
        "measurements {}, tracker rows (ticks) {}, tracker rows lost {}, columns missing {}",
        i.measurements, i.tick_rows, i.tracker_overflow, i.rows_skipped
    );
    let _ = writeln!(o, "cycles {} (out of order {})", t.cycles, t.out_of_order);
    let _ = writeln!(o, "track rows {}", t.rows);
    let _ = writeln!(o, "tracks evaluated {}", t.evaluated);
    let _ = writeln!(
        o,
        "excluded on the ground (listed per track) {}",
        t.ground_excluded
    );
    let _ = writeln!(o, "ground state unknown (evaluated) {}", t.ground_unknown);
    let _ = writeln!(o, "no altitude (listed, rule 4) {}", t.alt_missing);
    let _ = writeln!(
        o,
        "not monitored: altitude stale (older than the hold of {} s; listed per track, a gap of the aircraft; the net degrades only when more than {} of the airborne tracks of a cycle are lost this way, {} track cycles) {}",
        i.cfg.stale_hold_s, i.cfg.altitude_loss_degrade_share, t.alt_loss, t.alt_stale
    );
    let _ = writeln!(
        o,
        "monitored at an extrapolated altitude (stale from {} s to the hold; a descent carried on, never a climb; listed per track) {}",
        i.cfg.max_altitude_age_s, t.alt_extrapolated
    );
    let _ = writeln!(
        o,
        "altitude of unknown age taken as fresh by the core {}; rows with only a geometric altitude taken as no altitude by the shell {}",
        t.alt_age_unknown, i.data.alt_age_unknown
    );
    let _ = writeln!(
        o,
        "protection lost: altitude missing or past the hold, whatever the share (track cycles) {}; of them with an open alert, held open (track cycles) {}",
        t.protection_lost, t.protection_lost_open
    );
    let _ = writeln!(
        o,
        "track cycles with a point that qualified as a landing (the whole 95 % circle over the pavement plus 60 m, a known rate not above zero and no rise over 10 s, a fresh altitude with a real setting and no cold correction, at or below the runway elevation at the point; the points after it are the roll-out and are not judged) {}",
        t.landings
    );
    let _ = writeln!(
        o,
        "tracks with no altimeter setting (listed, rule 1) {}",
        t.qnh_missing
    );
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
    let _ = writeln!(
        o,
        "95 % radius unusable or above 10 km, held to 10 km {}; above 2 km (flagged wide_uncertainty, track cycles; the net reads degraded when more than {} of the airborne tracks of a cycle are, {} track cycles) {}",
        t.r95_held, i.cfg.wide_degrade_share, t.wide_degrade, t.wide
    );
    let _ = writeln!(
        o,
        "roll-out path points not judged (after a landing point) {}, of them outside every runway strip (past the far end or off it) {}",
        t.rollout_points, t.rollout_off_runway
    );
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
    let below_tracks = i.below_mva_tracks;
    let _ = writeln!(
        o,
        "below the chart altitude and above the alert floor (below_mva.csv, no alert, no shortfall): {} tracks, {} track cycles; per hour {:.1} tracks",
        below_tracks,
        t.below_mva,
        below_tracks as f64 / (t.cycles as f64 / 3600.0).max(1.0e-9)
    );
    let warnings = i.alerts.iter().filter(|a| a.peak == Grade::Warning).count();
    let _ = writeln!(
        o,
        "alerts {}: reached warning {}, caution only {}",
        i.alerts.len(),
        warnings,
        i.alerts.len() - warnings
    );
    // One cycle a second: the cycles are the seconds of the run.
    let hours = t.cycles as f64 / 3600.0;
    if hours > 0.0 {
        let _ = writeln!(
            o,
            "alerts per hour over {:.4} h: reached warning {:.1}, caution only {:.1}, all {:.1}",
            hours,
            warnings as f64 / hours,
            (i.alerts.len() - warnings) as f64 / hours,
            i.alerts.len() as f64 / hours
        );
    }
    let (eps, with_alert) = episodes(i.alerts, EPISODE_GAP_S);
    let _ = writeln!(
        o,
        "alert episodes (alerts of one track {EPISODE_GAP_S} s or less apart count as one): {eps}, tracks with an alert {with_alert}, per hour {:.1} episodes",
        eps as f64 / hours.max(1.0e-9)
    );
    if let Some(e) = &i.data.eligibility {
        eligibility_lines(&mut o, e, hours, i.data);
    }
    let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
    for a in i.alerts {
        *kinds.entry(kind_text(a.worst_kind)).or_default() += 1;
    }
    for (k, n) in kinds {
        let _ = writeln!(o, "  worst floor set by {k}: {n}");
    }
    let nears: Vec<Option<f64>> = i
        .alerts
        .iter()
        .map(|a| near_of(a, &i.data.airports))
        .collect();
    let near_n = nears
        .iter()
        .filter(|d| d.is_some_and(|d| d <= NEAR_AIRPORT_NM))
        .count();
    let _ = writeln!(
        o,
        "alerts within {NEAR_AIRPORT_NM} NM of an airport with an approach: {near_n} of {}",
        i.alerts.len()
    );
    // A first classification: every alert, and 20 of them spread evenly through the sorted list.
    let mut order: Vec<usize> = (0..i.alerts.len()).collect();
    order.sort_by_key(|&k| i.alerts.get(k).map_or((0, 0), |a| (a.start_ns, a.addr)));
    let count = |picked: &[usize]| {
        let mut n = [0usize; 4];
        for &k in picked {
            if let (Some(a), Some(d)) = (i.alerts.get(k), nears.get(k)) {
                let slot = DOING_NAMES.iter().position(|(x, _)| *x == classify(a, *d));
                if let Some(c) = slot.and_then(|s| n.get_mut(s)) {
                    *c += 1;
                }
            }
        }
        n
    };
    let sample: Vec<usize> = if order.len() <= 20 {
        order.clone()
    } else {
        (0..20)
            .filter_map(|k| order.get(k * order.len() / 20).copied())
            .collect()
    };
    for (what, picked) in [("20 spread through the list", &sample), ("all", &order)] {
        let n = count(picked);
        let _ = writeln!(
            o,
            "what the track was doing, {what} ({} alerts): {}",
            picked.len(),
            DOING_NAMES
                .iter()
                .zip(n.iter())
                .map(|((_, name), c)| format!("{name} {c}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    // Alerts and episodes by what the track was doing.
    let mut phases = Vec::new();
    for (d, name) in DOING_NAMES {
        let of: Vec<Alert> = i
            .alerts
            .iter()
            .zip(nears.iter())
            .filter(|(a, n)| classify(a, **n) == d)
            .map(|(a, _)| *a)
            .collect();
        let (eps, tracks) = episodes(&of, EPISODE_GAP_S);
        phases.push(format!(
            "{name}: {} alerts, {eps} episodes, {tracks} tracks",
            of.len()
        ));
    }
    let _ = writeln!(o, "by phase: {}", phases.join("; "));
    let landing: Vec<Alert> = i
        .alerts
        .iter()
        .filter(|a| {
            landing_corridor(
                &i.data.cifp_corridors,
                a.worst_lat_e7,
                a.worst_lon_e7,
                i.cfg,
            )
            .is_some()
        })
        .copied()
        .collect();
    let (leps, ltracks) = episodes(&landing, EPISODE_GAP_S);
    let _ = writeln!(
        o,
        "landing alerts (the worst point in the runway strip or the last 1 NM of a capture box): {} alerts, {leps} episodes, {ltracks} tracks",
        landing.len()
    );
    let ext = i
        .alerts
        .iter()
        .filter(|a| a.flags & AF_STALE_EXTRAPOLATED != 0)
        .count();
    let lost = i
        .alerts
        .iter()
        .filter(|a| a.end_reason != oa_msaw::END_CLEAR)
        .count();
    let _ = writeln!(
        o,
        "alerts judged at a stale-extrapolated altitude at some cycle {ext}; alerts ended \"protection lost: altitude\" {lost}"
    );
    nuisance_lines(&mut o, i, &order, &nears);
    diagnostic_lines(&mut o, i);
    let _ = writeln!(o, "net state by cycle:");
    for (s, n) in i.net_cycles {
        let _ = writeln!(o, "  {s}: {n}");
    }
    let _ = writeln!(o, "net state at the end: {}", i.final_net);
    let _ = writeln!(
        o,
        "track-cycles by reason (a per-track status in skipped.csv; the altitude ones do not degrade the net):"
    );
    for (name, n) in [
        ("MSAW not monitoring: altitude missing", t.alt_missing),
        ("MSAW not monitoring: altitude stale", t.alt_stale),
        ("no altimeter setting", t.qnh_missing),
        ("point without coverage", t.no_coverage),
        ("not evaluated for want of budget", t.over_budget),
    ] {
        let _ = writeln!(o, "  {name}: {n}");
    }
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

/// The summary's account of who was alerted: counts by basis of the tracks and of the violations,
/// the advisories, and what the advisories were doing.
fn eligibility_lines(o: &mut String, e: &EligibilityReport, hours: f64, data: &DataUsed) {
    if !e.judged {
        let _ = writeln!(
            o,
            "eligibility: not judged (no --sfdps recordings), every track is alerted; msaw_request lines count only as a basis"
        );
        return;
    }
    let by = |a: &[u64; BASIS_COUNT]| {
        [
            Basis::Address,
            Basis::Callsign,
            Basis::Request,
            Basis::VfrPlan,
            Basis::NotActive,
            Basis::Ambiguous,
            Basis::NoFlight,
            Basis::VfrBeacon,
        ]
        .iter()
        .map(|b| format!("{} {}", b.text(), a.get(*b as usize).copied().unwrap_or(0)))
        .collect::<Vec<_>>()
        .join(", ")
    };
    let _ = writeln!(
        o,
        "eligibility: every track is alerted except one SFDPS has a VFR flight for and one with no flight, an ambiguous or an inactive one whose beacon code is a VFR code; those are advisories ({} SFDPS operations applied, {} flights held; the lag of SFDPS never suppresses an alert)",
        e.sfdps_ops, e.sfdps_flights
    );
    let _ = writeln!(
        o,
        "tracks ({}) by basis at first sight: {}",
        e.tracks,
        by(&e.tracks_by)
    );
    let _ = writeln!(
        o,
        "violating tracks by basis (alerts and advisories): {}",
        by(&e.alerts_by)
    );
    let w = e
        .advisories
        .iter()
        .filter(|(a, _)| a.peak == Grade::Warning)
        .count();
    let n = e.advisories.len();
    let _ = writeln!(
        o,
        "advisories (advisory.csv, not alerted): {n}: reached warning {w}, caution only {}",
        n - w
    );
    if hours > 0.0 {
        let _ = writeln!(
            o,
            "advisories per hour over {hours:.4} h: reached warning {:.1}, caution only {:.1}, all {:.1}",
            w as f64 / hours,
            (n - w) as f64 / hours,
            n as f64 / hours
        );
    }
    let mut c = [0usize; 4];
    for (a, _) in &e.advisories {
        let d = classify(a, near_of(a, &data.airports));
        if let Some(slot) = DOING_NAMES.iter().position(|(x, _)| *x == d) {
            if let Some(x) = c.get_mut(slot) {
                *x += 1;
            }
        }
    }
    let _ = writeln!(
        o,
        "what the advised tracks were doing ({n}): {}",
        DOING_NAMES
            .iter()
            .zip(c.iter())
            .map(|((_, name), x)| format!("{name} {x}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
}

/// Counts per bucket: `edges` are the upper bounds (exclusive) of all buckets but the last.
fn buckets(values: &[f64], edges: &[f64], unit: &str) -> String {
    let mut n = vec![0usize; edges.len() + 1];
    for v in values {
        let k = edges.iter().position(|e| v < e).unwrap_or(edges.len());
        if let Some(c) = n.get_mut(k) {
            *c += 1;
        }
    }
    let mut parts = Vec::new();
    for (k, c) in n.iter().enumerate() {
        let lo = if k == 0 { None } else { edges.get(k - 1) };
        let hi = edges.get(k);
        let name = match (lo, hi) {
            (None, Some(h)) => format!("under {h}{unit}"),
            (Some(l), Some(h)) => format!("{l} to {h}{unit}"),
            (Some(l), None) => format!("{l}{unit} or more"),
            (None, None) => "all".to_string(),
        };
        parts.push(format!("{name} {c}"));
    }
    parts.join(", ")
}

/// Smallest difference between two courses, degrees (0 to 180).
fn course_gap(a: f64, b: f64) -> f64 {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d)
}

/// The distributions of the alerts that remain: depth below the floor at the predicted violation,
/// time to violation, query radius at the violating point, distance from that point to the cell
/// that set the floor, and whether the track is aligned within 30 degrees of a runway heading of
/// the nearest airport with corridors. Counts per bucket; no aircraft is named.
fn diagnostic_lines(o: &mut String, i: &SummaryInput) {
    let a = i.alerts;
    let depth: Vec<f64> = a
        .iter()
        .map(|x| f64::from((-x.worst_margin_ft).max(0.0)))
        .collect();
    let ttv: Vec<f64> = a.iter().map(|x| f64::from(x.ttv_min_s)).collect();
    let radius: Vec<f64> = a.iter().map(|x| f64::from(x.worst_radius_m)).collect();
    let setter: Vec<f64> = a
        .iter()
        .filter_map(|x| {
            i.data
                .setter_m
                .get(&(x.addr, x.start_ns))
                .map(|d| f64::from(*d))
        })
        .collect();
    let _ = writeln!(o, "diagnostics of the {} alerts:", a.len());
    let _ = writeln!(
        o,
        "  depth below the floor at the predicted violation: {}",
        buckets(&depth, &[25.0, 50.0, 100.0, 200.0, 500.0], " ft")
    );
    let _ = writeln!(
        o,
        "  time to violation: {}",
        buckets(&ttv, &[1.0, 10.0, 20.0, 30.0, 45.0], " s")
    );
    let _ = writeln!(
        o,
        "  query radius at the violating point: {}",
        buckets(&radius, &[100.0, 200.0, 300.0, 500.0, 1000.0], " m")
    );
    let _ = writeln!(
        o,
        "  distance from the violating point to the cell that set the floor ({} alerts with a terrain or obstacle floor): {}",
        setter.len(),
        buckets(&setter, &[50.0, 100.0, 200.0, 500.0, 1000.0], " m")
    );
    let (mut yes, mut no, mut none) = (0usize, 0usize, 0usize);
    for x in a {
        let (here_lat, here_lon) = (
            f64::from(x.worst_lat_e7) / 1e7,
            f64::from(x.worst_lon_e7) / 1e7,
        );
        // The nearest corridor threshold names the airport; its runways give the headings.
        let near = i
            .data
            .cifp_corridors
            .iter()
            .map(|c| {
                let (la, lo) = (
                    f64::from(c.corridor.thr_lat_e7) / 1e7,
                    f64::from(c.corridor.thr_lon_e7) / 1e7,
                );
                let (dy, dx) = (la - here_lat, (lo - here_lon) * here_lat.to_radians().cos());
                (dy * dy + dx * dx, c.label.split(' ').next().unwrap_or(""))
            })
            .min_by(|p, q| oa_math::cmp_canon_f64(p.0, q.0));
        let Some((_, apt)) = near else {
            none += 1;
            continue;
        };
        let course = f64::from(x.vel_east_mps)
            .atan2(f64::from(x.vel_north_mps))
            .to_degrees();
        let aligned = i
            .data
            .cifp_corridors
            .iter()
            .filter(|c| c.label.split(' ').next() == Some(apt))
            .any(|c| {
                let h = c.corridor.out_bearing_deg;
                course_gap(course, h) <= 30.0 || course_gap(course, h + 180.0) <= 30.0
            });
        if aligned {
            yes += 1;
        } else {
            no += 1;
        }
    }
    let _ = writeln!(
        o,
        "  aligned within 30 degrees of a runway heading at the nearest airport: yes {yes}, no {no}, no corridors {none}"
    );
}

/// How many alerts the nuisance list holds.
const NUISANCE_LIST: usize = 20;

/// The alerts most likely to be nuisance (a heuristic, not a rule): an alert within 10 NM of an
/// airport with an approach on a track climbing or descending at 300 ft/min or more, whose floor
/// is not an approach surface, ranked by the shallowest predicted violation first. Each with what
/// the track was doing, its altitude over the floor now, the floor kind and the basis.
fn nuisance_lines(o: &mut String, i: &SummaryInput, order: &[usize], nears: &[Option<f64>]) {
    let mut cand: Vec<(f32, usize)> = Vec::new();
    for &k in order {
        let (Some(a), Some(d)) = (i.alerts.get(k), nears.get(k)) else {
            continue;
        };
        let phase = classify(a, *d);
        if matches!(phase, Doing::OnApproach | Doing::Departing)
            && a.worst_kind != FloorKind::Procedure
        {
            cand.push((-a.worst_margin_ft, k));
        }
    }
    cand.sort_by(|x, y| oa_math::cmp_canon_f64(f64::from(x.0), f64::from(y.0)).then(x.1.cmp(&y.1)));
    let _ = writeln!(
        o,
        "nuisance candidates (heuristic: within {NEAR_AIRPORT_NM} NM of an airport with an approach, climbing or descending, floor not an approach surface; shallowest predicted violation first): {} of {} alerts, first {}",
        cand.len(),
        i.alerts.len(),
        NUISANCE_LIST.min(cand.len())
    );
    for (n, &(_, k)) in cand.iter().take(NUISANCE_LIST).enumerate() {
        let (Some(a), Some(d)) = (i.alerts.get(k), nears.get(k)) else {
            continue;
        };
        let basis = i
            .data
            .eligibility
            .as_ref()
            .and_then(|e| e.basis.get(&(a.addr, a.start_ns)))
            .map_or("not_judged", |b| b.text());
        let name = DOING_NAMES
            .iter()
            .find(|(x, _)| *x == classify(a, *d))
            .map_or("", |(_, n)| *n);
        let _ = writeln!(
            o,
            "  {:2}: {name}, {:.0} ft over the floor now, predicted {:.0} ft at worst, floor {} ({} ft), associated {basis}",
            n + 1,
            a.alt_ft - a.worst_floor_ft as f32,
            a.worst_margin_ft,
            kind_text(a.worst_kind),
            a.worst_floor_ft
        );
    }
}
