//! The printed census: counts, shares and distributions only. The only text taken from the data
//! is the producing facility's source id, and only when it is plain letters and digits.

use crate::census::{
    ClassStats, DirStats, Group, IdShare, Results, CHANGE_BIN_NAMES, CLASS_NAMES, FLAG_NAMES,
    RESYNC_AFTER, STATUS_NAMES,
};
use crate::dist::{Summary, HIST_BINS};
use crate::hist::{Hist, COARSE_BINS};
use crate::read::ReadStats;
use crate::xcheck::{source_names, Params, OUTCOME_NAMES};
use crate::xcres::{addr_range, cidx, second_span, XcGroup, GAP_NAMES, MIN_CELL_N, Q_NAMES};
use oa_stdds::STD_ERROR_NAMES;
use oa_truth::REMOVE_NAMES;
use std::fmt::Write;

fn pct(n: u64, d: u64) -> String {
    if d == 0 {
        "n/a".to_string()
    } else {
        format!("{:.2}%", n as f64 * 100.0 / d as f64)
    }
}

/// "reconciles" or "DOES NOT RECONCILE".
fn verdict(ok: bool) -> &'static str {
    if ok {
        "reconciles"
    } else {
        "DOES NOT RECONCILE"
    }
}

fn secs(v: Option<f64>) -> String {
    v.map_or_else(|| "n/a".to_string(), |s| format!("{s:.3}"))
}

/// One line of summary for a distribution: n, median, p10, p90, zero-length count, and the
/// count at 15 s and over.
pub fn hist_line(h: &Hist) -> String {
    let (_, over) = h.coarse();
    format!(
        "n {} median {} s p10 {} s p90 {} s zero {} at-or-over-15s {} ({})",
        h.n,
        secs(h.quantile(1, 2)),
        secs(h.quantile(1, 10)),
        secs(h.quantile(9, 10)),
        h.zeros(),
        over,
        pct(over, h.n)
    )
}

fn hist_bins(out: &mut String, h: &Hist) {
    let (bins, over) = h.coarse();
    for i in 0..COARSE_BINS {
        let c = bins.get(i).copied().unwrap_or(0);
        let _ = writeln!(
            out,
            "    [{:4.1},{:4.1}) s {c} {}",
            i as f64 * 0.5,
            (i + 1) as f64 * 0.5,
            pct(c, h.n)
        );
    }
    let _ = writeln!(out, "    [15.0, 120] s {over} {}", pct(over, h.n));
}

fn ids(out: &mut String, indent: &str, s: &IdShare) {
    let _ = writeln!(out, "{indent}updates {}", s.n);
    for (name, c) in [
        ("with an address field (acAddress, any value)", s.icao_field),
        ("  of which the address reads zero", s.icao_zero),
        ("with a non-zero Mode S address", s.icao),
        ("with a Mode 3/A code (reportedBeaconCode)", s.beacon),
        ("with code 1200", s.beacon_1200),
        ("with a callsign (flightPlan/acid)", s.acid),
        ("with a flight plan (any flightPlan field)", s.plan),
        ("with non-zero address and Mode 3/A code", s.icao_and_beacon),
        (
            "with none of non-zero address, Mode 3/A code, callsign",
            s.none_of_icao_beacon_acid,
        ),
        ("new flag true", s.is_new),
    ] {
        let _ = writeln!(out, "{indent}  {name}: {c} {}", pct(c, s.n));
    }
}

fn dir_block(out: &mut String, name: &str, d: &DirStats) {
    let _ = writeln!(out, "  {name}: changes {}", d.changes);
    let _ = writeln!(
        out,
        "    interval before the change: {}",
        hist_line(&d.before)
    );
    let _ = writeln!(
        out,
        "    interval across the change: {}",
        hist_line(&d.across)
    );
    let _ = writeln!(
        out,
        "    interval after the change:  {}",
        hist_line(&d.after)
    );
}

/// The facility label: the source id when it is 1 to 8 letters and digits, else a placeholder.
pub fn fac_label(k: &[u8; 8], index: usize) -> String {
    let n = k.iter().position(|&b| b == 0).unwrap_or(8);
    let t = k.get(..n).unwrap_or(&[]);
    if t.is_empty() {
        "facility unknown".to_string()
    } else if t.iter().all(u8::is_ascii_alphanumeric) {
        String::from_utf8_lossy(t).into_owned()
    } else {
        format!("(other #{index})")
    }
}

fn class_line(c: &ClassStats) -> String {
    format!(
        "track segments {} updates false {} (non-zero address {}) true {}; intervals false: {}; true: {}",
        c.segments,
        c.updates[0],
        pct(c.false_nonzero, c.updates[0]),
        c.updates[1],
        hist_line(&c.iv[0]),
        hist_line(&c.iv[1])
    )
}

fn fac_line(g: &Group) -> String {
    // The share leaves out pseudo, frozen and coasting updates, like the class lines.
    let flagged = g.ids[0].n + g.ids[1].n;
    format!(
        "updates {} (false {} true {} absent {}) false share of the measured flagged updates {}; track segments {} (with false {}, of which false carried a non-zero address {}); interval false: {}; true: {}; false updates with non-zero address {} callsign {} flight plan {} code 1200 {}",
        g.updates.iter().sum::<u64>(),
        g.updates[0],
        g.updates[1],
        g.updates[2],
        pct(g.ids[0].n, flagged),
        g.segments,
        g.segments_with_false,
        g.segments_false_with_icao,
        hist_line(&g.iv[0]),
        hist_line(&g.iv[1]),
        pct(g.ids[0].icao, g.ids[0].n),
        pct(g.ids[0].acid, g.ids[0].n),
        pct(g.ids[0].plan, g.ids[0].n),
        pct(g.ids[0].beacon_1200, g.ids[0].n),
    )
}

fn sum_line(s: &Summary) -> String {
    let f = |v: Option<f64>| v.map_or_else(|| "n/a".to_string(), |x| format!("{x:.1}"));
    format!(
        "n {} median {} p10 {} p90 {} p99 {} (outside the histogram range: below {} above {})",
        s.n,
        f(s.p50),
        f(s.p10),
        f(s.p90),
        f(s.p99),
        s.below,
        s.above
    )
}

fn sum_bins(o: &mut String, s: &Summary) {
    let w = (s.hi - s.lo) / HIST_BINS as f64;
    for (i, c) in s.bins.iter().enumerate() {
        let _ = writeln!(
            o,
            "      [{:8.1},{:8.1}) {c} {}",
            s.lo + i as f64 * w,
            s.lo + (i + 1) as f64 * w,
            pct(*c, s.n)
        );
    }
}

fn outcomes_line(c: &[u64]) -> String {
    let total: u64 = c.iter().sum();
    OUTCOME_NAMES
        .iter()
        .zip(c)
        .map(|(n, &v)| format!("{n} {v} ({})", pct(v, total)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn counts_of(g: &XcGroup, flag: usize, gate: usize) -> Vec<u64> {
    (0..3)
        .map(|o| g.counts.get(cidx(flag, gate, o)).copied().unwrap_or(0))
        .collect()
}

fn med(s: Option<&Summary>) -> String {
    s.and_then(|s| s.p50)
        .map_or_else(|| "n/a".to_string(), |x| format!("{x:.1}"))
}

/// Label of adsb=true in the cross-check.
const FUSED: &str = " (ADS-B-fused, not independent of the truth)";

/// One line for a class or facility: the outcomes at the primary gate and, for a flag with at
/// least [`MIN_CELL_N`] matches, the median differences of the matches.
fn xc_group_line(g: &XcGroup, p: &Params) -> String {
    (0..2)
        .map(|f| xc_flag_part(g, p, f))
        .collect::<Vec<_>>()
        .join(" | ")
}

/// The part of [`xc_group_line`] for one flag.
fn xc_flag_part(g: &XcGroup, p: &Params, f: usize) -> String {
    let name = FLAG_NAMES.get(f).copied().unwrap_or_default();
    {
        let n = g.sums.get(f * 5).map_or(0, |s| s.n);
        let meds = if n < MIN_CELL_N {
            format!("n<30 (matches {n})")
        } else {
            format!(
                "matches {n}, medians horizontal {} m, vertical {} ft, along {} m, cross {} m, along {} s",
                med(g.sums.get(f * 5)),
                med(g.sums.get(f * 5 + 1)),
                med(g.sums.get(f * 5 + 2)),
                med(g.sums.get(f * 5 + 3)),
                med(g.sums.get(f * 5 + 4)),
            )
        };
        format!(
            "{name}{}: {}; {meds}; placebo-to-real ratio (rough indication, unknown direction, not a chance share) {}",
            if f == 1 { FUSED } else { "" },
            outcomes_line(&counts_of(g, f, p.primary)),
            chance_share(g, f, p.primary)
        )
    }
}

/// The placebo-to-real ratio: the placebo exactly-one matches (the mean of the two shifts) over
/// the real exactly-one matches, at the primary gate. A ratio of counts, not a probability, and
/// it can exceed 100%. The placebo shares route structure with the real match (aircraft in trail
/// about 60 s apart, slow or circling aircraft matching themselves) and is not suppressed by the
/// single-candidate rule the way a real chance match is, so the ratio is a rough indication of
/// unknown direction, not an estimate of the chance share.
fn chance_share(g: &XcGroup, f: usize, primary: usize) -> String {
    let real = g.counts.get(cidx(f, primary, 1)).copied().unwrap_or(0);
    let pl = |k: usize| g.pcounts.get((k * 2 + f) * 3 + 1).copied().unwrap_or(0);
    if real == 0 {
        return "n/a".to_string();
    }
    format!("{:.1}%", (pl(0) + pl(1)) as f64 * 50.0 / real as f64)
}

/// The adsb=false part of [`xc_group_line`].
fn xc_false_line(g: &XcGroup, p: &Params) -> String {
    xc_flag_part(g, p, 0)
}

fn at(a: &[u64], i: usize) -> u64 {
    a.get(i).copied().unwrap_or(0)
}

fn xc_section(o: &mut String, r: &Results) {
    let _ = writeln!(o, "== 5. position cross-check against ADS-B (same day) ==");
    let Some(x) = &r.xc else {
        let _ = writeln!(o, "not run (no ADS-B clip given: --adsb CLIP)");
        return;
    };
    let p = &x.params;
    let g = &x.global;
    let h_primary = p.gates_nm.get(p.primary).copied().unwrap_or(0.0);
    let _ = writeln!(
        o,
        "time compared: STDDS track/mrtTime against the ADS-B report time, each aircraft interpolated linearly between the two reports around it (at most {} ms apart); altitude: STDDS reportedAltitude (assumed feet) against ADS-B barometric altitude, gated only when both exist",
        p.max_gap_ms
    );
    let _ = writeln!(
        o,
        "gates: horizontal {:?} NM (primary {h_primary} NM), vertical {} ft; both inclusive; a candidate is one aircraft inside both gates",
        p.gates_nm, p.v_gate_ft
    );
    if let Some(w) = x.window {
        let _ = writeln!(
            o,
            "window: UTC time of day {} s to {} s, both sides; updates outside it left out: {}",
            w.from_s, w.to_s, r.outside_window
        );
    }
    let _ = writeln!(
        o,
        "caveats: STARS reuses track numbers, so a track is cut into track segments: a gap over 120 s, an update flagged new, or 3 backwards updates in a row starts a new segment, but a reuse within 120 s without the new flag is not detected. Track classes and the medians are per segment. Matches lie inside the gate by construction, so the tails are cut at the gate; the no-candidate and ambiguous counts say how many updates fall outside."
    );
    let _ = writeln!(
        o,
        "adsb=true{FUSED}: STARS fuses ADS-B into those updates, so their difference from ADS-B measures STARS latency and filtering, not accuracy; it shows what an ADS-B-built position looks like here and is no control for an error of ADS-B itself"
    );
    let _ = writeln!(
        o,
        "time: the ADS-B time is the receipt time of the report, so the along-track time is the STARS latency minus the ADS-B latency (the fleet's ADS-B latency was 0.49 s, UK-008)"
    );
    let _ = writeln!(
        o,
        "time spans (UTC, to the second, start rounded down and end up; the span of each recording, not of any aircraft): STDDS updates counted {}; ADS-B clip reports {}",
        second_span(x.stdds_span_ms),
        second_span(x.clip.span_ms)
    );
    let _ = writeln!(
        o,
        "headline, adsb=false updates by track class (an adsb=false update on a mixed track is not ADS-B-free: the same track carries ADS-B on other updates; only the all adsb=false class is without ADS-B on every update):"
    );
    for &ci in &[1usize, 3, 2] {
        if let (Some(name), Some(gr)) = (CLASS_NAMES.get(ci), x.classes.get(ci)) {
            let _ = writeln!(o, "  {name}: {}", xc_false_line(gr, p));
        }
    }
    let c = &x.clip;
    let _ = writeln!(
        o,
        "ADS-B reference: the tracker's and truth's gating (oa_truth quality_check, default config). Clip reports {} by source type, before any gating:",
        c.reports
    );
    for (name, n) in source_names().iter().zip(&c.by_source) {
        if *n > 0 {
            let _ = writeln!(o, "  {name}: {n}");
        }
    }
    let _ = writeln!(o, "  left out by the time window: {}", c.outside_window);
    for (name, n) in REMOVE_NAMES.iter().zip(&c.removed).take(8) {
        let _ = writeln!(o, "  removed by gate {name}: {n}");
    }
    let _ = writeln!(
        o,
        "  on the ground, dropped: {}; kept: {}",
        c.ground, c.kept
    );
    let _ = writeln!(
        o,
        "ADS-B index: reports {} aircraft {} interpolable segments {} (left out: faster than 600 m/s {}, above 85 degrees {}, neighbouring reports more than {} ms apart {})",
        x.points, x.aircraft, x.segments, x.segments_fast, x.segments_polar, p.max_gap_ms, x.segments_long
    );
    let _ = writeln!(
        o,
        "memory estimate (an estimate under stated assumptions, not a bound; the index, its quality-state map, the tracks map, the seen-update set and the per-segment records are counted): index {} MB, held in all {} MB",
        x.index_bytes / 1_000_000,
        x.held_bytes.saturating_add(x.index_bytes) / 1_000_000
    );
    let nc = &x.not_checked;
    let _ = writeln!(
        o,
        "updates by flag, checked and not checked (they reconcile with section 0):"
    );
    for (f, name) in FLAG_NAMES.iter().take(2).enumerate() {
        let checked: u64 = (0..3).map(|k| at(&g.counts, cidx(f, 0, k))).sum();
        let total = checked
            + at(&nc.no_position, f)
            + at(&nc.polar, f)
            + at(&nc.no_track, f)
            + at(&nc.duplicate, f)
            + at(&nc.status, f)
            + at(&nc.outside_clip, f);
        let _ = writeln!(
            o,
            "  {name}: updates {} = checked {checked} + no position or time {} + above 85 degrees {} + no trackNum {} + same time and position as the track's previous update {} + pseudo, frozen or coasting {} + outside the ADS-B clip (time span) {} ({})",
            at(&r.global.updates, f),
            at(&nc.no_position, f),
            at(&nc.polar, f),
            at(&nc.no_track, f),
            at(&nc.duplicate, f),
            at(&nc.status, f),
            at(&nc.outside_clip, f),
            verdict(total == at(&r.global.updates, f))
        );
    }
    let _ = writeln!(
        o,
        "  no flag: updates {} = not checked {} ({})",
        at(&r.global.updates, 2),
        nc.no_flag,
        verdict(at(&r.global.updates, 2) == nc.no_flag)
    );
    let _ = writeln!(o, "outcome per update (only exactly-one is used):");
    for (f, name) in FLAG_NAMES.iter().take(2).enumerate() {
        let checked: u64 = (0..3).map(|k| at(&g.counts, cidx(f, 0, k))).sum();
        let _ = writeln!(
            o,
            "  {name}: ADS-B coverage (any indexed aircraft within 30 NM at that time) {} of {checked} checked ({})",
            at(&g.covered, f),
            pct(at(&g.covered, f), checked)
        );
        for (gi, nm) in p.gates_nm.iter().enumerate() {
            let _ = writeln!(
                o,
                "  {name}, gate {nm} NM / {} ft: {}",
                p.v_gate_ft,
                outcomes_line(&counts_of(g, f, gi))
            );
        }
        let h: Vec<u64> = (0..3).map(|k| at(&g.hcounts, f * 3 + k)).collect();
        let _ = writeln!(
            o,
            "  {name}, horizontal gate {h_primary} NM with no altitude gate: {}",
            outcomes_line(&h)
        );
        let n0 = g.sums.get(f * 5).map_or(0, |s| s.n);
        let n1 = g.sums.get(f * 5 + 1).map_or(0, |s| s.n);
        let _ = writeln!(
            o,
            "  {name}: matches {n0}, of which without a vertical difference (altitude gate not applied) {}; distinct ADS-B aircraft matched {}; distinct STDDS track segments with a match {}",
            n0.saturating_sub(n1),
            at(&x.matched_aircraft, f),
            at(&g.tracks_matched, f)
        );
    }
    let _ = writeln!(
        o,
        "null control (placebo): the same positions with the STDDS time shifted by plus and minus 60 s. The placebo shares route structure with the real match (aircraft in trail about 60 s apart, slow or circling aircraft matching themselves) and is not suppressed by the single-candidate rule the way a real chance match is; its ratio to the real matches is a rough indication of unknown direction, not an estimate of the chance share. Lookups whose shifted time falls outside the clip's time span are counted apart (placebo outside the clip), never as non-matches:"
    );
    for (f, name) in FLAG_NAMES.iter().take(2).enumerate() {
        let real = counts_of(g, f, p.primary);
        let _ = writeln!(
            o,
            "  {name}, real time: {}; placebo-to-real ratio (rough indication, unknown direction, not a chance share) {}",
            outcomes_line(&real),
            chance_share(g, f, p.primary)
        );
        for (k, sh) in ["+60 s", "-60 s"].iter().enumerate() {
            let code = k * 2 + f;
            let c: Vec<u64> = (0..3).map(|j| at(&g.pcounts, code * 3 + j)).collect();
            let checked: u64 = (0..3).map(|j| at(&g.counts, cidx(f, 0, j))).sum();
            let _ = writeln!(
                o,
                "  {name}, shifted {sh}: {}; placebo outside the clip {}",
                outcomes_line(&c),
                at(&g.pout, code)
            );
            let _ = writeln!(
                o,
                "    placebo reconciliation: matched {} + not matched {} + outside the clip {} = checked {checked} ({})",
                at(&c, 1),
                at(&c, 0) + at(&c, 2),
                at(&g.pout, code),
                verdict(at(&c, 0) + at(&c, 1) + at(&c, 2) + at(&g.pout, code) == checked)
            );
            if let Some(s) = g.psums.get(code) {
                let _ = writeln!(
                    o,
                    "    horizontal difference of its exactly-one matches, metres: {}",
                    sum_line(s)
                );
            }
        }
    }
    let _ = writeln!(
        o,
        "differences STDDS minus ADS-B of the exactly-one matches at the primary gate:"
    );
    for (f, name) in FLAG_NAMES.iter().take(2).enumerate() {
        let _ = writeln!(o, "  {name}{}:", if f == 1 { FUSED } else { "" });
        let gc = g.gap_counts.get(f).copied().unwrap_or_default();
        let gt: u64 = gc.iter().sum();
        let _ = writeln!(
            o,
            "    ADS-B bracket width of the matches: {}",
            GAP_NAMES
                .iter()
                .zip(gc)
                .map(|(n, c)| format!("{n} {c} ({})", pct(c, gt)))
                .collect::<Vec<_>>()
                .join(", ")
        );
        for (q, qn) in Q_NAMES.iter().enumerate() {
            if let Some(s) = g.sums.get(f * 5 + q) {
                let _ = writeln!(o, "    {qn}: {}", sum_line(s));
                sum_bins(o, s);
            }
            if let Some(s) = g.tsums.get(f * 5 + q) {
                let _ = writeln!(
                    o,
                    "      over track segments (each segment's median, segments with 3 or more matches): {}",
                    sum_line(s)
                );
            }
        }
        if let Some(s) = g.hdv.get(f) {
            let _ = writeln!(
                o,
                "    vertical difference of horizontal-only matches, feet (wide range, to check the altitude unit): {}",
                sum_line(s)
            );
        }
    }
    let _ = writeln!(
        o,
        "adsb=true updates carrying a Mode S address, checked by address (the aircraft of that address at that time, no gate, so the differences are not cut at the gate); reported apart from the proximity matches above:"
    );
    let _ = writeln!(
        o,
        "  addressed adsb=true updates {}, with that aircraft bracketed in the clip {} ({})",
        g.addr_n,
        g.addr_found,
        pct(g.addr_found, g.addr_n)
    );
    for (q, qn) in Q_NAMES.iter().enumerate() {
        if let Some(s) = g.asums.get(q) {
            let (lo, hi) = addr_range(q, p);
            let _ = writeln!(o, "    {qn} (range {lo:.0} to {hi:.0}): {}", sum_line(s));
        }
    }
    let _ = writeln!(o, "by track class:");
    for (name, gr) in CLASS_NAMES.iter().zip(&x.classes) {
        let _ = writeln!(o, "  {name}: {}", xc_group_line(gr, p));
    }
    let _ = writeln!(o, "by facility (source id the data carries):");
    let mut order: Vec<usize> = (0..r.facs.len()).collect();
    order.sort_by_key(|&i| {
        std::cmp::Reverse(
            r.facs
                .get(i)
                .map_or(0, |(_, f)| f.updates.iter().sum::<u64>()),
        )
    });
    for i in order {
        if let (Some((k, _)), Some(gr)) = (r.facs.get(i), x.facs.get(i)) {
            let _ = writeln!(
                o,
                "  facility {}: {}",
                fac_label(k, i),
                xc_group_line(gr, p)
            );
        }
    }
}

/// The whole report.
pub fn render(rs: &ReadStats, r: &Results) -> String {
    let mut o = String::new();
    let g = &r.global;
    let _ = writeln!(
        o,
        "STDDS TAIS adsb-flag census (counts and distributions only)"
    );
    let _ = writeln!(o, "== 0. read ==");
    let _ = writeln!(
        o,
        "files {} (gzip truncated {}, gzip damaged {}); frames: message {} other {} damaged {}; frame reader stalled, bytes not read {}",
        rs.files, rs.files_gzip_truncated, rs.files_gzip_damaged, rs.frames, rs.other_frames, rs.bad_frames, rs.stalled_bytes
    );
    let _ = writeln!(
        o,
        "TAIS messages {} records {}; other-kind messages {}",
        rs.tais_messages, rs.tais_records, rs.other_messages
    );
    for (name, c) in STD_ERROR_NAMES.iter().zip(&rs.refused) {
        let _ = writeln!(o, "refused whole, {name}: {c}");
    }
    let flagged = g.ids[0].n + g.ids[1].n;
    for (name, c) in FLAG_NAMES.iter().zip(&g.updates) {
        let _ = writeln!(o, "updates {name}: {c}");
    }
    let _ = writeln!(
        o,
        "adsb=false share of flagged updates, leaving out pseudo, frozen and coasting ones (as the class lines do): {}",
        pct(g.ids[0].n, flagged)
    );
    let input = rs.tais_records;
    let accounted = g
        .updates
        .iter()
        .sum::<u64>()
        .saturating_add(r.duplicates)
        .saturating_add(r.outside_window)
        .saturating_add(r.no_time_outside);
    let _ = writeln!(
        o,
        "input TAIS records {input} = updates {} + duplicate updates dropped {} + outside the window {} + no time, outside the window {} ({})",
        g.updates.iter().sum::<u64>(),
        r.duplicates,
        r.outside_window,
        r.no_time_outside,
        verdict(input == accounted)
    );
    let _ = writeln!(
        o,
        "track segments (a track key of facility, recSTARSSrcID and trackNum, cut at every restart): {} on {} track keys; records with no trackNum {}; distinct recSTARSSrcID {}; facilities {}",
        g.segments,
        r.track_keys,
        r.no_track_num,
        r.distinct_src_ids,
        r.facs.len()
    );
    let _ = writeln!(
        o,
        "updates with no mrtTime {}; with a time before the track's previous {}; with a gap over 120 s (reset, counted not binned) {}",
        g.no_mrt, g.backwards, g.resets
    );
    let _ = writeln!(
        o,
        "track restarts: after {} backwards updates in a row {}; at an update flagged new on a track number with state {}; updates left out by the time window {}",
        RESYNC_AFTER, g.resyncs, g.new_resets, r.outside_window
    );

    let _ = writeln!(
        o,
        "duplicate updates dropped (same key, time, position and flags, across files or within one; updates without a time or track number are not checked for duplicates): {}; with a window, updates with no time, outside the window: {}",
        r.duplicates, r.no_time_outside
    );
    for (name, row) in STATUS_NAMES.iter().zip(&g.status) {
        let _ = writeln!(
            o,
            "{name} updates (in no flag tally, class, change or interval): adsb=false {} adsb=true {} adsb absent {}",
            at(row, 0),
            at(row, 1),
            at(row, 2)
        );
    }
    let _ = writeln!(
        o,
        "== 1. update interval by flag (track time mrtTime, per track) =="
    );
    for (i, name) in FLAG_NAMES.iter().take(2).enumerate() {
        if let Some(h) = g.iv.get(i) {
            let _ = writeln!(o, "{name}: {}", hist_line(h));
            hist_bins(&mut o, h);
        }
    }
    let _ = writeln!(o, "by track class (the flags seen on the track segment; pseudo, frozen and coasting updates not counted):");
    for (name, c) in CLASS_NAMES.iter().zip(&r.classes) {
        let _ = writeln!(o, "  {name}: {}", class_line(c));
    }

    let _ = writeln!(
        o,
        "track segments with no flagged update (only status updates, or only updates with no flag; in no class): {}",
        r.segments_no_flag
    );
    let _ = writeln!(o, "== 2. flag changes within a track ==");
    let _ = writeln!(
        o,
        "changes {} on {} track segments; flagged update pairs {} (change rate per pair {})",
        r.changes,
        r.segments_with_change,
        r.flagged_pairs,
        pct(r.changes, r.flagged_pairs)
    );
    let _ = writeln!(
        o,
        "track segments with 2 or more flagged updates, by number of changes:"
    );
    for (name, c) in CHANGE_BIN_NAMES.iter().zip(&r.change_bins) {
        let _ = writeln!(o, "  {name}: {c}");
    }
    dir_block(&mut o, "false to true", &r.dirs[0]);
    dir_block(&mut o, "true to false", &r.dirs[1]);

    let _ = writeln!(o, "== 3. identity carried by the updates ==");
    for (i, name) in FLAG_NAMES.iter().take(2).enumerate() {
        let _ = writeln!(o, "{name} (pseudo, frozen and coasting updates left out):");
        if let Some(s) = g.ids.get(i) {
            ids(&mut o, "  ", s);
        }
    }
    let _ = writeln!(
        o,
        "track segments with an adsb=false update: {}; of those, segments whose adsb=false updates ever carried a non-zero address: {} ({})",
        g.segments_with_false,
        g.segments_false_with_icao,
        pct(g.segments_false_with_icao, g.segments_with_false)
    );

    let _ = writeln!(
        o,
        "== 4. per source facility (the source id the data carries) =="
    );
    let _ = writeln!(o, "facilities: {}", r.facs.len());
    let mut order: Vec<usize> = (0..r.facs.len()).collect();
    order.sort_by_key(|&i| {
        std::cmp::Reverse(
            r.facs
                .get(i)
                .map_or(0, |(_, f)| f.updates.iter().sum::<u64>()),
        )
    });
    for i in order {
        if let Some((k, f)) = r.facs.get(i) {
            let _ = writeln!(o, "facility {}: {}", fac_label(k, i), fac_line(f));
        }
    }

    xc_section(&mut o, r);
    o
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::census::tests::{rec, src};
    use crate::census::Census;
    use oa_stdds::tais::tais_bit;
    use oa_stdds::TaisField;

    // REQ: OA-CEN-007
    #[test]
    fn output_has_counts_and_no_aircraft_values() {
        let mut c = Census::new();
        for i in 0..4 {
            let mut r = rec(1, i * 4600, Some(false));
            r.present |= tais_bit(TaisField::AcAddress)
                | tais_bit(TaisField::Acid)
                | tais_bit(TaisField::ReportedBeacon);
            r.mode_s_addr = 0xABCDEF;
            r.acid = *b"CALLSIGN";
            r.reported_beacon = 5432;
            r.lat_e7 = 401_234_567;
            r.lon_e7 = -747_654_321;
            r.mrt_time_ms = 1_893_553_440_123 + i * 4600;
            c.observe(src("ZZA"), &r);
        }
        let text = render(&crate::read::ReadStats::new(), &c.finish());
        for forbidden in [
            "ABCDEF", "abcdef", "CALLSIGN", "5432", "4012345", "7476543", "1893553",
        ] {
            assert!(!text.contains(forbidden), "{forbidden}");
        }
        assert!(text.contains("facility ZZA"));
        assert!(text.contains("with a non-zero Mode S address: 4 100.00%"));
        assert!(text.contains("not run"));
    }

    // REQ: OA-CEN-007
    #[test]
    fn facility_label_prints_only_plain_ids() {
        assert_eq!(fac_label(&src("ZZA"), 0), "ZZA");
        assert_eq!(fac_label(&[0; 8], 1), "facility unknown");
        let mut odd = src("AB");
        odd[2] = b'<';
        assert_eq!(fac_label(&odd, 7), "(other #7)");
    }

    // REQ: OA-CEN-001
    #[test]
    fn histogram_lines_cover_all_bins() {
        let mut h = Hist::new();
        h.add(470);
        let mut o = String::new();
        hist_bins(&mut o, &h);
        assert_eq!(o.lines().count(), COARSE_BINS + 1);
        assert!(o.contains("[ 4.5, 5.0) s 1 100.00%"));
        assert!(hist_line(&h).contains("median 4.705 s"));
    }
}
