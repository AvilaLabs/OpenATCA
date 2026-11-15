//! The counters and limits of the archive scan, on small archives built here.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::Cursor;

use oa_adsb::*;

const BLOCK: usize = 512;

fn put(h: &mut [u8; BLOCK], at: usize, bytes: &[u8]) {
    h[at..at + bytes.len()].copy_from_slice(bytes);
}

fn tar_header(name: &str, size: usize, typeflag: u8) -> [u8; BLOCK] {
    let mut h = [0u8; BLOCK];
    put(&mut h, 0, name.as_bytes());
    put(&mut h, 100, b"0000644\0");
    put(&mut h, 124, format!("{size:011o}\0").as_bytes());
    put(&mut h, 136, b"14000000000\0");
    h[156] = typeflag;
    put(&mut h, 257, b"ustar\x0000");
    put(&mut h, 148, b"        ");
    let sum: u32 = h.iter().map(|&b| u32::from(b)).sum();
    put(&mut h, 148, format!("{sum:06o}\0 ").as_bytes());
    h
}

fn tar_entry(out: &mut Vec<u8>, name: &str, data: &[u8]) {
    out.extend_from_slice(&tar_header(name, data.len(), b'0'));
    out.extend_from_slice(data);
    out.resize(out.len() + (BLOCK - data.len() % BLOCK) % BLOCK, 0);
}

fn end(t: &mut Vec<u8>) {
    t.resize(t.len() + 2 * BLOCK, 0);
}

fn scan(tar: Vec<u8>) -> ScanResult {
    scan_archive(
        vec![Cursor::new(tar)],
        &ZAU_BOX,
        &mut |_: &Reports| -> Result<(), String> { Ok(()) },
    )
    .unwrap()
}

const EMPTY_TRACE: &str = r#"{"icao":"aaaaaa","timestamp":1794528000.000,"trace":[] } "#;

// REQ: OA-ADSB-159
#[test]
fn entry_counters_add_and_a_cut_entry_is_not_counted() {
    let mut t = Vec::new();
    tar_entry(&mut t, "./README", b"x");
    t.extend_from_slice(&tar_header("./traces/trace_full_dir.json", 0, b'5'));
    tar_entry(
        &mut t,
        "./traces/aa/trace_full_aaaaaa.json",
        EMPTY_TRACE.as_bytes(),
    );
    tar_entry(
        &mut t,
        "./traces/aa/trace_full_bbbbbb.json",
        EMPTY_TRACE.as_bytes(),
    );
    tar_entry(&mut t, "./traces/aa/trace_aaaaaa.json", b"{}");
    tar_entry(&mut t, "./traces/aa/trace_full_aaaaaa.json.gz", b"{}");
    tar_entry(&mut t, "./traces/aa/xtrace_full_aaaaaa.json", b"{}");
    tar_entry(&mut t, "trace_full_top.json", EMPTY_TRACE.as_bytes());
    let whole = t.len();
    end(&mut t);
    let r = scan(t.clone());
    assert_eq!(r.end, ScanEnd::Complete);
    // Three traces; the directory entry and four other names are skipped.
    assert_eq!((r.trace_entries, r.other_entries), (3, 5));
    assert_eq!((r.stats.traces_read, r.stats.traces_refused), (3, 0));
    assert!(r.error_samples.is_empty());
    // A cut inside a trace's data: the entries before it count, the cut one does not.
    let mut cut = t[..whole].to_vec();
    tar_entry(
        &mut cut,
        "./traces/zz/trace_full_cccccc.json",
        &[b' '; 2000],
    );
    cut.truncate(whole + BLOCK + 700);
    let r = scan(cut);
    assert!(matches!(r.end, ScanEnd::Truncated(_)));
    assert_eq!((r.trace_entries, r.other_entries), (3, 5));
    assert_eq!(r.stats.traces_read, 3);
}

// REQ: OA-ADSB-159
#[test]
fn refusal_messages_are_kept_up_to_sixteen_and_the_counts_are_complete() {
    for n in [1usize, 15, 16, 17, 20] {
        let mut t = Vec::new();
        for i in 0..n {
            tar_entry(
                &mut t,
                &format!("./traces/trace_full_{i:06x}.json"),
                b"{\"icao\":",
            );
        }
        end(&mut t);
        let r = scan(t);
        assert_eq!(r.stats.traces_refused, n as u64, "{n}");
        assert_eq!(r.stats.traces_read, 0);
        assert_eq!(r.trace_entries, n as u64);
        assert_eq!(r.error_samples.len(), n.min(16), "{n}");
        // The first ones are the ones kept, in order, as `name: reason`.
        assert!(r.error_samples[0].starts_with("./traces/trace_full_000000.json: "));
        if n > 16 {
            assert!(r.error_samples[15].starts_with("./traces/trace_full_00000f.json: "));
        }
    }
}

// REQ: OA-ADSB-159
#[test]
fn an_entry_over_the_trace_size_limit_is_refused_unread() {
    let limit = oa_json::MAX_INPUT_BYTES;
    // Over: refused at once, by size, whatever follows.
    let mut t = tar_header("./traces/trace_full_aaaaaa.json", limit + 1, b'0').to_vec();
    end(&mut t);
    let r = scan(t);
    assert_eq!(r.stats.traces_refused, 1);
    assert_eq!(r.trace_entries, 1);
    assert!(r.error_samples[0].contains("larger than the trace size limit"));
    // At the limit it is read, and the missing data is a cut, not a refusal.
    let mut t = tar_header("./traces/trace_full_aaaaaa.json", limit, b'0').to_vec();
    end(&mut t);
    let r = scan(t);
    assert_eq!(r.stats.traces_refused, 0);
    assert!(r.error_samples.is_empty());
    assert!(matches!(r.end, ScanEnd::Truncated(_)));
    assert_eq!(r.trace_entries, 0);
}

/// LSB-first bit writer for hand-made deflate.
struct Bits {
    out: Vec<u8>,
    n: u32,
}

impl Bits {
    fn put(&mut self, v: u32, bits: u32) {
        for i in 0..bits {
            if self.n.is_multiple_of(8) {
                self.out.push(0);
            }
            if (v >> i) & 1 == 1 {
                *self.out.last_mut().unwrap() |= 1 << (self.n % 8);
            }
            self.n += 1;
        }
    }
}

/// A gzip member of `prefix` (stored block) followed by `runs` repeats of its last byte, 258 at
/// a time, in a dynamic block that spends 2 bits per run: a ratio of about 1000.
fn gzip_run(prefix: &[u8], runs: usize) -> Vec<u8> {
    let mut o = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 3];
    o.push(0); // stored, not final
    o.extend_from_slice(&(prefix.len() as u16).to_le_bytes());
    o.extend_from_slice(&(!(prefix.len() as u16)).to_le_bytes());
    o.extend_from_slice(prefix);
    let mut b = Bits {
        out: Vec::new(),
        n: 0,
    };
    b.put(1, 1); // final
    b.put(2, 2); // dynamic
    b.put(29, 5); // 286 literal/length codes
    b.put(0, 5); // 1 distance code
    b.put(14, 4); // 18 code-length codes
                  // Order 16,17,18,0,8,7,9,6,10,5,11,4,12,3,13,2,14,1: only 18 and 1 have a (1 bit) code.
    for l in [0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1] {
        b.put(l, 3);
    }
    // Code-length code: symbol 1 is `0`, symbol 18 is `1`.
    b.put(1, 1);
    b.put(127, 7); // 138 zeros
    b.put(1, 1);
    b.put(107, 7); // 118 zeros: literals 0..=255 unused
    b.put(0, 1); // length 1 for 256
    b.put(1, 1);
    b.put(17, 7); // 28 zeros: 257..=284 unused
    b.put(0, 1); // length 1 for 285
    b.put(0, 1); // the one distance code, length 1
                 // Data: 285 is `1`, 256 is `0`; distance 0 (one byte back) is `0`.
    for _ in 0..runs {
        b.put(1, 1);
        b.put(0, 1);
    }
    b.put(0, 1);
    o.extend_from_slice(&b.out);
    let mut data = prefix.to_vec();
    let last = *prefix.last().unwrap();
    data.resize(prefix.len() + 258 * runs, last);
    o.extend_from_slice(&oa_zip::crc32(&data).to_le_bytes());
    o.extend_from_slice(&(data.len() as u32).to_le_bytes());
    o
}

// REQ: OA-ADSB-159
#[test]
fn gzip_ratio_limit_has_a_one_mebibyte_exemption() {
    // Sanity: the builder makes what it says (inflates to the padded text).
    let g = gzip_run(EMPTY_TRACE.as_bytes(), 3);
    let mut gz = oa_zip::GzipReader::new(oa_zip::InflateLimits {
        max_output: 1 << 20,
        max_ratio: u64::MAX,
        ratio_exempt_bytes: 0,
    });
    let mut out = Vec::new();
    gz.push(&g, &mut |b: &[u8]| -> Result<(), String> {
        out.extend_from_slice(b);
        Ok(())
    })
    .unwrap();
    assert_eq!(out.len(), EMPTY_TRACE.len() + 3 * 258);

    // 300 KB out of about 500 bytes in: over the ratio but under the exemption, so read.
    let small = gzip_run(EMPTY_TRACE.as_bytes(), 1200);
    assert!(300_000 > small.len() * MAX_GZIP_RATIO as usize);
    let mut t = Vec::new();
    tar_entry(&mut t, "./traces/trace_full_aaaaaa.json", &small);
    end(&mut t);
    let r = scan(t);
    assert_eq!(r.error_samples, Vec::<String>::new());
    assert_eq!((r.stats.traces_read, r.stats.traces_refused), (1, 0));
    // 1.6 MB out of the same kind of input: past the exemption, refused for the ratio.
    let big = gzip_run(EMPTY_TRACE.as_bytes(), 6200);
    let mut t = Vec::new();
    tar_entry(&mut t, "./traces/trace_full_aaaaaa.json", &big);
    end(&mut t);
    let r = scan(t);
    assert_eq!((r.stats.traces_read, r.stats.traces_refused), (0, 1));
    assert!(
        r.error_samples[0].contains("decompression bomb"),
        "{:?}",
        r.error_samples
    );
}
