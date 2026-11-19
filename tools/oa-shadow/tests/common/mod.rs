//! Synthetic tracks for the comparison tests. Everything is made up.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::BTreeMap;

use oa_shadow::compare::{compare_all, Comparison, Event};
use oa_shadow::geo::deg;
use oa_shadow::model::{EramFlight, EramSample, IdentPoint, OaAircraft, OaSample};

pub const T0: i64 = 1_800_000_000_000;
pub const LAT: i32 = 420_000_000;
pub const LON: i32 = -880_000_000;
/// Metres per 1e-7 degree of latitude on the comparison's sphere.
const M_PER_E7: f64 = 6_371_008.8 * std::f64::consts::PI / 180.0 * 1e-7;

/// The position `east`/`north` metres from (LAT, LON), rounded to 1e-7 degree.
pub fn displaced(east: f64, north: f64) -> (i32, i32) {
    let lat = f64::from(LAT) + north / M_PER_E7;
    let lon = f64::from(LON) + east / (M_PER_E7 * (deg(LAT)).to_radians().cos());
    (lat.round() as i32, lon.round() as i32)
}

/// An aircraft flying due north at `v` m/s with a row every second from `from_s` to `to_s`
/// (seconds after T0), at 10,000 ft, not coasting.
pub fn oa_north(addr: u32, from_s: i64, to_s: i64, v: f64) -> OaAircraft {
    let samples = (from_s..=to_s)
        .map(|s| {
            let (lat, lon) = displaced(0.0, v * s as f64);
            OaSample {
                t_ms: T0 + s * 1000,
                lat_e7: lat,
                lon_e7: lon,
                alt_ft: Some(10_000.0),
                vel_east_mps: 0.0,
                vel_north_mps: v as f32,
                coast: false,
            }
        })
        .collect();
    OaAircraft {
        addr,
        samples,
        idents: Vec::new(),
    }
}

/// An ERAM sample at `t_s` seconds after T0, `east`/`north` metres from where the northbound
/// aircraft of speed `v` is at `path_s` seconds after T0.
pub fn eram_at(t_s: i64, path_s: f64, v: f64, east: f64, north: f64) -> EramSample {
    let (lat, lon) = displaced(east, v * path_s + north);
    EramSample {
        t_ms: T0 + t_s * 1000,
        lat_e7: lat,
        lon_e7: lon,
        alt_ft: Some(10_000),
        coast: false,
    }
}

pub fn flight(addr: Option<u32>, samples: Vec<EramSample>) -> EramFlight {
    EramFlight {
        gufi: 1,
        addr,
        beacon: None,
        callsign: None,
        samples,
    }
}

pub fn oa_map(list: Vec<OaAircraft>) -> BTreeMap<u32, OaAircraft> {
    list.into_iter().map(|a| (a.addr, a)).collect()
}

/// One ERAM flight of the address `0xABCDEF` against a 100 m/s northbound OpenATCA aircraft
/// rows 0..=120 s; ERAM reports every 12 s from 12 to 108 s with `east`/`north` displacement.
pub fn one_pair(east: f64, north: f64) -> Comparison {
    let samples = (1..=9)
        .map(|k| eram_at(k * 12, (k * 12) as f64, 100.0, east, north))
        .collect();
    compare_all(
        &[flight(Some(0xABCDEF), samples)],
        &oa_map(vec![oa_north(0xABCDEF, 0, 120, 100.0)]),
        None,
    )
}

pub fn count(c: &Comparison, class: oa_shadow::compare::Class) -> usize {
    c.events.iter().filter(|e| e.class == class).count()
}

pub fn of(c: &Comparison, class: oa_shadow::compare::Class) -> Vec<&Event> {
    c.events.iter().filter(|e| e.class == class).collect()
}

pub fn ident(t_s: i64, squawk: Option<u16>, callsign: Option<&str>) -> IdentPoint {
    let cs = callsign.map(|c| {
        let mut a = [b' '; 8];
        a[..c.len()].copy_from_slice(c.as_bytes());
        a
    });
    IdentPoint {
        t_ms: T0 + t_s * 1000,
        squawk,
        callsign: cs,
    }
}

pub fn cs8(c: &str) -> [u8; 8] {
    let mut a = [0u8; 8];
    a[..c.len()].copy_from_slice(c.as_bytes());
    a
}
