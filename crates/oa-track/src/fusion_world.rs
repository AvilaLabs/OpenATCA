//! The worlds the fusion study runs in: a base set of smooth synthetic trajectories, crossing and
//! parallel pairs at stated separations, and the shapes of a real clip's `oa-truth`
//! trajectories. Shell-side study code (it reads a file for the clip); every function is a
//! function of its arguments.
//!
//! The synthetic worlds are planned in the plane about the radar (`synth_truth.rs`), so a plan
//! is in metres from the radar. Every world's truth is what the simulator treats as exact.

use oa_bench::adsb_truth::BuiltTruth;
use oa_sensorsim::prng::Rng;
use oa_truth::{build_truth, TruthConfig, DEFAULT_CONFIG};

use crate::fusion_score::{built_of, site_def, Kind, EPOCH_NS};
use crate::synth_truth::{build_synthetic_truth, duration_s, state_at, Seg, Traj};

/// Turn rates of the base set, degrees per second (the three classes of the study).
pub const TURN_RATES_DPS: [f64; 3] = [1.5, 3.0, 6.0];
/// Climb and descent rate of the base set, feet per minute.
pub const CLIMB_FPM: f64 = 1_500.0;
/// Horizontal separation of the "3 NM" pairs, metres.
pub const SEP_3NM_M: f64 = 3.0 * 1852.0;
/// Vertical separation of the "1,000 ft" pairs, feet.
pub const SEP_1000FT: f64 = 1_000.0;
/// The base set keeps every pair of aircraft at least this far apart horizontally (5 NM) unless
/// they are 2,000 ft apart vertically, so the base set has no close encounters of its own; the
/// pair worlds are where those are tested.
pub const BASE_MIN_SEP_M: f64 = 5.0 * 1852.0;

/// Duration of a base world, seconds.
pub fn base_duration_s(kind: Kind) -> f64 {
    match kind {
        Kind::Terminal => 600.0,
        Kind::EnRoute => 900.0,
    }
}

fn speed_range(kind: Kind) -> (f64, f64) {
    match kind {
        Kind::Terminal => (80.0, 150.0),
        Kind::EnRoute => (180.0, 250.0),
    }
}

fn alt_range_ft(kind: Kind) -> (f64, f64) {
    match kind {
        Kind::Terminal => (4_000.0, 18_000.0),
        Kind::EnRoute => (24_000.0, 40_000.0),
    }
}

fn range_km(kind: Kind) -> (f64, f64) {
    match kind {
        Kind::Terminal => (12.0, 85.0),
        Kind::EnRoute => (30.0, 210.0),
    }
}

/// Inside coverage and clear of the cone near the antenna at every 10 s of the path.
fn path_ok(t: &Traj, kind: Kind) -> bool {
    let cov = site_def(kind).coverage_m * 0.92;
    let dur = duration_s(t);
    let mut s = 0.0;
    while s <= dur {
        let st = state_at(t, s);
        let r = st.e.hypot(st.n);
        if !(8_000.0..=cov).contains(&r) {
            return false;
        }
        s += 10.0;
    }
    true
}

/// Closest horizontal approach of two paths at times both exist, sampled every 2 s, with the
/// smallest altitude difference at that sample (feet).
fn too_close(a: &Traj, b: &Traj) -> bool {
    let end = duration_s(a).min(duration_s(b));
    let mut s = 0.0;
    while s <= end {
        let (x, y) = (state_at(a, s), state_at(b, s));
        if (x.e - y.e).hypot(x.n - y.n) < BASE_MIN_SEP_M && (x.alt_ft - y.alt_ft).abs() < 2_000.0 {
            return true;
        }
        s += 2.0;
    }
    false
}

/// The base set of a kind: 20 straight, 12 turning (each rate both ways, two of each) and 12
/// climbing or descending aircraft, none closer than 5 NM at one altitude. Addresses
/// `0xA10000 + index`; even index is the ADS-B-equipped half of mixed runs. Deterministic in
/// `seed`.
pub fn base_trajs(kind: Kind, seed: u64) -> Vec<Traj> {
    let mut rng = Rng::from_seed(seed.wrapping_mul(0x9E37_79B9).wrapping_add(77));
    let dur = base_duration_s(kind);
    let (v_lo, v_hi) = speed_range(kind);
    let (a_lo, a_hi) = alt_range_ft(kind);
    let (r_lo, r_hi) = range_km(kind);
    let mut out: Vec<Traj> = Vec::new();
    // (kind of aircraft, turn rate, turn direction, vertical rate)
    let mut plan: Vec<(u8, f64, f64, f64)> = Vec::new();
    for _ in 0..20 {
        plan.push((0, 0.0, 0.0, 0.0));
    }
    for rate in TURN_RATES_DPS {
        for dir in [-1.0, 1.0] {
            for _ in 0..2 {
                plan.push((1, rate, dir, 0.0));
            }
        }
    }
    for k in 0..12 {
        plan.push((2, 0.0, 0.0, if k % 2 == 0 { CLIMB_FPM } else { -CLIMB_FPM }));
    }
    for (class, rate, dir, vr) in plan {
        let mut placed = false;
        for _ in 0..400 {
            let bearing = rng.unit() * 360.0;
            let r = (r_lo + (r_hi - r_lo) * rng.unit()) * 1000.0;
            let speed = v_lo + (v_hi - v_lo) * rng.unit();
            let (st, ct) = bearing.to_radians().sin_cos();
            let mut alt = a_lo + (a_hi - a_lo) * rng.unit();
            let segs = match class {
                0 => vec![Seg {
                    dur_s: dur,
                    turn_dps: 0.0,
                    vrate_fpm: 0.0,
                }],
                1 => vec![
                    Seg {
                        dur_s: 150.0,
                        turn_dps: 0.0,
                        vrate_fpm: 0.0,
                    },
                    Seg {
                        dur_s: 90.0 / rate,
                        turn_dps: dir * rate,
                        vrate_fpm: 0.0,
                    },
                    Seg {
                        dur_s: dur - 150.0 - 90.0 / rate,
                        turn_dps: 0.0,
                        vrate_fpm: 0.0,
                    },
                ],
                _ => {
                    // Start low for a climb and high for a descent so the profile fits the band.
                    alt = if vr > 0.0 { a_lo } else { a_hi } + 1_000.0 * rng.unit();
                    vec![
                        Seg {
                            dur_s: 100.0,
                            turn_dps: 0.0,
                            vrate_fpm: 0.0,
                        },
                        Seg {
                            dur_s: 200.0,
                            turn_dps: 0.0,
                            vrate_fpm: vr,
                        },
                        Seg {
                            dur_s: dur - 300.0,
                            turn_dps: 0.0,
                            vrate_fpm: 0.0,
                        },
                    ]
                }
            };
            let t = Traj {
                addr: 0xA1_0000 + out.len() as u32,
                t0_s: 0.0,
                x0: r * st,
                y0: r * ct,
                heading_deg: rng.unit() * 360.0,
                speed_mps: speed,
                alt_ft: alt,
                segs,
            };
            if path_ok(&t, kind) && !out.iter().any(|o| too_close(o, &t)) {
                out.push(t);
                placed = true;
                break;
            }
        }
        if !placed {
            // Cannot happen with these densities; a shorter set is still a valid world.
            continue;
        }
    }
    out
}

/// The base world's truth.
pub fn base_world(kind: Kind, seed: u64) -> Result<BuiltTruth, String> {
    let d = site_def(kind);
    let truth = build_synthetic_truth(d.lat_deg, d.lon_deg, EPOCH_NS, &base_trajs(kind, seed))
        .map_err(|e| e.to_string())?;
    Ok(built_of(truth))
}

/// The shape of a pair of aircraft.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PairShape {
    /// Straight paths crossing at this angle (degrees between the two headings).
    Cross(f64),
    /// Same heading, side by side.
    ParallelSame,
    /// Opposite headings, passing.
    ParallelOpposite,
}

/// A pair geometry: shape, closest horizontal approach and vertical separation. Plain data
/// (A1); A7.2: cold struct.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PairGeom {
    pub name: &'static str,
    pub shape: PairShape,
    pub sep_m: f64,
    pub dz_ft: f64,
}

/// The study's pair geometries: crossing at 90 and 30 degrees and parallel (same and opposite
/// direction) at 3 NM and 1,000 ft separation, then two stress cases below the stated
/// separations (not claimed to be resolved).
pub const PAIR_GEOMS: [PairGeom; 12] = [
    PairGeom {
        name: "cross 90deg, 3 NM, same alt",
        shape: PairShape::Cross(90.0),
        sep_m: SEP_3NM_M,
        dz_ft: 0.0,
    },
    PairGeom {
        name: "cross 30deg, 3 NM, same alt",
        shape: PairShape::Cross(30.0),
        sep_m: SEP_3NM_M,
        dz_ft: 0.0,
    },
    PairGeom {
        name: "parallel same dir, 3 NM",
        shape: PairShape::ParallelSame,
        sep_m: SEP_3NM_M,
        dz_ft: 0.0,
    },
    PairGeom {
        name: "parallel opposite, 3 NM",
        shape: PairShape::ParallelOpposite,
        sep_m: SEP_3NM_M,
        dz_ft: 0.0,
    },
    PairGeom {
        name: "cross 90deg, 0 NM, 1,000 ft",
        shape: PairShape::Cross(90.0),
        sep_m: 0.0,
        dz_ft: SEP_1000FT,
    },
    PairGeom {
        name: "cross 30deg, 0 NM, 1,000 ft",
        shape: PairShape::Cross(30.0),
        sep_m: 0.0,
        dz_ft: SEP_1000FT,
    },
    PairGeom {
        name: "same track, 0 NM, 1,000 ft",
        shape: PairShape::ParallelSame,
        sep_m: 0.0,
        dz_ft: SEP_1000FT,
    },
    PairGeom {
        name: "head-on, 0 NM, 1,000 ft",
        shape: PairShape::ParallelOpposite,
        sep_m: 0.0,
        dz_ft: SEP_1000FT,
    },
    PairGeom {
        name: "STRESS cross 90deg, 1 NM, same alt",
        shape: PairShape::Cross(90.0),
        sep_m: 1852.0,
        dz_ft: 0.0,
    },
    PairGeom {
        name: "STRESS cross 30deg, 1 NM, same alt",
        shape: PairShape::Cross(30.0),
        sep_m: 1852.0,
        dz_ft: 0.0,
    },
    PairGeom {
        name: "STRESS cross 90deg, 0.5 NM, same alt",
        shape: PairShape::Cross(90.0),
        sep_m: 926.0,
        dz_ft: 0.0,
    },
    PairGeom {
        name: "STRESS same track, 0.5 NM along, same alt",
        shape: PairShape::ParallelSame,
        sep_m: 926.0,
        dz_ft: 0.0,
    },
];

/// Pair duration and speed by kind.
fn pair_params(kind: Kind) -> (f64, f64, f64, f64) {
    // (duration s, speed m/s, altitude ft, distance of the crossing point from the radar, m)
    match kind {
        Kind::Terminal => (400.0, 120.0, 10_000.0, 40_000.0),
        Kind::EnRoute => (600.0, 220.0, 30_000.0, 90_000.0),
    }
}

fn min_sep(a: &Traj, b: &Traj) -> f64 {
    let end = duration_s(a).min(duration_s(b));
    let mut s = 0.0;
    let mut best = f64::INFINITY;
    while s <= end {
        let (x, y) = (state_at(a, s), state_at(b, s));
        best = best.min((x.e - y.e).hypot(x.n - y.n));
        s += 0.25;
    }
    best
}

/// The two aircraft (addresses `0xA00001` and `0xA00002`) of a pair geometry. The "same track,
/// along" stress case (`ParallelSame` with a separation and no vertical separation) puts the
/// second aircraft behind the first by that distance.
pub fn pair_trajs(kind: Kind, g: &PairGeom) -> Vec<Traj> {
    let (dur, v, alt, rc) = pair_params(kind);
    let c = (
        rc * 45.0_f64.to_radians().sin(),
        rc * 45.0_f64.to_radians().cos(),
    );
    let one_seg = |d: f64| {
        vec![Seg {
            dur_s: d,
            turn_dps: 0.0,
            vrate_fpm: 0.0,
        }]
    };
    let h_a = 270.0_f64;
    let t_mid = dur / 2.0;
    // Start of a straight path through `p` at time `t_mid` with heading `h`.
    let start = |p: (f64, f64), h: f64, shift_s: f64| {
        let (s, co) = h.to_radians().sin_cos();
        let back = t_mid + shift_s;
        (p.0 - v * s * back, p.1 - v * co * back)
    };
    let a_start = start(c, h_a, 0.0);
    let a = Traj {
        addr: 0xA0_0001,
        t0_s: 0.0,
        x0: a_start.0,
        y0: a_start.1,
        heading_deg: h_a,
        speed_mps: v,
        alt_ft: alt,
        segs: one_seg(dur),
    };
    let b = |h: f64, p: (f64, f64), shift_s: f64| {
        let st = start(p, h, shift_s);
        Traj {
            addr: 0xA0_0002,
            t0_s: 0.0,
            x0: st.0,
            y0: st.1,
            heading_deg: h,
            speed_mps: v,
            alt_ft: alt + g.dz_ft,
            segs: one_seg(dur),
        }
    };
    let b_traj = match g.shape {
        PairShape::Cross(angle) => {
            let h_b = (h_a + angle).rem_euclid(360.0);
            if g.sep_m <= 0.0 {
                b(h_b, c, 0.0)
            } else {
                // B reaches the crossing point `tau` seconds after A; bisect `tau` so that the
                // closest approach is `sep_m`.
                let (mut lo, mut hi) = (0.0, 6.0 * g.sep_m / v + 2.0);
                for _ in 0..50 {
                    let mid = 0.5 * (lo + hi);
                    if min_sep(&a, &b(h_b, c, mid)) < g.sep_m {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                b(h_b, c, 0.5 * (lo + hi))
            }
        }
        PairShape::ParallelSame => {
            if g.dz_ft == 0.0 && g.sep_m > 0.0 && g.sep_m < SEP_3NM_M {
                // Behind A along the track by `sep_m`.
                b(h_a, c, g.sep_m / v)
            } else {
                // Beside A, to its right (north of a westbound track), by `sep_m`.
                b(h_a, (c.0, c.1 + g.sep_m), 0.0)
            }
        }
        PairShape::ParallelOpposite => b(90.0, (c.0, c.1 + g.sep_m), 0.0),
    };
    vec![a, b_traj]
}

/// The truth of one pair geometry.
pub fn pair_world(kind: Kind, g: &PairGeom) -> Result<BuiltTruth, String> {
    let d = site_def(kind);
    let truth = build_synthetic_truth(d.lat_deg, d.lon_deg, EPOCH_NS, &pair_trajs(kind, g))
        .map_err(|e| e.to_string())?;
    Ok(built_of(truth))
}

/// The truth built from a clip: its `oa-truth` trajectories, used only as shapes. The
/// simulator and the scorer both read this truth, so it is exact for them.
pub fn clip_world(path: &str) -> Result<BuiltTruth, String> {
    let (reports, bad) = oa_sensorsim::clip::read_clip(path)?;
    let cfg = TruthConfig {
        loo: false,
        ..DEFAULT_CONFIG
    };
    let (truth, st) = build_truth(&reports, &cfg).map_err(|e| e.to_string())?;
    let _ = (bad, st);
    Ok(built_of(truth))
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

    // REQ: OA-TRK-057
    #[test]
    fn the_base_set_has_the_stated_mix_and_no_close_encounters() {
        for kind in [Kind::Terminal, Kind::EnRoute] {
            let t = base_trajs(kind, 1);
            assert_eq!(t.len(), 44, "{kind:?}: {}", t.len());
            let turning = t
                .iter()
                .filter(|x| x.segs.len() == 3 && x.segs[1].turn_dps != 0.0)
                .count();
            assert_eq!(turning, 12);
            for i in 0..t.len() {
                assert!(path_ok(&t[i], kind));
                for j in (i + 1)..t.len() {
                    assert!(!too_close(&t[i], &t[j]), "{kind:?} {i} {j}");
                }
            }
            // Deterministic in the seed, different across seeds.
            assert_eq!(base_trajs(kind, 1), t);
            assert_ne!(base_trajs(kind, 2), t);
        }
    }

    // REQ: OA-TRK-058
    #[test]
    fn pair_geometries_have_their_stated_closest_approach() {
        for kind in [Kind::Terminal, Kind::EnRoute] {
            for g in &PAIR_GEOMS {
                let t = pair_trajs(kind, g);
                let sep = min_sep(&t[0], &t[1]);
                let dz = (state_at(&t[0], 100.0).alt_ft - state_at(&t[1], 100.0).alt_ft).abs();
                assert!(
                    (sep - g.sep_m).abs() < 15.0,
                    "{} {kind:?}: closest approach {sep} m, wanted {}",
                    g.name,
                    g.sep_m
                );
                assert!((dz - g.dz_ft).abs() < 1.0, "{}", g.name);
                assert!(
                    path_ok_loose(&t[0], kind) && path_ok_loose(&t[1], kind),
                    "{} {kind:?}",
                    g.name
                );
            }
        }
    }

    fn path_ok_loose(t: &Traj, kind: Kind) -> bool {
        // Pair paths must stay in coverage; they pass nearer the antenna than the base set
        // would allow, so the cone limit is the only relaxation (3 km).
        let cov = site_def(kind).coverage_m * 0.95;
        let mut s = 0.0;
        while s <= duration_s(t) {
            let st = state_at(t, s);
            let r = st.e.hypot(st.n);
            if !(3_000.0..=cov).contains(&r) {
                return false;
            }
            s += 10.0;
        }
        true
    }

    // REQ: OA-TRK-059
    #[test]
    fn synthetic_worlds_build_into_exact_truths() {
        let b = base_world(Kind::Terminal, 1).unwrap();
        assert_eq!(b.truth.ac_addr.len(), 44);
        assert_eq!(b.legs.len(), 44);
        let p = pair_world(Kind::EnRoute, &PAIR_GEOMS[0]).unwrap();
        assert_eq!(p.truth.ac_addr, vec![0xA00001, 0xA00002]);
    }
}
