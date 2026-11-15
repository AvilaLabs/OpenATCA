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
// A7.2: cold struct, size guard waived
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
// A7.2: cold struct, size guard waived
pub struct DiffEntry {
    pub class: &'static str,
    pub key: String,
    pub change: Change,
}

/// The result of comparing two models.
#[derive(Clone, Debug, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
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

/// Column `col` at `row`. A validated model has every column the same length, so a miss is a
/// defect in this crate or an unvalidated model; it is an error, never a default (E5).
fn g<T: Copy>(col: &[T], row: usize) -> Result<T, ModelError> {
    col.get(row).copied().ok_or_else(|| ModelError {
        table: "diff",
        what: format!("column too short for row {row}"),
    })
}

/// The rows `start .. start + len` of a child table as a range, or an error when they overflow.
fn span_rows(start: usize, len: u32) -> Result<std::ops::Range<usize>, ModelError> {
    let end = start.checked_add(len as usize).ok_or_else(|| ModelError {
        table: "diff",
        what: "span overflows".to_string(),
    })?;
    Ok(start..end)
}

/// Adds an object under `key`. Two objects with one key would hide a change behind the second
/// (OA-ASP-007), so a duplicate is an error that names the key.
fn add(
    out: &mut Objects,
    class: &'static str,
    key: String,
    fields: Fields,
) -> Result<(), ModelError> {
    if out.contains_key(&key) {
        return Err(ModelError {
            table: class,
            what: format!("two objects share the diff key {key:?}"),
        });
    }
    out.insert(key, fields);
    Ok(())
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

/// A point's full identity for use in a key: the class of kind plus [`point_ref`]. Two points
/// with one identifier and region but different classes (a fix and a VOR) are different objects.
fn point_identity(m: &Model, ix: PointIx) -> String {
    match point_key(m, ix) {
        None => "-".to_string(),
        Some(k) => format!("{} {}", point_kind::name(k.class), point_ref(m, ix)),
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

fn point_objects(m: &Model) -> Result<Objects, ModelError> {
    let p = &m.points;
    let mut out = Objects::new();
    for row in 0..p.kind.len() {
        let ix = PointIx::new(row as u32);
        let Some(k) = point_key(m, ix) else { continue };
        let mut key = format!("{} {}", point_kind::name(k.class), k.ident);
        let _ = write!(key, " {}", if k.region.is_empty() { "-" } else { k.region });
        if !k.scope.is_empty() {
            let _ = write!(key, " {}", k.scope);
        }
        let flags = |b: &BitSet| bit_get(b, row).to_string();
        let f: Fields = vec![
            ("kind".into(), point_kind::name(g(&p.kind, row)?).into()),
            ("position".into(), pos(g(&p.lat, row)?, g(&p.lon, row)?)),
            ("elevation".into(), alt(g(&p.elev_ft, row)?)),
            ("alt_ident".into(), s(m, g(&p.alt_ident, row)?).into()),
            ("name".into(), s(m, g(&p.name, row)?).into()),
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
        add(&mut out, "point", key, f)?;
    }
    Ok(out)
}

fn boundary_objects(m: &Model) -> Result<Objects, ModelError> {
    let b = &m.boundaries;
    let mut out = Objects::new();
    for row in 0..b.source.len() {
        let source = match g(&b.source, row)? {
            boundary_source::NASR_ARB => "NASR-ARB",
            boundary_source::CIFP_UC => "CIFP-UC",
            _ => "CIFP-UR",
        };
        let multiple = g(&b.multiple, row)?;
        let name = s(m, g(&b.name, row)?);
        let mut key = format!(
            "{source} {} {}",
            stratum::name(g(&b.stratum, row)?),
            s(m, g(&b.facility, row)?)
        );
        if multiple != 0 {
            let _ = write!(key, " {}", char::from(multiple));
        }
        if !name.is_empty() {
            let _ = write!(key, " {name}");
        }
        let agl = |flag: &BitSet| if bit_get(flag, row) { " AGL" } else { "" };
        let mut f: Fields = vec![
            (
                "edge_interp".into(),
                if g(&b.edge_interp, row)? == edge_interp::UNKNOWN {
                    "unknown"
                } else {
                    "per segment"
                }
                .into(),
            ),
            (
                "floor".into(),
                format!("{}{}", alt(g(&b.floor_ft, row)?), agl(&b.floor_agl)),
            ),
            (
                "ceiling".into(),
                format!("{}{}", alt(g(&b.ceiling_ft, row)?), agl(&b.ceiling_agl)),
            ),
            (
                "outline".into(),
                if bit_get(&b.incomplete, row) {
                    "incomplete (no end marker)"
                } else {
                    "complete"
                }
                .into(),
            ),
            (
                "time_code".into(),
                match g(&b.time_code, row)? {
                    time_code::CONTINUOUS => "continuous",
                    time_code::PART_TIME => "part-time",
                    _ => "unknown",
                }
                .into(),
            ),
            (
                "level".into(),
                match g(&b.level, row)? {
                    level::HIGH => "high",
                    level::LOW => "low",
                    level::BOTH => "both",
                    _ => "unknown",
                }
                .into(),
            ),
        ];
        let vs = g(&b.vertices, row)?;
        let vr = span_rows(vs.start.row(), vs.len)?;
        let verts = m.vertices.get(vr).unwrap_or(&[]);
        f.push(("vertex_count".into(), verts.len().to_string()));
        for (i, v) in verts.iter().enumerate() {
            f.push((format!("vertex {i:04}"), pos(v.lat, v.lon)));
        }
        let ss = g(&b.segs, row)?;
        let gs = &m.segments;
        f.push(("segment_count".into(), ss.len.to_string()));
        for (n, i) in span_rows(ss.start.row(), ss.len)?.enumerate() {
            f.push((
                format!("segment {n:04}"),
                format!(
                    "via {} at {} centre {} radius {} m bearing {}",
                    g(&gs.via, i)?,
                    pos(g(&gs.lat, i)?, g(&gs.lon, i)?),
                    pos(g(&gs.arc_lat, i)?, g(&gs.arc_lon, i)?),
                    g(&gs.radius_m, i)?,
                    opt_u16(g(&gs.bearing_cdeg, i)?)
                ),
            ));
        }
        add(&mut out, "boundary", key, f)?;
    }
    Ok(out)
}

fn airway_objects(m: &Model) -> Result<Objects, ModelError> {
    let a = &m.airways;
    let l = &m.airway_legs;
    let mut out = Objects::new();
    for row in 0..a.designator.len() {
        let key = format!(
            "airway {} ({})",
            s(m, g(&a.designator, row)?),
            char::from(g(&a.location, row)?)
        );
        let sp = g(&a.legs, row)?;
        let mut f: Fields = vec![("leg_count".into(), sp.len.to_string())];
        let mut seen: BTreeMap<String, usize> = BTreeMap::new();
        for i in span_rows(sp.start.row(), sp.len)? {
            let from = point_ref(m, g(&l.from, i)?);
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
                    point_ref(m, g(&l.to, i)?),
                    alt(g(&l.mea_ft, i)?),
                    alt(g(&l.mea_opp_ft, i)?),
                    alt(g(&l.gps_mea_ft, i)?),
                    g(&l.mea_dir, i)?,
                    g(&l.gps_mea_dir, i)?,
                    bit_get(&l.gap, i),
                    bit_get(&l.signal_gap, i),
                    bit_get(&l.dogleg, i)
                ),
            ));
        }
        add(&mut out, "airway", key, f)?;
    }
    Ok(out)
}

fn proc_objects(m: &Model) -> Result<Objects, ModelError> {
    let pr = &m.procs;
    let mut out = Objects::new();
    for row in 0..pr.kind.len() {
        let key = format!(
            "{} {} at {}",
            proc_kind::name(g(&pr.kind, row)?),
            s(m, g(&pr.ident, row)?),
            point_ref(m, g(&pr.airport, row)?)
        );
        let rs = g(&pr.routes, row)?;
        let mut f: Fields = vec![
            ("name".into(), s(m, g(&pr.name, row)?).into()),
            (
                "sources".into(),
                sources(bit_get(&pr.src_nasr, row), bit_get(&pr.src_cifp, row)),
            ),
            ("coded".into(), bit_get(&pr.coded, row).to_string()),
            ("route_count".into(), rs.len.to_string()),
        ];
        for r in span_rows(rs.start.row(), rs.len)? {
            let rt = g(&m.routes.route_type, r)?;
            let rname = format!(
                "route {}/{}",
                if rt == 0 {
                    "-".to_string()
                } else {
                    char::from(rt).to_string()
                },
                s(m, g(&m.routes.transition, r)?)
            );
            let ls = g(&m.routes.legs, r)?;
            f.push((format!("{rname} leg_count"), ls.len.to_string()));
            for (n, i) in span_rows(ls.start.row(), ls.len)?.enumerate() {
                let lg = &m.legs;
                let dist_raw = g(&lg.dist_or_time, i)?;
                let dist = if dist_raw == NONE_U32 {
                    "-".to_string()
                } else if bit_get(&lg.dist_is_time, i) {
                    format!("{dist_raw} s")
                } else {
                    format!("{dist_raw} m")
                };
                f.push((
                    format!("{rname} leg {n:03}"),
                    format!(
                        "{} fix {} centre {} turn {} course {} dist {} alt_desc {} alt1 {} alt2 {} speed {}",
                        path_term::name(g(&lg.path_term, i)?),
                        point_ref(m, g(&lg.fix, i)?),
                        point_ref(m, g(&lg.center, i)?),
                        g(&lg.turn, i)?,
                        opt_u16(g(&lg.course_mag_cdeg, i)?),
                        dist,
                        g(&lg.alt_desc, i)?,
                        alt(g(&lg.alt1_ft, i)?),
                        alt(g(&lg.alt2_ft, i)?),
                        opt_u16(g(&lg.speed_kt, i)?)
                    ),
                ));
            }
        }
        add(&mut out, "procedure", key, f)?;
    }
    Ok(out)
}

fn mora_objects(m: &Model) -> Result<Objects, ModelError> {
    let mo = &m.mora;
    let mut out = Objects::new();
    for i in 0..mo.lat.len() {
        let h = g(&mo.hundreds_ft, i)?;
        let v = if h == MORA_UNKNOWN {
            "UNK".to_string()
        } else {
            format!("{} ft", i32::from(h) * 100)
        };
        add(
            &mut out,
            "mora",
            format!("MORA cell {},{}", g(&mo.lat, i)?, g(&mo.lon, i)?),
            vec![("MORA".into(), v)],
        )?;
    }
    Ok(out)
}

fn unresolved_objects(m: &Model) -> Result<Objects, ModelError> {
    let u = &m.unresolved;
    let mut out = Objects::new();
    for i in 0..u.role.len() {
        // The identifier and region that could not be found are part of the identity: the NASR
        // procedure ordinals restart in every route, so owner, scope, ordinal and role alone
        // can name two different references.
        let key = format!(
            "unresolved {} {} {} #{} {} {:?} {:?}",
            if g(&u.owner_kind, i)? == owner_kind::AIRWAY {
                "airway"
            } else {
                "procedure"
            },
            s(m, g(&u.owner, i)?),
            s(m, g(&u.scope, i)?),
            g(&u.ordinal, i)?,
            unresolved_role::name(g(&u.role, i)?),
            s(m, g(&u.name, i)?),
            s(m, g(&u.region, i)?),
        );
        add(
            &mut out,
            "unresolved",
            key,
            vec![
                ("name".into(), s(m, g(&u.name, i)?).into()),
                ("region".into(), s(m, g(&u.region, i)?).into()),
                ("wanted".into(), s(m, g(&u.wanted, i)?).into()),
            ],
        )?;
    }
    Ok(out)
}

fn disagreement_objects(m: &Model) -> Result<Objects, ModelError> {
    let d = &m.disagreements;
    let mut out = Objects::new();
    for i in 0..d.point.len() {
        let key = format!("disagreement {}", point_identity(m, g(&d.point, i)?));
        add(
            &mut out,
            "disagreement",
            key,
            vec![
                (
                    "nasr_position".into(),
                    pos(g(&d.nasr_lat, i)?, g(&d.nasr_lon, i)?),
                ),
                (
                    "distance".into(),
                    format!("{:.3} m", f64::from(g(&d.dist_mm, i)?) / 1000.0),
                ),
            ],
        )?;
    }
    Ok(out)
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
    push(
        "cycle override",
        a.cycle_override.clone().unwrap_or_default(),
        b.cycle_override.clone().unwrap_or_default(),
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
/// fields (OA-ASP-007). Deterministic order. Both models are validated first; an inconsistent
/// model is an error, not a diff, and so is a model in which two objects share a key.
pub fn diff_models(a: &Model, b: &Model) -> Result<Diff, ModelError> {
    validate_model(a)?;
    validate_model(b)?;
    let mut entries = Vec::new();
    compare(
        "point",
        &point_objects(a)?,
        &point_objects(b)?,
        &mut entries,
    );
    compare(
        "boundary",
        &boundary_objects(a)?,
        &boundary_objects(b)?,
        &mut entries,
    );
    compare(
        "airway",
        &airway_objects(a)?,
        &airway_objects(b)?,
        &mut entries,
    );
    compare(
        "procedure",
        &proc_objects(a)?,
        &proc_objects(b)?,
        &mut entries,
    );
    compare("mora", &mora_objects(a)?, &mora_objects(b)?, &mut entries);
    compare(
        "unresolved",
        &unresolved_objects(a)?,
        &unresolved_objects(b)?,
        &mut entries,
    );
    compare(
        "disagreement",
        &disagreement_objects(a)?,
        &disagreement_objects(b)?,
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
