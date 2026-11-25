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
