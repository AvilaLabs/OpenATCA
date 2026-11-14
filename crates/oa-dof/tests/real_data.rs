//! The real FAA DOF files: reads `OPENATCA_DATA` (the openatca-data directory) and skips when it
//! is unset or the zip is absent. Run with `--nocapture` for the report.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_dof::*;
use std::path::PathBuf;

fn zip_path() -> Option<PathBuf> {
    let d = match std::env::var_os("OPENATCA_DATA") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => {
            eprintln!("OPENATCA_DATA is not set; skipping real-data test");
            return None;
        }
    };
    let p = d.join("raw/dof/DOF_260927.zip");
    if p.is_file() {
        Some(p)
    } else {
        eprintln!("{} is absent; skipping real-data test", p.display());
        None
    }
}

// REQ: OA-DOF-008
#[test]
fn real_dof_parses_without_rejections() {
    let Some(p) = zip_path() else { return };
    let bytes = oa_zip::read_archive_file(&p).unwrap();
    let a = oa_zip::Archive::parse(&bytes).unwrap();
    let all = parse_dof(&a.extract("DOF.DAT").unwrap()).unwrap();
    eprintln!(
        "DOF.DAT: currency {:?}, {} lines, {} obstacles, {} rejected",
        all.currency,
        all.lines,
        len(&all),
        all.rejected_total
    );
    assert_eq!(
        all.currency,
        Currency {
            year: 2026,
            month: 9,
            day: 27
        }
    );
    assert_eq!(all.rejected_total, 0, "{:?}", all.rejected.first());
    assert_eq!(all.lines, all.lat_e7.len() as u64 + HEADER_LINES);
    // The per-state and per-country files nearly partition DOF.DAT: the FAA's own split leaves
    // out 38 obstacles (Midway and Netherlands Antilles numbers and a few state ones), which
    // was checked by OAS number against this file.
    let mut parts = 0usize;
    let mut sum = 0usize;
    for e in a.entries() {
        let n = e.name().to_ascii_lowercase();
        if n.ends_with(".dat") && n != "dof.dat" && n != "chg.dat" {
            let d = parse_dof(&a.extract(e.name()).unwrap()).unwrap();
            assert_eq!(d.rejected_total, 0, "{}", e.name());
            sum += len(&d);
            parts += 1;
        }
    }
    eprintln!(
        "{parts} part files hold {sum} obstacles; {} in DOF.DAT only",
        len(&all) - sum
    );
    assert!(
        sum <= len(&all) && len(&all) - sum <= 100,
        "{} obstacles are in no part file",
        len(&all) - sum
    );
    // Distribution of the decoded fields.
    let mut verified = 0;
    let mut no_acc = 0;
    let mut neg_amsl = 0;
    let mut tallest = 0;
    for i in 0..len(&all) {
        verified += usize::from(all.verification[i] == b'O');
        no_acc += usize::from(all.h_acc[i] == b' ');
        neg_amsl += usize::from(all.amsl_ft[i] < 0);
        tallest = tallest.max(all.agl_ft[i]);
    }
    eprintln!("verified {verified}, without accuracy codes {no_acc}, negative AMSL {neg_amsl}, tallest AGL {tallest} ft");
    // Chicago O'Hare area: a plausible number of obstacles inside a 0.2 degree box.
    let hits = in_bbox(&all, 41.88, 42.08, -87.99, -87.79);
    eprintln!("obstacles within 0.1 deg of ORD: {}", hits.len());
    assert!(!hits.is_empty());
    // Every position is inside the world and every height field decoded.
    assert!(all
        .lat_e7
        .iter()
        .all(|&v| (-900_000_000..=900_000_000).contains(&v)));
    assert!(all
        .lon_e7
        .iter()
        .all(|&v| (-1_800_000_000..=1_800_000_000).contains(&v)));
}
