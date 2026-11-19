#![forbid(unsafe_code)]
//! oa-shadow: an offline shadow comparator. It runs OpenATCA's tracker beside ERAM's published
//! track (SWIM SFDPS) on recorded data and reports where the two systems disagree. A difference
//! is decided by the two systems' outputs alone, never by truth: a live shadow has no truth. The
//! ADS-B truth the benchmark builds can be named afterwards in one adjudication column (which
//! system was closer), computed after classification and never used to decide a class. Class
//! `shell` (B1): the crate reads files and runs the tracker, so none of it is core; the
//! comparison modules ([`compare`], [`geo`], [`pairing`], [`report`]) take plain values and do
//! no I/O, so a test runs them on synthetic tracks.
//!
//! Requirements: OA-SHD-001 and the following, in REQUIREMENTS.md.
//!
//! # What is compared
//!
//! Both systems are restricted to the ZAU clip box (the ADS-B clip's own limit), so ERAM's
//! nation-wide tracks outside it are not "missing" from OpenATCA. Flights are paired by one rule
//! ([`pairing`]): the Mode S address, else the transponder code plus proximity. At each ERAM
//! report time OpenATCA's one-second output is interpolated to that time and the two are
//! compared ([`compare`]); the thresholds are one table ([`thresholds`]). ERAM stamps its
//! positions on whole seconds; the along-track comparison allows for that ([`geo::separation`]).
//!
//! # Identity on the OpenATCA side
//!
//! The tracker's output carries no callsign or transponder code. The identity OpenATCA is
//! credited with is what its adapter was offered: the squawk and callsign of the ADS-B reports
//! of that address in the tracker's input set, near the flight's time. ERAM's beacon code is the
//! last `currentBeaconCode` any SFDPS message gave the flight; its callsign is the flight
//! identification.

pub mod args;
pub mod compare;
pub mod geo;
pub mod guard;
pub mod model;
pub mod pairing;
pub mod report;
pub mod run;
pub mod screen;
pub mod thresholds;
pub mod window;
