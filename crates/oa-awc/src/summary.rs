//! The offline validation scan behind `oa-awc-summary`: parses every AWC JSON file under a
//! directory, finalizes the tables and reports counts. Shell code (it reads files); the parsers
//! it calls are pure.

use crate::error::{RefusedCounts, REASON_NAMES};
use crate::hazard::{
    active_hazards_at, finalize_hazard, hazard_len, hazard_name, new_hazard_table, parse_airsigmet,
    parse_gairmet, parse_sigmet, HazardDedupe, HazardStats, HazardTable, SOURCE_NAMES,
};
use crate::metar::{
    altimeter_at, finalize_metar, metar_len, metar_station_count, new_metar_table, parse_metar,
    DedupeReport, MetarStats, MetarTable,
};
use crate::pirep::{
    finalize_pirep, new_pirep_table, parse_pirep, PirepDedupe, PirepStats, PirepTable,
};
use crate::taf::{finalize_taf, new_taf_table, parse_taf, TafDedupe, TafStats, TafTable};
use crate::units::{unpack_ident, NONE_U16};
use crate::{ParseError, DEFAULT_MAX_ALTIMETER_AGE_S};
use std::fmt::Write as _;
use std::io::Read;
use std::path::{Path, PathBuf};

/// The ZAU box the hourly files were fetched with: lat 38.5 to 46.5, lon -93.5 to -83.5.
pub const ZAU_BOX: (f64, f64, f64, f64) = (38.5, 46.5, -93.5, -83.5);
/// Share of hours a station must have a trusted altimeter in to count as available.
pub const AVAILABILITY_TARGET: f64 = 0.95;
/// Bound: files read in one scan.
pub const MAX_FILES: usize = 10_000;

/// Bound: bytes of one file read, the largest document `oa_json` accepts (32 MiB). The largest
/// real file is about 150 kB.
pub const MAX_FILE_BYTES: u64 = oa_json::MAX_INPUT_BYTES as u64;

/// Reads a whole file of at most `limit` bytes: its length is checked first, and the read
/// itself stops one byte past the limit, so a file that grows meanwhile is refused too.
pub fn read_file_capped(path: &Path, limit: u64) -> std::io::Result<Vec<u8>> {
    let f = std::fs::File::open(path)?;
    if f.metadata()?.len() > limit {
        return Err(std::io::ErrorKind::InvalidInput.into());
    }
    let mut out = Vec::new();
    f.take(limit.saturating_add(1)).read_to_end(&mut out)?;
    if out.len() as u64 > limit {
        return Err(std::io::ErrorKind::InvalidInput.into());
    }
    Ok(out)
}

/// Everything the scan learned.
// A7.2: cold struct (one per run), size guard waived
pub struct Summary {
    pub metar: MetarTable,
    pub metar_stats: MetarStats,
    pub metar_dedupe: DedupeReport,
    pub pirep: PirepTable,
    pub pirep_stats: PirepStats,
    pub pirep_dedupe: PirepDedupe,
    pub taf: TafTable,
    pub taf_stats: TafStats,
    pub taf_dedupe: TafDedupe,
    /// Hazard tables by source: international SIGMET, US SIGMET/AIRMET, G-AIRMET.
    pub hazard: [HazardTable; 3],
    pub hazard_stats: [HazardStats; 3],
    pub hazard_dedupe: [HazardDedupe; 3],
    /// Files refused whole, with the reason.
    pub file_errors: Vec<(String, String)>,
    /// Files whose name matched no product.
    pub unrecognized: Vec<String>,
    pub availability: Availability,
}

/// METAR altimeter availability of the stations inside the ZAU box.
// A7.2: cold struct (one per run), size guard waived
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Availability {
    /// Full hours in the observed span; hour `k` holds observations in `(k*3600, (k+1)*3600]`, the
    /// window of one hourly file.
    pub hours: u32,
    pub stations: u32,
    /// Stations with a trusted altimeter in at least [`AVAILABILITY_TARGET`] of the hours.
    pub stations_ok: u32,
    /// (station, share of hours) for stations below the target, worst first (at most 10).
    pub worst: Vec<(String, f64)>,
    /// `altimeter_at` calls at the end of each hour for each station, and how many succeeded
    /// within the default maximum age.
    pub lookups: u32,
    pub lookups_ok: u32,
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            collect(&p, out)?;
        } else if p.extension().is_some_and(|e| e == "json") {
            out.push(p);
        }
    }
    Ok(())
}

/// Product of a file name, by prefix.
fn product(name: &str) -> Option<&'static str> {
    // `airsigmet` and `isigmet` must be tested before `sigmet`.
    if name.starts_with("metar") {
        Some("metar")
    } else if name.starts_with("pirep") {
        Some("pirep")
    } else if name.starts_with("taf") {
        Some("taf")
    } else if name.starts_with("airsigmet") {
        Some("airsigmet")
    } else if name.starts_with("gairmet") {
        Some("gairmet")
    } else if name.starts_with("sigmet") || name.starts_with("isigmet") {
        Some("sigmet")
    } else {
        None
    }
}

/// Reads and parses every `*.json` under `dir` (recursively, in path order), finalizes the
/// tables and computes the altimeter availability.
pub fn scan_dir(dir: &Path) -> Result<Summary, String> {
    let mut files = Vec::new();
    collect(dir, &mut files).map_err(|e| format!("{}: {e}", dir.display()))?;
    if files.len() > MAX_FILES {
        return Err(format!(
            "more than {MAX_FILES} files under {}",
            dir.display()
        ));
    }
    let mut s = Summary {
        metar: new_metar_table(),
        metar_stats: MetarStats::default(),
        metar_dedupe: DedupeReport::default(),
        pirep: new_pirep_table(),
        pirep_stats: PirepStats::default(),
        pirep_dedupe: PirepDedupe::default(),
        taf: new_taf_table(),
        taf_stats: TafStats::default(),
        taf_dedupe: TafDedupe::default(),
        hazard: [new_hazard_table(), new_hazard_table(), new_hazard_table()],
        hazard_stats: [HazardStats::default(); 3],
        hazard_dedupe: [HazardDedupe::default(); 3],
        file_errors: Vec::new(),
        unrecognized: Vec::new(),
        availability: Availability::default(),
    };
    for path in &files {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let Some(prod) = product(&name) else {
            s.unrecognized.push(name);
            continue;
        };
        let bytes = match read_file_capped(path, MAX_FILE_BYTES) {
            Ok(b) => b,
            Err(e) => {
                s.file_errors.push((name, format!("read: {e}")));
                continue;
            }
        };
        let r: Result<(), ParseError> = match prod {
            "metar" => parse_metar(&bytes, &mut s.metar, &mut s.metar_stats),
            "pirep" => parse_pirep(&bytes, &mut s.pirep, &mut s.pirep_stats),
            "taf" => parse_taf(&bytes, &mut s.taf, &mut s.taf_stats),
            "airsigmet" => parse_airsigmet(&bytes, &mut s.hazard[1], &mut s.hazard_stats[1]),
            "gairmet" => parse_gairmet(&bytes, &mut s.hazard[2], &mut s.hazard_stats[2]),
            _ => parse_sigmet(&bytes, &mut s.hazard[0], &mut s.hazard_stats[0]),
        };
        if let Err(e) = r {
            s.file_errors.push((name, e.to_string()));
        }
    }
    s.metar_dedupe = finalize_metar(&mut s.metar);
    s.pirep_dedupe = finalize_pirep(&mut s.pirep);
    s.taf_dedupe = finalize_taf(&mut s.taf);
    for i in 0..3 {
        if let (Some(t), Some(d)) = (s.hazard.get_mut(i), s.hazard_dedupe.get_mut(i)) {
            *d = finalize_hazard(t);
        }
    }
    s.availability = availability(&s.metar);
    Ok(s)
}

/// Altimeter availability per station inside [`ZAU_BOX`], over the hours the table spans.
pub fn availability(t: &MetarTable) -> Availability {
    let mut a = Availability::default();
    let (Some(&lo), Some(&hi)) = (t.obs_s.iter().min(), t.obs_s.iter().max()) else {
        return a;
    };
    let hour = |obs: i64| (obs - 1).div_euclid(3600);
    // An edge hour holding fewer than half the median hour's observations is a partial window
    // (a file's inclusive end point spills one observation into the next bucket) and is dropped.
    let (mut h0, mut h1) = (hour(lo), hour(hi));
    let mut count = vec![0u32; (h1 - h0 + 1) as usize];
    for &o in &t.obs_s {
        if let Some(c) = count.get_mut((hour(o) - h0) as usize) {
            *c += 1;
        }
    }
    let mut sorted = count.clone();
    sorted.sort_unstable();
    let median = sorted.get(sorted.len() / 2).copied().unwrap_or(0);
    let thin = |c: u32| u64::from(c) * 2 < u64::from(median);
    while count.first().is_some_and(|&c| thin(c)) && h0 < h1 {
        count.remove(0);
        h0 += 1;
    }
    while count.last().is_some_and(|&c| thin(c)) && h0 < h1 {
        count.pop();
        h1 -= 1;
    }
    let hours = (h1 - h0 + 1) as usize;
    a.hours = hours as u32;
    let mut i = 0;
    let n = metar_len(t);
    let mut rows: Vec<(String, f64)> = Vec::new();
    while i < n {
        let st = t.station.get(i).copied().unwrap_or(0);
        let mut j = i;
        while j < n && t.station.get(j).copied() == Some(st) {
            j += 1;
        }
        let lat = f64::from(t.lat_e5.get(i).copied().unwrap_or(0)) / 1e5;
        let lon = f64::from(t.lon_e5.get(i).copied().unwrap_or(0)) / 1e5;
        let inside =
            (ZAU_BOX.0..=ZAU_BOX.1).contains(&lat) && (ZAU_BOX.2..=ZAU_BOX.3).contains(&lon);
        if inside {
            let mut seen = vec![false; hours];
            for k in i..j {
                let (Some(&alt), Some(&obs)) = (t.altim_dhpa.get(k), t.obs_s.get(k)) else {
                    continue;
                };
                if alt != NONE_U16 {
                    let off = hour(obs) - h0;
                    if let Some(f) = usize::try_from(off).ok().and_then(|o| seen.get_mut(o)) {
                        *f = true;
                    }
                }
            }
            let share = seen.iter().filter(|&&f| f).count() as f64 / hours as f64;
            a.stations += 1;
            if share >= AVAILABILITY_TARGET {
                a.stations_ok += 1;
            } else {
                rows.push((unpack_ident(st), share));
            }
            let name = unpack_ident(st);
            for h in h0..=h1 {
                a.lookups += 1;
                if altimeter_at(t, &name, (h + 1) * 3600, DEFAULT_MAX_ALTIMETER_AGE_S).is_ok() {
                    a.lookups_ok += 1;
                }
            }
        }
        i = j;
    }
    rows.sort_by(|x, y| oa_math::cmp_canon_f64(x.1, y.1).then_with(|| x.0.cmp(&y.0)));
    rows.truncate(10);
    a.worst = rows;
    a
}

fn refused_line(c: &RefusedCounts) -> String {
    let total: u32 = c.iter().sum();
    if total == 0 {
        return "none".to_string();
    }
    let parts: Vec<String> = REASON_NAMES
        .iter()
        .zip(c.iter())
        .filter(|(_, &n)| n > 0)
        .map(|(name, n)| format!("{name}={n}"))
        .collect();
    format!("{total} ({})", parts.join(", "))
}

/// The report `oa-awc-summary` prints.
pub fn format_summary(s: &Summary) -> String {
    let mut o = String::new();
    let ms = &s.metar_stats;
    let _ = writeln!(
        o,
        "METAR  files={} records={} accepted={}",
        ms.files, ms.records, ms.accepted
    );
    let _ = writeln!(o, "  refused: {}", refused_line(&ms.refused));
    let d = &s.metar_dedupe;
    let _ = writeln!(
        o,
        "  de-duplicated: kept={} duplicates={} keys-with-differing-text={}",
        d.rows_kept, d.duplicates, d.conflicting_keys
    );
    let _ = writeln!(
        o,
        "  altim vs rawOb: compared={} disagree(>{:.1} hPa)={} max-diff={:.1} hPa; altim absent={} no-rawOb-group={}",
        ms.altim_checked,
        f64::from(crate::ALTIM_AGREE_TOL_DHPA) / 10.0,
        ms.altim_disagree,
        f64::from(ms.altim_max_diff_dhpa) / 10.0,
        ms.altim_missing,
        ms.altim_no_raw
    );
    let a = &s.availability;
    let _ = writeln!(
        o,
        "  stations={} (in ZAU box={}) hours={}",
        metar_station_count(&s.metar),
        a.stations,
        a.hours
    );
    let _ = writeln!(
        o,
        "  ZAU-box stations with an altimeter in >={:.0}% of hours: {} of {}",
        AVAILABILITY_TARGET * 100.0,
        a.stations_ok,
        a.stations
    );
    for (st, share) in &a.worst {
        let _ = writeln!(o, "    below target: {st} {:.0}%", share * 100.0);
    }
    let _ = writeln!(
        o,
        "  altimeter_at at each hour end (max age {} s): ok={} of {}",
        DEFAULT_MAX_ALTIMETER_AGE_S, a.lookups_ok, a.lookups
    );
    let ps = &s.pirep_stats;
    let _ = writeln!(
        o,
        "PIREP  files={} records={} accepted={}",
        ps.files, ps.records, ps.accepted
    );
    let _ = writeln!(o, "  refused: {}", refused_line(&ps.refused));
    let _ = writeln!(
        o,
        "  de-duplicated: kept={} duplicates={}",
        s.pirep_dedupe.rows_kept, s.pirep_dedupe.duplicates
    );
    let p = &s.pirep;
    let with = |v: &[u8]| v.iter().filter(|&&x| x > 1).count();
    let _ = writeln!(
        o,
        "  with a single flight level={} with turbulence (>NEG)={} with icing (>NEG)={}",
        p.fl.iter().filter(|&&x| x != NONE_U16).count(),
        with(&p.turb_int),
        with(&p.ice_int)
    );
    let ts = &s.taf_stats;
    let _ = writeln!(
        o,
        "TAF    files={} records={} accepted={}",
        ts.files, ts.records, ts.accepted
    );
    let _ = writeln!(o, "  refused: {}", refused_line(&ts.refused));
    let _ = writeln!(
        o,
        "  de-duplicated: kept={} duplicates={} periods={}",
        s.taf_dedupe.rows_kept,
        s.taf_dedupe.duplicates,
        s.taf.p_from_s.len()
    );
    for (i, name) in SOURCE_NAMES.iter().enumerate() {
        let (Some(st), Some(dd), Some(t)) = (
            s.hazard_stats.get(i),
            s.hazard_dedupe.get(i),
            s.hazard.get(i),
        ) else {
            continue;
        };
        let _ = writeln!(
            o,
            "{:<6} files={} records={} accepted={}",
            name.to_uppercase(),
            st.files,
            st.records,
            st.accepted
        );
        let _ = writeln!(o, "  refused: {}", refused_line(&st.refused));
        if st.points > 0 {
            let _ = writeln!(
                o,
                "  points={} rawSigmet position checked={} disagree={}",
                st.points, st.point_raw_checked, st.point_raw_disagree
            );
        }
        let mut by_hazard: Vec<(String, usize)> = Vec::new();
        for r in 0..hazard_len(t) {
            let n = hazard_name(t, r);
            match by_hazard.iter_mut().find(|(h, _)| h == n) {
                Some(e) => e.1 += 1,
                None => by_hazard.push((n.to_string(), 1)),
            }
        }
        let list: Vec<String> = by_hazard.iter().map(|(h, n)| format!("{h}={n}")).collect();
        let mut act = Vec::new();
        let latest = t.valid_from_s.iter().copied().max().unwrap_or(0);
        active_hazards_at(t, latest, &mut act);
        let _ = writeln!(
            o,
            "  de-duplicated: kept={} duplicates={} hazards: {} active at latest start: {}",
            dd.rows_kept,
            dd.duplicates,
            list.join(" "),
            act.len()
        );
    }
    if !s.file_errors.is_empty() {
        let _ = writeln!(o, "FILES REFUSED WHOLE: {}", s.file_errors.len());
        for (n, e) in &s.file_errors {
            let _ = writeln!(o, "  {n}: {e}");
        }
    }
    if !s.unrecognized.is_empty() {
        let _ = writeln!(o, "not recognised (skipped): {}", s.unrecognized.join(" "));
    }
    o
}
