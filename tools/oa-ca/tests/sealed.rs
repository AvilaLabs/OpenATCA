//! `oa-ca` drops the clip rows whose time is in a sealed window and prints the count once, on
//! every exit (a temp dir, a clip made here).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

const DEV_CFG: &str = include_str!("../testdata/dev.cfg");

// REQ: OA-CA-031
#[test]
fn the_program_drops_and_counts_clip_rows_in_a_sealed_window_and_prints_the_line_once() {
    // The window of the list: 2026-11-19T23:45:00Z to 2026-11-23T00:32:00Z, Unix nanoseconds.
    const FROM: i64 = 1_795_131_900_000_000_000;
    const TO: i64 = 1_795_393_920_000_000_000;
    let t = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("ca-window");
    let _ = fs::remove_dir_all(&t);
    fs::create_dir_all(&t).unwrap();
    let mut r = oa_adsb::Reports::default();
    for (k, time) in [FROM - 1_000_000_000, FROM, TO - 1, TO + 1_000_000_000]
        .into_iter()
        .enumerate()
    {
        let mut row = oa_adsb::ROW_EMPTY;
        row.time_ns = time;
        row.icao = 0xA00001 + k as u32;
        row.lat_e7 = 410_000_000;
        row.lon_e7 = -880_000_000;
        row.nic = 8;
        row.nac_p = 10;
        row.sil = 3;
        row.version = 2;
        oa_adsb::reports_push(&mut r, &row);
    }
    // One row per frame: a frame with a row in the window is dropped whole.
    let mut bytes = Vec::new();
    for k in 0..4 {
        let mut payload = Vec::new();
        oa_adsb::encode_payload(&mut payload, &r, k, k + 1).unwrap();
        let mut buf = vec![0u8; payload.len() + 256];
        let n = oa_feed::encode_frame(&mut buf, 1, oa_feed::FLAG_BYTES, 0, 0, 0, &payload).unwrap();
        bytes.extend_from_slice(&buf[..n]);
    }
    let clip = t.join("neutral.oaf");
    fs::write(&clip, &bytes).unwrap();
    let cfg = t.join("dev.cfg");
    fs::write(&cfg, DEV_CFG).unwrap();
    let run = |out: &str, cfgp: &PathBuf| {
        Command::new(env!("CARGO_BIN_EXE_oa-ca"))
            .arg("--adsb")
            .arg(&clip)
            .arg("--config")
            .arg(cfgp)
            .arg("--out")
            .arg(t.join(out))
            .output()
            .unwrap()
    };
    let line = "sealed-window rows dropped: 2";
    let o = run("out", &cfg);
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(o.status.success(), "{err}");
    // Two rows (at from and at to - 1 ns) are dropped; the line is on standard error once and in
    // the summary.
    assert_eq!(err.matches(line).count(), 1, "{err}");
    let summary = fs::read_to_string(t.join("out/summary.txt")).unwrap();
    assert!(summary.contains(line), "{summary}");
    // The configuration is read before the clip, so a bad one fails with the clip unread: the
    // line is printed once, with a count of 0.
    let bad = t.join("bad.cfg");
    fs::write(&bad, "not a configuration\n").unwrap();
    let o = run("out-bad", &bad);
    let err = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(1), "{err}");
    assert_eq!(
        err.matches("sealed-window rows dropped: 0").count(),
        1,
        "{err}"
    );
    assert_eq!(err.matches("sealed-window rows dropped: ").count(), 1, "{err}");
    // A run that fails after the clip is read (the output directory cannot be made: its parent
    // is a file) carries the true count, 2, once.
    let file = t.join("a-file");
    fs::write(&file, b"x").unwrap();
    let o = run("a-file/out", &cfg);
    let err = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(1), "{err}");
    assert_eq!(err.matches(line).count(), 1, "{err}");
    assert_eq!(err.matches("sealed-window rows dropped: ").count(), 1, "{err}");
    // A usage error and a refusal print the line too, with a count of 0.
    let o = Command::new(env!("CARGO_BIN_EXE_oa-ca")).output().unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(2), "{err}");
    assert_eq!(
        err.matches("sealed-window rows dropped: 0").count(),
        1,
        "{err}"
    );
    let _ = fs::remove_dir_all(&t);
}

// REQ: OA-CA-031
#[test]
fn a_clip_that_fails_after_rows_were_dropped_still_reports_the_true_count() {
    use std::io::{self, Cursor, Read};
    use std::rc::Rc;

    const FROM: i64 = 1_795_131_900_000_000_000;
    /// The clip's bytes, then a read error: damage in the middle of the clip.
    struct Damaged(Cursor<Vec<u8>>);
    impl Read for Damaged {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            match self.0.read(out)? {
                0 => Err(io::Error::other("disk gone")),
                n => Ok(n),
            }
        }
    }
    let t = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("ca-window-damaged");
    let _ = fs::remove_dir_all(&t);
    fs::create_dir_all(&t).unwrap();
    // Three rows in the window, one frame each, then the read fails.
    let mut r = oa_adsb::Reports::default();
    for k in 0..3u32 {
        let mut row = oa_adsb::ROW_EMPTY;
        row.time_ns = FROM + i64::from(k) * 1_000_000_000;
        row.icao = 0xA00001 + k;
        row.lat_e7 = 410_000_000;
        row.lon_e7 = -880_000_000;
        row.nic = 8;
        row.nac_p = 10;
        row.sil = 3;
        row.version = 2;
        oa_adsb::reports_push(&mut r, &row);
    }
    let mut bytes = Vec::new();
    for k in 0..3 {
        let mut payload = Vec::new();
        oa_adsb::encode_payload(&mut payload, &r, k, k + 1).unwrap();
        let mut buf = vec![0u8; payload.len() + 256];
        let n = oa_feed::encode_frame(&mut buf, 1, oa_feed::FLAG_BYTES, 0, 0, 0, &payload).unwrap();
        bytes.extend_from_slice(&buf[..n]);
    }
    let cfg = t.join("dev.cfg");
    fs::write(&cfg, DEV_CFG).unwrap();
    let args = oa_ca_run::args::Args {
        adsb: t.join("never-opened.oaf"),
        config: cfg,
        out: t.join("out"),
        scorer_half: false,
    };
    let open: oa_bench::input::Opener =
        Rc::new(move |_| Ok(Box::new(Damaged(Cursor::new(bytes.clone()))) as Box<dyn Read>));
    let mut sealed = 0u64;
    let e = oa_ca_run::run::run_counted_with(&args, &open, &mut sealed).unwrap_err();
    assert!(e.contains("disk gone"), "{e}");
    assert_eq!(sealed, 3);
    assert!(!t.join("out/summary.txt").exists());
    let _ = fs::remove_dir_all(&t);
}
