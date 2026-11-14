//! The airspace lookups of [`crate::truth::Env`] filled from the M0 model (`oa-airspace`).
//! Core (pure): a function of the model.
//!
//! * Fixes and navaids: points of the fix, VOR, DME, TACAN, NDB and terminal-waypoint kinds by
//!   identifier. An identifier that occurs in more than one region takes the first row of the
//!   model's sort order (ident, region, scope), which is deterministic; the others are counted
//!   in [`AirspaceLookup::duplicate_fixes`] so the report can say so.
//! * Airports: the airport points by ICAO identifier and by FAA identifier; field elevation from
//!   the point, thresholds from the runway-threshold points whose scope is the airport. An
//!   airport with no published elevation is not offered (nothing is made up).
//! * Zone: the model's own ARTCC (`Meta::artcc`), its HIGH and LOW boundaries together.
//! * Unit: the ARTCC boundary containing the point (any ARTCC in the model, HIGH or LOW), as the
//!   ARTCC identifier in bytes 0 to 3 with the sector bytes zero. NASR carries no sector
//!   geometry and ARTCC boundaries carry no altitude, so the sector and the altitude are not
//!   used; a handoff between sectors of one ARTCC therefore has no truth crossing and H1 counts
//!   it as unresolved.

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

/// Lookups built once from a model.
// A7.2: cold struct, size guard waived
pub struct AirspaceLookup {
    fixes: BTreeMap<String, (f64, f64)>,
    airports: BTreeMap<String, AirportRef>,
    units: Vec<Unit>,
    zone: Option<usize>,
    /// The model's ARTCC identifier.
    pub artcc: String,
    /// Fix identifiers that occur more than once (first row used).
    pub duplicate_fixes: usize,
    /// ARTCC boundaries that were not a usable polygon and were left out.
    pub unusable_boundaries: usize,
}

/// Builds the lookups from a model.
pub fn lookup_from_model(m: &Model) -> AirspaceLookup {
    let p = &m.points;
    let mut fixes: BTreeMap<String, (f64, f64)> = BTreeMap::new();
    let mut duplicate_fixes = 0usize;
    let mut airports: BTreeMap<String, AirportRef> = BTreeMap::new();
    let mut thresholds: BTreeMap<String, Vec<(f64, f64)>> = BTreeMap::new();
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
                if fixes.contains_key(ident) {
                    duplicate_fixes += 1;
                } else {
                    fixes.insert(ident.to_string(), pos);
                }
            }
            point_kind::THRESHOLD => {
                if let Some(s) = at(&p.scope, raw).filter(|s| s.get() != NONE_U32) {
                    thresholds
                        .entry(str_or_empty(&m.strings, s).to_string())
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
                };
                airports
                    .entry(ident.to_string())
                    .or_insert_with(|| r.clone());
                if let Some(alt) = at(&p.alt_ident, raw).filter(|s| s.get() != NONE_U32) {
                    airports
                        .entry(str_or_empty(&m.strings, alt).to_string())
                        .or_insert(r);
                }
            }
            _ => {}
        }
    }
    // Thresholds are scoped by the airport's `ident` (the ICAO identifier where there is one);
    // the FAA identifier entry of the same airport gets the same list.
    for i in 0..p.kind.len() {
        let raw = i as u32;
        if at(&p.kind, raw) != Some(point_kind::AIRPORT) {
            continue;
        }
        let ident = at(&p.ident, raw).map_or("", |s| str_or_empty(&m.strings, s));
        let Some(list) = thresholds.get(ident) else {
            continue;
        };
        let alt = at(&p.alt_ident, raw)
            .filter(|s| s.get() != NONE_U32)
            .map(|s| str_or_empty(&m.strings, s));
        for name in std::iter::once(ident).chain(alt) {
            if let Some(a) = airports.get_mut(name) {
                a.thresholds = list.clone();
            }
        }
    }
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
    let zone = units.iter().position(|u| u.id == m.meta.artcc);
    AirspaceLookup {
        fixes,
        airports,
        units,
        zone,
        artcc: m.meta.artcc.clone(),
        duplicate_fixes,
        unusable_boundaries: unusable,
    }
}

impl AirspaceLookup {
    /// Fix or navaid identifier to `(lat, lon)`.
    pub fn fix(&self, name: &str) -> Option<(f64, f64)> {
        self.fixes.get(name).copied()
    }

    /// Airport identifier (ICAO or FAA) to its reference data.
    pub fn airport(&self, name: &str) -> Option<AirportRef> {
        self.airports.get(name).cloned()
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

    /// The ARTCC containing the point, in the `oa-fixm` unit format (sector bytes zero).
    pub fn unit_at(&self, lat: f64, lon: f64) -> Option<[u8; 8]> {
        let p = LatLon::new(lat, lon).ok()?;
        let u = self.units.iter().find(|u| area_inside(&u.area, p))?;
        let mut out = [0u8; 8];
        for (o, b) in out.iter_mut().take(4).zip(u.id.bytes()) {
            *o = b;
        }
        Some(out)
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
