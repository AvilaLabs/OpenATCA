//! Versioned, bounded runway-aid configuration values (pure core).
//!
//! The `oa-adapt` shell binds these exact bytes to its independently pinned issued manifest.
//! This parser validates only the supplied values; the issuer still owns source provenance and
//! the caller owns resolution against built runway geometry before using an aid.

pub const MAGIC: &[u8; 8] = b"OARWYAD1";
/// Version two binds the complete runway/taxiway candidate geometry used by the runtime ARV
/// bridge. Version one remains readable, but its pair-only geometry is never a complete ARV
/// coverage claim.
pub const MAGIC_V2: &[u8; 8] = b"OARWYAD2";
pub const HEADER_BYTES: usize = 10;
pub const HEADER_V2_BYTES: usize = 16;
pub const PAIR_BYTES: usize = 48;
pub const PATH_BYTES: usize = 36;
pub const MAX_AID_PAIRS: usize = 64;
pub const MAX_AID_PATHS: usize = 512;
pub const MAX_PROFILE_BYTES: usize =
    HEADER_V2_BYTES + PAIR_BYTES * MAX_AID_PAIRS + PATH_BYTES * MAX_AID_PATHS;

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

/// One explicitly issued, directed runway or taxiway centerline segment. Coordinates are WGS-84
/// E7; `threshold_*` is the direction-of-travel endpoint and `outer_*` defines its axis. Taxiway
/// segments that need detection in both directions are represented as two rows with distinct IDs
/// and reversed endpoints. `path_id` is unique within its airport and equals the runway ID for
/// runway rows.
// A7.2: cold fixed-size profile row; the canonical 36-byte encoding is parsed field-by-field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct AidPathConfig {
    pub airport: [u8; 4],
    pub path_id: u32,
    pub kind: u8,
    pub name: [u8; 4],
    pub width_mm: u32,
    pub threshold_lat_e7: i32,
    pub threshold_lon_e7: i32,
    pub outer_lat_e7: i32,
    pub outer_lon_e7: i32,
}
const _: () = assert!(core::mem::size_of::<AidPathConfig>() == 36);

pub const PATH_KIND_RUNWAY: u8 = 1;
pub const PATH_KIND_TAXIWAY: u8 = 2;
pub const COVERAGE_PAIR_ONLY: u8 = 0;
pub const COVERAGE_COMPLETE_AIRPORT: u8 = 1;

/// A validated view over caller-owned profile bytes. It does not copy or allocate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunwayAidProfile<'a> {
    bytes: &'a [u8],
    v2: bool,
}
// A7.2: cold parser view; a slice pointer plus version flag is 24 bytes on the pinned 64-bit targets.
const _: () = assert!(core::mem::size_of::<RunwayAidProfile<'static>>() == 24);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ProfileError {
    BadHeader = 1,
    BadLength = 2,
    TooManyPairs = 3,
    InvalidPair = 4,
    PairOrder = 5,
    PathOrder = 6,
    InvalidPath = 7,
    MissingPairPath = 8,
    InvalidCoverage = 9,
    TooManyPaths = 10,
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

fn parse_path(bytes: &[u8]) -> Option<AidPathConfig> {
    if bytes.len() != PATH_BYTES || bytes.get(33..36) != Some([0, 0, 0].as_slice()) {
        return None;
    }
    Some(AidPathConfig {
        airport: bytes.get(..4)?.try_into().ok()?,
        path_id: u32_at(bytes, 4)?,
        kind: *bytes.get(8)?,
        name: bytes.get(9..13)?.try_into().ok()?,
        width_mm: u32_at(bytes, 13)?,
        threshold_lat_e7: i32_at(bytes, 17)?,
        threshold_lon_e7: i32_at(bytes, 21)?,
        outer_lat_e7: i32_at(bytes, 25)?,
        outer_lon_e7: i32_at(bytes, 29)?,
    })
}

fn valid_path(row: AidPathConfig) -> bool {
    let valid_ident = |value: &[u8]| {
        value
            .iter()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || *byte == b' ')
    };
    row.airport.iter().any(u8::is_ascii_alphanumeric)
        && valid_ident(&row.airport)
        && row.path_id != 0
        && matches!(row.kind, PATH_KIND_RUNWAY | PATH_KIND_TAXIWAY)
        && row.name.iter().any(u8::is_ascii_alphanumeric)
        && valid_ident(&row.name)
        && (1_000..=200_000).contains(&row.width_mm)
        && (-900_000_000..=900_000_000).contains(&row.threshold_lat_e7)
        && (-1_800_000_000..=1_800_000_000).contains(&row.threshold_lon_e7)
        && (-900_000_000..=900_000_000).contains(&row.outer_lat_e7)
        && (-1_800_000_000..=1_800_000_000).contains(&row.outer_lon_e7)
        && (row.threshold_lat_e7 != row.outer_lat_e7 || row.threshold_lon_e7 != row.outer_lon_e7)
        && (row.kind != PATH_KIND_RUNWAY
            || (row.name.get(..3).is_some_and(|name| {
                name.iter()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
            }) && row.name.get(3) == Some(&b' ')))
}

fn path_order(row: AidPathConfig) -> ([u8; 4], u32) {
    (row.airport, row.path_id)
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
    let v2 = bytes.get(..8) == Some(MAGIC_V2.as_slice());
    if (!v2 && bytes.get(..8) != Some(MAGIC.as_slice())) || u16_at(bytes, 8).is_none() {
        return Err(ProfileError::BadHeader);
    }
    let count = usize::from(u16_at(bytes, 8).ok_or(ProfileError::BadHeader)?);
    if count == 0 || count > MAX_AID_PAIRS {
        return Err(ProfileError::TooManyPairs);
    }
    let path_count = if v2 {
        usize::from(u16_at(bytes, 10).ok_or(ProfileError::BadHeader)?)
    } else {
        0
    };
    let header_bytes = if v2 { HEADER_V2_BYTES } else { HEADER_BYTES };
    if path_count > MAX_AID_PATHS {
        return Err(ProfileError::TooManyPaths);
    }
    if v2 {
        let coverage = *bytes.get(12).ok_or(ProfileError::BadHeader)?;
        if !matches!(coverage, COVERAGE_PAIR_ONLY | COVERAGE_COMPLETE_AIRPORT)
            || bytes.get(13..16) != Some([0, 0, 0].as_slice())
        {
            return Err(ProfileError::InvalidCoverage);
        }
        if coverage == COVERAGE_COMPLETE_AIRPORT && path_count == 0 {
            return Err(ProfileError::InvalidCoverage);
        }
    }
    let expected = header_bytes
        .checked_add(
            count
                .checked_mul(PAIR_BYTES)
                .ok_or(ProfileError::BadLength)?,
        )
        .and_then(|size| size.checked_add(path_count.checked_mul(PATH_BYTES)?))
        .ok_or(ProfileError::BadLength)?;
    if bytes.len() != expected {
        return Err(ProfileError::BadLength);
    }
    let mut prior = None;
    for index in 0..count {
        let start = header_bytes
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
    if v2 {
        let paths_start = header_bytes
            .checked_add(
                count
                    .checked_mul(PAIR_BYTES)
                    .ok_or(ProfileError::BadLength)?,
            )
            .ok_or(ProfileError::BadLength)?;
        let mut prior_path = None;
        for index in 0..path_count {
            let start = paths_start
                .checked_add(
                    index
                        .checked_mul(PATH_BYTES)
                        .ok_or(ProfileError::BadLength)?,
                )
                .ok_or(ProfileError::BadLength)?;
            let end = start
                .checked_add(PATH_BYTES)
                .ok_or(ProfileError::BadLength)?;
            let row = parse_path(bytes.get(start..end).ok_or(ProfileError::BadLength)?)
                .ok_or(ProfileError::InvalidPath)?;
            if !valid_path(row) {
                return Err(ProfileError::InvalidPath);
            }
            let order = path_order(row);
            if prior_path.is_some_and(|previous| previous >= order) {
                return Err(ProfileError::PathOrder);
            }
            prior_path = Some(order);
        }
        if *bytes.get(12).ok_or(ProfileError::BadHeader)? == COVERAGE_COMPLETE_AIRPORT {
            for pair_index in 0..count {
                let pair_start = header_bytes
                    .checked_add(
                        pair_index
                            .checked_mul(PAIR_BYTES)
                            .ok_or(ProfileError::BadLength)?,
                    )
                    .ok_or(ProfileError::BadLength)?;
                let pair = parse_pair(
                    bytes
                        .get(pair_start..pair_start + PAIR_BYTES)
                        .ok_or(ProfileError::BadLength)?,
                )
                .ok_or(ProfileError::BadLength)?;
                let has_path = |path_id: u32, end_name: [u8; 3]| {
                    (0..path_count).any(|path_index| {
                        let Some(start) = path_index
                            .checked_mul(PATH_BYTES)
                            .and_then(|offset| paths_start.checked_add(offset))
                        else {
                            return false;
                        };
                        let Some(end) = start.checked_add(PATH_BYTES) else {
                            return false;
                        };
                        let Some(path) = bytes.get(start..end).and_then(parse_path) else {
                            return false;
                        };
                        path.airport == pair.airport
                            && path.path_id == path_id
                            && path.kind == PATH_KIND_RUNWAY
                            && path.name.get(..3) == Some(end_name.as_slice())
                    })
                };
                if !has_path(pair.first_runway_id, pair.first_end_name)
                    || !has_path(pair.second_runway_id, pair.second_end_name)
                {
                    return Err(ProfileError::MissingPairPath);
                }
            }
        }
    }
    Ok(RunwayAidProfile { bytes, v2 })
}

impl RunwayAidProfile<'_> {
    pub fn len(self) -> usize {
        u16_at(self.bytes, 8).map_or(0, usize::from)
    }

    pub fn has_complete_airport_coverage(self) -> bool {
        self.v2 && self.bytes.get(12) == Some(&COVERAGE_COMPLETE_AIRPORT)
    }

    pub fn path_len(self) -> usize {
        if self.v2 {
            u16_at(self.bytes, 10).map_or(0, usize::from)
        } else {
            0
        }
    }

    pub fn path(self, index: usize) -> Option<AidPathConfig> {
        if !self.v2 || index >= self.path_len() {
            return None;
        }
        let start = HEADER_V2_BYTES
            .checked_add(self.len().checked_mul(PAIR_BYTES)?)?
            .checked_add(index.checked_mul(PATH_BYTES)?)?;
        let end = start.checked_add(PATH_BYTES)?;
        parse_path(self.bytes.get(start..end)?)
    }

    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    pub fn pair(self, index: usize) -> Option<AidPairConfig> {
        if index >= self.len() {
            return None;
        }
        let header = if self.v2 {
            HEADER_V2_BYTES
        } else {
            HEADER_BYTES
        };
        let start = header.checked_add(index.checked_mul(PAIR_BYTES)?)?;
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

    fn path_bytes(
        path_id: u32,
        kind: u8,
        name: [u8; 4],
        threshold_lat_e7: i32,
        outer_lat_e7: i32,
    ) -> [u8; PATH_BYTES] {
        let mut bytes = [0; PATH_BYTES];
        bytes[..4].copy_from_slice(b"KORD");
        bytes[4..8].copy_from_slice(&path_id.to_le_bytes());
        bytes[8] = kind;
        bytes[9..13].copy_from_slice(&name);
        bytes[13..17].copy_from_slice(&45_000u32.to_le_bytes());
        bytes[17..21].copy_from_slice(&threshold_lat_e7.to_le_bytes());
        bytes[21..25].copy_from_slice(&(-876_000_000i32).to_le_bytes());
        bytes[25..29].copy_from_slice(&outer_lat_e7.to_le_bytes());
        bytes[29..33].copy_from_slice(&(-876_000_000i32).to_le_bytes());
        bytes
    }

    fn complete_v2_profile(paths: &[[u8; PATH_BYTES]]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(HEADER_V2_BYTES + PAIR_BYTES + PATH_BYTES * paths.len());
        bytes.extend_from_slice(MAGIC_V2);
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(
            &u16::try_from(paths.len())
                .expect("test path count")
                .to_le_bytes(),
        );
        bytes.push(COVERAGE_COMPLETE_AIRPORT);
        bytes.extend_from_slice(&[0; 3]);
        bytes.extend_from_slice(&pair_bytes(1, 2));
        for path in paths {
            bytes.extend_from_slice(path);
        }
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

    // REQ: OA-RWY-024
    #[test]
    fn v1_pair_only_profile_is_not_complete_arv_coverage() {
        let bytes = profile(pair_bytes(1, 2));
        let parsed = parse(&bytes).expect("valid legacy profile");
        assert!(!parsed.has_complete_airport_coverage());
        assert_eq!(parsed.path_len(), 0);
        assert!(parsed.path(0).is_none());
    }

    // REQ: OA-RWY-024
    #[test]
    fn v2_complete_profile_preserves_sorted_runway_and_taxiway_paths() {
        let paths = [
            path_bytes(1, PATH_KIND_RUNWAY, *b"27L ", 418000000, 418100000),
            path_bytes(2, PATH_KIND_RUNWAY, *b"27R ", 418000000, 418100000),
            path_bytes(100, PATH_KIND_TAXIWAY, *b"T1  ", 418020000, 418030000),
            path_bytes(101, PATH_KIND_TAXIWAY, *b"T1  ", 418030000, 418020000),
        ];
        let bytes = complete_v2_profile(&paths);
        let parsed = parse(&bytes).expect("valid full candidate set");
        assert!(parsed.has_complete_airport_coverage());
        assert_eq!(parsed.path_len(), 4);
        assert_eq!(parsed.path(2).expect("taxiway row").kind, PATH_KIND_TAXIWAY);
        assert_eq!(parsed.path(2).expect("taxiway row").path_id, 100);
        assert_eq!(parsed.path(3).expect("reverse taxiway row").path_id, 101);
    }

    // REQ: OA-RWY-024
    #[test]
    fn v2_complete_coverage_requires_both_assigned_pair_runways_and_valid_order() {
        let missing_second = [path_bytes(
            1,
            PATH_KIND_RUNWAY,
            *b"27L ",
            418000000,
            418100000,
        )];
        assert_eq!(
            parse(&complete_v2_profile(&missing_second)),
            Err(ProfileError::MissingPairPath)
        );
        let duplicate = [
            path_bytes(1, PATH_KIND_RUNWAY, *b"27L ", 418000000, 418100000),
            path_bytes(1, PATH_KIND_RUNWAY, *b"27L ", 418000000, 418100000),
        ];
        assert_eq!(
            parse(&complete_v2_profile(&duplicate)),
            Err(ProfileError::PathOrder)
        );
        let mut malformed = complete_v2_profile(&[
            path_bytes(1, PATH_KIND_RUNWAY, *b"27L ", 418000000, 418100000),
            path_bytes(2, PATH_KIND_RUNWAY, *b"27R ", 418000000, 418100000),
        ]);
        let reserved = malformed.len().checked_sub(1).expect("reserved byte");
        *malformed.get_mut(reserved).expect("last byte") = 1;
        assert_eq!(parse(&malformed), Err(ProfileError::InvalidPath));
    }
}
