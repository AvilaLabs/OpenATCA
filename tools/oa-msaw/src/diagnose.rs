//! The diagnosis of the alerts (`--diagnose`): what altitude was judged, what the radius was made
//! of, and what the obstacle and terrain behind the floor were, in numbers with no aircraft named.
//! Shell (B3): it reads the fine terrain tiles and the obstacle file again for the records.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use oa_dof::Dof;
use oa_msaw::{Alert, FloorKind, MsawConfig, AF_COAST, AF_QNH_MISSING, AF_UNCERTAIN, FLOOR_NONE};
use oa_sealed::Sealed;
use oa_tiff::{grid_elevation_at, grid_max_in_radius, Elevation, Grid};

use crate::cifp::CifpCorridor;
use crate::floors::{read_tile, tile_name, Floors};
use crate::guard::refuse_sealed;
use crate::qnh::Qnh;
use crate::report::landing_corridor;

const FT_PER_M: f64 = 3.280_839_895;
const M_PER_NM: f64 = 1852.0;
const M_PER_DEG: f64 = 111_194.926_644_558_74;
/// The radius of a point query: about one 1-arc-second cell.
const POINT_M: f64 = 30.0;
/// An obstacle whose top differs from terrain plus height by more than this is counted, feet.
const AMSL_TOL_FT: f64 = 50.0;
/// Fine tiles kept at once.
const TILES_KEPT: usize = 6;
/// The geoid undulation taken for the Chicago region, metres (EGM96 is about -33 m there), for the
/// geometric altitude's conversion to MSL. An approximation, said where it is used.
const GEOID_M: f64 = -33.0;

/// A row of the clip kept for the diagnosis: time, barometric and geometric altitude, NACp.
pub type Row = (i64, i32, i32, u8);

/// What the diagnosis reads.
// A7.2: cold struct, size guard waived
pub struct DiagIn<'a> {
    pub alerts: &'a [Alert],
    pub rows: &'a BTreeMap<u32, Vec<Row>>,
    pub floors: &'a mut Floors,
    pub dof: Option<&'a Dof>,
    pub cfg: &'a MsawConfig,
    pub terrain_dir: Option<&'a Path>,
    /// Airports of the CIFP with an approach: identifier, latitude, longitude, elevation in feet.
    pub airports: &'a [(String, f64, f64, i32)],
    /// Every airport with an approach (identifier, latitude, longitude) and every runway threshold
    /// (latitude, longitude) of them, for the distances of the deep alerts.
    pub all_airports: &'a [(String, f64, f64)],
    pub all_thresholds: &'a [(f64, f64)],
    /// How many of `corridors` are the configured airports' (the first ones); the rest are the
    /// area's.
    pub n_configured: usize,
    pub sealed: &'a Sealed,
    /// The corridors of the CIFP, and the altimeter settings, for the landing zone.
    pub corridors: &'a [CifpCorridor],
    pub qnh: Option<&'a mut Qnh>,
    /// Why each alert is alerted (its address and start): empty where eligibility was not judged.
    pub basis: &'a BTreeMap<(u32, i64), crate::eligibility::Basis>,
}

struct Tiles<'a> {
    dir: Option<PathBuf>,
    sealed: &'a Sealed,
    kept: Vec<((i32, i32), Grid)>,
}

impl Tiles<'_> {
    fn grid(&mut self, lat: f64, lon: f64) -> Option<&Grid> {
        let cell = (lat.floor() as i32, lon.floor() as i32);
        if let Some(i) = self.kept.iter().position(|(c, _)| *c == cell) {
            return self.kept.get(i).map(|(_, g)| g);
        }
        let p = self.dir.as_ref()?.join(tile_name(cell.0, cell.1)?);
        refuse_sealed(self.sealed, &p).ok()?;
        if !p.is_file() {
            return None;
        }
        let g = read_tile(&p).ok()?;
        if self.kept.len() >= TILES_KEPT {
            self.kept.remove(0);
        }
        self.kept.push((cell, g));
        self.kept.last().map(|(_, g)| g)
    }

    /// The bilinear elevation at a point, feet.
    fn at_ft(&mut self, lat: f64, lon: f64) -> Option<f64> {
        let g = self.grid(lat, lon)?;
        match grid_elevation_at(g, lat, lon) {
            Elevation::Metres(m) => Some(f64::from(m) * FT_PER_M),
            _ => None,
        }
    }

    /// The highest sample within `radius_m`: feet and its distance from the point, metres.
    fn peak(&mut self, lat: f64, lon: f64, radius_m: f64) -> Option<(f64, f64)> {
        let g = self.grid(lat, lon)?;
        let q = grid_max_in_radius(g, lat, lon, radius_m / M_PER_NM).ok()?;
        let p = q.peak?;
        let (dy, dx) = (
            (p.lat - lat) * M_PER_DEG,
            (p.lon - lon) * M_PER_DEG * lat.to_radians().cos(),
        );
        Some((f64::from(p.metres) * FT_PER_M, (dx * dx + dy * dy).sqrt()))
    }
}

fn dist_m(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dy, dx) = (
        (a.0 - b.0) * M_PER_DEG,
        (a.1 - b.1) * M_PER_DEG * a.0.to_radians().cos(),
    );
    (dx * dx + dy * dy).sqrt()
}

fn row_at(rows: &BTreeMap<u32, Vec<Row>>, addr: u32, t_ns: i64) -> Option<Row> {
    row_within(rows, addr, t_ns, 10_000_000_000)
}

/// The latest row of an aircraft at or before a time and at most `max_age_ns` before it.
fn row_within(
    rows: &BTreeMap<u32, Vec<Row>>,
    addr: u32,
    t_ns: i64,
    max_age_ns: i64,
) -> Option<Row> {
    let v = rows.get(&addr)?;
    let i = v.partition_point(|r| r.0 <= t_ns).checked_sub(1)?;
    v.get(i).copied().filter(|r| t_ns - r.0 < max_age_ns)
}

fn buckets(values: &[f64], edges: &[f64], unit: &str) -> String {
    let mut n = vec![0usize; edges.len() + 1];
    for v in values {
        let k = edges.iter().position(|e| v < e).unwrap_or(edges.len());
        if let Some(c) = n.get_mut(k) {
            *c += 1;
        }
    }
    let mut parts = Vec::new();
    for (k, c) in n.iter().enumerate() {
        let name = match (k.checked_sub(1).and_then(|j| edges.get(j)), edges.get(k)) {
            (None, Some(h)) => format!("under {h}{unit}"),
            (Some(l), Some(h)) => format!("{l} to {h}{unit}"),
            (Some(l), None) => format!("{l}{unit} or more"),
            (None, None) => "all".to_string(),
        };
        parts.push(format!("{name} {c}"));
    }
    parts.join(", ")
}

fn median(v: &mut [f64]) -> Option<f64> {
    v.sort_by(|a, b| oa_math::cmp_canon_f64(*a, *b));
    v.get(v.len() / 2).copied()
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).trim().to_string()
}

/// The diagnosis text.
pub fn diagnosis_text(i: &mut DiagIn) -> String {
    let mut o = String::new();
    let _ = writeln!(
        o,
        "diagnosis of {} alerts (numbers only; no aircraft is named)",
        i.alerts.len()
    );
    altitude(&mut o, i);
    radius(&mut o, i);
    let mut tiles = Tiles {
        dir: i.terrain_dir.map(Path::to_path_buf),
        sealed: i.sealed,
        kept: Vec::new(),
    };
    datum(&mut o, i, &mut tiles);
    obstacles(&mut o, i, &mut tiles);
    terrain(&mut o, i, &mut tiles);
    depth(&mut o, i);
    landing(&mut o, i);
    deep(&mut o, i, &mut tiles);
    alignment(&mut o, i);
    o
}

/// What the depth below the floor is made of: how far above the floor the aircraft is now against
/// how far the straight-line prediction takes it, the vertical rate, and the age of the data.
fn depth(o: &mut String, i: &DiagIn) {
    let _ = writeln!(o, "\n5. what the predicted depth is made of");
    let now: Vec<f64> = i
        .alerts
        .iter()
        .map(|a| f64::from(a.alt_ft) - f64::from(a.worst_floor_ft))
        .collect();
    let vr: Vec<f64> = i
        .alerts
        .iter()
        .map(|a| {
            if a.vrate_fpm.is_finite() {
                f64::from(a.vrate_fpm)
            } else {
                0.0
            }
        })
        .collect();
    let _ = writeln!(
        o,
        "  altitude at the first cycle minus the floor at the worst point: {}",
        buckets(
            &now,
            &[-500.0, -100.0, 0.0, 100.0, 300.0, 600.0, 1000.0, 2000.0],
            " ft"
        )
    );
    let _ = writeln!(
        o,
        "  vertical rate at the first cycle: {}",
        buckets(
            &vr,
            &[-3000.0, -2000.0, -1000.0, -500.0, -200.0, 200.0, 500.0, 1000.0],
            " ft/min"
        )
    );
    let dur: Vec<f64> = i
        .alerts
        .iter()
        .map(|a| (a.end_ns - a.start_ns) as f64 / 1e9)
        .collect();
    let _ = writeln!(
        o,
        "  alert length: {}",
        buckets(&dur, &[1.0, 5.0, 10.0, 30.0, 60.0, 120.0], " s")
    );
    let n = |f: u16| i.alerts.iter().filter(|a| a.flags & f != 0).count();
    let _ = writeln!(
        o,
        "  alerts on a coasting track (no measurement in the cycle) at some cycle: {}; with an uncertain track: {}; both: {}",
        n(AF_COAST),
        n(AF_UNCERTAIN),
        i.alerts.iter().filter(|a| a.flags & AF_COAST != 0 && a.flags & AF_UNCERTAIN != 0).count()
    );
    let age: Vec<f64> = i
        .alerts
        .iter()
        .filter_map(|a| {
            row_within(i.rows, a.addr, a.start_ns, 600_000_000_000)
                .map(|r| (a.start_ns - r.0) as f64 / 1e9)
        })
        .collect();
    let _ = writeln!(
        o,
        "  age of the latest clip row at the first cycle ({} of {} alerts have a row within 10 minutes): {}",
        age.len(),
        i.alerts.len(),
        buckets(&age, &[1.0, 2.0, 5.0, 10.0, 20.0, 30.0, 60.0, 120.0], " s")
    );
}

fn altitude(o: &mut String, i: &DiagIn) {
    let _ = writeln!(o, "\n1. the altitude judged");
    let _ = writeln!(
        o,
        "  The altitude is the ADS-B barometric altitude (pressure altitude, standard setting). Below FL180 it is\n  corrected with the altimeter setting of the nearest METAR station within the radius and age limits (the\n  indicated altitude of the standard atmosphere); from FL180 up it is used as it is; with no setting it is lowered\n  by the no-setting allowance and flagged. The geometric altitude is never used for judging: with a barometric\n  altitude present it is dropped, and a row with only a geometric altitude passes it flagged and the tracker ignores it."
    );
    let (mut raw_geom, mut cor_geom, mut cor, mut msl_geom, mut nacp) =
        (vec![], vec![], vec![], vec![], vec![]);
    let (mut none, mut missing) = (0usize, 0usize);
    for a in i.alerts {
        if a.flags & AF_QNH_MISSING != 0 {
            missing += 1;
        }
        let Some((_, baro, geom, np)) = row_at(i.rows, a.addr, a.start_ns) else {
            none += 1;
            continue;
        };
        nacp.push(f64::from(np));
        if baro == oa_adsb::NONE_I32 {
            continue;
        }
        cor.push(f64::from(a.alt_ft) - f64::from(baro));
        if geom != oa_adsb::NONE_I32 {
            raw_geom.push(f64::from(baro) - f64::from(geom));
            cor_geom.push(f64::from(a.alt_ft) - f64::from(geom));
            msl_geom.push(f64::from(a.alt_ft) - (f64::from(geom) - GEOID_M * FT_PER_M));
        }
    }
    let _ = writeln!(o, "  alerts with no clip row within 10 s: {none}; flagged with no altimeter setting: {missing}");
    let edges = [-300.0, -150.0, -50.0, 50.0, 150.0, 300.0];
    let _ = writeln!(
        o,
        "  correction applied (judged minus raw barometric), {} alerts, median {:.0} ft: {}",
        cor.len(),
        median(&mut cor.clone()).unwrap_or(f64::NAN),
        buckets(&cor, &edges, " ft")
    );
    let wide = [-500.0, -300.0, -150.0, 0.0, 150.0, 300.0, 500.0];
    let _ = writeln!(
        o,
        "  raw barometric minus geometric, {} alerts with both, median {:.0} ft: {}",
        raw_geom.len(),
        median(&mut raw_geom.clone()).unwrap_or(f64::NAN),
        buckets(&raw_geom, &wide, " ft")
    );
    let _ = writeln!(
        o,
        "  judged minus geometric, median {:.0} ft: {}",
        median(&mut cor_geom.clone()).unwrap_or(f64::NAN),
        buckets(&cor_geom, &wide, " ft")
    );
    let _ = writeln!(
        o,
        "  judged minus geometric converted to MSL with a geoid of {GEOID_M} m (about -33 m at the region; geometric altitude is height above the ellipsoid), median {:.0} ft: {}",
        median(&mut msl_geom.clone()).unwrap_or(f64::NAN),
        buckets(&msl_geom, &[-300.0, -150.0, -50.0, 50.0, 150.0, 300.0], " ft")
    );
    let _ = writeln!(
        o,
        "  NACp of the nearest row: {}",
        buckets(&nacp, &[5.0, 7.0, 8.0, 9.0, 10.0, 11.0], "")
    );
    let _ = writeln!(
        o,
        "  SFDPS carries `enRoute/position/altitude` (the reported surveillance altitude, a pressure altitude) and an assigned altitude; its known\n  fields hold no altimeter setting and no corrected altitude. No comparison with the reported altitude is made here."
    );
}

fn radius(o: &mut String, i: &DiagIn) {
    let _ = writeln!(o, "\n2. the query radius");
    let _ = writeln!(
        o,
        "  radius of a point = 95 % radius of the track + velocity uncertainty ({} m/s) x prediction time + half the distance to the next point (speed x {:.2} s / 2), then the lookup rounds it up to a 250 m bucket and adds the cell's half diagonal (158 m)",
        i.cfg.velocity_uncertainty_mps,
        i.cfg.look_ahead_s / ((i.cfg.samples.saturating_sub(1)).max(1) as f64)
    );
    let step = i.cfg.look_ahead_s / ((i.cfg.samples.saturating_sub(1)).max(1) as f64);
    // (core radius, r95, growth, half step, effective, alert index)
    let mut v: Vec<(f64, f64, f64, f64, f64)> = Vec::new();
    for a in i.alerts {
        let ts =
            oa_msaw::calc::sample_time_s(usize::from(a.worst_k), i.cfg.samples, i.cfg.look_ahead_s);
        let speed = f64::from(a.vel_east_mps).hypot(f64::from(a.vel_north_mps));
        let r95 = f64::from(a.r95_m);
        let growth = i.cfg.velocity_uncertainty_mps * ts;
        let half = speed * step / 2.0;
        v.push((
            f64::from(a.worst_radius_m),
            r95,
            growth,
            half,
            Floors::effective_radius_m(f64::from(a.worst_radius_m)),
        ));
    }
    let dom = |t: &(f64, f64, f64, f64, f64)| {
        let extra = t.4 - t.0;
        let m = t.1.max(t.2).max(t.3).max(extra);
        if m == t.1 {
            "r95"
        } else if m == t.2 {
            "growth"
        } else if m == t.3 {
            "half-step"
        } else {
            "rounding"
        }
    };
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for t in &v {
        *counts.entry(dom(t)).or_default() += 1;
    }
    let _ = writeln!(
        o,
        "  dominant term over all {} alerts: {:?}",
        v.len(),
        counts
    );
    let _ = writeln!(
        o,
        "  95 % radius of the track at the alert: {}",
        buckets(
            &v.iter().map(|t| t.1).collect::<Vec<_>>(),
            &[50.0, 100.0, 200.0, 500.0, 1000.0],
            " m"
        )
    );
    let _ = writeln!(
        o,
        "  half-step term (speed): {}",
        buckets(
            &v.iter().map(|t| t.3).collect::<Vec<_>>(),
            &[100.0, 200.0, 400.0, 800.0],
            " m"
        )
    );
    v.sort_by(|a, b| oa_math::cmp_canon_f64(a.0, b.0));
    let _ = writeln!(o, "  20 alerts spread through the radius order (core radius m = r95 + growth + half step; rounding to the lookup; effective m; dominant):");
    for n in 0..20usize {
        let k = (v.len().saturating_sub(1)) * n / 19;
        if let Some(t) = v.get(k) {
            let _ = writeln!(
                o,
                "    {:4.0} = {:4.0} + {:4.0} + {:4.0}; rounding {:3.0}; effective {:4.0}; {}",
                t.0,
                t.1,
                t.2,
                t.3,
                t.4 - t.0,
                t.4,
                dom(t)
            );
        }
    }
}

fn datum(o: &mut String, i: &DiagIn, tiles: &mut Tiles) {
    let _ = writeln!(o, "\n4a. the terrain datum and cell size");
    let mut found = false;
    let mut diffs = Vec::new();
    for (_, lat, lon, elev) in i.airports {
        if let Some(t) = tiles.at_ft(*lat, *lon) {
            diffs.push(t - f64::from(*elev));
            if !found {
                found = true;
            }
        }
    }
    if let Some((_, g)) = tiles.kept.first() {
        let _ = writeln!(
            o,
            "  a 3DEP tile: {} x {} samples, step {:.3} arc-second north-south and {:.3} east-west ({:.1} m by {:.1} m at the tile's latitude), PixelIsPoint {}, horizontal CRS EPSG {}, vertical datum key in the file: {:?}; the file names none: 3DEP 1 arc-second is NAVD88 (about mean sea level), elevations in metres",
            g.width,
            g.height,
            g.dlat * 3600.0,
            g.dlon * 3600.0,
            g.dlat * M_PER_DEG,
            g.dlon * M_PER_DEG * g.lat0.to_radians().cos(),
            g.point_registered,
            g.crs,
            g.vertical_cs
        );
    }
    let _ = writeln!(
        o,
        "  DEM (bilinear, at the reference point) minus the published airport elevation, {} airports with a tile, median {:.0} ft: {}",
        diffs.len(),
        median(&mut diffs.clone()).unwrap_or(f64::NAN),
        buckets(&diffs, &[-100.0, -30.0, -10.0, 10.0, 30.0, 100.0], " ft")
    );
}

fn obstacles(o: &mut String, i: &mut DiagIn, tiles: &mut Tiles) {
    let _ = writeln!(o, "\n3. the obstacles");
    let Some(d) = i.dof else {
        let _ = writeln!(o, "  no obstacle file given");
        return;
    };
    let _ = writeln!(
        o,
        "  The file has a height above ground, a height above mean sea level, horizontal and vertical accuracy codes and a verified/unverified letter; it has no dismantled or temporary field (a dismantled obstacle is removed from the file, and a construction obstacle is listed as an addition); the floor uses the height above mean sea level as given, plus the clearance, and never adds the height above ground to anything."
    );
    // The deepest obstacle-floor alerts.
    let mut order: Vec<&Alert> = i
        .alerts
        .iter()
        .filter(|a| a.worst_kind == FloorKind::Obstacle)
        .collect();
    order.sort_by(|a, b| {
        oa_math::cmp_canon_f64(f64::from(a.worst_margin_ft), f64::from(b.worst_margin_ft))
    });
    let _ = writeln!(
        o,
        "  {} alerts have an obstacle floor; the 20 deepest:",
        order.len()
    );
    let _ = writeln!(o, "    depth | floor | amsl agl terrain-under-obstacle | amsl-(terrain+agl) | hacc vacc ver | type x qty | dist to aircraft / to violating point | baro raw/judged/geom");
    for a in order.iter().take(20) {
        let (lat, lon) = (
            f64::from(a.worst_lat_e7) / 1e7,
            f64::from(a.worst_lon_e7) / 1e7,
        );
        let eff = Floors::effective_radius_m(f64::from(a.worst_radius_m));
        let Some((top, rec, dpt)) = i.floors.obstacle_record_within(lat, lon, eff) else {
            let _ = writeln!(o, "    (no obstacle found within the lookup radius)");
            continue;
        };
        let r = rec as usize;
        let (amsl, agl) = (
            d.amsl_ft.get(r).copied().unwrap_or(0),
            d.agl_ft.get(r).copied().unwrap_or(0),
        );
        let (olat, olon) = (
            f64::from(d.lat_e7.get(r).copied().unwrap_or(0)) / 1e7,
            f64::from(d.lon_e7.get(r).copied().unwrap_or(0)) / 1e7,
        );
        let terr = tiles.at_ft(olat, olon);
        let aircraft = dist_m(
            (f64::from(a.lat_e7) / 1e7, f64::from(a.lon_e7) / 1e7),
            (olat, olon),
        );
        let row = row_at(i.rows, a.addr, a.start_ns);
        let _ = writeln!(
            o,
            "    {:5.0} | {:5} (top {}) | {} {} {} | {} | {} {} {} | {} x{} | {:.0} m / {:.0} m | {} / {:.0} / {}",
            -a.worst_margin_ft,
            a.worst_floor_ft,
            top,
            amsl,
            agl,
            terr.map_or("?".to_string(), |t| format!("{t:.0}")),
            terr.map_or("?".to_string(), |t| format!("{:.0}", f64::from(amsl) - (t + f64::from(agl)))),
            text(&[d.h_acc.get(r).copied().unwrap_or(b' ')]),
            text(&[d.v_acc.get(r).copied().unwrap_or(b' ')]),
            text(&[d.verification.get(r).copied().unwrap_or(b' ')]),
            text(d.kind.get(r).map_or(&[][..], |k| &k[..])),
            d.quantity.get(r).copied().unwrap_or(0),
            aircraft,
            dpt,
            row.map_or("?".to_string(), |x| x.1.to_string()),
            a.alt_ft,
            row.map_or("?".to_string(), |x| x.2.to_string())
        );
    }
    // AMSL against terrain plus AGL for the obstacles near the alerts.
    let (mut lo_la, mut hi_la, mut lo_lo, mut hi_lo) = (90.0f64, -90.0f64, 180.0f64, -180.0f64);
    for a in i.alerts {
        let (la, lo) = (
            f64::from(a.worst_lat_e7) / 1e7,
            f64::from(a.worst_lon_e7) / 1e7,
        );
        lo_la = lo_la.min(la - 0.3);
        hi_la = hi_la.max(la + 0.3);
        lo_lo = lo_lo.min(lo - 0.3);
        hi_lo = hi_lo.max(lo + 0.3);
    }
    let mut idx: Vec<usize> = (0..d.lat_e7.len())
        .filter(|&r| {
            let (la, lo) = (
                f64::from(d.lat_e7.get(r).copied().unwrap_or(0)) / 1e7,
                f64::from(d.lon_e7.get(r).copied().unwrap_or(0)) / 1e7,
            );
            la >= lo_la && la <= hi_la && lo >= lo_lo && lo <= hi_lo
        })
        .collect();
    idx.sort_by_key(|&r| {
        (
            d.lat_e7.get(r).map_or(0, |v| v.div_euclid(10_000_000)),
            d.lon_e7.get(r).map_or(0, |v| v.div_euclid(10_000_000)),
        )
    });
    let (mut total, mut off, mut notile) = (0usize, 0usize, 0usize);
    let mut gaps = Vec::new();
    for r in idx {
        let (olat, olon) = (
            f64::from(d.lat_e7.get(r).copied().unwrap_or(0)) / 1e7,
            f64::from(d.lon_e7.get(r).copied().unwrap_or(0)) / 1e7,
        );
        let Some(t) = tiles.at_ft(olat, olon) else {
            notile += 1;
            continue;
        };
        total += 1;
        let gap = f64::from(d.amsl_ft.get(r).copied().unwrap_or(0))
            - (t + f64::from(d.agl_ft.get(r).copied().unwrap_or(0)));
        if gap.abs() > AMSL_TOL_FT {
            off += 1;
        }
        gaps.push(gap);
    }
    let _ = writeln!(
        o,
        "  obstacles within 0.3 degree of an alert with terrain under them: {total} (no tile or no value for {notile}); AMSL differs from terrain + AGL by more than {AMSL_TOL_FT} ft for {off}; median difference {:.0} ft: {}",
        median(&mut gaps.clone()).unwrap_or(f64::NAN),
        buckets(&gaps, &[-200.0, -50.0, 50.0, 200.0, 1000.0], " ft")
    );
}

fn terrain(o: &mut String, i: &mut DiagIn, tiles: &mut Tiles) {
    let _ = writeln!(o, "\n4. the terrain-floor alerts");
    let mut order: Vec<&Alert> = i
        .alerts
        .iter()
        .filter(|a| a.worst_kind == FloorKind::Terrain)
        .collect();
    order.sort_by(|a, b| {
        oa_math::cmp_canon_f64(f64::from(a.worst_margin_ft), f64::from(b.worst_margin_ft))
    });
    let _ = writeln!(o, "  {} alerts have a terrain floor:", order.len());
    let _ = writeln!(o, "    depth | floor | coarse lookup | DEM max in the effective radius (dist m) | DEM at the point | DEM max within 30 m | baro raw/judged/geom");
    for a in &order {
        let (lat, lon) = (
            f64::from(a.worst_lat_e7) / 1e7,
            f64::from(a.worst_lon_e7) / 1e7,
        );
        let eff = Floors::effective_radius_m(f64::from(a.worst_radius_m));
        let coarse = i.floors.terrain_within_ft(lat, lon, eff);
        let peak = tiles.peak(lat, lon, eff);
        let at = tiles.at_ft(lat, lon);
        let near = tiles.peak(lat, lon, POINT_M);
        let row = row_at(i.rows, a.addr, a.start_ns);
        let _ = writeln!(
            o,
            "    {:5.0} | {:5} | {} | {} | {} | {} | {} / {:.0} / {}",
            -a.worst_margin_ft,
            a.worst_floor_ft,
            if coarse == FLOOR_NONE {
                "none".to_string()
            } else {
                coarse.to_string()
            },
            peak.map_or("?".to_string(), |(t, d)| format!("{t:.0} ({d:.0})")),
            at.map_or("?".to_string(), |t| format!("{t:.0}")),
            near.map_or("?".to_string(), |(t, _)| format!("{t:.0}")),
            row.map_or("?".to_string(), |x| x.1.to_string()),
            a.alt_ft,
            row.map_or("?".to_string(), |x| x.2.to_string())
        );
    }
    // What a point query would have left of the terrain and obstacle alerts.
    let (mut gone, mut kept, mut unknown) = (0usize, 0usize, 0usize);
    for a in i.alerts {
        let (lat, lon) = (
            f64::from(a.worst_lat_e7) / 1e7,
            f64::from(a.worst_lon_e7) / 1e7,
        );
        let eff = Floors::effective_radius_m(f64::from(a.worst_radius_m));
        let (Some(tf), Some(t0)) = (tiles.peak(lat, lon, eff), tiles.peak(lat, lon, POINT_M))
        else {
            unknown += 1;
            continue;
        };
        let (of, o0) = (
            i.floors
                .obstacle_record_within(lat, lon, eff)
                .map(|x| f64::from(x.0)),
            i.floors
                .obstacle_record_within(lat, lon, POINT_M)
                .map(|x| f64::from(x.0)),
        );
        let tc = f64::from(i.cfg.terrain_clearance_ft);
        let oc = f64::from(i.cfg.obstacle_clearance_ft);
        let floor_eff = (tf.0 + tc).max(of.map_or(f64::MIN, |x| x + oc));
        let floor_pt = (t0.0 + tc).max(o0.map_or(f64::MIN, |x| x + oc));
        // Only where the general floor, not an approach surface, was the worst floor.
        if matches!(a.worst_kind, FloorKind::Terrain | FloorKind::Obstacle) {
            if f64::from(a.worst_margin_ft) + (floor_eff - floor_pt) >= 0.0 {
                gone += 1;
            } else {
                kept += 1;
            }
        } else {
            unknown += 1;
        }
    }
    let _ = writeln!(
        o,
        "  with a radius of {POINT_M} m instead of the lookup's: {gone} of the {} terrain and obstacle alerts would not be below their floor, {kept} still would; {unknown} not assessed (surface floor or no tile); this uses the general clearances ({} ft terrain, {} ft obstacle), not the airport-area one",
        gone + kept,
        i.cfg.terrain_clearance_ft,
        i.cfg.obstacle_clearance_ft
    );
}

/// The age of an altimeter setting under which it counts as recent, seconds.
const QNH_RECENT_S: i64 = 1800;
/// The look-back for the descent angle the aircraft is flying, seconds.
const ANGLE_BACK_S: i64 = 30;

/// The altimeter behind an alert's worst cycle.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Datum {
    /// A station's setting no older than [`QNH_RECENT_S`] (the age bucket's end).
    Recent,
    Older,
    /// The lowest setting of the last 12 h.
    Fallback,
    /// None at all: the allowance.
    Allowance,
    /// Not below FL180, or the setting was not asked.
    Standard,
}

fn datum_of(a: &Alert, q: &mut Option<&mut Qnh>) -> Datum {
    let Some(q) = q.as_deref_mut() else {
        return Datum::Standard;
    };
    if a.worst_flags & AF_QNH_MISSING == 0 {
        let (lat, lon) = (
            f64::from(a.worst_lat_e7) / 1e7,
            f64::from(a.worst_lon_e7) / 1e7,
        );
        let t_s = a.worst_ns.div_euclid(1_000_000_000);
        return match q.setting_age_s(lat, lon, t_s) {
            Some(age) if age <= QNH_RECENT_S => Datum::Recent,
            Some(_) => Datum::Older,
            None => Datum::Standard,
        };
    }
    let (lat, lon) = (f64::from(a.lat_e7) / 1e7, f64::from(a.lon_e7) / 1e7);
    if q.lowest_recent_hpa(lat, lon, a.worst_ns.div_euclid(1_000_000_000))
        .is_some()
    {
        Datum::Fallback
    } else {
        Datum::Allowance
    }
}

fn datum_name(d: Datum) -> &'static str {
    match d {
        Datum::Recent => "station within 30 min",
        Datum::Older => "station older than 30 min",
        Datum::Fallback => "lowest in 12 h",
        Datum::Allowance => "allowance, no setting",
        Datum::Standard => "standard or none needed",
    }
}

/// Metres along the corridor's outward bearing from its start point, and across it.
fn along_cross_m(k: &oa_msaw::Corridor, lat: f64, lon: f64) -> (f64, f64) {
    let (tlat, tlon) = (f64::from(k.thr_lat_e7) / 1e7, f64::from(k.thr_lon_e7) / 1e7);
    let (dy, dx) = (
        (lat - tlat) * M_PER_DEG,
        (lon - tlon) * M_PER_DEG * tlat.to_radians().cos(),
    );
    let b = k.out_bearing_deg.to_radians();
    (dx * b.sin() + dy * b.cos(), dx * b.cos() - dy * b.sin())
}

/// The angle the aircraft flew down over the 30 s before the cycle, degrees (positive is
/// descending), from the barometric altitudes of the rows and the speed of the alert.
fn flown_angle_deg(a: &Alert, rows: &BTreeMap<u32, Vec<Row>>) -> Option<f64> {
    let v = rows.get(&a.addr)?;
    let t = a.worst_ns;
    let from = t - ANGLE_BACK_S * 1_000_000_000;
    let inside: Vec<&Row> = v
        .iter()
        .filter(|r| r.0 > from && r.0 <= t && r.1 > -2000 && r.1 < 60_000)
        .collect();
    let (first, last) = (inside.first()?, inside.last()?);
    let dt = (last.0 - first.0) as f64 / 1e9;
    if dt < 10.0 {
        return None;
    }
    let speed = f64::from(a.vel_east_mps).hypot(f64::from(a.vel_north_mps));
    let drop_m = f64::from(first.1 - last.1) / FT_PER_M;
    Some((drop_m / (speed * dt).max(1.0)).atan().to_degrees())
}

/// The alerts whose worst point is in a runway strip or the last 1 NM of a capture box: how deep,
/// what floor, how high and how far from the threshold the aircraft was at that point, the angle it
/// was flying against the procedure's, the altimeter behind its altitude, and the time to
/// violation. Item 4: how many of all the alerts used the lowest-in-12-h setting.
fn landing(o: &mut String, i: &mut DiagIn) {
    let _ = writeln!(
        o,
        "\n6. the landing zone (worst point in a runway strip or the last 1 NM of a capture box)"
    );
    let mut all = [0usize; 5];
    let at = |all: &[usize; 5], d: Datum| all.get(d as usize).copied().unwrap_or(0);
    for a in i.alerts {
        let d = datum_of(a, &mut i.qnh);
        if let Some(n) = all.get_mut(d as usize) {
            *n += 1;
        }
    }
    let _ = writeln!(
        o,
        "  all {} alerts, the altimeter behind the altitude at the worst cycle: station within 30 min {}, station older {}, lowest in 12 h (fallback) {}, allowance {}, standard or none needed {}",
        i.alerts.len(),
        at(&all, Datum::Recent),
        at(&all, Datum::Older),
        at(&all, Datum::Fallback),
        at(&all, Datum::Allowance),
        at(&all, Datum::Standard)
    );
    // (alert, corridor)
    let mut rows: Vec<(Alert, &CifpCorridor)> = Vec::new();
    for a in i.alerts {
        if let Some(k) = landing_corridor(i.corridors, a.worst_lat_e7, a.worst_lon_e7, i.cfg) {
            rows.push((*a, k));
        }
    }
    let _ = writeln!(o, "  landing alerts: {}", rows.len());
    if rows.is_empty() {
        return;
    }
    struct R {
        depth: f64,
        kind: FloorKind,
        floor: i32,
        height: f64,
        dist_nm: f64,
        flown: Option<f64>,
        published: Option<f64>,
        datum: Datum,
        stale: bool,
        ttv: f64,
        on_strip: bool,
    }
    // The deepest: each alert judged 500 ft or more below its floor, as numbers only: depth,
    // the altimeter's age (seconds at the end of the 5 min bucket; -1 where the setting was the
    // lowest-in-12-h one or none), the stale-extrapolated flag at the worst cycle, the track's age
    // (seconds from its first row in the window to the worst cycle) and the age of its last row.
    let mut deep: Vec<String> = Vec::new();
    for (a, _) in &rows {
        if a.worst_margin_ft > -500.0 {
            continue;
        }
        let (lat, lon) = (
            f64::from(a.worst_lat_e7) / 1e7,
            f64::from(a.worst_lon_e7) / 1e7,
        );
        let t_s = a.worst_ns.div_euclid(1_000_000_000);
        let age = match i.qnh.as_deref() {
            Some(q) if a.worst_flags & AF_QNH_MISSING == 0 => {
                q.setting_age_s(lat, lon, t_s).unwrap_or(-1)
            }
            _ => -1,
        };
        let v = i.rows.get(&a.addr);
        let first = v
            .and_then(|v| v.first())
            .map_or(f64::NAN, |r| (a.worst_ns - r.0) as f64 / 1e9);
        let last = v
            .and_then(|v| v.iter().rev().find(|r| r.0 <= a.worst_ns))
            .map_or(f64::NAN, |r| (a.worst_ns - r.0) as f64 / 1e9);
        deep.push(format!(
            "    {:.0}; {age}; {}; {first:.0}; {last:.1}",
            -a.worst_margin_ft,
            u8::from(a.worst_flags & oa_msaw::AF_STALE_EXTRAPOLATED != 0)
        ));
    }
    let _ = writeln!(
        o,
        "  the {} alerts judged 500 ft or more below the floor (depth ft; altimeter age s; stale-extrapolated 0/1; track age s since its first row in the window; age of its last row s):\n{}",
        deep.len(),
        deep.join("\n")
    );
    let mut out: Vec<R> = Vec::new();
    for (a, k) in &rows {
        let (lat, lon) = (
            f64::from(a.worst_lat_e7) / 1e7,
            f64::from(a.worst_lon_e7) / 1e7,
        );
        let (along, _) = along_cross_m(&k.corridor, lat, lon);
        let on_strip = k.kind == oa_msaw::CK_RUNWAY;
        // Before the threshold is positive, past it negative.
        let to_thr = if on_strip {
            k.corridor.land_from_m - along
        } else {
            along
        };
        let judged = f64::from(a.worst_margin_ft) + f64::from(a.worst_floor_ft);
        let published = if on_strip {
            let label = k.label.trim_end_matches(" strip");
            i.corridors
                .iter()
                .find(|x| x.kind == oa_msaw::CK_APPROACH && x.label == label)
                .map(|x| x.corridor.path_angle_deg)
        } else {
            Some(k.corridor.path_angle_deg)
        };
        out.push(R {
            depth: -f64::from(a.worst_margin_ft),
            kind: a.worst_kind,
            floor: a.worst_floor_ft,
            height: judged - f64::from(k.corridor.thr_elev_ft),
            dist_nm: to_thr / M_PER_NM,
            flown: flown_angle_deg(a, i.rows),
            published,
            datum: datum_of(a, &mut i.qnh),
            stale: a.worst_flags & oa_msaw::AF_STALE_EXTRAPOLATED != 0,
            ttv: f64::from(a.ttv_min_s),
            on_strip,
        });
    }
    let col = |f: &dyn Fn(&R) -> f64| out.iter().map(f).collect::<Vec<_>>();
    let _ = writeln!(
        o,
        "  in the runway strip {}, in a capture box's last mile {}",
        out.iter().filter(|r| r.on_strip).count(),
        out.iter().filter(|r| !r.on_strip).count()
    );
    let _ = writeln!(
        o,
        "  depth below the floor, ft: {}",
        buckets(
            &col(&|r| r.depth),
            &[25.0, 50.0, 100.0, 200.0, 500.0],
            " ft"
        )
    );
    let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
    for r in &out {
        *kinds.entry(crate::report::kind_text(r.kind)).or_default() += 1;
    }
    let _ = writeln!(o, "  floor kind: {kinds:?}");
    let _ = writeln!(
        o,
        "  height above the threshold at the worst point, ft: {}",
        buckets(
            &col(&|r| r.height),
            &[0.0, 50.0, 100.0, 200.0, 500.0, 1000.0],
            " ft"
        )
    );
    let _ = writeln!(
        o,
        "  distance before the threshold at the worst point (negative: past it), NM: {}",
        buckets(
            &col(&|r| r.dist_nm),
            &[-1.0, 0.0, 0.25, 0.5, 1.0, 2.0],
            " NM"
        )
    );
    let flown: Vec<f64> = out.iter().filter_map(|r| r.flown).collect();
    let _ = writeln!(
        o,
        "  angle flown over the last 30 s, degrees ({} of {} have two rows 10 s or more apart): {}",
        flown.len(),
        out.len(),
        buckets(&flown, &[0.0, 1.0, 2.0, 2.5, 3.0, 3.5, 4.0, 5.0], " deg")
    );
    let gap: Vec<f64> = out
        .iter()
        .filter_map(|r| Some(r.flown? - r.published?))
        .collect();
    let _ = writeln!(
        o,
        "  angle flown less the procedure's angle, degrees: {}",
        buckets(&gap, &[-3.0, -1.0, -0.5, 0.0, 0.5, 1.0], " deg")
    );
    let mut dat: BTreeMap<&str, usize> = BTreeMap::new();
    for r in &out {
        *dat.entry(datum_name(r.datum)).or_default() += 1;
    }
    let _ = writeln!(
        o,
        "  altimeter at the worst cycle: {dat:?}; stale-extrapolated at the worst cycle {}",
        out.iter().filter(|r| r.stale).count()
    );
    let _ = writeln!(
        o,
        "  time to violation, s: {}",
        buckets(&col(&|r| r.ttv), &[0.5, 5.0, 10.0, 15.0], " s")
    );
    // The obstacle behind an obstacle floor: how tall, how far from the worst point, and whether a
    // lookup of the point alone (not the circle) would still put the aircraft below it.
    if let Some(d) = i.dof {
        let (mut agl, mut dist, mut still, mut n) = (Vec::new(), Vec::new(), 0usize, 0usize);
        // The obstacle's top less the highest surface of the corridors holding the point, at the
        // obstacle's own position: positive is above the surface (it counts), negative below.
        let mut over: Vec<f64> = Vec::new();
        for (a, k) in &rows {
            if a.worst_kind != FloorKind::Obstacle {
                continue;
            }
            let (lat, lon) = (
                f64::from(a.worst_lat_e7) / 1e7,
                f64::from(a.worst_lon_e7) / 1e7,
            );
            let eff = Floors::effective_radius_m(f64::from(a.worst_radius_m));
            if let Some((_, rec, dpt)) = i.floors.obstacle_record_within(lat, lon, eff) {
                n += 1;
                agl.push(f64::from(d.agl_ft.get(rec as usize).copied().unwrap_or(0)));
                let r = rec as usize;
                let (ola, olo) = (
                    d.lat_e7.get(r).copied().unwrap_or(0),
                    d.lon_e7.get(r).copied().unwrap_or(0),
                );
                if let Some(s) = oa_msaw::corridor_hit_ft(&k.corridor, ola, olo, 0.0, i.cfg)
                    .and_then(|h| h.surface_ft)
                {
                    over.push(f64::from(d.amsl_ft.get(r).copied().unwrap_or(0)) - f64::from(s));
                }
                dist.push(dpt);
            }
            let judged = f64::from(a.worst_margin_ft) + f64::from(a.worst_floor_ft);
            if i.floors
                .obstacle_record_within(lat, lon, POINT_M)
                .is_some_and(|x| judged < f64::from(x.0))
            {
                still += 1;
            }
        }
        let _ = writeln!(
            o,
            "  the {n} obstacle floors: height above ground of the obstacle {}; distance from the worst point to it {}; obstacle top less the corridor surface at the obstacle ({} of {} inside the corridor) {}; with a lookup of the point alone ({POINT_M} m) {still} would still be below an obstacle top",
            buckets(&agl, &[20.0, 50.0, 100.0, 200.0], " ft"),
            buckets(&dist, &[100.0, 250.0, 500.0, 1000.0], " m"),
            over.len(),
            n,
            buckets(&over, &[-100.0, -25.0, 0.0, 25.0, 100.0], " ft")
        );
    }
    let _ = writeln!(o, "  10 alerts spread through the depth order (depth ft; floor kind; floor ft; height above the threshold ft; distance before the threshold NM; angle flown deg; procedure's angle deg; altimeter; time to violation s):");
    out.sort_by(|a, b| oa_math::cmp_canon_f64(a.depth, b.depth));
    for n in 0..10usize {
        let Some(r) = out.get(n * out.len() / 10) else {
            break;
        };
        let _ = writeln!(
            o,
            "    {:.0}; {}; {}; {:.0}; {:.2}; {}; {}; {}; {:.1}",
            r.depth,
            crate::report::kind_text(r.kind),
            r.floor,
            r.height,
            r.dist_nm,
            r.flown.map_or("n/a".to_string(), |x| format!("{x:.1}")),
            r.published.map_or("n/a".to_string(), |x| format!("{x:.1}")),
            datum_name(r.datum),
            r.ttv
        );
    }
}

/// An alert judged this far below its floor is deep (feet).
const DEEP_FT: f64 = 500.0;
/// Within this distance of a threshold, a descending track is on final, and within it of an airport a
/// climbing one is departing (nautical miles).
const NEAR_NM: f64 = 10.0;
/// Descending and climbing, feet per minute.
const VRATE_FPM: f64 = 200.0;

/// One deep alert, classified.
struct Deep {
    hash: String,
    clock: String,
    depth: f64,
    kind: FloorKind,
    height_terrain: Option<f64>,
    apt_nm: f64,
    thr_nm: f64,
    corridor: &'static str,
    phase: &'static str,
    setting_age_s: i64,
    cold_ft: f64,
    basis: &'static str,
    ttv: f64,
    /// Stale-extrapolated at the worst cycle, and the judged altitude less the first cycle's, feet.
    stale: bool,
    alt_delta: f64,
    /// Seconds ahead of the cycle of the worst point, seconds from the alert's start to that cycle,
    /// and the track's heading less the inbound heading of the approach corridor holding the worst
    /// point, degrees (-180 to 180).
    ahead_s: f64,
    span_s: f64,
    hdg_off: Option<f64>,
    /// The nearest airport with an approach and whether the configuration gives it corridors.
    apt_id: String,
    apt_cat: &'static str,
    obst: Option<(f64, f64)>,
    r95: f64,
    growth: f64,
    half: f64,
    rounding: f64,
    eff: f64,
    r95_still: bool,
    point_still: bool,
}

/// Every alert 500 ft or more below its floor, classified by what floor it is, how high above the
/// terrain, how far from an airport and a threshold (all of the CIFP's), whether in a corridor of the
/// configured airports, what the track was doing,
/// the altimeter behind its altitude, why it is alerted, the time to violation, the obstacle behind an
/// obstacle floor and what the lookup radius was made of. No rule is changed by it. Addresses are
/// hashed. The groups are by floor kind, phase, distance to the nearest airport with an approach and
/// corridor.
fn deep(o: &mut String, i: &mut DiagIn, tiles: &mut Tiles) {
    let _ = writeln!(
        o,
        "\n7. the deep alerts ({DEEP_FT} ft or more below the floor at the worst point)"
    );
    let step = i.cfg.look_ahead_s / ((i.cfg.samples.saturating_sub(1)).max(1) as f64);
    let mut all: Vec<Deep> = Vec::new();
    let mut shallow_eff: Vec<f64> = Vec::new();
    let mut shallow_apt: Vec<f64> = Vec::new();
    for a in i.alerts {
        let eff = Floors::effective_radius_m(f64::from(a.worst_radius_m));
        let (lat, lon) = (f64::from(a.lat_e7) / 1e7, f64::from(a.lon_e7) / 1e7);
        let (apt_nm, apt_id) = i
            .all_airports
            .iter()
            .map(|(id, la, lo)| (dist_m((lat, lon), (*la, *lo)) / M_PER_NM, id.as_str()))
            .fold((f64::INFINITY, ""), |b, x| if x.0 < b.0 { x } else { b });
        if f64::from(a.worst_margin_ft) > -DEEP_FT {
            shallow_eff.push(eff);
            shallow_apt.push(apt_nm);
            continue;
        }
        let thr_nm = i
            .all_thresholds
            .iter()
            .map(|t| dist_m((lat, lon), *t) / M_PER_NM)
            .fold(f64::INFINITY, f64::min);
        let (wlat, wlon) = (
            f64::from(a.worst_lat_e7) / 1e7,
            f64::from(a.worst_lon_e7) / 1e7,
        );
        let apt_cat = airport_category(i, apt_id, apt_nm);
        let (corridor, off) = rules_of(a, i);
        let vr = f64::from(a.vrate_fpm);
        let phase = if vr.is_nan() {
            "rate unknown"
        } else if thr_nm <= NEAR_NM && vr <= -VRATE_FPM {
            "final (descending, within 10 NM of a threshold)"
        } else if apt_nm <= NEAR_NM && vr >= VRATE_FPM {
            "departure (climbing, within 10 NM of an airport)"
        } else if apt_nm <= NEAR_NM {
            "level or slow near an airport"
        } else if vr <= -VRATE_FPM {
            "descending away from airports"
        } else if vr >= VRATE_FPM {
            "climbing away from airports"
        } else {
            "level away from airports"
        };
        let t_s = a.worst_ns.div_euclid(1_000_000_000);
        let judged = f64::from(a.worst_margin_ft) + f64::from(a.worst_floor_ft);
        let (setting_age_s, cold_ft) = match i.qnh.as_deref_mut() {
            Some(q) => {
                let age = if a.worst_flags & AF_QNH_MISSING == 0 {
                    q.setting_age_s(wlat, wlon, t_s).unwrap_or(-1)
                } else {
                    -1
                };
                let (c, _) = q.cold_ft(wlat, wlon, t_s, judged);
                (age, judged - c)
            }
            None => (-1, 0.0),
        };
        let ts =
            oa_msaw::calc::sample_time_s(usize::from(a.worst_k), i.cfg.samples, i.cfg.look_ahead_s);
        let speed = f64::from(a.vel_east_mps).hypot(f64::from(a.vel_north_mps));
        let (r95, growth, half) = (
            f64::from(a.r95_m),
            i.cfg.velocity_uncertainty_mps * ts,
            speed * step / 2.0,
        );
        let obst = if a.worst_kind == FloorKind::Obstacle {
            i.floors
                .obstacle_record_within(wlat, wlon, eff)
                .map(|(_, rec, dpt)| {
                    let agl = i
                        .dof
                        .and_then(|d| d.agl_ft.get(rec as usize).copied())
                        .map_or(f64::NAN, f64::from);
                    (agl, dpt)
                })
        } else {
            None
        };
        // Still below an obstacle top with the lookup circle cut to the track's 95 % radius, and to
        // the point alone.
        let still = |fl: &mut Floors, r: f64| {
            a.worst_kind == FloorKind::Obstacle
                && fl
                    .obstacle_record_within(wlat, wlon, r)
                    .is_some_and(|x| judged < f64::from(x.0))
        };
        let (r95_still, point_still) = (still(i.floors, r95), still(i.floors, POINT_M));
        let digest = oa_sha256::to_hex(&oa_sha256::sha256(&a.addr.to_be_bytes()));
        let secs = a.worst_ns.div_euclid(1_000_000_000).rem_euclid(86_400);
        all.push(Deep {
            hash: digest.chars().take(8).collect(),
            clock: format!("{:02}:{:02}:{:02}Z", secs / 3600, secs / 60 % 60, secs % 60),
            depth: -f64::from(a.worst_margin_ft),
            kind: a.worst_kind,
            height_terrain: tiles
                .peak(lat, lon, r95.max(POINT_M))
                .map(|(m, _)| f64::from(a.alt_ft) - m),
            apt_nm,
            thr_nm,
            corridor,
            phase,
            setting_age_s,
            cold_ft,
            basis: i
                .basis
                .get(&(a.addr, a.start_ns))
                .map_or("not judged", |b| b.text()),
            ttv: f64::from(a.ttv_min_s),
            stale: a.worst_flags & oa_msaw::AF_STALE_EXTRAPOLATED != 0,
            alt_delta: judged - f64::from(a.alt_ft),
            ahead_s: ts,
            span_s: (a.worst_ns - a.start_ns) as f64 / 1e9,
            hdg_off: off,
            apt_id: apt_id.to_string(),
            apt_cat,
            obst,
            r95,
            growth,
            half,
            rounding: eff - f64::from(a.worst_radius_m),
            eff,
            r95_still,
            point_still,
        });
    }
    let _ = writeln!(
        o,
        "  {} of {} alerts are deep. Phase: within {NEAR_NM} NM of a threshold and descending at {VRATE_FPM} fpm or more is final, within {NEAR_NM} NM of an airport with an approach and climbing at {VRATE_FPM} fpm or more is departure (the track's heading is not used). Height above the terrain is the alert's first-cycle altitude less the highest terrain within the 95 % radius of the track's present position.",
        all.len(),
        i.alerts.len()
    );
    if all.is_empty() {
        return;
    }
    let col = |f: &dyn Fn(&Deep) -> f64| all.iter().map(f).collect::<Vec<_>>();
    let count = |f: &dyn Fn(&Deep) -> String| {
        let mut m: BTreeMap<String, usize> = BTreeMap::new();
        for d in &all {
            *m.entry(f(d)).or_default() += 1;
        }
        format!("{m:?}")
    };
    let _ = writeln!(
        o,
        "  floor kind: {}",
        count(&|d| crate::report::kind_text(d.kind).to_string())
    );
    let _ = writeln!(o, "  phase: {}", count(&|d| d.phase.to_string()));
    let _ = writeln!(
        o,
        "  in a corridor at the worst point: {}",
        count(&|d| d.corridor.to_string())
    );
    let _ = writeln!(
        o,
        "  eligibility basis: {}",
        count(&|d| d.basis.to_string())
    );
    let _ = writeln!(
        o,
        "  the nearest airport is: {}; the nearest airport: {}",
        count(&|d| d.apt_cat.to_string()),
        count(&|d| d.apt_id.clone())
    );
    let _ = writeln!(
        o,
        "  depth below the floor, ft: {}",
        buckets(
            &col(&|d| d.depth),
            &[750.0, 1000.0, 1500.0, 2000.0, 3000.0],
            " ft"
        )
    );
    let heights: Vec<f64> = all.iter().filter_map(|d| d.height_terrain).collect();
    let _ = writeln!(
        o,
        "  track height above the highest terrain within its 95 % radius ({} of {} have terrain data): {}",
        heights.len(),
        all.len(),
        buckets(&heights, &[0.0, 500.0, 1000.0, 2000.0, 4000.0], " ft")
    );
    let _ = writeln!(
        o,
        "  distance to the nearest airport with an approach, NM: {}",
        buckets(&col(&|d| d.apt_nm), &[3.0, 5.0, 10.0, 20.0, 40.0], " NM")
    );
    let _ = writeln!(
        o,
        "  distance to the nearest runway threshold, NM: {}",
        buckets(&col(&|d| d.thr_nm), &[3.0, 5.0, 10.0, 20.0, 40.0], " NM")
    );
    let _ = writeln!(
        o,
        "  altimeter setting age (-1: fallback or none), s: {}",
        buckets(
            &col(&|d| d.setting_age_s as f64),
            &[0.0, 1800.0, 3600.0, 7200.0],
            " s"
        )
    );
    let _ = writeln!(
        o,
        "  cold correction at the judged altitude (about, ft): {}",
        buckets(
            &col(&|d| d.cold_ft),
            &[0.5, 25.0, 50.0, 100.0, 200.0],
            " ft"
        )
    );
    let _ = writeln!(
        o,
        "  time to violation, s: {}",
        buckets(&col(&|d| d.ttv), &[0.5, 5.0, 10.0, 15.0, 20.0], " s")
    );
    let _ = writeln!(
        o,
        "  stale-extrapolated at the worst cycle {} of {}; judged altitude less the first cycle's, ft: {}",
        all.iter().filter(|d| d.stale).count(),
        all.len(),
        buckets(&col(&|d| d.alt_delta), &[-1000.0, -500.0, -100.0, -1.0, 1.0], " ft")
    );
    let hdg: Vec<f64> = all.iter().filter_map(|d| d.hdg_off).collect();
    let _ = writeln!(
        o,
        "  how far ahead the worst point was, s: {}; seconds from the alert's start to the worst cycle: {}; track heading less the inbound heading of the best aligned runway or approach corridor holding the worst point ({} of {} are in one), degrees: {}",
        buckets(&col(&|d| d.ahead_s), &[1.0, 10.0, 30.0, 50.0], " s"),
        buckets(&col(&|d| d.span_s), &[1.0, 10.0, 30.0, 60.0, 120.0], " s"),
        hdg.len(),
        all.len(),
        buckets(&hdg, &[-90.0, -30.0, -10.0, 10.0, 30.0, 90.0], " deg")
    );
    let _ = writeln!(
        o,
        "  lookup radius, m: effective {}; 95 % radius of the track {}; speed term (half the step) {}; growth (velocity uncertainty x time) {}; rounding to the lookup {}",
        buckets(&col(&|d| d.eff), &[250.0, 500.0, 1000.0, 2000.0, 4000.0], " m"),
        buckets(&col(&|d| d.r95), &[100.0, 250.0, 500.0, 1000.0], " m"),
        buckets(&col(&|d| d.half), &[100.0, 200.0, 400.0, 800.0], " m"),
        buckets(&col(&|d| d.growth), &[50.0, 100.0, 200.0, 400.0], " m"),
        buckets(&col(&|d| d.rounding), &[100.0, 200.0, 400.0], " m")
    );
    // The suspicion that large circles near airports pick up tall obstacles: the obstacle floors
    // among them, against the other alerts, and what a smaller circle would say.
    let ob: Vec<&Deep> = all.iter().filter(|d| d.obst.is_some()).collect();
    let _ = writeln!(
        o,
        "  the circle against the rest: the deep alerts' median effective radius {:.0} m, median distance to an airport {:.1} NM, within 5 NM of an airport {} of {}; the shallower alerts' {:.0} m, {:.1} NM, {} of {}",
        median(&mut col(&|d| d.eff)).unwrap_or(f64::NAN),
        median(&mut col(&|d| d.apt_nm)).unwrap_or(f64::NAN),
        all.iter().filter(|d| d.apt_nm <= 5.0).count(),
        all.len(),
        median(&mut shallow_eff.clone()).unwrap_or(f64::NAN),
        median(&mut shallow_apt.clone()).unwrap_or(f64::NAN),
        shallow_apt.iter().filter(|x| **x <= 5.0).count(),
        shallow_apt.len()
    );
    let agl: Vec<f64> = ob.iter().filter_map(|d| d.obst.map(|x| x.0)).collect();
    let dist: Vec<f64> = ob.iter().filter_map(|d| d.obst.map(|x| x.1)).collect();
    let beyond_r95 = ob
        .iter()
        .filter(|d| d.obst.is_some_and(|x| x.1 > d.r95))
        .count();
    let _ = writeln!(
        o,
        "  the {} deep obstacle floors: obstacle height above ground {}; distance from the worst point to the obstacle {}; obstacle farther than the track's 95 % radius {}; still below an obstacle top with the lookup cut to the 95 % radius {}, and to the point alone ({POINT_M} m) {}",
        ob.len(),
        buckets(&agl, &[20.0, 50.0, 100.0, 200.0, 500.0], " ft"),
        buckets(&dist, &[100.0, 250.0, 500.0, 1000.0, 2000.0], " m"),
        beyond_r95,
        ob.iter().filter(|d| d.r95_still).count(),
        ob.iter().filter(|d| d.point_still).count()
    );
    // The groups.
    let mut groups: BTreeMap<String, Vec<&Deep>> = BTreeMap::new();
    for d in &all {
        let apt = if d.apt_nm <= 3.0 {
            "airport within 3 NM"
        } else if d.apt_nm <= NEAR_NM {
            "airport 3 to 10 NM"
        } else {
            "airport over 10 NM"
        };
        groups
            .entry(format!(
                "{} | {} | {apt} | corridor: {}",
                crate::report::kind_text(d.kind),
                d.phase,
                d.corridor
            ))
            .or_default()
            .push(d);
    }
    let mut order: Vec<(&String, &Vec<&Deep>)> = groups.iter().collect();
    order.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));
    let _ = writeln!(
        o,
        "  {} groups; the 10 largest, each with its deepest, a middle and its shallowest alert (hash; time; nearest airport; depth ft; height above terrain ft; airport NM; threshold NM; setting age s; cold ft; basis; ttv s; stale 0/1; judged less first altitude ft; worst point ahead s; start to worst cycle s; heading off the corridor deg; obstacle height above ground ft and distance m; lookup radius m = r95 + growth + speed term, rounding, effective):",
        groups.len()
    );
    for (name, g) in order.into_iter().take(10) {
        let _ = writeln!(o, "   {} alerts: {name}", g.len());
        let mut v: Vec<&&Deep> = g.iter().collect();
        v.sort_by(|a, b| oa_math::cmp_canon_f64(b.depth, a.depth));
        let mut picks = vec![0, v.len() / 2, v.len().saturating_sub(1)];
        picks.dedup();
        for k in picks {
            let Some(d) = v.get(k) else { continue };
            let _ = writeln!(
                o,
                "     {}; {}; {}; {:.0}; {}; {:.1}; {:.1}; {}; {:.0}; {}; {:.1}; {}; {:.0}; {:.0}; {:.0}; {}; {}; {:.0} = {:.0} + {:.0} + {:.0}, {:.0}, {:.0}",
                d.hash,
                d.clock,
                d.apt_id,
                d.depth,
                d.height_terrain.map_or("n/a".to_string(), |x| format!("{x:.0}")),
                d.apt_nm,
                d.thr_nm,
                d.setting_age_s,
                d.cold_ft,
                d.basis,
                d.ttv,
                u8::from(d.stale),
                d.alt_delta,
                d.ahead_s,
                d.span_s,
                d.hdg_off.map_or("-".to_string(), |x| format!("{x:.0}")),
                d.obst.map_or("-".to_string(), |x| format!("{:.0} ft, {:.0} m", x.0, x.1)),
                d.r95 + d.growth + d.half,
                d.r95,
                d.growth,
                d.half,
                d.rounding,
                d.eff
            );
        }
    }
}

/// The rules that held the worst point of an alert, and how far the track's ground track was off the
/// best aligned runway or approach corridor that holds the point (degrees, -180 to 180): `box rules,
/// approach box` and `box rules, runway strip` where the core held it in an aligned corridor,
/// `general rules, in a box but not aligned` where a box or strip holds the point and the track is
/// not aligned with it, `general rules, no box` elsewhere.
fn rules_of(a: &Alert, i: &DiagIn) -> (&'static str, Option<f64>) {
    let mut held = false;
    let mut off: Option<f64> = None;
    for k in i.corridors {
        if k.kind == oa_msaw::CK_DEPARTURE {
            continue;
        }
        if oa_msaw::corridor_hit_ft(
            &k.corridor,
            a.worst_lat_e7,
            a.worst_lon_e7,
            f64::from(a.worst_radius_m),
            i.cfg,
        )
        .is_some()
        {
            held = true;
            if a.track_deg >= 0 {
                let t = f64::from(a.track_deg);
                let mut d =
                    (t - (k.corridor.out_bearing_deg + 180.0) + 540.0).rem_euclid(360.0) - 180.0;
                if k.kind == oa_msaw::CK_RUNWAY {
                    let e = (t - k.corridor.out_bearing_deg + 540.0).rem_euclid(360.0) - 180.0;
                    if e.abs() < d.abs() {
                        d = e;
                    }
                }
                if off.is_none_or(|b| d.abs() < b.abs()) {
                    off = Some(d);
                }
            }
        }
    }
    let rules = if a.worst_kinds & oa_msaw::KB_RUNWAY != 0 {
        "box rules, runway strip"
    } else if a.worst_kinds & oa_msaw::KB_APPROACH != 0 {
        "box rules, approach box"
    } else if held {
        "general rules, in a box but not aligned"
    } else {
        "general rules, no box"
    };
    (rules, off)
}

/// The nearest airport with an approach to an alert, by what the run built for it: `configured` (its
/// own corridors from the configuration), `CIFP-built` (built from the CIFP for the area),
/// `approach, no corridors` (it has an approach and got none) and `no approach airport within 10 NM`.
fn airport_category(i: &DiagIn, id: &str, nm: f64) -> &'static str {
    let label_is = |k: &crate::cifp::CifpCorridor| k.label.split(' ').next() == Some(id);
    if nm > NEAR_NM {
        "no approach airport within 10 NM"
    } else if i.corridors.iter().take(i.n_configured).any(label_is) {
        "configured"
    } else if i.corridors.iter().skip(i.n_configured).any(label_is) {
        "CIFP-built"
    } else {
        "approach, no corridors"
    }
}

/// What rules held the worst point of every alert, and of the airports they were near.
fn alignment(o: &mut String, i: &DiagIn) {
    let _ = writeln!(
        o,
        "\n8. the alignment of every alert (rules at the worst point; the track's ground track is the first cycle's)"
    );
    let mut rules: BTreeMap<&str, usize> = BTreeMap::new();
    let mut cross: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    let mut off_all: Vec<f64> = Vec::new();
    let mut off_unaligned: Vec<f64> = Vec::new();
    let mut shallow_deep: BTreeMap<(&str, bool), usize> = BTreeMap::new();
    for a in i.alerts {
        let (r, off) = rules_of(a, i);
        *rules.entry(r).or_default() += 1;
        let (lat, lon) = (f64::from(a.lat_e7) / 1e7, f64::from(a.lon_e7) / 1e7);
        let (apt_nm, apt_id) = i
            .all_airports
            .iter()
            .map(|(id, la, lo)| (dist_m((lat, lon), (*la, *lo)) / M_PER_NM, id.as_str()))
            .fold((f64::INFINITY, ""), |b, x| if x.0 < b.0 { x } else { b });
        let cat = airport_category(i, apt_id, apt_nm);
        *cross.entry((cat, r)).or_default() += 1;
        *shallow_deep
            .entry((r, f64::from(a.worst_margin_ft) <= -DEEP_FT))
            .or_default() += 1;
        if let Some(d) = off {
            off_all.push(d.abs());
            if r == "general rules, in a box but not aligned" {
                off_unaligned.push(d.abs());
            }
        }
    }
    let _ = writeln!(
        o,
        "  rules at the worst point, all {} alerts: {rules:?}",
        i.alerts.len()
    );
    let fast: Vec<&Alert> = i
        .alerts
        .iter()
        .filter(|a| f64::from(a.vrate_fpm).abs() > 6000.0 && a.alt_ft < 10_000.0)
        .collect();
    let _ = writeln!(
        o,
        "  alerts whose vertical rate at the first cycle was over 6,000 ft/min either way below 10,000 ft: {} (of the 500 ft or more deep ones {})",
        fast.len(),
        fast.iter()
            .filter(|a| f64::from(a.worst_margin_ft) <= -DEEP_FT)
            .count()
    );
    for ((cat, r), n) in &cross {
        let _ = writeln!(o, "  nearest airport {cat}; {r}: {n}");
    }
    let _ = writeln!(
        o,
        "  by depth (rules; 500 ft or more below the floor): {shallow_deep:?}"
    );
    let _ = writeln!(
        o,
        "  ground track off the best aligned runway or approach corridor holding the worst point (absolute, degrees; {} alerts have one): {}; of them not aligned (general rules) {}: {}",
        off_all.len(),
        buckets(&off_all, &[10.0, 30.0, 60.0, 120.0, 150.0], " deg"),
        off_unaligned.len(),
        buckets(&off_unaligned, &[60.0, 120.0, 150.0], " deg")
    );
}
