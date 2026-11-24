//! Deterministic conversion of decoded recording rows into the normalized regional timeline.
//! Shell adapter (RULESET B1, B3, B7); callers own file decoding and receipt metadata.

use crate::input::{EventKind, Identity, NormalizedEvent, SourceKind};

/// Stable source-order key supplied by the recording reader. It is preserved verbatim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordedPosition {
    pub receipt_ns: i64,
    pub source_ns: i64,
    pub sequence: u64,
    pub address: u32,
    /// `Unique` is positive source evidence. `Unknown` and `Ambiguous` never receive a guessed ID.
    pub identity: RecordedIdentity,
    pub centre: [u8; 4],
    pub latitude_e7: i32,
    pub longitude_e7: i32,
    pub altitude_ft: Option<i32>,
    pub east_mmps: i32,
    pub north_mmps: i32,
    pub vertical_rate_fpm: Option<i32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordedIdentity {
    Unknown,
    Unique { gufi: u128 },
    Ambiguous,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AdapterCounts {
    pub examined: u64,
    pub emitted: u64,
    pub unknown_identity: u64,
    pub ambiguous_identity: u64,
    pub invalid_record: u64,
    pub invalid_time: u64,
    pub sealed_time: u64,
    pub invalid_address: u64,
    pub invalid_coordinate: u64,
    pub invalid_altitude: u64,
    pub invalid_centre: u64,
    pub over_capacity: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterOutput {
    /// Emitted rows remain in the exact input order; the adapter never sorts by either clock.
    pub events: Vec<NormalizedEvent>,
    pub counts: AdapterCounts,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdapterError {
    ZeroCapacity,
    CountOverflow,
    UnsupportedEvent,
    InvalidCentre,
}

/// Development sizing for the next adapter increment. The work cap is checked against decoded
/// rows before conversion; byte capacity is checked by the caller before retaining source rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordingBudget {
    pub max_rows: usize,
    pub max_input_bytes: usize,
    pub max_work_items: usize,
}

/// 100k rows, 16 MiB on disk, bounded to one row conversion each. This is an offline development
/// profile aligned to the session input envelope, not a realtime or facility capacity claim.
pub const DEVELOPMENT_RECORDING_BUDGET: RecordingBudget = RecordingBudget {
    max_rows: 100_000,
    max_input_bytes: 16 * 1024 * 1024,
    max_work_items: 100_000,
};

/// Worst-case retained normalized event storage for this profile, excluding allocator overhead.
/// This arithmetic is deliberately based on the actual inline event size, including route payload
/// space, rather than assuming every future input is a small position row.
pub fn event_storage_budget(budget: RecordingBudget) -> Result<usize, AdapterError> {
    core::mem::size_of::<NormalizedEvent>()
        .checked_mul(budget.max_rows)
        .ok_or(AdapterError::CountOverflow)
}

/// Converts a bounded prefix of decoded ADS-B positions. Oversize input emits no prefix: the
/// whole recording batch is refused and `over_capacity` records its size. Missing GUFI is emitted
/// as explicit zero for raw positions; ambiguous identity is refused. GUFI is never inferred.
pub fn adapt_positions(rows: &[RecordedPosition], capacity: usize) -> Result<AdapterOutput, AdapterError> {
    if capacity == 0 {
        return Err(AdapterError::ZeroCapacity);
    }
    let mut counts = AdapterCounts {
        examined: u64::try_from(rows.len()).map_err(|_| AdapterError::CountOverflow)?,
        ..AdapterCounts::default()
    };
    let mut events = Vec::with_capacity(rows.len().min(capacity));
    if rows.len() > capacity {
        counts.over_capacity = u64::try_from(rows.len() - capacity).map_err(|_| AdapterError::CountOverflow)?;
        return Ok(AdapterOutput { events, counts });
    }
    for row in rows {
        let gufi = match row.identity {
            RecordedIdentity::Unknown => { counts.unknown_identity += 1; 0 }
            RecordedIdentity::Ambiguous => { counts.ambiguous_identity += 1; continue; }
            RecordedIdentity::Unique { gufi: 0 } => { counts.invalid_record += 1; continue; }
            RecordedIdentity::Unique { gufi } => gufi,
        };
        if row.receipt_ns < 0 || row.source_ns < 0 || row.source_ns > row.receipt_ns {
            counts.invalid_record += 1;
            counts.invalid_time += 1;
            continue;
        }
        if oa_sealed::in_sealed_window(row.receipt_ns) || oa_sealed::in_sealed_window(row.source_ns) {
            counts.invalid_record += 1;
            counts.sealed_time += 1;
            continue;
        }
        if row.address == 0 || row.address > 0x01ff_ffff {
            counts.invalid_record += 1;
            counts.invalid_address += 1;
            continue;
        }
        if !(-900_000_000..=900_000_000).contains(&row.latitude_e7)
            || !(-1_800_000_000..=1_800_000_000).contains(&row.longitude_e7)
        {
            counts.invalid_record += 1;
            counts.invalid_coordinate += 1;
            continue;
        }
        if row.altitude_ft.is_some_and(|v| !(-2_000..=100_000).contains(&v)) {
            counts.invalid_record += 1;
            counts.invalid_altitude += 1;
            continue;
        }
        if !row.centre.iter().all(u8::is_ascii_graphic) {
            counts.invalid_record += 1;
            counts.invalid_centre += 1;
            continue;
        }
        events.push(NormalizedEvent {
            receipt_ns: row.receipt_ns,
            source_ns: row.source_ns,
            sequence: row.sequence,
            source: SourceKind::Surveillance,
            identity: Identity { flight_gufi: gufi, aircraft_address: row.address },
            centre: row.centre,
            kind: EventKind::Position {
                latitude_e7: row.latitude_e7,
                longitude_e7: row.longitude_e7,
                altitude_ft: row.altitude_ft,
                east_mmps: row.east_mmps,
                north_mmps: row.north_mmps,
                vertical_rate_fpm: row.vertical_rate_fpm,
            },
        });
        counts.emitted += 1;
    }
    Ok(AdapterOutput { events, counts })
}

/// Serializes supported output rows using the existing normalized protocol's position grammar.
/// The caller must supply complete output; refusal accounting remains with `AdapterOutput`.
pub fn serialize_positions(events: &[NormalizedEvent]) -> Result<String, AdapterError> {
    let mut out = String::from("OA-RS\t1\n");
    for event in events {
        let EventKind::Position { latitude_e7, longitude_e7, altitude_ft, east_mmps, north_mmps, vertical_rate_fpm } = event.kind else {
            return Err(AdapterError::UnsupportedEvent);
        };
        if event.source != SourceKind::Surveillance { return Err(AdapterError::UnsupportedEvent); }
        if !event.centre.iter().all(u8::is_ascii_graphic) { return Err(AdapterError::InvalidCentre); }
        use core::fmt::Write;
        writeln!(out, "v1\t{}\t{}\t{}\tSURVEILLANCE\t{:032X}\t{:06X}\t{}\tPOS\t{}\t{}\t{}\t{}\t{}\t{}",
            event.receipt_ns, event.source_ns, event.sequence, event.identity.flight_gufi,
            event.identity.aircraft_address,
            core::str::from_utf8(&event.centre).map_err(|_| AdapterError::InvalidCentre)?, latitude_e7, longitude_e7,
            altitude_ft.map_or_else(|| "-".to_owned(), |v| v.to_string()), east_mmps,
            north_mmps, vertical_rate_fpm.map_or_else(|| "-".to_owned(), |v| v.to_string()))
            .map_err(|_| AdapterError::CountOverflow)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: RecordedIdentity, seq: u64) -> RecordedPosition {
        RecordedPosition { receipt_ns: 20, source_ns: 10, sequence: seq, address: 0xABCDEF,
            identity: id, centre: *b"ZAU1", latitude_e7: 410_000_000, longitude_e7: -870_000_000,
            altitude_ft: None, east_mmps: 123, north_mmps: -456, vertical_rate_fpm: Some(0) }
    }

    #[test]
    fn retains_input_order_and_source_clock_with_explicit_unknown_gufi() {
        let rows = [row(RecordedIdentity::Unique { gufi: 0x1122 }, 7), row(RecordedIdentity::Unique { gufi: 0x3344 }, 6)];
        let got = adapt_positions(&rows, 2).unwrap();
        assert_eq!(got.events.iter().map(|e| (e.sequence, e.source_ns, e.identity.flight_gufi)).collect::<Vec<_>>(), vec![(7, 10, 0x1122), (6, 10, 0x3344)]);
        assert_eq!(serialize_positions(&got.events).unwrap(), "OA-RS\t1\nv1\t20\t10\t7\tSURVEILLANCE\t00000000000000000000000000001122\tABCDEF\tZAU1\tPOS\t410000000\t-870000000\t-\t123\t-456\t0\nv1\t20\t10\t6\tSURVEILLANCE\t00000000000000000000000000003344\tABCDEF\tZAU1\tPOS\t410000000\t-870000000\t-\t123\t-456\t0\n");
    }

    #[test]
    fn accounts_unknown_ambiguous_invalid_and_oversize_without_guessing() {
        let rows = [row(RecordedIdentity::Unknown, 1), row(RecordedIdentity::Ambiguous, 2), row(RecordedIdentity::Unique { gufi: 0 }, 3)];
        let got = adapt_positions(&rows, 3).unwrap();
        assert_eq!(got.events.len(), 1);
        assert_eq!(got.events[0].identity.flight_gufi, 0);
        assert_eq!(got.counts.unknown_identity, 1);
        assert_eq!(got.counts.ambiguous_identity, 1);
        assert_eq!(got.counts.invalid_record, 1);
        let too_many = [row(RecordedIdentity::Unique { gufi: 1 }, 1), row(RecordedIdentity::Unique { gufi: 2 }, 2)];
        let got = adapt_positions(&too_many, 1).unwrap();
        assert!(got.events.is_empty());
        assert_eq!(got.counts.over_capacity, 1);
    }

    #[test]
    fn empty_and_invalid_time_or_coordinate_have_exact_accounting() {
        let empty = adapt_positions(&[], 4).unwrap();
        assert!(empty.events.is_empty());
        assert_eq!(empty.counts, AdapterCounts::default());
        let mut bad_time = row(RecordedIdentity::Unique { gufi: 5 }, 1);
        bad_time.source_ns = 21;
        let mut bad_coordinate = row(RecordedIdentity::Unique { gufi: 6 }, 2);
        bad_coordinate.latitude_e7 = 900_000_001;
        let got = adapt_positions(&[bad_time, bad_coordinate], 2).unwrap();
        assert!(got.events.is_empty());
        assert_eq!(got.counts.examined, 2);
        assert_eq!(got.counts.invalid_record, 2);
        assert_eq!(event_storage_budget(DEVELOPMENT_RECORDING_BUDGET).unwrap(),
            core::mem::size_of::<NormalizedEvent>() * 100_000);
    }
}
