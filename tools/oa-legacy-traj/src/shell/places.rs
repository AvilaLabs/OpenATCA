//! Where the named points and the airports are: NASR (FIX_BASE, NAV_BASE, APT_BASE) and an
//! optional places file that overrides it. Nothing is derived from the recordings.

use std::collections::BTreeMap;
use std::path::Path;

use crate::core::cross::flat_dist_m;
use crate::core::types::Place;

/// Metres within which two places of one name are the same place.
const SAME_PLACE_M: f64 = 1852.0;

/// The places of every name: points (fixes, waypoints, navaids) and airports.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, Default)]
pub struct PlaceTable {
    /// Distinct places by upper case identifier, fixes and navaids together.
    pub points: BTreeMap<String, Vec<Place>>,
    /// Distinct places by airport key (the FAA identifier and the ICAO identifier with the K of a
    /// contiguous US code dropped).
    pub airports: BTreeMap<String, Vec<Place>>,
    /// Rows NASR parsing rejected, by file.
    pub rejected: u64,
    /// The elevation of an airport, feet, by airport key (NASR `APT_BASE`).
    pub elev_ft: BTreeMap<String, f64>,
    /// Airport keys that two distinct airports (site numbers) share; none in a sound NASR file.
    pub collisions: Vec<String>,
}

fn add(m: &mut BTreeMap<String, Vec<Place>>, name: &str, here: Place) {
    if name.is_empty() {
        return;
    }
    let list = m.entry(name.to_string()).or_default();
    if !list
        .iter()
        .any(|q| flat_dist_m(*q, here.lat_deg, here.lon_deg) <= SAME_PLACE_M)
    {
        list.push(here);
    }
}

/// The airport key of a code: upper case, the K of a four-letter contiguous US code dropped, so
/// `KORD` and `ORD` are one airport.
pub fn airport_key(code: &str) -> String {
    let s = code.trim().to_ascii_uppercase();
    match s.strip_prefix('K') {
        Some(rest) if s.len() == 4 => rest.to_string(),
        _ => s,
    }
}

/// Builds the table from the three NASR CSV files (`FIX_BASE`, `NAV_BASE`, `APT_BASE`).
pub fn table_from_csv(fix: &[u8], nav: &[u8], apt: &[u8]) -> Result<PlaceTable, String> {
    let mut t = PlaceTable::default();
    let fixes =
        oa_nasr::fix::parse_fixes("FIX_BASE.csv", fix).map_err(|e| format!("FIX_BASE.csv: {e}"))?;
    t.rejected += fixes.rejected.len() as u64 + fixes.rejected_dropped;
    for f in &fixes.records {
        let here = Place {
            lat_deg: f.position.lat,
            lon_deg: f.position.lon,
        };
        add(&mut t.points, &f.fix_id.to_ascii_uppercase(), here);
    }
    let navs = oa_nasr::nav::parse_navaids("NAV_BASE.csv", nav)
        .map_err(|e| format!("NAV_BASE.csv: {e}"))?;
    t.rejected += navs.rejected.len() as u64 + navs.rejected_dropped;
    for n in &navs.records {
        let here = Place {
            lat_deg: n.position.lat,
            lon_deg: n.position.lon,
        };
        add(&mut t.points, &n.nav_id.to_ascii_uppercase(), here);
    }
    let apts = oa_nasr::apt::parse_airports("APT_BASE.csv", apt)
        .map_err(|e| format!("APT_BASE.csv: {e}"))?;
    t.rejected += apts.rejected.len() as u64 + apts.rejected_dropped;
    let mut sites: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for a in apts.records.iter().filter(|a| a.site_type_code == "A") {
        let here = Place {
            lat_deg: a.position.lat,
            lon_deg: a.position.lon,
        };
        let mut keys = vec![airport_key(&a.arpt_id)];
        if let Some(icao) = &a.icao_id {
            keys.push(airport_key(icao));
        }
        for k in keys {
            add(&mut t.airports, &k, here);
            t.elev_ft.entry(k.clone()).or_insert(a.elev);
            let v = sites.entry(k).or_default();
            if !v.contains(&a.site_no) {
                v.push(a.site_no.clone());
            }
        }
    }
    t.collisions = sites
        .into_iter()
        .filter(|(_, v)| v.len() > 1)
        .map(|(k, _)| k)
        .collect();
    Ok(t)
}

/// Reads the table from an NASR CSV archive, through the seal guard; also the archive's SHA-256.
pub fn load_nasr(path: &Path) -> Result<(PlaceTable, String), String> {
    let bytes = oa_zip::read_archive_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let zip = oa_zip::Archive::parse(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    let get = |n: &str| {
        zip.extract(n)
            .map_err(|e| format!("{}: {n}: {e}", path.display()))
    };
    let t = table_from_csv(
        &get("FIX_BASE.csv")?,
        &get("NAV_BASE.csv")?,
        &get("APT_BASE.csv")?,
    )?;
    Ok((t, oa_sha256::to_hex(&oa_sha256::sha256(&bytes))))
}

/// Applies a places file: a name it gives has only the places the file gives.
pub fn apply_places(t: &mut PlaceTable, f: &PlacesFile) {
    let mut seen: BTreeMap<String, bool> = BTreeMap::new();
    for (name, here) in &f.fixes {
        let n = oa_bench::legacy::name(name).to_string();
        if seen.insert(n.clone(), true).is_none() {
            t.points.remove(&n);
        }
        add(&mut t.points, &n, *here);
    }
    for (apt, here) in &f.airports {
        t.elev_ft.remove(apt);
        t.airports.insert(apt.clone(), vec![*here]);
    }
}

/// The places of a point name (a fix or navaid; failing that, an airport code, since routes
/// name airports too).
pub fn point_places<'a>(t: &'a PlaceTable, name: &str) -> &'a [Place] {
    let n = name.trim().to_ascii_uppercase();
    match t.points.get(&n) {
        Some(v) if !v.is_empty() => v,
        _ => t
            .airports
            .get(&airport_key(&n))
            .map_or(&[][..], Vec::as_slice),
    }
}

/// The places of an airport code.
pub fn airport_places<'a>(t: &'a PlaceTable, code: &str) -> &'a [Place] {
    t.airports
        .get(&airport_key(code))
        .map_or(&[][..], Vec::as_slice)
}

/// The place of a coordinate pair in 1e-7 degree.
pub fn place_of_e7(lat_e7: i32, lon_e7: i32) -> Place {
    Place {
        lat_deg: f64::from(lat_e7) / 1e7,
        lon_deg: f64::from(lon_e7) / 1e7,
    }
}

/// The named points and airport reference points a caller supplies, which replace NASR's for
/// the names they give.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlacesFile {
    /// Named points, a name as many times as it has places.
    pub fixes: Vec<([u8; 8], Place)>,
    /// Airport reference points by [`crate::shell::run::airport_key`].
    pub airports: BTreeMap<String, Place>,
}

/// Reads a places file: lines `fix,NAME,lat,lon` and `airport,CODE,lat,lon` (degrees), blank
/// lines and lines starting with `#` ignored. A line that is not one of these, a name too long
/// for a name or a coordinate out of range is an error naming the line.
pub fn parse_places(text: &str) -> Result<PlacesFile, String> {
    let mut out = PlacesFile::default();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let bad = |why: &str| format!("places line {}: {why}", n + 1);
        let f: Vec<&str> = line.split(',').map(str::trim).collect();
        let [kind, name, lat, lon] = f.as_slice() else {
            return Err(bad("expected kind,name,lat,lon"));
        };
        let lat: f64 = lat.parse().map_err(|_| bad("latitude is not a number"))?;
        let lon: f64 = lon.parse().map_err(|_| bad("longitude is not a number"))?;
        if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
            return Err(bad("coordinate out of range"));
        }
        let here = Place {
            lat_deg: lat,
            lon_deg: lon,
        };
        let upper = name.to_ascii_uppercase();
        match *kind {
            "fix" if !upper.is_empty() && upper.len() <= 8 => {
                out.fixes
                    .push((oa_bench::legacy::pad::<8>(upper.as_bytes()), here));
            }
            "fix" => return Err(bad("a fix name is 1 to 8 characters")),
            "airport" if !upper.is_empty() => {
                let key = airport_key(&upper);
                if out.airports.insert(key, here).is_some() {
                    return Err(bad("airport given twice"));
                }
            }
            _ => return Err(bad("kind is fix or airport, with a name")),
        }
    }
    Ok(out)
}

/// The elevation of an airport, feet, when the table has one.
pub fn airport_elev_ft(t: &PlaceTable, code: &str) -> Option<f64> {
    t.elev_ft.get(&airport_key(code)).copied()
}
