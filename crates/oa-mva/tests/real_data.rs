//! Real-data validation against the FAA MVA and MIA charts (AIXM 5.1 and PDF), the 3DEP tiles
//! and the Digital Obstacle File. Reads `OPENATCA_DATA` (the openatca-data directory) and skips
//! when it is unset or the files are absent. Run with `--nocapture` for the reports.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_mva::check::{clearance, coverage, pdf, rings};
use oa_mva::*;
use std::path::PathBuf;

fn mva_dir() -> Option<PathBuf> {
    let Some(d) = std::env::var_os("OPENATCA_DATA").filter(|d| !d.is_empty()) else {
        eprintln!("OPENATCA_DATA is not set; skipping real-data test");
        return None;
    };
    let p = PathBuf::from(d).join("raw/mva");
    if p.join("ZAU_TAV.xml").is_file() {
        Some(p)
    } else {
        eprintln!("{} is absent; skipping real-data test", p.display());
        None
    }
}

fn loaded() -> Option<(PathBuf, DirLoad)> {
    let dir = mva_dir()?;
    let l = load_directory(&dir).unwrap();
    Some((dir, l))
}

fn label(m: &Model, c: usize) -> String {
    let s = sector_range_of_chart(m, c).start;
    format!(
        "{} {}",
        facility_text(m, SectorIx(s as u32)),
        m.chart_set[c].name()
    )
}

// REQ: OA-MVA-015
#[test]
fn real_charts_all_load_with_no_refusal() {
    let Some((_, l)) = loaded() else { return };
    for (name, r) in &l.files {
        assert!(r.is_ok(), "{name}: {r:?}");
    }
    let m = &l.model;
    eprintln!(
        "{} files, {} charts, {} sectors, {} rings, {} vertices",
        l.files.len(),
        chart_count(m),
        sector_count(m),
        m.ring_vert_count.len(),
        m.vert_lat_e7.len()
    );
    assert_eq!(l.files.len(), 35);
    assert_eq!(chart_count(m), 35);
    assert_eq!(sector_count(m), 423);
    assert_eq!(m.ring_vert_count.len(), 604);
    for set in [SetKind::Fus3, SetKind::Fus5, SetKind::Tav] {
        let alts: Vec<i32> = (0..sector_count(m))
            .filter(|&i| m.set[i] == set)
            .map(|i| m.min_alt_ft[i])
            .collect();
        eprintln!(
            "{}: {} sectors, {} to {} ft",
            set.name(),
            alts.len(),
            alts.iter().min().unwrap(),
            alts.iter().max().unwrap()
        );
        assert!(alts.iter().all(|a| (2000..=4300).contains(a)));
    }
    // Queries on known places. O'Hare (41.9786 N, 87.9048 W) is inside Chicago TRACON's charts and
    // the Chicago center chart; Rockford's field (42.1954, -89.0972) inside the RFD chart.
    for (lat, lon, set) in [
        (41.9786, -87.9048, SetKind::Fus3),
        (41.9786, -87.9048, SetKind::Fus5),
        (41.9786, -87.9048, SetKind::Tav),
        (42.1954, -89.0972, SetKind::Fus3),
    ] {
        let a = min_altitude_at(m, lat, lon, set)
            .unwrap()
            .expect("inside a chart");
        eprintln!("{lat},{lon} {}: {a:?}", set.name());
        assert!((2000..=4300).contains(&a.min_alt_ft));
    }
    // The middle of Lake Superior is in no chart.
    assert_eq!(
        min_altitude_at(m, 47.5, -88.0, SetKind::Fus3).unwrap(),
        None
    );
    // Radius lookahead from O'Hare finds the Chicago sectors and is a superset of containment.
    let near = sectors_within_radius(m, 41.9786, -87.9048, 20.0, SetKind::Fus3).unwrap();
    let mut inside = Vec::new();
    sectors_containing(m, 41.9786, -87.9048, SetKind::Fus3, &mut inside).unwrap();
    assert!(!inside.is_empty() && inside.iter().all(|s| near.contains(s)));
    eprintln!("sectors within 20 nm of O'Hare (FUS3): {}", near.len());
}

// REQ: OA-MVA-016
#[test]
fn real_rings_close_and_do_not_cross() {
    let Some((_, l)) = loaded() else { return };
    let r = rings::model_rings(&l.model);
    eprintln!("{r:?}");
    assert_eq!(r.not_closed, 0);
    assert_eq!(r.proper_crossings, 0, "a polygon crosses itself");
    assert_eq!(r.spikes, 0);
    assert_eq!(r.holes_outside, 0);
    assert_eq!(r.zero_area, 0);
}

// REQ: OA-MVA-017
#[test]
fn real_coverage_has_no_large_gap() {
    let Some((_, l)) = loaded() else { return };
    let m = &l.model;
    let (mut gaps, mut overlaps) = (0, 0);
    let mut worst_gap = 0;
    let mut worst_overlap = 0.0f64;
    let mut most_visits = 0u64;
    for c in 0..chart_count(m) {
        let (r, visits) = coverage::chart_coverage_budget(m, c, coverage::MAX_CELL_VISITS);
        let r = r.unwrap();
        most_visits = most_visits.max(visits);
        eprintln!("{}: {visits} cell visits", label(m, c));
        gaps += r.gaps.len();
        overlaps += r.overlaps.len();
        worst_gap = worst_gap.max(r.gaps.first().map_or(0, |g| g.cells));
        worst_overlap = worst_overlap.max(r.overlaps.first().map_or(0.0, |o| o.area_nm2));
        eprintln!(
            "{}: {} gaps, {} overlaps",
            label(m, c),
            r.gaps.len(),
            r.overlaps.len()
        );
    }
    eprintln!("total {gaps} gaps (largest {worst_gap} cells), {overlaps} overlapping pairs (largest {worst_overlap:.2} nm2)");
    eprintln!(
        "most cell visits for one chart {most_visits} of {}",
        coverage::MAX_CELL_VISITS
    );
    // The budget is several times the largest real chart's need.
    assert!(most_visits.saturating_mul(4) < coverage::MAX_CELL_VISITS);
    // Every pocket between sectors is smaller than a few cells (a sliver along a shared edge)
    // and every overlap is under 2 square nautical miles.
    assert!(worst_gap < 10, "a gap of {worst_gap} cells");
    assert!(worst_overlap < 2.0);
}

// REQ: OA-MVA-018
#[test]
fn real_altitudes_agree_with_the_pdf_plots() {
    let Some((dir, l)) = loaded() else { return };
    if std::process::Command::new("pdftotext")
        .arg("-v")
        .output()
        .is_err()
    {
        eprintln!("pdftotext is not installed: PDF cross-check not verified");
        return;
    }
    let m = &l.model;
    let (mut matched, mut bad_charts) = (0, Vec::new());
    for (c, st) in pdf::check_all(m, &dir).into_iter().enumerate() {
        match st {
            pdf::PdfStatus::Checked(r) => {
                matched += r.matched;
                if !r.mismatched.is_empty() {
                    bad_charts.push(r.chart.clone());
                }
            }
            pdf::PdfStatus::Unavailable(why) => eprintln!("{}: not verified: {why}", label(m, c)),
        }
    }
    eprintln!("{matched} sector altitudes match their PDF; charts with mismatches: {bad_charts:?}");
    assert!(matched >= 390);
    // Known disagreements (recorded as findings): the CVG FUS3 and TOL FUS5 plots carry the
    // same nine labels, which match neither chart's XML.
    for c in &bad_charts {
        assert!(c == "CVG FUS3" || c == "TOL FUS5", "new mismatch in {c}");
    }
}

// REQ: OA-MVA-019
#[test]
fn real_clearance_of_one_chart_against_terrain_and_obstacles() {
    let Some((_, l)) = loaded() else { return };
    let root = PathBuf::from(std::env::var_os("OPENATCA_DATA").unwrap());
    let tiles = root.join("raw/terrain/3dep");
    let zip = root.join("raw/dof/DOF_260927.zip");
    if !tiles.is_dir() || !zip.is_file() {
        eprintln!("terrain or DOF absent; skipping");
        return;
    }
    let m = &l.model;
    let bytes = oa_zip::read_archive_file(&zip).unwrap();
    let dat = oa_zip::Archive::parse(&bytes)
        .unwrap()
        .extract("DOF.DAT")
        .unwrap();
    let dof = oa_dof::parse_dof(&dat).unwrap();
    // Rockford FUS3 by default (one terrain tile); every chart with OPENATCA_MVA_FULL.
    let full = std::env::var_os("OPENATCA_MVA_FULL").is_some();
    let sectors: Vec<SectorIx> = (0..chart_count(m))
        .filter(|&c| full || label(m, c) == "RFD FUS3")
        .flat_map(|c| sector_range_of_chart(m, c))
        .map(|i| SectorIx(i as u32))
        .collect();
    assert!(!sectors.is_empty());
    let res = clearance::clearance(m, &sectors, &tiles, &dof).unwrap();
    let mut margins = Vec::new();
    for c in &res {
        let mg = clearance::margin_ft(c).expect("every sector holds terrain");
        eprintln!(
            "{} {} {}: MVA {} terrain {:?} obstacle {:?} margin {mg:.0}",
            c.facility,
            c.set,
            c.sector_id,
            c.min_alt_ft,
            c.terrain_max_ft.map(f64::round),
            c.obstacle_max_ft
        );
        assert!(c.terrain_samples > 0 && c.tiles_missing == 0 || full);
        margins.push(mg);
    }
    // The sector minimum is above its own terrain everywhere, with room: a negative margin
    // would be a finding to report, and the Rockford chart has none.
    assert!(margins.iter().all(|&x| x > 0.0), "{margins:?}");
}

// REQ: OA-MVA-020
#[test]
fn the_check_binary_prints_the_summary_and_exits_by_result() {
    let exe = env!("CARGO_BIN_EXE_oa-mva-check");
    let bad = std::process::Command::new(exe)
        .arg("--bogus")
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(2));
    let Some(dir) = mva_dir() else { return };
    // `--dof` at a path that is not there: the clearance section reports it did not run.
    let out = std::process::Command::new(exe)
        .args([
            "--dir",
            dir.to_str().unwrap(),
            "--charts",
            "RFD",
            "--dof",
            "/nonexistent.zip",
            "--terrain",
            "/nonexistent",
        ])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{text}");
    for want in [
        "== Load",
        "35 files, 35 loaded, 0 refused",
        "423 sectors",
        "== Ring integrity",
        "== Coverage",
        "== PDF cross-check",
        "== Clearance",
        "not run",
    ] {
        assert!(text.contains(want), "missing {want:?} in\n{text}");
    }
    // A directory with a refused file exits 1 and names the file.
    let tmp = std::env::temp_dir().join(format!("oa-mva-bad-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::write(tmp.join("BAD.xml"), b"<a xmlns:x=\"y\"/>").unwrap();
    let out = std::process::Command::new(exe)
        .args([
            "--dir",
            tmp.to_str().unwrap(),
            "--terrain",
            "/nonexistent",
            "--dof",
            "/nonexistent.zip",
        ])
        .output()
        .unwrap();
    std::fs::remove_dir_all(&tmp).ok();
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(text.contains("BAD.xml: REFUSED"), "{text}");
}

// REQ: OA-MVA-024
#[test]
fn real_zjx_charts_all_load_including_the_sub_area_chart() {
    let Some(d) = std::env::var_os("OPENATCA_DATA").filter(|d| !d.is_empty()) else {
        eprintln!("OPENATCA_DATA is not set; skipping real-data test");
        return;
    };
    let dir = PathBuf::from(d).join("raw/mva-zjx");
    if !dir.join("SAV_LHW_MVA.xml").is_file() {
        eprintln!("{} is absent; skipping real-data test", dir.display());
        return;
    }
    let l = load_directory(&dir).unwrap();
    for (name, r) in &l.files {
        assert!(r.is_ok(), "{name}: {r:?}");
    }
    let m = &l.model;
    eprintln!(
        "{} files, {} charts, {} sectors",
        l.files.len(),
        chart_count(m),
        sector_count(m)
    );
    assert_eq!(l.files.len(), 20);
    assert_eq!(chart_count(m), 20);
    let c = (0..chart_count(m))
        .find(|&c| m.chart_set[c] == SetKind::Sub)
        .unwrap();
    assert_eq!(chart_set_text(m, c), "SUB LHW");
    assert_eq!(
        sector_count(m) - m.set.iter().filter(|s| **s != SetKind::Sub).count(),
        17
    );
}
