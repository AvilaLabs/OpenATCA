//! Resolve issued runway-aid adaptations against the exact airspace snapshot supplied to the
//! regional runtime. Source rows remain in `oa-airspace`; this shell refuses missing or
//! ambiguous endpoints instead of inventing runway geometry.

use oa_airspace::core::model::{self, point_kind, Model, PointKey};
use oa_runway::adaptation::AidPairConfig;
use oa_runway::final_monitor::{self, FinalCourseInput, FinalMonitorConfig, FinalPair};
use oa_runway::plane::{self, Geo, Local};

const MAX_IDENT_MATCHES: usize = 64;
const FT_TO_M: f64 = 0.3048;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RunwayAidGeometry {
    pub pair: FinalPair,
    pub first_threshold: Local,
    pub first_inbound_east: f64,
    pub first_inbound_north: f64,
    pub first_width_m: f64,
    pub second_threshold: Local,
    pub second_inbound_east: f64,
    pub second_inbound_north: f64,
    pub second_width_m: f64,
    pub crda_offset_m: f64,
}
const _: () = assert!(core::mem::size_of::<RunwayAidGeometry>() == 184);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum GeometryRefusal {
    MalformedModel = 1,
    AirportMissing = 2,
    AirportAmbiguous = 3,
    EndpointMissing = 4,
    EndpointAmbiguous = 5,
    UnsupportedRunwayEnd = 6,
    GeometryOutOfRange = 7,
    PairRejected = 8,
}
const _: () = assert!(core::mem::size_of::<GeometryRefusal>() == 1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum RunwayAidRefusal {
    AdaptationUnavailable = 1,
    AdaptationInvalid = 2,
    GeometryRefused = 3,
}
const _: () = assert!(core::mem::size_of::<RunwayAidRefusal>() == 1);

fn lower_bound_ident(model: &Model, ident: &str) -> Result<usize, GeometryRefusal> {
    let n = model.points.kind.len();
    if model.points.ident.len() != n
        || model.points.region.len() != n
        || model.points.scope.len() != n
        || model.points.lat.len() != n
        || model.points.lon.len() != n
    {
        return Err(GeometryRefusal::MalformedModel);
    }
    let (mut low, mut high) = (0usize, n);
    while low < high {
        let middle = low + (high - low) / 2;
        let raw = u32::try_from(middle).map_err(|_| GeometryRefusal::MalformedModel)?;
        let key = model::point_key(model, model::PointIx::new(raw))
            .ok_or(GeometryRefusal::MalformedModel)?;
        if key.ident < ident {
            low = middle
                .checked_add(1)
                .ok_or(GeometryRefusal::MalformedModel)?;
        } else {
            high = middle;
        }
    }
    Ok(low)
}

fn airport_row(model: &Model, airport: &[u8; 4]) -> Result<(usize, String), GeometryRefusal> {
    let ident = std::str::from_utf8(airport).map_err(|_| GeometryRefusal::AirportMissing)?;
    let mut found = None;
    let mut seen = 0usize;
    let mut row = lower_bound_ident(model, ident)?;
    while row < model.points.kind.len() {
        let raw = u32::try_from(row).map_err(|_| GeometryRefusal::MalformedModel)?;
        let key = model::point_key(model, model::PointIx::new(raw))
            .ok_or(GeometryRefusal::MalformedModel)?;
        if key.ident != ident {
            break;
        }
        seen = seen.checked_add(1).ok_or(GeometryRefusal::MalformedModel)?;
        if seen > MAX_IDENT_MATCHES {
            return Err(GeometryRefusal::AirportAmbiguous);
        }
        if key.class == point_kind::class(point_kind::AIRPORT) && key.scope.is_empty() {
            if found.is_some() {
                return Err(GeometryRefusal::AirportAmbiguous);
            }
            found = Some((row, key.region.to_owned()));
        }
        row = row.checked_add(1).ok_or(GeometryRefusal::MalformedModel)?;
    }
    found.ok_or(GeometryRefusal::AirportMissing)
}

fn endpoint_row(
    model: &Model,
    airport: &str,
    region: &str,
    runway_end: &[u8; 3],
) -> Result<usize, GeometryRefusal> {
    let end = std::str::from_utf8(runway_end)
        .map_err(|_| GeometryRefusal::UnsupportedRunwayEnd)?
        .trim_end_matches('\0');
    let key = PointKey {
        ident: end,
        region,
        scope: airport,
        class: point_kind::class(point_kind::THRESHOLD),
    };
    model::find_point(model, key)
        .map(|ix| ix.row())
        .ok_or(GeometryRefusal::EndpointMissing)
}

fn reciprocal_end(name: &[u8; 3]) -> Result<[u8; 3], GeometryRefusal> {
    let text = std::str::from_utf8(name)
        .map_err(|_| GeometryRefusal::UnsupportedRunwayEnd)?
        .trim_end_matches('\0');
    let bytes = text.as_bytes();
    if bytes.len() < 2 || bytes.len() > 3 || !bytes[..2].iter().all(u8::is_ascii_digit) {
        return Err(GeometryRefusal::UnsupportedRunwayEnd);
    }
    let number = usize::from(bytes[0] - b'0') * 10 + usize::from(bytes[1] - b'0');
    if !(1..=36).contains(&number) {
        return Err(GeometryRefusal::UnsupportedRunwayEnd);
    }
    let opposite = ((number - 1 + 18) % 36) + 1;
    let side = match bytes.get(2).copied() {
        None => None,
        Some(b'L') => Some(b'R'),
        Some(b'R') => Some(b'L'),
        Some(b'C') => Some(b'C'),
        _ => return Err(GeometryRefusal::UnsupportedRunwayEnd),
    };
    let mut out = [0; 3];
    out[0] =
        b'0' + u8::try_from(opposite / 10).map_err(|_| GeometryRefusal::UnsupportedRunwayEnd)?;
    out[1] =
        b'0' + u8::try_from(opposite % 10).map_err(|_| GeometryRefusal::UnsupportedRunwayEnd)?;
    if let Some(side) = side {
        out[2] = side;
    }
    Ok(out)
}

fn geo_at(model: &Model, row: usize) -> Result<Geo, GeometryRefusal> {
    let lat = model
        .points
        .lat
        .get(row)
        .copied()
        .ok_or(GeometryRefusal::MalformedModel)?;
    let lon = model
        .points
        .lon
        .get(row)
        .copied()
        .ok_or(GeometryRefusal::MalformedModel)?;
    Ok(Geo {
        lat_deg: f64::from(lat) / 10_000_000.0,
        lon_deg: f64::from(lon) / 10_000_000.0,
    })
}

fn course_input(
    model: &Model,
    airport: &str,
    region: &str,
    plane: &plane::Plane,
    runway_id: u32,
    end_name: [u8; 3],
    extension_m: f64,
) -> Result<(FinalCourseInput, Local, f64, f64), GeometryRefusal> {
    let landing_row = endpoint_row(model, airport, region, &end_name)?;
    let opposite_name = reciprocal_end(&end_name)?;
    let opposite_row = endpoint_row(model, airport, region, &opposite_name)?;
    let threshold = plane::to_local(plane, geo_at(model, landing_row)?)
        .ok_or(GeometryRefusal::GeometryOutOfRange)?;
    let opposite = plane::to_local(plane, geo_at(model, opposite_row)?)
        .ok_or(GeometryRefusal::GeometryOutOfRange)?;
    let delta_east = threshold.east_m - opposite.east_m;
    let delta_north = threshold.north_m - opposite.north_m;
    let length = oa_math::hypot(delta_east, delta_north);
    if !length.is_finite() || !(1.0..=40_000.0).contains(&length) {
        return Err(GeometryRefusal::GeometryOutOfRange);
    }
    let inbound_east = delta_east / length;
    let inbound_north = delta_north / length;
    let outer = Local {
        east_m: threshold.east_m - inbound_east * extension_m,
        north_m: threshold.north_m - inbound_north * extension_m,
    };
    if !outer.east_m.is_finite()
        || !outer.north_m.is_finite()
        || oa_math::hypot(outer.east_m, outer.north_m) > plane::EXTENT_M
    {
        return Err(GeometryRefusal::GeometryOutOfRange);
    }
    Ok((
        FinalCourseInput {
            runway_id,
            end_name,
            outer_fix: outer,
            threshold,
        },
        threshold,
        inbound_east,
        inbound_north,
    ))
}

/// Build source-pinned final-course, CRDA and ARV geometry from one verified airspace model.
/// The result contains runway-centerline-derived endpoints only; it does not declare a published
/// FAF or charted procedure. Every width, extension, limit and tolerance comes from the issued
/// adaptation row.
pub fn resolve(model: &Model, config: AidPairConfig) -> Result<RunwayAidGeometry, GeometryRefusal> {
    let (airport_row, region) = airport_row(model, &config.airport)?;
    let airport_geo = geo_at(model, airport_row)?;
    let local_plane = plane::plane_new(airport_geo.lat_deg, airport_geo.lon_deg)
        .ok_or(GeometryRefusal::GeometryOutOfRange)?;
    let airport =
        std::str::from_utf8(&config.airport).map_err(|_| GeometryRefusal::AirportMissing)?;
    let extension_m = f64::from(config.centerline_extension_m);
    let (first, first_threshold, first_east, first_north) = course_input(
        model,
        airport,
        &region,
        &local_plane,
        config.first_runway_id,
        config.first_end_name,
        extension_m,
    )?;
    let (second, second_threshold, second_east, second_north) = course_input(
        model,
        airport,
        &region,
        &local_plane,
        config.second_runway_id,
        config.second_end_name,
        extension_m,
    )?;
    let pair = final_monitor::prepare_pair(
        first,
        second,
        FinalMonitorConfig {
            ntz_width_ft: f64::from(config.ntz_width_ft),
            lookahead_ns: u64::from(config.lookahead_ms) * 1_000_000,
            update_ns: u64::from(config.update_ms) * 1_000_000,
            minimum_separation_ft: f64::from(config.minimum_spacing_ft),
        },
    )
    .map_err(|_| GeometryRefusal::PairRejected)?;
    Ok(RunwayAidGeometry {
        pair,
        first_threshold,
        first_inbound_east: first_east,
        first_inbound_north: first_north,
        first_width_m: f64::from(config.first_width_ft) * FT_TO_M,
        second_threshold,
        second_inbound_east: second_east,
        second_inbound_north: second_north,
        second_width_m: f64::from(config.second_width_ft) * FT_TO_M,
        crda_offset_m: f64::from(config.crda_offset_mm) / 1_000.0,
    })
}

/// Resolve one pair from a package created by `oa-adapt::verify_current`. The package model is
/// compared with the exact model already installed in the regional runtime; a package pin alone
/// is not an issuer authentication claim.
pub fn resolve_verified(
    package: &oa_adapt::manifest::VerifiedPackage,
    runtime_model: &Model,
    pair_index: usize,
) -> Result<RunwayAidGeometry, RunwayAidRefusal> {
    if package.model() != runtime_model {
        return Err(RunwayAidRefusal::AdaptationInvalid);
    }
    let bytes = package
        .runway_aids_bytes()
        .ok_or(RunwayAidRefusal::AdaptationUnavailable)?;
    let profile =
        oa_runway::adaptation::parse(bytes).map_err(|_| RunwayAidRefusal::AdaptationInvalid)?;
    let config = profile
        .pair(pair_index)
        .ok_or(RunwayAidRefusal::AdaptationInvalid)?;
    resolve(runtime_model, config).map_err(|_| RunwayAidRefusal::GeometryRefused)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    // REQ: OA-RWY-022
    #[test]
    fn reciprocal_end_preserves_parallel_designator_sides() {
        assert_eq!(reciprocal_end(b"27L"), Ok(*b"09R"));
        assert_eq!(reciprocal_end(b"09R"), Ok(*b"27L"));
        assert_eq!(reciprocal_end(b"18C"), Ok(*b"36C"));
        assert_eq!(reciprocal_end(b"09\0"), Ok(*b"27\0"));
        assert_eq!(
            reciprocal_end(b"NE\0"),
            Err(GeometryRefusal::UnsupportedRunwayEnd)
        );
    }
}
