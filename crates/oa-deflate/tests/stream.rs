//! The streaming inflater and the gzip member reader of oa-zip, checked against the compressor of
//! this crate (and the system `gzip` when it exists): every chunking, concatenated members, every
//! header form, each kind of damage, bombs, and files cut by a crash. They live here because this
//! crate can build the inputs; oa-zip cannot depend on it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;
use common::*;
use oa_deflate::GzipWriter;
use oa_zip::{
    crc32, inflate, Consumed, GzipError, GzipReader, InflateError, InflateLimits, Inflater,
    StreamError, TruncStage, MAX_HEADER_FIELD,
};
use std::convert::Infallible;

const BIG: InflateLimits = InflateLimits::output_only(1 << 40);

/// Pushes `stream` in `chunk`-byte pieces. Returns the outcome, the output, and the bytes of the
/// stream consumed (stops at the end of the stream, as a caller must).
fn run_stream(
    stream: &[u8],
    chunk: usize,
    limits: InflateLimits,
) -> (Result<bool, InflateError>, Vec<u8>, usize) {
    let mut inf = Inflater::new(limits);
    let mut out = Vec::new();
    let mut used = 0usize;
    let mut sink = |b: &[u8]| -> Result<(), Infallible> {
        out.extend_from_slice(b);
        Ok(())
    };
    let mut finished = false;
    for piece in stream.chunks(chunk.max(1)) {
        match inf.push(piece, &mut sink) {
            Ok(Consumed { input, finished: f }) => {
                used += input;
                if f {
                    finished = true;
                    break;
                }
            }
            Err(StreamError::Inflate(e)) => return (Err(e), out, used),
            Err(StreamError::Sink(n)) => match n {},
        }
    }
    (Ok(finished), out, used)
}

fn chunkings() -> Vec<usize> {
    let mut v: Vec<usize> = (1..=17).collect();
    v.extend([31, 64, 257, 1000, 4096, 65_536, usize::MAX / 2]);
    v
}

/// Test inputs: empty, tiny, text, random (stored blocks), long repeats.
fn corpus() -> Vec<(&'static str, Vec<u8>)> {
    let mut r = Rng(7);
    vec![
        ("empty", vec![]),
        ("one", b"x".to_vec()),
        ("hello", b"hello hello hello".to_vec()),
        ("xml", xmlish(30_000, 1)),
        ("random", r.bytes(70_000)),
        ("zeros", vec![0u8; 100_000]),
        (
            "mixed",
            [
                xmlish(5000, 2),
                r.bytes(3000),
                vec![b'a'; 700],
                xmlish(9000, 3),
            ]
            .concat(),
        ),
    ]
}

// REQ: OA-ZIP-005
#[test]
fn streaming_equals_one_shot_for_every_chunking() {
    for (name, data) in corpus() {
        for level in [1u8, 6, 9] {
            let z = deflate(level, &data);
            let one = inflate(&z, data.len(), data.len() + 16).unwrap();
            assert_eq!(one, data, "{name} level {level}: one-shot");
            for chunk in chunkings() {
                // Skip the slowest corner: 1-byte pushes of the largest inputs at all levels.
                if chunk == 1 && data.len() > 40_000 && level != 6 {
                    continue;
                }
                let (res, out, used) = run_stream(&z, chunk, BIG);
                assert_eq!(res, Ok(true), "{name} L{level} chunk {chunk}");
                assert!(out == one, "{name} L{level} chunk {chunk}: output differs");
                assert_eq!(used, z.len(), "{name} L{level} chunk {chunk}: consumed");
            }
        }
    }
}

// REQ: OA-ZIP-005
#[test]
fn consumed_stops_at_the_end_of_the_stream() {
    let data = xmlish(8000, 4);
    let z = deflate(6, &data);
    let mut padded = z.clone();
    padded.extend_from_slice(b"TRAILING-BYTES-NOT-DEFLATE");
    for chunk in chunkings() {
        let (res, out, used) = run_stream(&padded, chunk, BIG);
        assert_eq!(res, Ok(true), "chunk {chunk}");
        assert_eq!(out, data, "chunk {chunk}");
        // The bytes it took are the stream and, for a chunk that straddles the end, no more
        // than that: the caller gets the rest back.
        let stop = if chunk >= padded.len() { z.len() } else { used };
        assert_eq!(used, stop);
        assert_eq!(used, z.len(), "chunk {chunk}: consumed exactly the stream");
    }
    // Stored, fixed and dynamic blocks all end exactly: a stored final block, then trailing.
    let stored = [0x01, 0x03, 0x00, 0xfc, 0xff, b'a', b'b', b'c', 9, 9, 9];
    for chunk in 1..=12 {
        let (res, out, used) = run_stream(&stored, chunk, BIG);
        assert_eq!((res, out.as_slice(), used), (Ok(true), &b"abc"[..], 8));
    }
}

// REQ: OA-ZIP-005
#[test]
fn sink_errors_come_back_unchanged_and_runs_are_at_most_a_window() {
    let data = xmlish(200_000, 5);
    let z = deflate(6, &data);
    let mut inf = Inflater::new(BIG);
    let mut biggest = 0usize;
    let mut total = 0usize;
    let mut sink = |b: &[u8]| -> Result<(), Infallible> {
        biggest = biggest.max(b.len());
        total += b.len();
        Ok(())
    };
    let c = inf.push(&z, &mut sink).unwrap();
    assert!(c.finished && c.input == z.len());
    assert_eq!(total, data.len());
    assert!(biggest <= oa_zip::WINDOW_LEN, "run of {biggest}");
    let mut inf = Inflater::new(BIG);
    let mut calls = 0;
    let mut failing = |_: &[u8]| -> Result<(), &'static str> {
        calls += 1;
        Err("disk full")
    };
    assert_eq!(
        inf.push(&z, &mut failing),
        Err(StreamError::Sink("disk full"))
    );
    assert_eq!(calls, 1);
}

// REQ: OA-ZIP-006
#[test]
fn bomb_stops_at_the_output_cap_and_the_ratio_cap() {
    let bomb_data = vec![0u8; 40 << 20];
    let z = deflate(6, &bomb_data);
    assert!(z.len() < 100_000, "a bomb: {} bytes", z.len());
    let cap = 1 << 20;
    for chunk in [1usize << 20, 4096, 7] {
        let (res, out, _) = run_stream(&z, chunk, InflateLimits::output_only(cap));
        assert_eq!(
            res,
            Err(InflateError::OutputLimit {
                max_output: cap as usize
            }),
            "chunk {chunk}"
        );
        assert!(
            out.len() as u64 <= cap && out.len() as u64 > cap - 300,
            "{}",
            out.len()
        );
        assert!(out.iter().all(|&b| b == 0));
    }
    // Ratio cap (no output cap in reach): stops soon after the exempt size.
    let lim = InflateLimits {
        max_output: 1 << 40,
        max_ratio: 200,
        ratio_exempt_bytes: 1 << 20,
    };
    let (res, out, _) = run_stream(&z, 4096, lim);
    assert_eq!(res, Err(InflateError::RatioLimit { max_ratio: 200 }));
    assert!(
        out.len() > (1 << 20) - 300 && out.len() < 2 << 20,
        "{}",
        out.len()
    );
    // Below the exempt size a high ratio is fine; ordinary data is far below the ratio.
    let small = vec![0u8; 900_000];
    let (res, out, _) = run_stream(&deflate(6, &small), 4096, lim);
    assert_eq!((res, out.len()), (Ok(true), small.len()));
    let text = xmlish(3 << 20, 6);
    let (res, out, _) = run_stream(&deflate(6, &text), 4096, lim);
    assert_eq!((res, out.len()), (Ok(true), text.len()));
}

// REQ: OA-ZIP-006
#[test]
fn gzip_bomb_stops_at_the_cap_and_the_cap_spans_members() {
    let bomb = gzip(6, &vec![0u8; 40 << 20]);
    let got = std::cell::Cell::new(0usize);
    let mut sink = |b: &[u8]| -> Result<(), Infallible> {
        got.set(got.get() + b.len());
        Ok(())
    };
    let mut r = GzipReader::new(InflateLimits::output_only(1 << 20));
    let e = r.push(&bomb, &mut sink).unwrap_err();
    assert_eq!(
        e,
        GzipError::Data(InflateError::OutputLimit {
            max_output: 1 << 20
        })
    );
    let got = got.get();
    assert!(got <= 1 << 20 && got > (1 << 20) - 300);
    assert_eq!(r.push(b"x", &mut sink), Err(GzipError::Failed));

    let ratio = InflateLimits {
        max_output: 1 << 40,
        max_ratio: 200,
        ratio_exempt_bytes: 1 << 20,
    };
    let mut r = GzipReader::new(ratio);
    let e = r.push(&bomb, &mut sink).unwrap_err();
    assert_eq!(
        e,
        GzipError::Data(InflateError::RatioLimit { max_ratio: 200 })
    );

    // Two members of 600 KB against a 1 MiB total.
    let two = [gzip(6, &xmlish(600_000, 8)), gzip(6, &xmlish(600_000, 9))].concat();
    let mut r = GzipReader::new(InflateLimits::output_only(1 << 20));
    let e = r.push(&two, &mut sink).unwrap_err();
    assert!(matches!(
        e,
        GzipError::Data(InflateError::OutputLimit { .. })
    ));
}

/// A gzip member built by hand: header fields chosen by the caller, deflate body from the
/// compressor, correct trailer.
fn member(flags: u8, extra: &[u8], name: &[u8], comment: &[u8], data: &[u8]) -> Vec<u8> {
    let mut m = vec![0x1f, 0x8b, 8, flags, 1, 2, 3, 4, 0, 3];
    if flags & 4 != 0 {
        m.extend((extra.len() as u16).to_le_bytes());
        m.extend(extra);
    }
    if flags & 8 != 0 {
        m.extend(name);
        m.push(0);
    }
    if flags & 16 != 0 {
        m.extend(comment);
        m.push(0);
    }
    if flags & 2 != 0 {
        let h = (crc32(&m) & 0xffff) as u16;
        m.extend(h.to_le_bytes());
    }
    m.extend(deflate(6, data));
    m.extend(crc32(data).to_le_bytes());
    m.extend((data.len() as u32).to_le_bytes());
    m
}

/// Reads `bytes` through a gzip reader in `chunk`-byte pieces.
fn read_gz(
    bytes: &[u8],
    chunk: usize,
    limits: InflateLimits,
) -> (
    Result<(), GzipError<Infallible>>,
    Vec<u8>,
    oa_zip::GzipSummary,
) {
    let mut r = GzipReader::new(limits);
    let mut out = Vec::new();
    let mut sink = |b: &[u8]| -> Result<(), Infallible> {
        out.extend_from_slice(b);
        Ok(())
    };
    let mut res = Ok(());
    for piece in bytes.chunks(chunk.max(1)) {
        res = r.push(piece, &mut sink);
        if res.is_err() {
            break;
        }
    }
    let s = r.finish();
    (res, out, s)
}

// REQ: OA-ZIP-008
#[test]
fn gzip_reader_reads_concatenated_members_at_every_chunking() {
    let parts: Vec<Vec<u8>> = vec![
        xmlish(20_000, 11),
        vec![],
        b"short".to_vec(),
        Rng(3).bytes(9000),
        xmlish(3000, 12),
    ];
    let mut file = Vec::new();
    let mut want = Vec::new();
    for (i, p) in parts.iter().enumerate() {
        file.extend(gzip([1u8, 6, 9, 6, 1][i], p));
        want.extend(p);
    }
    for chunk in chunkings() {
        let (res, out, s) = read_gz(&file, chunk, BIG);
        assert_eq!(res, Ok(()), "chunk {chunk}");
        assert!(out == want, "chunk {chunk}: output differs");
        assert_eq!(s.members, parts.len() as u64, "chunk {chunk}");
        assert_eq!(s.bytes_out, want.len() as u64);
        assert_eq!(s.truncated, None, "chunk {chunk}");
    }
    // The system gzip agrees on what the file holds.
    if let Some(o) = run_tool("gzip", &["-dc"], &file) {
        assert!(o.status.success());
        assert!(o.stdout == want);
    }
}

// REQ: OA-ZIP-008
#[test]
fn every_header_form_parses_and_agrees_with_system_gzip() {
    let data = xmlish(5000, 13);
    let extra: Vec<u8> = (0..300u32).map(|i| (i * 7) as u8).collect();
    for flags in 0u8..16 {
        let m = member(flags, &extra, b"name.txt", b"a comment", &data);
        for chunk in [1usize, 2, 3, 5, 11, 4096, usize::MAX / 2] {
            let (res, out, s) = read_gz(&m, chunk, BIG);
            assert_eq!(res, Ok(()), "flags {flags:#x} chunk {chunk}");
            assert!(out == data, "flags {flags:#x} chunk {chunk}");
            assert_eq!((s.members, s.truncated), (1, None));
        }
        if let Some(o) = run_tool("gzip", &["-dc"], &m) {
            assert!(o.status.success(), "system gzip rejects flags {flags:#x}");
            assert!(o.stdout == data);
        }
    }
    // An empty extra field and empty name are legal.
    let m = member(4 | 8 | 16, &[], b"", b"", b"x");
    assert_eq!(read_gz(&m, 1, BIG).1, b"x");
    // A member made of two headers' worth of fields followed by a plain member.
    let two = [
        member(15, &extra, b"n", b"c", b"one"),
        member(0, &[], b"", b"", b"two"),
    ]
    .concat();
    assert_eq!(read_gz(&two, 1, BIG).1, b"onetwo");
}

fn expect_err(m: &[u8]) -> GzipError<Infallible> {
    for chunk in [1usize, 3, usize::MAX / 2] {
        let (res, _, _) = read_gz(m, chunk, BIG);
        let e = res.unwrap_err();
        assert_eq!(
            read_gz(m, 1, BIG).0.unwrap_err(),
            e,
            "chunk {chunk}: same error"
        );
        let _ = e;
    }
    read_gz(m, usize::MAX / 2, BIG).0.unwrap_err()
}

// REQ: OA-ZIP-009
#[test]
fn each_kind_of_gzip_damage_has_its_own_error() {
    let data = xmlish(4000, 14);
    let good = member(0, &[], b"", b"", &data);
    let n = good.len();

    let mut m = good.clone();
    m[0] = 0x1e;
    assert_eq!(expect_err(&m), GzipError::BadMagic);
    let mut m = good.clone();
    m[1] = 0x8c;
    assert_eq!(expect_err(&m), GzipError::BadMagic);
    let mut m = good.clone();
    m[2] = 7;
    assert_eq!(expect_err(&m), GzipError::UnsupportedMethod(7));
    let mut m = good.clone();
    m[3] = 0x20;
    assert_eq!(expect_err(&m), GzipError::ReservedFlags(0x20));

    // Header CRC-16.
    let mut m = member(2, &[], b"", b"", &data);
    m[10] ^= 0x01;
    assert!(matches!(expect_err(&m), GzipError::HeaderCrc { .. }));
    let mut m = member(2 | 8, &[], b"abc", b"", &data);
    m[11] ^= 0x40; // a name byte: the stored CRC no longer fits
    assert!(matches!(expect_err(&m), GzipError::HeaderCrc { .. }));

    // A name with no end in reach.
    let mut m = vec![0x1f, 0x8b, 8, 8, 0, 0, 0, 0, 0, 3];
    m.extend(vec![b'a'; MAX_HEADER_FIELD as usize + 10]);
    assert_eq!(expect_err(&m), GzipError::HeaderFieldTooLong);

    // Invalid data: block type 3 at the start of the body.
    let mut m = good.clone();
    m[10] |= 0x06;
    assert_eq!(
        expect_err(&m),
        GzipError::Data(InflateError::ReservedBlockType)
    );

    // Trailer CRC-32 and ISIZE, each alone.
    let mut m = good.clone();
    m[n - 8] ^= 0x10;
    match expect_err(&m) {
        GzipError::CrcMismatch { expected, actual } => {
            assert_eq!(actual, crc32(&data));
            assert_ne!(expected, actual);
        }
        e => panic!("{e:?}"),
    }
    let mut m = good.clone();
    m[n - 4] ^= 0x01;
    assert_eq!(
        expect_err(&m),
        GzipError::SizeMismatch {
            expected: data.len() as u32 ^ 1,
            actual: data.len() as u32
        }
    );

    // Garbage after a complete member: the member's data is delivered, then BadMagic.
    let mut m = good.clone();
    m.extend(b"junk after the member");
    let (res, out, s) = read_gz(&m, 5, BIG);
    assert_eq!(res, Err(GzipError::BadMagic));
    assert!(out == data);
    assert_eq!(s.members, 1);
    // After an error the reader takes nothing more.
    let mut r = GzipReader::new(BIG);
    let mut sink = |_: &[u8]| -> Result<(), Infallible> { Ok(()) };
    assert_eq!(r.push(b"\x00", &mut sink), Err(GzipError::BadMagic));
    assert_eq!(r.push(&good, &mut sink), Err(GzipError::Failed));

    // Flipping any single body bit is noticed. (The unused padding bits of the last body byte
    // carry nothing: skip that byte.)
    for i in (10..n - 9).step_by(13) {
        for bit in [0u8, 3, 7] {
            let mut m = good.clone();
            m[i] ^= 1 << bit;
            // Caught by the data, by the trailer, or (if the damage makes the stream want
            // more bits than the member has) as a truncated member: never as success.
            let (res, _, s) = read_gz(&m, 64, BIG);
            assert!(
                res.is_err() || s.truncated.is_some(),
                "bit {bit} of byte {i} went unnoticed (member is {n} bytes)"
            );
        }
    }
}

/// A gzip file written with sync flushes after each part, and the file length and data length at
/// each flush.
fn flushed_file(parts: &[Vec<u8>], finish: bool) -> (Vec<u8>, Vec<(usize, usize)>) {
    let mut w = GzipWriter::new(6);
    let mut file = Vec::new();
    let mut marks = Vec::new();
    let mut done = 0usize;
    for p in parts {
        let mut sink = |b: &[u8]| -> Result<(), Infallible> {
            file.extend_from_slice(b);
            Ok(())
        };
        w.write(p, &mut sink).unwrap();
        w.flush(&mut sink).unwrap();
        done += p.len();
        marks.push((file.len(), done));
    }
    if finish {
        let mut sink = |b: &[u8]| -> Result<(), Infallible> {
            file.extend_from_slice(b);
            Ok(())
        };
        w.finish(&mut sink).unwrap();
    }
    (file, marks)
}

// REQ: OA-ZIP-010
#[test]
fn a_file_cut_at_a_flush_is_recovered_exactly() {
    let parts = vec![xmlish(30_000, 21), xmlish(9000, 22), Rng(5).bytes(5000)];
    let all = parts.concat();
    let (file, marks) = flushed_file(&parts, false);
    for chunk in [1usize, 7, 4096, usize::MAX / 2] {
        for &(flen, dlen) in &marks {
            let (res, out, s) = read_gz(&file[..flen], chunk, BIG);
            assert_eq!(res, Ok(()), "a cut is not an error");
            assert!(out == all[..dlen], "flush {flen} chunk {chunk}");
            let t = s.truncated.expect("never silent success");
            assert_eq!(t.stage, TruncStage::Data);
            assert_eq!((t.tail_bytes, t.flushed_bytes), (dlen as u64, dlen as u64));
            assert_eq!((s.members, s.bytes_out), (0, dlen as u64));
        }
    }
}

// REQ: OA-ZIP-010
#[test]
fn a_cut_member_after_complete_ones_reports_only_its_own_tail() {
    let first = gzip(6, &xmlish(10_000, 23));
    let first_data = xmlish(10_000, 23);
    let parts = vec![xmlish(7000, 24), xmlish(7000, 25)];
    let (second, marks) = flushed_file(&parts, true);
    let file = [first.clone(), second.clone()].concat();
    // Cut after the second member's first flush.
    let cut = first.len() + marks[0].0;
    let (res, out, s) = read_gz(&file[..cut], 100, BIG);
    assert_eq!(res, Ok(()));
    assert!(out == [first_data.clone(), parts[0].clone()].concat());
    let t = s.truncated.unwrap();
    assert_eq!((s.members, t.stage), (1, TruncStage::Data));
    assert_eq!((t.tail_bytes, t.flushed_bytes), (7000, 7000));
    assert_eq!(s.bytes_out, 10_000 + 7000);
    // Cut inside the next member's header, and inside the trailer.
    let (_, out, s) = read_gz(&file[..first.len() + 5], 3, BIG);
    assert!(out == first_data);
    let t = s.truncated.unwrap();
    assert_eq!(
        (s.members, t.stage, t.tail_bytes),
        (1, TruncStage::Header, 0)
    );
    let (_, out, s) = read_gz(&file[..file.len() - 3], 3, BIG);
    assert_eq!(out.len(), 10_000 + 14_000);
    let t = s.truncated.unwrap();
    assert_eq!((s.members, t.stage), (1, TruncStage::Trailer));
    assert_eq!((t.tail_bytes, t.flushed_bytes), (14_000, 14_000));
    // The whole file is not truncated; an empty one is.
    let (_, _, s) = read_gz(&file, 1000, BIG);
    assert_eq!((s.members, s.truncated), (2, None));
    let (res, out, s) = read_gz(&[], 1, BIG);
    assert_eq!((res, out.len()), (Ok(()), 0));
    assert_eq!(s.truncated.unwrap().stage, TruncStage::Header);
}

// REQ: OA-ZIP-010
#[test]
fn every_cut_point_recovers_a_prefix_and_everything_before_the_last_flush() {
    let parts = vec![xmlish(6000, 26), xmlish(2500, 27), xmlish(4000, 28)];
    let all = parts.concat();
    let (file, marks) = flushed_file(&parts, true);
    for chunk in [1usize, usize::MAX / 2] {
        for cut in 0..=file.len() {
            let (res, out, s) = read_gz(&file[..cut], chunk, BIG);
            assert_eq!(res, Ok(()), "cut {cut}");
            assert!(all.starts_with(&out), "cut {cut}: not a prefix");
            assert_eq!(s.bytes_out, out.len() as u64);
            if cut == file.len() {
                assert_eq!((s.members, s.truncated), (1, None));
                assert!(out == all);
                continue;
            }
            let t = s
                .truncated
                .unwrap_or_else(|| panic!("cut {cut} reported complete"));
            // Everything up to the last flush that fits in the cut is delivered and counted.
            let at_flush = marks
                .iter()
                .rev()
                .find(|(flen, _)| *flen <= cut)
                .map_or(0, |m| m.1);
            assert!(t.flushed_bytes >= at_flush as u64, "cut {cut}");
            assert!(out.len() >= at_flush, "cut {cut}");
            assert!(t.flushed_bytes <= t.tail_bytes && t.tail_bytes == out.len() as u64);
            if cut < 10 {
                assert_eq!((t.stage, t.tail_bytes), (TruncStage::Header, 0));
            }
        }
    }
}

// REQ: OA-ZIP-011
#[test]
fn damaged_streams_never_panic_and_streaming_is_chunking_independent() {
    let data = xmlish(3000, 31);
    let good = gzip(6, &data);
    let raw = deflate(6, &data);
    let mut rng = Rng(99);
    let mut cases: Vec<Vec<u8>> = Vec::new();
    for i in 0..good.len() {
        let mut m = good.clone();
        m[i] ^= 1 << (i % 8);
        cases.push(m);
        let mut m = good.clone();
        m.remove(i);
        cases.push(m);
        let mut m = good.clone();
        m.insert(i, (i * 31) as u8);
        cases.push(m);
        cases.push(good[..i].to_vec());
    }
    for _ in 0..300 {
        let n = (rng.next() % 600) as usize;
        let mut m = rng.bytes(n);
        if n > 3 && rng.next() & 1 == 0 {
            m[0] = 0x1f;
            m[1] = 0x8b;
            m[2] = 8;
        }
        cases.push(m);
    }
    for c in &cases {
        for chunk in [1usize, 9, usize::MAX / 2] {
            let _ = read_gz(c, chunk, BIG);
        }
    }
    // The raw inflater: same output and same error however the input is cut.
    let mut raw_cases: Vec<Vec<u8>> = Vec::new();
    for i in 0..raw.len() {
        let mut m = raw.clone();
        m[i] ^= 1 << (i % 8);
        raw_cases.push(m);
        raw_cases.push(raw[..i].to_vec());
    }
    for _ in 0..2000 {
        let n = (rng.next() % 300) as usize;
        raw_cases.push(rng.bytes(n));
    }
    for c in &raw_cases {
        let whole = run_stream(c, usize::MAX / 2, BIG);
        for chunk in [1usize, 2, 5, 64] {
            let part = run_stream(c, chunk, BIG);
            assert_eq!(part.0, whole.0, "chunk {chunk}: result");
            assert!(part.1 == whole.1, "chunk {chunk}: output");
            if whole.0.is_ok() {
                assert_eq!(part.2, whole.2, "chunk {chunk}: consumed");
            }
        }
    }
    // 200 000 bytes of noise in several chunkings, as one stream.
    let noise = rng.bytes(200_000);
    for chunk in [1usize, 100, 65_536] {
        let _ = run_stream(&noise, chunk, InflateLimits::output_only(1 << 24));
        let _ = read_gz(&noise, chunk, InflateLimits::output_only(1 << 24));
    }
}
