//! Compressor tests: round trips through our own inflater, gzip framing, system-tool
//! interoperability, sync flush, stored fallback, determinism and a no-panic sweep.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;
use common::*;
use oa_deflate::{Compressor, GzipWriter};
use oa_zip::inflate;
use std::cell::RefCell;

const LEVELS: [u8; 3] = [1, 6, 9];

// REQ: OA-DEF-001
#[test]
fn round_trips_edge_inputs_at_every_level() {
    let mut rng = Rng(0xC0FFEE);
    let mut cases: Vec<(String, Vec<u8>)> = vec![
        ("empty".into(), vec![]),
        ("one byte".into(), vec![0x5a]),
        ("two bytes".into(), vec![1, 1]),
        ("three same".into(), vec![9; 3]),
        ("all same 1 MiB".into(), vec![0xAB; 1 << 20]),
        ("random 100000".into(), rng.bytes(100_000)),
        ("xml 300000".into(), xmlish(300_000, 7)),
    ];
    // Window and buffer boundaries: 32 KiB, 64 KiB, and the 256 KiB buffer slide, each +-1.
    for base in [32 * 1024, 64 * 1024, 65_535, 256 * 1024, 7 * 32 * 1024] {
        for d in [-1i64, 0, 1] {
            let n = (base as i64 + d) as usize;
            cases.push((format!("random {n}"), rng.bytes(n)));
            cases.push((format!("xml {n}"), xmlish(n, n as u64)));
            cases.push((format!("zeros {n}"), vec![0; n]));
        }
    }
    for (name, data) in &cases {
        for level in LEVELS {
            let z = deflate(level, data);
            assert_roundtrip(&z, data, &format!("{name} level {level}"));
        }
    }
    // Every level from 1 to 9 on one mixed input; out-of-range levels are clamped.
    let mixed = xmlish(120_000, 3);
    for level in 0..=12u8 {
        assert_roundtrip(&deflate(level, &mixed), &mixed, &format!("level {level}"));
    }
}

// REQ: OA-DEF-002
#[test]
fn long_matches_and_maximum_distance() {
    for level in LEVELS {
        // 258-byte matches: a run of one byte is all maximum-length matches.
        let run = vec![b'x'; 258 * 40 + 5];
        let z = deflate(level, &run);
        assert_roundtrip(&z, &run, "long run");
        assert!(
            z.len() < 80,
            "run of {} bytes took {} bytes",
            run.len(),
            z.len()
        );

        // Distance 32768: random 32 KiB repeated; each repeat must be found at the far edge.
        let mut rng = Rng(99);
        let block = rng.bytes(32 * 1024);
        let mut data = Vec::new();
        for _ in 0..5 {
            data.extend_from_slice(&block);
        }
        let z = deflate(level, &data);
        assert_roundtrip(&z, &data, "distance 32768");
        assert!(
            z.len() < 32 * 1024 + 2_000,
            "level {level}: 5 repeats of 32 KiB took {} bytes, matches at distance 32768 missed",
            z.len()
        );

        // One distance short of the window (32767) and one past it (32769, must not match).
        for gap in [32_767usize, 32_769] {
            let mut d = rng.bytes(300);
            d.extend(rng.bytes(gap - 300));
            let tail = d[..300].to_vec();
            d.extend_from_slice(&tail);
            assert_roundtrip(&deflate(level, &d), &d, &format!("gap {gap}"));
        }
    }
}

// REQ: OA-DEF-003
#[test]
fn gzip_member_framing_header_crc_isize() {
    let data = xmlish(50_000, 11);
    for level in LEVELS {
        let g = gzip(level, &data);
        assert_eq!(&g[..4], &[0x1f, 0x8b, 8, 0]);
        assert_eq!(&g[4..8], &[0, 0, 0, 0], "no timestamp");
        assert_eq!(
            g[8],
            match level {
                9 => 2,
                1 => 4,
                _ => 0,
            }
        );
        assert_eq!(g[9], 255);
        let n = g.len();
        assert_eq!(&g[n - 4..], &(data.len() as u32).to_le_bytes());
        assert_eq!(&g[n - 8..n - 4], &oa_zip::crc32(&data).to_le_bytes());
        assert_eq!(gunzip(&g).unwrap(), data);
    }
    // An empty member still has header and trailer.
    let g = gzip(6, &[]);
    assert_eq!(g.len(), 10 + 2 + 8);
    assert_eq!(gunzip(&g).unwrap(), Vec::<u8>::new());
    // A writer is reusable: the second member is independent and identical.
    let mut w = GzipWriter::new(6);
    let mut out = Vec::new();
    for _ in 0..2 {
        let mut sink = |b: &[u8]| -> Result<(), ()> {
            out.extend_from_slice(b);
            Ok(())
        };
        w.write(&data, &mut sink).unwrap();
        w.finish(&mut sink).unwrap();
    }
    let half = out.len() / 2;
    assert_eq!(out[..half], out[half..]);
    assert_eq!(gunzip(&out[..half]).unwrap(), data);
}

// REQ: OA-DEF-004
#[test]
fn system_gzip_reads_our_output() {
    let mut rng = Rng(5);
    let inputs = [
        vec![],
        vec![7],
        xmlish(400_000, 1),
        rng.bytes(70_000),
        vec![0u8; 3_000_000],
    ];
    for data in &inputs {
        for level in LEVELS {
            let g = gzip_chunked(level, data, 9_999);
            let Some(t) = run_tool("gzip", &["-t"], &g) else {
                return;
            };
            assert!(
                t.status.success(),
                "gzip -t: {}",
                String::from_utf8_lossy(&t.stderr)
            );
            let Some(d) = run_tool("gzip", &["-dc"], &g) else {
                return;
            };
            assert!(d.status.success());
            assert!(d.stdout == *data, "gunzip output differs at level {level}");
        }
    }
    // Concatenated members are one valid file.
    let a = xmlish(10_000, 2);
    let b = xmlish(20_000, 3);
    let mut both = gzip(6, &a);
    both.extend(gzip(1, &b));
    let Some(d) = run_tool("gzip", &["-dc"], &both) else {
        return;
    };
    assert!(d.status.success());
    assert!(d.stdout == [a, b].concat());
}

// REQ: OA-DEF-005
#[test]
fn our_inflater_reads_system_gzip_output() {
    let mut rng = Rng(8);
    let inputs = [
        vec![],
        vec![1],
        xmlish(500_000, 4),
        rng.bytes(100_000),
        vec![b'q'; 1_000_000],
    ];
    for data in &inputs {
        for lvl in ["-1", "-6", "-9"] {
            let Some(o) = run_tool("gzip", &[lvl, "-c"], data) else {
                return;
            };
            assert!(o.status.success());
            assert!(gunzip(&o.stdout).unwrap() == *data, "system gzip {lvl}");
        }
    }
}

// REQ: OA-DEF-007
#[test]
fn incompressible_data_expands_by_framing_only() {
    let mut rng = Rng(77);
    for n in [1usize, 100, 1000, 65_535, 65_536, 200_000, 600_000] {
        let data = rng.bytes(n);
        for level in LEVELS {
            let z = deflate(level, &data);
            assert_roundtrip(&z, &data, "random");
            // Stored blocks: 5 bytes per 65535, plus a little for chunk-boundary blocks.
            let bound = n + 5 * (n / 30_000 + 1) + 8;
            assert!(
                z.len() <= bound,
                "random {n} level {level}: {} > {bound}",
                z.len()
            );
        }
    }
}

// REQ: OA-DEF-008
#[test]
fn sync_flush_makes_the_prefix_decodable() {
    let data = xmlish(400_000, 21);
    for level in LEVELS {
        for cut in [0usize, 1, 100, 1_000, 33_000, 100_000, 399_999, 400_000] {
            let mut c = Compressor::new(level);
            let out = RefCell::new(Vec::new());
            let mut sink = |b: &[u8]| -> Result<(), ()> {
                out.borrow_mut().extend_from_slice(b);
                Ok(())
            };
            // Flush at `cut`, one byte later, and at the end, recording (input, output) sizes.
            let mut marks: Vec<(usize, usize)> = Vec::new();
            let mut at = 0;
            for end in [cut, cut + 1, 400_000] {
                let end = end.min(data.len()).max(at);
                c.write(&data[at..end], &mut sink).unwrap();
                at = end;
                c.flush(&mut sink).unwrap();
                marks.push((at, out.borrow().len()));
            }
            let out = out.into_inner();
            for (n, outlen) in marks {
                // The stream cut right after the flush ends without a final block; close it
                // with an empty final fixed block (03 00) and everything before must decode.
                let mut t = out[..outlen].to_vec();
                t.extend_from_slice(&[0x03, 0x00]);
                assert_roundtrip(&t, &data[..n], &format!("level {level} flush at {n}"));
                // The system gzip also yields the prefix (then complains about the cut).
                if n > 0 {
                    let mut g = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 255];
                    g.extend_from_slice(&out[..outlen]);
                    if let Some(o) = run_tool("gzip", &["-dc"], &g) {
                        assert!(o.stdout == data[..n], "system gzip prefix at {n}");
                    }
                }
            }
        }
    }
    // The gzip writer's flush behaves the same, and a flush with no new input is free.
    let mut w = GzipWriter::new(6);
    let out = RefCell::new(Vec::new());
    let mut sink = |b: &[u8]| -> Result<(), ()> {
        out.borrow_mut().extend_from_slice(b);
        Ok(())
    };
    w.write(&data[..5000], &mut sink).unwrap();
    w.flush(&mut sink).unwrap();
    w.flush(&mut sink).unwrap();
    let n1 = out.borrow().len();
    w.flush(&mut sink).unwrap();
    assert_eq!(out.borrow().len(), n1, "idle flush emits nothing");
    w.write(&data[5000..9000], &mut sink).unwrap();
    w.finish(&mut sink).unwrap();
    assert_eq!(gunzip(&out.borrow()).unwrap(), data[..9000]);
}

// REQ: OA-DEF-010
#[test]
fn odd_chunk_sizes_and_flushes_never_panic() {
    let mut rng = Rng(1234);
    let mut data = xmlish(150_000, 6);
    data.extend(rng.bytes(40_000));
    data.extend(vec![0u8; 100_000]);
    data.extend(xmlish(100_000, 9));
    for level in [1u8, 2, 4, 6, 9] {
        for chunk in [
            1usize, 2, 3, 7, 13, 255, 258, 259, 1000, 4097, 32_768, 65_537, 300_000,
        ] {
            // 1-byte chunks over 390 KB at level 9 are slow in debug builds; thin them there.
            let d: &[u8] = if chunk < 8 && cfg!(debug_assertions) {
                &data[..20_000]
            } else {
                &data
            };
            let z = deflate_chunked(level, d, chunk);
            assert_roundtrip(&z, d, &format!("level {level} chunk {chunk}"));
        }
        // Irregular chunks with flushes sprinkled in; the stream must still decode fully.
        let mut c = Compressor::new(level);
        let mut out = Vec::new();
        let mut sink = |b: &[u8]| -> Result<(), ()> {
            out.extend_from_slice(b);
            Ok(())
        };
        let mut at = 0;
        let mut r = Rng(level as u64);
        while at < data.len() {
            let n = (r.next() % 5_000) as usize + 1;
            let end = (at + n).min(data.len());
            c.write(&data[at..end], &mut sink).unwrap();
            at = end;
            if r.next() & 7 == 0 {
                c.flush(&mut sink).unwrap();
            }
        }
        c.finish(&mut sink).unwrap();
        assert_roundtrip(&out, &data, &format!("level {level} irregular"));
    }
    // Empty writes, finish on nothing, flush on nothing.
    let mut c = Compressor::new(6);
    let mut out = Vec::new();
    let mut sink = |b: &[u8]| -> Result<(), ()> {
        out.extend_from_slice(b);
        Ok(())
    };
    c.write(&[], &mut sink).unwrap();
    c.flush(&mut sink).unwrap();
    c.finish(&mut sink).unwrap();
    assert_eq!(inflate(&out, 0, 16).unwrap(), Vec::<u8>::new());
}

// REQ: OA-DEF-011
#[test]
fn output_is_deterministic_and_the_compressor_is_reusable() {
    let data = xmlish(300_000, 31);
    for level in LEVELS {
        assert_eq!(deflate(level, &data), deflate(level, &data));
        assert_eq!(gzip(level, &data), gzip(level, &data));
        // finish() resets: the same compressor yields identical streams, also after a
        // different stream and after an explicit reset mid-stream.
        let mut c = Compressor::new(level);
        let run = |c: &mut Compressor, d: &[u8]| {
            let mut out = Vec::new();
            let mut sink = |b: &[u8]| -> Result<(), ()> {
                out.extend_from_slice(b);
                Ok(())
            };
            c.write(d, &mut sink).unwrap();
            c.finish(&mut sink).unwrap();
            out
        };
        let first = run(&mut c, &data);
        let other = run(&mut c, &data[..1000]);
        assert_roundtrip(&other, &data[..1000], "second stream");
        assert_eq!(run(&mut c, &data), first);
        let mut sink = |_: &[u8]| -> Result<(), ()> { Ok(()) };
        c.write(&data[..5000], &mut sink).unwrap();
        c.reset();
        assert_eq!(run(&mut c, &data), first);
    }
}

// REQ: OA-DEF-012
#[test]
fn closure_errors_propagate() {
    let data = xmlish(200_000, 41);
    let mut c = Compressor::new(6);
    let mut calls = 0;
    let mut failing = |_: &[u8]| -> Result<(), &'static str> {
        calls += 1;
        Err("disk full")
    };
    // Nothing is emitted until a block is complete, so the error surfaces at flush at the latest.
    let r = c
        .write(&data, &mut failing)
        .and_then(|()| c.flush(&mut failing));
    assert_eq!(r, Err("disk full"));
    assert!(calls >= 1);
    // After an error the compressor is usable again following a reset.
    c.reset();
    let mut out = Vec::new();
    let mut sink = |b: &[u8]| -> Result<(), &'static str> {
        out.extend_from_slice(b);
        Ok(())
    };
    c.write(&data, &mut sink).unwrap();
    c.finish(&mut sink).unwrap();
    assert_roundtrip(&out, &data, "after reset");
}
