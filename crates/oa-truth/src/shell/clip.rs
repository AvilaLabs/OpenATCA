//! Reads an OAF1 clip (docs/ADSB_PAYLOAD.md) into ADS-B reports, with caps. Shell (B7): owns the file.
//! Shared by `oa-truth-check` and `oa-sensorsim`.

use oa_adsb::{decode_payload_lenient, reports_clear, reports_extend_unsealed, Reports};
use oa_feed::{read_frames, Kind};

/// The largest clip read, in bytes: ten times the 330 MB raw ZAU day (2026-11-14). A larger
/// file is refused before any of it is read.
pub const MAX_CLIP_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// The most rows a clip may claim in its frame headers: ten times the rows of the same day
/// (4.07 million), well under [`crate::MAX_REPORTS`].
pub const MAX_CLIP_ROWS: u64 = 50_000_000;

/// Why a clip was not read.
#[derive(Debug, PartialEq, Eq)]
pub enum ClipError {
    /// The file could not be opened or read.
    Io(String),
    /// The file is larger than the byte cap.
    TooManyBytes {
        /// The file size.
        bytes: u64,
        /// The cap.
        cap: u64,
    },
    /// The frame headers claim more rows than the row cap.
    TooManyRows {
        /// The rows claimed so far.
        rows: u64,
        /// The cap.
        cap: u64,
    },
}

impl std::fmt::Display for ClipError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClipError::Io(e) => write!(f, "{e}"),
            ClipError::TooManyBytes { bytes, cap } => {
                write!(f, "clip is {bytes} bytes, over the cap of {cap}")
            }
            ClipError::TooManyRows { rows, cap } => {
                write!(f, "clip claims {rows} rows, over the cap of {cap}")
            }
        }
    }
}

/// The running row count after a frame claiming `claimed` rows: `Ok` while it stays at or under
/// `max_rows`, `Err` with the count that crossed it otherwise. Saturating, so it cannot wrap.
fn claim_rows(rows: u64, claimed: u32, max_rows: u64) -> Result<u64, u64> {
    let total = rows.saturating_add(u64::from(claimed));
    if total > max_rows {
        Err(total)
    } else {
        Ok(total)
    }
}

/// Reads every ADS-B payload frame of an OAF1 file into one set of reports. Returns the reports
/// the number of frames that did not decode and the number of records dropped from frames that
/// did (a bad record does not refuse its frame). A file over `max_bytes` is refused unread; a
/// frame whose header takes the running row count over `max_rows` is refused before it is decoded.
///
/// Rows whose time is in a sealed window are dropped and counted (OA-SEAL-014); see
/// [`read_clip_counted`] for the count.
pub fn read_clip(
    path: &str,
    max_bytes: u64,
    max_rows: u64,
) -> Result<(Reports, u64, u64), ClipError> {
    read_clip_counted(path, max_bytes, max_rows).map(|(r, bad, dropped, _)| (r, bad, dropped))
}

/// [`read_clip`], also returning the number of rows dropped because their time is in a sealed
/// window (the count is added to the process-wide one a tool prints as well).
pub fn read_clip_counted(
    path: &str,
    max_bytes: u64,
    max_rows: u64,
) -> Result<(Reports, u64, u64, u64), ClipError> {
    let io = |e: std::io::Error| ClipError::Io(format!("{path}: {e}"));
    let mut f = oa_sealed::open(std::path::Path::new(path)).map_err(io)?;
    let bytes = f.metadata().map_err(io)?.len();
    if bytes > max_bytes {
        return Err(ClipError::TooManyBytes {
            bytes,
            cap: max_bytes,
        });
    }
    let mut out = Reports::default();
    let mut frame = Reports::default();
    let mut undecodable = 0u64;
    let mut dropped = 0u64;
    let mut sealed = 0u64;
    let mut rows = 0u64;
    let mut over: Option<u64> = None;
    let damaged = read_frames(&mut f, &mut |fr| {
        if fr.kind == Kind::Message && over.is_none() {
            // The row count is the u32 at byte 8 of the payload header; a payload too short to
            // have one fails the decode below.
            let claimed = fr
                .payload
                .get(8..12)
                .and_then(|b| <[u8; 4]>::try_from(b).ok())
                .map_or(0, u32::from_le_bytes);
            match claim_rows(rows, claimed, max_rows) {
                Ok(total) => rows = total,
                Err(total) => {
                    over = Some(total);
                    return;
                }
            }
            reports_clear(&mut frame);
            match decode_payload_lenient(fr.payload, &mut frame) {
                Ok(c) => {
                    dropped += c.dropped as u64;
                    sealed += reports_extend_unsealed(&mut out, &frame);
                }
                Err(_) => undecodable += 1,
            }
        }
    })
    .map_err(io)?;
    if let Some(rows) = over {
        return Err(ClipError::TooManyRows {
            rows,
            cap: max_rows,
        });
    }
    Ok((out, damaged + undecodable, dropped, sealed))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use oa_adsb::PAYLOAD_MAGIC;
    use oa_feed::{encode_frame, FLAG_BYTES, MAX_FRAME_LEN};

    /// One OAF1 frame whose payload is just a header claiming `claimed` rows (no records).
    fn header_only_frame(claimed: u32) -> Vec<u8> {
        let mut payload = vec![0u8; 16];
        payload[..4].copy_from_slice(&PAYLOAD_MAGIC);
        payload[4..6].copy_from_slice(&80u16.to_le_bytes());
        payload[8..12].copy_from_slice(&claimed.to_le_bytes());
        let mut buf = vec![0u8; MAX_FRAME_LEN];
        let n = encode_frame(&mut buf, 1, FLAG_BYTES, 0, 0, 0, &payload).unwrap();
        buf.truncate(n);
        buf
    }

    fn write(name: &str, bytes: &[u8]) -> String {
        let p = std::env::temp_dir().join(format!("oa_clip_{}_{name}", std::process::id()));
        std::fs::write(&p, bytes).unwrap();
        p.to_str().unwrap().to_string()
    }

    // REQ: OA-SEAL-014
    #[test]
    fn rows_at_the_window_edges_are_kept_or_dropped_and_counted() {
        use oa_adsb::{encode_payload, reports_push, ROW_EMPTY};
        // The list's window: 2026-11-20T00:00:00Z to 2026-11-23T00:17:00Z.
        const FROM: i64 = 1_795_132_800_000_000_000;
        const TO: i64 = 1_795_393_020_000_000_000;
        let mut r = Reports::default();
        for (i, t) in [FROM - 1, FROM, TO - 1, TO].into_iter().enumerate() {
            let mut row = ROW_EMPTY;
            row.icao = 0xA00000 + i as u32;
            row.time_ns = t;
            row.lat_e7 = 410_000_000;
            row.lon_e7 = -880_000_000;
            reports_push(&mut r, &row);
        }
        let mut payload = Vec::new();
        encode_payload(&mut payload, &r, 0, 4).unwrap();
        let mut buf = vec![0u8; MAX_FRAME_LEN];
        let n = encode_frame(&mut buf, 1, FLAG_BYTES, 0, FROM - 1, 0, &payload).unwrap();
        let path = write("window.oaf", &buf[..n]);
        let (got, bad, dropped, sealed) = read_clip_counted(&path, 1 << 20, 1000).unwrap();
        assert_eq!(got.time_ns, [FROM - 1, TO]);
        assert_eq!((bad, dropped, sealed), (0, 0, 2));
        // The plain reader drops the same rows.
        let (got, _, _) = read_clip(&path, 1 << 20, 1000).unwrap();
        assert_eq!(got.time_ns, [FROM - 1, TO]);
    }

    // REQ: OA-TRUTH-045
    #[test]
    fn a_header_claiming_more_rows_than_the_cap_is_refused_before_decoding() {
        let path = write("rows_over_cap.oaf", &header_only_frame(1001));
        // The payload is not a whole frame of 1001 records, so decoding it would fail: the
        // refusal is the row cap, not a decode error.
        assert_eq!(
            read_clip(&path, 1 << 20, 1000).unwrap_err(),
            ClipError::TooManyRows {
                rows: 1001,
                cap: 1000
            }
        );
    }

    // REQ: OA-TRUTH-045
    #[test]
    fn a_file_over_the_byte_cap_is_refused_unread() {
        let path = write("bytes_over_cap.oaf", &header_only_frame(0));
        let len = std::fs::metadata(&path).unwrap().len();
        assert_eq!(
            read_clip(&path, len - 1, 1000).unwrap_err(),
            ClipError::TooManyBytes {
                bytes: len,
                cap: len - 1
            }
        );
        // At the cap it is read.
        assert!(read_clip(&path, len, 1000).is_ok());
    }

    /// One frame of `kind` carrying `payload`.
    fn frame_of(kind: u8, seq: u64, payload: &[u8]) -> Vec<u8> {
        let mut buf = vec![0u8; MAX_FRAME_LEN];
        let n = encode_frame(&mut buf, kind, FLAG_BYTES, seq, 0, 0, payload).unwrap();
        buf.truncate(n);
        buf
    }

    /// A payload of `rows` stationary ADS-B reports, the first `bad` of them with a latitude past
    /// 90 degrees (a record the lenient decoder drops).
    fn payload_with(rows: usize, bad: usize) -> Vec<u8> {
        let mut r = oa_adsb::Reports::default();
        for k in 0..rows {
            let mut row = oa_adsb::ROW_EMPTY;
            row.icao = 0xa00001;
            row.time_ns = 1_700_000_000_000_000_000 + k as i64 * 1_000_000_000;
            row.lat_e7 = if k < bad { 910_000_000 } else { 410_000_000 };
            row.lon_e7 = -880_000_000;
            oa_adsb::reports_push(&mut r, &row);
        }
        let mut payload = Vec::new();
        oa_adsb::encode_payload(&mut payload, &r, 0, rows).unwrap();
        payload
    }

    // REQ: OA-TRUTH-060
    #[test]
    fn the_caps_and_the_error_texts_are_the_documented_ones() {
        assert_eq!(MAX_CLIP_BYTES, 4_294_967_296);
        assert_eq!(MAX_CLIP_ROWS, 50_000_000);
        assert_eq!(ClipError::Io("x: gone".to_string()).to_string(), "x: gone");
        assert_eq!(
            ClipError::TooManyBytes { bytes: 12, cap: 10 }.to_string(),
            "clip is 12 bytes, over the cap of 10"
        );
        assert_eq!(
            ClipError::TooManyRows {
                rows: 1200,
                cap: 1000
            }
            .to_string(),
            "clip claims 1200 rows, over the cap of 1000"
        );
    }

    // REQ: OA-TRUTH-060
    #[test]
    fn a_running_row_total_at_the_cap_is_accepted_and_one_past_it_is_refused() {
        assert_eq!(claim_rows(0, 1000, 1000), Ok(1000));
        assert_eq!(claim_rows(400, 600, 1000), Ok(1000));
        assert_eq!(claim_rows(400, 601, 1000), Err(1001));
        assert_eq!(claim_rows(u64::MAX, 5, 1000), Err(u64::MAX));
        // A file whose single frame claims exactly the cap is not refused for rows (the empty
        // payload below it then fails to decode and is counted as a refused frame).
        let path = write("rows_at_cap.oaf", &header_only_frame(1000));
        let (r, refused, dropped) = read_clip(&path, 1 << 20, 1000).unwrap();
        assert_eq!((oa_adsb::reports_len(&r), refused, dropped), (0, 1, 0));
    }

    // REQ: OA-TRUTH-060
    #[test]
    fn after_the_row_cap_is_crossed_later_frames_do_not_change_the_refusal() {
        // 600 + 600 crosses 1000 at 1200; the third frame would make it 1500 if it were looked at.
        let mut bytes = header_only_frame(600);
        bytes.extend(frame_of(1, 2, &header_only_payload(600)));
        bytes.extend(frame_of(1, 3, &header_only_payload(900)));
        let path = write("rows_after_cap.oaf", &bytes);
        assert_eq!(
            read_clip(&path, 1 << 20, 1000).unwrap_err(),
            ClipError::TooManyRows {
                rows: 1200,
                cap: 1000
            }
        );
    }

    fn header_only_payload(claimed: u32) -> Vec<u8> {
        let mut payload = vec![0u8; 16];
        payload[..4].copy_from_slice(&PAYLOAD_MAGIC);
        payload[4..6].copy_from_slice(&80u16.to_le_bytes());
        payload[8..12].copy_from_slice(&claimed.to_le_bytes());
        payload
    }

    // REQ: OA-TRUTH-060
    #[test]
    fn frames_that_are_not_messages_are_ignored_and_not_counted_as_refused() {
        // A status frame and a reject frame around one good message frame.
        let mut bytes = frame_of(3, 1, b"state=up");
        bytes.extend(frame_of(1, 2, &payload_with(3, 0)));
        bytes.extend(frame_of(2, 3, b"reason"));
        let path = write("mixed_kinds.oaf", &bytes);
        let (r, refused, dropped) = read_clip(&path, 1 << 20, 1000).unwrap();
        assert_eq!((oa_adsb::reports_len(&r), refused, dropped), (3, 0, 0));
    }

    // REQ: OA-TRUTH-060
    #[test]
    fn dropped_records_and_undecodable_and_damaged_frames_are_each_counted() {
        let mut bytes = frame_of(1, 1, &payload_with(4, 2));
        bytes.extend(frame_of(1, 2, &payload_with(3, 1)));
        // Two frames whose payload is not a payload at all.
        bytes.extend(frame_of(1, 3, &[0u8; 16]));
        bytes.extend(frame_of(1, 4, &[0u8; 30]));
        // A frame cut short: counted as damaged by the frame reader.
        let cut = frame_of(1, 5, &payload_with(2, 0));
        bytes.extend_from_slice(&cut[..cut.len() - 20]);
        let path = write("counts.oaf", &bytes);
        let (r, refused, dropped) = read_clip(&path, 1 << 20, 1000).unwrap();
        // 2 + 1 records dropped; 4 - 2 and 3 - 1 kept.
        assert_eq!(dropped, 3);
        assert_eq!(oa_adsb::reports_len(&r), 2 + 2);
        // Two undecodable payloads and one cut frame.
        assert_eq!(refused, 3);
    }
}

#[cfg(kani)]
mod proofs {
    //! Kani proof harnesses (docs/VERIFICATION.md). Compiled only under `cargo kani`.
    use super::*;

    // REQ: OA-TRUTH-046
    #[kani::proof]
    #[kani::unwind(5)]
    fn clip_row_count_refuses_exactly_when_the_total_crosses_the_cap() {
        let max_rows: u64 = kani::any();
        let mut rows = 0u64;
        // The exact total, which three u32 claims cannot overflow.
        let mut exact = 0u64;
        for _ in 0..3 {
            let claimed: u32 = kani::any();
            exact += u64::from(claimed);
            match claim_rows(rows, claimed, max_rows) {
                Ok(total) => {
                    assert!(exact <= max_rows);
                    assert_eq!(total, exact);
                    rows = total;
                }
                Err(total) => {
                    assert!(exact > max_rows);
                    assert_eq!(total, exact);
                    // The reader stops at the first refusal.
                    break;
                }
            }
        }
    }

    // REQ: OA-TRUTH-046
    #[kani::proof]
    fn clip_claim_rows_is_total() {
        // Any running count, including one at the top of the range.
        let rows: u64 = kani::any();
        let claimed: u32 = kani::any();
        let max_rows: u64 = kani::any();
        match claim_rows(rows, claimed, max_rows) {
            Ok(t) => assert!(t <= max_rows && t >= rows),
            Err(t) => assert!(t > max_rows && t >= rows),
        }
    }
}
