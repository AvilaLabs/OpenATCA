//! Offline validation of loaded charts. Shell: allocates freely (RULESET C6) and the PDF check
//! runs `pdftotext`; the loader and the queries it examines are not touched by it.
//!
//! * [`rings`]: ring integrity per sector (self-intersection, spikes, holes), counted and never
//!   repaired.
//! * [`coverage`]: gaps between sectors and overlaps, by rasterising each chart.
//! * [`pdf`]: sector altitudes against the text of the FAA's PDF plots.
//! * [`clearance`]: the minimum altitude of each sector against terrain (3DEP) and obstacles
//!   (DOF).
//!
//! The functions return plain data; `oa-mva-check` prints it.

pub mod clearance;
pub mod coverage;
pub mod pdf;
pub mod rings;
