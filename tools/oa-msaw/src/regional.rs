//! Bounded reusable regional data services for MSAW. Shell-only startup reads and decodes exact
//! configuration, terrain, DOF and METAR inputs; runtime terrain reads remain lazily bounded by
//! `Floors`. This service is intentionally separate from any regional session or display runner.

use std::io::Read;
use std::path::{Path, PathBuf};

use oa_dof::{parse_dof, Dof};
use oa_sealed::Sealed;
use oa_sha256::{sha256, to_hex};
use oa_zip::{Archive, Limits as ZipLimits};

use crate::config::{parse_config, GtmShape, Settings};
use crate::floors::{obstacle_index, Area, Floors, ObstacleIndex, TerrainServiceLimits};
use crate::guard::{open_guarded, OpenFail};
use crate::qnh::Qnh;

pub const DEFAULT_MAX_CONFIG_BYTES: u64 = 1 << 20;
pub const DEFAULT_MAX_TERRAIN_TILES: usize = 64;
pub const DEFAULT_MAX_TERRAIN_FILE_BYTES: u64 = 128 * 1024 * 1024;
pub const DEFAULT_MAX_TERRAIN_GRID_BYTES: usize = 128 * 1024 * 1024;
/// Peak read budget includes source bytes, decoded grid, current/future coarse cache and parser scratch.
pub const DEFAULT_MAX_TERRAIN_WORKING_BYTES: usize = 640 * 1024 * 1024;
pub const DEFAULT_MAX_TERRAIN_RESIDENT_BYTES: usize = 128 * 1024 * 1024;
/// `CoarseTerrain::lookup` refuses above 250,000 cells; the service uses the same hard ceiling.
pub const DEFAULT_MAX_TERRAIN_QUERY_CELLS: u64 = 250_000;
pub const DEFAULT_MAX_DOF_FILES: usize = 32;
pub const DEFAULT_MAX_DOF_COMPRESSED_FILE_BYTES: u64 = 128 * 1024 * 1024;
pub const DEFAULT_MAX_DOF_COMPRESSED_TOTAL_BYTES: u64 = 512 * 1024 * 1024;
pub const DEFAULT_MAX_DOF_DATA_BYTES: u64 = 128 * 1024 * 1024;
pub const DEFAULT_MAX_DOF_TOTAL_DATA_BYTES: u64 = 128 * 1024 * 1024;
pub const DEFAULT_MAX_DOF_RECORDS: usize = 1_000_000;
pub const DEFAULT_MAX_DOF_COVERAGE_CELLS: u64 = 300_000;
pub const DEFAULT_MAX_METAR_FILES: usize = 32;
pub const DEFAULT_MAX_METAR_FILE_BYTES: u64 = 8 * 1024 * 1024;
pub const DEFAULT_MAX_METAR_TOTAL_BYTES: u64 = 64 * 1024 * 1024;
/// One complete CIFP file or its extracted `FAACIFP18` member.
pub const DEFAULT_MAX_CIFP_BYTES: u64 = 128 * 1024 * 1024;
pub const DEFAULT_MAX_MVA_XML_FILES: usize = 64;
pub const DEFAULT_MAX_GTM_POLYGON_BYTES: u64 = 1024 * 1024;
pub const DEFAULT_MAX_GTM_POLYGON_VERTICES: usize = 100_000;

/// A METAR source and the caller's receipt-time evidence. Receipt is per file, not a global
/// timestamp that could make other files or later observations appear available earlier.
// A7.2: cold startup input record, size guard waived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetarSource {
    pub path: PathBuf,
    pub received_at_ms: i64,
}

/// Exact startup inputs. Missing optional sources remain absent; no terrain, obstacle, chart or
/// altimeter value is synthesized to fill them.
// A7.2: cold startup input record, size guard waived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionalDataPaths {
    pub config: PathBuf,
    pub terrain_dir: Option<PathBuf>,
    pub mva_dir: Option<PathBuf>,
    pub cifp_file: Option<PathBuf>,
    pub dof_files: Vec<PathBuf>,
    pub metar_files: Vec<MetarSource>,
}

/// Every caller-supplied limit is required to be no larger than these implementation ceilings.
// A7.2: cold startup limit record, size guard waived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionalDataLimits {
    pub max_config_bytes: u64,
    pub max_terrain_tiles: usize,
    pub max_terrain_file_bytes: u64,
    pub max_terrain_grid_bytes: usize,
    pub max_terrain_working_bytes: usize,
    pub max_terrain_resident_bytes: usize,
    pub max_terrain_query_cells: u64,
    pub max_dof_files: usize,
    pub max_dof_compressed_file_bytes: u64,
    pub max_dof_compressed_total_bytes: u64,
    pub max_dof_data_bytes: u64,
    pub max_dof_total_data_bytes: u64,
    pub max_dof_records: usize,
    pub max_dof_coverage_cells: u64,
    pub max_metar_files: usize,
    pub max_metar_file_bytes: u64,
    pub max_metar_total_bytes: u64,
    pub max_cifp_bytes: u64,
    pub max_gtm_polygon_bytes: u64,
    pub max_gtm_polygon_vertices: usize,
}

impl Default for RegionalDataLimits {
    fn default() -> Self {
        Self {
            max_config_bytes: DEFAULT_MAX_CONFIG_BYTES,
            max_terrain_tiles: DEFAULT_MAX_TERRAIN_TILES,
            max_terrain_file_bytes: DEFAULT_MAX_TERRAIN_FILE_BYTES,
            max_terrain_grid_bytes: DEFAULT_MAX_TERRAIN_GRID_BYTES,
            max_terrain_working_bytes: DEFAULT_MAX_TERRAIN_WORKING_BYTES,
            max_terrain_resident_bytes: DEFAULT_MAX_TERRAIN_RESIDENT_BYTES,
            max_terrain_query_cells: DEFAULT_MAX_TERRAIN_QUERY_CELLS,
            max_dof_files: DEFAULT_MAX_DOF_FILES,
            max_dof_compressed_file_bytes: DEFAULT_MAX_DOF_COMPRESSED_FILE_BYTES,
            max_dof_compressed_total_bytes: DEFAULT_MAX_DOF_COMPRESSED_TOTAL_BYTES,
            max_dof_data_bytes: DEFAULT_MAX_DOF_DATA_BYTES,
            max_dof_total_data_bytes: DEFAULT_MAX_DOF_TOTAL_DATA_BYTES,
            max_dof_records: DEFAULT_MAX_DOF_RECORDS,
            max_dof_coverage_cells: DEFAULT_MAX_DOF_COVERAGE_CELLS,
            max_metar_files: DEFAULT_MAX_METAR_FILES,
            max_metar_file_bytes: DEFAULT_MAX_METAR_FILE_BYTES,
            max_metar_total_bytes: DEFAULT_MAX_METAR_TOTAL_BYTES,
            max_cifp_bytes: DEFAULT_MAX_CIFP_BYTES,
            max_gtm_polygon_bytes: DEFAULT_MAX_GTM_POLYGON_BYTES,
            max_gtm_polygon_vertices: DEFAULT_MAX_GTM_POLYGON_VERTICES,
        }
    }
}

/// Historical METAR weather is admissible only when the caller provides an issue cutoff and a
/// receipt time for each file. Parsed report observation timestamps after cutoff are discarded.
// A7.2: cold one-shot issue policy, size guard waived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WeatherAdmission {
    pub cutoff_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RegionalSourceKind {
    Config = 0,
    Dof = 1,
    Metar = 2,
    GtmPolygon = 3,
    MvaChart = 4,
    Cifp = 5,
}

/// Content evidence. `available_at_ms` is zero for non-weather files; weather has the caller's
/// per-file receipt stamp. No wall-clock read or timing measurement enters this record or digest.
// A7.2: cold manifest record, size guard waived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionalSourceEvidence {
    pub kind: RegionalSourceKind,
    pub path: String,
    pub bytes: u64,
    pub sha256: [u8; 32],
    pub available_at_ms: i64,
}

// A7.2: cold startup provenance, size guard waived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionalProvenance {
    pub cutoff_ms: i64,
    pub sources: Vec<RegionalSourceEvidence>,
    pub manifest_sha256: [u8; 32],
    /// Terrain is lazy-loaded by `Floors`; exact tile hashes are exposed by `Floors::terrain_hashes`.
    pub terrain_dir_present: bool,
    pub mva_chart_files: usize,
    pub cifp_corridors: usize,
    pub obstacle_source_present: bool,
    pub chart_available: bool,
    pub metar_source_files: usize,
    pub dof_records: usize,
    pub metar_rows: u64,
    pub metar_stations: u64,
}

/// A7.2: cold startup service container, size guard waived.
/// Values needed by the shared regional runtime. Absent source counts remain visible in provenance
/// and in `Floors`/`Qnh`; there is no synthetic source value.
pub struct RegionalData {
    pub settings: Settings,
    pub floors: Floors,
    pub qnh: Qnh,
    pub provenance: RegionalProvenance,
}

fn checked_limits(l: RegionalDataLimits) -> Result<(), String> {
    let d = RegionalDataLimits::default();
    if l.max_config_bytes == 0
        || l.max_config_bytes > d.max_config_bytes
        || l.max_terrain_tiles == 0
        || l.max_terrain_tiles > d.max_terrain_tiles
        || l.max_terrain_file_bytes == 0
        || l.max_terrain_file_bytes > d.max_terrain_file_bytes
        || l.max_terrain_grid_bytes == 0
        || l.max_terrain_grid_bytes > d.max_terrain_grid_bytes
        || l.max_terrain_working_bytes == 0
        || l.max_terrain_working_bytes > d.max_terrain_working_bytes
        || l.max_terrain_resident_bytes == 0
        || l.max_terrain_resident_bytes > d.max_terrain_resident_bytes
        || l.max_terrain_query_cells == 0
        || l.max_terrain_query_cells > d.max_terrain_query_cells
        || l.max_dof_files > d.max_dof_files
        || l.max_dof_compressed_file_bytes == 0
        || l.max_dof_compressed_file_bytes > d.max_dof_compressed_file_bytes
        || l.max_dof_compressed_total_bytes == 0
        || l.max_dof_compressed_total_bytes > d.max_dof_compressed_total_bytes
        || l.max_dof_data_bytes == 0
        || l.max_dof_data_bytes > d.max_dof_data_bytes
        || l.max_dof_total_data_bytes == 0
        || l.max_dof_total_data_bytes > d.max_dof_total_data_bytes
        || l.max_dof_records == 0
        || l.max_dof_records > d.max_dof_records
        || l.max_dof_coverage_cells == 0
        || l.max_dof_coverage_cells > d.max_dof_coverage_cells
        || l.max_metar_files > d.max_metar_files
        || l.max_metar_file_bytes == 0
        || l.max_metar_file_bytes > d.max_metar_file_bytes
        || l.max_metar_total_bytes == 0
        || l.max_metar_total_bytes > d.max_metar_total_bytes
        || l.max_cifp_bytes == 0
        || l.max_cifp_bytes > d.max_cifp_bytes
        || l.max_gtm_polygon_bytes == 0
        || l.max_gtm_polygon_bytes > d.max_gtm_polygon_bytes
        || l.max_gtm_polygon_vertices == 0
        || l.max_gtm_polygon_vertices > d.max_gtm_polygon_vertices
    {
        return Err(
            "regional data limits must be positive and within implementation ceilings".into(),
        );
    }
    Ok(())
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let file = open_guarded(Sealed::production(), path).map_err(|e| match e {
        OpenFail::Refused(m) | OpenFail::Io(m) => m,
    })?;
    let mut bytes = Vec::new();
    let mut limited = file.take(limit.saturating_add(1));
    limited
        .read_to_end(&mut bytes)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limit {
        return Err(format!("{}: source exceeds {limit} bytes", path.display()));
    }
    Ok(bytes)
}

fn evidence(
    kind: RegionalSourceKind,
    path: &Path,
    bytes: &[u8],
    available_at_ms: i64,
) -> RegionalSourceEvidence {
    RegionalSourceEvidence {
        kind,
        path: path.display().to_string(),
        bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        sha256: sha256(bytes),
        available_at_ms,
    }
}

fn manifest_hash(
    cutoff_ms: i64,
    sources: &[RegionalSourceEvidence],
    limits: RegionalDataLimits,
    terrain_dir_present: bool,
    mva_dir_present: bool,
    cifp_present: bool,
    mva_chart_count: usize,
    cifp_corridor_count: usize,
    dof_file_count: usize,
    metar_file_count: usize,
) -> [u8; 32] {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"OA-MSAW-REGIONAL-DATA-v1\0");
    bytes.extend_from_slice(&cutoff_ms.to_le_bytes());
    bytes.push(u8::from(terrain_dir_present));
    bytes.push(u8::from(mva_dir_present));
    bytes.push(u8::from(cifp_present));
    bytes.extend_from_slice(
        &u64::try_from(mva_chart_count)
            .unwrap_or(u64::MAX)
            .to_le_bytes(),
    );
    bytes.extend_from_slice(
        &u64::try_from(cifp_corridor_count)
            .unwrap_or(u64::MAX)
            .to_le_bytes(),
    );
    bytes.extend_from_slice(
        &u64::try_from(dof_file_count)
            .unwrap_or(u64::MAX)
            .to_le_bytes(),
    );
    bytes.extend_from_slice(
        &u64::try_from(metar_file_count)
            .unwrap_or(u64::MAX)
            .to_le_bytes(),
    );
    for source in sources {
        bytes.push(source.kind as u8);
        bytes.extend_from_slice(&source.bytes.to_le_bytes());
        bytes.extend_from_slice(&source.available_at_ms.to_le_bytes());
        bytes.extend_from_slice(&source.sha256);
    }
    for value in [
        limits.max_config_bytes,
        limits.max_terrain_file_bytes,
        u64::try_from(limits.max_terrain_grid_bytes).unwrap_or(u64::MAX),
        u64::try_from(limits.max_terrain_working_bytes).unwrap_or(u64::MAX),
        limits.max_dof_compressed_file_bytes,
        limits.max_dof_compressed_total_bytes,
        limits.max_dof_data_bytes,
        limits.max_dof_total_data_bytes,
        limits.max_metar_file_bytes,
        limits.max_metar_total_bytes,
        limits.max_cifp_bytes,
        limits.max_gtm_polygon_bytes,
    ] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for value in [
        limits.max_terrain_tiles,
        limits.max_terrain_resident_bytes,
        limits.max_dof_files,
        limits.max_dof_records,
        limits.max_metar_files,
        limits.max_gtm_polygon_vertices,
    ] {
        bytes.extend_from_slice(&u64::try_from(value).unwrap_or(u64::MAX).to_le_bytes());
    }
    bytes.extend_from_slice(&limits.max_terrain_query_cells.to_le_bytes());
    bytes.extend_from_slice(&limits.max_dof_coverage_cells.to_le_bytes());
    sha256(&bytes)
}

fn parse_dof_data(path: &Path, data: &[u8], limits: RegionalDataLimits) -> Result<Dof, String> {
    if u64::try_from(data.len()).unwrap_or(u64::MAX) > limits.max_dof_data_bytes {
        return Err(format!(
            "{}: DOF data exceeds the byte limit",
            path.display()
        ));
    }
    let lines = line_count(data);
    if lines
        > limits
            .max_dof_records
            .saturating_add(oa_dof::HEADER_LINES as usize)
    {
        return Err(format!(
            "{}: DOF line count exceeds the record limit",
            path.display()
        ));
    }
    let dof = parse_dof(data).map_err(|e| format!("{}: {e}", path.display()))?;
    if dof.lat_e7.is_empty() {
        return Err(format!(
            "{}: no obstacle in the file (an empty file is not coverage)",
            path.display()
        ));
    }
    if dof.lat_e7.len() > limits.max_dof_records {
        return Err(format!(
            "{}: DOF accepted record count exceeds the limit",
            path.display()
        ));
    }
    Ok(dof)
}

fn line_count(bytes: &[u8]) -> usize {
    let endings = bytes.iter().filter(|&&b| b == b'\n').count();
    endings.saturating_add(if bytes.last().is_some_and(|b| *b != b'\n') {
        1
    } else {
        0
    })
}

fn load_mva_directory(
    directory: Option<&Path>,
    sources: &mut Vec<RegionalSourceEvidence>,
) -> Result<(Option<oa_mva::Model>, usize), String> {
    let Some(directory) = directory else {
        return Ok((None, 0));
    };
    oa_sealed::refuse_io(directory).map_err(|e| e.to_string())?;
    let mut paths = Vec::new();
    for entry in
        std::fs::read_dir(directory).map_err(|e| format!("{}: {e}", directory.display()))?
    {
        let entry = entry.map_err(|e| format!("{}: {e}", directory.display()))?;
        let path = entry.path();
        if path.extension().is_some_and(|extension| extension == "xml") {
            if paths.len() >= DEFAULT_MAX_MVA_XML_FILES {
                return Err(format!(
                    "{}: more than {DEFAULT_MAX_MVA_XML_FILES} MVA XML files",
                    directory.display()
                ));
            }
            paths.push(path);
        }
    }
    paths.sort();
    if paths.is_empty() {
        return Err(format!("{}: no MVA XML files", directory.display()));
    }
    let mut model = oa_mva::Model::default();
    let mut parser = oa_mva::aixm::new_parser().map_err(|e| format!("MVA parser: {e}"))?;
    for path in &paths {
        let bytes =
            oa_mva::aixm::read_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
        sources.push(evidence(RegionalSourceKind::MvaChart, path, &bytes, 0));
        oa_mva::aixm::load_message(&mut model, &mut parser, &bytes)
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok((Some(model), paths.len()))
}

fn load_cifp_corridors(
    path: Option<&Path>,
    settings: &Settings,
    max_bytes: u64,
    sources: &mut Vec<RegionalSourceEvidence>,
) -> Result<
    (
        Vec<crate::cifp::CifpCorridor>,
        Vec<crate::floors::AirportArea>,
    ),
    String,
> {
    let Some(path) = path else {
        return Ok((Vec::new(), Vec::new()));
    };
    let source = read_bounded(path, max_bytes)?;
    sources.push(evidence(RegionalSourceKind::Cifp, path, &source, 0));
    let is_zip = path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"));
    let data = if is_zip {
        let limits = ZipLimits {
            max_entries: 10,
            max_entry_bytes: max_bytes,
            max_total_bytes: max_bytes,
            ..oa_zip::DEFAULT_LIMITS
        };
        let archive = Archive::parse_with_limits(&source, limits)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        archive
            .extract("FAACIFP18")
            .map_err(|e| format!("{}: {e}", path.display()))?
    } else {
        source
    };
    if u64::try_from(data.len()).unwrap_or(u64::MAX) > max_bytes {
        return Err(format!(
            "{}: CIFP member exceeds {max_bytes} bytes",
            path.display()
        ));
    }
    let cifp = oa_arinc424::parse_bytes(&data, &path.to_string_lossy())
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let mut corridors = Vec::new();
    let mut airports = Vec::new();
    let approach_airports = crate::cifp::airports_with_approach(&cifp);
    for airport in &settings.cifp_airports {
        let made =
            crate::cifp::corridors_from_cifp_with(&cifp, airport, settings.approach_half_width_nm)?;
        if made.corridors.is_empty() {
            return Err(format!(
                "CIFP airport {airport} supplied no usable approach/runway corridors"
            ));
        }
        corridors.extend(made.corridors);
        let Some((_, lat, lon, elevation)) =
            approach_airports.iter().find(|item| &item.0 == airport)
        else {
            return Err(format!(
                "CIFP airport {airport} has corridors but no approach reference point"
            ));
        };
        airports.push(crate::floors::AirportArea {
            lat: *lat,
            lon: *lon,
            radius_m: settings.airport_area_nm * 1852.0,
            ceiling_ft: elevation.saturating_add(settings.airport_area_ceiling_ft_agl),
            clearance_ft: settings.airport_area_clearance_ft,
        });
    }
    if corridors.len() > oa_msaw::MAX_CORRIDORS {
        return Err("CIFP corridors exceed the fixed MSAW corridor ceiling".into());
    }
    Ok((corridors, airports))
}

fn extract_dof_bounded(
    path: &Path,
    source: Vec<u8>,
    limits: RegionalDataLimits,
) -> Result<(Vec<u8>, RegionalSourceEvidence), String> {
    let zip = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("zip"));
    let source_evidence = evidence(RegionalSourceKind::Dof, path, &source, 0);
    if zip {
        let zip_limits = ZipLimits {
            max_entries: 32,
            max_entry_bytes: limits.max_dof_data_bytes,
            max_total_bytes: limits.max_dof_data_bytes,
            max_ratio: oa_zip::MAX_RATIO,
            ratio_exempt_bytes: oa_zip::RATIO_EXEMPT_BYTES,
            max_name_bytes: oa_zip::MAX_NAME_BYTES,
        };
        let archive = Archive::parse_with_limits(&source, zip_limits)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let entry = archive
            .entries()
            .iter()
            .find(|e| e.name() == "DOF.DAT")
            .ok_or_else(|| format!("{}: zip has no DOF.DAT", path.display()))?;
        if entry.uncompressed_size() > limits.max_dof_data_bytes {
            return Err(format!(
                "{}: DOF.DAT exceeds the decompressed byte limit",
                path.display()
            ));
        }
        let data = archive
            .extract_entry(entry)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Ok((data, source_evidence))
    } else {
        Ok((source, source_evidence))
    }
}

fn coverage_cell_estimate(d: &Dof) -> u64 {
    let cell = |x: i32| ((f64::from(x) / 1e7 / 0.5).floor()) as i64;
    let bounds = |values: &[i32]| {
        values
            .iter()
            .copied()
            .min()
            .zip(values.iter().copied().max())
    };
    let (Some((south, north)), Some((west, east))) = (bounds(&d.lat_e7), bounds(&d.lon_e7)) else {
        return 0;
    };
    let rows = cell(north) - cell(south) + 1;
    let cols = cell(east) - cell(west) + 1;
    if rows <= 0 || cols <= 0 {
        u64::MAX
    } else {
        (rows as u64).saturating_mul(cols as u64)
    }
}

fn load_areas(
    settings: &Settings,
    limits: RegionalDataLimits,
    sources: &mut Vec<RegionalSourceEvidence>,
) -> Result<Vec<(Area, i32)>, String> {
    let mut out = Vec::with_capacity(settings.gtm_areas.len());
    let mut total_vertices = 0usize;
    for a in &settings.gtm_areas {
        let shape = match &a.shape {
            GtmShape::Box(s, n, w, e) => Area::Box(
                f64::from(*s) / 1e7,
                f64::from(*n) / 1e7,
                f64::from(*w) / 1e7,
                f64::from(*e) / 1e7,
            ),
            GtmShape::File(name) => {
                let path = Path::new(name);
                let bytes = read_bounded(path, limits.max_gtm_polygon_bytes)?;
                sources.push(evidence(RegionalSourceKind::GtmPolygon, path, &bytes, 0));
                let text = String::from_utf8(bytes)
                    .map_err(|_| format!("{name}: polygon file is not UTF-8"))?;
                let mut vertices = Vec::new();
                for line in text.lines() {
                    let line = line.split('#').next().unwrap_or("").trim();
                    if line.is_empty() {
                        continue;
                    }
                    if total_vertices.saturating_add(vertices.len())
                        >= limits.max_gtm_polygon_vertices
                    {
                        return Err(format!("{name}: polygon vertex limit exceeded"));
                    }
                    let (lat, lon): (f64, f64) = line
                        .split_once(',')
                        .and_then(|(lat, lon)| {
                            Some((lat.trim().parse().ok()?, lon.trim().parse().ok()?))
                        })
                        .ok_or_else(|| format!("{name}: `{line}` is not lat,lon"))?;
                    if !lat.is_finite()
                        || !lon.is_finite()
                        || !(-90.0..=90.0).contains(&lat)
                        || !(-180.0..=180.0).contains(&lon)
                    {
                        return Err(format!("{name}: `{line}` is outside coordinate bounds"));
                    }
                    vertices.push((lat, lon));
                }
                if vertices.len() < 3 {
                    return Err(format!("{name}: a polygon needs 3 or more points"));
                }
                total_vertices = total_vertices.saturating_add(vertices.len());
                Area::Polygon(vertices)
            }
        };
        out.push((shape, a.ft));
    }
    Ok(out)
}

/// Reads and constructs one bounded regional service. Weather receipt stamps and the issue cutoff
/// are explicit; later METAR observations are excluded even when the file itself was admitted.
pub fn load_regional_data(
    paths: &RegionalDataPaths,
    limits: RegionalDataLimits,
    weather: WeatherAdmission,
) -> Result<RegionalData, String> {
    checked_limits(limits)?;
    if paths.dof_files.len() > limits.max_dof_files {
        return Err(format!("more than {} DOF files", limits.max_dof_files));
    }
    if paths.metar_files.len() > limits.max_metar_files {
        return Err(format!("more than {} METAR files", limits.max_metar_files));
    }
    if let Some(dir) = &paths.terrain_dir {
        crate::guard::refuse_sealed(Sealed::production(), dir)?;
        if !dir.is_dir() {
            return Err(format!(
                "{}: terrain path is not a directory",
                dir.display()
            ));
        }
    }
    if let Some(dir) = &paths.mva_dir {
        crate::guard::refuse_sealed(Sealed::production(), dir)?;
        if !dir.is_dir() {
            return Err(format!(
                "{}: MVA chart path is not a directory",
                dir.display()
            ));
        }
    }

    let config_bytes = read_bounded(&paths.config, limits.max_config_bytes)?;
    let config_text = String::from_utf8(config_bytes.clone())
        .map_err(|_| format!("{}: not UTF-8 text", paths.config.display()))?;
    let mut settings =
        parse_config(&config_text).map_err(|e| format!("{}: {e}", paths.config.display()))?;
    if settings.cifp_area_margin_nm.is_some() {
        return Err("CIFP area-margin corridors need an explicit bounded regional extent".into());
    }
    if settings.cifp_airports.is_empty() != paths.cifp_file.is_none() {
        return Err(
            "CIFP source and explicit cifp_airport settings must be supplied together".into(),
        );
    }
    let mut sources = vec![evidence(
        RegionalSourceKind::Config,
        &paths.config,
        &config_bytes,
        0,
    )];
    let areas = load_areas(&settings, limits, &mut sources)?;
    let (chart, mva_chart_files) = load_mva_directory(paths.mva_dir.as_deref(), &mut sources)?;
    let (cifp_corridors, cifp_airports) = load_cifp_corridors(
        paths.cifp_file.as_deref(),
        &settings,
        limits.max_cifp_bytes,
        &mut sources,
    )?;
    let mut corridors = settings.msaw.corridors.into_vec();
    if corridors.len().saturating_add(cifp_corridors.len()) > oa_msaw::MAX_CORRIDORS {
        return Err(
            "explicit MSAW corridors plus CIFP corridors exceed the fixed corridor ceiling".into(),
        );
    }
    corridors.extend(cifp_corridors.iter().map(|item| item.corridor));
    settings.msaw.corridors = corridors.into_boxed_slice();

    let mut obstacles: Option<ObstacleIndex> = None;
    let mut dof_total_records = 0usize;
    let mut dof_total_coverage = 0u64;
    let mut dof_compressed_total = 0u64;
    let mut dof_inputs = Vec::with_capacity(paths.dof_files.len());
    for path in &paths.dof_files {
        let source_bytes = read_bounded(path, limits.max_dof_compressed_file_bytes)?;
        dof_compressed_total = dof_compressed_total
            .saturating_add(u64::try_from(source_bytes.len()).unwrap_or(u64::MAX));
        if dof_compressed_total > limits.max_dof_compressed_total_bytes {
            return Err("combined DOF compressed byte limit exceeded".into());
        }
        dof_inputs.push((path.clone(), source_bytes));
    }
    let mut decoded_dof = Vec::with_capacity(dof_inputs.len());
    let mut dof_total_data = 0u64;
    let mut dof_line_upper_bound = 0usize;
    for (path, source_bytes) in dof_inputs {
        let (data, source_evidence) = extract_dof_bounded(&path, source_bytes, limits)?;
        dof_total_data =
            dof_total_data.saturating_add(u64::try_from(data.len()).unwrap_or(u64::MAX));
        if dof_total_data > limits.max_dof_total_data_bytes {
            return Err("combined DOF decompressed byte limit exceeded".into());
        }
        let file_lines = line_count(&data);
        dof_line_upper_bound = dof_line_upper_bound
            .saturating_add(file_lines.saturating_sub(oa_dof::HEADER_LINES as usize));
        if dof_line_upper_bound > limits.max_dof_records {
            return Err("combined DOF line-count limit exceeded before parse".into());
        }
        decoded_dof.push((path, source_evidence, data));
    }
    let mut parsed_dof = Vec::with_capacity(decoded_dof.len());
    for (path, source_evidence, data) in decoded_dof {
        let dof = parse_dof_data(&path, &data, limits)?;
        dof_total_records = dof_total_records.saturating_add(dof.lat_e7.len());
        if dof_total_records > limits.max_dof_records {
            return Err("combined DOF record limit exceeded".into());
        }
        dof_total_coverage = dof_total_coverage.saturating_add(coverage_cell_estimate(&dof));
        if dof_total_coverage > limits.max_dof_coverage_cells {
            return Err("DOF coverage index cell limit exceeded".into());
        }
        parsed_dof.push((source_evidence, dof));
    }
    for (source_evidence, dof) in parsed_dof {
        let index = obstacle_index(&dof);
        match obstacles.as_mut() {
            Some(all) => all.merge(index),
            None => obstacles = Some(index),
        }
        sources.push(source_evidence);
    }

    let mut metar_bytes_total = 0u64;
    let mut metar_data = Vec::with_capacity(paths.metar_files.len());
    for source in &paths.metar_files {
        if source.received_at_ms > weather.cutoff_ms {
            return Err(format!(
                "{}: METAR file received after issue cutoff",
                source.path.display()
            ));
        }
        let bytes = read_bounded(&source.path, limits.max_metar_file_bytes)?;
        metar_bytes_total =
            metar_bytes_total.saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        if metar_bytes_total > limits.max_metar_total_bytes {
            return Err("combined METAR byte limit exceeded".into());
        }
        sources.push(evidence(
            RegionalSourceKind::Metar,
            &source.path,
            &bytes,
            source.received_at_ms,
        ));
        metar_data.push((
            source.path.to_string_lossy().into_owned(),
            bytes,
            source.received_at_ms,
        ));
    }
    let metar_refs: Vec<(&str, &[u8], i64)> = metar_data
        .iter()
        .map(|(path, bytes, received)| (path.as_str(), bytes.as_slice(), *received))
        .collect();
    let qnh = Qnh::from_admitted_bytes(
        &metar_refs,
        weather.cutoff_ms,
        settings.metar_radius_nm,
        settings.metar_max_age_s,
    )?;

    let obstacle_source_present = obstacles.is_some();
    let mut floors = Floors::new(paths.terrain_dir.clone(), obstacles, chart);
    floors.set_terrain_limits(TerrainServiceLimits {
        max_tiles: limits.max_terrain_tiles,
        max_file_bytes: limits.max_terrain_file_bytes,
        max_grid_bytes: limits.max_terrain_grid_bytes,
        max_working_bytes: limits.max_terrain_working_bytes,
        max_resident_bytes: limits.max_terrain_resident_bytes,
        max_query_cells: limits.max_terrain_query_cells,
    });
    floors.set_areas(areas);
    floors.set_airports(cifp_airports);
    floors.set_corridors(&settings.msaw);

    let manifest_sha256 = manifest_hash(
        weather.cutoff_ms,
        &sources,
        limits,
        paths.terrain_dir.is_some(),
        paths.mva_dir.is_some(),
        paths.cifp_file.is_some(),
        mva_chart_files,
        cifp_corridors.len(),
        paths.dof_files.len(),
        paths.metar_files.len(),
    );
    let provenance = RegionalProvenance {
        cutoff_ms: weather.cutoff_ms,
        sources,
        manifest_sha256,
        terrain_dir_present: paths.terrain_dir.is_some(),
        mva_chart_files,
        cifp_corridors: cifp_corridors.len(),
        obstacle_source_present,
        chart_available: floors.has_chart(),
        metar_source_files: paths.metar_files.len(),
        dof_records: dof_total_records,
        metar_rows: qnh.stats.rows,
        metar_stations: qnh.stats.stations,
    };
    Ok(RegionalData {
        settings,
        floors,
        qnh,
        provenance,
    })
}

pub fn source_digest_hex(source: &RegionalSourceEvidence) -> String {
    to_hex(&source.sha256)
}
