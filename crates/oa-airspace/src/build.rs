//! Building the model from parsed sources (OA-ASP-001..004). **Core** (B1): a pure function from
//! [`Sources`] and [`BuildParams`] to a [`Model`] plus [`BuildInfo`] (what the report needs
//! that the model does not hold). No I/O, no clock, no randomness; no hash-map iteration order
//! reaches the output (maps are only looked up, every output list is sorted).
//!
//! Steps:
//!
//! 1. **Candidates.** Every NASR and CIFP point object becomes a candidate. Objects present in
//!    both sources are merged under one identity (class of kind, ident, ICAO region, scope):
//!    fixes with enroute/terminal waypoints by (ident, region), airports by ICAO or FAA ident,
//!    navaids by ident and class (nearest first, within [`MAX_PAIR_M`], because short NDB
//!    identifiers recur across the country), runway thresholds by airport and designator.
//!    Position rule: CIFP when both sources have the object; both positions are kept to report
//!    every pair that differs by more than [`DISAGREE_M`] (OA-ASP-003).
//! 2. **Selection area** (OA-ASP-002): the ARTCC's HIGH and LOW polygons plus the buffer.
//!    Points in the area are selected; so is every procedure of every airport inside a polygon,
//!    every airway with a point in the area, and every point those reference.
//! 3. **Boundaries and MORA**: the ARTCC's own boundaries and every NASR or CIFP boundary that
//!    touches the area; MORA cells that overlap it.
//! 4. **Assembly**: every table sorted by its stable key, strings interned in that order, so
//!    the snapshot depends only on the inputs.
//!
//! References that name a point neither source has are kept as unresolved rows (OA-ASP-004).
//! CIFP fields UNKNOWNS UK-002 lists as unverified are not consumed: procedure leg RNP, GNSS
//! and route-qualifier codes, arc radius on legs, altitude description codes other than `+`
//! and `B`, the TCH indicator, the header CRC and southern-hemisphere MORA rows.

use crate::geom::{
    area_contains, area_inside, build_area, cell_overlaps_area, deg_to_e7, distance_m, latlon_e7,
    ring_samples, samples_touch_area, segment_samples, Area, SegIn,
};
use crate::model::*;
use crate::source::Sources;
use oa_arinc424 as c424;
use oa_geo::{wgs84, Ellipsoid, GeoError, LatLon};
use oa_nasr as nasr;
use std::collections::{BTreeMap, HashMap};
use std::fmt;

/// Metres per nautical mile.
pub const NM_M: f64 = 1852.0;
/// A NASR and a CIFP position closer than this agree; farther apart is reported (OA-ASP-003).
pub const DISAGREE_M: f64 = 1.0;
/// Navaids with the same identifier and class pair up when within this distance (short NDB
/// identifiers recur across the country, so the nearest one is the same object).
pub const MAX_PAIR_M: f64 = 100_000.0;
/// A NASR airport and a CIFP airport with the same identifier pair up only when within this
/// distance, 1 nautical mile (OA-ASP-011). A farther same-identifier pair is two different
/// places that share a code: it is not merged, and the report lists it as a disagreement.
pub const MAX_AIRPORT_PAIR_M: f64 = NM_M;
/// A NASR record whose own decimal and DMS positions differ by more than this (degrees, one
/// model unit) is reported as a source defect.
pub const POSITION_DEFECT_DEG: f64 = 1e-7;
/// Limit: largest accepted buffer, 1000 nautical miles.
pub const MAX_BUFFER_M: u32 = 1_852_000;
/// Limit: longest ARTCC identifier accepted.
pub const MAX_ARTCC_LEN: usize = 8;

/// Selection parameters.
#[derive(Clone, Debug, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct BuildParams {
    /// ARTCC identifier, e.g. `ZAU`.
    pub artcc: String,
    /// Buffer around the HIGH and LOW boundaries, metres (default 50 NM = 92,600).
    pub buffer_m: u32,
}

/// Why a build failed.
#[derive(Debug)]
pub enum BuildError {
    /// A parameter is out of range.
    BadParams(String),
    /// The ARTCC is not in `ARB_BASE` or has no boundary.
    UnknownArtcc(String),
    /// An effective date is malformed, or the NASR records carry none.
    NasrCycle(String),
    /// The CIFP file has no header cycle.
    NoCifpCycle,
    /// The ARTCC's boundary is not a valid polygon.
    BadBoundary {
        stratum: &'static str,
        error: GeoError,
    },
    /// A geodesic computation failed (not expected for valid input).
    Geo {
        context: &'static str,
        error: GeoError,
    },
    /// A source value cannot be stored (out of range).
    Unstorable(String),
    /// A source record is incomplete or out of range in a way that would change its meaning
    /// (OA-ASP-009); `location` names the file or line and the record.
    BadRecord { location: String, reason: String },
    /// The assembled model failed its own validation (a defect in this crate).
    Model(ModelError),
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildError::BadParams(m) => write!(f, "bad parameters: {m}"),
            BuildError::UnknownArtcc(a) => {
                write!(f, "ARTCC {a:?} has no boundary in this NASR cycle")
            }
            BuildError::NasrCycle(m) => write!(f, "NASR cycle: {m}"),
            BuildError::NoCifpCycle => write!(f, "the CIFP file has no header cycle"),
            BuildError::BadBoundary { stratum, error } => {
                write!(
                    f,
                    "ARTCC {stratum} boundary is not a usable polygon: {error:?}"
                )
            }
            BuildError::Geo { context, error } => {
                write!(f, "geodesy failed while {context}: {error:?}")
            }
            BuildError::Unstorable(m) => write!(f, "cannot store: {m}"),
            BuildError::BadRecord { location, reason } => write!(f, "{location}: {reason}"),
            BuildError::Model(e) => write!(f, "internal error: {e}"),
        }
    }
}

impl std::error::Error for BuildError {}

fn geo(context: &'static str) -> impl Fn(GeoError) -> BuildError {
    move |error| BuildError::Geo { context, error }
}

/// A NASR record whose published decimal position disagrees with its own DMS position.
#[derive(Clone, Debug, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct PositionDefect {
    pub file: &'static str,
    pub line: u64,
    pub ident: String,
    pub diff_deg: f64,
}

/// Largest error of the fixed-point conversion (A9). Cold struct (A7.2).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rounding {
    /// Geodesic distance between a source coordinate and its stored 1e-7 degree value, largest
    /// over every stored coordinate, metres.
    pub pos_max_m: f64,
    /// Which value produced it.
    pub pos_at: String,
    /// Coordinates converted.
    pub pos_values: u64,
    /// Largest error converting a leg distance from nautical miles to whole metres.
    pub dist_max_m: f64,
}

/// What the build learned that the model tables do not hold; the report prints it.
#[derive(Clone, Debug, Default, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct BuildInfo {
    pub artcc_name: String,
    /// Every distinct NASR `EFF_DATE` in the consumed files; the model keeps the latest.
    pub nasr_dates: Vec<String>,
    /// The boundaries that define the selection area, with vertex counts.
    pub area_rings: Vec<String>,
    pub rounding: Rounding,
    /// NASR positions whose decimal and DMS disagree, for objects in the model.
    pub position_defects: Vec<PositionDefect>,
    /// Keys that occurred twice within a source (second dropped), and positions that differ.
    pub duplicates: Vec<String>,
    /// Navaids whose NASR type and CIFP class disagree.
    pub kind_mismatches: Vec<String>,
    /// NASR-only procedures (not coded in CIFP), as `airport kind ident`.
    pub uncoded_procedures: Vec<String>,
    /// Airports whose NASR class B/C/D flag and CIFP class airspace disagree.
    pub class_mismatches: Vec<String>,
    /// Source records skipped, with the reason and count.
    pub skipped: Vec<(String, usize)>,
    /// Procedure legs whose altitude description code is not decoded (UK-002).
    pub unverified_alt_desc: usize,
    /// Boundaries that could not be used for the area test, with the reason.
    pub boundary_notes: Vec<String>,
    /// Airway legs whose end point is not the next row of the same airway (OA-ASP-010): the leg
    /// is kept with no end point, and the reason is listed here.
    pub airway_breaks: Vec<String>,
    /// Candidate counts: (label, count).
    pub candidates: Vec<(String, usize)>,
    /// NASR/CIFP pairs compared over the whole cycle (not only the selection).
    pub pairs_compared: usize,
    /// Of those, pairs more than [`DISAGREE_M`] apart.
    pub pairs_over_limit: usize,
    /// The largest distance among them, metres.
    pub pairs_max_m: f64,
    /// Those pairs, largest distance first: `(metres, label)`.
    pub pairs_over: Vec<(f64, String)>,
}

/// The model and what the report needs beside it.
#[derive(Debug)]
// A7.2: cold struct, size guard waived
pub struct BuildOutput {
    pub model: Model,
    pub info: BuildInfo,
}

// ---------------------------------------------------------------------------------------------
// Candidates

/// A NASR record's own decimal/DMS disagreement.
#[derive(Clone, Debug)]
// A7.2: cold struct, size guard waived
struct Defect {
    file: &'static str,
    line: u64,
    diff_deg: f64,
}

/// A point object before selection: both sources' views merged.
#[derive(Clone, Debug)]
// A7.2: cold struct, size guard waived
struct Cand {
    kind: u8,
    ident: String,
    alt_ident: Option<String>,
    region: Option<String>,
    scope: Option<String>,
    name: Option<String>,
    /// The position taken (CIFP when present).
    pos: (f64, f64),
    nasr_pos: Option<(f64, f64)>,
    cifp_pos: Option<(f64, f64)>,
    defect: Option<Defect>,
    elev: Option<i32>,
    nasr: bool,
    cifp: bool,
    /// Bits: 1 class B, 2 class C, 4 class D, 8 class E.
    cls: u8,
    /// Thresholds and localizers: the owning airport.
    airport: Option<usize>,
    e7: (i32, i32),
    in_area: bool,
    inside: bool,
    referenced: bool,
}

type Key = (u8, String, String, String);

fn new_cand(kind: u8, ident: &str, pos: (f64, f64)) -> Cand {
    Cand {
        kind,
        ident: ident.to_string(),
        alt_ident: None,
        region: None,
        scope: None,
        name: None,
        pos,
        nasr_pos: None,
        cifp_pos: None,
        defect: None,
        elev: None,
        nasr: false,
        cifp: false,
        cls: 0,
        airport: None,
        e7: (0, 0),
        in_area: false,
        inside: false,
        referenced: false,
    }
}

/// The candidates and the identity index over them.
// A7.2: cold struct, size guard waived
struct Registry {
    cands: Vec<Cand>,
    by_key: HashMap<Key, usize>,
    duplicates: Vec<String>,
}

fn key_of(c: &Cand, drop_region: bool) -> Key {
    (
        point_kind::class(c.kind),
        c.ident.clone(),
        if drop_region {
            String::new()
        } else {
            c.region.clone().unwrap_or_default()
        },
        c.scope.clone().unwrap_or_default(),
    )
}

impl Registry {
    /// Adds a candidate, or returns the existing one with the same key (reporting the second
    /// as a duplicate with the distance between the two positions).
    fn add(&mut self, ell: &Ellipsoid, c: Cand, drop_region: bool, what: &str) -> usize {
        let key = key_of(&c, drop_region);
        if let Some(&i) = self.by_key.get(&key) {
            let d = self
                .cands
                .get(i)
                .map_or(f64::NAN, |o| pos_distance(ell, o.pos, c.pos));
            self.duplicates.push(format!(
                "{what} {} (region {}, scope {}) occurs twice in the sources; the second is dropped (positions {d:.1} m apart)",
                key.1, key.2, key.3
            ));
            return i;
        }
        let i = self.cands.len();
        self.by_key.insert(key, i);
        self.cands.push(c);
        i
    }
}

fn latlon(p: (f64, f64)) -> Option<LatLon> {
    LatLon::new(p.0, p.1).ok()
}

/// Geodesic distance between two positions given as degrees; NaN when a position is invalid.
fn pos_distance(ell: &Ellipsoid, a: (f64, f64), b: (f64, f64)) -> f64 {
    match (latlon(a), latlon(b)) {
        (Some(x), Some(y)) => distance_m(ell, x, y).unwrap_or(f64::NAN),
        _ => f64::NAN,
    }
}

fn nasr_pos(p: &nasr::Position) -> (f64, f64) {
    (p.lat, p.lon)
}

fn defect_of(p: &nasr::Position, file: &'static str, line: u64) -> Option<Defect> {
    let d = nasr::position::max_diff(p);
    (d > POSITION_DEFECT_DEG).then_some(Defect {
        file,
        line,
        diff_deg: d,
    })
}

fn round_i32(v: f64) -> Option<i32> {
    let r = v.round();
    (r.is_finite() && r >= f64::from(i32::MIN) + 8.0 && r <= f64::from(i32::MAX) - 8.0)
        .then_some(r as i32)
}

fn round_u32(v: f64) -> Option<u32> {
    let r = v.round();
    (r.is_finite() && r >= 0.0 && r < f64::from(u32::MAX)).then_some(r as u32)
}

/// NASR navaid type to a point kind; `None` for types the model does not carry.
fn nasr_nav_kind(t: &str) -> Option<u8> {
    Some(match t {
        "VOR" | "VOT" => point_kind::VOR,
        "VORTAC" => point_kind::VORTAC,
        "VOR/DME" => point_kind::VOR_DME,
        "DME" => point_kind::DME,
        "TACAN" => point_kind::TACAN,
        "NDB" | "NDB/DME" | "MARINE NDB" | "UHF/NDB" => point_kind::NDB,
        _ => return None,
    })
}

/// CIFP VHF navaid class (cols 28-32) to a point kind. Verified shapes: ` DHW ` is a DME and
/// `VTHW ` a VORTAC; the other combinations follow ARINC 424 and are checked against NASR's
/// own type on every paired navaid (mismatches are reported).
fn cifp_vhf_kind(class: [char; 5]) -> u8 {
    let vor = class[0] == 'V';
    match (vor, class[1]) {
        (true, 'T') | (true, 'M') => point_kind::VORTAC,
        (true, 'D') | (true, 'I') => point_kind::VOR_DME,
        (true, _) => point_kind::VOR,
        (false, 'T') | (false, 'M') => point_kind::TACAN,
        (false, _) => point_kind::DME,
    }
}

/// Everything the later steps look references up in.
// A7.2: cold struct, size guard waived
struct Maps {
    /// (ident, region) to the fix-class candidate.
    fix: HashMap<(String, String), usize>,
    /// ident to fix-class candidates.
    fix_by_ident: HashMap<String, Vec<usize>>,
    /// CIFP navaid (ident, region, subsection) to candidate.
    nav_cifp: HashMap<(String, String, char), usize>,
    /// NASR navaid (id, type) to NASR navaid indexes.
    nasr_nav: HashMap<(String, String), Vec<usize>>,
    /// NASR navaid id to NASR navaid indexes.
    nasr_nav_by_id: HashMap<String, Vec<usize>>,
    /// NASR navaid index to candidate (`usize::MAX` when not carried).
    nasr_nav_cand: Vec<usize>,
    /// (airport candidate, identifier) of a terminal NDB (PN) to candidate.
    tndb: HashMap<(usize, String), usize>,
    apt_cifp: HashMap<String, usize>,
    apt_faa: HashMap<String, usize>,
    thr: HashMap<(usize, String), usize>,
    loc: HashMap<(usize, String), usize>,
}

const NO_CAND: usize = usize::MAX;

/// Records a CIFP navaid under its lookup key; a terminal NDB also under its airport.
fn register_cifp_nav(
    maps: &mut Maps,
    sub: char,
    ident: &str,
    region: &str,
    airport: Option<&str>,
    cand: usize,
) {
    if sub != 'N' {
        maps.nav_cifp
            .insert((ident.to_string(), region.to_string(), sub), cand);
    }
    if let Some(a) = airport.and_then(|a| maps.apt_cifp.get(a).copied()) {
        maps.tndb.insert((a, ident.to_string()), cand);
    }
}

/// The CIFP-side data needed to compare navaid classes.
fn pair_navaids(
    src: &Sources,
    ell: &Ellipsoid,
    reg: &mut Registry,
    maps: &mut Maps,
    info: &mut BuildInfo,
) {
    // CIFP navaids, with the position taken and the kind derived from the class.
    // A7.2: cold struct, size guard waived
    struct CNav {
        kind: u8,
        ident: String,
        region: String,
        sub: char,
        /// The airport of a terminal NDB.
        airport: Option<String>,
        pos: (f64, f64),
        name: String,
        elev: Option<i32>,
    }
    let mut cnavs: Vec<CNav> = Vec::new();
    let mut no_position = 0usize;
    for v in &src.cifp.vhf_navaids {
        match v.vor_position.or(v.dme_position) {
            Some(p) => cnavs.push(CNav {
                kind: cifp_vhf_kind(v.class),
                ident: v.ident.clone(),
                region: v.icao_region.clone(),
                sub: ' ',
                airport: None,
                pos: (p.lat, p.lon),
                name: v.name.clone(),
                elev: v.dme_elevation_ft,
            }),
            None => no_position += 1,
        }
    }
    let terminal = src.cifp.terminal_ndbs.iter().map(|n| ('N', n));
    for (sub, n) in src.cifp.ndbs.iter().map(|n| ('B', n)).chain(terminal) {
        cnavs.push(CNav {
            kind: point_kind::NDB,
            ident: n.ident.clone(),
            region: n.icao_region.clone(),
            sub,
            airport: n.airport_ident.clone(),
            pos: (n.position.lat, n.position.lon),
            name: n.name.clone(),
            elev: None,
        });
    }
    if no_position > 0 {
        info.skipped.push((
            "CIFP VHF navaids without a position".to_string(),
            no_position,
        ));
    }
    let mut by_class_ident: HashMap<(u8, &str), Vec<usize>> = HashMap::new();
    for (i, c) in cnavs.iter().enumerate() {
        by_class_ident
            .entry((point_kind::class(c.kind), c.ident.as_str()))
            .or_default()
            .push(i);
    }
    let mut paired = vec![false; cnavs.len()];
    let mut skipped_types = 0usize;
    maps.nasr_nav_cand = vec![NO_CAND; src.nasr.navaids.len()];
    for (ni, n) in src.nasr.navaids.iter().enumerate() {
        let Some(kind) = nasr_nav_kind(&n.nav_type) else {
            skipped_types += 1;
            continue;
        };
        let npos = nasr_pos(&n.position);
        // Nearest unpaired CIFP navaid with the same ident and class, within MAX_PAIR_M.
        let mut best: Option<(f64, usize)> = None;
        if let Some(list) = by_class_ident.get(&(point_kind::class(kind), n.nav_id.as_str())) {
            for &ci in list {
                if paired.get(ci).copied().unwrap_or(true) {
                    continue;
                }
                let Some(c) = cnavs.get(ci) else { continue };
                let d = pos_distance(ell, npos, c.pos);
                if d <= MAX_PAIR_M && best.is_none_or(|(bd, _)| d < bd) {
                    best = Some((d, ci));
                }
            }
        }
        let mut cand = new_cand(kind, &n.nav_id, npos);
        cand.nasr = true;
        cand.nasr_pos = Some(npos);
        cand.defect = defect_of(&n.position, "NAV_BASE.csv", n.line);
        cand.name = Some(n.name.clone());
        cand.elev = n.elev.and_then(round_i32);
        match best {
            Some((_, ci)) => {
                if let Some(p) = paired.get_mut(ci) {
                    *p = true;
                }
                if let Some(c) = cnavs.get(ci) {
                    cand.cifp = true;
                    cand.cifp_pos = Some(c.pos);
                    cand.pos = c.pos;
                    cand.region = Some(c.region.clone());
                    if cand.elev.is_none() {
                        cand.elev = c.elev;
                    }
                    if point_kind::name(c.kind) != point_kind::name(kind) {
                        info.kind_mismatches.push(format!(
                            "navaid {} ({}): NASR type {}, CIFP class gives {}",
                            n.nav_id,
                            c.region,
                            n.nav_type,
                            point_kind::name(c.kind)
                        ));
                    }
                    let i = reg.add(ell, cand, false, "navaid");
                    register_cifp_nav(maps, c.sub, &c.ident, &c.region, c.airport.as_deref(), i);
                    if let Some(slot) = maps.nasr_nav_cand.get_mut(ni) {
                        *slot = i;
                    }
                }
            }
            None => {
                cand.scope = n.state_code.clone();
                let i = reg.add(ell, cand, false, "navaid");
                if let Some(slot) = maps.nasr_nav_cand.get_mut(ni) {
                    *slot = i;
                }
            }
        }
        maps.nasr_nav
            .entry((n.nav_id.clone(), n.nav_type.clone()))
            .or_default()
            .push(ni);
        maps.nasr_nav_by_id
            .entry(n.nav_id.clone())
            .or_default()
            .push(ni);
    }
    if skipped_types > 0 {
        info.skipped.push((
            "NASR navaids of types the model does not carry (FAN MARKER, ...)".to_string(),
            skipped_types,
        ));
    }
    for (ci, c) in cnavs.iter().enumerate() {
        if paired.get(ci).copied().unwrap_or(true) {
            continue;
        }
        let mut cand = new_cand(c.kind, &c.ident, c.pos);
        cand.cifp = true;
        cand.cifp_pos = Some(c.pos);
        cand.region = Some(c.region.clone());
        cand.name = Some(c.name.clone());
        cand.elev = c.elev;
        if c.sub == 'N' {
            // A terminal NDB is often the same NDB as an enroute one (same identifier, region
            // and position): it then shares that point. A different NDB with the same
            // identifier is told apart by its airport, kept as the point's scope.
            let same = reg.by_key.get(&key_of(&cand, false)).copied().filter(|&e| {
                reg.cands
                    .get(e)
                    .is_some_and(|o| pos_distance(ell, o.pos, c.pos) <= TERMINAL_NDB_SAME_M)
            });
            if let Some(e) = same {
                register_cifp_nav(maps, c.sub, &c.ident, &c.region, c.airport.as_deref(), e);
                continue;
            }
            cand.scope = c.airport.clone();
        }
        let i = reg.add(ell, cand, false, "navaid");
        register_cifp_nav(maps, c.sub, &c.ident, &c.region, c.airport.as_deref(), i);
    }
}

/// Two NDB records of one identifier and region closer than this are one NDB, metres.
const TERMINAL_NDB_SAME_M: f64 = 1.0;

fn gather_candidates(src: &Sources, ell: &Ellipsoid, info: &mut BuildInfo) -> (Registry, Maps) {
    let mut reg = Registry {
        cands: Vec::new(),
        by_key: HashMap::new(),
        duplicates: Vec::new(),
    };
    let mut maps = Maps {
        fix: HashMap::new(),
        fix_by_ident: HashMap::new(),
        nav_cifp: HashMap::new(),
        nasr_nav: HashMap::new(),
        nasr_nav_by_id: HashMap::new(),
        nasr_nav_cand: Vec::new(),
        tndb: HashMap::new(),
        apt_cifp: HashMap::new(),
        apt_faa: HashMap::new(),
        thr: HashMap::new(),
        loc: HashMap::new(),
    };

    // Airports: NASR by ICAO then FAA identifier; CIFP pairs by identifier.
    let mut nasr_by_icao: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut nasr_by_faa: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, a) in src.nasr.airports.iter().enumerate() {
        if let Some(ic) = a.icao_id.as_deref() {
            nasr_by_icao.entry(ic).or_default().push(i);
        }
        nasr_by_faa.entry(a.arpt_id.as_str()).or_default().push(i);
    }
    let mut nasr_pair: Vec<Option<usize>> = vec![None; src.nasr.airports.len()];
    let mut cifp_paired: Vec<bool> = vec![false; src.cifp.airports.len()];
    for (ci, c) in src.cifp.airports.iter().enumerate() {
        let cpos = (c.position.lat, c.position.lon);
        // The nearest same-identifier airport beyond the pairing cap, if no list has one in it.
        let mut too_far: Option<(f64, usize)> = None;
        let mut paired_here = false;
        let lists = [
            nasr_by_icao.get(c.ident.as_str()),
            nasr_by_faa.get(c.ident.as_str()),
            c.iata.as_deref().and_then(|t| nasr_by_faa.get(t)),
        ];
        for list in lists.into_iter().flatten() {
            let mut best: Option<(f64, usize)> = None;
            for &ni in list {
                if nasr_pair.get(ni).copied().flatten().is_some() {
                    continue;
                }
                let Some(n) = src.nasr.airports.get(ni) else {
                    continue;
                };
                let d = pos_distance(ell, nasr_pos(&n.position), cpos);
                // A distance that cannot be computed (NaN) is not within the cap.
                if d <= MAX_AIRPORT_PAIR_M {
                    if best.is_none_or(|(bd, _)| d < bd) {
                        best = Some((d, ni));
                    }
                } else if too_far.is_none_or(|(bd, _)| d.is_nan() || d < bd) {
                    too_far = Some((d, ni));
                }
            }
            if let Some((_, ni)) = best {
                if let Some(slot) = nasr_pair.get_mut(ni) {
                    *slot = Some(ci);
                }
                if let Some(p) = cifp_paired.get_mut(ci) {
                    *p = true;
                }
                paired_here = true;
                break;
            }
        }
        if let (false, Some((d, ni))) = (paired_here, too_far) {
            let d = if d.is_nan() { f64::INFINITY } else { d };
            info.pairs_compared += 1;
            info.pairs_over_limit += 1;
            if d.is_finite() {
                info.pairs_max_m = info.pairs_max_m.max(d);
            }
            info.pairs_over.push((
                d,
                format!(
                    "airport {} ({}): NASR {} is beyond the {} m pairing cap, not paired",
                    c.ident,
                    c.icao_region,
                    src.nasr
                        .airports
                        .get(ni)
                        .map_or("?", |n| n.arpt_id.as_str()),
                    MAX_AIRPORT_PAIR_M
                ),
            ));
        }
    }
    let airport_cand = |reg: &mut Registry,
                        maps: &mut Maps,
                        c: Cand,
                        ell: &Ellipsoid,
                        nasr_faa: Option<&str>,
                        cifp_ident: Option<&str>| {
        let i = reg.add(ell, c, true, "airport");
        if let Some(f) = nasr_faa {
            maps.apt_faa.insert(f.to_string(), i);
        }
        if let Some(id) = cifp_ident {
            maps.apt_cifp.insert(id.to_string(), i);
        }
        i
    };
    for (ni, a) in src.nasr.airports.iter().enumerate() {
        let paired = nasr_pair
            .get(ni)
            .copied()
            .flatten()
            .and_then(|ci| src.cifp.airports.get(ci));
        let npos = nasr_pos(&a.position);
        let ident = match paired {
            Some(c) => c.ident.clone(),
            None => a.icao_id.clone().unwrap_or_else(|| a.arpt_id.clone()),
        };
        let mut cand = new_cand(point_kind::AIRPORT, &ident, npos);
        cand.alt_ident = (a.arpt_id != ident).then(|| a.arpt_id.clone());
        cand.name = Some(a.arpt_name.clone());
        cand.nasr = true;
        cand.nasr_pos = Some(npos);
        cand.defect = defect_of(&a.position, "APT_BASE.csv", a.line);
        cand.elev = round_i32(a.elev);
        if let Some(c) = paired {
            cand.cifp = true;
            cand.cifp_pos = Some((c.position.lat, c.position.lon));
            cand.pos = (c.position.lat, c.position.lon);
            cand.region = Some(c.icao_region.clone());
            cand.elev = c.elevation_ft.or(cand.elev);
        }
        airport_cand(
            &mut reg,
            &mut maps,
            cand,
            ell,
            Some(&a.arpt_id),
            paired.map(|c| c.ident.as_str()),
        );
    }
    for (ci, c) in src.cifp.airports.iter().enumerate() {
        if cifp_paired.get(ci).copied().unwrap_or(true) {
            continue;
        }
        let pos = (c.position.lat, c.position.lon);
        let mut cand = new_cand(point_kind::AIRPORT, &c.ident, pos);
        cand.cifp = true;
        cand.cifp_pos = Some(pos);
        cand.region = Some(c.icao_region.clone());
        cand.name = Some(c.name.clone());
        cand.elev = c.elevation_ft;
        cand.alt_ident = c.iata.clone().filter(|t| *t != c.ident);
        airport_cand(&mut reg, &mut maps, cand, ell, None, Some(&c.ident));
    }

    // Navaids.
    pair_navaids(src, ell, &mut reg, &mut maps, info);

    // Fixes and waypoints, by (ident, region). A CIFP waypoint repeated under one key is
    // compared with the first and reported if the positions differ.
    let mut firsts: HashMap<(&str, &str), (bool, &c424::Waypoint)> = HashMap::new();
    let mut first_order: Vec<(&str, &str)> = Vec::new();
    let all_wpts = src
        .cifp
        .enroute_waypoints
        .iter()
        .map(|w| (false, w))
        .chain(src.cifp.terminal_waypoints.iter().map(|w| (true, w)));
    for (terminal, w) in all_wpts {
        let key = (w.ident.as_str(), w.icao_region.as_str());
        match firsts.get(&key) {
            None => {
                firsts.insert(key, (terminal, w));
                first_order.push(key);
            }
            Some((_, f)) => {
                let d = pos_distance(
                    ell,
                    (f.position.lat, f.position.lon),
                    (w.position.lat, w.position.lon),
                );
                if d.is_nan() || d > DISAGREE_M {
                    reg.duplicates.push(format!(
                        "CIFP waypoint {} ({}) occurs twice with positions {d:.1} m apart; the first is used",
                        w.ident, w.icao_region
                    ));
                }
            }
        }
    }
    let mut used: HashMap<(&str, &str), ()> = HashMap::new();
    for f in &src.nasr.fixes {
        let npos = nasr_pos(&f.position);
        let mut cand = new_cand(point_kind::FIX, &f.fix_id, npos);
        cand.region = Some(f.icao_region_code.clone());
        cand.nasr = true;
        cand.nasr_pos = Some(npos);
        cand.defect = defect_of(&f.position, "FIX_BASE.csv", f.line);
        let key = (f.fix_id.as_str(), f.icao_region_code.as_str());
        if let Some(&(terminal, w)) = firsts.get(&key) {
            cand.cifp = true;
            cand.cifp_pos = Some((w.position.lat, w.position.lon));
            cand.pos = (w.position.lat, w.position.lon);
            cand.kind = if terminal {
                point_kind::TERMINAL_WAYPOINT
            } else {
                point_kind::FIX
            };
            cand.name = Some(w.name.clone()).filter(|n| *n != w.ident && !n.is_empty());
            used.insert(key, ());
        }
        let i = reg.add(ell, cand, false, "fix");
        maps.fix
            .insert((f.fix_id.clone(), f.icao_region_code.clone()), i);
    }
    for key in &first_order {
        let Some(&(terminal, w)) = firsts.get(key) else {
            continue;
        };
        if used.contains_key(key) {
            continue;
        }
        let pos = (w.position.lat, w.position.lon);
        let mut cand = new_cand(
            if terminal {
                point_kind::TERMINAL_WAYPOINT
            } else {
                point_kind::FIX
            },
            &w.ident,
            pos,
        );
        cand.region = Some(w.icao_region.clone());
        cand.cifp = true;
        cand.cifp_pos = Some(pos);
        cand.name = Some(w.name.clone()).filter(|n| *n != w.ident && !n.is_empty());
        let i = reg.add(ell, cand, false, "waypoint");
        maps.fix.insert((w.ident.clone(), w.icao_region.clone()), i);
    }
    for ((ident, _), &ci) in &maps.fix {
        maps.fix_by_ident.entry(ident.clone()).or_default().push(ci);
    }
    for v in maps.fix_by_ident.values_mut() {
        v.sort_unstable();
    }

    // Runway thresholds.
    let (mut no_airport, mut no_position) = (0usize, 0usize);
    for r in &src.nasr.runway_ends {
        let Some(&ai) = maps.apt_faa.get(&r.arpt_id) else {
            no_airport += 1;
            continue;
        };
        let Some(p) = &r.position else {
            no_position += 1;
            continue;
        };
        let displaced = r.displaced_thr_len.is_some_and(|l| l > 0.0);
        let (pos, elev, posrec) = match (&r.displaced_thr_position, displaced) {
            (Some(d), true) => (nasr_pos(d), r.displaced_thr_elev.or(r.rwy_end_elev), d),
            _ => (nasr_pos(p), r.rwy_end_elev, p),
        };
        let Some(airport) = reg.cands.get(ai) else {
            continue;
        };
        let mut cand = new_cand(point_kind::THRESHOLD, &r.rwy_end_id, pos);
        cand.region = airport.region.clone();
        cand.scope = Some(airport.ident.clone());
        cand.nasr = true;
        cand.nasr_pos = Some(pos);
        cand.defect = defect_of(posrec, "APT_RWY_END.csv", r.line);
        cand.elev = elev.and_then(round_i32);
        cand.airport = Some(ai);
        let i = reg.add(ell, cand, false, "runway threshold");
        maps.thr.insert((ai, r.rwy_end_id.clone()), i);
    }
    for r in &src.cifp.runways {
        let Some(&ai) = maps.apt_cifp.get(&r.airport_ident) else {
            no_airport += 1;
            continue;
        };
        let desig = c424::runway_designator(r).to_string();
        let pos = (r.threshold.lat, r.threshold.lon);
        if let Some(&i) = maps.thr.get(&(ai, desig.clone())) {
            if let Some(c) = reg.cands.get_mut(i) {
                if c.cifp {
                    reg.duplicates.push(format!(
                        "CIFP runway {} {} occurs twice; the second is dropped",
                        r.airport_ident, desig
                    ));
                    continue;
                }
                c.cifp = true;
                c.cifp_pos = Some(pos);
                c.pos = pos;
                c.elev = r.threshold_elevation_ft.or(c.elev);
            }
            continue;
        }
        let Some(airport) = reg.cands.get(ai) else {
            continue;
        };
        let mut cand = new_cand(point_kind::THRESHOLD, &desig, pos);
        cand.region = airport.region.clone();
        cand.scope = Some(airport.ident.clone());
        cand.cifp = true;
        cand.cifp_pos = Some(pos);
        cand.elev = r.threshold_elevation_ft;
        cand.airport = Some(ai);
        let i = reg.add(ell, cand, false, "runway threshold");
        maps.thr.insert((ai, desig), i);
    }

    if no_position > 0 {
        info.skipped.push((
            "NASR runway ends without a published position".to_string(),
            no_position,
        ));
    }
    // Localizers (CIFP only).
    for l in &src.cifp.localizers {
        let Some(&ai) = maps.apt_cifp.get(&l.airport_ident) else {
            no_airport += 1;
            continue;
        };
        let Some(airport) = reg.cands.get(ai) else {
            continue;
        };
        let pos = (l.localizer_position.lat, l.localizer_position.lon);
        let mut cand = new_cand(point_kind::LOCALIZER, &l.ident, pos);
        cand.region = airport.region.clone();
        cand.scope = Some(airport.ident.clone());
        cand.cifp = true;
        cand.cifp_pos = Some(pos);
        cand.airport = Some(ai);
        let i = reg.add(ell, cand, false, "localizer");
        maps.loc.insert((ai, l.ident.clone()), i);
    }

    if no_airport > 0 {
        info.skipped.push((
            "runway ends, runways and localizers whose airport is not in the sources".to_string(),
            no_airport,
        ));
    }
    // Class airspace flags from NASR.
    for c in &src.nasr.class_airspace {
        let Some(&i) = maps.apt_faa.get(&c.arpt_id) else {
            continue;
        };
        if let Some(cand) = reg.cands.get_mut(i) {
            cand.cls |= u8::from(c.class_b)
                | (u8::from(c.class_c) << 1)
                | (u8::from(c.class_d) << 2)
                | (u8::from(c.class_e) << 3);
        }
    }
    (reg, maps)
}

// ---------------------------------------------------------------------------------------------
// References

/// Outcome of resolving a reference to a candidate.
enum Res {
    Found(usize),
    Ambiguous,
    NotFound,
}

/// What a NASR reference tells us about the point it names.
// A7.2: cold struct, size guard waived
struct NasrRef<'a> {
    name: &'a str,
    ptype: &'a str,
    region: Option<&'a str>,
    nav_name: Option<&'a str>,
    nav_city: Option<&'a str>,
    state: Option<&'a str>,
}

fn is_navaid_type(t: &str) -> bool {
    matches!(
        t,
        "VOR"
            | "VORTAC"
            | "VOR/DME"
            | "DME"
            | "TACAN"
            | "NDB"
            | "NDB/DME"
            | "VOT"
            | "MARINE NDB"
            | "FAN MARKER"
            | "UHF/NDB"
    )
}

fn resolve_nasr_fix(maps: &Maps, r: &NasrRef<'_>) -> Res {
    if let Some(region) = r.region.filter(|g| !g.is_empty()) {
        return match maps.fix.get(&(r.name.to_string(), region.to_string())) {
            Some(&i) => Res::Found(i),
            None => Res::NotFound,
        };
    }
    match maps.fix_by_ident.get(r.name).map(Vec::as_slice) {
        Some([only]) => Res::Found(*only),
        Some([]) | None => Res::NotFound,
        Some(_) => Res::Ambiguous,
    }
}

fn resolve_nasr_nav(src: &Sources, maps: &Maps, list: &[usize], r: &NasrRef<'_>) -> Res {
    let cand_of = |ni: usize| {
        maps.nasr_nav_cand
            .get(ni)
            .copied()
            .filter(|&c| c != NO_CAND)
    };
    match list {
        [] => Res::NotFound,
        [one] => cand_of(*one).map_or(Res::NotFound, Res::Found),
        many => {
            // Narrow by the name, city and state the reference carries.
            let matching: Vec<usize> = many
                .iter()
                .copied()
                .filter(|&ni| {
                    src.nasr.navaids.get(ni).is_some_and(|n| {
                        r.nav_name.is_none_or(|x| x == n.name)
                            && r.nav_city.is_none_or(|x| x == n.city)
                            && r.state.is_none_or(|x| n.state_code.as_deref() == Some(x))
                    })
                })
                .collect();
            match matching.as_slice() {
                [one] => cand_of(*one).map_or(Res::NotFound, Res::Found),
                [] => Res::NotFound,
                _ => Res::Ambiguous,
            }
        }
    }
}

fn resolve_nasr(src: &Sources, maps: &Maps, r: &NasrRef<'_>) -> Res {
    let t = r.ptype.trim();
    if is_navaid_type(t) {
        let list = maps
            .nasr_nav
            .get(&(r.name.to_string(), t.to_string()))
            .map_or(&[][..], Vec::as_slice);
        return resolve_nasr_nav(src, maps, list, r);
    }
    if !t.is_empty() {
        return resolve_nasr_fix(maps, r);
    }
    // No type: a fix by name and region, else a navaid of any type when the name is unique.
    match resolve_nasr_fix(maps, r) {
        Res::Found(i) => Res::Found(i),
        other => {
            let list = maps
                .nasr_nav_by_id
                .get(r.name)
                .map_or(&[][..], Vec::as_slice);
            match resolve_nasr_nav(src, maps, list, r) {
                Res::NotFound => other,
                found => found,
            }
        }
    }
}

/// Resolves a CIFP reference. `airport` is the candidate of the airport the record belongs to
/// (runway and localizer identifiers are only unique within an airport).
fn resolve_cifp(maps: &Maps, f: &c424::FixRef, airport: Option<usize>) -> Option<usize> {
    match (f.section, f.subsection) {
        ('E', 'A') | ('P', 'C') => maps
            .fix
            .get(&(f.ident.clone(), f.icao_region.clone()))
            .copied(),
        ('P', 'A') => maps.apt_cifp.get(&f.ident).copied(),
        ('P', 'G') => {
            let desig = f.ident.strip_prefix("RW").unwrap_or(&f.ident).to_string();
            maps.thr.get(&(airport?, desig)).copied()
        }
        ('P', 'I') => maps.loc.get(&(airport?, f.ident.clone())).copied(),
        ('P', 'N') => maps.tndb.get(&(airport?, f.ident.clone())).copied(),
        ('D', sub) => maps
            .nav_cifp
            .get(&(
                f.ident.clone(),
                f.icao_region.clone(),
                if sub == 'B' { 'B' } else { ' ' },
            ))
            .copied(),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------
// Output structures (before assembly)

// A7.2: cold struct, size guard waived
struct UnresolvedOut {
    role: u8,
    owner_kind: u8,
    owner: String,
    scope: String,
    ordinal: u32,
    name: String,
    region: String,
    wanted: String,
}

// A7.2: cold struct, size guard waived
struct LegOut {
    fix: Option<usize>,
    center: Option<usize>,
    path_term: u8,
    turn: u8,
    course: u16,
    dist_or_time: u32,
    is_time: bool,
    alt_desc: u8,
    alt1: i32,
    alt2: i32,
    speed: u16,
}

// A7.2: cold struct, size guard waived
struct RouteOut {
    route_type: u8,
    transition: String,
    legs: Vec<LegOut>,
}

// A7.2: cold struct, size guard waived
struct ProcOut {
    airport: usize,
    kind: u8,
    ident: String,
    name: Option<String>,
    nasr: bool,
    cifp: bool,
    coded: bool,
    routes: Vec<RouteOut>,
}

// A7.2: cold struct, size guard waived
struct AirwayOut<'a> {
    src: &'a nasr::awy::Airway,
    designator: String,
    location: u8,
    /// (from, to, segment index) per leg.
    legs: Vec<(Option<usize>, Option<usize>, usize)>,
}

// A7.2: cold struct, size guard waived
struct SegOut {
    via: u8,
    lat: i32,
    lon: i32,
    arc_lat: i32,
    arc_lon: i32,
    radius_m: u32,
    bearing_cdeg: u16,
}

// A7.2: cold struct, size guard waived
struct BoundaryOut {
    source: u8,
    stratum: u8,
    multiple: u8,
    facility: String,
    name: String,
    floor: i32,
    ceiling: i32,
    floor_agl: bool,
    ceiling_agl: bool,
    verts: Vec<PosE7>,
    segs: Vec<SegOut>,
}

fn path_term_code(p: c424::PathTerminator) -> u8 {
    use c424::PathTerminator as P;
    let name = match p {
        P::AF => "AF",
        P::CA => "CA",
        P::CD => "CD",
        P::CF => "CF",
        P::CI => "CI",
        P::CR => "CR",
        P::DF => "DF",
        P::FA => "FA",
        P::FC => "FC",
        P::FD => "FD",
        P::FM => "FM",
        P::HA => "HA",
        P::HF => "HF",
        P::HM => "HM",
        P::IF => "IF",
        P::PI => "PI",
        P::RF => "RF",
        P::TF => "TF",
        P::VA => "VA",
        P::VD => "VD",
        P::VI => "VI",
        P::VM => "VM",
        P::VR => "VR",
    };
    path_term::NAMES
        .iter()
        .position(|n| *n == name)
        .and_then(|i| u8::try_from(i + 1).ok())
        .unwrap_or(path_term::NONE)
}

fn altitude_ft(a: Option<c424::Altitude>) -> i32 {
    match a {
        None => ALT_NONE,
        Some(c424::Altitude::Feet(f)) => f,
        Some(c424::Altitude::FlightLevel(l)) => i32::from(l) * 100,
        Some(c424::Altitude::Ground) => ALT_GROUND,
        Some(c424::Altitude::Unlimited) => ALT_UNLIMITED,
        Some(c424::Altitude::Unknown) => ALT_UNKNOWN,
    }
}

fn compass_code(dir: Option<&str>) -> u8 {
    match dir.map(str::trim) {
        None | Some("") => compass::NONE,
        Some("N BND") => compass::NORTH,
        Some("NE BND") => compass::NORTHEAST,
        Some("E BND") => compass::EAST,
        Some("SE BND") => compass::SOUTHEAST,
        Some("S BND") => compass::SOUTH,
        Some("SW BND") => compass::SOUTHWEST,
        Some("W BND") => compass::WEST,
        Some("NW BND") => compass::NORTHWEST,
        Some(_) => compass::OTHER,
    }
}

fn wanted_of(f: &c424::FixRef) -> String {
    format!(
        "{}/{}",
        f.section,
        if f.subsection == ' ' {
            '-'
        } else {
            f.subsection
        }
    )
}

/// Converts one CIFP leg; unresolved fix and centre references are appended to `unresolved`.
#[allow(clippy::too_many_arguments)]
fn cifp_leg(
    leg: &c424::Leg,
    ordinal: u32,
    airport: usize,
    maps: &Maps,
    owner: &str,
    scope: &str,
    unresolved: &mut Vec<UnresolvedOut>,
    info: &mut BuildInfo,
) -> Result<LegOut, BuildError> {
    // A course is a bearing in 0 to 360 degrees (OA-ASP-009); the model stores it in 1/100
    // degree. 360 is allowed: ARINC 424 codes north as 3600.
    let course = match leg.course_deg {
        None => NONE_U16,
        Some(d) => (d.is_finite() && (0.0..=360.0).contains(&d))
            .then(|| round_u32(d * 100.0))
            .flatten()
            .and_then(|v| u16::try_from(v).ok())
            .ok_or_else(|| BuildError::BadRecord {
                location: format!(
                    "CIFP line {} procedure {} at {} (leg {})",
                    leg.meta.line, owner, scope, ordinal
                ),
                reason: format!("course {d} degrees is outside 0 to 360"),
            })?,
    };
    let mut resolve = |f: &Option<c424::FixRef>, role: u8| -> Option<usize> {
        let f = f.as_ref()?;
        let r = resolve_cifp(maps, f, Some(airport));
        if r.is_none() {
            unresolved.push(UnresolvedOut {
                role,
                owner_kind: owner_kind::PROCEDURE,
                owner: owner.to_string(),
                scope: scope.to_string(),
                ordinal,
                name: f.ident.clone(),
                region: f.icao_region.clone(),
                wanted: wanted_of(f),
            });
        }
        r
    };
    let fix = resolve(&leg.fix, unresolved_role::LEG_FIX);
    let center = resolve(&leg.center_fix, unresolved_role::LEG_CENTER);
    let (dist_or_time, is_time) = match leg.distance_or_time {
        Some(c424::DistanceOrTime::Distance(nm)) => {
            let exact = nm * NM_M;
            let m = round_u32(exact);
            if let Some(v) = m {
                info.rounding.dist_max_m =
                    info.rounding.dist_max_m.max((exact - f64::from(v)).abs());
            }
            (m.unwrap_or(NONE_U32), false)
        }
        Some(c424::DistanceOrTime::Time(min)) => (round_u32(min * 60.0).unwrap_or(NONE_U32), true),
        None => (NONE_U32, false),
    };
    let alt_desc = match leg.altitude_description {
        None => alt_desc::NONE,
        Some(c424::AltitudeDescription::AtOrAbove) => alt_desc::AT_OR_ABOVE,
        Some(c424::AltitudeDescription::Between) => alt_desc::BETWEEN,
        Some(_) => {
            info.unverified_alt_desc += 1;
            alt_desc::UNVERIFIED
        }
    };
    Ok(LegOut {
        fix,
        center,
        path_term: path_term_code(leg.path_terminator),
        turn: match leg.turn_direction {
            None => turn::NONE,
            Some(c424::TurnDirection::Left) => turn::LEFT,
            Some(c424::TurnDirection::Right) => turn::RIGHT,
            Some(c424::TurnDirection::Either) => turn::EITHER,
        },
        course,
        dist_or_time,
        is_time,
        alt_desc,
        alt1: altitude_ft(leg.altitude1),
        alt2: altitude_ft(leg.altitude2),
        speed: leg
            .speed_limit_kt
            .and_then(|v| u16::try_from(v).ok())
            .filter(|&v| v != NONE_U16)
            .unwrap_or(NONE_U16),
    })
}

fn nasr_proc_ident(
    kind: nasr::procedure::ProcedureKind,
    b: &nasr::procedure::ProcedureBase,
) -> String {
    use nasr::procedure::ProcedureKind as K;
    if b.computer_code == nasr::procedure::UNASSIGNED_CODE {
        return format!("{} {}", b.name, b.amendment_no);
    }
    match (kind, b.computer_code.split_once('.')) {
        (K::Star, Some((_, id))) => id.to_string(),
        (K::Dp, Some((id, _))) => id.to_string(),
        _ => b.computer_code.clone(),
    }
}

// ---------------------------------------------------------------------------------------------
// The build

/// The NASR effective date: the latest `EFF_DATE` over the consumed files, as `YYYYMMDD`, and
/// every distinct date seen. A cycle's archive mixes dates: the FAA keeps the previous date on
/// files that did not change (the 2026-10-01 archive carries 2026/09/03 on the boundary, airway
/// and procedure files).
fn nasr_effective(src: &Sources) -> Result<(u32, Vec<String>), BuildError> {
    let mut dates: Vec<&str> = Vec::new();
    let n = &src.nasr;
    dates.extend(n.arb_base.iter().map(|r| r.eff_date.as_str()));
    dates.extend(n.fixes.iter().map(|r| r.eff_date.as_str()));
    dates.extend(n.navaids.iter().map(|r| r.eff_date.as_str()));
    dates.extend(n.airports.iter().map(|r| r.eff_date.as_str()));
    dates.extend(n.runway_ends.iter().map(|r| r.eff_date.as_str()));
    dates.extend(n.airways.iter().map(|r| r.base.eff_date.as_str()));
    dates.extend(
        n.stars
            .iter()
            .chain(&n.dps)
            .map(|r| r.base.eff_date.as_str()),
    );
    dates.extend(n.class_airspace.iter().map(|r| r.eff_date.as_str()));
    dates.sort_unstable();
    dates.dedup();
    let mut latest = 0u32;
    for d in &dates {
        let digits: String = d.chars().filter(char::is_ascii_digit).collect();
        let v = if digits.len() == 8 {
            digits.parse::<u32>().ok()
        } else {
            None
        };
        let v = v.ok_or_else(|| {
            BuildError::NasrCycle(format!("effective date {d:?} is not YYYY/MM/DD"))
        })?;
        latest = latest.max(v);
    }
    if latest == 0 {
        return Err(BuildError::NasrCycle(
            "no effective date in the NASR records".into(),
        ));
    }
    Ok((latest, dates.iter().map(|d| (*d).to_string()).collect()))
}

fn stratum_of(altitude: &str, boundary_type: &str) -> Option<u8> {
    Some(match (altitude, boundary_type) {
        ("HIGH", "ARTCC") => stratum::ARTCC_HIGH,
        ("LOW", "ARTCC") => stratum::ARTCC_LOW,
        ("UNLIMITED", "ARTCC") => stratum::ARTCC_UNLIMITED,
        (_, "FIR") => stratum::FIR,
        (_, "CTA") => stratum::CTA,
        (_, "CTA/FIR") => stratum::CTA_FIR,
        (_, "UTA") => stratum::UTA,
        _ => return None,
    })
}

fn sua_stratum(t: char) -> u8 {
    match t {
        'R' => stratum::SUA_RESTRICTED,
        'M' => stratum::SUA_MOA,
        'W' => stratum::SUA_WARNING,
        'A' => stratum::SUA_ALERT,
        'P' => stratum::SUA_PROHIBITED,
        _ => stratum::SUA_OTHER,
    }
}

fn limit_ft(l: Option<c424::AltitudeLimit>, unverified: &mut usize) -> (i32, bool) {
    let Some(l) = l else { return (ALT_NONE, false) };
    match l.altitude {
        c424::Altitude::Feet(_) | c424::Altitude::FlightLevel(_) => match l.unit {
            c424::AltitudeUnit::Agl => (altitude_ft(Some(l.altitude)), true),
            c424::AltitudeUnit::Msl => (altitude_ft(Some(l.altitude)), false),
            c424::AltitudeUnit::Other(_) => {
                *unverified += 1;
                (ALT_UNKNOWN, false)
            }
        },
        other => (altitude_ft(Some(other)), false),
    }
}

/// Stores a coordinate pair (degrees) as model units, tracking the conversion error.
fn store_pos(
    ell: &Ellipsoid,
    r: &mut Rounding,
    what: &str,
    p: (f64, f64),
) -> Result<(i32, i32), BuildError> {
    let (Some(la), Some(lo)) = (deg_to_e7(p.0), deg_to_e7(p.1)) else {
        return Err(BuildError::Unstorable(format!("position {p:?} of {what}")));
    };
    r.pos_values += 1;
    if let (Some(orig), Ok(stored)) = (latlon(p), latlon_e7(la, lo)) {
        let err = distance_m(ell, orig, stored).unwrap_or(0.0);
        if err > r.pos_max_m {
            r.pos_max_m = err;
            r.pos_at = what.to_string();
        }
    }
    Ok((la, lo))
}

/// Builds the model for one ARTCC (OA-ASP-001).
pub fn build_model(src: &Sources, params: &BuildParams) -> Result<BuildOutput, BuildError> {
    if params.artcc.is_empty()
        || params.artcc.len() > MAX_ARTCC_LEN
        || !params.artcc.bytes().all(|b| b.is_ascii_alphanumeric())
    {
        return Err(BuildError::BadParams(format!(
            "ARTCC {:?} must be 1 to {MAX_ARTCC_LEN} letters or digits",
            params.artcc
        )));
    }
    if params.buffer_m > MAX_BUFFER_M {
        return Err(BuildError::BadParams(format!(
            "buffer {} m exceeds {MAX_BUFFER_M} m",
            params.buffer_m
        )));
    }
    let ell = wgs84();
    let mut info = BuildInfo::default();
    let (nasr_eff, nasr_dates) = nasr_effective(src)?;
    info.nasr_dates = nasr_dates;
    let cifp_cycle = src
        .cifp
        .header
        .as_ref()
        .map(|h| u32::from(h.cycle.year) * 100 + u32::from(h.cycle.release))
        .ok_or(BuildError::NoCifpCycle)?;

    // The area: the ARTCC's HIGH and LOW boundaries, in model units.
    let own: Vec<&nasr::arb::Boundary> = src
        .nasr
        .boundaries
        .iter()
        .filter(|b| {
            b.location_id == params.artcc
                && b.boundary_type == "ARTCC"
                && (b.altitude == "HIGH" || b.altitude == "LOW")
        })
        .collect();
    if own.is_empty()
        || !src
            .nasr
            .arb_base
            .iter()
            .any(|a| a.location_id == params.artcc)
    {
        return Err(BuildError::UnknownArtcc(params.artcc.clone()));
    }
    info.artcc_name = src
        .nasr
        .arb_base
        .iter()
        .find(|a| a.location_id == params.artcc)
        .and_then(|a| a.location_name.clone())
        .unwrap_or_default();
    let mut rings: Vec<Vec<LatLon>> = Vec::new();
    for b in &own {
        let mut ring = Vec::with_capacity(b.points.len());
        for p in &b.points {
            let (la, lo) = store_pos(
                &ell,
                &mut Rounding::default(),
                "boundary",
                nasr_pos(&p.position),
            )?;
            ring.push(latlon_e7(la, lo).map_err(geo("reading a boundary vertex"))?);
        }
        let label: &'static str = if b.altitude == "HIGH" { "HIGH" } else { "LOW" };
        info.area_rings.push(format!(
            "{} {} ({} vertices, source NASR ARB_SEG)",
            params.artcc,
            label,
            ring.len()
        ));
        // Validate each ring on its own so the error names it.
        build_area(vec![ring.clone()], 0.0).map_err(|error| BuildError::BadBoundary {
            stratum: label,
            error,
        })?;
        rings.push(ring);
    }
    let area: Area =
        build_area(rings, f64::from(params.buffer_m)).map_err(|error| BuildError::BadBoundary {
            stratum: "HIGH/LOW",
            error,
        })?;

    // 1. Candidates.
    let (mut reg, maps) = gather_candidates(src, &ell, &mut info);
    info.duplicates.append(&mut reg.duplicates);
    info.candidates.push((
        "point candidates from both sources".to_string(),
        reg.cands.len(),
    ));
    for c in &mut reg.cands {
        let (la, lo) = match (deg_to_e7(c.pos.0), deg_to_e7(c.pos.1)) {
            (Some(a), Some(b)) => (a, b),
            _ => {
                return Err(BuildError::Unstorable(format!(
                    "position {:?} of {}",
                    c.pos, c.ident
                )))
            }
        };
        c.e7 = (la, lo);
        if let (Some(n), Some(f)) = (c.nasr_pos, c.cifp_pos) {
            let d = pos_distance(&ell, n, f);
            info.pairs_compared += 1;
            if d.is_nan() || d > DISAGREE_M {
                info.pairs_over_limit += 1;
                info.pairs_over.push((
                    if d.is_nan() { f64::INFINITY } else { d },
                    format!(
                        "{} {} {}",
                        point_kind::name(c.kind),
                        c.ident,
                        c.region.as_deref().unwrap_or("-")
                    ),
                ));
            }
            if d.is_finite() {
                info.pairs_max_m = info.pairs_max_m.max(d);
            }
        }
    }

    info.pairs_over
        .sort_by(|x, y| y.0.total_cmp(&x.0).then_with(|| x.1.cmp(&y.1)));

    // 2. Selection of points.
    for i in 0..reg.cands.len() {
        let Some(c) = reg.cands.get(i) else { continue };
        if c.airport.is_some() {
            continue;
        }
        let p = latlon_e7(c.e7.0, c.e7.1).map_err(geo("selecting points"))?;
        let inside = c.kind == point_kind::AIRPORT && area_inside(&area, p);
        let in_area = inside || area_contains(&area, p).map_err(geo("selecting points"))?;
        if let Some(c) = reg.cands.get_mut(i) {
            c.in_area = in_area;
            c.inside = inside;
        }
    }
    for i in 0..reg.cands.len() {
        let owner = reg.cands.get(i).and_then(|c| c.airport);
        if let Some(a) = owner {
            let (in_area, inside) = reg
                .cands
                .get(a)
                .map_or((false, false), |c| (c.in_area, c.inside));
            if let Some(c) = reg.cands.get_mut(i) {
                c.in_area = in_area;
                c.inside = inside;
            }
        }
    }

    let mut unresolved: Vec<UnresolvedOut> = Vec::new();
    let mut referenced: Vec<usize> = Vec::new();

    // 3. Procedures of airports inside the ARTCC.
    let mut procs: BTreeMap<(usize, u8, String), ProcOut> = BTreeMap::new();
    for p in &src.cifp.procedures {
        let Some(&ai) = maps.apt_cifp.get(&p.airport_ident) else {
            continue;
        };
        if !reg.cands.get(ai).is_some_and(|c| c.inside) {
            continue;
        }
        let kind = match p.kind {
            c424::ProcedureKind::Sid => proc_kind::SID,
            c424::ProcedureKind::Star => proc_kind::STAR,
            c424::ProcedureKind::Approach => proc_kind::APPROACH,
        };
        let key = (ai, kind, p.ident.clone());
        if procs.contains_key(&key) {
            info.duplicates.push(format!(
                "CIFP procedure {} {} at {} occurs twice; the second is dropped",
                proc_kind::name(kind),
                p.ident,
                p.airport_ident
            ));
            continue;
        }
        let mut routes: Vec<RouteOut> = Vec::new();
        let mut ordinal = 0u32;
        for r in &p.routes {
            let mut legs = Vec::with_capacity(r.legs.len());
            for l in &r.legs {
                let lo = cifp_leg(
                    l,
                    ordinal,
                    ai,
                    &maps,
                    &p.ident,
                    &p.airport_ident,
                    &mut unresolved,
                    &mut info,
                )?;
                referenced.extend(lo.fix.iter().chain(lo.center.iter()));
                legs.push(lo);
                ordinal += 1;
            }
            routes.push(RouteOut {
                route_type: u8::try_from(r.route_type).unwrap_or(b'?'),
                transition: r.transition.clone(),
                legs,
            });
        }
        procs.insert(
            key,
            ProcOut {
                airport: ai,
                kind,
                ident: p.ident.clone(),
                name: None,
                nasr: false,
                cifp: true,
                coded: true,
                routes,
            },
        );
    }
    // NASR procedures: join with CIFP ones, or enter as uncoded procedures (UK-003).
    for (pk, list) in [
        (nasr::procedure::ProcedureKind::Star, &src.nasr.stars),
        (nasr::procedure::ProcedureKind::Dp, &src.nasr.dps),
    ] {
        for p in list {
            let kind = match (pk, p.base.graphical_dp_type.as_deref()) {
                (nasr::procedure::ProcedureKind::Star, _) => proc_kind::STAR,
                (_, Some("OBSTACLE")) => proc_kind::ODP,
                _ => proc_kind::SID,
            };
            let ident = nasr_proc_ident(pk, &p.base);
            let name = format!("{} {}", p.base.name, p.base.amendment_no);
            let mut served: Vec<&str> = p.airports.iter().map(|a| a.arpt_id.as_str()).collect();
            served.extend(nasr::procedure::served_airports(&p.base));
            served.sort_unstable();
            served.dedup();
            for faa in served {
                let Some(&ai) = maps.apt_faa.get(faa) else {
                    continue;
                };
                if !reg.cands.get(ai).is_some_and(|c| c.inside) {
                    continue;
                }
                let key = (ai, kind, ident.clone());
                if let Some(existing) = procs.get_mut(&key) {
                    if existing.cifp {
                        existing.nasr = true;
                        existing.name = Some(name.clone());
                    }
                    continue;
                }
                let airport_ident = reg
                    .cands
                    .get(ai)
                    .map(|c| c.ident.clone())
                    .unwrap_or_default();
                let mut routes: Vec<RouteOut> = Vec::new();
                for r in &p.routes {
                    let mentions = |pt: &nasr::procedure::RoutePoint| {
                        nasr::procedure::associations(pt)
                            .iter()
                            .any(|(a, _)| *a == faa)
                    };
                    let any_assoc = r.points.iter().any(|pt| pt.arpt_rwy_assoc.is_some());
                    if any_assoc && !r.points.iter().any(mentions) {
                        continue;
                    }
                    let mut legs = Vec::with_capacity(r.points.len());
                    for (ord, pt) in r.points.iter().enumerate() {
                        let rf = NasrRef {
                            name: &pt.point,
                            ptype: &pt.point_type,
                            region: pt.icao_region_code.as_deref(),
                            nav_name: None,
                            nav_city: None,
                            state: None,
                        };
                        let fix = match resolve_nasr(src, &maps, &rf) {
                            Res::Found(i) => {
                                referenced.push(i);
                                Some(i)
                            }
                            Res::Ambiguous | Res::NotFound => {
                                unresolved.push(UnresolvedOut {
                                    role: unresolved_role::ROUTE_POINT,
                                    owner_kind: owner_kind::PROCEDURE,
                                    owner: ident.clone(),
                                    scope: airport_ident.clone(),
                                    ordinal: ord as u32,
                                    name: pt.point.clone(),
                                    region: pt.icao_region_code.clone().unwrap_or_default(),
                                    wanted: pt.point_type.trim().to_string(),
                                });
                                None
                            }
                        };
                        legs.push(LegOut {
                            fix,
                            center: None,
                            path_term: path_term::NONE,
                            turn: turn::NONE,
                            course: NONE_U16,
                            dist_or_time: NONE_U32,
                            is_time: false,
                            alt_desc: alt_desc::NONE,
                            alt1: ALT_NONE,
                            alt2: ALT_NONE,
                            speed: NONE_U16,
                        });
                    }
                    routes.push(RouteOut {
                        route_type: 0,
                        transition: r.route_name.clone(),
                        legs,
                    });
                }
                info.uncoded_procedures.push(format!(
                    "{} {} {}",
                    airport_ident,
                    proc_kind::name(kind),
                    ident
                ));
                procs.insert(
                    key,
                    ProcOut {
                        airport: ai,
                        kind,
                        ident: ident.clone(),
                        name: Some(name.clone()),
                        nasr: true,
                        cifp: false,
                        coded: false,
                        routes,
                    },
                );
            }
        }
    }

    // 4. Airways with a point in the area.
    let mut airways: Vec<AirwayOut<'_>> = Vec::new();
    for a in &src.nasr.airways {
        let refs: Vec<Res> = a
            .segments
            .iter()
            .map(|s| {
                resolve_nasr(
                    src,
                    &maps,
                    &NasrRef {
                        name: &s.from_point,
                        ptype: s.from_pt_type.as_deref().unwrap_or(""),
                        region: s.icao_region_code.as_deref(),
                        nav_name: s.nav_name.as_deref(),
                        nav_city: s.nav_city.as_deref(),
                        state: s.state_code.as_deref(),
                    },
                )
            })
            .collect();
        let found = |i: usize| match refs.get(i) {
            Some(Res::Found(c)) => Some(*c),
            _ => None,
        };
        if !(0..refs.len()).any(|i| {
            found(i)
                .and_then(|c| reg.cands.get(c))
                .is_some_and(|c| c.in_area)
        }) {
            continue;
        }
        let joined = |i: usize| match (a.segments.get(i), a.segments.get(i + 1)) {
            (Some(s), Some(n)) => {
                n.awy_id == a.base.awy_id
                    && n.awy_location == a.base.awy_location
                    && s.awy_id == a.base.awy_id
                    && s.awy_location == a.base.awy_location
                    && n.point_seq > s.point_seq
                    && s.to_point
                        .as_deref()
                        .is_some_and(|t| t.trim().eq_ignore_ascii_case(n.from_point.trim()))
            }
            _ => false,
        };
        let mut legs = Vec::new();
        for (i, s) in a.segments.iter().enumerate() {
            let has_next = s.to_point.is_some();
            if found(i).is_none() {
                unresolved.push(UnresolvedOut {
                    role: if is_border_crossing_name(&s.from_point) {
                        unresolved_role::BORDER_CROSSING
                    } else if has_next {
                        unresolved_role::AIRWAY_FROM
                    } else {
                        unresolved_role::AIRWAY_TO
                    },
                    owner_kind: owner_kind::AIRWAY,
                    owner: a.base.awy_id.clone(),
                    scope: a.base.awy_location.clone(),
                    ordinal: i as u32,
                    name: s.from_point.clone(),
                    region: s.icao_region_code.clone().unwrap_or_default(),
                    wanted: s
                        .from_pt_type
                        .clone()
                        .unwrap_or_default()
                        .trim()
                        .to_string(),
                });
            }
            if has_next {
                // The leg ends at the next row only when that row is the point this row names as
                // its end, on the same airway, further along it (OA-ASP-010). A row after a
                // break in the airway is not the end of this leg. A segment flagged as a gap is
                // still a leg (the flag is stored with it); the flag does not move its end.
                let end = if joined(i) {
                    found(i + 1)
                } else {
                    info.airway_breaks.push(format!(
                        "airway {} {} seq {} (line {}): end point {:?} is not the next row of the airway{}",
                        a.base.awy_id,
                        a.base.awy_location,
                        s.point_seq,
                        s.line,
                        s.to_point.as_deref().unwrap_or(""),
                        if s.awy_seg_gap_flag { " (gap flagged)" } else { "" },
                    ));
                    None
                };
                legs.push((found(i), end, i));
            }
            referenced.extend(found(i));
        }
        airways.push(AirwayOut {
            src: a,
            designator: a.base.awy_id.clone(),
            location: a.base.awy_location.bytes().next().unwrap_or(b'?'),
            legs,
        });
    }

    for i in referenced {
        if let Some(c) = reg.cands.get_mut(i) {
            c.referenced = true;
        }
    }

    // 5. Boundaries: NASR, then CIFP class airspace and SUA.
    let mut boundaries: Vec<BoundaryOut> = Vec::new();
    let mut unverified_limits = 0usize;
    for b in &src.nasr.boundaries {
        let Some(stratum) = stratum_of(&b.altitude, &b.boundary_type) else {
            info.boundary_notes.push(format!(
                "{} {} {}: altitude/type combination not modeled, skipped",
                b.location_id, b.altitude, b.boundary_type
            ));
            continue;
        };
        let is_own = b.location_id == params.artcc
            && (stratum == stratum::ARTCC_HIGH || stratum == stratum::ARTCC_LOW);
        let mut verts = Vec::with_capacity(b.points.len());
        let mut ring = Vec::with_capacity(b.points.len());
        for p in &b.points {
            let (la, lo) = store_pos(
                &ell,
                &mut info.rounding,
                "boundary vertex",
                nasr_pos(&p.position),
            )?;
            verts.push(PosE7 { lat: la, lon: lo });
            ring.push(latlon_e7(la, lo).map_err(geo("reading a boundary vertex"))?);
        }
        if !is_own {
            let samples = match ring_samples(&ell, &ring) {
                Ok(s) => s,
                Err(e) => {
                    info.boundary_notes.push(format!(
                        "{} {} {}: cannot be sampled ({e:?}), skipped",
                        b.location_id, b.altitude, b.boundary_type
                    ));
                    continue;
                }
            };
            if !samples_touch_area(&area, &samples)
                .map_err(geo("testing a boundary against the area"))?
            {
                continue;
            }
        }
        boundaries.push(BoundaryOut {
            source: boundary_source::NASR_ARB,
            stratum,
            multiple: 0,
            facility: b.location_id.clone(),
            name: b.location_name.clone().unwrap_or_default(),
            floor: ALT_NONE,
            ceiling: ALT_NONE,
            floor_agl: false,
            ceiling_agl: false,
            verts,
            segs: Vec::new(),
        });
    }
    let mut open_volumes = 0usize;
    for (list, is_sua) in [
        (&src.cifp.controlled_airspace, false),
        (&src.cifp.special_use_airspace, true),
    ] {
        for v in list {
            let mut segs: Vec<SegOut> = Vec::with_capacity(v.boundary.len());
            let mut ins: Vec<SegIn> = Vec::with_capacity(v.boundary.len());
            let mut ok = true;
            for p in &v.boundary {
                let ll = |pos: Option<c424::Position>,
                          what: &str,
                          r: &mut Rounding|
                 -> Result<(i32, i32), BuildError> {
                    match pos {
                        Some(q) => store_pos(&ell, r, what, (q.lat, q.lon)),
                        None => Ok((POS_NONE, POS_NONE)),
                    }
                };
                let (la, lo) = ll(p.position, "airspace boundary point", &mut info.rounding)?;
                let (ala, alo) = ll(p.arc_origin, "airspace arc centre", &mut info.rounding)?;
                let via = match p.via {
                    c424::BoundaryVia::Circle => via::CIRCLE,
                    c424::BoundaryVia::GreatCircle => via::GEODESIC,
                    c424::BoundaryVia::RhumbLine => via::RHUMB,
                    c424::BoundaryVia::CounterClockwiseArc => via::ARC_CCW,
                    c424::BoundaryVia::ClockwiseArc => via::ARC_CW,
                };
                let radius = p.arc_distance_nm.and_then(|nm| round_u32(nm * NM_M));
                // A circle or an arc without a radius has no shape: refuse it rather than
                // store a radius of zero (OA-ASP-009).
                let radius = match radius {
                    Some(r) if r > 0 => r,
                    _ if matches!(via, via::GEODESIC | via::RHUMB) => radius.unwrap_or(0),
                    _ => {
                        return Err(BuildError::BadRecord {
                            location: format!(
                                "CIFP line {} airspace {} {} (boundary sequence {})",
                                v.meta.line, v.designation, v.multiple_code, p.sequence
                            ),
                            reason: "circle or arc has a missing, unusable or zero radius"
                                .to_string(),
                        })
                    }
                };
                ins.push(SegIn {
                    via,
                    pos: (la != POS_NONE).then(|| latlon_e7(la, lo).ok()).flatten(),
                    centre: (ala != POS_NONE)
                        .then(|| latlon_e7(ala, alo).ok())
                        .flatten(),
                    radius_m: f64::from(radius),
                });
                segs.push(SegOut {
                    via,
                    lat: la,
                    lon: lo,
                    arc_lat: ala,
                    arc_lon: alo,
                    radius_m: radius,
                    bearing_cdeg: p
                        .arc_bearing_deg
                        .and_then(|d| round_u32(d * 100.0))
                        .and_then(|x| u16::try_from(x).ok())
                        .unwrap_or(NONE_U16),
                });
                ok &= via == via::CIRCLE || la != POS_NONE;
            }
            if !ok || segs.is_empty() {
                info.boundary_notes.push(format!(
                    "CIFP {} {}: boundary point missing, skipped",
                    v.designation, v.multiple_code
                ));
                continue;
            }
            if !c424::is_closed(v) {
                open_volumes += 1;
            }
            let samples =
                segment_samples(&ell, &ins).map_err(geo("sampling an airspace volume"))?;
            if !samples_touch_area(&area, &samples)
                .map_err(geo("testing an airspace volume against the area"))?
            {
                continue;
            }
            let (floor, floor_agl) = limit_ft(v.lower, &mut unverified_limits);
            let (ceiling, ceiling_agl) = limit_ft(v.upper, &mut unverified_limits);
            let stratum = if is_sua {
                sua_stratum(v.airspace_type)
            } else {
                match v.classification {
                    Some('B') => stratum::CLASS_B,
                    Some('C') => stratum::CLASS_C,
                    Some('D') => stratum::CLASS_D,
                    _ => stratum::CLASS_OTHER,
                }
            };
            boundaries.push(BoundaryOut {
                source: if is_sua {
                    boundary_source::CIFP_UR
                } else {
                    boundary_source::CIFP_UC
                },
                stratum,
                multiple: u8::try_from(v.multiple_code).unwrap_or(b'?'),
                facility: v.designation.clone(),
                name: v.name.clone(),
                floor,
                ceiling,
                floor_agl,
                ceiling_agl,
                verts: Vec::new(),
                segs,
            });
        }
    }
    if open_volumes > 0 {
        info.boundary_notes.push(format!("{open_volumes} selected CIFP volumes are not closed (no end marker); closed implicitly"));
    }
    if unverified_limits > 0 {
        info.boundary_notes.push(format!("{unverified_limits} airspace limits have an unrecognised altitude unit; stored as unknown"));
    }
    boundaries.sort_by(|a, b| {
        (a.source, a.stratum, &a.facility, a.multiple, &a.name).cmp(&(
            b.source,
            b.stratum,
            &b.facility,
            b.multiple,
            &b.name,
        ))
    });
    let before = boundaries.len();
    boundaries.dedup_by(|b, a| {
        (a.source, a.stratum, &a.facility, a.multiple, &a.name)
            == (b.source, b.stratum, &b.facility, b.multiple, &b.name)
    });
    if boundaries.len() != before {
        info.duplicates.push(format!(
            "{} boundaries repeat another's key; the repeats are dropped",
            before - boundaries.len()
        ));
    }

    // 6. MORA cells overlapping the area.
    let mut mora: BTreeMap<(i16, i16), i16> = BTreeMap::new();
    let mut south = 0usize;
    let mut mora_range = 0usize;
    for row in &src.cifp.mora {
        if row.start_lat < 0 {
            south += 1;
            continue;
        }
        for (i, cell) in row.cells.iter().enumerate() {
            let lon = i32::from(row.start_lon) + i as i32;
            if !(-180..180).contains(&lon) {
                continue;
            }
            if !cell_overlaps_area(&area, i32::from(row.start_lat), lon)
                .map_err(geo("testing a MORA cell"))?
            {
                continue;
            }
            let value = match cell {
                None => MORA_UNKNOWN,
                Some(h) => match i16::try_from(*h) {
                    Ok(v) if v != MORA_UNKNOWN => v,
                    _ => {
                        mora_range += 1;
                        MORA_UNKNOWN
                    }
                },
            };
            let key = (row.start_lat, lon as i16);
            if let Some(prev) = mora.get(&key) {
                if *prev != value {
                    info.duplicates.push(format!(
                        "MORA cell {},{} occurs twice with different values; the first is kept",
                        key.0, key.1
                    ));
                }
                continue;
            }
            mora.insert(key, value);
        }
    }
    if south > 0 {
        info.skipped.push((
            "CIFP MORA rows in the southern hemisphere (convention unverified, UK-002)".to_string(),
            south,
        ));
    }
    if mora_range > 0 {
        info.skipped.push((
            "MORA cells with a value that does not fit the table (stored as unknown)".to_string(),
            mora_range,
        ));
    }

    // 7. Assembly.
    // Points included: in the area, referenced, or owned by an included airport.
    let mut order: Vec<usize> = (0..reg.cands.len())
        .filter(|&i| reg.cands.get(i).is_some_and(|c| c.in_area || c.referenced))
        .collect();
    order.sort_by(|&a, &b| {
        let (Some(x), Some(y)) = (reg.cands.get(a), reg.cands.get(b)) else {
            return std::cmp::Ordering::Equal;
        };
        let k = |c: &Cand| {
            (
                c.ident.clone(),
                c.region.clone().unwrap_or_default(),
                c.scope.clone().unwrap_or_default(),
                point_kind::class(c.kind),
            )
        };
        k(x).cmp(&k(y))
    });
    let mut ix_of: Vec<u32> = vec![NONE_U32; reg.cands.len()];
    for (ix, &ci) in order.iter().enumerate() {
        if let Some(slot) = ix_of.get_mut(ci) {
            *slot = u32::try_from(ix)
                .map_err(|_| BuildError::Unstorable("more than 2^32 points".to_string()))?;
        }
    }
    let point = |ci: Option<usize>| -> PointIx {
        ci.and_then(|c| ix_of.get(c).copied())
            .map_or(PointIx::NONE, PointIx::new)
    };

    let mut strings = Interner::default();
    let mut model = empty_model(Meta {
        nasr_effective: nasr_eff,
        cifp_cycle,
        artcc: params.artcc.clone(),
        buffer_m: params.buffer_m,
        nasr_sha256: src.nasr_archive.sha256,
        cifp_sha256: src.cifp_archive.sha256,
    });

    let n_points = order.len();
    let pt = &mut model.points;
    pt.src_nasr = bit_zeros(n_points);
    pt.src_cifp = bit_zeros(n_points);
    pt.in_area = bit_zeros(n_points);
    pt.cls_b = bit_zeros(n_points);
    pt.cls_c = bit_zeros(n_points);
    pt.cls_d = bit_zeros(n_points);
    pt.cls_e = bit_zeros(n_points);
    let mut disagreements: Vec<(u32, (i32, i32), u32)> = Vec::new();
    for (row, &ci) in order.iter().enumerate() {
        let Some(c) = reg.cands.get(ci) else { continue };
        let (la, lo) = store_pos(
            &ell,
            &mut info.rounding,
            &format!("{} {}", point_kind::name(c.kind), c.ident),
            c.pos,
        )?;
        pt.kind.push(c.kind);
        pt.lat.push(la);
        pt.lon.push(lo);
        pt.elev_ft.push(c.elev.unwrap_or(ELEV_NONE));
        pt.ident.push(strings.intern(&c.ident));
        pt.alt_ident
            .push(strings.intern_opt(c.alt_ident.as_deref()));
        pt.region.push(strings.intern_opt(c.region.as_deref()));
        pt.scope.push(strings.intern_opt(c.scope.as_deref()));
        pt.name.push(strings.intern_opt(c.name.as_deref()));
        bit_set(&mut pt.src_nasr, row, c.nasr);
        bit_set(&mut pt.src_cifp, row, c.cifp);
        bit_set(&mut pt.in_area, row, c.in_area);
        bit_set(&mut pt.cls_b, row, c.cls & 1 != 0);
        bit_set(&mut pt.cls_c, row, c.cls & 2 != 0);
        bit_set(&mut pt.cls_d, row, c.cls & 4 != 0);
        bit_set(&mut pt.cls_e, row, c.cls & 8 != 0);
        if let (Some(n), Some(f)) = (c.nasr_pos, c.cifp_pos) {
            let d = pos_distance(&ell, n, f);
            if d.is_nan() || d > DISAGREE_M {
                let (nla, nlo) = store_pos(
                    &ell,
                    &mut info.rounding,
                    "NASR position of a disagreement",
                    n,
                )?;
                let mm = if d.is_nan() {
                    u32::MAX
                } else {
                    round_u32(d * 1000.0).unwrap_or(u32::MAX)
                };
                disagreements.push((row as u32, (nla, nlo), mm));
            }
        }
        if let Some(d) = &c.defect {
            info.position_defects.push(PositionDefect {
                file: d.file,
                line: d.line,
                ident: c.ident.clone(),
                diff_deg: d.diff_deg,
            });
        }
        // Class airspace cross-check (selected airports only).
        if c.kind == point_kind::AIRPORT && c.nasr {
            let has = |stratum: u8| {
                boundaries.iter().any(|b| {
                    b.source == boundary_source::CIFP_UC
                        && b.stratum == stratum
                        && b.facility == c.ident
                })
            };
            for (bit, st, label) in [
                (1u8, stratum::CLASS_B, "B"),
                (2, stratum::CLASS_C, "C"),
                (4, stratum::CLASS_D, "D"),
            ] {
                let nasr_says = c.cls & bit != 0;
                let cifp_has = has(st);
                if c.in_area && nasr_says != cifp_has {
                    info.class_mismatches.push(format!(
                        "{}: NASR CLS_ARSP {} class {label}, CIFP {} a class {label} volume in the model",
                        c.ident,
                        if nasr_says { "has" } else { "has no" },
                        if cifp_has { "has" } else { "has no" },
                    ));
                }
            }
        }
    }
    disagreements.sort_unstable();
    for (p, (la, lo), mm) in disagreements {
        model.disagreements.point.push(PointIx::new(p));
        model.disagreements.nasr_lat.push(la);
        model.disagreements.nasr_lon.push(lo);
        model.disagreements.dist_mm.push(mm);
    }
    info.position_defects
        .sort_by(|a, b| (a.file, a.line).cmp(&(b.file, b.line)));
    info.candidates
        .push(("points in the model".to_string(), n_points));

    // Boundaries.
    {
        let b = &mut model.boundaries;
        b.floor_agl = bit_zeros(boundaries.len());
        b.ceiling_agl = bit_zeros(boundaries.len());
        for (row, bo) in boundaries.iter().enumerate() {
            b.source.push(bo.source);
            b.stratum.push(bo.stratum);
            b.edge_interp
                .push(if bo.source == boundary_source::NASR_ARB {
                    edge_interp::UNKNOWN
                } else {
                    edge_interp::PER_SEGMENT
                });
            b.multiple.push(bo.multiple);
            b.facility.push(strings.intern(&bo.facility));
            b.name
                .push(strings.intern_opt(Some(bo.name.as_str()).filter(|n| !n.is_empty())));
            b.floor_ft.push(bo.floor);
            b.ceiling_ft.push(bo.ceiling);
            b.vertices.push(Span {
                start: VertexIx::new(model.vertices.len() as u32),
                len: bo.verts.len() as u32,
            });
            model.vertices.extend_from_slice(&bo.verts);
            b.segs.push(Span {
                start: SegIx::new(model.segments.via.len() as u32),
                len: bo.segs.len() as u32,
            });
            for s in &bo.segs {
                let g = &mut model.segments;
                g.via.push(s.via);
                g.lat.push(s.lat);
                g.lon.push(s.lon);
                g.arc_lat.push(s.arc_lat);
                g.arc_lon.push(s.arc_lon);
                g.radius_m.push(s.radius_m);
                g.bearing_cdeg.push(s.bearing_cdeg);
            }
            bit_set(&mut b.floor_agl, row, bo.floor_agl);
            bit_set(&mut b.ceiling_agl, row, bo.ceiling_agl);
        }
    }

    // Airways.
    airways.sort_by(|a, b| (&a.designator, a.location).cmp(&(&b.designator, b.location)));
    {
        let n_legs: usize = airways.iter().map(|a| a.legs.len()).sum();
        let l = &mut model.airway_legs;
        l.gap = bit_zeros(n_legs);
        l.signal_gap = bit_zeros(n_legs);
        l.dogleg = bit_zeros(n_legs);
        let mut leg_row = 0usize;
        for a in &airways {
            model.airways.designator.push(strings.intern(&a.designator));
            model.airways.location.push(a.location);
            model.airways.legs.push(Span {
                start: AirwayLegIx::new(leg_row as u32),
                len: a.legs.len() as u32,
            });
            for (from, to, si) in &a.legs {
                let Some(seg) = a.src.segments.get(*si) else {
                    continue;
                };
                l.from.push(point(*from));
                l.to.push(point(*to));
                l.mea_ft.push(
                    seg.min_enroute_alt
                        .alt
                        .and_then(|v| i32::try_from(v).ok())
                        .unwrap_or(ALT_NONE),
                );
                l.mea_opp_ft.push(
                    seg.min_enroute_alt_opposite
                        .alt
                        .and_then(|v| i32::try_from(v).ok())
                        .unwrap_or(ALT_NONE),
                );
                l.gps_mea_ft.push(
                    seg.gps_min_enroute_alt
                        .alt
                        .and_then(|v| i32::try_from(v).ok())
                        .unwrap_or(ALT_NONE),
                );
                l.mea_dir
                    .push(compass_code(seg.min_enroute_alt.dir.as_deref()));
                l.gps_mea_dir
                    .push(compass_code(seg.gps_min_enroute_alt.dir.as_deref()));
                bit_set(&mut l.gap, leg_row, seg.awy_seg_gap_flag);
                bit_set(&mut l.signal_gap, leg_row, seg.signal_gap_flag);
                bit_set(&mut l.dogleg, leg_row, seg.dogleg);
                leg_row += 1;
            }
        }
    }

    // Procedures: sorted by (airport ident, kind, ident).
    let mut proc_list: Vec<&ProcOut> = procs.values().collect();
    proc_list.sort_by(|a, b| {
        let ai = |p: &ProcOut| {
            reg.cands
                .get(p.airport)
                .map(|c| c.ident.clone())
                .unwrap_or_default()
        };
        (ai(a), a.kind, &a.ident).cmp(&(ai(b), b.kind, &b.ident))
    });
    {
        let n = proc_list.len();
        let pr = &mut model.procs;
        pr.src_nasr = bit_zeros(n);
        pr.src_cifp = bit_zeros(n);
        pr.coded = bit_zeros(n);
        let mut dist_is_time: Vec<bool> = Vec::new();
        for (row, p) in proc_list.iter().enumerate() {
            pr.kind.push(p.kind);
            pr.airport.push(point(Some(p.airport)));
            pr.ident.push(strings.intern(&p.ident));
            pr.name.push(strings.intern_opt(p.name.as_deref()));
            pr.routes.push(Span {
                start: RouteIx::new(model.routes.route_type.len() as u32),
                len: p.routes.len() as u32,
            });
            bit_set(&mut pr.src_nasr, row, p.nasr);
            bit_set(&mut pr.src_cifp, row, p.cifp);
            bit_set(&mut pr.coded, row, p.coded);
            let mut routes: Vec<&RouteOut> = p.routes.iter().collect();
            routes
                .sort_by(|a, b| (&a.transition, a.route_type).cmp(&(&b.transition, b.route_type)));
            for r in routes {
                model.routes.route_type.push(r.route_type);
                model.routes.transition.push(
                    strings.intern_opt(Some(r.transition.as_str()).filter(|t| !t.is_empty())),
                );
                model.routes.legs.push(Span {
                    start: LegIx::new(model.legs.fix.len() as u32),
                    len: r.legs.len() as u32,
                });
                for l in &r.legs {
                    let g = &mut model.legs;
                    g.fix.push(point(l.fix));
                    g.center.push(point(l.center));
                    g.path_term.push(l.path_term);
                    g.turn.push(l.turn);
                    g.course_mag_cdeg.push(l.course);
                    g.dist_or_time.push(l.dist_or_time);
                    g.alt_desc.push(l.alt_desc);
                    g.alt1_ft.push(l.alt1);
                    g.alt2_ft.push(l.alt2);
                    g.speed_kt.push(l.speed);
                    dist_is_time.push(l.is_time);
                }
            }
        }
        model.legs.dist_is_time = bit_zeros(dist_is_time.len());
        for (i, t) in dist_is_time.iter().enumerate() {
            bit_set(&mut model.legs.dist_is_time, i, *t);
        }
    }

    // MORA.
    for ((lat, lon), v) in &mora {
        model.mora.lat.push(*lat);
        model.mora.lon.push(*lon);
        model.mora.hundreds_ft.push(*v);
    }

    // Unresolved references, sorted.
    unresolved.sort_by(|a, b| {
        (a.owner_kind, &a.owner, &a.scope, a.ordinal, a.role, &a.name).cmp(&(
            b.owner_kind,
            &b.owner,
            &b.scope,
            b.ordinal,
            b.role,
            &b.name,
        ))
    });
    unresolved.dedup_by(|b, a| {
        (a.owner_kind, &a.owner, &a.scope, a.ordinal, a.role, &a.name)
            == (b.owner_kind, &b.owner, &b.scope, b.ordinal, b.role, &b.name)
    });
    for u in &unresolved {
        let t = &mut model.unresolved;
        t.role.push(u.role);
        t.owner_kind.push(u.owner_kind);
        t.owner.push(strings.intern(&u.owner));
        t.scope
            .push(strings.intern_opt(Some(u.scope.as_str()).filter(|s| !s.is_empty())));
        t.ordinal.push(u.ordinal);
        t.name.push(strings.intern(&u.name));
        t.region
            .push(strings.intern_opt(Some(u.region.as_str()).filter(|s| !s.is_empty())));
        t.wanted
            .push(strings.intern_opt(Some(u.wanted.as_str()).filter(|s| !s.is_empty())));
    }

    model.strings = strings.table;
    validate_model(&model).map_err(BuildError::Model)?;
    info.duplicates.sort();
    info.uncoded_procedures.sort();
    info.class_mismatches.sort();
    info.kind_mismatches.sort();
    info.boundary_notes.sort();
    Ok(BuildOutput { model, info })
}

/// Builds the string table: identical strings are stored once, in first-use order.
#[derive(Default)]
// A7.2: cold struct, size guard waived
struct Interner {
    table: StrTable,
    map: HashMap<String, u32>,
}

impl Interner {
    fn intern(&mut self, s: &str) -> StrIx {
        if let Some(&i) = self.map.get(s) {
            return StrIx::new(i);
        }
        let i = u32::try_from(str_count(&self.table)).unwrap_or(NONE_U32);
        self.table.text.push_str(s);
        self.table
            .offsets
            .push(u32::try_from(self.table.text.len()).unwrap_or(NONE_U32));
        self.map.insert(s.to_string(), i);
        StrIx::new(i)
    }

    fn intern_opt(&mut self, s: Option<&str>) -> StrIx {
        s.map_or(StrIx::NONE, |s| self.intern(s))
    }
}
