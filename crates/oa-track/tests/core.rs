//! Tracker tests on synthetic worlds with known truth.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use common::*;
use oa_geo::aeqd::project;
use oa_sha256::{to_hex, Sha256};
use oa_track::{
    measurement_check, output_hash_into, output_new, tracker_advance, tracker_buffers,
    tracker_bytes, tracker_flush, tracker_live, tracker_log, tracker_new, tracker_stats,
    tracker_update, Measurement, TrackOutput, TrackerConfig, DEFAULT_CONFIG, END_DROPPED,
    END_TENTATIVE, M_HAS_VEL, NONE_I32, NO_ADDR, O_COAST, O_HAS_ALT, O_TICK,
};

/// Error summary of a scenario in a time window.
#[derive(Debug, Clone, Copy)]
struct Errs {
    pos: f64,
    speed: f64,
    heading: f64,
    n: usize,
}

/// Runs one aircraft with measurement interval `dt` for `dur` seconds and measures the update
/// rows with times in `[w0, w1]` against truth.
fn scenario(man: &Manoeuvre, dt: f64, dur: f64, mode: u8, seed: u64, w: (f64, f64)) -> Errs {
    let f = frame0();
    let mut rng = Rng(seed);
    let mut ms = Vec::new();
    let mut t = 0.0;
    while t <= dur {
        let tr = man.at_lagged_velocity(t, 0.0, 0.0);
        ms.push(measure(
            &f,
            0xABC123,
            t,
            &tr,
            SIGMA_POS,
            Some(SIGMA_VEL),
            false,
            &mut rng,
        ));
        t += dt;
    }
    let cfg = TrackerConfig {
        h_mode: mode,
        expected_interval_ns: (dt * 1.0e9) as i64,
        init_interval_ns: (dt * 1.0e9) as i64,
        drop_after_ns: 600_000_000_000,
        ..big_config()
    };
    let (_t, out) = run(&cfg, &ms, dur);
    let (mut p, mut s, mut h) = (Vec::new(), Vec::new(), Vec::new());
    for i in 0..out.t_ns.len() {
        // The one-second output, so a filter that loses the aircraft is scored for it.
        if out.flags[i] & O_TICK == 0 {
            continue;
        }
        let ts = out.t_ns[i] as f64 / 1e9;
        if ts < w.0 || ts > w.1 {
            continue;
        }
        let tr = man.at(ts, 0.0, 0.0);
        p.push(pos_err(&f, &out, i, &tr));
        let (ds, dh) = vel_err(&f, &out, i, &tr);
        s.push(ds);
        h.push(dh);
    }
    Errs {
        pos: rms(&p),
        speed: rms(&s),
        heading: rms(&h),
        n: p.len(),
    }
}

// REQ: OA-TRK-002
#[test]
fn straight_flight_is_tracked_better_than_the_raw_reports() {
    let man = Manoeuvre::straight(230.0, 60.0);
    let e = scenario(&man, 1.0, 400.0, 0, 7, (30.0, 400.0));
    println!(
        "straight 1 Hz: {e:?} (raw per-axis sigma {SIGMA_POS:.1} m, radial RMS {:.1} m)",
        SIGMA_POS * 2f64.sqrt()
    );
    assert!(e.n > 300);
    assert!(e.pos < 7.0, "position RMS {}", e.pos);
    assert!(e.speed < 1.0, "speed RMS {}", e.speed);
    assert!(e.heading < 0.2, "heading RMS {}", e.heading);
    // 17 s intervals (the thinned adsb.lol input).
    let e17 = scenario(&man, 17.0, 3400.0, 0, 7, (300.0, 3400.0));
    println!("straight 17 s: {e17:?}");
    assert!(e17.pos < 25.0, "position RMS {}", e17.pos);
}

// REQ: OA-TRK-003
#[test]
fn position_noise_comes_from_nacp_and_innovations_are_consistent() {
    // The filter is told the NACp 9 sigma; the mean normalised innovation squared of a
    // two-dimensional position is 2 when the noise model is honest.
    let man = Manoeuvre::straight(230.0, 10.0);
    let f = frame0();
    let mut rng = Rng(11);
    let ms: Vec<Measurement> = (0..600)
        .map(|k| {
            let t = f64::from(k);
            measure(
                &f,
                0x111111,
                t,
                &man.at(t, 0.0, 0.0),
                SIGMA_POS,
                Some(SIGMA_VEL),
                false,
                &mut rng,
            )
        })
        .collect();
    let (t, _) = run(&big_config(), &ms, 600.0);
    let s = tracker_stats(&t);
    let nis = s.nis_sum / s.nis_n as f64;
    println!("mean NIS {nis:.3} over {} updates", s.nis_n);
    assert!((1.7..=2.4).contains(&nis), "mean NIS {nis}");
    // A tighter reported noise than the truth makes the innovations too large: the check has
    // power to see a wrong model.
    let mut rng = Rng(11);
    let bad: Vec<Measurement> = (0..600)
        .map(|k| {
            let t = f64::from(k);
            let mut m = measure(
                &f,
                0x111111,
                t,
                &man.at(t, 0.0, 0.0),
                SIGMA_POS,
                Some(SIGMA_VEL),
                false,
                &mut rng,
            );
            let v = ((SIGMA_POS / 3.0) * (SIGMA_POS / 3.0)) as f32;
            m.pos_cov_m2 = [v, 0.0, v];
            m
        })
        .collect();
    let (t2, _) = run(&big_config(), &bad, 600.0);
    let s2 = tracker_stats(&t2);
    assert!(s2.nis_sum / s2.nis_n as f64 > 3.0);
}

// REQ: OA-TRK-004
#[test]
fn constant_rate_turns_are_followed_and_the_imm_beats_one_straight_filter() {
    // Scored on the one-second output over the last 54 s of a 60 s turn, input at 1 s and 5 s.
    // CV1 is the IMM's own straight model alone (the same process noise); CV2 and CV3 are single
    // filters with larger process noise (0.5 and 5 m^2/s^3), the best a single filter can do.
    let mut worst_vs_cv1 = (f64::MAX, f64::MAX);
    let mut worst_vs_best_pos = f64::MAX;
    for (rate, dt) in [(1.5, 1.0), (3.0, 1.0), (6.0, 1.0), (3.0, 5.0)] {
        let man = Manoeuvre::turning(230.0, 20.0, 120.0, 180.0, rate);
        let imm = scenario(&man, dt, 300.0, 0, 5, (126.0, 180.0));
        let mut cvs = Vec::new();
        for mode in 1..=3 {
            cvs.push(scenario(&man, dt, 300.0, mode, 5, (126.0, 180.0)));
        }
        let best_pos = cvs.iter().map(|c| c.pos).fold(f64::MAX, f64::min);
        let g = |a: f64, b: f64| 100.0 * (1.0 - a / b);
        println!(
            "turn {rate} deg/s, {dt} s input: IMM pos {:.2} m, speed {:.2} m/s, heading {:.2} deg | CV1 pos {:.1}, speed {:.2} | CV2 pos {:.1}, speed {:.2} | CV3 pos {:.1}, speed {:.2} | IMM better than CV1 by {:.0} % (position), {:.0} % (speed); than the best single CV by {:.0} % (position)",
            imm.pos, imm.speed, imm.heading, cvs[0].pos, cvs[0].speed, cvs[1].pos, cvs[1].speed, cvs[2].pos, cvs[2].speed,
            g(imm.pos, cvs[0].pos), g(imm.speed, cvs[0].speed), g(imm.pos, best_pos)
        );
        // At 5 s the single filters lose the aircraft in the turn (position in kilometres) and
        // their speed is only the last velocity report, so speed is compared at 1 s only.
        worst_vs_cv1.0 = worst_vs_cv1.0.min(g(imm.pos, cvs[0].pos));
        if dt == 1.0 {
            worst_vs_cv1.1 = worst_vs_cv1.1.min(g(imm.speed, cvs[0].speed));
        }
        worst_vs_best_pos = worst_vs_best_pos.min(g(imm.pos, best_pos));
        assert!(
            imm.pos < 15.0 && imm.speed < 2.0 && imm.heading < 3.0,
            "{imm:?}"
        );
    }
    // The speed is within a quarter of the single filter's (its reports carry no lag error in
    // speed, and the IMM's velocity update spends some of them on the turn rate, OA-TRK-395; the
    // two were 1 % apart before and are noise-level apart, 17 % at the slowest turn, now).
    assert!(
        worst_vs_cv1.0 > 0.0 && worst_vs_cv1.1 > -25.0,
        "{worst_vs_cv1:?}"
    );
    assert!(worst_vs_best_pos > 0.0, "{worst_vs_best_pos}");
}

// REQ: OA-TRK-005
#[test]
fn the_imm_costs_nothing_on_straight_flight_and_turn_probability_follows_the_turn() {
    let man = Manoeuvre::turning(230.0, 20.0, 150.0, 210.0, 3.0);
    let imm = scenario(&man, 1.0, 400.0, 0, 3, (40.0, 140.0));
    let cv = scenario(&man, 1.0, 400.0, 1, 3, (40.0, 140.0));
    println!("straight part: IMM {imm:?} CV1 {cv:?}");
    assert!(imm.pos <= cv.pos * 1.15 + 0.2);
    // Model probabilities in the middle of the turn and of the straight leg.
    let f = frame0();
    let mut rng = Rng(3);
    let ms: Vec<Measurement> = (0..400)
        .map(|k| {
            let t = f64::from(k);
            measure(
                &f,
                0x222222,
                t,
                &man.at_lagged_velocity(t, 0.0, 0.0),
                SIGMA_POS,
                Some(SIGMA_VEL),
                false,
                &mut rng,
            )
        })
        .collect();
    let (_, out) = run(&big_config(), &ms, 400.0);
    let at = |s: i64| {
        let i = (0..out.t_ns.len())
            .find(|&i| out.t_ns[i] == s * 1_000_000_000 && out.flags[i] & O_TICK == 0)
            .unwrap();
        (out.mu_h[i], out.turn_rate_rps[i])
    };
    let (mu_turn, w) = at(190);
    let (mu_line, _) = at(100);
    println!(
        "turn: p(turn) {:.3} rate {:.2} deg/s; straight: p(turn) {:.3}",
        mu_turn[1],
        f64::from(w).to_degrees(),
        mu_line[1]
    );
    assert!(mu_turn[1] > 0.8 && mu_line[1] < 0.1);
    assert!((f64::from(w).to_degrees() - 3.0).abs() < 0.5);
}

// REQ: OA-TRK-006
#[test]
fn accelerating_flight_is_tracked() {
    let man = Manoeuvre {
        accel: 1.0,
        ..Manoeuvre::straight(120.0, 200.0)
    };
    let e = scenario(&man, 1.0, 120.0, 0, 9, (20.0, 120.0));
    println!("acceleration 1 m/s^2: {e:?}");
    assert!(e.pos < 10.0 && e.speed < 2.0, "{e:?}");
}

// REQ: OA-TRK-007
#[test]
fn climbs_descents_and_level_flight_are_tracked_in_altitude_with_quantisation() {
    let f = frame0();
    let man = Manoeuvre::straight(200.0, 90.0);
    // Altitude profile: level 10,012 ft for 120 s, climb 2,000 fpm for 180 s, level, descend
    // 1,500 fpm for 120 s.
    let profile = |t: f64| -> (f64, f64) {
        if t < 120.0 {
            (10_012.0, 0.0)
        } else if t < 300.0 {
            (10_012.0 + 2000.0 * (t - 120.0) / 60.0, 2000.0)
        } else if t < 420.0 {
            (16_012.0, 0.0)
        } else if t < 540.0 {
            (16_012.0 - 1500.0 * (t - 420.0) / 60.0, -1500.0)
        } else {
            (13_012.0, 0.0)
        }
    };
    let mut rng = Rng(21);
    let mut ms = Vec::new();
    for k in 0..650 {
        let t = f64::from(k);
        let mut tr = man.at(t, 0.0, 0.0);
        (tr.alt_ft, tr.vrate_fpm) = profile(t);
        ms.push(measure(
            &f,
            0x333333,
            t,
            &tr,
            SIGMA_POS,
            Some(SIGMA_VEL),
            true,
            &mut rng,
        ));
    }
    let (_, out) = run(&big_config(), &ms, 650.0);
    let (mut ea, mut er) = (Vec::new(), Vec::new());
    let mut p_climb_mid = 0.0;
    let mut p_level_mid = 0.0;
    let mut p_desc_mid = 0.0;
    for i in 0..out.t_ns.len() {
        if out.flags[i] & O_TICK != 0 {
            continue;
        }
        let t = out.t_ns[i] as f64 / 1e9;
        let (a, r) = profile(t);
        // Away from the model changes.
        let near_change = [120.0, 300.0, 420.0, 540.0]
            .iter()
            .any(|c| (t - c).abs() < 15.0);
        if t > 30.0 && !near_change {
            ea.push(f64::from(out.alt_ft[i]) - a);
            er.push(f64::from(out.vrate_fpm[i]) - r);
        }
        if (t - 210.0).abs() < 0.5 {
            p_climb_mid = f64::from(out.mu_v[i][1]);
        }
        if (t - 70.0).abs() < 0.5 {
            p_level_mid = f64::from(out.mu_v[i][0]);
        }
        if (t - 480.0).abs() < 0.5 {
            p_desc_mid = f64::from(out.mu_v[i][1]);
        }
    }
    println!(
        "altitude RMS {:.1} ft, rate RMS {:.0} fpm; p(climb) mid-climb {p_climb_mid:.3}, p(level) mid-level {p_level_mid:.3}, p(climb/descent) mid-descent {p_desc_mid:.3}",
        rms(&ea),
        rms(&er)
    );
    assert!(rms(&ea) < 12.0, "altitude RMS {}", rms(&ea));
    assert!(rms(&er) < 120.0, "rate RMS {}", rms(&er));
    assert!(p_climb_mid > 0.9 && p_level_mid > 0.9 && p_desc_mid > 0.9);
}

// REQ: OA-TRK-008
#[test]
fn a_far_track_is_as_accurate_as_one_at_the_origin() {
    // 600 km east at 64 degrees north: the frame is re-centred on every update, so the
    // projection's distortion and the meridian convergence never accumulate.
    let man = Manoeuvre::straight(240.0, 80.0);
    let f = oa_geo::aeqd::frame_new(64.0, 10.0);
    let mut rng = Rng(17);
    let mut ms = Vec::new();
    for k in 0..2500 {
        let t = f64::from(k);
        let tr = man.at(t, 0.0, 0.0);
        ms.push(measure(
            &f,
            0x444444,
            t,
            &tr,
            SIGMA_POS,
            Some(SIGMA_VEL),
            false,
            &mut rng,
        ));
    }
    let (_, out) = run(&big_config(), &ms, 2500.0);
    // Truth is a straight line in the frame (a path of constant frame heading, not a geodesic,
    // is what the generator integrates), so compare in the same frame.
    let (mut p, mut s) = (Vec::new(), Vec::new());
    for i in 0..out.t_ns.len() {
        if out.flags[i] & O_TICK != 0 || out.t_ns[i] < 100_000_000_000 {
            continue;
        }
        let tr = man.at(out.t_ns[i] as f64 / 1e9, 0.0, 0.0);
        let (e, n) = project(
            &f,
            f64::from(out.lat_e7[i]) / 1e7,
            f64::from(out.lon_e7[i]) / 1e7,
        );
        p.push((e - tr.e).hypot(n - tr.n));
        let (te, tn) = oa_geo::aeqd::try_frame_velocity(&f, tr.e, tr.n, tr.ve, tr.vn).unwrap();
        s.push(
            f64::from(out.vel_east_mps[i]).hypot(f64::from(out.vel_north_mps[i])) - te.hypot(tn),
        );
    }
    println!(
        "600 km at 64 N: position RMS {:.2} m, speed RMS {:.2} m/s",
        rms(&p),
        rms(&s)
    );
    assert!(rms(&p) < 8.0 && rms(&s) < 1.0);
}

fn base_measurements(addr: u32, n: u32, dt: f64, seed: u64) -> Vec<Measurement> {
    let man = Manoeuvre::straight(200.0, 45.0);
    let f = frame0();
    let mut rng = Rng(seed);
    (0..n)
        .map(|k| {
            let t = f64::from(k) * dt;
            measure(
                &f,
                addr,
                t,
                &man.at(t, 0.0, 0.0),
                SIGMA_POS,
                Some(SIGMA_VEL),
                true,
                &mut rng,
            )
        })
        .collect()
}

fn hash_of(o: &TrackOutput) -> String {
    let mut h = Sha256::new();
    output_hash_into(&mut h, o);
    to_hex(&h.finalize())
}

// REQ: OA-TRK-066
#[test]
fn an_absent_altitude_is_not_a_number_in_the_output_columns() {
    let with = base_measurements(0x515151, 60, 1.0, 11);
    assert!(with.iter().all(|m| m.alt_ft != NONE_I32));
    let without: Vec<Measurement> = with
        .iter()
        .map(|m| Measurement {
            alt_ft: NONE_I32,
            vrate_fpm: NONE_I32,
            ..*m
        })
        .collect();
    let (_, out) = run(&big_config(), &without, 70.0);
    assert!(out.t_ns.len() > 30);
    for i in 0..out.t_ns.len() {
        assert_eq!(out.flags[i] & O_HAS_ALT, 0, "row {i}");
        // Not 0 ft: the columns say "no altitude" by themselves, as well as the flag.
        assert!(out.alt_ft[i].is_nan(), "row {i}: {}", out.alt_ft[i]);
        assert!(out.alt_var_ft2[i].is_nan(), "row {i}");
        assert!(out.vrate_fpm[i].is_nan(), "row {i}");
    }
    // With an altitude the columns are numbers and the flag is set.
    let (_, out) = run(&big_config(), &with, 70.0);
    for i in 0..out.t_ns.len() {
        assert_ne!(out.flags[i] & O_HAS_ALT, 0, "row {i}");
        assert!(out.alt_ft[i].is_finite() && out.alt_var_ft2[i].is_finite());
        assert!(out.vrate_fpm[i].is_finite());
    }
}

// REQ: OA-TRK-066
#[test]
fn the_digest_does_not_depend_on_how_an_absent_altitude_is_written() {
    let without: Vec<Measurement> = base_measurements(0x525252, 40, 1.0, 12)
        .iter()
        .map(|m| Measurement {
            alt_ft: NONE_I32,
            vrate_fpm: NONE_I32,
            ..*m
        })
        .collect();
    let (_, out) = run(&big_config(), &without, 50.0);
    assert!(out.alt_ft.iter().all(|a| a.is_nan()));
    // The same rows with the absent altitude written as 0.0, as the tracker used to: same digest.
    let mut zeroed = out.clone();
    for col in [
        &mut zeroed.alt_ft,
        &mut zeroed.alt_var_ft2,
        &mut zeroed.vrate_fpm,
    ] {
        col.iter_mut().for_each(|v| *v = 0.0);
    }
    assert_eq!(hash_of(&out), hash_of(&zeroed));
    // A different altitude on a row that has one changes it.
    let with = base_measurements(0x525252, 40, 1.0, 12);
    let (_, out) = run(&big_config(), &with, 50.0);
    let mut moved = out.clone();
    moved.alt_ft[3] += 1.0;
    assert_ne!(hash_of(&out), hash_of(&moved));
    // and a row that has none keeps hashing as absent whatever its columns hold
    let mut flagged_off = out.clone();
    flagged_off.flags[3] &= !O_HAS_ALT;
    assert_ne!(hash_of(&out), hash_of(&flagged_off));
}

// REQ: OA-TRK-009
#[test]
fn out_of_order_measurements_inside_the_window_are_refiltered_to_the_in_order_state() {
    let ms = base_measurements(0x555555, 120, 1.0, 4);
    let (_, in_order) = run(&big_config(), &ms, 130.0);
    // Swap every pair of neighbours from the 30th on: each arrives one position late.
    let mut swapped = ms.clone();
    let mut k = 30;
    let mut n_swaps = 0;
    while k + 1 < 100 {
        swapped.swap(k, k + 1);
        n_swaps += 1;
        k += 5;
    }
    let (t, ooo) = run(&big_config(), &swapped, 130.0);
    let s = tracker_stats(&t);
    println!(
        "{n_swaps} swaps: refiltered {}, replayed {}",
        s.refiltered, s.replayed
    );
    assert_eq!(s.refiltered, n_swaps);
    assert_eq!(s.replayed, n_swaps);
    assert_eq!(s.late_window + s.late_ring + s.duplicates, 0);
    // The final state is identical: the same measurements were applied in the same order from
    // the same base. The last 30 ticks (after the last swap) agree bit for bit.
    let last = |o: &TrackOutput| {
        let i = o.t_ns.len() - 30;
        (
            o.lat_e7[i..].to_vec(),
            o.lon_e7[i..].to_vec(),
            o.vel_east_mps[i..]
                .iter()
                .map(|v| v.to_bits())
                .collect::<Vec<_>>(),
            o.pos_cov_m2[i..]
                .iter()
                .map(|c| c.map(f32::to_bits))
                .collect::<Vec<_>>(),
        )
    };
    assert_eq!(last(&in_order), last(&ooo));
}

// REQ: OA-TRK-010
#[test]
fn measurements_older_than_the_window_or_the_stored_states_are_counted_and_dropped() {
    let ms = base_measurements(0x666666, 100, 1.0, 5);
    let mut cfg = big_config();
    // Window of 60 s but a ring of only 12 updates: 30 s old is inside the window and outside
    // the ring.
    cfg.ooo_window_ns = 60_000_000_000;
    // The step limit is at least the window and the drop age above the limit.
    cfg.clock_step_limit_ns = 60_000_000_000;
    cfg.drop_after_ns = 120_000_000_000;
    let mut t = tracker_new(&cfg).unwrap();
    let mut out = output_new(cfg.out_capacity);
    for m in &ms {
        tracker_update(&mut t, m, &mut out);
    }
    let mut old = ms[60];
    old.sensor = 9;
    old.toa_ns += 400_000_000; // not a duplicate of the stored 60 s report
    tracker_update(&mut t, &old, &mut out);
    assert_eq!(tracker_stats(&t).late_ring, 1);
    // 90 s old: outside the window.
    let mut older = ms[5];
    older.toa_ns += 400_000_000;
    tracker_update(&mut t, &older, &mut out);
    assert_eq!(tracker_stats(&t).late_window, 1);
    // The exact repeat of the newest report (same time, same sensor) is a duplicate.
    tracker_update(&mut t, &ms[99], &mut out);
    assert_eq!(tracker_stats(&t).duplicates, 1);
    // An inside-the-ring late report is applied.
    let mut inside = ms[95];
    inside.sensor = 9;
    inside.toa_ns += 300_000_000;
    tracker_update(&mut t, &inside, &mut out);
    assert_eq!(tracker_stats(&t).refiltered, 1);
    assert_eq!(tracker_stats(&t).late_ring, 1);
}

// REQ: OA-TRK-011
#[test]
fn a_track_is_confirmed_by_two_reports_of_three_and_restarts_on_an_implausible_second() {
    let ms = base_measurements(0x777777, 6, 1.0, 6);
    let mut t = tracker_new(&big_config()).unwrap();
    let mut out = output_new(1 << 16);
    tracker_update(&mut t, &ms[0], &mut out);
    // One report: tentative, not output.
    assert!(out.t_ns.is_empty() && tracker_live(&t).is_empty());
    tracker_update(&mut t, &ms[1], &mut out);
    // The second confirms it, one second after the first.
    assert_eq!(tracker_live(&t).len(), 1);
    let log_conf = out.t_ns.last().copied().unwrap();
    assert_eq!(log_conf, ms[1].toa_ns);
    assert_eq!(tracker_stats(&t).confirmed, 1);
    tracker_flush(&mut t, 10_000_000_000, &mut out);
    let lg = tracker_log(&t);
    assert_eq!(lg.confirmed_ns[0] - lg.first_ns[0], 1_000_000_000);

    // A lone report that is never followed expires after the window (3 expected intervals).
    let mut t = tracker_new(&big_config()).unwrap();
    let mut out = output_new(1 << 16);
    tracker_update(&mut t, &ms[0], &mut out);
    let other = base_measurements(0x888888, 40, 1.0, 6);
    for m in &other {
        tracker_update(&mut t, m, &mut out);
    }
    assert_eq!(tracker_stats(&t).ended[usize::from(END_TENTATIVE)], 1);

    // A second report beyond the window restarts the track from it.
    let mut t = tracker_new(&big_config()).unwrap();
    let mut out = output_new(1 << 16);
    tracker_update(&mut t, &ms[0], &mut out);
    let mut late = ms[5];
    late.toa_ns = 3_500_000_000;
    tracker_update(&mut t, &late, &mut out);
    assert_eq!(tracker_stats(&t).init_window_expired, 1);
    assert!(tracker_live(&t).is_empty());

    // A second report 500 km away one second later is not the same aircraft: restart.
    let mut t = tracker_new(&big_config()).unwrap();
    let mut out = output_new(1 << 16);
    tracker_update(&mut t, &ms[0], &mut out);
    let mut jump = ms[1];
    jump.lat_e7 += 45_000_000; // 4.5 degrees north
    tracker_update(&mut t, &jump, &mut out);
    assert_eq!(tracker_stats(&t).init_implausible, 1);
    assert!(tracker_live(&t).is_empty());
}

/// Two reports `gap_s` apart of an aircraft flying east at `kt` knots, with its ground speed
/// (an east velocity) on both.
fn east_pair(addr: u32, kt: f64, gap_s: f64) -> [Measurement; 2] {
    let f = frame0();
    let v = kt * 0.514_444_444_444;
    let mut rng = Rng(5);
    let mut at = |t: f64| {
        let tr = Truth {
            e: v * t,
            n: 0.0,
            ve: v,
            vn: 0.0,
            alt_ft: 30000.0,
            vrate_fpm: 0.0,
        };
        measure(&f, addr, t, &tr, 3.0, Some(1.0), true, &mut rng)
    };
    [at(0.0), at(gap_s)]
}

// REQ: OA-TRK-067
#[test]
fn a_fast_aircraft_reporting_its_speed_is_not_restarted_as_implausible() {
    // 950 kt for 2 s is 977 m: over what a single 700 kt limit and 200 m of slack allow (920 m),
    // but the aircraft says it flies 950 kt.
    let [a, b] = east_pair(0x424242, 950.0, 2.0);
    let mut t = tracker_new(&big_config()).unwrap();
    let mut out = output_new(1 << 12);
    tracker_update(&mut t, &a, &mut out);
    tracker_update(&mut t, &b, &mut out);
    assert_eq!(tracker_stats(&t).init_implausible, 0);
    assert_eq!(tracker_live(&t).len(), 1);
    // The same displacement claimed by an aircraft that says it flies 300 kt is a different one.
    let [mut a, mut b] = east_pair(0x424243, 300.0, 2.0);
    b.lon_e7 = a.lon_e7 + (977.0 / (111_195.0 * LAT0.to_radians().cos()) * 1.0e7) as i32;
    a.category = 0xa3;
    b.category = 0xa3;
    let mut t = tracker_new(&big_config()).unwrap();
    tracker_update(&mut t, &a, &mut out);
    tracker_update(&mut t, &b, &mut out);
    assert_eq!(tracker_stats(&t).init_implausible, 1);
}

// REQ: OA-TRK-067
#[test]
fn one_report_claiming_a_fast_category_does_not_excuse_a_jump_in_the_tracker() {
    // 2,782 kt in 2 s without a ground speed: possible only for a space vehicle.
    let [mut a, mut b] = east_pair(0x424244, 450.0, 2.0);
    for m in [&mut a, &mut b] {
        m.flags = 0;
        m.vel_cov_m2s2 = [0.0; 3];
        m.vel_east_mps = 0.0;
        m.category = 0xa3;
    }
    b.lon_e7 = a.lon_e7 + (2862.0 / (111_195.0 * LAT0.to_radians().cos()) * 1.0e7) as i32;
    b.category = 0xb7;
    let mut t = tracker_new(&big_config()).unwrap();
    let mut out = output_new(1 << 12);
    tracker_update(&mut t, &a, &mut out);
    tracker_update(&mut t, &b, &mut out);
    assert_eq!(tracker_stats(&t).init_implausible, 1);
}

/// Reports of an uncategorised aircraft flying east at `kt` with no ground speed (a radar-only
/// track), one per second at the given times; the last is displaced by `jump_kt` instead of `kt`.
fn radar_only_run(addr: u32, kt: f64, times: &[f64], last_kt: f64) -> oa_track::Tracker {
    let f = frame0();
    let v = kt * 0.514_444_444_444;
    let mut rng = Rng(9);
    let cfg = TrackerConfig {
        init_m: 3,
        ..big_config()
    };
    let mut t = tracker_new(&cfg).unwrap();
    let mut out = output_new(1 << 12);
    for (i, &s) in times.iter().enumerate() {
        let e = if i + 1 == times.len() && i > 0 {
            v * times[i - 1] + last_kt * 0.514_444_444_444 * (s - times[i - 1])
        } else {
            v * s
        };
        let tr = Truth {
            e,
            n: 0.0,
            ve: v,
            vn: 0.0,
            alt_ft: 30000.0,
            vrate_fpm: 0.0,
        };
        let m = measure(&f, addr, s, &tr, 1.0, None, false, &mut rng);
        tracker_update(&mut t, &m, &mut out);
    }
    t
}

// REQ: OA-TRK-150
#[test]
fn an_uncategorised_track_without_ground_speed_is_judged_by_its_own_estimated_speed() {
    // 300 kt for two reports, then a report displaced as a 1,500 kt move: under the class ceiling
    // of no category (2,000 kt) but nothing the track could have done.
    let t = radar_only_run(0x515151, 300.0, &[0.0, 1.0, 2.0], 1500.0);
    assert_eq!(tracker_stats(&t).init_implausible, 1);
    // A 900 kt track that keeps flying 900 kt is not restarted by its own next report.
    let t = radar_only_run(0x515152, 900.0, &[0.0, 1.0, 2.0], 900.0);
    assert_eq!(tracker_stats(&t).init_implausible, 0);
    // With one update there is no velocity estimate: the second report is judged as before.
    let t = radar_only_run(0x515153, 300.0, &[0.0, 1.0], 1500.0);
    assert_eq!(tracker_stats(&t).init_implausible, 0);
}

/// A tracker that has followed the aircraft `addr` for four seconds.
fn tracker_with_one_track(addr: u32) -> (oa_track::Tracker, TrackOutput, Vec<Measurement>) {
    let ms = base_measurements(addr, 8, 1.0, 3);
    let mut t = tracker_new(&big_config()).unwrap();
    let mut out = output_new(1 << 16);
    for m in &ms[..4] {
        tracker_update(&mut t, m, &mut out);
    }
    assert_eq!(tracker_live(&t).len(), 1);
    (t, out, ms)
}

// REQ: OA-TRK-068
#[test]
fn one_measurement_far_ahead_does_not_move_the_clock_or_expire_other_tracks() {
    let (mut t, mut out, ms) = tracker_with_one_track(0xAAAAAA);
    // Another aircraft's report an hour ahead, then the first aircraft's next real report.
    let mut far = ms[0];
    far.addr = 0xBBBBBB;
    far.toa_ns = 3600 * 1_000_000_000;
    tracker_update(&mut t, &far, &mut out);
    tracker_update(&mut t, &ms[4], &mut out);
    assert_eq!(tracker_live(&t).len(), 1, "the other track expired");
    assert_eq!(tracker_stats(&t).late_window, 0);
    assert!(tracker_stats(&t).ticks < 20, "{}", tracker_stats(&t).ticks);
    assert_eq!(tracker_stats(&t).applied, 5);
}

// REQ: OA-TRK-068
#[test]
fn three_far_measurements_in_a_row_step_the_clock_without_a_tick_for_every_second() {
    let (mut t, mut out, ms) = tracker_with_one_track(0xAAAAAA);
    for k in 0..3 {
        let mut far = ms[0];
        far.addr = 0xBBBBBB;
        far.toa_ns = (7200 + k) * 1_000_000_000;
        tracker_update(&mut t, &far, &mut out);
    }
    // The old track is long gone; the clock is two hours on and the grid did not walk there.
    assert!(!tracker_live(&t).iter().any(|l| l.0 == 0xAAAAAA));
    assert!(tracker_stats(&t).ticks < 200, "{}", tracker_stats(&t).ticks);
    assert!(tracker_stats(&t).ticks_skipped > 6000);
    assert_eq!(tracker_stats(&t).time_steps, 1);
    // Advancing the caller's clock a day on is as cheap.
    let before = tracker_stats(&t).ticks;
    tracker_advance(
        &mut t,
        7200 * 1_000_000_000 + 86_400 * 1_000_000_000,
        &mut out,
    );
    assert!(tracker_stats(&t).ticks - before < 200);
}

// REQ: OA-TRK-069
#[test]
fn extreme_times_are_refused_and_tick_arithmetic_cannot_overflow_or_stall() {
    use oa_track::{REF_TIME, TIME_LIMIT_NS};
    let ms = base_measurements(0xCCCCCC, 4, 1.0, 4);
    let mut t = tracker_new(&big_config()).unwrap();
    let mut out = output_new(1 << 12);
    for at in [i64::MAX, i64::MIN, TIME_LIMIT_NS + 1, -TIME_LIMIT_NS - 1] {
        let mut m = ms[0];
        m.toa_ns = at;
        tracker_update(&mut t, &m, &mut out);
    }
    assert_eq!(tracker_stats(&t).refused[REF_TIME], 4);
    assert_eq!(tracker_stats(&t).started, 0);
    // The edge of the allowed range is accepted, and its ticks are few.
    let mut m = ms[0];
    m.toa_ns = TIME_LIMIT_NS;
    tracker_update(&mut t, &m, &mut out);
    tracker_advance(&mut t, i64::MAX, &mut out);
    tracker_advance(&mut t, TIME_LIMIT_NS, &mut out);
    assert_eq!(tracker_stats(&t).refused[REF_TIME], 5);
    assert_eq!(tracker_stats(&t).started, 1);
    assert!(tracker_stats(&t).ticks <= 2);
    // A tick under 100 ms, or one beyond a quarter of the range, is a bad configuration.
    for tick_ns in [1, 999_999, 99_999_999, i64::MAX / 4 + 1, i64::MAX] {
        let c = TrackerConfig {
            tick_ns,
            ..big_config()
        };
        assert!(tracker_new(&c).is_err(), "{tick_ns}");
    }
    // The shortest tick near the top of the range still advances and ends.
    let c = TrackerConfig {
        tick_ns: 100_000_000,
        ..big_config()
    };
    let mut t = tracker_new(&c).unwrap();
    let mut m = ms[0];
    m.toa_ns = TIME_LIMIT_NS - 500_000_000;
    tracker_update(&mut t, &m, &mut out);
    tracker_advance(&mut t, TIME_LIMIT_NS, &mut out);
    assert!(tracker_stats(&t).ticks + tracker_stats(&t).ticks_skipped <= 6);
}

// REQ: OA-TRK-290
#[test]
fn a_full_output_table_stops_row_computation_and_counts_the_rows_it_could_not_take() {
    // One confirmed track, 100 ms ticks, a drop age of two hours and an advance of one hour into a
    // one-row table.
    let cfg = TrackerConfig {
        tick_ns: 100_000_000,
        drop_after_ns: 7_200_000_000_000,
        ..big_config()
    };
    let ms = base_measurements(0xA00001, 30, 1.0, 2);
    let mut t = tracker_new(&cfg).unwrap();
    let mut big = output_new(1 << 20);
    for m in &ms {
        tracker_update(&mut t, m, &mut big);
    }
    assert_eq!(tracker_live(&t).len(), 1);
    let last = ms.last().unwrap().toa_ns;
    let before = tracker_stats(&t).rows_computed;
    let mut one = output_new(1);
    tracker_advance(&mut t, last + 3_600_000_000_000, &mut one);
    let computed = tracker_stats(&t).rows_computed - before;
    println!("row computations for the hour: {computed}");
    // Ticks from `last` to `last` + 3600 s inclusive, one row each; one fits.
    assert_eq!(one.t_ns.len(), 1);
    assert_eq!(one.overflow, 36_001 - 1);
    assert!(computed <= 2, "{computed} rows computed");
    assert_eq!(tracker_live(&t).len(), 1);
}

// REQ: OA-TRK-290
#[test]
fn rows_lost_to_a_full_table_equal_the_rows_a_large_table_takes_when_tracks_drop_mid_advance() {
    let cfg = TrackerConfig {
        tick_ns: 100_000_000,
        drop_after_ns: 60_000_000_000,
        // Row accounting is under test, not the coasting display limit (OA-TRK-338).
        coast_max_r95_m: f64::INFINITY,
        reacquire_gap_ns: i64::MAX,
        ..big_config()
    };
    // Two tracks that stop 7 s apart, so they are dropped at different ticks of the advance.
    let mut ms = base_measurements(0xA00001, 30, 1.0, 2);
    ms.extend(base_measurements(0xA00002, 37, 1.0, 3));
    ms.sort_by_key(|m| m.toa_ns);
    let last = ms.last().unwrap().toa_ns;
    let go = |capacity: usize| {
        let mut t = tracker_new(&cfg).unwrap();
        let mut big = output_new(1 << 20);
        for m in &ms {
            tracker_update(&mut t, m, &mut big);
        }
        let before = tracker_stats(&t).rows_computed;
        let mut out = output_new(capacity);
        tracker_advance(&mut t, last + 300_000_000_000, &mut out);
        let computed = tracker_stats(&t).rows_computed - before;
        (t, out, computed)
    };
    let (t_big, big, computed_big) = go(1 << 20);
    let (t_one, one, computed_one) = go(1);
    assert_eq!(big.overflow, 0);
    assert_eq!(one.t_ns.len(), 1);
    assert_eq!(one.overflow + 1, big.t_ns.len() as u64);
    // The tracker ends in the same state.
    assert_eq!(tracker_live(&t_one), tracker_live(&t_big));
    assert_eq!(tracker_stats(&t_one).ticks, tracker_stats(&t_big).ticks);
    assert_eq!(
        tracker_stats(&t_one).ended[usize::from(END_DROPPED)],
        tracker_stats(&t_big).ended[usize::from(END_DROPPED)]
    );
    assert_eq!(computed_big, big.t_ns.len() as u64);
    assert!(computed_one <= 2, "{computed_one} rows computed");
}

// REQ: OA-TRK-012
#[test]
fn a_silent_track_coasts_after_three_intervals_and_is_dropped_after_sixty_seconds() {
    let ms = base_measurements(0x999999, 101, 1.0, 8); // reports at 0 to 100 s
                                                       // Coast and drop timing are under test, not the coasting display limit (OA-TRK-338).
    let cfg = TrackerConfig {
        coast_max_r95_m: f64::INFINITY,
        reacquire_gap_ns: i64::MAX,
        ..big_config()
    };
    let mut t = tracker_new(&cfg).unwrap();
    let mut out = output_new(1 << 18);
    for m in &ms {
        tracker_update(&mut t, m, &mut out);
    }
    tracker_advance(&mut t, 200_000_000_000, &mut out);
    let ticks: Vec<(i64, bool)> = (0..out.t_ns.len())
        .filter(|&i| out.flags[i] & O_TICK != 0)
        .map(|i| (out.t_ns[i] / 1_000_000_000, out.flags[i] & O_COAST != 0))
        .collect();
    // Ticks are whole seconds, one per second per track.
    assert!(ticks.windows(2).all(|w| w[1].0 == w[0].0 + 1));
    let first_coast = ticks.iter().find(|x| x.1).unwrap().0;
    let last_tick = ticks.last().unwrap().0;
    println!("coast from tick {first_coast} s, last tick {last_tick} s");
    // Last update at 100 s: coasting once older than 3 s (strictly), dropped once older than 60 s.
    assert_eq!(first_coast, 104);
    assert_eq!(last_tick, 160);
    assert!(ticks.iter().filter(|x| x.0 <= 103).all(|x| !x.1));
    assert_eq!(tracker_stats(&t).ended[usize::from(END_DROPPED)], 1);
    assert!(tracker_live(&t).is_empty());
}

// REQ: OA-TRK-013
#[test]
fn ticks_are_whole_seconds_at_a_fixed_rate_with_one_row_per_confirmed_track() {
    let mut ms = base_measurements(0xA00001, 30, 0.5, 2);
    ms.extend(base_measurements(0xA00002, 30, 0.7, 3));
    ms.sort_by_key(|m| m.toa_ns);
    let (_, out) = run(&big_config(), &ms, 20.0);
    let ticks: Vec<usize> = (0..out.t_ns.len())
        .filter(|&i| out.flags[i] & O_TICK != 0)
        .collect();
    assert!(!ticks.is_empty());
    assert!(ticks.iter().all(|&i| out.t_ns[i] % 1_000_000_000 == 0));
    // Two tracks at every second from the first full second after confirmation.
    for s in 2..=19i64 {
        let n = ticks
            .iter()
            .filter(|&&i| out.t_ns[i] == s * 1_000_000_000)
            .count();
        assert_eq!(n, 2, "tick at {s} s");
    }
    // Update rows come at the reports' own times (not whole seconds).
    assert!(
        (0..out.t_ns.len()).any(|i| out.flags[i] & O_TICK == 0 && out.t_ns[i] % 1_000_000_000 != 0)
    );
}

// REQ: OA-TRK-014
#[test]
fn the_same_measurements_give_the_same_bytes() {
    let mut ms = Vec::new();
    for a in 0..6u32 {
        ms.extend(base_measurements(
            0xB00000 + a,
            60,
            1.0 + f64::from(a) * 0.1,
            40 + u64::from(a),
        ));
    }
    ms.sort_by_key(|m| (m.toa_ns, m.addr));
    let (t1, o1) = run(&big_config(), &ms, 90.0);
    let (t2, o2) = run(&big_config(), &ms, 90.0);
    assert_eq!(hash_of(&o1), hash_of(&o2));
    assert_eq!(o1, o2);
    assert_eq!(tracker_stats(&t1), tracker_stats(&t2));
    println!("output digest {}", hash_of(&o1));
    // A different capacity changes no result.
    let mut cfg = big_config();
    cfg.max_tracks = 200;
    let (_, o3) = run(&cfg, &ms, 90.0);
    assert_eq!(hash_of(&o1), hash_of(&o3));
}

// REQ: OA-TRK-015
#[test]
fn capacity_is_bounded_and_overflow_is_counted_not_silent() {
    let mut ms = Vec::new();
    for a in 0..5u32 {
        ms.extend(base_measurements(0xC00000 + a, 20, 1.0, 50 + u64::from(a)));
    }
    ms.sort_by_key(|m| (m.toa_ns, m.addr));
    let mut cfg = big_config();
    cfg.max_tracks = 3;
    let (t, _) = run(&cfg, &ms, 30.0);
    let s = tracker_stats(&t);
    // Two aircraft never get a slot; every one of their 20 reports is refused and counted.
    assert_eq!(s.started, 3);
    assert_eq!(s.capacity_refused, 40);
    // Output table: a small capacity loses rows and says so.
    let mut cfg = big_config();
    cfg.out_capacity = 25;
    let mut t = tracker_new(&cfg).unwrap();
    let mut out = output_new(cfg.out_capacity);
    for m in &base_measurements(0xC10000, 60, 1.0, 1) {
        tracker_update(&mut t, m, &mut out);
    }
    assert_eq!(out.t_ns.len(), 25);
    assert!(out.overflow > 0);
    assert_eq!(out.overflow + 25, 59 + 58); // 59 update rows, 58 whole-second ticks before the last report
                                            // Track log: capacity 1, two tracks end.
    let mut cfg = big_config();
    cfg.log_capacity = 1;
    let mut t = tracker_new(&cfg).unwrap();
    let mut out = output_new(1 << 16);
    for a in 0..2u32 {
        for m in &base_measurements(0xC20000 + a, 5, 1.0, 1) {
            tracker_update(&mut t, m, &mut out);
        }
    }
    tracker_flush(&mut t, 10_000_000_000, &mut out);
    assert_eq!(tracker_log(&t).addr.len(), 1);
    assert_eq!(tracker_log(&t).overflow, 1);
}

// REQ: OA-TRK-001
#[test]
fn invalid_measurements_are_refused_with_a_reason() {
    let good = base_measurements(0xD00001, 3, 1.0, 1)[0];
    assert!(measurement_check(&good).is_ok());
    let mut m = good;
    m.lat_e7 = 910_000_000;
    assert_eq!(measurement_check(&m), Err(0));
    let mut m = good;
    m.pos_cov_m2 = [f32::NAN, 0.0, 1.0];
    assert_eq!(measurement_check(&m), Err(1));
    let mut m = good;
    m.pos_cov_m2 = [1.0, 2.0, 1.0];
    assert_eq!(measurement_check(&m), Err(1));
    let mut m = good;
    m.vel_cov_m2s2 = [0.0; 3];
    assert!(m.flags & M_HAS_VEL != 0);
    assert_eq!(measurement_check(&m), Err(2));
    let mut m = good;
    m.alt_ft = 500_000;
    assert_eq!(measurement_check(&m), Err(3));
    let mut m = good;
    m.addr = NO_ADDR;
    assert_eq!(measurement_check(&m), Err(4));
    let mut m = good;
    m.alt_ft = NONE_I32;
    m.vrate_fpm = NONE_I32;
    m.flags = 0;
    assert!(measurement_check(&m).is_ok());
    // A refusal reaches the counters and no track is made.
    let mut t = tracker_new(&big_config()).unwrap();
    let mut out = output_new(100);
    let mut bad = good;
    bad.pos_cov_m2 = [0.0, 0.0, 0.0];
    tracker_update(&mut t, &bad, &mut out);
    assert_eq!(tracker_stats(&t).refused[1], 1);
    assert_eq!(tracker_stats(&t).started, 0);
    // Bad configurations are refused.
    for c in [
        TrackerConfig {
            max_tracks: 0,
            ..DEFAULT_CONFIG
        },
        TrackerConfig {
            init_m: 0,
            ..DEFAULT_CONFIG
        },
        TrackerConfig {
            init_n: 1,
            init_m: 2,
            ..DEFAULT_CONFIG
        },
        TrackerConfig {
            tick_ns: 0,
            ..DEFAULT_CONFIG
        },
        TrackerConfig {
            h_mode: 9,
            ..DEFAULT_CONFIG
        },
    ] {
        assert!(tracker_new(&c).is_err());
    }
}

// REQ: OA-TRK-016
#[test]
fn aircraft_crossing_close_together_keep_their_own_identity() {
    // Two aircraft converge head-on and cross 120 m apart (inside the 3-sigma of one report);
    // tracks are keyed by address, so identity cannot swap, and each stays near its own truth.
    let f = frame0();
    let a = Manoeuvre::straight(200.0, 90.0);
    let b = Manoeuvre::straight(200.0, 270.0);
    let mut rng = Rng(77);
    let mut ms = Vec::new();
    for k in 0..200 {
        let t = f64::from(k);
        let ta = a.at(t, -10_000.0, 0.0);
        let tb = b.at(t, 10_000.0, 120.0);
        ms.push(measure(
            &f,
            0xE00001,
            t,
            &ta,
            SIGMA_POS,
            Some(SIGMA_VEL),
            false,
            &mut rng,
        ));
        ms.push(measure(
            &f,
            0xE00002,
            t,
            &tb,
            SIGMA_POS,
            Some(SIGMA_VEL),
            false,
            &mut rng,
        ));
    }
    let (_, out) = run(&big_config(), &ms, 200.0);
    let mut worst: f64 = 0.0;
    for i in 0..out.t_ns.len() {
        if out.flags[i] & O_TICK != 0 || out.t_ns[i] < 20_000_000_000 {
            continue;
        }
        let t = out.t_ns[i] as f64 / 1e9;
        let tr = if out.addr[i] == 0xE00001 {
            a.at(t, -10_000.0, 0.0)
        } else {
            b.at(t, 10_000.0, 120.0)
        };
        worst = worst.max(pos_err(&f, &out, i, &tr));
    }
    println!("crossing at 120 m: worst position error {worst:.1} m");
    assert!(worst < 50.0, "worst {worst}");
}

// REQ: OA-TRK-017
#[test]
fn nothing_allocates_after_the_tracker_is_built() {
    // Every buffer keeps its address and capacity through a run with confirmations, late
    // measurements, drops, restarts and overflow of the output and the log.
    let mut cfg = big_config();
    cfg.max_tracks = 8;
    cfg.out_capacity = 500;
    cfg.log_capacity = 3;
    let mut t = tracker_new(&cfg).unwrap();
    let mut out = output_new(cfg.out_capacity);
    let before = oa_track::tracker_buffers(&t);
    let out_before = (
        out.t_ns.as_ptr() as usize,
        out.t_ns.capacity(),
        out.mu_h.as_ptr() as usize,
        out.mu_h.capacity(),
    );
    let mut ms = Vec::new();
    for a in 0..12u32 {
        ms.extend(base_measurements(
            0xF00000 + a * 7919,
            40,
            1.0,
            100 + u64::from(a),
        ));
    }
    ms.sort_by_key(|m| (m.toa_ns, m.addr));
    // Some late ones.
    for k in (50..ms.len() - 3).step_by(37) {
        ms.swap(k, k + 2);
    }
    for m in &ms {
        tracker_update(&mut t, m, &mut out);
    }
    tracker_advance(&mut t, 400_000_000_000, &mut out);
    tracker_flush(&mut t, 500_000_000_000, &mut out);
    assert!(
        out.overflow > 0 && tracker_log(&t).overflow > 0 && tracker_stats(&t).capacity_refused > 0
    );
    assert_eq!(before, oa_track::tracker_buffers(&t));
    assert_eq!(
        out_before,
        (
            out.t_ns.as_ptr() as usize,
            out.t_ns.capacity(),
            out.mu_h.as_ptr() as usize,
            out.mu_h.capacity()
        )
    );
}

/// Three aircraft whose reports arrive scrambled by up to 14 positions (past the 12-deep window
/// for some), with a different-sensor repeat of some reports and a wild position now and then.
fn scrambled_world() -> Vec<Measurement> {
    let mut all: Vec<(f64, Measurement)> = Vec::new();
    let mut rng = Rng(0xF00D);
    for (a, addr) in [0x1A1A1Au32, 0x2B2B2B, 0x3C3C3C].into_iter().enumerate() {
        for (k, m) in base_measurements(addr, 160, 1.0, 20 + a as u64)
            .into_iter()
            .enumerate()
        {
            let mut m = m;
            if k % 37 == 21 {
                m.lat_e7 += 90_000; // about a kilometre of jump: rejected by the gate
            }
            let arrival = k as f64 + rng.uniform() * 14.0;
            all.push((arrival, m));
            if k % 9 == 4 {
                let mut d = m;
                d.sensor = 3;
                all.push((arrival + 0.5, d));
            }
            if k % 11 == 5 {
                all.push((arrival + 2.5, m)); // the exact same report again
            }
        }
    }
    all.sort_by(|x, y| x.0.total_cmp(&y.0));
    all.into_iter().map(|x| x.1).collect()
}

// REQ: OA-TRK-040
#[test]
fn scrambled_input_keeps_its_pinned_replay_counters_and_output() {
    // These pins originated with the full-state history ring. They now lock the current
    // estimator's replay, duplicates and ring/window drops. The separate in-order versus
    // out-of-order tests compare replay states directly.
    let ms = scrambled_world();
    // With the coasting display limit off the output is bit for bit the pinned one, so
    // OA-TRK-338 changes nothing but the withheld rows.
    let cfg = TrackerConfig {
        coast_max_r95_m: f64::INFINITY,
        reacquire_gap_ns: i64::MAX,
        ..big_config()
    };
    let (t, out) = run(&cfg, &ms, 200.0);
    let s = tracker_stats(&t);
    let counters = format!(
        "{} {} {} {} {} {} {} {} {} {}",
        s.offered,
        s.applied,
        s.duplicates,
        s.late_window,
        s.late_ring,
        s.refiltered,
        s.replayed,
        s.jump_rejected,
        s.resyncs,
        s.nis_n
    );
    let digest = hash_of(&out);
    println!(
        "scrambled: {counters} nis_sum {:016x} digest {digest}",
        s.nis_sum.to_bits()
    );
    println!("scrambled full stats {s:?}");
    assert_eq!(s.numerical_refused, 0);
    assert!(s.refiltered > 20 && s.late_ring > 0 && s.duplicates > 0 && s.jump_rejected > 0);
    assert_eq!(counters, GOLDEN_COUNTERS);
    assert_eq!(s.nis_sum.to_bits(), GOLDEN_NIS_BITS);
    assert_eq!(digest, GOLDEN_DIGEST);
}

// Re-recorded when the repeat check began to look at every stored report (OA-TRK-077) and a late
// re-filter began to keep the run of rejected reports (OA-TRK-078): five more repeats are now
// counted as duplicates instead of applied again (30 became 35, applied 477 became 472). The
// earlier values were:
// "579 477 30 53 4 288 869 14 0 1343", 0x40a4_9f6c_7a87_a22b and
// "6ea4de8c2647f225e2b723d0422fb68eef0b81dea5a641f733827844421c6cd5".
const GOLDEN_COUNTERS: &str = "579 472 35 53 4 284 847 14 0 1316";
// Re-recorded when the covariance mapping and the exact velocity differential came in
// (OA-TRK-260 to OA-TRK-263): the counters are unchanged (nothing applied, dropped or gained); the
// innovation sum went from 2588.444 to 2589.553 (4e-4 of itself) and the rows differ in
// covariance and velocity. The earlier values were 0x40a4_38e3_6693_06a4 and
// "67be4436e36e6b6b39b89cea17490227140bfdccddf9df9e8b42d9af2d811bfc".
// Platform libm replaced by oa-math: counters and digest unchanged (no output row differs), the
// innovation sum 2589.552684637037 to 2589.552684636896 (5e-14 of itself, 310 ulp). The earlier
// value was 0x40a4_3b1a_f97b_1227.
// Elementary-function calls cut on the hot path (the frame's latitude sine and cosine carried
// with the filter and the small-angle series in the chart, one exponential for the model and
// vertical transitions, the model probabilities from the difference of the log weights): counters
// unchanged; of 1,062 rows 8 differ, in covariance and turn rate by at most 1e-7 of themselves
// (one single-precision unit), in no position, velocity or probability; the innovation sum
// 2589.552684636896 to 2589.552684607768 (1.1e-11 of itself, 64,053 ulp). The earlier values were
// 0x40a4_3b1a_f97b_10f1 and "c953ab2374b272a955edec2c2fd4cfe0ec9afb8911947e58f8f1ac6590791133".
// Nonlinear turn moments with separate discovery regularization (OA-TRK-329) change the
// innovations and output values; all ten replay counters above remain unchanged. Prior values:
// 0x40a4_3b1a_f97a_16bc and
// "8924b0a9d9aef420b74326d06d45fe3b055b3ea20a74845e0b03bfa4f6d1a6bd".
// The velocity update carries the turn rate over the transponder's lag (OA-TRK-395): the counters
// are unchanged; the innovation sum went from 2585.2 to 2557.2 and the rows differ in velocity,
// turn rate and covariance. The earlier values were
// 0x40a4_3264_bf63_657b and
// "5864f9c330900223ad8e431a98d2f5e1ba70f82867c4a64a3005d89d9921488f".
const GOLDEN_NIS_BITS: u64 = 0x40a3_fa65_dc6f_6157;
const GOLDEN_DIGEST: &str = "cf1bc8249e2c9d4ae3322654205a29880d5bfbc5dd2438915e41a4448cfdaa75";

/// Aircraft that start, stop and start again with long gaps between: one that stops early (and
/// is dropped), one that flies on, and two that begin after gaps far longer than the drop time.
fn gappy_world() -> Vec<Measurement> {
    let mut all: Vec<Measurement> = Vec::new();
    for (addr, start_s, n, dt, seed) in [
        (0x1A1A1Au32, 0i64, 40u32, 1.0, 71u64),
        (0x2B2B2B, 0, 300, 1.0, 72),
        (0x3C3C3C, 1_000, 60, 1.0, 73),
        (0x4D4D4D, 20_000, 5, 1.0, 74),
        (0x5E5E5E, 20_000, 120, 2.0, 75),
    ] {
        for mut m in base_measurements(addr, n, dt, seed) {
            m.toa_ns += start_s * 1_000_000_000;
            all.push(m);
        }
    }
    all.sort_by_key(|m| m.toa_ns);
    all
}

// REQ: OA-TRK-173
#[test]
fn the_rows_across_drops_and_long_gaps_are_the_ones_the_full_scan_gave() {
    // The digest was recorded when the jump over a long gap found the newest live update by
    // scanning every slot; the tracker now keeps it, and the rows are the same bit for bit.
    // With the coasting display limit off the output is bit for bit the pinned one (OA-TRK-338).
    let cfg = TrackerConfig {
        coast_max_r95_m: f64::INFINITY,
        reacquire_gap_ns: i64::MAX,
        ..big_config()
    };
    let mut t = tracker_new(&cfg).unwrap();
    let mut out = output_new(cfg.out_capacity);
    for (i, m) in gappy_world().iter().enumerate() {
        tracker_update(&mut t, m, &mut out);
        if i % 50 == 0 {
            // Time also passes between measurements.
            tracker_advance(&mut t, m.toa_ns + 3_000_000_000, &mut out);
        }
    }
    tracker_advance(&mut t, 30_000_000_000_000, &mut out);
    tracker_flush(&mut t, 30_000_000_000_000, &mut out);
    let s = tracker_stats(&t);
    println!(
        "gappy: rows {} ended {:?} digest {} held {} applied after {}",
        out.t_ns.len(),
        s.ended,
        hash_of(&out),
        s.time_held,
        s.time_held_applied
    );
    assert!(s.ended[usize::from(END_DROPPED)] >= 4, "{:?}", s.ended);
    println!("gappy full stats {s:?}");
    assert_eq!(s.numerical_refused, 0);
    assert_eq!(out.overflow, 0);
    assert_eq!(hash_of(&out), GAPPY_DIGEST);
}

// REQ: OA-TRK-173
#[test]
fn the_newest_update_is_kept_and_is_an_upper_bound_on_the_live_ones() {
    let cfg = big_config();
    let mut t = tracker_new(&cfg).unwrap();
    let mut out = output_new(cfg.out_capacity);
    assert_eq!(oa_track::tracker_newest_update_ns(&t), i64::MIN);
    let mut newest = i64::MIN;
    for m in gappy_world().iter().take(200) {
        tracker_update(&mut t, m, &mut out);
        newest = newest.max(m.toa_ns);
        let kept = oa_track::tracker_newest_update_ns(&t);
        assert!(kept <= newest, "{kept} > {newest}");
        for (_, last) in tracker_live(&t) {
            assert!(
                last <= kept,
                "a live track's update {last} is newer than {kept}"
            );
        }
    }
    // The last update of an aircraft still flying is the newest there is.
    assert!(oa_track::tracker_newest_update_ns(&t) > newest - 2_000_000_000);
}

// Re-recorded when the measurements held for a far-ahead run began to be applied after the step
// that confirms them (OA-TRK-199): the first report of the aircraft starting after the 1,000 s
// gap and the second were lost to the confirmation, now they are applied (2 measurements changed
// fate; a third, held at 20,000 s, is discarded because the caller's `tracker_advance` moved the
// clock first). The earlier value was
// "5cbeb052deda04a2f6ebf9a7e1e34fa8f94955283f59bd03cec60ceb8cbcd86f".
// Re-recorded for the covariance mapping and exact velocity differential (OA-TRK-260 to
// OA-TRK-263): the same 1,446 rows and the same end and hold counts; only covariance and velocity
// values moved. The earlier value was
// "797b729ae9d90ef7a363ffd0bafbebeed9d1e1edfee527696a6df1f4b0d14290".
// Re-recorded when elementary-function calls were cut on the hot path (see the scrambled-input
// digest above): the same 1,446 rows and the same end and hold counts; 13 rows differ, in
// covariance and turn rate by at most 1e-7 of themselves, in no position, velocity or
// probability. The earlier value was
// "9aab86ffa408a1516de991484686298f0ee0fd676a803e2c6a2664a8bf4d4163".
// OA-TRK-329 changes predicted values with the same 1,446 rows, end counts, held reports and
// applied-after-step counts. Prior digest:
// "6bf2d72d551760c04e78ed96d9054f6519e8db2ee59fa9b4a1300bbf212578fd".
// Re-recorded for the velocity update with the transponder's lag (OA-TRK-395): the same 1,446 rows
// and the same end and hold counts; velocity, turn rate and covariance values moved. The earlier
// value was "4a705106b8b2beecf911272b615cf909e7b4c87082b8a2de8b62cf66b3f04b44".
const GAPPY_DIGEST: &str = "facdf5997ed53697123759ad52c8c6d3aaade1f546220181ded9d328aff4f4f9";

// REQ: OA-TRK-041
#[test]
fn a_full_table_of_max_tracks_refuses_the_next_aircraft_and_never_reallocates() {
    let n = oa_track::params::MAX_TRACKS;
    let cfg = TrackerConfig {
        max_tracks: n,
        out_capacity: 1 << 16,
        log_capacity: 16,
        ..DEFAULT_CONFIG
    };
    let mut t = tracker_new(&cfg).unwrap();
    let before = tracker_buffers(&t);
    let mut out = output_new(cfg.out_capacity);
    let man = Manoeuvre::straight(200.0, 45.0);
    let f = frame0();
    let mut rng = Rng(77);
    // Every aircraft gets 16 reports, more than the ring holds, so every anchor is advanced.
    for k in 0..16 {
        for a in 0..n {
            let m = measure(
                &f,
                0x100000 + a as u32,
                f64::from(k),
                &man.at(f64::from(k), 0.0, 0.0),
                SIGMA_POS,
                Some(SIGMA_VEL),
                true,
                &mut rng,
            );
            tracker_update(&mut t, &m, &mut out);
            if out.t_ns.len() > cfg.out_capacity / 2 {
                oa_track::output_clear(&mut out);
            }
        }
    }
    let s = *tracker_stats(&t);
    assert_eq!(s.started as usize, n);
    assert_eq!(s.capacity_refused, 0);
    // One aircraft too many.
    let extra = measure(
        &f,
        0x900000,
        15.0,
        &man.at(15.0, 0.0, 0.0),
        SIGMA_POS,
        Some(SIGMA_VEL),
        true,
        &mut rng,
    );
    tracker_update(&mut t, &extra, &mut out);
    assert_eq!(tracker_stats(&t).capacity_refused, 1);
    assert_eq!(tracker_stats(&t).started as usize, n);
    assert_eq!(tracker_buffers(&t), before, "a buffer was reallocated");
    // A late report for a track in the full table is still re-filtered from its anchor.
    let mut late = measure(
        &f,
        0x100007,
        14.5,
        &man.at(14.5, 0.0, 0.0),
        SIGMA_POS,
        Some(SIGMA_VEL),
        true,
        &mut rng,
    );
    late.sensor = 4;
    tracker_update(&mut t, &late, &mut out);
    assert_eq!(tracker_stats(&t).refiltered, 1);
    let bytes = tracker_bytes(&t);
    println!(
        "tracker_bytes at {n} tracks: {bytes} ({} per track)",
        bytes / n
    );
    // The columns, one current and one anchor filter and the measurement ring: under 2.3 kB each
    // (it was 8.9 kB with a filter state in every ring slot; 2.2 kB before the filter carried
    // the newest rejected report of a run, 16 bytes in each of its two copies). The large-move
    // rule's 39 bytes of columns per track (the sum of its columns, checked in the tracker's own tests) and its pool of 64 rollback snapshots (RT-239,
    // 141 kB in all, 34 bytes per track at this 4096-track size, under 4 at 38,000) bring it to
    // 2.27 kB: still under 2.3 kB.
    assert!(bytes / n < 2_300, "{} bytes per track", bytes / n);
}

// REQ: OA-TRK-042
#[test]
fn tracker_memory_is_linear_in_capacity_and_about_a_quarter_of_the_full_state_ring() {
    let mk = |n: usize| {
        tracker_bytes(
            &tracker_new(&TrackerConfig {
                max_tracks: n,
                out_capacity: 16,
                log_capacity: 16,
                // The rollback pool is a fixed cost, not per track: measured apart below.
                confirm_pool: 0,
                ..DEFAULT_CONFIG
            })
            .unwrap(),
        )
    };
    let (a, b) = (mk(4096), mk(38_000));
    println!("tracker_bytes: 4096 tracks {a}, 38000 tracks {b}");
    // The index is the only part that is not exactly linear (a power of two entries).
    let per = |x: usize, n: usize| x as f64 / n as f64;
    assert!((per(a, 4096) - per(b, 38_000)).abs() < 12.0);
    // A ring of 12 full filter states and 12 measurements cost 8.9 kB per track. The large-move
    // rule (RT-239) adds 39 bytes of columns per track (the bound was 2300 before it).
    assert!(per(b, 38_000) < 2_300.0);
    // And its pool of rollback snapshots, 2200 bytes each and a 4-byte free-list entry, a fixed
    // cost independent of the number of tracks.
    let with_pool = tracker_bytes(
        &tracker_new(&TrackerConfig {
            max_tracks: 4096,
            out_capacity: 16,
            log_capacity: 16,
            ..DEFAULT_CONFIG
        })
        .unwrap(),
    );
    assert_eq!(
        with_pool - a,
        oa_track::params::CONFIRM_POOL * (2_200 + 4),
        "the pool"
    );
    println!("tracker_bytes with the pool: 4096 tracks {with_pool}");
}

// REQ: OA-TRK-077
#[test]
fn a_repeat_of_a_report_is_a_duplicate_even_after_another_sensor_reported_at_the_same_time() {
    let ms = base_measurements(0x777777, 30, 1.0, 6);
    let cfg = big_config();
    let mut t = tracker_new(&cfg).unwrap();
    let mut out = output_new(cfg.out_capacity);
    for m in &ms {
        tracker_update(&mut t, m, &mut out);
    }
    // A second sensor reports at the very instant of the newest report: applied, not a repeat.
    let mut other = ms[29];
    other.sensor = 5;
    tracker_update(&mut t, &other, &mut out);
    assert_eq!(tracker_stats(&t).duplicates, 0);
    let applied = tracker_stats(&t).applied;
    // The first sensor's report arrives again (a replayed frame): a duplicate, though the newest
    // stored report is the other sensor's.
    tracker_update(&mut t, &ms[29], &mut out);
    assert_eq!(tracker_stats(&t).duplicates, 1);
    assert_eq!(tracker_stats(&t).applied, applied);
    // The same inside the window: the other sensor reports at the time of an older report, then
    // that older report repeats.
    let mut o25 = ms[25];
    o25.sensor = 5;
    tracker_update(&mut t, &o25, &mut out);
    assert_eq!(tracker_stats(&t).refiltered, 1);
    let applied = tracker_stats(&t).applied;
    tracker_update(&mut t, &ms[25], &mut out);
    assert_eq!(tracker_stats(&t).duplicates, 2);
    assert_eq!(tracker_stats(&t).applied, applied);
}

// REQ: OA-TRK-232
#[test]
fn the_velocity_limit_is_the_speed_envelope_of_the_category_table() {
    let mut m = base_measurements(0xD00003, 1, 1.0, 1)[0];
    m.vel_cov_m2s2 = [1.0, 0.0, 1.0];
    m.flags |= M_HAS_VEL;
    m.vel_north_mps = 0.0;
    let envelope = oa_adsb::SPEED_ENVELOPE_MPS as f32;
    m.vel_east_mps = envelope - 1.0;
    assert_eq!(measurement_check(&m), Ok(()));
    m.vel_east_mps = envelope + 1.0;
    assert_eq!(measurement_check(&m), Err(2));
}

// REQ: OA-TRK-233
#[test]
fn a_velocity_is_accepted_up_to_the_supported_speed_envelope_and_refused_above_it() {
    let mut base = base_measurements(0xD00002, 3, 1.0, 1)[0];
    base.category = 0xb7;
    base.vel_cov_m2s2 = [1.0, 0.0, 1.0];
    let kt = |k: f64| (k * 0.514_444_444_444) as f32;
    let with_vel = |k: f64| {
        let mut m = base;
        m.vel_east_mps = kt(k);
        m.vel_north_mps = 0.0;
        m.flags |= M_HAS_VEL;
        m
    };
    let without = {
        let mut m = base;
        m.flags &= !M_HAS_VEL;
        m
    };
    assert!(measurement_check(&without).is_ok());
    // 2,916 kt is 1,500 m/s; 3,000 kt is a valid B7 speed.
    for k in [2_916.0, 3_000.0, 4_000.0, oa_adsb::SPEED_ENVELOPE_KT] {
        assert_eq!(measurement_check(&with_vel(k)), Ok(()), "{k} kt");
    }
    // Just above the envelope, and far above: refused as a bad velocity.
    for k in [4_090.0, 6_000.0] {
        assert_eq!(measurement_check(&with_vel(k)), Err(2), "{k} kt");
    }
    // The refusal is counted and no track is made; the same report below the envelope is applied.
    let mut t = tracker_new(&big_config()).unwrap();
    let mut out = output_new(100);
    tracker_update(&mut t, &with_vel(4_090.0), &mut out);
    assert_eq!(tracker_stats(&t).refused[2], 1);
    assert_eq!(tracker_stats(&t).started, 0);
    tracker_update(&mut t, &with_vel(3_000.0), &mut out);
    assert_eq!(tracker_stats(&t).refused[2], 1);
    assert_eq!(tracker_stats(&t).started, 1);
}

// REQ: OA-TRK-078
#[test]
fn a_late_report_does_not_erase_the_run_of_rejected_reports() {
    let ms = base_measurements(0x888888, 40, 1.0, 7);
    let cfg = big_config();
    let mut t = tracker_new(&cfg).unwrap();
    let mut out = output_new(cfg.out_capacity);
    for m in &ms[..30] {
        tracker_update(&mut t, m, &mut out);
    }
    // Three reports 55 km off: rejected, a run of three.
    let wild = |k: usize| {
        let mut m = ms[k];
        m.lat_e7 += 5_000_000;
        m
    };
    for k in 30..33 {
        tracker_update(&mut t, &wild(k), &mut out);
    }
    assert_eq!(tracker_stats(&t).jump_rejected, 3);
    // A late report from another sensor, between two of the track's own reports, is applied by
    // re-filtering.
    let half = base_measurements(0x888888, 80, 0.5, 7);
    let mut late = half[57];
    late.sensor = 9;
    tracker_update(&mut t, &late, &mut out);
    assert_eq!(tracker_stats(&t).refiltered, 1);
    // Two more wild reports make the fifth in a row: the filter must start again at the fifth.
    for k in 33..35 {
        tracker_update(&mut t, &wild(k), &mut out);
    }
    assert_eq!(tracker_stats(&t).jump_rejected, 5);
    assert_eq!(tracker_stats(&t).resyncs, 1);
}

// REQ: OA-TRK-083
#[test]
fn a_position_on_the_far_side_of_the_earth_is_a_jump_not_a_coherent_update() {
    // A stationary aircraft at the frame origin of its own filter, reported at (0, 0) every
    // second; then a report of the same address at (0, 180) with ordinary covariance and a
    // consistent zero velocity. The antipode used to project onto the origin.
    let cfg = big_config();
    let mut t = tracker_new(&cfg).unwrap();
    let mut out = output_new(cfg.out_capacity);
    let m_at = |k: i64, lon_e7: i32| Measurement {
        toa_ns: k * 1_000_000_000,
        pos_cov_m2: [100.0, 0.0, 100.0],
        vel_cov_m2s2: [1.0, 0.0, 1.0],
        vel_east_mps: 0.0,
        vel_north_mps: 0.0,
        lat_e7: 0,
        lon_e7,
        alt_ft: NONE_I32,
        vrate_fpm: NONE_I32,
        addr: 0x5A5A5A,
        alt_quant_ft: 0,
        sensor: 1,
        ident: oa_track::NO_IDENT,
        flags: M_HAS_VEL,
        category: 0,
    };
    for k in 0..10 {
        tracker_update(&mut t, &m_at(k, 0), &mut out);
    }
    let before = *tracker_stats(&t);
    tracker_update(&mut t, &m_at(10, 1_800_000_000), &mut out);
    let after = *tracker_stats(&t);
    assert_eq!(after.jump_rejected, before.jump_rejected + 1);
    assert_eq!(after.applied, before.applied);
}

// REQ: OA-TRK-312
#[test]
fn a_track_extrapolated_past_the_projection_emits_no_zero_position_and_is_counted() {
    let f = frame0();
    let man = Manoeuvre::straight(300.0, 90.0);
    let mut rng = Rng(5);
    let ms: Vec<Measurement> = (0..30)
        .map(|k| {
            let t = f64::from(k);
            measure(
                &f,
                0xA11111,
                t,
                &man.at(t, 0.0, 0.0),
                SIGMA_POS,
                Some(SIGMA_VEL),
                true,
                &mut rng,
            )
        })
        .collect();
    let cfg = TrackerConfig {
        h_mode: 1,
        tick_ns: 60_000_000_000,
        drop_after_ns: 24 * 3_600_000_000_000,
        // The projection limit is under test, not the coasting display limit (OA-TRK-338).
        coast_max_r95_m: f64::INFINITY,
        reacquire_gap_ns: i64::MAX,
        ..big_config()
    };
    let mut t = tracker_new(&cfg).unwrap();
    let mut out = output_new(cfg.out_capacity);
    for m in &ms {
        tracker_update(&mut t, m, &mut out);
    }
    let last = ms.last().unwrap().toa_ns;
    out = output_new(cfg.out_capacity);
    let before = tracker_stats(&t).rows_computed;
    // 300 m/s for 20 h is 21,600 km: past half the sphere's circumference (20,015 km).
    tracker_advance(&mut t, last + 20 * 3_600_000_000_000, &mut out);
    assert!(out.t_ns.len() > 100, "{} rows", out.t_ns.len());
    let zero = (0..out.t_ns.len())
        .filter(|&i| out.lat_e7[i] == 0 && out.lon_e7[i] == 0)
        .count();
    assert_eq!(zero, 0, "rows at (0, 0)");
    let refused = tracker_stats(&t).rows_unrepresentable;
    assert!(refused > 0, "no refusal counted");
    // Every row computed is either emitted or a counted refusal.
    let computed = tracker_stats(&t).rows_computed - before;
    assert_eq!(computed, out.t_ns.len() as u64 + refused);
}
