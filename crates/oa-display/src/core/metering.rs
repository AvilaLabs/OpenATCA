//! TBFM metering values (**core**, B1, B2): bounded plain-value packets for an optional meter
//! list. This module knows neither TBFM nor its decoder. The shell supplies Unix-millisecond
//! values and identity copied from a decoded row. No styling or compliance judgement beyond the
//! explicitly named ±60 second comparison is implied.

use core::fmt;

/// Inclusive absolute difference accepted by the display's metering window, in milliseconds.
pub const METER_WINDOW_MS: i64 = 60_000;
/// Maximum packet rows in one meter-list packet.
///
/// Engineering memory budget: `MeterPacket` is exactly 104 bytes (asserted below), so this cap
/// bounds requested row payload to 6,815,744 bytes (6.5 MiB), plus the `Vec` header and allocator
/// overhead. The count is 256 batches of the TBFM decoder's 256-row default message capacity;
/// that decoder also has an independent 1 MiB message limit. For scale, it is 1/16 of the
/// display traffic target ceiling (`MAX_TARGETS`), not an assumption that meter rows join
/// one-to-one with displayed targets. This is a resource refusal bound, not a forecast of feed
/// demand or a completeness guarantee. Larger requests are refused and pushes at capacity leave
/// the list unchanged.
pub const MAX_METER_ROWS: usize = 1 << 16;

/// One same-reference schedule/estimate pair in Unix milliseconds. `None` means the source did
/// not provide that field; it never stands for zero.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct MeterPointTimes {
    pub sta_ms: Option<i64>,
    pub eta_ms: Option<i64>,
}

const _: () = assert!(core::mem::size_of::<MeterPointTimes>() == 32);

/// No value is available for this meter reference.
pub const NO_METER_POINT_TIMES: MeterPointTimes = MeterPointTimes {
    sta_ms: None,
    eta_ms: None,
};

/// Same-reference values supplied for the meter fix and schedule fix. The two points are kept
/// separate so an SFX/arc STA cannot be mistaken for an MFX STA.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct TargetMetering {
    pub mfx: MeterPointTimes,
    pub sfx: MeterPointTimes,
}

const _: () = assert!(core::mem::size_of::<TargetMetering>() == 64);

/// No metering values are available for this target.
pub const NO_TARGET_METERING: TargetMetering = TargetMetering {
    mfx: NO_METER_POINT_TIMES,
    sfx: NO_METER_POINT_TIMES,
};

/// Result of comparing exact supplied STA and ETA values. `Early` means ETA precedes STA;
/// `Late` means ETA follows STA. The ±60 second boundary is inclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MeterStatus {
    MissingBoth = 0,
    MissingSta = 1,
    MissingEta = 2,
    WithinWindow = 3,
    Early = 4,
    Late = 5,
    DifferenceOverflow = 6,
}

/// The metering field values supplied to one packet, in Unix milliseconds.
// A7.1: fixed C layout; three Option<i64> slots plus status and alignment padding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct MeterFields {
    pub sta_ms: Option<i64>,
    pub eta_ms: Option<i64>,
    /// `STA - ETA`; positive is early and negative is late.
    pub delay_sta_minus_eta_ms: Option<i64>,
    pub status: MeterStatus,
}

const _: () = assert!(core::mem::size_of::<MeterFields>() == 56);

/// Compares the supplied values without substituting one meter point for another.
pub const fn meter_fields(sta_ms: Option<i64>, eta_ms: Option<i64>) -> MeterFields {
    let (delay_sta_minus_eta_ms, status) = match (sta_ms, eta_ms) {
        (None, None) => (None, MeterStatus::MissingBoth),
        (None, Some(_)) => (None, MeterStatus::MissingSta),
        (_, None) => (None, MeterStatus::MissingEta),
        (Some(sta), Some(eta)) => match sta.checked_sub(eta) {
            None => (None, MeterStatus::DifferenceOverflow),
            Some(delay) if delay >= -METER_WINDOW_MS && delay <= METER_WINDOW_MS => {
                (Some(delay), MeterStatus::WithinWindow)
            }
            Some(delay) if delay > METER_WINDOW_MS => (Some(delay), MeterStatus::Early),
            Some(delay) => (Some(delay), MeterStatus::Late),
        },
    };
    MeterFields {
        sta_ms,
        eta_ms,
        delay_sta_minus_eta_ms,
        status,
    }
}

/// Counts observations without hiding missing fields or arithmetic failures.
// A7.1: fixed eight-counter accumulator; counters are checked before mutation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct MeterCounts {
    pub observations: u64,
    /// Both exact STA and ETA were present; includes difference-overflow rows.
    pub paired_values: u64,
    pub within_window: u64,
    pub early: u64,
    pub late: u64,
    pub missing_sta: u64,
    pub missing_eta: u64,
    pub difference_overflow: u64,
}

const _: () = assert!(core::mem::size_of::<MeterCounts>() == 64);

/// Counter overflow refuses the observation and leaves every counter unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeterCountOverflow;

/// Adds one packet's field disposition to the bounded run counters.
pub fn meter_counts_observe(
    counts: &mut MeterCounts,
    fields: MeterFields,
) -> Result<(), MeterCountOverflow> {
    let mut next = *counts;
    next.observations = next.observations.checked_add(1).ok_or(MeterCountOverflow)?;
    match fields.status {
        MeterStatus::MissingBoth => {
            next.missing_sta = next.missing_sta.checked_add(1).ok_or(MeterCountOverflow)?;
            next.missing_eta = next.missing_eta.checked_add(1).ok_or(MeterCountOverflow)?;
        }
        MeterStatus::MissingSta => {
            next.missing_sta = next.missing_sta.checked_add(1).ok_or(MeterCountOverflow)?;
        }
        MeterStatus::MissingEta => {
            next.missing_eta = next.missing_eta.checked_add(1).ok_or(MeterCountOverflow)?;
        }
        MeterStatus::WithinWindow => {
            next.paired_values = next
                .paired_values
                .checked_add(1)
                .ok_or(MeterCountOverflow)?;
            next.within_window = next
                .within_window
                .checked_add(1)
                .ok_or(MeterCountOverflow)?;
        }
        MeterStatus::Early => {
            next.paired_values = next
                .paired_values
                .checked_add(1)
                .ok_or(MeterCountOverflow)?;
            next.early = next.early.checked_add(1).ok_or(MeterCountOverflow)?;
        }
        MeterStatus::Late => {
            next.paired_values = next
                .paired_values
                .checked_add(1)
                .ok_or(MeterCountOverflow)?;
            next.late = next.late.checked_add(1).ok_or(MeterCountOverflow)?;
        }
        MeterStatus::DifferenceOverflow => {
            next.paired_values = next
                .paired_values
                .checked_add(1)
                .ok_or(MeterCountOverflow)?;
            next.difference_overflow = next
                .difference_overflow
                .checked_add(1)
                .ok_or(MeterCountOverflow)?;
        }
    }
    *counts = next;
    Ok(())
}

/// One aircraft's exact supplied values and their comparison for one observation.
// A7.1: 104 bytes, fixed-size identity and fields; no hidden allocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct MeterPacket {
    /// Opaque consumer-supplied aircraft key, zero padded; no key is inferred here.
    pub aircraft_key: [u8; 16],
    /// Meter fix label, zero padded; no geographic resolution is performed.
    pub meter_fix: [u8; 8],
    /// Receive time of the source observation, Unix nanoseconds.
    pub observed_at_ns: i64,
    /// Source message time, Unix milliseconds; absent when the decoder reports no time.
    pub source_time_ms: Option<i64>,
    pub fields: MeterFields,
}

const _: () = assert!(core::mem::size_of::<MeterPacket>() == 104);

/// Builds a packet from exact supplied Unix-time values. A missing value stays absent.
pub const fn meter_packet_new(
    aircraft_key: [u8; 16],
    meter_fix: [u8; 8],
    observed_at_ns: i64,
    source_time_ms: Option<i64>,
    sta_ms: Option<i64>,
    eta_ms: Option<i64>,
) -> MeterPacket {
    MeterPacket {
        aircraft_key,
        meter_fix,
        observed_at_ns,
        source_time_ms,
        fields: meter_fields(sta_ms, eta_ms),
    }
}

/// A bounded, reusable packet of per-aircraft meter values.
// A7.2: cold container; allocates once at construction.
#[derive(Clone, Debug)]
pub struct MeterList {
    cap: usize,
    rows: Vec<MeterPacket>,
}

/// Why a meter-list packet could not be constructed or extended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeterListError {
    CapacityTooLarge { requested: usize, limit: usize },
    Full { capacity: usize },
    AllocationFailed { requested: usize },
}

impl fmt::Display for MeterListError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CapacityTooLarge { requested, limit } => {
                write!(f, "meter list capacity {requested} exceeds {limit}")
            }
            Self::Full { capacity } => write!(f, "meter list reached capacity {capacity}"),
            Self::AllocationFailed { requested } => {
                write!(f, "could not reserve {requested} meter rows")
            }
        }
    }
}

impl std::error::Error for MeterListError {}

/// Creates a meter list with exactly the stated row limit; the allocation is fallible.
pub fn meter_list_new(capacity: usize) -> Result<MeterList, MeterListError> {
    if capacity > MAX_METER_ROWS {
        return Err(MeterListError::CapacityTooLarge {
            requested: capacity,
            limit: MAX_METER_ROWS,
        });
    }
    let mut rows = Vec::new();
    rows.try_reserve_exact(capacity)
        .map_err(|_| MeterListError::AllocationFailed {
            requested: capacity,
        })?;
    Ok(MeterList {
        cap: capacity,
        rows,
    })
}

/// Clears a packet while preserving its preallocated capacity.
pub fn meter_list_clear(list: &mut MeterList) {
    list.rows.clear();
}

/// Adds one precomputed packet; a full list refuses without changing existing rows.
pub fn meter_list_push(list: &mut MeterList, packet: MeterPacket) -> Result<(), MeterListError> {
    if list.rows.len() >= list.cap {
        return Err(MeterListError::Full { capacity: list.cap });
    }
    list.rows.push(packet);
    Ok(())
}

/// Borrowed packet rows in insertion order.
pub fn meter_list_rows(list: &MeterList) -> &[MeterPacket] {
    &list.rows
}

/// Declared capacity of the packet.
pub fn meter_list_capacity(list: &MeterList) -> usize {
    list.cap
}

#[cfg(test)]
mod tests {
    use super::*;

    // REQ: OA-DSP-079
    #[test]
    fn scheduled_and_sixty_second_boundaries_are_inside_window() {
        assert_eq!(
            meter_fields(Some(10), Some(10)).status,
            MeterStatus::WithinWindow
        );
        assert_eq!(
            meter_fields(Some(60_010), Some(10)).status,
            MeterStatus::WithinWindow
        );
        assert_eq!(
            meter_fields(Some(10), Some(60_010)).status,
            MeterStatus::WithinWindow
        );
        assert_eq!(
            meter_fields(Some(70_010), Some(10)).delay_sta_minus_eta_ms,
            Some(70_000)
        );
    }

    // REQ: OA-DSP-080
    #[test]
    fn early_and_late_beyond_one_minute_are_signed_and_distinct() {
        let early = meter_fields(Some(120_001), Some(0));
        assert_eq!(early.delay_sta_minus_eta_ms, Some(120_001));
        assert_eq!(early.status, MeterStatus::Early);
        let late = meter_fields(Some(0), Some(120_001));
        assert_eq!(late.delay_sta_minus_eta_ms, Some(-120_001));
        assert_eq!(late.status, MeterStatus::Late);
    }

    // REQ: OA-DSP-081
    #[test]
    fn missing_fields_are_not_zero_and_schedule_change_recomputes() {
        assert_eq!(meter_fields(None, None).status, MeterStatus::MissingBoth);
        assert_eq!(meter_fields(None, Some(0)).status, MeterStatus::MissingSta);
        assert_eq!(meter_fields(Some(0), None).status, MeterStatus::MissingEta);
        assert_eq!(meter_fields(None, Some(0)).delay_sta_minus_eta_ms, None);
        assert_eq!(
            meter_fields(Some(100_000), Some(0)).status,
            MeterStatus::Early
        );
        assert_eq!(
            meter_fields(Some(50_000), Some(0)).status,
            MeterStatus::WithinWindow
        );
    }

    // REQ: OA-DSP-082
    #[test]
    fn difference_overflow_is_explicit() {
        let fields = meter_fields(Some(i64::MAX), Some(i64::MIN + 1));
        assert_eq!(fields.delay_sta_minus_eta_ms, None);
        assert_eq!(fields.status, MeterStatus::DifferenceOverflow);
        let mut counts = MeterCounts::default();
        for fields in [
            meter_fields(None, None),
            meter_fields(None, Some(1)),
            meter_fields(Some(1), None),
            meter_fields(Some(10), Some(10)),
            meter_fields(Some(120_001), Some(0)),
            meter_fields(Some(0), Some(120_001)),
        ] {
            assert_eq!(meter_counts_observe(&mut counts, fields), Ok(()));
        }
        assert_eq!(counts.observations, 6);
        assert_eq!(counts.paired_values, 3);
        assert_eq!(counts.within_window, 1);
        assert_eq!(counts.early, 1);
        assert_eq!(counts.late, 1);
        assert_eq!(counts.missing_sta, 2);
        assert_eq!(counts.missing_eta, 2);
        let mut overflow = MeterCounts {
            observations: u64::MAX,
            ..MeterCounts::default()
        };
        let before = overflow;
        assert_eq!(
            meter_counts_observe(&mut overflow, meter_fields(Some(0), Some(0))),
            Err(MeterCountOverflow)
        );
        assert_eq!(overflow, before);
    }

    // REQ: OA-DSP-083
    #[test]
    fn meter_list_is_bounded_and_reusable() {
        assert!(matches!(
            meter_list_new(MAX_METER_ROWS + 1),
            Err(MeterListError::CapacityTooLarge { .. })
        ));
        let result = meter_list_new(1);
        assert!(result.is_ok());
        let Ok(mut list) = result else { return };
        let packet = meter_packet_new([b'A'; 16], [b'M'; 8], 123, Some(1), Some(2), Some(1));
        assert_eq!(meter_list_push(&mut list, packet), Ok(()));
        assert_eq!(meter_list_rows(&list), &[packet]);
        assert_eq!(
            meter_list_push(&mut list, packet),
            Err(MeterListError::Full { capacity: 1 })
        );
        meter_list_clear(&mut list);
        assert!(meter_list_rows(&list).is_empty());
        assert_eq!(meter_list_capacity(&list), 1);
    }
}
