//! Whole-corpus test against real FAA SWIM messages. Needs `OPENATCA_SCDS` (a directory searched
//! recursively for `*.msg` files) and skips when it is unset. The data is never copied into the
//! repository.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_xml::{Limits, NsIx, Parser};
use std::path::{Path, PathBuf};
use std::time::Instant;

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    // Sealed directories are skipped and counted, never descended into (OA-SEAL-012).
    let walk = oa_sealed::walk_unsealed(dir).unwrap();
    eprintln!("sealed entries skipped: {}", walk.skipped);
    out.extend(
        walk.files
            .into_iter()
            .filter(|p| p.extension().is_some_and(|e| e == "msg")),
    );
}

// REQ: OA-XML-020
#[test]
fn every_real_swim_message_parses() {
    let Some(dir) = std::env::var_os("OPENATCA_SCDS") else {
        eprintln!("OPENATCA_SCDS is not set: skipping the real SWIM data test");
        return;
    };
    let mut files = Vec::new();
    collect(Path::new(&dir), &mut files);
    assert!(
        !files.is_empty(),
        "no *.msg files under {}",
        dir.to_string_lossy()
    );
    let data: Vec<Vec<u8>> = files.iter().map(|f| oa_sealed::read(f).unwrap()).collect();
    let total: usize = data.iter().map(Vec::len).sum();

    let mut parser = Parser::new(Limits::default()).unwrap();
    let mut failed = 0usize;
    let mut max_depth = 0u32;
    let mut max_elems = 0usize;
    let mut depth: Vec<u32> = Vec::new();
    // Namespace bindings in scope at an element: the length of its declaration chain.
    let mut max_ns = 0usize;
    for (f, bytes) in files.iter().zip(&data) {
        match parser.parse(bytes) {
            Ok(d) => {
                max_elems = max_elems.max(d.elem_name.len());
                depth.clear();
                for head in d.elem_ns_head {
                    let mut n = 0usize;
                    let mut row = *head;
                    while row != NsIx::NONE {
                        n += 1;
                        row = d.ns_next[row.0 as usize];
                    }
                    max_ns = max_ns.max(n);
                }
                for p in d.elem_parent {
                    let v = if p.0 == u32::MAX {
                        1
                    } else {
                        depth[p.0 as usize] + 1
                    };
                    max_depth = max_depth.max(v);
                    depth.push(v);
                }
            }
            Err(e) => {
                failed += 1;
                if failed <= 10 {
                    eprintln!("FAIL {}: {e}", f.display());
                }
            }
        }
    }

    // Timed pass: parse only, results discarded.
    let rounds = 5;
    let t = Instant::now();
    let mut sink = 0usize;
    for _ in 0..rounds {
        for bytes in &data {
            if let Ok(d) = parser.parse(bytes) {
                sink += d.elem_name.len();
            }
        }
    }
    let secs = t.elapsed().as_secs_f64();
    let mbps = (total * rounds) as f64 / 1e6 / secs;
    eprintln!(
        "real data: {} files, {} bytes, failed {failed}, max depth {max_depth}, \
         max elements {max_elems}, max ns bindings {max_ns}, {mbps:.0} MB/s ({} msg/s) [{sink}]",
        files.len(),
        total,
        ((files.len() * rounds) as f64 / secs) as u64,
    );
    assert_eq!(failed, 0, "{failed} real messages failed to parse");
    // The default binding limit stays at least 4x the corpus maximum.
    assert!(
        4 * max_ns <= Limits::default().max_ns_bindings,
        "max ns bindings {max_ns} is within 4x of the default limit"
    );
}
