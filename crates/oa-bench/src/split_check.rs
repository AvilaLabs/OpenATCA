//! The check on a split of ADS-B reports into the tracker's input (set A) and the truth's input
//! (set B). Core (pure).
//!
//! The split is made by the scoring path (`oa-track`); the check is here so the benchmark owns
//! the rule: an observation is never in both sets, and a set holds no copy of one of its own
//! observations. A violation refuses the run; it is not a warning.

use oa_adsb::{reports_len, reports_row, Reports, Row};

/// Why a split is not valid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitViolation {
    /// A report with this address and time is in both sets.
    SharedTime { addr: u32, time_ns: i64 },
    /// A set holds two reports with the same address, time and content (`set` is `b'A'` or
    /// `b'B'`).
    Duplicate { set: u8, addr: u32, time_ns: i64 },
    /// A row of a set could not be read back (its columns disagree).
    Unreadable { set: u8 },
}

type Key = (u32, i64, [i64; 12]);

fn key(r: &Row) -> Key {
    (
        r.icao,
        r.time_ns,
        [
            i64::from(r.lat_e7),
            i64::from(r.lon_e7),
            i64::from(r.baro_alt_ft),
            i64::from(r.geom_alt_ft),
            i64::from(r.gs_dkt),
            i64::from(r.track_ddeg),
            i64::from(r.vrate_fpm),
            i64::from(r.nic),
            i64::from(r.nac_p),
            i64::from(r.nac_v),
            i64::from(r.sil),
            i64::from(r.version),
        ],
    )
}

/// The sorted keys of a set, or the violation if a row cannot be read or two keys are equal.
fn keys_of(r: &Reports, set: u8) -> Result<Vec<Key>, SplitViolation> {
    let mut v = Vec::with_capacity(reports_len(r));
    for i in 0..reports_len(r) {
        let row = reports_row(r, i).ok_or(SplitViolation::Unreadable { set })?;
        v.push(key(&row));
    }
    v.sort_unstable();
    for w in v.windows(2) {
        if let (Some(x), Some(y)) = (w.first(), w.get(1)) {
            if x == y {
                return Err(SplitViolation::Duplicate {
                    set,
                    addr: x.0,
                    time_ns: x.1,
                });
            }
        }
    }
    Ok(v)
}

/// Checks that no (address, time) is in both sets and that neither set holds a copy of one of
/// its own reports. The first violation is returned.
pub fn check_split(a: &Reports, b: &Reports) -> Result<(), SplitViolation> {
    let (ka, kb) = (keys_of(a, b'A')?, keys_of(b, b'B')?);
    let (mut i, mut j) = (0, 0);
    while let (Some(x), Some(y)) = (ka.get(i), kb.get(j)) {
        match (x.0, x.1).cmp(&(y.0, y.1)) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                return Err(SplitViolation::SharedTime {
                    addr: x.0,
                    time_ns: x.1,
                })
            }
        }
    }
    Ok(())
}
