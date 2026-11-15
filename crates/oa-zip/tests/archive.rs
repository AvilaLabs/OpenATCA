//! Integration tests for oa-zip. Fixtures are built by tests/fixtures/make_fixtures.py (Python
//! standard library only); expected extractions sit in tests/fixtures/expected/.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_zip::{
    crc32, inflate, is_safe_path, read_archive_file, Archive, InflateError, Limits, ZipError,
    DEFAULT_LIMITS, MAX_EOCD_SEARCH,
};
use std::path::{Path, PathBuf};

const STORED: &[u8] = include_bytes!("fixtures/stored.zip");
const DEFLATE: &[u8] = include_bytes!("fixtures/deflate.zip");
const MANY: &[u8] = include_bytes!("fixtures/many.zip");
const BOMB: &[u8] = include_bytes!("fixtures/bomb.zip");
const TRAVERSAL: &[u8] = include_bytes!("fixtures/traversal.zip");
const DESCRIPTOR: &[u8] = include_bytes!("fixtures/descriptor.zip");
const ZIP64: &[u8] = include_bytes!("fixtures/zip64.zip");
const ZIP64_HUGE: &[u8] = include_bytes!("fixtures/zip64_huge.zip");

fn expected(name: &str) -> Vec<u8> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/expected")
        .join(name);
    std::fs::read(p).unwrap()
}

fn find(d: &[u8], pat: &[u8], from: usize) -> usize {
    (from..d.len() - pat.len() + 1)
        .find(|&i| &d[i..i + pat.len()] == pat)
        .unwrap()
}

fn central(d: &[u8]) -> usize {
    find(d, &[b'P', b'K', 1, 2], 0)
}

fn eocd(d: &[u8]) -> usize {
    (0..d.len() - 3)
        .rev()
        .find(|&i| d[i..i + 4] == [b'P', b'K', 5, 6])
        .unwrap()
}

struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

// REQ: OA-ZIP-001
#[test]
fn stored_archive_lists_and_extracts() {
    let a = Archive::parse(STORED).unwrap();
    let names: Vec<&str> = a.entries().iter().map(|e| e.name()).collect();
    assert_eq!(names, ["a.txt", "empty.txt", "dir/"]);
    assert_eq!(a.extract("a.txt").unwrap(), b"stored contents\n");
    assert_eq!(a.extract("empty.txt").unwrap(), b"");
    assert_eq!(a.extract("dir/").unwrap(), b"");
    assert!(matches!(a.extract("nope"), Err(ZipError::NameNotFound(_))));
}

// REQ: OA-ZIP-001, OA-ZIP-002
#[test]
fn deflate_archive_every_entry_matches() {
    let a = Archive::parse(DEFLATE).unwrap();
    assert_eq!(a.entries().len(), 8);
    for e in a.entries() {
        assert_eq!(e.method(), 8, "{}", e.name());
        assert_eq!(
            a.extract(e.name()).unwrap(),
            expected(e.name()),
            "{}",
            e.name()
        );
    }
}

// REQ: OA-ZIP-002
#[test]
fn block_types_all_exercised() {
    // zlib emits fixed blocks for tiny text, dynamic for larger text, stored blocks at level 0.
    let a = Archive::parse(DEFLATE).unwrap();
    let first_byte_type = |name: &str| {
        let e = a.entries().iter().find(|e| e.name() == name).unwrap();
        let off = e.local_offset() as usize;
        let n = u16::from_le_bytes([DEFLATE[off + 26], DEFLATE[off + 27]]) as usize;
        let x = u16::from_le_bytes([DEFLATE[off + 28], DEFLATE[off + 29]]) as usize;
        (DEFLATE[off + 30 + n + x] >> 1) & 3
    };
    assert_eq!(first_byte_type("fixed.txt"), 1);
    assert_eq!(first_byte_type("dynamic.txt"), 2);
    assert_eq!(first_byte_type("storedblock.bin"), 0);
}

// REQ: OA-ZIP-001, OA-ZIP-003
#[test]
fn many_entries() {
    let a = Archive::parse(MANY).unwrap();
    assert_eq!(a.entries().len(), 300);
    for i in [0usize, 1, 150, 299] {
        let want = format!("entry {i}\n").repeat(i % 7 + 1);
        assert_eq!(a.extract(&format!("f{i:03}.txt")).unwrap(), want.as_bytes());
    }
    let tight = Limits {
        max_entries: 299,
        ..DEFAULT_LIMITS
    };
    assert!(matches!(
        Archive::parse_with_limits(MANY, tight),
        Err(ZipError::Limit {
            name: "MAX_ENTRIES",
            ..
        })
    ));
}

// REQ: OA-ZIP-002
#[test]
fn bad_crc_rejected() {
    let mut d = STORED.to_vec();
    let at = find(&d, b"stored contents", 0);
    d[at] ^= 0x20;
    let a = Archive::parse(&d).unwrap();
    assert!(matches!(
        a.extract("a.txt"),
        Err(ZipError::CrcMismatch { .. })
    ));
}

// REQ: OA-ZIP-002
#[test]
fn wrong_declared_size_rejected() {
    // Central-directory uncompressed size too small for a deflate entry (local header kept in
    // step): the stream inflates to more than declared.
    let mut d = DEFLATE.to_vec();
    let a = Archive::parse(&d).unwrap();
    let e = a
        .entries()
        .iter()
        .find(|e| e.name() == "dynamic.txt")
        .unwrap()
        .clone();
    let cd = find(&d, b"dynamic.txt", central(&d)) - 46;
    d[cd + 24..cd + 28].copy_from_slice(&(e.uncompressed_size() as u32 - 1).to_le_bytes());
    let off = e.local_offset() as usize;
    d[off + 22..off + 26].copy_from_slice(&(e.uncompressed_size() as u32 - 1).to_le_bytes());
    let a = Archive::parse(&d).unwrap();
    assert!(matches!(
        a.extract("dynamic.txt"),
        Err(ZipError::SizeMismatch { .. })
    ));
    // Too large a declared size: the stream ends early.
    let mut d = DEFLATE.to_vec();
    d[cd + 24..cd + 28].copy_from_slice(&(e.uncompressed_size() as u32 + 5).to_le_bytes());
    d[off + 22..off + 26].copy_from_slice(&(e.uncompressed_size() as u32 + 5).to_le_bytes());
    let a = Archive::parse(&d).unwrap();
    assert!(matches!(
        a.extract("dynamic.txt"),
        Err(ZipError::SizeMismatch { .. })
    ));
}

// REQ: OA-ZIP-001
#[test]
fn truncated_archive_rejected_at_every_length() {
    // Cutting the archive anywhere removes (part of) the end-of-central-directory record, which
    // is the last thing in it, so every prefix is refused for that reason.
    for len in 0..DEFLATE.len() {
        let r = Archive::parse(&DEFLATE[..len]);
        assert!(
            matches!(r, Err(ZipError::NoEndOfCentralDirectory)),
            "prefix of {len} bytes: {r:?}"
        );
    }
    assert!(matches!(
        Archive::parse(&[]),
        Err(ZipError::NoEndOfCentralDirectory)
    ));
}

// REQ: OA-ZIP-003
#[test]
fn zip_bomb_rejected_by_ratio() {
    match Archive::parse(BOMB) {
        Err(ZipError::Limit {
            name: "MAX_RATIO", ..
        }) => {}
        other => panic!("expected ratio limit, got {other:?}"),
    }
    // Lifting the ratio limit shows the archive itself is well formed.
    let lax = Limits {
        max_ratio: 100_000,
        ..DEFAULT_LIMITS
    };
    let a = Archive::parse_with_limits(BOMB, lax).unwrap();
    assert_eq!(a.extract("zeros.bin").unwrap().len(), 10_000_000);
}

// REQ: OA-ZIP-003
#[test]
fn size_limits_enforced() {
    let entry = Limits {
        max_entry_bytes: 50_000,
        ..DEFAULT_LIMITS
    };
    assert!(matches!(
        Archive::parse_with_limits(DEFLATE, entry),
        Err(ZipError::Limit {
            name: "MAX_ENTRY_BYTES",
            ..
        })
    ));
    let total = Limits {
        max_total_bytes: 100_000,
        ..DEFAULT_LIMITS
    };
    assert!(matches!(
        Archive::parse_with_limits(DEFLATE, total),
        Err(ZipError::Limit {
            name: "MAX_TOTAL_BYTES",
            ..
        })
    ));
    let name = Limits {
        max_name_bytes: 3,
        ..DEFAULT_LIMITS
    };
    assert!(matches!(
        Archive::parse_with_limits(STORED, name),
        Err(ZipError::Limit {
            name: "MAX_NAME_BYTES",
            ..
        })
    ));
}

// REQ: OA-ZIP-003
#[test]
fn a_lying_stream_cannot_exceed_declared_size() {
    // A deflate stream of 100 000 'a' with the declared size patched to 10: output is capped.
    let mut d = DEFLATE.to_vec();
    let a = Archive::parse(&d).unwrap();
    let e = a
        .entries()
        .iter()
        .find(|e| e.name() == "run.bin")
        .unwrap()
        .clone();
    let cd = find(&d, b"run.bin", central(&d)) - 46;
    d[cd + 24..cd + 28].copy_from_slice(&10u32.to_le_bytes());
    let off = e.local_offset() as usize;
    d[off + 22..off + 26].copy_from_slice(&10u32.to_le_bytes());
    let a = Archive::parse(&d).unwrap();
    assert!(matches!(
        a.extract("run.bin"),
        Err(ZipError::SizeMismatch { .. })
    ));
}

// REQ: OA-ZIP-001
#[test]
fn traversal_names_listed_but_never_extracted() {
    let a = Archive::parse(TRAVERSAL).unwrap();
    assert_eq!(a.entries().len(), 6);
    for n in [
        "../evil.txt",
        "/abs.txt",
        "a/../../b.txt",
        "C:\\x.txt",
        "..\\win.txt",
    ] {
        assert!(!is_safe_path(n), "{n}");
        assert!(matches!(a.extract(n), Err(ZipError::UnsafePath(_))), "{n}");
    }
    assert_eq!(a.extract("ok.txt").unwrap(), b"x");
    assert!(is_safe_path("dir/sub/file.csv"));
    assert!(is_safe_path("a..b/c"));
    assert!(!is_safe_path(""));
    assert!(!is_safe_path("a\0b"));
}

// REQ: OA-ZIP-001
#[test]
fn encrypted_multidisk_and_unsupported_method_rejected() {
    let mut d = STORED.to_vec();
    let cd = central(&d);
    d[cd + 8] |= 1;
    assert!(matches!(Archive::parse(&d), Err(ZipError::Encrypted(_))));

    let mut d = STORED.to_vec();
    d[cd + 8] |= 1 << 6;
    assert!(matches!(Archive::parse(&d), Err(ZipError::Encrypted(_))));

    let mut d = STORED.to_vec();
    d[cd + 10] = 99; // AES marker method
    assert!(matches!(Archive::parse(&d), Err(ZipError::Encrypted(_))));

    let mut d = STORED.to_vec();
    d[cd + 10] = 12; // bzip2
    assert!(matches!(
        Archive::parse(&d),
        Err(ZipError::UnsupportedMethod { method: 12, .. })
    ));

    let mut d = STORED.to_vec();
    let e = eocd(&d);
    d[e + 4] = 1; // this disk number
    assert!(matches!(Archive::parse(&d), Err(ZipError::MultiDisk)));

    let mut d = STORED.to_vec();
    d[cd + 34] = 1; // entry starts on disk 1
    assert!(matches!(Archive::parse(&d), Err(ZipError::MultiDisk)));

    // Encryption flag set only in the local header is caught at extraction.
    let mut d = STORED.to_vec();
    let l = find(&d, &[b'P', b'K', 3, 4], 0);
    d[l + 6] |= 1;
    let a = Archive::parse(&d).unwrap();
    assert!(matches!(a.extract("a.txt"), Err(ZipError::Encrypted(_))));
}

// REQ: OA-ZIP-001
#[test]
fn local_header_disagreeing_with_directory_rejected() {
    let mut d = STORED.to_vec();
    let l = find(&d, &[b'P', b'K', 3, 4], 0);
    d[l + 14] ^= 1; // local crc
    let a = Archive::parse(&d).unwrap();
    assert!(matches!(a.extract("a.txt"), Err(ZipError::Inconsistent(_))));

    let mut d = STORED.to_vec();
    d[l + 30] = b'b'; // local name differs
    let a = Archive::parse(&d).unwrap();
    assert!(matches!(a.extract("a.txt"), Err(ZipError::Inconsistent(_))));
}

// REQ: OA-ZIP-001
#[test]
fn data_descriptors_accepted_and_mismatch_rejected() {
    let a = Archive::parse(DESCRIPTOR).unwrap();
    assert!(a.entries().iter().all(|e| e.flags() & 8 != 0));
    assert_eq!(a.extract("streamed.txt").unwrap(), expected("streamed.txt"));
    assert_eq!(
        a.extract("streamed2.bin").unwrap(),
        expected("streamed2.bin")
    );

    // Corrupt the descriptor's CRC of the stored entry (signature PK\x07\x08 then crc).
    let mut d = DESCRIPTOR.to_vec();
    let last = (0..d.len() - 3)
        .rev()
        .find(|&i| d[i..i + 4] == [b'P', b'K', 7, 8])
        .unwrap();
    d[last + 4] ^= 0xff;
    let a = Archive::parse(&d).unwrap();
    assert!(matches!(
        a.extract("streamed2.bin"),
        Err(ZipError::DataDescriptorMismatch(_))
    ));
}

// REQ: OA-ZIP-001
#[test]
fn zip64_read_and_limits() {
    let a = Archive::parse(ZIP64).unwrap();
    assert!(a.entries()[0].is_zip64());
    assert_eq!(a.extract("big64.txt").unwrap(), expected("big64.txt"));
    // A zip64 entry declaring 5 GiB exceeds MAX_ENTRY_BYTES; nothing is allocated for it.
    assert!(matches!(
        Archive::parse(ZIP64_HUGE),
        Err(ZipError::Limit {
            name: "MAX_ENTRY_BYTES",
            ..
        })
    ));
    // Zip64 EOCD without its locator is refused.
    let mut d = ZIP64.to_vec();
    let loc = find(&d, &[b'P', b'K', 6, 7], 0);
    d[loc + 2] = 0;
    assert!(matches!(Archive::parse(&d), Err(ZipError::Zip64(_))));
}

// REQ: OA-ZIP-001, OA-GEN-003
#[test]
fn eocd_search_is_bounded() {
    let mut d = vec![0u8; MAX_EOCD_SEARCH + 100];
    d.extend_from_slice(STORED);
    // Valid archive preceded by junk: the directory offsets point at the wrong bytes.
    assert!(matches!(
        Archive::parse(&d),
        Err(ZipError::BadSignature("central directory"))
    ));
    // A comment longer than the search window pushes the record out of reach.
    let mut d = STORED.to_vec();
    let e = eocd(&d);
    d[e + 20] = 0xff;
    d[e + 21] = 0xff;
    d.extend(std::iter::repeat_n(b'c', 65535));
    d.extend_from_slice(&[0u8; 10]);
    assert!(matches!(
        Archive::parse(&d),
        Err(ZipError::NoEndOfCentralDirectory)
    ));
    // A maximal comment is still found.
    let mut d = STORED.to_vec();
    let e = eocd(&d);
    d[e + 20] = 0xff;
    d[e + 21] = 0xff;
    d.extend(std::iter::repeat_n(b'c', 65535));
    let plain = Archive::parse(STORED).unwrap().entries().len();
    assert_eq!(Archive::parse(&d).unwrap().entries().len(), plain);
}

// REQ: OA-ZIP-002
#[test]
fn inflate_rejects_each_kind_of_malformed_stream_with_its_error() {
    // Reserved block type 3.
    assert_eq!(
        inflate(&[0x07], 0, 1 << 20),
        Err(InflateError::ReservedBlockType)
    );
    // Stored block whose LEN and NLEN are not complements.
    assert_eq!(
        inflate(&[0x01, 5, 0, 0, 0], 0, 1 << 20),
        Err(InflateError::StoredLengthMismatch)
    );
    // Stored block of 5 bytes with 1 present.
    assert_eq!(
        inflate(&[0x01, 5, 0, 0xfa, 0xff, b'a'], 0, 1 << 20),
        Err(InflateError::Truncated)
    );
    // The same stored block, fully present, against an output limit of 2.
    assert_eq!(
        inflate(
            &[0x01, 5, 0, 0xfa, 0xff, b'a', b'b', b'c', b'd', b'e'],
            0,
            2
        ),
        Err(InflateError::OutputLimit { max_output: 2 })
    );
    // A fixed-Huffman block that copies from before the start of the output.
    assert_eq!(
        inflate(&[0x03, 0x02, 0x00, 0x00], 0, 1 << 20),
        Err(InflateError::DistanceTooFar)
    );
    // Empty input.
    assert_eq!(inflate(&[], 0, 1 << 20), Err(InflateError::Truncated));
}

// REQ: OA-ZIP-002, OA-GEN-004
#[test]
fn inflate_random_input_never_panics() {
    let mut r = XorShift(0x9e3779b97f4a7c15);
    for _ in 0..3000 {
        let n = (r.next() % 200) as usize;
        let buf: Vec<u8> = (0..n).map(|_| r.next() as u8).collect();
        let _ = inflate(&buf, 0, 1 << 20);
    }
}

// REQ: OA-GEN-004, OA-ZIP-001, OA-ZIP-002
#[test]
fn mutated_archives_never_panic() {
    let seeds: [&[u8]; 6] = [STORED, DEFLATE, DESCRIPTOR, ZIP64, TRAVERSAL, MANY];
    let mut r = XorShift(0x2545f4914f6cdd1d);
    let mut rejected = 0usize;
    for (si, seed) in seeds.iter().enumerate() {
        for iter in 0..1500 {
            let mut d = seed.to_vec();
            match r.next() % 4 {
                0 => {
                    let n = (r.next() as usize) % d.len();
                    d.truncate(n);
                }
                1 => {
                    // Concentrate flips on the headers at both ends, where parsing logic lives.
                    for _ in 0..(r.next() % 4 + 1) {
                        let tail = d.len().saturating_sub(300);
                        let i = tail + (r.next() as usize) % (d.len() - tail);
                        d[i] ^= 1 << (r.next() % 8);
                    }
                }
                _ => {
                    for _ in 0..(r.next() % 6 + 1) {
                        let i = (r.next() as usize) % d.len();
                        d[i] = r.next() as u8;
                    }
                }
            }
            match Archive::parse(&d) {
                Ok(a) => {
                    for e in a.entries() {
                        // Extraction either verifies (size and CRC) or names what is wrong.
                        match a.extract_entry(e) {
                            Ok(v) => {
                                assert_eq!(v.len() as u64, e.uncompressed_size());
                                assert_eq!(crc32(&v), e.crc32());
                            }
                            Err(err) => {
                                assert!(!matches!(err, ZipError::Io(_)), "seed {si}/{iter}: {err}");
                                rejected += 1;
                            }
                        }
                    }
                }
                Err(err) => {
                    assert!(!matches!(err, ZipError::Io(_)), "seed {si}/{iter}: {err}");
                    rejected += 1;
                }
            }
        }
    }
    // Most mutations damage something the reader checks; a reader that accepted everything
    // would fail here.
    assert!(
        rejected > 2000,
        "only {rejected} of 9000 mutations rejected"
    );
}

// REQ: OA-ZIP-002
#[test]
fn crc32_known_vector_via_public_api() {
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
}

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
    let bytes = read_archive_file(&raw.join(zip_rel)).unwrap();
    let a = Archive::parse(&bytes).unwrap();
    let mut total = 0u64;
    let mut inflate_secs = 0.0f64;
    for e in a.entries() {
        let t = std::time::Instant::now();
        let got = a.extract_entry(e).unwrap();
        if e.method() == 8 {
            inflate_secs += t.elapsed().as_secs_f64();
        }
        let want = std::fs::read(raw.join(dir_rel).join(e.name())).unwrap();
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
        std::fs::read(raw.join("nasr-2026-10-29/x/01_Oct_2026_CSV-29_Oct_2026_CSV.zip")).unwrap();
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

// REQ: OA-ZIP-004
#[test]
fn a_zip64_extra_field_must_match_the_fields_that_ask_for_it() {
    // The fixture's entry has 0xFFFFFFFF in the compressed and uncompressed size fields and a
    // zip64 extra field of two 8-byte values.
    let cd = central(ZIP64);
    let is_mismatch = |d: &[u8]| matches!(Archive::parse(d), Err(ZipError::Zip64(w)) if w.contains("zip64 extra"));
    // The extra field's declared size is changed to one value.
    let mut d = ZIP64.to_vec();
    let extra = cd + 46 + 9;
    assert_eq!(&d[extra..extra + 4], &[1, 0, 16, 0]);
    d[extra + 2] = 8;
    assert!(is_mismatch(&d));
    // One size field no longer asks for the extra, which still holds both values.
    let mut d = ZIP64.to_vec();
    d[cd + 24..cd + 28].copy_from_slice(&5u32.to_le_bytes());
    assert!(is_mismatch(&d));
    // Neither asks for it.
    d[cd + 20..cd + 24].copy_from_slice(&5u32.to_le_bytes());
    assert!(is_mismatch(&d));
    // The unmodified fixture is still accepted.
    assert!(Archive::parse(ZIP64).unwrap().entries()[0].is_zip64());
}

/// A stored archive of empty entries with the given names (no data, CRC 0).
fn empty_entries_zip(names: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut offsets = Vec::new();
    for n in names {
        offsets.push(out.len() as u32);
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0, 0]); // version, flags, method, time, date
        out.extend_from_slice(&[0u8; 12]); // crc, sizes
        out.extend_from_slice(&(n.len() as u16).to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(n);
    }
    let cd_start = out.len();
    for (n, off) in names.iter().zip(&offsets) {
        out.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        out.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        out.extend_from_slice(&[0u8; 12]);
        out.extend_from_slice(&(n.len() as u16).to_le_bytes());
        out.extend_from_slice(&[0u8; 8]); // extra, comment, disk, internal attributes
        out.extend_from_slice(&[0u8; 4]); // external attributes
        out.extend_from_slice(&off.to_le_bytes());
        out.extend_from_slice(n);
    }
    let cd_len = (out.len() - cd_start) as u32;
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0u8; 4]);
    out.extend_from_slice(&(names.len() as u16).to_le_bytes());
    out.extend_from_slice(&(names.len() as u16).to_le_bytes());
    out.extend_from_slice(&cd_len.to_le_bytes());
    out.extend_from_slice(&(cd_start as u32).to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    out
}

// REQ: OA-ZIP-061
#[test]
fn duplicate_names_are_found_exactly_and_cheaply() {
    let n = |s: &str| s.as_bytes().to_vec();
    // Exact repeats are refused, the first in directory order; near names are not repeats.
    let d = empty_entries_zip(&[n("a"), n("b"), n("A"), n("a "), n("b"), n("a")]);
    assert!(matches!(Archive::parse(&d), Err(ZipError::DuplicateName(x)) if x == "b"));
    let d = empty_entries_zip(&[n("a"), n("b"), n("A"), n("a ")]);
    assert_eq!(Archive::parse(&d).unwrap().entries().len(), 4);
    // Ten thousand long names with a common prefix, distinct only in the last three bytes.
    let mut names = Vec::new();
    for i in 0..10_000usize {
        let mut v = vec![b'p'; 4093];
        v.extend_from_slice(&[
            b'a' + (i % 26) as u8,
            b'a' + (i / 26 % 26) as u8,
            b'a' + (i / 676) as u8,
        ]);
        names.push(v);
    }
    let d = empty_entries_zip(&names);
    let t = std::time::Instant::now();
    let a = Archive::parse(&d).unwrap();
    assert_eq!(a.entries().len(), 10_000);
    assert!(t.elapsed().as_secs_f64() < 3.0, "took {:?}", t.elapsed());
    // And one repeat at the very end is still found.
    names[9_999] = names[0].clone();
    let d = empty_entries_zip(&names);
    assert!(matches!(
        Archive::parse(&d),
        Err(ZipError::DuplicateName(_))
    ));
}
