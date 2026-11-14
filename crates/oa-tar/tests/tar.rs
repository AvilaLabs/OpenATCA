//! Tests for oa-tar. Archives are built here by a small writer (no tar binary needed).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_tar::{Data, Limits, Next, Stage, TarError, TarReader, Truncated, BLOCK, DEFAULT_LIMITS};
use std::io::{self, Cursor, Read};

fn put(h: &mut [u8; BLOCK], at: usize, bytes: &[u8]) {
    h[at..at + bytes.len()].copy_from_slice(bytes);
}

fn header(name: &str, size: u64, typeflag: u8, prefix: &str, magic: &[u8]) -> [u8; BLOCK] {
    let mut h = [0u8; BLOCK];
    put(&mut h, 0, name.as_bytes());
    put(&mut h, 100, b"0000644\0");
    put(&mut h, 108, b"0000000\0");
    put(&mut h, 116, b"0000000\0");
    put(&mut h, 124, format!("{size:011o}\0").as_bytes());
    put(&mut h, 136, b"14000000000\0");
    h[156] = typeflag;
    put(&mut h, 257, magic);
    put(&mut h, 345, prefix.as_bytes());
    seal(&mut h);
    h
}

fn seal(h: &mut [u8; BLOCK]) {
    put(h, 148, b"        ");
    let sum: u32 = h.iter().map(|&b| u32::from(b)).sum();
    put(h, 148, format!("{sum:06o}\0 ").as_bytes());
}

fn entry(out: &mut Vec<u8>, name: &str, data: &[u8]) {
    out.extend_from_slice(&header(name, data.len() as u64, b'0', "", b"ustar\x0000"));
    data_blocks(out, data);
}

fn data_blocks(out: &mut Vec<u8>, data: &[u8]) {
    out.extend_from_slice(data);
    out.resize(out.len() + (BLOCK - data.len() % BLOCK) % BLOCK, 0);
}

fn finish(out: &mut Vec<u8>) {
    out.resize(out.len() + 2 * BLOCK, 0);
}

fn sample() -> Vec<u8> {
    let mut t = Vec::new();
    entry(&mut t, "traces/00/trace_full_000001.json", b"{\"a\":1}");
    t.extend_from_slice(&header("traces/", 0, b'5', "", b"ustar\x0000"));
    entry(&mut t, "big.bin", &vec![0xabu8; 1500]);
    entry(&mut t, "empty", b"");
    t.extend_from_slice(&header(
        "last",
        512,
        b'0',
        "some/long/prefix",
        b"ustar\x0000",
    ));
    data_blocks(&mut t, &[7u8; 512]);
    finish(&mut t);
    t
}

type Seen = Vec<(String, u8, Vec<u8>)>;

fn read_all<R: Read>(parts: Vec<R>, lim: Limits) -> (Seen, Result<Next, TarError>) {
    let mut r = TarReader::new(parts, lim);
    let mut seen = Vec::new();
    loop {
        match r.next_entry() {
            Ok(Next::Entry(h)) => {
                let mut v = Vec::new();
                let t = r.read_entry_to_vec(&mut v).unwrap();
                if let Some(t) = t {
                    seen.push((h.name, h.typeflag, v));
                    return (seen, Ok(Next::Truncated(t)));
                }
                assert_eq!(v.len() as u64, h.size);
                seen.push((h.name, h.typeflag, v));
            }
            other => return (seen, other),
        }
    }
}

fn names(s: &Seen) -> Vec<&str> {
    s.iter().map(|e| e.0.as_str()).collect()
}

// REQ: OA-TAR-001
#[test]
fn reads_ustar_entries_names_sizes_and_padding() {
    let t = sample();
    let (seen, end) = read_all(vec![Cursor::new(t)], DEFAULT_LIMITS);
    assert_eq!(end.unwrap(), Next::End);
    assert_eq!(
        names(&seen),
        [
            "traces/00/trace_full_000001.json",
            "traces/",
            "big.bin",
            "empty",
            "some/long/prefix/last"
        ]
    );
    assert_eq!(seen[0].2, b"{\"a\":1}");
    assert_eq!(seen[1].1, b'5');
    assert!(seen[1].2.is_empty());
    assert_eq!(seen[2].2, vec![0xabu8; 1500]);
    assert!(seen[3].2.is_empty());
    assert_eq!(seen[4].2, vec![7u8; 512]);
}

// REQ: OA-TAR-002
#[test]
fn split_anywhere_gives_the_same_archive() {
    let t = sample();
    let (whole, _) = read_all(vec![Cursor::new(t.clone())], DEFAULT_LIMITS);
    for cut in 0..=t.len() {
        let parts = vec![
            Cursor::new(t[..cut].to_vec()),
            Cursor::new(t[cut..].to_vec()),
        ];
        let (got, end) = read_all(parts, DEFAULT_LIMITS);
        assert_eq!(got, whole, "cut at {cut}");
        assert_eq!(end.unwrap(), Next::End, "cut at {cut}");
    }
    // Several parts, with an empty one between and a one-byte one.
    let parts = vec![
        Cursor::new(t[..700].to_vec()),
        Cursor::new(Vec::new()),
        Cursor::new(t[700..701].to_vec()),
        Cursor::new(t[701..].to_vec()),
    ];
    let (got, end) = read_all(parts, DEFAULT_LIMITS);
    assert_eq!(got, whole);
    assert_eq!(end.unwrap(), Next::End);
}

// REQ: OA-TAR-003
#[test]
fn first_part_only_ends_in_a_typed_truncated_result() {
    let t = sample();
    // Every possible cut: the entries before it are complete, then exactly one Truncated.
    let (whole, _) = read_all(vec![Cursor::new(t.clone())], DEFAULT_LIMITS);
    for cut in 0..t.len() {
        let (got, end) = read_all(vec![Cursor::new(t[..cut].to_vec())], DEFAULT_LIMITS);
        let Next::Truncated(tr) = end.unwrap() else {
            panic!("cut at {cut} was not truncated");
        };
        assert_eq!(tr.offset, cut as u64, "cut at {cut}");
        assert!(got.len() <= whole.len());
        for (a, b) in got.iter().zip(&whole) {
            if tr.stage == Stage::Data && a.0 == tr.entry {
                assert!(b.2.starts_with(&a.2));
            } else {
                assert_eq!(a, b);
            }
        }
    }
    // A cut inside entry data names the entry and how much was present.
    let first_data = BLOCK;
    let (got, end) = read_all(
        vec![Cursor::new(t[..first_data + 3].to_vec())],
        DEFAULT_LIMITS,
    );
    let Next::Truncated(tr) = end.unwrap() else {
        panic!()
    };
    assert_eq!(
        tr,
        Truncated {
            offset: (first_data + 3) as u64,
            stage: Stage::Data,
            entry: "traces/00/trace_full_000001.json".into(),
            entry_size: 7,
            data_present: 3,
        }
    );
    assert_eq!(got[0].2, b"{\"a");
    // Between entries without the end marker, and with half the marker.
    let mut t2 = Vec::new();
    entry(&mut t2, "a", b"xyz");
    let (_, end) = read_all(vec![Cursor::new(t2.clone())], DEFAULT_LIMITS);
    let Next::Truncated(tr) = end.unwrap() else {
        panic!()
    };
    assert_eq!((tr.stage, tr.offset), (Stage::Header, 1024));
    t2.resize(t2.len() + BLOCK, 0);
    let (_, end) = read_all(vec![Cursor::new(t2)], DEFAULT_LIMITS);
    let Next::Truncated(tr) = end.unwrap() else {
        panic!()
    };
    assert_eq!(tr.offset, 1536);
    // The reader stays finished.
    let mut r = TarReader::new(vec![Cursor::new(Vec::<u8>::new())], DEFAULT_LIMITS);
    assert!(matches!(r.next_entry().unwrap(), Next::Truncated(_)));
    assert!(matches!(r.next_entry().unwrap(), Next::Truncated(_)));
}

// REQ: OA-TAR-004
#[test]
fn gnu_long_names_pax_and_base256_sizes() {
    let long = format!("{}/{}", "d".repeat(150), "f".repeat(120));
    let mut t = Vec::new();
    // GNU long name: type L, data is the name plus NUL, applies to the next entry.
    let mut nm = long.clone().into_bytes();
    nm.push(0);
    t.extend_from_slice(&header(
        "././@LongLink",
        nm.len() as u64,
        b'L',
        "",
        b"ustar  \0",
    ));
    data_blocks(&mut t, &nm);
    t.extend_from_slice(&header("short", 4, b'0', "", b"ustar  \0"));
    data_blocks(&mut t, b"data");
    // PAX path override.
    let rec = |k: &str, v: &str| {
        let body = format!(" {k}={v}\n");
        let mut n = body.len() + 1;
        if n.to_string().len() + body.len() != n {
            n += 1;
        }
        format!("{n}{body}")
    };
    let pax = format!(
        "{}{}",
        rec("path", "pax/name.json"),
        rec("comment", "ignored")
    );
    t.extend_from_slice(&header(
        "PaxHeader",
        pax.len() as u64,
        b'x',
        "",
        b"ustar\x0000",
    ));
    data_blocks(&mut t, pax.as_bytes());
    t.extend_from_slice(&header("orig", 2, b'0', "", b"ustar\x0000"));
    data_blocks(&mut t, b"ok");
    // A global PAX header is skipped.
    let g = rec("comment", "global");
    t.extend_from_slice(&header("global", g.len() as u64, b'g', "", b"ustar\x0000"));
    data_blocks(&mut t, g.as_bytes());
    // Base-256 size field (value 3), as GNU tar writes for large files.
    let mut h = header("b256", 0, b'0', "", b"ustar  \0");
    h[124..136].copy_from_slice(&[0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3]);
    seal(&mut h);
    t.extend_from_slice(&h);
    data_blocks(&mut t, b"abc");
    finish(&mut t);
    let (seen, end) = read_all(vec![Cursor::new(t)], DEFAULT_LIMITS);
    assert_eq!(end.unwrap(), Next::End);
    assert_eq!(names(&seen), [long.as_str(), "pax/name.json", "b256"]);
    assert_eq!(seen[0].2, b"data");
    assert_eq!(seen[1].2, b"ok");
    assert_eq!(seen[2].2, b"abc");
}

// REQ: OA-TAR-005
#[test]
fn bounds_and_bad_headers_are_refused() {
    let mut t = Vec::new();
    entry(&mut t, "a", &[1u8; 600]);
    finish(&mut t);
    let tight = Limits {
        max_entry_bytes: 599,
        ..DEFAULT_LIMITS
    };
    let (_, end) = read_all(vec![Cursor::new(t.clone())], tight);
    assert!(matches!(
        end,
        Err(TarError::EntryTooLarge {
            offset: 0,
            size: 600
        })
    ));
    let exact = Limits {
        max_entry_bytes: 600,
        ..DEFAULT_LIMITS
    };
    let (seen, end) = read_all(vec![Cursor::new(t.clone())], exact);
    assert_eq!((seen.len(), end.unwrap()), (1, Next::End));
    // A name over the name limit.
    let (_, end) = read_all(
        vec![Cursor::new(t.clone())],
        Limits {
            max_name_bytes: 0,
            ..DEFAULT_LIMITS
        },
    );
    assert!(matches!(
        end,
        Err(TarError::NameTooLong { offset: 0, len: 1 })
    ));
    // A long-name record over the meta limit.
    let m = header("x", 70_000, b'L', "", b"ustar  \0");
    let (_, end) = read_all(vec![Cursor::new(m.to_vec())], DEFAULT_LIMITS);
    assert!(matches!(
        end,
        Err(TarError::MetaTooLarge { size: 70_000, .. })
    ));
    // Checksum, size and name damage.
    let mut b = t.clone();
    b[10] ^= 1;
    let (_, end) = read_all(vec![Cursor::new(b)], DEFAULT_LIMITS);
    assert!(matches!(
        end,
        Err(TarError::BadHeader {
            offset: 0,
            what: "checksum mismatch"
        })
    ));
    let mut h = header("a", 1, b'0', "", b"ustar\x0000");
    h[124] = b'9';
    seal(&mut h);
    let (_, end) = read_all(vec![Cursor::new(h.to_vec())], DEFAULT_LIMITS);
    assert!(matches!(
        end,
        Err(TarError::BadHeader {
            what: "size field is not a number",
            ..
        })
    ));
    let mut h = header("a", 1, b'0', "", b"ustar\x0000");
    h[0] = 0xff;
    seal(&mut h);
    let (_, end) = read_all(vec![Cursor::new(h.to_vec())], DEFAULT_LIMITS);
    assert!(matches!(
        end,
        Err(TarError::BadHeader {
            what: "name is not valid UTF-8",
            ..
        })
    ));
    // Negative base-256 size.
    let mut h = header("a", 0, b'0', "", b"ustar\x0000");
    h[124..136].copy_from_slice(&[0xff; 12]);
    seal(&mut h);
    let (_, end) = read_all(vec![Cursor::new(h.to_vec())], DEFAULT_LIMITS);
    assert!(matches!(end, Err(TarError::BadHeader { .. })));
    // Junk after a single zero block.
    let mut z = vec![0u8; BLOCK];
    z.extend_from_slice(&header("a", 0, b'0', "", b"ustar\x0000"));
    let (_, end) = read_all(vec![Cursor::new(z)], DEFAULT_LIMITS);
    assert!(matches!(end, Err(TarError::BadHeader { offset: 512, .. })));
    // After an error the reader is finished.
    let mut r = TarReader::new(vec![Cursor::new(vec![1u8; BLOCK])], DEFAULT_LIMITS);
    assert!(r.next_entry().is_err());
    assert!(matches!(r.next_entry(), Err(TarError::Failed)));
}

// REQ: OA-TAR-006
#[test]
fn unread_data_is_skipped_and_links_carry_none() {
    let mut t = Vec::new();
    entry(&mut t, "skipme", &[9u8; 1300]);
    // A symlink with a nonzero size field must not swallow the next block.
    t.extend_from_slice(&header("link", 99, b'2', "", b"ustar\x0000"));
    entry(&mut t, "after", b"yes");
    finish(&mut t);
    let total = t.len() as u64;
    let mut r = TarReader::new(vec![Cursor::new(t)], DEFAULT_LIMITS);
    let Next::Entry(h) = r.next_entry().unwrap() else {
        panic!()
    };
    assert_eq!(h.size, 1300);
    let mut small = [0u8; 100];
    assert_eq!(r.read_data(&mut small).unwrap(), Data::Bytes(100));
    let Next::Entry(h) = r.next_entry().unwrap() else {
        panic!()
    };
    assert_eq!((h.name.as_str(), h.typeflag, h.size), ("link", b'2', 0));
    assert_eq!(r.read_data(&mut small).unwrap(), Data::End);
    let Next::Entry(h) = r.next_entry().unwrap() else {
        panic!()
    };
    assert_eq!(h.name, "after");
    let mut v = Vec::new();
    assert_eq!(r.read_entry_to_vec(&mut v).unwrap(), None);
    assert_eq!(v, b"yes");
    assert_eq!(r.next_entry().unwrap(), Next::End);
    assert_eq!(r.position(), total);
}

// REQ: OA-TAR-007
#[test]
fn io_errors_are_reported_with_the_offset_and_interrupts_retried() {
    struct Flaky {
        data: Cursor<Vec<u8>>,
        interrupted: bool,
        fail_at: u64,
    }
    impl Read for Flaky {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if !self.interrupted {
                self.interrupted = true;
                return Err(io::ErrorKind::Interrupted.into());
            }
            if self.data.position() >= self.fail_at {
                return Err(io::Error::other("disk gone"));
            }
            let take = buf.len().min(100);
            self.data.read(&mut buf[..take])
        }
    }
    let mut t = Vec::new();
    entry(&mut t, "a", &[1u8; 600]);
    finish(&mut t);
    let ok = Flaky {
        data: Cursor::new(t.clone()),
        interrupted: false,
        fail_at: u64::MAX,
    };
    let (seen, end) = read_all(vec![ok], DEFAULT_LIMITS);
    assert_eq!((seen.len(), end.unwrap()), (1, Next::End));
    let bad = Flaky {
        data: Cursor::new(t),
        interrupted: false,
        fail_at: 800,
    };
    let mut r = TarReader::new(vec![bad], DEFAULT_LIMITS);
    let mut res = Ok(Next::End);
    for _ in 0..4 {
        res = r.next_entry();
        if res.is_err() {
            break;
        }
    }
    match res {
        Err(TarError::Io { offset, .. }) => assert!((800..900).contains(&offset), "{offset}"),
        other => panic!("{other:?}"),
    }
}

// REQ: OA-TAR-008
#[test]
fn damaged_archives_never_panic() {
    let t = sample();
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    for _ in 0..3000 {
        let mut b = t.clone();
        for _ in 0..4 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let at = (seed % b.len() as u64) as usize;
            b[at] = (seed >> 24) as u8;
        }
        let _ = read_all(vec![Cursor::new(b)], DEFAULT_LIMITS);
    }
    for len in [0usize, 1, 511, 512, 513, 1024, 4000] {
        let g: Vec<u8> = (0..len).map(|i| (i * 31 % 251) as u8).collect();
        let _ = read_all(vec![Cursor::new(g)], DEFAULT_LIMITS);
    }
}
