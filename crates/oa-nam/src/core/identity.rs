//! The plain-value identity of a NAM message: [`Kind`], [`Facility`], [`MsgId`], [`Header`] and
//! the validation [`Profile`]. Core (pure, B1): `Copy` data with no allocation, split out of
//! the codec's message model so the session core names nothing in the shell (B7).

/// The NAM coordination messages and the ICAO standard DLA field message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Fpl,
    Dla,
    Chg,
    Est,
    Cnl,
    Cpl,
    Mod,
    Abi,
    Mis,
    Irq,
    Irs,
    Trq,
    Trs,
    Asm,
    Rti,
    Rtu,
    Rla,
    Rta,
    Poi,
    Pla,
    Poa,
    Poj,
    Toc,
    Aoc,
    Lam,
    Lrm,
}
// Budget: a one-byte discriminant; carried in every Header, Pending and Action.
const _: () = assert!(core::mem::size_of::<Kind>() == 1);

/// Every [`Kind`] supported by the field codec.
pub const ALL_KINDS: [Kind; 26] = [
    Kind::Fpl,
    Kind::Dla,
    Kind::Chg,
    Kind::Est,
    Kind::Cnl,
    Kind::Cpl,
    Kind::Mod,
    Kind::Abi,
    Kind::Mis,
    Kind::Irq,
    Kind::Irs,
    Kind::Trq,
    Kind::Trs,
    Kind::Asm,
    Kind::Rti,
    Kind::Rtu,
    Kind::Rla,
    Kind::Rta,
    Kind::Poi,
    Kind::Pla,
    Kind::Poa,
    Kind::Poj,
    Kind::Toc,
    Kind::Aoc,
    Kind::Lam,
    Kind::Lrm,
];

/// A facility designator: exactly four letters (Part I 3.8, Part II 2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Facility(pub [u8; 4]);
// Budget: four ASCII letters; carried in every MsgId.
const _: () = assert!(core::mem::size_of::<Facility>() == 4);

/// Sending unit, receiving unit and message number: the `KZMP/CZWG812` of Field 03(b), and the
/// same triple as the reference data of 03(c).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct MsgId {
    pub sender: Facility,
    pub receiver: Facility,
    /// 0 to 999 (B.2.1: the number after 999 is 000).
    pub number: u16,
}
// Budget: 4 + 4 + 2 bytes, align 2; carried in every Header, Action and pending lookup.
const _: () = assert!(core::mem::size_of::<MsgId>() == 10);

/// Field 03: message type, number and reference data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct Header {
    pub kind: Kind,
    pub id: MsgId,
    pub reference: Option<MsgId>,
}
// Budget: kind (1) + pad (1) + MsgId (10) + Option<MsgId> (12, tagged); carried by every Event and
// by the Deliver and Response actions of the session.
const _: () = assert!(core::mem::size_of::<Header>() == 24);

/// Which rule set a message is validated against.
///
/// `Core` is Part II of NAS-IC-21009205 Rev G alone. `EramAttachment2` adds what the ERAM to
/// NAV CANADA interface agrees in Attachment 2 (the other attachments differ per partner):
/// block altitudes in Field 14 (2.6), unknown ground speed and heading in Field 32 (2.15), and
/// the CPDLC contact data in Field 31(c) of an RTA, up to 28 characters (2.17); the session
/// repeats an IRQ with a new number (2.10, see [`crate::session::Config::for_profile`]) and
/// the LRM codes of Attachment 2 Table 2-2 apply ([`crate::lrm::lrm_for`]).
///
///
/// `RevE` is Part II of Rev E (15 April 2016) alone, the revision a 2023 ICAO NACC paper reports
/// the US operating: Core with the four message definitions of Rev E (ASM 03(c), no POI 03(c),
/// POA 07 a, b, c, POJ a Field 18) and metric levels allowed in Field 32(e). POA and POJ expect
/// no response in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived: a parsed message value, built per message in the codec, never held by the session
pub enum Profile {
    Core,
    EramAttachment2,
    RevE,
}

impl Default for Profile {
    /// The target partner is ERAM.
    fn default() -> Self {
        Profile::EramAttachment2
    }
}
