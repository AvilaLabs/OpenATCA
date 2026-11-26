//! Per-scan TBFM state binding for the regional scope caller (shell boundary).
//!
//! This module accepts a caller-owned merged TBFM snapshot. It does not decode XML or infer a
//! cross-feed identity. A row is attached only when the source adapter supplies an exact full
//! regional flight key and address, and the live regional flight table confirms that pair at the
//! requested receipt time. Without that source-qualified binding, the row is counted unbound.
//!
//! The checked public TBFM publication is namespace 1.1.0 (`oa_tbfm::NAMESPACE`). Its surveyed
//! `air/sta` section has no `sta_mfx` element; `sta_sfx` is a separate scheduling-fix value and is
//! not substituted for it. The feed schema also does not establish a regional-flight join
//! contract for the caller. Until FAA supplies the STA-at-meter-fix field or an authoritative
//! join key, source rows remain unbound and the packet is an optional caller input only.

use crate::flight::{
    flight_view_for_gufi, flight_views_for_address, FlightDriver, FlightViewStatus,
};
use oa_display::core::metering::{meter_counts_observe, meter_fields, MeterCounts, TargetMetering};
use oa_scope_replay::rows::AddressMetering;
use std::fmt;

pub const MAX_METERING_SCAN_ROWS: usize = 4096;

/// One merged metering state row supplied for a single regional scan.
///
/// `feed_gufi` and `feed_row_sha256` preserve source provenance. `flight_gufi` and `address` are
/// present only after the upstream adapter has established an exact identity relation; this
/// function independently checks that exact pair against the current regional flight table.
/// Missing identity fields remain unbound and are never joined by callsign or address alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeteringScanRow {
    centre: [u8; 4],
    feed_gufi: [u8; 12],
    feed_row_sha256: [u8; 32],
    source_time_ns: Option<i64>,
    received_time_ns: i64,
    binding: Option<RegionalFlightBinding>,
    values: TargetMetering,
}

/// Exact SFDPS flight key/address association. This cannot be constructed by the public caller;
/// an in-crate decoder adapter may create it only after its identity contract is source-verified.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RegionalFlightBinding {
    flight_gufi: u128,
    address: u32,
}

impl MeteringScanRow {
    /// Preserves a source row that has no independently supported target identity link.
    pub const fn unbound(
        centre: [u8; 4],
        feed_gufi: [u8; 12],
        feed_row_sha256: [u8; 32],
        source_time_ns: Option<i64>,
        received_time_ns: i64,
        values: TargetMetering,
    ) -> Self {
        Self {
            centre,
            feed_gufi,
            feed_row_sha256,
            source_time_ns,
            received_time_ns,
            binding: None,
            values,
        }
    }

    /// Adapter-only constructor for an exact, source-verified full identity relation.
    ///
    /// The caller must have a normative source contract for the feed-GUFI-to-regional-flight
    /// relation. The current public FAA material does not establish the TBFM relation, so there
    /// is intentionally no current production caller for this constructor.
    #[cfg(test)]
    pub(crate) const fn with_verified_binding(
        mut row: Self,
        flight_gufi: u128,
        address: u32,
    ) -> Self {
        row.binding = Some(RegionalFlightBinding {
            flight_gufi,
            address,
        });
        row
    }
}

/// Checked outcomes for the optional per-scan binding step.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MeteringScanReport {
    pub source_rows: u32,
    pub flight_bound_rows: u32,
    pub target_attached_rows: u32,
    pub target_unmatched_rows: u32,
    pub targets_without_meter_row: u32,
    pub unbound_rows: u32,
    pub future_source_time_rows: u32,
    pub missing_source_time_rows: u32,
    pub invalid_source_time_rows: u32,
    pub ambiguous_address_rows: u32,
    pub oldest_source_age_ms: Option<u64>,
    /// STA/ETA dispositions for the unique, current-flight-bound MFX values in this scan.
    pub mfx_counts: MeterCounts,
    /// STA/ETA dispositions for the unique, current-flight-bound SFX values in this scan.
    pub sfx_counts: MeterCounts,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeteringScanError {
    TooMany { requested: usize, limit: usize },
    MalformedFeedIdentity { index: usize },
    UnorderedOrDuplicateFeedIdentity { index: usize },
    InvalidReceiveTime { index: usize },
    FutureReceiveTime { index: usize },
    AddressCapacity,
    MeterCountOverflow,
}

impl fmt::Display for MeteringScanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooMany { requested, limit } => {
                write!(f, "metering scan has {requested} rows; maximum is {limit}")
            }
            Self::MalformedFeedIdentity { index } => {
                write!(f, "metering source row {index} has malformed feed GUFI")
            }
            Self::UnorderedOrDuplicateFeedIdentity { index } => write!(
                f,
                "metering source row {index} duplicates or precedes the prior (centre, feed GUFI)"
            ),
            Self::InvalidReceiveTime { index } => {
                write!(
                    f,
                    "metering source row {index} has a nonpositive receive time"
                )
            }
            Self::FutureReceiveTime { index } => write!(
                f,
                "metering source row {index} was received after the regional scan"
            ),
            Self::AddressCapacity => f.write_str("resolved metering rows exceed target capacity"),
            Self::MeterCountOverflow => f.write_str("metering scan disposition counter overflow"),
        }
    }
}

impl std::error::Error for MeteringScanError {}

/// Validates source order and resolves only exact current full-key/address bindings.
///
/// A missing source time or identity is retained in the disposition counters, not replaced by a
/// regional timestamp or guessed key. Old source time is reported as age only; this function does
/// not invent a staleness threshold. The result is address-sorted for the bounded scope builder.
pub fn resolve_metering_scan(
    rows: &[MeteringScanRow],
    centre: [u8; 4],
    scan_time_ns: i64,
    cutoff_ms: i64,
    flights: &mut FlightDriver,
    view_scratch: &mut Vec<crate::flight::SharedFlightView>,
) -> Result<(Vec<AddressMetering>, MeteringScanReport), MeteringScanError> {
    if rows.len() > MAX_METERING_SCAN_ROWS {
        return Err(MeteringScanError::TooMany {
            requested: rows.len(),
            limit: MAX_METERING_SCAN_ROWS,
        });
    }
    let mut report = MeteringScanReport {
        source_rows: u32::try_from(rows.len()).unwrap_or(u32::MAX),
        ..MeteringScanReport::default()
    };
    let mut resolved = Vec::new();
    resolved
        .try_reserve_exact(rows.len())
        .map_err(|_| MeteringScanError::AddressCapacity)?;
    for (index, row) in rows.iter().enumerate() {
        let name_len = row
            .feed_gufi
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(12);
        if name_len != 10
            || row.feed_gufi.iter().skip(name_len).any(|byte| *byte != 0)
            || !row
                .feed_gufi
                .iter()
                .take(name_len)
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
            || row.feed_row_sha256.iter().all(|byte| *byte == 0)
        {
            return Err(MeteringScanError::MalformedFeedIdentity { index });
        }
        if index > 0 {
            let prior = rows
                .get(index - 1)
                .ok_or(MeteringScanError::UnorderedOrDuplicateFeedIdentity { index })?;
            if (prior.centre, prior.feed_gufi) >= (row.centre, row.feed_gufi) {
                return Err(MeteringScanError::UnorderedOrDuplicateFeedIdentity { index });
            }
        }
        if row.received_time_ns <= 0 {
            return Err(MeteringScanError::InvalidReceiveTime { index });
        }
        if row.received_time_ns > scan_time_ns {
            return Err(MeteringScanError::FutureReceiveTime { index });
        }
        if row
            .source_time_ns
            .is_some_and(|source_time_ns| source_time_ns > row.received_time_ns)
        {
            report.future_source_time_rows = report.future_source_time_rows.saturating_add(1);
            report.unbound_rows = report.unbound_rows.saturating_add(1);
            continue;
        }
        if row
            .source_time_ns
            .is_some_and(|source_time_ns| source_time_ns < 0)
        {
            report.invalid_source_time_rows = report.invalid_source_time_rows.saturating_add(1);
            report.unbound_rows = report.unbound_rows.saturating_add(1);
            continue;
        }
        if let Some(source_time_ns) = row.source_time_ns {
            let source_age_ns = scan_time_ns.saturating_sub(source_time_ns);
            let source_age_ms = u64::try_from(source_age_ns.max(0) / 1_000_000).unwrap_or(u64::MAX);
            report.oldest_source_age_ms = Some(
                report
                    .oldest_source_age_ms
                    .map_or(source_age_ms, |prior| prior.max(source_age_ms)),
            );
        }
        if row.centre != centre {
            report.unbound_rows = report.unbound_rows.saturating_add(1);
            continue;
        }
        let Some(binding) = row.binding else {
            report.unbound_rows = report.unbound_rows.saturating_add(1);
            if row.source_time_ns.is_none() {
                report.missing_source_time_rows = report.missing_source_time_rows.saturating_add(1);
            }
            continue;
        };
        let flight_gufi = binding.flight_gufi;
        let address = binding.address;
        if flight_gufi == 0 || address == 0 || address > 0x00ff_ffff {
            report.unbound_rows = report.unbound_rows.saturating_add(1);
            continue;
        }
        let _source_time_ns = match row.source_time_ns {
            Some(value) => value,
            None => {
                report.missing_source_time_rows = report.missing_source_time_rows.saturating_add(1);
                report.unbound_rows = report.unbound_rows.saturating_add(1);
                continue;
            }
        };
        let by_key = flight_view_for_gufi(flights, flight_gufi, centre, cutoff_ms);
        let by_address =
            flight_views_for_address(flights, address, centre, cutoff_ms, view_scratch);
        let exact = matches!(by_key, Ok(view) if view.gufi == flight_gufi && view.address == address)
            && by_address.status == FlightViewStatus::Found
            && by_address.claimants == 1
            && view_scratch.len() == 1
            && view_scratch
                .first()
                .is_some_and(|view| view.gufi == flight_gufi && view.address == address);
        if !exact {
            if by_address.status == FlightViewStatus::AmbiguousAddress || by_address.claimants > 1 {
                report.ambiguous_address_rows = report.ambiguous_address_rows.saturating_add(1);
            }
            report.unbound_rows = report.unbound_rows.saturating_add(1);
            continue;
        }
        resolved.push(AddressMetering {
            address,
            values: row.values,
        });
    }
    resolved.sort_unstable_by_key(|row| row.address);
    if resolved.len() > crate::surveillance::MAX_TRACKS {
        return Err(MeteringScanError::AddressCapacity);
    }
    let mut unique = Vec::new();
    unique
        .try_reserve_exact(resolved.len())
        .map_err(|_| MeteringScanError::AddressCapacity)?;
    let mut index = 0usize;
    while index < resolved.len() {
        let row = resolved
            .get(index)
            .copied()
            .ok_or(MeteringScanError::AddressCapacity)?;
        let mut end = index + 1;
        while resolved
            .get(end)
            .is_some_and(|other| other.address == row.address)
        {
            end += 1;
        }
        if end - index == 1 {
            unique.push(row);
        } else {
            report.ambiguous_address_rows = report
                .ambiguous_address_rows
                .saturating_add(u32::try_from(end - index).unwrap_or(u32::MAX));
            report.unbound_rows = report
                .unbound_rows
                .saturating_add(u32::try_from(end - index).unwrap_or(u32::MAX));
        }
        index = end;
    }
    let resolved = unique;
    report.flight_bound_rows = u32::try_from(resolved.len()).unwrap_or(u32::MAX);
    for row in &resolved {
        meter_counts_observe(
            &mut report.mfx_counts,
            meter_fields(row.values.mfx.sta_ms, row.values.mfx.eta_ms),
        )
        .map_err(|_| MeteringScanError::MeterCountOverflow)?;
        meter_counts_observe(
            &mut report.sfx_counts,
            meter_fields(row.values.sfx.sta_ms, row.values.sfx.eta_ms),
        )
        .map_err(|_| MeteringScanError::MeterCountOverflow)?;
    }
    Ok((resolved, report))
}

// Deterministic fixtures have fixed valid inputs; test-only panic assertions report fixture bugs.
#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn driver() -> FlightDriver {
        crate::flight::flight_driver_new(crate::flight::FlightDriverConfig {
            flight_capacity: 1,
            refusal_log_capacity: 1,
            conflict_log_capacity: 1,
        })
        .expect("bounded test flight driver")
    }

    fn row(key: [u8; 12]) -> MeteringScanRow {
        MeteringScanRow::unbound(
            *b"ZAU1",
            key,
            [1; 32],
            Some(900_000_000),
            1_000_000_000,
            TargetMetering::default(),
        )
    }

    // REQ: OA-REG-021
    #[test]
    fn unbound_source_rows_are_counted_and_never_joined_by_feed_identity_alone() {
        let rows = [row(*b"N123456789\0\0")];
        let mut flights = driver();
        let mut scratch = Vec::new();
        let (bound, report) = resolve_metering_scan(
            &rows,
            *b"ZAU1",
            1_000_000_000,
            1_000,
            &mut flights,
            &mut scratch,
        )
        .expect("valid unbound source row");
        assert!(bound.is_empty());
        assert_eq!(report.source_rows, 1);
        assert_eq!(report.flight_bound_rows, 0);
        assert_eq!(report.unbound_rows, 1);
        assert_eq!(report.oldest_source_age_ms, Some(100));
    }

    // REQ: OA-REG-022
    #[test]
    fn malformed_feed_identity_duplicate_order_and_future_receive_refuse() {
        let malformed = [row(*b"bad-key\0\0\0\0\0")];
        let duplicate = [row(*b"N123456789\0\0"), row(*b"N123456789\0\0")];
        let future = [MeteringScanRow::unbound(
            *b"ZAU1",
            *b"N123456789\0\0",
            [1; 32],
            Some(1_000_000_000),
            1_000_000_001,
            TargetMetering::default(),
        )];
        let invalid_receive = [MeteringScanRow::unbound(
            *b"ZAU1",
            *b"N123456789\0\0",
            [1; 32],
            None,
            0,
            TargetMetering::default(),
        )];
        let mut flights = driver();
        let mut scratch = Vec::new();
        assert!(matches!(
            resolve_metering_scan(
                &malformed,
                *b"ZAU1",
                1_000_000_000,
                1_000,
                &mut flights,
                &mut scratch,
            ),
            Err(MeteringScanError::MalformedFeedIdentity { .. })
        ));
        assert!(matches!(
            resolve_metering_scan(
                &duplicate,
                *b"ZAU1",
                1_000_000_000,
                1_000,
                &mut flights,
                &mut scratch,
            ),
            Err(MeteringScanError::UnorderedOrDuplicateFeedIdentity { .. })
        ));
        assert!(matches!(
            resolve_metering_scan(
                &future,
                *b"ZAU1",
                1_000_000_000,
                1_000,
                &mut flights,
                &mut scratch,
            ),
            Err(MeteringScanError::FutureReceiveTime { .. })
        ));
        assert!(matches!(
            resolve_metering_scan(
                &invalid_receive,
                *b"ZAU1",
                1_000_000_000,
                1_000,
                &mut flights,
                &mut scratch,
            ),
            Err(MeteringScanError::InvalidReceiveTime { .. })
        ));
    }
}
