//! Independent raw-surveillance display reduction (core, B1): the caller supplies bounded,
//! address/time-sorted ADS-B observations as plain columns. The caller must include only
//! observations available by the as-of time: these columns encode observation time, not receipt
//! time. This path does not accept tracker rows, infer ownership, extrapolate, or coast. Only the
//! newest observation at or before the display time is eligible; age at the inclusive finite
//! cutoff is shown, and anything older is dropped.

use oa_display::core::project::Projection;
use oa_display::core::traffic::{
    map_position, targets_clear, targets_malformed, targets_push, targets_refused, MapPos, Target,
    TargetSet, MAX_TRAIL, NO_REACQ, T_UNCERTAIN,
};
use std::fmt;

/// One input frame can contain at most this many observations in the raw fallback.
pub const MAX_RAW_OBSERVATIONS: usize = 65_536;
/// Raw observations older than this are never displayed. The boundary is inclusive.
pub const RAW_STALE_CUTOFF_NS: i64 = 3 * 1_000_000_000;

#[derive(Clone, Copy, Debug)]
pub struct RawObservationColumns<'a> {
    /// Observation time in Unix nanoseconds (not receipt time).
    /// Caller must filter by actual availability/receipt time before passing this slice.
    pub t_ns: &'a [i64],
    pub addr: &'a [u32],
    pub lat_e7: &'a [i32],
    pub lon_e7: &'a [i32],
    /// Feet, or NO_ALT when not measured.
    pub alt_ft: &'a [i32],
    /// Ground speed in tenths of a knot, or NO_U16 when unavailable.
    pub gs_dkt: &'a [u16],
    /// True track in tenths of a degree, or NO_U16 when unavailable.
    pub track_ddeg: &'a [u16],
}

/// Counts for one raw fallback build. Counts are per input observation, per address group, or per
/// refused target as named. A7.2: cold diagnostic record.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RawStats {
    pub observations_in: u64,
    pub future_observations: u64,
    pub future_only_addresses: u64,
    pub stale_addresses: u64,
    pub invalid_positions: u64,
    pub unprojectable_positions: u64,
    pub off_map_positions: u64,
    /// Distinct targets admitted to the display set (projection may still omit them).
    pub targets_admitted: u64,
    /// Address groups whose newest as-of timestamp has contradictory payloads.
    pub conflicting_timestamps: u64,
    pub refused_capacity: u64,
    pub refused_malformed: u64,
}

/// Refusal before any observation is processed. A7.2: cold diagnostic record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RawInputError {
    ColumnLength {
        column: &'static str,
        got: usize,
        want: usize,
    },
    TooMany {
        wanted: usize,
        max: usize,
    },
    NotSorted {
        index: usize,
    },
}

impl fmt::Display for RawInputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RawInputError::ColumnLength { column, got, want } => {
                write!(f, "raw column {column} has {got} values; expected {want}")
            }
            RawInputError::TooMany { wanted, max } => {
                write!(f, "{wanted} raw observations exceed the limit {max}")
            }
            RawInputError::NotSorted { index } => write!(
                f,
                "raw observations are not ordered by address then observation time at row {index}"
            ),
        }
    }
}

impl std::error::Error for RawInputError {}

fn same_or_before(a_addr: u32, a_time: i64, b_addr: u32, b_time: i64) -> bool {
    a_addr < b_addr || (a_addr == b_addr && a_time <= b_time)
}

/// Validates all column lengths, size, and the address/time sort before changing set.
fn validate_raw(columns: &RawObservationColumns<'_>) -> Result<usize, RawInputError> {
    let n = columns.t_ns.len();
    for (column, got) in [
        ("addr", columns.addr.len()),
        ("lat_e7", columns.lat_e7.len()),
        ("lon_e7", columns.lon_e7.len()),
        ("alt_ft", columns.alt_ft.len()),
        ("gs_dkt", columns.gs_dkt.len()),
        ("track_ddeg", columns.track_ddeg.len()),
    ] {
        if got != n {
            return Err(RawInputError::ColumnLength {
                column,
                got,
                want: n,
            });
        }
    }
    if n > MAX_RAW_OBSERVATIONS {
        return Err(RawInputError::TooMany {
            wanted: n,
            max: MAX_RAW_OBSERVATIONS,
        });
    }
    let mut prev: Option<(u32, i64)> = None;
    for (index, (&addr, &t_ns)) in columns.addr.iter().zip(columns.t_ns).enumerate() {
        if prev.is_some_and(|(a, t)| !same_or_before(a, t, addr, t_ns)) {
            return Err(RawInputError::NotSorted { index });
        }
        prev = Some((addr, t_ns));
    }
    Ok(n)
}

/// Fills set with latest-at-as-of raw observations in address order. Exact duplicate rows at an
/// equal newest timestamp collapse; conflicting payloads at that timestamp refuse the address,
/// without falling back to older data. Observations outside the map do not consume target slots.
/// Targets carry reported positions and kinematics. The reducer supplies no predicted positions
/// or ownership metadata.
pub fn raw_targets_at(
    columns: &RawObservationColumns<'_>,
    as_of_ns: i64,
    proj: &Projection,
    set: &mut TargetSet,
) -> Result<RawStats, RawInputError> {
    let n = validate_raw(columns)?;
    targets_clear(set);
    let mut stats = RawStats {
        observations_in: n as u64,
        ..RawStats::default()
    };
    let mut start = 0usize;
    while start < n {
        let Some(&addr) = columns.addr.get(start) else {
            return Err(RawInputError::ColumnLength {
                column: "addr",
                got: start,
                want: n,
            });
        };
        let mut end = start.saturating_add(1);
        while end < n && columns.addr.get(end).copied() == Some(addr) {
            end = end.saturating_add(1);
        }

        let mut newest_time = None;
        for index in start..end {
            let Some(&t_ns) = columns.t_ns.get(index) else {
                return Err(RawInputError::ColumnLength {
                    column: "t_ns",
                    got: index,
                    want: n,
                });
            };
            if t_ns <= as_of_ns {
                newest_time = Some(t_ns);
            } else {
                stats.future_observations = stats.future_observations.saturating_add(1);
            }
        }
        let Some(newest_time) = newest_time else {
            stats.future_only_addresses = stats.future_only_addresses.saturating_add(1);
            start = end;
            continue;
        };
        let mut newest_payload = None;
        let mut conflict = false;
        for index in start..end {
            if columns.t_ns.get(index).copied() != Some(newest_time) {
                continue;
            }
            let payload = columns
                .lat_e7
                .get(index)
                .zip(columns.lon_e7.get(index))
                .zip(columns.alt_ft.get(index))
                .zip(columns.gs_dkt.get(index))
                .zip(columns.track_ddeg.get(index))
                .map(|(((((lat, lon), alt), gs), track))| (*lat, *lon, *alt, *gs, *track));
            let Some(payload) = payload else {
                return Err(RawInputError::ColumnLength {
                    column: "observation",
                    got: index,
                    want: n,
                });
            };
            if newest_payload.is_some_and(|previous| previous != payload) {
                conflict = true;
                break;
            }
            newest_payload = Some(payload);
        }
        if conflict {
            stats.conflicting_timestamps = stats.conflicting_timestamps.saturating_add(1);
            start = end;
            continue;
        }
        let Some((lat_e7, lon_e7, alt_ft, gs_dkt, track_ddeg)) = newest_payload else {
            return Err(RawInputError::ColumnLength {
                column: "observation",
                got: start,
                want: n,
            });
        };
        if as_of_ns
            .checked_sub(newest_time)
            .is_none_or(|age| age > RAW_STALE_CUTOFF_NS)
        {
            stats.stale_addresses = stats.stale_addresses.saturating_add(1);
            start = end;
            continue;
        }
        match map_position(proj, lat_e7, lon_e7) {
            MapPos::Invalid => stats.invalid_positions = stats.invalid_positions.saturating_add(1),
            MapPos::Unprojectable => {
                stats.unprojectable_positions = stats.unprojectable_positions.saturating_add(1);
            }
            MapPos::Off => stats.off_map_positions = stats.off_map_positions.saturating_add(1),
            MapPos::On(_) => {
                let target = Target {
                    lat_e7,
                    lon_e7,
                    alt_ft,
                    addr,
                    r95_m: f32::NAN,
                    trail: [(0, 0); MAX_TRAIL],
                    callsign: [0; 8],
                    track_ddeg,
                    gs_dkt,
                    reacq_age_s: NO_REACQ,
                    flags: T_UNCERTAIN,
                    trail_len: 0,
                };
                if targets_push(set, &target).is_ok() {
                    stats.targets_admitted = stats.targets_admitted.saturating_add(1);
                }
            }
        }
        start = end;
    }
    stats.refused_capacity = u64::from(targets_refused(set));
    stats.refused_malformed = u64::from(targets_malformed(set));
    Ok(stats)
}
