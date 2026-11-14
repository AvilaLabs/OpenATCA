//! VHF navaid (D), enroute NDB (DB) and terminal NDB (PN) records.

use crate::airport::check_primary;
use crate::error::ErrorKind;
use crate::field::Rec;
use crate::types::{Meta, Position};

/// VHF navaid record (section D, blank subsection): VOR, VOR/DME, VORTAC, DME, TACAN, ILS/DME.
#[derive(Debug, Clone, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct VhfNavaid {
    pub meta: Meta,
    /// Cols 7-10 / 11-12. The airport an ILS/DME or terminal navaid belongs to; blank for
    /// enroute navaids. Verified: `KAAO` / `K3` on a navaid at Colonel James Jabara.
    pub airport_ident: Option<String>,
    pub airport_region: Option<String>,
    /// Cols 14-17. Verified: `ABI` equals NASR `NAV_BASE.NAV_ID`; whole file: 1,526 navaids and
    /// NDBs found in `NAV_BASE` by identifier with a matching position.
    pub ident: String,
    /// Cols 20-21. ICAO region code (`K4` for ABI).
    pub icao_region: String,
    /// Cols 23-27, in units of 10 kHz, stored as MHz. `00000` (VOR frequency unavailable
    /// per the FAA readme) and blank give `None`. Verified: 113.70 for ABI equals NASR `FREQ`;
    /// whole file: all but 1 of 1,030 US navaids agree.
    pub frequency_mhz: Option<f64>,
    /// Cols 28-32, raw: facility type (`V` VOR, ...), DME/TACAN indicator (`D`, `T`, `M`, `I`),
    /// altitude class (`H`, `L`, `T`, `U`), weather capability (`W`, `A`, `B`).
    /// Verified: ` DHW ` for the DME-only AAT, `VTHW ` for the VORTAC ABI.
    pub class: [char; 5],
    /// Cols 33-51. The VOR position; blank for DME-only. Verified against NASR
    /// `NAV_BASE.LAT_DECIMAL/LONG_DECIMAL`: whole file, no US navaid disagrees (NASR holds a
    /// single position where the CIFP has separate VOR and DME positions, so either CIFP
    /// position may match). Three Canadian navaids (YUL, YVV, YXY) differ by 10 to 100 m.
    pub vor_position: Option<Position>,
    /// Cols 52-55. DME identifier; blank when it equals the navaid identifier. Verified: `AAT`
    /// on the DME-only fixture, blank for ABI, `ISYK` for an ILS/DME.
    pub dme_ident: Option<String>,
    /// Cols 56-74. DME position. Verified against NASR `TACAN_DME_LAT_DECIMAL` (see above).
    pub dme_position: Option<Position>,
    /// Cols 75-79 (`E0100`), degrees east positive. Verified: NASR `MAG_VARN` 10 E at ABI.
    /// DME-only facilities carry a computed value (FAA readme) that NASR lacks.
    pub station_declination: Option<f64>,
    /// Cols 80-84, feet. Verified: NASR `NAV_BASE.ELEV`.
    pub dme_elevation_ft: Option<i32>,
    /// Col 85. Figure of merit (`2` for ABI and AAT).
    pub figure_of_merit: Option<char>,
    /// Cols 91-93. Datum code.
    pub datum: String,
    /// Cols 94-123. Verified: `ABILENE` equals NASR `NAME`.
    pub name: String,
}

pub(crate) fn parse_vhf(r: &Rec, meta: Meta) -> Result<VhfNavaid, ErrorKind> {
    check_primary(r, 22)?;
    let freq = r.uint("VOR frequency", 23, 27)?.filter(|&v| v != 0);
    let mut class = [' '; 5];
    for (i, slot) in class.iter_mut().enumerate() {
        *slot = r.ch(28 + i);
    }
    Ok(VhfNavaid {
        meta,
        airport_ident: r.opt_text(7, 10),
        airport_region: r.opt_text(11, 12),
        ident: r.text(14, 17),
        icao_region: r.text(20, 21),
        frequency_mhz: freq.map(|v| f64::from(v) / 100.0),
        class,
        vor_position: r.position("VOR position", 33)?,
        dme_ident: r.opt_text(52, 55),
        dme_position: r.position("DME position", 56)?,
        station_declination: r.variation("station declination", 75)?,
        dme_elevation_ft: r.signed("DME elevation", 80, 84)?,
        figure_of_merit: r.opt_ch(85),
        datum: r.text(91, 93),
        name: r.text(94, 123),
    })
}

/// NDB record: enroute (section D, subsection B) or terminal (section P, subsection N).
#[derive(Debug, Clone, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct Ndb {
    pub meta: Meta,
    /// Cols 7-10 / 11-12. The airport a terminal NDB (`PN`) belongs to; blank for enroute
    /// NDBs. Verified: `KIKK` / `K5` on the NDB `IK` (LUKOW) at Kankakee, whose I04R approach
    /// legs name it. Verified over the whole file: 180 of the 181 terminal NDBs are found in
    /// `NAV_BASE` by identifier and name with the same position and frequency (ADDMO `AI` at
    /// KADM is not in NASR; NASR's `AI` is a different NDB in Indiana).
    pub airport_ident: Option<String>,
    pub airport_region: Option<String>,
    /// Cols 14-17. Verified: `AA` equals NASR `NAV_BASE.NAV_ID` for the NDB KENIE at Fargo.
    pub ident: String,
    /// Cols 20-21. ICAO region code.
    pub icao_region: String,
    /// Cols 23-27 in units of 0.1 kHz, stored as kHz. Verified: 365.0 for KENIE equals NASR
    /// `FREQ` 365; whole file: 317 of 317 US NDBs agree.
    pub frequency_khz: Option<f64>,
    /// Cols 28-32, raw NDB class (`HOMW`: facility class, capabilities, weather).
    pub class: String,
    /// Cols 33-51. Verified against NASR `NAV_BASE.LAT_DECIMAL/LONG_DECIMAL`.
    pub position: Position,
    /// Cols 75-79, degrees east positive. Verified: 4.0 E for KENIE and 4.0 W for CEDAR equal
    /// NASR `MAG_VARN`.
    pub magnetic_variation: Option<f64>,
    /// Cols 91-93. Datum code.
    pub datum: String,
    /// Cols 94-123. Verified: `KENIE` equals NASR `NAME`.
    pub name: String,
}

pub(crate) fn parse_ndb(r: &Rec, meta: Meta) -> Result<Ndb, ErrorKind> {
    check_primary(r, 22)?;
    let position = r
        .position("NDB position", 33)?
        .ok_or_else(|| r.bad("NDB position", 33, 51, "required but blank"))?;
    Ok(Ndb {
        meta,
        airport_ident: r.opt_text(7, 10),
        airport_region: r.opt_text(11, 12),
        ident: r.text(14, 17),
        icao_region: r.text(20, 21),
        frequency_khz: r.scaled("NDB frequency", 23, 27, 10.0)?,
        class: r.text(28, 32),
        position,
        magnetic_variation: r.variation("magnetic variation", 75)?,
        datum: r.text(91, 93),
        name: r.text(94, 123),
    })
}
