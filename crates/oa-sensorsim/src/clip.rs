//! Reads an OAF1 clip (docs/ADSB_PAYLOAD.md) into ADS-B reports. Shell: owns the file.

use oa_adsb::Reports;
use oa_truth::clip::{MAX_CLIP_BYTES, MAX_CLIP_ROWS};

/// Every ADS-B payload frame of the file at `path` as one set of reports, the number of frames
/// that did not decode and the number of records dropped from frames that did (a bad record does
/// not refuse its frame). The reader and its byte and row caps are `oa-truth`'s.
pub fn read_clip(path: &str) -> Result<(Reports, u64, u64), String> {
    oa_truth::clip::read_clip(path, MAX_CLIP_BYTES, MAX_CLIP_ROWS).map_err(|e| e.to_string())
}

/// [`read_clip`], also returning the rows dropped because their time is in a sealed window
/// (OA-SEAL-014): `(reports, frames not decoded, records dropped, sealed-window rows dropped)`.
pub fn read_clip_counted(path: &str) -> Result<(Reports, u64, u64, u64), String> {
    oa_truth::clip::read_clip_counted(path, MAX_CLIP_BYTES, MAX_CLIP_ROWS)
        .map_err(|e| e.to_string())
}

/// [`read_clip_counted`] with the sealed-window count put in `sealed` on every exit, a refusal
/// for too many rows included: `(reports, frames not decoded, records dropped)`.
pub fn read_clip_tallied(path: &str, sealed: &mut u64) -> Result<(Reports, u64, u64), String> {
    oa_truth::clip::read_clip_tallied(path, MAX_CLIP_BYTES, MAX_CLIP_ROWS, sealed)
        .map_err(|e| e.to_string())
}
