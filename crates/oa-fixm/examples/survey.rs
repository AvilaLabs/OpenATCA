//! Offline survey of SFDPS samples: `survey <dir>`. Prints structure counts only. Shell tool.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
use oa_xml::{span_bytes, Doc, ElemIx, Limits, Parser};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn collect(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    oa_sealed::refuse(dir).map_err(|e| format!("survey: {e}"))?;
    let mut v: Vec<PathBuf> = Vec::new();
    // seal-allow: the directory was refused by the guard above; each subdirectory is recursed through the same check
    for e in std::fs::read_dir(dir).map_err(|e| format!("survey: {}: {e}", dir.display()))? {
        v.push(
            e.map_err(|e| format!("survey: {}: {e}", dir.display()))?
                .path(),
        );
    }
    v.sort();
    for p in v {
        if p.is_dir() {
            collect(&p, out)?;
        } else if p.extension().is_some_and(|e| e == "msg") {
            out.push(p);
        }
    }
    Ok(())
}

fn s(d: &Doc<'_>, sp: oa_xml::Span) -> String {
    String::from_utf8_lossy(span_bytes(d.input, sp).unwrap()).into_owned()
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}

/// The survey. An error is returned, not unwrapped, so the drop count of the files done so far
/// is still printed when `run` returns.
fn run() -> Result<(), String> {
    // Printed when `main` returns, also on a panic: the messages left out for their own time.
    let mut drops = oa_sealed::DropReport::new();
    let window = oa_sealed::sealed_window_ms();
    let dir = std::env::args()
        .nth(1)
        .ok_or_else(|| "usage: survey <dir>".to_string())?;
    let mut files = Vec::new();
    collect(Path::new(&dir), &mut files)?;
    let mut parser = Parser::new(Limits::default()).map_err(|e| format!("survey: {e:?}"))?;
    // path -> (messages containing, occurrences, attribute names)
    let mut paths: BTreeMap<String, (u64, u64, BTreeSet<String>)> = BTreeMap::new();
    let mut src: BTreeMap<String, u64> = BTreeMap::new();
    let mut centre: BTreeMap<String, u64> = BTreeMap::new();
    let mut msgs_per_file: BTreeMap<usize, u64> = BTreeMap::new();
    let mut src_paths: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
    let mut uoms: BTreeMap<String, u64> = BTreeMap::new();
    let mut root_types: BTreeMap<String, u64> = BTreeMap::new();
    for f in &files {
        let bytes = oa_sealed::read(f).map_err(|e| format!("survey: {}: {e}", f.display()))?;
        let d = parser
            .parse(&bytes)
            .map_err(|e| format!("survey: {}: {e:?}", f.display()))?;
        let n = d.elem_name.len();
        // A message whose own timestamp or position time is in the sealed window is left out of
        // every count below (OA-SEAL-014): found first, so no count has seen it.
        let mut sealed: Vec<bool> = Vec::new();
        {
            let mut of = vec![u32::MAX; n];
            for i in 0..n {
                let p = d.elem_parent[i];
                if p == ElemIx::NONE {
                    continue;
                }
                of[i] = if d.elem_parent[p.0 as usize] == ElemIx::NONE {
                    sealed.push(false);
                    (sealed.len() - 1) as u32
                } else {
                    of[p.0 as usize]
                };
                let first = d.elem_first_attr[i].0 as usize;
                for a in first..first + d.elem_attr_count[i] as usize {
                    let an = s(&d, d.attr_name[a]);
                    if an == "timestamp" || an == "positionTime" {
                        let t = oa_fixm::parse_iso_ms(s(&d, d.attr_value[a]).as_bytes());
                        if window.is_some_and(|(a, b)| t.is_some_and(|t| a <= t && t < b)) {
                            sealed[of[i] as usize] = true;
                        }
                    }
                }
            }
        }
        // Added as each file finishes its scan, so a later file's error keeps this count.
        drops.add(sealed.iter().filter(|x| **x).count() as u64);
        // Message of each element: the child of the root.
        let mut msg_of = vec![u32::MAX; n];
        let mut nmsg = 0;
        let mut msg_src: Vec<String> = Vec::new();
        let mut path_of: Vec<String> = vec![String::new(); n];
        let mut seen: Vec<BTreeSet<String>> = Vec::new();
        for i in 0..n {
            let name = s(&d, d.elem_name[i]);
            let p = d.elem_parent[i];
            let (path, m) = if p == ElemIx::NONE {
                *root_types.entry(name.clone()).or_default() += 1;
                (name.clone(), u32::MAX)
            } else if d.elem_parent[p.0 as usize] == ElemIx::NONE {
                nmsg += 1;
                msg_src.push(String::new());
                seen.push(BTreeSet::new());
                (String::from("message"), (nmsg - 1) as u32)
            } else {
                let pp = &path_of[p.0 as usize];
                (format!("{pp}/{name}"), msg_of[p.0 as usize])
            };
            path_of[i] = path.clone();
            msg_of[i] = m;
            if m != u32::MAX && sealed.get(m as usize) == Some(&true) {
                continue;
            }
            let first = d.elem_first_attr[i].0 as usize;
            let cnt = d.elem_attr_count[i] as usize;
            let e = paths.entry(path.clone()).or_default();
            e.1 += 1;
            if m != u32::MAX && seen[m as usize].insert(path.clone()) {
                e.0 += 1;
            }
            for a in first..first + cnt {
                let an = s(&d, d.attr_name[a]);
                let av = s(&d, d.attr_value[a]);
                e.2.insert(an.clone());
                if path == "message/flight" && an == "source" {
                    msg_src[m as usize] = av.clone();
                    *src.entry(av.clone()).or_default() += 1;
                }
                if path == "message/flight" && an == "centre" {
                    *centre.entry(av.clone()).or_default() += 1;
                }
                if an == "uom" {
                    *uoms.entry(format!("{path}={av}")).or_default() += 1;
                }
            }
        }
        *msgs_per_file
            .entry(nmsg - sealed.iter().filter(|x| **x).count())
            .or_default() += 1;
        for (m, set) in seen.iter().enumerate() {
            for p in set {
                *src_paths
                    .entry(p.clone())
                    .or_default()
                    .entry(msg_src[m].clone())
                    .or_default() += 1;
            }
        }
    }
    println!("files {}", files.len());
    println!("roots {root_types:?}");
    println!("messages per file {msgs_per_file:?}");
    println!("source {src:?}");
    println!("centre {centre:?}");
    println!("uoms {uoms:#?}");
    for (p, (m, o, a)) in &paths {
        println!(
            "PATH\t{m}\t{o}\t{p}\t{}",
            a.iter().cloned().collect::<Vec<_>>().join(",")
        );
    }
    for (p, by) in &src_paths {
        println!("SRC\t{p}\t{by:?}");
    }
    Ok(())
}
