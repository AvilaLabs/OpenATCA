//! The tracker's and the decoder's types as plain columns (**shell**, B1, D3): the one place where
//! `oa_track::TrackOutput` and `oa_adsb::Reports` meet the core. Every function here is shell:
//! each is a field-by-field view or a unit conversion (covariance to 95% radius, the tracker's
//! `O_*` bits to the core's `RF_*` bits), with no decision in it.

use crate::core::rows::{
    callsigns_build, CallsignStats, Callsigns, RowColumns, RowsError, RF_COAST, RF_HAS_ALT,
    RF_REACQUIRED, RF_UNCERTAIN,
};
use oa_adsb::Reports;
use oa_track::{r95_m_of, TrackOutput, O_COAST, O_HAS_ALT, O_REACQUIRED, O_UNCERTAIN};

/// The two columns that are computed, not viewed: reused from drain to drain. Plain data.
// A7.2: cold struct, size guard waived (one per run).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ColumnBuf {
    r95_m: Vec<f64>,
    flags: Vec<u8>,
}

/// The `RF_*` bits of the tracker's `O_*` bits.
pub fn row_flags(o: u8) -> u8 {
    let mut f = 0;
    for (from, to) in [
        (O_COAST, RF_COAST),
        (O_HAS_ALT, RF_HAS_ALT),
        (O_REACQUIRED, RF_REACQUIRED),
        (O_UNCERTAIN, RF_UNCERTAIN),
    ] {
        if o & from != 0 {
            f |= to;
        }
    }
    f
}

/// The columns of a tracker output the display needs, as slices; the radius and flag columns are
/// filled into `buf`.
pub fn row_columns<'a>(out: &'a TrackOutput, buf: &'a mut ColumnBuf) -> RowColumns<'a> {
    buf.r95_m.clear();
    buf.r95_m.extend(out.pos_cov_m2.iter().map(r95_m_of));
    buf.flags.clear();
    buf.flags.extend(out.flags.iter().map(|&o| row_flags(o)));
    RowColumns {
        t_ns: &out.t_ns,
        addr: &out.addr,
        lat_e7: &out.lat_e7,
        lon_e7: &out.lon_e7,
        vel_east_mps: &out.vel_east_mps,
        vel_north_mps: &out.vel_north_mps,
        r95_m: &buf.r95_m,
        alt_ft: &out.alt_ft,
        flags: &buf.flags,
    }
}

/// The callsigns heard in `reports` (see [`callsigns_build`]).
pub fn callsigns_from_reports(r: &Reports) -> Result<(Callsigns, CallsignStats), RowsError> {
    callsigns_build(&r.time_ns, &r.icao, &r.callsign)
}
