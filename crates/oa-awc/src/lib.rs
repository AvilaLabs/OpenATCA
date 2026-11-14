#![forbid(unsafe_code)]
//! oa-awc: parsers for the Aviation Weather Center data API (`aviationweather.gov/api/data`,
//! `format=json`) into struct-of-arrays tables, built on `oa-json`. The data is public domain
//! (NOAA/NWS); fields and null patterns come from the files in `raw/awc` (see its SOURCE.md).
//!
//! * [`metar`]: METAR and SPECI observations, sorted by station and time, with
//!   [`altimeter_at`] (the latest trusted altimeter setting at or before a time, with its age,
//!   refusing a stale one) and [`indicated_altitude_ft`] (pressure altitude to indicated
//!   altitude for a given setting). Needed by MSAW and conflict alert to compare ADS-B or Mode C
//!   pressure altitude with terrain and with other aircraft below the transition level.
//! * [`pirep`]: pilot reports (position, time, level, aircraft type, turbulence, icing, wind,
//!   temperature) for trajectory-prediction validation.
//! * [`hazard`]: international SIGMET, US SIGMET/AIRMET and G-AIRMET polygons with altitude bands
//!   and validity times ([`active_hazards_at`], [`hazard_covers`]).
//! * [`taf`]: terminal forecasts.
//! * [`units`]: unit conversions and bounds, [`error`]: typed refusals, [`summary`]: the offline
//!   validation scan behind `oa-awc-summary`.
//!
//! Every parser is a pure function of its input bytes into a table (RULESET B1). File level is
//! all or nothing ([`ParseError`]); record level refuses the bad record alone and counts it by
//! [`Reason`] (E5). Overlapping files are de-duplicated by `finalize_*`, which states its key and
//! gives the same result whatever the order of the files.

pub mod error;
pub mod hazard;
mod json;
pub mod metar;
pub mod pirep;
pub mod pool;
pub mod summary;
pub mod taf;
pub mod units;

#[cfg(test)]
mod tests;

pub use error::{
    count_refusal, AltimeterError, ParseError, Reason, Refusal, RefusedCounts, REASON_COUNT,
    REASON_NAMES,
};
pub use hazard::{
    active_hazards_at, finalize_hazard, hazard_covers, hazard_label, hazard_len, hazard_name,
    hazard_rings, new_hazard_table, parse_airsigmet, parse_gairmet, parse_sigmet, ring_points,
    HazardDedupe, HazardStats, HazardTable,
};
pub use metar::{
    altimeter_at, finalize_metar, metar_altim_hpa, metar_len, metar_raw, metar_station_count,
    new_metar_table, parse_metar, AltimeterFix, DedupeReport, MetarStats, MetarTable,
    ALTIM_AGREE_TOL_DHPA, WDIR_VRB,
};
pub use pirep::{
    finalize_pirep, new_pirep_table, parse_pirep, pirep_alt_ft, pirep_len, pirep_raw, PirepDedupe,
    PirepStats, PirepTable,
};
pub use taf::{
    finalize_taf, new_taf_table, parse_taf, taf_len, taf_periods, taf_raw, TafDedupe, TafStats,
    TafTable,
};
pub use units::{indicated_altitude_ft, pack_ident, parse_iso_ms, unpack_ident};

/// Bound: records in one file. The API caps a METAR response at 400 and a PIREP response at a
/// few hundred; 100 000 leaves room for a CONUS-wide TAF or SIGMET dump and still bounds memory.
pub const MAX_RECORDS_PER_FILE: usize = 100_000;
/// Bound: rows in one table (all files together), 4 Mi. A month of CONUS METARs is about 2 M.
pub const MAX_TABLE_ROWS: usize = 4 * 1024 * 1024;
/// Bound: bytes of one raw report text.
pub const MAX_RAW_BYTES: usize = 4096;
/// Recommended maximum age of an altimeter setting, seconds: 90 minutes. METARs are hourly, so
/// the latest one is normally under an hour old; the extra half hour covers a late or missed
/// report. Callers that need a different margin pass their own to [`altimeter_at`].
pub const DEFAULT_MAX_ALTIMETER_AGE_S: i64 = 5400;
