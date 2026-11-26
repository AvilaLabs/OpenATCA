//! Stable LRM catalog for semantic Item 15 resolution refusals. The caller resolves parsed
//! values against its current route model; this module owns only the NAM refusal-to-LRM mapping.
use crate::error::{ErrorKind, NamError};
use crate::field::LrmText;
use crate::lrm;
use crate::model::{
    AmendData, Field13, Field15, Field16, Header, Indicator, Kind, Message, MsgId, Profile,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item15Reject {
    MissingItem15,
    DuplicateItem15Amendment,
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

impl Item15Reject {
    pub const fn error(self) -> NamError {
        let kind = match self {
            Self::MissingItem15
            | Self::MalformedGroup
            | Self::Discontinuity
            | Self::Capacity
            | Self::Model => ErrorKind::RouteElement,
            Self::DuplicateItem15Amendment => ErrorKind::AmendmentField,
            Self::UnknownPoint
            | Self::AmbiguousPoint
            | Self::MissingVariation
            | Self::InvalidVariation => ErrorKind::RoutePoint,
            Self::UnknownAirway | Self::AirwayDoesNotContainPoint | Self::UnsupportedProcedure => {
                ErrorKind::RouteDesignator
            }
            Self::MissingRouteLevel => ErrorKind::LevelMissing,
            Self::AltitudeBelowMea => ErrorKind::Level,
        };
        let field = match self {
            Self::DuplicateItem15Amendment => 22,
            _ => 15,
        };
        NamError::new(kind, field)
    }

    pub fn lrm(self, profile: Profile, received_kind: Kind) -> (u8, u8) {
        lrm::lrm_for(&self.error(), profile, Some(received_kind))
    }

    pub const fn explanation(self) -> &'static str {
        match self {
            Self::MissingItem15 => "FPL HAS NO ITEM 15",
            Self::DuplicateItem15Amendment => "MULTIPLE ITEM 15 AMENDMENTS",
            Self::MalformedGroup => "MALFORMED ITEM 15 ROUTE GROUP",
            Self::UnknownPoint => "UNKNOWN SIGNIFICANT POINT",
            Self::AmbiguousPoint => "AMBIGUOUS SIGNIFICANT POINT IDENTITY",
            Self::UnknownAirway => "UNKNOWN ATS ROUTE DESIGNATOR",
            Self::AirwayDoesNotContainPoint => "AIRWAY DOES NOT CONTAIN FILED ENTRY OR EXIT POINT",
            Self::Discontinuity => "ROUTE DISCONTINUITY OR CODED GAP",
            Self::UnsupportedProcedure => "PROCEDURE OR TRANSITION NOT UNIQUELY RESOLVED",
            Self::MissingVariation => {
                "SOURCE MAGNETIC VARIATION REQUIRED FOR BEARING-DISTANCE POINT"
            }
            Self::InvalidVariation => "SOURCE MAGNETIC VARIATION INVALID OR CONFLICTING",
            Self::MissingRouteLevel => "NUMERIC CRUISING LEVEL REQUIRED FOR AIRWAY MEA CHECK",
            Self::AltitudeBelowMea => {
                "FILED LEVEL DOES NOT SATISFY AIRWAY MINIMUM ENROUTE ALTITUDE"
            }
            Self::Capacity => "ROUTE RESOLUTION WORK LIMIT EXCEEDED",
            Self::Model => "CURRENT NASR/CIFP MODEL IS INCONSISTENT",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FiledChange<'a> {
    Replace(&'a Field15),
    Cancel,
    Delay {
        departure: &'a Field13,
        destination: &'a Field16,
    },
    NoRouteChange,
}

/// Item 15 effects of the four flight-plan messages. DLA is delay-only; CNL cancels; FPL
/// replaces; CHG replaces only when its amendments contain Field 15.
pub fn filed_change(message: &Message) -> Result<FiledChange<'_>, Item15Reject> {
    match message.header.kind {
        Kind::Fpl => message
            .f15
            .as_ref()
            .map(FiledChange::Replace)
            .ok_or(Item15Reject::MissingItem15),
        Kind::Chg => {
            let mut field15 = None;
            for amendment in &message.f22 {
                if let AmendData::F15(field) = &amendment.data {
                    if field15.replace(field).is_some() {
                        return Err(Item15Reject::DuplicateItem15Amendment);
                    }
                }
            }
            Ok(field15
                .map(FiledChange::Replace)
                .unwrap_or(FiledChange::NoRouteChange))
        }
        Kind::Dla => Ok(FiledChange::Delay {
            departure: message.f13.as_ref().ok_or(Item15Reject::MalformedGroup)?,
            destination: message.f16.as_ref().ok_or(Item15Reject::MalformedGroup)?,
        }),
        Kind::Cnl => Ok(FiledChange::Cancel),
        _ => Ok(FiledChange::NoRouteChange),
    }
}

/// Create a correlated LRM for a semantic route refusal. `profile` must be the active NAM
/// interface profile; the received message kind selects any message-specific LRM variant.
pub fn lrm_reply(
    received: &Message,
    profile: Profile,
    response_number: u16,
    refusal: Item15Reject,
) -> Result<Message, Item15Reject> {
    if response_number > 999 {
        return Err(Item15Reject::MalformedGroup);
    }
    let (code, field) = refusal.lrm(profile, received.header.kind);
    let entry = lrm::lrm_entry(code).ok_or(Item15Reject::Model)?;
    let header = Header {
        kind: Kind::Lrm,
        id: MsgId {
            sender: received.header.id.receiver,
            receiver: received.header.id.sender,
            number: response_number,
        },
        reference: Some(received.header.id),
    };
    let mut reply = crate::model::bare(header);
    reply.f18 = Some(crate::model::Field18::Items(vec![Indicator {
        name: "RMK".to_owned(),
        text: crate::field::fmt_lrm_text(&LrmText {
            code,
            field,
            text: entry.text.to_owned(),
        }),
    }]));
    Ok(reply)
}
