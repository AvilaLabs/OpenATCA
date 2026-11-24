//! Guarded offline session execution and evidence publication (shell, B1/B3/C6).
//! Each successful run is a new directory; COMPLETE is written last. Prefix reconstruction
//! recomputes state and does not claim a live or distributed process checkpoint.

use crate::runtime::{regional_new, regional_pixels, regional_step};
use crate::surveillance::{SurveillanceConfig, FRAME_HEIGHT, FRAME_WIDTH, MAX_FRAMES};
use crate::{flush_receipt_group, parse_event_line, push_event, session_cursor_new};
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
}

pub enum MsawSource {
    Disabled,
    Synthetic,
    Manifest(PathBuf),
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
    if bytes.len() > 16 * 1024 * 1024 {
        return Err("normalized input exceeds byte limit".into());
    }
    oa_sealed::refuse(output)?;
    let model_bytes = match source {
        ModelSource::Snapshot(path) => read_guarded(path, MAX_SNAPSHOT_BYTES)?,
        ModelSource::Synthetic => oa_airspace::core::snapshot::encode(&synthetic_model()?)
            .map_err(|e| format!("synthetic snapshot: {e:?}"))?,
    };
    let model = oa_airspace::core::snapshot::decode(&model_bytes)
        .map_err(|e| format!("snapshot: {e:?}"))?;
    let snapshot_sha = oa_sha256::sha256(&model_bytes);
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let executable_sha = oa_sha256::sha256(&read_guarded(&executable, MAX_EXECUTABLE_BYTES)?);
    // Explicit synthetic-development settings, not FAA separation minima or facility sizing.
    let cfg = SurveillanceConfig {
        centre_lat_e7: 410_000_000,
        centre_lon_e7: -870_000_000,
        range_nm: 180.0,
    };
    let mut runtime = regional_new(model, snapshot_sha, *b"ZAU1", cfg)?;
    let text = std::str::from_utf8(bytes).map_err(|_| "input not UTF-8")?;
    if text.lines().next() != Some("OA-RS\t1") {
        return Err("expected OA-RS header".into());
    }
    let mut cursor = session_cursor_new();
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
                crate::msaw_bridge::load_msaw_manifest(path, first / 1_000_000)?;
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
    // Runtime failures precede final output publication. Temporary evidence is identifiable by
    // the absence of COMPLETE and is never accepted as a closed result.
    fs::create_dir(output).map_err(|e| format!("create new output: {e}"))?;
    let mut settings = format!("OA_REGIONAL_SETTINGS\t1\ncentre\tZAU1\nconfiguration\t{cfg:?}\ntracks\t32\nframes\t512\nframe_gap_max_ns\t60000000000\nCA_h_m\t1300\nCA_v_ft\t300\nCA_lookahead_ns\t120000000000\ntrajectory_track_age_max_ms\t5000\nnormalization\tpositive-address airborne positions\nmodel\t{}\n", match source { ModelSource::Synthetic => "SYNTHETIC", ModelSource::Snapshot(_) => "SUPPLIED_SNAPSHOT" });
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
