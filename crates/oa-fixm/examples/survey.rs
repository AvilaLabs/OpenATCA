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

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    v.sort();
    for p in v {
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().is_some_and(|e| e == "msg") {
            out.push(p);
        }
    }
}

fn s(d: &Doc<'_>, sp: oa_xml::Span) -> String {
    String::from_utf8_lossy(span_bytes(d.input, sp).unwrap()).into_owned()
}

fn main() {
    let dir = std::env::args().nth(1).unwrap();
    let mut files = Vec::new();
    collect(Path::new(&dir), &mut files);
    let mut parser = Parser::new(Limits::default()).unwrap();
    // path -> (messages containing, occurrences, attribute names)
    let mut paths: BTreeMap<String, (u64, u64, BTreeSet<String>)> = BTreeMap::new();
    let mut src: BTreeMap<String, u64> = BTreeMap::new();
    let mut centre: BTreeMap<String, u64> = BTreeMap::new();
    let mut msgs_per_file: BTreeMap<usize, u64> = BTreeMap::new();
    let mut src_paths: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
    let mut uoms: BTreeMap<String, u64> = BTreeMap::new();
    let mut root_types: BTreeMap<String, u64> = BTreeMap::new();
    for f in &files {
        let bytes = std::fs::read(f).unwrap();
        let d = parser.parse(&bytes).unwrap();
        let n = d.elem_name.len();
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
        *msgs_per_file.entry(nmsg).or_default() += 1;
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
}
