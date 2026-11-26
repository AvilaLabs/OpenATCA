//! Guarded recorded-file hash comparison against the expected system sha256sum.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_sha256::{sha256_reader, to_hex};

// REQ: OA-SHA-001
#[test]
fn real_nasr_zip_matches_sha256sum() {
    let Ok(raw) = std::env::var("OPENATCA_RAW") else {
        eprintln!("skipped: OPENATCA_RAW not set");
        return;
    };
    let p = std::path::Path::new(&raw).join("nasr-2026-10-29/NASR_CSV_29_Oct_2026.zip");
    let f = oa_sealed::open(&p).unwrap();
    let t = std::time::Instant::now();
    let d = sha256_reader(f).unwrap();
    eprintln!("nasr zip hashed in {:?}", t.elapsed());
    assert_eq!(
        to_hex(&d),
        "60b072454319f166797c5f4667520e4cf9c54d2e4e5faffbcdbae846aea276da"
    );
}
