//! The damage check of P-006 section 6: the interval logic on synthetic times, and the readers on
//! small synthetic recordings.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use oa_feed::{encode_frame, FLAG_BYTES, MAX_FRAME_LEN};
use oa_feed_gaps::core::gaps::{reasons, scan_unsorted, Scan, Stats, Window, GAP_CAP};
use oa_feed_gaps::core::report::{render, Feed};
use oa_feed_gaps::core::timefmt::{iso, parse_iso};
use oa_feed_gaps::shell::args::parse;
use oa_feed_gaps::shell::read::{read_adsb, read_sfdps};
use oa_feed_gaps::shell::run::write_atomic;

const S: i64 = 1_000_000_000;
const MS: i64 = 1_000_000;
/// 2026-11-15T00:00:00Z, a development day.
const BASE: i64 = 20_772 * 86_400 * S;

fn win(len_s: i64) -> Window {
    Window::new(BASE, BASE + len_s * S).unwrap()
}

fn run(w: Window, times: &[i64]) -> Stats {
    let mut t: Vec<i64> = times.iter().map(|t| BASE + t).collect();
    scan_unsorted(w, &mut t)
}

fn verdict(st: &Stats, damage: &[(String, u64)]) -> (String, bool) {
    let f = Feed {
        name: "SFDPS",
        stats: st.clone(),
        damage: damage.to_vec(),
        info: vec![],
    };
    render(st.window, &[f])
}

// REQ: OA-GAP-001
#[test]
fn a_feed_without_a_gap_is_clean() {
    let ts: Vec<i64> = (0..=60).map(|k| k * 60 * S).collect();
    let st = run(win(3600), &ts);
    assert_eq!(st.count, 61);
    assert_eq!(st.largest.unwrap().len_ns(), 60 * S);
    assert_eq!(st.over_total, 0);
    let (text, damaged) = verdict(&st, &[]);
    assert!(!damaged && text.ends_with("CLEAN\n"), "{text}");
}

// REQ: OA-GAP-001
#[test]
fn an_interval_of_exactly_600_s_is_clean_and_600_001_s_is_damaged() {
    let w = win(1200);
    let st = run(w, &[0, 600 * S, 1200 * S]);
    assert_eq!(st.largest.unwrap().len_ns(), 600 * S);
    assert!(reasons("SFDPS", &st, &[]).is_empty());
    let st = run(w, &[0, 600 * S + MS, 1200 * S]);
    assert_eq!(st.over_total, 1);
    let g = st.over[0];
    assert_eq!((g.start_ns, g.end_ns), (BASE, BASE + 600 * S + MS));
    let (text, damaged) = verdict(&st, &[]);
    assert!(
        damaged && text.contains("DAMAGED: SFDPS has 1 interval(s) over 600 s"),
        "{text}"
    );
    assert!(
        text.contains("600.001000000 s from 2026-11-15T00:00:00.000000000Z"),
        "{text}"
    );
}

// REQ: OA-GAP-002
#[test]
fn a_gap_at_the_start_or_the_end_of_the_window_counts() {
    let w = win(3600);
    // First message 601 s after the window start.
    let st = run(
        w,
        &[601 * S, 1200 * S, 1800 * S, 2400 * S, 3000 * S, 3600 * S],
    );
    assert_eq!(st.over[0].start_ns, BASE);
    assert_eq!(st.over[0].end_ns, BASE + 601 * S);
    assert_eq!(st.first_ns, Some(BASE + 601 * S));
    assert!(!reasons("SFDPS", &st, &[]).is_empty());
    // Last message 601 s before the window end.
    let st = run(w, &[0, 600 * S, 1200 * S, 1800 * S, 2400 * S, 2999 * S]);
    assert_eq!(st.over[0].start_ns, BASE + 2999 * S);
    assert_eq!(st.over[0].end_ns, BASE + 3600 * S);
    assert_eq!(st.last_ns, Some(BASE + 2999 * S));
    assert!(!reasons("SFDPS", &st, &[]).is_empty());
    // Exactly 600 s at either edge is clean.
    let st = run(w, &[600 * S, 1200 * S, 1800 * S, 2400 * S, 3000 * S]);
    assert!(reasons("SFDPS", &st, &[]).is_empty());
    // Times outside the window are not counted.
    let st = run(w, &[-5 * S, 0, 3600 * S, 3601 * S]);
    assert_eq!(st.count, 2);
}

// REQ: OA-GAP-003
#[test]
fn a_window_with_no_message_is_damaged() {
    let st = run(win(3600), &[-10 * S, 4000 * S]);
    assert_eq!(st.count, 0);
    let (text, damaged) = verdict(&st, &[]);
    assert!(damaged, "{text}");
    assert!(text.contains("DAMAGED: SFDPS has no message in the window"));
    assert!(text.contains("first message: none"));
}

// REQ: OA-GAP-004
#[test]
fn the_gap_list_is_capped_counts_the_overflow_and_stays_damaged() {
    let n = GAP_CAP as i64 + 50;
    let ts: Vec<i64> = (0..=n).map(|k| k * 601 * S).collect();
    let st = run(win(n * 601 + 1), &ts);
    assert_eq!(st.over.len(), GAP_CAP);
    // n intervals between messages, plus the 1 s left at the end of the window, which is not over.
    assert_eq!(st.over_total, n as u64);
    assert_eq!(st.overflow, 50);
    let (text, damaged) = verdict(&st, &[]);
    assert!(damaged && text.contains("50 more are not listed"), "{text}");
}

// REQ: OA-GAP-005
#[test]
fn a_reader_error_count_is_damage_even_with_no_gap() {
    let ts: Vec<i64> = (0..=60).map(|k| k * 60 * S).collect();
    let st = run(win(3600), &ts);
    let (text, damaged) = verdict(&st, &[("reader frame-crc".to_string(), 2)]);
    assert!(
        damaged && text.contains("DAMAGED: SFDPS reader frame-crc: 2"),
        "{text}"
    );
    let (_, damaged) = verdict(&st, &[("reader frame-crc".to_string(), 0)]);
    assert!(!damaged);
}

// REQ: OA-GAP-006
#[test]
fn a_time_out_of_order_is_counted_and_left_out_of_the_intervals() {
    let mut sc = Scan::new(win(100));
    for t in [10, 20, 15, 30] {
        sc.observe(BASE + t * S);
    }
    let st = sc.finish();
    assert_eq!((st.count, st.out_of_order), (3, 1));
    assert!(!reasons("SFDPS", &st, &[]).is_empty());
}

fn tmp(name: &str) -> PathBuf {
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("gaps-{name}"));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

fn frames(times: &[i64]) -> Vec<u8> {
    let mut out = Vec::new();
    for (k, t) in times.iter().enumerate() {
        let mut buf = vec![0u8; MAX_FRAME_LEN];
        let n = encode_frame(&mut buf, 1, FLAG_BYTES, k as u64, BASE + t, 0, b"<x/>").unwrap();
        out.extend_from_slice(&buf[..n]);
    }
    out
}

fn sfdps_file(dir: &Path, n: u32, times_s: &[i64]) -> PathBuf {
    let p = dir.join(format!("scds-sfdps-20261115T000000Z-{n:04}.oaf"));
    let t: Vec<i64> = times_s.iter().map(|s| s * S).collect();
    fs::write(&p, frames(&t)).unwrap();
    p
}

// REQ: OA-GAP-007
#[test]
fn overlapping_recording_files_out_of_order_are_read_by_receive_time() {
    let d = tmp("overlap");
    // The later file by name holds the earlier times, and the two overlap.
    sfdps_file(&d, 0, &[40, 50, 60]);
    sfdps_file(&d, 1, &[10, 20, 30, 45]);
    let f = read_sfdps(&d, win(70)).unwrap();
    assert_eq!(f.stats.count, 7);
    assert_eq!(f.stats.out_of_order, 0);
    assert!(f
        .info
        .iter()
        .any(|(k, n)| k.starts_with("frames earlier") && *n > 0));
    assert_eq!(f.stats.first_ns, Some(BASE + 10 * S));
    assert_eq!(f.stats.last_ns, Some(BASE + 60 * S));
    assert_eq!(f.stats.largest.unwrap().len_ns(), 10 * S);
    assert!(f.damage.is_empty());
}

// REQ: OA-GAP-008
#[test]
fn a_damaged_frame_in_the_recording_is_counted_as_damage() {
    let d = tmp("damaged");
    let p = sfdps_file(&d, 0, &[10, 20, 30]);
    // Cut the last frame short.
    let mut b = fs::read(&p).unwrap();
    b.truncate(b.len() - 5);
    fs::write(&p, b).unwrap();
    let f = read_sfdps(&d, win(40)).unwrap();
    assert_eq!(f.stats.count, 2);
    let total: u64 = f.damage.iter().map(|(_, n)| n).sum();
    assert!(total > 0, "{:?}", f.damage);
    let (text, damaged) = render(win(40), &[f]);
    assert!(damaged && text.contains("DAMAGED:"), "{text}");
}

fn adsb_clip(path: &Path, times_s: &[i64]) {
    let mut r = oa_adsb::Reports::default();
    for (i, t) in times_s.iter().enumerate() {
        let mut row = oa_adsb::ROW_EMPTY;
        row.icao = 0xA00000 + i as u32;
        row.time_ns = BASE + t * S;
        row.lat_e7 = 410_000_000;
        row.lon_e7 = -880_000_000;
        row.nic = 8;
        row.nac_p = 10;
        row.sil = 3;
        row.version = 2;
        row.source = 0;
        oa_adsb::reports_push(&mut r, &row);
    }
    let mut bytes = Vec::new();
    for k in 0..times_s.len() {
        let mut payload = Vec::new();
        oa_adsb::encode_payload(&mut payload, &r, k, k + 1).unwrap();
        let mut buf = vec![0u8; MAX_FRAME_LEN];
        let n = encode_frame(&mut buf, 1, FLAG_BYTES, k as u64, BASE + 1, 0, &payload).unwrap();
        bytes.extend_from_slice(&buf[..n]);
    }
    fs::write(path, bytes).unwrap();
}

// REQ: OA-GAP-009
#[test]
fn the_adsb_clip_is_checked_sorted_for_gaps_and_for_damaged_frames() {
    let d = tmp("adsb");
    let clip = d.join("c.oaf");
    // Reports are not in time order in the file.
    adsb_clip(&clip, &[30, 0, 20, 10, 40]);
    let f = read_adsb(&clip, win(40)).unwrap();
    assert_eq!(
        (f.stats.count, f.stats.largest.unwrap().len_ns()),
        (5, 10 * S)
    );
    assert!(f.damage.iter().all(|(_, n)| *n == 0));
    // A gap of 700 s.
    adsb_clip(&clip, &[0, 700, 710]);
    let f = read_adsb(&clip, win(710)).unwrap();
    assert_eq!(f.stats.over_total, 1);
    assert!(render(win(710), &[f]).1);
    // A flipped byte in a frame is a damaged frame.
    adsb_clip(&clip, &[0, 10, 20]);
    let mut b = fs::read(&clip).unwrap();
    let mid = b.len() / 2;
    b[mid] ^= 0xFF;
    fs::write(&clip, b).unwrap();
    let f = read_adsb(&clip, win(20)).unwrap();
    assert!(
        f.damage
            .iter()
            .any(|(k, n)| k == "damaged frames" && *n > 0),
        "{:?}",
        f.damage
    );
    assert!(render(win(20), &[f]).1);
}

const BIN: &str = env!("CARGO_BIN_EXE_oa-feed-gaps");

fn cli(d: &Path, from: &str, to: &str, extra: &[&str]) -> std::process::Output {
    Command::new(BIN)
        .arg("--sfdps-dir")
        .arg(d.join("sfdps"))
        .arg("--adsb")
        .arg(d.join("c.oaf"))
        .args(["--from", from, "--to", to])
        .args(extra)
        .output()
        .unwrap()
}

// REQ: OA-GAP-010
#[test]
fn the_exit_status_is_0_clean_1_damaged_2_error_and_the_report_is_written_atomically() {
    let d = tmp("cli");
    fs::create_dir_all(d.join("sfdps")).unwrap();
    sfdps_file(&d.join("sfdps"), 0, &[0, 300, 600, 900]);
    adsb_clip(&d.join("c.oaf"), &[0, 300, 600, 900]);
    let (from, to) = ("2026-11-15T00:00:00Z", "2026-11-15T00:15:00Z");
    let o = cli(&d, from, to, &[]);
    assert_eq!(o.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&o.stdout).ends_with("CLEAN\n"));
    let out = d.join("report.txt");
    let o = cli(&d, from, to, &["--out", out.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0));
    assert!(o.stdout.is_empty());
    assert!(fs::read_to_string(&out).unwrap().ends_with("CLEAN\n"));
    assert!(!d.join("report.txt.partial").exists());
    // The same report twice is the same bytes.
    let first = fs::read(&out).unwrap();
    cli(&d, from, to, &["--out", out.to_str().unwrap()]);
    assert_eq!(first, fs::read(&out).unwrap());
    // A window that runs on 700 s past the last message is damaged.
    let o = cli(&d, from, "2026-11-15T00:30:00Z", &[]);
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stdout).contains("DAMAGED:"));
    // Usage and read errors.
    assert_eq!(cli(&d, from, "nonsense", &[]).status.code(), Some(2));
    assert_eq!(cli(&d, to, from, &[]).status.code(), Some(2));
    let o = Command::new(BIN).output().unwrap();
    assert_eq!(o.status.code(), Some(2));
    fs::remove_file(d.join("c.oaf")).unwrap();
    assert_eq!(cli(&d, from, to, &[]).status.code(), Some(2));
    // A failed write leaves no partial file and is exit 2.
    let bad = d.join("no-such-dir/report.txt");
    assert!(write_atomic(&bad, "x").is_err());
}

// REQ: OA-GAP-011
#[test]
fn a_sealed_input_is_refused_through_the_shared_guard() {
    let t = tmp("sealed");
    let sealed = t.join("scds/stdds-smoke-20261119T192936Z");
    fs::create_dir_all(&sealed).unwrap();
    fs::write(sealed.join("clip.oaf"), b"x").unwrap();
    let o = Command::new(BIN)
        .arg("--sfdps-dir")
        .arg(&sealed)
        .arg("--adsb")
        .arg(sealed.join("clip.oaf"))
        .args([
            "--from",
            "2026-11-15T00:00:00Z",
            "--to",
            "2026-11-15T01:00:00Z",
        ])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("OA-SEAL-001"));
    assert!(o.stdout.is_empty());
}

// REQ: OA-GAP-012
#[test]
fn times_and_arguments_are_parsed_strictly() {
    assert_eq!(parse_iso("2026-11-15T00:00:00Z"), Some(BASE));
    assert_eq!(iso(BASE + 5 * MS), "2026-11-15T00:00:00.005000000Z");
    for bad in [
        "2026-11-15T00:00:00",
        "2026-11-15 00:00:00Z",
        "2026-02-30T00:00:00Z",
        "2026-11-15T24:00:00Z",
        "1969-12-31T23:59:59Z",
        "",
    ] {
        assert_eq!(parse_iso(bad), None, "{bad}");
    }
    let a = |v: &[&str]| parse(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    let ok = [
        "--sfdps-dir",
        "d",
        "--adsb",
        "c",
        "--from",
        "2026-11-15T00:00:00Z",
        "--to",
        "2026-11-15T01:00:00Z",
    ];
    assert!(a(&ok).is_ok());
    assert!(a(&ok[..ok.len() - 2]).is_err());
    assert!(a(&[&ok[..], &["--bogus", "x"]].concat()).is_err());
    assert!(a(&[&ok[..], &["--from", "2026-11-15T00:00:00Z"]].concat()).is_err());
}
