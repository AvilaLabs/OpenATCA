//! The altitude datum. Shell (B3): it reads METAR files.
//!
//! An ADS-B barometric altitude is a pressure altitude (standard setting 29.92 inHg); the floors
//! are heights above mean sea level. From FL180 up the standard setting is what is flown, and the
//! pressure altitude is used as it is. Below, it is corrected with the altimeter setting of the
//! nearest METAR station within the configured radius that has an observation at most the
//! configured age old; the station is the nearest one that has such an observation. With none the
//! pressure altitude is lowered by the allowance (the conservative direction: a lower altitude can
//! only raise an alert) and the track is counted as having no setting (`QnhMissing`).
//!
//! A lookup is made for the 0.1 degree cell of the track at the 5 minute bucket of the time: at
//! the cell's centre, with observations at or before the start of the bucket and at most the age
//! old at its end, so that the age of what a track uses is never more than the limit.

use std::collections::HashMap;
use std::path::Path;

use oa_awc::{
    finalize_metar, indicated_altitude_ft, metar_altim_hpa, metar_len, new_metar_table,
    parse_metar, MetarStats,
};
use oa_sealed::Sealed;

use crate::guard::{open_guarded, OpenFail};

const NM_M: f64 = 1852.0;
/// Metres per degree of latitude (the core's scale).
const M_PER_DEG: f64 = 111_194.926_644_558_74;
/// The cell and time bucket of a lookup.
const CELL_DEG: f64 = 0.1;
const BUCKET_S: i64 = 300;
/// The altitude from which the standard setting is flown, feet.
pub const FL180_FT: f64 = 18_000.0;

/// A station given directly: latitude, longitude and observations (Unix seconds, hPa).
pub type StationSpec = (f64, f64, Vec<(i64, f64)>);

/// One station: its position and its observations with an altimeter setting, by time.
struct Station {
    lat: f64,
    lon: f64,
    obs: Vec<(i64, f64)>,
}

/// What the lookups did. A7.2: cold struct, size guard waived.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QnhStats {
    pub files: u64,
    pub rows: u64,
    pub stations: u64,
    /// Altitudes corrected with a setting, left as they were (FL180 and above), lowered by the
    /// allowance for want of a setting.
    pub corrected: u64,
    pub standard: u64,
    pub missing: u64,
    pub lookups: u64,
    pub cache_hits: u64,
}

/// The METAR stations of the run with a cache of the lookups. A7.2: cold struct.
pub struct Qnh {
    stations: Vec<Station>,
    radius_m: f64,
    max_age_s: i64,
    cache: HashMap<(i32, i32, i64), Option<f64>>,
    pub stats: QnhStats,
}

fn cell_of(deg: f64) -> i32 {
    (deg / CELL_DEG).floor() as i32
}

impl Qnh {
    /// A set with no station: every altitude below FL180 reads as having no setting.
    pub fn empty(radius_nm: f64, max_age_s: i64) -> Qnh {
        Qnh {
            stations: Vec::new(),
            radius_m: radius_nm * NM_M,
            max_age_s,
            cache: HashMap::new(),
            stats: QnhStats::default(),
        }
    }

    /// A set from stations given directly: (latitude, longitude, observations as Unix seconds and
    /// hPa, in time order). For a test.
    pub fn from_stations(stations: Vec<StationSpec>, radius_nm: f64, max_age_s: i64) -> Qnh {
        let mut q = Qnh::empty(radius_nm, max_age_s);
        q.stations = stations
            .into_iter()
            .map(|(lat, lon, obs)| Station { lat, lon, obs })
            .collect();
        q.stats.stations = q.stations.len() as u64;
        q
    }

    /// Reads the METAR files `metar_00z_hours1.json` to `metar_23z_hours1.json` of `dir` that are
    /// there (each name is opened by name, through the guard), de-duplicates them and builds the
    /// set. A file that is there and cannot be read or parsed is an error.
    pub fn from_dir(dir: &Path, radius_nm: f64, max_age_s: i64) -> Result<Qnh, String> {
        let g = Sealed::production();
        let mut table = new_metar_table();
        let mut ms = MetarStats::default();
        let mut files = 0u64;
        for hour in 0..24 {
            let p = dir.join(format!("metar_{hour:02}z_hours1.json"));
            if !p.is_file() {
                continue;
            }
            let mut f = open_guarded(g, &p).map_err(|e| match e {
                OpenFail::Refused(m) | OpenFail::Io(m) => m,
            })?;
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut f, &mut bytes)
                .map_err(|e| format!("{}: {e}", p.display()))?;
            parse_metar(&bytes, &mut table, &mut ms)
                .map_err(|e| format!("{}: {e}", p.display()))?;
            files += 1;
        }
        finalize_metar(&mut table);
        let mut stations: Vec<Station> = Vec::new();
        let mut prev = None;
        for i in 0..metar_len(&table) {
            let (Some(&id), Some(&la), Some(&lo), Some(&obs)) = (
                table.station.get(i),
                table.lat_e5.get(i),
                table.lon_e5.get(i),
                table.obs_s.get(i),
            ) else {
                continue;
            };
            if prev != Some(id) {
                prev = Some(id);
                stations.push(Station {
                    lat: f64::from(la) / 1e5,
                    lon: f64::from(lo) / 1e5,
                    obs: Vec::new(),
                });
            }
            if let (Some(hpa), Some(s)) = (metar_altim_hpa(&table, i), stations.last_mut()) {
                s.obs.push((obs, hpa));
            }
        }
        let mut q = Qnh::empty(radius_nm, max_age_s);
        q.stats.files = files;
        q.stats.rows = metar_len(&table) as u64;
        q.stats.stations = stations.len() as u64;
        q.stations = stations;
        Ok(q)
    }

    /// The altimeter setting for a track at (`lat`, `lon`) at Unix second `t_s`, hPa; `None` if no
    /// station within the radius has an observation within the age.
    pub fn setting_hpa(&mut self, lat: f64, lon: f64, t_s: i64) -> Option<f64> {
        self.stats.lookups += 1;
        let bucket = t_s.div_euclid(BUCKET_S);
        let key = (cell_of(lat), cell_of(lon), bucket);
        if let Some(&v) = self.cache.get(&key) {
            self.stats.cache_hits += 1;
            return v;
        }
        let (clat, clon) = (
            (f64::from(key.0) + 0.5) * CELL_DEG,
            (f64::from(key.1) + 0.5) * CELL_DEG,
        );
        let (start, end) = (bucket * BUCKET_S, (bucket + 1) * BUCKET_S);
        let cos = clat.to_radians().cos().max(0.01);
        let mut near: Vec<(f64, usize)> = Vec::new();
        for (i, s) in self.stations.iter().enumerate() {
            let (dy, dx) = ((s.lat - clat) * M_PER_DEG, (s.lon - clon) * M_PER_DEG * cos);
            // Taken at the cell's centre, like the answer, which every track in the cell gets.
            let d = (dx * dx + dy * dy).sqrt();
            if d <= self.radius_m {
                near.push((d, i));
            }
        }
        near.sort_by(|a, b| oa_math::cmp_canon_f64(a.0, b.0).then(a.1.cmp(&b.1)));
        let mut found = None;
        for (_, i) in near {
            let Some(s) = self.stations.get(i) else {
                continue;
            };
            let upto = s.obs.partition_point(|&(o, _)| o <= start);
            if let Some(&(o, hpa)) = upto.checked_sub(1).and_then(|k| s.obs.get(k)) {
                if end - o <= self.max_age_s {
                    found = Some(hpa);
                    break;
                }
            }
        }
        self.cache.insert(key, found);
        found
    }

    /// An altitude on the datum of the floors. `pressure_alt_ft` is the pressure altitude of the
    /// report; the answer is the altitude to hand to the net and whether the setting was missing.
    pub fn datum_ft(
        &mut self,
        lat: f64,
        lon: f64,
        t_s: i64,
        pressure_alt_ft: f64,
        no_qnh_allowance_ft: i32,
    ) -> (f64, bool) {
        if pressure_alt_ft >= FL180_FT {
            self.stats.standard += 1;
            return (pressure_alt_ft, false);
        }
        if let Some(v) = self
            .setting_hpa(lat, lon, t_s)
            .and_then(|hpa| indicated_altitude_ft(pressure_alt_ft, hpa))
        {
            self.stats.corrected += 1;
            return (v, false);
        }
        self.stats.missing += 1;
        (pressure_alt_ft - f64::from(no_qnh_allowance_ft), true)
    }
}
