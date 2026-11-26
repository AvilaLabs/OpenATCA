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

/// Raw readsb ADS-B source and report fields retained by OA-RS v2. These source facts are not a
/// facility reliability assessment, physical receiver/site identity, or ISR determination.
/// Integer sentinels mean the source decoder did not retain a value. All-zero callsign bytes are
/// retained as-is; this decoder does not distinguish a missing callsign from an explicitly blank
/// callsign.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdsbSourceEvidence {
    pub source_type: u8,
    /// Raw readsb emitter category; zero is the decoder's absent/default value.
    pub category: u8,
    /// Raw readsb ADS-B version value, or `u8::MAX` if not retained.
    pub version: u8,
    pub nic: u8,
    pub nac_p: u8,
    pub nac_v: u8,
    pub sil: u8,
    pub sil_type: u8,
    pub sda: u8,
    pub gva: u8,
    pub nic_baro: u8,
    /// Raw position flags from the decoded ADS-B row.
    pub row_flags: u8,
    /// Raw readsb decimal parse of the four-character octal-digit squawk, or `u16::MAX` if not
    /// retained. Do not convert to Mode A octal in this protocol layer.
    pub squawk: u16,
    /// Raw fixed-width source callsign bytes, preserved without trimming.
    pub callsign: [u8; 8],
}

/// Flight operation supplied by the normalized source adapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlightOperation {
    SetPlan {
        plan_id: [u8; 12],
        status: u8,
    },
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
    },
    /// OA-RS v2 position with the raw per-report ADS-B evidence preserved.
    PositionV2 {
        latitude_e7: i32,
        longitude_e7: i32,
        altitude_ft: Option<i32>,
        east_mmps: i32,
        north_mmps: i32,
        vertical_rate_fpm: Option<i32>,
        evidence: AdsbSourceEvidence,
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
    Ok(EventKind::Position {
        latitude_e7: lat,
        longitude_e7: lon,
        altitude_ft: alt,
        east_mmps: east,
        north_mmps: north,
        vertical_rate_fpm: vertical,
    })
}

fn parse_u8_sentinel(text: &str, max: u8) -> Result<u8, InputError> {
    if text == "-" {
        return Ok(u8::MAX);
    }
    let value = text.parse::<u8>().map_err(|_| InputError::InvalidInteger)?;
    if value > max {
        return Err(InputError::InvalidInteger);
    }
    Ok(value)
}

fn parse_position_v2(parts: &[&str]) -> Result<EventKind, InputError> {
    if parts.len() != 29 {
        return Err(InputError::BadFieldCount);
    }
    let EventKind::Position {
        latitude_e7,
        longitude_e7,
        altitude_ft,
        east_mmps,
        north_mmps,
        vertical_rate_fpm,
    } = parse_position(parts)?
    else {
        return Err(InputError::BadFieldCount);
    };
    Ok(EventKind::PositionV2 {
        latitude_e7,
        longitude_e7,
        altitude_ft,
        east_mmps,
        north_mmps,
        vertical_rate_fpm,
        evidence: AdsbSourceEvidence {
            source_type: parse_u8_sentinel(part(parts, 15)?, 13)?,
            category: part(parts, 16)?
                .parse()
                .map_err(|_| InputError::InvalidInteger)?,
            version: parse_u8_sentinel(part(parts, 17)?, 7)?,
            nic: parse_u8_sentinel(part(parts, 18)?, 11)?,
            nac_p: parse_u8_sentinel(part(parts, 19)?, 15)?,
            nac_v: parse_u8_sentinel(part(parts, 20)?, 7)?,
            sil: parse_u8_sentinel(part(parts, 21)?, 3)?,
            sil_type: parse_u8_sentinel(part(parts, 22)?, 2)?,
            sda: parse_u8_sentinel(part(parts, 23)?, 3)?,
            gva: parse_u8_sentinel(part(parts, 24)?, 3)?,
            nic_baro: parse_u8_sentinel(part(parts, 25)?, 1)?,
            row_flags: parse_u8_sentinel(part(parts, 26)?, 31)?,
            squawk: parse_squawk(part(parts, 27)?)?,
            callsign: parse_hex_bytes(part(parts, 28)?)?,
        },
    })
}

fn parse_squawk(text: &str) -> Result<u16, InputError> {
    if text == "-" {
        return Ok(u16::MAX);
    }
    if text.len() != 4 || !text.bytes().all(|byte| (b'0'..=b'7').contains(&byte)) {
        return Err(InputError::InvalidInteger);
    }
    text.parse().map_err(|_| InputError::InvalidInteger)
}

fn parse_hex_bytes(text: &str) -> Result<[u8; 8], InputError> {
    if text.len() != 16 || !text.is_ascii() {
        return Err(InputError::BadFieldCount);
    }
    let mut bytes = [0u8; 8];
    for (index, byte) in bytes.iter_mut().enumerate() {
        let start = index * 2;
        *byte = u8::from_str_radix(&text[start..start + 2], 16)
            .map_err(|_| InputError::InvalidInteger)?;
    }
    Ok(bytes)
}

fn parse_flight(parts: &[&str]) -> Result<EventKind, InputError> {
    let op = match part(parts, 9)? {
        "PLAN" if parts.len() == 12 => FlightOperation::SetPlan {
            plan_id: fixed_field::<12>(part(parts, 10)?)?,
            status: part(parts, 11)?
                .parse()
                .map_err(|_| InputError::InvalidInteger)?,
        },
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
        "ADDRESS" if parts.len() == 10 => FlightOperation::SetAddress,
        "IDENTITY" if parts.len() == 13 => FlightOperation::SetIdentity {
            callsign: field::<8>(part(parts, 10)?)?,
            departure: field::<12>(part(parts, 11)?)?,
            arrival: field::<12>(part(parts, 12)?)?,
        },
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

/// Parses one versioned protocol event. The caller checks line length before calling this function.
///
/// Rows use `vN<TAB>receipt_ns<TAB>source_ns<TAB>sequence<TAB>source<TAB>gufi_hex<TAB>address_hex<TAB>centre<TAB>kind...`.
/// Position rows in v1 have 15 fields; v2 appends raw readsb source, category, version, quality,
/// flags, squawk and
/// fixed-width callsign fields. Squawk is its original four decimal digits (each 0–7); callsign
/// is eight source bytes encoded as sixteen hexadecimal characters.
/// Flight and route payloads retain their v1 body shape in either event version.
pub fn parse_event_line(line: &str) -> Result<NormalizedEvent, InputError> {
    if line.len() > MAX_EVENT_LINE_BYTES {
        return Err(InputError::RowTooLong);
    }
    let parts: Vec<&str> = line.split('\t').collect();
    if parts.len() < 9 {
        return Err(InputError::BadFieldCount);
    }
    let version = part(&parts, 0)?;
    if !matches!(version, "v1" | "v2") {
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
    let source = parse_source(part(&parts, 4)?)?;
    let flight_gufi = parse_hex_u128(part(&parts, 5)?)?;
    let aircraft_address = parse_hex_u32(part(&parts, 6)?)?;
    let centre = fixed_field::<4>(part(&parts, 7)?)?;
    let kind_name = part(&parts, 8)?;
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
    let kind = match kind_name {
        "POS" if version == "v1" && parts.len() == 15 => parse_position(&parts)?,
        "POS" if version == "v2" => parse_position_v2(&parts)?,
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
                ("ACK_MSAW", 15) => {
                    let alert_start_ns = parse_i64(part(&parts, 14)?)?;
                    if alert_start_ns <= 0 {
                        return Err(InputError::InvalidInteger);
                    }
                    Intent::AcknowledgeMsaw { alert_start_ns }
                }
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
mod aural_ack_tests {
    use super::*;

    fn ack_line(start_ns: &str) -> String {
        format!(
            "v1\t2000000000\t1000000000\t1\tFLIGHT\t000000000000000000000001\t00ABCDEF\tZAU\tCOMMAND\tPLAN00000001\t1\t1\t42\tACK_MSAW\t{start_ns}"
        )
    }

    #[test]
    fn ack_msaw_parser_retains_positive_alert_instance_start() {
        let parsed = parse_event_line(&ack_line("1500000000")).expect("valid typed ACK");
        let EventKind::Operator(command) = parsed.kind else {
            panic!("operator intent expected");
        };
        assert_eq!(
            command.intent,
            oa_display::interaction::Intent::AcknowledgeMsaw {
                alert_start_ns: 1_500_000_000
            }
        );
    }

    #[test]
    fn ack_msaw_parser_rejects_missing_or_nonpositive_instance_start() {
        assert!(parse_event_line(&ack_line("0")).is_err());
        assert!(parse_event_line(&ack_line("-1")).is_err());
        assert!(parse_event_line(&ack_line("not-a-time")).is_err());
    }
}
