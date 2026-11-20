#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
//! End-to-end run of the `oa-feed` binary on frames written by the Rust encoder (no Java).

use std::io::Write;
use std::process::{Command, Stdio};

use oa_feed::{encode_frame, FrameReader, HEADER_LEN, MAX_FRAME_LEN, TRAILER_LEN};

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

/// The recorded frames with their monotonic time taken off: every frame must carry one, not
/// going back, and the rest of each frame is what the recorder was given.
fn without_mono(recorded: &[u8]) -> Vec<u8> {
    let mut buf = vec![0u8; MAX_FRAME_LEN];
    let mut r = FrameReader::new(&mut buf).unwrap();
    assert_eq!(r.push(recorded), recorded.len());
    let mut out = Vec::new();
    let mut last = 0u64;
    while let Some(f) = r.next() {
        let f = f.unwrap();
        let mono = f
            .mono_ns()
            .expect("a recorded frame carries a monotonic time");
        assert!(mono >= last, "monotonic time went back");
        last = mono;
        let mut v = vec![0u8; HEADER_LEN + f.payload.len() + TRAILER_LEN];
        let n = encode_frame(
            &mut v,
            f.kind as u8,
            f.flags,
            f.seq,
            f.recv_unix_ns,
            f.jms_timestamp_ms,
            f.payload,
        )
        .unwrap();
        out.extend(&v[..n]);
    }
    assert!(r.finish().is_none());
    out
}

// REQ: OA-FEED-023, OA-FEED-026, OA-SEAL-014
#[test]
fn stdin_mode_records_verbatim_and_replay_agrees() {
    let dir = std::env::temp_dir().join(format!("oa-feed-it-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let data = frames();
    let bin = env!("CARGO_BIN_EXE_oa-feed");
    let mut child = Command::new(bin)
        .args(["--stdin", "--feed", "it", "--record-raw"])
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
    assert!(files[0].to_string_lossy().ends_with(".oaf"));
    assert_eq!(
        without_mono(&std::fs::read(&files[0]).unwrap()),
        data,
        "recording is the input, each frame with the recorder's monotonic time added"
    );
    let rep = Command::new(bin)
        .arg("replay")
        .arg(&files[0])
        .output()
        .unwrap();
    assert!(rep.status.success());
    // The count line (frames), printed once.
    let rerr = String::from_utf8_lossy(&rep.stderr);
    assert_eq!(
        rerr.matches("sealed-window rows dropped: 0 (frames")
            .count(),
        1,
        "{rerr}"
    );
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

// REQ: OA-FEED-178
#[test]
fn duration_arguments_at_the_limit_run_and_above_it_are_refused_without_a_panic() {
    let bin = env!("CARGO_BIN_EXE_oa-feed");
    let run = |flag: &str, v: u64| {
        let mut child = Command::new(bin)
            .args(["--stdin", flag, &v.to_string()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        drop(child.stdin.take());
        child.wait_with_output().unwrap()
    };
    for (flag, limit) in [
        ("--duration", oa_feed::cli::MAX_DURATION_SECS),
        ("--summary-every", oa_feed::cli::MAX_SUMMARY_SECS),
    ] {
        let ok = run(flag, limit);
        assert!(ok.status.success(), "{flag} {limit}: {:?}", ok.status);
        for over in [limit + 1, u64::MAX] {
            let out = run(flag, over);
            let err = String::from_utf8_lossy(&out.stderr);
            assert!(!out.status.success(), "{flag} {over} was accepted");
            assert!(!err.contains("panicked"), "{flag} {over}: {err}");
            assert!(err.contains("at most"), "{flag} {over}: {err}");
        }
    }
}

// REQ: OA-FEED-031
#[test]
fn stdin_mode_records_compressed_and_replay_matches_the_raw_recording() {
    let base = std::env::temp_dir().join(format!("oa-feed-it-gz-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let data = frames();
    let bin = env!("CARGO_BIN_EXE_oa-feed");
    let mut outputs = Vec::new();
    for flag in ["--record", "--record-raw"] {
        let dir = base.join(&flag[2..]);
        let mut child = Command::new(bin)
            .args(["--stdin", "--feed", "it", flag])
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
        let mut files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(files.len(), 1);
        let rep = Command::new(bin)
            .arg("replay")
            .arg(&files[0])
            .output()
            .unwrap();
        assert!(
            rep.status.success(),
            "{}",
            String::from_utf8_lossy(&rep.stderr)
        );
        outputs.push((files.remove(0), rep.stdout));
    }
    let (gz, gz_out) = &outputs[0];
    let (raw, raw_out) = &outputs[1];
    assert!(gz.to_string_lossy().ends_with(".oaf.gz"), "{gz:?}");
    assert!(raw.to_string_lossy().ends_with(".oaf"), "{raw:?}");
    let gz_bytes = std::fs::read(gz).unwrap();
    assert_eq!(&gz_bytes[..2], &[0x1f, 0x8b]);
    assert!(gz_bytes.len() < data.len());
    // The whole replay output, health summary included, is the same text.
    assert_eq!(
        String::from_utf8_lossy(gz_out),
        String::from_utf8_lossy(raw_out)
    );
    assert!(String::from_utf8_lossy(gz_out).contains("msgs=20 "));
    // The system gzip accepts the file and decodes it to the input.
    match Command::new("gzip").arg("-dc").arg(gz).output() {
        Ok(o) => {
            assert!(o.status.success());
            assert_eq!(without_mono(&o.stdout), data);
        }
        Err(e) => eprintln!("gzip not available: {e}"),
    }
    // A copy cut short, as a crash would leave it, replays and says so.
    let cut = base.join("cut.oaf.gz");
    std::fs::write(&cut, &gz_bytes[..gz_bytes.len() / 2]).unwrap();
    let rep = Command::new(bin).arg("replay").arg(&cut).output().unwrap();
    assert!(
        !rep.status.success(),
        "a truncated file is not a clean replay"
    );
    let text = String::from_utf8(rep.stdout).unwrap();
    assert!(text.contains("gzip truncated"), "{text}");
    assert!(text.contains("replay files=1 "), "{text}");
    std::fs::remove_dir_all(&base).unwrap();
}
