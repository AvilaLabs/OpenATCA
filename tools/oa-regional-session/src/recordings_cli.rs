//! Thin shell adapter facade for root CLI dispatch. It opens sealed-guarded files, validates OAF
//! frames, decodes source payloads, and produces OA-RS v1 or v2 normalized input text.

use crate::recordings::{
    adapt_recorded_frames, serialize_provenance, serialize_quarantine, serialize_recorded_events,
    AdapterError, RecordedAdaptation,
};
use crate::recordings_reader::{
    read_oaf_inputs, RecordingFrames, RecordingInput, RecordingReadError,
    RecordingStreamProvenance, MAX_RECORDING_FRAMES,
};

#[derive(Debug)]
pub enum RecordingCliError {
    Read(RecordingReadError),
    Adapt(AdapterError),
}

// A7.2: cold shell output for one offline recording conversion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordingCliOutput {
    pub normalized_tsv: String,
    pub provenance_tsv: String,
    pub quarantine_tsv: String,
    pub adaptation: RecordedAdaptation,
    pub frame_count: u64,
    pub streams: Vec<RecordingStreamProvenance>,
}

/// Runs the actual OAF→decoder→regional normalized-input path. Root owns argument dispatch and
/// destination creation; this function remains deterministic for an identical set of source files.
pub fn convert_recordings(
    inputs: &[RecordingInput],
    adsb_centre: [u8; 4],
) -> Result<RecordingCliOutput, RecordingCliError> {
    let RecordingFrames {
        frames,
        counts,
        streams,
    } = read_oaf_inputs(inputs).map_err(RecordingCliError::Read)?;
    let frame_count = counts.total_frames;
    let adaptation = adapt_recorded_frames(&frames, counts, MAX_RECORDING_FRAMES, adsb_centre)
        .map_err(RecordingCliError::Adapt)?;
    let normalized_tsv =
        serialize_recorded_events(&adaptation.events).map_err(RecordingCliError::Adapt)?;
    let provenance_tsv = serialize_provenance(&adaptation.events, &adaptation.provenance, &streams)
        .map_err(RecordingCliError::Adapt)?;
    let quarantine_tsv =
        serialize_quarantine(&adaptation.quarantine).map_err(RecordingCliError::Adapt)?;
    Ok(RecordingCliOutput {
        normalized_tsv,
        provenance_tsv,
        quarantine_tsv,
        adaptation,
        frame_count,
        streams,
    })
}
