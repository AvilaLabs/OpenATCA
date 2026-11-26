//! Bounded normalized route-instruction parser, independent of command/file policy.
use crate::core::route::{FixIdentity, Label, RoutePlan, RouteStep, MAX_ROUTE_STEPS};

/// C6/C7: 32 instructions of up to 128 bytes normally fit; longer input is refused before scan.
pub const MAX_ROUTE_TEXT_BYTES: usize = 4096;
const MAX_TOKEN_BYTES: usize = 256;
const MAX_TOKEN_FIELDS: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseReason {
    Empty,
    TextLimit,
    StepLimit,
    TokenLimit,
    UnknownToken,
    FieldCount,
    InvalidLabel,
    InvalidNumber,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouteParseError {
    pub reason: ParseReason,
    /// Unknown when the byte cap prevented token scanning.
    pub offered: Option<usize>,
    pub admitted: usize,
    pub refused: usize,
    pub unexamined: usize,
}
// A7.2: cold one-shot parser result; all work is bounded by MAX_ROUTE_TEXT_BYTES.

fn label<const N: usize>(text: &str, empty: bool) -> Result<Label<N>, ParseReason> {
    let text = if empty && text == "-" { "" } else { text };
    if (!empty && text.is_empty())
        || text.len() > N
        || !text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(ParseReason::InvalidLabel);
    }
    let mut bytes = [0; N];
    for (dst, src) in bytes.iter_mut().zip(text.bytes()) {
        *dst = src;
    }
    Ok(Label {
        bytes,
        len: u8::try_from(text.len()).map_err(|_| ParseReason::InvalidLabel)?,
    })
}
fn field<'a>(fields: &[&'a str], i: usize) -> Result<&'a str, ParseReason> {
    fields.get(i).copied().ok_or(ParseReason::FieldCount)
}
fn number<T: std::str::FromStr>(text: &str) -> Result<T, ParseReason> {
    text.parse().map_err(|_| ParseReason::InvalidNumber)
}
fn one(text: &str) -> Result<u8, ParseReason> {
    if text.len() != 1 {
        return Err(ParseReason::InvalidLabel);
    };
    text.as_bytes()
        .first()
        .copied()
        .ok_or(ParseReason::InvalidLabel)
}
fn fix(fields: &[&str], start: usize) -> Result<FixIdentity, ParseReason> {
    Ok(FixIdentity {
        ident: label(field(fields, start)?, false)?,
        region: label(field(fields, start + 1)?, true)?,
        class: number(field(fields, start + 2)?)?,
        scope: label(field(fields, start + 3)?, true)?,
    })
}
fn token(text: &str) -> Result<RouteStep, ParseReason> {
    if text.len() > MAX_TOKEN_BYTES {
        return Err(ParseReason::TokenLimit);
    }
    let fields: Vec<_> = text.split(':').take(MAX_TOKEN_FIELDS + 1).collect();
    match (field(&fields, 0)?, fields.len()) {
        ("FIX", 5) => Ok(RouteStep::Fix(fix(&fields, 1)?)),
        ("DCT", 5) => Ok(RouteStep::Direct(fix(&fields, 1)?)),
        ("AWY", 12) => Ok(RouteStep::Airway {
            designator: label(field(&fields, 1)?, false)?,
            location: one(field(&fields, 2)?)?,
            entry: fix(&fields, 3)?,
            exit: fix(&fields, 7)?,
            planned_altitude_ft: number(field(&fields, 11)?)?,
        }),
        ("PROC", 7) => Ok(RouteStep::Procedure {
            airport_ident: label(field(&fields, 1)?, false)?,
            airport_region: label(field(&fields, 2)?, false)?,
            kind: number(field(&fields, 3)?)?,
            ident: label(field(&fields, 4)?, false)?,
            route_type: one(field(&fields, 5)?)?,
            transition: label(field(&fields, 6)?, true)?,
        }),
        ("FIX" | "DCT" | "AWY" | "PROC", _) => Err(ParseReason::FieldCount),
        _ => Err(ParseReason::UnknownToken),
    }
}

/// Parse bounded, identity-qualified instructions. No ambiguity is resolved from proximity or
/// implicit region/class preference. On a malformed token its unexamined suffix is counted.
pub fn parse_route_steps(text: &str) -> Result<RoutePlan, RouteParseError> {
    if text.len() > MAX_ROUTE_TEXT_BYTES {
        return Err(RouteParseError {
            reason: ParseReason::TextLimit,
            offered: None,
            admitted: 0,
            refused: 0,
            unexamined: 0,
        });
    }
    let offered = text.split_ascii_whitespace().count(); // at most 4096 byte-delimited tokens
    if offered == 0 {
        return Err(RouteParseError {
            reason: ParseReason::Empty,
            offered: Some(offered),
            admitted: 0,
            refused: 0,
            unexamined: 0,
        });
    }
    if offered > MAX_ROUTE_STEPS {
        return Err(RouteParseError {
            reason: ParseReason::StepLimit,
            offered: Some(offered),
            admitted: 0,
            refused: 0,
            unexamined: offered,
        });
    }
    let empty = FixIdentity {
        ident: Label {
            bytes: [0; 12],
            len: 0,
        },
        region: Label {
            bytes: [0; 4],
            len: 0,
        },
        scope: Label {
            bytes: [0; 12],
            len: 0,
        },
        class: 0,
    };
    let mut plan = RoutePlan {
        steps: [RouteStep::Fix(empty); MAX_ROUTE_STEPS],
        len: 0,
    };
    for (index, text) in text.split_ascii_whitespace().enumerate() {
        let step = token(text).map_err(|reason| RouteParseError {
            reason,
            offered: Some(offered),
            admitted: index,
            refused: 1,
            unexamined: offered - index - 1,
        })?;
        let destination = plan.steps.get_mut(index).ok_or(RouteParseError {
            reason: ParseReason::StepLimit,
            offered: Some(offered),
            admitted: index,
            refused: 1,
            unexamined: offered - index - 1,
        })?;
        *destination = step;
        plan.len = plan.len.checked_add(1).ok_or(RouteParseError {
            reason: ParseReason::StepLimit,
            offered: Some(offered),
            admitted: index,
            refused: 1,
            unexamined: offered - index - 1,
        })?;
    }
    Ok(plan)
}

// A6: shell adapter for already parsed ICAO Item 15 values. This module deliberately accepts
// parsed atoms rather than depending on oa-nam, allowing the NAM-to-session dependency proposal
// to remain separate. All identities are resolved from the caller's immutable current model.
use crate::core::{model, route};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item15Reject {
    MalformedGroup,
    UnknownPoint,
    AmbiguousPoint,
    UnknownAirway,
    AirwayDoesNotContainPoint,
    Discontinuity,
    UnsupportedProcedure,
    MissingVariation,
    InvalidVariation,
    MissingRouteLevel,
    AltitudeBelowMea,
    Capacity,
    Model,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MagneticVariationSource {
    pub reference: route::FixIdentity,
    /// East-positive hundredths of a degree, supplied by a parsed NASR/CIFP source row.
    pub variation_cdeg: i16,
    pub epoch: Option<String>,
    pub document: String,
    pub locator: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BearingDistanceEvidence {
    pub reference: route::FixIdentity,
    pub bearing_magnetic_deg: u16,
    pub distance_nm: u16,
    pub variation_cdeg: i16,
    pub epoch: Option<String>,
    pub document: String,
    pub locator: String,
    pub target: route::FixIdentity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedItem15 {
    pub route: route::ConvertedRoute,
    pub initial_level_ft: Option<i32>,
    /// Parsed level changes keyed by original element index and spelling, preserved alongside
    /// the expanded geometry for a caller that maintains the filed profile.
    pub level_changes: Vec<FiledLevelChange>,
    pub bearing_distance: Vec<BearingDistanceEvidence>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FiledLevelChange {
    pub element_index: u8,
    pub point_token: String,
    pub altitude_ft: i32,
}

fn route_string(m: &model::Model, ix: model::StrIx) -> Result<&str, Item15Reject> {
    if ix == model::StrIx::NONE {
        return Ok("");
    }
    model::str_at(&m.strings, ix).ok_or(Item15Reject::Model)
}

fn route_label<const N: usize>(s: &str, empty_ok: bool) -> Result<route::Label<N>, Item15Reject> {
    if (!empty_ok && s.is_empty()) || s.len() > N || !s.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err(Item15Reject::Model);
    }
    let mut bytes = [0; N];
    bytes
        .get_mut(..s.len())
        .ok_or(Item15Reject::Model)?
        .copy_from_slice(s.as_bytes());
    Ok(route::Label {
        bytes,
        len: u8::try_from(s.len()).map_err(|_| Item15Reject::Model)?,
    })
}

fn point_identity(m: &model::Model, row: usize) -> Result<route::FixIdentity, Item15Reject> {
    let kind = *m.points.kind.get(row).ok_or(Item15Reject::Model)?;
    if kind == 0 || kind > model::point_kind::MAX {
        return Err(Item15Reject::Model);
    }
    Ok(route::FixIdentity {
        ident: route_label(
            route_string(m, *m.points.ident.get(row).ok_or(Item15Reject::Model)?)?,
            false,
        )?,
        region: route_label(
            route_string(m, *m.points.region.get(row).ok_or(Item15Reject::Model)?)?,
            true,
        )?,
        scope: route_label(
            route_string(m, *m.points.scope.get(row).ok_or(Item15Reject::Model)?)?,
            true,
        )?,
        class: model::point_kind::class(kind),
    })
}

fn identity_eq(a: route::FixIdentity, b: route::FixIdentity) -> bool {
    a.ident == b.ident && a.region == b.region && a.scope == b.scope && a.class == b.class
}

fn push_identity(
    out: &mut Vec<route::FixIdentity>,
    value: route::FixIdentity,
) -> Result<(), Item15Reject> {
    if out.iter().any(|item| identity_eq(*item, value)) {
        return Ok(());
    }
    if out.len() >= route::MAX_ROUTE_POINTS {
        return Err(Item15Reject::Capacity);
    }
    out.push(value);
    Ok(())
}

fn coordinate_e7(text: &str) -> Result<Option<(i32, i32)>, Item15Reject> {
    let b = text.as_bytes();
    let number = |part: &[u8]| -> Result<u16, Item15Reject> {
        if part.is_empty() || !part.iter().all(u8::is_ascii_digit) {
            return Err(Item15Reject::UnknownPoint);
        }
        part.iter()
            .try_fold(0u16, |n, c| {
                n.checked_mul(10)?.checked_add(u16::from(*c - b'0'))
            })
            .ok_or(Item15Reject::UnknownPoint)
    };
    let signed = |deg: u16, min: u16, hemi: u8, max: u16| -> Result<i32, Item15Reject> {
        if deg > max || min >= 60 || (deg == max && min != 0) {
            return Err(Item15Reject::UnknownPoint);
        }
        let frac = (u32::from(min) * 10_000_000 + 30) / 60;
        let v = i32::from(deg) * 10_000_000
            + i32::try_from(frac).map_err(|_| Item15Reject::UnknownPoint)?;
        match hemi {
            b'N' | b'E' => Ok(v),
            b'S' | b'W' => Ok(-v),
            _ => Err(Item15Reject::UnknownPoint),
        }
    };
    match b.len() {
        7 => Ok(Some((
            signed(
                number(b.get(0..2).ok_or(Item15Reject::UnknownPoint)?)?,
                0,
                *b.get(2).ok_or(Item15Reject::UnknownPoint)?,
                90,
            )?,
            signed(
                number(b.get(3..6).ok_or(Item15Reject::UnknownPoint)?)?,
                0,
                *b.get(6).ok_or(Item15Reject::UnknownPoint)?,
                180,
            )?,
        ))),
        11 => Ok(Some((
            signed(
                number(b.get(0..2).ok_or(Item15Reject::UnknownPoint)?)?,
                number(b.get(2..4).ok_or(Item15Reject::UnknownPoint)?)?,
                *b.get(4).ok_or(Item15Reject::UnknownPoint)?,
                90,
            )?,
            signed(
                number(b.get(5..8).ok_or(Item15Reject::UnknownPoint)?)?,
                number(b.get(8..10).ok_or(Item15Reject::UnknownPoint)?)?,
                *b.get(10).ok_or(Item15Reject::UnknownPoint)?,
                180,
            )?,
        ))),
        _ => Ok(None),
    }
}

fn named_candidates(m: &model::Model, name: &str) -> Result<Vec<route::FixIdentity>, Item15Reject> {
    let n = m.points.kind.len();
    if [
        m.points.ident.len(),
        m.points.alt_ident.len(),
        m.points.region.len(),
        m.points.scope.len(),
        m.points.lat.len(),
        m.points.lon.len(),
    ]
    .iter()
    .any(|len| *len != n)
    {
        return Err(Item15Reject::Model);
    }
    let mut out = Vec::new();
    for row in 0..n {
        let ident = route_string(m, *m.points.ident.get(row).ok_or(Item15Reject::Model)?)?;
        let alt = route_string(m, *m.points.alt_ident.get(row).ok_or(Item15Reject::Model)?)?;
        if (ident == name || (!alt.is_empty() && alt == name))
            && *m.points.lat.get(row).ok_or(Item15Reject::Model)? != model::POS_NONE
            && *m.points.lon.get(row).ok_or(Item15Reject::Model)? != model::POS_NONE
        {
            push_identity(&mut out, point_identity(m, row)?)?;
        }
    }
    if out.is_empty() {
        Err(Item15Reject::UnknownPoint)
    } else {
        Ok(out)
    }
}

fn exact_coordinate_candidates(
    m: &model::Model,
    lat: i32,
    lon: i32,
) -> Result<Vec<route::FixIdentity>, Item15Reject> {
    let mut out = Vec::new();
    for row in 0..m.points.kind.len() {
        if m.points.lat.get(row) == Some(&lat) && m.points.lon.get(row) == Some(&lon) {
            push_identity(&mut out, point_identity(m, row)?)?;
        }
    }
    if out.is_empty() {
        Err(Item15Reject::UnknownPoint)
    } else {
        Ok(out)
    }
}

fn bearing_distance_parts(token: &str) -> Result<Option<(&str, u16, u16)>, Item15Reject> {
    let bytes = token.as_bytes();
    if !(8..=11).contains(&bytes.len()) || !bytes.iter().rev().take(6).all(u8::is_ascii_digit) {
        return Ok(None);
    }
    let split = bytes
        .len()
        .checked_sub(6)
        .ok_or(Item15Reject::MalformedGroup)?;
    let reference = token.get(..split).ok_or(Item15Reject::MalformedGroup)?;
    let rest = token.get(split..).ok_or(Item15Reject::MalformedGroup)?;
    if !(2..=5).contains(&reference.len()) || !reference.bytes().all(|b| b.is_ascii_alphanumeric())
    {
        return Err(Item15Reject::MalformedGroup);
    }
    let bearing = rest
        .get(..3)
        .ok_or(Item15Reject::MalformedGroup)?
        .parse::<u16>()
        .map_err(|_| Item15Reject::MalformedGroup)?;
    let distance = rest
        .get(3..6)
        .ok_or(Item15Reject::MalformedGroup)?
        .parse::<u16>()
        .map_err(|_| Item15Reject::MalformedGroup)?;
    if bearing > 360 || !(1..=999).contains(&distance) {
        return Err(Item15Reject::MalformedGroup);
    }
    Ok(Some((reference, bearing, distance)))
}

fn exact_position(m: &model::Model, key: route::FixIdentity) -> Result<(i32, i32), Item15Reject> {
    let mut result = None;
    for row in 0..m.points.kind.len() {
        if identity_eq(point_identity(m, row)?, key) {
            if result.is_some() {
                return Err(Item15Reject::AmbiguousPoint);
            }
            let lat = *m.points.lat.get(row).ok_or(Item15Reject::Model)?;
            let lon = *m.points.lon.get(row).ok_or(Item15Reject::Model)?;
            if lat == model::POS_NONE || lon == model::POS_NONE {
                return Err(Item15Reject::UnknownPoint);
            }
            result = Some((lat, lon));
        }
    }
    result.ok_or(Item15Reject::UnknownPoint)
}

fn resolve_point(
    m: &model::Model,
    token: &str,
    variations: &[MagneticVariationSource],
    evidence: &mut Vec<BearingDistanceEvidence>,
) -> Result<Vec<route::FixIdentity>, Item15Reject> {
    if let Some((lat, lon)) = coordinate_e7(token)? {
        return exact_coordinate_candidates(m, lat, lon);
    }
    if let Some((ref_name, bearing, distance)) = bearing_distance_parts(token)? {
        let references = named_candidates(m, ref_name)?;
        let mut source_qualified = references
            .iter()
            .copied()
            .filter(|key| variations.iter().any(|v| identity_eq(v.reference, *key)))
            .collect::<Vec<_>>();
        if source_qualified.len() == 1 {
            // The variation row carries a full source identity and can disambiguate identical
            // filed spellings without preference rules.
        } else if references.len() == 1 {
            source_qualified = references;
        } else {
            return Err(Item15Reject::AmbiguousPoint);
        }
        let reference = *source_qualified.first().ok_or(Item15Reject::UnknownPoint)?;
        let mut matches = variations
            .iter()
            .filter(|v| identity_eq(v.reference, reference));
        let variation = matches.next().ok_or(Item15Reject::MissingVariation)?;
        if matches.next().is_some()
            || !(-18_000..=18_000).contains(&variation.variation_cdeg)
            || variation.document.is_empty()
            || variation.locator.is_empty()
        {
            return Err(Item15Reject::InvalidVariation);
        }
        let (lat, lon) = exact_position(m, reference)?;
        let destination =
            item15_bearing_distance_e7(lat, lon, bearing, distance, variation.variation_cdeg)
                .map_err(|_| Item15Reject::InvalidVariation)?;
        let candidates = exact_coordinate_candidates(m, destination.0, destination.1)?;
        for target in candidates.iter().copied() {
            evidence.push(BearingDistanceEvidence {
                reference,
                bearing_magnetic_deg: bearing,
                distance_nm: distance,
                variation_cdeg: variation.variation_cdeg,
                epoch: variation.epoch.clone(),
                document: variation.document.clone(),
                locator: variation.locator.clone(),
                target,
            });
        }
        return Ok(candidates);
    }
    named_candidates(m, token)
}

fn route_empty_plan() -> RoutePlan {
    let none = FixIdentity {
        ident: Label {
            bytes: [0; 12],
            len: 0,
        },
        region: Label {
            bytes: [0; 4],
            len: 0,
        },
        scope: Label {
            bytes: [0; 12],
            len: 0,
        },
        class: 0,
    };
    RoutePlan {
        steps: [RouteStep::Fix(none); MAX_ROUTE_STEPS],
        len: 0,
    }
}

fn route_once(
    m: &model::Model,
    step: RouteStep,
) -> Result<crate::core::route::ConvertedRoute, crate::core::route::RouteFailure> {
    let mut plan = route_empty_plan();
    plan.steps[0] = step;
    plan.len = 1;
    let mut out = crate::core::route::empty_converted_route();
    let report = crate::core::route::convert_route(m, &plan, &mut out);
    if report.failure == crate::core::route::RouteFailure::None {
        Ok(out)
    } else {
        Err(report.failure)
    }
}

fn unique_step(out: &mut Vec<RouteStep>, value: RouteStep) -> Result<(), Item15Reject> {
    if out.contains(&value) {
        return Ok(());
    }
    if out.len() >= MAX_ROUTE_STEPS {
        return Err(Item15Reject::Capacity);
    }
    out.push(value);
    Ok(())
}

fn bounded_candidate_work(parts: &[usize]) -> Result<usize, Item15Reject> {
    parts
        .iter()
        .try_fold(0usize, |sum, part| sum.checked_add(*part))
        .filter(|sum| *sum <= route::MAX_ROUTE_WORK as usize)
        .ok_or(Item15Reject::Capacity)
}

fn named_airway(m: &model::Model, ident: &str) -> Result<bool, Item15Reject> {
    for ix in &m.airways.designator {
        if route_string(m, *ix)? == ident {
            return Ok(true);
        }
    }
    Ok(false)
}

fn looks_like_ats_designator(text: &str) -> bool {
    let bytes = text.as_bytes();
    let letters = bytes.iter().take_while(|b| b.is_ascii_uppercase()).count();
    let digits = bytes.len().saturating_sub(letters);
    (1..=2).contains(&letters)
        && (1..=3).contains(&digits)
        && bytes
            .get(letters..)
            .is_some_and(|part| part.iter().all(u8::is_ascii_digit))
}

fn airway_candidate(
    m: &model::Model,
    ident: &str,
    entries: &[FixIdentity],
    exits: &[FixIdentity],
    altitude_ft: i32,
) -> Result<RouteStep, Item15Reject> {
    let mut accepted = Vec::new();
    let mut found = false;
    let mut gap = false;
    let mut low = false;
    let mut ambiguous = false;
    let scan_cost = bounded_candidate_work(&[
        m.points
            .kind
            .len()
            .checked_mul(2)
            .ok_or(Item15Reject::Capacity)?,
        m.airways.designator.len(),
        m.airway_legs.from.len(),
        route::MAX_ROUTE_POINTS,
    ])?;
    let mut work = 0usize;
    if m.airways.location.len() != m.airways.designator.len() {
        return Err(Item15Reject::Model);
    }
    for row in 0..m.airways.designator.len() {
        if route_string(
            m,
            *m.airways.designator.get(row).ok_or(Item15Reject::Model)?,
        )? != ident
        {
            continue;
        }
        found = true;
        let designator = route_label(ident, false)?;
        let location = *m.airways.location.get(row).ok_or(Item15Reject::Model)?;
        for entry in entries {
            for exit in exits {
                work = work.checked_add(scan_cost).ok_or(Item15Reject::Capacity)?;
                if work > route::MAX_ROUTE_WORK as usize {
                    return Err(Item15Reject::Capacity);
                }
                let step = RouteStep::Airway {
                    designator,
                    location,
                    entry: *entry,
                    exit: *exit,
                    planned_altitude_ft: altitude_ft,
                };
                match route_once(m, step) {
                    Ok(_) => unique_step(&mut accepted, step)?,
                    Err(route::RouteFailure::Ambiguous) => ambiguous = true,
                    Err(route::RouteFailure::AirwayGap | route::RouteFailure::BorderCrossing) => {
                        gap = true
                    }
                    Err(route::RouteFailure::MeaNotSatisfied) => low = true,
                    Err(route::RouteFailure::MalformedModel) => return Err(Item15Reject::Model),
                    Err(route::RouteFailure::WorkLimit | route::RouteFailure::PointLimit) => {
                        return Err(Item15Reject::Capacity);
                    }
                    Err(_) => {}
                }
            }
        }
    }
    match accepted.as_slice() {
        [step] => Ok(*step),
        [] if ambiguous => Err(Item15Reject::AmbiguousPoint),
        [] if gap => Err(Item15Reject::Discontinuity),
        [] if low => Err(Item15Reject::AltitudeBelowMea),
        [] if found => Err(Item15Reject::AirwayDoesNotContainPoint),
        [] => Err(Item15Reject::UnknownAirway),
        _ => Err(Item15Reject::AmbiguousPoint),
    }
}

fn airport_candidates(
    m: &model::Model,
    ident: &str,
) -> Result<Vec<(model::PointIx, Label<12>, Label<4>)>, Item15Reject> {
    let mut out = Vec::new();
    for row in 0..m.points.kind.len() {
        if *m.points.kind.get(row).ok_or(Item15Reject::Model)? != model::point_kind::AIRPORT
            || route_string(m, *m.points.ident.get(row).ok_or(Item15Reject::Model)?)? != ident
        {
            continue;
        }
        let ix = model::PointIx::new(u32::try_from(row).map_err(|_| Item15Reject::Model)?);
        let name = route_label(
            route_string(m, *m.points.ident.get(row).ok_or(Item15Reject::Model)?)?,
            false,
        )?;
        let region = route_label(
            route_string(m, *m.points.region.get(row).ok_or(Item15Reject::Model)?)?,
            false,
        )?;
        if !out.iter().any(|(old, _, _)| *old == ix) {
            out.push((ix, name, region));
        }
    }
    if out.is_empty() {
        Err(Item15Reject::UnknownPoint)
    } else {
        Ok(out)
    }
}

fn procedure_candidate(
    m: &model::Model,
    airport: &str,
    kind: u8,
    ident: &str,
    transition: Option<&str>,
) -> Result<RouteStep, Item15Reject> {
    let airports = airport_candidates(m, airport)?;
    let mut accepted = Vec::new();
    let mut work = bounded_candidate_work(&[airports
        .len()
        .checked_mul(m.procs.kind.len())
        .ok_or(Item15Reject::Capacity)?])?;
    if m.procs.airport.len() != m.procs.kind.len()
        || m.procs.ident.len() != m.procs.kind.len()
        || m.procs.routes.len() != m.procs.kind.len()
    {
        return Err(Item15Reject::Model);
    }
    for (airport_ix, airport_ident, airport_region) in airports {
        for row in 0..m.procs.kind.len() {
            if *m.procs.kind.get(row).ok_or(Item15Reject::Model)? != kind
                || *m.procs.airport.get(row).ok_or(Item15Reject::Model)? != airport_ix
                || route_string(m, *m.procs.ident.get(row).ok_or(Item15Reject::Model)?)? != ident
                || !model::bit_get(&m.procs.coded, row)
            {
                continue;
            }
            let span = *m.procs.routes.get(row).ok_or(Item15Reject::Model)?;
            let start = usize::try_from(span.start.get()).map_err(|_| Item15Reject::Model)?;
            let end = start
                .checked_add(usize::try_from(span.len).map_err(|_| Item15Reject::Model)?)
                .ok_or(Item15Reject::Model)?;
            if end > m.routes.route_type.len() || end > m.routes.transition.len() {
                return Err(Item15Reject::Model);
            }
            for route_row in start..end {
                let route_type = *m
                    .routes
                    .route_type
                    .get(route_row)
                    .ok_or(Item15Reject::Model)?;
                let transition_name = route_string(
                    m,
                    *m.routes
                        .transition
                        .get(route_row)
                        .ok_or(Item15Reject::Model)?,
                )?;
                if route_type == 0 || transition.is_some_and(|hint| hint != transition_name) {
                    continue;
                }
                let step = RouteStep::Procedure {
                    airport_ident,
                    airport_region,
                    kind,
                    ident: route_label(ident, false)?,
                    route_type,
                    transition: route_label(transition_name, true)?,
                };
                let leg_span = *m.routes.legs.get(route_row).ok_or(Item15Reject::Model)?;
                let leg_count = usize::try_from(leg_span.len).map_err(|_| Item15Reject::Model)?;
                let per_candidate = bounded_candidate_work(&[
                    m.points
                        .kind
                        .len()
                        .checked_mul(leg_count.saturating_add(1))
                        .ok_or(Item15Reject::Capacity)?,
                    m.procs.kind.len(),
                    m.routes.route_type.len(),
                    m.legs.fix.len(),
                    route::MAX_ROUTE_POINTS,
                ])?;
                work = work
                    .checked_add(per_candidate)
                    .ok_or(Item15Reject::Capacity)?;
                if work > route::MAX_ROUTE_WORK as usize {
                    return Err(Item15Reject::Capacity);
                }
                match route_once(m, step) {
                    Ok(_) => unique_step(&mut accepted, step)?,
                    Err(route::RouteFailure::MalformedModel) => return Err(Item15Reject::Model),
                    Err(route::RouteFailure::WorkLimit | route::RouteFailure::PointLimit) => {
                        return Err(Item15Reject::Capacity);
                    }
                    Err(_) => {}
                }
            }
        }
    }
    match accepted.as_slice() {
        [step] => Ok(*step),
        [] => Err(Item15Reject::UnsupportedProcedure),
        _ => Err(Item15Reject::AmbiguousPoint),
    }
}

fn procedure_name_exists(
    m: &model::Model,
    airport: &str,
    kind: u8,
    ident: &str,
) -> Result<bool, Item15Reject> {
    let airports = match airport_candidates(m, airport) {
        Ok(values) => values,
        Err(Item15Reject::UnknownPoint) => return Ok(false),
        Err(other) => return Err(other),
    };
    bounded_candidate_work(&[airports
        .len()
        .checked_mul(m.procs.kind.len())
        .ok_or(Item15Reject::Capacity)?])?;
    for (airport_ix, _, _) in airports {
        for row in 0..m.procs.kind.len() {
            if *m.procs.kind.get(row).ok_or(Item15Reject::Model)? == kind
                && *m.procs.airport.get(row).ok_or(Item15Reject::Model)? == airport_ix
                && route_string(m, *m.procs.ident.get(row).ok_or(Item15Reject::Model)?)? == ident
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Use the filed first/last point as route context for a SID/STAR without inventing a
/// procedure identifier. At departure the filed first point must be the final coded SID point;
/// at destination the filed last point must be the first coded STAR point.
fn procedure_for_edge_point(
    m: &model::Model,
    airport: &str,
    kind: u8,
    filed_candidates: &[FixIdentity],
    match_terminal: bool,
) -> Result<Option<RouteStep>, Item15Reject> {
    let airports = airport_candidates(m, airport)?;
    let mut accepted = Vec::new();
    let mut work = bounded_candidate_work(&[airports
        .len()
        .checked_mul(m.procs.kind.len())
        .ok_or(Item15Reject::Capacity)?])?;
    if m.procs.airport.len() != m.procs.kind.len()
        || m.procs.ident.len() != m.procs.kind.len()
        || m.procs.routes.len() != m.procs.kind.len()
    {
        return Err(Item15Reject::Model);
    }
    for (airport_ix, airport_ident, airport_region) in airports {
        for proc_row in 0..m.procs.kind.len() {
            if *m.procs.kind.get(proc_row).ok_or(Item15Reject::Model)? != kind
                || *m.procs.airport.get(proc_row).ok_or(Item15Reject::Model)? != airport_ix
                || !model::bit_get(&m.procs.coded, proc_row)
            {
                continue;
            }
            let ident = route_string(m, *m.procs.ident.get(proc_row).ok_or(Item15Reject::Model)?)?;
            let span = *m.procs.routes.get(proc_row).ok_or(Item15Reject::Model)?;
            let start = usize::try_from(span.start.get()).map_err(|_| Item15Reject::Model)?;
            let end = start
                .checked_add(usize::try_from(span.len).map_err(|_| Item15Reject::Model)?)
                .ok_or(Item15Reject::Model)?;
            if end > m.routes.route_type.len()
                || end > m.routes.transition.len()
                || end > m.routes.legs.len()
            {
                return Err(Item15Reject::Model);
            }
            for route_row in start..end {
                let route_type = *m
                    .routes
                    .route_type
                    .get(route_row)
                    .ok_or(Item15Reject::Model)?;
                let transition = route_string(
                    m,
                    *m.routes
                        .transition
                        .get(route_row)
                        .ok_or(Item15Reject::Model)?,
                )?;
                if route_type == 0 {
                    continue;
                }
                let leg_span = *m.routes.legs.get(route_row).ok_or(Item15Reject::Model)?;
                let leg_count = usize::try_from(leg_span.len).map_err(|_| Item15Reject::Model)?;
                let per_candidate = bounded_candidate_work(&[
                    m.points
                        .kind
                        .len()
                        .checked_mul(leg_count.saturating_add(1))
                        .ok_or(Item15Reject::Capacity)?,
                    m.procs.kind.len(),
                    m.routes.route_type.len(),
                    m.legs.fix.len(),
                    route::MAX_ROUTE_POINTS,
                ])?;
                work = work
                    .checked_add(per_candidate)
                    .ok_or(Item15Reject::Capacity)?;
                if work > route::MAX_ROUTE_WORK as usize {
                    return Err(Item15Reject::Capacity);
                }
                let step = RouteStep::Procedure {
                    airport_ident,
                    airport_region,
                    kind,
                    ident: route_label(ident, false)?,
                    route_type,
                    transition: route_label(transition, true)?,
                };
                match route_once(m, step) {
                    Ok(converted) => {
                        let edge_row = if match_terminal {
                            usize::from(converted.len).checked_sub(1)
                        } else {
                            Some(0)
                        }
                        .ok_or(Item15Reject::Model)?;
                        let edge = FixIdentity {
                            ident: *converted.ident.get(edge_row).ok_or(Item15Reject::Model)?,
                            region: *converted.region.get(edge_row).ok_or(Item15Reject::Model)?,
                            scope: *converted.scope.get(edge_row).ok_or(Item15Reject::Model)?,
                            class: *converted.class.get(edge_row).ok_or(Item15Reject::Model)?,
                        };
                        if filed_candidates
                            .iter()
                            .any(|filed| identity_eq(*filed, edge))
                        {
                            unique_step(&mut accepted, step)?;
                        }
                    }
                    Err(route::RouteFailure::MalformedModel) => return Err(Item15Reject::Model),
                    Err(route::RouteFailure::WorkLimit | route::RouteFailure::PointLimit) => {
                        return Err(Item15Reject::Capacity);
                    }
                    Err(_) => {}
                }
            }
        }
    }
    match accepted.as_slice() {
        [step] => Ok(Some(*step)),
        [] => Ok(None),
        _ => Err(Item15Reject::AmbiguousPoint),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FiledAtom<'a> {
    Point {
        name: &'a str,
        level_ft: Option<i32>,
    },
    Dct,
    Rules,
    Truncate,
}

fn filed_level_change(group: &str) -> Result<Option<i32>, Item15Reject> {
    let bytes = group.as_bytes();
    let mut at = 0usize;
    let metric_m = bytes.first() == Some(&b'M') && bytes.get(4).is_some_and(u8::is_ascii_digit);
    if matches!(bytes.first(), Some(b'N' | b'K' | b'M')) && !metric_m {
        let width = if bytes.first() == Some(&b'M') { 4 } else { 5 };
        let speed = group.get(..width).ok_or(Item15Reject::MalformedGroup)?;
        if !(speed.starts_with('N') && speed.len() == 5
            || speed.starts_with('K') && speed.len() == 5
            || speed.starts_with('M') && speed.len() == 4)
            || !speed
                .get(1..)
                .is_some_and(|v| v.bytes().all(|b| b.is_ascii_digit()))
        {
            return Err(Item15Reject::MalformedGroup);
        }
        at = width;
    }
    let mut min = None;
    while at < bytes.len() {
        if group.get(at..) == Some("PLUS") {
            break;
        }
        let prefix = *bytes.get(at).ok_or(Item15Reject::MalformedGroup)?;
        let width = if matches!(prefix, b'S' | b'M') { 5 } else { 4 };
        let part = group
            .get(at..at.checked_add(width).ok_or(Item15Reject::MalformedGroup)?)
            .ok_or(Item15Reject::MalformedGroup)?;
        if !part
            .get(1..)
            .is_some_and(|v| v.bytes().all(|b| b.is_ascii_digit()))
        {
            return Err(Item15Reject::MalformedGroup);
        }
        let number = part
            .get(1..)
            .ok_or(Item15Reject::MalformedGroup)?
            .parse::<i32>()
            .map_err(|_| Item15Reject::MalformedGroup)?;
        let feet = match prefix {
            b'A' if width == 4 && number < 180 => number
                .checked_mul(100)
                .ok_or(Item15Reject::MalformedGroup)?,
            b'F' if width == 4 && number >= 180 => number
                .checked_mul(100)
                .ok_or(Item15Reject::MalformedGroup)?,
            b'S' | b'M' if width == 5 => {
                number
                    .checked_mul(3281)
                    .ok_or(Item15Reject::MalformedGroup)?
                    / 100
            }
            _ => return Err(Item15Reject::MalformedGroup),
        };
        min = Some(min.map_or(feet, |old: i32| old.min(feet)));
        at = at.checked_add(width).ok_or(Item15Reject::MalformedGroup)?;
    }
    if at == bytes.len() || group.get(at..) == Some("PLUS") {
        return Ok(min);
    }
    Err(Item15Reject::MalformedGroup)
}

fn filed_atom(token: &str) -> Result<FiledAtom<'_>, Item15Reject> {
    if token == "DCT" {
        return Ok(FiledAtom::Dct);
    }
    if matches!(token, "IFR" | "VFR") {
        return Ok(FiledAtom::Rules);
    }
    if token == "T" {
        return Ok(FiledAtom::Truncate);
    }
    if let Some(tail) = token.strip_prefix("C/") {
        let (name, group) = tail.split_once('/').ok_or(Item15Reject::MalformedGroup)?;
        if name.is_empty() {
            return Err(Item15Reject::MalformedGroup);
        }
        return Ok(FiledAtom::Point {
            name,
            level_ft: filed_level_change(group)?,
        });
    }
    if let Some((name, group)) = token.split_once('/') {
        if name.is_empty() || group.is_empty() {
            return Err(Item15Reject::MalformedGroup);
        }
        return Ok(FiledAtom::Point {
            name,
            level_ft: filed_level_change(group)?,
        });
    }
    if token.is_empty() || token.len() > 32 {
        return Err(Item15Reject::MalformedGroup);
    }
    Ok(FiledAtom::Point {
        name: token,
        level_ft: None,
    })
}

/// Resolve already parsed Item 15 route elements against the current NASR/CIFP model. No text is
/// repaired, no point identity is guessed, and candidates are retained until route topology or
/// airport/procedure context resolves them uniquely. Magnetic variations are explicit inputs.
pub fn resolve_icao_item15(
    m: &model::Model,
    elements: &[String],
    level_ft: Option<i32>,
    departure: &str,
    destination: &str,
    variations: &[MagneticVariationSource],
) -> Result<ResolvedItem15, Item15Reject> {
    if elements.is_empty() || elements.len() > MAX_ROUTE_STEPS {
        return Err(Item15Reject::Capacity);
    }
    let total_text_bytes = elements
        .iter()
        .try_fold(0usize, |total, element| total.checked_add(element.len()))
        .ok_or(Item15Reject::Capacity)?;
    if total_text_bytes > MAX_ROUTE_TEXT_BYTES
        || elements
            .iter()
            .any(|element| element.len() > MAX_TOKEN_BYTES)
    {
        return Err(Item15Reject::Capacity);
    }
    bounded_candidate_work(&[
        m.points
            .kind
            .len()
            .checked_mul(elements.len())
            .ok_or(Item15Reject::Capacity)?,
        m.airways.designator.len(),
        m.airway_legs.from.len(),
        m.procs.kind.len(),
        m.routes.route_type.len(),
        m.legs.fix.len(),
    ])?;
    let atoms = elements
        .iter()
        .map(|token| filed_atom(token))
        .collect::<Result<Vec<_>, _>>()?;
    let mut steps = Vec::new();
    let mut evidence = Vec::new();
    let mut level_changes = Vec::new();
    let mut previous = None;
    let mut current_altitude = level_ft;
    let mut i = 0usize;
    while i < atoms.len() {
        match *atoms.get(i).ok_or(Item15Reject::MalformedGroup)? {
            FiledAtom::Rules | FiledAtom::Dct => i += 1,
            FiledAtom::Truncate => break,
            FiledAtom::Point {
                name,
                level_ft: changed,
            } => {
                if let Some(value) = changed {
                    current_altitude = Some(value);
                    level_changes.push(FiledLevelChange {
                        element_index: u8::try_from(i).map_err(|_| Item15Reject::Capacity)?,
                        point_token: name.to_owned(),
                        altitude_ft: value,
                    });
                }
                let significant_start = atoms.iter().position(|a| !matches!(a, FiledAtom::Rules));
                let significant_end = atoms
                    .iter()
                    .rposition(|a| !matches!(a, FiledAtom::Rules | FiledAtom::Truncate));
                if significant_start == Some(i) || significant_end == Some(i) {
                    let at_departure = significant_start == Some(i);
                    let (apt, kind) = if at_departure {
                        (departure, model::proc_kind::SID)
                    } else {
                        (destination, model::proc_kind::STAR)
                    };
                    let mut edge_evidence = Vec::new();
                    if let Ok(candidates) = resolve_point(m, name, variations, &mut edge_evidence) {
                        match procedure_for_edge_point(m, apt, kind, &candidates, at_departure) {
                            Ok(Some(step)) => {
                                steps.push(step);
                                evidence.extend(edge_evidence);
                                let converted =
                                    route_once(m, step).map_err(|_| Item15Reject::Model)?;
                                let last = usize::from(converted.len)
                                    .checked_sub(1)
                                    .ok_or(Item15Reject::Model)?;
                                previous = Some(FixIdentity {
                                    ident: *converted.ident.get(last).ok_or(Item15Reject::Model)?,
                                    region: *converted
                                        .region
                                        .get(last)
                                        .ok_or(Item15Reject::Model)?,
                                    scope: *converted.scope.get(last).ok_or(Item15Reject::Model)?,
                                    class: *converted.class.get(last).ok_or(Item15Reject::Model)?,
                                });
                                i += 1;
                                continue;
                            }
                            Ok(None) | Err(Item15Reject::UnknownPoint) => {}
                            Err(other) => return Err(other),
                        }
                    }
                    let hint = atoms.get(i + 1).and_then(|a| match a {
                        FiledAtom::Point { name, .. } => Some(*name),
                        _ => None,
                    });
                    let proc = procedure_candidate(m, apt, kind, name, hint);
                    match proc {
                        Ok(step) => {
                            steps.push(step);
                            let converted =
                                route_once(m, step).map_err(|failure| match failure {
                                    route::RouteFailure::MalformedModel => Item15Reject::Model,
                                    route::RouteFailure::WorkLimit
                                    | route::RouteFailure::PointLimit => Item15Reject::Capacity,
                                    _ => Item15Reject::UnsupportedProcedure,
                                })?;
                            let last = usize::from(converted.len)
                                .checked_sub(1)
                                .ok_or(Item15Reject::Model)?;
                            previous = Some(FixIdentity {
                                ident: *converted.ident.get(last).ok_or(Item15Reject::Model)?,
                                region: *converted.region.get(last).ok_or(Item15Reject::Model)?,
                                scope: *converted.scope.get(last).ok_or(Item15Reject::Model)?,
                                class: *converted.class.get(last).ok_or(Item15Reject::Model)?,
                            });
                            i += 1;
                            continue;
                        }
                        Err(Item15Reject::UnsupportedProcedure)
                            if procedure_name_exists(m, apt, kind, name)? =>
                        {
                            return Err(Item15Reject::UnsupportedProcedure);
                        }
                        Err(
                            other @ (Item15Reject::AmbiguousPoint
                            | Item15Reject::Model
                            | Item15Reject::Capacity),
                        ) => return Err(other),
                        Err(_) => {}
                    }
                }
                if let (
                    Some(FiledAtom::Point {
                        name: airway_name, ..
                    }),
                    Some(FiledAtom::Point {
                        name: exit_name,
                        level_ft: exit_level,
                    }),
                ) = (atoms.get(i + 1), atoms.get(i + 2))
                {
                    if named_airway(m, airway_name)? {
                        let entries = resolve_point(m, name, variations, &mut evidence)?;
                        let exits = resolve_point(m, exit_name, variations, &mut evidence)?;
                        let mut contextual_entries = entries;
                        if let Some(last) = previous {
                            if contextual_entries.iter().any(|key| identity_eq(*key, last)) {
                                contextual_entries.retain(|key| identity_eq(*key, last));
                            }
                        }
                        let altitude = current_altitude.ok_or(Item15Reject::MissingRouteLevel)?;
                        let airway = airway_candidate(
                            m,
                            airway_name,
                            &contextual_entries,
                            &exits,
                            altitude,
                        )?;
                        let entry = match airway {
                            RouteStep::Airway { entry, .. } => entry,
                            _ => return Err(Item15Reject::Model),
                        };
                        if previous.is_some_and(|old| !identity_eq(old, entry)) {
                            steps.push(RouteStep::Direct(entry));
                        }
                        steps.push(airway);
                        let converted =
                            route_once(m, airway).map_err(|_| Item15Reject::Discontinuity)?;
                        let last = usize::from(converted.len)
                            .checked_sub(1)
                            .ok_or(Item15Reject::Model)?;
                        previous = Some(FixIdentity {
                            ident: *converted.ident.get(last).ok_or(Item15Reject::Model)?,
                            region: *converted.region.get(last).ok_or(Item15Reject::Model)?,
                            scope: *converted.scope.get(last).ok_or(Item15Reject::Model)?,
                            class: *converted.class.get(last).ok_or(Item15Reject::Model)?,
                        });
                        if let Some(value) = exit_level {
                            current_altitude = Some(*value);
                        }
                        i += 3;
                        continue;
                    }
                    if looks_like_ats_designator(airway_name) {
                        match named_candidates(m, airway_name) {
                            Err(Item15Reject::UnknownPoint) => {
                                return Err(Item15Reject::UnknownAirway);
                            }
                            Err(other) => return Err(other),
                            Ok(_) => {}
                        }
                    }
                }
                let candidates = match resolve_point(m, name, variations, &mut evidence) {
                    Ok(values) => values,
                    Err(Item15Reject::UnknownPoint) if named_airway(m, name)? => {
                        return Err(Item15Reject::UnknownAirway);
                    }
                    Err(error) => return Err(error),
                };
                let key = if let Some(last) =
                    previous.filter(|last| candidates.iter().any(|key| identity_eq(*key, *last)))
                {
                    last
                } else if candidates.len() == 1 {
                    *candidates.first().ok_or(Item15Reject::UnknownPoint)?
                } else {
                    return Err(Item15Reject::AmbiguousPoint);
                };
                steps.push(if steps.is_empty() {
                    RouteStep::Fix(key)
                } else {
                    RouteStep::Direct(key)
                });
                previous = Some(key);
                i += 1;
            }
        }
    }
    if steps.is_empty() || steps.len() > MAX_ROUTE_STEPS {
        return Err(Item15Reject::Capacity);
    }
    let mut plan = route_empty_plan();
    for (index, step) in steps.iter().enumerate() {
        *plan.steps.get_mut(index).ok_or(Item15Reject::Capacity)? = *step;
    }
    plan.len = u8::try_from(steps.len()).map_err(|_| Item15Reject::Capacity)?;
    let mut converted = route::empty_converted_route();
    let report = route::convert_route(m, &plan, &mut converted);
    match report.failure {
        route::RouteFailure::None => Ok(ResolvedItem15 {
            route: converted,
            initial_level_ft: level_ft,
            level_changes,
            bearing_distance: evidence,
        }),
        route::RouteFailure::Ambiguous | route::RouteFailure::ProcedureConflict => {
            Err(Item15Reject::AmbiguousPoint)
        }
        route::RouteFailure::NotFound => Err(Item15Reject::UnknownPoint),
        route::RouteFailure::AirwayGap | route::RouteFailure::BorderCrossing => {
            Err(Item15Reject::Discontinuity)
        }
        route::RouteFailure::MalformedModel => Err(Item15Reject::Model),
        route::RouteFailure::WorkLimit | route::RouteFailure::PointLimit => {
            Err(Item15Reject::Capacity)
        }
        route::RouteFailure::MeaNotSatisfied => Err(Item15Reject::AltitudeBelowMea),
        _ => Err(Item15Reject::Discontinuity),
    }
}

/// Convert a filed magnetic bearing and distance using a caller-supplied source variation.
/// East-positive variation is added to magnetic bearing to obtain true azimuth. This routine
/// never substitutes a default variation and rounds only the final coordinate to E7.
fn item15_bearing_distance_e7(
    lat_e7: i32,
    lon_e7: i32,
    bearing_magnetic_deg: u16,
    distance_nm: u16,
    variation_cdeg_east: i16,
) -> Result<(i32, i32), ()> {
    if !(-90 * 10_000_000..=90 * 10_000_000).contains(&lat_e7)
        || !(-180 * 10_000_000..=180 * 10_000_000).contains(&lon_e7)
        || bearing_magnetic_deg > 360
        || !(1..=999).contains(&distance_nm)
        || !(-18_000..=18_000).contains(&variation_cdeg_east)
    {
        return Err(());
    }
    let lat = f64::from(lat_e7) / 10_000_000.0;
    let lon = f64::from(lon_e7) / 10_000_000.0;
    let origin = oa_geo::LatLon::new(lat, lon).map_err(|_| ())?;
    let azimuth = f64::from(bearing_magnetic_deg) + f64::from(variation_cdeg_east) / 100.0;
    let distance_m = f64::from(distance_nm) * 1852.0;
    let result = oa_geo::wgs84()
        .direct(origin, azimuth, distance_m)
        .map_err(|_| ())?;
    let lat = crate::core::geom::deg_to_e7(result.position.lat()).ok_or(())?;
    let lon = crate::core::geom::deg_to_e7(result.position.lon()).ok_or(())?;
    Ok((lat, lon))
}
