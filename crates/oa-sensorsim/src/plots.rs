//! The plot table and its CAT048 encoding. Core (pure).
//!
//! [`Plots`] is struct-of-arrays, one entry per plot in time order (A3). Alongside what a radar
//! reports (time, measured range and azimuth, altitude, codes) it keeps the labels a scorer
//! needs: the plot kind, the true aircraft and the noise-free, bias-free range and azimuth.
//! [`to_cat048`] writes the radar-visible part as ASTERIX CAT048 data blocks, one block per scan
//! (split at [`RECORDS_PER_BLOCK`]), with I048/020 SIM set; the encoder's own element
//! quantisation (range 1/256 NM, azimuth 360/2^16 degree, flight level 1/4) applies on top of
//! the profile's.

use oa_asterix::cat048::{Cat048, FlightLevel048, Polar, Trd048};
use oa_asterix::units::{CAT048_FL, CAT048_RHO_NM, CAT048_THETA_DEG};
use oa_asterix::{encode_block, ms_to_time128, Block, Mode3a, SourceId};

use crate::error::SimError;
use crate::geom::NM_M;

/// Limit: plots in one table, 64 Mi (a day over one terminal radar is a few million).
pub const MAX_PLOTS: usize = 64 * 1024 * 1024;
/// Limit: records written per CAT048 data block.
pub const RECORDS_PER_BLOCK: usize = 1000;
/// Absent address (the 24-bit address space leaves `u32::MAX` free).
pub const NONE_ADDR: u32 = u32::MAX;
/// Absent altitude.
pub const NONE_ALT: i32 = i32::MIN;
/// Absent Mode A code.
pub const NONE_CODE: u16 = u16::MAX;

/// Plot kind: a detection of a real aircraft.
pub const KIND_TRUE: u8 = 0;
/// Plot kind: a false plot, no aircraft behind it.
pub const KIND_FALSE: u8 = 1;
/// Plot kind: the second plot of a split target.
pub const KIND_SPLIT: u8 = 2;

/// Radar plots, struct-of-arrays. Entry `i` of every column is one plot.
#[derive(Debug, Clone, Default, PartialEq)]
// A7.2: cold struct (collection of columns built once, read-only after)
pub struct Plots {
    /// Plot time, Unix nanoseconds (the instant the beam crossed the target).
    pub time_ns: Vec<i64>,
    /// Scan index since the site's epoch (scan 0 starts at the epoch).
    pub scan: Vec<i64>,
    /// [`KIND_TRUE`], [`KIND_FALSE`] or [`KIND_SPLIT`].
    pub kind: Vec<u8>,
    /// Measured slant range, metres (noise, bias and quantum applied).
    pub range_m: Vec<f64>,
    /// Measured azimuth, degrees clockwise from true north, in `[0, 360)`.
    pub az_deg: Vec<f64>,
    /// Altitude after quantisation, feet; [`NONE_ALT`] if the plot has none.
    pub alt_ft: Vec<i32>,
    /// Mode A code (twelve bits, octal digits packed); [`NONE_CODE`] if absent.
    pub mode_a: Vec<u16>,
    /// Mode S address (24 bits); [`NONE_ADDR`] if absent.
    pub addr: Vec<u32>,
    /// Label: the truth aircraft key (with `oa_adsb::NON_ICAO` where set); [`NONE_ADDR`] for a
    /// false plot.
    pub truth_addr: Vec<u32>,
    /// Label: true slant range, metres (no noise, no bias); 0 for a false plot.
    pub truth_range_m: Vec<f64>,
    /// Label: true azimuth, degrees (no noise, no bias); 0 for a false plot.
    pub truth_az_deg: Vec<f64>,
}

/// One plot as a plain record, used while a run builds its rows. A7.2: cold, transient.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plot {
    pub time_ns: i64,
    pub scan: i64,
    pub kind: u8,
    pub range_m: f64,
    pub az_deg: f64,
    pub alt_ft: i32,
    pub mode_a: u16,
    pub addr: u32,
    pub truth_addr: u32,
    pub truth_range_m: f64,
    pub truth_az_deg: f64,
}

/// Number of plots.
pub fn plots_len(p: &Plots) -> usize {
    p.time_ns.len()
}

/// Appends one plot.
pub fn plots_push(p: &mut Plots, r: &Plot) {
    p.time_ns.push(r.time_ns);
    p.scan.push(r.scan);
    p.kind.push(r.kind);
    p.range_m.push(r.range_m);
    p.az_deg.push(r.az_deg);
    p.alt_ft.push(r.alt_ft);
    p.mode_a.push(r.mode_a);
    p.addr.push(r.addr);
    p.truth_addr.push(r.truth_addr);
    p.truth_range_m.push(r.truth_range_m);
    p.truth_az_deg.push(r.truth_az_deg);
}

/// Plot `i`, or `None` if out of range.
pub fn plots_row(p: &Plots, i: usize) -> Option<Plot> {
    Some(Plot {
        time_ns: *p.time_ns.get(i)?,
        scan: *p.scan.get(i)?,
        kind: *p.kind.get(i)?,
        range_m: *p.range_m.get(i)?,
        az_deg: *p.az_deg.get(i)?,
        alt_ft: *p.alt_ft.get(i)?,
        mode_a: *p.mode_a.get(i)?,
        addr: *p.addr.get(i)?,
        truth_addr: *p.truth_addr.get(i)?,
        truth_range_m: *p.truth_range_m.get(i)?,
        truth_az_deg: *p.truth_az_deg.get(i)?,
    })
}

fn columns_match(p: &Plots) -> bool {
    let n = p.time_ns.len();
    [
        p.scan.len(),
        p.kind.len(),
        p.range_m.len(),
        p.az_deg.len(),
        p.alt_ft.len(),
        p.mode_a.len(),
        p.addr.len(),
        p.truth_addr.len(),
        p.truth_range_m.len(),
        p.truth_az_deg.len(),
    ]
    .iter()
    .all(|&l| l == n)
}

/// One plot as a CAT048 record. Refuses a range past the 16-bit element, an altitude past the
/// 14-bit flight level and an address past 24 bits.
fn record_of(r: &Plot, source: SourceId) -> Result<Cat048, SimError> {
    let rho = (r.range_m / NM_M / CAT048_RHO_NM).round();
    if !(0.0..=65_535.0).contains(&rho) {
        return Err(SimError::Encode("I048/040 range"));
    }
    // 65 536 counts of azimuth wrap to 0.
    let theta = ((r.az_deg / CAT048_THETA_DEG).round() as u32 % 65_536) as u16;
    let ms_of_day = r.time_ns.div_euclid(1_000_000).rem_euclid(86_400_000) as u32;
    let typ = if r.addr != NONE_ADDR { 5 } else { 2 };
    let mut rec = Cat048 {
        source,
        time_of_day: Some(ms_to_time128(ms_of_day) % (86_400 * 128)),
        descriptor: Some(Trd048 {
            typ,
            simulated: true,
            ..Trd048::default()
        }),
        polar: Some(Polar {
            rho: rho as u16,
            theta,
        }),
        ..Cat048::default()
    };
    if r.mode_a != NONE_CODE {
        rec.mode3a = Some(Mode3a {
            code: r.mode_a & 0xFFF,
            ..Mode3a::default()
        });
    }
    if r.alt_ft != NONE_ALT {
        // Flight level is the altitude in 100 ft, 1/4 FL (25 ft) per count.
        let raw = (f64::from(r.alt_ft) / 100.0 / CAT048_FL).round();
        if !(-8192.0..=8191.0).contains(&raw) {
            return Err(SimError::Encode("I048/090 flight level"));
        }
        rec.flight_level = Some(FlightLevel048 {
            not_validated: false,
            garbled: false,
            raw: raw as i16,
        });
    }
    if r.addr != NONE_ADDR {
        if r.addr > 0xFF_FFFF {
            return Err(SimError::Encode("I048/220 address"));
        }
        rec.address = Some(r.addr);
    }
    Ok(rec)
}

/// The plots as CAT048 data blocks, concatenated, in table order: a new block at each change of
/// scan and every [`RECORDS_PER_BLOCK`] records. `source` is the radar's SAC/SIC.
pub fn to_cat048(p: &Plots, source: SourceId) -> Result<Vec<u8>, SimError> {
    if !columns_match(p) {
        return Err(SimError::PlotColumns);
    }
    let mut out = Vec::new();
    let mut batch: Vec<Cat048> = Vec::new();
    let mut batch_scan = i64::MIN;
    let flush = |batch: &mut Vec<Cat048>, out: &mut Vec<u8>| -> Result<(), SimError> {
        if batch.is_empty() {
            return Ok(());
        }
        let b = encode_block(&Block::Cat048(std::mem::take(batch)))
            .map_err(|e| SimError::Asterix(e.to_string()))?;
        out.extend_from_slice(&b);
        Ok(())
    };
    for i in 0..plots_len(p) {
        let Some(r) = plots_row(p, i) else { break };
        if r.scan != batch_scan || batch.len() >= RECORDS_PER_BLOCK {
            flush(&mut batch, &mut out)?;
            batch_scan = r.scan;
        }
        batch.push(record_of(&r, source)?);
    }
    flush(&mut batch, &mut out)?;
    Ok(out)
}
