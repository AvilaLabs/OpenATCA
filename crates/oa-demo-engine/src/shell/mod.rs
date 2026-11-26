//! Class: shell (RULESET B1, B7). Validates a complete prepared OASD artifact before constructing
//! pure engine state. File reads belong to the native runner binary; this API accepts bounded bytes.

use crate::core::{DemoEngine, DemoEngineConfig, EngineError, NORMAL_SAMPLE_INTERVAL_MS};
use oa_demo_scenario::core::{SyntheticReport, NORMAL_ORD_DURATION_MS, NORMAL_ORD_SEED};
use oa_demo_scenario::shell::ScenarioArtifact;
use std::vec::Vec;

const MAX_OASD_BYTES: usize = 16 * 1024 * 1024;
const NORMAL_SCENARIO_ID: &str = "ord-normal-push-v1";

/// Validates the complete asset hash and OASD structure, then builds the pure engine from the
/// rows and metadata in that exact artifact. The same bytes are hashed before they are parsed.
pub fn from_oasd(bytes: &[u8], expected_asset_sha256: [u8; 32]) -> Result<DemoEngine, EngineError> {
    if bytes.is_empty() || bytes.len() > MAX_OASD_BYTES {
        return Err(EngineError::Scenario(
            "OASD asset is empty or exceeds 16 MiB",
        ));
    }
    if oa_sha256::core::sha256::sha256(bytes) != expected_asset_sha256 {
        return Err(EngineError::Scenario("OASD complete-file SHA-256 mismatch"));
    }
    let artifact = ScenarioArtifact::parse(bytes)
        .map_err(|_| EngineError::Scenario("OASD parser refused the artifact"))?;
    let header = &artifact.header;
    if header.scenario_id != NORMAL_SCENARIO_ID
        || header.seed != NORMAL_ORD_SEED
        || !matches!(header.duration_ms, NORMAL_ORD_DURATION_MS | 3_600_000)
        || header.sample_interval_ms != NORMAL_SAMPLE_INTERVAL_MS
    {
        return Err(EngineError::Scenario(
            "OASD header does not match the frozen normal-push profile",
        ));
    }
    let mut reports = Vec::with_capacity(usize::try_from(header.record_count).unwrap_or(0));
    for row in artifact.records() {
        let row = row.map_err(|_| EngineError::Scenario("OASD report record refused"))?;
        reports.push(SyntheticReport {
            aircraft_index: row.aircraft_index,
            flags: row.flags,
            virtual_time_ms: row.virtual_time_ms,
            lat_e7: row.lat_e7,
            lon_e7: row.lon_e7,
            altitude_ft: row.altitude_ft,
            ground_speed_dkt: row.ground_speed_dkt,
            track_ddeg: row.track_ddeg,
            vertical_rate_fpm: row.vertical_rate_fpm,
        });
    }
    if reports.len() != usize::try_from(header.record_count).unwrap_or(usize::MAX) {
        return Err(EngineError::Scenario("OASD record count mismatch"));
    }
    let config = DemoEngineConfig::normal_ord(
        header.seed,
        header.duration_ms,
        header.epoch_unix_ns,
        header.sample_interval_ms,
        header.center_lat_e7,
        header.center_lon_e7,
    );
    DemoEngine::from_reports(config, reports)
}
