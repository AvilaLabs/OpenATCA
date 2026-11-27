//! Guarded offline session execution and evidence publication (shell, B1/B3/C6).
//! Each successful run is a new directory; COMPLETE is written last. Prefix reconstruction
//! recomputes state and does not claim a live or distributed process checkpoint.

use crate::runtime::{
    regional_acknowledge_quarantine_before_commit, regional_adaptation_sha256,
    regional_new_from_issued_package_with_audit_checkpoint,
    regional_new_with_profile_and_quarantine_audit_checkpoint, regional_pixels,
    regional_quarantine_stats, regional_step, QuarantineMaintenanceRequest, RegionalRunProfile,
    DEFAULT_REGIONAL_RUN_PROFILE,
};
use crate::surveillance::{SurveillanceConfig, FRAME_HEIGHT, FRAME_WIDTH, MAX_FRAMES};
use crate::{flush_receipt_group, parse_event_line, push_event, session_cursor_new_with_limit};
use oa_airspace::core::model::{self, Model};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const MAX_SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;
const MAX_EXECUTABLE_BYTES: usize = 128 * 1024 * 1024;
const MAX_FRAME_REPORT_BYTES: usize = 512 * 1024;

// A7.2: cold CLI choice; synthetic adaptation is always explicit.
pub enum ModelSource {
    Snapshot(PathBuf),
    Synthetic,
    IssuedPackage {
        root: PathBuf,
        expected_manifest_sha256: [u8; 32],
        processor_id: u32,
    },
}

impl ModelSource {
    fn evidence_label(&self) -> &'static str {
        match self {
            Self::Snapshot(_) => "SUPPLIED_UNPINNED_SNAPSHOT",
            Self::Synthetic => "PUBLIC_SYNTHETIC",
            Self::IssuedPackage { .. } => "ISSUED_PACKAGE_PIN_VERIFIED",
        }
    }
}

pub enum MsawSource {
    Disabled,
    Synthetic,
    Manifest(PathBuf),
}

/// Explicit caller-supplied inputs for one in-process manual quarantine handoff. The pinned
/// registry is local role configuration, not authenticated identity.
#[derive(Clone, Debug)]
pub struct QuarantineReleaseOptions {
    pub source: oa_quarantine::InputSource,
    pub identity: oa_quarantine::InputIdentity,
    pub manual_handoff_ref: String,
    pub audit_log: PathBuf,
    pub audit_checkpoint: oa_audit_core::Checkpoint,
    pub principal_registry: oa_audit_core::PrincipalRegistry,
    pub principal: u64,
    pub time_ns: u64,
}

/// Host-pinned authority context for typed MSAW service requests in the normalized operator
/// queue. The registry digest is supplied independently of the registry file contents.
pub struct MsawServiceAuditOptions {
    pub audit_log: PathBuf,
    pub audit_checkpoint: oa_audit_core::Checkpoint,
    pub principal_registry: oa_audit_log::shell::PinnedPrincipalRegistry,
    pub principal: u64,
}

pub struct SessionAuditOptions<'a> {
    pub quarantine_release: Option<&'a QuarantineReleaseOptions>,
    pub msaw_service_audit: Option<MsawServiceAuditOptions>,
}

/// Persisted CLI profile selection. The absent/default option remains the historical 32-track
/// rendered run; the recorded profile is an explicit bounded high-volume ADS-B replay variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionalCliProfile {
    Default,
    RecordedPeakRendered,
}

pub const fn regional_cli_profile_id(profile: RegionalCliProfile) -> &'static str {
    match profile {
        RegionalCliProfile::Default => "DEFAULT_V1",
        RegionalCliProfile::RecordedPeakRendered => "RECORDED_PEAK_RENDERED_V1",
    }
}

pub const fn regional_cli_runtime_profile(profile: RegionalCliProfile) -> RegionalRunProfile {
    match profile {
        RegionalCliProfile::Default => DEFAULT_REGIONAL_RUN_PROFILE,
        RegionalCliProfile::RecordedPeakRendered => RegionalRunProfile::recorded_peak_rendered(),
    }
}

pub fn read_guarded(path: &Path, max: usize) -> Result<Vec<u8>, String> {
    let (file, len) = oa_sealed::open_regular(path).map_err(|e| format!("guarded input: {e}"))?;
    let cap = u64::try_from(max).map_err(|_| "byte limit width")?;
    if len > cap {
        return Err("input exceeds byte limit".into());
    }
    let mut bytes = Vec::new();
    file.take(cap.checked_add(1).ok_or("byte limit overflow")?)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > max {
        return Err("input grew beyond byte limit".into());
    }
    Ok(bytes)
}

/// Public fixed synthetic fixture, containing no operational adaptation/data. The two fixes
/// support an explicit normalized direct route for integration drills.
pub fn synthetic_model() -> Result<Model, String> {
    let mut model = Model {
        meta: model::Meta {
            nasr_effective: 20261101,
            cifp_cycle: 2611,
            artcc: "TEST".into(),
            buffer_m: 0,
            nasr_sha256: [0; 32],
            cifp_sha256: [0; 32],
        },
        strings: model::StrTable::default(),
        points: model::Points::default(),
        vertices: Vec::new(),
        boundaries: model::Boundaries::default(),
        segments: model::Segments::default(),
        airways: model::Airways::default(),
        airway_legs: model::AirwayLegs::default(),
        procs: model::Procs::default(),
        routes: model::Routes::default(),
        legs: model::Legs::default(),
        preferred_route_state: model::preferred_route_state::UNAVAILABLE,
        preferred_routes: model::PreferredRoutes::default(),
        preferred_route_segments: model::PreferredRouteSegments::default(),
        mora: model::Mora::default(),
        unresolved: model::Unresolved::default(),
        disagreements: model::Disagreements::default(),
        cycle_override: None,
    };
    for (name, lat, lon) in [
        ("FIXA", 410_000_000, -870_000_000),
        ("FIXB", 410_000_000, -850_000_000),
    ] {
        let mut intern = |text: &str| -> Result<model::StrIx, String> {
            let raw = model
                .strings
                .offsets
                .len()
                .checked_sub(1)
                .ok_or("invalid string table")?;
            let index = model::StrIx::new(u32::try_from(raw).map_err(|_| "string index overflow")?);
            model.strings.text.push_str(text);
            model.strings.offsets.push(
                u32::try_from(model.strings.text.len()).map_err(|_| "string length overflow")?,
            );
            Ok(index)
        };
        let ident = intern(name)?;
        let region = intern("K1")?;
        model.points.kind.push(model::point_kind::FIX);
        model.points.lat.push(lat);
        model.points.lon.push(lon);
        model.points.elev_ft.push(model::ELEV_NONE);
        model.points.ident.push(ident);
        model.points.alt_ident.push(model::StrIx::NONE);
        model.points.region.push(region);
        model.points.scope.push(model::StrIx::NONE);
        model.points.name.push(model::StrIx::NONE);
        model::bit_push(&mut model.points.src_nasr, false);
        model::bit_push(&mut model.points.src_cifp, false);
        model::bit_push(&mut model.points.in_area, true);
        model::bit_push(&mut model.points.cls_b, false);
        model::bit_push(&mut model.points.cls_c, false);
        model::bit_push(&mut model.points.cls_d, false);
        model::bit_push(&mut model.points.cls_e, false);
    }
    Ok(model)
}

pub fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    oa_sealed::refuse(path)?;
    // seal-allow: create-new evidence output after path refusal; write only, never reads an input
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    oa_sealed::Sealed::production()
        .verify_open(path, &file)
        .map_err(|e| e.to_string())?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())
}

pub fn run_session(bytes: &[u8], output: &Path, source: &ModelSource) -> Result<(), String> {
    run_session_with_safety(bytes, output, source, &MsawSource::Disabled)
}

pub fn run_session_with_safety(
    bytes: &[u8],
    output: &Path,
    source: &ModelSource,
    safety: &MsawSource,
) -> Result<(), String> {
    run_session_with_options(bytes, output, source, safety, None)
}

pub fn run_session_with_options(
    bytes: &[u8],
    output: &Path,
    source: &ModelSource,
    safety: &MsawSource,
    performance: Option<&Path>,
) -> Result<(), String> {
    run_session_with_profile_options(
        bytes,
        output,
        source,
        safety,
        performance,
        RegionalCliProfile::Default,
    )
}

pub fn run_session_with_profile_options(
    bytes: &[u8],
    output: &Path,
    source: &ModelSource,
    safety: &MsawSource,
    performance: Option<&Path>,
    selected_profile: RegionalCliProfile,
) -> Result<(), String> {
    run_session_with_profile_options_and_quarantine_release(
        bytes,
        output,
        source,
        safety,
        performance,
        selected_profile,
        None,
    )
}

/// Run a bounded replay and optionally append before acknowledging one explicitly selected
/// pending queue front as a caller-claimed manual-responsibility handoff.
pub fn run_session_with_profile_options_and_quarantine_release(
    bytes: &[u8],
    output: &Path,
    source: &ModelSource,
    safety: &MsawSource,
    performance: Option<&Path>,
    selected_profile: RegionalCliProfile,
    quarantine_release: Option<&QuarantineReleaseOptions>,
) -> Result<(), String> {
    run_session_with_profile_options_and_audit_context(
        bytes,
        output,
        source,
        safety,
        performance,
        selected_profile,
        SessionAuditOptions {
            quarantine_release,
            msaw_service_audit: None,
        },
    )
}

pub fn run_session_with_profile_options_and_audit_context(
    bytes: &[u8],
    output: &Path,
    source: &ModelSource,
    safety: &MsawSource,
    performance: Option<&Path>,
    selected_profile: RegionalCliProfile,
    audit: SessionAuditOptions<'_>,
) -> Result<(), String> {
    let SessionAuditOptions {
        quarantine_release,
        msaw_service_audit,
    } = audit;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err("normalized input exceeds byte limit".into());
    }
    oa_sealed::refuse(output)?;
    // Explicit synthetic-development settings, not FAA separation minima or facility sizing.
    let cfg = SurveillanceConfig {
        centre_lat_e7: 410_000_000,
        centre_lon_e7: -870_000_000,
        range_nm: 180.0,
    };
    let profile = regional_cli_runtime_profile(selected_profile);
    let audit_checkpoint =
        quarantine_release.map_or_else(oa_audit_core::genesis, |request| request.audit_checkpoint);
    let mut activation = None;
    let mut startup_processor_report = None;
    let mut runtime = match source {
        ModelSource::Snapshot(path) => {
            let model_bytes = read_guarded(path, MAX_SNAPSHOT_BYTES)?;
            let model = oa_airspace::core::snapshot::decode(&model_bytes)
                .map_err(|e| format!("snapshot: {e:?}"))?;
            let snapshot_sha = oa_sha256::sha256(&model_bytes);
            regional_new_with_profile_and_quarantine_audit_checkpoint(
                model,
                snapshot_sha,
                *b"ZAU1",
                cfg,
                profile,
                audit_checkpoint,
            )?
        }
        ModelSource::Synthetic => {
            let model_bytes = oa_airspace::core::snapshot::encode(&synthetic_model()?)
                .map_err(|e| format!("synthetic snapshot: {e:?}"))?;
            let model = oa_airspace::core::snapshot::decode(&model_bytes)
                .map_err(|e| format!("synthetic snapshot: {e:?}"))?;
            let snapshot_sha = oa_sha256::sha256(&model_bytes);
            regional_new_with_profile_and_quarantine_audit_checkpoint(
                model,
                snapshot_sha,
                *b"ZAU1",
                cfg,
                profile,
                audit_checkpoint,
            )?
        }
        ModelSource::IssuedPackage {
            root,
            expected_manifest_sha256,
            processor_id,
        } => {
            let (package, local_activation) = match oa_adapt::local_inventory::verify_local_startup(
                root,
                *expected_manifest_sha256,
                *processor_id,
            ) {
                Ok(verified) => verified,
                Err(_) => {
                    fs::create_dir(output)
                        .map_err(|error| format!("create startup refusal output: {error}"))?;
                    let report = crate::processor_inventory::startup_refusal_report(
                        *processor_id,
                        *expected_manifest_sha256,
                        crate::processor_inventory::observed_unix_time_ns(),
                    );
                    write_new(&output.join("processor-report.tsv"), report.as_bytes())?;
                    return Err(
                        "issued adaptation startup verification refused; no model activated".into(),
                    );
                }
            };
            let runtime = regional_new_from_issued_package_with_audit_checkpoint(
                package,
                *b"ZAU1",
                cfg,
                profile,
                audit_checkpoint,
            )?;
            let report = crate::processor_inventory::check_active_runtime_adaptation(
                &runtime,
                &local_activation,
                crate::processor_inventory::observed_unix_time_ns(),
            )
            .map_err(str::to_owned)?;
            if report.status != oa_adapt::local_inventory::LocalInventoryStatus::Match {
                return Err(format!(
                    "issued adaptation startup disk recheck refused: {:?}\n{}",
                    report.status,
                    oa_adapt::local_inventory::render_processor_report_tsv("STARTUP", &report)
                ));
            }
            activation = Some(local_activation);
            startup_processor_report = Some(report);
            runtime
        }
    };
    let snapshot_sha = regional_adaptation_sha256(&runtime);
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let executable_sha = oa_sha256::sha256(&read_guarded(&executable, MAX_EXECUTABLE_BYTES)?);
    let text = std::str::from_utf8(bytes).map_err(|_| "input not UTF-8")?;
    if !matches!(text.lines().next(), Some("OA-RS\t1" | "OA-RS\t2")) {
        return Err("expected OA-RS header".into());
    }
    let mut cursor = session_cursor_new_with_limit(profile.max_total_events)
        .map_err(|error| format!("session profile event capacity: {error:?}"))?;
    let mut batches = Vec::with_capacity(MAX_FRAMES);
    for row in text.lines().skip(1).filter(|line| !line.is_empty()) {
        let event = parse_event_line(row).map_err(|e| format!("input: {e:?}"))?;
        if let Some(batch) =
            push_event(&mut cursor, event).map_err(|e| format!("input order: {e:?}"))?
        {
            if batches.len() >= MAX_FRAMES {
                return Err("frame capacity exhausted".into());
            }
            batches.push(batch);
        }
    }
    if let Some(batch) =
        flush_receipt_group(&mut cursor).map_err(|e| format!("input order: {e:?}"))?
    {
        if batches.len() >= MAX_FRAMES {
            return Err("frame capacity exhausted".into());
        }
        batches.push(batch);
    }
    if batches.is_empty() {
        return Err("session contains no events".into());
    }
    let first = batches.first().ok_or("session has no cutoff")?.receipt_ns;
    let mut cifp_area: Option<(f64, f64, f64, f64)> = None;
    for event in batches.iter().flat_map(|batch| batch.events.iter()) {
        if let crate::input::EventKind::Position {
            latitude_e7,
            longitude_e7,
            ..
        }
        | crate::input::EventKind::PositionV2 {
            latitude_e7,
            longitude_e7,
            ..
        } = event.kind
        {
            let lat = f64::from(latitude_e7) / 1e7;
            let lon = f64::from(longitude_e7) / 1e7;
            if let Some((south, north, west, east)) = &mut cifp_area {
                *south = south.min(lat);
                *north = north.max(lat);
                *west = west.min(lon);
                *east = east.max(lon);
            } else {
                cifp_area = Some((lat, lat, lon, lon));
            }
        }
    }
    let performance_data = performance
        .map(crate::performance::load_performance)
        .transpose()?;
    let mut safety_data = None;
    let safety_manifest_sha = match safety {
        MsawSource::Disabled => None,
        MsawSource::Synthetic => {
            if !matches!(source, ModelSource::Synthetic) {
                return Err(
                    "synthetic MSAW requires the explicit synthetic adaptation model".into(),
                );
            }
            crate::runtime::regional_enable_synthetic_msaw(&mut runtime)?;
            None
        }
        MsawSource::Manifest(path) => {
            let (data, manifest_sha) =
                crate::msaw_bridge::load_msaw_manifest(path, first / 1_000_000, cifp_area)?;
            let evidence = format!(
                "OA_REGIONAL_MSAW_DATA\t1\nprovenance\n{:#?}\nsettings\n{:#?}\nlimits\n{:#?}\n",
                data.provenance,
                data.settings,
                oa_msaw_run::regional::RegionalDataLimits::default()
            );
            if evidence.len() > 2 * 1024 * 1024 {
                return Err("MSAW startup evidence exceeds 2 MiB".into());
            }
            safety_data = Some(evidence);
            crate::runtime::regional_enable_msaw(&mut runtime, data)?;
            Some(manifest_sha)
        }
    };
    if let Some(request) = msaw_service_audit {
        if let Some(quarantine) = quarantine_release {
            if request.audit_log == quarantine.audit_log {
                return Err(
                    "MSAW and quarantine audit contexts require separate chain checkpoints".into(),
                );
            }
        }
        crate::runtime::regional_configure_msaw_service_audit(
            &mut runtime,
            crate::runtime::MsawServiceAuditConfiguration {
                audit_log: request.audit_log,
                checkpoint: request.audit_checkpoint,
                registry: request.principal_registry,
                principal: request.principal,
            },
        )?;
    }
    // Runtime failures precede final output publication. Temporary evidence is identifiable by
    // the absence of COMPLETE and is never accepted as a closed result.
    fs::create_dir(output).map_err(|e| format!("create new output: {e}"))?;
    let mut processor_report = match &startup_processor_report {
        Some(report) => oa_adapt::local_inventory::render_processor_report_tsv("STARTUP", report),
        None => crate::processor_inventory::unpinned_source_report(),
    };
    let mut settings = match selected_profile {
        // Preserve the prior default settings bytes and behavior exactly.
        RegionalCliProfile::Default => format!("OA_REGIONAL_SETTINGS\t1\ncentre\tZAU1\nconfiguration\t{cfg:?}\ntracks\t32\nframes\t512\nframe_gap_max_ns\t60000000000\nCA_h_m\t1300\nCA_v_ft\t300\nCA_lookahead_ns\t120000000000\ntrajectory_track_age_max_ms\t5000\nnormalization\tpositive-address airborne positions\nmodel\t{}\n", source.evidence_label()),
        RegionalCliProfile::RecordedPeakRendered => format!("OA_REGIONAL_SETTINGS\t1\ncentre\tZAU1\nprofile\t{}\nconfiguration\t{cfg:?}\ntracks\t{}\nflights\t{}\nframes\t512\nsurveillance_max_frames\t{}\nmax_total_events\t{}\nmax_drain_rows\t{}\nmax_baro_evidence\t{}\nmax_open_alerts\t{}\nmax_candidate_pairs\t{}\nmax_ca_work_per_frame\t{}\nmax_flight_log_entries\t{}\nmax_presentation_updates\t{}\nrender_scope\t{}\nframe_gap_max_ns\t60000000000\nCA_h_m\t1300\nCA_v_ft\t300\nCA_lookahead_ns\t120000000000\ntrajectory_track_age_max_ms\t5000\nnormalization\tpositive-address airborne positions\nmodel\t{}\n", regional_cli_profile_id(selected_profile), profile.surveillance.max_tracks, profile.max_flights, profile.surveillance.max_frames, profile.max_total_events, profile.surveillance.max_drain_rows, profile.surveillance.max_baro_evidence, profile.surveillance.max_open_alerts, profile.surveillance.max_candidate_pairs, profile.surveillance.max_ca_work_per_frame, profile.max_flight_log_entries, profile.surveillance.max_presentation_updates, profile.surveillance.render_scope, source.evidence_label()),
    };
    settings.push_str(match safety {
        MsawSource::Disabled => "msaw\tUNAVAILABLE_NOT_CONFIGURED\n",
        MsawSource::Synthetic => "msaw\tPUBLIC_SYNTHETIC_FLOOR_WEATHER\n",
        MsawSource::Manifest(_) => "msaw\tSUPPLIED_DATA_MANIFEST\n",
    });
    if let Some(sha) = safety_manifest_sha {
        settings.push_str(&format!(
            "msaw_input_manifest_sha256\t{}\n",
            oa_sha256::to_hex(&sha)
        ));
    }
    if let Some(data) = &performance_data {
        settings.push_str(&format!(
            "vertical_profile\tEXPLICIT_PERFORMANCE\nperformance_sha256\t{}\n",
            oa_sha256::to_hex(&data.sha256)
        ));
    } else {
        settings.push_str("vertical_profile\tDISABLED\n");
    }
    write_new(&output.join("settings.tsv"), settings.as_bytes())?;
    let mut manifest = format!("OA_REGIONAL_MANIFEST\t1\ninput_sha256\t{}\nsnapshot_sha256\t{}\nexecutable_sha256\t{}\nsettings_sha256\t{}\n", oa_sha256::to_hex(&oa_sha256::sha256(bytes)), oa_sha256::to_hex(&snapshot_sha), oa_sha256::to_hex(&executable_sha), oa_sha256::to_hex(&oa_sha256::sha256(settings.as_bytes())));
    if let Some(data) = safety_data {
        write_new(&output.join("msaw-data.txt"), data.as_bytes())?;
        manifest.push_str(&format!(
            "msaw_data_sha256\t{}\n",
            oa_sha256::to_hex(&oa_sha256::sha256(data.as_bytes()))
        ));
    }
    if let Some(data) = &performance_data {
        write_new(&output.join("performance-inputs.tsv"), &data.bytes)?;
        manifest.push_str(&format!(
            "performance_sha256\t{}\n",
            oa_sha256::to_hex(&data.sha256)
        ));
    }
    for batch in &batches {
        let frame = match &performance_data {
            Some(data) => crate::runtime::regional_step_with_performance(
                &mut runtime,
                batch,
                &data.envelopes,
            )?,
            None => regional_step(&mut runtime, batch)?,
        };
        let evidence = format!("OA_REGIONAL_FRAME\t1\n{frame:#?}\n");
        if evidence.len() > MAX_FRAME_REPORT_BYTES {
            return Err("frame report byte budget exceeded".into());
        }
        let name = format!("frame-{:04}.txt", frame.sequence);
        write_new(&output.join(&name), evidence.as_bytes())?;
        let pixels = regional_pixels(&runtime)?;
        let mut ppm = Vec::new();
        let spec = oa_display::raster::FrameSpec {
            width: FRAME_WIDTH,
            height: FRAME_HEIGHT,
            stride: FRAME_WIDTH * 4,
        };
        oa_display::ppm::write_ppm(&mut ppm, &spec, pixels)
            .map_err(|e| format!("frame encoding: {e}"))?;
        let image = format!("frame-{:04}.ppm", frame.sequence);
        write_new(&output.join(&image), &ppm)?;
        manifest.push_str(&format!(
            "frame\t{}\t{}\t{}\t{}\t{}\n",
            frame.sequence,
            frame.input.receipt_ns,
            name,
            oa_sha256::to_hex(&oa_sha256::sha256(evidence.as_bytes())),
            oa_sha256::to_hex(&oa_sha256::sha256(&ppm))
        ));
    }
    if let Some(request) = quarantine_release {
        let receipt = regional_acknowledge_quarantine_before_commit(
            &mut runtime,
            QuarantineMaintenanceRequest {
                source: request.source,
                expected_identity: request.identity,
                audit_log: &request.audit_log,
                expected_checkpoint: request.audit_checkpoint,
                principal_registry: &request.principal_registry,
                principal: request.principal,
                time_ns: request.time_ns,
                manual_handoff_ref: &request.manual_handoff_ref,
            },
        )?;
        let stats = regional_quarantine_stats(&runtime)?;
        let report = format!(
            "OA_REGIONAL_QUARANTINE_MAINTENANCE\t1\nsource\t{:?}\nsource_instance\t{}\nitem_id\t{}\nreasons\t{}\nclock_domain_id\t{}\ncompleted_at_ns\t{}\nmanual_handoff_ref\t{}\naudit_count\t{}\naudit_tail\t{}\nsource_stats\t{stats:?}\n",
            receipt.event.identity.source,
            receipt.event.identity.source_instance,
            receipt.event.identity.item_id,
            receipt.event.reasons.0,
            receipt.event.clock_domain_id,
            receipt.event.completed_at_ns,
            request.manual_handoff_ref,
            receipt.audit_checkpoint.count,
            oa_sha256::to_hex(&receipt.audit_checkpoint.tail),
        );
        if report.len() > 4096 {
            return Err("quarantine maintenance report exceeds its fixed bound".into());
        }
        write_new(
            &output.join("quarantine-maintenance.tsv"),
            report.as_bytes(),
        )?;
        manifest.push_str(&format!(
            "quarantine_maintenance\tquarantine-maintenance.tsv\t{}\n",
            oa_sha256::to_hex(&oa_sha256::sha256(report.as_bytes()))
        ));
    }
    if let Some(local_activation) = activation.as_ref() {
        let report = crate::processor_inventory::check_active_runtime_adaptation(
            &runtime,
            local_activation,
            crate::processor_inventory::observed_unix_time_ns(),
        )
        .map_err(str::to_owned)?;
        processor_report.push_str(&oa_adapt::local_inventory::render_processor_report_tsv(
            "ON_DEMAND",
            &report,
        ));
        if report.status != oa_adapt::local_inventory::LocalInventoryStatus::Match {
            write_new(
                &output.join("processor-report.tsv"),
                processor_report.as_bytes(),
            )?;
            return Err(format!(
                "on-demand issued adaptation check reported {:?}; loaded model retained and disk model not adopted",
                report.status
            ));
        }
    }
    write_new(
        &output.join("processor-report.tsv"),
        processor_report.as_bytes(),
    )?;
    manifest.push_str(&format!(
        "processor_report_sha256\t{}\n",
        oa_sha256::to_hex(&oa_sha256::sha256(processor_report.as_bytes()))
    ));

    write_new(&output.join("manifest.tsv"), manifest.as_bytes())?;
    let directory = oa_sealed::open(output).map_err(|e| e.to_string())?;
    directory.sync_all().map_err(|e| e.to_string())?;
    let complete = output.join("COMPLETE");
    if let Err(error) = write_new(
        &complete,
        format!(
            "{}\n",
            oa_sha256::to_hex(&oa_sha256::sha256(manifest.as_bytes()))
        )
        .as_bytes(),
    ) {
        // Never leave an apparently valid commit marker after a write or durability failure.
        let cleanup = std::fs::remove_file(&complete);
        return Err(format!(
            "completion marker failed: {error}; cleanup={cleanup:?}"
        ));
    }
    if let Err(error) = directory.sync_all() {
        let cleanup = std::fs::remove_file(&complete);
        return Err(format!(
            "completion directory sync failed: {error}; cleanup={cleanup:?}"
        ));
    }
    Ok(())
}
