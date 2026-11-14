//! Cross-check of decoded rows against the M0 airspace model: which location designators,
//! airports, runways and navaids in the feed name something the model has, and which NOTAM
//! polygons touch the model's ARTCC boundary. Shell/offline code (it allocates freely, RULESET
//! C6); a pure function of the rows and the model.
//!
//! The model is for one ARTCC, and the FNS feed carries every centre and many countries, so two
//! scopes are reported: every row, and only the rows whose affected FIR is the model's ARTCC
//! (`ZAU` or `KZAU`). Only the second is expected to resolve nearly completely.
//!
//! * A **location** (`location`) resolves when the model has an airport whose identifier or
//!   alternate identifier is the value (the feed uses the FAA identifier for most United States
//!   airports and the ICAO one elsewhere), a fix, navaid or terminal waypoint of that
//!   identifier, or an ARTCC boundary of that facility.
//! * An **airport** (the `AirportHeliport` designator, else its ICAO indicator, else the
//!   location) resolves as an airport only.
//! * A **runway** resolves when the airport does and every runway end named (`10L/28R` is two
//!   ends) is a threshold of that airport in the model.
//! * A **navaid** (a feed navaid names only its type, `ILS`, and no designator) resolves when
//!   the airport does and, for an ILS, the model has a localizer of that airport: the model
//!   has no link from a localizer to a runway end that the feed's data could use.
//! * **Polygons** are tested against the model's ARTCC boundary of the model's own facility,
//!   high and low strata: a polygon touches it when a polygon vertex is inside the boundary or
//!   a sample of the boundary (at most 20 km apart along each geodesic edge) is inside the
//!   polygon. This is the geodesic containment of oa-geo, not an exact edge intersection; a
//!   boundary edge that crosses a polygon without a vertex of either inside would be missed,
//!   which the polygon sizes in the feed (runway surfaces) do not allow. Circles are tested by
//!   their centre only.

use crate::fields::Field;
use crate::rows::{code_str, has_circle, has_field, polygon_vertices, polygons_of, Notams};
use oa_airspace::geom::{latlon_e7, ring_samples};
use oa_airspace::model::{boundary_source, point_kind, str_or_empty, stratum, Model};
use oa_geo::{wgs84, LatLon, Polygon};
use std::collections::{BTreeMap, BTreeSet};

/// Counts for one identifier role in one scope.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    /// Distinct non-empty identifiers.
    pub distinct: u32,
    /// Of those, how many the model has.
    pub resolved: u32,
    /// Rows with a non-empty value.
    pub rows: u32,
    /// Of those, rows whose value the model has.
    pub rows_resolved: u32,
}

/// The four roles in one scope, with the distinct identifiers that did not resolve.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scope {
    pub location: Tally,
    pub airport: Tally,
    pub runway: Tally,
    pub navaid: Tally,
    pub unresolved_locations: Vec<String>,
    pub unresolved_airports: Vec<String>,
    pub unresolved_runways: Vec<String>,
    pub unresolved_navaids: Vec<String>,
}

/// The polygon and circle test against the model's own ARTCC boundary.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PolyCheck {
    /// Boundary polygons of the model's facility that were usable (high and low).
    pub boundaries: u32,
    /// Boundary polygons of the model's facility that oa-geo refused.
    pub boundaries_refused: u32,
    pub rows_with_polygons: u32,
    pub polygons: u32,
    /// Polygons oa-geo refused to build (fewer than three distinct vertices, an edge over
    /// 10,000 km, ...); they are not in `touching`.
    pub unusable: u32,
    /// Polygons that touch the ARTCC boundary, and the rows that own one.
    pub touching: u32,
    pub rows_touching: u32,
    pub circles: u32,
    pub circles_centre_inside: u32,
}

/// The result of [`cross_check`].
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CrossCheck {
    /// Every row.
    pub all: Scope,
    /// Rows whose affected FIR is the model's ARTCC.
    pub own: Scope,
    /// Rows of the own scope that have at least one location, for context.
    pub own_rows: u32,
    pub polygons: PolyCheck,
}

/// What each row asks of the model.
struct Ask {
    own: bool,
    location: String,
    airport: String,
    runway: String,
    navaid: String,
}

fn text(b: &[u8]) -> String {
    code_str(b).to_string()
}

fn asks(n: &Notams, artcc: &str) -> Vec<Ask> {
    let mut out = Vec::with_capacity(n.len);
    let own_fir = |fir: &str| fir == artcc || fir.strip_prefix('K') == Some(artcc);
    for i in 0..n.len {
        let get = |f: Field| has_field(n, i, f);
        let location = if get(Field::Location) {
            n.location.get(i).map(|v| text(v)).unwrap_or_default()
        } else {
            String::new()
        };
        let fir = if get(Field::Fir) {
            n.fir.get(i).map(|v| text(v)).unwrap_or_default()
        } else {
            String::new()
        };
        let apt = if get(Field::Airport) {
            n.apt.get(i).map(|v| text(v)).unwrap_or_default()
        } else {
            String::new()
        };
        let apt_icao = if get(Field::AirportIcao) {
            n.apt_icao.get(i).map(|v| text(v)).unwrap_or_default()
        } else {
            String::new()
        };
        let airport = if !apt.is_empty() { apt } else { apt_icao };
        let rwy = if get(Field::Runway) {
            n.rwy.get(i).map(|v| text(v)).unwrap_or_default()
        } else {
            String::new()
        };
        let nav = if get(Field::Navaid) {
            n.navaid.get(i).map(|v| text(v)).unwrap_or_default()
        } else {
            String::new()
        };
        out.push(Ask {
            own: own_fir(&fir),
            runway: if rwy.is_empty() || airport.is_empty() {
                String::new()
            } else {
                format!("{airport} {rwy}")
            },
            navaid: if nav.is_empty() || airport.is_empty() {
                String::new()
            } else {
                format!("{airport} {nav}")
            },
            location,
            airport,
        });
    }
    out
}

/// What the model has, keyed for lookup.
struct Known<'a> {
    /// Airport identifier or alternate identifier to the model's airport identifier.
    airports: BTreeMap<&'a str, &'a str>,
    /// Fix, navaid and terminal waypoint identifiers.
    fixes: BTreeSet<&'a str>,
    facilities: BTreeSet<&'a str>,
    /// (airport identifier, runway end) of every threshold.
    thresholds: BTreeSet<(&'a str, &'a str)>,
    /// Airport identifiers with a localizer.
    localizers: BTreeSet<&'a str>,
}

fn known(model: &Model) -> Known<'_> {
    let p = &model.points;
    let st = &model.strings;
    let mut k = Known {
        airports: BTreeMap::new(),
        fixes: BTreeSet::new(),
        facilities: BTreeSet::new(),
        thresholds: BTreeSet::new(),
        localizers: BTreeSet::new(),
    };
    for (i, &kind) in p.kind.iter().enumerate() {
        let id = p.ident.get(i).map_or("", |&s| str_or_empty(st, s));
        let scope = p.scope.get(i).map_or("", |&s| str_or_empty(st, s));
        match kind {
            point_kind::FIX..=point_kind::NDB | point_kind::TERMINAL_WAYPOINT => {
                k.fixes.insert(id);
            }
            point_kind::AIRPORT => {
                k.airports.insert(id, id);
                let alt = p.alt_ident.get(i).map_or("", |&s| str_or_empty(st, s));
                if !alt.is_empty() {
                    k.airports.insert(alt, id);
                }
            }
            point_kind::THRESHOLD => {
                k.thresholds.insert((scope, id));
            }
            point_kind::LOCALIZER => {
                k.localizers.insert(scope);
            }
            _ => {}
        }
    }
    for f in &model.boundaries.facility {
        k.facilities.insert(str_or_empty(st, *f));
    }
    k
}

/// Splits `AIRPORT RWY` and resolves every runway end.
fn runway_resolves(k: &Known<'_>, key: &str) -> bool {
    let Some((apt, rwy)) = key.split_once(' ') else {
        return false;
    };
    let Some(ident) = k.airports.get(apt) else {
        return false;
    };
    let mut ends = rwy.split('/').peekable();
    ends.peek().is_some() && ends.all(|e| !e.is_empty() && k.thresholds.contains(&(*ident, e)))
}

fn navaid_resolves(k: &Known<'_>, key: &str) -> bool {
    let Some((apt, ty)) = key.split_once(' ') else {
        return false;
    };
    let Some(ident) = k.airports.get(apt) else {
        return false;
    };
    ty == "ILS" && k.localizers.contains(ident)
}

/// Tallies one role: `values` maps an identifier to (rows in all scope, rows in own scope).
fn tally(
    values: &BTreeMap<String, (u32, u32)>,
    resolves: impl Fn(&str) -> bool,
    own: bool,
) -> (Tally, Vec<String>) {
    let mut t = Tally::default();
    let mut missing = Vec::new();
    for (name, &(all_rows, own_rows)) in values {
        let n = if own { own_rows } else { all_rows };
        if n == 0 {
            continue;
        }
        t.distinct += 1;
        t.rows += n;
        if resolves(name) {
            t.resolved += 1;
            t.rows_resolved += n;
        } else {
            missing.push(name.clone());
        }
    }
    (t, missing)
}

fn collect<'a>(asks: &'a [Ask], pick: impl Fn(&'a Ask) -> &'a str) -> BTreeMap<String, (u32, u32)> {
    let mut m: BTreeMap<String, (u32, u32)> = BTreeMap::new();
    for a in asks {
        let s = pick(a);
        if s.is_empty() {
            continue;
        }
        let e = m.entry(s.to_string()).or_insert((0, 0));
        e.0 += 1;
        if a.own {
            e.1 += 1;
        }
    }
    m
}

fn ring_of(model: &Model, row: usize) -> Option<Vec<LatLon>> {
    let span = model.boundaries.vertices.get(row)?;
    let a = span.start.row();
    let verts = model.vertices.get(a..a.checked_add(span.len as usize)?)?;
    verts.iter().map(|v| latlon_e7(v.lat, v.lon).ok()).collect()
}

/// One usable boundary: the polygon and samples of its edges.
struct Boundary {
    polygon: Polygon,
    samples: Vec<LatLon>,
}

fn boundaries(model: &Model, check: &mut PolyCheck) -> Vec<Boundary> {
    let ell = wgs84();
    let b = &model.boundaries;
    let mut out = Vec::new();
    for row in 0..b.source.len() {
        let is_own = b
            .facility
            .get(row)
            .is_some_and(|&f| str_or_empty(&model.strings, f) == model.meta.artcc);
        let strat = b.stratum.get(row).copied().unwrap_or(0);
        let nasr = b.source.get(row).copied() == Some(boundary_source::NASR_ARB);
        if !is_own || !nasr || !matches!(strat, stratum::ARTCC_HIGH | stratum::ARTCC_LOW) {
            continue;
        }
        let built = ring_of(model, row).and_then(|ring| {
            let polygon = Polygon::new(&ell, &ring).ok()?;
            let samples = ring_samples(&ell, &ring).ok()?;
            Some(Boundary { polygon, samples })
        });
        match built {
            Some(x) => {
                check.boundaries += 1;
                out.push(x);
            }
            None => check.boundaries_refused += 1,
        }
    }
    out
}

fn bbox(v: &[LatLon]) -> (f64, f64, f64, f64) {
    let mut b = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for p in v {
        b.0 = b.0.min(p.lat());
        b.1 = b.1.max(p.lat());
        b.2 = b.2.min(p.lon());
        b.3 = b.3.max(p.lon());
    }
    b
}

fn polygon_check(n: &Notams, bounds: &[Boundary], check: &mut PolyCheck) {
    let ell = wgs84();
    for row in 0..n.len {
        let polys = polygons_of(n, row);
        if !polys.is_empty() {
            check.rows_with_polygons += 1;
        }
        let mut row_touches = false;
        for p in polys {
            check.polygons += 1;
            let (lat, lon) = polygon_vertices(n, p);
            let ring: Option<Vec<LatLon>> = lat
                .iter()
                .zip(lon)
                .map(|(&a, &b)| latlon_e7(a, b).ok())
                .collect();
            let Some(ring) = ring else {
                check.unusable += 1;
                continue;
            };
            let Ok(poly) = Polygon::new(&ell, &ring) else {
                check.unusable += 1;
                continue;
            };
            let (la0, la1, lo0, lo1) = bbox(&ring);
            let touches = bounds.iter().any(|bd| {
                ring.iter().any(|&v| bd.polygon.contains(v))
                    || bd.samples.iter().any(|s| {
                        (la0..=la1).contains(&s.lat())
                            && (lo0..=lo1).contains(&s.lon())
                            && poly.contains(*s)
                    })
            });
            if touches {
                check.touching += 1;
                row_touches = true;
            }
        }
        if row_touches {
            check.rows_touching += 1;
        }
        if has_circle(n, row) {
            check.circles += 1;
            let c = n
                .lat_e7
                .get(row)
                .zip(n.lon_e7.get(row))
                .and_then(|(&a, &b)| latlon_e7(a, b).ok());
            if c.is_some_and(|c| bounds.iter().any(|bd| bd.polygon.contains(c))) {
                check.circles_centre_inside += 1;
            }
        }
    }
}

/// Resolves the rows' locations, airports, runways and navaids against `model`, and tests their
/// geometry against the model's ARTCC boundary.
pub fn cross_check(model: &Model, rows: &Notams) -> CrossCheck {
    let k = known(model);
    let asks = asks(rows, &model.meta.artcc);
    let loc = collect(&asks, |a| &a.location);
    let apt = collect(&asks, |a| &a.airport);
    let rwy = collect(&asks, |a| &a.runway);
    let nav = collect(&asks, |a| &a.navaid);
    let loc_ok =
        |s: &str| k.airports.contains_key(s) || k.fixes.contains(s) || k.facilities.contains(s);
    let apt_ok = |s: &str| k.airports.contains_key(s);
    let rwy_ok = |s: &str| runway_resolves(&k, s);
    let nav_ok = |s: &str| navaid_resolves(&k, s);

    let mut out = CrossCheck::default();
    for (own, scope) in [(false, &mut out.all), (true, &mut out.own)] {
        (scope.location, scope.unresolved_locations) = tally(&loc, loc_ok, own);
        (scope.airport, scope.unresolved_airports) = tally(&apt, apt_ok, own);
        (scope.runway, scope.unresolved_runways) = tally(&rwy, rwy_ok, own);
        (scope.navaid, scope.unresolved_navaids) = tally(&nav, nav_ok, own);
    }
    out.own_rows = asks.iter().filter(|a| a.own).count() as u32;
    let bounds = boundaries(model, &mut out.polygons);
    polygon_check(rows, &bounds, &mut out.polygons);
    out
}

/// One line per role for a report.
pub fn format_cross_check(c: &CrossCheck) -> String {
    fn line(label: &str, t: &Tally) -> String {
        format!(
            "  {label}: {}/{} distinct resolve, {}/{} rows\n",
            t.resolved, t.distinct, t.rows_resolved, t.rows
        )
    }
    let mut s = String::new();
    for (name, sc) in [("all centres", &c.all), ("model ARTCC only", &c.own)] {
        s.push_str(name);
        s.push_str(":\n");
        s.push_str(&line("location", &sc.location));
        s.push_str(&line("airport", &sc.airport));
        s.push_str(&line("runway", &sc.runway));
        s.push_str(&line("navaid", &sc.navaid));
    }
    let p = &c.polygons;
    s.push_str(&format!(
        "polygons: {} boundary polygons usable ({} refused); {} rows with polygons, {} polygons \
         ({} unusable), {} touch the ARTCC boundary in {} rows; {} circles, {} centres inside\n",
        p.boundaries,
        p.boundaries_refused,
        p.rows_with_polygons,
        p.polygons,
        p.unusable,
        p.touching,
        p.rows_touching,
        p.circles,
        p.circles_centre_inside
    ));
    s
}
