//! `oa-perf clip` surfaces the clip reader's sealed-window count (OA-SEAL-014). Temp files only.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::process::Command;

/// An OAF1 message frame with a junk payload received at `recv_ns` (IEEE CRC-32 by hand, so the
/// test needs no other crate).
fn frame(recv_ns: i64) -> Vec<u8> {
    let payload = b"junk";
    let mut b = Vec::new();
    b.extend_from_slice(b"OAF1");
    b.extend_from_slice(&[1, 0, 0, 0]); // kind Message, flags, reserved
    b.extend_from_slice(&0u64.to_le_bytes());
    b.extend_from_slice(&recv_ns.to_le_bytes());
    b.extend_from_slice(&0i64.to_le_bytes());
    b.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    b.extend_from_slice(payload);
    let mut crc = 0xFFFF_FFFFu32;
    for x in &b {
        crc ^= u32::from(*x);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    b.extend_from_slice(&(!crc).to_le_bytes());
    b
}

fn run(name: &str, recv_ns: i64) -> (Option<i32>, String) {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("perf_{name}"));
    std::fs::create_dir_all(&dir).unwrap();
    let clip = dir.join("clip.oaf");
    std::fs::write(&clip, frame(recv_ns)).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_oa-perf"))
        .args(["clip", "--clip"])
        .arg(&clip)
        .arg("--out")
        .arg(dir.join("out"))
        .output()
        .unwrap();
    (
        o.status.code(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

// REQ: OA-SEAL-014
#[test]
fn the_clip_run_prints_the_count_of_the_frames_the_reader_dropped_on_an_error_exit() {
    // Received in the window (the list's window widened by 15 minutes starts at this instant).
    let (code, err) = run("in", 1_795_131_900_000_000_000);
    assert_eq!(code, Some(1), "{err}");
    assert_eq!(
        err.matches("sealed-window rows dropped: 1").count(),
        1,
        "{err}"
    );
    // Outside it: the line is printed with 0.
    let (code, err) = run("out", 1_700_000_000_000_000_000);
    assert_eq!(code, Some(1), "{err}");
    assert_eq!(
        err.matches("sealed-window rows dropped: 0").count(),
        1,
        "{err}"
    );
}
