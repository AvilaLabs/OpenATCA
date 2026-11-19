//! The quality rules on an `oa_adsb::Row`. Shell (B1, B7): the only part of the rules that names
//! `oa-adsb`; it copies the nine fields the rules read into a core [`QualityRow`] and calls the
//! core. The tracker calls these two functions, so its gating is the truth's, not a copy.

use oa_adsb::Row;

use crate::core::quality::{
    quality_check_row, quality_eval_row, QualityRow, QualityState, QualityVerdict,
};
use crate::core::types::TruthConfig;

fn quality_row(r: &Row) -> QualityRow {
    QualityRow {
        lat_e7: r.lat_e7,
        lon_e7: r.lon_e7,
        flags: r.flags,
        source: r.source,
        nac_p: r.nac_p,
        nac_v: r.nac_v,
        nic: r.nic,
        sil: r.sil,
        version: r.version,
    }
}

/// Applies the truth selection rules to one row at `t_ns` and commits the carried values (only
/// for a row that is in range, not stale and from an ADS-B source). The first failing rule is the
/// reason. [`quality_eval`] is the same without the commit.
pub fn quality_check(
    st: &mut QualityState,
    row: &Row,
    t_ns: i64,
    cfg: &TruthConfig,
) -> QualityVerdict {
    quality_check_row(st, &quality_row(row), t_ns, cfg)
}

/// The rules of [`quality_check`] against `st`, which is not changed; returns the verdict and the
/// carried values to commit if the caller keeps the row (equal to `st` for a row that is out of
/// range, stale or from another source).
pub fn quality_eval(
    st: &QualityState,
    row: &Row,
    t_ns: i64,
    cfg: &TruthConfig,
) -> (QualityVerdict, QualityState) {
    quality_eval_row(st, &quality_row(row), t_ns, cfg)
}
