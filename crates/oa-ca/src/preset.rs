//! Versioned per-facility conflict-alert adaptation data. Core: parses a bounded canonical body
//! into plain values; file access and manifest verification remain in the shell.

use crate::state::check_volume;
use crate::types::{Area, CaConfig, Volume, MAX_VOLUMES};
use oa_safety::geom::{polygon_new, MAX_VERTICES};

pub const CA_PRESET_MAX_BYTES: usize = 65_536;
const FACILITY_BYTES: usize = 8;

/// The minima and alert-control policy issued for one facility. The package manifest separately
/// binds these exact bytes to the facility, adaptation version and SHA-256.
#[derive(Clone, Copy, Debug, PartialEq)]
// A7.2: cold adaptation data, bounded by the companion component schema.
pub struct CaPreset {
    pub facility: [u8; FACILITY_BYTES],
    pub facility_len: u8,
    pub default_volume: Volume,
    pub volumes: [Volume; MAX_VOLUMES],
    pub n_volumes: usize,
    pub max_alert_suppression_ns: Option<i64>,
    pub allow_military_group_suppression: bool,
}

fn parse_i32(s: &str) -> Result<i32, &'static str> {
    s.parse::<i32>().map_err(|_| "invalid integer")
}

fn parse_f64(s: &str) -> Result<f64, &'static str> {
    let value = s.parse::<f64>().map_err(|_| "invalid number")?;
    if !value.is_finite() {
        return Err("number is not finite");
    }
    Ok(value)
}

fn volume(line: &str) -> Result<Volume, &'static str> {
    let mut fields = line.split(',');
    let kind = fields.next().ok_or("volume kind")?;
    let (area, alt_lo_ft, alt_hi_ft, h_m, v_ft) = match kind {
        "box" => {
            let lat_min_e7 = parse_i32(fields.next().ok_or("box latitude minimum")?)?;
            let lat_max_e7 = parse_i32(fields.next().ok_or("box latitude maximum")?)?;
            let lon_min_e7 = parse_i32(fields.next().ok_or("box longitude minimum")?)?;
            let lon_max_e7 = parse_i32(fields.next().ok_or("box longitude maximum")?)?;
            let low = parse_i32(fields.next().ok_or("altitude minimum")?)?;
            let high = parse_i32(fields.next().ok_or("altitude maximum")?)?;
            let h = parse_f64(fields.next().ok_or("horizontal minimum")?)?;
            let z = parse_f64(fields.next().ok_or("vertical minimum")?)?;
            if fields.next().is_some() {
                return Err("too many box fields");
            }
            (
                Area::Box {
                    lat_min_e7,
                    lat_max_e7,
                    lon_min_e7,
                    lon_max_e7,
                },
                low,
                high,
                h,
                z,
            )
        }
        "polygon" => {
            let low = parse_i32(fields.next().ok_or("altitude minimum")?)?;
            let high = parse_i32(fields.next().ok_or("altitude maximum")?)?;
            let h = parse_f64(fields.next().ok_or("horizontal minimum")?)?;
            let z = parse_f64(fields.next().ok_or("vertical minimum")?)?;
            let mut vertices = [(0i32, 0i32); MAX_VERTICES];
            let mut count = 0usize;
            for field in fields {
                if count >= MAX_VERTICES {
                    return Err("polygon vertex count");
                }
                let (lat, lon) = field.split_once(':').ok_or("polygon vertex separator")?;
                let Some(slot) = vertices.get_mut(count) else {
                    return Err("polygon vertex count");
                };
                *slot = (parse_i32(lat)?, parse_i32(lon)?);
                count += 1;
            }
            let polygon = polygon_new(vertices.get(..count).ok_or("polygon vertices")?)
                .map_err(|_| "invalid polygon")?;
            (Area::Polygon(polygon), low, high, h, z)
        }
        _ => return Err("unknown volume kind"),
    };
    let v = Volume {
        area,
        alt_lo_ft,
        alt_hi_ft,
        h_m,
        v_ft,
    };
    check_volume(&v)?;
    Ok(v)
}

/// Parses the `OACA-PRESET-1` body. Lines and keys have fixed order, duplicate/unknown fields and
/// non-printable bytes are refused, volumes are bounded, and all numeric/geometry checks are the
/// same as `ca_check` uses for live configuration.
pub fn ca_preset_parse(bytes: &[u8], expected_facility: &str) -> Result<CaPreset, &'static str> {
    if bytes.is_empty()
        || bytes.len() > CA_PRESET_MAX_BYTES
        || !bytes.ends_with(b"\n")
        || !bytes
            .iter()
            .all(|b| *b == b'\n' || (0x20..=0x7e).contains(b))
    {
        return Err("preset byte limit or encoding");
    }
    let text = core::str::from_utf8(bytes).map_err(|_| "preset encoding")?;
    let mut lines = text.lines();
    if lines.next() != Some("OACA-PRESET-1") {
        return Err("preset header");
    }
    let facility_line = lines.next().ok_or("preset facility")?;
    let facility = facility_line
        .strip_prefix("facility=")
        .ok_or("preset facility")?;
    if facility != expected_facility
        || facility.is_empty()
        || facility.len() > FACILITY_BYTES
        || !facility
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    {
        return Err("preset facility mismatch or invalid identifier");
    }
    let default_line = lines.next().ok_or("preset default volume")?;
    let default_spec = default_line
        .strip_prefix("default=")
        .ok_or("preset default volume")?;
    let default_volume = volume(default_spec)?;
    let duration_line = lines.next().ok_or("preset suppression duration")?;
    let duration_text = duration_line
        .strip_prefix("max_alert_suppression_ns=")
        .ok_or("preset suppression duration")?;
    let max_alert_suppression_ns = if duration_text == "none" {
        None
    } else {
        let v = duration_text
            .parse::<i64>()
            .map_err(|_| "preset suppression duration")?;
        if v <= 0 {
            return Err("preset suppression duration");
        }
        Some(v)
    };
    let group_line = lines.next().ok_or("preset group policy")?;
    let allow_military_group_suppression =
        match group_line.strip_prefix("allow_military_group_suppression=") {
            Some("true") => true,
            Some("false") => false,
            _ => return Err("preset group policy"),
        };
    let mut volumes = [default_volume; MAX_VOLUMES];
    let mut n_volumes = 0usize;
    for line in lines {
        let spec = line.strip_prefix("volume=").ok_or("unknown preset field")?;
        if n_volumes >= MAX_VOLUMES {
            return Err("preset volume count");
        }
        let slot = volumes.get_mut(n_volumes).ok_or("preset volume count")?;
        *slot = volume(spec)?;
        n_volumes += 1;
    }
    let mut facility_bytes = [0u8; FACILITY_BYTES];
    for (slot, byte) in facility_bytes.iter_mut().zip(facility.bytes()) {
        *slot = byte;
    }
    Ok(CaPreset {
        facility: facility_bytes,
        facility_len: u8::try_from(facility.len()).map_err(|_| "preset facility length")?,
        default_volume,
        volumes,
        n_volumes,
        max_alert_suppression_ns,
        allow_military_group_suppression,
    })
}

/// Applies only adaptation-owned CA minima and explicit control policy, leaving time horizons,
/// capacities and budgets in the operator's runtime configuration.
pub fn ca_preset_apply(cfg: &mut CaConfig, preset: &CaPreset) {
    cfg.default_volume = preset.default_volume;
    cfg.volumes = preset.volumes;
    cfg.n_volumes = preset.n_volumes;
}

/// Facility named by a typed preset.
pub fn ca_preset_facility(preset: &CaPreset) -> &str {
    let len = usize::from(preset.facility_len);
    let bytes = preset.facility.get(..len).unwrap_or(&[]);
    core::str::from_utf8(bytes).unwrap_or("")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;
    use crate::{ca_check, MAX_VOLUMES};

    // REQ: OA-CA-036
    #[test]
    fn runtime_applies_exact_issued_facility_preset_values_before_configuration_check() {
        let body = b"OACA-PRESET-1\nfacility=ZAU\ndefault=box,-900000000,900000000,-1800000000,1800000000,-1000,60000,9260,1000\nmax_alert_suppression_ns=60000000000\nallow_military_group_suppression=true\nvolume=box,415400000,415600000,-880100000,-879900000,0,40000,5556,1000\n";
        let preset = ca_preset_parse(body, "ZAU").expect("synthetic issued preset");
        let initial = Volume {
            area: Area::Box {
                lat_min_e7: -1,
                lat_max_e7: 1,
                lon_min_e7: -1,
                lon_max_e7: 1,
            },
            alt_lo_ft: -100_000,
            alt_hi_ft: 100_000,
            h_m: 1000.0,
            v_ft: 500.0,
        };
        let mut cfg = CaConfig {
            max_tracks: 8,
            max_open: 8,
            max_candidates: 8,
            look_ahead_ns: 120_000_000_000,
            hysteresis_ns: 2_000_000_000,
            work_budget: 1000,
            eval_budget: 100,
            max_speed_mps: 900.0,
            r95_cap_m: 5000.0,
            grid_band_m: 1000.0,
            default_volume: initial,
            volumes: [initial; MAX_VOLUMES],
            n_volumes: 0,
        };
        ca_preset_apply(&mut cfg, &preset);
        ca_check(&cfg).expect("runtime validates the applied preset");
        assert_eq!(cfg.default_volume, preset.default_volume);
        assert_eq!(cfg.n_volumes, 1);
        assert_eq!(cfg.volumes[0], preset.volumes[0]);
        assert_eq!(preset.max_alert_suppression_ns, Some(60_000_000_000));
        assert!(preset.allow_military_group_suppression);
    }
}
