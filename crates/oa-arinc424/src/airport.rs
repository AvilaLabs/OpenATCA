//! Airport (PA), runway (PG) and localizer/glide slope (PI) records.
//!
//! Each field's doc names its columns and how the position was verified against real
//! records (OA-424-007). "NASR" means the 2026-10-29 NASR CSV subscriber files; the
//! verifying tests are in `tests/layouts.rs`.

use crate::error::ErrorKind;
use crate::field::Rec;
use crate::types::{Altitude, Meta, Position};

/// Airport reference point record, section P subsection A (cols 13).
#[derive(Debug, Clone, PartialEq)]
pub struct Airport {
    pub meta: Meta,
    /// Cols 7-10. ICAO identifier, or the FAA identifier when there is none (FAA readme).
    /// Verified: `KORD` for Chicago O'Hare.
    pub ident: String,
    /// Cols 11-12. ICAO region code (`K5`). Verified with the same record.
    pub icao_region: String,
    /// Cols 14-18. IATA field; the FAA identifier when `ident` is not four characters
    /// (FAA readme), blank otherwise. Verified: `ORD` equals NASR `APT_BASE.ARPT_ID`.
    pub iata: Option<String>,
    /// Cols 28-30, hundreds of feet. Verified: `130` for ORD, whose longest runway (10L/28R) is
    /// 13,000 ft in NASR `APT_RWY.RWY_LEN`; whole file: equals the longest NASR runway
    /// (rounded down to 100 ft) at all but 48 of 13,308 airports (the FAA readme says it may
    /// not be the longest hard-surface runway).
    pub longest_runway_ft: Option<u32>,
    /// Col 31. `Y` when the airport has an instrument approach (`N` otherwise).
    /// Verified: `Y` at ORD.
    pub ifr_capable: bool,
    /// Col 32. Surface of the longest runway (`H` hard, `S` soft, `W` water, ...). Verified: `H` at ORD.
    pub longest_runway_surface: Option<char>,
    /// Cols 33-51. Verified against NASR `APT_BASE.LAT_DECIMAL/LONG_DECIMAL`: whole file,
    /// 13,308 of 13,308 matched airports agree within 3e-6 degrees (hundredth of a second).
    pub position: Position,
    /// Cols 52-56 (`W0030`). Degrees, east positive. Verified: NASR `MAG_VARN` 3 `MAG_HEMIS` W;
    /// whole file: 10,108 of 10,108 agree within 1 degree.
    pub magnetic_variation: Option<f64>,
    /// Cols 57-61, feet. Verified: NASR `APT_BASE.ELEV` 680 at ORD; whole file: 13,308 of
    /// 13,308 agree within 1 ft.
    pub elevation_ft: Option<i32>,
    /// Cols 71-75. Transition altitude. Verified: 18000 at ORD (US standard).
    pub transition_altitude: Option<Altitude>,
    /// Cols 76-80. Transition level. Verified: 18000 at ORD.
    pub transition_level: Option<Altitude>,
    /// Col 81. `C` civil, `P` public, `M` military, `J` joint (as coded). Verified: `C` at ORD.
    pub public_military: Option<char>,
    /// Col 86. `M` magnetic / `T` true. Verified: `M` at ORD.
    pub north_reference: Option<char>,
    /// Cols 87-89. Datum code (`NAR` = NAD 83).
    pub datum: String,
    /// Cols 94-123 (30 characters, so long names are cut). Verified: `CHICAGO O'HARE INTL`
    /// equals NASR `ARPT_NAME`; whole file: always a prefix of the NASR name.
    pub name: String,
}

pub(crate) fn parse_airport(r: &Rec, meta: Meta) -> Result<Airport, ErrorKind> {
    check_primary(r, 22)?;
    let position = r
        .position("airport reference point", 33)?
        .ok_or_else(|| r.bad("airport reference point", 33, 51, "required but blank"))?;
    Ok(Airport {
        meta,
        ident: r.text(7, 10),
        icao_region: r.text(11, 12),
        iata: r.opt_text(14, 18),
        longest_runway_ft: r
            .uint("longest runway", 28, 30)?
            .map(|h| h.saturating_mul(100)),
        ifr_capable: r.ch(31) == 'Y',
        longest_runway_surface: r.opt_ch(32),
        position,
        magnetic_variation: r.variation("magnetic variation", 52)?,
        elevation_ft: r.signed("airport elevation", 57, 61)?,
        transition_altitude: r.altitude("transition altitude", 71, 75)?,
        transition_level: r.altitude("transition level", 76, 80)?,
        public_military: r.opt_ch(81),
        north_reference: r.opt_ch(86),
        datum: r.text(87, 89),
        name: r.text(94, 123),
    })
}

/// Rejects a primary-record continuation number other than 0 or 1.
pub(crate) fn check_primary(r: &Rec, col: usize) -> Result<(), ErrorKind> {
    match r.ch(col) {
        '0' | '1' => Ok(()),
        number => Err(ErrorKind::UnsupportedContinuation { number }),
    }
}

/// Runway record (section P subsection G): the landing threshold.
#[derive(Debug, Clone, PartialEq)]
pub struct Runway {
    pub meta: Meta,
    /// Cols 7-10 / 11-12: the airport this runway belongs to.
    pub airport_ident: String,
    pub airport_region: String,
    /// Cols 14-18, as coded (`RW04L`; non-numeric runways have no `RW` prefix per the FAA
    /// readme: `E`, `NE`). Verified: `RW04L` at KORD against NASR `APT_RWY_END.RWY_END_ID` `04L`.
    pub ident: String,
    /// Cols 23-27, feet. Verified: 7500 for 04L, 8075 for 04R, 11245 for 09C equal NASR
    /// `APT_RWY.RWY_LEN`; whole file: 16,982 of 16,982 matched runways agree.
    pub length_ft: u32,
    /// Cols 28-31, tenths of a degree, magnetic. Blank on a few records. Verified: 042.0 for
    /// 04L (NASR true alignment 39, variation 3 W).
    pub magnetic_bearing: Option<f64>,
    /// Cols 33-51. The landing threshold: the displaced threshold where there is one.
    /// Verified against NASR `APT_RWY_END` (the displaced threshold columns when
    /// `DISPLACED_THR_LEN` is set, else the runway end): whole file, 16,982 of 16,982 matched
    /// runway ends agree within 3e-6 degrees.
    pub threshold: Position,
    /// Cols 61-66, signed tenths of a metre. Verified: +0166.2 m for ORD 04L is NASR
    /// `RWY_END_ELEV` 655.7 ft (199.9 m) plus a geoid height of about -33.7 m.
    pub ellipsoid_height_m: Option<f64>,
    /// Cols 67-71, feet. Verified: 656 for ORD 04L, NASR `RWY_END_ELEV` 655.7. Whole file:
    /// 915 of 16,978 differ from NASR by more than 1 ft (a few feet, different survey).
    pub threshold_elevation_ft: Option<i32>,
    /// Cols 72-75, feet. Verified: 758 for MDW 04L equals NASR `DISPLACED_THR_LEN`; whole file:
    /// equal for 2,051 of 2,108 NASR displaced thresholds. The other 57 are coded `0000` in
    /// the CIFP although the coordinate is the displaced threshold (a data defect).
    pub displaced_threshold_ft: u32,
    /// Cols 76-77, feet. Not the NASR visual glide path TCH (3,913 of 7,350 differ). Verified
    /// by consistency: where `tch_indicator` is `I` it equals the glide slope TCH of the named
    /// localizer record (55 at ORD 09C; 1,129 of 1,131 runway ends in the whole file).
    pub threshold_crossing_height_ft: Option<u32>,
    /// Cols 78-80, feet. Verified: 150 for 04L, 200 for 09C equal NASR `APT_RWY.RWY_WIDTH`;
    /// whole file: all but 22 water runways agree (`999` is a cap for wider runways).
    pub width_ft: Option<u32>,
    /// Col 81. Source of the crossing height; `I` where the localizer's glide slope supplies it
    /// (verified above), `D` and `R` also occur. The meaning of `D` and `R` is not decoded.
    pub tch_indicator: Option<char>,
    /// Cols 82-85. Localizer identifier of the ILS serving this runway end.
    /// Verified: `IFJU` for ORD 04R matches the localizer record `IFJU` with runway `RW04R`.
    pub localizer_ident: Option<String>,
    /// Col 86. ILS category (`1`, `3`). Verified: `3` for IOYG (ORD 09C) equals NASR
    /// `ILS_BASE.CATEGORY` III.
    pub localizer_category: Option<char>,
}

pub(crate) fn parse_runway(r: &Rec, meta: Meta) -> Result<Runway, ErrorKind> {
    check_primary(r, 22)?;
    let threshold = r
        .position("runway threshold", 33)?
        .ok_or_else(|| r.bad("runway threshold", 33, 51, "required but blank"))?;
    Ok(Runway {
        meta,
        airport_ident: r.text(7, 10),
        airport_region: r.text(11, 12),
        ident: r.text(14, 18),
        length_ft: r.req_uint("runway length", 23, 27)?,
        magnetic_bearing: r.scaled("runway bearing", 28, 31, 10.0)?,
        threshold,
        ellipsoid_height_m: r.signed_scaled("ellipsoid height", 61, 66, 10.0)?,
        threshold_elevation_ft: r.signed("threshold elevation", 67, 71)?,
        displaced_threshold_ft: r.req_uint("displaced threshold", 72, 75)?,
        threshold_crossing_height_ft: r.uint("threshold crossing height", 76, 77)?,
        width_ft: r.uint("runway width", 78, 80)?,
        tch_indicator: r.opt_ch(81),
        localizer_ident: r.opt_text(82, 85),
        localizer_category: r.opt_ch(86),
    })
}

/// The runway identifier without the `RW` prefix (`04L`).
pub fn runway_designator(r: &Runway) -> &str {
    r.ident.strip_prefix("RW").unwrap_or(&r.ident)
}

/// Localizer and glide slope record (section P subsection I).
#[derive(Debug, Clone, PartialEq)]
pub struct Localizer {
    pub meta: Meta,
    /// Cols 7-10 / 11-12: the airport.
    pub airport_ident: String,
    pub airport_region: String,
    /// Cols 14-17. Verified: `IABU` is NASR `ILS_BASE.ILS_LOC_ID` `ABU` with a leading `I`
    /// (whole file: all but 6 of 1,278 localizers found in `ILS_BASE` by that rule).
    pub ident: String,
    /// Col 18. Verified: `3` equals NASR `ILS_BASE.CATEGORY` `III` for IABU. Also `0` for
    /// localizers without glide slope and `A`..., as coded; 147 of 1,174 differ from NASR.
    pub category: char,
    /// Cols 23-27 in units of 10 kHz, stored as MHz. Verified: 111.75 equals NASR `LOC_FREQ`;
    /// whole file: 1,272 of 1,272 agree.
    pub frequency_mhz: f64,
    /// Cols 28-32 (`RW27R`). Verified with the same ILS (NASR `RWY_END_ID` 27R).
    pub runway_ident: String,
    /// Cols 33-51. Verified against NASR `ILS_BASE.LAT_DECIMAL/LONG_DECIMAL`; whole file: all
    /// but 7 of 1,272 agree within 3e-6 degrees.
    pub localizer_position: Position,
    /// Cols 52-55, tenths of a degree, magnetic. Verified: 273.0 vs NASR `APCH_BEAR` 273.01;
    /// whole file: 1,187 of 1,272 within 0.1 degree (the rest differ by variation epoch).
    pub bearing: f64,
    /// Cols 56-74. Blank for localizer-only records. Verified against NASR
    /// `ILS_GS.LAT_DECIMAL/LONG_DECIMAL`; whole file: all but 3 of 1,139 agree.
    pub glide_slope_position: Option<Position>,
    /// Cols 84-87, hundredths of a degree. Not in NASR; position is plausible only (4.68 for
    /// IABU, 3 to 7 degrees across the file).
    pub localizer_width_deg: f64,
    /// Cols 88-90, hundredths of a degree. Verified: 3.00 equals NASR `ILS_GS.G_S_ANGLE` 3;
    /// whole file: all but 1 of 1,139 agree.
    pub glide_slope_angle_deg: Option<f64>,
    /// Cols 91-95 (`W0030`), degrees east positive. Verified: NASR `ILS_BASE.MAG_VAR` 3 W for IABU.
    pub station_declination: Option<f64>,
    /// Cols 96-97, feet. Glide slope threshold crossing height.
    /// Verified: 55 equals the runway record TCH for ORD 09C (same ILS).
    pub glide_slope_tch_ft: Option<u32>,
    /// Cols 98-102, feet. Verified: 648 for IABU equals NASR `ILS_GS.SITE_ELEVATION` 648.4;
    /// whole file: all but 6 of 1,139 within 1 ft.
    pub glide_slope_elevation_ft: Option<i32>,
}

pub(crate) fn parse_localizer(r: &Rec, meta: Meta) -> Result<Localizer, ErrorKind> {
    check_primary(r, 22)?;
    let localizer_position = r
        .position("localizer position", 33)?
        .ok_or_else(|| r.bad("localizer position", 33, 51, "required but blank"))?;
    let frequency_mhz = r
        .scaled("localizer frequency", 23, 27, 100.0)?
        .ok_or_else(|| r.bad("localizer frequency", 23, 27, "required but blank"))?;
    Ok(Localizer {
        meta,
        airport_ident: r.text(7, 10),
        airport_region: r.text(11, 12),
        ident: r.text(14, 17),
        category: r.ch(18),
        frequency_mhz,
        runway_ident: r.text(28, 32),
        localizer_position,
        bearing: r
            .scaled("localizer bearing", 52, 55, 10.0)?
            .ok_or_else(|| r.bad("localizer bearing", 52, 55, "required but blank"))?,
        glide_slope_position: r.position("glide slope position", 56)?,
        localizer_width_deg: r
            .scaled("localizer width", 84, 87, 100.0)?
            .ok_or_else(|| r.bad("localizer width", 84, 87, "required but blank"))?,
        glide_slope_angle_deg: r.scaled("glide slope angle", 88, 90, 100.0)?,
        station_declination: r.variation("station declination", 91)?,
        glide_slope_tch_ft: r.uint("glide slope TCH", 96, 97)?,
        glide_slope_elevation_ft: r.signed("glide slope elevation", 98, 102)?,
    })
}
