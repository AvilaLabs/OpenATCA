//! The model built from the committed fixtures: a cut of the 2026-10-29 NASR and CIFP data (the
//! ZAU boundary, KORD, KMDW and KJFK with a few procedures, two airways, and what they reference).
//! See `fixtures/PROVENANCE.txt`.

mod common;

use common::*;
use oa_airspace::build::{build_model, BuildError};
use oa_airspace::load::{load_bytes, parse_expectation, Expectation, LoadError};
use oa_airspace::model::*;
use oa_airspace::report::{boundary_separations, render_report, table_sizes, ReportInput};
use oa_airspace::snapshot::{decode, encode};
use oa_sha256::{sha256, to_hex};

fn key<'a>(ident: &'a str, region: &'a str, scope: &'a str, kind: u8) -> PointKey<'a> {
    PointKey {
        ident,
        region,
        scope,
        class: point_kind::class(kind),
    }
}

fn s(m: &Model, ix: StrIx) -> &str {
    str_or_empty(&m.strings, ix)
}

fn airport(m: &Model, ident: &str) -> Option<PointIx> {
    find_point(m, key(ident, "K5", "", point_kind::AIRPORT))
        .or_else(|| find_point(m, key(ident, "K6", "", point_kind::AIRPORT)))
}

fn boundaries_of(m: &Model, source: u8, stratum: u8, facility: &str) -> usize {
    let b = &m.boundaries;
    (0..b.source.len())
        .filter(|&i| {
            b.source[i] == source && b.stratum[i] == stratum && s(m, b.facility[i]) == facility
        })
        .count()
}

fn procs_at(m: &Model, ap: PointIx) -> Vec<(u8, String)> {
    let p = &m.procs;
    (0..p.kind.len())
        .filter(|&i| p.airport[i] == ap)
        .map(|i| (p.kind[i], s(m, p.ident[i]).to_string()))
        .collect()
}

// REQ: OA-ASP-001
#[test]
fn the_fixture_builds_a_valid_model_for_zau() {
    let out = fixture_build();
    let m = &out.model;
    validate_model(m).expect("valid model");
    assert_eq!(m.meta.artcc, "ZAU");
    assert_eq!(m.meta.nasr_effective, 20_261_029);
    assert_eq!(m.meta.cifp_cycle, 2611);
    assert_eq!(m.meta.buffer_m, 92_600);
    let nasr = nasr_zip(&nasr_files());
    let cifp = cifp_zip(&cifp_file());
    assert_eq!(m.meta.nasr_sha256, sha256(&nasr));
    assert_eq!(m.meta.cifp_sha256, sha256(&cifp));
    assert!(m.points.kind.len() > 300);
    for t in table_sizes(m) {
        if t.name != "disagreements" {
            assert!(t.rows > 0, "{} is empty", t.name);
        }
    }
    // Strings are interned once: no string appears twice.
    let n = str_count(&m.strings);
    let mut all: Vec<&str> = (0..n as u32)
        .filter_map(|i| str_at(&m.strings, StrIx(i)))
        .collect();
    let before = all.len();
    all.sort_unstable();
    all.dedup();
    assert_eq!(all.len(), before, "the string table holds duplicates");
}

// REQ: OA-ASP-001
#[test]
fn a_point_is_identified_by_ident_region_and_kind_class() {
    let m = fixture_build().model;
    let ord = airport(&m, "KORD").expect("KORD");
    let k = point_key(&m, ord).expect("key");
    assert_eq!((k.ident, k.region), ("KORD", "K5"));
    // The same ident under another kind class or another region is another point (or none).
    assert_eq!(find_point(&m, key("KORD", "K5", "", point_kind::FIX)), None);
    assert_eq!(
        find_point(&m, key("KORD", "K9", "", point_kind::AIRPORT)),
        None
    );
    // A fix found by the NASR kind (FIX) is found by the CIFP kind (terminal waypoint) too: one class.
    assert_eq!(
        point_kind::class(point_kind::FIX),
        point_kind::class(point_kind::TERMINAL_WAYPOINT)
    );
    let mosco = find_point(&m, key("MOSCO", "K3", "", point_kind::FIX)).expect("MOSCO");
    assert_eq!(m.points.kind[mosco.0 as usize], point_kind::FIX);
    // Runway thresholds are scoped by their airport.
    let thr = find_point(&m, key("09L", "K5", "KORD", point_kind::THRESHOLD)).expect("KORD 09L");
    assert_eq!(m.points.kind[thr.0 as usize], point_kind::THRESHOLD);
    assert!(
        bit_get(&m.points.src_nasr, thr.0 as usize) && bit_get(&m.points.src_cifp, thr.0 as usize)
    );
}

// REQ: OA-ASP-001
#[test]
fn objects_in_both_sources_are_matched_and_flagged() {
    let m = fixture_build().model;
    let p = &m.points;
    let mut both = [0usize; 12];
    let mut one = [0usize; 12];
    for i in 0..p.kind.len() {
        let k = usize::from(point_kind::class(p.kind[i]));
        if bit_get(&p.src_nasr, i) && bit_get(&p.src_cifp, i) {
            both[k] += 1;
        } else {
            one[k] += 1;
        }
        assert!(
            bit_get(&p.src_nasr, i) || bit_get(&p.src_cifp, i),
            "row {i} has no source"
        );
    }
    // Fixes, navaids, airports and runway ends are matched across the sources.
    assert!(
        both[usize::from(point_kind::FIX)] > 20,
        "fixes and terminal waypoints"
    );
    assert!(both[usize::from(point_kind::VOR)] > 0, "navaids");
    assert!(both[usize::from(point_kind::AIRPORT)] >= 2, "airports");
    assert!(both[usize::from(point_kind::THRESHOLD)] > 10, "runway ends");
    // No two rows share an identity key (validate_model checks strict order of the keys).
    validate_model(&m).expect("valid");
    // CIFP-only localizers exist and are scoped to their airport.
    assert!(one[usize::from(point_kind::LOCALIZER)] > 0);
}

// REQ: OA-ASP-002
#[test]
fn objects_inside_the_artcc_are_selected_and_distant_ones_are_not() {
    let m = fixture_build().model;
    let ord = airport(&m, "KORD").expect("KORD");
    let mdw = airport(&m, "KMDW").expect("KMDW");
    assert!(bit_get(&m.points.in_area, ord.0 as usize));
    assert!(bit_get(&m.points.in_area, mdw.0 as usize));
    // KJFK is in the data but 1,000 km from ZAU: not selected, and none of its procedures.
    if let Some(jfk) = airport(&m, "KJFK") {
        assert!(!bit_get(&m.points.in_area, jfk.0 as usize));
        assert!(
            procs_at(&m, jfk).is_empty(),
            "no KJFK procedure may be in the ZAU model"
        );
    }
    assert!((0..m.procs.kind.len()).all(|i| s(&m, m.procs.ident[i]) != "CAMRN5"));
    // KJFK class B and the Alaska MOA in the fixture are outside: not selected.
    assert_eq!(
        boundaries_of(&m, boundary_source::CIFP_UC, stratum::CLASS_B, "KJFK"),
        0
    );
    assert_eq!(
        boundaries_of(&m, boundary_source::CIFP_UR, stratum::SUA_MOA, "BIG BEAR"),
        0
    );
}

// REQ: OA-ASP-002
#[test]
fn every_procedure_of_every_airport_inside_is_selected_with_its_points() {
    let m = fixture_build().model;
    let ord = airport(&m, "KORD").expect("KORD");
    let mdw = airport(&m, "KMDW").expect("KMDW");
    let ord_procs = procs_at(&m, ord);
    for want in [
        (proc_kind::STAR, "BENKY6"),
        (proc_kind::APPROACH, "I09L"),
        (proc_kind::SID, "O'HARE NINE"),
    ] {
        assert!(
            ord_procs.iter().any(|(k, i)| *k == want.0 && i == want.1),
            "KORD {want:?} in {ord_procs:?}"
        );
    }
    let mdw_procs = procs_at(&m, mdw);
    assert!(mdw_procs
        .iter()
        .any(|(k, i)| *k == proc_kind::STAR && i == "ENDEE8"));
    assert!(mdw_procs
        .iter()
        .any(|(k, i)| *k == proc_kind::APPROACH && i == "I04R"));
    // Every point a selected procedure names is a model row, in or out of the area, never a dangling index.
    let l = &m.legs;
    for i in 0..l.fix.len() {
        for ix in [l.fix[i], l.center[i]] {
            assert!(ix == PointIx::NONE || point_key(&m, ix).is_some());
        }
    }
    // BENKY6 is coded: it has legs with path terminators and fixes.
    let st = find_proc(&m, "KORD", proc_kind::STAR, "BENKY6").expect("BENKY6");
    assert!(bit_get(&m.procs.coded, st.0 as usize));
    let routes = m.procs.routes[st.0 as usize];
    assert!(routes.len > 0);
    let mut legs = 0;
    for r in routes.start.0..routes.start.0 + routes.len {
        let ls = m.routes.legs[r as usize];
        for i in ls.start.0..ls.start.0 + ls.len {
            assert_ne!(m.legs.path_term[i as usize], path_term::NONE);
            legs += 1;
        }
    }
    assert!(legs >= 10, "{legs}");
}

// REQ: OA-ASP-002
#[test]
fn class_b_volumes_and_mora_cells_that_touch_the_area_are_selected_with_their_source() {
    let m = fixture_build().model;
    assert!(
        boundaries_of(&m, boundary_source::CIFP_UC, stratum::CLASS_B, "KORD") >= 1,
        "ORD class B"
    );
    assert_eq!(
        boundaries_of(&m, boundary_source::NASR_ARB, stratum::ARTCC_HIGH, "ZAU"),
        1
    );
    assert_eq!(
        boundaries_of(&m, boundary_source::NASR_ARB, stratum::ARTCC_LOW, "ZAU"),
        1
    );
    // Each boundary keeps its source: NASR ones have vertices, CIFP ones have segments.
    let b = &m.boundaries;
    for i in 0..b.source.len() {
        match b.source[i] {
            boundary_source::NASR_ARB => {
                assert!(b.vertices[i].len >= 3 && b.segs[i].len == 0);
                assert_eq!(b.edge_interp[i], edge_interp::UNKNOWN);
            }
            _ => {
                assert!(b.segs[i].len > 0 && b.vertices[i].len == 0);
                assert_eq!(b.edge_interp[i], edge_interp::PER_SEGMENT);
            }
        }
    }
    // MORA cells are whole-degree cells that overlap the ZAU area plus its buffer.
    assert!(!m.mora.lat.is_empty());
    for i in 0..m.mora.lat.len() {
        assert!(
            (38..=46).contains(&m.mora.lat[i]) && (-95..=-80).contains(&m.mora.lon[i]),
            "cell {} {}",
            m.mora.lat[i],
            m.mora.lon[i]
        );
    }
}

// REQ: OA-ASP-002
#[test]
fn the_buffer_adds_objects_around_the_artcc_and_zero_buffer_selects_fewer() {
    let src = fixture_sources();
    let wide = build_model(&src, &params("ZAU", 92_600))
        .expect("wide")
        .model;
    let tight = build_model(&src, &params("ZAU", 0)).expect("tight").model;
    let narrow = build_model(&src, &params("ZAU", 9_260))
        .expect("narrow")
        .model;
    let n = |m: &Model| bit_count(&m.points.in_area);
    assert!(
        n(&tight) <= n(&narrow) && n(&narrow) <= n(&wide),
        "{} {} {}",
        n(&tight),
        n(&narrow),
        n(&wide)
    );
    assert!(
        n(&tight) < n(&wide),
        "the buffer must matter for this data: {} {}",
        n(&tight),
        n(&wide)
    );
    // The buffer is recorded and every snapshot of a different buffer differs.
    assert_eq!(tight.meta.buffer_m, 0);
    assert_ne!(
        encode(&tight).expect("encode"),
        encode(&wide).expect("encode")
    );
    // Airports and the procedures that follow them are the same: KORD and KMDW are well inside.
    for m in [&tight, &wide] {
        assert!(airport(m, "KORD").is_some() && airport(m, "KMDW").is_some());
    }
}

// REQ: OA-ASP-001
#[test]
fn an_unknown_artcc_and_a_bad_buffer_are_refused() {
    let src = fixture_sources();
    assert!(matches!(
        build_model(&src, &params("ZZZ", 92_600)),
        Err(BuildError::UnknownArtcc(_))
    ));
    assert!(matches!(
        build_model(&src, &params("", 92_600)),
        Err(BuildError::BadParams(_))
    ));
    assert!(matches!(
        build_model(&src, &params("ZAU;", 92_600)),
        Err(BuildError::BadParams(_))
    ));
    assert!(matches!(
        build_model(&src, &params(&"Z".repeat(100), 92_600)),
        Err(BuildError::BadParams(_))
    ));
    assert!(matches!(
        build_model(&src, &params("ZAU", u32::MAX)),
        Err(BuildError::BadParams(_))
    ));
}

// REQ: OA-ASP-003
#[test]
fn matching_sources_report_no_disagreement() {
    let out = fixture_build();
    assert!(out.model.disagreements.point.is_empty());
    assert!(out.info.pairs_compared > 100);
    assert_eq!(out.info.pairs_over_limit, 0, "{:?}", out.info.pairs_over);
}

// REQ: OA-ASP-003
#[test]
fn an_injected_105_metre_offset_is_caught_with_its_distance() {
    // One fix is moved 105 m north in NASR only (the DD-001 pattern: one source puts an object
    // in the wrong place). The CIFP position is taken and the pair is listed with its distance.
    let clean = fixture_build().model;
    let target = (0..clean.points.kind.len())
        .find(|&i| {
            point_kind::class(clean.points.kind[i]) == point_kind::FIX
                && bit_get(&clean.points.src_nasr, i)
                && bit_get(&clean.points.src_cifp, i)
                && bit_get(&clean.points.in_area, i)
        })
        .expect("a fix in both sources");
    let k = point_key(&clean, PointIx(target as u32)).expect("key");
    let (id, region) = (k.ident.to_string(), k.region.to_string());
    let cifp_lat = clean.points.lat[target];
    let cifp_lon = clean.points.lon[target];

    let mut files = nasr_files();
    shift_lat(
        &mut files,
        "FIX_BASE.csv",
        &format!("\"2026/10/29\",\"{id}\",\"{region}\""),
        5,
        105.0,
    );
    let src = sources_from(&files, &cifp_file());
    let out = build_model(&src, &params("ZAU", 92_600)).expect("build");
    let m = &out.model;
    assert_eq!(m.disagreements.point.len(), 1, "exactly the moved fix");
    let d = &m.disagreements;
    let row = d.point[0];
    assert_eq!(point_key(m, row).expect("key").ident, id);
    let dist = f64::from(d.dist_mm[0]) / 1000.0;
    assert!((dist - 105.0).abs() < 0.5, "distance {dist} m");
    // The CIFP position is the one taken; the NASR position is kept in the table.
    assert_eq!(m.points.lat[row.0 as usize], cifp_lat);
    assert_eq!(m.points.lon[row.0 as usize], cifp_lon);
    assert!(
        d.nasr_lat[0] > cifp_lat,
        "the NASR position is north of the CIFP one"
    );
    assert_eq!(out.info.pairs_over_limit, 1);
    assert_eq!(out.info.pairs_over.len(), 1);
    // The report lists it with the distance.
    let report = report_for(&out, &src);
    assert!(report.contains(&id) && report.contains("105."), "{report}");
    // The row was moved consistently (decimal and DMS), so it is not also a position defect.
    assert!(out.info.position_defects.is_empty());
}

// REQ: OA-ASP-003
#[test]
fn a_decimal_position_that_disagrees_with_its_own_dms_is_reported_as_a_defect() {
    let mut files = nasr_files();
    // MOSCO's DMS says 41 35 33.39 N (41.5926083); the decimal field is moved by 0.01 degree.
    edit_nasr(&mut files, "FIX_BASE.csv", "41.59260833", "41.60260833");
    let src = sources_from(&files, &cifp_file());
    let out = build_model(&src, &params("ZAU", 92_600)).expect("build");
    let d: Vec<_> = out
        .info
        .position_defects
        .iter()
        .filter(|x| x.ident == "MOSCO")
        .collect();
    assert_eq!(d.len(), 1, "{:?}", out.info.position_defects);
    assert!((d[0].diff_deg - 0.01).abs() < 1e-6, "{}", d[0].diff_deg);
    assert!(report_for(&out, &src).contains("MOSCO"));
}

// REQ: OA-ASP-003
#[test]
fn offsets_under_a_metre_are_not_disagreements_and_larger_ones_sort_by_distance() {
    let clean = fixture_build().model;
    let fixes: Vec<(String, String)> = (0..clean.points.kind.len())
        .filter(|&i| {
            point_kind::class(clean.points.kind[i]) == point_kind::FIX
                && bit_get(&clean.points.src_nasr, i)
                && bit_get(&clean.points.src_cifp, i)
                && bit_get(&clean.points.in_area, i)
        })
        .take(3)
        .map(|i| {
            let k = point_key(&clean, PointIx(i as u32)).expect("key");
            (k.ident.to_string(), k.region.to_string())
        })
        .collect();
    let mut files = nasr_files();
    for ((id, region), metres) in fixes.iter().zip([0.4, 30.0, 250.0]) {
        shift_lat(
            &mut files,
            "FIX_BASE.csv",
            &format!("\"2026/10/29\",\"{id}\",\"{region}\""),
            5,
            metres,
        );
    }
    let src = sources_from(&files, &cifp_file());
    let out = build_model(&src, &params("ZAU", 92_600)).expect("build");
    let d = &out.model.disagreements;
    assert_eq!(d.point.len(), 2, "0.4 m is under the 1 m threshold");
    let dists: Vec<f64> = d.dist_mm.iter().map(|&x| f64::from(x) / 1000.0).collect();
    assert!(
        dists.iter().any(|x| (x - 30.0).abs() < 0.5)
            && dists.iter().any(|x| (x - 250.0).abs() < 1.0),
        "{dists:?}"
    );
    // The report sorts by distance, largest first.
    let report = report_for(&out, &src);
    let at = |needle: &str| {
        report
            .find(needle)
            .unwrap_or_else(|| panic!("{needle} missing from the report"))
    };
    let big = at("250.");
    let small = at("30.0");
    assert!(big < small, "largest first");
}

// REQ: OA-ASP-003
#[test]
fn a_moved_airport_and_a_moved_runway_end_are_caught_too() {
    let mut files = nasr_files();
    shift_lat(
        &mut files,
        "APT_BASE.csv",
        "\"2026/10/29\",\"04508.\"",
        15,
        500.0,
    );
    let src = sources_from(&files, &cifp_file());
    let out = build_model(&src, &params("ZAU", 92_600)).expect("build");
    let m = &out.model;
    assert_eq!(m.disagreements.point.len(), 1);
    let k = point_key(m, m.disagreements.point[0]).expect("key");
    assert_eq!(k.ident, "KORD");
    assert!((f64::from(m.disagreements.dist_mm[0]) / 1000.0 - 500.0).abs() < 1.0);
}

// REQ: OA-ASP-004
#[test]
fn a_reference_to_a_point_neither_source_has_is_listed() {
    // In the unmodified fixture the navaid CGT that approach I04R names is such a reference: the
    // fixture cut holds neither a CIFP nor a NASR record for it.
    let m = fixture_build().model;
    let u = &m.unresolved;
    assert!(!u.role.is_empty());
    let found = (0..u.role.len()).any(|i| s(&m, u.owner[i]) == "I04R" && s(&m, u.name[i]) == "CGT");
    assert!(found, "I04R CGT must be listed");
    for i in 0..u.role.len() {
        assert!(!s(&m, u.name[i]).is_empty());
        assert!(u.owner_kind[i] == owner_kind::AIRWAY || u.owner_kind[i] == owner_kind::PROCEDURE);
    }
}

/// The fixture CIFP text with the header's record count following `edit`.
fn cifp_edited(edit: impl FnOnce(String) -> String) -> Vec<u8> {
    let text = edit(String::from_utf8(cifp_file()).expect("latin-1 fixture is ascii"));
    let n = text.lines().filter(|l| !l.starts_with("HDR")).count();
    let h = text.lines().next().expect("header").to_string();
    let patched = format!("{}{:07}{}", &h[..28], n, &h[35..]);
    text.replacen(&h, &patched, 1).into_bytes()
}

/// Rewrites the KMDW I04R leg that names CITGO to name the terminal NDB `IK` instead.
fn name_terminal_ndb(text: String) -> String {
    let from = "SUSAP KMDWK5FI04R  I      020CITGOK5PC0E";
    assert_eq!(text.matches(from).count(), 1);
    text.replacen(from, "SUSAP KMDWK5FI04R  I      020IK   K5PN0E", 1)
}

/// A terminal NDB (PN) record: the real `IK` record of KIKK moved to KMDW.
const MDW_TERMINAL_NDB: &str = "SUSAPNKMDWK5 IK    K5002720HO W N41010417W087541357                       W0040           NARLUKOW                         581802412\n";

// REQ: OA-ASP-004, OA-424-002
#[test]
fn a_leg_naming_a_terminal_ndb_resolves_to_it_by_airport() {
    // Without the PN record the leg's fix is unresolved and says it wanted a terminal NDB.
    let without = cifp_edited(name_terminal_ndb);
    let src = sources_from(&nasr_files(), &without);
    let m = build_model(&src, &params("ZAU", 92_600))
        .expect("build")
        .model;
    let u = &m.unresolved;
    let rows: Vec<usize> = (0..u.role.len())
        .filter(|&i| s(&m, u.name[i]) == "IK")
        .collect();
    assert_eq!(rows.len(), 1);
    assert_eq!(s(&m, u.wanted[rows[0]]), "P/N");
    // With it, the leg resolves and the NDB is a point scoped to its airport.
    let with = cifp_edited(|t| name_terminal_ndb(t) + MDW_TERMINAL_NDB);
    let src = sources_from(&nasr_files(), &with);
    let m2 = build_model(&src, &params("ZAU", 92_600))
        .expect("build")
        .model;
    validate_model(&m2).expect("valid");
    let u2 = &m2.unresolved;
    assert!((0..u2.role.len()).all(|i| s(&m2, u2.name[i]) != "IK"));
    assert_eq!(unresolved_error_count(&m2) + 1, unresolved_error_count(&m));
    let ik = find_point(&m2, key("IK", "K5", "KMDW", point_kind::NDB)).expect("IK at KMDW");
    // 41 01 04.17 N, 087 54 13.57 W in 1e-7 degrees.
    let (lat, lon) = (m2.points.lat[ik.0 as usize], m2.points.lon[ik.0 as usize]);
    assert!((lat - 410_178_250).abs() < 100 && (lon + 879_037_700).abs() < 100);
}

// REQ: OA-ASP-004
#[test]
fn airway_border_crossings_are_their_own_category_not_unresolved_errors() {
    let base = fixture_build().model;
    let mut files = nasr_files();
    edit_nasr(
        &mut files,
        "AWY_SEG_ALT.csv",
        "\"V156\",20,\"REZIE\",\"RP   \"",
        "\"V156\",20,\"U.S. CANADIAN BORDER-1\",\"\"",
    );
    let src = sources_from(&files, &cifp_file());
    let m = build_model(&src, &params("ZAU", 92_600))
        .expect("build")
        .model;
    validate_model(&m).expect("valid");
    let u = &m.unresolved;
    let rows: Vec<usize> = (0..u.role.len())
        .filter(|&i| u.role[i] == unresolved_role::BORDER_CROSSING)
        .collect();
    assert_eq!(rows.len(), 1);
    let i = rows[0];
    assert_eq!(s(&m, u.name[i]), "U.S. CANADIAN BORDER-1");
    assert_eq!(s(&m, u.owner[i]), "V156");
    assert_eq!(u.owner_kind[i], owner_kind::AIRWAY);
    assert_eq!(u.ordinal[i], 1);
    assert_eq!(unresolved_role::name(u.role[i]), "airway border crossing");
    // It does not count as an unresolved error, and the report lists it in its own section.
    assert_eq!(unresolved_error_count(&m), unresolved_error_count(&base));
    assert!(is_border_crossing_name("U.S. MEXICAN BORDER-3"));
    assert!(!is_border_crossing_name("BORDER"));
    let out = build_model(&src, &params("ZAU", 92_600)).expect("build");
    let report = report_for(&out, &src);
    assert!(report.contains("## Airway border crossings"));
}

// REQ: OA-ASP-004
#[test]
fn deleting_an_airway_fix_lists_the_leg_that_names_it() {
    let mut files = nasr_files();
    drop_row(&mut files, "FIX_BASE.csv", "\"2026/10/29\",\"REZIE\"");
    let src = sources_from(&files, &cifp_file());
    let out = build_model(&src, &params("ZAU", 92_600)).expect("build");
    let m = &out.model;
    let u = &m.unresolved;
    let rows: Vec<usize> = (0..u.role.len())
        .filter(|&i| s(m, u.name[i]) == "REZIE")
        .collect();
    // V156 is CID REZIE MOSCO MZV. Each leg's end is the next leg's start in the source, so the
    // reference is listed once, at the leg that starts there (leg 1).
    assert_eq!(rows.len(), 1, "{rows:?}");
    let i = rows[0];
    assert_eq!(u.role[i], unresolved_role::AIRWAY_FROM);
    assert_eq!(u.ordinal[i], 1);
    assert_eq!(s(m, u.owner[i]), "V156");
    assert_eq!(u.owner_kind[i], owner_kind::AIRWAY);
    // The airway keeps its other legs; the model stays valid.
    validate_model(m).expect("valid");
    let aw = find_airway(m, "V156", b'C').expect("V156");
    assert!(m.airways.legs[aw.0 as usize].len >= 1);
}

// REQ: OA-ASP-004
#[test]
fn an_unresolved_procedure_fix_is_listed_with_what_the_source_said_it_was() {
    // Remove a terminal waypoint's CIFP record: the leg that names it becomes unresolved.
    let cifp = String::from_utf8(cifp_file()).expect("latin-1 fixture is ascii");
    let line = cifp
        .lines()
        .find(|l| l.starts_with("SUSAP KORDK5C") && l.len() > 20)
        .expect("a KORD terminal waypoint")
        .to_string();
    let ident = line[13..18].trim().to_string();
    let used = cifp
        .lines()
        .any(|l| l.starts_with("SUSAP KORDK5F") && l[29..34].trim() == ident);
    if !used {
        eprintln!(
            "{ident} is not named by a kept KORD approach leg; only the airway case is exercised"
        );
        return;
    }
    let kept: String = cifp
        .lines()
        .filter(|l| *l != line)
        .map(|l| format!("{l}\n"))
        .collect();
    // The header's record count must follow.
    let mut text = kept;
    let n = text.lines().filter(|l| !l.starts_with("HDR")).count();
    let h = text.lines().next().expect("header").to_string();
    let patched = format!("{}{:07}{}", &h[..28], n, &h[35..]);
    text = text.replacen(&h, &patched, 1);
    let src = sources_from(&nasr_files(), text.as_bytes());
    let out = build_model(&src, &params("ZAU", 92_600)).expect("build");
    let u = &out.model.unresolved;
    assert!(
        (0..u.role.len()).any(|i| s(&out.model, u.name[i]) == ident),
        "{ident} must be unresolved"
    );
}

// REQ: OA-ASP-005
#[test]
fn zau_boundary_separation_is_reported_with_its_edge() {
    let m = fixture_build().model;
    let seps = boundary_separations(&m);
    assert_eq!(
        seps.len(),
        2,
        "the two ZAU boundaries have vertices; CIFP volumes carry their own via codes"
    );
    for sep in &seps {
        assert_eq!(sep.facility, "ZAU");
        let r = sep.result.as_ref().expect("computed");
        // Real ZAU data: the worst edge runs from 43.69167,-87.16944 to 43.58667,-84.96361, 592 m.
        assert!((r.max_m - 592.4).abs() < 1.0, "{}", r.max_m);
        let ((a_lat, a_lon), (b_lat, b_lon)) = sep.edge_ends.expect("edge ends");
        assert!((a_lat - 43.69167).abs() < 1e-4 && (a_lon + 87.16944).abs() < 1e-4);
        assert!((b_lat - 43.58667).abs() < 1e-4 && (b_lon + 84.96361).abs() < 1e-4);
    }
    assert_eq!(seps[0].stratum, stratum::ARTCC_HIGH);
    assert_eq!(seps[1].stratum, stratum::ARTCC_LOW);
    // The model keeps the vertices only and says the interpretation is unknown.
    assert!((0..m.boundaries.source.len())
        .filter(|&i| m.boundaries.source[i] == boundary_source::NASR_ARB)
        .all(|i| m.boundaries.edge_interp[i] == edge_interp::UNKNOWN));
}

fn report_for(out: &oa_airspace::build::BuildOutput, src: &oa_airspace::source::Sources) -> String {
    let bytes = encode(&out.model).expect("encode");
    render_report(&ReportInput {
        model: &out.model,
        info: &out.info,
        nasr: &src.nasr_archive,
        cifp: &src.cifp_archive,
        diag: &src.diag,
        snapshot_sha256: sha256(&bytes),
        snapshot_bytes: bytes.len() as u64,
    })
}

// REQ: OA-ASP-005
#[test]
fn the_report_lists_inputs_counts_unknowns_and_the_snapshot_hash() {
    let src = fixture_sources();
    let out = build_model(&src, &params("ZAU", 92_600)).expect("build");
    let report = report_for(&out, &src);
    let bytes = encode(&out.model).expect("encode");
    for needle in [
        "ARTCC ZAU",
        &to_hex(&src.nasr_archive.sha256),
        &to_hex(&src.cifp_archive.sha256),
        &to_hex(&sha256(&bytes)),
        "CIFP cycle 2611",
        "## Counts per table",
        "## Cross-source position disagreements",
        "## Unresolved references",
        "## Boundary geometry: geodesic versus rhumb",
        "592.4 m on edge",
        "largest rounding error",
        "UK-001",
        "UK-002",
        "UK-003",
        "KORD SID O'HARE NINE",
    ] {
        assert!(report.contains(needle), "report lacks {needle:?}");
    }
    // The report is a pure function of its inputs: no clock, no paths.
    assert_eq!(report, report_for(&out, &src));
    assert!(!report.contains("/home/") && !report.contains("/tmp/"));
}

// REQ: OA-ASP-001
#[test]
fn the_loader_checks_expected_hashes_and_refuses_mismatches() {
    let nasr = nasr_zip(&nasr_files());
    let cifp = cifp_zip(&cifp_file());
    let good = [
        Expectation {
            file: NASR_NAME.into(),
            sha256: sha256(&nasr),
        },
        Expectation {
            file: CIFP_NAME.into(),
            sha256: sha256(&cifp),
        },
    ];
    let ok = load_bytes(NASR_NAME, &nasr, CIFP_NAME, &cifp, &good).expect("matching hashes load");
    assert_eq!(ok.nasr_archive.expected, Some(sha256(&nasr)));
    assert_eq!(ok.cifp_archive.sha256, sha256(&cifp));
    // No expectation: loads, and the archive records that nothing was checked.
    let none = load_bytes(NASR_NAME, &nasr, CIFP_NAME, &cifp, &[]).expect("loads");
    assert_eq!(none.nasr_archive.expected, None);
    // A wrong hash is refused before anything is parsed.
    let mut wrong = good.clone();
    wrong[0].sha256[0] ^= 1;
    assert!(matches!(
        load_bytes(NASR_NAME, &nasr, CIFP_NAME, &cifp, &wrong),
        Err(LoadError::HashMismatch { .. })
    ));
    // An expectation naming neither archive is refused, not ignored.
    let stray = [Expectation {
        file: "OTHER.zip".into(),
        sha256: [0; 32],
    }];
    assert!(matches!(
        load_bytes(NASR_NAME, &nasr, CIFP_NAME, &cifp, &stray),
        Err(LoadError::ExpectationUnused(_))
    ));
    // A damaged archive is an error, never a panic.
    let mut bad = nasr.clone();
    let mid = bad.len() / 2;
    bad[mid] ^= 0xff;
    let _ = load_bytes(NASR_NAME, &bad, CIFP_NAME, &cifp, &[]);
    assert!(load_bytes(NASR_NAME, b"not a zip", CIFP_NAME, &cifp, &[]).is_err());
    assert!(load_bytes(NASR_NAME, &nasr, CIFP_NAME, &[], &[]).is_err());
}

// REQ: OA-ASP-001
#[test]
fn expectation_arguments_are_parsed_strictly() {
    let hex = "ab".repeat(32);
    let e = parse_expectation(&format!("A.zip={hex}")).expect("parses");
    assert_eq!(e.file, "A.zip");
    assert_eq!(e.sha256, [0xab; 32]);
    assert!(parse_expectation(&format!("A.zip={}", hex.to_uppercase())).is_ok());
    for bad in [
        "A.zip",
        "=".to_string().as_str(),
        "A.zip=abc",
        &format!("A.zip={}z", &hex[..63]),
        &format!("A.zip={hex}00"),
        &format!("={hex}"),
    ] {
        assert!(parse_expectation(bad).is_err(), "{bad:?}");
    }
}

// REQ: OA-ASP-001
#[test]
fn a_nasr_archive_missing_a_file_is_refused_with_the_name() {
    let mut files = nasr_files();
    files.retain(|(n, _)| n != "ARB_SEG.csv");
    let err = load_bytes(
        NASR_NAME,
        &nasr_zip(&files),
        CIFP_NAME,
        &cifp_zip(&cifp_file()),
        &[],
    )
    .expect_err("missing file");
    assert!(err.to_string().contains("ARB_SEG"), "{err}");
    // A NASR file the model needs but whose schema file is absent is refused too.
    let mut files = nasr_files();
    files.retain(|(n, _)| n != "FIX_CSV_DATA_STRUCTURE.csv");
    assert!(load_bytes(
        NASR_NAME,
        &nasr_zip(&files),
        CIFP_NAME,
        &cifp_zip(&cifp_file()),
        &[]
    )
    .is_err());
}

// REQ: OA-ASP-001
#[test]
fn the_effective_date_is_the_latest_when_a_cycle_mixes_dates() {
    // The 2026-10-01 archive keeps 2026/09/03 on files that did not change.
    let mut files = nasr_files();
    edit_nasr_all(&mut files, "ARB_BASE.csv", "2026/10/29", "2026/09/03");
    let src = sources_from(&files, &cifp_file());
    let out = build_model(&src, &params("ZAU", 92_600)).expect("build");
    assert_eq!(out.model.meta.nasr_effective, 20_261_029);
    assert_eq!(
        out.info.nasr_dates,
        vec!["2026/09/03".to_string(), "2026/10/29".to_string()]
    );
    assert!(report_for(&out, &src).contains("EFF_DATE values seen: 2026/09/03, 2026/10/29"));
}

fn edit_nasr_all(files: &mut [(String, Vec<u8>)], file: &str, from: &str, to: &str) {
    let entry = files
        .iter_mut()
        .find(|(n, _)| n == file)
        .expect("file present");
    let text = String::from_utf8(entry.1.clone()).expect("utf-8");
    entry.1 = text.replace(from, to).into_bytes();
}

// REQ: OA-ASP-006
#[test]
fn building_twice_gives_byte_identical_snapshots_and_hashes() {
    // Two independent loads and builds: nothing in the output may depend on map order or time.
    let a = encode(
        &build_model(&fixture_sources(), &params("ZAU", 92_600))
            .expect("a")
            .model,
    )
    .expect("encode");
    let b = encode(
        &build_model(&fixture_sources(), &params("ZAU", 92_600))
            .expect("b")
            .model,
    )
    .expect("encode");
    assert_eq!(a, b);
    assert_eq!(sha256(&a), sha256(&b));
    // Even the archive container does not matter, only the content it holds.
    let mut reversed = nasr_files();
    reversed.reverse();
    let src = load_bytes(
        NASR_NAME,
        &nasr_zip(&reversed),
        CIFP_NAME,
        &cifp_zip(&cifp_file()),
        &[],
    )
    .expect("loads");
    let c = build_model(&src, &params("ZAU", 92_600)).expect("c").model;
    // The archive hash differs (the zip bytes differ), so compare everything but the meta hash.
    let mut m = c.clone();
    m.meta.nasr_sha256 = fixture_build().model.meta.nasr_sha256;
    assert_eq!(encode(&m).expect("encode"), a);
}

// REQ: OA-ASP-006
#[test]
fn a_snapshot_reads_back_to_the_same_model_and_rewrites_identically() {
    let m = fixture_build().model;
    let bytes = encode(&m).expect("encode");
    assert!(bytes.starts_with(b"OATCA-ADAPT"));
    let back = decode(&bytes).expect("decode");
    assert_eq!(back, m);
    let again = encode(&back).expect("re-encode");
    assert_eq!(again, bytes, "write, read, write is the identity");
    assert_eq!(sha256(&again), sha256(&bytes));
    // The model's strings, points and procedures survive by value.
    assert!(airport(&back, "KORD").is_some());
    assert!(find_proc(&back, "KORD", proc_kind::STAR, "BENKY6").is_some());
}

// REQ: OA-ASP-006
#[test]
fn different_inputs_give_different_snapshots() {
    let a = encode(&fixture_build().model).expect("encode");
    let mut files = nasr_files();
    shift_lat(
        &mut files,
        "FIX_BASE.csv",
        "\"2026/10/29\",\"MOSCO\"",
        5,
        1.0,
    );
    let src = sources_from(&files, &cifp_file());
    let b = encode(&build_model(&src, &params("ZAU", 92_600)).expect("b").model).expect("encode");
    assert_ne!(a, b, "the NASR archive hash is part of the snapshot");
    assert_ne!(sha256(&a), sha256(&b));
}

// REQ: OA-ASP-007
#[test]
fn a_snapshot_diffed_with_itself_is_empty() {
    use oa_airspace::diff::diff_models;
    let m = fixture_build().model;
    let d = diff_models(&m, &m).expect("diff");
    assert!(d.header.is_empty() && d.entries.is_empty() && d.notes.is_empty());
}

// REQ: OA-ASP-007
#[test]
fn a_moved_fix_is_listed_as_changed_with_its_old_and_new_position() {
    use oa_airspace::diff::{diff_models, render_diff, Change};
    let a = fixture_build().model;
    let mut files = nasr_files();
    // MOSCO is in NASR only (the CIFP fixture has no record for it), so the NASR move is the move.
    shift_lat(
        &mut files,
        "FIX_BASE.csv",
        "\"2026/10/29\",\"MOSCO\"",
        5,
        1_000.0,
    );
    let src = sources_from(&files, &cifp_file());
    let b = build_model(&src, &params("ZAU", 92_600)).expect("b").model;
    let d = diff_models(&a, &b).expect("diff");
    let changed: Vec<_> = d.entries.iter().filter(|e| e.class == "point").collect();
    assert_eq!(changed.len(), 1, "{changed:?}");
    assert_eq!(changed[0].key, "fix MOSCO K3");
    let Change::Changed(fields) = &changed[0].change else {
        panic!("expected a change")
    };
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].field, "position");
    assert_ne!(fields[0].from, fields[0].to);
    // The archive hash also differs in the header, nothing else does.
    assert_eq!(d.header.len(), 1);
    assert_eq!(d.header[0].field, "NASR archive sha256");
    let text = render_diff(&d, "a", "b");
    assert!(text.contains("~ point fix MOSCO K3") && text.contains("position:"));
    // The same diff twice is the same text; swapping the sides swaps the values.
    assert_eq!(
        text,
        render_diff(&diff_models(&a, &b).expect("diff"), "a", "b")
    );
    let back = diff_models(&b, &a).expect("diff");
    let Change::Changed(rev) = &back
        .entries
        .iter()
        .find(|e| e.class == "point")
        .expect("entry")
        .change
    else {
        panic!()
    };
    assert_eq!((&rev[0].from, &rev[0].to), (&fields[0].to, &fields[0].from));
}

// REQ: OA-ASP-007
#[test]
fn added_and_removed_objects_are_listed_in_a_fixed_order() {
    use oa_airspace::diff::{diff_counts, diff_models, Change};
    let src = fixture_sources();
    let wide = build_model(&src, &params("ZAU", 92_600))
        .expect("wide")
        .model;
    let tight = build_model(&src, &params("ZAU", 0)).expect("tight").model;
    let d = diff_models(&tight, &wide).expect("diff");
    assert!(d.header.iter().any(|h| h.field == "buffer (m)"));
    let added = d
        .entries
        .iter()
        .filter(|e| e.change == Change::Added)
        .count();
    let removed = d
        .entries
        .iter()
        .filter(|e| e.change == Change::Removed)
        .count();
    assert!(added > 0, "the wider buffer adds objects");
    let counts = diff_counts(&d);
    assert_eq!(counts.iter().map(|c| c.1).sum::<usize>(), added);
    assert_eq!(counts.iter().map(|c| c.2).sum::<usize>(), removed);
    // Entries are grouped by class in a fixed order and sorted by key within a class.
    let order = [
        "point",
        "boundary",
        "airway",
        "procedure",
        "mora",
        "unresolved",
        "disagreement",
    ];
    let mut last = (0usize, String::new());
    for e in &d.entries {
        let c = order
            .iter()
            .position(|o| *o == e.class)
            .expect("known class");
        assert!(
            c > last.0 || (c == last.0 && e.key >= last.1),
            "{} {} after {:?}",
            e.class,
            e.key,
            last
        );
        last = (c, e.key.clone());
    }
    // Reversing the roles turns every addition into a removal.
    let r = diff_models(&wide, &tight).expect("diff");
    assert_eq!(
        r.entries
            .iter()
            .filter(|e| e.change == Change::Removed)
            .count(),
        added
    );
    assert_eq!(
        r.entries
            .iter()
            .filter(|e| e.change == Change::Added)
            .count(),
        removed
    );
}

// REQ: OA-ASP-007
#[test]
fn a_changed_artcc_low_boundary_is_called_out() {
    use oa_airspace::diff::diff_models;
    let a = fixture_build().model;
    let mut files = nasr_files();
    // Move one ZAU LOW vertex: the FAA plans to redefine this boundary (UNKNOWNS), so a diff must say so.
    let seg = String::from_utf8(
        files
            .iter()
            .find(|(n, _)| n == "ARB_SEG.csv")
            .expect("file")
            .1
            .clone(),
    )
    .expect("utf-8");
    let line = seg
        .lines()
        .find(|l| l.contains("\"LOW\"") && l.contains("\"ZAU\""))
        .expect("a LOW row")
        .to_string();
    let prefix = line.split(',').take(7).collect::<Vec<_>>().join(",");
    shift_lat(&mut files, "ARB_SEG.csv", &prefix, 7, 1_000.0);
    let b = build_model(&sources_from(&files, &cifp_file()), &params("ZAU", 92_600))
        .expect("b")
        .model;
    let d = diff_models(&a, &b).expect("diff");
    assert!(d
        .entries
        .iter()
        .any(|e| e.class == "boundary" && e.key.contains("ARTCC LOW")));
    assert!(
        d.notes.iter().any(|n| n.contains("ARTCC LOW")),
        "{:?}",
        d.notes
    );
}

// REQ: OA-ASP-007
#[test]
fn a_diff_of_an_inconsistent_model_is_an_error_not_a_panic() {
    use oa_airspace::diff::diff_models;
    let a = fixture_build().model;
    let mut b = a.clone();
    b.points.lat.pop();
    assert!(diff_models(&a, &b).is_err());
    assert!(diff_models(&b, &a).is_err());
}
