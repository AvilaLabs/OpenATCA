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

/// A public airport: where it is and how high the field is.
// A7.2: cold struct, size guard waived
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Airport {
    pub lat_deg: f64,
    pub lon_deg: f64,
    pub elev_ft: f64,
}

/// The surface rule: an encounter is SURFACE when either aircraft was on the ground at the
/// closest approach, or the closest approach was within this many NM of a public airport and
/// the mean pressure altitude there was under the field elevation plus [`SURFACE_MARGIN_FT`].
pub const SURFACE_NM: f64 = 5.0;
pub const SURFACE_MARGIN_FT: f64 = 200.0;
/// One altitude quantum of the barometric altitude, feet (25 ft steps).
pub const ALT_QUANTUM_FT: f32 = 25.0;
/// The level-flight rate: both aircraft under this, feet per minute, is not converging.
pub const LEVEL_RATE_FPM: f32 = 300.0;
/// AT_VERTICAL_MINIMUM: an AIRBORNE encounter whose vertical separation at the closest approach
/// is at least the record volume's 1,000 ft less one quantum, with neither aircraft climbing or
/// descending. Aircraft on levels 1,000 ft apart read 975 to 1,000 ft in 25 ft steps and so are
/// inside the strict 1,000 ft edge only through quantisation; they are kept, counted apart.
pub fn at_vertical_minimum(e: &Encounter, volume_v_ft: f32) -> bool {
    e.v_at_closest_ft >= volume_v_ft - ALT_QUANTUM_FT
        && e.vrate_a_fpm.abs() < LEVEL_RATE_FPM
        && e.vrate_b_fpm.abs() < LEVEL_RATE_FPM
}

/// Contexts, in the order the summary leads with them.
pub const CONTEXT_NAMES: [&str; 4] = [
    "AIRBORNE",
    "AIRBORNE_ALT_UNKNOWN",
    "SURFACE",
    "DEGRADED_TRACK",
];
const CTX_AIRBORNE: usize = 0;
const CTX_ALT_UNKNOWN: usize = 1;
const CTX_SURFACE: usize = 2;
const CTX_DEGRADED: usize = 3;
/// The AIRBORNE subgroup of [`at_vertical_minimum`], a context of the summary only.
const CTX_AT_MIN: usize = 4;
const SUMMARY_NAMES: [&str; 5] = [
    "AIRBORNE",
    "AIRBORNE_ALT_UNKNOWN",
    "SURFACE",
    "DEGRADED_TRACK",
    "AIRBORNE/AT_VERTICAL_MINIMUM",
];
fn ctx_name(c: usize) -> &'static str {
    SUMMARY_NAMES.get(c).copied().unwrap_or("?")
}
/// One line for each context, said once in the summary.
const CONTEXT_RULES: [&str; 4] = [
    "both aircraft airborne or not stated on the ground, both altitudes known at the closest approach, neither track coasting or uncertain",
    "not surface, neither track coasting or uncertain, an altitude unknown on at least one track at the closest approach",
    "either aircraft on the ground at the closest approach, or within 5 NM of a public airport and under its field elevation plus 200 ft (taxiing, gates, ramp)",
    "either track coasting or uncertain at the closest approach, and not surface (position not known well, or stale)",
];

/// The context of an encounter at its closest approach (an index into [`CONTEXT_NAMES`]).
pub fn context_of(e: &Encounter, airports: &[Airport]) -> usize {
    let (lat, lon) = (f64::from(e.lat_e7) / 1e7, f64::from(e.lon_e7) / 1e7);
    let low = !e.alt_ft.is_nan()
        && airports.iter().any(|a| {
            // Flat-earth test; the margin is metres, not precision.
            let dy = (a.lat_deg - lat) * 60.0;
            if dy.abs() > SURFACE_NM {
                return false;
            }
            let mut dl = a.lon_deg - lon;
            if dl > 180.0 {
                dl -= 360.0;
            } else if dl < -180.0 {
                dl += 360.0;
            }
            let dx = dl * lat.to_radians().cos() * 60.0;
            dx.hypot(dy) <= SURFACE_NM && f64::from(e.alt_ft) < a.elev_ft + SURFACE_MARGIN_FT
        });
    if e.ground_either != 0 || low {
        CTX_SURFACE
    } else if (e.flags_a | e.flags_b) & (O_COAST | O_UNCERTAIN) != 0 {
        CTX_DEGRADED
    } else if e.v_at_closest_ft.is_nan() {
        CTX_ALT_UNKNOWN
    } else {
        CTX_AIRBORNE
    }
}

/// `encounters.csv`: one row per encounter, sorted by start time then addresses.
pub fn encounters_csv(encounters: &[Encounter], airports: &[Airport], volume_v_ft: f32) -> String {
    let mut o = String::from(
        "address_a_hex,address_b_hex,start_s,end_s,closest_s,min_horizontal_m,interp_closest_s,min_horizontal_interp_m,vertical_at_closest_ft,min_vertical_ft,min_scaled_m,context,at_vertical_minimum,class,altitude_band,mean_altitude_ft,lat_deg,lon_deg,flags_a,flags_b,flags_a_raw,flags_b_raw,vrate_a_fpm,vrate_b_fpm,max_abs_vrate_fpm,ground_either,ticks_inside,ticks_alt_unknown,ticks_unevaluated\n",
    );
    for e in sorted(encounters) {
        let _ = writeln!(
            o,
            "{:06X},{:06X},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{:.5},{:.5},{},{},{},{},{},{},{},{},{},{},{}",
            e.addr_a & 0xFF_FFFF,
            e.addr_b & 0xFF_FFFF,
            secs(e.start_ns),
            secs(e.end_ns),
            secs(e.closest_ns),
            f(e.min_h_m, 1),
            secs(e.interp_ns),
            f(e.min_h_interp_m, 1),
            f(e.v_at_closest_ft, 0),
            f(e.min_v_ft, 0),
            f(e.min_scaled_m, 1),
            CONTEXT_NAMES
                .get(context_of(&e, airports))
                .copied()
                .unwrap_or("?"),
            u8::from(context_of(&e, airports) == CTX_AIRBORNE && at_vertical_minimum(&e, volume_v_ft)),
            class_name(e.class),
            BAND_NAMES.get(band_of(e.alt_ft)).copied().unwrap_or("unknown"),
            f(e.alt_ft, 0),
            f64::from(e.lat_e7) / 1e7,
            f64::from(e.lon_e7) / 1e7,
            flags_text(e.flags_a),
            flags_text(e.flags_b),
            e.flags_a,
            e.flags_b,
            f(e.vrate_a_fpm, 0),
            f(e.vrate_b_fpm, 0),
            f(e.max_abs_vrate_fpm, 0),
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
    pub airports: &'a [Airport],
    /// What the airports came from, for the summary's header.
    pub airports_from: &'a str,
    /// Smallest separation of the two aircraft's own reports near the encounter, metres, by
    /// address pair, for the encounters at or above 18,000 ft.
    pub report_sep_m: &'a BTreeMap<(u32, u32), f32>,
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

/// A climb or descent: either aircraft's vertical rate at or over this, feet per minute.
pub const CLIMB_FPM: f32 = 300.0;
/// The two aircraft's own reports within this of each other, metres, count as coinciding.
pub const COINCIDE_M: f32 = 50.0;
/// Vertical separation at or over this (1,000 ft levels with 25 ft quantisation), feet.
pub const LEVEL_FT: f32 = 975.0;

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

fn class_counts(o: &mut String, es: &[(&Encounter, usize)], ctx: usize) {
    let mut by: BTreeMap<u8, u64> = BTreeMap::new();
    for (e, c) in es {
        if *c == ctx {
            *by.entry(e.class).or_default() += 1;
        }
    }
    for (c, n) in &by {
        let _ = writeln!(o, "  {}: {n}", class_name(*c));
    }
}

/// `summary.txt`: counts only (the per-event table of the encounters at or above 18,000 ft has no
/// address). No address, callsign or squawk appears in it.
pub fn summary_text(i: &SummaryInput) -> String {
    let cfg = i.cfg;
    let t = &i.totals;
    let mut o = String::new();
    let ctx_of: Vec<usize> = i
        .encounters
        .iter()
        .map(|e| {
            let c = context_of(e, i.airports);
            if c == CTX_AIRBORNE && at_vertical_minimum(e, cfg.volume_v_ft as f32) {
                CTX_AT_MIN
            } else {
                c
            }
        })
        .collect();
    let es: Vec<(&Encounter, usize)> = i.encounters.iter().zip(ctx_of.iter().copied()).collect();
    let degraded = t.not_evaluated
        + t.tracks_unscanned
        + t.candidates_dropped
        + t.open_dropped
        + i.log_overflow
        + t.out_of_order
        + t.rows_over_capacity
        + i.tracker_overflow
        > 0;
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
    let _ = writeln!(o, "classes within each context (tightest first; strict limits; by the sampled one-second ticks):");
    o.push_str(&class_limits(cfg));
    let _ = writeln!(
        o,
        "  ALT_UNKNOWN: altitude unknown on either track at every tick inside"
    );
    let _ = writeln!(
        o,
        "altitude: barometric pressure altitude as the tracker holds it (surface to 1,000 ft above ground is not available)"
    );
    let _ = writeln!(o, "airports for the surface rule: {}", i.airports_from);
    let _ = writeln!(
        o,
        "record state: {}",
        if degraded {
            "DEGRADED (a shortfall under work and overflow is non-zero)"
        } else {
            "complete (no shortfall)"
        }
    );
    let _ = writeln!(o, "\nencounters: {}", es.len());
    let _ = writeln!(o, "\ncontexts (one per encounter, at the closest approach)");
    for (k, name) in CONTEXT_NAMES.iter().enumerate() {
        let n = ctx_of
            .iter()
            .filter(|&&c| c == k || (k == CTX_AIRBORNE && c == CTX_AT_MIN))
            .count();
        let _ = writeln!(
            o,
            "  {name}: {n}  [{}]",
            CONTEXT_RULES.get(k).copied().unwrap_or("")
        );
        if k == CTX_AIRBORNE {
            let m = ctx_of.iter().filter(|&&c| c == CTX_AT_MIN).count();
            let _ = writeln!(
                o,
                "    of which AT_VERTICAL_MINIMUM: {m}  [vertical separation at the closest approach at least {:.0} ft (the volume's {:.0} ft less one {ALT_QUANTUM_FT:.0} ft altitude quantum) and both vertical rates under {LEVEL_RATE_FPM:.0} fpm: aircraft on levels 1,000 ft apart, inside the strict edge only because altitude is reported in {ALT_QUANTUM_FT:.0} ft steps; kept in the record and counted apart, not in the class tables below]",
                cfg.volume_v_ft as f32 - ALT_QUANTUM_FT,
                cfg.volume_v_ft
            );
        }
    }
    // AIRBORNE.
    let _ = writeln!(
        o,
        "\nAIRBORNE, vertically converging or below the minimum, by class"
    );
    class_counts(&mut o, &es, CTX_AIRBORNE);
    let mut by_band = [0u64; 5];
    let mut cross: BTreeMap<(u8, usize), u64> = BTreeMap::new();
    let mut hist = [0u64; 4];
    let (mut v_under, mut v_total) = (0u64, 0u64);
    for (e, c) in &es {
        if *c != CTX_AIRBORNE && *c != CTX_AT_MIN {
            continue;
        }
        if *c == CTX_AIRBORNE {
            let b = band_of(e.alt_ft);
            if let Some(x) = by_band.get_mut(b) {
                *x += 1;
            }
            *cross.entry((e.class, b)).or_default() += 1;
        }
        v_total += 1;
        if e.v_at_closest_ft < 900.0 {
            v_under += 1;
        } else {
            let k = ((e.v_at_closest_ft - 900.0) / 25.0) as usize;
            if let Some(x) = hist.get_mut(k) {
                *x += 1;
            }
        }
    }
    let _ = writeln!(o, "\nAIRBORNE, vertically converging or below the minimum, by pressure-altitude band (closest approach)");
    for (name, n) in BAND_NAMES.iter().zip(by_band.iter()) {
        let _ = writeln!(o, "  {name} ft: {n}");
    }
    let _ = writeln!(
        o,
        "\nAIRBORNE, vertically converging or below the minimum, class by band"
    );
    for ((c, b), n) in &cross {
        let _ = writeln!(
            o,
            "  {} {} ft: {n}",
            class_name(*c),
            BAND_NAMES.get(*b).copied().unwrap_or("unknown")
        );
    }
    let _ = writeln!(
        o,
        "\nAIRBORNE (all, with AT_VERTICAL_MINIMUM) vertical separation at the closest approach, near 1,000 ft (feet; the volume's edge is strict at 1,000)"
    );
    let _ = writeln!(o, "  under 900: {v_under} of {v_total}");
    for (k, n) in hist.iter().enumerate() {
        let lo = 900 + 25 * k;
        let _ = writeln!(o, "  {lo} to {}: {n}", lo + 25);
    }
    let _ = writeln!(o, "\nAIRBORNE/AT_VERTICAL_MINIMUM by class");
    class_counts(&mut o, &es, CTX_AT_MIN);
    // AIRBORNE_ALT_UNKNOWN.
    let _ = writeln!(o, "\nAIRBORNE_ALT_UNKNOWN by class");
    class_counts(&mut o, &es, CTX_ALT_UNKNOWN);
    let mut unknown_h = [0u64; 8];
    for (e, c) in &es {
        if *c == CTX_ALT_UNKNOWN && e.class == CLASS_ALT_UNKNOWN {
            let h = f64::from(e.min_h_m);
            let mut slot = cfg.n_classes.min(7);
            for (k, cl) in cfg.classes.iter().take(cfg.n_classes).enumerate() {
                if h < cl.h_m {
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
        "AIRBORNE_ALT_UNKNOWN, class ALT_UNKNOWN, by minimum horizontal distance"
    );
    for (k, c) in cfg.classes.iter().take(cfg.n_classes).enumerate() {
        let _ = writeln!(
            o,
            "  under {:.0} ft ({:.2} NM): {}",
            c.h_m / FT,
            c.h_m / NM,
            unknown_h.get(k).copied().unwrap_or(0)
        );
    }
    // SURFACE and DEGRADED_TRACK.
    let _ = writeln!(
        o,
        "\nSURFACE by class (counted, not analysed: {})",
        CONTEXT_RULES.get(CTX_SURFACE).copied().unwrap_or("")
    );
    class_counts(&mut o, &es, CTX_SURFACE);
    let _ = writeln!(
        o,
        "\nDEGRADED_TRACK by class (counted, not analysed: {})",
        CONTEXT_RULES.get(CTX_DEGRADED).copied().unwrap_or("")
    );
    class_counts(&mut o, &es, CTX_DEGRADED);
    let _ = writeln!(o, "\ncontext by class");
    let mut grid: BTreeMap<(usize, u8), u64> = BTreeMap::new();
    for (e, c) in &es {
        *grid.entry((*c, e.class)).or_default() += 1;
    }
    for ((c, k), n) in &grid {
        let _ = writeln!(o, "  {} {}: {n}", ctx_name(*c), class_name(*k));
    }
    // At and above 18,000 ft.
    let hi: Vec<&(&Encounter, usize)> = es.iter().filter(|(e, _)| band_of(e.alt_ft) == 3).collect();
    let _ = writeln!(
        o,
        "\nat or above 18,000 ft pressure altitude at the closest approach: {} encounters (all contexts; altitude source barometric)",
        hi.len()
    );
    let _ = writeln!(
        o,
        "  columns: context, class, vertical at closest ft, minimum vertical ft, minimum horizontal sampled m, interpolated m, duration s, vertical rate a/b at closest fpm, largest |vertical rate| fpm, flags a / b, smallest separation of the aircraft's own reports m"
    );
    let (mut n_level, mut n_climb, mut n_other) = (0u64, 0u64, 0u64);
    let (mut n_degraded, mut n_coincide) = (0u64, 0u64);
    for (e, c) in &hi {
        let sep = i.report_sep_m.get(&(e.addr_a, e.addr_b)).copied();
        let sep_text = sep.map_or_else(|| "n/a".to_string(), |s| format!("{s:.1}"));
        let _ = writeln!(
            o,
            "  {}, {}, {}, {}, {}, {}, {:.0}, {}/{}, {}, {} / {}, {}",
            ctx_name(*c),
            class_name(e.class),
            f(e.v_at_closest_ft, 0),
            f(e.min_v_ft, 0),
            f(e.min_h_m, 1),
            f(e.min_h_interp_m, 1),
            (e.end_ns - e.start_ns) as f64 / 1e9,
            f(e.vrate_a_fpm, 0),
            f(e.vrate_b_fpm, 0),
            f(e.max_abs_vrate_fpm, 0),
            flags_text(e.flags_a),
            flags_text(e.flags_b),
            sep_text
        );
        if e.v_at_closest_ft >= LEVEL_FT {
            n_level += 1;
        } else if e.max_abs_vrate_fpm >= CLIMB_FPM {
            n_climb += 1;
        } else {
            n_other += 1;
        }
        if (e.flags_a | e.flags_b) & (O_COAST | O_UNCERTAIN) != 0 {
            n_degraded += 1;
        }
        if sep.is_some_and(|s| s <= COINCIDE_M) {
            n_coincide += 1;
        }
    }
    let _ = writeln!(
        o,
        "  breakdown of the {} (each in the first that fits):",
        hi.len()
    );
    let _ = writeln!(o, "    vertical at closest at or over {LEVEL_FT:.0} ft (1,000 ft levels, 25 ft quantisation): {n_level}");
    let _ = writeln!(o, "    under that, with a vertical rate of {CLIMB_FPM:.0} fpm or more on either (climb or descent): {n_climb}");
    let _ = writeln!(o, "    anything else: {n_other}");
    let _ = writeln!(o, "  of the {}: either track coasting or uncertain: {n_degraded}; the aircraft's own reports within {COINCIDE_M:.0} m of each other at some time: {n_coincide}", hi.len());
    // Hour and cell, for the encounters that can be taken at face value.
    let mut by_hour: BTreeMap<i64, (u64, u64)> = BTreeMap::new();
    let mut by_cell: BTreeMap<(i32, i32), (u64, u64)> = BTreeMap::new();
    for (e, c) in &es {
        if *c > CTX_ALT_UNKNOWN {
            continue;
        }
        let hard = u64::from(usize::from(e.class) < 2);
        let h = by_hour
            .entry((e.start_ns / 1_000_000_000).div_euclid(3600))
            .or_default();
        h.0 += 1;
        h.1 += hard;
        let c = by_cell
            .entry((
                e.lat_e7.div_euclid(2_500_000),
                e.lon_e7.div_euclid(2_500_000),
            ))
            .or_default();
        c.0 += 1;
        c.1 += hard;
    }
    let _ = writeln!(
        o,
        "\nper hour of start (UTC), AIRBORNE and AIRBORNE_ALT_UNKNOWN: all, C1 and C2"
    );
    for (h, (n, hard)) in &by_hour {
        let _ = writeln!(o, "  {}: {n}, {hard}", hour_label(*h));
    }
    let _ = writeln!(o, "\nper 0.25 degree cell of the closest approach, AIRBORNE and AIRBORNE_ALT_UNKNOWN: all, C1 and C2");
    for ((la, lo), (n, hard)) in &by_cell {
        let (la, lo) = (f64::from(*la) * 0.25, f64::from(*lo) * 0.25);
        let _ = writeln!(
            o,
            "  lat {la:.2} to {:.2}, lon {lo:.2} to {:.2}: {n}, {hard}",
            la + 0.25,
            lo + 0.25
        );
    }
    let _ = writeln!(o, "\nwork and overflow");
    let _ = writeln!(
        o,
        "  budgets per cycle: grid work {}, pair evaluations {}, candidates kept {}; capacity: tracks {}, open encounters {}",
        cfg.work_budget, cfg.eval_budget, cfg.max_candidates, cfg.max_tracks, cfg.max_open
    );
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
    let _ = writeln!(o, "\ndigest: encounters.csv sha256 {}", i.csv_sha256);
    o
}
