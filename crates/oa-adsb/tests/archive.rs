//! Archive scanning and the `oa-adsb-clip` binary, on archives built here (tar writer, gzip
//! with stored deflate blocks), plus a real-data run gated on OPENATCA_DATA.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::{Cursor, Read};
use std::path::PathBuf;
use std::process::Command;

use oa_adsb::*;
use oa_feed::{FrameReader, Kind, MAX_FRAME_LEN};

// ---- builders -------------------------------------------------------------------------------

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

/// A gzip member holding `data` in stored deflate blocks.
fn gzip(data: &[u8]) -> Vec<u8> {
    let mut o = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 3];
    let mut chunks = data.chunks(65535).peekable();
    if chunks.peek().is_none() {
        o.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
    }
    while let Some(c) = chunks.next() {
        o.push(u8::from(chunks.peek().is_none()));
        o.extend_from_slice(&(c.len() as u16).to_le_bytes());
        o.extend_from_slice(&(!(c.len() as u16)).to_le_bytes());
        o.extend_from_slice(c);
    }
    o.extend_from_slice(&oa_zip::crc32(data).to_le_bytes());
    o.extend_from_slice(&(data.len() as u32).to_le_bytes());
    o
}

fn trace_json(icao: &str, rows: &[&str]) -> String {
    format!(
        "{{\"icao\":\"{icao}\",\"timestamp\":1794528000.000,\"trace\":[{}]}}",
        rows.join(",")
    )
}

/// Rows in and around ZAU at one-second steps from the base.
fn in_box(n: usize, lat: f64) -> Vec<String> {
    (0..n)
        .map(|i| {
            format!(
                "[{}.00,{:.6},-90.000000,30000,400.0,90.0,0,0,null,\"adsb_icao\",30500,null,null,null]",
                1000 + i,
                lat + i as f64 * 0.0001
            )
        })
        .collect()
}

struct Day {
    tar: Vec<u8>,
    /// Offset in `tar` where the entry for trace C starts.
    c_at: usize,
}

fn day() -> Day {
    let mut t = Vec::new();
    tar_entry(&mut t, "./LICENSE-ODbL.txt", b"licence text");
    t.extend_from_slice(&tar_header("./traces/", 0, b'5'));
    let a = in_box(5, 41.0);
    let a: Vec<&str> = a.iter().map(String::as_str).collect();
    tar_entry(
        &mut t,
        "./traces/aa/trace_full_aaaaaa.json",
        &gzip(trace_json("aaaaaa", &a).as_bytes()),
    );
    let b = in_box(3, 42.0);
    let b: Vec<&str> = b.iter().map(String::as_str).collect();
    tar_entry(
        &mut t,
        "./traces/bb/trace_full_bbbbbb.json",
        trace_json("bbbbbb", &b).as_bytes(),
    );
    let c_at = t.len();
    // Outside the box (Europe): read, counted, not delivered.
    let c = trace_json(
        "cccccc",
        &["[1.0,49.0,10.0,1000,1.0,1.0,0,0,null,\"adsb_icao\"]"],
    );
    tar_entry(
        &mut t,
        "./traces/cc/trace_full_cccccc.json",
        &gzip(c.as_bytes()),
    );
    // Corrupt gzip (a flipped data byte breaks the CRC).
    let mut d = gzip(trace_json("dddddd", &a).as_bytes());
    let n = d.len();
    d[n - 12] ^= 0xff;
    tar_entry(&mut t, "./traces/dd/trace_full_dddddd.json", &d);
    // Valid gzip around broken JSON.
    tar_entry(
        &mut t,
        "./traces/ee/trace_full_eeeeee.json",
        &gzip(b"{\"icao\":"),
    );
    // One good row, one bad row.
    let f = trace_json(
        "fffffe",
        &[
            "[1.0,43.0,-90.0,1000,1.0,1.0,256,0]",
            "[2.0,43.0,-90.0,1000,1.0,1.0,0,0]",
        ],
    );
    tar_entry(
        &mut t,
        "./traces/ff/trace_full_fffffe.json",
        &gzip(f.as_bytes()),
    );
    // Not traces: other names are skipped unread.
    tar_entry(&mut t, "./acas/acas.json", b"{}");
    t.resize(t.len() + 2 * BLOCK, 0);
    Day { tar: t, c_at }
}

fn scan(parts: Vec<Cursor<Vec<u8>>>, f: &Filter) -> (ScanResult, Vec<(u32, usize)>) {
    let mut calls = Vec::new();
    let res = scan_archive(parts, f, &mut |r: &Reports| -> Result<(), String> {
        calls.push((r.icao[0], reports_len(r)));
        Ok(())
    })
    .unwrap();
    (res, calls)
}

// ---- tests ----------------------------------------------------------------------------------

// REQ: OA-ADSB-013
#[test]
fn archive_scan_decodes_traces_and_survives_bad_ones() {
    let d = day();
    let (res, calls) = scan(vec![Cursor::new(d.tar.clone())], &ZAU_BOX);
    assert_eq!(res.end, ScanEnd::Complete);
    assert_eq!(calls, [(0xaaaaaa, 5), (0xbbbbbb, 3), (0xfffffe, 1)]);
    assert_eq!((res.trace_entries, res.other_entries), (6, 3));
    let st = res.stats;
    assert_eq!((st.traces_read, st.traces_refused), (4, 2));
    assert_eq!((st.kept, st.outside_box), (9, 1));
    assert_eq!(st.rejected[REJ_FLAGS], 1);
    assert_eq!(st.rows_seen, 5 + 3 + 1 + 2);
    assert_eq!(res.error_samples.len(), 2);
    assert!(
        res.error_samples[0].contains("trace_full_dddddd"),
        "{:?}",
        res.error_samples
    );
    assert!(
        res.error_samples[0].contains("CRC"),
        "{:?}",
        res.error_samples
    );
    assert!(
        res.error_samples[1].contains("trace_full_eeeeee"),
        "{:?}",
        res.error_samples
    );
    // The time window applies while streaming.
    let f = Filter {
        from_ns: 1_794_528_000_000_000_000 + 1002 * 1_000_000_000,
        ..ZAU_BOX
    };
    let (res, calls) = scan(vec![Cursor::new(d.tar)], &f);
    // fffffe's rows are at +1 s and +2 s, before the window: both counted as outside it (the
    // bad one too, since the window is checked before the rest of the row).
    assert_eq!(calls, [(0xaaaaaa, 3), (0xbbbbbb, 1)]);
    assert_eq!(res.stats.outside_time, 2 + 2 + 2);
}

// REQ: OA-ADSB-014
#[test]
fn split_archive_gives_the_same_result_and_first_part_alone_is_truncated() {
    let d = day();
    let (whole, calls) = scan(vec![Cursor::new(d.tar.clone())], &ZAU_BOX);
    for cut in [
        1usize,
        511,
        512,
        700,
        1500,
        d.c_at,
        d.c_at + 3,
        d.tar.len() - 600,
    ] {
        let parts = vec![
            Cursor::new(d.tar[..cut].to_vec()),
            Cursor::new(d.tar[cut..].to_vec()),
        ];
        let (res, c) = scan(parts, &ZAU_BOX);
        assert_eq!(res, whole, "cut {cut}");
        assert_eq!(c, calls);
    }
    // Only the first part: everything before the cut is delivered, and the end is typed.
    let (res, c) = scan(vec![Cursor::new(d.tar[..d.c_at + 600].to_vec())], &ZAU_BOX);
    let ScanEnd::Truncated(t) = &res.end else {
        panic!("{:?}", res.end)
    };
    assert_eq!(t.offset, (d.c_at + 600) as u64);
    assert_eq!(t.entry, "./traces/cc/trace_full_cccccc.json");
    assert_eq!(c, [(0xaaaaaa, 5), (0xbbbbbb, 3)]);
    assert_eq!(res.stats.traces_read, 2);
    // A cut at an entry boundary has no entry named but is still truncated.
    let (res, _) = scan(vec![Cursor::new(d.tar[..d.c_at].to_vec())], &ZAU_BOX);
    assert!(matches!(res.end, ScanEnd::Truncated(_)));
    // The sink failing stops the scan with its error.
    let r = scan_archive(vec![Cursor::new(d.tar)], &ZAU_BOX, &mut |_: &Reports| {
        Err("disk full".to_string())
    });
    assert!(matches!(r, Err(ScanError::Sink(ref s)) if s == "disk full"));
}

fn tmp(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

fn read_frames(path: &PathBuf) -> (Vec<u8>, Reports, usize) {
    let bytes = std::fs::read(path).unwrap();
    let mut buf = vec![0u8; MAX_FRAME_LEN];
    let mut rd = FrameReader::new(&mut buf).unwrap();
    rd.push(&bytes);
    let mut out = Reports::default();
    let mut frames = 0;
    while let Some(f) = rd.next() {
        let f = f.unwrap();
        assert_eq!(f.kind, Kind::Message);
        assert_eq!(f.flags, oa_feed::FLAG_BYTES);
        assert_eq!(f.seq, frames as u64);
        let n = decode_payload(f.payload, &mut out).unwrap();
        // The frame's receive time is its first report's time.
        assert_eq!(f.recv_unix_ns, out.time_ns[reports_len(&out) - n]);
        assert_eq!(f.jms_timestamp_ms, f.recv_unix_ns / 1_000_000);
        frames += 1;
    }
    assert!(rd.finish().is_none());
    (bytes, out, frames)
}

// REQ: OA-ADSB-015
#[test]
fn clip_binary_writes_time_sorted_oaf_frames_and_reports_truncation() {
    let d = day();
    let a = tmp("clip.tar.aa");
    let b = tmp("clip.tar.ab");
    let cut = d.c_at + 77;
    std::fs::write(&a, &d.tar[..cut]).unwrap();
    std::fs::write(&b, &d.tar[cut..]).unwrap();
    let out = tmp("clip.oaf");
    let exe = env!("CARGO_BIN_EXE_oa-adsb-clip");
    let run = |args: &[&str]| Command::new(exe).args(args).output().unwrap();
    let (sa, sb, so) = (
        a.to_str().unwrap(),
        b.to_str().unwrap(),
        out.to_str().unwrap(),
    );
    // Both parts: complete, 9 reports, sorted by time (60 s frames: the clusters at +2 s and
    // +1000 s are two frames).
    let o = run(&["--out", so, "--frame-ms", "60000", sa, sb]);
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(o.status.success(), "{err}");
    assert!(
        err.contains("kept=9") && err.contains("archive: complete"),
        "{err}"
    );
    assert!(err.contains("traces_refused=2"), "{err}");
    let (_, r, frames) = read_frames(&out);
    assert_eq!((reports_len(&r), frames), (9, 2));
    assert!(r.time_ns.windows(2).all(|w| w[0] <= w[1]));
    // fffffe's good row is at +2 s, before the others' first report at +1000 s.
    assert_eq!(r.icao[0], 0xfffffe);
    assert_eq!(summarize(&r).aircraft, 3);
    // Window with an ISO time and a smaller box.
    let o = run(&[
        "--out",
        so,
        "--from",
        "2026-11-13T00:16:42Z",
        "--box",
        "41.5,42.5,-91,-89",
        sa,
        sb,
    ]);
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(o.status.success(), "{err}");
    let (_, r, _) = read_frames(&out);
    // 1794528000 is 2026-11-13T00:00:00Z; 00:16:42 is +1002 s; only bbbbbb (lat 42.x) is in the box.
    assert_eq!(r.icao, [0xbbbbbb]);
    // The first part alone: truncated, exit 3, frames still written; allowed with a flag.
    let o = run(&["--out", so, sa]);
    let err = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(3), "{err}");
    assert!(err.contains("TRUNCATED"), "{err}");
    let (_, r, _) = read_frames(&out);
    assert_eq!(reports_len(&r), 8);
    let o = run(&["--out", so, "--allow-truncated", sa]);
    assert!(o.status.success());
    // Frames to standard output with `--out -`, and nothing else there.
    let o = run(&["--out", "-", "--frame-ms", "60000", sa, sb]);
    let mut buf = vec![0u8; MAX_FRAME_LEN];
    let mut rd = FrameReader::new(&mut buf).unwrap();
    rd.push(&o.stdout);
    assert!(rd.next().unwrap().is_ok());
    assert!(rd.next().unwrap().is_ok());
    assert!(rd.next().is_none());
    // Argument errors exit 2.
    for bad in [
        &["--out", so][..],
        &["sa"][..],
        &["--out", so, "--box", "1,2,3", sa][..],
        &["--out", so, "--from", "yesterday", sa][..],
        &["--out", so, "--bogus", sa][..],
    ] {
        assert_eq!(run(bad).status.code(), Some(2), "{bad:?}");
    }
    let o = run(&["--out", so, "/nonexistent/part.tar"]);
    assert_eq!(o.status.code(), Some(1));
}

/// A tar with one trace of `n` rows at one-second steps, and the path it is written to.
fn one_trace_tar(name: &str, n: usize) -> String {
    let mut t = Vec::new();
    let rows = in_box(n, 41.0);
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    tar_entry(
        &mut t,
        "./traces/aa/trace_full_aaaaaa.json",
        trace_json("aaaaaa", &rows).as_bytes(),
    );
    t.resize(t.len() + 2 * BLOCK, 0);
    let p = tmp(name);
    std::fs::write(&p, &t).unwrap();
    p.to_str().unwrap().to_string()
}

// REQ: OA-ADSB-017
#[test]
fn clip_frames_split_by_report_time_and_keep_every_report_once_in_order() {
    let tar = one_trace_tar("span.tar", 25);
    let out = tmp("span.oaf");
    let so = out.to_str().unwrap();
    let exe = env!("CARGO_BIN_EXE_oa-adsb-clip");
    let clip = |extra: &[&str]| {
        let mut a = vec!["--out", so];
        a.extend_from_slice(extra);
        a.push(&tar);
        let o = Command::new(exe).args(&a).output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        read_frames(&out)
    };
    // Reports are one second apart; the default 1000 ms span puts each in its own frame.
    let (_, r, frames) = clip(&[]);
    assert_eq!((reports_len(&r), frames), (25, 25));
    let times = r.time_ns.clone();
    assert!(times.windows(2).all(|w| w[1] - w[0] == 1_000_000_000));
    // 5000 ms: reports 0..5, 5..10, ... (the sixth is exactly 5 s after the first).
    let (bytes, r, frames) = clip(&["--frame-ms", "5000"]);
    assert_eq!((reports_len(&r), frames), (25, 5));
    assert_eq!(r.time_ns, times);
    // Each frame's receive time is its first report's time and it holds five reports.
    let mut buf = vec![0u8; MAX_FRAME_LEN];
    let mut rd = FrameReader::new(&mut buf).unwrap();
    rd.push(&bytes);
    let mut k = 0;
    while let Some(f) = rd.next() {
        let f = f.unwrap();
        assert_eq!(f.recv_unix_ns, times[k * 5]);
        let mut one = Reports::default();
        assert_eq!(decode_payload(f.payload, &mut one).unwrap(), 5);
        k += 1;
    }
    // 2500 ms: 3 reports per frame (0, 1, 2 s), the fourth is 3 s later.
    let (_, r, frames) = clip(&["--frame-ms", "2500"]);
    assert_eq!((reports_len(&r), frames), (25, 9));
    assert_eq!(r.time_ns, times);
    // The largest span holds all of them.
    let (_, r, frames) = clip(&["--frame-ms", "60000"]);
    assert_eq!((reports_len(&r), frames), (25, 1));
    assert_eq!(r.time_ns, times);
}

// REQ: OA-ADSB-018
#[test]
fn clip_refuses_a_frame_span_outside_1_to_60000_ms() {
    let tar = one_trace_tar("flag.tar", 3);
    let out = tmp("flag.oaf");
    let so = out.to_str().unwrap();
    let exe = env!("CARGO_BIN_EXE_oa-adsb-clip");
    for bad in [
        &["--frame-ms", "0"][..],
        &["--frame-ms", "60001"],
        &["--frame-ms", "-5"],
        &["--frame-ms", "1.5"],
        &["--frame-ms", "soon"],
        &["--frame-ms"],
    ] {
        let mut a = vec!["--out", so];
        a.extend_from_slice(bad);
        a.push(&tar);
        // `--frame-ms` last in the list takes the part as its value and then has no parts.
        let o = Command::new(exe).args(&a).output().unwrap();
        assert_eq!(o.status.code(), Some(2), "{bad:?}");
        assert!(
            String::from_utf8_lossy(&o.stderr).contains("usage"),
            "{bad:?}"
        );
    }
    for ok in ["1", "60000"] {
        let o = Command::new(exe)
            .args(["--out", so, "--frame-ms", ok, &tar])
            .output()
            .unwrap();
        assert!(o.status.success(), "{ok}");
    }
}

// REQ: OA-ADSB-016
#[test]
fn real_day_archive_first_part() {
    let Some(root) = std::env::var_os("OPENATCA_DATA") else {
        eprintln!("skip: OPENATCA_DATA not set");
        return;
    };
    let dir = PathBuf::from(root).join("raw/adsblol");
    // Only unsealed days are considered: a sealed day directory or part is skipped and counted,
    // never opened (OA-SEAL-012), and none left means the test skips.
    let part = oa_sealed::walk_unsealed(&dir).ok().and_then(|w| {
        eprintln!("sealed entries skipped: {}", w.skipped);
        let mut v: Vec<PathBuf> = w
            .files
            .into_iter()
            .filter(|p| p.to_string_lossy().ends_with(".tar.aa"))
            .collect();
        v.sort();
        v.pop()
    });
    let Some(part) = part else {
        eprintln!("skip: no unsealed *.tar.aa under {}", dir.display());
        return;
    };
    // The whole 2 GB part takes minutes without optimisation; by default read a prefix.
    // OPENATCA_ADSB_FULL=1 reads all of it (use a release build).
    let limit = if std::env::var_os("OPENATCA_ADSB_FULL").is_some() {
        u64::MAX
    } else {
        8 * 1024 * 1024
    };
    let f = oa_sealed::open(&part).unwrap().take(limit);
    let mut all = Reports::default();
    let res = scan_archive(
        vec![std::io::BufReader::new(f)],
        &ZAU_BOX,
        &mut |r: &Reports| -> Result<(), ()> {
            reports_extend(&mut all, r);
            Ok(())
        },
    )
    .unwrap();
    let st = res.stats;
    let s = summarize(&all);
    eprintln!(
        "real {}: traces_read={} refused={} rows_seen={} kept={} aircraft={} nac_p_known={} nac_p>=8={} ({:.2}% of known, {:.2}% of kept) pairs={} jumps={} jitter={} max_implied_kt={:.0} archive_complete={}",
        part.display(), st.traces_read, st.traces_refused, st.rows_seen, st.kept, s.aircraft,
        s.nac_p_known, s.nac_p_ge_8,
        100.0 * s.nac_p_ge_8 as f64 / s.nac_p_known.max(1) as f64,
        100.0 * s.nac_p_ge_8 as f64 / s.reports.max(1) as f64,
        s.pairs, s.jumps, s.jitter, s.max_implied_kt,
        std::mem::discriminant(&res.end) == std::mem::discriminant(&ScanEnd::Complete),
    );
    assert!(st.traces_read > 100, "{st:?}");
    assert_eq!(st.traces_refused, 0, "{:?}", res.error_samples);
    assert_eq!(st.rejected.iter().sum::<u64>(), 0, "{:?}", st.rejected);
    assert!(st.kept > 0 && s.aircraft > 0);
    assert!(s.nac_p_known > 0);
    // Real positions do not jump: over 99.9 % of consecutive pairs are plausible.
    assert!(
        s.jumps * 1000 < s.pairs.max(1),
        "{} jumps of {} pairs",
        s.jumps,
        s.pairs
    );
    // The scan ended because the cap or the part ended, never on a refusal.
    assert!(matches!(res.end, ScanEnd::Truncated(_) | ScanEnd::Complete));
}

/// A tar with one trace of `n` rows at one-second steps whose first row (offset 1000 s from the
/// trace base) is at `first_unix_s`.
fn trace_tar_at(name: &str, first_unix_s: i64, n: usize) -> String {
    let mut t = Vec::new();
    let rows = in_box(n, 41.0);
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    let json = format!(
        "{{\"icao\":\"aaaaaa\",\"timestamp\":{}.000,\"trace\":[{}]}}",
        first_unix_s - 1000,
        rows.join(",")
    );
    tar_entry(
        &mut t,
        "./traces/aa/trace_full_aaaaaa.json",
        json.as_bytes(),
    );
    t.resize(t.len() + 2 * BLOCK, 0);
    let p = tmp(name);
    std::fs::write(&p, &t).unwrap();
    p.to_str().unwrap().to_string()
}

// REQ: OA-SEAL-015
#[test]
fn clip_refuses_a_report_in_a_sealed_window_and_writes_a_source_sidecar_otherwise() {
    // The window starts at 2026-11-20T00:00:00Z = 1795132800 s.
    const FROM: i64 = 1_795_132_800;
    let exe = env!("CARGO_BIN_EXE_oa-adsb-clip");
    let run = |args: &[&str]| Command::new(exe).args(args).output().unwrap();
    // Rows at from-1 s, from and from+1 s: refused, exit 2, nothing written, the count printed.
    let tar = trace_tar_at("seal-edge.tar", FROM - 1, 3);
    let out = tmp("seal-edge.oaf");
    let side = tmp("seal-edge.oaf.source");
    let _ = std::fs::remove_file(&out);
    let _ = std::fs::remove_file(&side);
    let o = run(&["--out", out.to_str().unwrap(), &tar]);
    let err = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(2), "{err}");
    assert!(err.contains("sealed-window rows dropped: 2"), "{err}");
    assert!(!out.exists() && !side.exists());
    // With --to at the window start only the row at from-1 is clipped: it is kept, the rows at
    // from and after are outside the time window, and the count is 0.
    let o = run(&[
        "--out",
        out.to_str().unwrap(),
        "--to",
        &FROM.to_string(),
        &tar,
    ]);
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(o.status.success(), "{err}");
    assert!(err.contains("sealed-window rows dropped: 0"), "{err}");
    let (_, r, _) = read_frames(&out);
    assert_eq!(r.time_ns, [(FROM - 1) * 1_000_000_000]);
    // The sidecar names the input with its SHA-256, the window and the counts.
    let text = std::fs::read_to_string(&side).unwrap();
    let want = oa_sha256::to_hex(&oa_sha256::sha256(&std::fs::read(&tar).unwrap()));
    assert!(
        text.contains(&format!("input: {tar} sha256 {want}\n")),
        "{text}"
    );
    assert!(
        text.contains(&format!("window_to_unix_ns: {}\n", FROM * 1_000_000_000)),
        "{text}"
    );
    assert!(text.contains("window_from_unix_ns: none\n"), "{text}");
    for k in [
        "rows_seen: 3",
        "outside_time: 2",
        "kept: 1",
        "frames: 1",
        "sealed-window rows dropped: 0",
    ] {
        assert!(text.contains(k), "{k} {text}");
    }
    // A report at the window end (to = 2026-11-23T00:17:00Z = 1795393020 s) is not sealed.
    let tar = trace_tar_at("seal-after.tar", 1_795_393_020, 1);
    let o = run(&["--out", out.to_str().unwrap(), &tar]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    // Standard output has no sidecar.
    let o = run(&["--out", "-", &tar]);
    assert!(o.status.success());
}
