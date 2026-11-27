//! Versioned, bounded runway-aid configuration values (pure core).
//!
//! The `oa-adapt` shell binds these exact bytes to its independently pinned issued manifest.
//! This parser validates only the supplied values; the issuer still owns source provenance and
//! the caller owns resolution against built runway geometry before using an aid.

pub const MAGIC: &[u8; 8] = b"OARWYAD1";
pub const HEADER_BYTES: usize = 10;
pub const PAIR_BYTES: usize = 48;
pub const MAX_AID_PAIRS: usize = 64;
pub const MAX_PROFILE_BYTES: usize = HEADER_BYTES + PAIR_BYTES * MAX_AID_PAIRS;

/// One pair's explicit aid adaptation. Runway IDs and end names resolve against built geometry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct AidPairConfig {
    pub airport: [u8; 4],
    pub first_runway_id: u32,
    pub first_end_name: [u8; 3],
    pub first_width_ft: u16,
    pub second_runway_id: u32,
    pub second_end_name: [u8; 3],
    pub second_width_ft: u16,
    pub ntz_width_ft: u16,
    pub minimum_spacing_ft: u16,
    pub lookahead_ms: u32,
    pub update_ms: u32,
    pub centerline_extension_m: u32,
    pub arv_lateral_tolerance_mm: u32,
    pub arv_heading_tolerance_cdeg: u16,
    pub crda_offset_mm: i32,
}
const _: () = assert!(core::mem::size_of::<AidPairConfig>() == 56);

/// A validated view over caller-owned profile bytes. It does not copy or allocate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunwayAidProfile<'a> {
    bytes: &'a [u8],
}
const _: () = assert!(core::mem::size_of::<RunwayAidProfile<'static>>() == 16);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ProfileError {
    BadHeader = 1,
    BadLength = 2,
    TooManyPairs = 3,
    InvalidPair = 4,
    PairOrder = 5,
}
const _: () = assert!(core::mem::size_of::<ProfileError>() == 1);

fn u16_at(bytes: &[u8], offset: usize) -> Option<u16> {
    let end = offset.checked_add(2)?;
    let value = bytes.get(offset..end)?;
    Some(u16::from_le_bytes([*value.first()?, *value.get(1)?]))
}

fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    let end = offset.checked_add(4)?;
    let value = bytes.get(offset..end)?;
    Some(u32::from_le_bytes([
        *value.first()?,
        *value.get(1)?,
        *value.get(2)?,
        *value.get(3)?,
    ]))
}

fn i32_at(bytes: &[u8], offset: usize) -> Option<i32> {
    u32_at(bytes, offset).map(|value| i32::from_le_bytes(u32::to_le_bytes(value)))
}

fn parse_pair(bytes: &[u8]) -> Option<AidPairConfig> {
    if bytes.len() != PAIR_BYTES {
        return None;
    }
    Some(AidPairConfig {
        airport: bytes.get(..4)?.try_into().ok()?,
        first_runway_id: u32_at(bytes, 4)?,
        first_end_name: bytes.get(8..11)?.try_into().ok()?,
        first_width_ft: u16_at(bytes, 11)?,
        second_runway_id: u32_at(bytes, 13)?,
        second_end_name: bytes.get(17..20)?.try_into().ok()?,
        second_width_ft: u16_at(bytes, 20)?,
        ntz_width_ft: u16_at(bytes, 22)?,
        minimum_spacing_ft: u16_at(bytes, 24)?,
        lookahead_ms: u32_at(bytes, 26)?,
        update_ms: u32_at(bytes, 30)?,
        centerline_extension_m: u32_at(bytes, 34)?,
        arv_lateral_tolerance_mm: u32_at(bytes, 38)?,
        arv_heading_tolerance_cdeg: u16_at(bytes, 42)?,
        crda_offset_mm: i32_at(bytes, 44)?,
    })
}

fn valid_ident(value: &[u8]) -> bool {
    value
        .iter()
        .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
}

fn valid_end_name(value: &[u8; 3]) -> bool {
    let mut seen_padding = false;
    let mut any = false;
    for byte in value {
        if *byte == 0 {
            seen_padding = true;
        } else if seen_padding || !(byte.is_ascii_uppercase() || byte.is_ascii_digit()) {
            return false;
        } else {
            any = true;
        }
    }
    any
}

fn valid_pair(row: AidPairConfig) -> bool {
    valid_ident(&row.airport)
        && row.airport.iter().any(u8::is_ascii_alphanumeric)
        && row.first_runway_id != 0
        && row.second_runway_id != 0
        && row.first_runway_id != row.second_runway_id
        && valid_end_name(&row.first_end_name)
        && valid_end_name(&row.second_end_name)
        && (1..=500).contains(&row.first_width_ft)
        && (1..=500).contains(&row.second_width_ft)
        && (200..=20_000).contains(&row.ntz_width_ft)
        && (1..=20_000).contains(&row.minimum_spacing_ft)
        && (1_000..=60_000).contains(&row.lookahead_ms)
        && (1..=4_800).contains(&row.update_ms)
        && (100..=16_000).contains(&row.centerline_extension_m)
        && row.arv_lateral_tolerance_mm <= 500_000
        && row.arv_heading_tolerance_cdeg <= 4_500
        && row.crda_offset_mm.unsigned_abs() <= 20_000_000
}

fn pair_order(row: AidPairConfig) -> ([u8; 4], u32, [u8; 3], u32, [u8; 3]) {
    (
        row.airport,
        row.first_runway_id,
        row.first_end_name,
        row.second_runway_id,
        row.second_end_name,
    )
}

/// Parse and validate a canonical v1 profile. Rows are unique and sorted so any runtime can walk
/// the same deterministic adaptation order without a map or a hot-path allocation.
pub fn parse(bytes: &[u8]) -> Result<RunwayAidProfile<'_>, ProfileError> {
    if bytes.get(..8) != Some(MAGIC.as_slice()) || u16_at(bytes, 8).is_none() {
        return Err(ProfileError::BadHeader);
    }
    let count = usize::from(u16_at(bytes, 8).ok_or(ProfileError::BadHeader)?);
    if count == 0 || count > MAX_AID_PAIRS {
        return Err(ProfileError::TooManyPairs);
    }
    let expected = HEADER_BYTES
        .checked_add(
            count
                .checked_mul(PAIR_BYTES)
                .ok_or(ProfileError::BadLength)?,
        )
        .ok_or(ProfileError::BadLength)?;
    if bytes.len() != expected {
        return Err(ProfileError::BadLength);
    }
    let mut prior = None;
    for index in 0..count {
        let start = HEADER_BYTES
            .checked_add(
                index
                    .checked_mul(PAIR_BYTES)
                    .ok_or(ProfileError::BadLength)?,
            )
            .ok_or(ProfileError::BadLength)?;
        let end = start
            .checked_add(PAIR_BYTES)
            .ok_or(ProfileError::BadLength)?;
        let row = parse_pair(bytes.get(start..end).ok_or(ProfileError::BadLength)?)
            .ok_or(ProfileError::BadLength)?;
        if !valid_pair(row) || row.first_runway_id == row.second_runway_id {
            return Err(ProfileError::InvalidPair);
        }
        let order = pair_order(row);
        if prior.is_some_and(|previous| previous >= order) {
            return Err(ProfileError::PairOrder);
        }
        prior = Some(order);
    }
    Ok(RunwayAidProfile { bytes })
}

impl RunwayAidProfile<'_> {
    pub fn len(self) -> usize {
        u16_at(self.bytes, 8).map_or(0, usize::from)
    }

    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    pub fn pair(self, index: usize) -> Option<AidPairConfig> {
        if index >= self.len() {
            return None;
        }
        let start = HEADER_BYTES.checked_add(index.checked_mul(PAIR_BYTES)?)?;
        let end = start.checked_add(PAIR_BYTES)?;
        parse_pair(self.bytes.get(start..end)?)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    fn pair_bytes(first: u32, second: u32) -> [u8; PAIR_BYTES] {
        let mut bytes = [0; PAIR_BYTES];
        bytes[..4].copy_from_slice(b"KORD");
        bytes[4..8].copy_from_slice(&first.to_le_bytes());
        bytes[8..11].copy_from_slice(b"27L");
        bytes[11..13].copy_from_slice(&150u16.to_le_bytes());
        bytes[13..17].copy_from_slice(&second.to_le_bytes());
        bytes[17..20].copy_from_slice(b"27R");
        bytes[20..22].copy_from_slice(&150u16.to_le_bytes());
        bytes[22..24].copy_from_slice(&2_000u16.to_le_bytes());
        bytes[24..26].copy_from_slice(&3_100u16.to_le_bytes());
        bytes[26..30].copy_from_slice(&10_000u32.to_le_bytes());
        bytes[30..34].copy_from_slice(&1_200u32.to_le_bytes());
        bytes[34..38].copy_from_slice(&10_000u32.to_le_bytes());
        bytes[38..42].copy_from_slice(&50_000u32.to_le_bytes());
        bytes[42..44].copy_from_slice(&500u16.to_le_bytes());
        bytes[44..48].copy_from_slice(&0i32.to_le_bytes());
        bytes
    }

    fn profile(pair: [u8; PAIR_BYTES]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(HEADER_BYTES + PAIR_BYTES);
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&pair);
        bytes
    }

    // REQ: OA-RWY-022, OA-RWY-023
    #[test]
    fn profile_accepts_bounded_sorted_pair_and_returns_exact_adaptation_values() {
        let bytes = profile(pair_bytes(1, 2));
        let parsed = parse(&bytes).expect("valid synthetic profile");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed.pair(0).expect("pair row").airport, *b"KORD");
        assert_eq!(parsed.pair(0).expect("pair row").minimum_spacing_ft, 3_100);
        assert!(parsed.pair(1).is_none());
    }

    // REQ: OA-RWY-022, OA-RWY-023
    #[test]
    fn profile_refuses_invalid_values_duplicate_or_unsorted_pairs() {
        let mut bad_minimum = pair_bytes(1, 2);
        bad_minimum[24..26].copy_from_slice(&0u16.to_le_bytes());
        assert_eq!(parse(&profile(bad_minimum)), Err(ProfileError::InvalidPair));
        assert_eq!(
            parse(&profile(pair_bytes(1, 1))),
            Err(ProfileError::InvalidPair)
        );
        let one = pair_bytes(1, 2);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&one);
        bytes.extend_from_slice(&one);
        assert_eq!(parse(&bytes), Err(ProfileError::PairOrder));
    }
}
