//! The four spoof transforms: rows added or changed are labelled, nothing else changes.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_adsb::{reports_len, reports_push, reports_row, Reports, ROW_EMPTY};
use oa_geo::{wgs84, LatLon};
use oa_sensorsim::plots::NONE_ADDR;
use oa_sensorsim::spoof::*;
use oa_sensorsim::SimError;

const T0: i64 = 1_700_000_000_000_000_000;
const A: u32 = 0xa00001;
const B: u32 = 0xa00002;

fn traffic() -> Labelled {
    let mut r = Reports::default();
    for (addr, lat0) in [(A, 41.0), (B, 42.0)] {
        for k in 0..600i64 {
            let mut row = ROW_EMPTY;
            row.icao = addr;
            row.time_ns = T0 + k * 1_000_000_000;
            row.lat_e7 = (lat0 * 1e7) as i32 + (k as i32) * 900;
            row.lon_e7 = -880_000_000 + (k as i32) * 1200;
            row.baro_alt_ft = 30_000;
            row.nic = 8;
            row.nac_p = 10;
            row.sil = 3;
            row.version = 2;
            row.source = 0;
            reports_push(&mut r, &row);
        }
    }
    // Interleave the two aircraft by time, as real data is.
    let mut l = from_clean(&r);
    let mut idx: Vec<usize> = (0..reports_len(&r)).collect();
    idx.sort_by_key(|&i| r.time_ns[i]);
    let mut out = Reports::default();
    for i in idx {
        reports_push(&mut out, &reports_row(&r, i).unwrap());
    }
    l.reports = out;
    l
}

/// WGS-84 geodesic distance between two positions, degrees in and metres out.
fn geo_m(la1: f64, lo1: f64, la2: f64, lo2: f64) -> f64 {
    wgs84()
        .inverse(
            LatLon::new(la1, lo1).unwrap(),
            LatLon::new(la2, lo2).unwrap(),
        )
        .unwrap()
        .distance_m
}

fn count(l: &Labelled, lab: u8) -> usize {
    l.label.iter().filter(|&&x| x == lab).count()
}

fn sorted(l: &Labelled) -> bool {
    l.reports.time_ns.windows(2).all(|w| w[0] <= w[1])
}

fn ghost() -> GhostParams {
    GhostParams {
        addr: 0xabcdef,
        start_ns: T0 + 100_000_000_000,
        interval_ms: 1000,
        count: 120,
        lat_deg: 41.5,
        lon_deg: -88.5,
        speed_kt: 250.0,
        track_deg: 90.0,
        alt_ft: 20_000,
        callsign: *b"GHOST01 ",
        squawk: 1234,
        category: 0xA3,
        nic: 8,
        nac_p: 10,
        nac_v: 2,
        sil: 3,
        version: 2,
    }
}

// REQ: OA-SIM-012
#[test]
fn ghost_adds_a_new_address_on_a_straight_track() {
    let base = traffic();
    let n0 = reports_len(&base.reports);
    let out = inject_ghost(&base, &ghost()).unwrap();
    assert_eq!(reports_len(&out.reports), n0 + 120);
    assert_eq!(count(&out, LABEL_GHOST), 120);
    assert_eq!(count(&out, LABEL_NONE), n0);
    assert!(sorted(&out));
    let mut last: Option<(i64, f64, f64)> = None;
    for i in 0..reports_len(&out.reports) {
        let is_ghost = out.reports.icao[i] == 0xabcdef;
        assert_eq!(is_ghost, out.label[i] == LABEL_GHOST);
        assert_eq!(out.victim[i], NONE_ADDR);
        if is_ghost {
            let (la, lo) = (
                f64::from(out.reports.lat_e7[i]) / 1e7,
                f64::from(out.reports.lon_e7[i]) / 1e7,
            );
            if let Some((t, pla, plo)) = last {
                let dt = (out.reports.time_ns[i] - t) as f64 / 1e9;
                let d = geo_m(pla, plo, la, lo);
                assert!((d / dt - 250.0 * 1852.0 / 3600.0).abs() < 0.1, "{}", d / dt);
            }
            last = Some((out.reports.time_ns[i], la, lo));
            assert_eq!(out.reports.gs_dkt[i], 2500);
            assert_eq!(out.reports.track_ddeg[i], 900);
        }
    }
    // The original rows are intact and in order.
    let orig: Vec<_> = (0..reports_len(&out.reports))
        .filter(|&i| out.label[i] == LABEL_NONE)
        .map(|i| reports_row(&out.reports, i).unwrap())
        .collect();
    let want: Vec<_> = (0..n0)
        .map(|i| reports_row(&base.reports, i).unwrap())
        .collect();
    assert_eq!(orig, want);
}

// REQ: OA-SIM-016
#[test]
fn spoof_parameters_are_validated() {
    let base = traffic();
    let mut g = ghost();
    g.addr = A;
    assert!(matches!(
        inject_ghost(&base, &g),
        Err(SimError::BadSpoof(_))
    ));
    g = ghost();
    g.interval_ms = 0;
    assert!(inject_ghost(&base, &g).is_err());
    g = ghost();
    g.addr = 0x1ff_ffff;
    assert!(inject_ghost(&base, &g).is_err());
    let d = DriftParams {
        victim: A,
        start_ns: T0,
        end_ns: T0,
        rate_mps: 1.0,
        bearing_deg: 0.0,
    };
    assert!(inject_drift(&base, &d).is_err());
    let d = DriftParams {
        victim: 0x123,
        start_ns: T0,
        end_ns: T0 + 1_000_000_000,
        rate_mps: 1.0,
        bearing_deg: 0.0,
    };
    assert!(inject_drift(&base, &d).is_err());
    let r = ReplayParams {
        victim: A,
        src_start_ns: T0,
        src_end_ns: T0 + 10,
        shift_ns: 0,
        as_addr: A,
    };
    assert!(inject_replay(&base, &r).is_err());
    let r = ReplayParams {
        victim: A,
        src_start_ns: T0 + 99_000_000_000_000,
        src_end_ns: T0 + 99_000_000_000_001,
        shift_ns: 5,
        as_addr: A,
    };
    assert!(inject_replay(&base, &r).is_err());
    assert!(inject_duplicate(
        &base,
        &DuplicateParams {
            victim: A,
            donor: A
        }
    )
    .is_err());
    assert!(inject_duplicate(
        &base,
        &DuplicateParams {
            victim: A,
            donor: 0x777
        }
    )
    .is_err());
}

// REQ: OA-SIM-013
#[test]
fn drift_walks_the_victim_away_at_the_stated_rate() {
    let base = traffic();
    let d = DriftParams {
        victim: A,
        start_ns: T0 + 100_000_000_000,
        end_ns: T0 + 400_000_000_000,
        rate_mps: 2.0,
        bearing_deg: 45.0,
    };
    let out = inject_drift(&base, &d).unwrap();
    assert_eq!(reports_len(&out.reports), reports_len(&base.reports));
    let mut drifted = 0;
    for i in 0..reports_len(&out.reports) {
        let t = out.reports.time_ns[i];
        let in_window = out.reports.icao[i] == A && t > d.start_ns && t <= d.end_ns;
        assert_eq!(out.label[i] == LABEL_DRIFT, in_window);
        if in_window {
            drifted += 1;
            assert_eq!(out.victim[i], A);
            let off = geo_m(
                f64::from(base.reports.lat_e7[i]) / 1e7,
                f64::from(base.reports.lon_e7[i]) / 1e7,
                f64::from(out.reports.lat_e7[i]) / 1e7,
                f64::from(out.reports.lon_e7[i]) / 1e7,
            );
            let want = 2.0 * (t - d.start_ns) as f64 / 1e9;
            assert!((off - want).abs() < 0.05 + want * 1e-4, "{off} {want}");
            // Moved to the north-east: both coordinates increased.
            assert!(out.reports.lat_e7[i] > base.reports.lat_e7[i]);
            assert!(out.reports.lon_e7[i] > base.reports.lon_e7[i]);
        } else {
            assert_eq!(out.reports.lat_e7[i], base.reports.lat_e7[i]);
            assert_eq!(out.reports.lon_e7[i], base.reports.lon_e7[i]);
            assert_eq!(out.label[i], LABEL_NONE);
        }
    }
    assert_eq!(drifted, 300);
}

// REQ: OA-SIM-014
#[test]
fn replay_copies_the_past_track_later() {
    let base = traffic();
    let p = ReplayParams {
        victim: A,
        src_start_ns: T0 + 50_000_000_000,
        src_end_ns: T0 + 149_000_000_000,
        shift_ns: 300_000_000_000,
        as_addr: A,
    };
    let out = inject_replay(&base, &p).unwrap();
    assert_eq!(count(&out, LABEL_REPLAY), 100);
    assert!(sorted(&out));
    for i in 0..reports_len(&out.reports) {
        if out.label[i] != LABEL_REPLAY {
            continue;
        }
        assert_eq!(out.victim[i], A);
        assert_eq!(out.reports.icao[i], A);
        // The same position as the original row 300 s earlier.
        let t = out.reports.time_ns[i] - p.shift_ns;
        let j = (0..reports_len(&base.reports))
            .find(|&j| base.reports.icao[j] == A && base.reports.time_ns[j] == t)
            .unwrap();
        assert_eq!(out.reports.lat_e7[i], base.reports.lat_e7[j]);
        assert_eq!(out.reports.lon_e7[i], base.reports.lon_e7[j]);
    }
    // Under a new identity.
    let q = ReplayParams {
        as_addr: 0xabcdef,
        ..p
    };
    let out = inject_replay(&base, &q).unwrap();
    assert_eq!(
        out.reports.icao.iter().filter(|&&a| a == 0xabcdef).count(),
        100
    );
}

// REQ: OA-SIM-015
#[test]
fn duplicate_address_copies_the_donor_under_the_victim() {
    let base = traffic();
    let out = inject_duplicate(
        &base,
        &DuplicateParams {
            victim: A,
            donor: B,
        },
    )
    .unwrap();
    assert_eq!(count(&out, LABEL_DUPLICATE), 600);
    assert_eq!(reports_len(&out.reports), reports_len(&base.reports) + 600);
    assert!(sorted(&out));
    // The victim address now has two reports at every second; the donor's own rows are intact.
    let under_a = out.reports.icao.iter().filter(|&&a| a == A).count();
    assert_eq!(under_a, 1200);
    assert_eq!(out.reports.icao.iter().filter(|&&a| a == B).count(), 600);
    for i in 0..reports_len(&out.reports) {
        if out.label[i] == LABEL_DUPLICATE {
            assert_eq!(out.reports.icao[i], A);
            assert_eq!(out.victim[i], A);
            assert!(out.reports.lat_e7[i] > 415_000_000); // the donor flies at 42 deg, the victim at 41
        }
    }
}

// REQ: OA-SIM-017
#[test]
fn transforms_compose_and_score_as_a_confusion_matrix() {
    let base = traffic();
    let g = inject_ghost(&base, &ghost()).unwrap();
    let d = inject_drift(
        &g,
        &DriftParams {
            victim: A,
            start_ns: T0,
            end_ns: T0 + 200_000_000_000,
            rate_mps: 1.0,
            bearing_deg: 0.0,
        },
    )
    .unwrap();
    assert_eq!(count(&d, LABEL_GHOST), 120);
    assert_eq!(count(&d, LABEL_DRIFT), 200);
    // A detector that flags exactly the ghosts and a tenth of the clean rows.
    let flagged: Vec<bool> = d
        .label
        .iter()
        .enumerate()
        .map(|(i, &l)| l == LABEL_GHOST || (l == LABEL_NONE && i % 10 == 0))
        .collect();
    let c = confusion(&d.label, &flagged).unwrap();
    assert_eq!(c.tp, 120);
    assert_eq!(c.fneg, 200);
    assert_eq!(c.tp + c.fneg, 320);
    assert_eq!(c.tp + c.fneg + c.tn + c.fp, reports_len(&d.reports) as u64);
    assert_eq!(c.flagged[usize::from(LABEL_GHOST)], 120);
    assert_eq!(c.flagged[usize::from(LABEL_DRIFT)], 0);
    assert_eq!(c.rows[usize::from(LABEL_DRIFT)], 200);
    assert!(c.fp > 0 && c.tn > 0);
    assert!(confusion(&d.label, &flagged[1..]).is_err());
    assert!(confusion(&[9], &[true]).is_err());
}

// REQ: OA-SIM-021
#[test]
fn ghost_and_drift_positions_are_geographic() {
    let base = traffic();
    let bad = |g: &GhostParams| matches!(inject_ghost(&base, g), Err(SimError::BadSpoof(_)));
    // A latitude past 90 degrees is refused, including one e-7 count past it, both signs.
    for lat in [100.0, -100.0, 90.0000001, -90.0000001, 179.0] {
        let mut g = ghost();
        g.lat_deg = lat;
        g.speed_kt = 0.0;
        assert!(bad(&g), "{lat}");
    }
    let mut g = ghost();
    g.lon_deg = 180.0000001;
    g.speed_kt = 0.0;
    assert!(bad(&g));
    // Legitimate extremes still work: the pole and the antimeridian, standing still.
    for (lat, lon) in [
        (90.0, 0.0),
        (-90.0, 0.0),
        (0.0, 180.0),
        (0.0, -180.0),
        (89.9999999, 179.9999999),
    ] {
        let mut g = ghost();
        g.lat_deg = lat;
        g.lon_deg = lon;
        g.speed_kt = 0.0;
        let out = inject_ghost(&base, &g).unwrap();
        let i = (0..reports_len(&out.reports))
            .find(|&i| out.label[i] == LABEL_GHOST)
            .unwrap();
        assert_eq!(out.reports.lat_e7[i], (lat * 1e7).round() as i32);
    }
    // A track that runs over the pole part way stays on the geodesic and in range.
    let mut g = ghost();
    g.lat_deg = 89.0;
    g.speed_kt = 1500.0;
    g.track_deg = 0.0;
    g.interval_ms = 60_000;
    let out = inject_ghost(&base, &g).unwrap();
    assert!(out
        .reports
        .lat_e7
        .iter()
        .all(|&x| (-900_000_000..=900_000_000).contains(&x)));
    // Drift that carries the victim past a pole does the same.
    let d = DriftParams {
        victim: A,
        start_ns: T0 + 100_000_000_000,
        end_ns: T0 + 400_000_000_000,
        rate_mps: 1.0e6,
        bearing_deg: 0.0,
    };
    let out = inject_drift(&base, &d).unwrap();
    assert!(out
        .reports
        .lat_e7
        .iter()
        .all(|&x| (-900_000_000..=900_000_000).contains(&x)));
}

// REQ: OA-SIM-022
#[test]
fn ghost_duration_overflow_is_refused_up_front() {
    let base = traffic();
    let bad = |g: &GhostParams| matches!(inject_ghost(&base, g), Err(SimError::BadSpoof(_)));
    // Standing still, so nothing but the arithmetic can refuse it.
    let mut g = ghost();
    g.speed_kt = 0.0;
    g.count = 3;
    g.interval_ms = i64::MAX / 2 + 1;
    assert!(bad(&g));
    // The product fits in milliseconds but not in nanoseconds.
    g.interval_ms = i64::MAX / 1_000_000 + 1;
    g.count = 2;
    assert!(bad(&g));
    // The last timestamp overflows although the span does not.
    g = ghost();
    g.speed_kt = 0.0;
    g.start_ns = i64::MAX - 1_000_000_000;
    assert!(bad(&g));
    // Count one has no span: any interval is fine, one row.
    g = ghost();
    g.count = 1;
    g.interval_ms = i64::MAX;
    assert_eq!(count(&inject_ghost(&base, &g).unwrap(), LABEL_GHOST), 1);
}

// REQ: OA-SIM-028
#[test]
fn a_long_ghost_leg_follows_the_geodesic() {
    let base = traffic();
    let mut g = ghost();
    g.lat_deg = 45.0;
    g.lon_deg = -88.0;
    g.speed_kt = 500.0;
    g.track_deg = 90.0;
    g.interval_ms = 1_800_000;
    g.count = 2; // the second row is 250 NM along the leg
    let out = inject_ghost(&base, &g).unwrap();
    let i = (0..reports_len(&out.reports))
        .rev()
        .find(|&i| out.label[i] == LABEL_GHOST)
        .unwrap();
    let (la, lo) = (
        f64::from(out.reports.lat_e7[i]) / 1e7,
        f64::from(out.reports.lon_e7[i]) / 1e7,
    );
    let e = wgs84();
    let want = e
        .direct(LatLon::new(45.0, -88.0).unwrap(), 90.0, 250.0 * 1852.0)
        .unwrap()
        .position;
    let d = e
        .inverse(LatLon::new(la, lo).unwrap(), want)
        .unwrap()
        .distance_m;
    assert!(d < 1.0, "ghost end is {d} m from the geodesic");
}

// REQ: OA-SIM-028
#[test]
fn a_ghost_leg_near_the_pole_stays_finite() {
    let base = traffic();
    let mut g = ghost();
    g.lat_deg = 89.9;
    g.lon_deg = 0.0;
    g.speed_kt = 400.0;
    g.track_deg = 90.0;
    g.interval_ms = 540_000;
    g.count = 2; // about 100 km along the leg
    let out = inject_ghost(&base, &g).unwrap();
    let i = (0..reports_len(&out.reports))
        .rev()
        .find(|&i| out.label[i] == LABEL_GHOST)
        .unwrap();
    let (la, lo) = (
        f64::from(out.reports.lat_e7[i]) / 1e7,
        f64::from(out.reports.lon_e7[i]) / 1e7,
    );
    assert!(lo.abs() <= 180.0 && la <= 90.0, "{la} {lo}");
    let e = wgs84();
    let want = e
        .direct(
            LatLon::new(89.9, 0.0).unwrap(),
            90.0,
            400.0 * 1852.0 / 3600.0 * 540.0,
        )
        .unwrap()
        .position;
    let d = e
        .inverse(LatLon::new(la, lo).unwrap(), want)
        .unwrap()
        .distance_m;
    assert!(d < 1.0, "{d}");
}

// REQ: OA-SIM-028
#[test]
fn a_long_drift_follows_the_geodesic() {
    let base = traffic();
    let d = DriftParams {
        victim: A,
        start_ns: T0 + 100_000_000_000,
        end_ns: T0 + 400_000_000_000,
        rate_mps: 1500.0,
        bearing_deg: 60.0,
    };
    let out = inject_drift(&base, &d).unwrap();
    let e = wgs84();
    let mut worst = 0.0_f64;
    let mut n = 0;
    for i in 0..reports_len(&out.reports) {
        if out.label[i] != LABEL_DRIFT {
            continue;
        }
        let t = out.reports.time_ns[i];
        let p0 = LatLon::new(
            f64::from(base.reports.lat_e7[i]) / 1e7,
            f64::from(base.reports.lon_e7[i]) / 1e7,
        )
        .unwrap();
        let p1 = LatLon::new(
            f64::from(out.reports.lat_e7[i]) / 1e7,
            f64::from(out.reports.lon_e7[i]) / 1e7,
        )
        .unwrap();
        let want = e
            .direct(p0, 60.0, 1500.0 * (t - d.start_ns) as f64 * 1e-9)
            .unwrap()
            .position;
        worst = worst.max(e.inverse(p1, want).unwrap().distance_m);
        n += 1;
    }
    assert!(n > 0);
    assert!(worst < 1.0, "worst {worst} m");
}

// REQ: OA-SIM-032
#[test]
fn a_drift_window_across_the_whole_time_range_does_not_overflow() {
    let base = traffic();
    let d = DriftParams {
        victim: A,
        start_ns: i64::MIN,
        end_ns: i64::MAX,
        rate_mps: 0.0,
        bearing_deg: 45.0,
    };
    // Either an error or a result, but no panic on the subtraction of the window.
    let _ = inject_drift(&base, &d);
}
