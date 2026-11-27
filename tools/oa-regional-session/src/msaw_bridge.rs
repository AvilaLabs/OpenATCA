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
const MAX_AURAL_AREA_ID_BYTES: usize = 24;
const MAX_PENDING_AURAL_EVENTS: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum AuralSinkStatus {
    NotConfigured,
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum AuralEventKind {
    Requested,
    Cleared,
    Acknowledged,
    AmbiguousArea,
    LedgerFull,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct AuralEvent {
    pub kind: AuralEventKind,
    pub address: u32,
    pub alert_start_ns: i64,
    pub event_time_ns: i64,
    pub area_id: [u8; MAX_AURAL_AREA_ID_BYTES],
    pub area_id_len: u8,
}
const _: () = assert!(core::mem::size_of::<AuralEvent>() == 56);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct AuralAlarmState {
    pub address: u32,
    pub alert_start_ns: i64,
    pub raised_at_ns: i64,
    pub cleared_at_ns: Option<i64>,
    pub acknowledged_at_ns: Option<i64>,
    pub area_id: [u8; MAX_AURAL_AREA_ID_BYTES],
    pub area_id_len: u8,
}
const _: () = assert!(core::mem::size_of::<AuralAlarmState>() == 88);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuralReport {
    pub sink: AuralSinkStatus,
    pub states: Vec<AuralAlarmState>,
    pub events: Vec<AuralEvent>,
    pub unlatched_alerts: u32,
    pub unreported_events: u32,
}

/// Eligibility basis joined from an ordered tracker row and an exact unique live-flight view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiveTrackBasis {
    pub address: u32,
    pub basis: oa_msaw_run::eligibility::Basis,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SuppressedAlert {
    pub address: u32,
    pub alert_start_ns: i64,
    pub basis: oa_msaw_run::eligibility::Basis,
}

/// Explicit, bounded startup manifest. Observation time is never substituted for a METAR
/// file's actual receipt time. Relative paths resolve beside this manifest.
pub fn load_msaw_manifest(
    path: &Path,
    cutoff_ms: i64,
    cifp_area: Option<(f64, f64, f64, f64)>,
) -> Result<(RegionalData, [u8; 32]), String> {
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
    let mut cifp = None;
    for (index, line) in lines.enumerate() {
        if index >= 67 || line.len() > 1100 {
            return Err("MSAW manifest row/byte budget exceeded".into());
        }
        let fields: Vec<_> = line.split('\t').collect();
        match fields.as_slice() {
            ["CONFIG", name] if config.is_none() => config = Some(resolve(name)?),
            ["TERRAIN", name] if terrain_dir.is_none() => terrain_dir = Some(resolve(name)?),
            ["CIFP", name] if cifp.is_none() => cifp = Some(resolve(name)?),
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
        cifp,
        cifp_area,
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
    cycles: usize,
    failed: bool,
    admitted_cutoff_ms: i64,
    inhibits_installed: bool,
    aural_latches: Vec<AuralAlarmState>,
    aural_pending_events: Vec<AuralEvent>,
    aural_events: Vec<AuralEvent>,
    max_aural_latches: usize,
    max_aural_events: usize,
    aural_event_overflow: u32,
    aural_pending_overflow: u32,
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
    pub aural: AuralReport,
    pub live_track_basis: Vec<LiveTrackBasis>,
    pub suppressed_alerts: Vec<SuppressedAlert>,
}

fn assemble(
    settings: Settings,
    floors: Option<Floors>,
    qnh: Qnh,
    provenance_sha256: [u8; 32],
    synthetic: bool,
    max_rows: usize,
) -> Result<RegionalMsaw, String> {
    if max_rows == 0 || max_rows > crate::surveillance::MAX_TRACKS {
        return Err("MSAW regional row capacity is outside the configured session envelope".into());
    }
    if settings.msaw.max_tracks > max_rows
        || settings.msaw.max_open > max_rows
        || settings.msaw.track_budget > max_rows
        || settings.msaw.samples > MAX_REGIONAL_MSAW_SAMPLES
        || settings.alert_log_capacity > MAX_REGIONAL_MSAW_LOGS
        || settings.skip_log_capacity > MAX_REGIONAL_MSAW_LOGS
    {
        return Err("MSAW configuration exceeds regional row/sample/log budgets".into());
    }
    // These settings need the corresponding coordinator service, not silent omission.
    if !settings.requests.is_empty() {
        return Err("MSAW service requests require joined full-flight and beacon evidence".into());
    }
    let max_points = max_rows
        .checked_mul(settings.msaw.samples)
        .ok_or("MSAW point budget overflow")?;
    let max_aural_events = max_rows
        .checked_add(settings.alert_log_capacity)
        .and_then(|events| events.checked_add(MAX_PENDING_AURAL_EVENTS))
        .ok_or("MSAW aural event budget overflow")?;
    let max_aural_latches = settings.alert_log_capacity;
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
        cycles: 0,
        failed: false,
        admitted_cutoff_ms: 0,
        inhibits_installed: false,
        aural_latches: Vec::with_capacity(max_aural_latches),
        aural_pending_events: Vec::with_capacity(MAX_PENDING_AURAL_EVENTS),
        aural_events: Vec::with_capacity(max_aural_events),
        max_aural_latches,
        max_aural_events,
        aural_event_overflow: 0,
        aural_pending_overflow: 0,
    })
}

pub fn regional_msaw_new(data: RegionalData, max_rows: usize) -> Result<RegionalMsaw, String> {
    if (!data.settings.cifp_airports.is_empty() || data.settings.cifp_area_margin_nm.is_some())
        && !data.provenance.cifp_source_present
    {
        return Err("CIFP-dependent MSAW settings lack loaded CIFP provenance".into());
    }
    let admitted_cutoff_ms = data.provenance.cutoff_ms;
    let mut service = assemble(
        data.settings,
        Some(data.floors),
        data.qnh,
        data.provenance.manifest_sha256,
        false,
        max_rows,
    )?;
    service.admitted_cutoff_ms = admitted_cutoff_ms;
    Ok(service)
}

/// Explicit public test model: a flat 500-ft floor in the Chicago test box and one synthetic
/// 15-C/1013.25-hPa station at elevation zero, observed at Unix second zero. No FAA inputs.
pub fn regional_msaw_synthetic(max_rows: usize) -> Result<RegionalMsaw, String> {
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
    )
}

pub fn regional_msaw_net_policy(service: &RegionalMsaw) -> (i64, i64) {
    (
        service.settings.net_stale_limit_ns,
        service.settings.net_future_horizon_ns,
    )
}

pub(crate) fn regional_msaw_vfr_code(service: &RegionalMsaw, squawk: u16) -> bool {
    service.settings.vfr_codes.contains(&squawk)
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

fn install_configured_inhibits(
    service: &mut RegionalMsaw,
    inhibits: &mut InhibitTable,
    first_ns: i64,
) -> Result<(), String> {
    if service.inhibits_installed {
        return Ok(());
    }
    let mut staged = inhibits.clone();
    for (index, spec) in service.settings.inhibits.iter().enumerate() {
        let start_ns = first_ns.saturating_add((spec.start_s * 1e9).round() as i64);
        let end_ns = first_ns.saturating_add((spec.end_s * 1e9).round() as i64);
        let request = oa_safety::inhibit::InhibitRequest {
            net: oa_safety::net::NetId::Msaw,
            vertices: &spec.vertices,
            alt_low_ft: spec.alt_lo_ft,
            alt_high_ft: spec.alt_hi_ft,
            start_ns,
            end_ns,
            reason: &spec.reason,
            author: &spec.author,
        };
        oa_safety::inhibit::inhibit_create(
            &mut staged,
            &service.settings.inhibit_cfg,
            &request,
            first_ns,
        )
        .map_err(|error| format!("MSAW inhibit line {} refused: {error:?}", index + 1))?;
    }
    *inhibits = staged;
    service.inhibits_installed = true;
    Ok(())
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
    if service.cycles >= crate::surveillance::MAX_FRAMES {
        return Err("MSAW cycle limit exceeded before evaluation".into());
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
        aural: AuralReport {
            sink: if service.settings.aural_areas.is_empty() {
                AuralSinkStatus::NotConfigured
            } else {
                AuralSinkStatus::Unavailable
            },
            states: Vec::new(),
            events: Vec::new(),
            unlatched_alerts: 0,
            unreported_events: 0,
        },
        live_track_basis: Vec::new(),
        suppressed_alerts: Vec::new(),
    })
}

fn aural_area_id(id: &str) -> Result<([u8; MAX_AURAL_AREA_ID_BYTES], u8), String> {
    if id.is_empty() || id.len() > MAX_AURAL_AREA_ID_BYTES || !id.is_ascii() {
        return Err("MSAW aural area identifier violates its fixed representation".into());
    }
    let mut bytes = [0; MAX_AURAL_AREA_ID_BYTES];
    let len = u8::try_from(id.len()).map_err(|_| "MSAW aural area identifier length overflow")?;
    bytes
        .get_mut(..id.len())
        .ok_or("MSAW aural area identifier length is not representable")?
        .copy_from_slice(id.as_bytes());
    Ok((bytes, len))
}

fn emit_aural_event(service: &mut RegionalMsaw, event: AuralEvent, pending: bool) {
    let events = if pending {
        &mut service.aural_pending_events
    } else {
        &mut service.aural_events
    };
    let limit = if pending {
        MAX_PENDING_AURAL_EVENTS
    } else {
        service.max_aural_events
    };
    if events.len() < limit {
        events.push(event);
    } else if pending {
        service.aural_pending_overflow = service.aural_pending_overflow.saturating_add(1);
    } else {
        service.aural_event_overflow = service.aural_event_overflow.saturating_add(1);
    }
}

fn update_aural(
    service: &mut RegionalMsaw,
    time_ns: i64,
    current_rows: &[MsawTrack],
    open: &[Alert],
) -> AuralReport {
    service.aural_events.clear();
    service.aural_event_overflow = 0;
    let pending_overflow = service.aural_pending_overflow;
    service.aural_pending_overflow = 0;
    // A cycle can emit at most two events per admitted row (new request and old clear),
    // plus the separately capped operator acknowledgements retained between cycles.
    service
        .aural_events
        .append(&mut service.aural_pending_events);
    let mut unlatched_alerts = 0u32;

    for alert in open {
        if service
            .aural_latches
            .iter()
            .any(|state| state.address == alert.addr && state.alert_start_ns == alert.start_ns)
        {
            continue;
        }
        if !current_rows.iter().any(|track| track.addr == alert.addr) {
            continue;
        }
        let mut matched = None;
        let mut ambiguous = false;
        for (index, area) in service.settings.aural_areas.iter().enumerate() {
            if oa_safety::geom::polygon_contains(&area.polygon, alert.lat_e7, alert.lon_e7) {
                if matched.is_some() {
                    ambiguous = true;
                    break;
                }
                matched = Some(index);
            }
        }
        if ambiguous {
            unlatched_alerts = unlatched_alerts.saturating_add(1);
            emit_aural_event(
                service,
                AuralEvent {
                    kind: AuralEventKind::AmbiguousArea,
                    address: alert.addr,
                    alert_start_ns: alert.start_ns,
                    event_time_ns: time_ns,
                    area_id: [0; MAX_AURAL_AREA_ID_BYTES],
                    area_id_len: 0,
                },
                false,
            );
            continue;
        }
        let Some(index) = matched else { continue };
        let Some(area) = service.settings.aural_areas.get(index) else {
            unlatched_alerts = unlatched_alerts.saturating_add(1);
            continue;
        };
        if service.aural_latches.len() >= service.max_aural_latches {
            unlatched_alerts = unlatched_alerts.saturating_add(1);
            emit_aural_event(
                service,
                AuralEvent {
                    kind: AuralEventKind::LedgerFull,
                    address: alert.addr,
                    alert_start_ns: alert.start_ns,
                    event_time_ns: time_ns,
                    area_id: [0; MAX_AURAL_AREA_ID_BYTES],
                    area_id_len: 0,
                },
                false,
            );
            continue;
        }
        let Ok((area_id, area_id_len)) = aural_area_id(&area.id) else {
            unlatched_alerts = unlatched_alerts.saturating_add(1);
            continue;
        };
        service.aural_latches.push(AuralAlarmState {
            address: alert.addr,
            alert_start_ns: alert.start_ns,
            raised_at_ns: time_ns,
            cleared_at_ns: None,
            acknowledged_at_ns: None,
            area_id,
            area_id_len,
        });
        emit_aural_event(
            service,
            AuralEvent {
                kind: AuralEventKind::Requested,
                address: alert.addr,
                alert_start_ns: alert.start_ns,
                event_time_ns: time_ns,
                area_id,
                area_id_len,
            },
            false,
        );
    }

    for index in 0..service.aural_latches.len() {
        let event = if let Some(state) = service.aural_latches.get_mut(index) {
            if state.cleared_at_ns.is_none()
                && !open.iter().any(|alert| {
                    alert.addr == state.address && alert.start_ns == state.alert_start_ns
                })
            {
                state.cleared_at_ns = Some(time_ns);
                Some(AuralEvent {
                    kind: AuralEventKind::Cleared,
                    address: state.address,
                    alert_start_ns: state.alert_start_ns,
                    event_time_ns: time_ns,
                    area_id: state.area_id,
                    area_id_len: state.area_id_len,
                })
            } else {
                None
            }
        } else {
            None
        };
        if let Some(event) = event {
            emit_aural_event(service, event, false);
        }
    }

    let report = AuralReport {
        sink: if service.settings.aural_areas.is_empty() {
            AuralSinkStatus::NotConfigured
        } else {
            AuralSinkStatus::Unavailable
        },
        states: service.aural_latches.clone(),
        events: service.aural_events.clone(),
        unlatched_alerts,
        unreported_events: pending_overflow.saturating_add(service.aural_event_overflow),
    };
    service
        .aural_latches
        .retain(|state| state.cleared_at_ns.is_none() || state.acknowledged_at_ns.is_none());
    report
}

/// Acknowledge one existing alert-instance aural request. The instance start and exact aircraft
/// address are both required; the caller must validate its current selected view as well.
pub fn regional_msaw_acknowledge_aural(
    service: &mut RegionalMsaw,
    address: u32,
    alert_start_ns: i64,
    source_time_ns: i64,
) -> Result<(), String> {
    if address == 0 || alert_start_ns <= 0 || source_time_ns < service.last_ns.unwrap_or(0) {
        return Err("MSAW aural acknowledgement has a stale or invalid instance/time".into());
    }
    let Some(index) = service
        .aural_latches
        .iter()
        .position(|state| state.address == address && state.alert_start_ns == alert_start_ns)
    else {
        return Err("MSAW aural acknowledgement names no retained alert instance".into());
    };
    if service.aural_pending_events.len() >= MAX_PENDING_AURAL_EVENTS {
        service.aural_pending_overflow = service.aural_pending_overflow.saturating_add(1);
        return Err("MSAW aural acknowledgement event capacity is exhausted".into());
    }
    let Some(state) = service.aural_latches.get_mut(index) else {
        return Err("MSAW aural acknowledgement state disappeared".into());
    };
    if state.acknowledged_at_ns.is_some() || source_time_ns < state.raised_at_ns {
        return Err("MSAW aural acknowledgement is duplicate or predates its request".into());
    }
    state.acknowledged_at_ns = Some(source_time_ns);
    let event = AuralEvent {
        kind: AuralEventKind::Acknowledged,
        address,
        alert_start_ns,
        event_time_ns: source_time_ns,
        area_id: state.area_id,
        area_id_len: state.area_id_len,
    };
    emit_aural_event(service, event, true);
    Ok(())
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
    regional_msaw_cycle_with_basis(service, time_ns, pressure_rows, &[], states, inhibits)
}

pub fn regional_msaw_cycle_with_basis(
    service: &mut RegionalMsaw,
    time_ns: i64,
    pressure_rows: &[MsawTrack],
    track_basis: &[LiveTrackBasis],
    states: &mut StateTable,
    inhibits: &mut InhibitTable,
) -> Result<RegionalMsawFrame, String> {
    if time_ns <= 0 || service.last_ns.is_some_and(|last| time_ns <= last) {
        return Err("MSAW report time is not a valid advancing regional cutoff".into());
    }
    if !track_basis.is_empty()
        && (track_basis.len() != pressure_rows.len()
            || track_basis
                .iter()
                .zip(pressure_rows)
                .any(|(basis, row)| basis.address != row.addr)
            || track_basis.windows(2).any(|pair| {
                pair.first()
                    .zip(pair.get(1))
                    .is_some_and(|(left, right)| left.address >= right.address)
            }))
    {
        return Err("MSAW eligibility rows do not match ordered track set".into());
    }
    alert_log_clear(&mut service.alerts);
    skip_log_clear(&mut service.skips);
    service.samples.clear();
    let attempted = !service.failed;
    let result = if service.failed {
        Err("MSAW service failed; reconstruct before resuming evaluation".into())
    } else {
        install_configured_inhibits(service, inhibits, time_ns)
            .and_then(|()| evaluate_cycle(service, time_ns, pressure_rows, states, inhibits))
    };
    match result {
        Ok(mut frame) => {
            let mut suppressed_alerts = Vec::with_capacity(frame.open.len());
            if !track_basis.is_empty() {
                frame.open.retain(|alert| {
                    let basis = track_basis
                        .binary_search_by_key(&alert.addr, |row| row.address)
                        .ok()
                        .and_then(|index| track_basis.get(index))
                        .map_or(oa_msaw_run::eligibility::Basis::NotJudged, |row| row.basis);
                    if basis.alerted() {
                        true
                    } else {
                        suppressed_alerts.push(SuppressedAlert {
                            address: alert.addr,
                            alert_start_ns: alert.start_ns,
                            basis,
                        });
                        false
                    }
                });
            }
            frame.aural = update_aural(service, time_ns, pressure_rows, &frame.open);
            frame.live_track_basis = track_basis.to_vec();
            frame.suppressed_alerts = suppressed_alerts;
            Ok(frame)
        }
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
                aural: AuralReport {
                    sink: if service.settings.aural_areas.is_empty() {
                        AuralSinkStatus::NotConfigured
                    } else {
                        AuralSinkStatus::Unavailable
                    },
                    states: service.aural_latches.clone(),
                    events: service.aural_pending_events.clone(),
                    unlatched_alerts: 0,
                    unreported_events: service
                        .aural_pending_overflow
                        .saturating_add(service.aural_event_overflow),
                },
                live_track_basis: Vec::new(),
                suppressed_alerts: Vec::new(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    fn configured_regional_service() -> (std::path::PathBuf, RegionalMsaw, [u8; 32]) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "oa-regional-msaw-loader-{}-{id}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let header = include_str!("../../../crates/oa-arinc424/tests/fixtures/header.txt");
        let body = [
            include_str!("../../../crates/oa-arinc424/tests/fixtures/airports.txt"),
            include_str!("../../../crates/oa-arinc424/tests/fixtures/runways.txt"),
            include_str!("../../../crates/oa-arinc424/tests/fixtures/terminal_waypoints.txt"),
            include_str!("../../../crates/oa-arinc424/tests/fixtures/approach.txt"),
        ]
        .concat();
        let mut header_rows: Vec<String> = header.lines().map(str::to_string).collect();
        header_rows[0].replace_range(28..35, &format!("{:07}", body.lines().count()));
        let mut cifp = header_rows.join("\n");
        cifp.push('\n');
        cifp.push_str(&body);
        std::fs::write(root.join("FAACIFP18"), cifp).unwrap();

        let config = include_str!("../../oa-msaw/testdata/dev.cfg")
            .replace("max_tracks = 8192", "max_tracks = 32")
            .replace("max_open = 4096", "max_open = 32")
            .replace("track_budget = 2048", "track_budget = 32")
            .replace("alert_log_capacity = 262144", "alert_log_capacity = 4096")
            .replace("skip_log_capacity = 262144", "skip_log_capacity = 4096");
        let config = format!(
            "{config}\ncifp_airport = KORD\ninhibit = 0 120 0 3000 A4TEST configured 40.9,-87.1 40.9,-86.9 41.1,-86.9 41.1,-87.1\n"
        );
        std::fs::write(root.join("dev.cfg"), config).unwrap();
        let manifest = root.join("inputs.tsv");
        std::fs::write(
            &manifest,
            "OA-MSAW-INPUTS\t1\nCONFIG\tdev.cfg\nCIFP\tFAACIFP18\n",
        )
        .unwrap();
        let (data, _) = load_msaw_manifest(&manifest, 0, None).unwrap();
        let expected_provenance = data.provenance.manifest_sha256;
        let service = regional_msaw_new(data, 32).unwrap();
        (root, service, expected_provenance)
    }

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

    // REQ: OA-MSAW-007, OA-MSAW-034
    #[test]
    fn normal_manifest_loads_cifp_corridors_and_installs_configured_inhibits_once() {
        let (root, mut service, expected_provenance) = configured_regional_service();
        // This test's alerting floor is explicitly synthetic; the production loader above still
        // proves that the configured inhibition and CIFP source enter the normal service.
        service.synthetic = true;
        service.floors = None;
        let mut states = oa_safety::net::state_table_new(10_000_000_000, 0).unwrap();
        let mut inhibits = oa_safety::inhibit::inhibits_new();
        assert_eq!(service.provenance_sha256, expected_provenance);
        assert!(!service.settings.msaw.corridors.is_empty());

        let first_ns = 2_000_000_000;
        let _ = regional_msaw_cycle(
            &mut service,
            first_ns,
            &[track(1500.0, -600.0)],
            &mut states,
            &mut inhibits,
        )
        .unwrap();
        assert_eq!(
            oa_safety::inhibit::inhibits_in_force(&inhibits, oa_safety::net::NetId::Msaw, first_ns),
            1
        );
        assert!(
            !oa_safety::inhibit::suppressed_log(&inhibits).is_empty(),
            "configured inhibition did not record the hidden MSAW alert"
        );
        assert!(service.inhibits_installed);

        let later_ns = 130_000_000_000;
        let later = regional_msaw_cycle(
            &mut service,
            later_ns,
            &[track(1500.0, -600.0)],
            &mut states,
            &mut inhibits,
        )
        .unwrap();
        assert_eq!(
            later.open.len(),
            1,
            "expired inhibition must stop hiding the alert"
        );
        assert_eq!(
            oa_safety::inhibit::inhibits_in_force(&inhibits, oa_safety::net::NetId::Msaw, later_ns),
            0
        );
        std::fs::remove_dir_all(root).unwrap();
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

    // REQ: OA-MSAW-083
    #[test]
    fn regional_aural_area_latches_one_request_per_alert_instance() {
        let mut service = regional_msaw_synthetic(32).unwrap();
        service
            .settings
            .aural_areas
            .push(oa_msaw_run::config::AuralArea {
                id: "TOWER".into(),
                polygon: oa_safety::geom::polygon_new(&[
                    (400_000_000, -880_000_000),
                    (420_000_000, -880_000_000),
                    (420_000_000, -860_000_000),
                    (400_000_000, -860_000_000),
                ])
                .unwrap(),
            });
        let mut runtime = regional(service);
        // Regional surveillance confirms a target from two position reports. The aural
        // request belongs to the first confirmed alert instance, not an unconfirmed first hit.
        let priming = crate::runtime::regional_step(&mut runtime, &receipt(1, Some(1510)))
            .unwrap()
            .surveillance
            .msaw
            .unwrap();
        assert!(priming.open.is_empty());
        let first = crate::runtime::regional_step(&mut runtime, &receipt(2, Some(1500)))
            .unwrap()
            .surveillance
            .msaw
            .unwrap();
        assert_eq!(first.open.len(), 1);
        assert_eq!(first.aural.sink, AuralSinkStatus::Unavailable);
        assert_eq!(first.aural.states.len(), 1);
        assert_eq!(first.aural.states[0].alert_start_ns, first.open[0].start_ns);
        assert_eq!(first.aural.states[0].area_id_len, 5);
        assert_eq!(first.aural.states[0].area_id[..5], *b"TOWER");
        assert_eq!(first.aural.events.len(), 1);
        assert_eq!(first.aural.events[0].kind, AuralEventKind::Requested);

        let ongoing = crate::runtime::regional_step(&mut runtime, &receipt(3, Some(1490)))
            .unwrap()
            .surveillance
            .msaw
            .unwrap();
        assert_eq!(ongoing.open.len(), 1);
        assert_eq!(
            ongoing.aural.events.len(),
            0,
            "ordinary scans do not replay request"
        );

        let later = crate::runtime::regional_step(&mut runtime, &receipt(4, Some(1480)))
            .unwrap()
            .surveillance
            .msaw
            .unwrap();
        assert_eq!(later.open.len(), 1);
        assert_eq!(
            later.aural.events.len(),
            0,
            "ordinary scans do not replay request"
        );
    }

    // REQ: OA-MSAW-083
    #[test]
    fn aural_acknowledgement_is_exact_and_does_not_clear_visual_alert() {
        let mut service = regional_msaw_synthetic(32).unwrap();
        service
            .settings
            .aural_areas
            .push(oa_msaw_run::config::AuralArea {
                id: "TOWER".into(),
                polygon: oa_safety::geom::polygon_new(&[
                    (400_000_000, -880_000_000),
                    (420_000_000, -880_000_000),
                    (420_000_000, -860_000_000),
                    (400_000_000, -860_000_000),
                ])
                .unwrap(),
            });
        let mut states = oa_safety::net::state_table_new(10_000_000_000, 0).unwrap();
        let mut inhibits = oa_safety::inhibit::inhibits_new();
        let first = regional_msaw_cycle(
            &mut service,
            2_000_000_000,
            &[track(1500.0, -600.0)],
            &mut states,
            &mut inhibits,
        )
        .unwrap();
        let alert_start_ns = first.open[0].start_ns;
        regional_msaw_acknowledge_aural(&mut service, 0xabc101, alert_start_ns, 2_000_000_000)
            .unwrap();
        assert!(regional_msaw_acknowledge_aural(
            &mut service,
            0xabc101,
            alert_start_ns,
            2_000_000_000,
        )
        .is_err());
        let acknowledged = regional_msaw_cycle(
            &mut service,
            3_000_000_000,
            &[track(1490.0, -600.0)],
            &mut states,
            &mut inhibits,
        )
        .unwrap();
        assert_eq!(acknowledged.open.len(), 1);
        assert!(acknowledged
            .aural
            .events
            .iter()
            .any(|event| event.kind == AuralEventKind::Acknowledged));
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
                },
            }],
        }
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
