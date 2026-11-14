//! Optional cross-check of decoded CIFP positions against the NASR CSV files.
//! Needs `OPENATCA_CIFP` (path to `FAACIFP18`) and `OPENATCA_NASR` (directory holding
//! `FIX_BASE.csv`, `NAV_BASE.csv`, `APT_RWY_END.csv`, `APT_BASE.csv`); skips when unset.
//!
//! This is OA-424-007 evidence at scale: where both sources carry a point, the DMS-derived
//! position must agree with the FAA's own decimal value to within rounding.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use oa_arinc424::{altitude_feet, parse_reader, runway_designator, Cifp};

/// Minimal CSV line splitter for the NASR files (quoted fields, doubled quotes; no embedded
/// newlines in the files used here).
fn split_csv(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, quoted) {
            ('"', true) if chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            ('"', _) => quoted = !quoted,
            (',', false) => out.push(std::mem::take(&mut cur)),
            (c, _) => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// Rows of a NASR CSV as column-name -> value maps. A quoted field may hold a line break,
/// so physical lines are joined while a quote is still open.
fn read_csv(dir: &Path, name: &str) -> Vec<HashMap<String, String>> {
    let f = File::open(dir.join(name)).unwrap_or_else(|e| panic!("open {name}: {e}"));
    let mut lines = BufReader::new(f).lines();
    let header = split_csv(&lines.next().expect("header").expect("read"));
    let mut rows = Vec::new();
    let mut pending = String::new();
    for l in lines {
        let l = l.expect("read");
        if !pending.is_empty() {
            pending.push('\n');
        }
        pending.push_str(&l);
        if pending.matches('"').count() % 2 == 1 {
            continue;
        }
        rows.push(header.iter().cloned().zip(split_csv(&pending)).collect());
        pending.clear();
    }
    rows
}

fn num(row: &HashMap<String, String>, col: &str) -> Option<f64> {
    row.get(col)?.trim().parse().ok()
}

fn load() -> Option<(Cifp, std::path::PathBuf)> {
    let (Ok(cifp), Ok(nasr)) = (
        std::env::var("OPENATCA_CIFP"),
        std::env::var("OPENATCA_NASR"),
    ) else {
        eprintln!("OPENATCA_CIFP / OPENATCA_NASR not set: skipping the CIFP vs NASR cross-check");
        return None;
    };
    let f = File::open(&cifp).expect("open OPENATCA_CIFP");
    Some((
        parse_reader(BufReader::new(f), &cifp).expect("parse CIFP"),
        nasr.into(),
    ))
}

/// Two decimal-degree values agree when they differ by less than a hundredth of an arc
/// second (the CIFP resolution) plus the NASR rounding.
const TOL_DEG: f64 = 3e-6;

fn close(a: f64, b: f64) -> bool {
    // +180 and -180 are the same meridian.
    let d = (a - b).abs();
    d <= TOL_DEG || (d - 360.0).abs() <= TOL_DEG
}

// REQ: OA-424-002, OA-424-007
#[test]
fn waypoints_match_nasr_fixes() {
    let Some((cifp, nasr)) = load() else { return };
    let mut fixes: HashMap<(String, String), (f64, f64)> = HashMap::new();
    for r in read_csv(&nasr, "FIX_BASE.csv") {
        if let (Some(lat), Some(lon)) = (num(&r, "LAT_DECIMAL"), num(&r, "LONG_DECIMAL")) {
            fixes.insert(
                (r["FIX_ID"].clone(), r["ICAO_REGION_CODE"].clone()),
                (lat, lon),
            );
        }
    }
    let (mut matched, mut missing, mut bad) = (0usize, 0usize, Vec::new());
    for w in cifp
        .enroute_waypoints
        .iter()
        .chain(&cifp.terminal_waypoints)
    {
        match fixes.get(&(w.ident.clone(), w.icao_region.clone())) {
            None => missing += 1,
            Some(&(lat, lon)) => {
                if close(lat, w.position.lat) && close(lon, w.position.lon) {
                    matched += 1;
                } else {
                    bad.push((w.meta.line, w.ident.clone(), w.position, (lat, lon)));
                }
            }
        }
    }
    eprintln!(
        "waypoints: {matched} match NASR, {missing} not in NASR FIX_BASE, {} disagree",
        bad.len()
    );
    for b in bad.iter().take(10) {
        eprintln!("  {b:?}");
    }
    assert!(
        bad.is_empty(),
        "{} waypoint positions disagree with NASR",
        bad.len()
    );
    assert!(matched > 30_000);
}

// REQ: OA-424-002, OA-424-007
#[test]
fn navaids_match_nasr() {
    let Some((cifp, nasr)) = load() else { return };
    // NASR gives a navaid position and, for DME/TACAN, a second position; CIFP carries both
    // a VOR and a DME position, so either CIFP position may match either NASR position.
    let mut by_id: HashMap<String, Vec<(f64, f64)>> = HashMap::new();
    // Terminal NDB identifiers are two letters and recur across the country, so they are
    // paired with NASR by identifier and name together; a CIFP-only NDB (such as ADDMO `AI`
    // at KADM, while NASR's `AI` is VIDEO in Indiana) is then not in NASR rather than a
    // position disagreement.
    let mut term_names: HashMap<String, Vec<String>> = HashMap::new();
    for r in read_csv(&nasr, "NAV_BASE.csv") {
        term_names
            .entry(r["NAV_ID"].clone())
            .or_default()
            .push(r["NAME"].clone());
        let v = by_id.entry(r["NAV_ID"].clone()).or_default();
        for (la, lo) in [
            ("LAT_DECIMAL", "LONG_DECIMAL"),
            ("TACAN_DME_LAT_DECIMAL", "TACAN_DME_LONG_DECIMAL"),
        ] {
            if let (Some(lat), Some(lon)) = (num(&r, la), num(&r, lo)) {
                v.push((lat, lon));
            }
        }
    }
    let (mut matched, mut missing) = (0usize, 0usize);
    let (mut bad_usa, mut bad_other) = (Vec::new(), Vec::new());
    let mut check = |meta_line: usize, area: &str, ident: &str, ps: Vec<oa_arinc424::Position>| {
        let Some(c) = by_id.get(ident) else {
            missing += 1;
            return;
        };
        if ps.iter().any(|p| {
            c.iter()
                .any(|&(la, lo)| close(la, p.lat) && close(lo, p.lon))
        }) {
            matched += 1;
        } else if area == "USA" {
            bad_usa.push((meta_line, ident.to_string(), ps, c.clone()));
        } else {
            bad_other.push((meta_line, area.to_string(), ident.to_string()));
        }
    };
    for v in &cifp.vhf_navaids {
        let ps: Vec<_> = v.vor_position.into_iter().chain(v.dme_position).collect();
        check(v.meta.line, &v.meta.customer_area, &v.ident, ps);
    }
    let (terminal, not_in_nasr): (Vec<_>, Vec<_>) = cifp.terminal_ndbs.iter().partition(|n| {
        term_names
            .get(&n.ident)
            .is_some_and(|v| v.contains(&n.name))
    });
    let terminal = terminal.into_iter();
    for n in cifp.ndbs.iter().chain(terminal) {
        check(
            n.meta.line,
            &n.meta.customer_area,
            &n.ident,
            vec![n.position],
        );
    }
    missing += not_in_nasr.len();
    eprintln!(
        "navaids: {matched} match NASR, {missing} not in NASR NAV_BASE, {} USA disagree, {} non-USA disagree {bad_other:?}",
        bad_usa.len(),
        bad_other.len()
    );
    for b in bad_usa.iter().take(10) {
        eprintln!("  {b:?}");
    }
    assert!(
        bad_usa.is_empty(),
        "{} US navaid positions disagree with NASR",
        bad_usa.len()
    );
    assert!(matched > 1_000);
}

// REQ: OA-424-002, OA-424-007
#[test]
fn runways_and_airports_match_nasr() {
    let Some((cifp, nasr)) = load() else { return };
    // NASR runway ends keyed by (ARPT_ID, RWY_END_ID): runway end and displaced threshold.
    type Ends = HashMap<(String, String), ((f64, f64), Option<(f64, f64)>, f64)>;
    let mut ends: Ends = HashMap::new();
    for r in read_csv(&nasr, "APT_RWY_END.csv") {
        if let (Some(lat), Some(lon)) = (num(&r, "LAT_DECIMAL"), num(&r, "LONG_DECIMAL")) {
            let disp =
                num(&r, "LAT_DISPLACED_THR_DECIMAL").zip(num(&r, "LONG_DISPLACED_THR_DECIMAL"));
            let len = num(&r, "DISPLACED_THR_LEN").unwrap_or(0.0);
            ends.insert(
                (r["ARPT_ID"].clone(), r["RWY_END_ID"].clone()),
                ((lat, lon), disp, len),
            );
        }
    }
    let mut faa_id: HashMap<(String, String), String> = HashMap::new();
    for a in &cifp.airports {
        faa_id.insert(
            (a.ident.clone(), a.icao_region.clone()),
            a.iata.clone().unwrap_or_else(|| a.ident.clone()),
        );
    }
    let (mut matched, mut missing, mut bad, mut disp_checked, mut disp_bad) =
        (0usize, 0usize, Vec::new(), 0usize, 0usize);
    for rw in &cifp.runways {
        let Some(id) = faa_id.get(&(rw.airport_ident.clone(), rw.airport_region.clone())) else {
            continue;
        };
        let Some(((lat, lon), disp, len)) =
            ends.get(&(id.clone(), runway_designator(rw).to_string()))
        else {
            missing += 1;
            continue;
        };
        let (tl, to) = match disp {
            Some(d) if *len > 0.0 => *d,
            _ => (*lat, *lon),
        };
        if close(tl, rw.threshold.lat) && close(to, rw.threshold.lon) {
            matched += 1;
        } else {
            bad.push((
                rw.meta.line,
                rw.airport_ident.clone(),
                rw.ident.clone(),
                rw.threshold,
                (tl, to),
            ));
        }
        if *len > 0.0 {
            disp_checked += 1;
            if (f64::from(rw.displaced_threshold_ft) - len).abs() > 1.0 {
                disp_bad += 1;
                if disp_bad <= 8 {
                    eprintln!(
                        "  displaced differs: {} {} cifp {} nasr {len}",
                        rw.airport_ident, rw.ident, rw.displaced_threshold_ft
                    );
                }
            }
        }
    }
    eprintln!("runways: {matched} match NASR, {missing} not found, {} disagree; displaced length checked {disp_checked}, differ {disp_bad}", bad.len());
    for b in bad.iter().take(10) {
        eprintln!("  {b:?}");
    }
    let mut apts: HashMap<String, (f64, f64)> = HashMap::new();
    for r in read_csv(&nasr, "APT_BASE.csv") {
        if let (Some(lat), Some(lon)) = (num(&r, "LAT_DECIMAL"), num(&r, "LONG_DECIMAL")) {
            apts.insert(r["ARPT_ID"].clone(), (lat, lon));
        }
    }
    let (mut am, mut ab, mut amiss) = (0usize, Vec::new(), 0usize);
    for a in &cifp.airports {
        let id = a.iata.clone().unwrap_or_else(|| a.ident.clone());
        match apts.get(&id) {
            None => amiss += 1,
            Some(&(lat, lon)) if close(lat, a.position.lat) && close(lon, a.position.lon) => {
                am += 1
            }
            Some(&p) => ab.push((a.meta.line, id, a.position, p)),
        }
    }
    eprintln!(
        "airports: {am} match NASR, {amiss} not found, {} disagree",
        ab.len()
    );
    for b in ab.iter().take(10) {
        eprintln!("  {b:?}");
    }
    assert!(
        bad.is_empty(),
        "{} runway thresholds disagree with NASR",
        bad.len()
    );
    assert!(
        ab.is_empty(),
        "{} airport reference points disagree with NASR",
        ab.len()
    );
    assert!(matched > 16_000 && am > 13_000);
    // Documented finding: 57 runway ends have a displaced threshold in NASR and the CIFP
    // coordinate is that displaced threshold, but the CIFP displacement field says 0000.
    assert!(
        disp_bad < 100,
        "displacement field disagreements grew to {disp_bad}"
    );
}

/// Counts of how many compared values agreed.
#[derive(Default, Debug)]
struct Tally {
    checked: usize,
    differ: usize,
    examples: Vec<String>,
}

impl Tally {
    fn add(&mut self, ok: bool, what: impl FnOnce() -> String) {
        self.checked += 1;
        if !ok {
            self.differ += 1;
            if self.examples.len() < 5 {
                self.examples.push(what());
            }
        }
    }
}

// REQ: OA-424-002, OA-424-007
#[test]
fn scalar_fields_match_nasr() {
    let Some((cifp, nasr)) = load() else { return };
    // Airports: elevation, magnetic variation, name.
    let mut apts: HashMap<String, HashMap<String, String>> = HashMap::new();
    for r in read_csv(&nasr, "APT_BASE.csv") {
        apts.insert(r["ARPT_ID"].clone(), r);
    }
    let (mut elev, mut name, mut var) = (Tally::default(), Tally::default(), Tally::default());
    for a in &cifp.airports {
        let id = a.iata.clone().unwrap_or_else(|| a.ident.clone());
        let Some(r) = apts.get(&id) else { continue };
        if let (Some(e), Some(ne)) = (a.elevation_ft, num(r, "ELEV")) {
            elev.add((f64::from(e) - ne).abs() <= 1.0, || {
                format!("{id}: {e} vs {ne}")
            });
        }
        name.add(r["ARPT_NAME"].starts_with(&a.name), || {
            format!("{id}: {:?} vs {:?}", a.name, r["ARPT_NAME"])
        });
        if let (Some(v), Some(nv)) = (a.magnetic_variation, num(r, "MAG_VARN")) {
            let signed = if r["MAG_HEMIS"] == "W" { -nv } else { nv };
            var.add((v - signed).abs() <= 1.0, || {
                format!("{id}: {v} vs {signed}")
            });
        }
    }
    eprintln!("airport elevation {elev:?}\nairport name {name:?}\nairport variation {var:?}");

    // Runways: length, width, threshold elevation.
    let mut faa_id: HashMap<(String, String), String> = HashMap::new();
    for a in &cifp.airports {
        faa_id.insert(
            (a.ident.clone(), a.icao_region.clone()),
            a.iata.clone().unwrap_or_else(|| a.ident.clone()),
        );
    }
    let mut rwy: HashMap<(String, String), HashMap<String, String>> = HashMap::new();
    for r in read_csv(&nasr, "APT_RWY.csv") {
        rwy.insert((r["ARPT_ID"].clone(), r["RWY_ID"].clone()), r);
    }
    let mut ends: HashMap<(String, String), HashMap<String, String>> = HashMap::new();
    let mut end_rwy: HashMap<(String, String), String> = HashMap::new();
    for r in read_csv(&nasr, "APT_RWY_END.csv") {
        end_rwy.insert(
            (r["ARPT_ID"].clone(), r["RWY_END_ID"].clone()),
            r["RWY_ID"].clone(),
        );
        ends.insert((r["ARPT_ID"].clone(), r["RWY_END_ID"].clone()), r);
    }
    let (mut len, mut width, mut telev, mut tch) = (
        Tally::default(),
        Tally::default(),
        Tally::default(),
        Tally::default(),
    );
    for r in &cifp.runways {
        let Some(id) = faa_id.get(&(r.airport_ident.clone(), r.airport_region.clone())) else {
            continue;
        };
        let key = (id.clone(), runway_designator(r).to_string());
        let (Some(end), Some(rid)) = (ends.get(&key), end_rwy.get(&key)) else {
            continue;
        };
        if let Some(base) = rwy.get(&(id.clone(), rid.clone())) {
            if let Some(l) = num(base, "RWY_LEN") {
                len.add((f64::from(r.length_ft) - l).abs() <= 1.0, || {
                    format!("{key:?}: {} vs {l}", r.length_ft)
                });
            }
            if let (Some(w), Some(nw)) = (r.width_ft, num(base, "RWY_WIDTH")) {
                width.add((f64::from(w) - nw).abs() <= 1.0, || {
                    format!("{key:?}: {w} vs {nw}")
                });
            }
        }
        if let (Some(e), Some(ne)) = (r.threshold_elevation_ft, num(end, "RWY_END_ELEV")) {
            telev.add((f64::from(e) - ne).abs() <= 1.0, || {
                format!("{key:?}: {e} vs {ne}")
            });
        }
        if let (Some(t), Some(nt)) = (r.threshold_crossing_height_ft, num(end, "THR_CROSSING_HGT"))
        {
            tch.add((f64::from(t) - nt).abs() <= 0.5, || {
                format!("{key:?}: {t} vs {nt}")
            });
        }
    }
    eprintln!(
        "runway length {len:?}\nrunway width {width:?}\nthreshold elevation {telev:?}\nTCH {tch:?}"
    );

    // VHF/NDB frequencies.
    let mut navs: HashMap<String, Vec<HashMap<String, String>>> = HashMap::new();
    for r in read_csv(&nasr, "NAV_BASE.csv") {
        navs.entry(r["NAV_ID"].clone()).or_default().push(r);
    }
    let (mut vf, mut nf) = (Tally::default(), Tally::default());
    for v in cifp
        .vhf_navaids
        .iter()
        .filter(|v| v.meta.customer_area == "USA")
    {
        let (Some(f), Some(rows)) = (v.frequency_mhz, navs.get(&v.ident)) else {
            continue;
        };
        let any = rows
            .iter()
            .any(|r| num(r, "FREQ").is_some_and(|x| (x - f).abs() < 0.005));
        if rows.iter().any(|r| num(r, "FREQ").is_some()) {
            vf.add(any, || format!("{}: {f}", v.ident));
        }
    }
    let terminal = cifp.terminal_ndbs.iter().map(|n| (true, n));
    for (is_terminal, n) in cifp.ndbs.iter().map(|n| (false, n)).chain(terminal) {
        if n.meta.customer_area != "USA" {
            continue;
        }
        let (Some(f), Some(rows)) = (n.frequency_khz, navs.get(&n.ident)) else {
            continue;
        };
        // Terminal NDB identifiers recur, so only the NASR row of the same name counts.
        let same: Vec<_> = rows
            .iter()
            .filter(|r| !is_terminal || r.get("NAME") == Some(&n.name))
            .collect();
        if same.is_empty() {
            continue;
        }
        let any = same
            .iter()
            .any(|r| num(r, "FREQ").is_some_and(|x| (x - f).abs() < 0.05));
        nf.add(any, || format!("{}: {f}", n.ident));
    }
    eprintln!("VOR frequency {vf:?}\nNDB frequency {nf:?}");

    // Longest runway (hundreds of feet): informational, the FAA readme says it may not be
    // the longest hard-surface runway.
    let mut longest: HashMap<String, u32> = HashMap::new();
    for ((arpt, _), r) in &rwy {
        if let Some(l) = num(r, "RWY_LEN") {
            let e = longest.entry(arpt.clone()).or_default();
            *e = (*e).max(l as u32);
        }
    }
    let mut lr = Tally::default();
    for a in &cifp.airports {
        let id = a.iata.clone().unwrap_or_else(|| a.ident.clone());
        if let (Some(l), Some(&n)) = (a.longest_runway_ft, longest.get(&id)) {
            lr.add(l == n / 100 * 100, || format!("{id}: {l} vs {n}"));
        }
    }
    eprintln!("longest runway {lr:?}");

    // Hard assertions: the fields that agree everywhere. The others are data differences
    // between the two FAA products and are reported above, not asserted.
    assert_eq!(
        (elev.differ, name.differ, var.differ, len.differ, nf.differ),
        (0, 0, 0, 0, 0)
    );
    assert!(elev.checked > 13_000 && len.checked > 16_000 && var.checked > 10_000);
    assert!(width.differ < 50, "999 is the wide-water-runway sentinel");
    assert!(vf.differ <= 2 && vf.checked > 1000);
    assert!(lr.differ * 10 < lr.checked);
}

// REQ: OA-424-002, OA-424-003, OA-424-007
#[test]
fn ils_and_airways_match_nasr() {
    let Some((cifp, nasr)) = load() else { return };
    // Localizers vs ILS_BASE / ILS_GS, keyed by (airport FAA id, runway end, loc id).
    let mut faa_id: HashMap<(String, String), String> = HashMap::new();
    for a in &cifp.airports {
        faa_id.insert(
            (a.ident.clone(), a.icao_region.clone()),
            a.iata.clone().unwrap_or_else(|| a.ident.clone()),
        );
    }
    let mut base: HashMap<(String, String, String), HashMap<String, String>> = HashMap::new();
    for r in read_csv(&nasr, "ILS_BASE.csv") {
        base.insert(
            (
                r["ARPT_ID"].clone(),
                r["RWY_END_ID"].clone(),
                r["ILS_LOC_ID"].clone(),
            ),
            r,
        );
    }
    let mut gs: HashMap<(String, String, String), HashMap<String, String>> = HashMap::new();
    for r in read_csv(&nasr, "ILS_GS.csv") {
        gs.insert(
            (
                r["ARPT_ID"].clone(),
                r["RWY_END_ID"].clone(),
                r["ILS_LOC_ID"].clone(),
            ),
            r,
        );
    }
    let (mut freq, mut brg, mut pos, mut gpos, mut gang, mut gelev, mut cat) = (
        Tally::default(),
        Tally::default(),
        Tally::default(),
        Tally::default(),
        Tally::default(),
        Tally::default(),
        Tally::default(),
    );
    let mut unmatched = 0usize;
    for l in &cifp.localizers {
        let Some(id) = faa_id.get(&(l.airport_ident.clone(), l.airport_region.clone())) else {
            continue;
        };
        let loc = l.ident.strip_prefix('I').unwrap_or(&l.ident).to_string();
        let rwy = l
            .runway_ident
            .strip_prefix("RW")
            .unwrap_or(&l.runway_ident)
            .to_string();
        let key = (id.clone(), rwy, loc);
        let Some(b) = base.get(&key) else {
            unmatched += 1;
            continue;
        };
        if let Some(f) = num(b, "LOC_FREQ") {
            freq.add((f - l.frequency_mhz).abs() < 0.005, || {
                format!("{key:?}: {} vs {f}", l.frequency_mhz)
            });
        }
        if let Some(x) = num(b, "APCH_BEAR") {
            brg.add((x - l.bearing).abs() <= 0.1, || {
                format!("{key:?}: {} vs {x}", l.bearing)
            });
        }
        if let (Some(la), Some(lo)) = (num(b, "LAT_DECIMAL"), num(b, "LONG_DECIMAL")) {
            pos.add(
                close(la, l.localizer_position.lat) && close(lo, l.localizer_position.lon),
                || format!("{key:?}"),
            );
        }
        let c = match b["CATEGORY"].as_str() {
            "I" => '1',
            "II" => '2',
            "III" => '3',
            _ => '?',
        };
        if c != '?' {
            cat.add(c == l.category, || {
                format!("{key:?}: {} vs {c}", l.category)
            });
        }
        if let Some(g) = gs.get(&key) {
            if let (Some(la), Some(lo), Some(p)) = (
                num(g, "LAT_DECIMAL"),
                num(g, "LONG_DECIMAL"),
                l.glide_slope_position,
            ) {
                gpos.add(close(la, p.lat) && close(lo, p.lon), || format!("{key:?}"));
            }
            if let (Some(a), Some(x)) = (l.glide_slope_angle_deg, num(g, "G_S_ANGLE")) {
                gang.add((a - x).abs() <= 0.05, || format!("{key:?}: {a} vs {x}"));
            }
            if let (Some(e), Some(x)) = (l.glide_slope_elevation_ft, num(g, "SITE_ELEVATION")) {
                gelev.add((f64::from(e) - x).abs() <= 1.0, || {
                    format!("{key:?}: {e} vs {x}")
                });
            }
        }
    }
    eprintln!("localizers not in ILS_BASE: {unmatched}");
    eprintln!("loc freq {freq:?}\nloc bearing {brg:?}\nloc pos {pos:?}\nloc cat {cat:?}\ngs pos {gpos:?}\ngs angle {gang:?}\ngs elev {gelev:?}");

    // Airways vs AWY_SEG_ALT: fix order, MEA, MAA, course, distance for conventional US airways.
    let mut segs: HashMap<String, Vec<HashMap<String, String>>> = HashMap::new();
    for r in read_csv(&nasr, "AWY_SEG_ALT.csv") {
        if r["AWY_LOCATION"] == "C" {
            segs.entry(r["AWY_ID"].clone()).or_default().push(r);
        }
    }
    let (mut order, mut mea, mut maa, mut crs, mut dist) = (
        Tally::default(),
        Tally::default(),
        Tally::default(),
        Tally::default(),
        Tally::default(),
    );
    for a in cifp.airways.iter().filter(|a| a.customer_area == "USA") {
        let Some(rows) = segs.get(&a.ident) else {
            continue;
        };
        let mut rows = rows.clone();
        rows.sort_by_key(|r| num(r, "POINT_SEQ").map_or(0, |x| x as i64));
        let ids: Vec<&str> = rows.iter().map(|r| r["FROM_POINT"].as_str()).collect();
        let cifp_ids: Vec<&str> = a.fixes.iter().map(|f| f.fix.ident.as_str()).collect();
        let ok = cifp_ids == ids;
        order.add(ok, || {
            format!(
                "{}: {} fixes vs {} segments",
                a.ident,
                cifp_ids.len(),
                ids.len()
            )
        });
        if !ok {
            continue;
        }
        for (f, r) in a.fixes.iter().zip(rows.iter()) {
            let conv = if a.route_type == 'R' {
                "GPS_MIN_ENROUTE_ALT"
            } else {
                "MIN_ENROUTE_ALT"
            };
            let nm = num(r, conv).or_else(|| num(r, "MIN_ENROUTE_ALT"));
            if let (Some(m), Some(x)) = (f.min_altitude.and_then(altitude_feet), nm) {
                mea.add(f64::from(m) == x, || {
                    format!("{} {}: {m} vs {x}", a.ident, f.fix.ident)
                });
            }
            if let (Some(m), Some(x)) = (
                f.max_altitude.and_then(altitude_feet),
                num(r, "MAX_AUTH_ALT"),
            ) {
                maa.add(f64::from(m) == x, || {
                    format!("{} {}: {m} vs {x}", a.ident, f.fix.ident)
                });
            }
            if let (Some(c), Some(x)) = (f.outbound_course, num(r, "MAG_COURSE")) {
                crs.add(
                    (c - x).abs() <= 0.15 || ((c - x).abs() - 360.0).abs() <= 0.15,
                    || format!("{} {}: {c} vs {x}", a.ident, f.fix.ident),
                );
            }
            if let (Some(d), Some(x)) = (f.distance_nm, num(r, "MAG_COURSE_DIST")) {
                dist.add((d - x).abs() <= 0.06, || {
                    format!("{} {}: {d} vs {x}", a.ident, f.fix.ident)
                });
            }
        }
    }
    eprintln!(
        "airway order {order:?}\nMEA {mea:?}\nMAA {maa:?}\ncourse {crs:?}\ndistance {dist:?}"
    );
    assert_eq!(
        (freq.differ, maa.differ, crs.differ, dist.differ),
        (0, 0, 0, 0)
    );
    assert!(freq.checked > 1200 && maa.checked > 10_000 && crs.checked > 10_000);
    assert!(
        brg.differ * 10 < brg.checked,
        "bearings differ by variation epoch"
    );
    assert!(
        pos.differ * 50 < pos.checked && gpos.differ <= 5 && gang.differ <= 2 && gelev.differ <= 10
    );
    assert!(
        order.differ * 5 < order.checked,
        "airways split differently in NASR"
    );
    assert!(mea.differ * 20 < mea.checked && mea.checked > 10_000);
    let _ = cat;
}
