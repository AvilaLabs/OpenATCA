//! Strict line parser for the bounded normalized-input protocol. Shell (B1, B3, B7).
//!
//! This is an explicit normalized-input adapter, not a raw FAA recording reader. The reader
//! rejects malformed rows whole and never repairs values. Input and row bounds are checked
//! before parsing or storing data (RULESET C6, C7, E5, E6, E8).

use oa_flight::FlightKey;

/// Maximum bytes in one protocol line, excluding its line ending.
pub const MAX_EVENT_LINE_BYTES: usize = 768;

/// Returns the normalized row version declared by a supported stream header.
pub fn stream_row_version(header: &str) -> Option<&'static str> {
    match header {
        "OA-RS\t1" => Some("v1"),
        "OA-RS\t2" => Some("v2"),
        _ => None,
    }
}

/// Rejects a known v1/v2 row whose version conflicts with the stream header.
/// Malformed prefixes remain the row parser's responsibility for stable error accounting.
pub fn row_version_conflicts_with_stream(line: &str, version: &str) -> bool {
    line.split_once('\t').is_some_and(|(row_version, _)| {
        matches!(row_version, "v1" | "v2") && row_version != version
    })
}
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
    /// Native oa-flight lifecycle operations retained by OA-RS v2.
    Lifecycle(FlightLifecycle),
    /// Native altitude specification, including non-simple forms and at-fix metadata.
    AssignedAltitude(FlightAltitude),
    RequestedAltitude(FlightAltitude),
    /// Native handoff events retain both units and distinguish update from retract.
    HandoffEvent(FlightHandoff),
}
// A7.2: cold enum, size guard waived; one normalized operation.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlightLifecycle {
    Depart,
    Complete,
    Cancel,
    Drop,
}

/// Native oa-flight altitude shape. Numeric values are feet; `AtFix` keeps the source fix key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlightAltitude {
    Simple(i32),
    Vfr,
    VfrOnTopPlus(i32),
    Block {
        above_ft: i32,
        below_ft: i32,
    },
    AtFix {
        before_ft: i32,
        after_ft: i32,
        fix: [u8; 12],
    },
    Nil,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlightHandoff {
    Initiate { from: [u8; 8], to: [u8; 8] },
    Update { to: [u8; 8] },
    Accept { by: [u8; 8] },
    Take { by: [u8; 8] },
    Retract,
    Fail,
}

/// Handoff state operation. Only `Take` transfers ownership.
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

fn centre_field(text: &str) -> Result<[u8; 4], InputError> {
    if text.is_empty()
        || text.len() > 4
        || !text.is_ascii()
        || !text.bytes().all(|byte| byte.is_ascii_alphanumeric())
    {
        return Err(InputError::InvalidLabel);
    }
    let mut out = [0u8; 4];
    let prefix = out.get_mut(..text.len()).ok_or(InputError::InvalidLabel)?;
    prefix.copy_from_slice(text.as_bytes());
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

fn parse_flight(parts: &[&str], version: &str) -> Result<EventKind, InputError> {
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
        _ if version == "v2" => parse_flight_v2(parts)?,
        _ => return Err(InputError::InvalidOperation),
    };
    Ok(EventKind::Flight(op))
}

fn parse_flight_v2(parts: &[&str]) -> Result<FlightOperation, InputError> {
    use FlightAltitude as A;
    use FlightHandoff as H;
    use FlightLifecycle as L;
    let operation = part(parts, 9)?;
    let altitude = |form: &str, a: &str, b: &str, fix: &str| -> Result<A, InputError> {
        Ok(match form {
            "SIMPLE" if b == "0" && fix == "-" => A::Simple(parse_i32(a)?),
            "VFR" if a == "0" && b == "0" && fix == "-" => A::Vfr,
            "VFR_PLUS" if b == "0" && fix == "-" => A::VfrOnTopPlus(parse_i32(a)?),
            "BLOCK" if fix == "-" => A::Block {
                above_ft: parse_i32(a)?,
                below_ft: parse_i32(b)?,
            },
            "AT_FIX" if fix != "-" => {
                let fix = field::<12>(fix)?;
                if fix.iter().all(|byte| matches!(*byte, 0 | b' '))
                    || !fix
                        .iter()
                        .all(|byte| *byte == 0 || (32..=126).contains(byte))
                {
                    return Err(InputError::InvalidLabel);
                }
                A::AtFix {
                    before_ft: parse_i32(a)?,
                    after_ft: parse_i32(b)?,
                    fix,
                }
            }
            "NIL" if a == "0" && b == "0" && fix == "-" => A::Nil,
            _ => return Err(InputError::InvalidOperation),
        })
    };
    let parse_alt = || -> Result<A, InputError> {
        if parts.len() != 14 {
            return Err(InputError::BadFieldCount);
        }
        altitude(
            part(parts, 10)?,
            part(parts, 11)?,
            part(parts, 12)?,
            part(parts, 13)?,
        )
    };
    Ok(match operation {
        "DEPART" if parts.len() == 10 => FlightOperation::Lifecycle(L::Depart),
        "COMPLETE" if parts.len() == 10 => FlightOperation::Lifecycle(L::Complete),
        "CANCEL" if parts.len() == 10 => FlightOperation::Lifecycle(L::Cancel),
        "DROP" if parts.len() == 10 => FlightOperation::Lifecycle(L::Drop),
        "ASSIGNED_ALT" => FlightOperation::AssignedAltitude(parse_alt()?),
        "INTERIM_ALT" => {
            let value = parse_alt()?;
            if !matches!(value, A::Simple(_) | A::Nil) {
                return Err(InputError::InvalidOperation);
            }
            FlightOperation::SetInterimAltitude {
                altitude_ft: match value {
                    A::Simple(altitude_ft) => Some(altitude_ft),
                    A::Nil => None,
                    _ => return Err(InputError::InvalidOperation),
                },
            }
        }
        "REQUESTED_ALT" => {
            let value = parse_alt()?;
            if !matches!(value, A::Simple(_) | A::Vfr | A::VfrOnTopPlus(_)) {
                return Err(InputError::InvalidOperation);
            }
            FlightOperation::RequestedAltitude(value)
        }
        "HO_INITIATE" if parts.len() == 12 => FlightOperation::HandoffEvent(H::Initiate {
            from: field::<8>(part(parts, 10)?)?,
            to: field::<8>(part(parts, 11)?)?,
        }),
        "HO_UPDATE" if parts.len() == 11 => FlightOperation::HandoffEvent(H::Update {
            to: field::<8>(part(parts, 10)?)?,
        }),
        "HO_UPDATE_LEGACY" if parts.len() == 11 => FlightOperation::Handoff {
            operation: HandoffOperation::Update,
            unit: field::<8>(part(parts, 10)?)?,
        },
        "HO_ACCEPT_NATIVE" if parts.len() == 11 => FlightOperation::HandoffEvent(H::Accept {
            by: field::<8>(part(parts, 10)?)?,
        }),
        "HO_TAKE_NATIVE" if parts.len() == 11 => FlightOperation::HandoffEvent(H::Take {
            by: field::<8>(part(parts, 10)?)?,
        }),
        "HO_RETRACT" if parts.len() == 10 => FlightOperation::HandoffEvent(H::Retract),
        "HO_FAIL_NATIVE" if parts.len() == 10 => FlightOperation::HandoffEvent(H::Fail),
        _ => return Err(InputError::InvalidOperation),
    })
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
/// Position rows have 15 fields; the event type selects the remaining fields:
/// `v1` or `v2<TAB>receipt_ns<TAB>source_ns<TAB>sequence<TAB>source<TAB>gufi_hex<TAB>address_hex<TAB>centre<TAB>kind...`.
/// OA-RS v2 adds lossless native flight-operation forms; v1 retains its original grammar.
/// `centre` is canonical one-to-four-character ASCII alphanumeric text; internal storage zero-pads
/// it to the fixed four-byte source identity.
/// Position rows append `lat<TAB>lon<TAB>alt-or--<TAB>east-mmps<TAB>north-mmps<TAB>vertical-rate-or--`;
/// flight and route payloads use the field counts checked below.
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
    let centre = centre_field(part(&parts, 7)?)?;
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
        "POS" if parts.len() == 15 => parse_position(&parts)?,
        "FLIGHT" => parse_flight(&parts, version)?,
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
        source,
        identity: Identity {
            flight_gufi,
            aircraft_address,
        },
        centre,
        kind,
    })
}
