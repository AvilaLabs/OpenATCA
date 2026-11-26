//! The output files, as text. Pure functions of plain values: the same alerts give the same
//! bytes (the alerts are sorted here, so the order they came in does not matter).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use oa_ca::{
    Alert, AltLost, CaConfig, CycleStats, AF_ALT_UNKNOWN, AF_COAST, AF_IDENTITY_UNCONFIRMED,
    AF_ONE_GROUND, AF_RADAR_ONLY, AF_RATE_UNKNOWN, AF_SUPPRESSED, AF_UNCERTAIN, ON_GROUND,
};
use oa_safety::net::{reason_text, NetState};

const NM: f64 = 1852.0;

/// The flag bits of an alert with their names, in the order the summary lists them.
pub const FLAG_NAMES: [(u16, &str); 8] = [
    (AF_ALT_UNKNOWN, "alt_unknown"),
    (AF_COAST, "coast"),
    (AF_UNCERTAIN, "uncertain"),
    (AF_RADAR_ONLY, "radar_only"),
    (AF_IDENTITY_UNCONFIRMED, "identity_unconfirmed"),
    (AF_ONE_GROUND, "one_ground"),
    (AF_RATE_UNKNOWN, "rate_unknown"),
    (AF_SUPPRESSED, "suppressed"),
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

/// Contexts of an alert, in the order the summary leads with them.
pub const CONTEXT_NAMES: [&str; 4] = ["AIRBORNE", "ALT_UNKNOWN", "GROUND", "DEGRADED_TRACK"];
const CTX_AIRBORNE: usize = 0;
const CTX_ALT_UNKNOWN: usize = 1;
const CTX_GROUND: usize = 2;
const CTX_DEGRADED: usize = 3;
const CONTEXT_RULES: [&str; 4] = [
    "both altitudes known at every cycle, neither track coasting or uncertain, neither aircraft on the ground",
    "not ground, neither track coasting or uncertain, an altitude unknown on either track at some cycle",
    "either aircraft on the ground at the peak cycle, or at any cycle (the one_ground flag)",
    "either track coasting or uncertain at some cycle, and not ground",
];

/// The context of an alert, from its flags (an index into [`CONTEXT_NAMES`]): GROUND, else
/// DEGRADED_TRACK, else ALT_UNKNOWN, else AIRBORNE (the order `oa-encounter` uses).
pub fn context_of(a: &Alert) -> usize {
    if a.ground_either != 0 || a.flags & AF_ONE_GROUND != 0 {
        CTX_GROUND
    } else if a.flags & (AF_COAST | AF_UNCERTAIN) != 0 {
        CTX_DEGRADED
    } else if a.flags & AF_ALT_UNKNOWN != 0 {
        CTX_ALT_UNKNOWN
    } else {
        CTX_AIRBORNE
    }
}

fn sorted(a: &[Alert]) -> Vec<Alert> {
    let mut v = a.to_vec();
    v.sort_by_key(|x| (x.start_ns, x.addr_a, x.addr_b));
    v
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

/// `alerts.csv`: one row per alert, sorted by start time then addresses. It holds addresses and
/// is not for publication.
pub fn alerts_csv(alerts: &[Alert]) -> String {
    let mut o = String::from(
        "address_a_hex,address_b_hex,start_s,end_s,peak_s,ttl_first_s,ttl_peak_s,h_now_peak_m,h_miss_peak_m,min_h_miss_m,min_v_miss_ft,h_eff_m,v_eff_ft,mean_altitude_ft,lat_deg,lon_deg,context,flags,flags_raw,ground_either,ticks_conflict,ticks_current,ticks_unevaluated\n",
    );
    for a in sorted(alerts) {
        let _ = writeln!(
            o,
            "{:08X},{:08X},{},{},{},{},{},{},{},{},{},{},{},{},{:.5},{:.5},{},{},{},{},{},{},{}",
            a.addr_a,
            a.addr_b,
            secs(a.start_ns),
            secs(a.end_ns),
            secs(a.peak_ns),
            f(a.ttl_first_s, 1),
            f(a.ttl_peak_s, 1),
            f(a.h_now_peak_m, 1),
            f(a.h_miss_peak_m, 1),
            f(a.min_h_miss_m, 1),
            f(a.min_v_miss_ft, 0),
            f(a.h_eff_m, 1),
            f(a.v_eff_ft, 0),
            f(a.alt_ft, 0),
            f64::from(a.lat_e7) / 1e7,
            f64::from(a.lon_e7) / 1e7,
            CONTEXT_NAMES.get(context_of(&a)).copied().unwrap_or("?"),
            flags_text(a.flags),
            a.flags,
            a.ground_either,
            a.ticks_conflict,
            a.ticks_current,
            a.ticks_unevaluated
        );
    }
    o
}

/// `altitude_lost.csv`: one row per altitude loss, sorted by time then address.
pub fn events_csv(events: &[AltLost]) -> String {
    let mut o =
        String::from("time_s,address_hex,last_altitude_ft,lat_deg,lon_deg,flags_raw,on_ground\n");
    let mut v = events.to_vec();
    v.sort_by_key(|e| (e.t_ns, e.addr));
    for e in v {
        let _ = writeln!(
            o,
            "{},{:08X},{},{:.5},{:.5},{},{}",
            secs(e.t_ns),
            e.addr,
            f(e.last_alt_ft, 0),
            f64::from(e.lat_e7) / 1e7,
            f64::from(e.lon_e7) / 1e7,
            e.flags,
            u8::from(e.ground == ON_GROUND)
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

/// Days since 1970-01-01 as (year, month, day), proleptic Gregorian.
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn hour_label(hour_since_epoch: i64) -> String {
    let (y, m, d) = civil(hour_since_epoch.div_euclid(24));
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:00Z",
        hour_since_epoch.rem_euclid(24)
    )
}

/// What the summary is made from. A7.2: cold struct, size guard waived.
pub struct SummaryInput<'a> {
    pub cfg: &'a CaConfig,
    pub alerts: &'a [Alert],
    pub events: &'a [AltLost],
    pub totals: CycleStats,
    /// Cycles by the text of the state the net showed after each.
    pub net_cycles: &'a BTreeMap<String, u64>,
    pub log_overflow: u64,
    pub event_overflow: u64,
    pub tracker_overflow: u64,
    /// Valid tracker measurements refused because no track slot was available.
    pub tracker_capacity_refused: u64,
    pub measurements: u64,
    pub tick_rows: u64,
    /// Rows of the tracker's output left out for a missing column.
    pub rows_skipped: u64,
    /// Cycles that left work undone and also had input missing.
    pub cycles_both: u64,
    /// The net state after the last report of the run, the end of it included.
    pub final_net: &'a str,
    pub clip_name: &'a str,
    pub clip_sha256: &'a str,
    /// Clip rows left out because their time is in a sealed window (OA-SEAL-014).
    pub sealed_window_dropped: u64,
    pub config_name: &'a str,
    pub config_sha256: &'a str,
    pub scope: &'a str,
    pub alerts_sha256: &'a str,
    pub adaptation_manifest_sha256: Option<&'a str>,
    pub adaptation_version: Option<u64>,
}

/// True if a shortfall of the run is non-zero: work left undone, rows left out, a log that lost
/// something, or a cycle refused.
pub fn degraded(i: &SummaryInput) -> bool {
    let t = &i.totals;
    t.count_overflow
        || t.not_evaluated > 0
        || t.tracks_unscanned > 0
        || t.rows_duplicate > 0
        || t.rows_bad > 0
        || t.rows_high_lat > 0
        || t.r95_clamped > 0
        || t.open_dropped > 0
        || t.rows_over_capacity > 0
        || t.out_of_order > 0
        || t.alerts_dropped > 0
        || t.alt_lost_dropped > 0
        || t.suppressions_dropped > 0
        || t.control_lookup_refused > 0
        || t.net_refused > 0
        || t.rows_lost_upstream > 0
        || t.finish_dropped > 0
        || i.rows_skipped > 0
        || i.log_overflow > 0
        || i.event_overflow > 0
        || i.tracker_overflow > 0
        || i.tracker_capacity_refused > 0
}

/// `summary.txt`: counts only, no address and no callsign.
pub fn summary_text(i: &SummaryInput) -> String {
    let cfg = i.cfg;
    let t = &i.totals;
    let mut o = String::new();
    let _ = writeln!(o, "oa-ca summary");
    let _ = writeln!(o, "clip: {} sha256 {}", i.clip_name, i.clip_sha256);
    let _ = writeln!(o, "sealed-window rows dropped: {}", i.sealed_window_dropped);
    let _ = writeln!(
        o,
        "configuration: {} sha256 {}",
        i.config_name, i.config_sha256
    );
    let _ = writeln!(o, "tracks from: {}", i.scope);
    if let (Some(pin), Some(version)) = (i.adaptation_manifest_sha256, i.adaptation_version) {
        let _ = writeln!(
            o,
            "adaptation package: version {version} manifest sha256 {pin}"
        );
    } else {
        let _ = writeln!(o, "adaptation package: none");
    }
    let _ = writeln!(
        o,
        "look-ahead {} s, hysteresis {} s; straight-line prediction; horizontal minimum widened by both 95% radii, vertical by {:.0} ft per coasting or uncertain track",
        cfg.look_ahead_ns as f64 / 1e9,
        cfg.hysteresis_ns as f64 / 1e9,
        oa_ca::COAST_V_ALLOWANCE_FT
    );
    let _ = writeln!(
        o,
        "pair conflict cycles detected: {}; suppressed and acknowledged: {}; presented: {}",
        t.in_conflict,
        t.suppressed,
        t.in_conflict.saturating_sub(t.suppressed)
    );
    let _ = writeln!(
        o,
        "default volume: horizontal {:.2} NM, vertical {:.0} ft",
        cfg.default_volume.h_m / NM,
        cfg.default_volume.v_ft
    );
    for (k, v) in cfg.volumes.iter().take(cfg.n_volumes).enumerate() {
        let _ = writeln!(
            o,
            "volume {}: {}, {} to {} ft, horizontal {:.2} NM, vertical {:.0} ft",
            k + 1,
            match v.area {
                oa_ca::Area::Box {
                    lat_min_e7,
                    lat_max_e7,
                    lon_min_e7,
                    lon_max_e7,
                } => format!(
                    "box lat {:.4} to {:.4}, lon {:.4} to {:.4}",
                    f64::from(lat_min_e7) / 1e7,
                    f64::from(lat_max_e7) / 1e7,
                    f64::from(lon_min_e7) / 1e7,
                    f64::from(lon_max_e7) / 1e7
                ),
                oa_ca::Area::Polygon(p) => format!("polygon of {} vertices", p.n),
            },
            v.alt_lo_ft,
            v.alt_hi_ft,
            v.h_m / NM,
            v.v_ft
        );
    }
    let _ = writeln!(
        o,
        "net state: {}",
        if degraded(i) {
            "DEGRADED at some cycle (a shortfall below is non-zero)"
        } else {
            "complete (no shortfall)"
        }
    );
    let _ = writeln!(o, "\nnet state after each cycle (cycles: {})", t.cycles);
    for (k, n) in i.net_cycles {
        let _ = writeln!(o, "  {k}: {n}");
    }
    let _ = writeln!(
        o,
        "  cycles with work left undone and input missing together: {} (the state shows the work's reason with both counts)",
        i.cycles_both
    );
    let _ = writeln!(
        o,
        "net state after the end of the run (alerts ended last included): {}",
        i.final_net
    );
    let _ = writeln!(o, "\nalerts: {}", i.alerts.len());
    let _ = writeln!(o, "\ncontexts (one per alert, from its flags)");
    for (k, name) in CONTEXT_NAMES.iter().enumerate() {
        let n = i.alerts.iter().filter(|a| context_of(a) == k).count();
        let _ = writeln!(
            o,
            "  {name}: {n}  [{}]",
            CONTEXT_RULES.get(k).copied().unwrap_or("")
        );
    }
    let _ = writeln!(o, "\nalerts having each flag at any cycle");
    for (bit, name) in FLAG_NAMES {
        let n = i.alerts.iter().filter(|a| a.flags & bit != 0).count();
        let _ = writeln!(o, "  {name}: {n}");
    }
    let cur = i.alerts.iter().filter(|a| a.ticks_current > 0).count();
    let _ = writeln!(
        o,
        "\nalerts that were current (inside both minima) at some cycle: {cur}"
    );
    let kept = i.alerts.iter().filter(|a| a.ticks_unevaluated > 0).count();
    let _ = writeln!(
        o,
        "alerts kept alive over a cycle that was not evaluated: {kept}"
    );
    let mut by_hour: BTreeMap<i64, u64> = BTreeMap::new();
    for a in i.alerts {
        *by_hour
            .entry(a.start_ns.div_euclid(3_600_000_000_000))
            .or_default() += 1;
    }
    let _ = writeln!(o, "\nper hour of start (UTC)");
    for (h, n) in &by_hour {
        let _ = writeln!(o, "  {}: {n}", hour_label(*h));
    }
    let on_ground = i.events.iter().filter(|e| e.ground == ON_GROUND).count();
    let _ = writeln!(
        o,
        "\naltitude lost (a track that had an altitude and lost it): {} (on the ground when lost: {on_ground}, not: {})",
        i.events.len(),
        i.events.len() - on_ground
    );
    let _ = writeln!(
        o,
        "\nshortfalls and counts (every one is a count; none is silent)"
    );
    let _ = writeln!(
        o,
        "bad-row reason counts below overlap; they are subsets of bad rows and must not be added"
    );
    for (name, n) in [
        ("cycles", t.cycles),
        ("cycles refused (time not after the last)", t.out_of_order),
        ("track rows", i.tick_rows),
        ("measurements", i.measurements),
        ("tracks in cycles", t.tracks),
        (
            "rows with a bad position, speed, altitude or rate",
            t.rows_bad,
        ),
        ("  bad latitude (overlapping reason subset)", t.rows_bad_lat),
        (
            "  bad longitude (overlapping reason subset)",
            t.rows_bad_lon,
        ),
        (
            "  non-finite velocity component (overlapping reason subset)",
            t.rows_bad_velocity_component,
        ),
        (
            "  speed over configured maximum (overlapping reason subset)",
            t.rows_bad_speed,
        ),
        (
            "  bad altitude (overlapping reason subset)",
            t.rows_bad_altitude,
        ),
        (
            "  bad vertical rate (overlapping reason subset)",
            t.rows_bad_vrate,
        ),
        ("rows sharing an address (all left out)", t.rows_duplicate),
        ("rows over the track capacity", t.rows_over_capacity),
        ("rows above 85 degrees of latitude", t.rows_high_lat),
        ("rows whose 95% radius was held to the cap", t.r95_clamped),
        ("grid work", t.grid_work),
        ("tracks the grid scan did not reach", t.tracks_unscanned),
        ("pairs tested by the chord (no flat test)", t.pairs_chord),
        ("candidate pairs", t.candidates),
        (
            "pairs skipped: cannot converge vertically",
            t.pairs_skipped_vertical,
        ),
        ("candidates dropped from a full table", t.candidates_dropped),
        ("pairs evaluated", t.evaluated),
        ("pairs not evaluated", t.not_evaluated),
        ("  of them, projection failed", t.eval_failed),
        ("pairs left out: both on the ground", t.ground_excluded),
        ("pairs found beyond their reach", t.beyond_reach),
        (
            "pairs tested horizontally only (altitude unknown)",
            t.alt_unknown_pairs,
        ),
        (
            "pairs tested horizontally only (vertical rate unknown)",
            t.rate_unknown_pairs,
        ),
        ("pair cycles in conflict", t.in_conflict),
        ("pair cycles suppressed and acknowledged", t.suppressed),
        ("suppression audit entries dropped", t.suppressions_dropped),
        (
            "control lookups refused for work limit",
            t.control_lookup_refused,
        ),
        ("alerts opened", t.opened),
        ("alerts closed", t.closed),
        ("alerts not opened (table full)", t.open_dropped),
        ("alerts lost to a full log", i.log_overflow),
        (
            "  of them, lost at the end of the run (after the last cycle's report)",
            t.finish_dropped,
        ),
        ("altitude-loss events lost to a full log", i.event_overflow),
        ("net state reports refused by the table", t.net_refused),
        ("tracker output rows lost", i.tracker_overflow),
        (
            "valid tracker measurements refused for lack of a track slot",
            i.tracker_capacity_refused,
        ),
        (
            "tracker output rows left out: a column missing",
            i.rows_skipped,
        ),
        (
            "rows lost before a cycle (told to conflict alert)",
            t.rows_lost_upstream,
        ),
        (
            "lifetime counter overflow (totals incomplete)",
            if t.count_overflow { 1 } else { 0 },
        ),
    ] {
        let _ = writeln!(o, "  {name}: {n}");
    }
    let _ = writeln!(o, "\nalerts.csv sha256 {}", i.alerts_sha256);
    o
}
