//! Shell-side policy adapter for the pure tracker core.

pub(crate) use oa_track_core::{
    check_advance_time, tracker_advance, tracker_flush, tracker_has_key, tracker_is_confirmed,
    tracker_live_iter, tracker_new, tracker_note_radar_disagreement, tracker_predict_checked,
    tracker_predict_for_association, tracker_set_expected_interval, tracker_stats, tracker_update,
    Tracker,
};
#[cfg(test)]
pub(crate) use oa_track_core::{
    measurement_check, r95_m_of, tracker_config, tracker_forecast, tracker_live, tracker_log,
    tracker_predict,
};

use oa_track_core::types::OutRow;
use oa_track_core::{tracker_altitude_forecast, PredictError};

/// Applies caller policy to the core's plain altitude forecast before building the full row.
#[cfg(test)]
pub(crate) fn tracker_predict_if_altitude(
    tracker: &Tracker,
    key: u32,
    at_ns: i64,
    accept: impl FnOnce(f32, f32, i64) -> bool,
) -> Result<Option<(OutRow, i64)>, PredictError> {
    if let Some((altitude_ft, variance_ft2, last_update_ns)) =
        tracker_altitude_forecast(tracker, key, at_ns)?
    {
        if !accept(altitude_ft, variance_ft2, last_update_ns) {
            return Ok(None);
        }
    }
    tracker_predict_checked(tracker, key, at_ns)
        .map(|(row, _, last_update_ns)| Some((row, last_update_ns)))
}

/// Applies caller policy to the core's plain altitude forecast before building association inputs.
pub(crate) fn tracker_predict_for_association_if_altitude(
    tracker: &Tracker,
    key: u32,
    at_ns: i64,
    accept: impl FnOnce(f32, f32, i64) -> bool,
) -> Result<Option<(OutRow, i64)>, PredictError> {
    if let Some((altitude_ft, variance_ft2, last_update_ns)) =
        tracker_altitude_forecast(tracker, key, at_ns)?
    {
        if !accept(altitude_ft, variance_ft2, last_update_ns) {
            return Ok(None);
        }
    }
    tracker_predict_for_association(tracker, key, at_ns).map(Some)
}
