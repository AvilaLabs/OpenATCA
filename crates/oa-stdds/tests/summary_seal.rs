//! The `stdds_summary` example drops and counts frames and TAIS records in a sealed window
//! (OA-SEAL-014). The example is run as built by cargo for this package.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_feed::frame::{encode_frame, Kind as FrameKind, MAX_FRAME_LEN};

/// The window of the list: 2026-11-19T23:45:00Z to 2026-11-23T00:32:00Z (the listed window widened by 15 minutes), Unix nanoseconds.
const FROM: i64 = 1_795_131_900_000_000_000;
const TO: i64 = 1_795_393_920_000_000_000;

fn message(times: &[&str]) -> Vec<u8> {
    let mut s = String::from(
        "<?xml version=\"1.0\"?><ns2:TATrackAndFlightPlan xmlns:ns2=\"urn:us:gov:dot:faa:atm:terminal:entities:v4-0:tais:terminalautomationinformation\"><src>ZZA</src>",
    );
    for (n, t) in times.iter().enumerate() {
        s += &format!(
            "<record><track><trackNum>{n}</trackNum><mrtTime>{t}</mrtTime><pseudo>1</pseudo></track></record>"
        );
    }
    s += "</ns2:TATrackAndFlightPlan>";
    s.into_bytes()
}

fn example() -> std::path::PathBuf {
    // target/<profile>/deps/<test> -> target/<profile>/examples/stdds_summary
    let exe = std::env::current_exe().unwrap();
    exe.parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("examples")
        .join("stdds_summary")
}

// REQ: OA-SEAL-014
#[test]
fn the_summary_example_drops_sealed_frames_and_records_and_prints_the_count() {
    let mut bytes = Vec::new();
    // (frame time, record times): the first frame, received at from - 1 ns, carries two records
    // in the window and one before it, so the whole message is dropped (three records); the next two frames are received in the window; the last
    // is received at to.
    let frames: [(i64, Vec<&str>); 4] = [
        (
            FROM - 1,
            vec![
                "2026-11-19T23:44:59Z",
                "2026-11-19T23:45:00Z",
                "2026-11-23T00:31:59Z",
            ],
        ),
        (FROM, vec!["2026-11-19T12:00:00Z"]),
        (TO - 1, vec!["2026-11-19T12:00:00Z"]),
        (TO, vec!["2026-11-23T00:32:00Z"]),
    ];
    for (i, (t, recs)) in frames.iter().enumerate() {
        let m = message(recs);
        let mut buf = vec![0u8; MAX_FRAME_LEN];
        let n = encode_frame(&mut buf, FrameKind::Message as u8, 0, i as u64, *t, 0, &m).unwrap();
        bytes.extend_from_slice(&buf[..n]);
    }
    let dir = std::env::temp_dir().join(format!("oa-stdds-seal-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("clip.oaf");
    std::fs::write(&file, bytes).unwrap();
    let exe = example();
    assert!(exe.exists(), "{}", exe.display());
    let out = std::process::Command::new(exe).arg(&file).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    // Two frames by receive time and the three records of the first frame's message, which has
    // two timed in the window; the line is the last of the output.
    assert!(
        text.trim_end().ends_with("sealed-window rows dropped: 5"),
        "{}",
        text.lines().rev().take(3).collect::<Vec<_>>().join("\n")
    );
    assert!(text.contains("frames: message 1 "), "{text}");
    // Nothing the parser dropped is in any count: only the message of the last frame is.
    assert!(text.contains("messages parsed: 1\n"), "{text}");
    assert!(text.contains("\nrecords: 1 "), "{text}");
    let tn = text
        .lines()
        .find(|l| l.starts_with("tais field track/trackNum"))
        .unwrap();
    assert!(tn.contains("present 1 "), "{tn}");
    let pf = text
        .lines()
        .find(|l| l.starts_with("tais flag track/pseudo"))
        .unwrap();
    assert!(pf.contains("false 0 true 1 of 1"), "{pf}");
    std::fs::remove_dir_all(&dir).ok();
}

// REQ: OA-SEAL-014
#[test]
fn the_summary_example_drops_status_and_reject_frames_received_in_the_window() {
    let mut bytes = Vec::new();
    // Status (3) and Reject (2): one received before the window, two inside it.
    for (i, (kind, t)) in [(3u8, FROM - 1), (3, FROM), (2, TO - 1)]
        .into_iter()
        .enumerate()
    {
        let mut buf = vec![0u8; MAX_FRAME_LEN];
        let n = encode_frame(&mut buf, kind, 0, i as u64, t, 0, b"x").unwrap();
        bytes.extend_from_slice(&buf[..n]);
    }
    let dir = std::env::temp_dir().join(format!("oa-stdds-seal-status-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("clip.oaf");
    std::fs::write(&file, bytes).unwrap();
    let out = std::process::Command::new(example())
        .arg(&file)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.trim_end().ends_with("sealed-window rows dropped: 2"),
        "{text}"
    );
    // Only the frame before the window is counted by kind.
    assert!(text.contains("other 1"), "{text}");
    std::fs::remove_dir_all(&dir).ok();
}

// REQ: OA-SEAL-014
#[test]
fn the_summary_example_gates_a_damaged_frame_by_its_neighbours_receive_times() {
    let run = |name: &str, t: [i64; 3]| {
        let mut bytes = Vec::new();
        let mut at = Vec::new();
        for (i, t) in t.into_iter().enumerate() {
            let mut buf = vec![0u8; MAX_FRAME_LEN];
            let n = encode_frame(&mut buf, 3, 0, i as u64, t, 0, b"xxxx").unwrap();
            at.push(bytes.len());
            bytes.extend_from_slice(&buf[..n]);
        }
        bytes[at[1] + 37] ^= 0xff;
        let dir = std::env::temp_dir().join(format!("oa-stdds-gate-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("clip.oaf");
        std::fs::write(&file, bytes).unwrap();
        let out = std::process::Command::new(example())
            .arg(&file)
            .output()
            .unwrap();
        std::fs::remove_dir_all(&dir).ok();
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let before = FROM - 3_600_000_000_000;
    let text = run("out", [before, before, before + 1]);
    assert!(text.contains("other 2 damaged 1"), "{text}");
    assert!(
        text.trim_end().ends_with("sealed-window rows dropped: 0"),
        "{text}"
    );
    let text = run("in", [before, before, FROM]);
    assert!(text.contains("other 1 damaged 0"), "{text}");
    assert!(
        text.trim_end().ends_with("sealed-window rows dropped: 2"),
        "{text}"
    );
}
