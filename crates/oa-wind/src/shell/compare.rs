//! The comparison of the archive's query with observations. Shell (B7): it queries an
//! [`Archive`], which owns files; the statistics it feeds are the pure code of [`crate::check`].

use crate::archive::{archive_sample, Archive};
use crate::check::{count, obs_components, refusal_name, summarize, ObsWind, VectorCompare};
use crate::error::{LoadError, QueryError};
use crate::sample::Query;

/// Compares the archive's query with observations, in the order given (time order keeps the
/// cache from reloading). A refused query is counted by reason and left out of the statistics;
/// a failed file load stops with its error.
pub fn compare_obs(a: &mut Archive, obs: &[ObsWind]) -> Result<VectorCompare, LoadError> {
    let (mut d, mut sp) = (Vec::new(), Vec::new());
    let (mut bu, mut bv) = (0.0, 0.0);
    let mut refused = Vec::new();
    for o in obs {
        let q = Query {
            lat: o.lat,
            lon: o.lon,
            alt_ft: o.alt_ft,
            time_s: o.time_s,
        };
        match archive_sample(a, &q) {
            Ok(s) => {
                let (ou, ov) = obs_components(o.wdir_deg, o.wspd_kt);
                d.push((s.u_ms - ou).hypot(s.v_ms - ov));
                sp.push((s.u_ms.hypot(s.v_ms) - ou.hypot(ov)).abs());
                bu += s.u_ms - ou;
                bv += s.v_ms - ov;
            }
            Err(QueryError::Refused(r)) => count(&mut refused, refusal_name(&r)),
            Err(QueryError::Load(e)) => return Err(e),
        }
    }
    let n = d.len();
    let nf = n.max(1) as f64;
    Ok(VectorCompare {
        n,
        refused,
        diff: summarize(&mut d),
        diff_unrotated: None,
        bias_u: bu / nf,
        bias_v: bv / nf,
        speed_diff: summarize(&mut sp),
    })
}
