//! The build report (OA-ASP-003, 004, 005; docs/M0_DESIGN.md "Report"). **Core** (B1): a pure
//! function from the model, what the build learned, the diagnostics and the snapshot hash to
//! text. The shell writes the text beside the snapshot.
//!
//! The report has no timestamps and no paths, so it is as reproducible as the snapshot.
//! Sections, in order: inputs, selection and rules, counts per table, cross-source
//! disagreements (all, largest first), NASR position defects, schema violations, unresolved
//! references (all), boundary geometry (the geodesic-versus-rhumb Unknown, UK-001),
//! fixed-point rounding, other findings, snapshot identity.

use crate::build::{BuildInfo, DISAGREE_M, MAX_PAIR_M, POSITION_DEFECT_DEG};
use crate::geom::{e7_to_deg, edge_separations, latlon_e7, RingSeparation};
use crate::model::*;
use crate::source::{ArchiveInfo, Diagnostics};
use oa_geo::{wgs84, LatLon};
use oa_nasr::schema::{SchemaViolation, ViolationKind};
use oa_sha256::to_hex;
use std::collections::BTreeMap;
use std::fmt::Write;

/// Everything the report prints.
pub struct ReportInput<'a> {
    pub model: &'a Model,
    pub info: &'a BuildInfo,
    pub nasr: &'a ArchiveInfo,
    pub cifp: &'a ArchiveInfo,
    pub diag: &'a Diagnostics,
    /// SHA-256 of the snapshot file.
    pub snapshot_sha256: [u8; 32],
    pub snapshot_bytes: u64,
}

/// One row of the boundary-geometry section.
#[derive(Clone, Debug, PartialEq)]
pub struct BoundarySep {
    pub facility: String,
    pub stratum: u8,
    pub vertices: usize,
    /// The result, or why it could not be computed.
    pub result: Result<RingSeparation, String>,
    /// The two vertices of the worst edge, as (lat, lon) degrees.
    pub edge_ends: Option<((f64, f64), (f64, f64))>,
}

/// Per boundary with vertices (NASR ARTCC, FIR, CTA, UTA), the largest geodesic-versus-rhumb
/// separation over its edges and the edge where it occurs (OA-ASP-005). Boundaries whose
/// segments carry their own via code (CIFP) have no such unknown and are not listed.
pub fn boundary_separations(m: &Model) -> Vec<BoundarySep> {
    let ell = wgs84();
    let b = &m.boundaries;
    let mut out = Vec::new();
    for row in 0..b.source.len() {
        let Some(span) = b.vertices.get(row) else {
            continue;
        };
        if span.len == 0 {
            continue;
        }
        let start = span.start.0 as usize;
        let verts = m
            .vertices
            .get(start..start.saturating_add(span.len as usize))
            .unwrap_or(&[]);
        let ring: Result<Vec<LatLon>, _> = verts.iter().map(|v| latlon_e7(v.lat, v.lon)).collect();
        let facility = b
            .facility
            .get(row)
            .map_or("", |f| str_or_empty(&m.strings, *f))
            .to_string();
        let stratum = b.stratum.get(row).copied().unwrap_or(0);
        let (result, edge_ends) = match ring {
            Ok(ring) => match edge_separations(&ell, &ring) {
                Ok(sep) => {
                    let a = ring.get(sep.edge).copied();
                    let z = ring.get((sep.edge + 1) % ring.len().max(1)).copied();
                    let ends = a
                        .zip(z)
                        .map(|(a, z)| ((a.lat(), a.lon()), (z.lat(), z.lon())));
                    (Ok(sep), ends)
                }
                Err(e) => (Err(format!("{e:?}")), None),
            },
            Err(e) => (Err(format!("{e:?}")), None),
        };
        out.push(BoundarySep {
            facility,
            stratum,
            vertices: verts.len(),
            result,
            edge_ends,
        });
    }
    out
}

/// Rows, bytes per row (sum of the columns' widths; a bitset column counts as one eighth of a
/// byte) and total bytes of one table.
#[derive(Clone, Debug, PartialEq)]
pub struct TableSize {
    pub name: &'static str,
    pub rows: usize,
    pub row_bytes: f64,
    pub total_bytes: f64,
}

/// Sizes of every table in memory.
pub fn table_sizes(m: &Model) -> Vec<TableSize> {
    let t = |name: &'static str, rows: usize, fixed: usize, bitsets: usize| TableSize {
        name,
        rows,
        row_bytes: fixed as f64 + bitsets as f64 / 8.0,
        total_bytes: (rows * fixed) as f64 + (bit_words(rows) * 8 * bitsets) as f64,
    };
    vec![
        TableSize {
            name: "strings (text + offsets)",
            rows: str_count(&m.strings),
            row_bytes: 4.0,
            total_bytes: (m.strings.text.len() + m.strings.offsets.len() * 4) as f64,
        },
        t("points", m.points.kind.len(), 1 + 3 * 4 + 5 * 4, 7),
        t(
            "vertices (PosE7)",
            m.vertices.len(),
            core::mem::size_of::<PosE7>(),
            0,
        ),
        t(
            "boundaries",
            m.boundaries.source.len(),
            4 + 2 * 4 + 2 * 4 + 2 * core::mem::size_of::<Span<VertexIx>>(),
            2,
        ),
        t("segments", m.segments.via.len(), 1 + 4 * 4 + 4 + 2, 0),
        t(
            "airways",
            m.airways.designator.len(),
            4 + 1 + core::mem::size_of::<Span<AirwayLegIx>>(),
            0,
        ),
        t(
            "airway legs",
            m.airway_legs.from.len(),
            2 * 4 + 3 * 4 + 2,
            3,
        ),
        t(
            "procedures",
            m.procs.kind.len(),
            1 + 3 * 4 + core::mem::size_of::<Span<RouteIx>>(),
            3,
        ),
        t(
            "routes",
            m.routes.route_type.len(),
            1 + 4 + core::mem::size_of::<Span<LegIx>>(),
            0,
        ),
        t(
            "legs",
            m.legs.fix.len(),
            2 * 4 + 2 + 2 + 4 + 1 + 2 * 4 + 2,
            1,
        ),
        t("MORA cells", m.mora.lat.len(), 6, 0),
        t(
            "unresolved references",
            m.unresolved.role.len(),
            2 + 6 * 4,
            0,
        ),
        t("disagreements", m.disagreements.point.len(), 16, 0),
    ]
}

fn date_str(d: u32) -> String {
    format!("{:04}-{:02}-{:02}", d / 10_000, d / 100 % 100, d % 100)
}

fn count_where(col: &[u8], v: u8) -> usize {
    col.iter().filter(|x| **x == v).count()
}

fn point_label(m: &Model, ix: PointIx) -> String {
    match point_key(m, ix) {
        Some(k) => {
            let kind = at(&m.points.kind, ix.0).map_or("?", point_kind::name);
            let mut s = format!("{kind} {}", k.ident);
            if !k.region.is_empty() {
                let _ = write!(s, " ({})", k.region);
            }
            if !k.scope.is_empty() {
                let _ = write!(s, " at {}", k.scope);
            }
            s
        }
        None => "?".to_string(),
    }
}

fn violation_kind(k: &ViolationKind) -> String {
    match k {
        ViolationKind::MissingColumn => "column missing from the file".to_string(),
        ViolationKind::UnexpectedColumn => "column not in the schema".to_string(),
        ViolationKind::TooLong { max, .. } => format!("text longer than {max}"),
        ViolationKind::NotNumeric(_) => "not numeric".to_string(),
        ViolationKind::TooManyIntegerDigits { max, .. } => {
            format!("more than {max} integer digits")
        }
        ViolationKind::TooManyFractionDigits { max, .. } => {
            format!("more than {max} fraction digits")
        }
        ViolationKind::NullViolation => "empty in a non-nullable column".to_string(),
    }
}

fn schema_section(out: &mut String, diag: &Diagnostics) {
    let _ = writeln!(out, "## NASR schema violations (oa-nasr validation against the FAA *_CSV_DATA_STRUCTURE files)\n");
    for w in &diag.schema_warnings {
        let _ = writeln!(out, "structure file note: {w}");
    }
    let mut total = 0u64;
    for d in &diag.nasr_files {
        let n = d.violations.len() as u64 + d.dropped;
        total += n;
        let _ = writeln!(
            out,
            "{}: {} rows checked, {} violations{}{}",
            d.file,
            d.rows,
            n,
            if d.dropped > 0 {
                format!(" ({} beyond the cap not kept)", d.dropped)
            } else {
                String::new()
            },
            if d.schema_missing {
                "; NO SCHEMA FOUND for this file"
            } else {
                ""
            }
        );
        let mut groups: BTreeMap<(String, String), (usize, &SchemaViolation)> = BTreeMap::new();
        for v in &d.violations {
            let e = groups
                .entry((v.column.clone(), violation_kind(&v.kind)))
                .or_insert((0, v));
            e.0 += 1;
        }
        for ((col, kind), (n, first)) in &groups {
            let _ = writeln!(
                out,
                "    column {col}: {kind}: {n} rows (first at line {})",
                first.line
            );
        }
        let rejected = d.rejected.len();
        if rejected > 0 {
            let _ = writeln!(out, "    {rejected} rows rejected by the typed parser:");
            for r in &d.rejected {
                let _ = writeln!(out, "      {r}");
            }
        }
    }
    let _ = writeln!(out, "total schema violations: {total}");
    let _ = writeln!(
        out,
        "CIFP records that failed to decode: {}{}",
        diag.cifp_errors.len() + diag.cifp_errors_dropped,
        if diag.cifp_errors_dropped > 0 {
            " (some beyond the parser's cap not kept)"
        } else {
            ""
        }
    );
    for e in &diag.cifp_errors {
        let _ = writeln!(out, "    {e}");
    }
    out.push('\n');
}

/// Renders the report. Pure: the same input gives the same text.
pub fn render_report(r: &ReportInput<'_>) -> String {
    let m = r.model;
    let info = r.info;
    let mut o = String::new();
    let _ = writeln!(o, "OpenATCA adaptation build report");
    let _ = writeln!(o, "================================\n");
    let _ = writeln!(o, "ARTCC {} {}", m.meta.artcc, info.artcc_name);
    let _ = writeln!(
        o,
        "NASR effective date {} (latest EFF_DATE in the files used); CIFP cycle {}",
        date_str(m.meta.nasr_effective),
        m.meta.cifp_cycle
    );
    if info.nasr_dates.len() > 1 {
        let _ = writeln!(
            o,
            "NASR EFF_DATE values seen: {} (the FAA leaves unchanged files on their earlier date)",
            info.nasr_dates.join(", ")
        );
    }
    let _ = writeln!(
        o,
        "selection buffer {} m ({:.1} NM)\n",
        m.meta.buffer_m,
        f64::from(m.meta.buffer_m) / 1852.0
    );

    let _ = writeln!(o, "## Inputs\n");
    for (label, a) in [("NASR CSV archive", r.nasr), ("CIFP archive", r.cifp)] {
        let _ = writeln!(o, "{label}: {} ({} bytes)", a.name, a.bytes);
        let _ = writeln!(o, "    sha256 {}", to_hex(&a.sha256));
        match a.expected {
            Some(_) => {
                let _ = writeln!(o, "    matches the --expect-sha256 value given");
            }
            None => {
                let _ = writeln!(
                    o,
                    "    NOT VERIFIED: no --expect-sha256 was given for this archive"
                );
            }
        }
    }
    o.push('\n');

    let _ = writeln!(o, "## Selection and rules\n");
    let _ = writeln!(o, "Selection area (OA-ASP-002): inside the union of these boundaries, or within {} m of an edge:", m.meta.buffer_m);
    for s in &info.area_rings {
        let _ = writeln!(o, "    {s}");
    }
    let _ = writeln!(o, "Also selected: every procedure of every airport inside a polygon (not the buffer), every airway with a point");
    let _ = writeln!(o, "in the area (whole airway), every point those reference, class B/C/D and SUA volumes and NASR boundaries");
    let _ = writeln!(o, "that touch the area, and MORA cells that overlap it.");
    let _ = writeln!(o, "Position rule: where NASR and CIFP both carry an object the CIFP position is taken (CIFP is the coded");
    let _ = writeln!(o, "navigation data; NASR is the registry). Every pair differing by more than {DISAGREE_M} m is listed below.");
    let _ = writeln!(o, "Pairing: fixes by (ident, region); airports by ICAO or FAA identifier; thresholds by airport and designator;");
    let _ = writeln!(
        o,
        "navaids by ident and class, nearest first within {} m (short NDB identifiers recur).",
        MAX_PAIR_M
    );
    let _ = writeln!(o, "Boundary sources are explicit per row: NASR ARB (published ARTCC outline, not ERAM operational boundaries or");
    let _ = writeln!(
        o,
        "sectors), CIFP UC (class B/C/D), CIFP UR (special use airspace)."
    );
    let _ = writeln!(o, "CIFP fields not consumed (UNKNOWNS UK-002): leg RNP and GNSS/route-qualifier codes, leg arc radius, altitude");
    let _ = writeln!(o, "description codes other than '+' and 'B' (stored as unverified), TCH indicator, header CRC, southern MORA rows.");
    let _ = writeln!(o, "Procedure legs carry magnetic courses (CIFP gives magnetic); no variation model is applied.\n");

    let _ = writeln!(o, "## Counts per table\n");
    let _ = writeln!(
        o,
        "{:<28}{:>9}{:>12}{:>14}",
        "table", "rows", "bytes/row", "total bytes"
    );
    let mut grand = 0.0;
    for t in table_sizes(m) {
        grand += t.total_bytes;
        let _ = writeln!(
            o,
            "{:<28}{:>9}{:>12.3}{:>14.0}",
            t.name, t.rows, t.row_bytes, t.total_bytes
        );
    }
    let _ = writeln!(o, "{:<28}{:>9}{:>12}{:>14.0}", "all tables", "", "", grand);
    let _ = writeln!(
        o,
        "size guards (compile-time, exact): PosE7 = {} bytes, Span = {} bytes",
        core::mem::size_of::<PosE7>(),
        core::mem::size_of::<Span<VertexIx>>()
    );
    let p = &m.points;
    let _ = writeln!(o, "\npoints by kind:");
    for k in 1..=point_kind::MAX {
        let rows: Vec<usize> = (0..p.kind.len())
            .filter(|&i| p.kind.get(i) == Some(&k))
            .collect();
        if rows.is_empty() {
            continue;
        }
        let both = rows
            .iter()
            .filter(|&&i| bit_get(&p.src_nasr, i) && bit_get(&p.src_cifp, i))
            .count();
        let nasr_only = rows
            .iter()
            .filter(|&&i| bit_get(&p.src_nasr, i) && !bit_get(&p.src_cifp, i))
            .count();
        let cifp_only = rows
            .iter()
            .filter(|&&i| !bit_get(&p.src_nasr, i) && bit_get(&p.src_cifp, i))
            .count();
        let _ = writeln!(
            o,
            "    {:<18}{:>6}  (both sources {both}, NASR only {nasr_only}, CIFP only {cifp_only})",
            point_kind::name(k),
            rows.len()
        );
    }
    let _ = writeln!(
        o,
        "    in the selection area: {}; pulled in only because a selected procedure or airway references them: {}",
        bit_count(&p.in_area),
        p.kind.len() - bit_count(&p.in_area)
    );
    let _ = writeln!(o, "boundaries by source and stratum:");
    for src in 1..=boundary_source::MAX {
        for st in 1..=stratum::MAX {
            let n = (0..m.boundaries.source.len())
                .filter(|&i| {
                    m.boundaries.source.get(i) == Some(&src)
                        && m.boundaries.stratum.get(i) == Some(&st)
                })
                .count();
            if n > 0 {
                let name = match src {
                    boundary_source::NASR_ARB => "NASR ARB",
                    boundary_source::CIFP_UC => "CIFP UC",
                    _ => "CIFP UR",
                };
                let _ = writeln!(o, "    {name:<9}{:<18}{n:>5}", stratum::name(st));
            }
        }
    }
    let pr = &m.procs;
    let _ = writeln!(o, "procedures:");
    for k in 1..=proc_kind::MAX {
        let rows: Vec<usize> = (0..pr.kind.len())
            .filter(|&i| pr.kind.get(i) == Some(&k))
            .collect();
        if rows.is_empty() {
            continue;
        }
        let coded = rows.iter().filter(|&&i| bit_get(&pr.coded, i)).count();
        let both = rows
            .iter()
            .filter(|&&i| bit_get(&pr.src_nasr, i) && bit_get(&pr.src_cifp, i))
            .count();
        let _ = writeln!(
            o,
            "    {:<10}{:>5}  (coded {coded}, uncoded NASR-only {}, in both sources {both})",
            proc_kind::name(k),
            rows.len(),
            rows.len() - coded
        );
    }
    let _ = writeln!(
        o,
        "airports with class airspace flags in NASR (B/C/D/E): {}/{}/{}/{}\n",
        bit_count(&p.cls_b),
        bit_count(&p.cls_c),
        bit_count(&p.cls_d),
        bit_count(&p.cls_e)
    );

    let d = &m.disagreements;
    let _ = writeln!(o, "## Cross-source position disagreements (OA-ASP-003)\n");
    let _ = writeln!(
        o,
        "Points in the model present in both NASR and CIFP whose positions differ by more than {DISAGREE_M} m: {}",
        d.point.len()
    );
    let mut rows: Vec<usize> = (0..d.point.len()).collect();
    rows.sort_by(|&a, &b| {
        d.dist_mm
            .get(b)
            .cmp(&d.dist_mm.get(a))
            .then(d.point.get(a).cmp(&d.point.get(b)))
    });
    for i in rows {
        let (Some(pix), Some(mm)) = (d.point.get(i), d.dist_mm.get(i)) else {
            continue;
        };
        let ix = pix.0 as usize;
        let cifp = (
            p.lat.get(ix).copied().map(e7_to_deg),
            p.lon.get(ix).copied().map(e7_to_deg),
        );
        let nasr = (
            d.nasr_lat.get(i).copied().map(e7_to_deg),
            d.nasr_lon.get(i).copied().map(e7_to_deg),
        );
        let _ = writeln!(
            o,
            "    {:>12.3} m  {}  CIFP {:.7},{:.7}  NASR {:.7},{:.7}",
            f64::from(*mm) / 1000.0,
            point_label(m, *pix),
            cifp.0.unwrap_or(f64::NAN),
            cifp.1.unwrap_or(f64::NAN),
            nasr.0.unwrap_or(f64::NAN),
            nasr.1.unwrap_or(f64::NAN)
        );
    }
    let _ = writeln!(
        o,
        "Whole-cycle check (informational; only points in the model are listed above): {} NASR/CIFP pairs compared, {} differ by more than {DISAGREE_M} m, largest {:.3} m.",
        info.pairs_compared, info.pairs_over_limit, info.pairs_max_m
    );
    for (d, label) in &info.pairs_over {
        let _ = writeln!(o, "    {label}: {d:.3} m");
    }
    o.push('\n');

    let _ = writeln!(
        o,
        "## NASR position defects (decimal versus own DMS, over {POSITION_DEFECT_DEG} degree)\n"
    );
    let _ = writeln!(o, "objects in the model: {}", info.position_defects.len());
    for x in &info.position_defects {
        let _ = writeln!(
            o,
            "    {}:{} {} differs by {:.8} degrees",
            x.file, x.line, x.ident, x.diff_deg
        );
    }
    o.push('\n');

    schema_section(&mut o, r.diag);

    let u = &m.unresolved;
    let _ = writeln!(o, "## Unresolved references (OA-ASP-004)\n");
    let _ = writeln!(
        o,
        "references to a point neither source has: {}",
        u.role.len()
    );
    for i in 0..u.role.len() {
        let g = |c: &[StrIx]| c.get(i).map_or("", |s| str_or_empty(&m.strings, *s));
        let owner = if u.owner_kind.get(i) == Some(&owner_kind::AIRWAY) {
            format!("airway {} ({})", g(&u.owner), g(&u.scope))
        } else {
            format!("procedure {} at {}", g(&u.owner), g(&u.scope))
        };
        let _ = writeln!(
            o,
            "    {owner}: {} #{}: {:?} region {:?} wanted {:?}",
            unresolved_role::name(u.role.get(i).copied().unwrap_or(0)),
            u.ordinal.get(i).copied().unwrap_or(0),
            g(&u.name),
            g(&u.region),
            g(&u.wanted)
        );
    }
    o.push('\n');

    let _ = writeln!(
        o,
        "## Boundary geometry: geodesic versus rhumb (OA-ASP-005, UNKNOWNS UK-001)\n"
    );
    let _ = writeln!(o, "NASR does not say whether boundary points are joined by great-circle or rhumb lines. Both are possible; the model");
    let _ = writeln!(o, "stores the vertices only. Per boundary: the largest distance between the two interpretations over its edges.");
    for s in boundary_separations(m) {
        match (&s.result, s.edge_ends) {
            (Ok(sep), Some((a, z))) => {
                let _ = writeln!(
                    o,
                    "    {:<5}{:<16}{:>4} vertices: max {:>9.1} m on edge {} ({:.5},{:.5} to {:.5},{:.5}; edge {:.1} km, at {:.0}% along)",
                    s.facility,
                    stratum::name(s.stratum),
                    s.vertices,
                    sep.max_m,
                    sep.edge,
                    a.0,
                    a.1,
                    z.0,
                    z.1,
                    sep.edge_len_m / 1000.0,
                    sep.at_fraction * 100.0
                );
            }
            (Err(e), _) => {
                let _ = writeln!(
                    o,
                    "    {:<5}{:<16}{:>4} vertices: not computable ({e})",
                    s.facility,
                    stratum::name(s.stratum),
                    s.vertices
                );
            }
            (Ok(_), None) => {}
        }
    }
    let _ = writeln!(o, "Selection used geodesic edges; compare the selected ARTCC's separation with the {} m buffer.", m.meta.buffer_m);
    let _ = writeln!(o, "The ARTCC LOW boundary is planned to be redefined by the FAA as the ground boundary; diff cycles to catch it.\n");

    let _ = writeln!(o, "## Fixed-point conversion (A9)\n");
    let _ = writeln!(
        o,
        "{} coordinates stored as 1e-7 degree integers; largest rounding error {:.6} m (at {}).",
        info.rounding.pos_values, info.rounding.pos_max_m, info.rounding.pos_at
    );
    let _ = writeln!(o, "Leg distances stored as whole metres; largest rounding error {:.3} m. Altitudes, courses, speeds exact.\n", info.rounding.dist_max_m);

    let _ = writeln!(o, "## Other findings\n");
    let list = |o: &mut String, title: &str, items: &[String]| {
        let _ = writeln!(o, "{title}: {}", items.len());
        for i in items {
            let _ = writeln!(o, "    {i}");
        }
    };
    list(&mut o, "duplicate keys within a source", &info.duplicates);
    list(
        &mut o,
        "navaid type disagreements (NASR type versus CIFP class)",
        &info.kind_mismatches,
    );
    list(
        &mut o,
        "procedures in NASR only, no coded legs (UK-003)",
        &info.uncoded_procedures,
    );
    list(
        &mut o,
        "class airspace disagreements (NASR CLS_ARSP versus CIFP UC)",
        &info.class_mismatches,
    );
    list(&mut o, "boundary notes", &info.boundary_notes);
    let _ = writeln!(o, "source records skipped:");
    for (what, n) in &info.skipped {
        let _ = writeln!(o, "    {n:>7}  {what}");
    }
    let _ = writeln!(
        o,
        "procedure legs with an undecoded altitude description code (UK-002): {}",
        info.unverified_alt_desc
    );
    let _ = writeln!(o, "legs by path terminator: {}", {
        let mut s = String::new();
        for c in 1..=path_term::MAX {
            let n = count_where(&m.legs.path_term, c);
            if n > 0 {
                let _ = write!(s, "{}={n} ", path_term::name(c));
            }
        }
        s
    });
    o.push('\n');

    let _ = writeln!(o, "## Snapshot\n");
    let _ = writeln!(o, "size {} bytes", r.snapshot_bytes);
    let _ = writeln!(o, "sha256 {}", to_hex(&r.snapshot_sha256));
    let _ = writeln!(o, "Defects found here are held and batched under the Avila Labs defect reporting policy; nothing is sent");
    let _ = writeln!(o, "to the FAA without approval. CIFP customer terms: report discovered CIFP errors to FAA AIS and notify users.");
    o
}
