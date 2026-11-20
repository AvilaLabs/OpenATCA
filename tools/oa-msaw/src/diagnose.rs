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
    pub sealed: &'a Sealed,
    /// The corridors of the CIFP, and the altimeter settings, for the landing zone.
    pub corridors: &'a [CifpCorridor],
    pub qnh: Option<&'a mut Qnh>,
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
