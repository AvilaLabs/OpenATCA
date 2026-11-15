//! Reads an OAF1 clip (docs/ADSB_PAYLOAD.md) into ADS-B reports, with caps. Shell: owns the file.
//! Shared by `oa-truth-check` and `oa-sensorsim`.

use std::fs::File;

use oa_adsb::{decode_payload_lenient, Reports};
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

/// Reads every ADS-B payload frame of an OAF1 file into one set of reports. Returns the reports
/// the number of frames that did not decode and the number of records dropped from frames that
/// did (a bad record does not refuse its frame). A file over `max_bytes` is refused unread; a
/// frame whose header takes the running row count over `max_rows` is refused before it is decoded.
pub fn read_clip(
    path: &str,
    max_bytes: u64,
    max_rows: u64,
) -> Result<(Reports, u64, u64), ClipError> {
    let io = |e: std::io::Error| ClipError::Io(format!("{path}: {e}"));
    let mut f = File::open(path).map_err(io)?;
    let bytes = f.metadata().map_err(io)?.len();
    if bytes > max_bytes {
        return Err(ClipError::TooManyBytes {
            bytes,
            cap: max_bytes,
        });
    }
    let mut out = Reports::default();
    let mut undecodable = 0u64;
    let mut dropped = 0u64;
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
                .map_or(0, |b| u64::from(u32::from_le_bytes(b)));
            rows = rows.saturating_add(claimed);
            if rows > max_rows {
                over = Some(rows);
                return;
            }
            match decode_payload_lenient(fr.payload, &mut out) {
                Ok(c) => dropped += c.dropped as u64,
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
    Ok((out, damaged + undecodable, dropped))
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
}
