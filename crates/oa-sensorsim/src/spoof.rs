//! ADS-B spoof injection with labelled ground truth (docs/M3_DESIGN.md section 4). Core (pure).
//!
//! Four transforms on an `oa_adsb::Reports` table, each returning a [`Labelled`]: the new table,
//! a label per row ([`LABEL_NONE`] for a row of the original traffic, otherwise the attack
//! kind) and the address of the real aircraft the row was aimed at ([`NONE_ADDR`] for a ghost).
//! They compose: every transform takes a `Labelled`, and [`from_clean`] starts a chain.
//!
//! * [`inject_ghost`]: a new aircraft on a straight, constant-speed track, with a new address.
//! * [`inject_drift`]: a real aircraft's reported position walks away from the truth at a
//!   constant rate and bearing.
//! * [`inject_replay`]: a real aircraft's past reports re-sent later, shifted in time.
//! * [`inject_duplicate`]: a second aircraft transmits under a real aircraft's address (the
//!   donor's reports re-addressed).
//!
//! Every attack number is a parameter; there are no defaults. Offsets are on a sphere of the
//! mean earth radius (the flat small-offset approximation), which is exact enough for the
//! kilometres involved and is the same in every transform. Injected rows are merged by time
//! (ties keep the original rows first). Rows that are changed or added are the labelled ones;
//! nothing else changes, and the confusion matrix ([`confusion`]) scores a detector against the
//! labels.

use oa_adsb::{reports_len, reports_push, reports_row, Reports, Row, NON_ICAO, ROW_EMPTY};

use crate::error::SimError;
use crate::geom::offset_deg;
use crate::plots::NONE_ADDR;

/// Label: a row of the original traffic.
pub const LABEL_NONE: u8 = 0;
/// Label: a ghost aircraft's row.
pub const LABEL_GHOST: u8 = 1;
/// Label: a drifted row of a real aircraft.
pub const LABEL_DRIFT: u8 = 2;
/// Label: a replayed row.
pub const LABEL_REPLAY: u8 = 3;
/// Label: a row sent under a duplicated address.
pub const LABEL_DUPLICATE: u8 = 4;
/// Number of labels.
pub const LABEL_KINDS: usize = 5;
/// Names of the labels, by value.
pub const LABEL_NAMES: [&str; LABEL_KINDS] = ["none", "ghost", "drift", "replay", "duplicate"];

/// Limit: rows a single ghost may add.
pub const MAX_GHOST_ROWS: u32 = 1_000_000;

/// A reports table with its spoof labels. Plain data (A1); A7.2: cold struct.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Labelled {
    pub reports: Reports,
    /// `LABEL_*` per row.
    pub label: Vec<u8>,
    /// Address of the real aircraft the row targets; [`NONE_ADDR`] for original rows and ghosts.
    pub victim: Vec<u32>,
}

/// A clean table: every row labelled [`LABEL_NONE`].
pub fn from_clean(reports: &Reports) -> Labelled {
    let n = reports_len(reports);
    Labelled {
        reports: reports.clone(),
        label: vec![LABEL_NONE; n],
        victim: vec![NONE_ADDR; n],
    }
}

fn check(l: &Labelled) -> Result<usize, SimError> {
    let n = reports_len(&l.reports);
    let cols = [
        l.reports.time_ns.len(),
        l.reports.lat_e7.len(),
        l.reports.lon_e7.len(),
        l.reports.callsign.len(),
        l.reports.track_rate_cdps.len(),
        l.label.len(),
        l.victim.len(),
    ];
    if cols.iter().any(|&c| c != n) {
        return Err(SimError::ColumnLength);
    }
    Ok(n)
}

/// Merges the rows of `base` and `added` by time; original rows come first at equal times.
fn merge(base: &Labelled, added: Vec<(Row, u8, u32)>) -> Result<Labelled, SimError> {
    let n = check(base)?;
    let mut rows: Vec<(Row, u8, u32)> = Vec::with_capacity(n + added.len());
    for i in 0..n {
        let (Some(r), Some(&l), Some(&v)) = (
            reports_row(&base.reports, i),
            base.label.get(i),
            base.victim.get(i),
        ) else {
            return Err(SimError::ColumnLength);
        };
        rows.push((r, l, v));
    }
    rows.extend(added);
    // Stable: equal times keep the order above (originals, then injected in the order given).
    rows.sort_by_key(|(r, _, _)| r.time_ns);
    let mut out = Labelled::default();
    for (r, l, v) in rows {
        reports_push(&mut out.reports, &r);
        out.label.push(l);
        out.victim.push(v);
    }
    Ok(out)
}

fn has_addr(l: &Labelled, addr: u32) -> bool {
    l.reports.icao.contains(&addr)
}

/// Degrees to 1e-7 degree, refused past `limit_e7` (900 000 000 for a latitude, 1 800 000 000
/// for a longitude) or not finite.
fn e7_within(deg: f64, limit_e7: i32) -> Result<i32, SimError> {
    let v = (deg * 1.0e7).round();
    if v.is_finite() && v.abs() <= f64::from(limit_e7) {
        Ok(v as i32)
    } else {
        Err(SimError::BadSpoof("position out of range"))
    }
}

fn e7_lat(deg: f64) -> Result<i32, SimError> {
    e7_within(deg, 900_000_000)
}

fn e7_lon(deg: f64) -> Result<i32, SimError> {
    e7_within(deg, 1_800_000_000)
}

/// A ghost aircraft: a new address and a straight constant-speed track. All fields explicit.
#[derive(Debug, Clone, Copy, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct GhostParams {
    /// The new address (24 bits, no `NON_ICAO` bit); must not occur in the traffic.
    pub addr: u32,
    /// Time of the first report, Unix ns.
    pub start_ns: i64,
    /// Interval between reports, ms.
    pub interval_ms: i64,
    /// Number of reports, 1 to [`MAX_GHOST_ROWS`].
    pub count: u32,
    pub lat_deg: f64,
    pub lon_deg: f64,
    /// Ground speed, knots.
    pub speed_kt: f64,
    /// Track, degrees true.
    pub track_deg: f64,
    pub alt_ft: i32,
    pub callsign: [u8; 8],
    pub squawk: u16,
    /// Emitter category byte, NIC, NACp, NACv, SIL and ADS-B version the ghost claims.
    pub category: u8,
    pub nic: u8,
    pub nac_p: u8,
    pub nac_v: u8,
    pub sil: u8,
    pub version: u8,
}

/// Adds a ghost aircraft. Refuses an address that already reports, an address past 24 bits, an
/// interval of zero or less, a count outside its bounds, a duration or last time that overflows, a speed outside 0 to 1500 kt and a
/// position out of range.
pub fn inject_ghost(base: &Labelled, g: &GhostParams) -> Result<Labelled, SimError> {
    check(base)?;
    if g.addr > 0xFF_FFFF {
        return Err(SimError::BadSpoof("ghost address past 24 bits"));
    }
    if has_addr(base, g.addr) {
        return Err(SimError::BadSpoof("ghost address already in the traffic"));
    }
    if g.interval_ms <= 0 || g.count == 0 || g.count > MAX_GHOST_ROWS {
        return Err(SimError::BadSpoof("ghost interval or count"));
    }
    if !(g.speed_kt.is_finite() && (0.0..=1500.0).contains(&g.speed_kt))
        || !g.track_deg.is_finite()
        || !g.lat_deg.is_finite()
        || !g.lon_deg.is_finite()
    {
        return Err(SimError::BadSpoof("ghost speed, track or position"));
    }
    e7_lat(g.lat_deg)?;
    e7_lon(g.lon_deg)?;
    // The whole duration and the last timestamp must fit before any row is made.
    let span_ms = i64::from(g.count - 1)
        .checked_mul(g.interval_ms)
        .ok_or(SimError::BadSpoof("ghost duration overflows"))?;
    span_ms
        .checked_mul(1_000_000)
        .and_then(|ns| g.start_ns.checked_add(ns))
        .ok_or(SimError::BadSpoof("ghost duration overflows"))?;
    let v_mps = g.speed_kt * 1852.0 / 3600.0;
    let (sin_t, cos_t) = g.track_deg.to_radians().sin_cos();
    let trk = g.track_deg.rem_euclid(360.0);
    let mut added = Vec::with_capacity(g.count as usize);
    for k in 0..g.count {
        let dt_ms = i64::from(k) * g.interval_ms;
        let d = v_mps * dt_ms as f64 / 1000.0;
        let (lat, lon) = offset_deg(g.lat_deg, g.lon_deg, d * sin_t, d * cos_t);
        let mut r = ROW_EMPTY;
        r.icao = g.addr;
        r.time_ns = g.start_ns.saturating_add(dt_ms.saturating_mul(1_000_000));
        r.lat_e7 = e7_lat(lat)?;
        r.lon_e7 = e7_lon(lon)?;
        r.baro_alt_ft = g.alt_ft;
        r.gs_dkt = (g.speed_kt * 10.0).round() as u16;
        r.track_ddeg = ((trk * 10.0).round() as u16) % 3600;
        r.callsign = g.callsign;
        r.squawk = g.squawk;
        r.category = g.category;
        r.nic = g.nic;
        r.nac_p = g.nac_p;
        r.nac_v = g.nac_v;
        r.sil = g.sil;
        r.version = g.version;
        r.source = 0;
        added.push((r, LABEL_GHOST, NONE_ADDR));
    }
    merge(base, added)
}

/// Gradual position drift on a real aircraft.
#[derive(Debug, Clone, Copy, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct DriftParams {
    pub victim: u32,
    /// The drift starts at this time (no offset yet), Unix ns.
    pub start_ns: i64,
    /// Reports after this time are not drifted (the spoofer stops).
    pub end_ns: i64,
    /// Offset growth, metres per second.
    pub rate_mps: f64,
    /// Direction of the offset, degrees true.
    pub bearing_deg: f64,
}

/// Drifts the victim's reports in `(start, end]` by `rate * (t - start)` along the bearing.
/// Refuses a victim with no reports in that window, a rate that is negative or not finite, and
/// an end not after the start.
pub fn inject_drift(base: &Labelled, d: &DriftParams) -> Result<Labelled, SimError> {
    let n = check(base)?;
    if d.end_ns <= d.start_ns || !d.rate_mps.is_finite() || d.rate_mps < 0.0 {
        return Err(SimError::BadSpoof("drift window or rate"));
    }
    if !d.bearing_deg.is_finite() {
        return Err(SimError::BadSpoof("drift bearing"));
    }
    let (sin_b, cos_b) = d.bearing_deg.to_radians().sin_cos();
    let mut out = base.clone();
    let mut hit = 0u64;
    for i in 0..n {
        let (Some(&a), Some(&t)) = (base.reports.icao.get(i), base.reports.time_ns.get(i)) else {
            return Err(SimError::ColumnLength);
        };
        if a != d.victim || t <= d.start_ns || t > d.end_ns {
            continue;
        }
        let off = d.rate_mps * (t - d.start_ns) as f64 * 1.0e-9;
        let (Some(&la), Some(&lo)) = (base.reports.lat_e7.get(i), base.reports.lon_e7.get(i))
        else {
            return Err(SimError::ColumnLength);
        };
        let (lat, lon) = offset_deg(
            f64::from(la) / 1.0e7,
            f64::from(lo) / 1.0e7,
            off * sin_b,
            off * cos_b,
        );
        if let (Some(x), Some(y), Some(l), Some(v)) = (
            out.reports.lat_e7.get_mut(i),
            out.reports.lon_e7.get_mut(i),
            out.label.get_mut(i),
            out.victim.get_mut(i),
        ) {
            *x = e7_lat(lat)?;
            *y = e7_lon(lon)?;
            *l = LABEL_DRIFT;
            *v = d.victim;
            hit += 1;
        }
    }
    if hit == 0 {
        return Err(SimError::BadSpoof(
            "drift victim has no reports in the window",
        ));
    }
    Ok(out)
}

/// Replay of a real aircraft's past reports.
#[derive(Debug, Clone, Copy, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct ReplayParams {
    pub victim: u32,
    /// The reports to copy: those of the victim in `[src_start_ns, src_end_ns]`.
    pub src_start_ns: i64,
    pub src_end_ns: i64,
    /// Shift forward in time, ns (must be positive: a replay is later than the original).
    pub shift_ns: i64,
    /// The address the replayed reports carry: the victim's own for a classic replay, another
    /// for a replay under a new identity.
    pub as_addr: u32,
}

/// Copies the victim's reports in the source window, shifted later by `shift_ns` and sent as
/// `as_addr`. Refuses a non-positive shift, an empty or inverted window, a window with no
/// victim reports and an address past 25 bits.
pub fn inject_replay(base: &Labelled, p: &ReplayParams) -> Result<Labelled, SimError> {
    let n = check(base)?;
    if p.shift_ns <= 0 || p.src_end_ns < p.src_start_ns {
        return Err(SimError::BadSpoof("replay shift or window"));
    }
    if p.as_addr >= (NON_ICAO << 1) {
        return Err(SimError::BadSpoof("replay address past 25 bits"));
    }
    let mut added = Vec::new();
    for i in 0..n {
        let Some(mut r) = reports_row(&base.reports, i) else {
            return Err(SimError::ColumnLength);
        };
        if r.icao != p.victim || r.time_ns < p.src_start_ns || r.time_ns > p.src_end_ns {
            continue;
        }
        r.time_ns = r.time_ns.saturating_add(p.shift_ns);
        r.icao = p.as_addr;
        added.push((r, LABEL_REPLAY, p.victim));
    }
    if added.is_empty() {
        return Err(SimError::BadSpoof(
            "replay victim has no reports in the window",
        ));
    }
    merge(base, added)
}

/// Address duplication: a donor aircraft's reports sent under the victim's address.
#[derive(Debug, Clone, Copy, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct DuplicateParams {
    /// The address that ends up on two aircraft; must already report.
    pub victim: u32,
    /// The aircraft whose reports are copied; must differ from the victim and report.
    pub donor: u32,
}

/// Copies every report of the donor, re-addressed to the victim. Refuses equal addresses and an
/// address that has no reports.
pub fn inject_duplicate(base: &Labelled, p: &DuplicateParams) -> Result<Labelled, SimError> {
    let n = check(base)?;
    if p.victim == p.donor {
        return Err(SimError::BadSpoof("duplicate donor equals victim"));
    }
    if !has_addr(base, p.victim) || !has_addr(base, p.donor) {
        return Err(SimError::BadSpoof("duplicate address not in the traffic"));
    }
    let mut added = Vec::new();
    for i in 0..n {
        let Some(mut r) = reports_row(&base.reports, i) else {
            return Err(SimError::ColumnLength);
        };
        if r.icao != p.donor || base.label.get(i).copied() != Some(LABEL_NONE) {
            continue;
        }
        r.icao = p.victim;
        added.push((r, LABEL_DUPLICATE, p.victim));
    }
    merge(base, added)
}

/// A detector scored against the labels: rows labelled spoofed (any label but [`LABEL_NONE`])
/// are the positives. Plain record (A1); A7.2: cold struct.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Confusion {
    pub tp: u64,
    pub fp: u64,
    pub tn: u64,
    pub fneg: u64,
    /// Spoofed rows per label ([`LABEL_NAMES`]); index 0 counts clean rows.
    pub rows: [u64; LABEL_KINDS],
    /// Of those, the rows the detector flagged.
    pub flagged: [u64; LABEL_KINDS],
}

/// Scores `flagged` (one bool per row) against the labels. Refuses a length mismatch and a
/// label above [`LABEL_DUPLICATE`].
pub fn confusion(label: &[u8], flagged: &[bool]) -> Result<Confusion, SimError> {
    if label.len() != flagged.len() {
        return Err(SimError::ColumnLength);
    }
    let mut c = Confusion::default();
    for (&l, &f) in label.iter().zip(flagged.iter()) {
        let k = usize::from(l);
        if k >= LABEL_KINDS {
            return Err(SimError::BadSpoof("label out of range"));
        }
        if let Some(x) = c.rows.get_mut(k) {
            *x += 1;
        }
        if f {
            if let Some(x) = c.flagged.get_mut(k) {
                *x += 1;
            }
        }
        match (l != LABEL_NONE, f) {
            (true, true) => c.tp += 1,
            (true, false) => c.fneg += 1,
            (false, true) => c.fp += 1,
            (false, false) => c.tn += 1,
        }
    }
    Ok(c)
}
