//! Diffing two snapshots (OA-ASP-007). **Core** (B1): pure functions over two models.
//!
//! Every object class is flattened into `key -> [(field, value)]` where the key is the object's
//! stable identity and the values are rendered with the string table resolved, so a change in
//! interning order never shows up as a change. Comparing the two maps gives added, removed and
//! changed objects; a changed object lists exactly the fields that differ. Output order is
//! fixed: object classes in a fixed order, keys in sorted order, fields in declaration order.

use crate::model::*;
use oa_sha256::to_hex;
use std::collections::BTreeMap;
use std::fmt::Write;

/// One differing field: its name and its rendering on each side (`None` where absent).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldChange {
    pub field: String,
    pub from: Option<String>,
    pub to: Option<String>,
}

/// What happened to one object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// Only in the second snapshot.
    Added,
    /// Only in the first snapshot.
    Removed,
    /// In both, with these fields differing.
    Changed(Vec<FieldChange>),
}

/// One diff entry: object class, stable key and what changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffEntry {
    pub class: &'static str,
    pub key: String,
    pub change: Change,
}

/// The result of comparing two models.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diff {
    /// Differences in the headers (cycle, ARTCC, buffer, input hashes).
    pub header: Vec<FieldChange>,
    pub entries: Vec<DiffEntry>,
    /// Notes the reader should not miss (for example a changed ARTCC LOW boundary).
    pub notes: Vec<String>,
}

type Fields = Vec<(String, String)>;
type Objects = BTreeMap<String, Fields>;

fn s(m: &Model, ix: StrIx) -> &str {
    str_or_empty(&m.strings, ix)
}

fn point_ref(m: &Model, ix: PointIx) -> String {
    match point_key(m, ix) {
        None => "-".to_string(),
        Some(k) => {
            let mut r = k.ident.to_string();
            if !k.region.is_empty() || !k.scope.is_empty() {
                let _ = write!(r, "/{}", k.region);
            }
            if !k.scope.is_empty() {
                let _ = write!(r, "/{}", k.scope);
            }
            r
        }
    }
}

fn alt(v: i32) -> String {
    match v {
        ALT_NONE => "-".to_string(),
        ALT_GROUND => "GND".to_string(),
        ALT_UNKNOWN => "UNKNOWN".to_string(),
        ALT_UNLIMITED => "UNLIMITED".to_string(),
        x => format!("{x} ft"),
    }
}

fn opt_u16(v: u16) -> String {
    if v == NONE_U16 {
        "-".to_string()
    } else {
        v.to_string()
    }
}

fn pos(lat: i32, lon: i32) -> String {
    if lat == POS_NONE {
        "-".to_string()
    } else {
        format!("{:.7},{:.7}", f64::from(lat) * 1e-7, f64::from(lon) * 1e-7)
    }
}

fn sources(nasr: bool, cifp: bool) -> String {
    match (nasr, cifp) {
        (true, true) => "NASR+CIFP",
        (true, false) => "NASR",
        (false, true) => "CIFP",
        (false, false) => "none",
    }
    .to_string()
}

fn point_objects(m: &Model) -> Objects {
    let p = &m.points;
    let mut out = Objects::new();
    for row in 0..p.kind.len() {
        let ix = PointIx(row as u32);
        let Some(k) = point_key(m, ix) else { continue };
        let mut key = format!(
            "{} {}",
            point_kind::name(point_kind::class(p.kind[row])),
            k.ident
        );
        let _ = write!(key, " {}", if k.region.is_empty() { "-" } else { k.region });
        if !k.scope.is_empty() {
            let _ = write!(key, " {}", k.scope);
        }
        let flags = |b: &BitSet| bit_get(b, row).to_string();
        let f: Fields = vec![
            ("kind".into(), point_kind::name(p.kind[row]).into()),
            ("position".into(), pos(p.lat[row], p.lon[row])),
            ("elevation".into(), alt(p.elev_ft[row])),
            ("alt_ident".into(), s(m, p.alt_ident[row]).into()),
            ("name".into(), s(m, p.name[row]).into()),
            (
                "sources".into(),
                sources(bit_get(&p.src_nasr, row), bit_get(&p.src_cifp, row)),
            ),
            ("in_area".into(), flags(&p.in_area)),
            ("class_b".into(), flags(&p.cls_b)),
            ("class_c".into(), flags(&p.cls_c)),
            ("class_d".into(), flags(&p.cls_d)),
            ("class_e".into(), flags(&p.cls_e)),
        ];
        out.insert(key, f);
    }
    out
}

fn boundary_objects(m: &Model) -> Objects {
    let b = &m.boundaries;
    let mut out = Objects::new();
    for row in 0..b.source.len() {
        let source = match b.source[row] {
            boundary_source::NASR_ARB => "NASR-ARB",
            boundary_source::CIFP_UC => "CIFP-UC",
            _ => "CIFP-UR",
        };
        let mut key = format!(
            "{source} {} {}",
            stratum::name(b.stratum[row]),
            s(m, b.facility[row])
        );
        if b.multiple[row] != 0 {
            let _ = write!(key, " {}", char::from(b.multiple[row]));
        }
        if !s(m, b.name[row]).is_empty() {
            let _ = write!(key, " {}", s(m, b.name[row]));
        }
        let mut f: Fields = vec![
            (
                "edge_interp".into(),
                if b.edge_interp[row] == edge_interp::UNKNOWN {
                    "unknown"
                } else {
                    "per segment"
                }
                .into(),
            ),
            (
                "floor".into(),
                format!(
                    "{}{}",
                    alt(b.floor_ft[row]),
                    if bit_get(&b.floor_agl, row) {
                        " AGL"
                    } else {
                        ""
                    }
                ),
            ),
            (
                "ceiling".into(),
                format!(
                    "{}{}",
                    alt(b.ceiling_ft[row]),
                    if bit_get(&b.ceiling_agl, row) {
                        " AGL"
                    } else {
                        ""
                    }
                ),
            ),
        ];
        let vs = b.vertices[row];
        let verts = m
            .vertices
            .get(vs.start.0 as usize..(vs.start.0 + vs.len) as usize)
            .unwrap_or(&[]);
        f.push(("vertex_count".into(), verts.len().to_string()));
        for (i, v) in verts.iter().enumerate() {
            f.push((format!("vertex {i:04}"), pos(v.lat, v.lon)));
        }
        let ss = b.segs[row];
        let g = &m.segments;
        f.push(("segment_count".into(), ss.len.to_string()));
        for i in ss.start.0 as usize..(ss.start.0 + ss.len) as usize {
            f.push((
                format!("segment {:04}", i - ss.start.0 as usize),
                format!(
                    "via {} at {} centre {} radius {} m bearing {}",
                    g.via[i],
                    pos(g.lat[i], g.lon[i]),
                    pos(g.arc_lat[i], g.arc_lon[i]),
                    g.radius_m[i],
                    opt_u16(g.bearing_cdeg[i])
                ),
            ));
        }
        out.insert(key, f);
    }
    out
}

fn airway_objects(m: &Model) -> Objects {
    let a = &m.airways;
    let l = &m.airway_legs;
    let mut out = Objects::new();
    for row in 0..a.designator.len() {
        let key = format!(
            "airway {} ({})",
            s(m, a.designator[row]),
            char::from(a.location[row])
        );
        let sp = a.legs[row];
        let mut f: Fields = vec![("leg_count".into(), sp.len.to_string())];
        let mut seen: BTreeMap<String, usize> = BTreeMap::new();
        for i in sp.start.0 as usize..(sp.start.0 + sp.len) as usize {
            let from = point_ref(m, l.from[i]);
            let n = seen.entry(from.clone()).or_insert(0);
            *n += 1;
            let name = if *n == 1 {
                format!("leg from {from}")
            } else {
                format!("leg from {from} #{n}")
            };
            f.push((
                name,
                format!(
                    "to {} MEA {} opposite {} GPS {} dir {}/{} flags gap={} signal_gap={} dogleg={}",
                    point_ref(m, l.to[i]),
                    alt(l.mea_ft[i]),
                    alt(l.mea_opp_ft[i]),
                    alt(l.gps_mea_ft[i]),
                    l.mea_dir[i],
                    l.gps_mea_dir[i],
                    bit_get(&l.gap, i),
                    bit_get(&l.signal_gap, i),
                    bit_get(&l.dogleg, i)
                ),
            ));
        }
        out.insert(key, f);
    }
    out
}

fn proc_objects(m: &Model) -> Objects {
    let pr = &m.procs;
    let mut out = Objects::new();
    for row in 0..pr.kind.len() {
        let key = format!(
            "{} {} at {}",
            proc_kind::name(pr.kind[row]),
            s(m, pr.ident[row]),
            point_ref(m, pr.airport[row])
        );
        let rs = pr.routes[row];
        let mut f: Fields = vec![
            ("name".into(), s(m, pr.name[row]).into()),
            (
                "sources".into(),
                sources(bit_get(&pr.src_nasr, row), bit_get(&pr.src_cifp, row)),
            ),
            ("coded".into(), bit_get(&pr.coded, row).to_string()),
            ("route_count".into(), rs.len.to_string()),
        ];
        for r in rs.start.0 as usize..(rs.start.0 + rs.len) as usize {
            let rt = m.routes.route_type[r];
            let rname = format!(
                "route {}/{}",
                if rt == 0 {
                    "-".to_string()
                } else {
                    char::from(rt).to_string()
                },
                s(m, m.routes.transition[r])
            );
            let ls = m.routes.legs[r];
            f.push((format!("{rname} leg_count"), ls.len.to_string()));
            for i in ls.start.0 as usize..(ls.start.0 + ls.len) as usize {
                let g = &m.legs;
                let dist = if g.dist_or_time[i] == NONE_U32 {
                    "-".to_string()
                } else if bit_get(&g.dist_is_time, i) {
                    format!("{} s", g.dist_or_time[i])
                } else {
                    format!("{} m", g.dist_or_time[i])
                };
                f.push((
                    format!("{rname} leg {:03}", i - ls.start.0 as usize),
                    format!(
                        "{} fix {} centre {} turn {} course {} dist {} alt_desc {} alt1 {} alt2 {} speed {}",
                        path_term::name(g.path_term[i]),
                        point_ref(m, g.fix[i]),
                        point_ref(m, g.center[i]),
                        g.turn[i],
                        opt_u16(g.course_mag_cdeg[i]),
                        dist,
                        g.alt_desc[i],
                        alt(g.alt1_ft[i]),
                        alt(g.alt2_ft[i]),
                        opt_u16(g.speed_kt[i])
                    ),
                ));
            }
        }
        out.insert(key, f);
    }
    out
}

fn mora_objects(m: &Model) -> Objects {
    let mo = &m.mora;
    let mut out = Objects::new();
    for i in 0..mo.lat.len() {
        let v = if mo.hundreds_ft[i] == MORA_UNKNOWN {
            "UNK".to_string()
        } else {
            format!("{} ft", i32::from(mo.hundreds_ft[i]) * 100)
        };
        out.insert(
            format!("MORA cell {},{}", mo.lat[i], mo.lon[i]),
            vec![("MORA".into(), v)],
        );
    }
    out
}

fn unresolved_objects(m: &Model) -> Objects {
    let u = &m.unresolved;
    let mut out = Objects::new();
    for i in 0..u.role.len() {
        let key = format!(
            "unresolved {} {} {} #{} {}",
            if u.owner_kind[i] == owner_kind::AIRWAY {
                "airway"
            } else {
                "procedure"
            },
            s(m, u.owner[i]),
            s(m, u.scope[i]),
            u.ordinal[i],
            unresolved_role::name(u.role[i])
        );
        out.insert(
            key,
            vec![
                ("name".into(), s(m, u.name[i]).into()),
                ("region".into(), s(m, u.region[i]).into()),
                ("wanted".into(), s(m, u.wanted[i]).into()),
            ],
        );
    }
    out
}

fn disagreement_objects(m: &Model) -> Objects {
    let d = &m.disagreements;
    let mut out = Objects::new();
    for i in 0..d.point.len() {
        let key = format!("disagreement {}", point_ref(m, d.point[i]));
        out.insert(
            key,
            vec![
                ("nasr_position".into(), pos(d.nasr_lat[i], d.nasr_lon[i])),
                (
                    "distance".into(),
                    format!("{:.3} m", f64::from(d.dist_mm[i]) / 1000.0),
                ),
            ],
        );
    }
    out
}

fn compare(class: &'static str, a: &Objects, b: &Objects, out: &mut Vec<DiffEntry>) {
    let mut keys: Vec<&String> = a.keys().chain(b.keys()).collect();
    keys.sort();
    keys.dedup();
    for k in keys {
        let change = match (a.get(k), b.get(k)) {
            (Some(_), None) => Change::Removed,
            (None, Some(_)) => Change::Added,
            (Some(x), Some(y)) => {
                if x == y {
                    continue;
                }
                let xm: BTreeMap<&str, &str> =
                    x.iter().map(|(f, v)| (f.as_str(), v.as_str())).collect();
                let ym: BTreeMap<&str, &str> =
                    y.iter().map(|(f, v)| (f.as_str(), v.as_str())).collect();
                // Fields in first-seen order: the first snapshot's, then those only in the second.
                let mut names: Vec<&str> = x.iter().map(|(f, _)| f.as_str()).collect();
                for (f, _) in y {
                    if !xm.contains_key(f.as_str()) {
                        names.push(f.as_str());
                    }
                }
                let changes: Vec<FieldChange> = names
                    .into_iter()
                    .filter(|f| xm.get(f) != ym.get(f))
                    .map(|f| FieldChange {
                        field: f.to_string(),
                        from: xm.get(f).map(|v| (*v).to_string()),
                        to: ym.get(f).map(|v| (*v).to_string()),
                    })
                    .collect();
                if changes.is_empty() {
                    continue;
                }
                Change::Changed(changes)
            }
            (None, None) => continue,
        };
        out.push(DiffEntry {
            class,
            key: k.clone(),
            change,
        });
    }
}

fn header_changes(a: &Model, b: &Model) -> Vec<FieldChange> {
    let mut out = Vec::new();
    let mut push = |field: &str, x: String, y: String| {
        if x != y {
            out.push(FieldChange {
                field: field.to_string(),
                from: Some(x),
                to: Some(y),
            });
        }
    };
    push(
        "NASR effective date",
        a.meta.nasr_effective.to_string(),
        b.meta.nasr_effective.to_string(),
    );
    push(
        "CIFP cycle",
        a.meta.cifp_cycle.to_string(),
        b.meta.cifp_cycle.to_string(),
    );
    push("ARTCC", a.meta.artcc.clone(), b.meta.artcc.clone());
    push(
        "buffer (m)",
        a.meta.buffer_m.to_string(),
        b.meta.buffer_m.to_string(),
    );
    push(
        "NASR archive sha256",
        to_hex(&a.meta.nasr_sha256),
        to_hex(&b.meta.nasr_sha256),
    );
    push(
        "CIFP archive sha256",
        to_hex(&a.meta.cifp_sha256),
        to_hex(&b.meta.cifp_sha256),
    );
    out
}

/// Compares two models: added, removed and changed objects by stable key, with the changed
/// fields (OA-ASP-007). Deterministic order. Both models are validated first, so the table
/// walks below read by index safely; an inconsistent model is an error, not a diff.
pub fn diff_models(a: &Model, b: &Model) -> Result<Diff, ModelError> {
    validate_model(a)?;
    validate_model(b)?;
    let mut entries = Vec::new();
    compare("point", &point_objects(a), &point_objects(b), &mut entries);
    compare(
        "boundary",
        &boundary_objects(a),
        &boundary_objects(b),
        &mut entries,
    );
    compare(
        "airway",
        &airway_objects(a),
        &airway_objects(b),
        &mut entries,
    );
    compare(
        "procedure",
        &proc_objects(a),
        &proc_objects(b),
        &mut entries,
    );
    compare("mora", &mora_objects(a), &mora_objects(b), &mut entries);
    compare(
        "unresolved",
        &unresolved_objects(a),
        &unresolved_objects(b),
        &mut entries,
    );
    compare(
        "disagreement",
        &disagreement_objects(a),
        &disagreement_objects(b),
        &mut entries,
    );
    let mut notes = Vec::new();
    let low = |e: &DiffEntry| e.class == "boundary" && e.key.contains("ARTCC LOW");
    if entries.iter().any(low) {
        notes.push(
            "an ARTCC LOW boundary changed: the FAA plans to redefine the NASR LOW boundary as the ground boundary; check this entry"
                .to_string(),
        );
    }
    Ok(Diff {
        header: header_changes(a, b),
        entries,
        notes,
    })
}

/// Counts of (added, removed, changed) per object class, in class order.
pub fn diff_counts(d: &Diff) -> Vec<(&'static str, usize, usize, usize)> {
    let mut out: Vec<(&'static str, usize, usize, usize)> = Vec::new();
    for class in [
        "point",
        "boundary",
        "airway",
        "procedure",
        "mora",
        "unresolved",
        "disagreement",
    ] {
        let mut c = (class, 0, 0, 0);
        for e in d.entries.iter().filter(|e| e.class == class) {
            match e.change {
                Change::Added => c.1 += 1,
                Change::Removed => c.2 += 1,
                Change::Changed(_) => c.3 += 1,
            }
        }
        out.push(c);
    }
    out
}

/// Renders a diff as text. `a` and `b` label the two snapshots.
pub fn render_diff(d: &Diff, a: &str, b: &str) -> String {
    let mut o = String::new();
    let _ = writeln!(o, "snapshot diff\n  from {a}\n  to   {b}\n");
    if d.header.is_empty() {
        let _ = writeln!(o, "headers identical");
    }
    for h in &d.header {
        let _ = writeln!(
            o,
            "header {}: {} -> {}",
            h.field,
            h.from.as_deref().unwrap_or("-"),
            h.to.as_deref().unwrap_or("-")
        );
    }
    for n in &d.notes {
        let _ = writeln!(o, "NOTE: {n}");
    }
    let _ = writeln!(o, "\nsummary (added / removed / changed):");
    for (class, add, rem, chg) in diff_counts(d) {
        let _ = writeln!(o, "  {class:<13}{add:>7}{rem:>7}{chg:>7}");
    }
    let _ = writeln!(o, "\nentries: {}\n", d.entries.len());
    for e in &d.entries {
        match &e.change {
            Change::Added => {
                let _ = writeln!(o, "+ {} {}", e.class, e.key);
            }
            Change::Removed => {
                let _ = writeln!(o, "- {} {}", e.class, e.key);
            }
            Change::Changed(fields) => {
                let _ = writeln!(o, "~ {} {}", e.class, e.key);
                for f in fields {
                    let _ = writeln!(
                        o,
                        "      {}: {} -> {}",
                        f.field,
                        f.from.as_deref().unwrap_or("(absent)"),
                        f.to.as_deref().unwrap_or("(absent)")
                    );
                }
            }
        }
    }
    o
}
