//! Bounded encoder for the retained FAA FNS AIXM 5.1 event and feature subset
//! profile. This is a serialization helper for the parsed row model, not a general AIXM writer.

use core::fmt::Write as _;

use crate::{
    code_str, has_field, polygons_of, schedule_of, text_of, Field, Notams, ALT_UNLIMITED,
    FEATURE_META_CORRECTION, FEATURE_META_SEQUENCE, FEATURE_TIME_BEGIN_PRESENT,
    FEATURE_TIME_END_PRESENT, FEATURE_TIME_INSTANT, FEATURE_TIME_INSTANT_PRESENT,
    FEATURE_TIME_PERIOD, FEAT_AIRPORT, FEAT_NAVAID, FEAT_OBSTACLE_AREA, FEAT_PRIMARY_RADAR,
    FEAT_RADAR_SYSTEM, FEAT_RUNWAY, FEAT_RUNWAY_DIRECTION, FEAT_RUNWAY_ELEMENT, FEAT_TAXIWAY,
    FEAT_TAXIWAY_ELEMENT, FEAT_UNIT, FEAT_VERTICAL_STRUCTURE, FLAG_END_UNKNOWN, FLAG_ESTIMATED,
    FLAG_PERMANENT, INTERPRETATION_BASELINE, INTERPRETATION_PERMDELTA, INTERPRETATION_SNAPSHOT,
    INTERPRETATION_TEMPDELTA, PURPOSE_B, PURPOSE_K, PURPOSE_M, PURPOSE_N, PURPOSE_O, REF_AGL,
    REF_FL, REF_MSL, SCOPE_A, SCOPE_E, SCOPE_K, SCOPE_W, SLICE_CORRECTION_PRESENT,
    SLICE_SEQUENCE_PRESENT, TRAFFIC_I, TRAFFIC_K, TRAFFIC_V, TYPE_CANCEL, TYPE_NEW, TYPE_REPLACE,
};

/// Maximum canonical XML output, including the XML declaration.
pub const MAX_ENCODED_BYTES: usize = 32 * 1024;

/// Why a row cannot be faithfully represented by this encoder profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncodeError {
    /// The row index is outside the source table.
    RowOutOfRange,
    /// The row contains parser errors or malformed caller-owned table spans.
    InvalidRow,
    /// A requested feature object or geometry cannot be reconstructed from retained row data.
    FeatureDataNotRetained,
    /// The row has unsupported flags or enumeration values.
    UnsupportedValue,
    /// A timestamp cannot be represented by the supported UTC timestamp grammar.
    InvalidTime,
    /// The required AIXM event interpretation was absent or not retained by the row.
    MissingInterpretation,
    /// An XML string contains invalid UTF-8 or XML 1.0-forbidden characters.
    InvalidText,
    /// The canonical message exceeds the explicit output bound.
    OutputLimit,
    /// The caller's output buffer is too small; it is left unchanged.
    OutputTooSmall,
}

/// Encodes one valid row into canonical FAA FNS AIXM 5.1 event XML.
///
/// Output is all-or-nothing: every refusal leaves `out` untouched. The profile emits the event,
/// retained feature time slices, row-supported feature properties, and retained polygon geometry.
/// Optional source properties absent from the row model are omitted; source XML byte identity is
/// not promised.
pub fn encode_row(n: &Notams, row: usize, out: &mut [u8]) -> Result<usize, EncodeError> {
    let r = crate::row_at(n, row).ok_or(EncodeError::RowOutOfRange)?;
    let present_mask = (1u64 << crate::FIELD_COUNT) - 1;
    if r.errors != 0 || r.present & !present_mask != 0 {
        return Err(EncodeError::InvalidRow);
    }
    if r.slice_errors != 0
        || r.slice_meta & !(SLICE_SEQUENCE_PRESENT | SLICE_CORRECTION_PRESENT) != 0
    {
        return Err(EncodeError::InvalidRow);
    }
    let interpretation =
        interpretation_text(r.interpretation).ok_or(EncodeError::MissingInterpretation)?;
    let feature_range = validate_features(n, row, &r)?;
    let has_end = has_field(n, row, Field::End);
    let unknown_end = r.flags & FLAG_END_UNKNOWN != 0;
    if r.flags & !(FLAG_PERMANENT | FLAG_ESTIMATED | FLAG_END_UNKNOWN) != 0
        || (has_field(n, row, Field::Start) && !(0..=253_402_300_799_999).contains(&r.start_ms))
        || (has_end && !(0..=253_402_300_799_999).contains(&r.end_ms))
        || (has_end && unknown_end)
        || (unknown_end && r.end_ms != 0)
        || (unknown_end && r.flags & FLAG_ESTIMATED != 0)
        || (r.flags & FLAG_PERMANENT != 0 && r.flags & FLAG_ESTIMATED != 0)
        || !valid_enum_fields(n, row)
    {
        return Err(EncodeError::UnsupportedValue);
    }
    if !has_field(n, row, Field::Start) || (!has_end && !unknown_end) {
        return Err(EncodeError::InvalidTime);
    }
    if r.flags & FLAG_ESTIMATED != 0 && r.end_ms % 60_000 != 0 {
        return Err(EncodeError::InvalidTime);
    }
    if !row_values_valid(n, row, &r) {
        return Err(EncodeError::InvalidRow);
    }

    let mut message = String::new();
    message.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
    message.push_str("<message:AIXMBasicMessage xmlns:message=\"http://www.aixm.aero/schema/5.1/message\" xmlns:gml=\"http://www.opengis.net/gml/3.2\" xmlns:aixm=\"http://www.aixm.aero/schema/5.1\" xmlns:event=\"http://www.aixm.aero/schema/5.1/event\" xmlns:fnse=\"http://www.aixm.aero/schema/5.1/extensions/FAA/FNSE\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" gml:id=\"oa-message\">");
    message.push_str("<message:hasMember><event:Event gml:id=\"oa-event\"><gml:identifier codeSpace=\"urn:uuid:\">");
    push_uuid(&mut message, &r.event_id);
    message.push_str("</gml:identifier><gml:boundedBy xsi:nil=\"true\"/><event:timeSlice><event:EventTimeSlice gml:id=\"oa-event-time-slice\"><gml:validTime><gml:TimePeriod gml:id=\"oa-valid-time\"><gml:beginPosition>");
    push_iso(&mut message, r.start_ms)?;
    message.push_str("</gml:beginPosition>");
    if has_end {
        message.push_str("<gml:endPosition>");
        push_iso(&mut message, r.end_ms)?;
        message.push_str("</gml:endPosition>");
    } else {
        message.push_str("<gml:endPosition indeterminatePosition=\"unknown\"></gml:endPosition>");
    }
    message.push_str("</gml:TimePeriod></gml:validTime><aixm:interpretation>");
    message.push_str(interpretation);
    message.push_str("</aixm:interpretation>");
    if r.slice_meta & SLICE_SEQUENCE_PRESENT != 0 {
        push_aixm_num(&mut message, "sequenceNumber", r.sequence_number);
    }
    if r.slice_meta & SLICE_CORRECTION_PRESENT != 0 {
        push_aixm_num(&mut message, "correctionNumber", r.correction_number);
    }
    if has_field(n, row, Field::Scenario) {
        let _ = write!(message, "<event:scenario>{}</event:scenario>", r.scenario);
    }
    message.push_str("<event:textNOTAM><event:NOTAM gml:id=\"oa-notam-record\">");
    push_code(
        &mut message,
        "series",
        code_str(&r.series),
        has_field(n, row, Field::Series),
    )?;
    push_num(
        &mut message,
        "number",
        r.number,
        has_field(n, row, Field::Number),
    );
    push_num(&mut message, "year", r.year, has_field(n, row, Field::Year));
    if has_field(n, row, Field::Type) {
        let value = match r.ntype {
            TYPE_NEW => "N",
            TYPE_REPLACE => "R",
            TYPE_CANCEL => "C",
            _ => return Err(EncodeError::UnsupportedValue),
        };
        push_text(&mut message, "type", value)?;
    }
    push_code(
        &mut message,
        "referredSeries",
        code_str(&r.ref_series),
        has_field(n, row, Field::Ref) && r.ref_series.first().copied().unwrap_or(0) != 0,
    )?;
    if has_field(n, row, Field::Ref) {
        push_num(&mut message, "referredNumber", r.ref_number, true);
        push_num(&mut message, "referredYear", r.ref_year, true);
    }
    if has_field(n, row, Field::Issued) {
        message.push_str("<event:issued>");
        push_iso(&mut message, r.issued_ms)?;
        message.push_str("</event:issued>");
    }
    push_code(
        &mut message,
        "affectedFIR",
        code_str(&r.fir),
        has_field(n, row, Field::Fir),
    )?;
    push_code(
        &mut message,
        "selectionCode",
        code_str(&r.selection),
        has_field(n, row, Field::Selection),
    )?;
    if has_field(n, row, Field::Floor) {
        push_limit(&mut message, "lowerLimit", r.floor_ft, r.floor_ref)?;
    }
    if has_field(n, row, Field::Ceiling) {
        push_limit(&mut message, "upperLimit", r.ceil_ft, r.ceil_ref)?;
    }
    if has_field(n, row, Field::Centre) {
        push_coordinates(&mut message, r.lat_e7, r.lon_e7)?;
    }
    if has_field(n, row, Field::Radius) {
        if r.radius_m % 1852 != 0 {
            return Err(EncodeError::UnsupportedValue);
        }
        let nm = r.radius_m / 1852;
        if !(1..=999).contains(&nm) {
            return Err(EncodeError::UnsupportedValue);
        }
        push_num(&mut message, "radius", nm, true);
    }
    push_code(
        &mut message,
        "location",
        code_str(&r.location),
        has_field(n, row, Field::Location),
    )?;
    if r.flags & FLAG_PERMANENT != 0 {
        push_text(&mut message, "effectiveEnd", "PERM")?;
    } else if has_end && (r.end_ms % 60_000 == 0 || r.flags & FLAG_ESTIMATED != 0) {
        message.push_str("<event:effectiveEnd>");
        push_compact_raw(&mut message, r.end_ms)?;
        if r.flags & FLAG_ESTIMATED != 0 {
            message.push_str("EST");
        }
        message.push_str("</event:effectiveEnd>");
    }
    if has_field(n, row, Field::Schedule) {
        push_text(&mut message, "schedule", schedule_of(n, row))?;
    }
    if has_field(n, row, Field::Text) {
        push_text(&mut message, "text", text_of(n, row))?;
    }
    if has_field(n, row, Field::Traffic) {
        push_flags(
            &mut message,
            "traffic",
            r.traffic,
            &[(TRAFFIC_I, "I"), (TRAFFIC_V, "V"), (TRAFFIC_K, "K")],
        )?;
    }
    if has_field(n, row, Field::Purpose) {
        push_flags(
            &mut message,
            "purpose",
            r.purpose,
            &[
                (PURPOSE_N, "N"),
                (PURPOSE_B, "B"),
                (PURPOSE_O, "O"),
                (PURPOSE_M, "M"),
                (PURPOSE_K, "K"),
            ],
        )?;
    }
    if has_field(n, row, Field::Scope) {
        push_flags(
            &mut message,
            "scope",
            r.scope,
            &[
                (SCOPE_A, "A"),
                (SCOPE_E, "E"),
                (SCOPE_W, "W"),
                (SCOPE_K, "K"),
            ],
        )?;
    }
    message.push_str("</event:NOTAM></event:textNOTAM>");
    let has_ext = has_field(n, row, Field::Canceled)
        || has_field(n, row, Field::Classification)
        || has_field(n, row, Field::Xover)
        || has_field(n, row, Field::IcaoLocation);
    if has_ext {
        message.push_str("<event:extension><fnse:EventExtension gml:id=\"oa-event-extension\">");
        if has_field(n, row, Field::Canceled) {
            message.push_str("<fnse:canceled>");
            push_iso(&mut message, r.canceled_ms)?;
            message.push_str("</fnse:canceled>");
        }
        if has_field(n, row, Field::Classification) {
            push_fnse_text(&mut message, "classification", class_text(r.class)?)?;
        }
        if has_field(n, row, Field::Xover) {
            push_fnse_text(&mut message, "xovernotamID", code_str(&r.xover))?;
        }
        if has_field(n, row, Field::IcaoLocation) {
            push_fnse_text(&mut message, "icaoLocation", code_str(&r.icao_location))?;
        }
        message.push_str("</fnse:EventExtension></event:extension>");
    }
    message.push_str("</event:EventTimeSlice></event:timeSlice></event:Event></message:hasMember>");
    write_features(n, row, feature_range, &mut message)?;
    message.push_str("</message:AIXMBasicMessage>");
    if message.len() > MAX_ENCODED_BYTES {
        return Err(EncodeError::OutputLimit);
    }
    if out.len() < message.len() {
        return Err(EncodeError::OutputTooSmall);
    }
    let Some(dst) = out.get_mut(..message.len()) else {
        return Err(EncodeError::OutputTooSmall);
    };
    dst.copy_from_slice(message.as_bytes());
    Ok(message.len())
}

fn interpretation_text(code: u8) -> Option<&'static str> {
    match code {
        INTERPRETATION_BASELINE => Some("BASELINE"),
        INTERPRETATION_SNAPSHOT => Some("SNAPSHOT"),
        INTERPRETATION_TEMPDELTA => Some("TEMPDELTA"),
        INTERPRETATION_PERMDELTA => Some("PERMDELTA"),
        _ => None,
    }
}

fn valid_enum_fields(n: &Notams, i: usize) -> bool {
    let r = crate::row_at(n, i);
    let Some(r) = r else {
        return false;
    };
    (!has_field(n, i, Field::Classification) || matches!(r.class, 1..=3))
        && (!has_field(n, i, Field::Traffic)
            || r.traffic & !(TRAFFIC_I | TRAFFIC_V | TRAFFIC_K) == 0)
        && (!has_field(n, i, Field::Purpose)
            || r.purpose & !(PURPOSE_N | PURPOSE_B | PURPOSE_O | PURPOSE_M | PURPOSE_K) == 0)
        && (!has_field(n, i, Field::Scope)
            || r.scope & !(SCOPE_A | SCOPE_E | SCOPE_W | SCOPE_K) == 0)
        && (!has_field(n, i, Field::Floor) || valid_ref(r.floor_ref))
        && (!has_field(n, i, Field::Ceiling) || valid_ref(r.ceil_ref))
}

fn validate_features(
    n: &Notams,
    row: usize,
    r: &crate::Row,
) -> Result<core::ops::Range<usize>, EncodeError> {
    let first = usize::try_from(r.feature_first).map_err(|_| EncodeError::InvalidRow)?;
    let count = usize::try_from(r.feature_count).map_err(|_| EncodeError::InvalidRow)?;
    let end = first.checked_add(count).ok_or(EncodeError::InvalidRow)?;
    if end > n.feature_kind.len() {
        return Err(EncodeError::InvalidRow);
    }
    let mut mask = 0u32;
    for ix in first..end {
        let f = crate::feature_at(n, ix).ok_or(EncodeError::InvalidRow)?;
        if f.errors != 0 || f.reserved != [0; 2] || interpretation_text(f.interpretation).is_none()
        {
            return Err(EncodeError::InvalidRow);
        }
        if !matches!(f.kind, FEAT_AIRPORT..=FEAT_PRIMARY_RADAR)
            || f.meta & !(FEATURE_META_SEQUENCE | FEATURE_META_CORRECTION) != 0
        {
            return Err(EncodeError::UnsupportedValue);
        }
        let designator_required = matches!(
            f.kind,
            FEAT_AIRPORT | FEAT_RUNWAY | FEAT_RUNWAY_DIRECTION | FEAT_NAVAID | FEAT_UNIT
        );
        if (designator_required && f.designator_len == 0)
            || usize::from(f.designator_len) > f.designator.len()
            || (f.icao_len != 0 && (f.kind != FEAT_AIRPORT || f.icao_len != 4))
        {
            return Err(EncodeError::FeatureDataNotRetained);
        }
        let time_ok = match f.time_kind {
            FEATURE_TIME_INSTANT => {
                f.time_meta == FEATURE_TIME_INSTANT_PRESENT
                    && (0..=253_402_300_799_999).contains(&f.begin_ms)
            }
            FEATURE_TIME_PERIOD => {
                f.time_meta == (FEATURE_TIME_BEGIN_PRESENT | FEATURE_TIME_END_PRESENT)
                    && (0..=253_402_300_799_999).contains(&f.begin_ms)
                    && (0..=253_402_300_799_999).contains(&f.end_ms)
            }
            _ => false,
        };
        if !time_ok {
            return Err(EncodeError::InvalidTime);
        }
        mask |= 1u32 << f.kind;
    }
    if mask != r.feat_mask || (count == 0 && r.feat_mask != 0) {
        return Err(EncodeError::FeatureDataNotRetained);
    }
    let has_feature_field = [
        Field::Airport,
        Field::AirportIcao,
        Field::Runway,
        Field::Navaid,
        Field::Unit,
        Field::Polygons,
    ]
    .into_iter()
    .any(|f| has_field(n, row, f));
    if count == 0 && has_feature_field {
        return Err(EncodeError::FeatureDataNotRetained);
    }
    let polygons = polygons_of(n, row);
    if !polygons.is_empty() && !has_field(n, row, Field::Polygons) {
        return Err(EncodeError::InvalidRow);
    }
    for p in polygons.clone() {
        let kind = *n.poly_kind.get(p).ok_or(EncodeError::InvalidRow)?;
        if !matches!(kind, FEAT_RUNWAY_ELEMENT | FEAT_TAXIWAY_ELEMENT)
            || !n
                .poly_feature
                .get(p)
                .is_some_and(|ix| (*ix as usize) >= first && (*ix as usize) < end)
            || n.poly_feature
                .get(p)
                .and_then(|ix| n.feature_kind.get(*ix as usize))
                != Some(&kind)
        {
            return Err(EncodeError::FeatureDataNotRetained);
        }
        let (lat, lon) = crate::polygon_vertices(n, p);
        if lat.len() < 3 || lat.len() != lon.len() {
            return Err(EncodeError::InvalidRow);
        }
    }
    Ok(first..end)
}

fn write_features(
    n: &Notams,
    row: usize,
    feature_range: core::ops::Range<usize>,
    out: &mut String,
) -> Result<(), EncodeError> {
    for index in feature_range {
        let feature = crate::feature_at(n, index).ok_or(EncodeError::InvalidRow)?;
        let name = feature_name(feature.kind).ok_or(EncodeError::UnsupportedValue)?;
        let _ = write!(
            out,
            "<message:hasMember><aixm:{name} gml:id=\"oa-feature-{index}\"><aixm:timeSlice><aixm:{name}TimeSlice gml:id=\"oa-feature-slice-{index}\"><gml:validTime>"
        );
        match feature.time_kind {
            FEATURE_TIME_INSTANT => {
                let _ = write!(
                    out,
                    "<gml:TimeInstant gml:id=\"oa-feature-valid-{index}\"><gml:timePosition>"
                );
                push_iso(out, feature.begin_ms)?;
                out.push_str("</gml:timePosition></gml:TimeInstant>");
            }
            FEATURE_TIME_PERIOD => {
                let _ = write!(
                    out,
                    "<gml:TimePeriod gml:id=\"oa-feature-valid-{index}\"><gml:beginPosition>"
                );
                push_iso(out, feature.begin_ms)?;
                out.push_str("</gml:beginPosition><gml:endPosition>");
                push_iso(out, feature.end_ms)?;
                out.push_str("</gml:endPosition></gml:TimePeriod>");
            }
            _ => return Err(EncodeError::InvalidTime),
        }
        out.push_str("</gml:validTime><aixm:interpretation>");
        out.push_str(
            interpretation_text(feature.interpretation)
                .ok_or(EncodeError::MissingInterpretation)?,
        );
        out.push_str("</aixm:interpretation>");
        if feature.meta & FEATURE_META_SEQUENCE != 0 {
            push_aixm_num(out, "sequenceNumber", feature.sequence_number);
        }
        if feature.meta & FEATURE_META_CORRECTION != 0 {
            push_aixm_num(out, "correctionNumber", feature.correction_number);
        }
        write_feature_properties(n, row, feature, index, out)?;
        let _ = write!(
            out,
            "</aixm:{name}TimeSlice></aixm:timeSlice></aixm:{name}></message:hasMember>"
        );
        if out.len() > MAX_ENCODED_BYTES {
            return Err(EncodeError::OutputLimit);
        }
    }
    Ok(())
}

fn feature_name(kind: u8) -> Option<&'static str> {
    match kind {
        FEAT_AIRPORT => Some("AirportHeliport"),
        FEAT_RUNWAY => Some("Runway"),
        FEAT_RUNWAY_DIRECTION => Some("RunwayDirection"),
        FEAT_RUNWAY_ELEMENT => Some("RunwayElement"),
        FEAT_TAXIWAY => Some("Taxiway"),
        FEAT_TAXIWAY_ELEMENT => Some("TaxiwayElement"),
        FEAT_NAVAID => Some("Navaid"),
        FEAT_UNIT => Some("Unit"),
        FEAT_OBSTACLE_AREA => Some("ObstacleArea"),
        FEAT_VERTICAL_STRUCTURE => Some("VerticalStructure"),
        FEAT_RADAR_SYSTEM => Some("RadarSystem"),
        FEAT_PRIMARY_RADAR => Some("PrimarySurveillanceRadar"),
        _ => None,
    }
}

fn write_feature_properties(
    n: &Notams,
    row: usize,
    feature: crate::FeatureSlice,
    feature_index: usize,
    out: &mut String,
) -> Result<(), EncodeError> {
    let kind = feature.kind;
    if feature.designator_len > 0 {
        let len = usize::from(feature.designator_len);
        let bytes = feature
            .designator
            .get(..len)
            .ok_or(EncodeError::InvalidRow)?;
        let value = core::str::from_utf8(bytes).map_err(|_| EncodeError::InvalidText)?;
        let tag = match kind {
            FEAT_AIRPORT | FEAT_RUNWAY | FEAT_RUNWAY_DIRECTION | FEAT_UNIT => "designator",
            FEAT_NAVAID => "type",
            _ => return Err(EncodeError::FeatureDataNotRetained),
        };
        push_aixm_text(out, tag, value)?;
        let tail = feature
            .designator
            .get(len..)
            .ok_or(EncodeError::InvalidRow)?;
        if tail.iter().any(|byte| *byte != 0) {
            return Err(EncodeError::InvalidRow);
        }
    }
    if feature.icao_len > 0 {
        if kind != FEAT_AIRPORT || feature.icao_len != 4 {
            return Err(EncodeError::InvalidRow);
        }
        let value = core::str::from_utf8(&feature.icao).map_err(|_| EncodeError::InvalidText)?;
        push_aixm_text(out, "locationIndicatorICAO", value)?;
    }
    match kind {
        FEAT_RUNWAY_ELEMENT | FEAT_TAXIWAY_ELEMENT => {
            for p in polygons_of(n, row) {
                if n.poly_feature.get(p) == Some(&(feature_index as u32)) {
                    write_polygon(n, p, feature_index, out)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn write_polygon(
    n: &Notams,
    polygon: usize,
    feature_index: usize,
    out: &mut String,
) -> Result<(), EncodeError> {
    let (lat, lon) = crate::polygon_vertices(n, polygon);
    if lat.len() < 3 || lat.len() != lon.len() {
        return Err(EncodeError::InvalidRow);
    }
    let max_poslist = lat
        .len()
        .checked_add(1)
        .and_then(|count| count.checked_mul(25))
        .and_then(|bytes| bytes.checked_add(240))
        .ok_or(EncodeError::OutputLimit)?;
    if out
        .len()
        .checked_add(max_poslist)
        .is_none_or(|len| len > MAX_ENCODED_BYTES)
    {
        return Err(EncodeError::OutputLimit);
    }
    let _ = write!(
        out,
        "<aixm:extent><aixm:ElevatedSurface gml:id=\"oa-surface-{feature_index}-{polygon}\" srsDimension=\"2\" srsName=\"urn:ogc:def:crs:EPSG::4326\"><gml:patches><gml:PolygonPatch><gml:exterior><gml:LinearRing><gml:posList>"
    );
    let mut first = true;
    for (la, lo) in lat.iter().zip(lon) {
        if !first {
            out.push(' ');
        }
        push_e7(out, *la);
        out.push(' ');
        push_e7(out, *lo);
        first = false;
    }
    if let (Some(&la), Some(&lo)) = (lat.first(), lon.first()) {
        out.push(' ');
        push_e7(out, la);
        out.push(' ');
        push_e7(out, lo);
    } else {
        return Err(EncodeError::InvalidRow);
    }
    out.push_str("</gml:posList></gml:LinearRing></gml:exterior></gml:PolygonPatch></gml:patches></aixm:ElevatedSurface></aixm:extent>");
    Ok(())
}

fn push_e7(out: &mut String, value: i32) {
    let magnitude = value.unsigned_abs();
    let _ = write!(
        out,
        "{}{}.{:07}",
        if value < 0 { "-" } else { "" },
        magnitude / 10_000_000,
        magnitude % 10_000_000
    );
}

fn push_aixm_text(out: &mut String, tag: &str, value: &str) -> Result<(), EncodeError> {
    if !xml_text_valid(value) {
        return Err(EncodeError::InvalidText);
    }
    let escaped_len = value
        .chars()
        .try_fold(0usize, |total, c| {
            total.checked_add(match c {
                '&' => 5,
                '<' | '>' => 4,
                _ => c.len_utf8(),
            })
        })
        .ok_or(EncodeError::OutputLimit)?;
    if out
        .len()
        .checked_add(
            tag.len()
                .checked_mul(2)
                .and_then(|v| v.checked_add(16))
                .ok_or(EncodeError::OutputLimit)?,
        )
        .and_then(|v| v.checked_add(escaped_len))
        .is_none_or(|len| len > MAX_ENCODED_BYTES)
    {
        return Err(EncodeError::OutputLimit);
    }
    let _ = write!(out, "<aixm:{tag}>");
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    let _ = write!(out, "</aixm:{tag}>");
    Ok(())
}

fn row_values_valid(n: &Notams, i: usize, r: &crate::Row) -> bool {
    let code = |v: &[u8], extra: &[u8]| {
        !v.is_empty()
            && v.iter().all(u8::is_ascii)
            && crate::scalar::parse_code::<16>(v, extra).is_some()
    };
    let text_ok = |off: u32, len: u32| {
        let start = off as usize;
        let Some(end) = start.checked_add(len as usize) else {
            return false;
        };
        end <= n.strings_used
            && n.strings
                .get(start..end)
                .and_then(|b| core::str::from_utf8(b).ok())
                .is_some()
    };
    let has = |f| has_field(n, i, f);
    (!has(Field::Series) || code(crate::code_str(&r.series).as_bytes(), b":_-"))
        && (!has(Field::Number) || r.number <= 999_999)
        && (!has(Field::Year) || (1970..=9999).contains(&r.year))
        && (!has(Field::Selection) || {
            let v = crate::code_str(&r.selection);
            v.len() == 5 && v.bytes().all(|b| b.is_ascii_uppercase())
        })
        && (!has(Field::Ref)
            || (r.ref_number <= 999_999
                && (1970..=9999).contains(&r.ref_year)
                && (r.ref_series.first().copied().unwrap_or(0) == 0
                    || code(crate::code_str(&r.ref_series).as_bytes(), b":_-"))))
        && (!has(Field::Location)
            || crate::scalar::parse_code::<4>(crate::code_str(&r.location).as_bytes(), b"")
                .is_some())
        && (!has(Field::Fir)
            || crate::scalar::parse_code::<4>(crate::code_str(&r.fir).as_bytes(), b"").is_some())
        && (!has(Field::IcaoLocation)
            || crate::scalar::parse_code::<4>(crate::code_str(&r.icao_location).as_bytes(), b"")
                .is_some())
        && (!has(Field::Xover) || code(crate::code_str(&r.xover).as_bytes(), b"/"))
        && (!has(Field::Text) || text_ok(r.text_off, r.text_len))
        && (!has(Field::Schedule) || text_ok(r.sched_off, r.sched_len))
        && (!has(Field::Issued) || (0..=253_402_300_799_999).contains(&r.issued_ms))
        && (!has(Field::Canceled) || (0..=253_402_300_799_999).contains(&r.canceled_ms))
        && (!has(Field::Centre)
            || (-900_000_000..=900_000_000).contains(&r.lat_e7)
                && (-1_800_000_000..=1_800_000_000).contains(&r.lon_e7))
}

fn valid_ref(v: u8) -> bool {
    matches!(v, REF_MSL | REF_AGL | REF_FL)
}
fn class_text(v: u8) -> Result<&'static str, EncodeError> {
    match v {
        1 => Ok("INTL"),
        2 => Ok("DOM"),
        3 => Ok("MIL"),
        _ => Err(EncodeError::UnsupportedValue),
    }
}
fn push_num(out: &mut String, tag: &str, value: impl core::fmt::Display, present: bool) {
    if present {
        let _ = write!(out, "<event:{tag}>{value}</event:{tag}>");
    }
}
fn push_aixm_num(out: &mut String, tag: &str, value: u32) {
    let _ = write!(out, "<aixm:{tag}>{value}</aixm:{tag}>");
}
fn push_code(out: &mut String, tag: &str, value: &str, present: bool) -> Result<(), EncodeError> {
    if present {
        push_text(out, tag, value)?;
    }
    Ok(())
}
fn push_text(out: &mut String, tag: &str, value: &str) -> Result<(), EncodeError> {
    if !xml_text_valid(value) {
        return Err(EncodeError::InvalidText);
    }
    let escaped_len = value
        .chars()
        .try_fold(0usize, |total, c| {
            total.checked_add(match c {
                '&' => 5,
                '<' | '>' => 4,
                _ => c.len_utf8(),
            })
        })
        .ok_or(EncodeError::OutputLimit)?;
    let tag_len = tag
        .len()
        .checked_mul(2)
        .and_then(|v| v.checked_add(17))
        .ok_or(EncodeError::OutputLimit)?;
    if out
        .len()
        .checked_add(tag_len)
        .and_then(|v| v.checked_add(escaped_len))
        .is_none_or(|len| len > MAX_ENCODED_BYTES)
    {
        return Err(EncodeError::OutputLimit);
    }
    let _ = write!(out, "<event:{tag}>");
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    let _ = write!(out, "</event:{tag}>");
    Ok(())
}
fn push_fnse_text(out: &mut String, tag: &str, value: &str) -> Result<(), EncodeError> {
    if !xml_text_valid(value) {
        return Err(EncodeError::InvalidText);
    }
    let escaped_len = value
        .chars()
        .try_fold(0usize, |total, c| {
            total.checked_add(match c {
                '&' => 5,
                '<' | '>' => 4,
                _ => c.len_utf8(),
            })
        })
        .ok_or(EncodeError::OutputLimit)?;
    let tag_len = tag
        .len()
        .checked_mul(2)
        .and_then(|v| v.checked_add(16))
        .ok_or(EncodeError::OutputLimit)?;
    if out
        .len()
        .checked_add(tag_len)
        .and_then(|v| v.checked_add(escaped_len))
        .is_none_or(|len| len > MAX_ENCODED_BYTES)
    {
        return Err(EncodeError::OutputLimit);
    }
    let _ = write!(out, "<fnse:{tag}>");
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    let _ = write!(out, "</fnse:{tag}>");
    Ok(())
}
fn xml_text_valid(value: &str) -> bool {
    value.chars().all(|c| matches!(c as u32, 0x9 | 0xA | 0xD | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x10000..=0x10FFFF))
}
fn push_flags(
    out: &mut String,
    tag: &str,
    bits: u8,
    names: &[(u8, &str)],
) -> Result<(), EncodeError> {
    let mut first = true;
    let mut text = String::new();
    for (bit, name) in names {
        if bits & bit != 0 {
            if !first {
                text.push(' ');
            }
            text.push_str(name);
            first = false;
        }
    }
    if first {
        return Err(EncodeError::UnsupportedValue);
    }
    push_text(out, tag, &text)
}
fn push_limit(out: &mut String, tag: &str, ft: i32, reference: u8) -> Result<(), EncodeError> {
    let value = match (ft, reference) {
        (ALT_UNLIMITED, REF_FL) => "UNL".to_string(),
        (0, REF_AGL) => "SFC".to_string(),
        (v, REF_FL) if (0..=99_900).contains(&v) && v % 100 == 0 => format!("FL{:03}", v / 100),
        (v, REF_MSL) if v >= 0 => format!("{v}FT MSL"),
        (v, REF_AGL) if v >= 0 => format!("{v}FT AGL"),
        _ => return Err(EncodeError::UnsupportedValue),
    };
    let _ = write!(out, "<event:{tag}>{value}</event:{tag}>");
    Ok(())
}
fn push_coordinates(out: &mut String, lat: i32, lon: i32) -> Result<(), EncodeError> {
    if !(-900_000_000..=900_000_000).contains(&lat)
        || !(-1_800_000_000..=1_800_000_000).contains(&lon)
    {
        return Err(EncodeError::UnsupportedValue);
    }
    let components = |v: i32| -> Result<(u32, u32), EncodeError> {
        let abs = i64::from(v.unsigned_abs());
        let total_minutes = (abs * 60 + 5_000_000) / 10_000_000;
        let reconstructed = (total_minutes * 10_000_000 + 30) / 60;
        if reconstructed != abs {
            return Err(EncodeError::UnsupportedValue);
        }
        Ok(((total_minutes / 60) as u32, (total_minutes % 60) as u32))
    };
    let (lat_deg, lat_min) = components(lat)?;
    let (lon_deg, lon_min) = components(lon)?;
    let _ = write!(
        out,
        "<event:coordinates>{lat_deg:02}{lat_min:02}{}{lon_deg:03}{lon_min:02}{}</event:coordinates>",
        if lat < 0 { 'S' } else { 'N' },
        if lon < 0 { 'W' } else { 'E' }
    );
    Ok(())
}
fn push_uuid(out: &mut String, id: &[u8; 16]) {
    for (i, b) in id.iter().enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) {
            out.push('-');
        }
        let _ = write!(out, "{b:02x}");
    }
}
fn push_iso(out: &mut String, ms: i64) -> Result<(), EncodeError> {
    if !(0..=253_402_300_799_999).contains(&ms) {
        return Err(EncodeError::InvalidTime);
    }
    let days = ms.div_euclid(86_400_000);
    let day_ms = ms.rem_euclid(86_400_000);
    let (year, month, day) = civil_from_days(days);
    let hour = day_ms / 3_600_000;
    let minute = day_ms / 60_000 % 60;
    let second = day_ms / 1000 % 60;
    let millis = day_ms % 1000;
    let _ = write!(
        out,
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z"
    );
    Ok(())
}
fn push_compact_raw(out: &mut String, ms: i64) -> Result<(), EncodeError> {
    if !(0..=253_402_300_799_999).contains(&ms) || ms % 60_000 != 0 {
        return Err(EncodeError::InvalidTime);
    }
    let days = ms.div_euclid(86_400_000);
    let day_ms = ms.rem_euclid(86_400_000);
    let (year, month, day) = civil_from_days(days);
    let _ = write!(
        out,
        "{year:04}{month:02}{day:02}{:02}{:02}",
        day_ms / 3_600_000,
        day_ms / 60_000 % 60
    );
    Ok(())
}
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    if m <= 2 {
        y += 1;
    }
    (y, m, d)
}
