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
    measurement_check, output_hash_into, output_new, tracker_advance, tracker_flush, tracker_live,
    tracker_log, tracker_new, tracker_stats, tracker_update, Measurement, TrackOutput,
    TrackerConfig, DEFAULT_CONFIG, END_DROPPED, END_TENTATIVE, M_HAS_VEL, NONE_I32, NO_ADDR,
    O_COAST, O_HAS_ALT, O_TICK,
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
        let tr = man.at(t, 0.0, 0.0);
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
    assert!(
        worst_vs_cv1.0 > 0.0 && worst_vs_cv1.1 > 0.0,
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
                &man.at(t, 0.0, 0.0),
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
        let (te, tn) = oa_geo::aeqd::frame_velocity(&f, tr.e, tr.n, tr.ve, tr.vn);
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

// REQ: OA-TRK-030
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

// REQ: OA-TRK-030
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

// REQ: OA-TRK-012
#[test]
fn a_silent_track_coasts_after_three_intervals_and_is_dropped_after_sixty_seconds() {
    let ms = base_measurements(0x999999, 101, 1.0, 8); // reports at 0 to 100 s
    let mut t = tracker_new(&big_config()).unwrap();
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
