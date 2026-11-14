//! SID (PD), STAR (PE) and approach (PF) records, grouped into ordered legs.
//!
//! Verification of field positions is by fixture (`tests/layouts.rs`: KMSP SID BITEZ1, KORD
//! STAR BENKY6, KORD approaches I04R and R28C) and, over the whole file, by cross-reference:
//! every fix named by a leg is a record in the file (see `tests/cifp_full.rs`). The ARINC 424
//! tables are not public, so semantics beyond what the data shows are left as raw codes.

use crate::airport::check_primary;
use crate::error::ErrorKind;
use crate::field::Rec;
use crate::types::{Altitude, AltitudeDescription, FixRef, Meta, TurnDirection};

/// Which procedure section a record belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProcedureKind {
    /// Subsection D.
    Sid,
    /// Subsection E.
    Star,
    /// Subsection F.
    Approach,
}

/// ARINC 424 path terminators (cols 48-49).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathTerminator {
    AF,
    CA,
    CD,
    CF,
    CI,
    CR,
    DF,
    FA,
    FC,
    FD,
    FM,
    HA,
    HF,
    HM,
    IF,
    PI,
    RF,
    TF,
    VA,
    VD,
    VI,
    VM,
    VR,
}

/// Parses a two-letter path terminator code.
pub fn parse_path_terminator(s: &str) -> Option<PathTerminator> {
    Some(match s {
        "AF" => PathTerminator::AF,
        "CA" => PathTerminator::CA,
        "CD" => PathTerminator::CD,
        "CF" => PathTerminator::CF,
        "CI" => PathTerminator::CI,
        "CR" => PathTerminator::CR,
        "DF" => PathTerminator::DF,
        "FA" => PathTerminator::FA,
        "FC" => PathTerminator::FC,
        "FD" => PathTerminator::FD,
        "FM" => PathTerminator::FM,
        "HA" => PathTerminator::HA,
        "HF" => PathTerminator::HF,
        "HM" => PathTerminator::HM,
        "IF" => PathTerminator::IF,
        "PI" => PathTerminator::PI,
        "RF" => PathTerminator::RF,
        "TF" => PathTerminator::TF,
        "VA" => PathTerminator::VA,
        "VD" => PathTerminator::VD,
        "VI" => PathTerminator::VI,
        "VM" => PathTerminator::VM,
        "VR" => PathTerminator::VR,
        _ => return None,
    })
}

/// A course given as magnetic degrees, or a distance/time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DistanceOrTime {
    /// Nautical miles.
    Distance(f64),
    /// Minutes (holding legs: `T010` is 1.0 minute).
    Time(f64),
}

/// A navaid or fix referenced by a leg (recommended navaid, arc/MSA center).
pub type NavRef = FixRef;

/// One leg of a procedure (one primary record).
#[derive(Debug, Clone, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct Leg {
    pub meta: Meta,
    /// Cols 7-10: airport identifier (`KORD`, `KMSP`).
    pub airport_ident: String,
    /// Cols 11-12: airport ICAO region (`K5` for KORD).
    pub airport_region: String,
    pub kind: ProcedureKind,
    /// Cols 14-19: SID/STAR/approach identifier (`BENKY6`, `I04R`, `R28C`). Verified: `BENKY6`
    /// is the ORD STAR named in NASR `STAR_BASE`.
    pub ident: String,
    /// Col 20: route type. Seen: `4`, `5`, `6` on SIDs/STARs (`4` and `6` carry enroute
    /// transitions, `5` a runway-agnostic common route), `A` approach transition, `I` ILS, `R`
    /// RNAV (GPS), `H` RNP (FAA readme). Verified on the fixtures listed above.
    pub route_type: char,
    /// Cols 21-25: transition identifier (`BFORD`, `RAHNN`, `ALL`), empty for final approach
    /// routes. Verified: `BFORD` is the first fix of the BFORD transition of BENKY6.
    pub transition: String,
    /// Cols 27-29: sequence number, strictly increasing within a route (checked while
    /// grouping; whole file: no violation).
    pub sequence: u32,
    /// Cols 30-34 fix identifier, 35-36 region, 37 section, 38 subsection; `None` for legs
    /// without a fix (CA, VI). Verified: `RAHNN` / `K5` / `P` / `C` names the terminal waypoint
    /// RAHNN at KORD; `HHOBA` / `K3` / `E` / `A` an enroute waypoint; `OBK` / `D` / ` ` a VOR.
    pub fix: Option<FixRef>,
    /// Cols 40-43: the four waypoint description codes (`E   `, `E  A`, `EE B`). Verified:
    /// position 4 is `A` on the initial approach fix and `B` on the transition's last fix,
    /// positions 1-2 are `EE` on the last fix of an enroute transition. Codes not decoded.
    pub description: [char; 4],
    /// Col 44. Verified: `R` on the HM leg at LAIRD and `L` on the DF leg at DPA.
    pub turn_direction: Option<TurnDirection>,
    /// Cols 45-47: required navigation performance as coded (`010`, `031`). Verified: `031` on
    /// the final segment of the RNAV approach R28C; the value encoding is not decoded.
    pub rnp_code: Option<String>,
    /// Cols 48-49. Verified: IF, TF, CF, CA, VI, HM, DF in the fixtures; every record of the file
    /// uses one of the 23 terminators.
    pub path_terminator: PathTerminator,
    /// Cols 51-54 ident, 55-56 region, 79 section, 80 subsection. Recommended navaid. Verified:
    /// `IFJU` / `K5` / `P` / `I` is the localizer IFJU of ORD runway 04R; `OBK` / `D` / ` ` a VOR.
    pub recommended_navaid: Option<NavRef>,
    /// Cols 57-62, thousandths of a nautical mile (`002500` = 2.5 NM), on arc legs. Position
    /// verified by occupancy (1,450 records, all RF/arc legs); scale not checked against a source.
    pub arc_radius_nm: Option<f64>,
    /// Cols 63-66, tenths of a degree: bearing from the recommended navaid to the fix.
    /// Verified: 224.5 from IFJU on the HARRL leg.
    pub theta_deg: Option<f64>,
    /// Cols 67-70, tenths of a nautical mile: distance from the recommended navaid. Verified:
    /// 14.9 NM on the same leg.
    pub rho_nm: Option<f64>,
    /// Cols 71-74, tenths of a degree magnetic. Verified: 44.5 on the ILS 04R legs, equal to
    /// the localizer bearing IFJU 44.5.
    pub course_deg: Option<f64>,
    /// Cols 75-78: tenths of a nautical mile, or `Tnnn` tenths of a minute. Verified: 8.1 NM on
    /// the HARRL CF leg and `T010` (1.0 minute) on the LAIRD hold.
    pub distance_or_time: Option<DistanceOrTime>,
    /// Col 83. Verified: `+` (at or above) on STAR and approach legs, `B` between FL230 and FL200
    /// in the BENKY6 fixture; `H`, `J`, `V` and others are kept raw.
    pub altitude_description: Option<AltitudeDescription>,
    /// Cols 85-89. Verified: 11000 at RAHNN, FL220 at BFORD, 713 at the RW04R runway fix (the
    /// threshold crossing altitude; NASR threshold elevation 661 plus crossing height 52).
    pub altitude1: Option<Altitude>,
    /// Cols 90-94. Verified: FL200 with FL230 as altitude 1 on a `B` leg (altitude 1 is the
    /// higher one on all 4,430 `B` legs of the file).
    pub altitude2: Option<Altitude>,
    /// Cols 95-99. Transition altitude. Verified: 18000 on the IF legs, equal to the airport
    /// record's transition altitude at KORD.
    pub transition_altitude: Option<Altitude>,
    /// Cols 100-102, knots. Position verified by occupancy: values such as 210, 250, 280, 230
    /// on STAR legs.
    pub speed_limit_kt: Option<u32>,
    /// Cols 103-106, hundredths of a degree, signed (`-300` is a 3.00 degree descent).
    /// Verified: -3.00 at the runway fix of the ILS 04R approach, equal to the glide slope
    /// angle of localizer IFJU.
    pub vertical_angle_deg: Option<f64>,
    /// Cols 107-111 ident, 113-114 region, 115 section, 116 subsection: arc centre fix, or the
    /// MSA centre fix on the final approach fix (FAA readme). Verified: `KORD` / `K5` / `P` /
    /// `A` (the airport) on the REKKS final approach fix.
    pub center_fix: Option<FixRef>,
    /// Col 117, raw (`A`, `B`, `0`, `P`, `3`). Occupied on approach legs only; the codes'
    /// meaning is not decoded.
    pub gnss_fms_indication: Option<char>,
    /// Col 118, raw: `-` on 4,102 legs and `+` on 3 (4,096 of those also carry a speed limit).
    pub speed_limit_description: Option<char>,
    /// Col 119, raw. `F` marks RNP approaches (FAA readme); `J`, `P`, `N`, `D`, `W`, `A` also occur.
    pub route_qualifier_1: Option<char>,
    /// Col 120, raw. `S` and `C` occur on approaches, `H` for helipad approaches (FAA readme).
    pub route_qualifier_2: Option<char>,
    /// Continuation records attached to this leg, in file order.
    pub continuations: Vec<LegContinuation>,
}

/// A continuation record of a leg.
#[derive(Debug, Clone, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct LegContinuation {
    pub meta: Meta,
    /// Col 39: continuation record number (`2` first).
    pub number: char,
    /// Col 40: application type. `W` is the FAA level-of-service record on approaches.
    pub application_type: char,
    /// Cols 41-123, right-trimmed, raw.
    pub text: String,
}

/// A level of service from an approach continuation (`LPV`, `LNAV/VNAV`, `LNAV`, ...).
#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct LevelOfService {
    /// `A` in the first column of the pair: authorized.
    pub authorized: bool,
    pub name: String,
}

/// Decodes an application-type `W` record as three authorized/name pairs (cols 41-73:
/// authorization flag and 10-character name, three times). `None` for other records or
/// records that do not follow the pattern.
pub fn levels_of_service(c: &LegContinuation) -> Option<Vec<LevelOfService>> {
    if c.application_type != 'W' {
        return None;
    }
    // `text` starts at column 41.
    let mut out = Vec::new();
    for i in 0..3usize {
        let start = i * 11;
        let flag = c.text.get(start..start + 1)?;
        let rest = c.text.get(start + 1..).unwrap_or("");
        let name = rest.get(..10).unwrap_or(rest);
        let authorized = match flag {
            "A" => true,
            "N" => false,
            _ => return None,
        };
        out.push(LevelOfService {
            authorized,
            name: name.trim().to_string(),
        });
    }
    Some(out)
}

/// The continuation fields of a procedure record, before attachment.
pub(crate) fn parse_leg_continuation(r: &Rec, meta: Meta) -> LegContinuation {
    LegContinuation {
        meta,
        number: r.ch(39),
        application_type: r.ch(40),
        text: r.raw(41, 123).trim_end_matches(' ').to_string(),
    }
}

pub(crate) fn parse_leg(r: &Rec, meta: Meta, kind: ProcedureKind) -> Result<Leg, ErrorKind> {
    check_primary(r, 39)?;
    let terminator = r.raw(48, 49);
    let path_terminator = parse_path_terminator(terminator)
        .ok_or_else(|| r.bad("path terminator", 48, 49, "unknown path terminator"))?;
    let fix = if r.is_blank(30, 38) {
        None
    } else {
        let (section, subsection) = r.section_codes("fix section", 37, 38)?;
        Some(FixRef {
            ident: r.ident("fix identifier", 30, 34)?,
            icao_region: r.text(35, 36),
            section,
            subsection,
        })
    };
    let recommended_navaid = if r.is_blank(51, 56) {
        None
    } else {
        Some(FixRef {
            ident: r.text(51, 54),
            icao_region: r.text(55, 56),
            section: r.ch(79),
            subsection: r.ch(80),
        })
    };
    let center_fix = if r.is_blank(107, 111) {
        None
    } else {
        Some(FixRef {
            ident: r.text(107, 111),
            icao_region: r.text(113, 114),
            section: r.ch(115),
            subsection: r.ch(116),
        })
    };
    let turn_direction = match r.ch(44) {
        ' ' => None,
        'L' => Some(TurnDirection::Left),
        'R' => Some(TurnDirection::Right),
        'E' => Some(TurnDirection::Either),
        _ => return Err(r.bad("turn direction", 44, 44, "expected L, R or E")),
    };
    let altitude_description = match r.ch(83) {
        ' ' => None,
        '+' => Some(AltitudeDescription::AtOrAbove),
        '-' => Some(AltitudeDescription::AtOrBelow),
        '@' => Some(AltitudeDescription::At),
        'B' => Some(AltitudeDescription::Between),
        c => Some(AltitudeDescription::Other(c)),
    };
    let distance_or_time = if r.ch(75) == 'T' {
        r.scaled("holding time", 76, 78, 10.0)?
            .map(DistanceOrTime::Time)
    } else {
        r.scaled("route distance", 75, 78, 10.0)?
            .map(DistanceOrTime::Distance)
    };
    let mut description = [' '; 4];
    for (i, slot) in description.iter_mut().enumerate() {
        *slot = r.ch(40 + i);
    }
    Ok(Leg {
        meta,
        airport_ident: r.text(7, 10),
        airport_region: r.text(11, 12),
        kind,
        ident: r.text(14, 19),
        route_type: r.ch(20),
        transition: r.text(21, 25),
        sequence: r.req_uint("sequence number", 27, 29)?,
        fix,
        description,
        turn_direction,
        rnp_code: r.opt_text(45, 47),
        path_terminator,
        recommended_navaid,
        arc_radius_nm: r.scaled("arc radius", 57, 62, 1000.0)?,
        theta_deg: r.bearing("theta", 63, 66)?,
        rho_nm: r.scaled("rho", 67, 70, 10.0)?,
        course_deg: r.bearing("magnetic course", 71, 74)?,
        distance_or_time,
        altitude_description,
        altitude1: r.altitude("altitude 1", 85, 89)?,
        altitude2: r.altitude("altitude 2", 90, 94)?,
        transition_altitude: r.altitude("transition altitude", 95, 99)?,
        speed_limit_kt: r.uint("speed limit", 100, 102)?,
        vertical_angle_deg: r.signed_scaled("vertical angle", 103, 106, 100.0)?,
        center_fix,
        gnss_fms_indication: r.opt_ch(117),
        speed_limit_description: r.opt_ch(118),
        route_qualifier_1: r.opt_ch(119),
        route_qualifier_2: r.opt_ch(120),
        continuations: Vec::new(),
    })
}

/// One route of a procedure: a transition identifier and its legs in sequence order.
#[derive(Debug, Clone, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct Route {
    /// Route type (col 20).
    pub route_type: char,
    /// Transition identifier (cols 21-25), empty for common routes and approach finals.
    pub transition: String,
    pub legs: Vec<Leg>,
}

/// A SID, STAR or approach at one airport: its routes in file order.
#[derive(Debug, Clone, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct Procedure {
    pub customer_area: String,
    pub airport_ident: String,
    pub airport_region: String,
    pub kind: ProcedureKind,
    pub ident: String,
    pub routes: Vec<Route>,
}
