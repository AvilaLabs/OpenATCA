//! The quality rules of truth selection, shared with the tracker. Core (pure).
//!
//! A readsb trace row prints NACp, NIC, SIL and the version on a minority of rows, so each is
//! carried forward within an aircraft for at most [`crate::QUALITY_CARRY_MS`]; a row with no
//! value in that age fails (reason `no_quality_in_age`). [`quality_check`] holds exactly the
//! per-row rules of [`crate::build_truth`] (validity, stale flag, source, the four gates) so the
//! tracker's input gating is the same code, not a copy. NACv is carried the same way for the
//! tracker's velocity noise; truth does not use it.

use oa_adsb::{Row, F_STALE, NONE_U8};

use crate::{
    TruthConfig, ADSB_SOURCES, REM_INVALID, REM_NACP, REM_NIC, REM_NO_QUALITY, REM_SIL, REM_SOURCE,
    REM_STALE, REM_VERSION,
};

const NS_PER_MS: i64 = 1_000_000;

/// 95 % radius in metres for NACp 0 to 11 (DO-260B table 2-14); NACp 0 is "unknown".
const NACP_EPU_M: [f64; 12] = [
    f64::INFINITY,
    18_520.0,
    7_408.0,
    3_704.0,
    1_852.0,
    926.0,
    555.6,
    185.2,
    92.6,
    30.0,
    10.0,
    3.0,
];

/// 95 % horizontal velocity error bound in m/s for NACv 0 to 4 (DO-260B table 2-22); NACv 0 is
/// "unknown or 10 m/s or more".
const NACV_EPU_MPS: [f64; 5] = [f64::INFINITY, 10.0, 3.0, 1.0, 0.3];

/// The 95 % horizontal position radius of a NACp, metres; infinite for 0 and above 11.
pub fn nacp_epu_m(nacp: u8) -> f64 {
    NACP_EPU_M
        .get(usize::from(nacp))
        .copied()
        .unwrap_or(f64::INFINITY)
}

/// The 95 % horizontal velocity bound of a NACv, m/s; infinite for 0 and above 4.
pub fn nacv_epu_mps(nacv: u8) -> f64 {
    NACV_EPU_MPS
        .get(usize::from(nacv))
        .copied()
        .unwrap_or(f64::INFINITY)
}

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

/// The value for a row: its own if present, else the held one if young enough.
fn held_value(h: &mut Held, own: u8, t_ns: i64, carry_ns: i64, carried: &mut bool) -> Option<u8> {
    if own != NONE_U8 {
        h.v = own;
        h.at_ns = t_ns;
        return Some(own);
    }
    if h.at_ns != i64::MIN && t_ns - h.at_ns <= carry_ns {
        *carried = true;
        return Some(h.v);
    }
    None
}

/// Applies the truth selection rules to one row at `t_ns`, updating the carried values (only for a row that is in range, not stale and from an ADS-B
/// source). The first failing rule is the reason.
pub fn quality_check(
    st: &mut QualityState,
    row: &Row,
    t_ns: i64,
    cfg: &TruthConfig,
) -> QualityVerdict {
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
    if !matches!(removal, Some(REM_INVALID | REM_STALE | REM_SOURCE)) {
        *st = trial;
    }
    QualityVerdict {
        removal,
        carried,
        nacp,
        nacv,
        nic,
        sil,
        version: ver,
    }
}
