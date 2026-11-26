//! Explicit UTC/GPS conversion and bounded clock-comparison evidence.
//!
//! Leap history is the IERS `TimeSteps.history` UTC step table, retrieved 2026-11-25,
//! https://hpiers.obspm.fr/eoppc/bul/bulc/TimeSteps.history . The 2016-12-31 positive
//! leap is additionally specified by IERS Bulletin C 52 (2016-07-06),
//! https://datacenter.iers.org/data/html/bulletinc-052.html . GPS epoch is 1980-01-06.
//! Bulletin C 72 (2026-07-06), https://datacenter.iers.org/data/html/bulletinc-072.html,
//! confirms no leap at 2026-12-31. Conversion is refused beyond that confirmed horizon.
//! These constants are conversion data, not a claim of source authentication or live freshness.

use crate::ProcessorId;

/// At most 128 expected processor rows in one daily clock report.
pub const MAX_CLOCK_EXPECTED: usize = 128;
/// At most 512 reported reference/sample rows in one daily clock report.
pub const MAX_CLOCK_SAMPLES: usize = 512;
pub const GPS_EPOCH_UNIX_SECONDS: i64 = 315_964_800;
const DAY_SECONDS: i64 = 86_400;
const NS_PER_SECOND: i128 = 1_000_000_000;
const VALID_UNIX_END_EXCLUSIVE: i64 = 1_798_761_600;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockError {
    InvalidUtc,
    InvalidGps,
    UnknownUtcRange,
    EmptyExpected,
    ExpectedOverCapacity,
    SamplesOverCapacity,
    DuplicateExpected,
    InvalidLimit,
    CounterOverflow,
    CapacityInvariant,
}

/// UTC civil instant. `day` is days since 1970-01-01; second 86400 is only a listed leap second.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// A7.2: cold struct, size guard waived; UTC value in an offline daily report, not per-cycle state.
pub struct UtcInstant {
    pub day: u32,
    pub second: u32,
    pub nanosecond: u32,
}

/// GPS continuous time from 1980-01-06 00:00:00.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// A7.2: cold struct, size guard waived; GPS value in an offline daily report, not per-cycle state.
pub struct GpsInstant {
    pub seconds: i64,
    pub nanosecond: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
// Budget: fixed IERS conversion-table row, scanned at most 18 times per conversion.
struct Leap {
    effective_unix: i64,
    gps_utc_after: i32,
}
const _: () = assert!(core::mem::size_of::<Leap>() == 16);

// IERS history effective UTC dates; Unix seconds and GPS-UTC after each positive step.
const LEAPS: [Leap; 18] = [
    Leap {
        effective_unix: 362_793_600,
        gps_utc_after: 1,
    },
    Leap {
        effective_unix: 394_329_600,
        gps_utc_after: 2,
    },
    Leap {
        effective_unix: 425_865_600,
        gps_utc_after: 3,
    },
    Leap {
        effective_unix: 489_024_000,
        gps_utc_after: 4,
    },
    Leap {
        effective_unix: 567_993_600,
        gps_utc_after: 5,
    },
    Leap {
        effective_unix: 631_152_000,
        gps_utc_after: 6,
    },
    Leap {
        effective_unix: 662_688_000,
        gps_utc_after: 7,
    },
    Leap {
        effective_unix: 709_948_800,
        gps_utc_after: 8,
    },
    Leap {
        effective_unix: 741_484_800,
        gps_utc_after: 9,
    },
    Leap {
        effective_unix: 773_020_800,
        gps_utc_after: 10,
    },
    Leap {
        effective_unix: 820_454_400,
        gps_utc_after: 11,
    },
    Leap {
        effective_unix: 867_715_200,
        gps_utc_after: 12,
    },
    Leap {
        effective_unix: 915_148_800,
        gps_utc_after: 13,
    },
    Leap {
        effective_unix: 1_136_073_600,
        gps_utc_after: 14,
    },
    Leap {
        effective_unix: 1_230_768_000,
        gps_utc_after: 15,
    },
    Leap {
        effective_unix: 1_341_100_800,
        gps_utc_after: 16,
    },
    Leap {
        effective_unix: 1_435_708_800,
        gps_utc_after: 17,
    },
    Leap {
        effective_unix: 1_483_228_800,
        gps_utc_after: 18,
    },
];

fn is_leap_day(day: u32) -> bool {
    let Some(next_day) = i64::from(day)
        .checked_add(1)
        .and_then(|value| value.checked_mul(DAY_SECONDS))
    else {
        return false;
    };
    LEAPS.iter().any(|leap| leap.effective_unix == next_day)
}

fn offset_at_day(day: u32) -> Result<i32, ClockError> {
    let unix = i64::from(day)
        .checked_mul(DAY_SECONDS)
        .ok_or(ClockError::UnknownUtcRange)?;
    Ok(LEAPS
        .iter()
        .take_while(|leap| leap.effective_unix <= unix)
        .last()
        .map_or(0, |leap| leap.gps_utc_after))
}

pub fn utc_to_gps(utc: UtcInstant) -> Result<GpsInstant, ClockError> {
    if utc.nanosecond >= 1_000_000_000 || utc.second > 86_400 {
        return Err(ClockError::InvalidUtc);
    }
    let leap_second = utc.second == 86_400;
    if leap_second && !is_leap_day(utc.day) {
        return Err(ClockError::InvalidUtc);
    }
    let unix = i64::from(utc.day)
        .checked_mul(DAY_SECONDS)
        .and_then(|v| v.checked_add(i64::from(utc.second)))
        .ok_or(ClockError::InvalidUtc)?;
    // On a leap label, offset_at_day still returns the pre-transition offset.
    let offset = offset_at_day(utc.day)?;
    let seconds = unix
        .checked_sub(GPS_EPOCH_UNIX_SECONDS)
        .and_then(|v| v.checked_add(i64::from(offset)))
        .ok_or(ClockError::InvalidUtc)?;
    if seconds < 0 {
        return Err(ClockError::UnknownUtcRange);
    }
    if unix >= VALID_UNIX_END_EXCLUSIVE {
        return Err(ClockError::UnknownUtcRange);
    }
    Ok(GpsInstant {
        seconds,
        nanosecond: utc.nanosecond,
    })
}

pub fn gps_to_utc(gps: GpsInstant) -> Result<UtcInstant, ClockError> {
    if gps.seconds < 0 || gps.nanosecond >= 1_000_000_000 {
        return Err(ClockError::InvalidGps);
    }
    for leap in LEAPS {
        let transition = leap
            .effective_unix
            .checked_sub(GPS_EPOCH_UNIX_SECONDS)
            .and_then(|v| v.checked_add(i64::from(leap.gps_utc_after)))
            .ok_or(ClockError::InvalidGps)?;
        if Some(gps.seconds) == transition.checked_sub(1) {
            let effective_day = leap.effective_unix / DAY_SECONDS;
            let day = u32::try_from(
                effective_day
                    .checked_sub(1)
                    .ok_or(ClockError::UnknownUtcRange)?,
            )
            .map_err(|_| ClockError::UnknownUtcRange)?;
            return Ok(UtcInstant {
                day,
                second: 86_400,
                nanosecond: gps.nanosecond,
            });
        }
    }
    let mut offset = 0_i64;
    for leap in LEAPS {
        let effective_gps = leap
            .effective_unix
            .checked_sub(GPS_EPOCH_UNIX_SECONDS)
            .and_then(|value| value.checked_add(i64::from(leap.gps_utc_after)))
            .ok_or(ClockError::InvalidGps)?;
        if gps.seconds < effective_gps {
            break;
        }
        offset = i64::from(leap.gps_utc_after);
    }
    let unix = GPS_EPOCH_UNIX_SECONDS
        .checked_add(gps.seconds)
        .and_then(|v| v.checked_sub(offset))
        .ok_or(ClockError::InvalidGps)?;
    if unix < GPS_EPOCH_UNIX_SECONDS {
        return Err(ClockError::UnknownUtcRange);
    }
    if unix >= VALID_UNIX_END_EXCLUSIVE {
        return Err(ClockError::UnknownUtcRange);
    }
    let day = u32::try_from(unix / DAY_SECONDS).map_err(|_| ClockError::UnknownUtcRange)?;
    Ok(UtcInstant {
        day,
        second: u32::try_from(unix % DAY_SECONDS).map_err(|_| ClockError::InvalidGps)?,
        nanosecond: gps.nanosecond,
    })
}

/// Parse one complete UBX-NAV-TIMEGPS frame as a caller-supplied GPS reference.
///
/// Profile: u-blox M10 SPG 5.10 Interface Description UBX-21035062 R03 (2023-06-27),
/// §3.15.21, message 0x01/0x20, 16-byte payload; official public source:
/// <https://content.u-blox.com/sites/default/files/u-blox-M10-SPG-5.10_InterfaceDescription_UBX-21035062.pdf>.
/// This parses exactly one frame (24 bytes), not a byte stream. Receiver identity, transport
/// framing, timestamp age, and authenticity remain caller responsibilities.
pub fn parse_ubx_nav_timegps(frame: &[u8]) -> Result<UbxGpsReference, UbxTimeGpsError> {
    if frame.len() != UBX_TIMEGPS_FRAME_BYTES {
        return Err(UbxTimeGpsError::FrameLength);
    }
    if frame.get(0..2) != Some(&[0xb5_u8, 0x62][..]) {
        return Err(UbxTimeGpsError::Sync);
    }
    if frame.get(2) != Some(&0x01_u8) || frame.get(3) != Some(&0x20_u8) {
        return Err(UbxTimeGpsError::MessageType);
    }
    if read_u16(frame, 4)? != UBX_TIMEGPS_PAYLOAD_BYTES_U16 {
        return Err(UbxTimeGpsError::PayloadLength);
    }
    let checksum_data = frame.get(2..22).ok_or(UbxTimeGpsError::FrameLength)?;
    let (ck_a, ck_b) = ubx_checksum(checksum_data);
    if frame.get(22) != Some(&ck_a) || frame.get(23) != Some(&ck_b) {
        return Err(UbxTimeGpsError::Checksum);
    }
    let payload = frame.get(6..22).ok_or(UbxTimeGpsError::FrameLength)?;
    let itow_ms = read_u32(payload, 0)?;
    let ftow_ns = read_i32(payload, 4)?;
    let week = read_i16(payload, 8)?;
    let leap_seconds = read_i8(payload, 10)?;
    let valid = *payload.get(11).ok_or(UbxTimeGpsError::FrameLength)?;
    let accuracy_ns = read_u32(payload, 12)?;
    if valid & 0x07 != 0x07 {
        return Err(UbxTimeGpsError::Validity);
    }
    if week < 0 || itow_ms >= GPS_WEEK_MILLISECONDS || !(-500_000..=500_000).contains(&ftow_ns) {
        return Err(UbxTimeGpsError::GpsFields);
    }
    let total_ns = i128::from(week)
        .checked_mul(i128::from(GPS_WEEK_SECONDS))
        .and_then(|value| value.checked_mul(NS_PER_SECOND))
        .and_then(|value| value.checked_add(i128::from(itow_ms) * 1_000_000))
        .and_then(|value| value.checked_add(i128::from(ftow_ns)))
        .ok_or(UbxTimeGpsError::GpsFields)?;
    let seconds = i64::try_from(i128::div_euclid(total_ns, NS_PER_SECOND))
        .map_err(|_| UbxTimeGpsError::GpsFields)?;
    let nanosecond = u32::try_from(i128::rem_euclid(total_ns, NS_PER_SECOND))
        .map_err(|_| UbxTimeGpsError::GpsFields)?;
    let gps = GpsInstant {
        seconds,
        nanosecond,
    };
    let utc = gps_to_utc(gps).map_err(UbxTimeGpsError::Clock)?;
    let expected_leap_seconds = offset_at_day(utc.day).map_err(UbxTimeGpsError::Clock)?;
    if i32::from(leap_seconds) != expected_leap_seconds {
        return Err(UbxTimeGpsError::LeapMismatch);
    }
    Ok(UbxGpsReference {
        gps,
        accuracy_ns,
        week: u16::try_from(week).map_err(|_| UbxTimeGpsError::GpsFields)?,
        itow_ms,
        ftow_ns,
        leap_seconds,
    })
}

/// UBX-NAV-TIMEGPS reference and source fields preserved from the checked frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// A7.2: cold struct, size guard waived; one decoded receiver message in an offline report input.
pub struct UbxGpsReference {
    pub gps: GpsInstant,
    pub accuracy_ns: u32,
    pub week: u16,
    pub itow_ms: u32,
    pub ftow_ns: i32,
    pub leap_seconds: i8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UbxTimeGpsError {
    FrameLength,
    Sync,
    MessageType,
    PayloadLength,
    Checksum,
    Validity,
    GpsFields,
    LeapMismatch,
    Clock(ClockError),
}

const UBX_TIMEGPS_PAYLOAD_BYTES: usize = 16;
const UBX_TIMEGPS_PAYLOAD_BYTES_U16: u16 = 16;
const UBX_TIMEGPS_FRAME_BYTES: usize = 8 + UBX_TIMEGPS_PAYLOAD_BYTES;
const GPS_WEEK_SECONDS: u32 = 604_800;
const GPS_WEEK_MILLISECONDS: u32 = GPS_WEEK_SECONDS * 1_000;

fn ubx_checksum(bytes: &[u8]) -> (u8, u8) {
    let mut ck_a = 0_u8;
    let mut ck_b = 0_u8;
    for byte in bytes {
        ck_a = ck_a.wrapping_add(*byte);
        ck_b = ck_b.wrapping_add(ck_a);
    }
    (ck_a, ck_b)
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, UbxTimeGpsError> {
    let end = offset.checked_add(2).ok_or(UbxTimeGpsError::FrameLength)?;
    let value = bytes.get(offset..end).ok_or(UbxTimeGpsError::FrameLength)?;
    let array: [u8; 2] = value.try_into().map_err(|_| UbxTimeGpsError::FrameLength)?;
    Ok(u16::from_le_bytes(array))
}

fn read_i16(bytes: &[u8], offset: usize) -> Result<i16, UbxTimeGpsError> {
    let end = offset.checked_add(2).ok_or(UbxTimeGpsError::FrameLength)?;
    let value = bytes.get(offset..end).ok_or(UbxTimeGpsError::FrameLength)?;
    let array: [u8; 2] = value.try_into().map_err(|_| UbxTimeGpsError::FrameLength)?;
    Ok(i16::from_le_bytes(array))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, UbxTimeGpsError> {
    let end = offset.checked_add(4).ok_or(UbxTimeGpsError::FrameLength)?;
    let value = bytes.get(offset..end).ok_or(UbxTimeGpsError::FrameLength)?;
    let array: [u8; 4] = value.try_into().map_err(|_| UbxTimeGpsError::FrameLength)?;
    Ok(u32::from_le_bytes(array))
}

fn read_i32(bytes: &[u8], offset: usize) -> Result<i32, UbxTimeGpsError> {
    let end = offset.checked_add(4).ok_or(UbxTimeGpsError::FrameLength)?;
    let value = bytes.get(offset..end).ok_or(UbxTimeGpsError::FrameLength)?;
    let array: [u8; 4] = value.try_into().map_err(|_| UbxTimeGpsError::FrameLength)?;
    Ok(i32::from_le_bytes(array))
}

fn read_i8(bytes: &[u8], offset: usize) -> Result<i8, UbxTimeGpsError> {
    bytes
        .get(offset)
        .copied()
        .map(|value| i8::from_le_bytes([value]))
        .ok_or(UbxTimeGpsError::FrameLength)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// A7.2: cold struct, size guard waived; input row in a bounded offline daily status snapshot.
pub struct ClockSample {
    pub processor_id: ProcessorId,
    pub processor_utc: UtcInstant,
    pub reference: ReferenceState,
    pub gps_reference: Option<GpsInstant>,
    /// Caller-supplied age of the GPS reference observation in nanoseconds.
    pub reference_age_ns: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferenceState {
    Valid,
    Unavailable,
    Invalid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockRowState {
    Missing,
    Complete,
    Alarm,
    Unavailable,
    Stale,
    Invalid,
    Duplicate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// A7.2: cold struct, size guard waived; one-row-per-processor result in a daily report.
pub struct ClockProcessorResult {
    pub processor_id: ProcessorId,
    pub rows: u16,
    pub offset_ns: i64,
    pub has_offset: bool,
    pub state: ClockRowState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// A7.2: cold struct, size guard waived; fixed-capacity report returned once per daily assessment.
pub struct ClockDailyResult {
    pub expected: u16,
    pub seen_expected: u16,
    pub missing: u16,
    pub unexpected: u16,
    pub invalid: u16,
    pub unavailable: u16,
    pub stale: u16,
    pub alarms: u16,
    pub coverage_complete: bool,
    pub provenance_unverified: bool,
    pub limit_ns: u64,
    pub rows: [ClockProcessorResult; MAX_CLOCK_EXPECTED],
}

const EMPTY_ROW: ClockProcessorResult = ClockProcessorResult {
    processor_id: ProcessorId(0),
    rows: 0,
    offset_ns: 0,
    has_offset: false,
    state: ClockRowState::Missing,
};

/// Compare expected processors against explicit GPS-derived reference samples for a UTC day.
/// `reference_day` anchors the GPS references; processor UTC samples may cross that day boundary.
/// Positive offset means the processor's reported clock is ahead of the reference.
// Bounded daily query over a cold snapshot; all time and tolerance inputs are explicit.
pub fn assess_daily_clock(
    expected: &[ProcessorId],
    samples: &[ClockSample],
    reference_day: u32,
    limit_ns: u64,
    max_reference_age_ns: u64,
) -> Result<ClockDailyResult, ClockError> {
    if expected.is_empty() {
        return Err(ClockError::EmptyExpected);
    }
    if expected.len() > MAX_CLOCK_EXPECTED {
        return Err(ClockError::ExpectedOverCapacity);
    }
    if samples.len() > MAX_CLOCK_SAMPLES {
        return Err(ClockError::SamplesOverCapacity);
    }
    if limit_ns > i64::MAX as u64 || max_reference_age_ns > i64::MAX as u64 {
        return Err(ClockError::InvalidLimit);
    }
    utc_to_gps(UtcInstant {
        day: reference_day,
        second: 0,
        nanosecond: 0,
    })
    .map_err(|_| ClockError::UnknownUtcRange)?;
    // Caps bound both matching scans to 2 * 128 * 512 comparisons.
    for (i, id) in expected.iter().enumerate() {
        if expected.iter().take(i).any(|prior| prior == id) {
            return Err(ClockError::DuplicateExpected);
        }
    }
    let mut result = ClockDailyResult {
        expected: u16::try_from(expected.len()).map_err(|_| ClockError::ExpectedOverCapacity)?,
        seen_expected: 0,
        missing: 0,
        unexpected: 0,
        invalid: 0,
        unavailable: 0,
        stale: 0,
        alarms: 0,
        coverage_complete: false,
        provenance_unverified: true,
        limit_ns,
        rows: [EMPTY_ROW; MAX_CLOCK_EXPECTED],
    };
    for sample in samples {
        let Some(index) = expected.iter().position(|id| *id == sample.processor_id) else {
            result.unexpected = result
                .unexpected
                .checked_add(1)
                .ok_or(ClockError::CounterOverflow)?;
            continue;
        };
        let row = result
            .rows
            .get_mut(index)
            .ok_or(ClockError::CapacityInvariant)?;
        row.rows = row.rows.checked_add(1).ok_or(ClockError::CounterOverflow)?;
    }
    for (i, id) in expected.iter().enumerate() {
        let row = result
            .rows
            .get_mut(i)
            .ok_or(ClockError::CapacityInvariant)?;
        row.processor_id = *id;
        if row.rows == 0 {
            result.missing = result
                .missing
                .checked_add(1)
                .ok_or(ClockError::CounterOverflow)?;
            continue;
        }
        result.seen_expected = result
            .seen_expected
            .checked_add(1)
            .ok_or(ClockError::CounterOverflow)?;
        if row.rows != 1 {
            row.state = ClockRowState::Duplicate;
            result.invalid = result
                .invalid
                .checked_add(1)
                .ok_or(ClockError::CounterOverflow)?;
            continue;
        }
        let sample = samples
            .iter()
            .find(|s| s.processor_id == *id)
            .ok_or(ClockError::CapacityInvariant)?;
        let proc_gps = match utc_to_gps(sample.processor_utc) {
            Ok(value) => value,
            Err(_) => {
                row.state = ClockRowState::Invalid;
                result.invalid = result
                    .invalid
                    .checked_add(1)
                    .ok_or(ClockError::CounterOverflow)?;
                continue;
            }
        };
        match (
            sample.reference,
            sample.gps_reference,
            sample.reference_age_ns,
        ) {
            (ReferenceState::Unavailable, None, None) => {
                row.state = ClockRowState::Unavailable;
                result.unavailable = result
                    .unavailable
                    .checked_add(1)
                    .ok_or(ClockError::CounterOverflow)?;
            }
            (ReferenceState::Valid, Some(gps), Some(age)) => {
                let ref_utc = gps_to_utc(gps);
                let Ok(ref_utc) = ref_utc else {
                    row.state = ClockRowState::Invalid;
                    result.invalid = result
                        .invalid
                        .checked_add(1)
                        .ok_or(ClockError::CounterOverflow)?;
                    continue;
                };
                if ref_utc.day != reference_day {
                    row.state = ClockRowState::Invalid;
                    result.invalid = result
                        .invalid
                        .checked_add(1)
                        .ok_or(ClockError::CounterOverflow)?;
                    continue;
                }
                if age > max_reference_age_ns {
                    row.state = ClockRowState::Stale;
                    result.stale = result
                        .stale
                        .checked_add(1)
                        .ok_or(ClockError::CounterOverflow)?;
                    continue;
                }
                let offset = i128::from(proc_gps.seconds)
                    .checked_sub(i128::from(gps.seconds))
                    .and_then(|seconds| seconds.checked_mul(NS_PER_SECOND))
                    .and_then(|value| value.checked_add(i128::from(proc_gps.nanosecond)))
                    .and_then(|value| value.checked_sub(i128::from(gps.nanosecond)));
                let Some(offset) = offset else {
                    row.state = ClockRowState::Invalid;
                    result.invalid = result
                        .invalid
                        .checked_add(1)
                        .ok_or(ClockError::CounterOverflow)?;
                    continue;
                };
                let Ok(offset) = i64::try_from(offset) else {
                    row.state = ClockRowState::Invalid;
                    result.invalid = result
                        .invalid
                        .checked_add(1)
                        .ok_or(ClockError::CounterOverflow)?;
                    continue;
                };
                row.offset_ns = offset;
                row.has_offset = true;
                if offset.unsigned_abs() > limit_ns {
                    row.state = ClockRowState::Alarm;
                    result.alarms = result
                        .alarms
                        .checked_add(1)
                        .ok_or(ClockError::CounterOverflow)?;
                } else {
                    row.state = ClockRowState::Complete;
                }
            }
            _ => {
                row.state = ClockRowState::Invalid;
                result.invalid = result
                    .invalid
                    .checked_add(1)
                    .ok_or(ClockError::CounterOverflow)?;
            }
        }
    }
    result.coverage_complete = result.missing == 0
        && result.unexpected == 0
        && result.invalid == 0
        && result.unavailable == 0
        && result.stale == 0;
    Ok(result)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::expect_used,
        clippy::unwrap_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;

    fn utc(day: u32, second: u32) -> UtcInstant {
        UtcInstant {
            day,
            second,
            nanosecond: 0,
        }
    }

    fn ubx_timegps_frame(
        week: i16,
        itow_ms: u32,
        ftow_ns: i32,
        leap_seconds: i8,
        valid: u8,
    ) -> Vec<u8> {
        let mut frame = vec![0xb5, 0x62, 0x01, 0x20, 16, 0];
        frame.extend_from_slice(&itow_ms.to_le_bytes());
        frame.extend_from_slice(&ftow_ns.to_le_bytes());
        frame.extend_from_slice(&week.to_le_bytes());
        frame.push(leap_seconds.to_le_bytes()[0]);
        frame.push(valid);
        frame.extend_from_slice(&25_000_u32.to_le_bytes());
        let (ck_a, ck_b) = ubx_checksum(frame.get(2..).expect("UBX header and payload"));
        frame.push(ck_a);
        frame.push(ck_b);
        frame
    }

    fn refresh_ubx_checksum(frame: &mut [u8]) {
        let checksum_data = frame.get(2..22).expect("fixed synthetic UBX frame");
        let (ck_a, ck_b) = ubx_checksum(checksum_data);
        *frame.get_mut(22).expect("checksum A") = ck_a;
        *frame.get_mut(23).expect("checksum B") = ck_b;
    }

    // REQ: OA-TECHOPS-006
    #[test]
    fn iers_2016_leap_second_is_distinct_and_round_trips() {
        let day = (1_483_228_800_i64 / DAY_SECONDS - 1) as u32;
        let a = utc_to_gps(utc(day, 86_399)).expect("valid pre-leap UTC");
        let b = utc_to_gps(utc(day, 86_400)).expect("announced leap second");
        let c = utc_to_gps(utc(day + 1, 0)).expect("valid post-leap UTC");
        assert_eq!(b.seconds, a.seconds + 1);
        assert_eq!(c.seconds, b.seconds + 1);
        assert_eq!(gps_to_utc(b).expect("leap GPS instant"), utc(day, 86_400));
        assert_eq!(
            gps_to_utc(c).expect("post-leap GPS instant"),
            utc(day + 1, 0)
        );
        for leap in LEAPS {
            let effective_day = (leap.effective_unix / DAY_SECONDS) as u32;
            let leap_utc = utc(effective_day - 1, 86_400);
            let gps = utc_to_gps(leap_utc).expect("IERS-listed leap");
            assert_eq!(gps_to_utc(gps), Ok(leap_utc));
        }
    }

    // REQ: OA-TECHOPS-006
    #[test]
    fn daily_report_distinguishes_alarm_unavailable_invalid_missing_and_duplicate() {
        let day = (1_483_228_800_i64 / DAY_SECONDS) as u32;
        let base = utc_to_gps(utc(day, 10)).expect("valid UTC reference");
        let ids = [
            ProcessorId(1),
            ProcessorId(2),
            ProcessorId(3),
            ProcessorId(4),
            ProcessorId(5),
            ProcessorId(6),
        ];
        let samples = [
            ClockSample {
                processor_id: ids[0],
                processor_utc: utc(day, 10),
                reference: ReferenceState::Valid,
                gps_reference: Some(base),
                reference_age_ns: Some(0),
            },
            ClockSample {
                processor_id: ids[5],
                processor_utc: utc(day, 86_400),
                reference: ReferenceState::Unavailable,
                gps_reference: None,
                reference_age_ns: None,
            },
            ClockSample {
                processor_id: ids[1],
                processor_utc: utc(day, 12),
                reference: ReferenceState::Valid,
                gps_reference: Some(base),
                reference_age_ns: Some(0),
            },
            ClockSample {
                processor_id: ids[2],
                processor_utc: utc(day, 10),
                reference: ReferenceState::Unavailable,
                gps_reference: None,
                reference_age_ns: None,
            },
            ClockSample {
                processor_id: ids[3],
                processor_utc: utc(day, 10),
                reference: ReferenceState::Invalid,
                gps_reference: Some(base),
                reference_age_ns: Some(0),
            },
            ClockSample {
                processor_id: ids[0],
                processor_utc: utc(day, 10),
                reference: ReferenceState::Valid,
                gps_reference: Some(base),
                reference_age_ns: Some(0),
            },
        ];
        let report = assess_daily_clock(&ids, &samples, day, 1_000_000_000, 1_000)
            .expect("bounded daily clock report");
        assert_eq!(report.rows[0].state, ClockRowState::Duplicate);
        assert_eq!(report.rows[1].state, ClockRowState::Alarm);
        assert_eq!(report.rows[2].state, ClockRowState::Unavailable);
        assert_eq!(report.rows[3].state, ClockRowState::Invalid);
        assert_eq!(report.rows[4].state, ClockRowState::Missing);
        assert_eq!(report.rows[5].state, ClockRowState::Invalid);
        assert_eq!(report.invalid, 3);
        assert_eq!(report.alarms, 1);
        assert_eq!(report.missing, 1);
        assert!(!report.coverage_complete);
    }

    // REQ: OA-TECHOPS-006
    #[test]
    fn caller_limit_is_inclusive_and_reference_day_is_checked() {
        let day = (1_483_228_800_i64 / DAY_SECONDS) as u32;
        let reference = utc_to_gps(UtcInstant {
            day,
            second: 10,
            nanosecond: 500,
        })
        .expect("valid UTC reference");
        let processor = UtcInstant {
            day,
            second: 10,
            nanosecond: 1_000,
        };
        let sample = ClockSample {
            processor_id: ProcessorId(9),
            processor_utc: processor,
            reference: ReferenceState::Valid,
            gps_reference: Some(reference),
            reference_age_ns: Some(0),
        };
        let result = assess_daily_clock(&[ProcessorId(9)], &[sample], day, 500, 10)
            .expect("valid comparison input");
        assert_eq!(result.rows[0].offset_ns, 500);
        assert_eq!(result.rows[0].state, ClockRowState::Complete);
        assert_eq!(
            assess_daily_clock(&[ProcessorId(9)], &[sample], day + 1, 500, 10)
                .expect("valid comparison input")
                .invalid,
            1
        );
        let mut stale = sample;
        stale.reference_age_ns = Some(11);
        let report = assess_daily_clock(&[ProcessorId(9)], &[stale], day, 500, 10)
            .expect("stale reference is reported");
        assert_eq!(report.rows[0].state, ClockRowState::Stale);
        assert_eq!(report.stale, 1);
        assert!(!report.coverage_complete);
    }

    // REQ: OA-TECHOPS-006
    #[test]
    fn daily_comparison_keeps_midnight_offsets_and_alarms_on_gross_day_error() {
        let day = (1_483_315_200_i64 / DAY_SECONDS) as u32;
        let reference = utc_to_gps(utc(day, 0)).expect("valid UTC reference");
        let samples = [
            ClockSample {
                processor_id: ProcessorId(1),
                processor_utc: UtcInstant {
                    day: day - 1,
                    second: 86_399,
                    nanosecond: 999_000_000,
                },
                reference: ReferenceState::Valid,
                gps_reference: Some(reference),
                reference_age_ns: Some(0),
            },
            ClockSample {
                processor_id: ProcessorId(2),
                processor_utc: UtcInstant {
                    day,
                    second: 0,
                    nanosecond: 1_000_000,
                },
                reference: ReferenceState::Valid,
                gps_reference: Some(reference),
                reference_age_ns: Some(0),
            },
            ClockSample {
                processor_id: ProcessorId(3),
                processor_utc: utc(day - 1, 0),
                reference: ReferenceState::Valid,
                gps_reference: Some(reference),
                reference_age_ns: Some(0),
            },
        ];
        let report = assess_daily_clock(
            &[ProcessorId(1), ProcessorId(2), ProcessorId(3)],
            &samples,
            day,
            1_000_000,
            0,
        )
        .expect("bounded daily assessment");
        assert_eq!(report.rows[0].offset_ns, -1_000_000);
        assert_eq!(report.rows[0].state, ClockRowState::Complete);
        assert_eq!(report.rows[1].offset_ns, 1_000_000);
        assert_eq!(report.rows[1].state, ClockRowState::Complete);
        assert_eq!(report.rows[2].offset_ns, -86_400_000_000_000);
        assert_eq!(report.rows[2].state, ClockRowState::Alarm);
        assert_eq!(report.alarms, 1);
        assert!(report.coverage_complete);
    }

    // REQ: OA-TECHOPS-006
    #[test]
    fn invalid_leap_labels_and_bounds_refuse() {
        assert_eq!(utc_to_gps(utc(20_000, 86_400)), Err(ClockError::InvalidUtc));
        assert_eq!(
            utc_to_gps(utc((VALID_UNIX_END_EXCLUSIVE / DAY_SECONDS) as u32, 0)),
            Err(ClockError::UnknownUtcRange)
        );
        assert_eq!(
            assess_daily_clock(&[], &[], 0, 1, 1),
            Err(ClockError::EmptyExpected)
        );
        assert_eq!(
            assess_daily_clock(&[ProcessorId(1)], &[], 0, u64::MAX, 1),
            Err(ClockError::InvalidLimit)
        );
        let bad_gps = GpsInstant {
            seconds: 0,
            nanosecond: 1_000_000_000,
        };
        assert_eq!(gps_to_utc(bad_gps), Err(ClockError::InvalidGps));
    }

    // REQ: OA-TECHOPS-006
    #[test]
    fn ubx_timegps_receiver_frame_feeds_leap_aware_clock_assessment() {
        let day = (1_483_228_800_i64 / DAY_SECONDS - 1) as u32;
        let leap_utc = utc(day, 86_400);
        let gps = utc_to_gps(leap_utc).expect("IERS-listed leap instant");
        let week =
            i16::try_from(gps.seconds / i64::from(GPS_WEEK_SECONDS)).expect("synthetic GPS week");
        let itow_ms = u32::try_from(gps.seconds % i64::from(GPS_WEEK_SECONDS))
            .expect("synthetic GPS time of week")
            * 1_000;
        let frame = ubx_timegps_frame(week, itow_ms, 0, 17, 0x07);
        let reference = parse_ubx_nav_timegps(&frame).expect("valid synthetic UBX TIMEGPS frame");
        let reserved_bits_frame = ubx_timegps_frame(week, itow_ms, 0, 17, 0xff);
        assert_eq!(parse_ubx_nav_timegps(&reserved_bits_frame), Ok(reference));
        assert_eq!(reference.gps, gps);
        assert_eq!(reference.accuracy_ns, 25_000);
        assert_eq!(reference.leap_seconds, 17);
        let sample = ClockSample {
            processor_id: ProcessorId(70),
            processor_utc: leap_utc,
            reference: ReferenceState::Valid,
            gps_reference: Some(reference.gps),
            reference_age_ns: Some(0),
        };
        let report = assess_daily_clock(&[ProcessorId(70)], &[sample], day, 0, 0)
            .expect("leap-aware report from parsed GPS reference");
        assert_eq!(report.rows[0].state, ClockRowState::Complete);
        assert_eq!(report.rows[0].offset_ns, 0);
    }

    // REQ: OA-TECHOPS-006
    #[test]
    fn ubx_timegps_refuses_bad_framing_checksum_flags_week_and_leap_value() {
        let valid = ubx_timegps_frame(3_000, 0, 0, 18, 0x07);
        assert_eq!(
            parse_ubx_nav_timegps(&valid),
            Err(UbxTimeGpsError::Clock(ClockError::UnknownUtcRange))
        );
        assert_eq!(
            parse_ubx_nav_timegps(&valid[..valid.len() - 1]),
            Err(UbxTimeGpsError::FrameLength)
        );
        let mut bad_sync = valid.clone();
        *bad_sync.get_mut(0).expect("sync byte") = 0;
        assert_eq!(parse_ubx_nav_timegps(&bad_sync), Err(UbxTimeGpsError::Sync));
        let mut bad_class = valid.clone();
        *bad_class.get_mut(2).expect("class byte") = 0x02;
        refresh_ubx_checksum(&mut bad_class);
        assert_eq!(
            parse_ubx_nav_timegps(&bad_class),
            Err(UbxTimeGpsError::MessageType)
        );
        let mut bad_length = valid.clone();
        *bad_length.get_mut(4).expect("length byte") = 15;
        refresh_ubx_checksum(&mut bad_length);
        assert_eq!(
            parse_ubx_nav_timegps(&bad_length),
            Err(UbxTimeGpsError::PayloadLength)
        );
        let mut corrupt_checksum = valid.clone();
        let checksum = corrupt_checksum.get_mut(22).expect("checksum byte");
        *checksum ^= 1;
        assert_eq!(
            parse_ubx_nav_timegps(&corrupt_checksum),
            Err(UbxTimeGpsError::Checksum)
        );

        let day = (1_483_228_800_i64 / DAY_SECONDS - 1) as u32;
        let gps = utc_to_gps(utc(day, 86_400)).expect("IERS-listed leap instant");
        let week = i16::try_from(gps.seconds / i64::from(GPS_WEEK_SECONDS)).expect("week");
        let itow_ms =
            u32::try_from(gps.seconds % i64::from(GPS_WEEK_SECONDS)).expect("tow") * 1_000;
        assert_eq!(
            parse_ubx_nav_timegps(&ubx_timegps_frame(week, itow_ms, 0, 18, 0x07)),
            Err(UbxTimeGpsError::LeapMismatch)
        );
        assert_eq!(
            parse_ubx_nav_timegps(&ubx_timegps_frame(week, itow_ms, 0, 17, 0x03)),
            Err(UbxTimeGpsError::Validity)
        );
        assert_eq!(
            parse_ubx_nav_timegps(&ubx_timegps_frame(week, itow_ms, 500_001, 17, 0x07)),
            Err(UbxTimeGpsError::GpsFields)
        );
        assert_eq!(
            parse_ubx_nav_timegps(&ubx_timegps_frame(-1, 0, 0, 0, 0x07)),
            Err(UbxTimeGpsError::GpsFields)
        );
        assert_eq!(
            parse_ubx_nav_timegps(&ubx_timegps_frame(week, GPS_WEEK_MILLISECONDS, 0, 17, 0x07)),
            Err(UbxTimeGpsError::GpsFields)
        );
    }
}
