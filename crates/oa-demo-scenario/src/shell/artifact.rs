//! Bounded, versioned OASD v1 synthetic scenario artifact parser.
//!
//! The parser consumes a borrowed byte slice, performs no I/O or allocation, and validates the
//! complete header, payload digest, row bounds, and strict ordering before exposing records.

pub const OASD_HEADER_LEN: usize = 352;
pub const OASD_RECORD_LEN: usize = 32;
pub const OASD_MAX_RECORDS: usize = 360_000;
pub const OASD_MAX_BYTES: usize = 16 * 1024 * 1024;
pub const OASD_EPOCH_NS: i64 = 1_800_000_000_000_000_000;
pub const OASD_DURATION_MS: u64 = 2_700_000;
pub const OASD_SAMPLE_INTERVAL_MS: u32 = 5_000;
pub const OASD_SEED: u64 = 0x4F52_442D_4E50_5631;

/// OASD v1 metadata. Hashes are raw 32-byte SHA-256 digests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactHeader<'a> {
    pub scenario_id: &'a str,
    pub seed: u64,
    pub epoch_unix_ns: i64,
    pub duration_ms: u64,
    pub sample_interval_ms: u32,
    pub record_count: u32,
    pub cifp_archive_sha256: [u8; 32],
    pub cifp_entry_sha256: [u8; 32],
    pub performance_sha256: [u8; 32],
    pub scenario_lib_sha256: [u8; 32],
    pub resolver_sha256: [u8; 32],
    pub traffic_sha256: [u8; 32],
    pub prep_tool_sha256: [u8; 32],
    pub rows_sha256: [u8; 32],
    pub center_lat_e7: i32,
    pub center_lon_e7: i32,
}

/// One decoded normalized row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScenarioArtifactRecord {
    pub aircraft_index: u16,
    pub flags: u16,
    pub virtual_time_ms: i64,
    pub lat_e7: i32,
    pub lon_e7: i32,
    pub altitude_ft: i32,
    pub ground_speed_dkt: u16,
    pub track_ddeg: u16,
    pub vertical_rate_fpm: i32,
}

/// Fully validated borrowed artifact.
#[derive(Debug, Clone)]
pub struct ScenarioArtifact<'a> {
    pub header: ArtifactHeader<'a>,
    rows: &'a [u8],
}

/// Structural, integrity, or bounded-value error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactError {
    TooShort,
    TooLarge,
    BadMagic,
    UnsupportedVersion,
    BadHeader,
    BadScenarioId,
    BadCount,
    BadLength,
    ZeroDigest,
    PayloadDigest,
    BadRecord(usize),
    RecordOrder(usize),
}

impl<'a> ScenarioArtifact<'a> {
    /// Parse and validate an entire OASD v1 artifact without allocating or reading files.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, ArtifactError> {
        if bytes.len() < OASD_HEADER_LEN {
            return Err(ArtifactError::TooShort);
        }
        if bytes.len() > OASD_MAX_BYTES {
            return Err(ArtifactError::TooLarge);
        }
        if bytes.get(0..4) != Some(b"OASD") {
            return Err(ArtifactError::BadMagic);
        }
        if le_u16(bytes, 4) != Some(1) {
            return Err(ArtifactError::UnsupportedVersion);
        }
        if le_u16(bytes, 6) != Some(OASD_HEADER_LEN as u16)
            || le_u16(bytes, 8) != Some(OASD_RECORD_LEN as u16)
            || le_u16(bytes, 10) != Some(0)
            || le_u32(bytes, 12) != Some(0)
            || take(bytes, 344, 8)
                .ok_or(ArtifactError::BadHeader)?
                .iter()
                .any(|b| *b != 0)
        {
            return Err(ArtifactError::BadHeader);
        }
        let id_bytes = take(bytes, 16, 32).ok_or(ArtifactError::BadHeader)?;
        let id_len = id_bytes
            .iter()
            .position(|b| *b == 0)
            .unwrap_or(id_bytes.len());
        if id_len == 0
            || id_bytes
                .get(id_len..)
                .ok_or(ArtifactError::BadScenarioId)?
                .iter()
                .any(|b| *b != 0)
            || !id_bytes
                .get(..id_len)
                .ok_or(ArtifactError::BadScenarioId)?
                .iter()
                .all(|b| b.is_ascii_alphanumeric() || *b == b'-' || *b == b'_')
        {
            return Err(ArtifactError::BadScenarioId);
        }
        let scenario_id =
            ::core::str::from_utf8(id_bytes.get(..id_len).ok_or(ArtifactError::BadScenarioId)?)
                .map_err(|_| ArtifactError::BadScenarioId)?;
        if scenario_id != "ord-normal-push-v1" {
            return Err(ArtifactError::BadScenarioId);
        }
        let seed = le_u64(bytes, 48).ok_or(ArtifactError::BadHeader)?;
        let epoch_ns = le_i64(bytes, 56).ok_or(ArtifactError::BadHeader)?;
        let duration_ms = le_u64(bytes, 64).ok_or(ArtifactError::BadHeader)?;
        let sample_interval_ms = le_u32(bytes, 72).ok_or(ArtifactError::BadHeader)?;
        let record_count = le_u32(bytes, 76).ok_or(ArtifactError::BadCount)?;
        let center_lat_e7 = le_i32(bytes, 336).ok_or(ArtifactError::BadHeader)?;
        let center_lon_e7 = le_i32(bytes, 340).ok_or(ArtifactError::BadHeader)?;
        if !(-900_000_000..=900_000_000).contains(&center_lat_e7)
            || !(-1_800_000_000..=1_800_000_000).contains(&center_lon_e7)
        {
            return Err(ArtifactError::BadHeader);
        }
        if seed != OASD_SEED
            || epoch_ns != OASD_EPOCH_NS
            || !matches!(duration_ms, OASD_DURATION_MS | 3_600_000)
            || sample_interval_ms != OASD_SAMPLE_INTERVAL_MS
            || record_count == 0
            || record_count as usize > OASD_MAX_RECORDS
        {
            return Err(ArtifactError::BadCount);
        }
        let expected_len = OASD_HEADER_LEN
            .checked_add(
                (record_count as usize)
                    .checked_mul(OASD_RECORD_LEN)
                    .ok_or(ArtifactError::BadLength)?,
            )
            .ok_or(ArtifactError::BadLength)?;
        if bytes.len() != expected_len {
            return Err(ArtifactError::BadLength);
        }
        let mut hashes = [[0u8; 32]; 8];
        for (i, hash) in hashes.iter_mut().enumerate() {
            let start = 80 + i * 32;
            hash.copy_from_slice(take(bytes, start, 32).ok_or(ArtifactError::BadHeader)?);
            if hash.iter().all(|b| *b == 0) {
                return Err(ArtifactError::ZeroDigest);
            }
        }
        let rows = bytes
            .get(OASD_HEADER_LEN..)
            .ok_or(ArtifactError::BadLength)?;
        let rows_hash = *hashes.get(7).ok_or(ArtifactError::BadHeader)?;
        if oa_sha256::sha256(rows) != rows_hash {
            return Err(ArtifactError::PayloadDigest);
        }
        let header = ArtifactHeader {
            scenario_id,
            seed,
            epoch_unix_ns: epoch_ns,
            duration_ms,
            sample_interval_ms,
            record_count,
            cifp_archive_sha256: *hashes.first().ok_or(ArtifactError::BadHeader)?,
            cifp_entry_sha256: *hashes.get(1).ok_or(ArtifactError::BadHeader)?,
            performance_sha256: *hashes.get(2).ok_or(ArtifactError::BadHeader)?,
            scenario_lib_sha256: *hashes.get(3).ok_or(ArtifactError::BadHeader)?,
            resolver_sha256: *hashes.get(4).ok_or(ArtifactError::BadHeader)?,
            traffic_sha256: *hashes.get(5).ok_or(ArtifactError::BadHeader)?,
            prep_tool_sha256: *hashes.get(6).ok_or(ArtifactError::BadHeader)?,
            rows_sha256: rows_hash,
            center_lat_e7,
            center_lon_e7,
        };
        let artifact = Self { header, rows };
        let mut previous: Option<(i64, u16)> = None;
        for (i, row) in artifact.records().enumerate() {
            let row = row?;
            let key = (row.virtual_time_ms, row.aircraft_index);
            if previous.is_some_and(|prior| key <= prior) {
                return Err(ArtifactError::RecordOrder(i));
            }
            previous = Some(key);
        }
        Ok(artifact)
    }

    pub fn records(
        &self,
    ) -> impl Iterator<Item = Result<ScenarioArtifactRecord, ArtifactError>> + '_ {
        self.rows
            .chunks_exact(OASD_RECORD_LEN)
            .enumerate()
            .map(|(i, row)| {
                decode_record(row, self.header.duration_ms).ok_or(ArtifactError::BadRecord(i))
            })
    }
}

fn decode_record(row: &[u8], duration_ms: u64) -> Option<ScenarioArtifactRecord> {
    if row.len() != OASD_RECORD_LEN {
        return None;
    }
    let aircraft_index = le_u16(row, 0)?;
    let flags = le_u16(row, 2)?;
    let virtual_time_ms = le_i64(row, 4)?;
    let lat_e7 = le_i32(row, 12)?;
    let lon_e7 = le_i32(row, 16)?;
    let altitude_ft = le_i32(row, 20)?;
    let ground_speed_dkt = le_u16(row, 24)?;
    let track_ddeg = le_u16(row, 26)?;
    let vertical_rate_fpm = le_i32(row, 28)?;
    if aircraft_index >= 60
        || flags & !3 != 0
        || !(-21_600_000..=duration_ms as i64).contains(&virtual_time_ms)
        || !(-900_000_000..=900_000_000).contains(&lat_e7)
        || !(-1_800_000_000..=1_800_000_000).contains(&lon_e7)
        || !(-2_000..=60_000).contains(&altitude_ft)
        || ground_speed_dkt > 4_000
        || track_ddeg >= 3_600
        || vertical_rate_fpm.unsigned_abs() > 1_800
    {
        return None;
    }
    Some(ScenarioArtifactRecord {
        aircraft_index,
        flags,
        virtual_time_ms,
        lat_e7,
        lon_e7,
        altitude_ft,
        ground_speed_dkt,
        track_ddeg,
        vertical_rate_fpm,
    })
}

fn le_u16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(take(bytes, at, 2)?.try_into().ok()?))
}

fn le_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(take(bytes, at, 4)?.try_into().ok()?))
}

fn le_u64(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(take(bytes, at, 8)?.try_into().ok()?))
}

fn le_i32(bytes: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_le_bytes(take(bytes, at, 4)?.try_into().ok()?))
}

fn le_i64(bytes: &[u8], at: usize) -> Option<i64> {
    Some(i64::from_le_bytes(take(bytes, at, 8)?.try_into().ok()?))
}

fn take(bytes: &[u8], at: usize, len: usize) -> Option<&[u8]> {
    bytes.get(at..at.checked_add(len)?)
}
