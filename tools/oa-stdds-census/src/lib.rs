#![forbid(unsafe_code)]
//! `oa-stdds-census`: counts and distributions of the ADS-B flag of STARS terminal tracks in
//! one STDDS recording directory (REQUIREMENTS.md OA-CEN-001 to OA-CEN-008). Shell class: it
//! reads files and prints; the counting itself is in [`census`] and takes parsed records.
//!
//! It collects evidence on whether a TAIS update with `track/adsb` false is a track with no
//! ADS-B contribution. It does not decide that: the output are counts of what the updates
//! carry and how often they arrive, never a position, address, callsign, code or time of an
//! aircraft.

pub mod census;
pub mod dist;
pub mod guard;
pub mod hist;
pub mod read;
pub mod report;
pub mod xcheck;
pub mod xcres;
