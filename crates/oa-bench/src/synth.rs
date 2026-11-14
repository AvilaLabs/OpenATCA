//! The synthetic self-test (M2_DESIGN section 4). Core (pure): truth is generated from closed-form
//! paths, the legacy stream from the truth with injected errors, and the metric code must
//! recover each injected value.
//!
//! **The world.** Aircraft fly on a flat east-north plane around `(LAT0, LON0)`: straight and
//! level, a constant-rate turn, a climb, an approach that ends on a runway threshold, a takeoff
//! from one, a crossing pair (the track swap), a formation pair sharing one callsign, a callsign
//! whose positions are far from its truth, and one flight with an address truth does not know.
//! The zone is a square of 80 km around the origin.
//!
//! **What is injected** ([`Injected`], recovered by the metrics, see [`self_test`]): a cross-track
//! bias and noise, a constant along-track time offset (and, in the drift scenario, a ramp), speed
//! and heading biases, an altitude bias on altitudes quantised to 100 ft, decimation to one
//! position per 12 s, a receive delay, a stretch of coasting with an extra position error, a
//! start delay and a dropout (presence and initiation), TBFM and TFMS estimate errors per
//! look-ahead, handoff times and an off-route stretch, and one deliberate track swap.
//!
//! The pseudo-random noise is a fixed-seed generator, so every run is identical.

use crate::geo::{enu, from_enu, KT_MPS};
use crate::legacy::{
    flight_ix, hev, new_legacy, pad, push_handoff, push_route, push_tbfm, push_tfms, push_th,
    set_addr, tbfm_kind, tfms_kind, Legacy, RoutePointIn, TfmsIn, NO_COORD, NO_TIME,
};
use crate::run::{find, run_benchmark, RunConfig, RunOutput};
use crate::stats::MetricResult;
use crate::truth::{AirportRef, Env, NoTruth, Truth, TruthLeg, TruthSample};

/// Latitude of the world origin, degrees.
pub const LAT0: f64 = 41.9;
/// Longitude of the world origin, degrees.
pub const LON0: f64 = -87.9;
/// Half-width of the zone square, metres.
pub const ZONE_M: f64 = 80_010.0;
/// Common start time of the world, Unix ms (2027-01-15T08:00:00Z).
pub const T0_MS: i64 = 1_800_000_000_000;

/// A closed-form path.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug)]
pub enum Path {
    /// Straight, constant speed, constant vertical rate.
    Line {
        x0: f64,
        y0: f64,
        vx: f64,
        vy: f64,
        alt0_ft: f64,
        vrate_fpm: f64,
    },
    /// Constant-rate turn at constant speed, level.
    Turn {
        x0: f64,
        y0: f64,
        speed: f64,
        heading0_rad: f64,
        omega_rad_s: f64,
        alt_ft: f64,
    },
}

/// One synthetic aircraft.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug)]
pub struct SynthAc {
    pub addr: u32,
    pub callsign: &'static str,
    pub path: Path,
    /// Leg start and end relative to [`T0_MS`], seconds.
    pub start_s: f64,
    pub end_s: f64,
}

/// East, north, velocity east, velocity north (m, m/s), altitude ft and vertical rate fpm at `s`
/// seconds after the aircraft's start.
fn path_state(p: &Path, s: f64) -> (f64, f64, f64, f64, f64, f64) {
    match *p {
        Path::Line {
            x0,
            y0,
            vx,
            vy,
            alt0_ft,
            vrate_fpm,
        } => (
            x0 + vx * s,
            y0 + vy * s,
            vx,
            vy,
            alt0_ft + vrate_fpm / 60.0 * s,
            vrate_fpm,
        ),
        Path::Turn {
            x0,
            y0,
            speed,
            heading0_rad,
            omega_rad_s,
            alt_ft,
        } => {
            let th = heading0_rad + omega_rad_s * s;
            let k = speed / omega_rad_s;
            (
                x0 + k * (heading0_rad.cos() - th.cos()),
                y0 + k * (th.sin() - heading0_rad.sin()),
                speed * th.sin(),
                speed * th.cos(),
                alt_ft,
                0.0,
            )
        }
    }
}

/// The injected errors (cold: A7.2).
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug)]
pub struct Injected {
    /// Cross-track bias, metres to the right of the track.
    pub cross_bias_m: f64,
    pub noise_cross_m: f64,
    pub noise_along_m: f64,
    /// ERAM positions are the truth positions this many seconds ahead.
    pub tau_s: f64,
    /// Growth of the offset, seconds of offset per second of time (the drift scenario).
    pub tau_ramp: f64,
    pub speed_bias_kt: f64,
    pub hdg_bias_deg: f64,
    /// Added to the altitude after quantising it to 100 ft.
    pub alt_bias_ft: f64,
    /// Decimation: seconds between published positions.
    pub interval_s: f64,
    /// Receive time minus position time, seconds.
    pub stale_s: f64,
    /// Coasting stretch of aircraft 3, seconds after its start, and its extra cross error.
    pub coast_from_s: f64,
    pub coast_to_s: f64,
    pub coast_extra_m: f64,
    /// Aircraft 1 publishes nothing between these times (seconds after its start).
    pub drop_from_s: f64,
    pub drop_to_s: f64,
    /// First position of aircraft 1 and 3, seconds after their zone entry.
    pub init_delay_s: f64,
    /// Handoff of aircraft 1: initiation and acceptance relative to the boundary crossing.
    pub handoff_init_s: f64,
    pub handoff_accept_s: f64,
    /// TBFM ETA errors at the meter fix (seconds) for look-aheads of 100, 400, 1500 and 3100 s.
    pub tbfm_eta_err_s: [f64; 4],
    /// TBFM `sta_sfx` error, issued 200 s ahead.
    pub tbfm_sta_err_s: f64,
    /// TFMS ETA errors for look-aheads of 200 and 600 s; departure error at 400 s.
    pub tfms_eta_err_s: [f64; 2],
    pub tfms_etd_err_s: f64,
    /// Whether the crossing pair swaps tracks.
    pub swap: bool,
}

impl Default for Injected {
    fn default() -> Injected {
        Injected {
            cross_bias_m: 35.0,
            noise_cross_m: 20.0,
            noise_along_m: 30.0,
            tau_s: 1.5,
            tau_ramp: 0.0,
            speed_bias_kt: 6.0,
            hdg_bias_deg: 2.0,
            alt_bias_ft: 200.0,
            interval_s: 12.0,
            stale_s: 61.0,
            coast_from_s: 400.0,
            coast_to_s: 500.0,
            coast_extra_m: 400.0,
            drop_from_s: 700.0,
            drop_to_s: 850.0,
            init_delay_s: 40.0,
            handoff_init_s: -30.0,
            handoff_accept_s: 12.0,
            tbfm_eta_err_s: [10.0, 30.0, 60.0, 120.0],
            tbfm_sta_err_s: -45.0,
            tfms_eta_err_s: [30.0, 90.0],
            tfms_etd_err_s: -120.0,
            swap: false,
        }
    }
}

/// The synthetic world: aircraft, truth legs and the legacy stream.
// A7.2: cold struct, size guard waived
pub struct World {
    pub aircraft: Vec<SynthAc>,
    pub legs: Vec<TruthLeg>,
    pub legacy: Legacy,
    pub injected: Injected,
    /// Callsign of each flight, in flight-index order, for the tests.
    pub flight_names: Vec<&'static str>,
}

/// The aircraft of the world.
pub fn aircraft() -> Vec<SynthAc> {
    let line = |x0, y0, vx, vy, alt0_ft, vrate_fpm| Path::Line {
        x0,
        y0,
        vx,
        vy,
        alt0_ft,
        vrate_fpm,
    };
    vec![
        SynthAc {
            addr: 0xA0_0001,
            callsign: "SYN001",
            path: line(-120_000.0, 0.0, 200.0, 0.0, 35_000.0, 0.0),
            start_s: 0.0,
            end_s: 1200.0,
        },
        SynthAc {
            addr: 0xA0_0002,
            callsign: "SYN002",
            path: Path::Turn {
                x0: -10_000.0,
                y0: 30_000.0,
                speed: 200.0,
                heading0_rad: 0.3,
                omega_rad_s: 3.0_f64.to_radians(),
                alt_ft: 20_000.0,
            },
            start_s: 0.0,
            end_s: 600.0,
        },
        SynthAc {
            addr: 0xA0_0003,
            callsign: "SYN003",
            path: line(0.0, -100_000.0, 0.0, 200.0, 5_000.0, 2_000.0),
            start_s: 0.0,
            end_s: 1000.0,
        },
        // Approach: ends on the KLND threshold at (50 km, 50 km), 600 ft, after 428.6 s.
        SynthAc {
            addr: 0xA0_0004,
            callsign: "SYN004",
            path: line(
                80_000.0,
                50_000.0,
                -70.0,
                0.0,
                600.0 + 25.0 * (30_000.0 / 70.0),
                -1_500.0,
            ),
            start_s: 0.0,
            end_s: 30_000.0 / 70.0,
        },
        // Takeoff from the KDEP threshold at (-50 km, -50 km), 400 ft.
        SynthAc {
            addr: 0xA0_0005,
            callsign: "SYN005",
            path: line(-50_000.0, -50_000.0, 0.0, 80.0, 400.0, 1_500.0),
            start_s: 0.0,
            end_s: 400.0,
        },
        // The crossing pair, at (0, 20 km) at 300 s.
        SynthAc {
            addr: 0xA0_0006,
            callsign: "SYN006",
            path: line(-60_000.0, 20_000.0, 200.0, 0.0, 25_000.0, 0.0),
            start_s: 0.0,
            end_s: 600.0,
        },
        SynthAc {
            addr: 0xA0_0007,
            callsign: "SYN007",
            path: line(0.0, -40_000.0, 0.0, 200.0, 27_000.0, 0.0),
            start_s: 0.0,
            end_s: 600.0,
        },
        // A formation pair sharing a callsign, outside the zone.
        SynthAc {
            addr: 0xA0_0008,
            callsign: "DUPLIC",
            path: line(-50_000.0, -95_000.0, 200.0, 0.0, 20_000.0, 0.0),
            start_s: 0.0,
            end_s: 500.0,
        },
        SynthAc {
            addr: 0xA0_0009,
            callsign: "DUPLIC",
            path: line(-50_000.0, -95_400.0, 200.0, 0.0, 20_000.0, 0.0),
            start_s: 0.0,
            end_s: 500.0,
        },
        // Its ERAM positions are 20 km away.
        SynthAc {
            addr: 0xA0_000A,
            callsign: "GATEFL",
            path: line(-50_000.0, -97_000.0, 200.0, 0.0, 20_000.0, 0.0),
            start_s: 0.0,
            end_s: 500.0,
        },
    ]
}

/// Truth at `t_ms` for the aircraft list.
pub fn truth_at(acs: &[SynthAc], addr: u32, t_ms: i64) -> Result<TruthSample, NoTruth> {
    let Some(i) = acs.iter().position(|a| a.addr == addr) else {
        return Err(NoTruth::UnknownAircraft);
    };
    let Some(a) = acs.get(i) else {
        return Err(NoTruth::UnknownAircraft);
    };
    let s = (t_ms - T0_MS) as f64 / 1000.0;
    if s < a.start_s || s > a.end_s {
        return Err(NoTruth::OutsideLegs);
    }
    let (x, y, vx, vy, alt, vr) = path_state(&a.path, s - a.start_s);
    let (lat, lon) = from_enu(LAT0, LON0, x, y);
    Ok(TruthSample {
        lat,
        lon,
        baro_alt_ft: Some(alt.round() as i32),
        vel_east_mps: vx,
        vel_north_mps: vy,
        vert_rate_fpm: Some(vr.round() as i32),
        sigma95_h_m: 5.0,
        leg: i as u32,
    })
}

struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn uniform(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }
    fn gauss(&mut self) -> f64 {
        let (u, v) = (self.uniform(), self.uniform());
        (-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos()
    }
}

fn plan_id(n: usize) -> [u8; 12] {
    pad(format!("PLAN{n:06}").as_bytes())
}

/// Fix positions of the world, in metres east and north of the origin.
pub fn fix_xy(name: &str) -> Option<(f64, f64)> {
    Some(match name {
        "RTA" => (-120_000.0, 0.0),
        "MFXA" => (0.0, 0.0),
        "RTC" => (120_000.0, 0.0),
        "RTD" => (0.0, -100_000.0),
        "RTE" => (0.0, -20_000.0),
        "RTF" => (60_000.0, -20_000.0),
        "FARX" => (150_000.0, 150_000.0),
        _ => return None,
    })
}

/// The airspace lookups of the world.
pub fn env() -> Env<'static> {
    Env {
        fix: &|n| fix_xy(n).map(|(x, y)| from_enu(LAT0, LON0, x, y)),
        airport: &|n| match n {
            "KLND" => Some(AirportRef {
                elev_ft: 600,
                thresholds: vec![from_enu(LAT0, LON0, 50_000.0, 50_000.0)],
            }),
            "KDEP" => Some(AirportRef {
                elev_ft: 400,
                thresholds: vec![from_enu(LAT0, LON0, -50_000.0, -50_000.0)],
            }),
            _ => None,
        },
        in_zone: &|lat, lon| {
            let (x, y) = enu(LAT0, LON0, lat, lon);
            x.abs() <= ZONE_M && y.abs() <= ZONE_M
        },
        unit_at: &|lat, lon, _| {
            let (x, _) = enu(LAT0, LON0, lat, lon);
            Some(if x < 40_000.0 {
                unit(b"ZAU", b"12")
            } else {
                unit(b"ZMP", b"05")
            })
        },
    }
}

/// A unit and sector in the `oa-fixm` format.
pub fn unit(u: &[u8], s: &[u8]) -> [u8; 8] {
    let mut out = [0u8; 8];
    for (o, b) in out.iter_mut().take(4).zip(u) {
        *o = *b;
    }
    for (o, b) in out.iter_mut().skip(4).zip(s) {
        *o = *b;
    }
    out
}

fn callsign_bytes(s: &str) -> [u8; 8] {
    pad(s.as_bytes())
}

/// Builds the world: truth legs and the legacy stream with the errors injected.
pub fn build_world(inj: &Injected) -> World {
    let acs = aircraft();
    let legs: Vec<TruthLeg> = acs
        .iter()
        .map(|a| TruthLeg {
            addr: a.addr,
            start_ms: T0_MS + (a.start_s * 1000.0).round() as i64,
            end_ms: T0_MS + (a.end_s * 1000.0).round() as i64,
            callsign: Some(callsign_bytes(a.callsign)),
        })
        .collect();
    let mut leg = new_legacy();
    let mut rng = Rng(0x0A7C_A001);
    let mut names: Vec<&'static str> = Vec::new();
    // Which flight each aircraft produces, with its address in SFDPS (None: callsign only) and
    // the aircraft whose truth its positions follow, after the swap time for the crossing pair.
    // (aircraft index, flight address, first sample s, ERAM position follows aircraft index)
    for (k, a) in acs.iter().enumerate() {
        let (first_s, flight_addr): (f64, Option<u32>) = match a.callsign {
            "SYN001" => (200.0 + inj.init_delay_s, Some(a.addr)),
            "SYN003" => (100.0 + inj.init_delay_s, None),
            "SYN002" => (a.start_s, None),
            "SYN005" => (a.start_s + inj.init_delay_s, Some(a.addr)),
            "DUPLIC" | "GATEFL" => (a.start_s, None),
            _ => (a.start_s, Some(a.addr)),
        };
        if a.addr == 0xA0_0009 {
            continue; // shares the DUPLIC flight with 0xA00008
        }
        let Some(f) = flight_ix(&mut leg, 0x1000 + k as u128) else {
            continue;
        };
        if let Some(c) = leg.flights.callsign.get_mut(f as usize) {
            *c = callsign_bytes(a.callsign);
        }
        if let Some(p) = leg.flights.plan_id.get_mut(f as usize) {
            *p = plan_id(k + 1);
        }
        if let Some(addr) = flight_addr {
            set_addr(&mut leg, f, addr);
        }
        names.push(a.callsign);
        let mut s = first_s;
        while s <= a.end_s {
            let dropped = a.callsign == "SYN001" && s > inj.drop_from_s && s < inj.drop_to_s;
            if !dropped {
                emit_sample(&mut leg, &acs, k, f, s, inj, &mut rng);
            }
            s += inj.interval_s;
        }
    }
    // A flight with an address truth does not know.
    if let Some(f) = flight_ix(&mut leg, 0x2000) {
        set_addr(&mut leg, f, 0xBB_BBBB);
        if let Some(c) = leg.flights.callsign.get_mut(f as usize) {
            *c = callsign_bytes("NOTRUE");
        }
        for i in 0..5 {
            push_th(
                &mut leg,
                f,
                T0_MS + i * 12_000,
                NO_TIME,
                419_000_000,
                -879_000_000,
                10_000,
                None,
                false,
            );
        }
        names.push("NOTRUE");
    }
    add_flow_data(&mut leg, &acs, inj);
    World {
        aircraft: acs,
        legs,
        legacy: leg,
        injected: *inj,
        flight_names: names,
    }
}

fn emit_sample(
    leg: &mut Legacy,
    acs: &[SynthAc],
    k: usize,
    f: u32,
    s: f64,
    inj: &Injected,
    rng: &mut Rng,
) {
    let Some(a) = acs.get(k) else { return };
    // Whose truth the position follows: the crossing pair swaps after 360 s when asked.
    let mut follow = a;
    if inj.swap && a.addr == 0xA0_0006 && s >= 360.0 {
        if let Some(b) = acs.iter().find(|b| b.addr == 0xA0_0007) {
            follow = b;
        }
    }
    // The 20 km offset of the gate-failure aircraft.
    let far = if a.callsign == "GATEFL" {
        20_000.0
    } else {
        0.0
    };
    let tau = inj.tau_s + inj.tau_ramp * s;
    let st = s + tau;
    let (x, y, _, _, _, _) = path_state(&follow.path, st - follow.start_s);
    // Velocity and altitude are ERAM's at the stamped time; only the position is shifted. The
    // closed-form path continues past the end of the leg, so the last samples carry the shift too.
    let (_, _, vx, vy, alt, _) = path_state(&follow.path, s - follow.start_s);
    let sp = vx.hypot(vy);
    let (ue, un) = if sp > 0.0 {
        (vx / sp, vy / sp)
    } else {
        (1.0, 0.0)
    };
    let coast = a.callsign == "SYN003" && s >= inj.coast_from_s && s <= inj.coast_to_s;
    let right = inj.cross_bias_m
        + inj.noise_cross_m * rng.gauss()
        + if coast { inj.coast_extra_m } else { 0.0 };
    let along = inj.noise_along_m * rng.gauss();
    let (ex, ey) = (
        x + right * un + along * ue + far,
        y - right * ue + along * un,
    );
    let (lat, lon) = from_enu(LAT0, LON0, ex, ey);
    let spd = (sp / KT_MPS + inj.speed_bias_kt) * 10.0;
    let hdg = (vx.atan2(vy).to_degrees() + inj.hdg_bias_deg).to_radians();
    let vel = (
        (spd * hdg.sin()).round() as i16,
        (spd * hdg.cos()).round() as i16,
    );
    let q_alt = (alt / 100.0).round() * 100.0 + inj.alt_bias_ft;
    let t = T0_MS + (s * 1000.0).round() as i64;
    let recv = t + ((inj.stale_s + 0.0) * 1000.0).round() as i64;
    push_th(
        leg,
        f,
        t,
        recv,
        (lat * 1e7).round() as i32,
        (lon * 1e7).round() as i32,
        q_alt.round() as i32,
        Some(vel),
        coast,
    );
}

fn add_flow_data(leg: &mut Legacy, acs: &[SynthAc], inj: &Injected) {
    let at = |s: f64| T0_MS + (s * 1000.0).round() as i64;
    let mfx = pad(b"MFXA");
    let aid = pad(b"SYN001");
    // A1 crosses the meter fix at 600 s. Five estimates per look-ahead, errors spread by +-2 s.
    let look = [100.0, 400.0, 1500.0, 3100.0];
    for (l, e) in look.iter().zip(inj.tbfm_eta_err_s.iter()) {
        for d in [-2.0, -1.0, 0.0, 1.0, 2.0] {
            push_tbfm(
                leg,
                plan_id(1),
                aid,
                mfx,
                tbfm_kind::ETA_MFX,
                at(600.0 + e + d),
                at(600.0 - l),
            );
        }
    }
    for d in [-2.0, -1.0, 0.0, 1.0, 2.0] {
        push_tbfm(
            leg,
            plan_id(1),
            aid,
            mfx,
            tbfm_kind::STA_SFX,
            at(600.0 + inj.tbfm_sta_err_s + d),
            at(600.0 - 200.0),
        );
    }
    // Excluded cases: a fix the aircraft never nears, an estimate issued after the crossing.
    push_tbfm(
        leg,
        plan_id(1),
        aid,
        pad(b"FARX"),
        tbfm_kind::ETA_MFX,
        at(900.0),
        at(500.0),
    );
    push_tbfm(
        leg,
        plan_id(1),
        aid,
        mfx,
        tbfm_kind::ETA_MFX,
        at(650.0),
        at(650.0),
    );
    // TFMS: A4 lands at 400 s at KLND (plan 4), A5 departs at 0 s from KDEP (plan 5).
    let a4_end = acs
        .iter()
        .find(|a| a.addr == 0xA0_0004)
        .map_or(0.0, |a| a.end_s);
    for (l, e) in [200.0, 600.0].iter().zip(inj.tfms_eta_err_s.iter()) {
        for d in [-2.0, -1.0, 0.0, 1.0, 2.0] {
            push_tfms(
                leg,
                &TfmsIn {
                    plan_id: pad(&plan_id(4)),
                    acid: pad(b"SYN004"),
                    dep: pad(b"KORD"),
                    arr: pad(b"KLND"),
                    kind: tfms_kind::ETA,
                    time_ms: at(a4_end + e + d),
                    issue_ms: at(a4_end - l),
                },
            );
        }
    }
    for d in [-2.0, -1.0, 0.0, 1.0, 2.0] {
        push_tfms(
            leg,
            &TfmsIn {
                plan_id: pad(&plan_id(5)),
                acid: pad(b"SYN005"),
                dep: pad(b"KDEP"),
                arr: pad(b"KORD"),
                kind: tfms_kind::ETD,
                time_ms: at(0.0 + inj.tfms_etd_err_s + d),
                issue_ms: at(-400.0),
            },
        );
    }
    // Handoffs of A1 at the boundary x = 40 km, crossed at 800 s.
    if let Some(f) = flight_ix(leg, 0x1000) {
        let (from, to) = (unit(b"ZAU", b"12"), unit(b"ZMP", b"05"));
        push_handoff(
            leg,
            f,
            at(800.0 + inj.handoff_init_s),
            hev::INITIATION,
            from,
            to,
        );
        push_handoff(
            leg,
            f,
            at(800.0 + inj.handoff_accept_s),
            hev::ACCEPTANCE,
            from,
            to,
        );
        let pts = |names: &[&[u8]]| -> Vec<RoutePointIn> {
            names
                .iter()
                .map(|n| RoutePointIn {
                    fix: pad(n),
                    est_ms: NO_TIME,
                    lat_e7: NO_COORD,
                    lon_e7: NO_COORD,
                })
                .collect()
        };
        push_route(leg, f, at(-100.0), &pts(&[b"RTA", b"MFXA", b"RTC"]));
    }
    // A3 follows a route that turns east at 20 km south, which it does not follow.
    if let Some(f) = flight_ix(leg, 0x1000 + 2) {
        let pts: Vec<RoutePointIn> = [b"RTD", b"RTE", b"RTF"]
            .iter()
            .map(|n| RoutePointIn {
                fix: pad(*n),
                est_ms: NO_TIME,
                lat_e7: NO_COORD,
                lon_e7: NO_COORD,
            })
            .collect();
        push_route(leg, f, at(-100.0), &pts);
    }
}

/// One recovery check: an injected value, what the metrics measured and the tolerance.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq)]
pub struct Check {
    pub metric: &'static str,
    pub what: String,
    pub injected: f64,
    pub recovered: f64,
    pub tolerance: f64,
}

/// Whether a check passed.
pub fn check_ok(c: &Check) -> bool {
    (c.recovered - c.injected).abs() <= c.tolerance
}

fn row<'a>(m: &'a MetricResult, label: &str) -> Option<&'a crate::stats::Stats> {
    m.rows.iter().find(|r| r.label == label).map(|r| &r.stats)
}

fn scalar(m: &MetricResult, k: &str) -> f64 {
    m.scalars
        .iter()
        .find(|(n, _)| n == k)
        .map_or(f64::NAN, |(_, v)| *v)
}

/// A world with no swap, run under `cfg`, and the output.
pub fn run_world(inj: &Injected, cfg: &RunConfig) -> (World, RunOutput) {
    let w = build_world(inj);
    let q = |a: u32, t: i64| truth_at(&w.aircraft, a, t);
    let truth = Truth {
        query: &q,
        legs: &w.legs,
    };
    let e = env();
    let out = run_benchmark(&w.legacy, &truth, &e, cfg);
    (w, out)
}

/// The settings the self-test runs with: time bins short enough for the world's duration.
pub fn self_test_config() -> RunConfig {
    let mut c = RunConfig::default();
    c.offset.bin_ms = 300_000;
    c.offset.min_group_n = 20;
    c
}

/// Runs the self-test on the baseline scenario and lists every recovery check. The swap and the
/// drift scenarios are separate runs (see the tests).
pub fn self_test() -> Vec<Check> {
    let inj = Injected::default();
    let cfg = self_test_config();
    let (_, out) = run_world(&inj, &cfg);
    let mut v = Vec::new();
    let mut add = |metric: &'static str, what: &str, injected: f64, recovered: f64, tol: f64| {
        v.push(Check {
            metric,
            what: what.to_string(),
            injected,
            recovered,
            tolerance: tol,
        });
    };
    let nc = "coast=not coasting";
    if let Some(m) = find(&out, "T1") {
        if let Some(s) = row(m, nc) {
            add(
                "T1",
                "cross-track bias (mean, not coasting)",
                inj.cross_bias_m,
                s.mean,
                4.0,
            );
            let exp = (inj.cross_bias_m.powi(2) + inj.noise_cross_m.powi(2)).sqrt();
            add("T1", "cross-track rms (not coasting)", exp, s.rms, 4.0);
        }
        if let Some(s) = row(m, "coast=coasting") {
            add(
                "T1",
                "cross-track bias while coasting (mean)",
                inj.cross_bias_m + inj.coast_extra_m,
                s.mean,
                15.0,
            );
        }
    }
    if let Some(m) = find(&out, "T2-offset") {
        add(
            "T2",
            "fitted time offset (s)",
            inj.tau_s,
            scalar(m, "fitted_offset_s"),
            0.1,
        );
        if let Some(s) = row(m, "all") {
            add(
                "T2",
                "offset-corrected along-track rms (m)",
                inj.noise_along_m,
                s.rms,
                5.0,
            );
            add(
                "T2",
                "offset-corrected along-track mean (m)",
                0.0,
                s.mean,
                5.0,
            );
        }
    }
    if let Some(m) = find(&out, "T3-speed") {
        if let Some(s) = row(m, "all") {
            add("T3", "speed bias (kt)", inj.speed_bias_kt, s.mean, 1.0);
        }
    }
    if let Some(m) = find(&out, "T3-heading") {
        if let Some(s) = row(m, "all") {
            add("T3", "heading bias (deg)", inj.hdg_bias_deg, s.mean, 0.4);
        }
    }
    if let Some(m) = find(&out, "T4") {
        if let Some(s) = row(m, "all") {
            add("T4", "altitude bias (ft)", inj.alt_bias_ft, s.mean, 8.0);
        }
    }
    if let Some(m) = find(&out, "T5") {
        if let Some(s) = row(m, "all") {
            add(
                "T5",
                "publication interval (median, s)",
                inj.interval_s,
                s.median,
                0.01,
            );
        }
    }
    if let Some(m) = find(&out, "T6") {
        if let Some(s) = row(m, "all") {
            add("T6", "staleness (s)", inj.stale_s, s.mean, 0.01);
        }
    }
    if let Some(m) = find(&out, "T8") {
        if let Some(s) = row(m, "all") {
            add("T8", "initiation delay (s)", inj.init_delay_s, s.mean, 5.1);
        }
    }
    if let Some(m) = find(&out, "T9") {
        add(
            "T9",
            "association errors without a swap",
            0.0,
            scalar(m, "association_errors"),
            0.0,
        );
    }
    for (i, bin) in ["0 to 5 min", "5 to 10 min", "25 to 30 min", "50 to 55 min"]
        .iter()
        .enumerate()
    {
        if let Some(m) = find(&out, "F1") {
            if let Some(s) = row(m, &format!("lookahead={bin}")) {
                add(
                    "F1",
                    &format!("TBFM ETA error, look-ahead {bin} (s)"),
                    inj.tbfm_eta_err_s.get(i).copied().unwrap_or(0.0),
                    s.mean,
                    0.5,
                );
            }
        }
    }
    if let Some(m) = find(&out, "F2") {
        if let Some(s) = row(m, "lookahead=0 to 5 min") {
            add(
                "F2",
                "TBFM scheduled time error (s)",
                inj.tbfm_sta_err_s,
                s.mean,
                0.5,
            );
        }
    }
    if let Some(m) = find(&out, "F3-arrival") {
        if let Some(s) = row(m, "lookahead=0 to 5 min") {
            add(
                "F3",
                "TFMS arrival error, 200 s ahead (s)",
                inj.tfms_eta_err_s[0],
                s.mean,
                1.0,
            );
        }
        if let Some(s) = row(m, "lookahead=10 to 15 min") {
            add(
                "F3",
                "TFMS arrival error, 600 s ahead (s)",
                inj.tfms_eta_err_s[1],
                s.mean,
                1.0,
            );
        }
    }
    if let Some(m) = find(&out, "F3-departure") {
        if let Some(s) = row(m, "lookahead=5 to 10 min") {
            add(
                "F3",
                "TFMS departure error (s)",
                inj.tfms_etd_err_s,
                s.mean,
                1.0,
            );
        }
    }
    if let Some(m) = find(&out, "R1") {
        add(
            "R1",
            "largest per-flight share of time beyond 2 NM",
            0.662,
            scalar(m, "max_flight_share"),
            0.01,
        );
    }
    if let Some(m) = find(&out, "H1") {
        if let Some(s) = row(m, "event=initiation") {
            add(
                "H1",
                "handoff initiation time (s)",
                inj.handoff_init_s,
                s.mean,
                1.5,
            );
        }
        if let Some(s) = row(m, "event=acceptance") {
            add(
                "H1",
                "handoff acceptance time (s)",
                inj.handoff_accept_s,
                s.mean,
                1.5,
            );
        }
    }
    v
}
