#![forbid(unsafe_code)]
//! Counts explicit surface flags in a bounded adsb.lol trace archive. Shell-only offline census;
//! this is not runway occupancy or a runtime safety product.
//!
//! The input is a private `.complete` manifest with this UTF-8 format:
//!
//! ```text
//! openatca-adsb-surface-complete-v1
//! release_tag=v2026.11.24-planes-readsb-prod-0
//! release_manifest_sha256=<64 lowercase hex digits>
//! from=2026-11-24T00:00:00Z
//! to=2026-11-25T00:00:00Z
//! bbox_source=<source name or URL>
//! bbox_version=<version>
//! bbox_license=<license>
//! bbox_effective_date=<YYYY-MM-DD>
//! bbox_source_sha256=<64 lowercase hex digits>
//! bbox_e7=<lat_min>,<lat_max>,<lon_min>,<lon_max>
//! bbox_sha256=<hash of canonical bbox provenance fields>
//! bin_seconds=10
//! part=<relative-file-name>\t<expected-bytes>\t<expected-sha256>
//! ```
//!
//! The coordinator authors this manifest only after checking the public release listing. This
//! program independently verifies every part's byte length and SHA-256 before decoding. Its bbox
//! is an analysis rectangle, not an airport boundary. A located-position-bin fraction is not a
//! fraction of all aircraft: coordinate-less trace rows cannot be assigned to this rectangle.

use std::collections::BTreeMap;
use std::fmt::Write as FmtWrite;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use oa_adsb::{
    reports_len, reports_row, scan_archive_counted, Filter, Reports, Row, ScanEnd, ScanError,
    ScanResult, F_GROUND, F_STALE, NONE_I32, SOURCE_NAMES,
};

const MANIFEST_LIMIT: u64 = 64 * 1024;
const MAX_PARTS: usize = 16;
const MAX_PART_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_TOTAL_ARCHIVE_BYTES: u64 = 64 * 1024 * 1024 * 1024;
const BIN_SECONDS: i64 = 10;
const DAY_SECONDS: i64 = 86_400;
const BIN_COUNT: usize = (DAY_SECONDS / BIN_SECONDS) as usize;
const MAX_REPORT_ROWS: u64 = 10_000_000;
const MAX_ADDRESS_BINS: usize = 250_000;

const S_GROUND: u8 = 1;
const S_ALTITUDE: u8 = 2;
const S_UNKNOWN: u8 = 4;
const S_STALE: u8 = 8;
const S_GROUND_STALE: u8 = 16;
const S_DIRECT_ADSB: u8 = 32;
const S_OTHER_SOURCE: u8 = 64;

#[derive(Clone, Copy, Default)]
#[repr(C)]
struct Bin {
    report_rows: u64,
    ground_rows: u64,
    altitude_rows_no_ground: u64,
    unknown_ground_rows: u64,
    stale_rows: u64,
    stale_ground_rows: u64,
    direct_adsb_rows: u64,
    other_or_unknown_source_rows: u64,
}
// Budget: one plain-data summary record per fixed 10-second UTC bin.
const _: () = assert!(core::mem::size_of::<Bin>() == 64);

#[derive(Clone, Copy, Default)]
#[repr(C)]
struct AddressBinCounts {
    positioned_address_bins: u64,
    explicit_ground_address_bins: u64,
    altitude_present_no_ground_address_bins: u64,
    unknown_ground_state_address_bins: u64,
    mixed_position_state_address_bins: u64,
    stale_address_bins: u64,
    stale_ground_address_bins: u64,
    direct_adsb_address_bins: u64,
    direct_adsb_ground_address_bins: u64,
    other_or_unknown_source_address_bins: u64,
    other_or_unknown_ground_address_bins: u64,
}
// Budget: one plain-data address-bin count record per fixed 10-second UTC bin.
const _: () = assert!(core::mem::size_of::<AddressBinCounts>() == 88);

// A7.2: cold manifest, parsed once before the archive scan.
struct Manifest {
    release_tag: String,
    release_manifest_sha256: String,
    bbox_source: String,
    bbox_version: String,
    bbox_license: String,
    bbox_effective_date: String,
    bbox_source_sha256: String,
    bbox_sha256: String,
    from_ns: i64,
    to_ns: i64,
    filter: Filter,
    parts: Vec<Part>,
    sha256: String,
}

// A7.2: cold part metadata, retained once during this one-shot shell scan.
struct Part {
    relative_path: PathBuf,
    expected_bytes: u64,
    expected_sha256: String,
    path: PathBuf,
}

fn checked_inc(value: &mut u64, by: u64, label: &str) -> Result<(), String> {
    *value = value
        .checked_add(by)
        .ok_or_else(|| format!("counter overflow: {label}"))?;
    Ok(())
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn valid_manifest_text(value: &str) -> bool {
    !value.trim().is_empty() && value.bytes().all(|byte| !byte.is_ascii_control())
}

fn valid_effective_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 10
        && bytes.get(4) == Some(&b'-')
        && bytes.get(7) == Some(&b'-')
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit())
}

fn parse_bbox(value: &str) -> Result<[i32; 4], String> {
    let mut it = value.split(',');
    let mut out = [0i32; 4];
    for item in &mut out {
        *item = it
            .next()
            .ok_or("bbox_e7 needs four comma-separated integers")?
            .parse::<i32>()
            .map_err(|_| "bbox_e7 contains a non-integer")?;
    }
    let [lat_min, lat_max, lon_min, lon_max] = out;
    if it.next().is_some()
        || !(-900_000_000..=900_000_000).contains(&lat_min)
        || !(-900_000_000..=900_000_000).contains(&lat_max)
        || !(-1_800_000_000..=1_800_000_000).contains(&lon_min)
        || !(-1_800_000_000..=1_800_000_000).contains(&lon_max)
        || lat_min > lat_max
        || lon_min > lon_max
    {
        return Err("bbox_e7 is out of range or reversed".into());
    }
    Ok(out)
}

fn safe_relative_path(value: &str) -> Result<PathBuf, String> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        || value.contains('|')
    {
        return Err(format!("part path is not a safe relative name: {value}"));
    }
    Ok(path.to_path_buf())
}

fn read_manifest(path: &Path) -> Result<Manifest, String> {
    let file = oa_sealed::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let bytes = file
        .metadata()
        .map_err(|e| format!("{}: {e}", path.display()))?
        .len();
    if bytes > MANIFEST_LIMIT {
        return Err(format!(
            "manifest is {bytes} bytes; limit is {MANIFEST_LIMIT}"
        ));
    }
    let mut text = String::new();
    file.take(MANIFEST_LIMIT + 1)
        .read_to_string(&mut text)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if text.len() as u64 > MANIFEST_LIMIT || !text.is_ascii() {
        return Err("manifest exceeds its limit or is not ASCII".into());
    }
    let digest = oa_sha256::to_hex(&oa_sha256::sha256(text.as_bytes()));
    let mut release_tag = None;
    let mut release_manifest_sha256 = None;
    let mut from = None;
    let mut to = None;
    let mut bbox = None;
    let mut bbox_source = None;
    let mut bbox_version = None;
    let mut bbox_license = None;
    let mut bbox_effective_date = None;
    let mut bbox_source_sha256 = None;
    let mut bbox_sha256 = None;
    let mut bin_seconds = None;
    let mut parts = Vec::new();
    let mut header_seen = false;
    for (line_number, line) in text.lines().enumerate() {
        if line_number == 0 {
            if line != "openatca-adsb-surface-complete-v1" {
                return Err("unsupported completion manifest header".into());
            }
            header_seen = true;
            continue;
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(value) = line.strip_prefix("release_tag=") {
            if release_tag.replace(value.to_string()).is_some() || value.is_empty() {
                return Err("duplicate or empty release_tag".into());
            }
        } else if let Some(value) = line.strip_prefix("release_manifest_sha256=") {
            if release_manifest_sha256
                .replace(value.to_ascii_lowercase())
                .is_some()
                || !valid_sha256(value)
            {
                return Err("duplicate or malformed release_manifest_sha256".into());
            }
        } else if let Some(value) = line.strip_prefix("from=") {
            if from.replace(value.to_string()).is_some() {
                return Err("duplicate from".into());
            }
        } else if let Some(value) = line.strip_prefix("to=") {
            if to.replace(value.to_string()).is_some() {
                return Err("duplicate to".into());
            }
        } else if let Some(value) = line.strip_prefix("bbox_source=") {
            if bbox_source.replace(value.to_string()).is_some() || !valid_manifest_text(value) {
                return Err("duplicate or empty bbox_source".into());
            }
        } else if let Some(value) = line.strip_prefix("bbox_version=") {
            if bbox_version.replace(value.to_string()).is_some() || !valid_manifest_text(value) {
                return Err("duplicate or empty bbox_version".into());
            }
        } else if let Some(value) = line.strip_prefix("bbox_license=") {
            if bbox_license.replace(value.to_string()).is_some() || !valid_manifest_text(value) {
                return Err("duplicate or empty bbox_license".into());
            }
        } else if let Some(value) = line.strip_prefix("bbox_effective_date=") {
            if bbox_effective_date.replace(value.to_string()).is_some()
                || !valid_effective_date(value)
            {
                return Err("duplicate or empty bbox_effective_date".into());
            }
        } else if let Some(value) = line.strip_prefix("bbox_source_sha256=") {
            if bbox_source_sha256
                .replace(value.to_ascii_lowercase())
                .is_some()
                || !valid_sha256(value)
            {
                return Err("duplicate or malformed bbox_source_sha256".into());
            }
        } else if let Some(value) = line.strip_prefix("bbox_e7=") {
            if bbox.replace(parse_bbox(value)?).is_some() {
                return Err("duplicate bbox_e7".into());
            }
        } else if let Some(value) = line.strip_prefix("bbox_sha256=") {
            if bbox_sha256.replace(value.to_ascii_lowercase()).is_some() || !valid_sha256(value) {
                return Err("duplicate or malformed bbox_sha256".into());
            }
        } else if let Some(value) = line.strip_prefix("bin_seconds=") {
            if bin_seconds.replace(value.to_string()).is_some() {
                return Err("duplicate bin_seconds".into());
            }
        } else if let Some(value) = line.strip_prefix("part=") {
            if parts.len() == MAX_PARTS {
                return Err(format!("manifest exceeds {MAX_PARTS} part limit"));
            }
            let mut fields = value.split('\t');
            let name = fields.next().ok_or("part is missing a path")?;
            let size = fields
                .next()
                .ok_or("part is missing expected byte length")?
                .parse::<u64>()
                .map_err(|_| "part byte length is malformed")?;
            let hash = fields.next().ok_or("part is missing SHA-256")?;
            if fields.next().is_some() || !valid_sha256(hash) {
                return Err("part has extra fields or malformed SHA-256".into());
            }
            let relative_path = safe_relative_path(name)?;
            if parts
                .iter()
                .any(|p: &Part| p.relative_path == relative_path)
            {
                return Err(format!("duplicate archive part {name}"));
            }
            let base = path.parent().ok_or("manifest path has no parent")?;
            parts.push(Part {
                path: base.join(&relative_path),
                relative_path,
                expected_bytes: size,
                expected_sha256: hash.to_ascii_lowercase(),
            });
        } else {
            return Err(format!(
                "manifest line {} has an unknown field",
                line_number + 1
            ));
        }
    }
    if !header_seen || parts.is_empty() {
        return Err("manifest header or archive part list is missing".into());
    }
    let from_text = from.ok_or("manifest is missing from")?;
    let to_text = to.ok_or("manifest is missing to")?;
    let from_s = oa_adsb::cli::parse_time(&from_text)?;
    let to_s = oa_adsb::cli::parse_time(&to_text)?;
    let duration_s = to_s
        .checked_sub(from_s)
        .ok_or("manifest time difference overflows")?;
    if duration_s != DAY_SECONDS || from_s.rem_euclid(DAY_SECONDS) != 0 {
        return Err("analysis window must be one UTC day beginning at midnight".into());
    }
    let bin_text = bin_seconds.ok_or("manifest is missing bin_seconds")?;
    if bin_text != BIN_SECONDS.to_string() {
        return Err("bin_seconds must be 10".into());
    }
    let [lat_min_e7, lat_max_e7, lon_min_e7, lon_max_e7] =
        bbox.ok_or("manifest is missing bbox_e7")?;
    let bbox_source = bbox_source.ok_or("manifest is missing bbox_source")?;
    let bbox_version = bbox_version.ok_or("manifest is missing bbox_version")?;
    let bbox_license = bbox_license.ok_or("manifest is missing bbox_license")?;
    let bbox_effective_date =
        bbox_effective_date.ok_or("manifest is missing bbox_effective_date")?;
    let bbox_source_sha256 = bbox_source_sha256.ok_or("manifest is missing bbox_source_sha256")?;
    let bbox_sha256 = bbox_sha256.ok_or("manifest is missing bbox_sha256")?;
    let bbox_canonical = format!(
        "source={bbox_source}\nversion={bbox_version}\nlicense={bbox_license}\neffective_date={bbox_effective_date}\nsource_sha256={bbox_source_sha256}\nbbox_e7={lat_min_e7},{lat_max_e7},{lon_min_e7},{lon_max_e7}\n"
    );
    let actual_bbox_sha256 = oa_sha256::to_hex(&oa_sha256::sha256(bbox_canonical.as_bytes()));
    if actual_bbox_sha256 != bbox_sha256 {
        return Err("bbox provenance and coordinates do not match bbox_sha256".into());
    }
    let release_tag = release_tag.ok_or("manifest is missing release_tag")?;
    if release_tag != "v2026.11.24-planes-readsb-prod-0" {
        return Err("release_tag must be the verified 2026.11.24 production archive tag".into());
    }
    let release_manifest_sha256 =
        release_manifest_sha256.ok_or("manifest is missing release_manifest_sha256")?;
    if !valid_sha256(&release_manifest_sha256) {
        return Err("release_manifest_sha256 is malformed".into());
    }
    Ok(Manifest {
        release_tag,
        release_manifest_sha256,
        bbox_source,
        bbox_version,
        bbox_license,
        bbox_effective_date,
        bbox_source_sha256,
        bbox_sha256,
        from_ns: from_s
            .checked_mul(1_000_000_000)
            .ok_or("from time overflows nanoseconds")?,
        to_ns: to_s
            .checked_mul(1_000_000_000)
            .ok_or("to time overflows nanoseconds")?,
        filter: Filter {
            lat_min_e7,
            lat_max_e7,
            lon_min_e7,
            lon_max_e7,
            from_ns: from_s
                .checked_mul(1_000_000_000)
                .ok_or("from time overflows nanoseconds")?,
            to_ns: to_s
                .checked_mul(1_000_000_000)
                .ok_or("to time overflows nanoseconds")?,
        },
        parts,
        sha256: digest,
    })
}

fn verify_parts(manifest: &Manifest) -> Result<Vec<BufReader<File>>, String> {
    let mut readers = Vec::with_capacity(manifest.parts.len());
    let mut total_bytes = 0u64;
    for part in &manifest.parts {
        let file =
            oa_sealed::open(&part.path).map_err(|e| format!("{}: {e}", part.path.display()))?;
        let actual_bytes = file
            .metadata()
            .map_err(|e| format!("{}: {e}", part.path.display()))?
            .len();
        if actual_bytes > MAX_PART_BYTES {
            return Err(format!(
                "{}: part exceeds {MAX_PART_BYTES} byte limit",
                part.relative_path.display()
            ));
        }
        total_bytes = total_bytes
            .checked_add(actual_bytes)
            .ok_or("archive byte total overflows")?;
        if total_bytes > MAX_TOTAL_ARCHIVE_BYTES {
            return Err(format!(
                "archive exceeds {MAX_TOTAL_ARCHIVE_BYTES} byte total limit"
            ));
        }
        if actual_bytes != part.expected_bytes {
            return Err(format!(
                "{}: byte length {actual_bytes} differs from release manifest {}",
                part.relative_path.display(),
                part.expected_bytes
            ));
        }
        let mut reader = BufReader::with_capacity(1 << 20, file);
        let digest = oa_sha256::sha256_reader(&mut reader)
            .map_err(|e| format!("{}: {e}", part.path.display()))?;
        let actual_sha256 = oa_sha256::to_hex(&digest);
        if actual_sha256 != part.expected_sha256 {
            return Err(format!(
                "{}: SHA-256 differs from completion manifest",
                part.relative_path.display()
            ));
        }
        reader
            .seek(SeekFrom::Start(0))
            .map_err(|e| format!("{}: {e}", part.path.display()))?;
        readers.push(reader);
    }
    Ok(readers)
}

// A3 exception: this bounded, one-shot shell index deduplicates address/bin pairs; it is not
// real-time core state and is discarded after writing the summary.
fn mark(
    map: &mut BTreeMap<(u32, u16), u8>,
    address: u32,
    bin: u16,
    state: u8,
) -> Result<(), String> {
    let key = (address, bin);
    if !map.contains_key(&key) && map.len() >= MAX_ADDRESS_BINS {
        return Err(format!(
            "more than {MAX_ADDRESS_BINS} positioned address bins; narrow bbox"
        ));
    }
    let flags = map.entry(key).or_default();
    *flags |= state;
    Ok(())
}

fn is_direct_adsb(source: u8) -> bool {
    SOURCE_NAMES
        .get(usize::from(source))
        .is_some_and(|name| matches!(*name, "adsb_icao" | "adsb_icao_nt" | "adsb_other"))
}

fn state_of(row: &Row) -> u8 {
    let ground = row.flags & F_GROUND != 0;
    let stale = row.flags & F_STALE != 0;
    let altitude = row.baro_alt_ft != NONE_I32 || row.geom_alt_ft != NONE_I32;
    let mut state = 0;
    if ground {
        // An explicitly ground-flagged aircraft can have a valid geometric elevation. That
        // elevation is not conflicting evidence of flight or an absent ground flag.
        state |= S_GROUND;
    } else if altitude {
        state |= S_ALTITUDE;
    } else {
        state |= S_UNKNOWN;
    }
    if stale {
        state |= S_STALE;
        if ground {
            state |= S_GROUND_STALE;
        }
    }
    state
}

fn add_reports(
    reports: &Reports,
    manifest: &Manifest,
    bins: &mut [Bin],
    address_bins: &mut BTreeMap<(u32, u16), u8>,
    total_rows: &mut u64,
) -> Result<(), String> {
    for index in 0..reports_len(reports) {
        let Some(row) = reports_row(reports, index) else {
            return Err("decoder report columns have inconsistent lengths".into());
        };
        checked_inc(total_rows, 1, "positioned reports")?;
        if *total_rows > MAX_REPORT_ROWS {
            return Err(format!(
                "more than {MAX_REPORT_ROWS} positioned reports; narrow bbox"
            ));
        }
        let relative_ns = row
            .time_ns
            .checked_sub(manifest.from_ns)
            .ok_or("report time precedes the requested day")?;
        if relative_ns < 0 || row.time_ns >= manifest.to_ns {
            return Err("decoder returned a report outside the requested day".into());
        }
        let bin_index = usize::try_from(relative_ns / (BIN_SECONDS * 1_000_000_000))
            .map_err(|_| "bin index conversion failed")?;
        let bin = bins
            .get_mut(bin_index)
            .ok_or("decoded report falls outside the fixed daily bin array")?;
        checked_inc(&mut bin.report_rows, 1, "bin report rows")?;
        let state = state_of(&row);
        let ground = state & S_GROUND != 0;
        let stale = state & S_STALE != 0;
        let altitude = state & S_ALTITUDE != 0;
        if ground {
            checked_inc(&mut bin.ground_rows, 1, "explicit ground rows")?;
        } else if altitude {
            checked_inc(&mut bin.altitude_rows_no_ground, 1, "altitude rows")?;
        } else {
            checked_inc(&mut bin.unknown_ground_rows, 1, "unknown ground rows")?;
        }
        if stale {
            checked_inc(&mut bin.stale_rows, 1, "stale rows")?;
            if ground {
                checked_inc(&mut bin.stale_ground_rows, 1, "stale ground rows")?;
            }
        }
        let direct = is_direct_adsb(row.source);
        if direct {
            checked_inc(&mut bin.direct_adsb_rows, 1, "direct ADS-B rows")?;
        } else {
            checked_inc(
                &mut bin.other_or_unknown_source_rows,
                1,
                "other source rows",
            )?;
        }
        let source_state = if direct {
            S_DIRECT_ADSB
        } else {
            S_OTHER_SOURCE
        };
        let bin_u16 = u16::try_from(bin_index).map_err(|_| "bin index exceeds u16")?;
        mark(address_bins, row.icao, bin_u16, state | source_state)?;
    }
    Ok(())
}

fn summarize_address_bins(
    address_bins: &BTreeMap<(u32, u16), u8>,
    per_bin: &mut [AddressBinCounts],
) -> Result<AddressBinCounts, String> {
    let mut total = AddressBinCounts::default();
    for (&(_, bin_index), &state) in address_bins {
        let bin = per_bin
            .get_mut(usize::from(bin_index))
            .ok_or("address-bin index exceeds daily bins")?;
        checked_inc(
            &mut bin.positioned_address_bins,
            1,
            "positioned address bins",
        )?;
        checked_inc(
            &mut total.positioned_address_bins,
            1,
            "positioned address bins",
        )?;
        let ground = state & S_GROUND != 0;
        let altitude = state & S_ALTITUDE != 0;
        let unknown = state & S_UNKNOWN != 0;
        let mixed = (ground && (altitude || unknown)) || (altitude && unknown);
        if mixed {
            checked_inc(
                &mut bin.mixed_position_state_address_bins,
                1,
                "mixed address bins",
            )?;
            checked_inc(
                &mut total.mixed_position_state_address_bins,
                1,
                "mixed address bins",
            )?;
        } else if ground {
            checked_inc(
                &mut bin.explicit_ground_address_bins,
                1,
                "ground address bins",
            )?;
            checked_inc(
                &mut total.explicit_ground_address_bins,
                1,
                "ground address bins",
            )?;
        } else if altitude {
            checked_inc(
                &mut bin.altitude_present_no_ground_address_bins,
                1,
                "altitude address bins",
            )?;
            checked_inc(
                &mut total.altitude_present_no_ground_address_bins,
                1,
                "altitude address bins",
            )?;
        } else {
            checked_inc(
                &mut bin.unknown_ground_state_address_bins,
                1,
                "unknown address bins",
            )?;
            checked_inc(
                &mut total.unknown_ground_state_address_bins,
                1,
                "unknown address bins",
            )?;
        }
        if state & S_STALE != 0 {
            checked_inc(&mut bin.stale_address_bins, 1, "stale address bins")?;
            checked_inc(&mut total.stale_address_bins, 1, "stale address bins")?;
        }
        if state & S_GROUND_STALE != 0 {
            checked_inc(
                &mut bin.stale_ground_address_bins,
                1,
                "stale ground address bins",
            )?;
            checked_inc(
                &mut total.stale_ground_address_bins,
                1,
                "stale ground address bins",
            )?;
        }
        if state & S_DIRECT_ADSB != 0 {
            checked_inc(
                &mut bin.direct_adsb_address_bins,
                1,
                "direct ADS-B address bins",
            )?;
            checked_inc(
                &mut total.direct_adsb_address_bins,
                1,
                "direct ADS-B address bins",
            )?;
            if ground && !mixed {
                checked_inc(
                    &mut bin.direct_adsb_ground_address_bins,
                    1,
                    "direct ADS-B ground bins",
                )?;
                checked_inc(
                    &mut total.direct_adsb_ground_address_bins,
                    1,
                    "direct ADS-B ground bins",
                )?;
            }
        }
        if state & S_OTHER_SOURCE != 0 {
            checked_inc(
                &mut bin.other_or_unknown_source_address_bins,
                1,
                "other source address bins",
            )?;
            checked_inc(
                &mut total.other_or_unknown_source_address_bins,
                1,
                "other source address bins",
            )?;
            if ground && !mixed {
                checked_inc(
                    &mut bin.other_or_unknown_ground_address_bins,
                    1,
                    "other source ground bins",
                )?;
                checked_inc(
                    &mut total.other_or_unknown_ground_address_bins,
                    1,
                    "other source ground bins",
                )?;
            }
        }
    }
    Ok(total)
}

fn render_output(
    manifest: &Manifest,
    scan: &ScanResult,
    report_bins: &[Bin],
    address_bins: &[AddressBinCounts],
    totals: AddressBinCounts,
    source_commit: &str,
) -> Result<String, String> {
    let mut output = String::new();
    writeln!(output, "openatca-adsb-surface-counts-v1").map_err(|e| e.to_string())?;
    writeln!(output, "release_tag={}", manifest.release_tag).map_err(|e| e.to_string())?;
    writeln!(output, "source_commit={source_commit}").map_err(|e| e.to_string())?;
    writeln!(
        output,
        "release_manifest_sha256={}",
        manifest.release_manifest_sha256
    )
    .map_err(|e| e.to_string())?;
    writeln!(output, "completion_manifest_sha256={}", manifest.sha256)
        .map_err(|e| e.to_string())?;
    writeln!(
        output,
        "decode_status={}",
        if scan.stats.traces_refused == 0 {
            "complete"
        } else {
            "partial_refused_trace_files_present"
        }
    )
    .map_err(|e| e.to_string())?;
    for part in &manifest.parts {
        writeln!(
            output,
            "verified_part={}|{}|{}",
            part.relative_path.display(),
            part.expected_bytes,
            part.expected_sha256
        )
        .map_err(|e| e.to_string())?;
    }
    writeln!(output, "window_from_unix_ns={}", manifest.from_ns).map_err(|e| e.to_string())?;
    writeln!(output, "window_to_unix_ns={}", manifest.to_ns).map_err(|e| e.to_string())?;
    writeln!(
        output,
        "analysis_bbox_e7={},{},{},{}",
        manifest.filter.lat_min_e7,
        manifest.filter.lat_max_e7,
        manifest.filter.lon_min_e7,
        manifest.filter.lon_max_e7
    )
    .map_err(|e| e.to_string())?;
    writeln!(output, "bbox_source={}", manifest.bbox_source).map_err(|e| e.to_string())?;
    writeln!(output, "bbox_version={}", manifest.bbox_version).map_err(|e| e.to_string())?;
    writeln!(output, "bbox_license={}", manifest.bbox_license).map_err(|e| e.to_string())?;
    writeln!(
        output,
        "bbox_effective_date={}",
        manifest.bbox_effective_date
    )
    .map_err(|e| e.to_string())?;
    writeln!(output, "bbox_source_sha256={}", manifest.bbox_source_sha256)
        .map_err(|e| e.to_string())?;
    writeln!(output, "bbox_sha256={}", manifest.bbox_sha256).map_err(|e| e.to_string())?;
    writeln!(output, "bin_seconds={BIN_SECONDS}").map_err(|e| e.to_string())?;
    writeln!(
        output,
        "runway_occupancy=unavailable_geometry_and_uncertainty_not_supplied"
    )
    .map_err(|e| e.to_string())?;
    writeln!(
        output,
        "taxiway_occupancy=unavailable_geometry_not_supplied"
    )
    .map_err(|e| e.to_string())?;
    writeln!(
        output,
        "denominator=distinct_address_utc_bins_with_valid_position_in_bbox"
    )
    .map_err(|e| e.to_string())?;
    writeln!(
        output,
        "denominator_scope=position_bearing_reports_only_not_all_aircraft"
    )
    .map_err(|e| e.to_string())?;
    writeln!(
        output,
        "address_semantics=transponder_keys_not_confirmed_aircraft_identities"
    )
    .map_err(|e| e.to_string())?;
    writeln!(output, "inter_sample_gap_semantics=unknown_not_clear").map_err(|e| e.to_string())?;
    writeln!(output, "rows_seen={}", scan.stats.rows_seen).map_err(|e| e.to_string())?;
    writeln!(output, "trace_entries={}", scan.trace_entries).map_err(|e| e.to_string())?;
    writeln!(output, "traces_read={}", scan.stats.traces_read).map_err(|e| e.to_string())?;
    writeln!(output, "traces_refused={}", scan.stats.traces_refused).map_err(|e| e.to_string())?;
    writeln!(output, "rows_kept_in_bbox_and_window={}", scan.stats.kept)
        .map_err(|e| e.to_string())?;
    writeln!(output, "rows_outside_bbox={}", scan.stats.outside_box).map_err(|e| e.to_string())?;
    writeln!(output, "rows_outside_window={}", scan.stats.outside_time)
        .map_err(|e| e.to_string())?;
    writeln!(
        output,
        "rows_rejected_shape={}",
        scan.stats.rejected.first().copied().unwrap_or_default()
    )
    .map_err(|e| e.to_string())?;
    writeln!(
        output,
        "rows_rejected_time={}",
        scan.stats.rejected.get(1).copied().unwrap_or_default()
    )
    .map_err(|e| e.to_string())?;
    writeln!(
        output,
        "rows_rejected_position={}",
        scan.stats.rejected.get(2).copied().unwrap_or_default()
    )
    .map_err(|e| e.to_string())?;
    writeln!(
        output,
        "rows_rejected_flags={}",
        scan.stats.rejected.get(3).copied().unwrap_or_default()
    )
    .map_err(|e| e.to_string())?;
    writeln!(
        output,
        "sealed_window_rows_dropped={}",
        scan.stats.sealed_window_dropped
    )
    .map_err(|e| e.to_string())?;
    writeln!(
        output,
        "position_report_rows={}",
        report_bins.iter().map(|b| b.report_rows).sum::<u64>()
    )
    .map_err(|e| e.to_string())?;
    writeln!(
        output,
        "explicit_ground_position_rows={}",
        report_bins.iter().map(|b| b.ground_rows).sum::<u64>()
    )
    .map_err(|e| e.to_string())?;
    writeln!(
        output,
        "altitude_present_no_ground_flag_rows={}",
        report_bins
            .iter()
            .map(|b| b.altitude_rows_no_ground)
            .sum::<u64>()
    )
    .map_err(|e| e.to_string())?;
    writeln!(
        output,
        "ground_state_unknown_position_rows={}",
        report_bins
            .iter()
            .map(|b| b.unknown_ground_rows)
            .sum::<u64>()
    )
    .map_err(|e| e.to_string())?;
    writeln!(
        output,
        "stale_position_rows={}",
        report_bins.iter().map(|b| b.stale_rows).sum::<u64>()
    )
    .map_err(|e| e.to_string())?;
    writeln!(
        output,
        "stale_explicit_ground_rows={}",
        report_bins.iter().map(|b| b.stale_ground_rows).sum::<u64>()
    )
    .map_err(|e| e.to_string())?;
    writeln!(
        output,
        "direct_adsb_source_position_rows={}",
        report_bins.iter().map(|b| b.direct_adsb_rows).sum::<u64>()
    )
    .map_err(|e| e.to_string())?;
    writeln!(
        output,
        "other_or_unknown_source_position_rows={}",
        report_bins
            .iter()
            .map(|b| b.other_or_unknown_source_rows)
            .sum::<u64>()
    )
    .map_err(|e| e.to_string())?;
    writeln!(
        output,
        "positioned_address_bins={}",
        totals.positioned_address_bins
    )
    .map_err(|e| e.to_string())?;
    writeln!(
        output,
        "explicit_ground_address_bins={}",
        totals.explicit_ground_address_bins
    )
    .map_err(|e| e.to_string())?;
    writeln!(
        output,
        "altitude_present_no_ground_address_bins={}",
        totals.altitude_present_no_ground_address_bins
    )
    .map_err(|e| e.to_string())?;
    writeln!(
        output,
        "unknown_ground_state_address_bins={}",
        totals.unknown_ground_state_address_bins
    )
    .map_err(|e| e.to_string())?;
    writeln!(
        output,
        "mixed_position_state_address_bins={}",
        totals.mixed_position_state_address_bins
    )
    .map_err(|e| e.to_string())?;
    writeln!(output, "stale_address_bins={}", totals.stale_address_bins)
        .map_err(|e| e.to_string())?;
    writeln!(
        output,
        "stale_ground_address_bins={}",
        totals.stale_ground_address_bins
    )
    .map_err(|e| e.to_string())?;
    writeln!(
        output,
        "direct_adsb_source_address_bins={}",
        totals.direct_adsb_address_bins
    )
    .map_err(|e| e.to_string())?;
    writeln!(
        output,
        "other_or_unknown_source_address_bins={}",
        totals.other_or_unknown_source_address_bins
    )
    .map_err(|e| e.to_string())?;
    writeln!(
        output,
        "observed_explicit_ground_flag_position_bin_fraction_including_stale={}/{}",
        totals.explicit_ground_address_bins, totals.positioned_address_bins
    )
    .map_err(|e| e.to_string())?;
    writeln!(
        output,
        "direct_adsb_observed_ground_flag_bin_fraction_including_stale={}/{}",
        totals.direct_adsb_ground_address_bins, totals.direct_adsb_address_bins
    )
    .map_err(|e| e.to_string())?;
    writeln!(
        output,
        "other_or_unknown_source_observed_ground_flag_bin_fraction_including_stale={}/{}",
        totals.other_or_unknown_ground_address_bins, totals.other_or_unknown_source_address_bins
    )
    .map_err(|e| e.to_string())?;
    writeln!(output, "coverage_claim=not_all_aircraft;archive_is_thinned_and_contains_no_ground_truth_for_nonreports")
        .map_err(|e| e.to_string())?;
    writeln!(output, "time_bin,position_rows,explicit_ground_rows,altitude_no_ground_rows,unknown_ground_rows,stale_rows,stale_ground_rows,direct_adsb_rows,other_source_rows,positioned_address_bins,explicit_ground_address_bins,altitude_no_ground_address_bins,unknown_ground_address_bins,mixed_position_state_address_bins,stale_address_bins,direct_adsb_address_bins,direct_adsb_ground_address_bins,other_source_address_bins,other_source_ground_address_bins")
        .map_err(|e| e.to_string())?;
    for (index, (rows, addresses)) in report_bins.iter().zip(address_bins).enumerate() {
        if rows.report_rows == 0 && addresses.positioned_address_bins == 0 {
            continue;
        }
        let offset = i64::try_from(index)
            .map_err(|_| "bin index conversion failed")?
            .checked_mul(BIN_SECONDS)
            .ok_or("bin offset overflows")?;
        let at = (manifest.from_ns / 1_000_000_000)
            .checked_add(offset)
            .ok_or("bin start time overflows")?;
        writeln!(
            output,
            "{at},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
            rows.report_rows,
            rows.ground_rows,
            rows.altitude_rows_no_ground,
            rows.unknown_ground_rows,
            rows.stale_rows,
            rows.stale_ground_rows,
            rows.direct_adsb_rows,
            rows.other_or_unknown_source_rows,
            addresses.positioned_address_bins,
            addresses.explicit_ground_address_bins,
            addresses.altitude_present_no_ground_address_bins,
            addresses.unknown_ground_state_address_bins,
            addresses.mixed_position_state_address_bins,
            addresses.stale_address_bins,
            addresses.direct_adsb_address_bins,
            addresses.direct_adsb_ground_address_bins,
            addresses.other_or_unknown_source_address_bins,
            addresses.other_or_unknown_ground_address_bins
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(output)
}

fn create_new(path: &Path) -> Result<File, String> {
    oa_sealed::refuse(path)?;
    // seal-allow: write-only count output after the path check; create_new neither reads nor overwrites existing data
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn valid_commit(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn run(
    manifest_path: &Path,
    output_path: &Path,
    source_commit: &str,
    drops: &mut oa_sealed::DropReport,
) -> Result<(), String> {
    if !valid_commit(source_commit) {
        return Err("source commit must be a 40-character hexadecimal hash".into());
    }
    let manifest = read_manifest(manifest_path)?;
    let parts = verify_parts(&manifest)?;
    // A3 exception: these fixed daily output bins are cold shell summaries, each written once.
    let mut bins = vec![Bin::default(); BIN_COUNT];
    // A3 exception: bounded one-shot dedup index, not a hot runtime collection.
    let mut address_bins = BTreeMap::new();
    let mut total_rows = 0u64;
    let mut sealed_rows = 0u64;
    let scan = scan_archive_counted(
        parts,
        &manifest.filter,
        &mut |reports: &Reports| {
            add_reports(
                reports,
                &manifest,
                &mut bins,
                &mut address_bins,
                &mut total_rows,
            )
        },
        &mut sealed_rows,
    );
    drops.add(sealed_rows);
    let scan = match scan {
        Ok(result) => result,
        Err(ScanError::Sink(error)) => return Err(error),
        Err(ScanError::Tar(error)) => return Err(error.to_string()),
    };
    if !matches!(scan.end, ScanEnd::Complete) {
        return Err("archive reader reports a truncated archive; no output written".into());
    }
    if scan.stats.sealed_window_dropped > 0 || sealed_rows > 0 {
        return Err(format!(
            "{} row(s) intersect a sealed window; no output written",
            scan.stats.sealed_window_dropped.max(sealed_rows)
        ));
    }
    // A3 exception: one-shot presentation totals indexed by the same fixed daily bin.
    let mut address_counts = vec![AddressBinCounts::default(); BIN_COUNT];
    let totals = summarize_address_bins(&address_bins, &mut address_counts)?;
    let text = render_output(
        &manifest,
        &scan,
        &bins,
        &address_counts,
        totals,
        source_commit,
    )?;
    let output_hash = oa_sha256::to_hex(&oa_sha256::sha256(text.as_bytes()));
    let mut file = create_new(output_path)?;
    if let Err(error) = file.write_all(text.as_bytes()).and_then(|()| file.flush()) {
        drop(file);
        let _ = std::fs::remove_file(output_path);
        return Err(format!("{}: {error}", output_path.display()));
    }
    drop(file);
    let sidecar_path = PathBuf::from(format!("{}.source", output_path.display()));
    let sidecar = format!(
        "openatca-adsb-surface-counts-source-v1\nsource_commit={source_commit}\noutput_sha256={output_hash}\ncompletion_manifest_sha256={}\nrelease_manifest_sha256={}\nrelease_tag={}\n",
        manifest.sha256, manifest.release_manifest_sha256, manifest.release_tag
    );
    let mut source_file = match create_new(&sidecar_path) {
        Ok(file) => file,
        Err(error) => {
            let _ = std::fs::remove_file(output_path);
            return Err(error);
        }
    };
    if let Err(error) = source_file
        .write_all(sidecar.as_bytes())
        .and_then(|()| source_file.flush())
    {
        drop(source_file);
        let _ = std::fs::remove_file(&sidecar_path);
        let _ = std::fs::remove_file(output_path);
        return Err(format!("{}: {error}", sidecar_path.display()));
    }
    Ok(())
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let manifest = args.next();
    let output = args.next();
    let source_commit = args.next();
    if manifest.is_none() || output.is_none() || source_commit.is_none() || args.next().is_some() {
        eprintln!("usage: cargo run --release -p oa-adsb --example surface_counts -- <private.complete> <private-output.txt> <source-commit>");
        return ExitCode::from(2);
    }
    let mut drops = oa_sealed::DropReport::new();
    let result = run(
        Path::new(manifest.as_deref().unwrap_or("")),
        Path::new(output.as_deref().unwrap_or("")),
        source_commit.as_deref().unwrap_or(""),
        &mut drops,
    );
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("surface_counts: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod synthetic_tests {
    use super::*;

    // Source-only cases exercise the same classifier used by the archive callback.
    // REQ: OA-ADSB-176
    #[test]
    fn surface_flag_and_unknown_altitude_remain_distinct() {
        let mut row = oa_adsb::ROW_EMPTY;
        row.baro_alt_ft = NONE_I32;
        row.geom_alt_ft = NONE_I32;
        assert_eq!(state_of(&row), S_UNKNOWN);
        row.flags = F_GROUND;
        assert_eq!(state_of(&row), S_GROUND);
        row.flags = F_STALE;
        row.geom_alt_ft = 100;
        assert_eq!(state_of(&row), S_ALTITUDE | S_STALE);
        row.flags = F_GROUND | F_STALE;
        row.geom_alt_ft = NONE_I32;
        assert_eq!(state_of(&row), S_GROUND | S_STALE | S_GROUND_STALE);
        row.geom_alt_ft = 100;
        assert_eq!(state_of(&row), S_GROUND | S_STALE | S_GROUND_STALE);
    }

    // REQ: OA-ADSB-176
    #[test]
    fn denominator_counts_distinct_position_bearing_address_bins() {
        let mut address_bins = BTreeMap::new();
        assert!(mark(&mut address_bins, 0xabc123, 7, S_GROUND).is_ok());
        assert!(mark(&mut address_bins, 0xabc123, 7, S_GROUND).is_ok());
        assert!(mark(&mut address_bins, 0xdef456, 7, S_UNKNOWN).is_ok());
        let mut by_time = vec![AddressBinCounts::default(); BIN_COUNT];
        let result = summarize_address_bins(&address_bins, &mut by_time);
        assert!(result.is_ok());
        let totals = result.unwrap_or_default();
        assert_eq!(totals.positioned_address_bins, 2);
        assert_eq!(totals.explicit_ground_address_bins, 1);
        assert_eq!(totals.unknown_ground_state_address_bins, 1);
        assert_eq!(
            by_time.get(7).map(|bin| bin.positioned_address_bins),
            Some(2)
        );
    }
}
