#![forbid(unsafe_code)]
#![cfg_attr(not(test), no_std)]

//! Pure decoder for one caller-supplied 112-bit Mode S extended-squitter frame.
//!
//! This first slice accepts exactly 14 octets, decodes DF17 and DF18 framing, checks the
//! 24-bit extended-squitter parity, preserves the complete frame, and decodes the directly
//! announced address field, DF17 / DF18 CF=0 or CF=6 aircraft-identification message (TC 1–4),
//! and the supported 25-foot altitude code in airborne position messages (TC 9–18).
//! It does not demodulate RF pulses, infer addresses from parity, decode 100-foot altitude codes,
//! pair CPR frames, reconstruct coordinates, or claim DO-260B conformance. DF18 address use is
//! reported from its control field; only CF=0 is exposed as an ICAO-24 address. CF=6 uses the DF17
//! type-code/message formats, so TC 1–4 identification is decoded while its AA field remains
//! unclassified. All other message bits remain available in `raw`.
//!
//! Core crate (RULESET B1, B2, B7). No I/O, clock, allocation, or external dependencies.

// The core assumes a fixed pointer width (RULESET B10).
#[cfg(not(target_pointer_width = "64"))]
compile_error!("oa-modes supports 64-bit targets only");

/// A 112-bit long Mode S downlink frame in octet form.
pub const FRAME_LEN: usize = 14;
const FRAME_BITS: usize = FRAME_LEN * 8;
const CRC_WIDTH: u32 = 24;
const CRC_MASK: u32 = 0x00ff_ffff;
/// Lower 24 bits of ICAO Annex 10 Vol IV polynomial (the x^24 term is implicit).
const CRC_POLYNOMIAL: u32 = 0x00ff_f409;

/// How the directly transmitted 24-bit AA field is identified by the decoded format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum AddressUse {
    /// DF17, or DF18 with CF=0: ICAO 24-bit address field.
    Icao24 = 0,
    /// DF18 with CF=1: the format reserves this for non-ICAO address techniques.
    NonIcao24 = 1,
    /// DF18 TIS-B format family, including CF=4 management messages; the field is not promoted as
    /// an aircraft identity here.
    TisB = 2,
    /// Reserved or not classified by this decoder. DF18 CF=6 has DF17 message formats, but its AA
    /// field is not promoted as an ICAO-24 identity here.
    Unclassified = 3,
}

/// Interpretation of the ME bits performed by this decoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum BodyKind {
    /// Message body belongs to a DF18 control-field format not interpreted by this slice.
    Uninterpreted = 0,
    /// An extended-squitter body other than the supported aircraft-identification type codes.
    OtherExtendedSquitter = 1,
    /// TC 1–4 with a supported eight-character identification.
    AircraftIdentification = 2,
    /// TC 1–4 with all eight character codes zero (no identification supplied).
    AircraftIdentificationAbsent = 3,
}

/// Interpretation status of the airborne-position altitude code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum AltitudeStatus {
    /// This decoder does not interpret an altitude field for this message.
    NotDecoded = 0,
    /// The airborne-position altitude subfield is all zero, meaning altitude unavailable/invalid.
    Unavailable = 1,
    /// A numeric altitude was decoded from the 25-foot Q=1 encoding; its datum/source is unknown.
    Encoded25Ft = 2,
    /// Q=0 selects the 100-foot Mode C/Gillham encoding, retained raw but not decoded here.
    OtherEncoding = 3,
}

/// Classification of a raw CPR position payload. Coordinates are not reconstructed here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CprKind {
    /// The message is not one of the supported surface/airborne position type codes.
    NotPosition = 0,
    /// Surface position payload (TC 5–8).
    Surface = 1,
    /// Airborne position payload (TC 9–18).
    Airborne = 2,
}

/// Raw compact-position-reporting fields from one message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct CprFields {
    pub kind: CprKind,
    /// CPR frame flag: 0 is even, 1 is odd; zero when `kind` is `NotPosition`.
    pub format: u8,
    /// Encoded latitude field, in the range 0–131071; not a latitude coordinate.
    pub latitude_raw: u32,
    /// Encoded longitude field, in the range 0–131071; not a longitude coordinate.
    pub longitude_raw: u32,
}
const _: () = assert!(core::mem::size_of::<CprFields>() == 12);

/// Decoded 112-bit extended-squitter frame. `announced_address` is the AA field as transmitted;
/// it is not a registry lookup or a claim that an address is assigned. `identification` contains
/// eight ASCII bytes only when `body_kind` is `AircraftIdentification`; for all other kinds it
/// is zero-filled. `raw` always retains the original frame, including uninterpreted ME bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct ExtendedSquitter {
    pub raw: [u8; FRAME_LEN],
    pub downlink_format: u8,
    /// CA for DF17, CF for DF18.
    pub control: u8,
    pub announced_address: u32,
    pub address_use: AddressUse,
    /// First five bits of ME. For DF18 this is interpreted for CF=0 and CF=6.
    pub type_code: u8,
    /// TC 1–4 category bits; zero when the body is not decoded as identification.
    pub aircraft_category: u8,
    pub body_kind: BodyKind,
    /// Eight ASCII bytes when `body_kind` is `AircraftIdentification`; otherwise all zero.
    pub identification: [u8; 8],
    /// Decoded 25-foot code value in feet; its datum/source is not identified by this frame alone.
    /// Zero when `altitude_status` is not `Encoded25Ft`.
    pub altitude_ft: i32,
    pub altitude_status: AltitudeStatus,
    pub cpr: CprFields,
}
const _: () = assert!(core::mem::size_of::<ExtendedSquitter>() == 52);

/// Why a frame was refused. Reserved or not-yet-decoded message bodies are retained, not refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// Input must contain one complete 112-bit frame and no trailing octets.
    Length { actual: usize },
    /// The five-bit downlink format is not DF17 or DF18.
    UnsupportedDownlinkFormat { format: u8 },
    /// The full frame's parity-check remainder is not zero.
    Parity { syndrome: u32 },
    /// An identification message contains a six-bit code outside the supported IA-5 subset.
    InvalidIdentificationCharacter { slot: u8, code: u8 },
    /// An internal field range is invalid; constant call sites should make this unreachable.
    FieldRange,
}

/// Decode exactly one long extended-squitter frame; no pulse-level demodulation is performed.
pub fn decode_extended_squitter(input: &[u8]) -> Result<ExtendedSquitter, DecodeError> {
    if input.len() != FRAME_LEN {
        return Err(DecodeError::Length {
            actual: input.len(),
        });
    }
    let mut raw = [0u8; FRAME_LEN];
    raw.copy_from_slice(input);

    let format = u8::try_from(bits(&raw, 0, 5)?).map_err(|_| DecodeError::FieldRange)?;
    if format != 17 && format != 18 {
        return Err(DecodeError::UnsupportedDownlinkFormat { format });
    }
    let syndrome = parity_syndrome(&raw);
    if syndrome != 0 {
        return Err(DecodeError::Parity { syndrome });
    }

    let control = u8::try_from(bits(&raw, 5, 3)?).map_err(|_| DecodeError::FieldRange)?;
    let announced_address = bits(&raw, 8, 24)?;
    let address_use = match format {
        17 => AddressUse::Icao24,
        18 => match control {
            0 => AddressUse::Icao24,
            1 => AddressUse::NonIcao24,
            2..=5 => AddressUse::TisB,
            _ => AddressUse::Unclassified,
        },
        _ => return Err(DecodeError::UnsupportedDownlinkFormat { format }),
    };

    let type_code = u8::try_from(bits(&raw, 32, 5)?).map_err(|_| DecodeError::FieldRange)?;
    // ICAO Annex 10 Vol IV §3.1.2.8.7.2 assigns CF=6 the same type codes and message
    // formats as DF17. CF=4 is TIS-B management, but its body is not decoded here.
    let decode_adsb_body = format == 17 || (format == 18 && (control == 0 || control == 6));
    let mut aircraft_category = 0;
    let mut body_kind = if decode_adsb_body {
        BodyKind::OtherExtendedSquitter
    } else {
        BodyKind::Uninterpreted
    };
    let mut identification = [0u8; 8];
    let mut altitude_ft = 0;
    let mut altitude_status = AltitudeStatus::NotDecoded;
    let mut cpr = CprFields {
        kind: CprKind::NotPosition,
        format: 0,
        latitude_raw: 0,
        longitude_raw: 0,
    };
    if decode_adsb_body && (1..=4).contains(&type_code) {
        aircraft_category =
            u8::try_from(bits(&raw, 37, 3)?).map_err(|_| DecodeError::FieldRange)?;
        let mut codes = [0u8; 8];
        for (slot, code) in codes.iter_mut().enumerate() {
            *code =
                u8::try_from(bits(&raw, 40 + slot * 6, 6)?).map_err(|_| DecodeError::FieldRange)?;
        }
        if codes == [0; 8] {
            body_kind = BodyKind::AircraftIdentificationAbsent;
        } else {
            for (slot, (&code, output)) in codes.iter().zip(identification.iter_mut()).enumerate() {
                let slot = u8::try_from(slot).map_err(|_| DecodeError::FieldRange)?;
                *output = identification_char(code)
                    .ok_or(DecodeError::InvalidIdentificationCharacter { slot, code })?;
            }
            body_kind = BodyKind::AircraftIdentification;
        }
    }
    if decode_adsb_body && (9..=18).contains(&type_code) {
        // Annex 10 Vol IV §3.1.2.8.6.3.1.2 maps this ACS subfield to AC with M omitted;
        // §3.1.2.6.5.4 defines Q=1 as 25-foot units and altitude as (25*N - 1000) ft.
        let code = u16::try_from(bits(&raw, 40, 12)?).map_err(|_| DecodeError::FieldRange)?;
        if code == 0 {
            altitude_status = AltitudeStatus::Unavailable;
        } else {
            let q = (code >> 4) & 1;
            if q == 1 {
                let n = ((code & 0x0fe0) >> 1) | (code & 0x000f);
                let altitude = i32::from(n) * 25 - 1_000;
                altitude_ft = altitude;
                altitude_status = AltitudeStatus::Encoded25Ft;
            } else {
                altitude_status = AltitudeStatus::OtherEncoding;
            }
        }
    }
    if decode_adsb_body && (5..=18).contains(&type_code) {
        cpr = CprFields {
            kind: if type_code <= 8 {
                CprKind::Surface
            } else {
                CprKind::Airborne
            },
            format: u8::try_from(bits(&raw, 53, 1)?).map_err(|_| DecodeError::FieldRange)?,
            latitude_raw: bits(&raw, 54, 17)?,
            longitude_raw: bits(&raw, 71, 17)?,
        };
    }

    Ok(ExtendedSquitter {
        raw,
        downlink_format: format,
        control,
        announced_address,
        address_use,
        type_code,
        aircraft_category,
        body_kind,
        identification,
        altitude_ft,
        altitude_status,
        cpr,
    })
}

fn bits(frame: &[u8; FRAME_LEN], start: usize, width: usize) -> Result<u32, DecodeError> {
    let end = start.checked_add(width).ok_or(DecodeError::FieldRange)?;
    if width > 32 || end > FRAME_BITS {
        return Err(DecodeError::FieldRange);
    }
    let mut value = 0u32;
    for bit in start..end {
        let octet = frame.get(bit / 8).copied().ok_or(DecodeError::FieldRange)?;
        let shift = 7 - bit % 8;
        value = (value << 1) | u32::from((octet >> shift) & 1);
    }
    Ok(value)
}

fn parity_syndrome(frame: &[u8; FRAME_LEN]) -> u32 {
    let mut remainder = 0u32;
    for &octet in frame {
        for shift in (0..8).rev() {
            let feedback = ((remainder >> (CRC_WIDTH - 1)) & 1) ^ u32::from((octet >> shift) & 1);
            remainder = (remainder << 1) & CRC_MASK;
            if feedback != 0 {
                remainder ^= CRC_POLYNOMIAL;
            }
        }
    }
    remainder
}

fn identification_char(code: u8) -> Option<u8> {
    match code {
        1..=26 => Some(code + 0x40),
        32 | 48..=57 => Some(code),
        _ => None,
    }
}
