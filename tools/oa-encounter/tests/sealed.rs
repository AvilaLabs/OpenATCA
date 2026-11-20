//! `oa-encounter` refuses a link into sealed data (a fake tree in a temp dir): exit 2, the rule
//! named, nothing created.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::fs;
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::process::Command;

// REQ: OA-SEAL-005
#[test]
fn a_link_into_sealed_data_is_refused_for_the_clip_the_output_and_the_nasr_file() {
    let t = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("enc-sealed");
    let _ = fs::remove_dir_all(&t);
    let sealed = t.join("scds/stdds-smoke-20261119T192936Z");
    fs::create_dir_all(&sealed).unwrap();
    fs::write(sealed.join("clip.oaf"), b"x").unwrap();
    symlink(sealed.join("clip.oaf"), t.join("clip-link.oaf")).unwrap();
    symlink(&sealed, t.join("dir-link")).unwrap();
    fs::write(t.join("fine.oaf"), b"x").unwrap();
    let out = t.join("out");
    let run = |adsb: PathBuf, o: PathBuf, nasr: Option<PathBuf>| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_oa-encounter"));
        c.arg("--adsb").arg(adsb).arg("--out").arg(o);
        if let Some(n) = nasr {
            c.arg("--nasr").arg(n);
        }
        c.output().unwrap()
    };
    for o in [
        run(t.join("clip-link.oaf"), out.clone(), None),
        run(t.join("dir-link/clip.oaf"), out.clone(), None),
        run(t.join("fine.oaf"), t.join("dir-link/new"), None),
        run(
            t.join("fine.oaf"),
            out.clone(),
            Some(t.join("clip-link.oaf")),
        ),
    ] {
        assert_eq!(o.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&o.stderr).contains("OA-SEAL-001"));
    }
    assert!(!out.exists() && !sealed.join("new").exists());
    let _ = fs::remove_dir_all(&t);
}

// REQ: OA-SEAL-014
#[test]
fn the_program_prints_the_count_of_clip_rows_dropped_for_a_sealed_window_once() {
    // The window of the list: 2026-11-19T23:45:00Z to 2026-11-23T00:32:00Z (the listed window widened by 15 minutes), Unix nanoseconds.
    const FROM: i64 = 1_795_131_900_000_000_000;
    const TO: i64 = 1_795_393_920_000_000_000;
    let t = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("enc-window");
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
    let mut payload = Vec::new();
    oa_adsb::encode_payload(&mut payload, &r, 0, 4).unwrap();
    let mut buf = vec![0u8; payload.len() + 256];
    let n = oa_feed_frame(&mut buf, &payload);
    let clip = t.join("neutral.oaf");
    fs::write(&clip, &buf[..n]).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_oa-encounter"))
        .arg("--adsb")
        .arg(&clip)
        .arg("--out")
        .arg(t.join("out"))
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(o.status.success(), "{err}");
    // Two rows (at from and at to - 1 ns) are dropped; the line is on standard error once and in
    // the summary.
    assert_eq!(
        err.matches("sealed-window rows dropped: 2").count(),
        1,
        "{err}"
    );
    let summary = fs::read_to_string(t.join("out/summary.txt")).unwrap();
    assert!(
        summary.contains("sealed-window rows dropped: 2"),
        "{summary}"
    );
    // The line is printed on the other exits too: `--hours` (which returns before the encounters
    // are found) and a failed run (the SFDPS directory is not there).
    let o = Command::new(env!("CARGO_BIN_EXE_oa-encounter"))
        .arg("--adsb")
        .arg(&clip)
        .arg("--out")
        .arg(t.join("out-hours"))
        .arg("--hours")
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(o.status.success(), "{err}");
    assert_eq!(
        err.matches("sealed-window rows dropped: 2").count(),
        1,
        "{err}"
    );
    let o = Command::new(env!("CARGO_BIN_EXE_oa-encounter"))
        .arg("--adsb")
        .arg(&clip)
        .arg("--out")
        .arg(t.join("out-fail"))
        .arg("--sfdps")
        .arg(t.join("no-such-dir"))
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(1), "{err}");
    assert_eq!(
        err.matches("sealed-window rows dropped: 2").count(),
        1,
        "{err}"
    );
    let _ = fs::remove_dir_all(&t);
}

/// One OAF1 message frame holding `payload`, written into `buf`; its length.
fn oa_feed_frame(buf: &mut [u8], payload: &[u8]) -> usize {
    oa_feed::encode_frame(buf, 1, oa_feed::FLAG_BYTES, 0, 0, 0, payload).unwrap()
}
