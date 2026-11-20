//! Extract every entry of an archive to memory and print totals; used for throughput and peak
//! memory measurements: `cargo build --release -p oa-zip --examples`, then run under
//! `/usr/bin/time -v`. Never writes files.

use oa_zip::{read_archive_file, Archive};
use std::path::Path;

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: extract_all <archive.zip>");
        std::process::exit(2);
    };
    if let Err(e) = oa_sealed::refuse(Path::new(&path)) {
        eprintln!("extract_all: {e}");
        std::process::exit(2);
    }
    let bytes = match read_archive_file(Path::new(&path)) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let archive = match Archive::parse(&bytes) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let start = std::time::Instant::now();
    let mut total = 0u64;
    for e in archive.entries() {
        match archive.extract_entry(e) {
            Ok(v) => total += v.len() as u64, // dropped immediately: peak is one entry
            Err(err) => {
                eprintln!("{}: {err}", e.name());
                std::process::exit(1);
            }
        }
    }
    let secs = start.elapsed().as_secs_f64();
    println!(
        "{} entries, {} bytes, {:.3} s, {:.1} MB/s",
        archive.entries().len(),
        total,
        secs,
        total as f64 / 1e6 / secs
    );
}
