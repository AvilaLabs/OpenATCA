//! Which volume's minima apply to a pair. Core (pure).

use oa_safety::geom::polygon_contains;

use crate::types::{Area, CaConfig, Volume};

fn in_area(a: &Area, lat_e7: i32, lon_e7: i32) -> bool {
    match a {
        Area::Box {
            lat_min_e7,
            lat_max_e7,
            lon_min_e7,
            lon_max_e7,
        } => {
            lat_e7 >= *lat_min_e7
                && lat_e7 <= *lat_max_e7
                && lon_e7 >= *lon_min_e7
                && lon_e7 <= *lon_max_e7
        }
        Area::Polygon(p) => polygon_contains(p, lat_e7, lon_e7),
    }
}

fn in_band(v: &Volume, alt_ft: f64) -> bool {
    alt_ft >= f64::from(v.alt_lo_ft) && alt_ft <= f64::from(v.alt_hi_ft)
}

/// The minima `(horizontal metres, vertical feet)` that apply to a pair whose midpoint is at
/// `(lat_e7, lon_e7)` and mean altitude `alt_ft` (`None` if either altitude is unknown).
///
/// With a known altitude, the first volume of the list whose area and altitude band hold the
/// midpoint; none: the default volume. With an unknown altitude the band cannot be tested, so the
/// answer is the larger horizontal and the larger vertical minimum among the volumes whose area
/// holds the midpoint (whatever their band) AND the default volume, which a known altitude
/// outside every band would get: a missing altitude never picks a smaller minimum than any
/// altitude would (M5 rule 4). Work: at most [`crate::MAX_VOLUMES`] volumes.
pub fn volume_minima(cfg: &CaConfig, lat_e7: i32, lon_e7: i32, alt_ft: Option<f64>) -> (f64, f64) {
    let n = cfg.n_volumes.min(cfg.volumes.len());
    let mut best = (cfg.default_volume.h_m, cfg.default_volume.v_ft);
    for v in cfg.volumes.iter().take(n) {
        if !in_area(&v.area, lat_e7, lon_e7) {
            continue;
        }
        match alt_ft {
            Some(a) => {
                if in_band(v, a) {
                    return (v.h_m, v.v_ft);
                }
            }
            None => {
                if v.h_m > best.0 {
                    best.0 = v.h_m;
                }
                if v.v_ft > best.1 {
                    best.1 = v.v_ft;
                }
            }
        }
    }
    match alt_ft {
        Some(_) => (cfg.default_volume.h_m, cfg.default_volume.v_ft),
        None => best,
    }
}

/// The largest horizontal and vertical minima of the configuration, default volume included
/// (the radius of the candidate scan and the vertical skip use them: they can only widen).
pub fn volume_maxima(cfg: &CaConfig) -> (f64, f64) {
    let n = cfg.n_volumes.min(cfg.volumes.len());
    let mut h = cfg.default_volume.h_m;
    let mut z = cfg.default_volume.v_ft;
    for v in cfg.volumes.iter().take(n) {
        if v.h_m > h {
            h = v.h_m;
        }
        if v.v_ft > z {
            z = v.v_ft;
        }
    }
    (h, z)
}
