//! Enroute airway (ER) records, grouped into ordered fixes.

use crate::airport::check_primary;
use crate::error::ErrorKind;
use crate::field::Rec;
use crate::types::{Altitude, FixRef, Meta};

/// One fix on an airway, with the data for the leg that leaves it toward the next fix.
#[derive(Debug, Clone, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct AirwayFix {
    pub meta: Meta,
    /// Cols 14-18. Route identifier (`V9`, `J80`, `Q1`). Verified: `V9` matches NASR `AWY_SEG_ALT.AWY_ID`.
    pub route_ident: String,
    /// Cols 26-29. Strictly increasing along the airway. Verified: 0100, 0110, ... in the V9 fixture.
    pub sequence: u32,
    /// Cols 30-34 fix identifier, 35-36 ICAO region, 37 section, 38 subsection. Verified: V9
    /// fixes LEV (VOR, `D`), PEPSI (`EA`) match NASR `FROM_POINT` order; whole file: the fix
    /// order equals NASR for 1,137 of 1,242 US airways (the rest are split differently).
    pub fix: FixRef,
    /// Cols 40-43. Waypoint description code.
    pub description: [char; 4],
    /// Col 45. `O` conventional, `R` RNAV (FAA readme). Verified: `O` for V9, `R` for Q1.
    pub route_type: char,
    /// Col 46. `L` low (V, T routes), `H` high (J, Q routes) per the FAA readme; blank otherwise.
    /// Verified: V9 `L`, J80 `H`, Q1 `H`.
    pub level: Option<char>,
    /// Cols 71-74, tenths of a degree magnetic: outbound course to the next fix.
    /// Verified: 354.4 at LEV equals NASR `MAG_COURSE` 354.44; whole file: 14,384 of 14,384
    /// agree within 0.15 degree.
    pub outbound_course: Option<f64>,
    /// Cols 75-78, tenths of a nautical mile: distance to the next fix.
    /// Verified: 10.0 NM LEV to PEPSI equals NASR `MAG_COURSE_DIST` 10; whole file: 14,384 of
    /// 14,384 agree within 0.06 NM.
    pub distance_nm: Option<f64>,
    /// Cols 79-82, tenths of a degree magnetic: inbound course at this fix from the previous
    /// one (353.4 at PEPSI after 354.4 outbound from LEV; not checkable against NASR).
    pub inbound_course: Option<f64>,
    /// Cols 84-88. Minimum enroute altitude of the leg to the next fix (`UNKNN` where not known).
    /// Verified: 2000 at LEV, 4000 at SAFES equal NASR `MIN_ENROUTE_ALT`; whole file: 12,980 of
    /// 13,269 agree (NASR holds direction-specific values the CIFP does not).
    pub min_altitude: Option<Altitude>,
    /// Cols 89-93. Second minimum altitude (opposite direction or GNSS), rarely used.
    pub min_altitude_2: Option<Altitude>,
    /// Cols 94-98. Maximum authorized altitude. Verified: 17500 for V9 equals NASR
    /// `MAX_AUTH_ALT`; whole file: 14,156 of 14,156 agree.
    pub max_altitude: Option<Altitude>,
}

pub(crate) fn parse_airway_fix(r: &Rec, meta: Meta) -> Result<AirwayFix, ErrorKind> {
    check_primary(r, 39)?;
    let sequence = r.req_uint("sequence number", 26, 29)?;
    let mut description = [' '; 4];
    for (i, slot) in description.iter_mut().enumerate() {
        *slot = r.ch(40 + i);
    }
    Ok(AirwayFix {
        meta,
        route_ident: r.text(14, 18),
        sequence,
        fix: {
            let (section, subsection) = r.section_codes("fix section", 37, 38)?;
            FixRef {
                ident: r.ident("fix identifier", 30, 34)?,
                icao_region: r.text(35, 36),
                section,
                subsection,
            }
        },
        description,
        route_type: r.ch(45),
        level: r.opt_ch(46),
        outbound_course: r.bearing("outbound course", 71, 74)?,
        distance_nm: r.scaled("route distance", 75, 78, 10.0)?,
        inbound_course: r.bearing("inbound course", 79, 82)?,
        min_altitude: r.altitude("minimum altitude", 84, 88)?,
        min_altitude_2: r.altitude("minimum altitude 2", 89, 93)?,
        max_altitude: r.altitude("maximum altitude", 94, 98)?,
    })
}

/// An airway: all fixes of one route identifier, in sequence order.
#[derive(Debug, Clone, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct Airway {
    /// Customer/area code of its records (`USA`, `CAN`, ...).
    pub customer_area: String,
    pub ident: String,
    /// Route type of the first fix.
    pub route_type: char,
    /// Level of the first fix.
    pub level: Option<char>,
    pub fixes: Vec<AirwayFix>,
}
