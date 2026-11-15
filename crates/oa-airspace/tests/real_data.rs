//! Builds over the full FAA files. These read the directory named by the `OPENATCA_RAW`
//! environment variable (it holds `nasr-2026-10-29/`, `cifp-2026-10-29/` and the 2026-10-01
//! pair) and skip, with a message, when it is unset. Run with `--release --nocapture`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_airspace::build::{build_model, BuildOutput, BuildParams};
use oa_airspace::diff::{diff_counts, diff_models, render_diff, Change};
use oa_airspace::load::{load_files, parse_expectation};
use oa_airspace::model::*;
use oa_airspace::report::boundary_separations;
use oa_airspace::snapshot::{decode, encode};
use oa_sha256::{sha256, to_hex};
use std::path::PathBuf;

const NASR_1029: (&str, &str, &str) = (
    "nasr-2026-10-29",
    "NASR_CSV_29_Oct_2026.zip",
    "60b072454319f166797c5f4667520e4cf9c54d2e4e5faffbcdbae846aea276da",
);
const CIFP_1029: (&str, &str, &str) = (
    "cifp-2026-10-29",
    "CIFP_261029.zip",
    "e3ff2b31a4f6ad3eda1342b83ada1ae399aae7bd8c1e67f3cacd5b288b5e141d",
);
const NASR_1001: (&str, &str, &str) = (
    "nasr-2026-10-01",
    "NASR_CSV_01_Oct_2026.zip",
    "8e42286d8af8350097397e6337a1b0d922f22b0c0e331f6142e8e1210f6828f4",
);
const CIFP_1001: (&str, &str, &str) = (
    "cifp-2026-10-01",
    "CIFP_261001.zip",
    "6a84a58d92c598b8effe7daabb85443e90ac216ccc148890df8f13590e5c1b8b",
);

fn raw() -> Option<PathBuf> {
    match std::env::var_os("OPENATCA_RAW") {
        Some(d) if !d.is_empty() => Some(PathBuf::from(d)),
        _ => {
            eprintln!("OPENATCA_RAW is not set; skipping real-data test");
            None
        }
    }
}

fn build(
    root: &std::path::Path,
    nasr: (&str, &str, &str),
    cifp: (&str, &str, &str),
) -> BuildOutput {
    let expect = [
        parse_expectation(&format!("{}={}", nasr.1, nasr.2)).expect("expectation"),
        parse_expectation(&format!("{}={}", cifp.1, cifp.2)).expect("expectation"),
    ];
    let src = load_files(
        &root.join(nasr.0).join(nasr.1),
        &root.join(cifp.0).join(cifp.1),
        &expect,
    )
    .expect("load");
    build_model(
        &src,
        &BuildParams {
            artcc: "ZAU".into(),
            buffer_m: 92_600,
            allow_cycle_mismatch: false,
        },
    )
    .expect("build")
}

fn s(m: &Model, ix: StrIx) -> &str {
    str_or_empty(&m.strings, ix)
}

fn find(m: &Model, ident: &str, kind: u8) -> Option<PointIx> {
    ["K5", "K3", "K4", "K6", "K7", "K2"].iter().find_map(|r| {
        find_point(
            m,
            PointKey {
                ident,
                region: r,
                scope: "",
                class: point_kind::class(kind),
            },
        )
    })
}

// REQ: OA-ASP-001, OA-ASP-002, OA-ASP-006
#[test]
fn zau_builds_from_the_2026_10_29_cycle() {
    let Some(root) = raw() else { return };
    let start = std::time::Instant::now();
    let out = build(&root, NASR_1029, CIFP_1029);
    let m = &out.model;
    eprintln!("build + load: {:.1} s", start.elapsed().as_secs_f64());
    validate_model(m).expect("valid");
    assert_eq!(
        (m.meta.nasr_effective, m.meta.cifp_cycle),
        (20_261_029, 2611)
    );
    // The airports the task names are inside.
    for icao in ["KORD", "KMDW", "KMKE", "KRFD"] {
        let ix = find(m, icao, point_kind::AIRPORT).unwrap_or_else(|| panic!("{icao} missing"));
        assert!(
            bit_get(&m.points.in_area, ix.row()),
            "{icao} must be inside ZAU"
        );
    }
    // ORD class B is present, from CIFP.
    let b = &m.boundaries;
    let class_b = (0..b.source.len())
        .filter(|&i| b.stratum[i] == stratum::CLASS_B && s(m, b.facility[i]) == "KORD")
        .count();
    assert!(class_b >= 1, "ORD class B");
    assert!((0..b.source.len())
        .all(|i| b.source[i] != boundary_source::CIFP_UC
            || b.edge_interp[i] == edge_interp::PER_SEGMENT));
    // At least one KORD STAR is coded.
    let ord = find(m, "KORD", point_kind::AIRPORT).expect("KORD");
    let p = &m.procs;
    let coded_stars = (0..p.kind.len())
        .filter(|&i| p.airport[i] == ord && p.kind[i] == proc_kind::STAR && bit_get(&p.coded, i))
        .count();
    assert!(coded_stars >= 1, "a coded KORD STAR");
    // Radar-vector SIDs enter as uncoded procedures.
    let uncoded = (0..p.kind.len())
        .filter(|&i| p.kind[i] == proc_kind::SID && !bit_get(&p.coded, i))
        .count();
    assert!(uncoded >= 1);
    // DD-001 (the Thedford, Nebraska VOR/DME whose NASR decimal position is its DME antenna, 105 m
    // off its own DMS) is outside ZAU, and the build takes NASR positions from the DMS fields, so
    // it neither appears in the model nor as a disagreement or position defect.
    assert!(
        find(m, "TDD", point_kind::VOR_DME).is_none(),
        "TDD is a Nebraska navaid, far outside ZAU and its buffer"
    );
    assert!(out.info.position_defects.iter().all(|d| d.ident != "TDD"));
    assert!(out
        .info
        .pairs_over
        .iter()
        .all(|(_, label)| !label.contains("TDD")));
    // The airway border pseudo-points are their own category. What is left unresolved are two
    // defects in the FAA data: ELANR9 at KDPA gives the airport KDPA the section code of an
    // enroute waypoint (E/A; the airport is coded P/A), and R36 at KDAF
    // names the NDB CMY, which neither CIFP (no D/DB record) nor NASR (no NAV_BASE row) holds.
    let border = m
        .unresolved
        .role
        .iter()
        .filter(|&&r| r == unresolved_role::BORDER_CROSSING)
        .count();
    assert_eq!(border, 36);
    assert_eq!(unresolved_error_count(m), 2);
    // Counts for the report.
    eprintln!(
        "points {} vertices {} boundaries {} segments {} airways {} airway legs {} procs {} routes {} legs {} mora {} unresolved {} disagreements {}",
        m.points.kind.len(), m.vertices.len(), m.boundaries.source.len(), m.segments.via.len(), m.airways.designator.len(),
        m.airway_legs.from.len(), m.procs.kind.len(), m.routes.legs.len(), m.legs.fix.len(), m.mora.lat.len(),
        m.unresolved.role.len(), m.disagreements.point.len()
    );
    eprintln!(
        "whole-cycle pairs {} over 1 m {} max {:.3} m",
        out.info.pairs_compared, out.info.pairs_over_limit, out.info.pairs_max_m
    );
    eprintln!(
        "rounding: position {:.6} m, distance {:.3} m",
        out.info.rounding.pos_max_m, out.info.rounding.dist_max_m
    );
    for sep in boundary_separations(m)
        .iter()
        .filter(|x| x.facility == "ZAU")
    {
        let r = sep.result.as_ref().expect("computed");
        eprintln!(
            "ZAU {} separation {:.1} m on edge {} ({:?})",
            stratum::name(sep.stratum),
            r.max_m,
            r.edge,
            sep.edge_ends
        );
    }
    // Determinism: the same Sources build to the same bytes, and the file reads back.
    let a = encode(m).expect("encode");
    let again = {
        let expect = [
            parse_expectation(&format!("{}={}", NASR_1029.1, NASR_1029.2)).expect("expectation"),
            parse_expectation(&format!("{}={}", CIFP_1029.1, CIFP_1029.2)).expect("expectation"),
        ];
        let src = load_files(
            &root.join(NASR_1029.0).join(NASR_1029.1),
            &root.join(CIFP_1029.0).join(CIFP_1029.1),
            &expect,
        )
        .expect("load");
        encode(
            &build_model(
                &src,
                &BuildParams {
                    artcc: "ZAU".into(),
                    buffer_m: 92_600,
                    allow_cycle_mismatch: false,
                },
            )
            .expect("build")
            .model,
        )
        .expect("encode")
    };
    assert_eq!(a, again);
    assert_eq!(decode(&a).expect("decode"), *m);
    eprintln!("snapshot {} bytes sha256 {}", a.len(), to_hex(&sha256(&a)));
}

// REQ: OA-ASP-003
#[test]
fn no_cross_source_pair_in_zau_is_more_than_a_metre_apart() {
    let Some(root) = raw() else { return };
    let out = build(&root, NASR_1029, CIFP_1029);
    // Whole-cycle statistics are informational; the ZAU model itself lists every pair over 1 m.
    let m = &out.model;
    eprintln!(
        "disagreements in ZAU: {}; whole cycle: {} of {} pairs",
        m.disagreements.point.len(),
        out.info.pairs_over_limit,
        out.info.pairs_compared
    );
    assert_eq!(
        m.disagreements.point.len(),
        0,
        "NASR and CIFP agree to a metre for every ZAU object"
    );
    assert!(out.info.pairs_compared > 50_000);
    for (d, label) in out.info.pairs_over.iter().take(10) {
        eprintln!("  whole-cycle pair over 1 m: {label} {d:.3} m");
    }
}

// REQ: OA-ASP-005
#[test]
fn zau_high_and_low_separations_are_reported() {
    let Some(root) = raw() else { return };
    let out = build(&root, NASR_1029, CIFP_1029);
    let zau: Vec<_> = boundary_separations(&out.model)
        .into_iter()
        .filter(|x| x.facility == "ZAU")
        .collect();
    assert_eq!(zau.len(), 2);
    for sep in zau {
        let r = sep.result.expect("computed");
        assert!(r.max_m > 100.0 && r.max_m < 10_000.0, "{}", r.max_m);
        assert!(sep.edge_ends.is_some());
    }
}

// REQ: OA-ASP-007
#[test]
fn the_october_diff_is_non_empty_and_plausible() {
    let Some(root) = raw() else { return };
    let old = build(&root, NASR_1001, CIFP_1001).model;
    let new = build(&root, NASR_1029, CIFP_1029).model;
    assert_eq!(
        (old.meta.nasr_effective, old.meta.cifp_cycle),
        (20_261_001, 2610)
    );
    let d = diff_models(&old, &new).expect("diff");
    assert!(!d.entries.is_empty());
    // A month of FAA changes in one ARTCC is dozens to a few hundred objects, not thousands.
    assert!(d.entries.len() < 2_000, "{}", d.entries.len());
    let counts = diff_counts(&d);
    eprintln!(
        "diff: {} entries; per class (added, removed, changed): {counts:?}",
        d.entries.len()
    );
    for e in d.entries.iter().take(10) {
        let what = match &e.change {
            Change::Added => "added".to_string(),
            Change::Removed => "removed".to_string(),
            Change::Changed(f) => format!(
                "changed {}",
                f.iter()
                    .map(|x| x.field.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        };
        eprintln!("  {} {}: {what}", e.class, e.key);
    }
    assert_eq!(
        d.header.iter().filter(|h| h.field == "CIFP cycle").count(),
        1
    );
    // The same pair of models diffs to the same text.
    assert_eq!(
        render_diff(&d, "a", "b"),
        render_diff(&diff_models(&old, &new).expect("diff"), "a", "b")
    );
    // Procedures revised between cycles show up as a removal and an addition (BLOKR8 to BLOKR9).
    assert!(d
        .entries
        .iter()
        .any(|e| e.class == "procedure" && e.key.contains("BLOKR")));
}
