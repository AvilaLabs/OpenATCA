//! The two output files, as text. Pure functions of plain values: the same encounters give the
//! same bytes (the encounters are sorted here, so the order they came in does not matter).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use oa_encounter::{
    ClassLimit, CycleStats, Encounter, EncounterConfig, CLASS_ALT_UNKNOWN, CLASS_VOLUME_ONLY,
};
use oa_track::{O_COAST, O_IDENTITY_UNCONFIRMED, O_RADAR_DISAGREES, O_REACQUIRED, O_UNCERTAIN};

const NM: f64 = 1852.0;
const FT: f64 = 0.3048;

/// Pressure-altitude bands of the summary: the lower edges, feet, and their names.
pub const BAND_NAMES: [&str; 5] = ["<3000", "3000-10000", "10000-18000", ">=18000", "unknown"];

/// The band index of a mean pressure altitude at the closest approach; 4 if it is unknown.
pub fn band_of(alt_ft: f32) -> usize {
    if alt_ft.is_nan() {
        4
    } else if alt_ft < 3000.0 {
        0
    } else if alt_ft < 10_000.0 {
        1
    } else if alt_ft < 18_000.0 {
        2
    } else {
        3
    }
}

/// The name of an encounter class: `C1`, `C2`, ..., `VOLUME_ONLY` or `ALT_UNKNOWN`.
pub fn class_name(code: u8) -> String {
    match code {
        CLASS_ALT_UNKNOWN => "ALT_UNKNOWN".to_string(),
        CLASS_VOLUME_ONLY => "VOLUME_ONLY".to_string(),
        k => format!("C{}", u32::from(k) + 1),
    }
}

/// The tracker's flag byte as names joined by `|`; empty if none. `radar_only` is the identity
/// flag of a track keyed by a radar handle.
pub fn flags_text(f: u8) -> String {
    let mut v: Vec<&str> = Vec::new();
    for (bit, name) in [
        (O_COAST, "coast"),
        (O_UNCERTAIN, "uncertain"),
        (O_IDENTITY_UNCONFIRMED, "radar_only"),
        (O_RADAR_DISAGREES, "radar_disagrees"),
        (O_REACQUIRED, "reacquired"),
    ] {
        if f & bit != 0 {
            v.push(name);
        }
    }
    v.join("|")
}

fn sorted(e: &[Encounter]) -> Vec<Encounter> {
    let mut v = e.to_vec();
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

/// `encounters.csv`: one row per encounter, sorted by start time then addresses.
pub fn encounters_csv(encounters: &[Encounter]) -> String {
    let mut o = String::from(
        "address_a_hex,address_b_hex,start_s,end_s,closest_s,min_horizontal_m,vertical_at_closest_ft,min_scaled_m,class,altitude_band,mean_altitude_ft,lat_deg,lon_deg,flags_a,flags_b,ground_either,ticks_inside,ticks_alt_unknown,ticks_unevaluated\n",
    );
    for e in sorted(encounters) {
        let _ = writeln!(
            o,
            "{:06X},{:06X},{},{},{},{},{},{},{},{},{},{:.5},{:.5},{},{},{},{},{},{}",
            e.addr_a & 0xFF_FFFF,
            e.addr_b & 0xFF_FFFF,
            secs(e.start_ns),
            secs(e.end_ns),
            secs(e.closest_ns),
            f(e.min_h_m, 1),
            f(e.v_at_closest_ft, 0),
            f(e.min_scaled_m, 1),
            class_name(e.class),
            BAND_NAMES
                .get(band_of(e.alt_ft))
                .copied()
                .unwrap_or("unknown"),
            f(e.alt_ft, 0),
            f64::from(e.lat_e7) / 1e7,
            f64::from(e.lon_e7) / 1e7,
            flags_text(e.flags_a),
            flags_text(e.flags_b),
            e.ground_either,
            e.ticks_inside,
            e.ticks_alt_unknown,
            e.ticks_unevaluated
        );
    }
    o
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
    pub cfg: &'a EncounterConfig,
    pub encounters: &'a [Encounter],
    pub totals: CycleStats,
    /// Encounters the tool's log lost (must be 0).
    pub log_overflow: u64,
    /// Rows the tracker's output table lost (must be 0).
    pub tracker_overflow: u64,
    pub measurements: u64,
    pub tick_rows: u64,
    pub clip_name: &'a str,
    pub clip_sha256: &'a str,
    pub scope: &'a str,
    pub csv_sha256: &'a str,
}

fn class_limits(cfg: &EncounterConfig) -> String {
    let mut s = String::new();
    for (k, c) in cfg.classes.iter().take(cfg.n_classes).enumerate() {
        let ClassLimit { h_m, v_ft } = *c;
        let _ = writeln!(
            s,
            "  {}: horizontal < {:.1} m ({:.2} NM, {:.0} ft), vertical < {:.0} ft",
            class_name(k as u8),
            h_m,
            h_m / NM,
            h_m / FT,
            v_ft
        );
    }
    s
}

/// `summary.txt`: counts only. No address, callsign or squawk appears in it.
pub fn summary_text(i: &SummaryInput) -> String {
    let cfg = i.cfg;
    let t = &i.totals;
    let mut o = String::new();
    let _ = writeln!(o, "oa-encounter summary");
    let _ = writeln!(o, "clip: {} sha256 {}", i.clip_name, i.clip_sha256);
    let _ = writeln!(o, "tracks from: {}", i.scope);
    let _ = writeln!(
        o,
        "record volume: horizontal < {:.1} m ({:.2} NM), vertical < {:.0} ft; hysteresis {} s",
        cfg.volume_h_m,
        cfg.volume_h_m / NM,
        cfg.volume_v_ft,
        cfg.hysteresis_ns / 1_000_000_000
    );
    let _ = writeln!(o, "classes (tightest first; strict limits):");
    o.push_str(&class_limits(cfg));
    let _ = writeln!(
        o,
        "  ALT_UNKNOWN: altitude unknown on either track at every tick inside; by horizontal distance below"
    );
    let _ = writeln!(
        o,
        "altitude bands: pressure altitude at the closest approach (surface to 1,000 ft above ground is not available)"
    );
    let _ = writeln!(
        o,
        "budgets per cycle: grid work {}, pair evaluations {}, candidates kept {}; capacity: tracks {}, open encounters {}",
        cfg.work_budget, cfg.eval_budget, cfg.max_candidates, cfg.max_tracks, cfg.max_open
    );
    let _ = writeln!(o, "\nwork and overflow");
    let _ = writeln!(
        o,
        "  measurements {}, tracker tick rows {}, tracker output rows lost {}",
        i.measurements, i.tick_rows, i.tracker_overflow
    );
    for (name, v) in [
        ("cycles", t.cycles),
        ("cycles refused (time not after the last)", t.out_of_order),
        ("track-ticks", t.tracks),
        ("rows with no valid position", t.rows_bad),
        ("rows with a duplicate address (left out)", t.rows_duplicate),
        ("rows over track capacity", t.rows_over_capacity),
        ("grid work units", t.grid_work),
        ("tracks not scanned (grid budget)", t.tracks_unscanned),
        ("candidate pairs", t.candidates),
        ("candidate pairs dropped (table full)", t.candidates_dropped),
        ("pairs evaluated", t.evaluated),
        ("pairs not evaluated (budget)", t.not_evaluated),
        ("pair-ticks excluded, both on the ground", t.ground_excluded),
        ("pair-ticks inside the record volume", t.in_volume),
        ("encounters opened", t.opened),
        ("encounters closed", t.closed),
        ("encounters not opened (open table full)", t.open_dropped),
        ("encounters lost (log full)", i.log_overflow),
    ] {
        let _ = writeln!(o, "  {name}: {v}");
    }
    let degraded = t.not_evaluated
        + t.tracks_unscanned
        + t.candidates_dropped
        + t.open_dropped
        + i.log_overflow
        + t.out_of_order
        + t.rows_over_capacity
        + i.tracker_overflow
        > 0;
    let _ = writeln!(
        o,
        "  record state: {}",
        if degraded {
            "DEGRADED (a shortfall above is non-zero)"
        } else {
            "complete (no shortfall)"
        }
    );
    let es = i.encounters;
    let _ = writeln!(o, "\nencounters: {}", es.len());
    // Class, band, hour and cell tables.
    let mut by_class: BTreeMap<u8, u64> = BTreeMap::new();
    let mut by_band = [0u64; 5];
    let mut cross: BTreeMap<(u8, usize), u64> = BTreeMap::new();
    let mut by_hour: BTreeMap<i64, (u64, u64)> = BTreeMap::new();
    let mut by_cell: BTreeMap<(i32, i32), (u64, u64)> = BTreeMap::new();
    let mut unknown_h = [0u64; 8];
    let mut ground = 0u64;
    let mut ground_unknown = 0u64;
    let hard = |c: u8| usize::from(c) < 2;
    for e in es {
        *by_class.entry(e.class).or_default() += 1;
        let b = band_of(e.alt_ft);
        if let Some(x) = by_band.get_mut(b) {
            *x += 1;
        }
        *cross.entry((e.class, b)).or_default() += 1;
        let h = by_hour
            .entry((e.start_ns / 1_000_000_000).div_euclid(3600))
            .or_default();
        h.0 += 1;
        h.1 += u64::from(hard(e.class));
        let c = by_cell
            .entry((
                e.lat_e7.div_euclid(2_500_000),
                e.lon_e7.div_euclid(2_500_000),
            ))
            .or_default();
        c.0 += 1;
        c.1 += u64::from(hard(e.class));
        if e.ground_either != 0 {
            ground += 1;
        }
        if e.class == CLASS_ALT_UNKNOWN {
            if e.ground_either != 0 {
                ground_unknown += 1;
            }
            let h = f64::from(e.min_h_m);
            let mut slot = cfg.n_classes.min(7);
            for (k, c) in cfg.classes.iter().take(cfg.n_classes).enumerate() {
                if h < c.h_m {
                    slot = k;
                    break;
                }
            }
            if let Some(x) = unknown_h.get_mut(slot) {
                *x += 1;
            }
        }
    }
    let _ = writeln!(
        o,
        "\nby class (tightest class any tick with known altitude met)"
    );
    for (c, n) in &by_class {
        let _ = writeln!(o, "  {}: {n}", class_name(*c));
    }
    let _ = writeln!(o, "\nby pressure-altitude band (closest approach)");
    for (name, n) in BAND_NAMES.iter().zip(by_band.iter()) {
        let _ = writeln!(o, "  {name} ft: {n}");
    }
    let _ = writeln!(o, "\nclass by band");
    for ((c, b), n) in &cross {
        let _ = writeln!(
            o,
            "  {} {} ft: {n}",
            class_name(*c),
            BAND_NAMES.get(*b).copied().unwrap_or("unknown")
        );
    }
    let _ = writeln!(o, "\nALT_UNKNOWN by minimum horizontal distance");
    for (k, c) in cfg.classes.iter().take(cfg.n_classes).enumerate() {
        let _ = writeln!(
            o,
            "  under {:.0} ft ({:.2} NM): {}",
            c.h_m / FT,
            c.h_m / NM,
            unknown_h.get(k).copied().unwrap_or(0)
        );
    }
    let _ = writeln!(
        o,
        "  outside the classes' distances: {}",
        unknown_h.get(cfg.n_classes.min(7)).copied().unwrap_or(0)
    );
    let _ = writeln!(
        o,
        "\nencounters with either aircraft on the ground at the closest approach: {ground} (of them ALT_UNKNOWN: {ground_unknown})"
    );
    let _ = writeln!(o, "\nper hour of start (UTC): all, C1 and C2");
    for (h, (n, hard)) in &by_hour {
        let _ = writeln!(o, "  {}: {n}, {hard}", hour_label(*h));
    }
    let _ = writeln!(
        o,
        "\nper 0.25 degree cell of the closest approach: all, C1 and C2"
    );
    for ((la, lo), (n, hard)) in &by_cell {
        let (la, lo) = (f64::from(*la) * 0.25, f64::from(*lo) * 0.25);
        let _ = writeln!(
            o,
            "  lat {la:.2} to {:.2}, lon {lo:.2} to {:.2}: {n}, {hard}",
            la + 0.25,
            lo + 0.25
        );
    }
    let _ = writeln!(o, "\ndigest: encounters.csv sha256 {}", i.csv_sha256);
    o
}
