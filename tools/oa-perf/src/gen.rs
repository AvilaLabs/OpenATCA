//! Deterministic synthetic traffic for the capacity runs: `n` aircraft on straight constant-speed
//! tracks, their 1 Hz ADS-B reports and the plots of overlapping radars.
//!
//! Everything is a pure function of the configuration and the window asked for. The simulator
//! of `oa-sensorsim` is not used: it builds a truth for at most 4,096 aircraft and solves every
//! beam crossing from scratch, far too slow for 38,000. The geometry here is the tracker's own
//! forward model (`oa_track::radar::polar_of`), the beam crossing is found by iterating the
//! aircraft's position against the beam azimuth, and the errors are the profile's range and
//! azimuth sigmas.
//!
//! Stated simplifications (the numbers in a report carry them): straight flight at constant
//! speed and altitude (reflected at the walls of the box where it would leave); every plot detected (no misses, no false plots, no splits); plots carry
//! no address; no line-of-sight test; ADS-B noise white at NACp 9.

use oa_geo::aeqd::{frame_new, try_frame_velocity, unproject, Frame};
use oa_sensorsim::prng::Rng;
use oa_sensorsim::profile::MSSR_EUROCONTROL_1997;
use oa_track::fusion_score::{make_radar, site_def, Kind, EPOCH_NS};
use oa_track::radar::{
    polar_of, radar_geom, RadarGeom, RadarPlot, RadarSite, FT_M, MIN_PLOT_RANGE_M, NONE_CODE,
    PLOT_NO_ADDR,
};
use oa_track::{Measurement, M_HAS_VEL, NO_IDENT};

/// Centre of the traffic box.
pub const CENTRE_LAT: f64 = 41.9;
pub const CENTRE_LON: f64 = -88.3;
/// Aircraft per square kilometre at the whole-NAS figure: 38,000 over about 8.1 million km2 of
/// the contiguous United States.
pub const NAS_DENSITY_PER_KM2: f64 = 38_000.0 / 8.1e6;
/// The half-width of the "stress" box: every aircraft is inside the coverage of every radar.
pub const STRESS_HALF_KM: f64 = 50.0;
/// First aircraft address.
pub const ADDR_BASE: u32 = 0xA0_0000;
/// Most aircraft: addresses stay below 2^24.
pub const MAX_AIRCRAFT: usize = 0x5F_0000;
const NS_PER_S: i64 = 1_000_000_000;
/// A beam crossing is settled when the beam's azimuth at the stamped time is within this of the
/// aircraft's, degrees (0.01 degrees at 100 km is 17 m; the loop converges to far less).
const CROSSING_TOL_DEG: f64 = 0.005;
/// ADS-B position sigma per axis, metres: the NACp 9 bound (30 m, 95 %) over 2.448.
const ADSB_SIGMA_POS_M: f64 = 30.0 / 2.447_746_830_680_816;
/// ADS-B velocity sigma per axis, m/s: NACv 2.
const ADSB_SIGMA_VEL_MPS: f64 = 3.0 / 2.447_746_830_680_816;

/// Where the aircraft are spread.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Layout {
    /// A box sized for the whole-NAS density: the radars see only the aircraft near them.
    Nas,
    /// A fixed small box: every radar sees every aircraft (the worst case for plot load).
    Stress,
}

impl Layout {
    pub fn parse(s: &str) -> Option<Layout> {
        match s {
            "nas" => Some(Layout::Nas),
            "stress" => Some(Layout::Stress),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Layout::Nas => "nas",
            Layout::Stress => "stress",
        }
    }
}

/// What to generate.
#[derive(Debug, Clone, PartialEq)]
pub struct TrafficCfg {
    pub seed: u64,
    pub n: usize,
    pub layout: Layout,
    /// Fraction of aircraft that send ADS-B.
    pub adsb_frac: f64,
    /// Radars, in order; offsets from the centre are fixed (see [`RADAR_OFFSETS_KM`]).
    pub radars: Vec<Kind>,
    /// Aircraft that would leave the box within this time fly the other way; any that would still
    /// leave it reflect at the wall, so every aircraft is in the box at every time.
    pub horizon_s: f64,
}

/// East and north offsets of the first radars from the centre, km.
pub const RADAR_OFFSETS_KM: [(f64, f64); 4] =
    [(0.0, 0.0), (35.0, 0.0), (-17.5, 30.0), (-17.5, -30.0)];

/// Half the width of the box, km.
pub fn half_km(cfg: &TrafficCfg) -> f64 {
    match cfg.layout {
        Layout::Stress => STRESS_HALF_KM,
        Layout::Nas => (cfg.n as f64 / NAS_DENSITY_PER_KM2).sqrt() / 2.0,
    }
}

/// One aircraft: a constant-speed track in the east-north plane of the centre. Straight while it
/// stays in the box; at a wall (the box is `half_m` either side of the centre) it reflects, so
/// the aircraft is in the box at every time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aircraft {
    pub addr: u32,
    pub e0: f64,
    pub n0: f64,
    pub ve: f64,
    pub vn: f64,
    /// Half the width of the box the aircraft keeps inside, metres.
    pub half_m: f64,
    pub alt_ft: i32,
    /// Offset of its reports within each second, ns (whole milliseconds).
    pub phase_ns: i64,
    pub adsb: bool,
}

/// A coordinate `x0 + v t` reflected into `[-half, half]`, and the sign of its motion.
fn reflect(x0: f64, v: f64, t_s: f64, half: f64) -> (f64, f64) {
    let u = (x0 + v * t_s + half).rem_euclid(4.0 * half);
    if u < 2.0 * half {
        (u - half, 1.0)
    } else {
        (3.0 * half - u, -1.0)
    }
}

impl Aircraft {
    fn pos(&self, t_s: f64) -> (f64, f64) {
        (
            reflect(self.e0, self.ve, t_s, self.half_m).0,
            reflect(self.n0, self.vn, t_s, self.half_m).0,
        )
    }

    /// East and north velocity at `t_s`, metres per second.
    pub fn vel(&self, t_s: f64) -> (f64, f64) {
        (
            self.ve * reflect(self.e0, self.ve, t_s, self.half_m).1,
            self.vn * reflect(self.n0, self.vn, t_s, self.half_m).1,
        )
    }
}

struct Rad {
    site: RadarSite,
    geom: RadarGeom,
    coverage_m: f64,
    /// Next scan to generate.
    next_scan: i64,
    /// Plots generated and not yet handed out, in time order.
    queue: Vec<RadarPlot>,
}

/// The generator. `adsb_window` and `plot_windows` take second numbers in increasing order.
pub struct Traffic {
    cfg: TrafficCfg,
    ac: Vec<Aircraft>,
    frame: Frame,
    rads: Vec<Rad>,
}

fn round_ms(t_ns: i64) -> i64 {
    (t_ns + 500_000).div_euclid(1_000_000) * 1_000_000
}

impl Traffic {
    pub fn new(cfg: &TrafficCfg) -> Result<Traffic, String> {
        if cfg.n == 0 || cfg.n > MAX_AIRCRAFT {
            return Err(format!(
                "aircraft count {} outside 1 to {MAX_AIRCRAFT}",
                cfg.n
            ));
        }
        if !(0.0..=1.0).contains(&cfg.adsb_frac) {
            return Err("adsb fraction outside 0 to 1".to_string());
        }
        if cfg.radars.len() > RADAR_OFFSETS_KM.len() {
            return Err(format!("at most {} radars", RADAR_OFFSETS_KM.len()));
        }
        if !(cfg.horizon_s.is_finite() && cfg.horizon_s > 0.0) {
            return Err("horizon".to_string());
        }
        let frame = frame_new(CENTRE_LAT, CENTRE_LON);
        let half = half_km(cfg) * 1000.0;
        let mut ac = Vec::with_capacity(cfg.n);
        for i in 0..cfg.n {
            let mut rng = Rng::from_stream(cfg.seed, 0x4143, i as u64);
            let e0 = (rng.unit() * 2.0 - 1.0) * half;
            let n0 = (rng.unit() * 2.0 - 1.0) * half;
            let speed = 120.0 + 140.0 * rng.unit();
            let mut hdg = rng.unit() * std::f64::consts::TAU;
            // An airliner that would leave the box within the horizon flies the other way; one
            // that would leave it either way (or later, past the horizon) reflects at the wall.
            let reach = speed * cfg.horizon_s;
            let (e1, n1) = (e0 + reach * hdg.sin(), n0 + reach * hdg.cos());
            if e1.abs() > half || n1.abs() > half {
                hdg += std::f64::consts::PI;
            }
            // Altitude 2,000 to 45,000 ft in 100 ft steps.
            let alt_ft = 2_000 + 100 * ((rng.unit() * 430.0) as i32);
            let phase_ns =
                round_ms((rng.unit() * NS_PER_S as f64) as i64).min(NS_PER_S - 1_000_000);
            let adsb = rng.unit() < cfg.adsb_frac;
            ac.push(Aircraft {
                addr: ADDR_BASE + i as u32,
                e0,
                n0,
                ve: speed * hdg.sin(),
                vn: speed * hdg.cos(),
                half_m: half,
                alt_ft,
                phase_ns,
                adsb,
            });
        }
        let mut rads = Vec::new();
        for (i, kind) in cfg.radars.iter().enumerate() {
            let &(oe, on) = RADAR_OFFSETS_KM.get(i).ok_or("radar index")?;
            let (lat, lon) =
                unproject(&frame, oe * 1000.0, on * 1000.0).ok_or("radar offset off the chart")?;
            let mut site = make_radar(*kind, MSSR_EUROCONTROL_1997, 0.0, 0.0).trk;
            site.lat_deg = lat;
            site.lon_deg = lon;
            site.sensor = 1 + i as u16;
            rads.push(Rad {
                geom: radar_geom(&site),
                site,
                coverage_m: site_def(*kind).coverage_m,
                next_scan: 0,
                queue: Vec::new(),
            });
        }
        Ok(Traffic {
            cfg: cfg.clone(),
            ac,
            frame,
            rads,
        })
    }

    pub fn aircraft(&self) -> &[Aircraft] {
        &self.ac
    }

    /// The tracker's sites for the radars.
    pub fn sites(&self) -> Vec<RadarSite> {
        self.rads.iter().map(|r| r.site).collect()
    }

    /// Aircraft that send ADS-B.
    pub fn adsb_count(&self) -> usize {
        self.ac.iter().filter(|a| a.adsb).count()
    }

    /// The ADS-B reports timed in second `k` (from `EPOCH_NS + k` s up to one second later):
    /// one per equipped aircraft, appended to `out`.
    pub fn adsb_window(&self, k: i64, out: &mut Vec<Measurement>) {
        let var = (ADSB_SIGMA_POS_M * ADSB_SIGMA_POS_M) as f32;
        let vvar = (ADSB_SIGMA_VEL_MPS * ADSB_SIGMA_VEL_MPS) as f32;
        for a in self.ac.iter().filter(|a| a.adsb) {
            let t_ns = EPOCH_NS + k * NS_PER_S + a.phase_ns;
            let t_s = k as f64 + a.phase_ns as f64 * 1.0e-9;
            let (e, n) = a.pos(t_s);
            let (ve, vn) = a.vel(t_s);
            let mut rng = Rng::from_stream(self.cfg.seed, u64::from(a.addr), k as u64);
            let (ne, nn) = (
                rng.normal() * ADSB_SIGMA_POS_M,
                rng.normal() * ADSB_SIGMA_POS_M,
            );
            let (nve, nvn) = (
                rng.normal() * ADSB_SIGMA_VEL_MPS,
                rng.normal() * ADSB_SIGMA_VEL_MPS,
            );
            let Some((lat, lon)) = unproject(&self.frame, e + ne, n + nn) else {
                continue;
            };
            // Inside the box both always succeed (OA-PRF-011); a failure is a generator fault the
            // coverage test would show, never a report with a made-up velocity.
            let Some((te, tn)) = try_frame_velocity(&self.frame, e, n, ve, vn) else {
                continue;
            };
            out.push(Measurement {
                toa_ns: t_ns,
                pos_cov_m2: [var, 0.0, var],
                vel_cov_m2s2: [vvar, 0.0, vvar],
                vel_east_mps: (te + nve) as f32,
                vel_north_mps: (tn + nvn) as f32,
                lat_e7: (lat * 1.0e7).round() as i32,
                lon_e7: (lon * 1.0e7).round() as i32,
                alt_ft: ((f64::from(a.alt_ft) / 25.0).round() * 25.0) as i32,
                vrate_fpm: 0,
                addr: a.addr,
                alt_quant_ft: 25,
                sensor: 0,
                ident: NO_IDENT,
                flags: M_HAS_VEL,
                category: 0,
            });
        }
    }

    /// The plots of every radar timed in second `k`, one vector per radar in `out` (cleared
    /// first), in time order. Seconds must be asked in increasing order.
    pub fn plot_windows(&mut self, k: i64, out: &mut Vec<Vec<RadarPlot>>) {
        out.resize_with(self.rads.len(), Vec::new);
        let end = EPOCH_NS + (k + 1) * NS_PER_S;
        let seed = self.cfg.seed;
        for (ri, (rad, o)) in self.rads.iter_mut().zip(out.iter_mut()).enumerate() {
            o.clear();
            let p_ns = rad.site.period_ns;
            while EPOCH_NS + rad.next_scan * p_ns < end {
                let j = rad.next_scan;
                let from = rad.queue.len();
                scan_plots(&self.ac, &self.frame, seed, ri as u64, rad, j);
                if let Some(tail) = rad.queue.get_mut(from..) {
                    tail.sort_by_key(|p| (p.time_ns, p.label));
                }
                rad.next_scan += 1;
            }
            let cut = rad.queue.partition_point(|p| p.time_ns < end);
            o.extend(rad.queue.drain(..cut));
        }
    }
}

/// Appends the plots of one revolution of `rad` (scan `j`) to its queue.
fn scan_plots(ac: &[Aircraft], frame: &Frame, seed: u64, ri: u64, rad: &mut Rad, j: i64) {
    let p_ns = rad.site.period_ns;
    let start = EPOCH_NS + j * p_ns;
    let sig_r = rad.site.sigma_range_m;
    let sig_a = rad.site.sigma_az_deg;
    let q = f64::from(rad.site.alt_quantum_ft.max(1));
    for a in ac {
        let h = f64::from(a.alt_ft) * FT_M;
        let mut t_ns = start + p_ns / 2;
        let mut polar = (0.0, 0.0);
        let mut ok = true;
        let mut settled = false;
        // The beam reaches azimuth az at start + az/360 of a period; the aircraft moves meanwhile.
        for _ in 0..5 {
            let (e, n) = a.pos((t_ns - EPOCH_NS) as f64 * 1.0e-9);
            let Some((lat, lon)) = unproject(frame, e, n) else {
                ok = false;
                break;
            };
            polar = polar_of(&rad.geom, lat, lon, h);
            let next = start + (polar.1 / 360.0 * p_ns as f64) as i64;
            // Settled when the beam is at the aircraft's azimuth at the time the aircraft was
            // placed at. An aircraft crossing the beam's starting azimuth during the revolution
            // has no crossing in it (the iteration then jumps between its two ends): no plot,
            // as the simulator of `oa-sensorsim` makes none for an unsolved crossing.
            let beam = (t_ns - start) as f64 / p_ns as f64 * 360.0;
            if ((polar.1 - beam + 540.0).rem_euclid(360.0) - 180.0).abs() < CROSSING_TOL_DEG {
                settled = true;
                break;
            }
            t_ns = next;
        }
        if !ok || !settled || polar.0 > rad.coverage_m || polar.0 < 2.0 * MIN_PLOT_RANGE_M {
            continue;
        }
        let mut rng = Rng::from_stream(seed.wrapping_add(ri + 1), u64::from(a.addr), j as u64);
        let (nr, na) = (rng.normal(), rng.normal());
        rad.queue.push(RadarPlot {
            time_ns: round_ms(t_ns),
            range_m: polar.0 + nr * sig_r,
            az_deg: (polar.1 + na * sig_a).rem_euclid(360.0),
            alt_ft: ((f64::from(a.alt_ft) / q).round() * q) as i32,
            addr: PLOT_NO_ADDR,
            label: a.addr,
            mode_a: NONE_CODE,
        });
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn cfg(seed: u64, n: usize, layout: Layout) -> TrafficCfg {
        TrafficCfg {
            seed,
            n,
            layout,
            adsb_frac: 0.5,
            radars: vec![Kind::EnRoute, Kind::Terminal, Kind::Terminal],
            horizon_s: 120.0,
        }
    }

    fn all(c: &TrafficCfg, secs: i64) -> (Vec<Measurement>, Vec<Vec<RadarPlot>>) {
        let mut t = Traffic::new(c).unwrap();
        let mut adsb = Vec::new();
        let mut plots: Vec<Vec<RadarPlot>> = vec![Vec::new(); c.radars.len()];
        let mut w = Vec::new();
        for k in 0..secs {
            t.adsb_window(k, &mut adsb);
            t.plot_windows(k, &mut w);
            for (a, b) in plots.iter_mut().zip(&w) {
                a.extend_from_slice(b);
            }
        }
        (adsb, plots)
    }

    // REQ: OA-PRF-002
    #[test]
    fn the_same_seed_gives_the_same_traffic_and_another_does_not() {
        for layout in [Layout::Stress, Layout::Nas] {
            let c = cfg(7, 60, layout);
            let (a1, p1) = all(&c, 30);
            let (a2, p2) = all(&c, 30);
            assert!(!a1.is_empty() && p1.iter().all(|p| !p.is_empty()));
            assert_eq!(a1, a2);
            assert_eq!(p1, p2);
            let t1 = Traffic::new(&c).unwrap();
            let t2 = Traffic::new(&c).unwrap();
            assert_eq!(t1.aircraft(), t2.aircraft());
            let (a3, p3) = all(&cfg(8, 60, layout), 30);
            assert_ne!(a1, a3);
            assert_ne!(p1, p3);
        }
    }

    // REQ: OA-PRF-008
    #[test]
    fn every_plot_is_where_its_aircraft_is_at_the_time_it_carries() {
        // The beam crossing is solved against the moving aircraft. An aircraft crossing the beam's
        // starting azimuth during a revolution has no crossing in it (or one at either end of the
        // revolution), and a plot made from a crossing that was not found would carry the position
        // of one time and the stamp of another.
        let c = cfg(1, 1200, Layout::Stress);
        let mut t = Traffic::new(&c).unwrap();
        let mut w = Vec::new();
        let mut checked = 0usize;
        for k in 0..120 {
            t.plot_windows(k, &mut w);
            for (ri, ps) in w.iter().enumerate() {
                let rad = &t.rads[ri];
                for p in ps {
                    let a = &t.ac[(p.label - ADDR_BASE) as usize];
                    let (e, n) = a.pos((p.time_ns - EPOCH_NS) as f64 * 1.0e-9);
                    let (lat, lon) = unproject(&t.frame, e, n).unwrap();
                    let (rho, az) = polar_of(&rad.geom, lat, lon, f64::from(a.alt_ft) * FT_M);
                    let dr = (p.range_m - rho).abs() / rad.site.sigma_range_m;
                    let da = ((p.az_deg - az + 540.0).rem_euclid(360.0) - 180.0).abs()
                        / rad.site.sigma_az_deg;
                    // 6 sigma of two normals, and the millisecond rounding of the stamp.
                    assert!(
                        dr < 6.0 && da < 6.0,
                        "radar {ri} aircraft {:#x} t={:.3}: {dr:.1} sigma in range, {da:.1} in azimuth",
                        p.label,
                        (p.time_ns - EPOCH_NS) as f64 * 1.0e-9
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked > 40_000, "{checked}");
    }

    // REQ: OA-PRF-002
    #[test]
    fn windows_are_the_same_whether_asked_in_one_run_or_two() {
        // The ADS-B of second k does not depend on the seconds before it.
        let c = cfg(3, 40, Layout::Stress);
        let t = Traffic::new(&c).unwrap();
        let (mut whole, mut late) = (Vec::new(), Vec::new());
        for k in 0..12 {
            t.adsb_window(k, &mut whole);
        }
        for k in 8..12 {
            t.adsb_window(k, &mut late);
        }
        assert!(whole.ends_with(&late));
    }

    // REQ: OA-PRF-002
    #[test]
    fn rates_match_the_stated_ones() {
        let c = cfg(1, 200, Layout::Stress);
        let t = Traffic::new(&c).unwrap();
        let equipped = t.adsb_count();
        assert!(equipped > 70 && equipped < 130, "{equipped}");
        let (adsb, plots) = all(&c, 120);
        // One ADS-B report per equipped aircraft per second, each in its own second.
        assert_eq!(adsb.len(), equipped * 120);
        for k in 0..120i64 {
            let lo = EPOCH_NS + k * NS_PER_S;
            let n = adsb
                .iter()
                .filter(|m| m.toa_ns >= lo && m.toa_ns < lo + NS_PER_S)
                .count();
            assert_eq!(n, equipped, "second {k}");
        }
        // Every aircraft is in range of every radar in the stress box: one plot per scan.
        let periods = [12.0, 4.8, 4.8];
        for (r, p) in plots.iter().enumerate() {
            let want = 200.0 * 120.0 / periods[r];
            assert!(
                (p.len() as f64 - want).abs() <= 200.0 * 2.0,
                "radar {r}: {} plots, want about {want}",
                p.len()
            );
            assert!(p.windows(2).all(|w| w[0].time_ns <= w[1].time_ns));
            assert!(p.iter().all(|x| oa_track::radar::plot_check(x).is_ok()));
        }
    }

    // REQ: OA-PRF-002
    #[test]
    fn plots_fall_in_the_second_they_are_asked_for() {
        let c = cfg(5, 80, Layout::Stress);
        let mut t = Traffic::new(&c).unwrap();
        let mut w = Vec::new();
        for k in 0..40i64 {
            t.plot_windows(k, &mut w);
            let (lo, hi) = (EPOCH_NS + k * NS_PER_S, EPOCH_NS + (k + 1) * NS_PER_S);
            assert!(w
                .iter()
                .flatten()
                .all(|p| p.time_ns >= lo && p.time_ns < hi));
        }
    }

    // REQ: OA-PRF-002
    #[test]
    fn the_nas_box_holds_the_nas_density() {
        let c = cfg(1, 38_000, Layout::Nas);
        let side = 2.0 * half_km(&c);
        let dens = 38_000.0 / (side * side);
        assert!((dens - NAS_DENSITY_PER_KM2).abs() < 1e-9);
        assert!(Traffic::new(&cfg(1, 0, Layout::Nas)).is_err());
    }

    // REQ: OA-PRF-011
    #[test]
    fn every_aircraft_stays_in_the_box_and_in_every_radars_coverage_for_the_whole_fed_interval() {
        // Warm-up plus measurement of the long runs, and a run far longer than the box is wide.
        for horizon_s in [120.0, 900.0, 7_200.0] {
            for seed in 0..40u64 {
                let c = TrafficCfg {
                    horizon_s,
                    ..cfg(seed, 300, Layout::Stress)
                };
                let t = Traffic::new(&c).unwrap();
                let half = STRESS_HALF_KM * 1000.0;
                let step = if horizon_s > 1_000.0 { 7 } else { 1 };
                for a in t.aircraft() {
                    // Corner and wall starts are the ones that leave: the generator may place the
                    // start anywhere in the box, and the box is covered at every point.
                    let h = f64::from(a.alt_ft) * FT_M;
                    for s in (0..horizon_s as i64).step_by(step) {
                        let (e, n) = a.pos(s as f64);
                        assert!(
                            e.abs() <= half + 1e-6 && n.abs() <= half + 1e-6,
                            "seed {seed} horizon {horizon_s}: {:#x} at {s} s is at ({e:.0}, {n:.0})",
                            a.addr
                        );
                        let (lat, lon) = unproject(&t.frame, e, n).unwrap();
                        for (ri, r) in t.rads.iter().enumerate() {
                            let (rho, _) = polar_of(&r.geom, lat, lon, h);
                            assert!(
                                rho <= r.coverage_m,
                                "seed {seed} horizon {horizon_s}: {:#x} at {s} s is {rho:.0} m from radar {ri} (coverage {:.0})",
                                a.addr,
                                r.coverage_m
                            );
                        }
                    }
                }
            }
        }
    }

    // REQ: OA-PRF-011
    #[test]
    fn reflection_folds_a_coordinate_into_the_box_and_flips_its_velocity_at_a_wall() {
        let half = 50_000.0;
        // From the east wall, heading east: straight back in.
        assert_eq!(reflect(half, 100.0, 10.0, half), (half - 1_000.0, -1.0));
        // From a corner, heading out on both axes, a long way: always inside.
        for k in 0..2_000 {
            let t = f64::from(k) * 37.0;
            for (x0, v) in [(half, 250.0), (-half, -250.0), (0.0, 120.0), (half, -120.0)] {
                let (x, sign) = reflect(x0, v, t, half);
                assert!(
                    x.abs() <= half + 1e-6 && sign.abs() == 1.0,
                    "{x0} {v} {t}: {x}"
                );
            }
        }
        // Inside the box the track is straight.
        assert_eq!(reflect(-10_000.0, 100.0, 50.0, half), (-5_000.0, 1.0));
    }
}
