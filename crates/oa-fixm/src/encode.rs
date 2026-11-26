//! Source-grounded FIXM 3.0 / FAA NAS 3.0 encoding for the decoder's representable subset.
//! This is a C6 shell utility: it allocates one XML document and reads no files or clocks.

use core::fmt::Write;

use crate::{
    code_str, has_field, route_text, sector_code, unit_code, Events, Field, ALT_FORM_AT_FIX,
    ALT_FORM_BLOCK, ALT_FORM_SIMPLE, ALT_FORM_VFR, ALT_FORM_VFR_PLUS, FLAG_ASSIGNED_VFR,
    FLAG_ASSIGNED_VFR_PLUS, FLAG_INTERIM_NIL, FTYPE_GENERAL, FTYPE_NON_SCHEDULED, FTYPE_SCHEDULED,
    HANDOFF_ACCEPTANCE, HANDOFF_FAILURE, HANDOFF_INITIATION, HANDOFF_RETRACTION,
    HANDOFF_TAKE_CONTROL, HANDOFF_UPDATE, STATUS_ACTIVE, STATUS_CANCELLED, STATUS_COMPLETED,
    STATUS_DROPPED, STATUS_PROPOSED,
};

/// A row that cannot be represented by this encoder's documented subset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncodeError {
    /// The struct-of-arrays columns do not all contain `Events::len` rows.
    BadTable,
    /// The NAS `MessageCollectionType` requires at least one message.
    EmptyCollection,
    /// The row has a decoder field-error bit; encoding it would erase malformed-input evidence.
    FieldError(Field),
    /// The requested field, form, or code has no unambiguous representation here.
    Unsupported(Field),
    /// A mandatory time, UUID, or scalar is outside this encoder's supported domain.
    InvalidValue(Field),
    /// A required GUFI is zero and cannot be serialized as a usable UUID identity.
    InvalidGufi,
    /// The required flight timestamp is outside the supported positive-year UTC dateTime subset.
    InvalidTimestamp,
}

const NS_NAS: &str = "http://www.faa.aero/nas/3.0";
const NS_BASE: &str = "http://www.fixm.aero/base/3.0";
const NS_FLIGHT: &str = "http://www.fixm.aero/flight/3.0";
const NS_FOUNDATION: &str = "http://www.fixm.aero/foundation/3.0";
const NS_XSI: &str = "http://www.w3.org/2001/XMLSchema-instance";
const ALT_MIN_FT: i32 = -2_000;
const ALT_MAX_FT: i32 = 100_000;
const SPEED_MAX_DKT: i32 = 20_000;

/// Encodes each row to one SFDPS `FlightMessageType`, preserving every supported value.
///
/// Required values (the per-flight timestamp and UUID GUFI) are always emitted. Any error bit,
/// unsupported altitude form, or out-of-range value refuses the whole document; no field is
/// silently dropped. This is not a general FIXM serializer and does not invent fields absent from
/// `Events`.
pub fn encode(events: &Events) -> Result<String, EncodeError> {
    if events.len == 0 {
        return Err(EncodeError::EmptyCollection);
    }
    if events.len > events.capacity || !columns_match(events) {
        return Err(EncodeError::BadTable);
    }
    let mut out = String::with_capacity(events.len.saturating_mul(1024).saturating_add(256));
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
    let _ = write!(
        out,
        "<nas:MessageCollection xmlns:nas=\"{NS_NAS}\" xmlns:fb=\"{NS_BASE}\" xmlns:fx=\"{NS_FLIGHT}\" xmlns:ff=\"{NS_FOUNDATION}\" xmlns:xsi=\"{NS_XSI}\">"
    );
    for row in 0..events.len {
        encode_row(events, row, &mut out)?;
    }
    out.push_str("</nas:MessageCollection>");
    Ok(out)
}

fn columns_match(e: &Events) -> bool {
    let n = e.len;
    [
        e.present.len(),
        e.errors.len(),
        e.flags.len(),
        e.source.len(),
        e.centre.len(),
        e.ts_ms.len(),
        e.gufi.len(),
        e.callsign.len(),
        e.computer_id.len(),
        e.plan_id.len(),
        e.departure.len(),
        e.arrival.len(),
        e.status.len(),
        e.flight_type.len(),
        e.assigned_ft.len(),
        e.reported_ft.len(),
        e.lat_e7.len(),
        e.lon_e7.len(),
        e.pos_time_ms.len(),
        e.gs_dkt.len(),
        e.vx_dkt.len(),
        e.vy_dkt.len(),
        e.aircraft_type.len(),
        e.route_off.len(),
        e.route_len.len(),
        e.operator.len(),
        e.beacon.len(),
        e.ctrl.len(),
        e.recv.len(),
        e.xfer.len(),
        e.dep_actual_ms.len(),
        e.arr_est_ms.len(),
        e.msg_seq.len(),
        e.plan_seq.len(),
        e.assigned_form.len(),
        e.requested_form.len(),
        e.requested_ft.len(),
        e.interim_ft.len(),
        e.block_above_ft.len(),
        e.block_below_ft.len(),
        e.fixalt_pre_ft.len(),
        e.fixalt_post_ft.len(),
        e.fixalt_point.len(),
        e.handoff_event.len(),
        e.acpt.len(),
        e.address.len(),
    ]
    .iter()
    .all(|&len| len == n)
        && e.strings_used <= e.strings.len()
}

fn encode_row(e: &Events, i: usize, out: &mut String) -> Result<(), EncodeError> {
    let errors = *e.errors.get(i).ok_or(EncodeError::BadTable)?;
    if errors != 0 {
        for field in crate::ALL_FIELDS {
            if errors & crate::field_bit(field) != 0 {
                return Err(EncodeError::FieldError(field));
            }
        }
        return Err(EncodeError::BadTable);
    }
    // `assignedAltitude` and `requestedAltitude` each select one representation. The present bits
    // report the selected form separately from the value column.
    if has_field(e, i, Field::AssignedForm)
        && !matches!(
            *e.assigned_form.get(i).ok_or(EncodeError::BadTable)?,
            ALT_FORM_SIMPLE | ALT_FORM_VFR | ALT_FORM_VFR_PLUS | ALT_FORM_BLOCK | ALT_FORM_AT_FIX
        )
    {
        return Err(EncodeError::Unsupported(Field::AssignedForm));
    }
    if has_field(e, i, Field::RequestedForm)
        && !matches!(
            *e.requested_form.get(i).ok_or(EncodeError::BadTable)?,
            ALT_FORM_SIMPLE | ALT_FORM_VFR | ALT_FORM_VFR_PLUS
        )
    {
        return Err(EncodeError::Unsupported(Field::RequestedForm));
    }
    validate_row_values(e, i)?;

    let timestamp = format_time(*e.ts_ms.get(i).ok_or(EncodeError::BadTable)?)
        .ok_or(EncodeError::InvalidTimestamp)?;
    let gufi = format_uuid(*e.gufi.get(i).ok_or(EncodeError::BadTable)?);
    out.push_str(
        "<message xsi:type=\"nas:FlightMessageType\"><flight xsi:type=\"nas:NasFlightType\"",
    );
    write_optional_attr(
        out,
        "centre",
        has_field(e, i, Field::Centre),
        code_str(e.centre.get(i).ok_or(EncodeError::BadTable)?),
    );
    write_optional_attr(
        out,
        "source",
        has_field(e, i, Field::Source),
        code_str(e.source.get(i).ok_or(EncodeError::BadTable)?),
    );
    let flight_type = match *e.flight_type.get(i).ok_or(EncodeError::BadTable)? {
        FTYPE_SCHEDULED => "SCHEDULED",
        FTYPE_NON_SCHEDULED => "NON_SCHEDULED",
        FTYPE_GENERAL => "GENERAL",
        0 if !has_field(e, i, Field::FlightType) => "",
        _ => return Err(EncodeError::InvalidValue(Field::FlightType)),
    };
    write_optional_attr(
        out,
        "flightType",
        has_field(e, i, Field::FlightType),
        flight_type,
    );
    let _ = write!(out, " timestamp=\"{timestamp}\">");
    encode_agreed_route(e, i, out)?;
    encode_aircraft(e, i, out)?;
    encode_arrival(e, i, out)?;
    encode_controlling(e, i, out)?;
    encode_departure(e, i, out)?;
    encode_enroute(e, i, out)?;
    encode_flight_identification(e, i, out)?;
    encode_status(e, i, out)?;
    let _ = write!(out, "<fx:gufi codeSpace=\"urn:uuid\">{gufi}</fx:gufi>");
    encode_operator(e, i, out)?;
    encode_supplemental(e, i, out)?;
    encode_assigned(e, i, out)?;
    encode_flight_plan(e, i, out)?;
    encode_interim(e, i, out)?;
    encode_requested(e, i, out)?;
    out.push_str("</flight></message>");
    Ok(())
}

fn validate_row_values(e: &Events, i: usize) -> Result<(), EncodeError> {
    let get = |field: Field, text: &str| {
        if has_field(e, i, field) && !is_xml10_text(text) {
            Err(EncodeError::InvalidValue(field))
        } else {
            Ok(())
        }
    };
    if !valid_gufi(*e.gufi.get(i).ok_or(EncodeError::BadTable)?) {
        return Err(EncodeError::InvalidGufi);
    }
    for (field, text, max, extra) in [
        (
            Field::Source,
            code_str(e.source.get(i).ok_or(EncodeError::BadTable)?),
            2,
            "",
        ),
        (
            Field::Centre,
            code_str(e.centre.get(i).ok_or(EncodeError::BadTable)?),
            4,
            "",
        ),
        (
            Field::Callsign,
            code_str(e.callsign.get(i).ok_or(EncodeError::BadTable)?),
            7,
            "",
        ),
        (
            Field::ComputerId,
            code_str(e.computer_id.get(i).ok_or(EncodeError::BadTable)?),
            4,
            "",
        ),
        (
            Field::FlightPlanId,
            code_str(e.plan_id.get(i).ok_or(EncodeError::BadTable)?),
            12,
            "",
        ),
        (
            Field::Departure,
            code_str(e.departure.get(i).ok_or(EncodeError::BadTable)?),
            12,
            "/",
        ),
        (
            Field::Arrival,
            code_str(e.arrival.get(i).ok_or(EncodeError::BadTable)?),
            12,
            "/",
        ),
        (
            Field::AircraftType,
            code_str(e.aircraft_type.get(i).ok_or(EncodeError::BadTable)?),
            4,
            "",
        ),
        (
            Field::Operator,
            code_str(e.operator.get(i).ok_or(EncodeError::BadTable)?),
            24,
            " -&.'/",
        ),
    ] {
        get(field, text)?;
        if has_field(e, i, field) && !valid_code(text, max, extra) {
            return Err(EncodeError::InvalidValue(field));
        }
    }
    for field in [
        Field::ControllingUnit,
        Field::HandoffReceiving,
        Field::HandoffAccepting,
        Field::HandoffTransferring,
    ] {
        if has_field(e, i, field) {
            let unit = match field {
                Field::ControllingUnit => e.ctrl.get(i).ok_or(EncodeError::BadTable)?,
                Field::HandoffReceiving => e.recv.get(i).ok_or(EncodeError::BadTable)?,
                Field::HandoffAccepting => e.acpt.get(i).ok_or(EncodeError::BadTable)?,
                _ => e.xfer.get(i).ok_or(EncodeError::BadTable)?,
            };
            get(field, unit_code(unit))?;
            get(field, sector_code(unit))?;
            if !valid_code(unit_code(unit), 4, "?")
                || (!sector_code(unit).is_empty() && !valid_code(sector_code(unit), 4, ""))
            {
                return Err(EncodeError::InvalidValue(field));
            }
        }
    }
    if has_field(e, i, Field::Route) {
        let start = *e.route_off.get(i).ok_or(EncodeError::BadTable)? as usize;
        let len = *e.route_len.get(i).ok_or(EncodeError::BadTable)? as usize;
        let end = start.checked_add(len).ok_or(EncodeError::BadTable)?;
        let route = e.strings.get(start..end).ok_or(EncodeError::BadTable)?;
        let route =
            std::str::from_utf8(route).map_err(|_| EncodeError::InvalidValue(Field::Route))?;
        get(Field::Route, route)?;
        if route.len() > 4096 {
            return Err(EncodeError::InvalidValue(Field::Route));
        }
    }
    if has_field(e, i, Field::FlightType)
        && !matches!(
            *e.flight_type.get(i).ok_or(EncodeError::BadTable)?,
            FTYPE_SCHEDULED | FTYPE_NON_SCHEDULED | FTYPE_GENERAL
        )
    {
        return Err(EncodeError::InvalidValue(Field::FlightType));
    }
    if has_field(e, i, Field::AssignedForm) {
        let form = *e.assigned_form.get(i).ok_or(EncodeError::BadTable)?;
        let flags = *e.flags.get(i).ok_or(EncodeError::BadTable)?;
        let vfr = flags & FLAG_ASSIGNED_VFR != 0;
        let plus = flags & FLAG_ASSIGNED_VFR_PLUS != 0;
        let valid_flags = match form {
            ALT_FORM_SIMPLE => !vfr && !plus,
            ALT_FORM_VFR => vfr && !plus,
            ALT_FORM_VFR_PLUS => plus && !vfr,
            ALT_FORM_BLOCK | ALT_FORM_AT_FIX => !vfr && !plus,
            _ => false,
        };
        if !valid_flags {
            return Err(EncodeError::InvalidValue(Field::AssignedForm));
        }
        match form {
            ALT_FORM_BLOCK => {
                let above = *e.block_above_ft.get(i).ok_or(EncodeError::BadTable)?;
                let below = *e.block_below_ft.get(i).ok_or(EncodeError::BadTable)?;
                if !(ALT_MIN_FT..=ALT_MAX_FT).contains(&above)
                    || !(ALT_MIN_FT..=ALT_MAX_FT).contains(&below)
                    || above > below
                {
                    return Err(EncodeError::InvalidValue(Field::AssignedForm));
                }
            }
            ALT_FORM_AT_FIX => {
                let point = code_str(e.fixalt_point.get(i).ok_or(EncodeError::BadTable)?);
                let pre = *e.fixalt_pre_ft.get(i).ok_or(EncodeError::BadTable)?;
                let post = *e.fixalt_post_ft.get(i).ok_or(EncodeError::BadTable)?;
                if !valid_code(point, 12, "/")
                    || !(ALT_MIN_FT..=ALT_MAX_FT).contains(&pre)
                    || !(ALT_MIN_FT..=ALT_MAX_FT).contains(&post)
                {
                    return Err(EncodeError::InvalidValue(Field::AssignedForm));
                }
            }
            _ => {}
        }
    }
    for field in [
        Field::AssignedForm,
        Field::RequestedForm,
        Field::InterimAlt,
        Field::ReportedAlt,
    ] {
        if !has_field(e, i, field) {
            continue;
        }
        if field == Field::InterimAlt
            && *e.flags.get(i).ok_or(EncodeError::BadTable)? & FLAG_INTERIM_NIL != 0
        {
            continue;
        }
        let value = match field {
            Field::AssignedForm
                if matches!(
                    *e.assigned_form.get(i).ok_or(EncodeError::BadTable)?,
                    ALT_FORM_SIMPLE | ALT_FORM_VFR_PLUS
                ) =>
            {
                *e.assigned_ft.get(i).ok_or(EncodeError::BadTable)?
            }
            Field::RequestedForm
                if matches!(
                    *e.requested_form.get(i).ok_or(EncodeError::BadTable)?,
                    ALT_FORM_SIMPLE | ALT_FORM_VFR_PLUS
                ) =>
            {
                *e.requested_ft.get(i).ok_or(EncodeError::BadTable)?
            }
            Field::InterimAlt => *e.interim_ft.get(i).ok_or(EncodeError::BadTable)?,
            Field::ReportedAlt => *e.reported_ft.get(i).ok_or(EncodeError::BadTable)?,
            _ => continue,
        };
        if !(ALT_MIN_FT..=ALT_MAX_FT).contains(&value) {
            return Err(EncodeError::InvalidValue(field));
        }
    }
    if has_field(e, i, Field::GroundSpeed)
        && i32::from(*e.gs_dkt.get(i).ok_or(EncodeError::BadTable)?) > SPEED_MAX_DKT
    {
        return Err(EncodeError::InvalidValue(Field::GroundSpeed));
    }
    if has_field(e, i, Field::Velocity)
        && (i32::from(*e.vx_dkt.get(i).ok_or(EncodeError::BadTable)?).abs() > SPEED_MAX_DKT
            || i32::from(*e.vy_dkt.get(i).ok_or(EncodeError::BadTable)?).abs() > SPEED_MAX_DKT)
    {
        return Err(EncodeError::InvalidValue(Field::Velocity));
    }
    if has_field(e, i, Field::Position)
        && (!(-900_000_000..=900_000_000).contains(e.lat_e7.get(i).ok_or(EncodeError::BadTable)?)
            || !(-1_800_000_000..=1_800_000_000)
                .contains(e.lon_e7.get(i).ok_or(EncodeError::BadTable)?))
    {
        return Err(EncodeError::InvalidValue(Field::Position));
    }
    if has_field(e, i, Field::AircraftAddress) {
        let address = *e.address.get(i).ok_or(EncodeError::BadTable)?;
        if address == 0 || address > 0x00ff_ffff {
            return Err(EncodeError::InvalidValue(Field::AircraftAddress));
        }
    }
    if has_field(e, i, Field::Beacon) {
        let value = *e.beacon.get(i).ok_or(EncodeError::BadTable)?;
        let text = format!("{value:04}");
        if crate::parse_beacon(text.as_bytes()).is_none() {
            return Err(EncodeError::InvalidValue(Field::Beacon));
        }
    }
    Ok(())
}

fn encode_flight_identification(e: &Events, i: usize, out: &mut String) -> Result<(), EncodeError> {
    if has_field(e, i, Field::Callsign) || has_field(e, i, Field::ComputerId) {
        out.push_str("<fx:flightIdentification xsi:type=\"nas:NasFlightIdentificationType\"");
        write_optional_attr(
            out,
            "aircraftIdentification",
            has_field(e, i, Field::Callsign),
            code_str(e.callsign.get(i).ok_or(EncodeError::BadTable)?),
        );
        write_optional_attr(
            out,
            "computerId",
            has_field(e, i, Field::ComputerId),
            code_str(e.computer_id.get(i).ok_or(EncodeError::BadTable)?),
        );
        out.push_str("/>");
    }
    Ok(())
}

fn encode_flight_plan(e: &Events, i: usize, out: &mut String) -> Result<(), EncodeError> {
    if has_field(e, i, Field::FlightPlanId) {
        out.push_str("<flightPlan");
        write_optional_attr(
            out,
            "identifier",
            true,
            code_str(e.plan_id.get(i).ok_or(EncodeError::BadTable)?),
        );
        out.push_str("/>");
    }
    Ok(())
}

fn encode_departure(e: &Events, i: usize, out: &mut String) -> Result<(), EncodeError> {
    if !has_field(e, i, Field::Departure) && !has_field(e, i, Field::DepartureActual) {
        return Ok(());
    }
    out.push_str("<fx:departure xsi:type=\"nas:NasDepartureType\"");
    write_optional_attr(
        out,
        "departurePoint",
        has_field(e, i, Field::Departure),
        code_str(e.departure.get(i).ok_or(EncodeError::BadTable)?),
    );
    if has_field(e, i, Field::DepartureActual) {
        let time = format_time(*e.dep_actual_ms.get(i).ok_or(EncodeError::BadTable)?)
            .ok_or(EncodeError::InvalidValue(Field::DepartureActual))?;
        let _ = write!(out, "><fb:runwayPositionAndTime><fb:runwayTime><fb:actual time=\"{time}\"/></fb:runwayTime></fb:runwayPositionAndTime></fx:departure>");
    } else {
        out.push_str("/>");
    }
    Ok(())
}

fn encode_arrival(e: &Events, i: usize, out: &mut String) -> Result<(), EncodeError> {
    if !has_field(e, i, Field::Arrival) && !has_field(e, i, Field::ArrivalEstimated) {
        return Ok(());
    }
    out.push_str("<fx:arrival xsi:type=\"nas:NasArrivalType\"");
    write_optional_attr(
        out,
        "arrivalPoint",
        has_field(e, i, Field::Arrival),
        code_str(e.arrival.get(i).ok_or(EncodeError::BadTable)?),
    );
    if has_field(e, i, Field::ArrivalEstimated) {
        let time = format_time(*e.arr_est_ms.get(i).ok_or(EncodeError::BadTable)?)
            .ok_or(EncodeError::InvalidValue(Field::ArrivalEstimated))?;
        let _ = write!(out, "><fb:runwayPositionAndTime><fb:runwayTime><fb:estimated time=\"{time}\"/></fb:runwayTime></fb:runwayPositionAndTime></fx:arrival>");
    } else {
        out.push_str("/>");
    }
    Ok(())
}

fn encode_status(e: &Events, i: usize, out: &mut String) -> Result<(), EncodeError> {
    if !has_field(e, i, Field::Status) {
        return Ok(());
    }
    let status = match *e.status.get(i).ok_or(EncodeError::BadTable)? {
        STATUS_PROPOSED => "PROPOSED",
        STATUS_ACTIVE => "ACTIVE",
        STATUS_COMPLETED => "COMPLETED",
        STATUS_CANCELLED => "CANCELLED",
        STATUS_DROPPED => "DROPPED",
        _ => return Err(EncodeError::InvalidValue(Field::Status)),
    };
    let _ = write!(
        out,
        "<fx:flightStatus xsi:type=\"nas:NasFlightStatusType\" fdpsFlightStatus=\"{status}\"/>"
    );
    Ok(())
}

fn encode_interim(e: &Events, i: usize, out: &mut String) -> Result<(), EncodeError> {
    if has_field(e, i, Field::InterimAlt) {
        if *e.flags.get(i).ok_or(EncodeError::BadTable)? & FLAG_INTERIM_NIL != 0 {
            out.push_str("<interimAltitude xsi:nil=\"true\"/>");
        } else {
            let _ = write!(
                out,
                "<interimAltitude uom=\"FEET\">{}</interimAltitude>",
                *e.interim_ft.get(i).ok_or(EncodeError::BadTable)?
            );
        }
    }
    Ok(())
}

fn is_xml10_text(value: &str) -> bool {
    value.chars().all(|c| {
        matches!(c as u32,
        0x9 | 0xA | 0xD | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x10000..=0x10FFFF)
    })
}

fn valid_code(value: &str, max: usize, extra: &str) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || extra.as_bytes().contains(&byte))
}

// Core FIXM 3.0.1 RestrictedGloballyUniqueFlightIdentifierType requires RFC-4122 version 4
// and the RFC variant bits; codeSpace is emitted as the fixed urn:uuid value.
fn valid_gufi(value: u128) -> bool {
    ((value >> 76) & 0xF) == 4 && ((value >> 62) & 0x3) == 0b10
}

fn encode_assigned(e: &Events, i: usize, out: &mut String) -> Result<(), EncodeError> {
    if !has_field(e, i, Field::AssignedForm) {
        return Ok(());
    }
    match *e.assigned_form.get(i).ok_or(EncodeError::BadTable)? {
        ALT_FORM_SIMPLE => {
            let _ = write!(
                out,
                "<assignedAltitude><simple uom=\"FEET\">{}</simple></assignedAltitude>",
                *e.assigned_ft.get(i).ok_or(EncodeError::BadTable)?
            );
        }
        ALT_FORM_VFR_PLUS
            if *e.flags.get(i).ok_or(EncodeError::BadTable)? & FLAG_ASSIGNED_VFR_PLUS != 0 =>
        {
            let _ = write!(
                out,
                "<assignedAltitude><vfrPlus uom=\"FEET\">{}</vfrPlus></assignedAltitude>",
                *e.assigned_ft.get(i).ok_or(EncodeError::BadTable)?
            );
        }
        ALT_FORM_VFR if *e.flags.get(i).ok_or(EncodeError::BadTable)? & FLAG_ASSIGNED_VFR != 0 => {
            out.push_str("<assignedAltitude><vfr/></assignedAltitude>")
        }
        ALT_FORM_BLOCK => {
            let above = *e.block_above_ft.get(i).ok_or(EncodeError::BadTable)?;
            let below = *e.block_below_ft.get(i).ok_or(EncodeError::BadTable)?;
            let _ = write!(out, "<assignedAltitude><block><above uom=\"FEET\">{above}</above><below uom=\"FEET\">{below}</below></block></assignedAltitude>");
        }
        ALT_FORM_AT_FIX => {
            let point = code_str(e.fixalt_point.get(i).ok_or(EncodeError::BadTable)?);
            let pre = *e.fixalt_pre_ft.get(i).ok_or(EncodeError::BadTable)?;
            let post = *e.fixalt_post_ft.get(i).ok_or(EncodeError::BadTable)?;
            let _ = write!(out, "<assignedAltitude><altFixAlt><point xsi:type=\"fb:FixPointType\" fix=\"{point}\"/><post uom=\"FEET\">{post}</post><pre uom=\"FEET\">{pre}</pre></altFixAlt></assignedAltitude>");
        }
        _ => return Err(EncodeError::InvalidValue(Field::AssignedForm)),
    }
    Ok(())
}

fn encode_requested(e: &Events, i: usize, out: &mut String) -> Result<(), EncodeError> {
    if !has_field(e, i, Field::RequestedForm) {
        return Ok(());
    }
    match *e.requested_form.get(i).ok_or(EncodeError::BadTable)? {
        ALT_FORM_SIMPLE => {
            let _ = write!(
                out,
                "<requestedAltitude><simple uom=\"FEET\">{}</simple></requestedAltitude>",
                *e.requested_ft.get(i).ok_or(EncodeError::BadTable)?
            );
        }
        ALT_FORM_VFR_PLUS => {
            let _ = write!(
                out,
                "<requestedAltitude><vfrPlus uom=\"FEET\">{}</vfrPlus></requestedAltitude>",
                *e.requested_ft.get(i).ok_or(EncodeError::BadTable)?
            );
        }
        ALT_FORM_VFR => out.push_str("<requestedAltitude><vfr/></requestedAltitude>"),
        _ => return Err(EncodeError::InvalidValue(Field::RequestedForm)),
    }
    Ok(())
}

fn encode_aircraft(e: &Events, i: usize, out: &mut String) -> Result<(), EncodeError> {
    if !has_field(e, i, Field::AircraftAddress) && !has_field(e, i, Field::AircraftType) {
        return Ok(());
    }
    out.push_str("<fx:aircraftDescription xsi:type=\"nas:NasAircraftType\"");
    write_optional_attr(
        out,
        "aircraftAddress",
        has_field(e, i, Field::AircraftAddress),
        &format!("{:06X}", *e.address.get(i).ok_or(EncodeError::BadTable)?),
    );
    if has_field(e, i, Field::AircraftType) {
        let _ = write!(out, "><fx:aircraftType><fx:icaoModelIdentifier>{}</fx:icaoModelIdentifier></fx:aircraftType></fx:aircraftDescription>", code_str(e.aircraft_type.get(i).ok_or(EncodeError::BadTable)?));
    } else {
        out.push_str("/>");
    }
    Ok(())
}

fn encode_agreed_route(e: &Events, i: usize, out: &mut String) -> Result<(), EncodeError> {
    if !has_field(e, i, Field::Route) {
        return Ok(());
    }
    out.push_str("<fx:agreed><fx:route xsi:type=\"nas:NasRouteType\" nasRouteText=\"");
    escape_attr(out, route_text(e, i));
    out.push_str("\"/></fx:agreed>");
    Ok(())
}

fn encode_operator(e: &Events, i: usize, out: &mut String) -> Result<(), EncodeError> {
    if has_field(e, i, Field::Operator) {
        out.push_str("<fx:operator><fx:operatingOrganization><ff:organization name=\"");
        escape_attr(
            out,
            code_str(e.operator.get(i).ok_or(EncodeError::BadTable)?),
        );
        out.push_str("\"/></fx:operatingOrganization></fx:operator>");
    }
    Ok(())
}

fn encode_controlling(e: &Events, i: usize, out: &mut String) -> Result<(), EncodeError> {
    if has_field(e, i, Field::ControllingUnit) {
        out.push_str("<fx:controllingUnit xsi:type=\"fb:IdentifiedUnitReferenceType\"");
        write_optional_attr(
            out,
            "unitIdentifier",
            true,
            unit_code(e.ctrl.get(i).ok_or(EncodeError::BadTable)?),
        );
        write_optional_attr(
            out,
            "sectorIdentifier",
            !sector_code(e.ctrl.get(i).ok_or(EncodeError::BadTable)?).is_empty(),
            sector_code(e.ctrl.get(i).ok_or(EncodeError::BadTable)?),
        );
        out.push_str("/>");
    }
    Ok(())
}

fn encode_enroute(e: &Events, i: usize, out: &mut String) -> Result<(), EncodeError> {
    let any = [
        Field::ReportedAlt,
        Field::Position,
        Field::PositionTime,
        Field::GroundSpeed,
        Field::Velocity,
        Field::Coast,
        Field::HandoffReceiving,
        Field::HandoffAccepting,
        Field::HandoffTransferring,
        Field::HandoffEvent,
        Field::Beacon,
    ]
    .iter()
    .any(|&f| has_field(e, i, f));
    if !any {
        return Ok(());
    }
    out.push_str("<fx:enRoute xsi:type=\"nas:NasEnRouteType\">");
    if has_field(e, i, Field::Beacon) {
        let _ = write!(out, "<fx:beaconCodeAssignment><fx:currentBeaconCode>{}</fx:currentBeaconCode></fx:beaconCodeAssignment>", *e.beacon.get(i).ok_or(EncodeError::BadTable)?);
    }
    if has_field(e, i, Field::HandoffEvent)
        || has_field(e, i, Field::HandoffReceiving)
        || has_field(e, i, Field::HandoffAccepting)
        || has_field(e, i, Field::HandoffTransferring)
    {
        out.push_str("<fx:boundaryCrossings xsi:type=\"nas:NasUnitBoundaryType\"><handoff xsi:type=\"nas:NasHandoffType\"");
        if has_field(e, i, Field::HandoffEvent) {
            let event = match *e.handoff_event.get(i).ok_or(EncodeError::BadTable)? {
                HANDOFF_INITIATION => "INITIATION",
                HANDOFF_ACCEPTANCE => "ACCEPTANCE",
                HANDOFF_UPDATE => "UPDATE",
                HANDOFF_RETRACTION => "RETRACTION",
                HANDOFF_FAILURE => "FAILURE",
                HANDOFF_TAKE_CONTROL => "TAKE_CONTROL",
                _ => return Err(EncodeError::InvalidValue(Field::HandoffEvent)),
            };
            write_optional_attr(out, "event", true, event);
        }
        out.push('>');
        for (field, name, unit, unit_type) in [
            (
                Field::HandoffReceiving,
                "receivingUnit",
                e.recv.get(i).ok_or(EncodeError::BadTable)?,
                "fb:IdentifiedUnitReferenceType",
            ),
            (
                Field::HandoffAccepting,
                "acceptingUnit",
                e.acpt.get(i).ok_or(EncodeError::BadTable)?,
                "fb:IdentifiedUnitReferenceType",
            ),
            (
                Field::HandoffTransferring,
                "transferringUnit",
                e.xfer.get(i).ok_or(EncodeError::BadTable)?,
                "fb:IdentifiedUnitReferenceType",
            ),
        ] {
            if has_field(e, i, field) {
                let _ = write!(out, "<{name} xsi:type=\"{unit_type}\"");
                write_optional_attr(out, "unitIdentifier", true, unit_code(unit));
                write_optional_attr(
                    out,
                    "sectorIdentifier",
                    !sector_code(unit).is_empty(),
                    sector_code(unit),
                );
                out.push_str("/>");
            }
        }
        out.push_str("</handoff></fx:boundaryCrossings>");
    }
    if has_field(e, i, Field::Position)
        || has_field(e, i, Field::PositionTime)
        || has_field(e, i, Field::GroundSpeed)
        || has_field(e, i, Field::ReportedAlt)
        || has_field(e, i, Field::Velocity)
        || has_field(e, i, Field::Coast)
    {
        out.push_str("<fx:position xsi:type=\"nas:NasAircraftPositionType\"");
        if has_field(e, i, Field::PositionTime) {
            let t = format_time(*e.pos_time_ms.get(i).ok_or(EncodeError::BadTable)?)
                .ok_or(EncodeError::InvalidValue(Field::PositionTime))?;
            write_optional_attr(out, "positionTime", true, &t);
        }
        if has_field(e, i, Field::Coast) {
            write_optional_attr(out, "coastIndicator", true, "COASTING");
        }
        out.push('>');
        if has_field(e, i, Field::GroundSpeed) {
            let _ = write!(
                out,
                "<fx:actualSpeed><fx:surveillance uom=\"KNOTS\">{}</fx:surveillance></fx:actualSpeed>",
                scaled_tenths(*e.gs_dkt.get(i).ok_or(EncodeError::BadTable)?)
            );
        }
        if has_field(e, i, Field::ReportedAlt) {
            let _ = write!(
                out,
                "<fx:altitude uom=\"FEET\">{}</fx:altitude>",
                *e.reported_ft.get(i).ok_or(EncodeError::BadTable)?
            );
        }
        if has_field(e, i, Field::Position) {
            let _ = write!(out, "<fx:position xsi:type=\"fb:LocationPointType\"><fb:location srsName=\"urn:ogc:def:crs:EPSG::4326\"><ff:pos>{} {}</ff:pos></fb:location></fx:position>", scaled_1e7(*e.lat_e7.get(i).ok_or(EncodeError::BadTable)?), scaled_1e7(*e.lon_e7.get(i).ok_or(EncodeError::BadTable)?));
        }
        if has_field(e, i, Field::Velocity) {
            let _ = write!(
                out,
                "<trackVelocity><x uom=\"KNOTS\">{}</x><y uom=\"KNOTS\">{}</y></trackVelocity>",
                scaled_tenths_signed(*e.vx_dkt.get(i).ok_or(EncodeError::BadTable)?),
                scaled_tenths_signed(*e.vy_dkt.get(i).ok_or(EncodeError::BadTable)?)
            );
        }
        out.push_str("</fx:position>");
    }
    out.push_str("</fx:enRoute>");
    Ok(())
}

fn encode_supplemental(e: &Events, i: usize, out: &mut String) -> Result<(), EncodeError> {
    if !has_field(e, i, Field::MsgSeq) && !has_field(e, i, Field::PlanSeq) {
        return Ok(());
    }
    out.push_str(
        "<fx:supplementalData xsi:type=\"nas:NasSupplementalDataType\"><additionalFlightInformation>",
    );
    if has_field(e, i, Field::MsgSeq) {
        let _ = write!(
            out,
            "<fb:nameValue name=\"MSG_SEQ_NO\" value=\"{}\"/>",
            *e.msg_seq.get(i).ok_or(EncodeError::BadTable)?
        );
    }
    if has_field(e, i, Field::PlanSeq) {
        let _ = write!(
            out,
            "<fb:nameValue name=\"FLIGHT_PLAN_SEQ_NO\" value=\"{}\"/>",
            *e.plan_seq.get(i).ok_or(EncodeError::BadTable)?
        );
    }
    out.push_str("</additionalFlightInformation></fx:supplementalData>");
    Ok(())
}

fn write_optional_attr(out: &mut String, name: &str, present: bool, value: &str) {
    if present {
        let _ = write!(out, " {name}=\"");
        escape_attr(out, value);
        out.push('"');
    }
}

fn escape_attr(out: &mut String, value: &str) {
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' => out.push_str("&#x9;"),
            '\n' => out.push_str("&#xA;"),
            '\r' => out.push_str("&#xD;"),
            _ => out.push(c),
        }
    }
}

fn scaled_tenths(v: u16) -> String {
    scaled_i64(i64::from(v), 1)
}
fn scaled_tenths_signed(v: i16) -> String {
    scaled_i64(i64::from(v), 1)
}
fn scaled_1e7(v: i32) -> String {
    scaled_i64(i64::from(v), 7)
}

fn scaled_i64(v: i64, decimals: u32) -> String {
    let scale = 10i64.pow(decimals);
    let negative = v < 0;
    let abs = v.unsigned_abs();
    let whole = abs / scale as u64;
    let frac = abs % scale as u64;
    if frac == 0 {
        return if negative {
            format!("-{whole}")
        } else {
            whole.to_string()
        };
    }
    let mut frac_text = format!("{frac:0width$}", width = decimals as usize);
    while frac_text.ends_with('0') {
        frac_text.pop();
    }
    if negative {
        format!("-{whole}.{frac_text}")
    } else {
        format!("{whole}.{frac_text}")
    }
}

fn format_uuid(value: u128) -> String {
    let h = format!("{value:032x}");
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

fn format_time(ms: i64) -> Option<String> {
    if ms < 0 {
        return None;
    }
    let days = ms.div_euclid(86_400_000);
    let rem = ms.rem_euclid(86_400_000);
    let z = days.checked_add(719_468)?;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    if !(1970..=9999).contains(&year) {
        return None;
    }
    let hour = rem / 3_600_000;
    let minute = rem % 3_600_000 / 60_000;
    let second = rem % 60_000 / 1000;
    let millis = rem % 1000;
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z"
    ))
}
