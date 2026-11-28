//! Regional MSAW composition (shell, B1/B3/C6). The tracker epoch and measured altitude age
//! are supplied by surveillance; route intent never changes the warning inputs.

use oa_msaw::{
    alert_log_clear, alert_log_new, msaw_evaluate, msaw_new, msaw_open, msaw_plan, msaw_points,
    skip_log_clear, skip_log_new, Alert, AlertLog, CycleStats, FloorSample, MsawState, MsawTrack,
    SkipLog, Skipped, FLOOR_NONE, F_COLD_CORRECTED, F_COLD_HIGH, F_COLD_UNKNOWN, F_QNH_MISSING,
};
use oa_msaw_run::config::Settings;
use oa_msaw_run::floors::Floors;
use oa_msaw_run::qnh::Qnh;
use oa_msaw_run::regional::RegionalData;
use oa_safety::inhibit::InhibitTable;
use oa_safety::net::{NetReport, StateTable};
use std::path::{Path, PathBuf};

const MAX_REGIONAL_MSAW_SAMPLES: usize = 64;
const MAX_REGIONAL_MSAW_LOGS: usize = 4096;
const MAX_REGIONAL_MSAW_ROWS: usize = 4096;
const MAX_REGIONAL_MSAW_CYCLES: usize = crate::surveillance::MAX_OFFLINE_FRAMES;
const MAX_REGIONAL_MSAW_CORE_TRACKS: usize = 8192;
const MAX_REGIONAL_MSAW_OPEN: usize = 4096;

/// Explicit, bounded startup manifest. Observation time is never substituted for a METAR
/// file's actual receipt time. Relative paths resolve beside this manifest.
pub fn load_msaw_manifest(path: &Path, cutoff_ms: i64) -> Result<(RegionalData, [u8; 32]), String> {
    use oa_msaw_run::regional::{
        load_regional_data, MetarSource, RegionalDataLimits, RegionalDataPaths, WeatherAdmission,
    };
    let bytes = crate::cli::read_guarded(path, 64 * 1024)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| "MSAW manifest is not UTF-8")?;
    let mut lines = text.lines();
    if lines.next() != Some("OA-MSAW-INPUTS\t1") {
        return Err("expected OA-MSAW-INPUTS header".into());
    }
    let parent = path.parent().unwrap_or(Path::new("."));
    let resolve = |value: &str| -> Result<PathBuf, String> {
        if value.is_empty() || value.len() > 1024 || value.chars().any(char::is_control) {
            return Err("MSAW input path is empty, too long or contains control bytes".into());
        }
        let candidate = parent.join(value);
        oa_sealed::refuse(&candidate)?;
        Ok(candidate)
    };
    let mut config = None;
    let mut terrain_dir = None;
    let mut dof_files = Vec::new();
    let mut metar_files = Vec::new();
    for (index, line) in lines.enumerate() {
        if index >= 66 || line.len() > 1100 {
            return Err("MSAW manifest row/byte budget exceeded".into());
        }
        let fields: Vec<_> = line.split('\t').collect();
        match fields.as_slice() {
            ["CONFIG", name] if config.is_none() => config = Some(resolve(name)?),
            ["TERRAIN", name] if terrain_dir.is_none() => terrain_dir = Some(resolve(name)?),
            ["DOF", name] if dof_files.len() < 32 => {
                let candidate = resolve(name)?;
                if dof_files.contains(&candidate) {
                    return Err("duplicate DOF input".into());
                }
                dof_files.push(candidate);
            }
            ["METAR", received, name] if metar_files.len() < 32 => {
                let received_at_ms = received
                    .parse::<i64>()
                    .map_err(|_| "invalid METAR receipt timestamp")?;
                if received_at_ms > cutoff_ms {
                    return Err("METAR receipt is after first regional cutoff".into());
                }
                let candidate = resolve(name)?;
                if metar_files
                    .iter()
                    .any(|source: &MetarSource| source.path == candidate)
                {
                    return Err("duplicate METAR input".into());
                }
                metar_files.push(MetarSource {
                    path: candidate,
                    received_at_ms,
                });
            }
            _ => {
                return Err(format!(
                    "invalid or duplicate MSAW manifest row {}",
                    index + 2
                ))
            }
        }
    }
    let paths = RegionalDataPaths {
        config: config.ok_or("MSAW configuration is required")?,
        terrain_dir,
        dof_files,
        metar_files,
    };
    let data = load_regional_data(
        &paths,
        RegionalDataLimits::default(),
        WeatherAdmission { cutoff_ms },
    )?;
    Ok((data, oa_sha256::sha256(&bytes)))
}

// A7.2: one cold owned offline composition; subsystem state remains owned by its core.
pub struct RegionalMsaw {
    state: MsawState,
    settings: Settings,
    floors: Option<Floors>,
    qnh: Qnh,
    synthetic: bool,
    provenance_sha256: [u8; 32],
    alerts: AlertLog,
    skips: SkipLog,
    rows: Vec<MsawTrack>,
    samples: Vec<FloorSample>,
    last_ns: Option<i64>,
    max_rows: usize,
    max_points: usize,
    max_cycles: usize,
    cycles: usize,
    failed: bool,
    admitted_cutoff_ms: i64,
}

// A3/C6: bounded cold frame evidence; all rows are copied before the core logs are drained.
#[derive(Clone, Debug, PartialEq)]
pub struct RegionalMsawFrame {
    pub time_ns: i64,
    pub report: NetReport,
    pub stats: Option<CycleStats>,
    pub open: Vec<Alert>,
    pub closed: Vec<Alert>,
    pub skipped: Vec<Skipped>,
    pub provenance_sha256: [u8; 32],
    pub synthetic: bool,
    /// Submitted tracker rows, including missing/stale-altitude rows counted by the core.
    pub submitted_rows: usize,
    pub vfr_service_eligibility_available: bool,
    pub terrain_tiles: Vec<(String, [u8; 32])>,
    pub failure: String,
    pub planned_points: usize,
    pub completed_floor_samples: usize,
    pub evaluation_attempted: bool,
}

fn assemble(
    settings: Settings,
    floors: Option<Floors>,
    qnh: Qnh,
    provenance_sha256: [u8; 32],
    synthetic: bool,
    max_rows: usize,
    max_cycles: usize,
) -> Result<RegionalMsaw, String> {
    if max_rows == 0 || max_rows > MAX_REGIONAL_MSAW_ROWS {
        return Err("MSAW regional row capacity is outside the configured session envelope".into());
    }
    if max_cycles == 0 || max_cycles > MAX_REGIONAL_MSAW_CYCLES {
        return Err(
            "MSAW regional cycle capacity is outside the configured session envelope".into(),
        );
    }
    if settings.msaw.max_tracks > MAX_REGIONAL_MSAW_CORE_TRACKS
        || settings.msaw.max_open > MAX_REGIONAL_MSAW_OPEN
        || settings.msaw.track_budget > MAX_REGIONAL_MSAW_ROWS
        || settings.msaw.samples > MAX_REGIONAL_MSAW_SAMPLES
        || settings.alert_log_capacity > MAX_REGIONAL_MSAW_LOGS
        || settings.skip_log_capacity > MAX_REGIONAL_MSAW_LOGS
    {
        return Err("MSAW configuration exceeds regional row/sample/log budgets".into());
    }
    // These settings need the corresponding coordinator service, not silent omission.
    if !settings.inhibits.is_empty() || !settings.requests.is_empty() {
        return Err(
            "configured MSAW inhibitions/service requests require an authority adapter".into(),
        );
    }
    if !settings.cifp_airports.is_empty() || settings.cifp_area_margin_nm.is_some() {
        return Err("automatic CIFP MSAW adaptation requires the corridor-loading adapter".into());
    }
    let max_points = max_rows
        .checked_mul(settings.msaw.samples)
        .ok_or("MSAW point budget overflow")?;
    let state = msaw_new(&settings.msaw).map_err(str::to_owned)?;
    let alerts = alert_log_new(settings.alert_log_capacity).map_err(str::to_owned)?;
    let skips = skip_log_new(settings.skip_log_capacity).map_err(str::to_owned)?;
    Ok(RegionalMsaw {
        state,
        settings,
        floors,
        qnh,
        synthetic,
        provenance_sha256,
        alerts,
        skips,
        rows: Vec::with_capacity(max_rows),
        samples: Vec::with_capacity(max_points),
        last_ns: None,
        max_rows,
        max_points,
        max_cycles,
        cycles: 0,
        failed: false,
        admitted_cutoff_ms: 0,
    })
}

pub fn regional_msaw_new(data: RegionalData, max_rows: usize) -> Result<RegionalMsaw, String> {
    regional_msaw_new_for_profile(data, max_rows, crate::surveillance::MAX_FRAMES)
}

pub fn regional_msaw_new_for_profile(
    data: RegionalData,
    max_rows: usize,
    max_cycles: usize,
) -> Result<RegionalMsaw, String> {
    let admitted_cutoff_ms = data.provenance.cutoff_ms;
    let mut service = assemble(
        data.settings,
        Some(data.floors),
        data.qnh,
        data.provenance.manifest_sha256,
        false,
        max_rows,
        max_cycles,
    )?;
    service.admitted_cutoff_ms = admitted_cutoff_ms;
    Ok(service)
}

/// Explicit public test model: a flat 500-ft floor in the Chicago test box and one synthetic
/// 15-C/1013.25-hPa station at elevation zero, observed at Unix second zero. No FAA inputs.
pub fn regional_msaw_synthetic(max_rows: usize) -> Result<RegionalMsaw, String> {
    regional_msaw_synthetic_for_profile(max_rows, crate::surveillance::MAX_FRAMES)
}

pub fn regional_msaw_synthetic_for_profile(
    max_rows: usize,
    max_cycles: usize,
) -> Result<RegionalMsaw, String> {
    let config = format!(
        "look_ahead_s=60\nwarning_s=20\nsamples=13\nhysteresis_s=3\ngtm_clearance_ft=500\nobstacle_clearance_ft=500\nmax_tracks={max_rows}\nmax_open={max_rows}\ntrack_budget={max_rows}\nmax_speed_mps=400\nwide_degrade_share=0.2\nnet_stale_limit_s=10\nnet_future_horizon_s=0\nalert_log_capacity=64\nskip_log_capacity=4096\ninhibit_max_duration_s=3600\ninhibit_max_start_ahead_s=0\ninhibit_max_extent_deg=2\napproach_half_width_nm=1\napproach_allowance_ft=100\napproach_min_clearance_ft=100\ndeparture_min_clearance_ft=100\nvelocity_uncertainty_mps=0\nmax_altitude_age_s=5\nstale_hold_s=30\naltitude_loss_degrade_share=0\nterminal_lookahead_s=60\ncapture_heading_deg=10\nno_qnh_allowance_ft=100\nmetar_radius_nm=50\nmetar_max_age_s=3600\nairport_area_nm=5\nairport_area_ceiling_ft_agl=3000\nairport_area_clearance_ft=200\n"
    );
    let settings = oa_msaw_run::config::parse_config(&config)?;
    let mut qnh = Qnh::from_stations(vec![(41.0, -87.0, vec![(0, 1013.25)])], 50.0, 3600);
    qnh.set_station_weather(0, 0.0, vec![15.0]);
    let mut provenance = b"OA_REGIONAL_SYNTHETIC_MSAW_V1\nflat=500ft\nbox=40,42,-88,-86\nstation=41,-87,0,1013.25,15\n".to_vec();
    provenance.extend_from_slice(config.as_bytes());
    assemble(
        settings,
        None,
        qnh,
        oa_sha256::sha256(&provenance),
        true,
        max_rows,
        max_cycles,
    )
}

pub fn regional_msaw_net_policy(service: &RegionalMsaw) -> (i64, i64) {
    (
        service.settings.net_stale_limit_ns,
        service.settings.net_future_horizon_ns,
    )
}

fn synthetic_floor(point: &oa_msaw::SamplePoint) -> FloorSample {
    // A conservative rectangular enclosure of the complete query circle. The longitude
    // scale is below the minimum in the supported 40–42N box, so it only enlarges the query.
    let lat = f64::from(point.lat_e7) / 1e7;
    let lon = f64::from(point.lon_e7) / 1e7;
    let radius = f64::from(point.radius_m);
    let covered = radius.is_finite()
        && radius >= 0.0
        && lat - radius / 110_000.0 >= 40.0
        && lat + radius / 110_000.0 <= 42.0
        && lon - radius / 82_000.0 >= -88.0
        && lon + radius / 82_000.0 <= -86.0;
    FloorSample {
        terrain_ft: if covered { 500 } else { FLOOR_NONE },
        obstacle_ft: FLOOR_NONE,
        obstacle_covered: covered,
        mva_ft: FLOOR_NONE,
        gtm_clearance_ft: FLOOR_NONE,
        apt_clearance_ft: FLOOR_NONE,
        apt_ceiling_ft: FLOOR_NONE,
    }
}

fn evaluate_cycle(
    service: &mut RegionalMsaw,
    time_ns: i64,
    pressure_rows: &[MsawTrack],
    states: &mut StateTable,
    inhibits: &mut InhibitTable,
) -> Result<RegionalMsawFrame, String> {
    if service.last_ns.is_some_and(|last| time_ns <= last) || time_ns <= 0 {
        return Err("MSAW cycle time must be positive and increase".into());
    }
    if service.cycles >= service.max_cycles {
        return Err("MSAW selected-profile cycle limit exceeded before evaluation".into());
    }
    if time_ns / 1_000_000 < service.admitted_cutoff_ms {
        return Err("MSAW input set was admitted after this frame cutoff".into());
    }
    if pressure_rows.len() > service.max_rows {
        return Err("MSAW row capacity exceeded before evaluation".into());
    }
    if pressure_rows.windows(2).any(|rows| {
        rows.first()
            .zip(rows.get(1))
            .is_none_or(|(a, b)| a.addr >= b.addr)
    }) {
        return Err("MSAW rows must have distinct increasing full addresses".into());
    }
    service.last_ns = Some(time_ns);
    service.cycles += 1;
    service.rows.clear();
    service.samples.clear();
    alert_log_clear(&mut service.alerts);
    skip_log_clear(&mut service.skips);
    for &input in pressure_rows {
        let mut row = input;
        if row.alt_ft.is_finite()
            && f64::from(row.alt_ft).abs() <= oa_msaw::MAX_ABS_ALT_FT
            && row.lat_deg.is_finite()
            && row.lat_deg.abs() <= oa_msaw::MAX_ABS_LAT_DEG
            && row.lon_deg.is_finite()
            && row.lon_deg.abs() <= 180.0
        {
            let (altitude, missing) = service.qnh.datum_ft(
                row.lat_deg,
                row.lon_deg,
                time_ns / 1_000_000_000,
                f64::from(row.alt_ft),
                service.settings.no_qnh_allowance_ft,
            );
            if !altitude.is_finite() || altitude.abs() > oa_msaw::MAX_ABS_ALT_FT {
                return Err("unrepresentable MSAW datum correction".into());
            }
            row.alt_ft = altitude as f32;
            if missing {
                row.flags |= F_QNH_MISSING;
            }
            let (cold_altitude, flag) =
                service
                    .qnh
                    .cold_ft(row.lat_deg, row.lon_deg, time_ns / 1_000_000_000, altitude);
            if !cold_altitude.is_finite() || cold_altitude.abs() > oa_msaw::MAX_ABS_ALT_FT {
                return Err("unrepresentable MSAW temperature correction".into());
            }
            row.alt_ft = cold_altitude as f32;
            row.flags |= match flag {
                1 => F_COLD_CORRECTED,
                2 => F_COLD_UNKNOWN,
                3 => F_COLD_HIGH,
                _ => 0,
            };
        }
        service.rows.push(row);
    }
    msaw_plan(
        &mut service.state,
        time_ns,
        &service.rows,
        &mut service.skips,
    );
    let points = msaw_points(&service.state);
    let planned_points = points.len();
    if points.len() > service.max_points {
        return Err("MSAW core exceeded the preflight point budget".into());
    }
    for point in points {
        let sample = if service.synthetic {
            synthetic_floor(point)
        } else {
            service
                .floors
                .as_mut()
                .ok_or("MSAW floor service absent")?
                .floor_at(point)?
        };
        service.samples.push(sample);
    }
    let cycle = msaw_evaluate(
        &mut service.state,
        &service.samples,
        &mut service.alerts,
        &mut service.skips,
        states,
        inhibits,
    );
    let terrain_tiles = service
        .floors
        .as_ref()
        .map_or_else(Vec::new, |floors| floors.terrain_hashes().to_vec());
    Ok(RegionalMsawFrame {
        time_ns,
        report: cycle.report,
        stats: Some(cycle.stats),
        open: msaw_open(&service.state).to_vec(),
        closed: service.alerts.items.clone(),
        skipped: service.skips.items.clone(),
        provenance_sha256: service.provenance_sha256,
        synthetic: service.synthetic,
        submitted_rows: pressure_rows.len(),
        vfr_service_eligibility_available: false,
        terrain_tiles,
        failure: String::new(),
        planned_points,
        completed_floor_samples: service.samples.len(),
        evaluation_attempted: true,
    })
}

/// An I/O/provider failure isolates this net. Observed CA, tracking and presentation keep
/// running while MSAW publishes unavailable every cycle; no partial evaluation is called safe.
pub fn regional_msaw_cycle(
    service: &mut RegionalMsaw,
    time_ns: i64,
    pressure_rows: &[MsawTrack],
    states: &mut StateTable,
    inhibits: &mut InhibitTable,
) -> Result<RegionalMsawFrame, String> {
    if time_ns <= 0 || service.last_ns.is_some_and(|last| time_ns <= last) {
        return Err("MSAW report time is not a valid advancing regional cutoff".into());
    }
    alert_log_clear(&mut service.alerts);
    skip_log_clear(&mut service.skips);
    service.samples.clear();
    let attempted = !service.failed;
    let result = if service.failed {
        Err("MSAW service failed; reconstruct before resuming evaluation".into())
    } else {
        evaluate_cycle(service, time_ns, pressure_rows, states, inhibits)
    };
    match result {
        Ok(frame) => Ok(frame),
        Err(reason) => {
            service.failed = true;
            service.last_ns = Some(time_ns);
            if !oa_msaw::msaw_publish_refused(states, time_ns) {
                return Err("regional safety table refused an MSAW unavailable report".into());
            }
            Ok(RegionalMsawFrame {
                time_ns,
                report: NetReport::Unavailable {
                    reason: oa_safety::net::Reason::InputMissing,
                },
                stats: None,
                open: msaw_open(&service.state).to_vec(),
                closed: service.alerts.items.clone(),
                skipped: service.skips.items.clone(),
                provenance_sha256: service.provenance_sha256,
                synthetic: service.synthetic,
                submitted_rows: pressure_rows.len(),
                vfr_service_eligibility_available: false,
                terrain_tiles: service
                    .floors
                    .as_ref()
                    .map_or_else(Vec::new, |floors| floors.terrain_hashes().to_vec()),
                failure: reason,
                planned_points: if attempted {
                    msaw_points(&service.state).len()
                } else {
                    0
                },
                completed_floor_samples: service.samples.len(),
                evaluation_attempted: attempted,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    const LEGACY_MAX_ROWS: usize = 32;
    const LEGACY_CORE_TRACKS: usize = 16;

    fn track(alt_ft: f32, vrate_fpm: f32) -> MsawTrack {
        MsawTrack {
            lat_deg: 41.0,
            lon_deg: -87.0,
            addr: 0xabc101,
            vel_east_mps: 0.0,
            vel_north_mps: 0.0,
            r95_m: 10.0,
            alt_ft,
            vrate_fpm,
            flags: 0,
            ground: oa_msaw::AIRBORNE,
            alt_age_s: 0.0,
            alt_change_10s_ft: f32::NAN,
            ground_ft: f32::NAN,
        }
    }

    // REQ: OA-REG-011
    #[test]
    fn actual_msaw_uses_datum_altitude_and_escalates_a_known_descent() {
        let mut service = regional_msaw_synthetic(32).unwrap();
        let mut states = oa_safety::net::state_table_new(10_000_000_000, 0).unwrap();
        let mut inhibits = oa_safety::inhibit::inhibits_new();
        // Flat terrain 500 + clearance500=1000ft. 1500ft descending600fpm reaches it
        // after50s: caution now, then warning after40s of consistent descent.
        let first = regional_msaw_cycle(
            &mut service,
            2_000_000_000,
            &[track(1500.0, -600.0)],
            &mut states,
            &mut inhibits,
        )
        .unwrap();
        assert_eq!(first.report, NetReport::Running);
        assert_eq!(first.open.len(), 1);
        assert_eq!(
            first.open[0].alt_ft, 1500.0,
            "cold_ft returns corrected altitude, not a delta"
        );
        assert_eq!(first.open[0].peak, oa_msaw::Grade::Caution);
        assert_eq!(first.open[0].warning_ns, None);
        assert_eq!(first.stats.unwrap().qnh_missing, 0);
        let later = regional_msaw_cycle(
            &mut service,
            42_000_000_000,
            &[track(1100.0, -600.0)],
            &mut states,
            &mut inhibits,
        )
        .unwrap();
        assert_eq!(later.open[0].peak, oa_msaw::Grade::Warning);
        assert_eq!(later.open[0].warning_ns, Some(42_000_000_000));
        assert_eq!(later.planned_points, later.completed_floor_samples);
        assert!(later.synthetic);
    }

    // REQ: OA-REG-011
    #[test]
    fn missing_coverage_and_old_altitude_are_visible_protection_gaps() {
        let mut service = regional_msaw_synthetic(32).unwrap();
        let mut states = oa_safety::net::state_table_new(10_000_000_000, 0).unwrap();
        let mut inhibits = oa_safety::inhibit::inhibits_new();
        let mut outside = track(1500.0, -600.0);
        outside.lat_deg = 43.0;
        let frame = regional_msaw_cycle(
            &mut service,
            2_000_000_000,
            &[outside],
            &mut states,
            &mut inhibits,
        )
        .unwrap();
        assert_ne!(frame.report, NetReport::Running);
        assert!(frame.stats.unwrap().no_coverage > 0);
        let mut stale = track(1500.0, -600.0);
        stale.alt_age_s = 31.0;
        let frame = regional_msaw_cycle(
            &mut service,
            3_000_000_000,
            &[stale],
            &mut states,
            &mut inhibits,
        )
        .unwrap();
        assert!(frame.stats.unwrap().alt_stale > 0);
        assert_ne!(frame.report, NetReport::Running);
    }

    fn regional(service: RegionalMsaw) -> crate::runtime::RegionalRuntime {
        let model = crate::cli::synthetic_model().unwrap();
        let sha = oa_sha256::sha256(&oa_airspace::core::snapshot::encode(&model).unwrap());
        let mut runtime = crate::runtime::regional_new(
            model,
            sha,
            *b"ZAU1",
            crate::surveillance::SurveillanceConfig {
                centre_lat_e7: 410_000_000,
                centre_lon_e7: -870_000_000,
                range_nm: 180.0,
            },
        )
        .unwrap();
        crate::runtime::regional_attach_msaw(&mut runtime, service).unwrap();
        runtime
    }

    fn receipt(second: i64, altitude: Option<i32>) -> crate::ReceiptBatch {
        crate::ReceiptBatch {
            receipt_ns: second * 1_000_000_000,
            source_clock_regressions: 0,
            events: vec![crate::NormalizedEvent {
                receipt_ns: second * 1_000_000_000,
                source_ns: second * 1_000_000_000,
                sequence: second as u64,
                source_message_id: None,
                source: crate::SourceKind::Surveillance,
                identity: crate::Identity {
                    flight_gufi: 0,
                    aircraft_address: 0xabc101,
                },
                centre: *b"ZAU1",
                kind: crate::EventKind::Position {
                    latitude_e7: 410_000_000,
                    longitude_e7: -870_000_000,
                    altitude_ft: altitude,
                    east_mmps: 0,
                    north_mmps: 0,
                    vertical_rate_fpm: Some(-600),
                    track_callsign: None,
                },
            }],
        }
    }

    fn development_provider_data() -> (RegionalData, PathBuf) {
        use oa_msaw_run::regional::{
            load_regional_data, RegionalDataLimits, RegionalDataPaths, WeatherAdmission,
        };

        let root =
            std::env::temp_dir().join(format!("oa-regional-msaw-provider-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("tiles")).unwrap();
        let config = root.join("dev.cfg");
        let config_text = include_str!("../../oa-msaw/testdata/dev.cfg")
            .replace("alert_log_capacity = 262144", "alert_log_capacity = 4096")
            .replace("skip_log_capacity = 262144", "skip_log_capacity = 4096");
        std::fs::write(&config, config_text).unwrap();
        let data = load_regional_data(
            &RegionalDataPaths {
                config,
                terrain_dir: Some(root.join("tiles")),
                dof_files: Vec::new(),
                metar_files: Vec::new(),
            },
            RegionalDataLimits::default(),
            WeatherAdmission { cutoff_ms: 0 },
        )
        .unwrap();
        (data, root)
    }

    fn legacy_row_capacity_data() -> (RegionalData, PathBuf) {
        let (mut data, root) = development_provider_data();
        data.settings.msaw.max_tracks = LEGACY_CORE_TRACKS;
        data.settings.msaw.max_open = LEGACY_MAX_ROWS;
        data.settings.msaw.track_budget = LEGACY_MAX_ROWS;
        (data, root)
    }

    // Mutation killed: reject the previously valid shared bridge configuration when its input
    // row allowance is larger than the core's independently bounded tracker table.
    // REQ: OA-REG-011
    #[test]
    fn legacy_bridge_accepts_row_capacity_larger_than_core_track_capacity() {
        let (data, root) = legacy_row_capacity_data();
        let mut service = regional_msaw_new(data, LEGACY_MAX_ROWS).unwrap();
        let tracks: Vec<_> = (0..LEGACY_CORE_TRACKS + 1)
            .map(|index| {
                let mut row = track(1500.0, -600.0);
                row.addr = 0xabc101 + u32::try_from(index).unwrap();
                row
            })
            .collect();
        let mut states = oa_safety::net::state_table_new(10_000_000_000, 0).unwrap();
        let mut inhibits = oa_safety::inhibit::inhibits_new();
        let frame = regional_msaw_cycle(
            &mut service,
            2_000_000_000,
            &tracks,
            &mut states,
            &mut inhibits,
        )
        .unwrap();

        assert_eq!(frame.submitted_rows, tracks.len());
        let stats = frame.stats.unwrap();
        assert_eq!(
            stats.rows_over_capacity,
            u64::try_from(tracks.len().saturating_sub(LEGACY_CORE_TRACKS)).unwrap()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    // REQ: OA-REG-011
    #[test]
    fn bridge_still_rejects_row_and_core_capacities_over_their_envelopes() {
        assert!(regional_msaw_synthetic_for_profile(
            MAX_REGIONAL_MSAW_ROWS + 1,
            crate::surveillance::MAX_FRAMES
        )
        .is_err());

        let (mut data, root) = development_provider_data();
        data.settings.msaw.max_tracks = MAX_REGIONAL_MSAW_CORE_TRACKS + 1;
        data.settings.msaw.max_open = LEGACY_MAX_ROWS;
        data.settings.msaw.track_budget = LEGACY_MAX_ROWS;
        assert!(regional_msaw_new(data, LEGACY_MAX_ROWS).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    fn bulk_receipt(second: i64, count: u32) -> crate::ReceiptBatch {
        let receipt_ns = second * 1_000_000_000;
        let events = (0..count)
            .map(|index| crate::NormalizedEvent {
                receipt_ns,
                source_ns: receipt_ns,
                sequence: u64::try_from((second - 1) * i64::from(count) + i64::from(index) + 1)
                    .unwrap(),
                source_message_id: None,
                source: crate::SourceKind::Surveillance,
                identity: crate::Identity {
                    flight_gufi: u128::from(index + 1),
                    aircraft_address: 0xabc000 + index,
                },
                centre: *b"ZAU1",
                kind: crate::EventKind::Position {
                    latitude_e7: 410_000_000,
                    longitude_e7: -870_000_000 + i32::try_from(index).unwrap() * 100_000,
                    altitude_ft: Some(10_000),
                    east_mmps: 0,
                    north_mmps: 0,
                    vertical_rate_fpm: Some(0),
                    track_callsign: None,
                },
            })
            .collect();
        crate::ReceiptBatch {
            receipt_ns,
            source_clock_regressions: 0,
            events,
        }
    }

    // Kills the hard-coded 32-row runtime MSAW attachment.
    // REQ: OA-REG-011
    #[test]
    fn recorded_profile_passes_more_than_default_rows_through_provider_path() {
        let track_count = crate::surveillance::MAX_TRACKS + 8;
        let pair_count = track_count * (track_count - 1) / 2;
        let (data, root) = development_provider_data();
        let model = crate::cli::synthetic_model().unwrap();
        let snapshot = oa_airspace::core::snapshot::encode(&model).unwrap();
        let mut profile = crate::runtime::DEFAULT_REGIONAL_RUN_PROFILE;
        profile.surveillance.max_tracks = track_count;
        // Flight evidence/controller views share the same selected offline population. Keeping
        // the default 32-flight cap while raising only surveillance rows refuses this intended
        // over-default provider-path case before MSAW receives its rows.
        profile.max_flights = track_count;
        profile.surveillance.max_frames = crate::surveillance::MAX_FRAMES + 1;
        profile.surveillance.max_open_alerts = pair_count;
        profile.surveillance.max_candidate_pairs = pair_count;
        let mut runtime = crate::runtime::regional_new_with_profile(
            model,
            oa_sha256::sha256(&snapshot),
            *b"ZAU1",
            crate::surveillance::SurveillanceConfig {
                centre_lat_e7: 410_000_000,
                centre_lon_e7: -870_000_000,
                range_nm: 180.0,
            },
            profile,
        )
        .unwrap();
        crate::runtime::regional_enable_msaw(&mut runtime, data).unwrap();

        for second in 1..=3 {
            crate::runtime::regional_step(
                &mut runtime,
                &bulk_receipt(second, u32::try_from(track_count).unwrap()),
            )
            .unwrap();
        }
        let batch = bulk_receipt(4, u32::try_from(track_count).unwrap());
        let expected_tracks = batch.events.len();
        let frame = crate::runtime::regional_step(&mut runtime, &batch).unwrap();
        assert_eq!(frame.surveillance.tracks.len(), expected_tracks);
        let msaw = frame.surveillance.msaw.as_ref().unwrap();
        assert_eq!(msaw.submitted_rows, expected_tracks);
        assert!(!msaw.synthetic);
        assert!(msaw.evaluation_attempted);
        let stats = msaw.stats.unwrap();
        assert!(stats.qnh_missing > 0, "absent weather stays flagged");
        assert!(
            stats.no_coverage > 0,
            "empty terrain fixture stays uncovered"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    // Kills the fixed 512-cycle MSAW bridge limit.
    // REQ: OA-REG-011
    #[test]
    fn selected_profile_may_run_more_than_default_bridge_cycles() {
        let selected_cycles = crate::surveillance::MAX_FRAMES + 1;
        let mut service = regional_msaw_synthetic_for_profile(1, selected_cycles).unwrap();
        let mut states = oa_safety::net::state_table_new(10_000_000_000, 0).unwrap();
        let mut inhibits = oa_safety::inhibit::inhibits_new();
        let row = track(15_000.0, 0.0);
        for second in 1..=i64::try_from(selected_cycles).unwrap() {
            regional_msaw_cycle(
                &mut service,
                second * 1_000_000_000,
                &[row],
                &mut states,
                &mut inhibits,
            )
            .unwrap();
        }
        assert_eq!(service.cycles, selected_cycles);
    }

    // REQ: OA-REG-011
    #[test]
    fn horizontal_reports_cannot_refresh_barometric_altitude_freshness() {
        let mut runtime = regional(regional_msaw_synthetic(32).unwrap());
        crate::runtime::regional_step(&mut runtime, &receipt(1, Some(1500))).unwrap();
        let confirmed =
            crate::runtime::regional_step(&mut runtime, &receipt(2, Some(1490))).unwrap();
        assert_eq!(
            confirmed
                .surveillance
                .msaw
                .as_ref()
                .unwrap()
                .stats
                .unwrap()
                .alt_age_unknown,
            0
        );
        let last = crate::runtime::regional_step(&mut runtime, &receipt(40, None)).unwrap();
        assert_eq!(
            last.surveillance.track_measurement_seen_ns,
            Some(40_000_000_000)
        );
        assert!(
            last.surveillance
                .msaw
                .as_ref()
                .unwrap()
                .stats
                .unwrap()
                .alt_stale
                > 0
        );
        assert!(crate::runtime::regional_pixels(&runtime).is_ok());
    }

    #[test]
    fn msaw_alert_state_survives_mode_withholding_and_recovers_with_fresh_track_data() {
        let mut runtime = regional(regional_msaw_synthetic(32).unwrap());
        crate::runtime::regional_step(&mut runtime, &receipt(1, Some(1500))).unwrap();
        let active = crate::runtime::regional_step(&mut runtime, &receipt(2, Some(1490))).unwrap();
        let active_msaw = active.surveillance.msaw.as_ref().unwrap();
        assert_eq!(
            active.surveillance.msaw_product,
            crate::surveillance::ProductAvailability::Available
        );
        assert!(!active_msaw.open.is_empty());
        let alert_start = active_msaw.open[0].start_ns;

        let outage = crate::runtime::regional_step(
            &mut runtime,
            &crate::ReceiptBatch {
                receipt_ns: 3_000_000_000,
                source_clock_regressions: 0,
                events: vec![crate::NormalizedEvent {
                    receipt_ns: 3_000_000_000,
                    source_ns: 3_000_000_000,
                    sequence: 3,
                    source_message_id: None,
                    source: crate::SourceKind::Surveillance,
                    identity: crate::Identity {
                        flight_gufi: 0,
                        aircraft_address: 0,
                    },
                    centre: *b"ZAU1",
                    kind: crate::EventKind::FeedState {
                        available: false,
                        reason: 0,
                    },
                }],
            },
        )
        .unwrap();
        assert_eq!(
            outage.surveillance.msaw_product,
            crate::surveillance::ProductAvailability::Withheld(
                crate::surveillance::ProductUnavailableReason::SurveillanceFeedUnavailable
            )
        );
        assert!(outage.surveillance.withheld_msaw_records > 0);
        let retained_msaw = outage.surveillance.msaw.as_ref().unwrap();
        assert!(retained_msaw
            .open
            .iter()
            .chain(&retained_msaw.closed)
            .any(|alert| alert.start_ns == alert_start));

        let recovered =
            crate::runtime::regional_step(&mut runtime, &receipt(4, Some(1490))).unwrap();
        assert_eq!(
            recovered.surveillance.msaw_product,
            crate::surveillance::ProductAvailability::Available
        );
    }

    // REQ: OA-REG-012
    #[test]
    fn malformed_terrain_isolates_msaw_while_tracker_ca_and_scope_continue() {
        let directory =
            std::env::temp_dir().join(format!("oa-regional-bad-terrain-{}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        for name in [
            "USGS_1_n41w088.tif",
            "USGS_1_n41w087.tif",
            "USGS_1_n42w088.tif",
            "USGS_1_n42w087.tif",
        ] {
            std::fs::write(
                directory.join(name),
                b"intentionally malformed synthetic TIFF",
            )
            .unwrap();
        }
        let mut service = regional_msaw_synthetic(32).unwrap();
        service.synthetic = false;
        service.provenance_sha256 =
            oa_sha256::sha256(b"deliberately malformed public synthetic terrain");
        service.floors = Some(Floors::new(Some(directory.clone()), None, None));
        let mut runtime = regional(service);
        crate::runtime::regional_step(&mut runtime, &receipt(1, Some(1500))).unwrap();
        let frame = crate::runtime::regional_step(&mut runtime, &receipt(2, Some(1490))).unwrap();
        assert_eq!(frame.surveillance.tracks.len(), 1);
        assert_eq!(frame.surveillance.ca_stats.tracks, 1);
        assert_eq!(frame.surveillance.displayed, 1);
        let msaw = frame.surveillance.msaw.as_ref().unwrap();
        assert!(matches!(msaw.report, NetReport::Unavailable { .. }));
        assert!(msaw.stats.is_none());
        assert!(!msaw.failure.is_empty());
        assert!(frame
            .surveillance
            .warnings
            .iter()
            .any(|line| line.starts_with("MSAW UNAVAILABLE")));
        assert!(crate::runtime::regional_pixels(&runtime).is_ok());
        let continued =
            crate::runtime::regional_step(&mut runtime, &receipt(3, Some(1480))).unwrap();
        assert_eq!(continued.surveillance.tracks.len(), 1);
        let msaw = continued.surveillance.msaw.unwrap();
        assert!(!msaw.evaluation_attempted);
        assert!(msaw.stats.is_none());
        std::fs::remove_dir_all(directory).unwrap();
    }
}
