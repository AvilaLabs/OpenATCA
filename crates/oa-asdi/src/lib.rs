#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
//! `oa-asdi` is the pure ASDI XML feed packet-header profile (ASDI XML ICD v1.8, 2011-04-15).
//! This initial slice reads and validates only the fixed 32-byte packet header and sequence
//! observations. It does not open sockets, register a vendor, handle credentials, decompress
//! zlib, parse XML, validate ASDI XSDs, or interpret flight messages. An XML packet is returned
//! with an explicit `XmlCompressedNotDecoded` status, never as validated ASDI content.
//!
//! Header structure, timestamp and sequence rules follow ASDI XML ICD v1.8 sections 5.2–5.3:
//! 16 timestamp bytes followed by four network-order `u32` values; type 1 is heartbeat and type 2
//! is XML; sequence 100000 wraps to 1. The zero-size type-1 heartbeat rule is from the public
//! TFMS-to-ASDI ICD (CSC/TFMM-08/0473, March 2010, section 3.2.1.3.2.2.10). The caller supplies
//! both payload size caps, so this parser never allocates or chooses a guessed feed capacity.

#[cfg(not(target_pointer_width = "64"))]
compile_error!("OpenATCA core requires a 64-bit target");

/// ASDI header length from ICD v1.8 section 5.3.
pub const HEADER_LEN: usize = 32;
/// Heartbeat data type from ICD v1.8 section 5.3.
pub const DATA_TYPE_HEARTBEAT: u32 = 1;
/// XML payload data type from ICD v1.8 section 5.3.
pub const DATA_TYPE_XML: u32 = 2;
/// Inclusive maximum sequence number from ICD v1.8 section 5.3.
pub const MAX_SEQUENCE_NUMBER: u32 = 100_000;
/// Provisional hard compressed-payload ceiling, matching the current XML parser input default.
/// This is an implementation limit, not a size promised by the ASDI ICD.
pub const MAX_COMPRESSED_PAYLOAD_BYTES: u32 = 1 << 20;
/// Provisional hard decompressed-payload ceiling, matching the current XML parser input default.
/// This is an implementation limit, not a size promised by the ASDI ICD.
pub const MAX_DECOMPRESSED_PAYLOAD_BYTES: u32 = 1 << 20;
/// Sentinel used when the first sequence observation has no expected predecessor.
/// It is outside the valid ASDI sequence-number range.
pub const NO_EXPECTED_SEQUENCE: u32 = u32::MAX;

/// Fourteen-digit timestamp components as transmitted in the header.
///
/// This is a calendar timestamp without a timezone offset. The ASDI ICD defines its digit format
/// but does not define conversion to UTC or an epoch, so this type exposes components only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct AsdiTimestamp {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    /// Explicit layout byte; parser output always sets this to zero.
    pub reserved: u8,
}
// Budget: year (2) + five byte components + explicit reserved byte, one timestamp per packet.
const _: () = assert!(core::mem::size_of::<AsdiTimestamp>() == 8);

/// Packet data type supported by the v1.8 header profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum PacketType {
    Heartbeat = DATA_TYPE_HEARTBEAT,
    Xml = DATA_TYPE_XML,
}

/// Validated fields from the ASDI v1.8 fixed packet header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct AsdiHeader {
    pub timestamp: AsdiTimestamp,
    pub packet_type: PacketType,
    pub sequence_number: u32,
    pub compressed_size: u32,
    pub decompressed_size: u32,
}
// Budget: one packet header value: 8-byte timestamp and four network-order u32 values.
const _: () = assert!(core::mem::size_of::<AsdiHeader>() == 24);

/// Caller-supplied payload bounds. These values are limits only; this crate allocates no buffers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct PayloadLimits {
    pub max_compressed_bytes: u32,
    pub max_decompressed_bytes: u32,
}
// Budget: two explicit u32 caller caps; configuration is copied into inspection calls, not rows.
const _: () = assert!(core::mem::size_of::<PayloadLimits>() == 8);

/// Header or packet-profile refusal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AsdiError {
    HeaderLength { actual: usize },
    TimestampLength { actual: usize },
    TimestampCharacter { offset: u8 },
    TimestampPadding { offset: u8 },
    InvalidCalendarDate,
    InvalidTimeOfDay,
    UnknownPacketType { value: u32 },
    SequenceOutOfRange { value: u32 },
    HeartbeatHasPayload,
    CompressedPayloadTooLarge { value: u32, maximum: u32 },
    DecompressedPayloadTooLarge { value: u32, maximum: u32 },
    CompressedLimitAboveHardCeiling { value: u32, maximum: u32 },
    DecompressedLimitAboveHardCeiling { value: u32, maximum: u32 },
    PacketLengthOverflow,
    PacketLengthMismatch { expected: u64, actual: u64 },
}

/// What packet-level checks established. XML compression and schema checks remain unperformed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub enum PayloadStatus {
    HeartbeatNoPayload,
    XmlCompressedNotDecoded {
        compressed_bytes: u32,
        expected_decompressed_bytes: u32,
    },
}
// Budget: C-repr discriminant plus two u32 XML size values.
const _: () = assert!(core::mem::size_of::<PayloadStatus>() == 12);

/// Header and typed payload-validation boundary for one complete byte packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct PacketInspection {
    pub header: AsdiHeader,
    pub payload_status: PayloadStatus,
}
// Budget: parsed packet header (24) plus payload status (12).
const _: () = assert!(core::mem::size_of::<PacketInspection>() == 36);

/// Parse one exact 32-byte header according to ASDI XML ICD v1.8 section 5.3.
pub fn parse_header(bytes: &[u8]) -> Result<AsdiHeader, AsdiError> {
    if bytes.len() != HEADER_LEN {
        return Err(AsdiError::HeaderLength {
            actual: bytes.len(),
        });
    }
    let timestamp_bytes = bytes.get(0..16).ok_or(AsdiError::HeaderLength {
        actual: bytes.len(),
    })?;
    let timestamp = parse_timestamp(timestamp_bytes)?;
    let packet_type_value = read_be_u32(bytes, 16)?;
    let packet_type = match packet_type_value {
        DATA_TYPE_HEARTBEAT => PacketType::Heartbeat,
        DATA_TYPE_XML => PacketType::Xml,
        value => return Err(AsdiError::UnknownPacketType { value }),
    };
    let sequence_number = read_be_u32(bytes, 20)?;
    if sequence_number > MAX_SEQUENCE_NUMBER {
        return Err(AsdiError::SequenceOutOfRange {
            value: sequence_number,
        });
    }
    Ok(AsdiHeader {
        timestamp,
        packet_type,
        sequence_number,
        compressed_size: read_be_u32(bytes, 24)?,
        decompressed_size: read_be_u32(bytes, 28)?,
    })
}

/// Inspect one complete packet against explicit caller bounds.
///
/// For type 1, the 2010 TFMS-to-ASDI ICD defines both size fields as zero and the packet is
/// header-only. For type 2,
/// this checks the declared compressed and decompressed sizes against the caller limits and
/// requires the byte slice to contain exactly the declared compressed payload. It does not check
/// the zlib stream or the decompressed XML/XSD content.
pub fn inspect_packet(bytes: &[u8], limits: PayloadLimits) -> Result<PacketInspection, AsdiError> {
    if limits.max_compressed_bytes > MAX_COMPRESSED_PAYLOAD_BYTES {
        return Err(AsdiError::CompressedLimitAboveHardCeiling {
            value: limits.max_compressed_bytes,
            maximum: MAX_COMPRESSED_PAYLOAD_BYTES,
        });
    }
    if limits.max_decompressed_bytes > MAX_DECOMPRESSED_PAYLOAD_BYTES {
        return Err(AsdiError::DecompressedLimitAboveHardCeiling {
            value: limits.max_decompressed_bytes,
            maximum: MAX_DECOMPRESSED_PAYLOAD_BYTES,
        });
    }
    let header_bytes = bytes.get(0..HEADER_LEN).ok_or(AsdiError::HeaderLength {
        actual: bytes.len(),
    })?;
    let header = parse_header(header_bytes)?;
    let payload_status = match header.packet_type {
        PacketType::Heartbeat => {
            if header.compressed_size != 0
                || header.decompressed_size != 0
                || bytes.len() != HEADER_LEN
            {
                return Err(AsdiError::HeartbeatHasPayload);
            }
            PayloadStatus::HeartbeatNoPayload
        }
        PacketType::Xml => {
            if header.compressed_size > limits.max_compressed_bytes {
                return Err(AsdiError::CompressedPayloadTooLarge {
                    value: header.compressed_size,
                    maximum: limits.max_compressed_bytes,
                });
            }
            if header.decompressed_size > limits.max_decompressed_bytes {
                return Err(AsdiError::DecompressedPayloadTooLarge {
                    value: header.decompressed_size,
                    maximum: limits.max_decompressed_bytes,
                });
            }
            let header_len =
                u64::try_from(HEADER_LEN).map_err(|_| AsdiError::PacketLengthOverflow)?;
            let expected = header_len
                .checked_add(u64::from(header.compressed_size))
                .ok_or(AsdiError::PacketLengthOverflow)?;
            let actual = u64::try_from(bytes.len()).map_err(|_| AsdiError::PacketLengthOverflow)?;
            if expected != actual {
                return Err(AsdiError::PacketLengthMismatch { expected, actual });
            }
            PayloadStatus::XmlCompressedNotDecoded {
                compressed_bytes: header.compressed_size,
                expected_decompressed_bytes: header.decompressed_size,
            }
        }
    };
    Ok(PacketInspection {
        header,
        payload_status,
    })
}

/// Sequence comparison from the ICD's sequence range and wrap rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum SequenceStatus {
    FirstObserved,
    InOrder,
    /// The current value differs from the next value expected by the documented sequence rule.
    /// It reports a discontinuity; it does not diagnose reordering, loss, or its cause.
    Gap,
    /// Sequence zero was observed after a prior value. The ICD associates zero with initial
    /// startup, but this core reports only the observed value and does not assert restart cause.
    ZeroObserved,
}

/// Result of comparing one observed number with a prior number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct SequenceObservation {
    pub status: SequenceStatus,
    /// Explicit alignment bytes; parser output always sets these to zero.
    pub reserved: [u8; 3],
    /// `NO_EXPECTED_SEQUENCE` for the first observation; otherwise the expected next value.
    pub expected_sequence: u32,
    pub observed: u32,
}
// Budget: status byte + three explicit alignment bytes + expected and observed u32 values.
const _: () = assert!(core::mem::size_of::<SequenceObservation>() == 12);

/// Compare two ASDI sequence values. The ICD's ordinary wrap is `100000 -> 1`; zero is not
/// treated as a recurring wrap value. A mismatch is a visible gap/discontinuity, not a diagnosis
/// of packet loss or server restart.
pub fn observe_sequence(
    previous: Option<u32>,
    observed: u32,
) -> Result<SequenceObservation, AsdiError> {
    if observed > MAX_SEQUENCE_NUMBER {
        return Err(AsdiError::SequenceOutOfRange { value: observed });
    }
    let Some(previous) = previous else {
        return Ok(SequenceObservation {
            status: SequenceStatus::FirstObserved,
            expected_sequence: NO_EXPECTED_SEQUENCE,
            observed,
            reserved: [0; 3],
        });
    };
    if previous > MAX_SEQUENCE_NUMBER {
        return Err(AsdiError::SequenceOutOfRange { value: previous });
    }
    let expected = if previous == MAX_SEQUENCE_NUMBER {
        1
    } else {
        previous + 1
    };
    let status = if observed == 0 {
        SequenceStatus::ZeroObserved
    } else if observed == expected {
        SequenceStatus::InOrder
    } else {
        SequenceStatus::Gap
    };
    Ok(SequenceObservation {
        status,
        expected_sequence: expected,
        observed,
        reserved: [0; 3],
    })
}

fn parse_timestamp(bytes: &[u8]) -> Result<AsdiTimestamp, AsdiError> {
    if bytes.len() != 16 {
        return Err(AsdiError::TimestampLength {
            actual: bytes.len(),
        });
    }
    for offset in 0..14 {
        let byte = bytes
            .get(usize::from(offset))
            .copied()
            .ok_or(AsdiError::TimestampLength {
                actual: bytes.len(),
            })?;
        if !byte.is_ascii_digit() {
            return Err(AsdiError::TimestampCharacter { offset });
        }
    }
    if bytes.get(14).copied() != Some(0) {
        return Err(AsdiError::TimestampPadding { offset: 14 });
    }
    if bytes.get(15).copied() != Some(b' ') {
        return Err(AsdiError::TimestampPadding { offset: 15 });
    }
    let year = digits4(bytes, 0)?;
    let month = digits2(bytes, 4)?;
    let day = digits2(bytes, 6)?;
    let hour = digits2(bytes, 8)?;
    let minute = digits2(bytes, 10)?;
    let second = digits2(bytes, 12)?;
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => return Err(AsdiError::InvalidCalendarDate),
    };
    if year == 0 || day == 0 || day > days_in_month {
        return Err(AsdiError::InvalidCalendarDate);
    }
    if hour > 23 || minute > 59 || second > 59 {
        return Err(AsdiError::InvalidTimeOfDay);
    }
    Ok(AsdiTimestamp {
        year,
        month,
        day,
        hour,
        minute,
        second,
        reserved: 0,
    })
}

fn digits2(bytes: &[u8], start: usize) -> Result<u8, AsdiError> {
    let next = start
        .checked_add(1)
        .ok_or(AsdiError::PacketLengthOverflow)?;
    let first_offset = u8::try_from(start).map_err(|_| AsdiError::PacketLengthOverflow)?;
    let second_offset = u8::try_from(next).map_err(|_| AsdiError::PacketLengthOverflow)?;
    let first = bytes
        .get(start)
        .copied()
        .ok_or(AsdiError::TimestampCharacter {
            offset: first_offset,
        })?;
    let second = bytes
        .get(next)
        .copied()
        .ok_or(AsdiError::TimestampCharacter {
            offset: second_offset,
        })?;
    let a = first
        .checked_sub(b'0')
        .ok_or(AsdiError::TimestampCharacter {
            offset: first_offset,
        })?;
    let b = second
        .checked_sub(b'0')
        .ok_or(AsdiError::TimestampCharacter {
            offset: second_offset,
        })?;
    if a > 9 || b > 9 {
        return Err(AsdiError::TimestampCharacter {
            offset: first_offset,
        });
    }
    a.checked_mul(10)
        .and_then(|n| n.checked_add(b))
        .ok_or(AsdiError::InvalidCalendarDate)
}

fn digits4(bytes: &[u8], start: usize) -> Result<u16, AsdiError> {
    let hi = u16::from(digits2(bytes, start)?);
    let second_pair = start
        .checked_add(2)
        .ok_or(AsdiError::PacketLengthOverflow)?;
    let lo = u16::from(digits2(bytes, second_pair)?);
    hi.checked_mul(100)
        .and_then(|n| n.checked_add(lo))
        .ok_or(AsdiError::InvalidCalendarDate)
}

fn is_leap_year(year: u16) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn read_be_u32(bytes: &[u8], start: usize) -> Result<u32, AsdiError> {
    let end = start
        .checked_add(4)
        .ok_or(AsdiError::PacketLengthOverflow)?;
    let word = bytes.get(start..end).ok_or(AsdiError::HeaderLength {
        actual: bytes.len(),
    })?;
    let first = word.first().copied().ok_or(AsdiError::HeaderLength {
        actual: bytes.len(),
    })?;
    let second = word.get(1).copied().ok_or(AsdiError::HeaderLength {
        actual: bytes.len(),
    })?;
    let third = word.get(2).copied().ok_or(AsdiError::HeaderLength {
        actual: bytes.len(),
    })?;
    let fourth = word.get(3).copied().ok_or(AsdiError::HeaderLength {
        actual: bytes.len(),
    })?;
    Ok(u32::from_be_bytes([first, second, third, fourth]))
}
