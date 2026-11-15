//! The fields of the metering publication, the ways a field can be refused and the whole-message
//! errors. Plain data. Core (pure).

use crate::time::TimeError;
use oa_xml::XmlError;

/// Every attribute and element the decoder reads, for error attribution. The discriminant
/// indexes [`crate::Stats::field_errors`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Field {
    EnvSrce,
    EnvTime,
    MsgTime,
    MsgId,
    TmaId,
    Gufi,
    Cid,
    Aid,
    AirType,
    Apt,
    Dap,
    MrpType,
    Tra,
    Mfx,
    Dfx,
    Sfx,
    Oma,
    Ooa,
    O3a,
    O4a,
    Gat,
    Rwy,
    Cfg,
    Cat,
    Scn,
    Man,
    Rfz,
    Sfz,
    Sus,
    Fps,
    Acs,
    Typ,
    Eng,
    Bcn,
    Spd,
    Ara,
    Ina,
    Trw,
    Drw,
    Tds,
    Cfx,
    Ctm,
    Etd,
    Std,
    EtaDfx,
    EtaSfx,
    EtaMfx,
    EtaOma,
    EtaOoa,
    EtaO3a,
    EtaO4a,
    EtaRwy,
    StaDfx,
    StaSfx,
    StaMfx,
    StaOma,
    StaOoa,
    StaO3a,
    StaO4a,
    StaRwy,
}

/// Number of [`Field`] values.
pub const FIELD_COUNT: usize = 60;

/// The wire name of a field (`eta_mfx`, `envSrce`).
pub fn field_name(f: Field) -> &'static str {
    const NAMES: [&str; FIELD_COUNT] = [
        "envSrce", "envTime", "msgTime", "msgId", "tmaId", "gufi", "cid", "aid", "airType", "apt",
        "dap", "mrpType", "tra", "mfx", "dfx", "sfx", "oma", "ooa", "o3a", "o4a", "gat", "rwy",
        "cfg", "cat", "scn", "man", "rfz", "sfz", "sus", "fps", "acs", "typ", "eng", "bcn", "spd",
        "ara", "ina", "trw", "drw", "tds", "cfx", "ctm", "etd", "std", "eta_dfx", "eta_sfx",
        "eta_mfx", "eta_oma", "eta_ooa", "eta_o3a", "eta_o4a", "eta_rwy", "sta_dfx", "sta_sfx",
        "sta_mfx", "sta_oma", "sta_ooa", "sta_o3a", "sta_o4a", "sta_rwy",
    ];
    NAMES.get(f as usize).copied().unwrap_or("?")
}

/// Why one field was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldErrorKind {
    /// A required attribute is absent.
    Missing,
    /// A name longer than its column.
    TooLong,
    /// A byte that is not printable ASCII (or not in the field's alphabet).
    BadChar,
    /// A value that does not have the field's shape (envSrce, a number, a transponder code).
    Format,
    /// A malformed timestamp.
    Time(TimeError),
    /// A value outside the field's vocabulary (a change type, a YES/NO flag).
    Vocabulary,
    /// The same field twice in one element.
    Duplicate,
    /// Two copies of one value disagree (`flt` repeating `air`'s `aid`, `apt` or `dap`).
    Inconsistent,
    /// A value element that has child elements.
    NotLeaf,
    /// Text that is split over several nodes, or whose references do not decode.
    Text,
}

/// A refused field: which, why, and a byte offset in the message: the value of a refused
/// attribute, the name of a refused element.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FieldError {
    pub field: Field,
    pub kind: FieldErrorKind,
    pub at: u32,
}

/// Why a whole message was refused (it yields no rows). A refused field no longer refuses the
/// message: the `air` (or `tma`) is dropped and counted in [`crate::Outcome::rejected`]; `Field`
/// is the reason carried by [`crate::Outcome::first_reject`] and by a refused envelope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocError {
    /// The XML parser refused the bytes (only from [`crate::decode_bytes`]).
    Xml(XmlError),
    /// The root is not a TBFM metering publication element (`env` or `mis`) in the 1.1.0
    /// namespace.
    NotTbfm { at: u32 },
    /// A field was refused.
    Field(FieldError),
    /// The message has more rows than the caller's [`crate::Rows`] can hold.
    Capacity,
}

/// Reject reasons for statistics: the index into [`crate::Stats::rejects`].
pub const REJECT_XML: usize = 0;
pub const REJECT_NOT_TBFM: usize = 1;
pub const REJECT_FIELD: usize = 2;
pub const REJECT_CAPACITY: usize = 3;
pub const REJECT_COUNT: usize = 4;

/// Display names of the reject reasons.
pub const REJECT_NAMES: [&str; REJECT_COUNT] = ["xml", "not-tbfm", "field", "capacity"];

/// The statistics index of an error.
pub fn reject_reason(e: &DocError) -> usize {
    match e {
        DocError::Xml(_) => REJECT_XML,
        DocError::NotTbfm { .. } => REJECT_NOT_TBFM,
        DocError::Field(_) => REJECT_FIELD,
        DocError::Capacity => REJECT_CAPACITY,
    }
}

/// What a message turned out to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MsgKind {
    /// An `env` with at least one `air` (flight metering data).
    Air,
    /// An `env` whose `tma` holds adaptation (`adp`) data.
    Adaptation,
    /// An `env` whose `tma` holds control (`con`) data: rates, configurations, spacing.
    Control,
    /// An `env` whose `tma` holds other (`oth`) data: interface status, metering status.
    Other,
    /// An `env` with no `tma`, or a `tma` with nothing the decoder knows.
    Empty,
    /// A `mis` heartbeat (`hb`).
    Heartbeat,
    /// A `mis` sync marker (`sync`).
    Sync,
    /// A `mis` with neither child.
    MisOther,
}

/// Number of [`MsgKind`] values.
pub const MSG_KIND_COUNT: usize = 8;

/// Display names of the message kinds, in [`MsgKind`] order.
pub const MSG_KIND_NAMES: [&str; MSG_KIND_COUNT] = [
    "air",
    "adaptation",
    "control",
    "other",
    "empty",
    "heartbeat",
    "sync",
    "mis-other",
];

/// Number of [`FieldErrorKind`] values.
pub const ERROR_KIND_COUNT: usize = 10;

/// The index of a field error kind (for per-reason counts), in declaration order.
pub fn error_kind_index(k: FieldErrorKind) -> usize {
    match k {
        FieldErrorKind::Missing => 0,
        FieldErrorKind::TooLong => 1,
        FieldErrorKind::BadChar => 2,
        FieldErrorKind::Format => 3,
        FieldErrorKind::Time(_) => 4,
        FieldErrorKind::Vocabulary => 5,
        FieldErrorKind::Duplicate => 6,
        FieldErrorKind::Inconsistent => 7,
        FieldErrorKind::NotLeaf => 8,
        FieldErrorKind::Text => 9,
    }
}

/// Display names of the field error kinds, for reports.
pub fn error_kind_name(k: FieldErrorKind) -> &'static str {
    match k {
        FieldErrorKind::Missing => "missing",
        FieldErrorKind::TooLong => "too-long",
        FieldErrorKind::BadChar => "bad-char",
        FieldErrorKind::Format => "format",
        FieldErrorKind::Time(_) => "time",
        FieldErrorKind::Vocabulary => "vocabulary",
        FieldErrorKind::Duplicate => "duplicate",
        FieldErrorKind::Inconsistent => "inconsistent",
        FieldErrorKind::NotLeaf => "not-leaf",
        FieldErrorKind::Text => "text",
    }
}
