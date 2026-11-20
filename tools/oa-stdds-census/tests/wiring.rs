//! The binary on tiny synthetic inputs: a clip and a STDDS folder of another day exit 2 with
//! the message, a window that holds no update says so, and a too-small budget refuses before
//! the ADS-B index is built. Temp files only.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use oa_adsb::{encode_payload, reports_push, Reports, Row, ROW_EMPTY};
use oa_feed::frame::{encode_frame, Kind, MAX_FRAME_LEN};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn frame(payload: &[u8]) -> Vec<u8> {
    let mut buf = vec![0u8; MAX_FRAME_LEN];
    let n = encode_frame(&mut buf, Kind::Message as u8, 0, 0, 0, 0, payload).unwrap();
    buf.truncate(n);
    buf
}

/// A clip with one aircraft reporting once a second from `t0_s` (Unix seconds) for ten seconds.
fn write_clip(path: &Path, t0_s: i64) {
    let mut r = Reports::default();
    for s in 0..10 {
        reports_push(
            &mut r,
            &Row {
                icao: 1,
                time_ns: (t0_s + s) * 1_000_000_000,
                lat_e7: 400_000_000,
                lon_e7: -750_000_000,
                baro_alt_ft: 10_000,
                gs_dkt: 4000,
                track_ddeg: 900,
                nac_p: 10,
                nic: 8,
                sil: 3,
                version: 2,
                ..ROW_EMPTY
            },
        );
    }
    let mut p = Vec::new();
    encode_payload(&mut p, &r, 0, 10).unwrap();
    std::fs::write(path, frame(&p)).unwrap();
}

/// A STDDS day folder (`rec/stdds`) with one update at `iso` (UTC).
fn write_stdds(day: &Path, iso: &str) {
    write_stdds_many(day, &[iso]);
}

/// A STDDS day folder with one update of track 1 at each of `isos` (UTC).
fn write_stdds_many(day: &Path, isos: &[&str]) {
    let d = day.join("rec").join("stdds");
    std::fs::create_dir_all(&d).unwrap();
    let mut xml = "<?xml version=\"1.0\"?><ns2:TATrackAndFlightPlan xmlns:ns2=\"urn:us:gov:dot:faa:atm:terminal:entities:v4-0:tais:terminalautomationinformation\"><src>AAA</src>".to_string();
    for iso in isos {
        xml += &format!("<record><track><trackNum>1</trackNum><mrtTime>{iso}</mrtTime><adsb>false</adsb><lat>40.0</lat><lon>-75.0</lon></track></record>");
    }
    xml += "</ns2:TATrackAndFlightPlan>";
    std::fs::write(
        d.join("scds-stdds-20261124T000000Z-0001.oaf"),
        frame(xml.as_bytes()),
    )
    .unwrap();
}

fn temp(tag: &str) -> PathBuf {
    let t = std::env::temp_dir().join(format!("oa-census-wire-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&t);
    std::fs::create_dir_all(&t).unwrap();
    t
}

fn run(args: &[&Path], extra: &[&str], env: &[(&str, &str)]) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_oa-stdds-census"));
    c.args(args).args(extra);
    for (k, v) in env {
        c.env(k, v);
    }
    c.output().unwrap()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

// 2026-11-24T10:00:00Z and 2026-11-25T10:00:00Z in Unix seconds.
const D24: i64 = 1_795_514_400;
const D25: i64 = D24 + 86_400;

// REQ: OA-CEN-009
#[test]
fn a_clip_and_a_folder_of_different_days_exit_2_with_the_spans_to_the_second() {
    let t = temp("days");
    let day = t.join("day-20261124");
    write_stdds(&day, "2026-11-24T10:00:30Z");
    let clip = t.join("clip.oaf");
    write_clip(&clip, D25);
    let o = run(&[&day, Path::new("--adsb"), &clip], &[], &[]);
    assert_eq!(o.status.code(), Some(2), "{}", err(&o));
    let e = err(&o);
    assert!(
        e.contains("do not overlap") && e.contains("wrong day or clip"),
        "{e}"
    );
    assert!(
        e.contains("2026-11-24 10:00:30 to 2026-11-24 10:00:30 UTC"),
        "{e}"
    );
    assert!(
        e.contains("2026-11-25 10:00:00 to 2026-11-25 10:00:09 UTC"),
        "{e}"
    );
    std::fs::remove_dir_all(&t).unwrap();
}

// REQ: OA-CEN-009, OA-SEAL-014
#[test]
fn a_window_with_no_stdds_update_says_so_and_overlapping_days_run() {
    let t = temp("window");
    let day = t.join("day-20261124");
    write_stdds(&day, "2026-11-24T10:00:30Z");
    let clip = t.join("clip.oaf");
    write_clip(&clip, D24 + 25);
    // 10:00:30 is second 36030 of the day; a window on 20000 to 21000 holds nothing.
    let o = run(
        &[&day, Path::new("--adsb"), &clip],
        &["--from", "20000", "--to", "21000"],
        &[],
    );
    assert_eq!(o.status.code(), Some(2), "{}", err(&o));
    assert!(
        err(&o).contains("no STDDS updates inside the window"),
        "{}",
        err(&o)
    );
    // The same inputs without the window overlap and run.
    let o = run(&[&day, Path::new("--adsb"), &clip], &[], &[]);
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    // The count line (recording frames, STDDS records and the clip's rows in one count), once.
    assert_eq!(
        err(&o).matches("sealed-window rows dropped: 0").count(),
        1,
        "{}",
        err(&o)
    );
    std::fs::remove_dir_all(&t).unwrap();
}

// REQ: OA-CEN-020
#[test]
fn a_budget_below_the_estimate_refuses_before_the_index_is_built() {
    let t = temp("budget");
    let day = t.join("day-20261124");
    write_stdds(&day, "2026-11-24T10:00:30Z");
    let clip = t.join("clip.oaf");
    write_clip(&clip, D24 + 25);
    // OA_STDDS_CENSUS_BUDGET_BYTES is for tests only.
    let o = run(
        &[&day, Path::new("--adsb"), &clip],
        &[],
        &[("OA_STDDS_CENSUS_BUDGET_BYTES", "100")],
    );
    assert_eq!(o.status.code(), Some(2), "{}", err(&o));
    assert!(
        err(&o).contains("building the ADS-B index is estimated"),
        "{}",
        err(&o)
    );
    std::fs::remove_dir_all(&t).unwrap();
}

// REQ: OA-CEN-023
#[test]
fn updates_after_the_clip_are_counted_apart_and_the_run_goes_on() {
    let t = temp("partial");
    let day = t.join("day-20261124");
    // The clip covers 10:00:25 to 10:00:34; the update at 10:00:50 is after it.
    write_stdds_many(&day, &["2026-11-24T10:00:30Z", "2026-11-24T10:00:50Z"]);
    let clip = t.join("clip.oaf");
    write_clip(&clip, D24 + 25);
    let o = run(&[&day, Path::new("--adsb"), &clip], &[], &[]);
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(
        out.contains("outside the ADS-B clip (time span) 1 (reconciles)"),
        "{out}"
    );
    assert!(
        out.contains("2026-11-24 10:00:30 to 2026-11-24 10:00:50 UTC"),
        "{out}"
    );
    std::fs::remove_dir_all(&t).unwrap();
}

// REQ: OA-CEN-005
#[test]
fn a_damaged_gzip_file_exits_3_after_the_report() {
    let t = temp("gzip");
    let d = t.join("day").join("rec").join("stdds");
    std::fs::create_dir_all(&d).unwrap();
    // The gzip magic, then a method byte no decoder takes, then junk.
    std::fs::write(
        d.join("scds-stdds-20261124T000000Z-0001.oaf.gz"),
        [0x1f, 0x8b, 0xff, 0xff, 0xde, 0xad, 0xbe, 0xef, 0, 1, 2, 3],
    )
    .unwrap();
    let o = run(&[&t.join("day")], &[], &[]);
    assert_eq!(o.status.code(), Some(3), "{}", err(&o));
    assert!(err(&o).contains("damaged"), "{}", err(&o));
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(
        out.contains("gzip damaged 1"),
        "the report is printed: {out}"
    );
    std::fs::remove_dir_all(&t).unwrap();
}

// REQ: OA-SEAL-014
#[test]
fn the_census_reader_drops_the_sealed_window() {
    let w = oa_stdds_census::read::read_limits().drop_window;
    assert_eq!(w, oa_sealed::DropWindow::sealed());
    assert_eq!(w.span(), oa_sealed::sealed_window_ms());
}
