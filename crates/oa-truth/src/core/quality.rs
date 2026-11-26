//! The quality rules of truth selection, shared with the tracker. Core (pure, B7). The rules
//! take a [`QualityRow`], the nine plain fields they read; `shell::quality` fills one from an
//! `oa_adsb::Row` and keeps the public `quality_check` and `quality_eval` signatures.
//!
//! A readsb trace row prints NACp, NIC, SIL and the version on a minority of rows, so each is
//! carried forward within an aircraft for at most [`crate::QUALITY_CARRY_MS`]; a row with no
//! value in that age fails (reason `no_quality_in_age`). [`quality_check_row`] holds exactly the
//! per-row rules of [`crate::build_truth`] (validity, stale flag, source, the four gates) so the
//! tracker's input gating is the same code, not a copy. NACv is carried the same way for the
//! tracker's velocity noise; truth does not use it.

use super::types::{
    TruthConfig, ADSB_SOURCES, F_STALE, NONE_U8, REM_INVALID, REM_NACP, REM_NIC, REM_NO_QUALITY,
    REM_SIL, REM_SOURCE, REM_STALE, REM_VERSION,
};

pub use oa_adsb_model::{nacp_epu_m, nacv_epu_mps};

const NS_PER_MS: i64 = 1_000_000;

/// A quality value and when it was last seen. Plain data (A1); A7.2: cold struct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Held {
    pub v: u8,
    pub at_ns: i64,
}

/// Nothing held.
pub const HELD_NONE: Held = Held {
    v: 0,
    at_ns: i64::MIN,
};

/// The carried-forward quality values of one aircraft. A7.2: cold struct, size guard waived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QualityState {
    pub nacp: Held,
    pub nic: Held,
    pub sil: Held,
    pub version: Held,
    pub nacv: Held,
}

impl Default for QualityState {
    fn default() -> Self {
        QualityState {
            nacp: HELD_NONE,
            nic: HELD_NONE,
            sil: HELD_NONE,
            version: HELD_NONE,
            nacv: HELD_NONE,
        }
    }
}

/// What [`quality_check`] decided about one row. A7.2: cold struct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QualityVerdict {
    /// The removal reason (`REM_*` index), `None` if the row passes.
    pub removal: Option<usize>,
    /// A checked field was carried forward from an earlier row.
    pub carried: bool,
    /// The NACp in force (own or carried).
    pub nacp: Option<u8>,
    /// The NACv in force (own or carried); not part of the pass/fail rule.
    pub nacv: Option<u8>,
    /// The NIC, SIL and version in force (own or carried).
    pub nic: Option<u8>,
    pub sil: Option<u8>,
    pub version: Option<u8>,
}

/// The fields of a report the quality rules read (the same names and values as the
/// `oa_adsb::Row` fields); an absent 8-bit field is [`NONE_U8`]. Cold struct (A7.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QualityRow {
    pub lat_e7: i32,
    pub lon_e7: i32,
    pub flags: u8,
    pub source: u8,
    pub nac_p: u8,
    pub nac_v: u8,
    pub nic: u8,
    pub sil: u8,
    pub version: u8,
}

/// The value for a row: its own if present, else the held one if young enough.
fn held_value(h: &mut Held, own: u8, t_ns: i64, carry_ns: i64, carried: &mut bool) -> Option<u8> {
    if own != NONE_U8 {
        h.v = own;
        h.at_ns = t_ns;
        return Some(own);
    }
    // A difference past the range of an i64 is not within the carry age.
    if h.at_ns != i64::MIN && t_ns.checked_sub(h.at_ns).is_some_and(|d| d <= carry_ns) {
        *carried = true;
        return Some(h.v);
    }
    None
}

/// Applies the truth selection rules to one row at `t_ns` and commits the carried values (only
/// for a row that is in range, not stale and from an ADS-B source). The first failing rule is the
/// reason. [`quality_eval_row`] is the same without the commit.
pub fn quality_check_row(
    st: &mut QualityState,
    row: &QualityRow,
    t_ns: i64,
    cfg: &TruthConfig,
) -> QualityVerdict {
    let (verdict, next) = quality_eval_row(st, row, t_ns, cfg);
    *st = next;
    verdict
}

/// The rules of [`quality_check_row`] against `st`, which is not changed; returns the verdict and the
/// carried values to commit if the caller keeps the row (equal to `st` for a row that is out of
/// range, stale or from another source).
pub fn quality_eval_row(
    st: &QualityState,
    row: &QualityRow,
    t_ns: i64,
    cfg: &TruthConfig,
) -> (QualityVerdict, QualityState) {
    let carry_ns = cfg.quality_carry_ms * NS_PER_MS;
    // The carried values are updated on a copy and kept only if the row is a usable ADS-B row:
    // one that is stale, out of range or from another source (which prints no meaningful quality)
    // must not vouch for the rows after it.
    let mut trial = *st;
    let mut carried = false;
    let nacp = held_value(&mut trial.nacp, row.nac_p, t_ns, carry_ns, &mut carried);
    let nic = held_value(&mut trial.nic, row.nic, t_ns, carry_ns, &mut carried);
    let sil = held_value(&mut trial.sil, row.sil, t_ns, carry_ns, &mut carried);
    let ver = held_value(
        &mut trial.version,
        row.version,
        t_ns,
        carry_ns,
        &mut carried,
    );
    // NACv does not count toward `carried` (truth statistics are about the checked fields).
    let mut ignore = false;
    let nacv = held_value(&mut trial.nacv, row.nac_v, t_ns, carry_ns, &mut ignore);
    let removal = if !(-900_000_000..=900_000_000).contains(&row.lat_e7)
        || !(-1_800_000_000..=1_800_000_000).contains(&row.lon_e7)
    {
        Some(REM_INVALID)
    } else if row.flags & F_STALE != 0 {
        Some(REM_STALE)
    } else if row.source != NONE_U8 && !ADSB_SOURCES.contains(&row.source) {
        Some(REM_SOURCE)
    } else if nacp.is_none() || nic.is_none() || sil.is_none() {
        Some(REM_NO_QUALITY)
    } else if nacp.is_some_and(|v| v < cfg.min_nacp || v > 11) {
        Some(REM_NACP)
    } else if nic.is_some_and(|v| v < cfg.min_nic) {
        Some(REM_NIC)
    } else if sil.is_some_and(|v| v < cfg.required_sil) {
        Some(REM_SIL)
    } else if ver.is_some_and(|v| v < cfg.min_version) {
        Some(REM_VERSION)
    } else {
        None
    };
    let next = if matches!(removal, Some(REM_INVALID | REM_STALE | REM_SOURCE)) {
        *st
    } else {
        trial
    };
    (
        QualityVerdict {
            removal,
            carried,
            nacp,
            nacv,
            nic,
            sil,
            version: ver,
        },
        next,
    )
}
