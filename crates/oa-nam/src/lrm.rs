//! LRM error codes: Rev G Appendix A Table A-1 (codes 1 to 91), Attachment 3 2.8 (92 to 94,
//! the Cuba/US interface) and Attachment 2 Table 2-2 (99 to 116, the radar handoff codes of the
//! Canada/US interface). Data only.
//! Shell (input-format adapter, B1, C6): allocates per message, so not real-time core (C1).

use crate::error::{lrm_code, ErrorKind, NamError};
use crate::model::{Kind, Profile};

/// One row of an error code table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived: constant code table, looked up on the refusal path
pub struct LrmEntry {
    pub code: u8,
    /// The "Field Number" column as printed: `Header`, `07`, `13, 16`, ..., or empty.
    pub field: &'static str,
    /// The "Supporting Text" column.
    pub text: &'static str,
    /// Where the row is defined.
    pub source: &'static str,
}

const A: &str = "Appendix A Table A-1";
const C: &str = "Attachment 2 Table 2-2";

macro_rules! row {
    ($c:expr, $f:expr, $t:expr, $s:expr) => {
        LrmEntry {
            code: $c,
            field: $f,
            text: $t,
            source: $s,
        }
    };
}

pub const LRM_TABLE: [LrmEntry; 112] = [
    row!(1, "Header", "INVALID SENDING UNIT", A),
    row!(2, "Header", "INVALID RECEIVING UNIT", A),
    row!(3, "Header", "INVALID TIME STAMP", A),
    row!(4, "Header", "INVALID MESSAGE ID", A),
    row!(5, "Header", "INVALID REFERENCE ID", A),
    row!(6, "07", "INVALID ACID", A),
    row!(7, "07", "DUPLICATE ACID", A),
    row!(8, "07", "UNKNOWN FUNCTIONAL ADDRESS", A),
    row!(9, "07", "INVALID SSR MODE", A),
    row!(10, "07", "INVALID SSR CODE", A),
    row!(11, "08", "INVALID FLIGHT RULES", A),
    row!(12, "08", "INVALID FLIGHT TYPE", A),
    row!(13, "09", "INVALID AIRCRAFT MODEL", A),
    row!(14, "09", "INVALID WAKE TURBULENCE CATEGORY", A),
    row!(15, "10", "INVALID CNA EQUIPMENT DESIGNATOR", A),
    row!(16, "10", "INVALID SSR EQUIPMENT DESIGNATOR", A),
    row!(17, "13, 16", "INVALID AERODROME DESIGNATOR", A),
    row!(18, "13", "INVALID DEPARTURE AERODROME", A),
    row!(19, "16", "INVALID DESTINATION AERODROME", A),
    row!(20, "17", "INVALID ARRIVAL AERODROME", A),
    row!(21, "13, 16", "EXPECTED TIME DESIGNATOR NOT FOUND", A),
    row!(22, "13, 16", "TIME DESIGNATOR PRESENT WHEN NOT EXPECTED", A),
    row!(23, "13, 14, 16", "INVALID TIME DESIGNATOR", A),
    row!(24, "13, 14, 16", "MISSING TIME DESIGNATOR", A),
    row!(25, "14", "INVALID BOUNDARY POINT DESIGNATOR", A),
    row!(26, "14, 15", "INVALID ENROUTE POINT", A),
    row!(27, "14, 15", "INVALID LAT/LON DESIGNATOR", A),
    row!(28, "14, 15", "INVALID NAVAID FIX", A),
    row!(29, "14, 15", "INVALID LEVEL DESIGNATOR", A),
    row!(30, "14, 15", "MISSING LEVEL DESIGNATOR", A),
    row!(31, "14", "INVALID SUPPLEMENTARY CROSSING DATA", A),
    row!(32, "14", "INVALID SUPPLEMENTARY CROSSING LEVEL", A),
    row!(33, "14", "MISSING SUPPLEMENTARY CROSSING LEVEL", A),
    row!(34, "14", "INVALID CROSSING CONDITION", A),
    row!(35, "14", "MISSING CROSSING CONDITION", A),
    row!(36, "15", "INVALID SPEED/LEVEL DESIGNATOR", A),
    row!(37, "15", "MISSING SPEED/LEVEL DESIGNATOR", A),
    row!(38, "15", "INVALID SPEED DESIGNATOR", A),
    row!(39, "15", "MISSING SPEED DESIGNATOR", A),
    row!(40, "15", "INVALID ROUTE ELEMENT DESIGNATOR", A),
    row!(
        41,
        "15",
        "INVALID ATS ROUTE/SIGNIFICANT POINT DESIGNATOR",
        A
    ),
    row!(42, "15", "INVALID ATS ROUTE DESIGNATOR", A),
    row!(43, "15", "INVALID SIGNFICANT POINT DESIGNATOR", A),
    row!(
        44,
        "15",
        "FLIGHT RULES INDICATOR DOES NOT FOLLOW SIGNIFICANT POINT",
        A
    ),
    row!(45, "15", "ADDITIONAL DATA FOLLOWS TRUNCATION INDICATOR", A),
    row!(46, "15", "INCORRECT CRUISE CLIMB FORMAT", A),
    row!(47, "15", "CONFLICTING DIRECTION", A),
    row!(48, "18", "INVALID OTHER INFORMATION ELEMENT", A),
    row!(49, "19", "INVALID SUPPLEMENTARY INFORMATION ELEMENT", A),
    row!(50, "22", "INVALID AMENDMENT FIELD DATA", A),
    row!(
        51,
        "Two numerics (field indicator if present)",
        "MISSING FIELD (two numerics)",
        A
    ),
    row!(52, "", "MORE THAN ONE FIELD MISSING", A),
    row!(53, "", "MESSAGE LOGICALLY TOO LONG", A),
    row!(54, "", "SYNTAX ERROR IN FIELD nn", A),
    row!(55, "", "INVALID MESSAGE LENGTH", A),
    row!(56, "", "NAT ERRORS", A),
    row!(57, "", "INVALID MESSAGE", A),
    row!(58, "", "MISSING PARENTHESIS", A),
    row!(59, "", "MESSAGE NOT APPLICABLE TO zzzz ACC", A),
    row!(
        60,
        "03",
        "INVALID MESSAGE MNEMONIC (i.e., 3 LETTER IDENTIFIER)",
        A
    ),
    row!(61, "Header", "INVALID CRC", A),
    row!(62, "", "MESSAGE REJECTED, MANUAL COORDINATION REQUIRED", A),
    row!(63, "", "INVALID DATE OF FLIGHT", A),
    row!(64, "", "INCONSISTENT ITEM 10 AND 18", A),
    row!(65, "", "INVALID ADS -B EQUIPMENT DESIGNATOR", A),
    row!(66, "", "INVALID ADS -C EQUIPMENT DESIGNATOR", A),
    row!(67, "", "ICAO FORMAT MIXED IN MESSAGE", A),
    row!(68, "", "MUST ENTER NEW ICAO FORMAT", A),
    row!(
        69,
        "",
        "Deleted NAM Version 'E'-MUST ENTER OLD ICAO FORMAT",
        A
    ),
    row!(70, "", "ICAO FORMAT CANNOT BE CHANGED", A),
    row!(71, "10", "DUPLICATE EQP EQUIP DESIGNATOR", A),
    row!(72, "10", "DUPLICATE SRV EQUIP DESIGNATOR", A),
    row!(73, "10", "INVALID EQP EQUIP DESIGNATOR", A),
    row!(74, "10", "INVALID SRV EQUIP DESIGNATOR", A),
    row!(75, "10", "INVALID EQP EQUIP DESIGNATOR COMBINATION", A),
    row!(76, "10", "INVALID SRV EQUIP DESIGNATOR COMBINATION", A),
    row!(77, "18", "INVALID PBN Data", A),
    row!(78, "10a", "EQP EQUIP DESIGNATOR EXCEEDS 50 CHR", A),
    row!(79, "10b", "SRV EQUIP DESIGNATOR EXCEEDS 20 CHR", A),
    row!(80, "13/18", "DEP/ NOT FOUND FOR ZZZZ", A),
    row!(81, "15/10a", "NO RVSM STATUS", A),
    row!(82, "16/18", "DEST/ NOT FOUND FOR ZZZZ", A),
    row!(83, "18", "INVALID ELEMENT ON STS/", A),
    row!(84, "18", "PBN/ VALUE EXCEEDED OR INVALID (MORE THAN 8)", A),
    row!(85, "18", "INVALID EET DATA", A),
    row!(86, "18", "PBN VALUE INCONSISTENT WITH ITEM 10", A),
    row!(87, "16", "ALTERNATE AERODROME PRESENT WHEN NOT EXPECTED", A),
    row!(88, "", "FPL IS ACTIVE", A),
    row!(89, "", "Unused", A),
    row!(90, "9/18", "TYP/ NOT FOUND FOR ZZZZ", A),
    row!(91, "10a/18", "INCONSISTENT ITEM 10 and 18", A),
    row!(92, "16", "INVALID ALTERNATIVE AIRPORT", "Attachment 3 2.8"),
    row!(93, "16", "MISSING ALTERNATIVE AIRPORT", "Attachment 3 2.8"),
    row!(94, "16", "ALTN/ NOT FOUND FOR ZZZZ", "Attachment 3 2.8"),
    row!(99, "", "RECEIVING SECTOR ID NON EXISTENT", C),
    row!(100, "", "FLIGHT NOT CORRELATED", C),
    row!(101, "", "TRACK MISMATCH", C),
    row!(102, "", "NO TRACK DATA AVAILABLE FOR TRACK", C),
    row!(103, "", "NO HANDOFF IN PROGRESS", C),
    row!(104, "", "FLIGHT NOT AIRBORNE", C),
    row!(105, "", "EXPECTED PROFILE DOES NOT EXIST (AMIR)", C),
    row!(106, "", "NIL JURISDICTION", C),
    row!(107, "", "RECEIVING JURISDICTION IS NIL", C),
    row!(108, "", "EXTERNAL REQUESTOR BUT JURISDICTION IS LOCAL", C),
    row!(109, "", "HANDOFF ALREADY IN PROGRESS FOR THE FLIGHT", C),
    row!(
        110,
        "",
        "NO RECEIVING JURISDICTION ON PROFILE WITHIN THE HANDOFF DETERMINATION PERIOD",
        C
    ),
    row!(
        111,
        "",
        "RECEIVING SECTOR IS NOT CONFIGURED FOR RADAR HANDOFF",
        C
    ),
    row!(112, "", "RECEIVING FACILITY ID NON EXISTENT", C),
    row!(113, "", "INCORRECT LEG", C),
    row!(114, "", "HANDOFF ROUTING FAILURE", C),
    row!(115, "", "ALREADY HANDED OFF OUTBOUND", C),
    row!(116, "", "FLIGHT NOT BEACON EQUIPPED", C),
];

/// The table row for a code, if the ICD defines one (92 to 98 are partly reserved).
pub fn lrm_entry(code: u8) -> Option<&'static LrmEntry> {
    LRM_TABLE.iter().find(|e| e.code == code)
}

/// The field number Attachment 2 Table 2-2 gives where it differs from Table A-1's column:
/// code 17 (16), 63 (18), 64 to 66 (10). The rest are the same.
pub const ATT2_FIELD: [(u8, &str); 5] =
    [(17, "16"), (63, "18"), (64, "10"), (65, "10"), (66, "10")];

/// The "Field Number" column of `code` under `profile`.
pub fn lrm_field(code: u8, profile: crate::model::Profile) -> Option<&'static str> {
    let base = lrm_entry(code)?.field;
    if profile == crate::model::Profile::EramAttachment2 {
        if let Some((_, f)) = ATT2_FIELD.iter().find(|(c, _)| *c == code) {
            return Some(f);
        }
    }
    Some(base)
}

/// The Table A-1 error code and field a refusal is reported with in an LRM
/// (`RMK/<code>/<field>/<text>`).
pub fn lrm(e: &NamError) -> (u8, u8) {
    lrm_for(e, Profile::default(), None)
}

/// The code and field under `profile`, for a refused message of kind `msg` when known (see
/// [`crate::message::peek_kind`]). Table A-1 gives one code per refusal; Attachment 2 Table
/// 2-2 differs in three places, applied under [`Profile::EramAttachment2`]:
/// * a bad message number is 57, not 4 ("N/A to ERAM/CAATS interface");
/// * Field 03 syntax failing in an RTI, RTA, RLA or RTU is 57, including a bad 03(c);
/// * a missing time designator in 13 or 16 is 21 (expected time designator not found),
///   in 14 it is 24.
pub fn lrm_for(e: &NamError, profile: Profile, msg: Option<Kind>) -> (u8, u8) {
    let (mut code, default_field) = lrm_code(e.kind);
    let field = if e.field != 0 { e.field } else { default_field };
    if e.kind == ErrorKind::TimeMissing && matches!(field, 13 | 16) {
        code = 21;
    }
    if profile == Profile::EramAttachment2 {
        let handoff = matches!(msg, Some(Kind::Rti | Kind::Rta | Kind::Rla | Kind::Rtu));
        match e.kind {
            ErrorKind::MessageNumber => code = 57,
            ErrorKind::Reference if handoff => code = 57,
            _ => {}
        }
    }
    (code, field)
}
