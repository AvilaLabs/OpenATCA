#![forbid(unsafe_code)]
//! oa-stdds-ref: scores the OpenATCA tracker and the ADS-B truth against STDDS (STARS) terminal
//! radar tracks. Class `shell`: it reads recordings and writes the report.
//!
//! STARS in fusion mode uses ADS-B as one of its sensors, so a STDDS track is not independent of
//! the ADS-B truth. Every STDDS record gets a class from its own flags ([`model::classify_record`]),
//! every metric is reported per class, and per class three error sets are computed on the same
//! paired samples: tracker against STDDS, ADS-B truth against STDDS, tracker against ADS-B truth.
//! If STDDS follows ADS-B the second set is small; that is the diagnostic.
//!
//! * [`model`]: records, fixes, classes, the reasons a record is not paired.
//! * [`sources`]: the tracker rows (interpolated here) and the ADS-B truth.
//! * [`engine`]: pairing by Mode S address, the per-track time-offset fit, the error sets.
//! * [`report`]: the summary text and the per-sample CSV, byte-identical for the same inputs.
//! * [`load`]: reading `.oaf` / `.oaf.gz` recordings.
//! * [`guard`]: the sealed-path refusal, through the shared guard `oa-sealed`.
//! * [`run`]: runs the tracker and builds the truth from the ADS-B clip, as `vs_eram` does.

pub mod engine;
pub mod guard;
pub mod load;
pub mod model;
pub mod report;
pub mod run;
pub mod sources;
