//! The ADS-B clip loader drops and counts the rows whose time is in a sealed window (OA-SEAL-014).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::PathBuf;

use oa_adsb::{encode_payload, reports_len, reports_push, Reports, ROW_EMPTY};
use oa_bench::input::{for_each_adsb_frame_counted, load_adsb};
use oa_feed::{encode_frame, FLAG_BYTES, MAX_FRAME_LEN};

/// The window of the list: 2026-11-19T23:45:00Z to 2026-11-23T00:32:00Z (the listed window widened by 15 minutes), Unix nanoseconds.
const FROM: i64 = 1_795_131_900_000_000_000;
const TO: i64 = 1_795_393_920_000_000_000;

fn rows_at(times: &[i64]) -> Reports {
    let mut r = Reports::default();
    for (i, t) in times.iter().enumerate() {
        let mut row = ROW_EMPTY;
        row.icao = 0xA1B2C3 + i as u32;
        row.time_ns = *t;
        row.lat_e7 = 410_000_000;
        row.lon_e7 = -880_000_000;
        row.nic = 8;
        row.nac_p = 10;
        row.sil = 3;
        row.version = 2;
        row.source = 0;
        reports_push(&mut r, &row);
    }
    r
}

/// An OAF1 file with `per_frame` reports in each frame.
fn clip(name: &str, r: &Reports, per_frame: usize) -> PathBuf {
    let n = reports_len(r);
    let mut bytes = Vec::new();
    let (mut at, mut seq) = (0usize, 0u64);
    while at < n {
        let end = (at + per_frame).min(n);
        let mut payload = Vec::new();
        encode_payload(&mut payload, r, at, end).unwrap();
        let mut buf = vec![0u8; MAX_FRAME_LEN];
        let t = r.time_ns[at];
        let len = encode_frame(&mut buf, 1, FLAG_BYTES, seq, t, t / 1_000_000, &payload).unwrap();
        bytes.extend_from_slice(&buf[..len]);
        seq += 1;
        at = end;
    }
    let p = std::env::temp_dir().join(format!("oa-bench-seal-{}-{name}", std::process::id()));
    std::fs::write(&p, bytes).unwrap();
    p
}

// REQ: OA-SEAL-014
#[test]
fn load_adsb_keeps_rows_at_from_minus_one_and_at_to_and_drops_the_rest() {
    let r = rows_at(&[FROM - 1, FROM, TO - 1, TO]);
    let p = clip("load.oaf", &r, 4);
    let c = load_adsb(&p).unwrap();
    assert_eq!(c.reports.time_ns, [FROM - 1, TO]);
    assert_eq!(c.sealed_window_dropped, 2);
    assert_eq!(c.bad_frames, 0);
    // A clip with nothing in the window still reports the count, 0.
    let r = rows_at(&[FROM - 1, TO]);
    let c = load_adsb(&clip("load0.oaf", &r, 1)).unwrap();
    assert_eq!((reports_len(&c.reports), c.sealed_window_dropped), (2, 0));
}

// REQ: OA-SEAL-014
#[test]
fn the_frame_walker_passes_no_frame_with_a_row_in_a_sealed_window() {
    let r = rows_at(&[FROM - 1, FROM, TO - 1, TO]);
    // One row per frame: the frames at from and to-1 are left out.
    let mut seen = Vec::new();
    let (damaged, sealed) =
        for_each_adsb_frame_counted(&clip("walk1.oaf", &r, 1), &mut |t, _| seen.push(t)).unwrap();
    assert_eq!((damaged, sealed), (0, 2));
    assert_eq!(seen, [FROM - 1, TO]);
    // All four in one frame: the frame has rows in the window, so none of it is passed on.
    let mut seen = Vec::new();
    let (_, sealed) =
        for_each_adsb_frame_counted(&clip("walk4.oaf", &r, 4), &mut |t, _| seen.push(t)).unwrap();
    assert_eq!((seen.len(), sealed), (0, 4));
}

// REQ: OA-SEAL-014
#[test]
fn the_reader_builds_every_decoder_with_the_sealed_window_as_its_drop_window() {
    let r = oa_bench::ingest::new_reader().unwrap();
    for w in r.drop_windows() {
        assert_eq!(w, oa_sealed::DropWindow::sealed());
        assert_eq!(w.span(), oa_sealed::sealed_window_ms());
    }
}
