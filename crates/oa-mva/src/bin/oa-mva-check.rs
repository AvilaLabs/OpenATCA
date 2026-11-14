#![forbid(unsafe_code)]
//! `oa-mva-check [--dir <mva dir>] [--terrain <3dep dir> --dof <DOF zip>] [--charts A,B]`
//! `[--list <n>]`
//!
//! Loads every chart in the directory and prints the validation summary: sector counts and
//! altitude ranges, refused files, ring integrity, coverage gaps and overlaps, the PDF
//! cross-check and (when terrain and DOF are given) the clearance margins. With `OPENATCA_DATA`
//! set, the defaults are `$OPENATCA_DATA/raw/mva`, `.../raw/terrain/3dep` and the newest
//! `.../raw/dof/DOF_*.zip`. Exit code 0 when everything loaded, 1 when a file was refused,
//! 2 on bad arguments.

use oa_mva::check::{clearance, coverage, pdf, rings};
use oa_mva::{
    chart_count, facility_text, load_directory, sector_count, sector_range_of_chart, Model,
    SectorIx, SetKind,
};
use std::path::PathBuf;
use std::process::ExitCode;

struct Args {
    dir: PathBuf,
    terrain: Option<PathBuf>,
    dof: Option<PathBuf>,
    charts: Option<Vec<String>>,
    list: usize,
}

fn parse_args() -> Result<Args, String> {
    let data = std::env::var_os("OPENATCA_DATA").map(PathBuf::from);
    let mut a = Args {
        dir: data.as_ref().map(|d| d.join("raw/mva")).unwrap_or_default(),
        terrain: data.as_ref().map(|d| d.join("raw/terrain/3dep")),
        dof: data.as_ref().and_then(|d| newest_dof(&d.join("raw/dof"))),
        charts: None,
        list: 10,
    };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let v = it.next().ok_or(format!("missing value for {k}"))?;
        match k.as_str() {
            "--dir" => a.dir = PathBuf::from(v),
            "--terrain" => a.terrain = Some(PathBuf::from(v)),
            "--dof" => a.dof = Some(PathBuf::from(v)),
            "--charts" => a.charts = Some(v.split(',').map(str::to_string).collect()),
            "--list" => a.list = v.parse().map_err(|_| "bad --list".to_string())?,
            _ => return Err(format!("unknown argument {k}")),
        }
    }
    if a.dir.as_os_str().is_empty() {
        return Err("no --dir and OPENATCA_DATA is unset".to_string());
    }
    Ok(a)
}

fn newest_dof(dir: &std::path::Path) -> Option<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("DOF_") && n.ends_with(".zip"))
        })
        .collect();
    v.sort();
    v.pop()
}

fn chart_label(m: &Model, c: usize) -> String {
    let s = sector_range_of_chart(m, c).start;
    format!(
        "{} {}",
        facility_text(m, SectorIx(s as u32)),
        m.chart_set.get(c).map_or("?", |s| s.name())
    )
}

fn main() -> ExitCode {
    let a = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("oa-mva-check: {e}");
            eprintln!("usage: oa-mva-check [--dir D] [--terrain D --dof ZIP] [--charts C90,ZAU] [--list N]");
            return ExitCode::from(2);
        }
    };
    let load = match load_directory(&a.dir) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("oa-mva-check: {}: {e}", a.dir.display());
            return ExitCode::from(1);
        }
    };
    let m = &load.model;
    println!("== Load ({})", a.dir.display());
    let mut refused = 0;
    for (name, r) in &load.files {
        match r {
            Ok(l) => println!(
                "  {name}: {} sectors, {} rings, {} vertices",
                l.sectors, l.rings, l.vertices
            ),
            Err(e) => {
                refused += 1;
                println!("  {name}: REFUSED: {e}");
            }
        }
    }
    println!(
        "  {} files, {} loaded, {} refused; {} charts, {} sectors, {} vertices",
        load.files.len(),
        load.files.len() - refused,
        refused,
        chart_count(m),
        sector_count(m),
        m.vert_lat_e7.len()
    );
    println!("== Altitude ranges (feet MSL)");
    for set in [SetKind::Fus3, SetKind::Fus5, SetKind::Tav] {
        let alts: Vec<i32> = (0..sector_count(m))
            .filter(|&i| m.set.get(i) == Some(&set))
            .filter_map(|i| m.min_alt_ft.get(i).copied())
            .collect();
        if let (Some(lo), Some(hi)) = (alts.iter().min(), alts.iter().max()) {
            println!("  {}: {} sectors, {lo} to {hi}", set.name(), alts.len());
        }
    }
    println!("== Ring integrity (counted, nothing repaired)");
    let t = rings::model_rings(m);
    println!(
        "  {} rings, {} vertices: not closed {}, proper crossings {}, touches {}, spikes {}, repeated vertices {}, zero-area {}, clockwise exteriors {}, holes outside exterior {}",
        t.rings, t.vertices, t.not_closed, t.proper_crossings, t.touches, t.spikes,
        t.repeated_vertices, t.zero_area, t.clockwise_exteriors, t.holes_outside
    );
    for i in 0..sector_count(m) {
        let r = rings::sector_rings(m, SectorIx(i as u32));
        if r.proper_crossings + r.touches + r.spikes + r.holes_outside + r.zero_area > 0 {
            println!(
                "    {} {} {}: crossings {}, touches {}, spikes {}, zero-area {}, holes outside {}",
                facility_text(m, SectorIx(i as u32)),
                m.set.get(i).map_or("?", |s| s.name()),
                oa_mva::sector_name_text(m, SectorIx(i as u32)),
                r.proper_crossings,
                r.touches,
                r.spikes,
                r.zero_area,
                r.holes_outside
            );
        }
    }
    println!(
        "== Coverage (cell {} deg; gaps = uncovered pockets inside the union of a chart)",
        coverage::COVERAGE_CELL_DEG
    );
    let (mut ngap, mut gap_area, mut nov, mut ov_area) = (0, 0.0, 0, 0.0);
    let (mut max_gap, mut max_ov) = (0usize, 0usize);
    for c in 0..chart_count(m) {
        let label = chart_label(m, c);
        if let Some(only) = &a.charts {
            if !only.iter().any(|f| label.starts_with(f.as_str())) {
                continue;
            }
        }
        match coverage::chart_coverage(m, c) {
            Ok(r) => {
                let ga: f64 = r.gaps.iter().map(|g| g.area_nm2).sum();
                let oa: f64 = r.overlaps.iter().map(|o| o.area_nm2).sum();
                max_gap = max_gap.max(r.gaps.first().map_or(0, |g| g.cells));
                max_ov = max_ov.max(r.overlaps.first().map_or(0, |o| o.cells));
                ngap += r.gaps.len();
                gap_area += ga;
                nov += r.overlaps.len();
                ov_area += oa;
                println!(
                    "  {label}: {} sectors, covered {:.0} nm2, {} gaps ({:.2} nm2), {} overlapping pairs ({:.2} nm2)",
                    r.sectors, r.covered_area_nm2, r.gaps.len(), ga, r.overlaps.len(), oa
                );
                for g in r.gaps.iter().take(a.list) {
                    println!(
                        "    gap {:.2} nm2 ({} cells) near {:.4},{:.4}{}",
                        g.area_nm2,
                        g.cells,
                        g.lat,
                        g.lon,
                        if g.covered_by_other_facility {
                            " [covered by another facility]"
                        } else {
                            ""
                        }
                    );
                }
                for o in r.overlaps.iter().take(a.list) {
                    println!(
                        "    overlap {}-{} ({} vs {} ft): {:.2} nm2 ({} cells), extent {:.1} nm (mean width {:.2} nm) near {:.4},{:.4}",
                        o.a, o.b, o.alt_a, o.alt_b, o.area_nm2, o.cells, o.extent_nm,
                        o.area_nm2 / o.extent_nm.max(1e-9), o.lat, o.lon
                    );
                }
            }
            Err(e) => println!("  {label}: not rasterised: {e}"),
        }
    }
    println!("  total: {ngap} gaps ({gap_area:.1} nm2; largest {max_gap} cells), {nov} overlapping pairs ({ov_area:.1} nm2; largest {max_ov} cells)");
    println!("== PDF cross-check (pdftotext)");
    let (mut matched, mut mism, mut unver) = (0, 0, 0);
    for (c, st) in pdf::check_all(m, &a.dir).into_iter().enumerate() {
        match st {
            pdf::PdfStatus::Checked(r) => {
                matched += r.matched;
                mism += r.mismatched.len();
                println!(
                    "  {}: {} sectors, {} PDF labels, {} matched, {} mismatched, {} missing in PDF, {} extra in PDF",
                    r.chart, r.xml_sectors, r.pdf_labels, r.matched, r.mismatched.len(),
                    r.missing_in_pdf.len(), r.extra_in_pdf.len()
                );
                for (id, x, p) in &r.mismatched {
                    println!("    MISMATCH {id}: XML {x} ft, PDF {p} ft");
                }
                for id in &r.missing_in_pdf {
                    println!("    not on PDF: {id}");
                }
            }
            pdf::PdfStatus::Unavailable(why) => {
                unver += 1;
                println!("  {}: NOT VERIFIED ({why})", chart_label(m, c));
            }
        }
    }
    println!("  total: {matched} matched, {mism} mismatched, {unver} charts not verified");
    if let (Some(tdir), Some(dofzip)) = (&a.terrain, &a.dof) {
        println!(
            "== Clearance against terrain ({}) and obstacles ({})",
            tdir.display(),
            dofzip.display()
        );
        match run_clearance(m, tdir, dofzip, &a) {
            Ok(()) => {}
            Err(e) => println!("  not run: {e}"),
        }
    } else {
        println!("== Clearance: NOT RUN (no --terrain / --dof)");
    }
    if refused > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn run_clearance(
    m: &Model,
    tdir: &std::path::Path,
    dofzip: &std::path::Path,
    a: &Args,
) -> Result<(), String> {
    let bytes = oa_zip::read_archive_file(dofzip).map_err(|e| e.to_string())?;
    let arc = oa_zip::Archive::parse(&bytes).map_err(|e| e.to_string())?;
    let dat = arc.extract("DOF.DAT").map_err(|e| e.to_string())?;
    let dof = oa_dof::parse_dof(&dat).map_err(|e| e.to_string())?;
    println!(
        "  DOF: {} obstacles, currency {:?}",
        oa_dof::len(&dof),
        dof.currency
    );
    let sectors: Vec<SectorIx> = (0..chart_count(m))
        .filter(|&c| {
            a.charts
                .as_ref()
                .is_none_or(|o| o.iter().any(|f| chart_label(m, c).starts_with(f.as_str())))
        })
        .flat_map(|c| sector_range_of_chart(m, c))
        .map(|i| SectorIx(i as u32))
        .collect();
    let res = clearance::clearance(m, &sectors, tdir, &dof)?;
    for set in [SetKind::Fus3, SetKind::Fus5, SetKind::Tav] {
        let mut mg: Vec<f64> = res
            .iter()
            .filter(|c| c.set == set.name())
            .filter(|c| c.tiles_missing == 0)
            .filter_map(clearance::margin_ft)
            .collect();
        mg.sort_by(f64::total_cmp);
        if mg.is_empty() {
            continue;
        }
        let p = |q| clearance::percentile(&mg, q);
        let under = mg
            .iter()
            .filter(|&&x| x < clearance::REQUIRED_CLEARANCE_FT)
            .count();
        let under950 = mg
            .iter()
            .filter(|&&x| x < clearance::REQUIRED_CLEARANCE_FT - clearance::ROUNDING_TOLERANCE_FT)
            .count();
        println!(
            "  {} margin over highest terrain/obstacle, ft, {} sectors with full terrain: min {:.0}, p5 {:.0}, p25 {:.0}, median {:.0}, p75 {:.0}, p95 {:.0}, max {:.0}; under 1000 ft: {under}; under 950 ft: {under950}; negative: {}",
            set.name(), mg.len(), p(0.0), p(0.05), p(0.25), p(0.5), p(0.75), p(0.95), p(1.0),
            mg.iter().filter(|&&x| x < 0.0).count()
        );
    }
    let incomplete = res.iter().filter(|c| c.tiles_missing > 0).count();
    println!("  sectors with missing terrain tiles (margin not trusted): {incomplete}");
    let nodata: u64 = res.iter().map(|c| c.terrain_nodata).sum();
    println!("  terrain nodata samples inside sectors: {nodata}");
    let mut neg: Vec<&clearance::SectorClearance> = res
        .iter()
        .filter(|c| clearance::margin_ft(c).is_some_and(|x| x < 0.0))
        .collect();
    neg.sort_by(|x, y| {
        clearance::margin_ft(x)
            .unwrap_or(0.0)
            .total_cmp(&clearance::margin_ft(y).unwrap_or(0.0))
    });
    println!("  sectors with NEGATIVE margin: {}", neg.len());
    for c in neg {
        println!(
            "    {} {} {}: MVA {} ft, terrain max {} ft, obstacle max {} ft, margin {:.0} ft{}",
            c.facility,
            c.set,
            c.sector_id,
            c.min_alt_ft,
            c.terrain_max_ft
                .map_or("none".to_string(), |t| format!("{t:.0}")),
            c.obstacle_max_ft
                .map_or("none".to_string(), |t| t.to_string()),
            clearance::margin_ft(c).unwrap_or(0.0),
            if c.tiles_missing > 0 {
                " [tiles missing]"
            } else {
                ""
            }
        );
    }
    let tol = clearance::ROUNDING_TOLERANCE_FT;
    let mut thin: Vec<&clearance::SectorClearance> = res
        .iter()
        .filter(|c| {
            clearance::margin_ft(c)
                .is_some_and(|x| (0.0..clearance::REQUIRED_CLEARANCE_FT - tol).contains(&x))
        })
        .collect();
    thin.sort_by(|x, y| {
        clearance::margin_ft(x)
            .unwrap_or(0.0)
            .total_cmp(&clearance::margin_ft(y).unwrap_or(0.0))
    });
    let rounded = res
        .iter()
        .filter(|c| {
            clearance::margin_ft(c).is_some_and(|x| {
                (clearance::REQUIRED_CLEARANCE_FT - tol..clearance::REQUIRED_CLEARANCE_FT)
                    .contains(&x)
            })
        })
        .count();
    println!(
        "  sectors with margin 0 to {} ft (short of 1000 by more than rounding to the hundred below): {}",
        clearance::REQUIRED_CLEARANCE_FT - tol,
        thin.len()
    );
    println!(
        "  sectors with margin {} to 1000 ft (within that rounding): {rounded}",
        clearance::REQUIRED_CLEARANCE_FT - tol
    );
    for c in thin.iter().take(a.list.max(thin.len().min(60))) {
        println!(
            "    {} {} {}: MVA {} ft, margin {:.0} ft (terrain {}, obstacle {} {}){}",
            c.facility,
            c.set,
            c.sector_id,
            c.min_alt_ft,
            clearance::margin_ft(c).unwrap_or(0.0),
            c.terrain_max_ft
                .map_or("none".to_string(), |t| format!("{t:.0}")),
            c.obstacle_max_ft
                .map_or("none".to_string(), |t| t.to_string()),
            c.obstacle_peak
                .as_ref()
                .map_or(String::new(), |p| format!("{} {}", p.0, p.1.trim())),
            if c.tiles_missing > 0 {
                " [tiles missing]"
            } else {
                ""
            }
        );
    }
    Ok(())
}
