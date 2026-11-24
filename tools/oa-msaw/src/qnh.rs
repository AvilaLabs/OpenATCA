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

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use oa_awc::{
    finalize_metar, indicated_altitude_ft, metar_altim_hpa, metar_len, new_metar_table,
    parse_metar, MetarStats,
};
use oa_sealed::Sealed;

use crate::guard::{open_guarded, OpenFail};

const NM_M: f64 = 1852.0;
/// Metres in a foot.
const FT_M: f64 = 0.3048;
/// The METAR table's value for a field that is absent.
const NONE_I16: i16 = i16::MIN;
/// Metres per degree of latitude (the core's scale).
const M_PER_DEG: f64 = 111_194.926_644_558_74;
/// The cell and time bucket of a lookup.
const CELL_DEG: f64 = 0.1;
const BUCKET_S: i64 = 300;
/// With no setting at the usual radius and age, the lowest setting of any station within this
/// distance in the last `FALLBACK_AGE_S` is used: a low setting lowers the indicated altitude, the
/// conservative direction.
const FALLBACK_RADIUS_NM: f64 = 200.0;
const FALLBACK_AGE_S: i64 = 12 * 3600;
/// The largest height above the altimeter station the cold-temperature correction is made up to, feet.
pub const COLD_MAX_HEIGHT_FT: f64 = 5_000.0;
/// The altitude from which the standard setting is flown, feet.
pub const FL180_FT: f64 = 18_000.0;

/// The cold-temperature correction of ICAO Doc 8168 (Vol. I, Part III), feet: for the height `h_ft`
/// above the altimeter source (elevation `elev_ft`) and the temperature `t_c` at the source below
/// 15 degrees, `h (15 - t) / (273 + t - 0.5 L (h + elev))` with the lapse rate L of 0.00198 degrees
/// per foot. Zero at or above 15 degrees.
pub fn cold_correction_ft(h_ft: f64, elev_ft: f64, t_c: f64) -> f64 {
    if t_c >= 15.0 || h_ft <= 0.0 {
        return 0.0;
    }
    let den = 273.0 + t_c - 0.5 * 0.00198 * (h_ft + elev_ft);
    if den <= 100.0 {
        // Beyond any air the formula describes: not a number to trust, left as no correction.
        return 0.0;
    }
    h_ft * (15.0 - t_c) / den
}

/// A station given directly: latitude, longitude and observations (Unix seconds, hPa).
pub type StationSpec = (f64, f64, Vec<(i64, f64)>);

/// One station: its position and its observations with an altimeter setting, by time.
struct Station {
    lat: f64,
    lon: f64,
    obs: Vec<(i64, f64)>,
    /// The station's elevation, feet (NaN if unknown), and the temperature of each observation,
    /// degrees C (NaN where absent), parallel to `obs`.
    elev_ft: f64,
    temp_c: Vec<f64>,
}

/// What a lookup finds: the altimeter setting (hPa), the temperature at the station (degrees C, NaN
/// if not reported) and the station's elevation (feet, NaN if unknown).
type Found = (f64, f64, f64);

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
    /// Of the altitudes without a setting at the usual radius and age, those corrected with the
    /// lowest setting of the last 12 h within 200 NM (the rest take the allowance).
    pub fallback: u64,
    /// Altitudes lowered by the cold-temperature correction.
    pub cold: u64,
    /// Altitudes over [`COLD_MAX_HEIGHT_FT`] above the station in cold air: not corrected.
    pub cold_high: u64,
    pub lookups: u64,
    pub cache_hits: u64,
}

/// The METAR stations of the run with a cache of the lookups. A7.2: cold struct.
pub struct Qnh {
    stations: Vec<Station>,
    radius_m: f64,
    max_age_s: i64,
    cache: HashMap<(i32, i32, i64), Option<Found>>,
    low_cache: HashMap<(i32, i32, i64), Option<f64>>,
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
            low_cache: HashMap::new(),
            stats: QnhStats::default(),
        }
    }

    /// A set from stations given directly: (latitude, longitude, observations as Unix seconds and
    /// hPa, in time order). For a test.
    pub fn from_stations(stations: Vec<StationSpec>, radius_nm: f64, max_age_s: i64) -> Qnh {
        let mut q = Qnh::empty(radius_nm, max_age_s);
        q.stations = stations
            .into_iter()
            .map(|(lat, lon, obs)| Station {
                lat,
                lon,
                temp_c: vec![f64::NAN; obs.len()],
                obs,
                elev_ft: f64::NAN,
            })
            .collect();
        q.stats.stations = q.stations.len() as u64;
        q
    }

    /// Reads the METAR files `metar_00z_hours1.json` to `metar_23z_hours1.json` of `dir` that are
    /// there (each name is opened by name, through the guard), de-duplicates them and builds the
    /// set. A file that is there and cannot be read or parsed is an error.
    /// `dir` is a directory of `metar_NNz_hours1.json`, a METAR file, or several of either
    /// separated by commas; a named file is read whatever its name.
    pub fn from_dir(dir: &Path, radius_nm: f64, max_age_s: i64) -> Result<Qnh, String> {
        let g = Sealed::production();
        let mut table = new_metar_table();
        let mut ms = MetarStats::default();
        let mut files = 0u64;
        let mut paths: Vec<PathBuf> = Vec::new();
        for part in path_parts(dir) {
            if part.is_dir() {
                for hour in 0..24 {
                    let p = part.join(format!("metar_{hour:02}z_hours1.json"));
                    if p.is_file() {
                        paths.push(p);
                    }
                }
            } else {
                paths.push(part);
            }
        }
        for p in paths {
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
                let elev = table.elev_m.get(i).copied().unwrap_or(NONE_I16);
                stations.push(Station {
                    lat: f64::from(la) / 1e5,
                    lon: f64::from(lo) / 1e5,
                    obs: Vec::new(),
                    elev_ft: if elev == NONE_I16 {
                        f64::NAN
                    } else {
                        f64::from(elev) / FT_M
                    },
                    temp_c: Vec::new(),
                });
            }
            if let (Some(hpa), Some(s)) = (metar_altim_hpa(&table, i), stations.last_mut()) {
                let t = table.temp_dc.get(i).copied().unwrap_or(NONE_I16);
                s.obs.push((obs, hpa));
                s.temp_c.push(if t == NONE_I16 {
                    f64::NAN
                } else {
                    f64::from(t) / 10.0
                });
            }
        }
        let mut q = Qnh::empty(radius_nm, max_age_s);
        q.stats.files = files;
        q.stats.rows = metar_len(&table) as u64;
        q.stats.stations = stations.len() as u64;
        q.stations = stations;
        Ok(q)
    }

    /// Builds from an explicit set of caller-admitted METAR bytes. `received_at_ms` is the
    /// caller's evidence that the whole named source was available by cutoff; observations whose
    /// report or METAR receipt time is later than its file receipt or `cutoff_ms` is excluded. The regional adapter
    /// performs byte and file-count checks before calling this decoder.
    pub(crate) fn from_admitted_bytes(
        files: &[(&str, &[u8], i64)],
        cutoff_ms: i64,
        radius_nm: f64,
        max_age_s: i64,
    ) -> Result<Qnh, String> {
        let cutoff_s = cutoff_ms.div_euclid(1000);
        let mut grouped: BTreeMap<u32, (f64, f64, f64, BTreeMap<i64, (f64, f64)>)> =
            BTreeMap::new();
        let mut retained_rows = 0u64;
        for (name, bytes, received_at_ms) in files {
            if *received_at_ms > cutoff_ms {
                return Err(format!("{name}: METAR source was received after cutoff"));
            }
            let mut table = new_metar_table();
            let mut stats = MetarStats::default();
            parse_metar(bytes, &mut table, &mut stats).map_err(|e| format!("{name}: {e}"))?;
            finalize_metar(&mut table);
            for i in 0..metar_len(&table) {
                let (Some(&id), Some(&la), Some(&lo), Some(&obs), Some(&receipt)) = (
                    table.station.get(i),
                    table.lat_e5.get(i),
                    table.lon_e5.get(i),
                    table.obs_s.get(i),
                    table.receipt_ms.get(i),
                ) else {
                    continue;
                };
                if obs > cutoff_s || receipt > cutoff_ms || receipt > *received_at_ms {
                    continue;
                }
                let Some(hpa) = metar_altim_hpa(&table, i) else {
                    continue;
                };
                let temp = table.temp_dc.get(i).copied().unwrap_or(NONE_I16);
                let elev = table.elev_m.get(i).copied().unwrap_or(NONE_I16);
                let entry = grouped.entry(id).or_insert_with(|| {
                    (
                        f64::from(la) / 1e5,
                        f64::from(lo) / 1e5,
                        if elev == NONE_I16 {
                            f64::NAN
                        } else {
                            f64::from(elev) / FT_M
                        },
                        BTreeMap::new(),
                    )
                });
                if entry.0 != f64::from(la) / 1e5 || entry.1 != f64::from(lo) / 1e5 {
                    return Err(format!(
                        "{name}: METAR station identity has conflicting coordinates"
                    ));
                }
                if entry.2.is_nan() && elev != NONE_I16 {
                    entry.2 = f64::from(elev) / FT_M;
                }
                let temp_c = if temp == NONE_I16 {
                    f64::NAN
                } else {
                    f64::from(temp) / 10.0
                };
                if let Some(existing) = entry.3.get(&obs) {
                    let same_temp =
                        existing.1 == temp_c || (existing.1.is_nan() && temp_c.is_nan());
                    if existing.0 != hpa || !same_temp {
                        return Err(format!(
                            "{name}: conflicting METAR values at the same station time"
                        ));
                    }
                } else {
                    entry.3.insert(obs, (hpa, temp_c));
                    retained_rows += 1;
                }
            }
        }
        let mut stations = Vec::with_capacity(grouped.len());
        for (_, (lat, lon, elev_ft, rows)) in grouped {
            let (obs, temp_c): (Vec<(i64, f64)>, Vec<f64>) = rows
                .into_iter()
                .map(|(time, (hpa, temp))| ((time, hpa), temp))
                .unzip();
            stations.push(Station {
                lat,
                lon,
                obs,
                elev_ft,
                temp_c,
            });
        }
        let mut q = Qnh::empty(radius_nm, max_age_s);
        q.stats.files = files.len() as u64;
        q.stats.rows = retained_rows;
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
            return v.map(|f| f.0);
        }
        let found = self.nearest_observation(key).map(|(f, _)| f);
        self.cache.insert(key, found);
        found.map(|f| f.0)
    }

    /// How old, seconds at the end of the time's bucket, the observation behind
    /// [`Qnh::setting_hpa`] is; `None` where that has no setting. For the diagnosis.
    pub fn setting_age_s(&self, lat: f64, lon: f64, t_s: i64) -> Option<i64> {
        let key = (cell_of(lat), cell_of(lon), t_s.div_euclid(BUCKET_S));
        self.nearest_observation(key).map(|(_, age)| age)
    }

    /// The setting of the nearest station within the radius with an observation within the age,
    /// and that observation's age at the end of the bucket.
    fn nearest_observation(&self, key: (i32, i32, i64)) -> Option<(Found, i64)> {
        let (clat, clon) = (
            (f64::from(key.0) + 0.5) * CELL_DEG,
            (f64::from(key.1) + 0.5) * CELL_DEG,
        );
        let (start, end) = (key.2 * BUCKET_S, (key.2 + 1) * BUCKET_S);
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
        for (_, i) in near {
            let Some(s) = self.stations.get(i) else {
                continue;
            };
            let upto = s.obs.partition_point(|&(o, _)| o <= start);
            if let Some((k, &(o, hpa))) = upto
                .checked_sub(1)
                .and_then(|k| s.obs.get(k).map(|x| (k, x)))
            {
                if end - o <= self.max_age_s {
                    let t = s.temp_c.get(k).copied().unwrap_or(f64::NAN);
                    return Some(((hpa, t, s.elev_ft), end - o));
                }
            }
        }
        None
    }

    /// The lowest altimeter setting reported within 200 NM in the 12 h before the time's bucket, hPa;
    /// `None` if no station within that distance reported one.
    pub fn lowest_recent_hpa(&mut self, lat: f64, lon: f64, t_s: i64) -> Option<f64> {
        let bucket = t_s.div_euclid(BUCKET_S);
        let key = (cell_of(lat), cell_of(lon), bucket);
        if let Some(&v) = self.low_cache.get(&key) {
            return v;
        }
        let (clat, clon) = (
            (f64::from(key.0) + 0.5) * CELL_DEG,
            (f64::from(key.1) + 0.5) * CELL_DEG,
        );
        let start = bucket * BUCKET_S;
        let cos = clat.to_radians().cos().max(0.01);
        let limit = FALLBACK_RADIUS_NM * NM_M;
        let mut low: Option<f64> = None;
        for s in &self.stations {
            let (dy, dx) = ((s.lat - clat) * M_PER_DEG, (s.lon - clon) * M_PER_DEG * cos);
            if (dx * dx + dy * dy).sqrt() > limit {
                continue;
            }
            let hi = s.obs.partition_point(|&(o, _)| o <= start);
            let lo = s.obs.partition_point(|&(o, _)| o < start - FALLBACK_AGE_S);
            for &(_, hpa) in s.obs.get(lo..hi).unwrap_or(&[]) {
                if low.is_none_or(|l| hpa < l) {
                    low = Some(hpa);
                }
            }
        }
        self.low_cache.insert(key, low);
        low
    }

    /// Sets the weather of the station `i` given to [`Qnh::from_stations`]: its elevation in feet and
    /// the temperature of each observation in degrees C. For a test.
    pub fn set_station_weather(&mut self, i: usize, elev_ft: f64, temp_c: Vec<f64>) {
        if let Some(s) = self.stations.get_mut(i) {
            s.elev_ft = elev_ft;
            s.temp_c = temp_c;
        }
    }

    /// The cold-temperature correction (ICAO Doc 8168) of an altitude that was corrected with the
    /// setting at (`lat`, `lon`), `t_s`: the altitude lowered by the correction for the temperature
    /// at the station of that setting, and a code (0: not needed, 1: corrected, 2: the temperature
    /// or elevation of the station not known, 3: over 5,000 ft above it: no correction). Never raises an altitude.
    pub fn cold_ft(&mut self, lat: f64, lon: f64, t_s: i64, indicated_ft: f64) -> (f64, u8) {
        self.stats.lookups += 1;
        let key = (cell_of(lat), cell_of(lon), t_s.div_euclid(BUCKET_S));
        let found = match self.cache.get(&key) {
            Some(&v) => v,
            None => {
                let v = self.nearest_observation(key).map(|(f, _)| f);
                self.cache.insert(key, v);
                v
            }
        };
        let Some((_, t, elev)) = found else {
            return (indicated_ft, 2);
        };
        if !t.is_finite() || !elev.is_finite() {
            return (indicated_ft, 2);
        }
        let h = indicated_ft - elev;
        if t >= 15.0 || h <= 0.0 {
            return (indicated_ft, 0);
        }
        // The correction is for the terminal and approach: above this the air column is too tall for
        // one station's temperature to stand for it.
        if h > COLD_MAX_HEIGHT_FT {
            self.stats.cold_high += 1;
            return (indicated_ft, 3);
        }
        let dh = cold_correction_ft(h, elev, t);
        self.stats.cold += 1;
        (indicated_ft - dh, 1)
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
        // No setting in reach: the lowest of the last 12 h within 200 NM, if any; the track stays
        // flagged as having no setting of its own.
        if let Some(v) = self
            .lowest_recent_hpa(lat, lon, t_s)
            .and_then(|hpa| indicated_altitude_ft(pressure_alt_ft, hpa))
        {
            self.stats.fallback += 1;
            return (v, true);
        }
        (pressure_alt_ft - f64::from(no_qnh_allowance_ft), true)
    }
}

/// The parts of a path argument that names several paths separated by commas.
pub fn path_parts(p: &Path) -> Vec<PathBuf> {
    p.to_string_lossy()
        .split(',')
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect()
}
