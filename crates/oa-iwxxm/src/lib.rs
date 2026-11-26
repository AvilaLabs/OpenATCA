#![forbid(unsafe_code)]
//! Bounded, read-only IWXXM METAR/SPECI/TAF decoding.
//!
//! This shell parser supports IWXXM 2025-2 / METAR-SPECI package 3.2.0 only. It accepts one
//! standalone `iwxxm:METAR`, `iwxxm:SPECI`, or non-cancellation `iwxxm:TAF` report, with bounded
//! normalized fields represented below. TAF supports one base group and at most five change or
//! probability groups. Constructs outside the explicit field set are refused and counted, not
//! returned after silently dropping the field.
//!
//! The parser has no file, network, clock, or `oa-sealed` access. It uses the existing bounded
//! `oa-xml` parser with the fixed ceilings in this crate. Callers decide their own input-source
//! policy. This is not a schema/Schematron validator, a TAC translator, an IWXXM encoder, or a
//! full-edition decoder. Status, nil values, units, weather code URIs and unsupported reports
//! remain explicit in the result. Aerodrome decoding extracts only the selected station/time-slice
//! metadata; it does not normalize every AIXM sibling property. METAR/SPECI trend data is separate
//! from the current observation. TAF has a separate forecast projection and no synthetic
//! observation timestamp.

mod decode;
mod encode;
mod model;

pub use decode::{decode_report, DecodeError, DecodeFailure, DecodeResult, DecodeStats};
pub use encode::{encode_report, EncodeError};
pub use model::{
    Cloud, CloudAmount, CloudLayer, ForecastChange, ForecastGroup, Measure, NilValue,
    NilValueObservation, Observation, PermissibleUsage, QuantityUnit, RelationalOperator, Report,
    ReportKind, ReportStatus, RunwayVisualRange, TafForecast, TrendChange, TrendForecast,
    TrendTimeIndicator, WeatherCode, WindShear, MAX_CLOUD_LAYERS, MAX_PRESENT_WEATHER,
};

/// Input XML bytes per report. Matches the local meteorological document cap, not a schema max.
pub const MAX_DOCUMENT_BYTES: usize = 256 * 1024;
/// XML element nodes per report.
pub const MAX_ELEMENTS: usize = 4096;
/// XML attributes per report.
pub const MAX_ATTRIBUTES: usize = 8192;
/// XML text nodes per report.
pub const MAX_TEXT_NODES: usize = 4096;
/// XML namespace declarations in scope / document.
pub const MAX_NAMESPACE_BINDINGS: usize = 64;
/// Cumulative child/text-table visits made by one decode call.
pub const MAX_DECODE_WORK: usize = 100_000;
/// One decoded scalar or preserved URI/nil-reason string, bytes.
pub const MAX_SCALAR_BYTES: usize = 512;
/// Maximum RVR groups retained for one observation.
pub const MAX_RVR_ITEMS: usize = 4;
/// Maximum recent-weather groups retained for one observation.
pub const MAX_RECENT_WEATHER: usize = 3;
/// Maximum trend groups retained for one METAR/SPECI report.
pub const MAX_TREND_FORECASTS: usize = 2;
/// Maximum TAF change/probability groups retained in one report.
pub const MAX_TAF_CHANGE_GROUPS: usize = 5;

#[cfg(test)]
mod tests;
