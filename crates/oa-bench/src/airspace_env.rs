//! The airspace lookups of [`crate::truth::Env`] filled from the M0 model (`oa-airspace`).
//! Core (pure): a function of the model.
//!
//! * Fixes and navaids: points of the fix, VOR, DME, TACAN, NDB and terminal-waypoint kinds by
//!   identifier. An identifier that occurs more than once is kept with every distinct place it
//!   names (rows at one place are one place); [`AirspaceLookup::fix`] answers only when there is
//!   one place, and [`AirspaceLookup::fix_all`] lists them all so a caller can choose with the
//!   context of a route or a flight. Repeated rows are counted in
//!   [`AirspaceLookup::duplicate_fixes`] and identifiers with several places in
//!   [`AirspaceLookup::ambiguous_fixes`]. No place is ever picked by row order.
//! * Airports: the airport points by ICAO identifier and by FAA identifier; field elevation from
//!   the point, thresholds from the runway-threshold points whose scope is the airport and whose
//!   region is the airport's (an airport is its identifier plus its ICAO region). An airport with
//!   no published elevation is not offered (nothing is made up). An identifier that names
//!   airports of more than one region is not answered (the caller gives no region), counted in
//!   [`AirspaceLookup::ambiguous_airports`]; no airport is picked by row order.
//! * Zone: the model's own ARTCC (`Meta::artcc`), its HIGH and LOW boundaries together.
//! * Unit: the ARTCC boundary containing the point (any ARTCC in the model, HIGH or LOW), as the
//!   ARTCC identifier in bytes 0 to 3 with the sector bytes zero. A point inside the areas of
//!   more than one ARTCC has no unit (the model has no altitude to choose by, and facility-name
//!   order is not a reason): [`AirspaceLookup::unit_ambiguous`] reports it and counts the
//!   facility pairs, listed by [`AirspaceLookup::overlap_counts`]. NASR carries no sector
//!   geometry and ARTCC boundaries carry no altitude, so the sector and the altitude are not
//!   used; a handoff between sectors of one ARTCC therefore has no truth crossing and H1 counts
//!   it as unresolved.

use std::cell::RefCell;
use std::collections::BTreeMap;

use oa_airspace::geom::{area_inside, build_area, e7_to_deg, latlon_e7, Area};
use oa_airspace::model::{
    at, point_kind, str_at, str_or_empty, stratum, Model, ELEV_NONE, NONE_U32,
};
use oa_geo::LatLon;

use crate::truth::AirportRef;

/// One ARTCC of the model: identifier and the polygons of its HIGH and LOW boundaries.
// A7.2: cold struct, size guard waived
pub struct Unit {
    /// ARTCC identifier.
    pub id: String,
    area: Area,
}

/// One airport under one of its identifiers.
// A7.2: cold struct, size guard waived
struct AirportEntry {
    /// The airport's ICAO region, empty when the model gives none.
    region: String,
    data: AirportRef,
}

/// Lookups built once from a model.
// A7.2: cold struct, size guard waived
pub struct AirspaceLookup {
    /// Every distinct place of an identifier, sorted by (latitude, longitude).
    fixes: BTreeMap<String, Vec<(f64, f64)>>,
    /// Every airport an identifier names, one entry per (airport, region).
    airports: BTreeMap<String, Vec<AirportEntry>>,
    units: Vec<Unit>,
    zone: Option<usize>,
    /// Ambiguous positions examined, by the pair of facilities whose areas both contain them.
    overlaps: RefCell<BTreeMap<(String, String), u64>>,
    /// The model's ARTCC identifier.
    pub artcc: String,
    /// Fix rows whose identifier occurs in an earlier row.
    pub duplicate_fixes: usize,
    /// Fix identifiers that name more than one distinct place.
    pub ambiguous_fixes: usize,
    /// Airport identifiers that name airports of more than one ICAO region.
    pub ambiguous_airports: usize,
    /// ARTCC boundaries that were not a usable polygon and were left out.
    pub unusable_boundaries: usize,
}

/// Builds the lookups from a model.
pub fn lookup_from_model(m: &Model) -> AirspaceLookup {
    let p = &m.points;
    let mut fixes: BTreeMap<String, Vec<(f64, f64)>> = BTreeMap::new();
    let mut duplicate_fixes = 0usize;
    let mut airports: BTreeMap<String, Vec<AirportEntry>> = BTreeMap::new();
    // Thresholds by (airport identifier, airport region).
    let mut thresholds: BTreeMap<(String, String), Vec<(f64, f64)>> = BTreeMap::new();
    let region_of = |raw: u32| {
        at(&p.region, raw)
            .map(|r| str_or_empty(&m.strings, r))
            .unwrap_or("")
            .to_string()
    };
    for i in 0..p.kind.len() {
        let raw = i as u32;
        let (Some(kind), Some(lat), Some(lon), Some(ident)) = (
            at(&p.kind, raw),
            at(&p.lat, raw),
            at(&p.lon, raw),
            at(&p.ident, raw).and_then(|s| str_at(&m.strings, s)),
        ) else {
            continue;
        };
        let pos = (e7_to_deg(lat), e7_to_deg(lon));
        match kind {
            point_kind::FIX
            | point_kind::VOR
            | point_kind::VORTAC
            | point_kind::VOR_DME
            | point_kind::DME
            | point_kind::TACAN
            | point_kind::NDB
            | point_kind::TERMINAL_WAYPOINT => {
                let places = fixes.entry(ident.to_string()).or_default();
                if !places.is_empty() {
                    duplicate_fixes += 1;
                }
                if !places.contains(&pos) {
                    places.push(pos);
                }
            }
            point_kind::THRESHOLD => {
                if let Some(s) = at(&p.scope, raw).filter(|s| s.get() != NONE_U32) {
                    thresholds
                        .entry((str_or_empty(&m.strings, s).to_string(), region_of(raw)))
                        .or_default()
                        .push(pos);
                }
            }
            point_kind::AIRPORT => {
                let elev = at(&p.elev_ft, raw).unwrap_or(ELEV_NONE);
                if elev == ELEV_NONE {
                    continue;
                }
                let r = AirportRef {
                    elev_ft: elev,
                    thresholds: Vec::new(),
                    altimeters: Vec::new(),
                };
                let region = region_of(raw);
                let mut names = vec![ident.to_string()];
                if let Some(alt) = at(&p.alt_ident, raw).filter(|s| s.get() != NONE_U32) {
                    names.push(str_or_empty(&m.strings, alt).to_string());
                }
                for name in names {
                    airports.entry(name).or_default().push(AirportEntry {
                        region: region.clone(),
                        data: r.clone(),
                    });
                }
            }
            _ => {}
        }
    }
    // Thresholds are scoped by the airport's `ident` (the ICAO identifier where there is one) and
    // carry its region; the FAA identifier entry of the same airport gets the same list.
    for i in 0..p.kind.len() {
        let raw = i as u32;
        if at(&p.kind, raw) != Some(point_kind::AIRPORT) {
            continue;
        }
        let ident = at(&p.ident, raw).map_or("", |s| str_or_empty(&m.strings, s));
        let region = region_of(raw);
        let Some(list) = thresholds.get(&(ident.to_string(), region.clone())) else {
            continue;
        };
        let alt = at(&p.alt_ident, raw)
            .filter(|s| s.get() != NONE_U32)
            .map(|s| str_or_empty(&m.strings, s));
        for name in std::iter::once(ident).chain(alt) {
            for e in airports.get_mut(name).into_iter().flatten() {
                if e.region == region {
                    e.data.thresholds = list.clone();
                }
            }
        }
    }
    let ambiguous_airports = airports.values().filter(|v| v.len() > 1).count();
    // ARTCC boundaries, grouped by facility.
    let b = &m.boundaries;
    let mut rings: BTreeMap<String, Vec<Vec<LatLon>>> = BTreeMap::new();
    let mut unusable = 0usize;
    for i in 0..b.stratum.len() {
        let raw = i as u32;
        let Some(st) = at(&b.stratum, raw) else {
            continue;
        };
        if st != stratum::ARTCC_HIGH && st != stratum::ARTCC_LOW && st != stratum::ARTCC_UNLIMITED {
            continue;
        }
        let (Some(span), Some(fac)) = (
            at(&b.vertices, raw),
            at(&b.facility, raw).and_then(|f| str_at(&m.strings, f)),
        ) else {
            continue;
        };
        let lo = span.start.row();
        let ring: Option<Vec<LatLon>> = m
            .vertices
            .get(lo..lo.saturating_add(span.len as usize))
            .and_then(|vs| {
                vs.iter()
                    .map(|v| latlon_e7(v.lat, v.lon).ok())
                    .collect::<Option<Vec<_>>>()
            });
        match ring {
            Some(r) if r.len() >= 3 => rings.entry(fac.to_string()).or_default().push(r),
            _ => unusable += 1,
        }
    }
    let mut units = Vec::new();
    for (id, rs) in rings {
        match build_area(rs, 0.0) {
            Ok(area) => units.push(Unit { id, area }),
            Err(_) => unusable += 1,
        }
    }
    for places in fixes.values_mut() {
        places.sort_by(|a, b| {
            oa_math::cmp_canon_f64(a.0, b.0).then(oa_math::cmp_canon_f64(a.1, b.1))
        });
    }
    let ambiguous_fixes = fixes.values().filter(|v| v.len() > 1).count();
    let zone = units.iter().position(|u| u.id == m.meta.artcc);
    AirspaceLookup {
        fixes,
        airports,
        units,
        zone,
        overlaps: RefCell::new(BTreeMap::new()),
        artcc: m.meta.artcc.clone(),
        duplicate_fixes,
        ambiguous_fixes,
        ambiguous_airports,
        unusable_boundaries: unusable,
    }
}

/// A name as the model spells identifiers: white space trimmed, ASCII upper case. A lookup that
/// finds nothing as given tries this form, so any case or padding of an identifier joins.
fn norm_name(name: &str) -> String {
    name.trim().to_ascii_uppercase()
}

impl AirspaceLookup {
    /// Fix or navaid identifier to `(lat, lon)`; `None` when unknown or when the identifier
    /// names more than one place.
    pub fn fix(&self, name: &str) -> Option<(f64, f64)> {
        match self.fixes_of(name).map(Vec::as_slice) {
            Some([only]) => Some(*only),
            _ => None,
        }
    }

    fn fixes_of(&self, name: &str) -> Option<&Vec<(f64, f64)>> {
        self.fixes
            .get(name)
            .or_else(|| self.fixes.get(&norm_name(name)))
    }

    /// Every distinct place the identifier names.
    pub fn fix_all(&self, name: &str) -> Vec<(f64, f64)> {
        self.fixes_of(name).cloned().unwrap_or_default()
    }

    /// Airport identifier (ICAO or FAA) to its reference data; `None` when unknown or when the
    /// identifier names more than one airport (another ICAO region).
    pub fn airport(&self, name: &str) -> Option<AirportRef> {
        let list = self
            .airports
            .get(name)
            .or_else(|| self.airports.get(&norm_name(name)))?;
        match list.as_slice() {
            [only] => Some(only.data.clone()),
            _ => None,
        }
    }

    /// The airport with this identifier (ICAO or FAA) in this ICAO region; `None` when there is
    /// none or when two airports of that region share the identifier.
    pub fn airport_in(&self, name: &str, region: &str) -> Option<AirportRef> {
        let list = self
            .airports
            .get(name)
            .or_else(|| self.airports.get(&norm_name(name)))?;
        let mut hits = list.iter().filter(|e| e.region == region);
        match (hits.next(), hits.next()) {
            (Some(e), None) => Some(e.data.clone()),
            _ => None,
        }
    }

    /// Whether the point is inside the model's ARTCC.
    pub fn in_zone(&self, lat: f64, lon: f64) -> bool {
        let (Some(z), Ok(p)) = (
            self.zone.and_then(|i| self.units.get(i)),
            LatLon::new(lat, lon),
        ) else {
            return false;
        };
        area_inside(&z.area, p)
    }

    /// Every ARTCC whose area contains the point, in facility-name order.
    fn containing(&self, lat: f64, lon: f64) -> Vec<&Unit> {
        let Ok(p) = LatLon::new(lat, lon) else {
            return Vec::new();
        };
        self.units
            .iter()
            .filter(|u| area_inside(&u.area, p))
            .collect()
    }

    /// The ARTCC containing the point, in the `oa-fixm` unit format (sector bytes zero). `None`
    /// outside every area and also where the areas of several ARTCCs contain the point: no
    /// unit is chosen by name order (see [`AirspaceLookup::unit_ambiguous`]).
    pub fn unit_at(&self, lat: f64, lon: f64) -> Option<[u8; 8]> {
        let [u] = self.containing(lat, lon)[..] else {
            return None;
        };
        let mut out = [0u8; 8];
        for (o, b) in out.iter_mut().take(4).zip(u.id.bytes()) {
            *o = b;
        }
        Some(out)
    }

    /// Whether the areas of more than one ARTCC contain the point. Each such answer is counted
    /// under every pair of those facilities ([`AirspaceLookup::overlap_counts`]).
    pub fn unit_ambiguous(&self, lat: f64, lon: f64) -> bool {
        let hits = self.containing(lat, lon);
        if hits.len() < 2 {
            return false;
        }
        let mut counts = self.overlaps.borrow_mut();
        for (i, a) in hits.iter().enumerate() {
            for b in hits.iter().skip(i + 1) {
                *counts.entry((a.id.clone(), b.id.clone())).or_insert(0) += 1;
            }
        }
        true
    }

    /// The facility pairs whose areas overlapped at an ambiguous position, with the number of
    /// such positions examined, in pair order.
    pub fn overlap_counts(&self) -> Vec<((String, String), u64)> {
        self.overlaps
            .borrow()
            .iter()
            .map(|(k, &v)| (k.clone(), v))
            .collect()
    }

    /// Number of fixes, airports (identifier keys) and ARTCC polygons groups loaded, for the report.
    pub fn counts(&self) -> (usize, usize, usize) {
        (self.fixes.len(), self.airports.len(), self.units.len())
    }

    /// Whether the model's own ARTCC has a usable boundary.
    pub fn has_zone(&self) -> bool {
        self.zone.is_some()
    }
}
