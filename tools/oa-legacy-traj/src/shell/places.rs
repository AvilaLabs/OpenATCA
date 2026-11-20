//! Where the named points are. The recordings are the only source: a route point of an SFDPS
//! expanded route carries its coordinates, and a name with coordinates in any route has a place.
//! A name that has coordinates at two places more than a nautical mile apart is ambiguous and
//! gets none. Nothing is looked up in a navigation database.

use std::collections::BTreeMap;

use crate::core::cross::flat_dist_m;
use crate::core::types::{AirState, Place};
use oa_bench::legacy::{Legacy, NO_COORD};

/// Metres in the distance within which two coordinates of one name are the same place.
const SAME_PLACE_M: f64 = 1852.0;

/// What a name resolved to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Found {
    At(Place),
    Unknown,
    Ambiguous,
}

/// The places of the names of the route points of a legacy table.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, Default)]
pub struct PlaceBook {
    names: BTreeMap<[u8; 8], Vec<Place>>,
}

/// The place of a coordinate pair in 1e-7 degree.
pub fn place_of_e7(lat_e7: i32, lon_e7: i32) -> Place {
    Place {
        lat_deg: f64::from(lat_e7) / 1e7,
        lon_deg: f64::from(lon_e7) / 1e7,
    }
}

/// Collects the places of every route point that has coordinates.
pub fn place_book(leg: &Legacy) -> PlaceBook {
    let mut book = PlaceBook::default();
    let p = &leg.route_points;
    for ((fix, &lat), &lon) in p.fix.iter().zip(&p.lat_e7).zip(&p.lon_e7) {
        if lat == NO_COORD || lon == NO_COORD || fix.iter().all(|&b| b == 0) {
            continue;
        }
        let here = place_of_e7(lat, lon);
        let list = book.names.entry(*fix).or_default();
        if !list
            .iter()
            .any(|q| flat_dist_m(*q, here.lat_deg, here.lon_deg) <= SAME_PLACE_M)
        {
            list.push(here);
        }
    }
    book
}

/// The place of a name.
pub fn find(book: &PlaceBook, name: &[u8; 8]) -> Found {
    match book.names.get(name).map(Vec::as_slice) {
        None | Some([]) => Found::Unknown,
        Some([one]) => Found::At(*one),
        Some(_) => Found::Ambiguous,
    }
}

/// Counts of the book: names with one place, and names with several.
pub fn book_counts(book: &PlaceBook) -> (u64, u64) {
    let one = book.names.values().filter(|v| v.len() == 1).count() as u64;
    let many = book.names.values().filter(|v| v.len() > 1).count() as u64;
    (one, many)
}

/// The most candidate landings kept per airport when deriving its reference point.
const MAX_CANDIDATES: usize = 2000;
/// Fewest landings that must agree for a derived reference point.
const MIN_AGREE: usize = 3;
/// Landings within this of each other agree, metres (the runway radius of the scoring).
const AGREE_M: f64 = 5.0 * 1852.0;

/// The airports' reference points where the recordings give none: for each airport that flights
/// name as their arrival, the centre of the largest group of landings (an airborne report followed
/// by a ground one, within 120 s) of those flights' aircraft that lie within [`AGREE_M`] of each
/// other, found among the landings in the flight's interval; at least [`MIN_AGREE`] must agree.
/// The result is the average position of the last airborne reports of that group. A landing
/// that happens at the airport by construction is within the radius of it, so the landing test
/// against a derived point only rejects landings that are elsewhere.
pub fn derive_airports(
    flights: &[(String, u32, i64, i64)],
    states: &BTreeMap<u32, Vec<AirState>>,
) -> BTreeMap<String, Place> {
    let mut cand: BTreeMap<&str, Vec<Place>> = BTreeMap::new();
    for (apt, addr, from, to) in flights {
        let Some(st) = states.get(addr) else {
            continue;
        };
        let list = cand.entry(apt.as_str()).or_default();
        for w in st.windows(2) {
            let (Some(a), Some(b)) = (w.first(), w.get(1)) else {
                continue;
            };
            if !a.ground
                && b.ground
                && b.t_ms - a.t_ms <= 120_000
                && a.t_ms >= *from
                && a.t_ms <= *to
            {
                list.push(Place {
                    lat_deg: a.lat_deg,
                    lon_deg: a.lon_deg,
                });
            }
        }
    }
    let mut out = BTreeMap::new();
    for (apt, mut list) in cand {
        list.truncate(MAX_CANDIDATES);
        let mut best: Option<(usize, usize)> = None;
        for (i, p) in list.iter().enumerate() {
            let n = list
                .iter()
                .filter(|q| flat_dist_m(*p, q.lat_deg, q.lon_deg) <= AGREE_M)
                .count();
            if best.is_none_or(|b| n > b.0) {
                best = Some((n, i));
            }
        }
        let Some((n, i)) = best else {
            continue;
        };
        let Some(&centre) = list.get(i) else {
            continue;
        };
        if n < MIN_AGREE {
            continue;
        }
        let group: Vec<&Place> = list
            .iter()
            .filter(|q| flat_dist_m(centre, q.lat_deg, q.lon_deg) <= AGREE_M)
            .collect();
        let k = group.len() as f64;
        out.insert(
            apt.to_string(),
            Place {
                lat_deg: group.iter().map(|q| q.lat_deg).sum::<f64>() / k,
                lon_deg: group.iter().map(|q| q.lon_deg).sum::<f64>() / k,
            },
        );
    }
    out
}

/// The place the route points give an airport, under its code as written or with the K of a
/// four-letter contiguous US code; an airport the book gives no single place for has none.
pub fn book_airport(book: &PlaceBook, key: &str) -> Option<Place> {
    [format!("K{key}"), key.to_string()].iter().find_map(|n| {
        match find(book, &oa_bench::legacy::pad::<8>(n.as_bytes())) {
            Found::At(p) => Some(p),
            _ => None,
        }
    })
}
