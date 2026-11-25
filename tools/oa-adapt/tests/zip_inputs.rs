#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
//! CORE: private guarded archive checks, independent of the reusable codec package.
use oa_zip::Archive;
use std::path::{Path, PathBuf};
fn raw_dir() -> Option<PathBuf> {
    match std::env::var("OPENATCA_RAW") {
        Ok(p) => Some(PathBuf::from(p)),
        Err(_) => {
            eprintln!("skipped: OPENATCA_RAW not set");
            None
        }
    }
}

fn compare_zip_to_extracted(raw: &Path, zip_rel: &str, dir_rel: &str) -> (usize, u64, f64) {
    let bytes = oa_sealed::open(&raw.join(zip_rel))
        .map_err(oa_zip::ZipError::Io)
        .and_then(oa_zip::read_archive)
        .unwrap();
    let a = Archive::parse(&bytes).unwrap();
    let mut total = 0u64;
    let mut inflate_secs = 0.0f64;
    for e in a.entries() {
        let t = std::time::Instant::now();
        let got = a.extract_entry(e).unwrap();
        if e.method() == 8 {
            inflate_secs += t.elapsed().as_secs_f64();
        }
        let want = oa_sealed::read(raw.join(dir_rel).join(e.name())).unwrap();
        assert!(got == want, "{} differs", e.name());
        total += got.len() as u64;
    }
    (a.entries().len(), total, inflate_secs)
}

// REQ: OA-ZIP-001, OA-ZIP-002
#[test]
fn real_nasr_zip_matches_extracted_copy() {
    let Some(raw) = raw_dir() else { return };
    let (n, total, secs) = compare_zip_to_extracted(
        &raw,
        "nasr-2026-10-29/NASR_CSV_29_Oct_2026.zip",
        "nasr-2026-10-29/x",
    );
    assert_eq!(n, 114);
    eprintln!(
        "nasr: {n} entries, {total} bytes, {:.1} MB/s (extract+crc)",
        total as f64 / 1e6 / secs
    );
    // The nested archive is itself readable.
    let inner =
        oa_sealed::read(raw.join("nasr-2026-10-29/x/01_Oct_2026_CSV-29_Oct_2026_CSV.zip")).unwrap();
    let ia = Archive::parse(&inner).unwrap();
    for e in ia.entries() {
        ia.extract_entry(e).unwrap();
    }
}

// REQ: OA-ZIP-001, OA-ZIP-002, OA-ZIP-003
#[test]
fn real_cifp_zip_matches_extracted_copy() {
    let Some(raw) = raw_dir() else { return };
    let (n, total, secs) =
        compare_zip_to_extracted(&raw, "cifp-2026-10-29/CIFP_261029.zip", "cifp-2026-10-29/x");
    assert_eq!(n, 6);
    eprintln!(
        "cifp: {n} entries, {total} bytes, {:.1} MB/s (extract+crc)",
        total as f64 / 1e6 / secs
    );
}

// REQ: OA-SEAL-013, OA-CORE-005, OA-CORE-006
#[test]
fn private_archive_caller_refuses_a_sealed_path() {
    let e = oa_sealed::open(Path::new("/x/derived/adsb/zau-2026.11.21-1s.zip"))
        .map_err(oa_zip::ZipError::Io)
        .and_then(oa_zip::read_archive)
        .unwrap_err()
        .to_string();
    assert!(e.contains("OA-SEAL-001"), "{e}");
}
