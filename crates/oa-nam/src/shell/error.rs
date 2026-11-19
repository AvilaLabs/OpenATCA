//! The one error type of the codec: every refusal names why, which field, where, and the LRM
//! error code a receiving system would answer with (Rev G Appendix A, Table A-1, and Attachment
//! 2 Table 2-2). Plain data (A1). Shell (input-format adapter, B1, B7, C6): allocates per message, so not real-time core (C1).

/// Why an input or a value was refused. Each variant maps to the Table A-1 code in
/// [`lrm_code`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived: error vocabulary, only on the refusal path
pub enum ErrorKind {
    // --- whole message
    /// Shorter than 17 bytes, longer than 1800, or a frame header disagrees with the length.
    Length,
    /// A byte outside printable ASCII (0x20 to 0x7E) in a message.
    Charset,
    /// The message does not start with `(` or does not end with `)`.
    Parens,
    /// A `(` or `)` inside the message.
    ParensInside,
    /// More fields than the message type has.
    TooManyFields,
    /// Fewer fields than the message type needs, more than one missing.
    TooFewFields,
    /// Exactly one required field is missing; the field number is in [`NamError::field`].
    MissingField,
    /// The three-letter message identifier is not in the core set.
    Mnemonic,
    /// Field 03 is malformed beyond what the next four kinds say (wrong length, extra slash).
    Field03Syntax,
    /// Field 03(b) sending unit is not exactly four letters.
    SendingUnit,
    /// Field 03(b) receiving unit is not exactly four letters.
    ReceivingUnit,
    /// Field 03(b) message number is not three digits.
    MessageNumber,
    /// Field 03(c) reference data is malformed.
    Reference,
    /// Field 03(c) is present where the message type has none, or absent where it is required.
    ReferencePresence,
    // --- field 07
    Acid,
    FunctionalAddress,
    SsrMode,
    SsrCode,
    /// 07(b) and 07(c) present where the message type does not allow them, or absent where it
    /// requires them.
    SsrPresence,
    // --- fields 08, 09, 10
    FlightRules,
    FlightType,
    AircraftCount,
    AircraftType,
    WakeCategory,
    EquipmentComm,
    EquipmentSurveillance,
    EquipmentCommLength,
    EquipmentSurveillanceLength,
    /// A letter repeated in 10(a) (code 71) or 10(b) (code 72).
    EquipmentCommDuplicate,
    EquipmentSurveillanceDuplicate,
    /// A combination of 10(a) values that cannot go together (code 75), or of 10(b) (76).
    EquipmentCommCombination,
    EquipmentSurveillanceCombination,
    /// An ADS-B designator in 10(b) with a bad number (code 65), or ADS-C (66).
    AdsB,
    AdsC,
    /// Items 10 and 18 disagree: `N` with COM/NAV/DAT (64); R, G or Z with nothing in Field
    /// 18 to specify them (91).
    ItemInconsistent,
    ItemUnspecified,
    /// PBN/ in Field 18 without R in 10(a) (86).
    PbnNotInEquipment,
    /// More than eight PBN values (84).
    PbnTooMany,
    // --- fields 13, 16
    DepartureAerodrome,
    DestinationAerodrome,
    AlternateAerodrome,
    TimeInvalid,
    TimeMissing,
    TimeUnexpected,
    AlternateUnexpected,
    // --- field 14
    BoundaryPoint,
    Level,
    LevelMissing,
    SupplementaryLevel,
    SupplementaryLevelMissing,
    CrossingCondition,
    CrossingConditionMissing,
    // --- field 15
    SpeedLevel,
    Speed,
    SpeedMissing,
    RouteElement,
    RoutePoint,
    RouteDesignator,
    FlightRulesIndicator,
    AfterTruncation,
    CruiseClimb,
    // --- fields 18, 22, 31, 32
    OtherInfo,
    DateOfFlight,
    Eet,
    Pbn,
    AmendmentField,
    AmendmentMissing,
    Field31,
    Field32,
    /// LRM text in Field 18 is not `code/field/text`.
    LrmText,
    /// Encode: the message encodes to text that does not parse back to the same value.
    NotCanonical,
    // --- framing
    FrameHeader,
}

/// A refusal with its place. Plain data (A1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct NamError {
    pub kind: ErrorKind,
    /// The field number the error belongs to (3, 7, 14, ...), 0 when it is not field-specific.
    pub field: u8,
    /// Byte offset in the message (or frame) given to the codec, or [`NO_OFFSET`] when the
    /// error was raised on a value (encode, or a field parsed on its own).
    pub offset: usize,
}

/// `offset` value of an error with no input byte to point at.
pub const NO_OFFSET: usize = usize::MAX;

impl NamError {
    pub const fn new(kind: ErrorKind, field: u8) -> NamError {
        NamError {
            kind,
            field,
            offset: NO_OFFSET,
        }
    }

    pub const fn at(mut self, offset: usize) -> NamError {
        self.offset = offset;
        self
    }
}

impl std::fmt::Display for NamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (code, field) = crate::lrm::lrm(self);
        write!(f, "{:?} (LRM {code:02}/{field:02}", self.kind)?;
        if self.offset != NO_OFFSET {
            write!(f, ", byte {}", self.offset)?;
        }
        write!(f, ")")
    }
}

impl std::error::Error for NamError {}

/// Table A-1 code and field number for a refusal. Codes 51 and 54 carry the field number of
/// the field at fault, so their field here is 0 and [`crate::lrm::lrm`] fills it in.
pub const fn lrm_code(kind: ErrorKind) -> (u8, u8) {
    use ErrorKind::*;
    match kind {
        Length | FrameHeader => (55, 0),
        Charset | ParensInside | Field03Syntax | ReferencePresence | LrmText | NotCanonical => {
            (57, 0)
        }
        Parens => (58, 0),
        TooManyFields => (53, 0),
        TooFewFields => (52, 0),
        MissingField => (51, 0),
        Mnemonic => (60, 3),
        SendingUnit => (1, 3),
        ReceivingUnit => (2, 3),
        MessageNumber => (4, 3),
        Reference => (5, 3),
        Acid => (6, 7),
        FunctionalAddress => (8, 7),
        SsrMode => (9, 7),
        SsrCode => (10, 7),
        SsrPresence => (54, 7),
        FlightRules => (11, 8),
        FlightType => (12, 8),
        AircraftCount => (54, 9),
        AircraftType => (13, 9),
        WakeCategory => (14, 9),
        EquipmentComm => (73, 10),
        EquipmentCommDuplicate => (71, 10),
        EquipmentSurveillanceDuplicate => (72, 10),
        EquipmentCommCombination => (75, 10),
        EquipmentSurveillanceCombination => (76, 10),
        AdsB => (65, 10),
        AdsC => (66, 10),
        ItemInconsistent => (64, 10),
        ItemUnspecified => (91, 10),
        PbnNotInEquipment => (86, 18),
        PbnTooMany => (84, 18),
        EquipmentSurveillance => (74, 10),
        EquipmentCommLength => (78, 10),
        EquipmentSurveillanceLength => (79, 10),
        DepartureAerodrome => (18, 13),
        DestinationAerodrome => (19, 16),
        AlternateAerodrome => (17, 16),
        TimeInvalid => (23, 0),
        TimeMissing => (24, 0),
        TimeUnexpected => (22, 0),
        AlternateUnexpected => (87, 16),
        BoundaryPoint => (25, 14),
        Level => (29, 0),
        LevelMissing => (30, 0),
        SupplementaryLevel => (32, 14),
        SupplementaryLevelMissing => (33, 14),
        CrossingCondition => (34, 14),
        CrossingConditionMissing => (35, 14),
        SpeedLevel => (36, 15),
        Speed => (38, 15),
        SpeedMissing => (39, 15),
        RouteElement => (40, 15),
        RoutePoint => (43, 15),
        RouteDesignator => (41, 15),
        FlightRulesIndicator => (44, 15),
        AfterTruncation => (45, 15),
        CruiseClimb => (46, 15),
        OtherInfo => (48, 18),
        DateOfFlight => (63, 18),
        Eet => (85, 18),
        Pbn => (77, 18),
        AmendmentField => (50, 22),
        AmendmentMissing => (51, 22),
        Field31 | Field32 => (54, 0),
    }
}
