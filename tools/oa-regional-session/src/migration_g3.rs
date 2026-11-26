//! Per-row development comparison against a frozen G3 empirical occupancy model.
//!
//! This is a transfer-proxy comparison, not official geometry, controller intent, or an
//! independent truth source. The expected model identity must be established by the caller's
//! guarded artifact/build receipts. No file I/O occurs here.

const MAX_MODEL_BYTES: usize = 64 * 1024 * 1024;
const MAX_MODEL_ROWS: usize = 2_000_000;
pub const PROFILE_ID: &str = "M4-H2-G3-OCCUPANCY-TRANSFER-V1";
pub const ASOF_PROFILE_ID: &str = "M4-H2-G3-OCCUPANCY-TRANSFER-V2";
pub const ASOF_MAX_STATE_AGE_NS: i64 = 1_000_000_000;

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LabelClass {
    Supported = 1,
    Contested = 2,
    Unknown = 3,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// A7.2: one cold aggregate row, bounded to 2,000,000 entries by the artifact parser.
pub struct CellLabel {
    pub lat_bin: i32,
    pub lon_bin: i32,
    pub altitude_band: i32,
    pub owner: [u8; 8],
    pub class: LabelClass,
    pub rows: u64,
    pub flights: u64,
    pub time_bins: u64,
}
const _: () = assert!(core::mem::size_of::<CellLabel>() == 48);

#[derive(Clone, Debug, PartialEq, Eq)]
// A7.2: one private development model, bounded to 64 MiB input and 2,000,000 labels.
pub struct TrainingModel {
    pub artifact_sha256: [u8; 32],
    pub training_sha256: [u8; 32],
    pub binary_sha256: [u8; 32],
    pub policy_sha256: [u8; 32],
    pub source_revision: String,
    pub labels: Vec<CellLabel>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelError {
    TooLarge,
    BadHeader,
    MissingMetadata,
    DuplicateMetadata,
    MetadataMismatch,
    MalformedRow,
    DuplicateRow,
    Capacity,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceReference {
    pub gufi: u128,
    pub source_time_ms: i64,
    pub receive_time_ms: i64,
    pub published_owner: Option<[u8; 8]>,
}
const _: () = assert!(core::mem::size_of::<SourceReference>() == 48);

fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn parse_hex<const N: usize>(value: &str) -> Option<[u8; N]> {
    if value.len() != N.checked_mul(2)? {
        return None;
    }
    let mut out = [0u8; N];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        *out.get_mut(index)? = (hex_nibble(*pair.first()?)? << 4) | hex_nibble(*pair.get(1)?)?;
    }
    Some(out)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct CellKey {
    lat: i32,
    lon: i32,
    band: i32,
    owner: [u8; 8],
}

#[derive(Clone, Copy)]
struct Aggregate {
    key: CellKey,
    rows: u64,
    flights: u64,
    bins: u64,
}

/// Parse the exact fixed G3 training profile. Expected hashes/revision must be taken from
/// independently verified receipts. The artifact's own claims are never trusted as expectations.
pub fn parse_training_model(
    bytes: &[u8],
    expected_artifact_sha256: &str,
    expected_training_sha256: &str,
    expected_source_revision: &str,
    expected_binary_sha256: &str,
    expected_policy_sha256: &str,
) -> Result<TrainingModel, ModelError> {
    if bytes.len() > MAX_MODEL_BYTES {
        return Err(ModelError::TooLarge);
    }
    if oa_sha256::to_hex(&oa_sha256::sha256(bytes)) != expected_artifact_sha256 {
        return Err(ModelError::MetadataMismatch);
    }
    let text = core::str::from_utf8(bytes).map_err(|_| ModelError::BadHeader)?;
    if !text.is_ascii() || !text.starts_with("OA-G3-OCCUPANCY-1\n") {
        return Err(ModelError::BadHeader);
    }
    let expected_artifact =
        parse_hex::<32>(expected_artifact_sha256).ok_or(ModelError::MetadataMismatch)?;
    let expected_binary =
        parse_hex::<32>(expected_binary_sha256).ok_or(ModelError::MetadataMismatch)?;
    let expected_policy =
        parse_hex::<32>(expected_policy_sha256).ok_or(ModelError::MetadataMismatch)?;
    let expected_training =
        parse_hex::<32>(expected_training_sha256).ok_or(ModelError::MetadataMismatch)?;
    let mut metadata = std::collections::BTreeMap::<&str, &str>::new();
    let mut cells: Vec<Aggregate> = Vec::new();
    let mut classes: Vec<(CellKey, LabelClass)> = Vec::new();
    for line in text.lines().skip(1) {
        if line.len() > 4096 {
            return Err(ModelError::TooLarge);
        }
        let mut fields = line.split('\t');
        match fields.next() {
            Some("cell") => {
                if cells.len() >= MAX_MODEL_ROWS {
                    return Err(ModelError::TooLarge);
                }
                let lat = fields
                    .next()
                    .and_then(|s| s.parse().ok())
                    .ok_or(ModelError::MalformedRow)?;
                let lon = fields
                    .next()
                    .and_then(|s| s.parse().ok())
                    .ok_or(ModelError::MalformedRow)?;
                let band = fields
                    .next()
                    .and_then(|s| s.parse().ok())
                    .ok_or(ModelError::MalformedRow)?;
                let owner = fields
                    .next()
                    .and_then(parse_hex::<8>)
                    .ok_or(ModelError::MalformedRow)?;
                let rows = fields
                    .next()
                    .and_then(|s| s.parse().ok())
                    .ok_or(ModelError::MalformedRow)?;
                let flights = fields
                    .next()
                    .and_then(|s| s.parse().ok())
                    .ok_or(ModelError::MalformedRow)?;
                let bins = fields
                    .next()
                    .and_then(|s| s.parse().ok())
                    .ok_or(ModelError::MalformedRow)?;
                if fields.next().is_some() || rows == 0 || flights == 0 || bins == 0 {
                    return Err(ModelError::MalformedRow);
                }
                cells.try_reserve(1).map_err(|_| ModelError::Capacity)?;
                cells.push(Aggregate {
                    key: CellKey {
                        lat,
                        lon,
                        band,
                        owner,
                    },
                    rows,
                    flights,
                    bins,
                });
            }
            Some("class") => {
                if classes.len() >= MAX_MODEL_ROWS {
                    return Err(ModelError::TooLarge);
                }
                let lat = fields
                    .next()
                    .and_then(|s| s.parse().ok())
                    .ok_or(ModelError::MalformedRow)?;
                let lon = fields
                    .next()
                    .and_then(|s| s.parse().ok())
                    .ok_or(ModelError::MalformedRow)?;
                let band = fields
                    .next()
                    .and_then(|s| s.parse().ok())
                    .ok_or(ModelError::MalformedRow)?;
                let owner = fields
                    .next()
                    .and_then(parse_hex::<8>)
                    .ok_or(ModelError::MalformedRow)?;
                let class = match fields.next() {
                    Some("supported") => LabelClass::Supported,
                    Some("contested") => LabelClass::Contested,
                    Some("unknown") => LabelClass::Unknown,
                    _ => return Err(ModelError::MalformedRow),
                };
                if fields.next().is_some() {
                    return Err(ModelError::MalformedRow);
                }
                classes.try_reserve(1).map_err(|_| ModelError::Capacity)?;
                classes.push((
                    CellKey {
                        lat,
                        lon,
                        band,
                        owner,
                    },
                    class,
                ));
            }
            Some("handoff") | Some("") | None => {}
            Some(_) => {
                if metadata.len() >= 128 {
                    return Err(ModelError::TooLarge);
                }
                let (key, value) = line.split_once('=').ok_or(ModelError::MissingMetadata)?;
                if key.is_empty() || key.len() > 128 || value.len() > 1024 {
                    return Err(ModelError::TooLarge);
                }
                if metadata.insert(key, value).is_some() {
                    return Err(ModelError::DuplicateMetadata);
                }
            }
        }
    }
    let get = |key: &str| {
        metadata
            .get(key)
            .copied()
            .ok_or(ModelError::MissingMetadata)
    };
    for (key, value) in [
        ("complete", "true"),
        ("completion_scope", "selected-window"),
        (
            "claim",
            "empirical_observed_occupancy_only_not_legal_geometry",
        ),
        ("source_schema", "H2-OBSERVED-STATE-1"),
        ("scope", "SFDPS-replay-compatibility"),
        ("independent_predictions", "not-measured"),
        ("independent_agreement_denominator", "not-observable"),
        (
            "source_reconstruction_claim",
            "none_exact_trace_and_binary_reproducibility_only",
        ),
        ("training_partition", "2026-11-15"),
        ("grid_e7", "500000"),
        ("altitude_slab_ft", "2000"),
        ("time_bin_ms", "900000"),
        ("grid_scope", "global_wgs84_no_official_coverage_mask"),
        ("minimum_flights", "3"),
        ("minimum_time_bins", "2"),
        ("support_is_not_probability", "true"),
    ] {
        if get(key)? != value {
            return Err(ModelError::MetadataMismatch);
        }
    }
    if get("source_revision")? != expected_source_revision
        || parse_hex::<32>(get("training_sha256")?).as_ref() != Some(&expected_training)
        || parse_hex::<32>(get("binary_sha256")?).as_ref() != Some(&expected_binary)
        || parse_hex::<32>(get("policy_sha256")?).as_ref() != Some(&expected_policy)
    {
        return Err(ModelError::MetadataMismatch);
    }
    let training_sha256 =
        parse_hex::<32>(get("training_sha256")?).ok_or(ModelError::MetadataMismatch)?;
    let binary_sha256 =
        parse_hex::<32>(get("binary_sha256")?).ok_or(ModelError::MetadataMismatch)?;
    let policy_sha256 =
        parse_hex::<32>(get("policy_sha256")?).ok_or(ModelError::MetadataMismatch)?;
    cells.sort_unstable_by_key(|cell| cell.key);
    classes.sort_unstable_by_key(|(key, _)| *key);
    if cells.windows(2).any(|pair| {
        pair.first()
            .zip(pair.get(1))
            .is_some_and(|(a, b)| a.key == b.key)
    }) || classes.windows(2).any(|pair| {
        pair.first()
            .zip(pair.get(1))
            .is_some_and(|(a, b)| a.0 == b.0)
    }) || cells.len() != classes.len()
    {
        return Err(ModelError::DuplicateRow);
    }
    let mut labels = Vec::new();
    labels
        .try_reserve(classes.len())
        .map_err(|_| ModelError::Capacity)?;
    for (cell, (key, class)) in cells.into_iter().zip(classes) {
        if cell.key != key
            || (matches!(class, LabelClass::Supported | LabelClass::Contested)
                && (cell.flights < 3 || cell.bins < 2))
            || (class == LabelClass::Unknown && cell.flights >= 3 && cell.bins >= 2)
        {
            return Err(ModelError::MalformedRow);
        }
        labels.push(CellLabel {
            lat_bin: key.lat,
            lon_bin: key.lon,
            altitude_band: key.band,
            owner: key.owner,
            class,
            rows: cell.rows,
            flights: cell.flights,
            time_bins: cell.bins,
        });
    }
    Ok(TrainingModel {
        artifact_sha256: expected_artifact,
        training_sha256,
        binary_sha256,
        policy_sha256,
        source_revision: expected_source_revision.to_owned(),
        labels,
    })
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disposition {
    SupportedMatch = 1,
    SupportedMismatch = 2,
    Contested = 3,
    UnknownSupport = 4,
    NoTrainingCell = 5,
    CandidateUnavailable = 6,
    IdentityMismatch = 7,
    InvalidTime = 8,
    InvalidPosition = 9,
    MissingAltitude = 10,
    UnknownSourceOwner = 11,
    InvalidAltitude = 12,
    CandidatePositionStale = 13,
    CandidateStateFuture = 14,
    CandidateStateStale = 15,
    CandidateAvailabilityUnknown = 16,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CandidatePosition {
    /// Address reported by the candidate surveillance output; compared with the flight's
    /// candidate-owned address before using its position.
    pub address: u32,
    pub state_ns: i64,
    pub newest_source_ns: i64,
    /// Actual local input availability for the point used to produce this state. None means the
    /// recorder cannot establish the availability clock and therefore cannot support this row.
    pub newest_available_ns: Option<i64>,
    pub lat_e7: i32,
    pub lon_e7: i32,
    /// None is missing/invalid position altitude, never a magic sentinel.
    pub altitude_ft: Option<i32>,
}
const _: () = assert!(core::mem::size_of::<CandidatePosition>() == 56);

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowResult {
    pub disposition: Disposition,
    pub gufi: u128,
    pub address: u32,
    pub state_ns: i64,
    pub receive_ns: i64,
    pub source_time_ns: i64,
    pub lat_bin: i32,
    pub lon_bin: i32,
    pub altitude_band: i32,
    pub source_owner: [u8; 8],
    pub predicted_owner: [u8; 8],
    pub support_rows: u64,
    pub support_flights: u64,
    pub support_time_bins: u64,
}

/// Per-disposition denominator for the G3 transfer proxy. The checked sum of every supplied
/// SFDPS row includes rows without a supported occupancy label or candidate position.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OutcomeCounts {
    pub supported_match: u64,
    pub supported_mismatch: u64,
    pub contested: u64,
    pub unknown_support: u64,
    pub no_training_cell: u64,
    pub candidate_unavailable: u64,
    pub identity_mismatch: u64,
    pub invalid_time: u64,
    pub invalid_position: u64,
    pub missing_altitude: u64,
    pub unknown_source_owner: u64,
    pub invalid_altitude: u64,
    pub candidate_position_stale: u64,
    pub candidate_state_future: u64,
    pub candidate_state_stale: u64,
    pub candidate_availability_unknown: u64,
    pub total: u64,
}
const _: () = assert!(core::mem::size_of::<OutcomeCounts>() == 136);

impl OutcomeCounts {
    pub fn record(&mut self, result: &RowResult) -> Result<(), ModelError> {
        self.total = self.total.checked_add(1).ok_or(ModelError::Capacity)?;
        let slot = match result.disposition {
            Disposition::SupportedMatch => &mut self.supported_match,
            Disposition::SupportedMismatch => &mut self.supported_mismatch,
            Disposition::Contested => &mut self.contested,
            Disposition::UnknownSupport => &mut self.unknown_support,
            Disposition::NoTrainingCell => &mut self.no_training_cell,
            Disposition::CandidateUnavailable => &mut self.candidate_unavailable,
            Disposition::IdentityMismatch => &mut self.identity_mismatch,
            Disposition::InvalidTime => &mut self.invalid_time,
            Disposition::InvalidPosition => &mut self.invalid_position,
            Disposition::MissingAltitude => &mut self.missing_altitude,
            Disposition::UnknownSourceOwner => &mut self.unknown_source_owner,
            Disposition::InvalidAltitude => &mut self.invalid_altitude,
            Disposition::CandidatePositionStale => &mut self.candidate_position_stale,
            Disposition::CandidateStateFuture => &mut self.candidate_state_future,
            Disposition::CandidateStateStale => &mut self.candidate_state_stale,
            Disposition::CandidateAvailabilityUnknown => &mut self.candidate_availability_unknown,
        };
        *slot = slot.checked_add(1).ok_or(ModelError::Capacity)?;
        Ok(())
    }

    pub fn reconciles(&self) -> bool {
        self.supported_match
            .checked_add(self.supported_mismatch)
            .and_then(|n| n.checked_add(self.contested))
            .and_then(|n| n.checked_add(self.unknown_support))
            .and_then(|n| n.checked_add(self.no_training_cell))
            .and_then(|n| n.checked_add(self.candidate_unavailable))
            .and_then(|n| n.checked_add(self.identity_mismatch))
            .and_then(|n| n.checked_add(self.invalid_time))
            .and_then(|n| n.checked_add(self.invalid_position))
            .and_then(|n| n.checked_add(self.missing_altitude))
            .and_then(|n| n.checked_add(self.unknown_source_owner))
            .and_then(|n| n.checked_add(self.invalid_altitude))
            .and_then(|n| n.checked_add(self.candidate_position_stale))
            .and_then(|n| n.checked_add(self.candidate_state_future))
            .and_then(|n| n.checked_add(self.candidate_state_stale))
            .and_then(|n| n.checked_add(self.candidate_availability_unknown))
            == Some(self.total)
    }
}

const EMPTY_RESULT: RowResult = RowResult {
    disposition: Disposition::CandidateUnavailable,
    gufi: 0,
    address: 0,
    state_ns: 0,
    receive_ns: 0,
    source_time_ns: 0,
    lat_bin: 0,
    lon_bin: 0,
    altitude_band: 0,
    source_owner: [0; 8],
    predicted_owner: [0; 8],
    support_rows: 0,
    support_flights: 0,
    support_time_bins: 0,
};

/// Compare an SFDPS controlling-unit observation with the unique supported G3 owner at the
/// candidate track's exact recorder-receive-time state. The caller must pass a candidate-owned
/// address-to-GUFI association; no matching key is synthesized here.
pub fn compare_row(
    source: &SourceReference,
    candidate_gufi: u128,
    candidate_address: u32,
    position: Option<&CandidatePosition>,
    model: &TrainingModel,
) -> RowResult {
    compare_row_with_age(
        source,
        candidate_gufi,
        candidate_address,
        position,
        model,
        None,
        None,
    )
}

/// Compare against the latest already-emitted candidate state at or before the SFDPS receive
/// time. V2 allows a fixed one-second causal state age and never uses a future tick or prediction.
pub fn compare_row_asof(
    source: &SourceReference,
    candidate_gufi: u128,
    candidate_address: u32,
    position: Option<&CandidatePosition>,
    model: &TrainingModel,
) -> RowResult {
    compare_row_with_age(
        source,
        candidate_gufi,
        candidate_address,
        position,
        model,
        Some(ASOF_MAX_STATE_AGE_NS),
        None,
    )
}

/// V2 as-of comparison with the exact recorder receive clock. `SourceReference` retains its
/// millisecond FIXM fields for source provenance, while state-age and availability checks use
/// the OAF nanosecond clock without rounding.
pub fn compare_row_asof_at_receive_ns(
    source: &SourceReference,
    receive_ns: i64,
    candidate_gufi: u128,
    candidate_address: u32,
    position: Option<&CandidatePosition>,
    model: &TrainingModel,
) -> RowResult {
    compare_row_with_age(
        source,
        candidate_gufi,
        candidate_address,
        position,
        model,
        Some(ASOF_MAX_STATE_AGE_NS),
        Some(receive_ns),
    )
}

fn compare_row_with_age(
    source: &SourceReference,
    candidate_gufi: u128,
    candidate_address: u32,
    position: Option<&CandidatePosition>,
    model: &TrainingModel,
    max_state_age_ns: Option<i64>,
    receive_ns_override: Option<i64>,
) -> RowResult {
    let mut result = EMPTY_RESULT;
    result.gufi = source.gufi;
    result.address = candidate_address;
    let Some(source_ns) = source.source_time_ms.checked_mul(1_000_000) else {
        result.disposition = Disposition::InvalidTime;
        return result;
    };
    let Some(receive_ns) =
        receive_ns_override.or_else(|| source.receive_time_ms.checked_mul(1_000_000))
    else {
        result.disposition = Disposition::InvalidTime;
        return result;
    };
    result.source_time_ns = source_ns;
    result.receive_ns = receive_ns;
    if source.source_time_ms < 0 || source.receive_time_ms < 0 || receive_ns < source_ns {
        result.disposition = Disposition::InvalidTime;
        return result;
    }
    if source.gufi == 0
        || candidate_gufi != source.gufi
        || candidate_address == 0
        || candidate_address > 0x00ff_ffff
    {
        result.disposition = Disposition::IdentityMismatch;
        return result;
    }
    let Some(position) = position else {
        result.disposition = Disposition::CandidateUnavailable;
        return result;
    };
    if position.address != candidate_address {
        result.disposition = Disposition::IdentityMismatch;
        return result;
    }
    if position.state_ns < 0
        || position.newest_source_ns < 0
        || position.newest_source_ns > position.state_ns
    {
        result.disposition = Disposition::InvalidTime;
        return result;
    }
    result.state_ns = position.state_ns;
    match max_state_age_ns {
        None => {
            if position.state_ns != receive_ns
                || position
                    .newest_available_ns
                    .is_none_or(|available| available < 0 || available > position.state_ns)
            {
                result.disposition = Disposition::InvalidTime;
                return result;
            }
        }
        Some(max_age) => {
            if position.state_ns > receive_ns {
                result.disposition = Disposition::CandidateStateFuture;
                return result;
            }
            let Some(age_ns) = receive_ns.checked_sub(position.state_ns) else {
                result.disposition = Disposition::InvalidTime;
                return result;
            };
            if age_ns > max_age {
                result.disposition = Disposition::CandidateStateStale;
                return result;
            }
            let Some(available_ns) = position.newest_available_ns else {
                result.disposition = Disposition::CandidateAvailabilityUnknown;
                return result;
            };
            if available_ns < 0 || available_ns > position.state_ns {
                result.disposition = Disposition::InvalidTime;
                return result;
            }
            if available_ns > receive_ns {
                result.disposition = Disposition::CandidateStateFuture;
                return result;
            }
        }
    }
    if position.newest_source_ns < source_ns {
        result.disposition = Disposition::CandidatePositionStale;
        return result;
    }
    if !(-900_000_000..=900_000_000).contains(&position.lat_e7)
        || !(-1_800_000_000..=1_800_000_000).contains(&position.lon_e7)
    {
        result.disposition = Disposition::InvalidPosition;
        return result;
    }
    let Some(raw_altitude_ft) = position.altitude_ft else {
        result.disposition = Disposition::MissingAltitude;
        return result;
    };
    if !(-2_000..=100_000).contains(&raw_altitude_ft) {
        result.disposition = Disposition::InvalidAltitude;
        return result;
    }
    let Some(altitude_ft) = raw_altitude_ft.checked_add(2_000) else {
        result.disposition = Disposition::InvalidAltitude;
        return result;
    };
    let lat_bin = position.lat_e7.div_euclid(500_000);
    let lon_bin = position.lon_e7.div_euclid(500_000);
    let altitude_band = altitude_ft.div_euclid(2_000);
    result.lat_bin = lat_bin;
    result.lon_bin = lon_bin;
    result.altitude_band = altitude_band;
    let Some(source_owner) = source.published_owner.filter(|owner| {
        owner
            .get(..4)
            .is_some_and(|unit| !unit.iter().all(|byte| *byte == 0) && unit != b"???\0")
            && owner
                .get(4..)
                .is_some_and(|sector| !sector.iter().all(|byte| *byte == 0))
    }) else {
        result.disposition = Disposition::UnknownSourceOwner;
        return result;
    };
    result.source_owner = source_owner;
    let cell_key = (lat_bin, lon_bin, altitude_band);
    let start = model
        .labels
        .partition_point(|label| (label.lat_bin, label.lon_bin, label.altitude_band) < cell_key);
    let mut any_cell = false;
    let mut supported: Option<&CellLabel> = None;
    for label in model.labels.get(start..).unwrap_or(&[]) {
        if (label.lat_bin, label.lon_bin, label.altitude_band) != cell_key {
            break;
        }
        any_cell = true;
        if label.class == LabelClass::Contested {
            result.disposition = Disposition::Contested;
            return result;
        }
        if label.class == LabelClass::Supported {
            if supported.is_some() {
                result.disposition = Disposition::Contested;
                return result;
            }
            supported = Some(label);
        }
    }
    let Some(label) = supported else {
        result.disposition = if any_cell {
            Disposition::UnknownSupport
        } else {
            Disposition::NoTrainingCell
        };
        return result;
    };
    result.predicted_owner = label.owner;
    result.support_rows = label.rows;
    result.support_flights = label.flights;
    result.support_time_bins = label.time_bins;
    result.disposition = if label.owner == source_owner {
        Disposition::SupportedMatch
    } else {
        Disposition::SupportedMismatch
    };
    result
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn model_text() -> String {
        format!(
            "OA-G3-OCCUPANCY-1\ncomplete=true\ncompletion_scope=selected-window\nclaim=empirical_observed_occupancy_only_not_legal_geometry\nsource_schema=H2-OBSERVED-STATE-1\nscope=SFDPS-replay-compatibility\nindependent_predictions=not-measured\nindependent_agreement_denominator=not-observable\nsource_reconstruction_claim=none_exact_trace_and_binary_reproducibility_only\ntraining_partition=2026-11-15\nsource_revision=fixture\ngrid_e7=500000\naltitude_slab_ft=2000\ntime_bin_ms=900000\ngrid_scope=global_wgs84_no_official_coverage_mask\nminimum_flights=3\nminimum_time_bins=2\nsupport_is_not_probability=true\ntraining_sha256={}\nbinary_sha256={}\npolicy_sha256={}\ncell\t820\t-1740\t16\t5a58584130313032\t30\t5\t4\nclass\t820\t-1740\t16\t5a58584130313032\tsupported\n",
            "02".repeat(32),
            "03".repeat(32),
            "04".repeat(32)
        )
    }

    // REQ: OA-MIG-409
    #[test]
    fn g3_model_parser_binds_the_full_artifact_and_fixed_development_split() {
        let text = model_text();
        let artifact = oa_sha256::to_hex(&oa_sha256::sha256(text.as_bytes()));
        let parsed = parse_training_model(
            text.as_bytes(),
            &artifact,
            &"02".repeat(32),
            "fixture",
            &"03".repeat(32),
            &"04".repeat(32),
        );
        assert_eq!(parsed.map(|model| model.labels.len()), Ok(1));

        let wrong_training_hash = parse_training_model(
            text.as_bytes(),
            &artifact,
            &"05".repeat(32),
            "fixture",
            &"03".repeat(32),
            &"04".repeat(32),
        );
        assert_eq!(wrong_training_hash, Err(ModelError::MetadataMismatch));

        let duplicated = format!("{text}{}", text.lines().last().unwrap_or(""));
        let digest = oa_sha256::to_hex(&oa_sha256::sha256(duplicated.as_bytes()));
        let duplicate_result = parse_training_model(
            duplicated.as_bytes(),
            &digest,
            &"02".repeat(32),
            "fixture",
            &"03".repeat(32),
            &"04".repeat(32),
        );
        assert_eq!(duplicate_result, Err(ModelError::DuplicateRow));
    }

    fn model() -> TrainingModel {
        TrainingModel {
            artifact_sha256: [1; 32],
            training_sha256: [2; 32],
            binary_sha256: [3; 32],
            policy_sha256: [4; 32],
            source_revision: "fixture".into(),
            labels: vec![CellLabel {
                lat_bin: 820,
                lon_bin: -1740,
                altitude_band: 16,
                owner: *b"ZXXA0102",
                class: LabelClass::Supported,
                rows: 30,
                flights: 5,
                time_bins: 4,
            }],
        }
    }

    fn position() -> CandidatePosition {
        CandidatePosition {
            address: 0x123456,
            state_ns: 1_100_000_000,
            newest_source_ns: 1_090_000_000,
            newest_available_ns: Some(1_100_000_000),
            lat_e7: 410_000_000,
            lon_e7: -870_000_000,
            altitude_ft: Some(30_000),
        }
    }

    // REQ: OA-MIG-409
    #[test]
    fn g3_result_is_explicitly_occupancy_proxy_with_support_denominators() {
        // This synthetic fixture tests only stable binning and status conservation. The
        // coordinates are replaced below with bins represented by the synthetic model.
        let mut point = position();
        point.lat_e7 = 410_250_000;
        point.lon_e7 = -869_750_000;
        let result = compare_row(
            &SourceReference {
                gufi: 7,
                source_time_ms: 1_000,
                receive_time_ms: 1_100,
                published_owner: Some(*b"ZXXA0102"),
            },
            7,
            0x123456,
            Some(&point),
            &model(),
        );
        assert_eq!(result.disposition, Disposition::SupportedMatch);
        assert_eq!(result.support_flights, 5);
        assert_eq!(result.support_time_bins, 4);
        assert_eq!(result.predicted_owner, *b"ZXXA0102");

        let mismatched_source = SourceReference {
            gufi: 7,
            source_time_ms: 1_000,
            receive_time_ms: 1_100,
            published_owner: Some(*b"ZXXA0104"),
        };
        let mismatch = compare_row(&mismatched_source, 7, 0x123456, Some(&point), &model());
        assert_eq!(mismatch.disposition, Disposition::SupportedMismatch);
    }

    // REQ: OA-MIG-412
    #[test]
    fn v2_asof_uses_only_pre_receive_state_and_enforces_one_second_age() {
        let source = SourceReference {
            gufi: 7,
            source_time_ms: 1_000,
            receive_time_ms: 2_100,
            published_owner: Some(*b"ZXXA0102"),
        };
        let mut point = position();
        point.lat_e7 = 410_250_000;
        point.lon_e7 = -869_750_000;
        point.state_ns = 1_100_000_000;
        point.newest_source_ns = 1_090_000_000;
        point.newest_available_ns = Some(1_100_000_000);
        let exact_age = compare_row_asof(&source, 7, 0x123456, Some(&point), &model());
        assert_eq!(exact_age.disposition, Disposition::SupportedMatch);
        assert_eq!(exact_age.state_ns, 1_100_000_000);
        let just_over_exact_age = compare_row_asof_at_receive_ns(
            &source,
            2_100_000_001,
            7,
            0x123456,
            Some(&point),
            &model(),
        );
        assert_eq!(
            just_over_exact_age.disposition,
            Disposition::CandidateStateStale
        );

        point.state_ns = 1_099_999_999;
        point.newest_source_ns = 1_090_000_000;
        point.newest_available_ns = Some(1_099_999_999);
        let stale = compare_row_asof(&source, 7, 0x123456, Some(&point), &model());
        assert_eq!(stale.disposition, Disposition::CandidateStateStale);

        point.state_ns = 2_101_000_000;
        point.newest_source_ns = 2_100_000_000;
        point.newest_available_ns = Some(2_101_000_000);
        let future = compare_row_asof(&source, 7, 0x123456, Some(&point), &model());
        assert_eq!(future.disposition, Disposition::CandidateStateFuture);

        point.state_ns = 1_100_000_000;
        point.newest_source_ns = 1_090_000_000;
        point.newest_available_ns = None;
        let unknown_availability = compare_row_asof(&source, 7, 0x123456, Some(&point), &model());
        assert_eq!(
            unknown_availability.disposition,
            Disposition::CandidateAvailabilityUnknown
        );
    }

    // REQ: OA-MIG-409
    #[test]
    fn contested_cells_never_produce_a_supported_match() {
        let mut evidence = model();
        evidence.labels.push(CellLabel {
            lat_bin: 820,
            lon_bin: -1740,
            altitude_band: 16,
            owner: *b"ZXXA0103",
            class: LabelClass::Contested,
            rows: 10,
            flights: 3,
            time_bins: 2,
        });
        let mut point = position();
        point.lat_e7 = 410_250_000;
        point.lon_e7 = -869_750_000;
        let result = compare_row(
            &SourceReference {
                gufi: 7,
                source_time_ms: 1_000,
                receive_time_ms: 1_100,
                published_owner: Some(*b"ZXXA0102"),
            },
            7,
            0x123456,
            Some(&point),
            &evidence,
        );
        assert_eq!(result.disposition, Disposition::Contested);
        assert_eq!(result.support_flights, 0);
    }

    // REQ: OA-MIG-409
    #[test]
    fn every_row_disposition_reconciles_in_the_shared_denominator() {
        let mut point = position();
        point.lat_e7 = 410_250_000;
        point.lon_e7 = -869_750_000;
        let source = SourceReference {
            gufi: 7,
            source_time_ms: 1_000,
            receive_time_ms: 1_100,
            published_owner: Some(*b"ZXXA0102"),
        };
        let matched = compare_row(&source, 7, 0x123456, Some(&point), &model());
        let missing = compare_row(&source, 7, 0x123456, None, &model());
        let mut counts = OutcomeCounts::default();
        assert!(counts.record(&matched).is_ok());
        assert!(counts.record(&missing).is_ok());
        assert_eq!(counts.total, 2);
        assert_eq!(counts.supported_match, 1);
        assert_eq!(counts.candidate_unavailable, 1);
        assert!(counts.reconciles());
    }

    // REQ: OA-MIG-409
    #[test]
    fn wrong_identity_unavailable_clock_and_missing_altitude_are_not_agreements() {
        let mut point = position();
        point.lat_e7 = 410_250_000;
        point.lon_e7 = -869_750_000;
        let source = SourceReference {
            gufi: 7,
            source_time_ms: 1_000,
            receive_time_ms: 1_100,
            published_owner: Some(*b"ZXXA0102"),
        };
        let identity = compare_row(&source, 8, 0x123456, Some(&point), &model());
        assert_eq!(identity.disposition, Disposition::IdentityMismatch);
        point.address = 0x654321;
        let wrong_address = compare_row(&source, 7, 0x123456, Some(&point), &model());
        assert_eq!(wrong_address.disposition, Disposition::IdentityMismatch);
        point.address = 0x123456;
        point.newest_available_ns = None;
        let unknown_clock = compare_row(&source, 7, 0x123456, Some(&point), &model());
        assert_eq!(unknown_clock.disposition, Disposition::InvalidTime);
        point.newest_available_ns = Some(1_100_000_000);
        point.altitude_ft = None;
        let no_altitude = compare_row(&source, 7, 0x123456, Some(&point), &model());
        assert_eq!(no_altitude.disposition, Disposition::MissingAltitude);
        point.altitude_ft = Some(30_000);
        point.newest_source_ns = 999_000_000;
        let stale = compare_row(&source, 7, 0x123456, Some(&point), &model());
        assert_eq!(stale.disposition, Disposition::CandidatePositionStale);
    }
}
