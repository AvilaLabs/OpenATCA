//! Strict line parser for the bounded normalized-input protocol. Shell (B1, B3, B7).
//!
//! This is an explicit normalized-input adapter, not a raw FAA recording reader. The reader
//! rejects malformed rows whole and never repairs values. Input and row bounds are checked
//! before parsing or storing data (RULESET C6, C7, E5, E6, E8).

use oa_flight::FlightKey;

/// Maximum bytes in one protocol line, excluding its line ending.
pub const MAX_EVENT_LINE_BYTES: usize = 768;
/// Maximum route fixes carried by one normalized route event.
pub const MAX_ROUTE_FIXES: usize = 16;
/// Maximum bytes in one source label or fix name.
pub const MAX_LABEL_BYTES: usize = 8;
/// Maximum bytes preserved from one filed route expression. Route interpretation belongs to the
/// independent route converter; this adapter retains bounded source text only.
pub const MAX_FILED_ROUTE_BYTES: usize = 256;

/// Input source category. The stable order is used only for equal-receipt event ordering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SourceKind {
    Surveillance,
    Flight,
    Route,
    Weather,
    Adaptation,
}
// A7.2: cold enum, size guard waived; protocol selector.

/// Stable report label for one normalized source kind.
pub fn source_name(source: SourceKind) -> &'static str {
    match source {
        SourceKind::Surveillance => "SURVEILLANCE",
        SourceKind::Flight => "FLIGHT",
        SourceKind::Route => "ROUTE",
        SourceKind::Weather => "WEATHER",
        SourceKind::Adaptation => "ADAPTATION",
    }
}

/// Positive aircraft/flight identity supplied by the normalized source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    /// Zero is explicit absence on raw positions/feed status, never a positive flight binding.
    pub flight_gufi: u128,
    /// Full address including the non-ICAO marker. Zero is explicit unknown for an unbound
    /// non-address flight update or a feed status; it is never an aircraft identity.
    pub aircraft_address: u32,
}
// A7.2: cold struct, size guard waived; one normalized event key.

/// One checked route fix, identified by name and region, with normalized coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouteFixInput {
    pub name: [u8; 8],
    pub region: [u8; 4],
    pub latitude_e7: i32,
    pub longitude_e7: i32,
}
// A7.2: cold struct, size guard waived; route geometry is capped at 16 fixes per event.

/// Flight operation supplied by the normalized source adapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlightOperation {
    SetPlan {
        plan_id: [u8; 12],
        status: u8,
    },
    Depart,
    Complete,
    Cancel,
    Drop,
    /// Full-key form used when the source identity is NAM flight-plan identity rather than GUFI.
    SetPlanByKey {
        key: FlightKey,
        plan_id: [u8; 12],
        status: u8,
    },
    CancelPlanByKey {
        key: FlightKey,
        plan_id: [u8; 12],
    },
    AssignAltitude {
        altitude_ft: i32,
    },
    ClearAssignedAltitude,
    SetInterimAltitude {
        altitude_ft: Option<i32>,
    },
    SetOwner {
        unit: [u8; 8],
    },
    /// SFDPS controlling-unit code and sector as distinct padded source fields.
    SetOwnerParts {
        unit_code: [u8; 4],
        sector_code: [u8; 4],
    },
    SetOwnerByKey {
        key: FlightKey,
        unit: [u8; 8],
    },
    SetAddress,
    SetIdentity {
        callsign: [u8; 8],
        departure: [u8; 12],
        arrival: [u8; 12],
    },
    Handoff {
        operation: HandoffOperation,
        unit: [u8; 8],
    },
    /// Shell-only NAM bridge operation carrying the exact flight-table key resolved by an
    /// explicit pending-RTI binding. This does not derive GUFI or join by callsign.
    HandoffByKey {
        key: FlightKey,
        operation: HandoffOperation,
        from_unit: [u8; 8],
        to_unit: [u8; 8],
    },
}
// A7.2: cold enum, size guard waived; one normalized operation.

/// Handoff state operation. Only `Take` transfers flight ownership.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandoffOperation {
    /// A NAM RLA acknowledges/routs an offer; it does not mean controller acceptance.
    Update,
    Offer,
    Accept,
    Take,
    /// An initiating-center RTA retracts an offer. It differs from LRM failure.
    Retract,
    Fail,
}
// A7.2: cold enum, size guard waived; one normalized handoff operation.

/// A local controller request, distinct from authoritative upstream flight state. The runtime
/// revalidates the captured visible identity/generation and actor authority before applying it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OperatorCommand {
    pub plan_id: [u8; 12],
    pub target_generation: u32,
    pub view_sequence: u32,
    pub actor_unit: u32,
    pub intent: oa_display::interaction::Intent,
}
// A7.2: cold offline normalized request, bounded by receipt/session limits.

/// One normalized event body. Coordinates/time units are fixed point or integer values.
// C6/A3: cold offline input, retained in receipt batches capped at 256 events. Keep the
// fixed 16-fix route inline rather than allocating a separate object for every route event.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    Position {
        latitude_e7: i32,
        longitude_e7: i32,
        altitude_ft: Option<i32>,
        east_mmps: i32,
        north_mmps: i32,
        vertical_rate_fpm: Option<i32>,
        /// Optional callsign retained from this exact surveillance source observation.
        /// Absence is preserved for legacy/manual inputs; this is not an authentication claim.
        track_callsign: Option<[u8; 8]>,
    },
    Flight(FlightOperation),
    Operator(OperatorCommand),
    Route {
        plan_id: [u8; 12],
        meter_fix: [u8; MAX_LABEL_BYTES],
        fix_count: u8,
        fixes: [RouteFixInput; MAX_ROUTE_FIXES],
    },
    /// Filed text with no adapter-guessed geometry. Conversion is owned by the route subsystem.
    FiledRoute {
        plan_id: [u8; 12],
        meter_fix: [u8; MAX_LABEL_BYTES],
        text_len: u16,
        text: [u8; MAX_FILED_ROUTE_BYTES],
    },
    /// Explicit active suffix. The full qualified fix is parsed by the route adapter and the
    /// route-text digest must match the selected revision; no progress is inferred by proximity.
    ActiveCursor {
        plan_id: [u8; 12],
        route_payload_sha256: [u8; 32],
        fix_len: u16,
        fix: [u8; MAX_FILED_ROUTE_BYTES],
    },
    FeedState {
        available: bool,
        reason: u8,
    },
}
// A7.2: cold enum, size guard waived; one normalized event body.

/// One validated input event. Source time is distinct from receipt time; only receipt time drives
/// session state. The reader rejects future source timestamps and source-clock regressions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NormalizedEvent {
    pub receipt_ns: i64,
    pub source_ns: i64,
    pub sequence: u64,
    /// Explicit source-message identity for grouped flight operations. `None` preserves legacy
    /// normalized-event behavior, where each operation sequence is its own message.
    pub source_message_id: Option<u64>,
    pub source: SourceKind,
    pub identity: Identity,
    pub centre: [u8; 4],
    pub kind: EventKind,
}
// A7.2: cold struct, size guard waived; one normalized event.

/// Input-row failure with a stable reason for external reporting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputError {
    RowTooLong,
    BadFieldCount,
    BadVersion,
    UnknownSource,
    UnknownEvent,
    InvalidInteger,
    InvalidHex,
    EmptyIdentity,
    AddressOutOfRange,
    SourceAfterReceipt,
    InvalidCoordinate,
    InvalidAltitude,
    InvalidCentre,
    InvalidLabel,
    TooManyFixes,
    InvalidOperation,
    InvalidFeedState,
    SourceEventMismatch,
    SealedTimestamp,
}
// A7.2: cold enum, size guard waived; one parser result.

fn part<'a>(parts: &[&'a str], index: usize) -> Result<&'a str, InputError> {
    parts.get(index).copied().ok_or(InputError::BadFieldCount)
}

fn parse_i64(text: &str) -> Result<i64, InputError> {
    text.parse::<i64>().map_err(|_| InputError::InvalidInteger)
}

fn parse_u64(text: &str) -> Result<u64, InputError> {
    text.parse::<u64>().map_err(|_| InputError::InvalidInteger)
}

fn parse_i32(text: &str) -> Result<i32, InputError> {
    text.parse::<i32>().map_err(|_| InputError::InvalidInteger)
}

fn parse_hex_u128(text: &str) -> Result<u128, InputError> {
    u128::from_str_radix(text, 16).map_err(|_| InputError::InvalidHex)
}

fn parse_hex_u32(text: &str) -> Result<u32, InputError> {
    u32::from_str_radix(text, 16).map_err(|_| InputError::InvalidHex)
}

fn field<const N: usize>(text: &str) -> Result<[u8; N], InputError> {
    if text.is_empty() || text.len() > N || !text.is_ascii() {
        return Err(InputError::InvalidLabel);
    }
    let mut out = [0u8; N];
    for (slot, byte) in out.iter_mut().zip(text.bytes()) {
        *slot = byte;
    }
    Ok(out)
}

fn alphanumeric_field<const N: usize>(text: &str) -> Result<[u8; N], InputError> {
    let padded = field::<N>(text)?;
    if !text.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return Err(InputError::InvalidLabel);
    }
    Ok(padded)
}

fn optional_field<const N: usize>(text: &str) -> Result<[u8; N], InputError> {
    if text == "-" {
        Ok([0; N])
    } else {
        field::<N>(text)
    }
}

fn unit_code_field(text: &str) -> Result<[u8; 4], InputError> {
    let padded = field::<4>(text)?;
    if !text
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'?')
    {
        return Err(InputError::InvalidLabel);
    }
    Ok(padded)
}

/// Returns whether a centre is a one-to-four byte alphanumeric code with only trailing zero
/// padding. Three-letter ARTCC codes such as `ZAU` therefore remain the distinct value `ZAU\0`.
pub fn is_valid_centre_code(centre: &[u8; 4]) -> bool {
    let mut saw_padding = false;
    let mut saw_code = false;
    for byte in centre {
        if *byte == 0 {
            saw_padding = true;
        } else if saw_padding || !byte.is_ascii_alphanumeric() {
            return false;
        } else {
            saw_code = true;
        }
    }
    saw_code
}

fn fixed_field<const N: usize>(text: &str) -> Result<[u8; N], InputError> {
    if text.len() != N || !text.is_ascii() {
        return Err(InputError::InvalidLabel);
    }
    let mut out = [0u8; N];
    for (slot, byte) in out.iter_mut().zip(text.bytes()) {
        *slot = byte;
    }
    Ok(out)
}

fn option_i32(text: &str) -> Result<Option<i32>, InputError> {
    if text == "-" {
        Ok(None)
    } else {
        parse_i32(text).map(Some)
    }
}

fn parse_source(text: &str) -> Result<SourceKind, InputError> {
    match text {
        "SURVEILLANCE" => Ok(SourceKind::Surveillance),
        "FLIGHT" => Ok(SourceKind::Flight),
        "ROUTE" => Ok(SourceKind::Route),
        "WEATHER" => Ok(SourceKind::Weather),
        "ADAPTATION" => Ok(SourceKind::Adaptation),
        _ => Err(InputError::UnknownSource),
    }
}

fn valid_lat_lon(lat: i32, lon: i32) -> bool {
    (-900_000_000..=900_000_000).contains(&lat) && (-1_800_000_000..=1_800_000_000).contains(&lon)
}

fn parse_position(parts: &[&str]) -> Result<EventKind, InputError> {
    let lat = parse_i32(part(parts, 9)?)?;
    let lon = parse_i32(part(parts, 10)?)?;
    let alt = option_i32(part(parts, 11)?)?;
    let east = parse_i32(part(parts, 12)?)?;
    let north = parse_i32(part(parts, 13)?)?;
    let vertical = option_i32(part(parts, 14)?)?;
    if !valid_lat_lon(lat, lon) {
        return Err(InputError::InvalidCoordinate);
    }
    if alt.is_some_and(|v| !(-2_000..=100_000).contains(&v)) {
        return Err(InputError::InvalidAltitude);
    }
    let track_callsign = match parts.len() {
        15 => None,
        17 if part(parts, 15)? == "CALLSIGN" => {
            let value = part(parts, 16)?;
            if value.is_empty()
                || value.len() > 8
                || !value.bytes().all(|byte| byte.is_ascii_alphanumeric())
            {
                return Err(InputError::InvalidLabel);
            }
            let mut callsign = [0; 8];
            let bytes = value.as_bytes();
            callsign
                .get_mut(..bytes.len())
                .ok_or(InputError::InvalidLabel)?
                .copy_from_slice(bytes);
            Some(callsign)
        }
        _ => return Err(InputError::BadFieldCount),
    };
    Ok(EventKind::Position {
        latitude_e7: lat,
        longitude_e7: lon,
        altitude_ft: alt,
        east_mmps: east,
        north_mmps: north,
        vertical_rate_fpm: vertical,
        track_callsign,
    })
}

fn parse_flight(parts: &[&str]) -> Result<EventKind, InputError> {
    let op = match part(parts, 9)? {
        "PLAN" if parts.len() == 12 => FlightOperation::SetPlan {
            plan_id: alphanumeric_field::<12>(part(parts, 10)?)?,
            status: part(parts, 11)?
                .parse()
                .map_err(|_| InputError::InvalidInteger)?,
        },
        "DEPART" if parts.len() == 10 => FlightOperation::Depart,
        "COMPLETE" if parts.len() == 10 => FlightOperation::Complete,
        "CANCEL" if parts.len() == 10 => FlightOperation::Cancel,
        "DROP" if parts.len() == 10 => FlightOperation::Drop,
        "ASSIGNED" if parts.len() == 11 => FlightOperation::AssignAltitude {
            altitude_ft: parse_i32(part(parts, 10)?)?,
        },
        "UNASSIGNED" if parts.len() == 10 => FlightOperation::ClearAssignedAltitude,
        "INTERIM" if parts.len() == 11 => FlightOperation::SetInterimAltitude {
            altitude_ft: option_i32(part(parts, 10)?)?,
        },
        "OWNER" if parts.len() == 11 => FlightOperation::SetOwner {
            unit: field::<8>(part(parts, 10)?)?,
        },
        "OWNER_PARTS" if parts.len() == 12 => FlightOperation::SetOwnerParts {
            unit_code: unit_code_field(part(parts, 10)?)?,
            sector_code: match part(parts, 11)? {
                "-" => [0; 4],
                text => alphanumeric_field::<4>(text)?,
            },
        },
        "ADDRESS" if parts.len() == 10 => FlightOperation::SetAddress,
        "IDENTITY" if parts.len() == 13 => FlightOperation::SetIdentity {
            callsign: field::<8>(part(parts, 10)?)?,
            departure: field::<12>(part(parts, 11)?)?,
            arrival: field::<12>(part(parts, 12)?)?,
        },
        "IDENTITY_PARTIAL" if parts.len() == 13 => {
            let callsign = optional_field::<8>(part(parts, 10)?)?;
            let departure = optional_field::<12>(part(parts, 11)?)?;
            let arrival = optional_field::<12>(part(parts, 12)?)?;
            if callsign == [0; 8] && departure == [0; 12] && arrival == [0; 12] {
                return Err(InputError::InvalidOperation);
            }
            FlightOperation::SetIdentity {
                callsign,
                departure,
                arrival,
            }
        }
        "HO_OFFER" if parts.len() == 11 => FlightOperation::Handoff {
            operation: HandoffOperation::Offer,
            unit: field::<8>(part(parts, 10)?)?,
        },
        "HO_UPDATE" if parts.len() == 11 => FlightOperation::Handoff {
            operation: HandoffOperation::Update,
            unit: field::<8>(part(parts, 10)?)?,
        },
        "HO_ACCEPT" if parts.len() == 11 => FlightOperation::Handoff {
            operation: HandoffOperation::Accept,
            unit: field::<8>(part(parts, 10)?)?,
        },
        "HO_TAKE" if parts.len() == 11 => FlightOperation::Handoff {
            operation: HandoffOperation::Take,
            unit: field::<8>(part(parts, 10)?)?,
        },
        "HO_RETRACT" if parts.len() == 11 => FlightOperation::Handoff {
            operation: HandoffOperation::Retract,
            unit: field::<8>(part(parts, 10)?)?,
        },
        "HO_FAIL" if parts.len() == 11 => FlightOperation::Handoff {
            operation: HandoffOperation::Fail,
            unit: field::<8>(part(parts, 10)?)?,
        },
        _ => return Err(InputError::InvalidOperation),
    };
    Ok(EventKind::Flight(op))
}

fn parse_route(parts: &[&str]) -> Result<EventKind, InputError> {
    let plan_id = fixed_field::<12>(part(parts, 9)?)?;
    let meter_fix = field::<MAX_LABEL_BYTES>(part(parts, 10)?)?;
    let mut fixes = [RouteFixInput {
        name: [0; 8],
        region: [0; 4],
        latitude_e7: 0,
        longitude_e7: 0,
    }; MAX_ROUTE_FIXES];
    let mut count = 0usize;
    for encoded in part(parts, 11)?.split(';') {
        if count >= MAX_ROUTE_FIXES {
            return Err(InputError::TooManyFixes);
        }
        let mut fields = encoded.split(',');
        let name = field::<8>(fields.next().ok_or(InputError::InvalidLabel)?)?;
        let region = fixed_field::<4>(fields.next().ok_or(InputError::InvalidCentre)?)?;
        let lat = parse_i32(fields.next().ok_or(InputError::InvalidCoordinate)?)?;
        let lon = parse_i32(fields.next().ok_or(InputError::InvalidCoordinate)?)?;
        if fields.next().is_some() || !valid_lat_lon(lat, lon) {
            return Err(InputError::InvalidCoordinate);
        }
        let Some(slot) = fixes.get_mut(count) else {
            return Err(InputError::TooManyFixes);
        };
        *slot = RouteFixInput {
            name,
            region,
            latitude_e7: lat,
            longitude_e7: lon,
        };
        count += 1;
    }
    if count == 0 {
        return Err(InputError::InvalidLabel);
    }
    let fix_count = u8::try_from(count).map_err(|_| InputError::TooManyFixes)?;
    Ok(EventKind::Route {
        plan_id,
        meter_fix,
        fix_count,
        fixes,
    })
}

fn parse_filed_route(parts: &[&str]) -> Result<EventKind, InputError> {
    let plan_id = fixed_field::<12>(part(parts, 9)?)?;
    let meter_fix = field::<MAX_LABEL_BYTES>(part(parts, 10)?)?;
    let route = part(parts, 11)?;
    if route.is_empty() || !route.is_ascii() || route.len() > MAX_FILED_ROUTE_BYTES {
        return Err(InputError::InvalidLabel);
    }
    let mut text = [0; MAX_FILED_ROUTE_BYTES];
    text.get_mut(..route.len())
        .ok_or(InputError::InvalidLabel)?
        .copy_from_slice(route.as_bytes());
    let text_len = u16::try_from(route.len()).map_err(|_| InputError::InvalidLabel)?;
    Ok(EventKind::FiledRoute {
        plan_id,
        meter_fix,
        text_len,
        text,
    })
}

/// Parses one protocol event. The caller checks line length before calling this function.
///
/// Position rows have 15 legacy fields, or 17 fields with a source callsign extension; the event type selects the remaining fields:
/// `v1<TAB>receipt_ns<TAB>source_ns<TAB>sequence<TAB>source<TAB>gufi_hex<TAB>address_hex<TAB>centre<TAB>kind...`.
/// Position rows append `lat<TAB>lon<TAB>alt-or--<TAB>east-mmps<TAB>north-mmps<TAB>vertical-rate-or--`, then optionally `CALLSIGN<TAB>value`;
/// flight and route payloads use the field counts checked below.
pub fn parse_event_line(line: &str) -> Result<NormalizedEvent, InputError> {
    if line.len() > MAX_EVENT_LINE_BYTES {
        return Err(InputError::RowTooLong);
    }
    let mut parts: Vec<&str> = line.split('\t').collect();
    let source_message_id =
        if parts.len() >= 2 && parts.get(parts.len() - 2).copied() == Some("SRCMSG") {
            let value = parts.last().copied().ok_or(InputError::BadFieldCount)?;
            let id = parse_u64(value)?;
            parts.truncate(parts.len() - 2);
            Some(id)
        } else {
            None
        };
    if parts.len() < 9 {
        return Err(InputError::BadFieldCount);
    }
    if part(&parts, 0)? != "v1" {
        return Err(InputError::BadVersion);
    }
    let receipt_ns = parse_i64(part(&parts, 1)?)?;
    let source_ns = parse_i64(part(&parts, 2)?)?;
    // The time envelope is checked before parsing identity or body payload, including malformed
    // rows. Renaming a sealed input to a neutral path cannot bypass the timed-row guard.
    if oa_sealed::in_sealed_window(source_ns) || oa_sealed::in_sealed_window(receipt_ns) {
        return Err(InputError::SealedTimestamp);
    }
    let sequence = parse_u64(part(&parts, 3)?)?;
    if source_message_id.is_some_and(|message_id| message_id > sequence) {
        return Err(InputError::InvalidOperation);
    }
    let source = parse_source(part(&parts, 4)?)?;
    let flight_gufi = parse_hex_u128(part(&parts, 5)?)?;
    let aircraft_address = parse_hex_u32(part(&parts, 6)?)?;
    let kind_name = part(&parts, 8)?;
    let centre_text = part(&parts, 7)?;
    let centre = alphanumeric_field::<4>(centre_text)?;
    // ADS-B carries an aircraft address, not a GUFI. Preserve explicit absence instead of
    // requiring a normalizer to manufacture a flight identity for raw surveillance.
    if flight_gufi == 0 && !matches!(kind_name, "POS" | "FEED") {
        return Err(InputError::EmptyIdentity);
    }
    let address_required = matches!(
        kind_name,
        "POS" | "COMMAND" | "ROUTE" | "FILED_ROUTE" | "CURSOR"
    ) || (kind_name == "FLIGHT" && part(&parts, 9)? == "ADDRESS");
    if aircraft_address == 0 && address_required {
        return Err(InputError::EmptyIdentity);
    }
    if aircraft_address > 0x01ff_ffff {
        return Err(InputError::AddressOutOfRange);
    }
    if receipt_ns < 0 || source_ns < 0 {
        return Err(InputError::InvalidInteger);
    }
    if source_ns > receipt_ns {
        return Err(InputError::SourceAfterReceipt);
    }
    let expected_source = match kind_name {
        "POS" => Some(SourceKind::Surveillance),
        "FLIGHT" | "COMMAND" => Some(SourceKind::Flight),
        "ROUTE" | "FILED_ROUTE" | "CURSOR" => Some(SourceKind::Route),
        _ => None,
    };
    if expected_source.is_some_and(|expected| expected != source) {
        return Err(InputError::SourceEventMismatch);
    }
    if source_message_id.is_some() && (source != SourceKind::Flight || kind_name != "FLIGHT") {
        return Err(InputError::InvalidOperation);
    }
    let kind = match kind_name {
        "POS" if parts.len() == 15 || parts.len() == 17 => parse_position(&parts)?,
        "FLIGHT" => parse_flight(&parts)?,
        "COMMAND" if parts.len() == 14 || parts.len() == 15 => {
            use oa_display::interaction::Intent;
            let name = part(&parts, 13)?;
            let intent = match (name, parts.len()) {
                ("ASSIGNED", 15) => Intent::SetAssignedAltitude(parse_i32(part(&parts, 14)?)?),
                ("UNASSIGNED", 14) => Intent::ClearAssignedAltitude,
                ("INTERIM", 15) => Intent::SetInterimAltitude(parse_i32(part(&parts, 14)?)?),
                ("CLEAR_INTERIM", 14) => Intent::ClearInterimAltitude,
                ("HO_OFFER", 15) => Intent::OfferHandoff(
                    part(&parts, 14)?
                        .parse()
                        .map_err(|_| InputError::InvalidInteger)?,
                ),
                ("HO_ACCEPT", 15) => Intent::AcceptHandoff(
                    part(&parts, 14)?
                        .parse()
                        .map_err(|_| InputError::InvalidInteger)?,
                ),
                ("HO_TAKE", 15) => Intent::TakeHandoff(
                    part(&parts, 14)?
                        .parse()
                        .map_err(|_| InputError::InvalidInteger)?,
                ),
                ("HO_CANCEL", 15) => Intent::CancelHandoff(
                    part(&parts, 14)?
                        .parse()
                        .map_err(|_| InputError::InvalidInteger)?,
                ),
                _ => return Err(InputError::InvalidOperation),
            };
            EventKind::Operator(OperatorCommand {
                plan_id: fixed_field::<12>(part(&parts, 9)?)?,
                target_generation: part(&parts, 10)?
                    .parse()
                    .map_err(|_| InputError::InvalidInteger)?,
                view_sequence: part(&parts, 11)?
                    .parse()
                    .map_err(|_| InputError::InvalidInteger)?,
                actor_unit: part(&parts, 12)?
                    .parse()
                    .map_err(|_| InputError::InvalidInteger)?,
                intent,
            })
        }
        "ROUTE" if parts.len() == 12 => parse_route(&parts)?,
        "FILED_ROUTE" if parts.len() == 12 => parse_filed_route(&parts)?,
        "CURSOR" if parts.len() == 12 => {
            let hex = part(&parts, 10)?;
            if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(InputError::InvalidHex);
            }
            let mut route_payload_sha256 = [0; 32];
            for (slot, pair) in route_payload_sha256
                .iter_mut()
                .zip(hex.as_bytes().chunks_exact(2))
            {
                let value = std::str::from_utf8(pair).map_err(|_| InputError::InvalidHex)?;
                *slot = u8::from_str_radix(value, 16).map_err(|_| InputError::InvalidHex)?;
            }
            let value = part(&parts, 11)?;
            let fix = field::<MAX_FILED_ROUTE_BYTES>(value)?;
            EventKind::ActiveCursor {
                plan_id: fixed_field::<12>(part(&parts, 9)?)?,
                route_payload_sha256,
                fix_len: u16::try_from(value.len()).map_err(|_| InputError::InvalidLabel)?,
                fix,
            }
        }
        "FEED" if parts.len() == 11 => {
            let available = match part(&parts, 9)? {
                "UP" => true,
                "DOWN" => false,
                _ => return Err(InputError::InvalidFeedState),
            };
            let reason = part(&parts, 10)?
                .parse()
                .map_err(|_| InputError::InvalidInteger)?;
            EventKind::FeedState { available, reason }
        }
        _ => return Err(InputError::BadFieldCount),
    };
    Ok(NormalizedEvent {
        receipt_ns,
        source_ns,
        sequence,
        source_message_id,
        source,
        identity: Identity {
            flight_gufi,
            aircraft_address,
        },
        centre,
        kind,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::{is_valid_centre_code, parse_event_line};
    use crate::recordings::serialize_recorded_events;

    #[test]
    fn padded_sfdps_centre_and_short_plan_id_round_trip_without_aliasing() {
        let centre_text = "ZAU";
        let plan_text = "FP7";
        let line = format!(
            "v1\t2000000000\t1000000000\t1\tFLIGHT\t00000000000000000000000000000001\t000000\t{centre_text}\tFLIGHT\tPLAN\t{plan_text}\t2"
        );
        let event = parse_event_line(&line).unwrap();
        assert!(is_valid_centre_code(&event.centre));
        assert_eq!(
            event.centre.get(..centre_text.len()),
            Some(centre_text.as_bytes())
        );
        assert!(event
            .centre
            .get(centre_text.len()..)
            .is_some_and(|padding| padding.iter().all(|byte| *byte == 0)));
        let crate::input::EventKind::Flight(crate::input::FlightOperation::SetPlan {
            plan_id, ..
        }) = event.kind
        else {
            panic!("short plan identifier did not parse as a plan operation");
        };
        assert_eq!(plan_id.get(..plan_text.len()), Some(plan_text.as_bytes()));
        assert!(plan_id
            .get(plan_text.len()..)
            .is_some_and(|padding| padding.iter().all(|byte| *byte == 0)));

        let serialized = serialize_recorded_events(&[event]).unwrap();
        let mut lines = serialized.lines();
        assert_eq!(lines.next(), Some("OA-RS\t1"));
        let row = lines.next().unwrap();
        assert!(lines.next().is_none());
        let round_trip = parse_event_line(row).unwrap();
        assert_eq!(round_trip, event);
    }

    #[test]
    fn padded_code_fields_reject_internal_nul_bytes() {
        let valid = "v1\t2000000000\t1000000000\t1\tFLIGHT\t00000000000000000000000000000001\t000000\tZAU\tFLIGHT\tPLAN\tFP7\t2";
        let invalid_centre = valid.replace("\tZAU\t", "\tZ\0AU\t");
        let invalid_plan = valid.replace("\tFP7\t", "\tF\0P7\t");
        assert!(parse_event_line(&invalid_centre).is_err());
        assert!(parse_event_line(&invalid_plan).is_err());

        let valid_centre = parse_event_line(valid).unwrap().centre;
        let mut embedded_nul = valid_centre;
        embedded_nul[1] = 0;
        embedded_nul[2] = b'A';
        assert!(!is_valid_centre_code(&embedded_nul));
    }

    #[test]
    fn identity_wire_format_preserves_absent_source_fields() {
        // Kills whole-row refusal of a valid callsign-only OP_IDENTITY source update.
        let line = "v1\t2000000000\t1000000000\t7\tFLIGHT\t00000000000000000000000000000001\t000000\tZAU\tFLIGHT\tIDENTITY_PARTIAL\tABC123\t-\t-";
        let event = parse_event_line(line).unwrap();
        assert!(matches!(
            event.kind,
            crate::input::EventKind::Flight(crate::input::FlightOperation::SetIdentity {
                callsign,
                departure,
                arrival,
            }) if callsign.get(..6) == Some(&b"ABC123"[..])
                && departure == [0; 12] && arrival == [0; 12]
        ));
        let serialized = serialize_recorded_events(&[event]).unwrap();
        assert!(serialized.contains("\tIDENTITY_PARTIAL\t"));
        let row = serialized.split_terminator('\n').nth(1).unwrap();
        assert_eq!(parse_event_line(row).unwrap(), event);

        let legacy_line = line
            .replace("IDENTITY_PARTIAL", "IDENTITY")
            .replace("ABC123\t-\t-", "-\tKAAA\tKBBB");
        let legacy = parse_event_line(&legacy_line).unwrap();
        assert!(matches!(
            legacy.kind,
            crate::input::EventKind::Flight(crate::input::FlightOperation::SetIdentity {
                callsign,
                departure,
                arrival,
            }) if callsign.first() == Some(&b'-')
                && departure.get(..4) == Some(b"KAAA")
                && arrival.get(..4) == Some(b"KBBB")
        ));
        let legacy_serialized = serialize_recorded_events(&[legacy]).unwrap();
        assert!(legacy_serialized.contains("\tIDENTITY\t-\tKAAA\tKBBB"));
    }

    #[test]
    fn parses_source_lifecycle_and_unit_sector_as_separate_fields() {
        let prefix = "v1\t2000000000\t1000000000\t1\tFLIGHT\t00000000000000000000000000000001\t000000\tZAU\tFLIGHT";
        for (name, expected) in [
            ("DEPART", super::FlightOperation::Depart),
            ("COMPLETE", super::FlightOperation::Complete),
            ("CANCEL", super::FlightOperation::Cancel),
            ("DROP", super::FlightOperation::Drop),
        ] {
            let event = parse_event_line(&format!("{prefix}\t{name}")).unwrap();
            assert_eq!(event.kind, super::EventKind::Flight(expected));
        }

        let event = parse_event_line(&format!("{prefix}\tOWNER_PARTS\tZXX\t03")).unwrap();
        assert_eq!(
            event.kind,
            super::EventKind::Flight(super::FlightOperation::SetOwnerParts {
                unit_code: *b"ZXX\0",
                sector_code: *b"03\0\0",
            })
        );
        let missing_sector = parse_event_line(&format!("{prefix}\tOWNER_PARTS\tZXX\t-")).unwrap();
        assert_eq!(
            missing_sector.kind,
            super::EventKind::Flight(super::FlightOperation::SetOwnerParts {
                unit_code: *b"ZXX\0",
                sector_code: [0; 4],
            })
        );
    }
}
