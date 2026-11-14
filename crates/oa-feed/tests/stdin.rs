#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
//! End-to-end run of the `oa-feed` binary on frames written by the Rust encoder (no Java).

use std::io::Write;
use std::process::{Command, Stdio};

use oa_feed::{encode_frame, HEADER_LEN, TRAILER_LEN};

fn frames() -> Vec<u8> {
    let mut out = Vec::new();
    for seq in 0..20u64 {
        let payload = vec![b'p'; 10 + seq as usize];
        let mut v = vec![0u8; HEADER_LEN + payload.len() + TRAILER_LEN];
        let recv = 1_700_000_000_000_000_000 + seq as i64 * 100_000_000;
        encode_frame(&mut v, 1, 0, seq, recv, recv / 1_000_000 - 7, &payload).unwrap();
        out.extend(v);
    }
    out
}

// REQ: OA-FEED-023
#[test]
fn stdin_mode_records_verbatim_and_replay_agrees() {
    let dir = std::env::temp_dir().join(format!("oa-feed-it-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let data = frames();
    let bin = env!("CARGO_BIN_EXE_oa-feed");
    let mut child = Command::new(bin)
        .args(["--stdin", "--feed", "it", "--record"])
        .arg(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&data).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let live = String::from_utf8(out.stdout).unwrap();
    assert!(
        live.contains("final t=") && live.contains("feed=it state=unknown msgs=20 "),
        "{live}"
    );
    let files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1);
    assert_eq!(
        std::fs::read(&files[0]).unwrap(),
        data,
        "recording is the input, byte for byte"
    );
    let rep = Command::new(bin)
        .arg("replay")
        .arg(&files[0])
        .output()
        .unwrap();
    assert!(rep.status.success());
    let text = String::from_utf8(rep.stdout).unwrap();
    assert!(text.starts_with("replay files=1 bytes="), "{text}");
    assert!(
        text.contains("msgs=20 ") && text.contains("gaps=0 missing=0"),
        "{text}"
    );
    let again = Command::new(bin)
        .arg("replay")
        .arg(&files[0])
        .output()
        .unwrap();
    assert_eq!(
        text.as_bytes(),
        &again.stdout[..],
        "replay output is identical on every run"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
