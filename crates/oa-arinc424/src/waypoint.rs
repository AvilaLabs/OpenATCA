//! Enroute waypoint (EA) and terminal waypoint (PC) records.

use crate::airport::check_primary;
use crate::error::ErrorKind;
use crate::field::Rec;
use crate::types::{Meta, Position};

/// Which section a waypoint came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaypointKind {
    /// Section E subsection A.
    Enroute,
    /// Section P subsection C: carries its parent airport's customer code (FAA readme).
    Terminal,
}

/// A named fix.
#[derive(Debug, Clone, PartialEq)]
pub struct Waypoint {
    pub meta: Meta,
    pub kind: WaypointKind,
    /// Cols 7-10: `ENRT` for enroute waypoints, the airport identifier for terminal ones.
    /// Verified: `ENRT` on EA fixtures, `KORD` on the ORD PC fixtures.
    pub area: String,
    /// Cols 11-12: the airport's ICAO region for terminal waypoints, blank for enroute.
    pub area_region: String,
    /// Cols 14-18. Verified: `AAALL` equals NASR `FIX_BASE.FIX_ID`; whole file: 70,050 of
    /// 70,056 waypoints are found in `FIX_BASE` by identifier and region (6 are not).
    pub ident: String,
    /// Cols 20-21, trimmed (`K6`, or `K`/`P` for offshore fixes). Verified: `K6` equals NASR
    /// `ICAO_REGION_CODE` (used as half of the lookup key above).
    pub icao_region: String,
    /// Cols 27-29, trimmed. `W` waypoint, `R` ground-based, `C` both (FAA readme).
    pub waypoint_type: String,
    /// Cols 30-32, trimmed. Usage (`L`, `H`, `B`); blank for terminal waypoints.
    pub usage: String,
    /// Cols 33-51. Verified against NASR `FIX_BASE.LAT_DECIMAL/LONG_DECIMAL`: whole file, all
    /// 70,050 matched waypoints agree within 3e-6 degrees (`E180` equals NASR `-180`).
    pub position: Position,
    /// Cols 75-79 (`W0094`): dynamic magnetic variation, degrees east positive.
    pub magnetic_variation: Option<f64>,
    /// Cols 85-87. Datum code.
    pub datum: String,
    /// Cols 99-123. Name; usually the identifier (`AAARG`), sometimes `ADAME(CNF)` style.
    pub name: String,
}

pub(crate) fn parse_waypoint(
    r: &Rec,
    meta: Meta,
    kind: WaypointKind,
) -> Result<Waypoint, ErrorKind> {
    check_primary(r, 22)?;
    let position = r
        .position("waypoint position", 33)?
        .ok_or_else(|| r.bad("waypoint position", 33, 51, "required but blank"))?;
    Ok(Waypoint {
        meta,
        kind,
        area: r.text(7, 10),
        area_region: r.text(11, 12),
        ident: r.text(14, 18),
        icao_region: r.text(20, 21),
        waypoint_type: r.text(27, 29),
        usage: r.text(30, 32),
        position,
        magnetic_variation: r.variation("magnetic variation", 75)?,
        datum: r.text(85, 87),
        name: r.text(99, 123),
    })
}
