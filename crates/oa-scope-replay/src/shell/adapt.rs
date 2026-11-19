//! The tracker's and the decoder's types as plain columns (**shell**, B1, D3): the one place where
//! `oa_track::TrackOutput` and `oa_adsb::Reports` meet the core. Every function here is shell:
//! each is a field-by-field view, with no decision in it.

use crate::rows::{callsigns_build, CallsignStats, Callsigns, RowColumns, RowsError};
use oa_adsb::Reports;
use oa_track::TrackOutput;

/// The columns of a tracker output the display needs, as slices (no copy).
pub fn row_columns(out: &TrackOutput) -> RowColumns<'_> {
    RowColumns {
        t_ns: &out.t_ns,
        addr: &out.addr,
        lat_e7: &out.lat_e7,
        lon_e7: &out.lon_e7,
        vel_east_mps: &out.vel_east_mps,
        vel_north_mps: &out.vel_north_mps,
        pos_cov_m2: &out.pos_cov_m2,
        alt_ft: &out.alt_ft,
        flags: &out.flags,
    }
}

/// The callsigns heard in `reports` (see [`callsigns_build`]).
pub fn callsigns_from_reports(r: &Reports) -> Result<(Callsigns, CallsignStats), RowsError> {
    callsigns_build(&r.time_ns, &r.icao, &r.callsign)
}
