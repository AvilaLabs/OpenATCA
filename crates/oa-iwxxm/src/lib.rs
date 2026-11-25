#![forbid(unsafe_code)]
//! Bounded, read-only IWXXM METAR/SPECI decoding.
//!
//! This shell parser supports IWXXM 2025-2 / METAR-SPECI package 3.2.0 only. It accepts one
//! standalone `iwxxm:METAR` or `iwxxm:SPECI` report, with inline time/aerodrome references and
//! the normalized current-observation fields represented below. The accepted field set is
//! intentionally narrower than the schema: a report outside the required CAVOK observation
//! subset, or containing trend, runway visual range, present/recent weather, cloud, wind
//! variability extremes, wind shear, sea condition, or an extension, is refused and counted,
//! not returned with that feature silently removed.
//!
//! The parser has no file, network, clock, or `oa-sealed` access. It uses the existing bounded
//! `oa-xml` parser with the fixed ceilings in this crate. Callers decide their own input-source
//! policy. This is not a schema/Schematron validator, a TAC translator, an IWXXM encoder, or a
//! full-edition decoder. Status, nil values, units, weather code URIs and unsupported reports
//! remain explicit in the result. Aerodrome decoding extracts only the selected station/time-slice
//! metadata; it does not normalize every AIXM sibling property. Observation children outside the
//! supported CAVOK subset are refused.

mod decode;
mod model;

pub use decode::{decode_report, DecodeError, DecodeFailure, DecodeResult, DecodeStats};
pub use model::{
    Cloud, CloudAmount, CloudLayer, Measure, NilValue, NilValueObservation, Observation,
    PermissibleUsage, QuantityUnit, RelationalOperator, Report, ReportKind, ReportStatus,
    WeatherCode, MAX_CLOUD_LAYERS, MAX_PRESENT_WEATHER,
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

#[cfg(test)]
mod tests;
