//! Reads an OAF1 clip (docs/ADSB_PAYLOAD.md) into ADS-B reports. Shell: owns the file.

use std::fs::File;

use oa_adsb::{decode_payload_lenient, Reports};
use oa_feed::{read_frames, Kind};

/// Every ADS-B payload frame of the file at `path` as one set of reports, the number of frames
/// that did not decode and the number of records dropped from frames that did (a bad record does
/// not refuse its frame).
pub fn read_clip(path: &str) -> Result<(Reports, u64, u64), String> {
    let mut f = File::open(path).map_err(|e| format!("{path}: {e}"))?;
    let mut out = Reports::default();
    let mut dropped = 0u64;
    let mut undecodable = 0u64;
    let damaged = read_frames(&mut f, &mut |fr| {
        if fr.kind == Kind::Message {
            match decode_payload_lenient(fr.payload, &mut out) {
                Ok(c) => dropped += c.dropped as u64,
                Err(_) => undecodable += 1,
            }
        }
    })
    .map_err(|e| format!("{path}: {e}"))?;
    Ok((out, damaged + undecodable, dropped))
}
